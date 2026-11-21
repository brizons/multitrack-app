//! S3: stems stream from disk through one disk thread per song, with bounded
//! buffers ahead of the playhead and at the jump targets. S2: a jump whose
//! target is not buffered in time moves to the next boundary.
//!
//! Unless a test says otherwise, the disk "keeps up": before every buffer the
//! test waits until the disk thread has served every request it may serve.

mod common;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{write_float, write_int, write_manifest};
use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use engine::live::{Command, Controller, JumpTiming, Player, PlayerConfig, Status, memory_sources};
use engine::{ClickSettings, MixSettings};
use library::{DiskMonitor, LoadError, StreamConfig, load_song, open_streaming};

/// 120 bpm in 4/4 at 8 kHz: a bar is 16 000 frames.
const RATE: u32 = 8_000;
const BAR: u64 = 16_000;
const LENGTH: u64 = 112_000;
/// Positions of the Default arrangement.
const POS_VERSE: usize = 1;
const POS_OUTRO: usize = 3;
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);

fn package(rate: u32, length: u64) -> SongPackage {
    let section = |id: &str, section_type, start| SectionMarker {
        id: id.to_owned(),
        section_type,
        number: None,
        start,
    };
    let arrangement = |id: &str, sections: &[&str]| Arrangement {
        id: id.to_owned(),
        name: id.to_owned(),
        sections: sections.iter().map(|s| (*s).to_owned()).collect(),
        infinite_loop: Vec::new(),
    };
    let stem = |id: &str, role| Stem {
        id: id.to_owned(),
        file: format!("stems/{id}.wav"),
        part: id.to_owned(),
        role,
    };
    let quarter = length / 7;
    SongPackage {
        format_version: 1,
        title: "Stream Test".to_owned(),
        artist: None,
        sample_rate: rate,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![
            stem("drums", StemRole::Track),
            stem("keys", StemRole::Track),
            stem("guide", StemRole::Guide),
        ],
        sections: vec![
            section("intro", SectionType::Intro, 0),
            section("verse", SectionType::Verse, 2 * quarter),
            section("chorus", SectionType::Chorus, 4 * quarter),
            section("outro", SectionType::Outro, 6 * quarter),
        ],
        arrangements: vec![
            arrangement("default", &["intro", "verse", "chorus", "outro"]),
            arrangement("repeat", &["intro", "chorus", "chorus", "verse", "outro"]),
        ],
    }
}

/// Deterministic noise as 16-bit integers.
fn noise_i16(seed: u64, count: usize) -> Vec<i32> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 48) as i16) as i32
        })
        .collect()
}

fn noise_f32(seed: u64, count: usize) -> Vec<f32> {
    noise_i16(seed, count)
        .into_iter()
        .map(|sample| sample as f32 / 65_536.0)
        .collect()
}

/// A song folder with a mono 16-bit track, a stereo float track 1.5 s
/// shorter than the song, and a mono 24-bit guide.
fn write_song(dir: &Path, rate: u32, length: u64) {
    write_manifest(dir, &package(rate, length));
    let frames = length as usize;
    write_int(
        &dir.join("stems/drums.wav"),
        16,
        1,
        rate,
        &noise_i16(1, frames),
    );
    let keys = frames - (rate as usize * 3 / 2);
    write_float(
        &dir.join("stems/keys.wav"),
        2,
        rate,
        &noise_f32(2, 2 * keys),
    );
    let guide: Vec<i32> = noise_i16(3, frames).iter().map(|s| s * 256).collect();
    write_int(&dir.join("stems/guide.wav"), 24, 1, rate, &guide);
}

fn settings() -> MixSettings {
    let mut settings = MixSettings {
        master_gain: 0.8,
        click: ClickSettings {
            enabled: true,
            levels: [1.0, 0.8, 0.5, 0.25],
        },
        ..MixSettings::default()
    };
    settings.stem_gains.insert("keys".to_owned(), 0.5);
    settings
}

struct Run {
    output: Vec<f32>,
    /// Status after every 1 024 callback frames.
    statuses: Vec<Status>,
    last: Status,
}

