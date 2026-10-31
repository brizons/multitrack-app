//! S2: real-time transport and live changes.
//!
//! Most expectations are offline renders of a hand-built plan: the sections
//! the live player should have played, end to end, with the seam fades and
//! clicks `compile` gives such a plan. Every command scenario also runs at
//! buffer sizes 1, 64 and 1024 and must give identical output.

mod common;

use std::f64::consts::FRAC_PI_2;

use common::{SongSpec, bits, mono, noise, onsets, sine, stereo};
use domain::{
    BeatGrid, ClickEvent, CompileOptions, DEFAULT_SEAM_CROSSFADE, RenderPlan, SectionType, Segment,
    Song, StemRole, compile,
};
use engine::live::{
    Command, DECLICK_FRAMES, FadeState, JumpKind, JumpTiming, LiveError, Player, QueuedJump,
    SendError, Status, StemSource, Transport, memory_sources,
};
use engine::{ClickSettings, MixSettings, RenderError, StemAudio, render_offline};

/// 120 bpm in 4/4 at 8 kHz: a beat is 4 000 frames, a bar 16 000.
const RATE: u32 = 8_000;
const BAR: u64 = 16_000;
const LENGTH: u64 = 112_000;
const INTRO: (u64, u64) = (0, 2 * BAR);
const VERSE: (u64, u64) = (2 * BAR, 2 * BAR);
const CHORUS: (u64, u64) = (4 * BAR, 2 * BAR);
const OUTRO: (u64, u64) = (6 * BAR, BAR);
/// Arrangement positions of the Default arrangement.
const POS_VERSE: usize = 1;
const POS_CHORUS: usize = 2;
const POS_OUTRO: usize = 3;

const SIZES: [usize; 3] = [1, 64, 1024];
/// Status snapshots are taken every this many callback frames.
const SNAPSHOT: u64 = 1_024;

fn spec() -> SongSpec {
    SongSpec::new(
        RATE,
        LENGTH,
        vec![
            ("intro", SectionType::Intro, INTRO.0),
            ("verse", SectionType::Verse, VERSE.0),
            ("chorus", SectionType::Chorus, CHORUS.0),
            ("outro", SectionType::Outro, OUTRO.0),
        ],
        vec![
            ("drums", StemRole::Track),
            ("keys", StemRole::Track),
            ("guide", StemRole::Guide),
            ("click", StemRole::Click),
        ],
    )
}

struct Setup {
    song: Song,
    arrangement: &'static str,
    stems: Vec<StemAudio>,
    settings: MixSettings,
}

fn all_levels() -> MixSettings {
    let mut settings = MixSettings {
        master_gain: 0.7,
        click: ClickSettings {
            enabled: true,
            levels: [1.0, 0.8, 0.5, 0.25],
        },
        ..MixSettings::default()
    };
    settings.stem_gains.insert("drums".to_owned(), 0.5);
    settings
}

fn no_click() -> MixSettings {
    let mut settings = all_levels();
    settings.click.enabled = false;
    settings
}

/// Two track stems, a guide stem, and the generated click at every level.
fn full() -> Setup {
    Setup {
        song: spec().build(),
        arrangement: "default",
        stems: vec![
            mono("drums", StemRole::Track, noise(1, LENGTH as usize)),
            stereo("keys", StemRole::Track, noise(2, 2 * LENGTH as usize)),
            mono("guide", StemRole::Guide, noise(3, LENGTH as usize)),
        ],
        settings: all_levels(),
    }
}

/// `setup` with every track stem silent.
fn without_tracks(mut setup: Setup) -> Setup {
    for stem in &mut setup.stems {
        if stem.role == StemRole::Track {
            stem.samples.iter_mut().for_each(|sample| *sample = 0.0);
        }
    }
    setup
}

/// One constant track stem at 0.5 and no click: the output is the gain curve.
fn dc() -> Setup {
    Setup {
        song: spec().build(),
        arrangement: "default",
        stems: vec![mono("drums", StemRole::Track, vec![0.5; LENGTH as usize])],
        settings: MixSettings {
            click: ClickSettings {
                enabled: false,
                ..ClickSettings::default()
            },
            ..MixSettings::default()
        },
    }
}

