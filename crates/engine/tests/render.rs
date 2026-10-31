//! S5: deterministic offline renderer.

mod common;

use std::f64::consts::FRAC_PI_2;

use common::{SongSpec, bits, four_sections, mono, noise, onsets, plan, sine, stereo};
use domain::{ClickLevel, RenderPlan, SectionType, StemRole};
use engine::{
    ClickSettings, DEFAULT_CLICK_LEVELS, MixSettings, RenderError, StemAudio, audible_click_count,
    render_offline,
};

fn no_click() -> MixSettings {
    MixSettings {
        click: ClickSettings {
            enabled: false,
            ..ClickSettings::default()
        },
        ..MixSettings::default()
    }
}

fn click_levels(levels: [f32; 4]) -> MixSettings {
    MixSettings {
        click: ClickSettings {
            enabled: true,
            levels,
        },
        ..MixSettings::default()
    }
}

fn render(plan: &RenderPlan, stems: &[StemAudio], settings: &MixSettings) -> Vec<f32> {
    render_offline(plan, stems, settings).expect("plan renders")
}

fn click_positions(plan: &RenderPlan, levels: &[ClickLevel]) -> Vec<u64> {
    plan.clicks
        .iter()
        .filter(|click| levels.contains(&click.level))
        .map(|click| click.at)
        .collect()
}

/// Left and right sample of `stem` at `frame`; silence past its end.
fn frame_of(stem: &StemAudio, frame: u64) -> (f32, f32) {
    if frame >= stem.frames {
        return (0.0, 0.0);
    }
    let frame = frame as usize;
    match stem.channels {
        1 => (stem.samples[frame], stem.samples[frame]),
        _ => (stem.samples[2 * frame], stem.samples[2 * frame + 1]),
    }
}

/// The plain sum of the stems, frame by frame, in stem order.
fn plain_sum(stems: &[StemAudio], frames: u64) -> Vec<f32> {
    let mut sum = Vec::with_capacity(2 * frames as usize);
    for frame in 0..frames {
        let (mut left, mut right) = (0.0f32, 0.0f32);
        for stem in stems {
            let (l, r) = frame_of(stem, frame);
            left += l;
            right += r;
        }
        sum.push(left);
        sum.push(right);
    }
    sum
}

#[test]
fn null_test_contiguous_render_is_the_plain_sum_of_the_stems() {
    let song = SongSpec::new(
        48_000,
        192_000,
        vec![
            ("intro", SectionType::Intro, 0),
            ("verse", SectionType::Verse, 96_000),
        ],
        vec![
            ("drums", StemRole::Track),
            ("keys", StemRole::Track),
            ("bass", StemRole::Track),
            ("guide", StemRole::Guide),
        ],
    )
    .build();
    let plan = plan(&song, "default");
    assert!(
        plan.segments
            .iter()
            .all(|s| s.fade_in == 0 && s.fade_out == 0)
    );
    let stems = [
        mono("drums", StemRole::Track, noise(1, 192_000)),
        stereo("keys", StemRole::Track, noise(2, 2 * 192_000)),
        mono("bass", StemRole::Track, noise(3, 192_000)),
        mono("guide", StemRole::Guide, noise(4, 192_000)),
    ];

    let output = render(&plan, &stems, &no_click());

    assert_eq!(output.len() as u64, 2 * plan.length);
    assert_eq!(bits(&output), bits(&plain_sum(&stems, plan.length)));
}

#[test]
fn rendering_twice_gives_bit_identical_output() {
    let song = four_sections(vec![("drums", StemRole::Track), ("pad", StemRole::Track)]).build();
    let plan = plan(&song, "f23");
    let stems = [
        mono("drums", StemRole::Track, noise(5, 1_152_000)),
        stereo("pad", StemRole::Track, noise(6, 2 * 1_152_000)),
    ];
    let settings = MixSettings {
        master_gain: 0.7,
        click: ClickSettings {
            enabled: true,
            levels: [1.0, 0.8, 0.5, 0.25],
        },
        ..MixSettings::default()
    };

    let first = render(&plan, &stems, &settings);
    let second = render(&plan, &stems, &settings);

    assert_eq!(bits(&first), bits(&second));
}