impl Run {
    fn status(&self, at: u64) -> Status {
        self.statuses[(at / 1_024 - 1) as usize]
    }

    fn frames(&self, range: std::ops::Range<u64>) -> &[f32] {
        &self.output[2 * range.start as usize..2 * range.end as usize]
    }
}

fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

/// Drives a player in buffers of `buffer` frames. Before the buffer at
/// callback frame `at`: `before(at)`, the scripted commands for `at`, and,
/// with a monitor, a wait until the disk thread is idle.
fn drive(
    (mut player, mut controller): (Player, Controller),
    buffer: usize,
    frames: u64,
    script: &[(u64, Command)],
    monitor: Option<&DiskMonitor>,
    mut before: impl FnMut(u64),
) -> Run {
    let mut output = vec![0.0f32; 2 * frames as usize];
    let mut statuses = Vec::new();
    let mut done = 0u64;
    while done < frames {
        before(done);
        for &(_, command) in script.iter().filter(|(at, _)| *at == done) {
            controller.send(command).unwrap();
        }
        if let Some(monitor) = monitor {
            assert!(monitor.wait_idle(IDLE_TIMEOUT), "the disk thread keeps up");
        }
        let n = (buffer as u64).min(frames - done);
        player.process(&mut output[2 * done as usize..2 * (done + n) as usize]);
        done += n;
        if done.is_multiple_of(1_024) {
            statuses.push(player.status());
        }
    }
    let last = player.status();
    Run {
        output,
        statuses,
        last,
    }
}

fn memory_player(dir: &Path, arrangement: &str, buffer: usize) -> (Player, Controller) {
    let loaded = load_song(dir).unwrap();
    Player::new(
        &loaded.song,
        arrangement,
        &settings(),
        &PlayerConfig::new(RATE, buffer),
        memory_sources(loaded.stems).unwrap(),
    )
    .unwrap()
}

fn streaming_player(
    dir: &Path,
    arrangement: &str,
    buffer: usize,
    config: &StreamConfig,
) -> ((Player, Controller), DiskMonitor) {
    let streams = open_streaming(dir, config).unwrap();
    let player = Player::new(
        &streams.song,
        arrangement,
        &settings(),
        &PlayerConfig::new(RATE, buffer),
        streams.sources,
    )
    .unwrap();
    (player, streams.monitor)
}

/// A gate that holds back the blocks of `stem` (any stem when `None`) that
/// reach into `frames` until `open` is set.
fn gate(
    stem: Option<&'static str>,
    frames: std::ops::Range<u64>,
    open: &Arc<AtomicBool>,
) -> StreamConfig {
    let open = Arc::clone(open);
    StreamConfig {
        gate: Some(Arc::new(move |id: &str, block: std::ops::Range<u64>| {
            let held = stem.is_none_or(|stem| stem == id)
                && block.start < frames.end
                && frames.start < block.end;
            !held || open.load(Ordering::SeqCst)
        })),
        ..StreamConfig::default()
    }
}

fn scripted() -> Vec<(u64, Command)> {
    vec![
        (0, Command::Play),
        (0, Command::SetFadeDuration { frames: 6_000 }),
        (4_096, Command::QueueJump { position: 3 }),
        (20_480, Command::ToggleFade),
        (40_960, Command::SetJumpTiming(JumpTiming::NextBar)),
        (40_960, Command::QueueJump { position: 1 }),
        (51_200, Command::ToggleFade),
        (61_440, Command::LiveLoop),
        (81_920, Command::SetInfinite(true)),
        (102_400, Command::SetInfinite(false)),
        (110_592, Command::BackToStart),
        (122_880, Command::Pause),
        (126_976, Command::Play),
        (135_168, Command::Stop),
        (139_264, Command::Play),
    ]
}