struct Run {
    output: Vec<f32>,
    /// Status after every `SNAPSHOT` callback frames.
    statuses: Vec<Status>,
    /// Status after the last buffer.
    last: Status,
}

impl Run {
    /// Status after the process call that ends at callback frame `at`.
    fn status(&self, at: u64) -> Status {
        assert!(at > 0 && at.is_multiple_of(SNAPSHOT));
        self.statuses[(at / SNAPSHOT - 1) as usize]
    }

    fn left(&self, frame: u64) -> f32 {
        self.output[2 * frame as usize]
    }

    fn frames(&self, range: std::ops::Range<u64>) -> &[f32] {
        &self.output[2 * range.start as usize..2 * range.end as usize]
    }
}

fn player(setup: &Setup, max_frames: usize) -> (Player, engine::live::Controller) {
    let sources = memory_sources(setup.stems.clone()).expect("stems are well formed");
    Player::new(
        &setup.song,
        setup.arrangement,
        &setup.settings,
        setup.song.sample_rate(),
        max_frames,
        sources,
    )
    .expect("player builds")
}

/// Plays `frames` callback frames in buffers of `buffer` frames; before the
/// buffer that starts at callback frame `at`, sends every scripted command
/// for `at`.
fn run(setup: &Setup, buffer: usize, frames: u64, script: &[(u64, Command)]) -> Run {
    let (mut player, mut controller) = player(setup, buffer);
    let mut output = vec![0.0f32; 2 * frames as usize];
    let mut statuses = Vec::new();
    let mut done = 0u64;
    while done < frames {
        for &(_, command) in script.iter().filter(|(at, _)| *at == done) {
            controller.send(command).expect("queue has room");
        }
        let n = (buffer as u64).min(frames - done);
        player.process(&mut output[2 * done as usize..2 * (done + n) as usize]);
        done += n;
        if done.is_multiple_of(SNAPSHOT) {
            statuses.push(player.status());
        }
    }
    Run {
        output,
        statuses,
        last: player.status(),
    }
}

/// Runs the script at every size in `SIZES` and checks the outputs and
/// statuses are identical.
fn run_sizes(setup: &Setup, frames: u64, script: &[(u64, Command)]) -> Run {
    let first = run(setup, SIZES[0], frames, script);
    for &size in &SIZES[1..] {
        let other = run(setup, size, frames, script);
        assert_eq!(
            bits(&other.output),
            bits(&first.output),
            "buffer size {size} changes the output"
        );
        assert_eq!(
            other.statuses, first.statuses,
            "buffer size {size} changes the status"
        );
    }
    first
}

fn played(setup: &Setup, frames: u64) -> Run {
    run_sizes(setup, frames, &[(0, Command::Play)])
}

/// A plan that plays the source ranges `pieces` end to end, with the seam
/// fades and the clicks `compile` gives such a plan.
fn timeline(song: &Song, pieces: &[(u64, u64)]) -> RenderPlan {
    let grid = BeatGrid::new(song);
    let mut segments: Vec<Segment> = Vec::new();
    let mut out_start = 0;
    for &(src_start, len) in pieces {
        let mut fade_in = 0;
        if let Some(previous) = segments.last_mut() {
            let end = previous.src_start + previous.len;
            if end != src_start {
                fade_in = DEFAULT_SEAM_CROSSFADE
                    .min(len)
                    .min(song.length().saturating_sub(end));
                previous.fade_out = fade_in;
            }
        }
        segments.push(Segment {
            section_id: String::new(),
            out_start,
            src_start,
            len,
            fade_in,
            fade_out: 0,
        });
        out_start += len;
    }
    let clicks = segments
        .iter()
        .flat_map(|segment| {
            grid.clicks_in(segment.src_start..segment.src_start + segment.len)
                .iter()
                .map(move |click| ClickEvent {
                    at: segment.out_start + (click.at - segment.src_start),
                    level: click.level,
                })
        })
        .collect();
    RenderPlan {
        sample_rate: song.sample_rate(),
        length: out_start,
        segments,
        clicks,
        cues: Vec::new(),
    }
}

fn offline(setup: &Setup, plan: &RenderPlan) -> Vec<f32> {
    render_offline(plan, &setup.stems, &setup.settings).expect("plan renders")
}