#[test]
fn impulse_test_every_click_burst_starts_on_its_planned_frame() {
    // 113 bpm: a sixteenth is 6 371.68 samples, so ticks fall between frames.
    let song = SongSpec::new(
        48_000,
        816_000,
        vec![
            ("intro", SectionType::Intro, 0),
            ("verse", SectionType::Verse, 204_000),
        ],
        vec![("silence", StemRole::Track)],
    )
    .bpm(113.0)
    .build();
    let plan = plan(&song, "default");
    let stems = [mono("silence", StemRole::Track, vec![0.0; 816_000])];
    let levels = [
        ClickLevel::Downbeat,
        ClickLevel::Beat,
        ClickLevel::HalfBeat,
        ClickLevel::QuarterBeat,
    ];

    for (index, level) in levels.into_iter().enumerate() {
        let mut one_level = [0.0; 4];
        one_level[index] = 1.0;
        let output = render(&plan, &stems, &click_levels(one_level));
        assert_eq!(
            onsets(&output),
            click_positions(&plan, &[level]),
            "{level:?} bursts"
        );

        // The level scales the burst; halving is exact in f32.
        one_level[index] = 0.5;
        let half = render(&plan, &stems, &click_levels(one_level));
        let halved: Vec<f32> = output.iter().map(|sample| sample * 0.5).collect();
        assert_eq!(bits(&half), bits(&halved), "{level:?} at 0.5");
    }

    let defaults = render(&plan, &stems, &click_levels(DEFAULT_CLICK_LEVELS));
    assert_eq!(
        onsets(&defaults),
        click_positions(&plan, &[ClickLevel::Downbeat, ClickLevel::Beat]),
        "levels at 0.0 emit nothing"
    );
    let silent = render(&plan, &stems, &click_levels([0.0; 4]));
    assert!(silent.iter().all(|&sample| sample == 0.0));
}

#[test]
fn jump_seams_crossfade_with_equal_power_and_contiguous_seams_are_copies() {
    let song = four_sections(vec![("tone", StemRole::Track)]).build();
    let plan = plan(&song, "f23");
    let source = sine(441.0, 48_000, 1_152_000);
    let stems = [mono("tone", StemRole::Track, source.clone())];

    let output = render(&plan, &stems, &no_click());

    assert_eq!(output.len() as u64, 2 * plan.length);
    let fades: Vec<u64> = plan.segments.iter().map(|s| s.fade_in).collect();
    assert_eq!(fades, [0, 0, 0, 256, 0], "only the chorus repeat jumps");
    let left: Vec<f32> = output.iter().step_by(2).copied().collect();
    let right: Vec<f32> = output.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(bits(&left), bits(&right), "a mono stem plays on both sides");

    let mut faded = 0;
    for (index, segment) in plan.segments.iter().enumerate() {
        for i in 0..segment.len {
            let out = (segment.out_start + i) as usize;
            let incoming = source[(segment.src_start + i) as usize];
            if i < segment.fade_in {
                let outgoing = &plan.segments[index - 1];
                let tail = source[(outgoing.src_start + outgoing.len + i) as usize];
                let t = (i as f64 + 0.5) / segment.fade_in as f64;
                let expected = f64::from(tail) * (FRAC_PI_2 * t).cos()
                    + f64::from(incoming) * (FRAC_PI_2 * t).sin();
                assert!(
                    (f64::from(left[out]) - expected).abs() < 1e-6,
                    "frame {out}: {} != {expected}",
                    left[out]
                );
                faded += 1;
            } else {
                assert_eq!(
                    left[out].to_bits(),
                    incoming.to_bits(),
                    "frame {out} is an untouched copy"
                );
            }
        }
    }
    assert_eq!(faded, 256);
}

#[test]
fn a_click_stem_replaces_the_generated_click() {
    let song = SongSpec::new(
        48_000,
        192_000,
        vec![("intro", SectionType::Intro, 0)],
        vec![("drums", StemRole::Track), ("click", StemRole::Click)],
    )
    .build();
    let plan = plan(&song, "default");
    assert!(!plan.clicks.is_empty());
    let mut stems = vec![
        mono("drums", StemRole::Track, noise(7, 192_000)),
        mono("click", StemRole::Click, noise(8, 192_000)),
    ];
    let settings = click_levels([1.0; 4]);

    let output = render(&plan, &stems, &settings);
    assert_eq!(bits(&output), bits(&plain_sum(&stems, plan.length)));
    assert_eq!(audible_click_count(&plan, &stems, &settings), 0);

    stems[1].role = StemRole::Track;
    let generated = render(&plan, &stems, &settings);
    assert_ne!(
        bits(&generated),
        bits(&output),
        "without a click stem it clicks"
    );
    assert_eq!(
        audible_click_count(&plan, &stems, &settings),
        plan.clicks.len()
    );
}