#[test]
fn streaming_output_equals_memory_output_bit_exactly() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    let frames = 4 * LENGTH;
    for buffer in [64, 1_024] {
        for (arrangement, script) in [
            ("repeat", vec![(0, Command::Play)]),
            ("default", scripted()),
        ] {
            let memory = drive(
                memory_player(dir.path(), arrangement, buffer),
                buffer,
                frames,
                &script,
                None,
                |_| {},
            );
            let (player, monitor) =
                streaming_player(dir.path(), arrangement, buffer, &StreamConfig::default());
            let streamed = drive(player, buffer, frames, &script, Some(&monitor), |_| {});

            assert_eq!(
                bits(&streamed.output),
                bits(&memory.output),
                "{arrangement} at buffer size {buffer}"
            );
            assert_eq!(streamed.last.underruns, 0);
            assert!(memory.output.iter().any(|&sample| sample != 0.0));
        }
    }
}

#[test]
fn a_jump_whose_target_is_not_buffered_moves_to_the_next_boundary_and_never_plays_silence() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    let outro = 6 * (LENGTH / 7);
    let verse_end = 4 * (LENGTH / 7);
    let open = Arc::new(AtomicBool::new(false));
    let config = gate(None, outro..LENGTH, &open);
    let frames = 4 * 32_768;
    let script = [
        (0, Command::Play),
        (
            1_024,
            Command::QueueJump {
                position: POS_OUTRO,
            },
        ),
    ];

    let (player, monitor) = streaming_player(dir.path(), "default", 1_024, &config);
    let streamed = drive(player, 1_024, frames, &script, Some(&monitor), |at| {
        if at == 34_816 {
            open.store(true, Ordering::SeqCst);
        }
    });

    let deferred = streamed.status(33_792);
    assert_eq!(deferred.position, POS_VERSE, "the arrangement played on");
    let queued = deferred.queued.expect("the jump is still queued");
    assert_eq!(
        (queued.target, queued.deferrals, queued.executes_at),
        (POS_OUTRO, 1, verse_end)
    );
    // The same path, with the jump queued during the verse.
    let expected = drive(
        memory_player(dir.path(), "default", 1_024),
        1_024,
        frames,
        &[
            (0, Command::Play),
            (
                33_792,
                Command::QueueJump {
                    position: POS_OUTRO,
                },
            ),
        ],
        None,
        |_| {},
    );
    assert_eq!(bits(&streamed.output), bits(&expected.output));
    assert_eq!(streamed.last.underruns, 0, "no silence for the target");
}

#[test]
fn a_stalled_stream_plays_silence_for_its_stem_counts_underruns_and_recovers() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    let open = Arc::new(AtomicBool::new(false));
    let (stall, recover) = (16_384, 24_576);
    let config = gate(Some("drums"), stall..LENGTH, &open);
    let script = [(0, Command::Play)];
    let frames = 4 * BAR;

    let (player, monitor) = streaming_player(dir.path(), "default", 1_024, &config);
    let streamed = drive(player, 1_024, frames, &script, Some(&monitor), |at| {
        if at == recover {
            open.store(true, Ordering::SeqCst);
        }
    });
    let full = drive(
        memory_player(dir.path(), "default", 1_024),
        1_024,
        frames,
        &script,
        None,
        |_| {},
    );
    let loaded = load_song(dir.path()).unwrap();
    let others = loaded
        .stems
        .into_iter()
        .filter(|stem| stem.id != "drums")
        .collect();
    let mut without_drums = settings();
    without_drums.stem_gains.retain(|id, _| id != "drums");
    let silent_drums = drive(
        Player::new(
            &loaded.song,
            "default",
            &without_drums,
            &PlayerConfig::new(RATE, 1_024),
            memory_sources(others).unwrap(),
        )
        .unwrap(),
        1_024,
        frames,
        &script,
        None,
        |_| {},
    );

    assert_eq!(bits(streamed.frames(0..stall)), bits(full.frames(0..stall)));
    assert_eq!(
        bits(streamed.frames(stall..recover)),
        bits(silent_drums.frames(stall..recover)),
        "the stalled stem is silent"
    );
    assert_eq!(
        bits(streamed.frames(recover..frames)),
        bits(full.frames(recover..frames)),
        "it plays again once the data arrives"
    );
    assert_eq!(
        streamed.status(recover).underruns,
        8,
        "one per stalled buffer"
    );
    assert_eq!(streamed.last.underruns, 8);
}