/// The run played exactly `expected`, then stopped into silence.
fn assert_plays(run: &Run, expected: &[f32]) {
    let len = expected.len();
    assert!(
        run.output.len() > len,
        "the run outlasts the expected audio"
    );
    assert_eq!(bits(&run.output[..len]), bits(expected), "played audio");
    assert!(
        run.output[len..].iter().all(|&sample| sample == 0.0),
        "silence after the end"
    );
}

fn round_up(frames: u64) -> u64 {
    frames.div_ceil(SNAPSHOT) * SNAPSHOT + SNAPSHOT
}

#[test]
fn without_live_commands_the_player_reproduces_the_offline_render_bit_exactly() {
    // Tempo and meter changes, jump seams, and the source jumping back from
    // the outro to the intro.
    let spec = spec()
        .tempo(&[(0, 120.0), (CHORUS.0, 150.0)])
        .meters(&[(0, 4, 4), (OUTRO.0, 3, 4)])
        .arrangement(
            "live",
            &[
                "intro", "verse", "chorus", "chorus", "verse", "outro", "intro",
            ],
        );
    let generated = Setup {
        song: spec.build(),
        arrangement: "live",
        ..full()
    };
    let recorded = Setup {
        song: spec.build(),
        arrangement: "live",
        stems: vec![
            stereo("keys", StemRole::Track, noise(4, 2 * LENGTH as usize)),
            mono("click", StemRole::Click, noise(5, LENGTH as usize)),
            mono("guide", StemRole::Guide, noise(6, LENGTH as usize)),
        ],
        settings: all_levels_without_drums(),
    };

    for setup in [&generated, &recorded] {
        let plan = compile(&setup.song, "live", &CompileOptions::default()).unwrap();
        assert!(plan.segments.iter().any(|segment| segment.fade_in > 0));
        let expected = offline(setup, &plan);
        let frames = round_up(plan.length);
        for buffer in [1, 7, 64, 256, 1024] {
            let run = run(setup, buffer, frames, &[(0, Command::Play)]);
            assert_plays(&run, &expected);
            let status = run.last;
            assert_eq!(status.transport, Transport::Stopped, "the end stops");
            assert_eq!((status.position, status.output_frame), (0, 0));
        }
    }
}

fn all_levels_without_drums() -> MixSettings {
    let mut settings = all_levels();
    settings.stem_gains.clear();
    settings.stem_gains.insert("keys".to_owned(), 0.5);
    settings
}

#[test]
fn a_queued_jump_executes_at_the_end_of_the_current_section() {
    let setup = full();
    let run = run_sizes(
        &setup,
        round_up(INTRO.1 + OUTRO.1),
        &[
            (0, Command::Play),
            (
                1_024,
                Command::QueueJump {
                    position: POS_OUTRO,
                },
            ),
        ],
    );

    assert_plays(
        &run,
        &offline(&setup, &timeline(&setup.song, &[INTRO, OUTRO])),
    );
    let queued = Some(QueuedJump {
        target: POS_OUTRO,
        kind: JumpKind::Jump,
        executes_at: INTRO.1,
    });
    assert_eq!(run.status(2_048).queued, queued);
    assert_eq!(run.status(31_744).queued, queued, "shown until it executes");
    let after = run.status(32_768);
    assert_eq!(after.queued, None);
    assert_eq!(after.position, POS_OUTRO);
    assert_eq!(after.source_frame, OUTRO.0 + 768);
}