#[test]
fn a_ten_minute_song_has_zero_drift() {
    // 8 kHz keeps the buffers small; 127 bpm puts ticks between frames.
    let rate = 8_000;
    let length = 10 * 60 * u64::from(rate) + 1_234;
    let song = SongSpec::new(
        rate,
        length,
        vec![
            ("intro", SectionType::Intro, 0),
            ("verse", SectionType::Verse, 1_000_000),
            ("outro", SectionType::Outro, 4_000_000),
        ],
        vec![("marker", StemRole::Track)],
    )
    .bpm(127.0)
    .build();
    let plan = plan(&song, "default");
    assert_eq!(plan.length, length);

    let silence = [mono("marker", StemRole::Track, vec![0.0; length as usize])];
    let clicks = render(&plan, &silence, &MixSettings::default());
    assert_eq!(clicks.len() as u64, 2 * length);
    let expected = click_positions(&plan, &[ClickLevel::Downbeat, ClickLevel::Beat]);
    let found = onsets(&clicks);
    assert_eq!(
        found.last(),
        expected.last(),
        "last click on its planned frame"
    );
    assert_eq!(found, expected);

    let mut marked = vec![0.0; length as usize];
    marked[0] = 0.25;
    marked[length as usize - 1] = -0.25;
    let markers = [mono("marker", StemRole::Track, marked)];
    let audio = render(&plan, &markers, &no_click());
    assert_eq!(audio.len() as u64, 2 * length);
    assert_eq!(audio[..2], [0.25, 0.25]);
    assert_eq!(
        audio[audio.len() - 2..],
        [-0.25, -0.25],
        "last frame aligned"
    );
}

#[test]
fn stem_and_master_gains_scale_the_mix() {
    let song = SongSpec::new(
        48_000,
        96_000,
        vec![("intro", SectionType::Intro, 0)],
        vec![("drums", StemRole::Track), ("bass", StemRole::Track)],
    )
    .build();
    let plan = plan(&song, "default");
    let drums = noise(9, 96_000);
    let bass = noise(10, 96_000);
    let stems = [
        mono("drums", StemRole::Track, drums.clone()),
        mono("bass", StemRole::Track, bass.clone()),
    ];
    let mut settings = no_click();
    settings.stem_gains.insert("drums".to_owned(), 0.5);
    settings.master_gain = 0.25;

    let output = render(&plan, &stems, &settings);

    for frame in 0..96_000 {
        let expected = (drums[frame] * 0.5 + bass[frame]) * 0.25;
        assert_eq!(
            output[2 * frame].to_bits(),
            expected.to_bits(),
            "frame {frame}"
        );
    }
}

#[test]
fn stems_shorter_than_the_song_are_silent_past_their_end() {
    let song = SongSpec::new(
        48_000,
        96_000,
        vec![("intro", SectionType::Intro, 0)],
        vec![("drums", StemRole::Track), ("short", StemRole::Track)],
    )
    .build();
    let plan = plan(&song, "default");
    let stems = [
        mono("drums", StemRole::Track, noise(11, 96_000)),
        mono("short", StemRole::Track, noise(12, 50_000)),
    ];

    let output = render(&plan, &stems, &no_click());

    assert_eq!(output.len(), 2 * 96_000);
    assert_eq!(bits(&output), bits(&plain_sum(&stems, plan.length)));
}

#[test]
fn defaults_play_every_stem_at_unity_with_the_beat_click() {
    let settings = MixSettings::default();
    assert_eq!(settings.master_gain, 1.0);
    assert_eq!(settings.stem_gain("anything"), 1.0);
    assert!(settings.click.enabled);
    assert_eq!(settings.click.levels, [1.0, 0.8, 0.0, 0.0]);
    assert_eq!(settings.click.level(ClickLevel::Beat), 0.8);
}

#[test]
fn malformed_stems_and_unknown_gains_are_errors() {
    let song = SongSpec::new(
        48_000,
        96_000,
        vec![("intro", SectionType::Intro, 0)],
        vec![("drums", StemRole::Track)],
    )
    .build();
    let plan = plan(&song, "default");

    let mut surround = mono("drums", StemRole::Track, vec![0.0; 6 * 10]);
    surround.channels = 6;
    surround.frames = 10;
    assert_eq!(
        render_offline(&plan, &[surround], &no_click()),
        Err(RenderError::UnsupportedChannels {
            id: "drums".into(),
            channels: 6
        })
    );

    let mut short = stereo("drums", StemRole::Track, vec![0.0; 20]);
    short.frames = 11;
    let error = render_offline(&plan, &[short], &no_click()).unwrap_err();
    assert_eq!(
        error,
        RenderError::SampleCountMismatch {
            id: "drums".into(),
            frames: 11,
            channels: 2,
            samples: 20
        }
    );
    assert!(error.to_string().contains("\"drums\""));

    let mut settings = no_click();
    settings.stem_gains.insert("vocals".to_owned(), 0.5);
    let stems = [mono("drums", StemRole::Track, vec![0.0; 96_000])];
    let error = render_offline(&plan, &stems, &settings).unwrap_err();
    assert_eq!(
        error,
        RenderError::UnknownStemGain {
            id: "vocals".into()
        }
    );
    assert!(error.to_string().contains("\"vocals\""));
}