#[test]
fn buffer_capacities_do_not_depend_on_song_length() {
    let rate = 1_000;
    let capacity = |minutes: u64| {
        let dir = tempfile::tempdir().unwrap();
        write_song(dir.path(), rate, minutes * 60 * u64::from(rate));
        open_streaming(dir.path(), &StreamConfig::default())
            .unwrap()
            .capacity
    };
    let short = capacity(1);
    let long = capacity(60);

    assert_eq!(short, long);
    assert_eq!(short.ahead_frames, 1_000, "one second ahead at 1 kHz");
    assert_eq!(short.head_frames, 500, "half a second at each target");
    assert_eq!(
        short.bytes_per_stem(),
        short.blocks_per_stem as u64 * short.block_frames as u64 * 8
    );
    assert!(short.frames_per_stem() < 60 * 60 * u64::from(rate) / 10);
}

#[test]
fn the_disk_thread_stops_when_the_player_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);

    let ((player, controller), monitor) =
        streaming_player(dir.path(), "default", 256, &StreamConfig::default());
    assert!(monitor.is_running());
    drop(controller);
    assert!(monitor.is_running(), "the sources still need it");
    drop(player);
    assert!(
        !monitor.is_running(),
        "dropping the player joined the thread"
    );

    let streams = open_streaming(dir.path(), &StreamConfig::default()).unwrap();
    let monitor = streams.monitor.clone();
    assert!(monitor.is_running());
    drop(streams);
    assert!(
        !monitor.is_running(),
        "dropping unused sources joins it too"
    );
}

#[test]
fn open_streaming_rejects_what_load_song_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let error = open_streaming(dir.path(), &StreamConfig::default()).expect_err("no manifest");
    assert!(matches!(error, LoadError::ReadManifest { .. }), "{error}");

    write_song(dir.path(), RATE, LENGTH);
    std::fs::remove_file(dir.path().join("stems/guide.wav")).unwrap();
    let error =
        open_streaming(dir.path(), &StreamConfig::default()).expect_err("a stem is missing");
    assert!(matches!(error, LoadError::Invalid(_)), "{error}");
}

#[test]
fn streaming_sources_report_their_stems() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    let streams = open_streaming(dir.path(), &StreamConfig::default()).unwrap();
    let stems: Vec<(String, StemRole)> = streams
        .sources
        .iter()
        .map(|source| (source.id().to_owned(), source.role()))
        .collect();
    assert_eq!(
        stems,
        [
            ("drums".to_owned(), StemRole::Track),
            ("keys".to_owned(), StemRole::Track),
            ("guide".to_owned(), StemRole::Guide),
        ]
    );
    assert_eq!(streams.song.length(), LENGTH);
}

// Regression tests: live changes in the buffer that holds a seam, with a
// disk that keeps up.

/// Positions played, in order, from the statuses (one per 1 024 frames).
fn positions(run: &Run) -> Vec<(u64, usize)> {
    let mut seen: Vec<(u64, usize)> = Vec::new();
    let mut last_source = 0;
    for (i, status) in run.statuses.iter().enumerate() {
        let at = (i as u64 + 1) * 1_024;
        if seen.last().is_none_or(|&(_, p)| p != status.position)
            || status.source_frame < last_source
        {
            seen.push((at, status.position));
        }
        last_source = status.source_frame;
    }
    seen
}

#[test]
fn turning_the_infinite_loop_on_just_before_the_section_end_never_plays_silence() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    // Default arrangement: the verse ends at 64 000, inside the buffer
    // 63 488..64 512, where the infinite loop is switched on.
    let script = [(0, Command::Play), (63_488, Command::SetInfinite(true))];
    let frames = 4 * 32_768;
    let (player, monitor) =
        streaming_player(dir.path(), "default", 1_024, &StreamConfig::default());
    let streamed = drive(player, 1_024, frames, &script, Some(&monitor), |_| {});
    let expected = drive(
        memory_player(dir.path(), "default", 1_024),
        1_024,
        frames,
        &script,
        None,
        |_| {},
    );
    assert_eq!(streamed.last.underruns, 0, "the loop seam played silence");
    assert_eq!(bits(&streamed.output), bits(&expected.output));
}