#[test]
fn a_queued_jump_at_next_bar_executes_on_the_next_bar_line_with_an_equal_power_seam() {
    let setup = full();
    let script = [
        (0, Command::Play),
        (1_024, Command::SetJumpTiming(JumpTiming::NextBar)),
        (
            1_024,
            Command::QueueJump {
                position: POS_CHORUS,
            },
        ),
    ];
    let run = run_sizes(&setup, round_up(BAR + CHORUS.1 + OUTRO.1), &script);

    let cut = (INTRO.0, BAR);
    assert_plays(
        &run,
        &offline(&setup, &timeline(&setup.song, &[cut, CHORUS, OUTRO])),
    );
    let status = run.status(2_048);
    assert_eq!(status.jump_timing, JumpTiming::NextBar);
    assert_eq!(status.queued.map(|queued| queued.executes_at), Some(BAR));
    assert_eq!(run.status(16_384).position, POS_CHORUS);

    // The seam itself, on a plain tone with no click.
    let tone = sine(441.0, RATE, LENGTH as usize);
    let tone_setup = Setup {
        stems: vec![mono("drums", StemRole::Track, tone.clone())],
        settings: MixSettings {
            click: ClickSettings {
                enabled: false,
                ..ClickSettings::default()
            },
            ..MixSettings::default()
        },
        ..full()
    };
    let run = run_sizes(&tone_setup, 2 * BAR, &script);
    let n = DEFAULT_SEAM_CROSSFADE;
    assert_eq!(
        run.left(BAR - 1).to_bits(),
        tone[BAR as usize - 1].to_bits()
    );
    for i in 0..n {
        let t = (i as f64 + 0.5) / n as f64;
        let expected = f64::from(tone[(BAR + i) as usize]) * (FRAC_PI_2 * t).cos()
            + f64::from(tone[(CHORUS.0 + i) as usize]) * (FRAC_PI_2 * t).sin();
        let actual = f64::from(run.left(BAR + i));
        assert!((actual - expected).abs() < 1e-6, "seam frame {i}");
    }
    assert_eq!(
        run.left(BAR + n).to_bits(),
        tone[(CHORUS.0 + n) as usize].to_bits()
    );
}

#[test]
fn a_cancelled_jump_never_executes() {
    let setup = full();
    let run = run_sizes(
        &setup,
        round_up(LENGTH),
        &[
            (0, Command::Play),
            (
                1_024,
                Command::QueueJump {
                    position: POS_OUTRO,
                },
            ),
            (2_048, Command::CancelJump),
        ],
    );

    assert!(run.status(2_048).queued.is_some());
    assert_eq!(run.status(3_072).queued, None);
    let plan = compile(&setup.song, "default", &CompileOptions::default()).unwrap();
    assert_plays(&run, &offline(&setup, &plan));
}

#[test]
fn a_live_loop_repeats_the_current_section_once() {
    let setup = full();
    let run = run_sizes(
        &setup,
        round_up(LENGTH + INTRO.1),
        &[(0, Command::Play), (1_024, Command::LiveLoop)],
    );

    assert_eq!(
        run.status(2_048).queued,
        Some(QueuedJump {
            target: 0,
            kind: JumpKind::Loop,
            executes_at: INTRO.1,
        })
    );
    assert_plays(
        &run,
        &offline(
            &setup,
            &timeline(&setup.song, &[INTRO, INTRO, VERSE, CHORUS, OUTRO]),
        ),
    );
}

#[test]
fn an_infinite_loop_repeats_until_switched_off() {
    let setup = full();
    let run = run_sizes(
        &setup,
        round_up(3 * INTRO.1 + VERSE.1 + CHORUS.1 + OUTRO.1),
        &[
            (0, Command::Play),
            (1_024, Command::SetInfinite(true)),
            (70_656, Command::SetInfinite(false)),
        ],
    );

    assert!(run.status(2_048).infinite);
    assert_eq!(run.status(65_536).position, 0, "third pass of the intro");
    assert!(!run.status(71_680).infinite);
    assert_plays(
        &run,
        &offline(
            &setup,
            &timeline(&setup.song, &[INTRO, INTRO, INTRO, VERSE, CHORUS, OUTRO]),
        ),
    );
}

#[test]
fn a_queued_jump_overrides_the_infinite_loop_and_switches_it_off() {
    let setup = full();
    let run = run_sizes(
        &setup,
        round_up(2 * INTRO.1 + CHORUS.1 + OUTRO.1),
        &[
            (0, Command::Play),
            (1_024, Command::SetInfinite(true)),
            (
                33_792,
                Command::QueueJump {
                    position: POS_CHORUS,
                },
            ),
        ],
    );

    assert_eq!(
        run.status(34_816).queued.map(|queued| queued.executes_at),
        Some(2 * INTRO.1)
    );
    let after = run.status(65_536);
    assert_eq!(after.position, POS_CHORUS);
    assert!(!after.infinite);
    assert_plays(
        &run,
        &offline(
            &setup,
            &timeline(&setup.song, &[INTRO, INTRO, CHORUS, OUTRO]),
        ),
    );
}

const FADE: u64 = 4_096;

fn fade_script() -> Vec<(u64, Command)> {
    vec![
        (0, Command::Play),
        (0, Command::SetFadeDuration { frames: FADE }),
        (8_192, Command::ToggleFade),
        (20_480, Command::ToggleFade),
    ]
}

#[test]
fn fade_ramps_the_track_stems_out_and_back_in_over_the_configured_time() {
    let frames = 3 * BAR;
    let curve = run_sizes(&dc(), frames, &fade_script());
    for i in 0..8_192 {
        assert_eq!(curve.left(i), 0.5);
    }
    let mut previous = 0.5;
    for i in 0..FADE {
        let level = curve.left(8_192 + i);
        assert!(level < previous, "fade-out frame {i} falls");
        previous = level;
    }
    assert_eq!(
        curve.left(8_192 + FADE - 1),
        0.0,
        "silent after the duration"
    );
    assert!((curve.left(8_192 + FADE / 2 - 1) - 0.25).abs() < 1e-3);
    assert!(curve.frames(8_192 + FADE..20_480).iter().all(|&s| s == 0.0));
    for i in 0..FADE {
        let level = curve.left(20_480 + i);
        assert!(level > previous, "fade-in frame {i} rises");
        previous = level;
    }
    assert_eq!(curve.left(20_480 + FADE - 1), 0.5, "back to full level");
    assert!(
        curve
            .frames(20_480 + FADE..frames)
            .iter()
            .all(|&s| s == 0.5)
    );

    let status = curve.status(9_216);
    assert_eq!(status.fade.state, FadeState::FadingOut);
    assert_eq!(curve.status(13_312).fade.state, FadeState::Out);
    assert_eq!(curve.status(13_312).fade.level, 0.0);
    assert_eq!(curve.status(21_504).fade.state, FadeState::FadingIn);
    assert_eq!(curve.status(25_600).fade.state, FadeState::In);
    assert_eq!(curve.status(25_600).fade.level, 1.0);
}

#[test]
fn fade_leaves_the_guide_stem_and_the_generated_click_playing() {
    let frames = 3 * BAR;
    let faded = run_sizes(&full(), frames, &fade_script());
    let reference = played(&full(), frames);
    let others = played(&without_tracks(full()), frames);

    assert_eq!(
        bits(faded.frames(0..8_192)),
        bits(reference.frames(0..8_192))
    );
    assert_eq!(
        bits(faded.frames(8_192 + FADE..20_480)),
        bits(others.frames(8_192 + FADE..20_480)),
        "only the guide stem and the click remain"
    );
    assert_eq!(
        bits(faded.frames(20_480 + FADE..frames)),
        bits(reference.frames(20_480 + FADE..frames)),
        "the tracks are back at full level"
    );
    let others_faded = run_sizes(&without_tracks(full()), frames, &fade_script());
    assert_eq!(
        bits(&others_faded.output),
        bits(&others.output),
        "guide and click never fade"
    );
}

#[test]
fn fade_leaves_a_recorded_click_stem_playing() {
    let frames = 3 * BAR;
    let setup = || Setup {
        stems: vec![
            mono("drums", StemRole::Track, vec![0.0; LENGTH as usize]),
            mono("click", StemRole::Click, noise(7, LENGTH as usize)),
        ],
        ..full()
    };
    let faded = run_sizes(&setup(), frames, &fade_script());
    let reference = played(&setup(), frames);
    assert_eq!(bits(&faded.output), bits(&reference.output));
}