#[test]
fn turning_the_infinite_loop_off_just_before_the_section_end_never_plays_silence() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    // repeat: intro, chorus, chorus, verse, outro. Position 2 (64 000..96 000)
    // loops twice; it ends at 160 000, inside the buffer 159 744..160 768,
    // where the loop is switched off. The verse does not start where the
    // chorus ends.
    let script = [
        (0, Command::Play),
        (66_560, Command::SetInfinite(true)),
        (159_744, Command::SetInfinite(false)),
    ];
    let frames = 200_704;
    let (player, monitor) = streaming_player(dir.path(), "repeat", 1_024, &StreamConfig::default());
    let streamed = drive(player, 1_024, frames, &script, Some(&monitor), |_| {});
    let expected = drive(
        memory_player(dir.path(), "repeat", 1_024),
        1_024,
        frames,
        &script,
        None,
        |_| {},
    );
    assert_eq!(
        streamed.last.underruns, 0,
        "the seam into the verse played silence"
    );
    assert_eq!(bits(&streamed.output), bits(&expected.output));
}

#[test]
fn leaving_a_vamp_just_before_the_section_end_plays_the_next_position_once() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    let script = [
        (0, Command::Play),
        (66_560, Command::SetInfinite(true)),
        (159_744, Command::SetInfinite(false)),
        (159_744, Command::QueueJump { position: 3 }),
    ];
    let frames = 200_704;
    let (player, monitor) = streaming_player(dir.path(), "repeat", 1_024, &StreamConfig::default());
    let streamed = drive(player, 1_024, frames, &script, Some(&monitor), |_| {});
    let expected = drive(
        memory_player(dir.path(), "repeat", 1_024),
        1_024,
        frames,
        &script,
        None,
        |_| {},
    );
    assert_eq!(
        positions(&streamed),
        positions(&expected),
        "the verse plays once"
    );
    assert_eq!(streamed.last.underruns, 0);
    assert_eq!(bits(&streamed.output), bits(&expected.output));
}

#[test]
fn a_deferred_jump_to_the_position_the_arrangement_enters_anyway_does_not_repeat_it() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE, LENGTH);
    // repeat: intro, chorus, chorus, verse, outro. The chorus start stays
    // unbuffered until the second chorus is playing; a jump to the second
    // chorus is queued during the first.
    let chorus = 4 * (LENGTH / 7);
    let open = Arc::new(AtomicBool::new(false));
    let config = gate(None, chorus..chorus + 4_000, &open);
    let script = [
        (0, Command::Play),
        (36_864, Command::QueueJump { position: 2 }),
    ];
    let frames = 4 * 32_768;
    let (player, monitor) = streaming_player(dir.path(), "repeat", 1_024, &config);
    let streamed = drive(player, 1_024, frames, &script, Some(&monitor), |at| {
        if at == 70_656 {
            open.store(true, Ordering::SeqCst);
        }
    });
    let expected = drive(
        memory_player(dir.path(), "repeat", 1_024),
        1_024,
        frames,
        &[(0, Command::Play)],
        None,
        |_| {},
    );
    assert_eq!(
        positions(&streamed),
        positions(&expected),
        "two choruses, then the verse"
    );
}

/// S3: a click-only tile streams nothing, and its monitor says so.
#[test]
fn a_default_disk_monitor_watches_no_thread_and_is_always_idle() {
    let monitor = library::DiskMonitor::default();
    assert!(!monitor.is_running());
    assert!(monitor.is_idle());
    assert!(monitor.wait_idle(std::time::Duration::ZERO));
    assert_eq!(monitor.decode_errors(), 0);
}