#[test]
fn panic_fades_everything_to_silence_within_half_a_second_and_stops() {
    let half_second = u64::from(RATE) / 2;
    let script = [(0, Command::Play), (8_192, Command::Panic)];
    let frames = 8_192 + 2 * SNAPSHOT + half_second;

    let curve = run_sizes(&dc(), frames, &script);
    assert_eq!(curve.left(8_191), 0.5);
    let mut previous = 0.5;
    for i in 0..half_second {
        let level = curve.left(8_192 + i);
        assert!(level < previous, "panic frame {i} falls");
        previous = level;
    }
    assert_eq!(curve.left(8_192 + half_second - 1), 0.0);

    let full_run = run_sizes(&full(), frames, &script);
    let reference = played(&full(), 8_192);
    assert_eq!(
        bits(full_run.frames(0..8_192)),
        bits(reference.frames(0..8_192))
    );
    assert!(
        full_run
            .frames(8_192 + half_second..frames)
            .iter()
            .all(|&sample| sample == 0.0),
        "the click and the guide fade too"
    );
    let stopped = full_run.status(frames - frames % SNAPSHOT);
    assert_eq!(stopped.transport, Transport::Stopped);
    assert_eq!((stopped.position, stopped.output_frame), (0, 0));
    assert_eq!(full_run.status(9_216).transport, Transport::Panicking);
}

#[test]
fn pause_ramps_out_and_play_resumes_where_it_paused() {
    let script = [
        (0, Command::Play),
        (8_192, Command::Pause),
        (16_384, Command::Play),
    ];
    let frames = 2 * BAR;
    let declick = DECLICK_FRAMES;

    let curve = run_sizes(&dc(), frames, &script);
    for i in 0..declick {
        assert!(curve.left(8_192 + i) < 0.5, "pause ramp frame {i}");
    }
    assert_eq!(curve.left(8_192 + declick - 1), 0.0);
    assert!(
        curve
            .frames(8_192 + declick..16_384)
            .iter()
            .all(|&s| s == 0.0)
    );
    for i in 0..declick - 1 {
        assert!(curve.left(16_384 + i) < 0.5, "resume ramp frame {i}");
    }
    assert_eq!(curve.left(16_384 + declick - 1), 0.5);

    let paused = curve.status(9_216);
    assert_eq!(paused.transport, Transport::Paused);
    assert_eq!(paused.output_frame, 8_192 + declick);
    assert_eq!(curve.status(17_408).transport, Transport::Playing);

    let full_run = run_sizes(&full(), frames, &script);
    let reference = played(&full(), frames);
    let resumed = 16_384 + declick;
    let timeline_frame = 8_192 + 2 * declick;
    let tail = frames - resumed;
    assert_eq!(
        bits(full_run.frames(resumed..frames)),
        bits(reference.frames(timeline_frame..timeline_frame + tail)),
        "after the ramp it plays on from where it paused"
    );
}

#[test]
fn stop_returns_to_the_start_and_play_starts_the_song_again() {
    let setup = full();
    let run = run_sizes(
        &setup,
        16_384 + BAR,
        &[
            (0, Command::Play),
            (8_192, Command::Stop),
            (16_384, Command::Play),
        ],
    );
    let reference = played(&setup, BAR);

    assert!(
        run.frames(8_192 + DECLICK_FRAMES..16_384)
            .iter()
            .all(|&s| s == 0.0)
    );
    let stopped = run.status(9_216);
    assert_eq!(stopped.transport, Transport::Stopped);
    assert_eq!((stopped.position, stopped.output_frame), (0, 0));
    assert_eq!(stopped.source_frame, 0);
    assert_eq!(
        bits(run.frames(16_384..16_384 + BAR)),
        bits(reference.frames(0..BAR)),
        "the song starts again from its first frame"
    );
}

#[test]
fn back_to_start_crossfades_into_the_first_section() {
    let setup = full();
    let cut = (VERSE.0, 40_960 - INTRO.1);
    let run = run_sizes(
        &setup,
        round_up(40_960 + LENGTH),
        &[(0, Command::Play), (40_960, Command::BackToStart)],
    );
    assert_plays(
        &run,
        &offline(
            &setup,
            &timeline(&setup.song, &[INTRO, cut, INTRO, VERSE, CHORUS, OUTRO]),
        ),
    );
}

#[test]
fn click_off_and_on_ramps_the_generated_click() {
    let frames = 3 * BAR;
    let script = [
        (0, Command::Play),
        (8_192, Command::SetClick(false)),
        (20_480, Command::SetClick(true)),
    ];
    let run = run_sizes(&full(), frames, &script);
    let reference = played(&full(), frames);
    let silent_click = played(
        &Setup {
            settings: no_click(),
            ..full()
        },
        frames,
    );

    assert_eq!(bits(run.frames(0..8_192)), bits(reference.frames(0..8_192)));
    let off = 8_192 + DECLICK_FRAMES..20_480;
    assert_eq!(
        bits(run.frames(off.clone())),
        bits(silent_click.frames(off))
    );
    let on = 20_480 + DECLICK_FRAMES..frames;
    assert_eq!(bits(run.frames(on.clone())), bits(reference.frames(on)));
    assert!(!run.status(9_216).click);
    assert!(run.status(21_504).click);
}

#[test]
fn generated_clicks_after_a_jump_land_on_the_bar_grid_of_the_played_section() {
    // The second section starts mid-bar and changes tempo on a bar line.
    let length = 120_000;
    let song = SongSpec::new(
        RATE,
        length,
        vec![
            ("a", SectionType::Intro, 0),
            ("b", SectionType::Verse, 40_000),
        ],
        vec![("silence", StemRole::Track)],
    )
    .tempo(&[(0, 120.0), (48_000, 90.0)])
    .build();
    let grid = BeatGrid::new(&song);
    let setup = Setup {
        song,
        arrangement: "default",
        stems: vec![mono("silence", StemRole::Track, vec![0.0; length as usize])],
        settings: MixSettings {
            click: ClickSettings {
                enabled: true,
                levels: [1.0, 0.0, 0.0, 0.0],
            },
            ..MixSettings::default()
        },
    };
    let b = (40_000u64, 80_000u64);
    let script = [
        (0, Command::Play),
        (1_024, Command::SetJumpTiming(JumpTiming::NextBar)),
        (1_024, Command::QueueJump { position: 1 }),
        (17_408, Command::LiveLoop),
    ];
    let run = run_sizes(&setup, round_up(BAR + 2 * b.1), &script);

    let bars_of_b: Vec<u64> = (b.0..b.0 + b.1)
        .filter(|&at| grid.next_bar_line(at) == Some(at))
        .collect();
    assert_eq!(bars_of_b[0], 48_000);
    assert_eq!(bars_of_b.len(), 4, "48 000 then three bars at 90 bpm");
    let mut expected = vec![0];
    for pass_start in [BAR, BAR + b.1] {
        expected.extend(bars_of_b.iter().map(|bar| pass_start + (bar - b.0)));
    }
    assert_eq!(onsets(&run.output), expected);
}

#[test]
fn a_source_without_data_plays_silence_and_counts_underruns() {
    struct Missing;
    impl StemSource for Missing {
        fn id(&self) -> &str {
            "keys"
        }
        fn role(&self) -> StemRole {
            StemRole::Track
        }
        fn read(&mut self, _at: u64, out: &mut [f32]) -> bool {
            out.fill(1.0);
            false
        }
    }
    let setup = full();
    let mut sources = memory_sources(setup.stems.clone()).unwrap();
    sources.retain(|source| source.id() != "keys");
    sources.insert(1, Box::new(Missing));
    let (mut player, mut controller) =
        Player::new(&setup.song, "default", &setup.settings, RATE, 256, sources).unwrap();
    controller.send(Command::Play).unwrap();
    let mut output = vec![0.0; 2 * 4_096];
    for chunk in output.chunks_mut(2 * 256) {
        player.process(chunk);
    }

    let without = Setup {
        stems: setup
            .stems
            .iter()
            .filter(|stem| stem.id != "keys")
            .cloned()
            .collect(),
        ..full()
    };
    let reference = played(&without, 4_096);
    assert_eq!(bits(&output), bits(&reference.output));
    assert_eq!(player.status().underruns, 16, "one per buffer");
}

#[test]
fn construction_rejects_a_bad_arrangement_rate_buffer_or_gain() {
    let setup = full();
    let build = |arrangement: &str, rate: u32, frames: usize, settings: &MixSettings| {
        Player::new(
            &setup.song,
            arrangement,
            settings,
            rate,
            frames,
            memory_sources(setup.stems.clone()).unwrap(),
        )
        .map(|_| ())
    };
    assert_eq!(
        build("nope", RATE, 64, &setup.settings),
        Err(LiveError::UnknownArrangement { id: "nope".into() })
    );
    assert_eq!(
        build("default", 48_000, 64, &setup.settings),
        Err(LiveError::SampleRateMismatch {
            song: RATE,
            output: 48_000
        })
    );
    assert_eq!(
        build("default", RATE, 0, &setup.settings),
        Err(LiveError::ZeroBufferSize)
    );
    let mut settings = setup.settings.clone();
    settings.stem_gains.insert("vocals".to_owned(), 1.0);
    assert_eq!(
        build("default", RATE, 64, &settings),
        Err(LiveError::UnknownStemGain {
            id: "vocals".into()
        })
    );

    let mut malformed = mono("drums", StemRole::Track, vec![0.0; 10]);
    malformed.frames = 11;
    assert!(matches!(
        memory_sources(vec![malformed]),
        Err(RenderError::SampleCountMismatch { .. })
    ));
}

#[test]
fn the_controller_rejects_positions_outside_the_arrangement() {
    let (_player, mut controller) = player(&full(), 64);
    assert_eq!(controller.positions(), 4);
    assert_eq!(
        controller.send(Command::QueueJump { position: 4 }),
        Err(SendError::UnknownPosition {
            position: 4,
            positions: 4
        })
    );
    assert_eq!(
        controller.send(Command::QueueJump {
            position: POS_VERSE
        }),
        Ok(())
    );
}

#[test]
fn a_new_player_is_stopped_at_the_start() {
    let (player, _controller) = player(&full(), 64);
    let status = player.status();
    assert_eq!(status.transport, Transport::Stopped);
    assert_eq!(
        (status.output_frame, status.position, status.source_frame),
        (0, 0, 0)
    );
    assert_eq!((status.bar_beat.bar, status.bar_beat.beat), (1, 1));
    assert_eq!(status.queued, None);
    assert_eq!(status.jump_timing, JumpTiming::SectionEnd);
    assert!(!status.infinite);
    assert_eq!(status.fade.state, FadeState::In);
    assert!(status.click);
    assert_eq!(status.underruns, 0);
}

#[test]
fn a_section_cut_during_its_fade_in_fades_out_from_where_it_was() {
    // `b` is entered by a jump (a 256-frame crossfade) and cut 200 frames
    // later by a next-bar jump, while its own fade-in is still running.
    let length = 48_000;
    let setup = Setup {
        song: SongSpec::new(
            RATE,
            length,
            vec![
                ("x", SectionType::Intro, 0),
                ("y", SectionType::Verse, 8_000),
                ("b", SectionType::Chorus, BAR - 200),
                ("c", SectionType::Outro, 2 * BAR),
            ],
            vec![("drums", StemRole::Track)],
        )
        .build(),
        ..dc()
    };
    let script = [
        (0, Command::Play),
        (1_024, Command::QueueJump { position: 2 }),
        (8_192, Command::SetJumpTiming(JumpTiming::NextBar)),
        (8_192, Command::QueueJump { position: 3 }),
    ];
    let run = run_sizes(&setup, 9_216, &script);
    assert_eq!(run.status(9_216).position, 3);

    let n = DEFAULT_SEAM_CROSSFADE;
    let phase = |i: u64| FRAC_PI_2 * (i as f64 + 0.5) / n as f64;
    let (fade_in, fade_out) = (|i: u64| phase(i).sin(), |i: u64| phase(i).cos());
    for frame in 8_000..8_600u64 {
        let i = frame - 8_000;
        let gains = if i < 200 {
            fade_out(i) + fade_in(i)
        } else {
            let j = i - 200;
            let first_tail = if i < n { fade_out(i) } else { 0.0 };
            let cut_level = if i < n { fade_in(i) } else { 1.0 };
            let second_tail = if j < n { cut_level * fade_out(j) } else { 0.0 };
            first_tail + second_tail + if j < n { fade_in(j) } else { 1.0 }
        };
        let expected = 0.5 * gains;
        let actual = f64::from(run.left(frame));
        assert!(
            (actual - expected).abs() < 1e-6,
            "frame {frame}: {actual} != {expected}"
        );
    }
}

#[test]
fn the_player_and_the_controller_can_move_to_their_own_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Player>();
    assert_send::<engine::live::Controller>();
    assert_send::<Status>();
}
