//! S2-S4: arrangement compiler, click events, and guide cue events.
//!
//! Unless a test says otherwise the song runs at 120 bpm in 4/4 at 48 kHz:
//! one sixteenth is 6 000 samples, one beat 24 000, one bar 96 000.

mod common;

use common::{RATE, SongSpec, four_section_spec, marker};
use domain::{
    ClickEvent, ClickLevel, CompileError, CompileOptions, CueEvent, CueTiming, RenderPlan,
    SectionType, Segment, Song, compile,
};

fn seg(section_id: &str, out_start: u64, src_start: u64, len: u64, crossfade_in: u64) -> Segment {
    Segment {
        section_id: section_id.to_owned(),
        out_start,
        src_start,
        len,
        crossfade_in,
    }
}

fn cue(at: u64, section_id: &str, section_type: SectionType, number: Option<u32>) -> CueEvent {
    CueEvent {
        at,
        section_id: section_id.to_owned(),
        section_type,
        number,
    }
}

fn plan(song: &Song, arrangement: &str) -> RenderPlan {
    compile(song, arrangement, &CompileOptions::default()).expect("arrangement compiles")
}

fn plan_with(song: &Song, arrangement: &str, options: CompileOptions) -> RenderPlan {
    compile(song, arrangement, &options).expect("arrangement compiles")
}

fn at_section_start() -> CompileOptions {
    CompileOptions {
        cue_timing: CueTiming::AtSectionStart,
        ..CompileOptions::default()
    }
}

fn positions(plan: &RenderPlan, levels: &[ClickLevel]) -> Vec<u64> {
    plan.clicks
        .iter()
        .filter(|click| levels.contains(&click.level))
        .map(|click| click.at)
        .collect()
}

const BEATS: [ClickLevel; 2] = [ClickLevel::Accent, ClickLevel::Quarter];

fn assert_contiguous(plan: &RenderPlan) {
    assert!(!plan.segments.is_empty());
    assert_eq!(plan.segments[0].out_start, 0);
    for pair in plan.segments.windows(2) {
        assert_eq!(
            pair[0].out_start + pair[0].len,
            pair[1].out_start,
            "gap or overlap"
        );
    }
    let total: u64 = plan.segments.iter().map(|s| s.len).sum();
    assert_eq!(plan.length, total);
}

fn assert_clicks_sorted(plan: &RenderPlan) {
    assert!(
        plan.clicks
            .windows(2)
            .all(|w| (w[0].at, w[0].level) <= (w[1].at, w[1].level)),
        "clicks are sorted by position, then level"
    );
}

#[test]
fn f2_3_repeated_chorus_plays_twice_with_one_crossfaded_seam() {
    let song = four_section_spec()
        .arrangement("live", &["intro", "verse", "chorus", "chorus", "outro"])
        .build();
    let plan = plan(&song, "live");

    assert_eq!(plan.sample_rate, RATE);
    assert_eq!(
        plan.segments,
        [
            seg("intro", 0, 0, 192_000, 0),
            seg("verse", 192_000, 192_000, 384_000, 0),
            seg("chorus", 576_000, 576_000, 384_000, 0),
            seg("chorus", 960_000, 576_000, 384_000, 256),
            seg("outro", 1_344_000, 960_000, 192_000, 0),
        ]
    );
    assert_eq!(plan.segments[3].src_start, plan.segments[2].src_start);
    assert_eq!(plan.length, 1_536_000);
    assert_contiguous(&plan);

    // No gap in the click either: every bar-aligned section keeps the grid.
    assert_eq!(plan.clicks.len(), 1_536_000 / 6_000);
    assert!(
        plan.clicks
            .iter()
            .enumerate()
            .all(|(i, c)| c.at == i as u64 * 6_000)
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Accent]),
        (0..16).map(|bar| bar * 96_000).collect::<Vec<_>>()
    );
}

#[test]
fn default_arrangement_plays_the_song_straight_through() {
    let song = four_section_spec().build();
    let plan = plan(&song, "default");
    assert_eq!(
        plan.segments,
        [
            seg("intro", 0, 0, 192_000, 0),
            seg("verse", 192_000, 192_000, 384_000, 0),
            seg("chorus", 576_000, 576_000, 384_000, 0),
            seg("outro", 960_000, 960_000, 192_000, 0),
        ]
    );
    assert_eq!(plan.length, song.length());
}

#[test]
fn seam_crossfade_is_recorded_only_where_the_source_jumps() {
    let song = four_section_spec()
        .arrangement("back", &["chorus", "intro", "verse"])
        .build();
    let options = CompileOptions {
        seam_crossfade: 1_024,
        ..CompileOptions::default()
    };
    assert_eq!(
        plan_with(&song, "back", options).segments,
        [
            seg("chorus", 0, 576_000, 384_000, 0),
            seg("intro", 384_000, 0, 192_000, 1_024),
            seg("verse", 576_000, 192_000, 384_000, 0),
        ]
    );

    let longer_than_intro = CompileOptions {
        seam_crossfade: 1_000_000,
        ..CompileOptions::default()
    };
    let plan = plan_with(&song, "back", longer_than_intro);
    assert_eq!(
        plan.segments[1].crossfade_in, 192_000,
        "capped at the segment length"
    );
}

#[test]
fn rejects_unknown_or_empty_arrangement() {
    let song = four_section_spec().arrangement("empty", &[]).build();
    let options = CompileOptions::default();

    let error = compile(&song, "nope", &options).expect_err("unknown arrangement");
    assert_eq!(
        error,
        CompileError::UnknownArrangement { id: "nope".into() }
    );
    assert!(error.to_string().contains("\"nope\""));

    let error = compile(&song, "empty", &options).expect_err("empty arrangement");
    assert_eq!(error, CompileError::EmptyArrangement { id: "empty".into() });
    assert!(error.to_string().contains("\"empty\""));
}

#[test]
fn constant_tempo_clicks_land_on_exact_samples() {
    let song = SongSpec::new(192_000, vec![marker("all", SectionType::Verse, None, 0)]).build();
    let plan = plan(&song, "default");

    assert_eq!(plan.clicks.len(), 32);
    assert_clicks_sorted(&plan);
    assert_eq!(positions(&plan, &[ClickLevel::Accent]), [0, 96_000]);
    assert_eq!(
        positions(&plan, &[ClickLevel::Quarter]),
        [24_000, 48_000, 72_000, 120_000, 144_000, 168_000]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Eighth]),
        (0..8).map(|n| 12_000 + n * 24_000).collect::<Vec<_>>()
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Sixteenth]),
        (0..16).map(|n| 6_000 + n * 12_000).collect::<Vec<_>>()
    );
}

#[test]
fn subdivisions_split_each_beat_into_eighths_and_sixteenths() {
    let song = SongSpec::new(96_000, vec![marker("all", SectionType::Verse, None, 0)]).build();
    let plan = plan(&song, "default");
    assert_eq!(
        plan.clicks[..9],
        [
            ClickEvent {
                at: 0,
                level: ClickLevel::Accent
            },
            ClickEvent {
                at: 6_000,
                level: ClickLevel::Sixteenth
            },
            ClickEvent {
                at: 12_000,
                level: ClickLevel::Eighth
            },
            ClickEvent {
                at: 18_000,
                level: ClickLevel::Sixteenth
            },
            ClickEvent {
                at: 24_000,
                level: ClickLevel::Quarter
            },
            ClickEvent {
                at: 30_000,
                level: ClickLevel::Sixteenth
            },
            ClickEvent {
                at: 36_000,
                level: ClickLevel::Eighth
            },
            ClickEvent {
                at: 42_000,
                level: ClickLevel::Sixteenth
            },
            ClickEvent {
                at: 48_000,
                level: ClickLevel::Quarter
            },
        ]
    );
    let count = |level| plan.clicks.iter().filter(|c| c.level == level).count();
    assert_eq!(count(ClickLevel::Accent), 1);
    assert_eq!(count(ClickLevel::Quarter), 3);
    assert_eq!(count(ClickLevel::Eighth), 4);
    assert_eq!(count(ClickLevel::Sixteenth), 8);
}

#[test]
fn clicks_follow_a_tempo_change_on_a_bar_line() {
    // 120 bpm for one bar, then 90 bpm (32 000 samples per beat) for two bars.
    let song = SongSpec::new(352_000, vec![marker("all", SectionType::Verse, None, 0)])
        .tempo(&[(0, 120.0), (96_000, 90.0)])
        .build();
    let plan = plan(&song, "default");
    assert_eq!(
        positions(&plan, &BEATS),
        [
            0, 24_000, 48_000, 72_000, 96_000, 128_000, 160_000, 192_000, 224_000, 256_000,
            288_000, 320_000
        ]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Accent]),
        [0, 96_000, 224_000]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Eighth])[4..6],
        [112_000, 144_000]
    );
}

#[test]
fn beat_grid_stays_continuous_across_a_mid_beat_tempo_change() {
    // The tempo halves at beat 1.5: beat 2 lands half a slow beat later.
    let song = SongSpec::new(210_000, vec![marker("all", SectionType::Verse, None, 0)])
        .tempo(&[(0, 120.0), (36_000, 60.0)])
        .build();
    let plan = plan(&song, "default");
    assert_eq!(
        positions(&plan, &BEATS),
        [0, 24_000, 60_000, 108_000, 156_000, 204_000]
    );
    assert_eq!(positions(&plan, &[ClickLevel::Accent]), [0, 156_000]);
    let first: Vec<(u64, ClickLevel)> = plan.clicks[..9].iter().map(|c| (c.at, c.level)).collect();
    assert_eq!(
        first,
        [
            (0, ClickLevel::Accent),
            (6_000, ClickLevel::Sixteenth),
            (12_000, ClickLevel::Eighth),
            (18_000, ClickLevel::Sixteenth),
            (24_000, ClickLevel::Quarter),
            (30_000, ClickLevel::Sixteenth),
            (36_000, ClickLevel::Eighth),
            (48_000, ClickLevel::Sixteenth),
            (60_000, ClickLevel::Quarter),
        ]
    );
}

/// Expected beat position from the region start, computed independently.
fn expected_beat(n: u64, tempo: &[(u64, f64)]) -> f64 {
    let samples_per_beat = |bpm: f64| f64::from(RATE) * 60.0 / bpm;
    let mut region_beat = 0.0;
    let mut index = 0;
    while index + 1 < tempo.len() {
        let (at, bpm) = tempo[index];
        let next_beat = region_beat + (tempo[index + 1].0 - at) as f64 / samples_per_beat(bpm);
        if (n as f64) < next_beat {
            break;
        }
        region_beat = next_beat;
        index += 1;
    }
    let (at, bpm) = tempo[index];
    at as f64 + (n as f64 - region_beat) * samples_per_beat(bpm)
}

#[test]
fn every_beat_lands_within_one_sample_across_tempo_changes_for_sixty_minutes() {
    let length = u64::from(RATE) * 3_600;
    let tempo = [(0, 123.45), (40_000_003, 77.7), (100_000_000, 141.1)];
    let song = SongSpec::new(length, vec![marker("all", SectionType::Verse, None, 0)])
        .tempo(&tempo)
        .build();
    let plan = plan(&song, "default");
    let beats = positions(&plan, &BEATS);
    assert!(beats.len() > 6_000, "a full hour of beats");
    for (n, &at) in beats.iter().enumerate() {
        let expected = expected_beat(n as u64, &tempo);
        assert!(
            (at as f64 - expected).abs() <= 1.0,
            "beat {n} at {at}, expected {expected}"
        );
    }
    assert!(*beats.last().unwrap() < length);
    assert!(plan.clicks.iter().all(|c| c.at < length));
}

#[test]
fn meter_change_restarts_bars_with_an_accent() {
    // Two bars of 4/4, then two bars of 3/4 (72 000 samples per bar).
    let song = SongSpec::new(336_000, vec![marker("all", SectionType::Verse, None, 0)])
        .meters(&[(0, 4, 4), (192_000, 3, 4)])
        .build();
    let plan = plan(&song, "default");
    assert_eq!(
        positions(&plan, &[ClickLevel::Accent]),
        [0, 96_000, 192_000, 264_000]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Quarter]),
        [
            24_000, 48_000, 72_000, 120_000, 144_000, 168_000, 216_000, 240_000, 288_000, 312_000
        ]
    );
    assert_eq!(plan.clicks.len(), 336_000 / 6_000);
    assert_clicks_sorted(&plan);
}

#[test]
fn clicks_follow_source_positions_after_rearranging() {
    // The second section starts mid-beat, so its first click is a sixteenth.
    let song = SongSpec::new(
        192_000,
        vec![
            marker("a", SectionType::Intro, None, 0),
            marker("b", SectionType::Verse, None, 30_000),
        ],
    )
    .arrangement("swap", &["b", "a"])
    .build();
    let straight = plan(&song, "default");
    let swapped = plan(&song, "swap");
    assert_eq!(
        swapped.segments,
        [
            seg("b", 0, 30_000, 162_000, 0),
            seg("a", 162_000, 0, 30_000, 256)
        ]
    );
    assert_eq!(
        swapped.clicks[..2],
        [
            ClickEvent {
                at: 0,
                level: ClickLevel::Sixteenth
            },
            ClickEvent {
                at: 6_000,
                level: ClickLevel::Eighth
            },
        ]
    );
    for segment in &swapped.segments {
        let source: Vec<ClickEvent> = straight
            .clicks
            .iter()
            .filter(|c| (segment.src_start..segment.src_start + segment.len).contains(&c.at))
            .map(|c| ClickEvent {
                at: segment.out_start + (c.at - segment.src_start),
                level: c.level,
            })
            .collect();
        let output: Vec<ClickEvent> = swapped
            .clicks
            .iter()
            .filter(|c| (segment.out_start..segment.out_start + segment.len).contains(&c.at))
            .copied()
            .collect();
        assert_eq!(output, source, "segment {}", segment.section_id);
    }
    assert_eq!(swapped.clicks.len(), straight.clicks.len());
    assert_clicks_sorted(&swapped);
}

#[test]
fn reordering_sections_reorders_the_cues() {
    let song = four_section_spec()
        .arrangement("vc", &["intro", "verse", "chorus"])
        .arrangement("cv", &["intro", "chorus", "verse"])
        .build();
    assert_eq!(
        plan(&song, "vc").cues,
        [
            cue(96_000, "verse", SectionType::Verse, Some(1)),
            cue(480_000, "chorus", SectionType::Chorus, None),
        ]
    );
    assert_eq!(
        plan(&song, "cv").cues,
        [
            cue(96_000, "chorus", SectionType::Chorus, None),
            cue(480_000, "verse", SectionType::Verse, Some(1)),
        ]
    );
    assert_eq!(
        plan_with(&song, "cv", at_section_start()).cues,
        [
            cue(0, "intro", SectionType::Intro, None),
            cue(192_000, "chorus", SectionType::Chorus, None),
            cue(576_000, "verse", SectionType::Verse, Some(1)),
        ]
    );
}

#[test]
fn cues_default_to_one_bar_before_and_can_play_at_section_start() {
    let defaults = CompileOptions::default();
    assert_eq!(defaults.cue_timing, CueTiming::OneBarBefore);
    assert_eq!(defaults.seam_crossfade, 256);
    assert_eq!(defaults.cue_length, None);

    let song = four_section_spec().build();
    assert_eq!(
        plan(&song, "default").cues,
        [
            cue(96_000, "verse", SectionType::Verse, Some(1)),
            cue(480_000, "chorus", SectionType::Chorus, None),
            cue(864_000, "outro", SectionType::Outro, None),
        ]
    );
    assert_eq!(
        plan_with(&song, "default", at_section_start()).cues,
        [
            cue(0, "intro", SectionType::Intro, None),
            cue(192_000, "verse", SectionType::Verse, Some(1)),
            cue(576_000, "chorus", SectionType::Chorus, None),
            cue(960_000, "outro", SectionType::Outro, None),
        ]
    );
}

#[test]
fn one_bar_before_uses_the_tempo_and_meter_at_the_section_start() {
    // From the verse on the tempo is 60 bpm: one bar is 192 000 samples.
    let slow = four_section_spec()
        .tempo(&[(0, 120.0), (192_000, 60.0)])
        .build();
    assert_eq!(
        plan(&slow, "default").cues,
        [
            cue(0, "verse", SectionType::Verse, Some(1)),
            cue(384_000, "chorus", SectionType::Chorus, None),
            cue(768_000, "outro", SectionType::Outro, None),
        ]
    );

    // From the verse on the meter is 3/4: one bar is 72 000 samples.
    let waltz = four_section_spec()
        .meters(&[(0, 4, 4), (192_000, 3, 4)])
        .build();
    assert_eq!(
        plan(&waltz, "default").cues,
        [
            cue(120_000, "verse", SectionType::Verse, Some(1)),
            cue(504_000, "chorus", SectionType::Chorus, None),
            cue(888_000, "outro", SectionType::Outro, None),
        ]
    );
}

#[test]
fn cue_that_would_start_before_zero_is_dropped() {
    // The count-off is half a bar, so even the verse cue would start before 0.
    let song = SongSpec::new(
        240_000,
        vec![
            marker("count", SectionType::CountOff, None, 0),
            marker("verse", SectionType::Verse, Some(1), 48_000),
        ],
    )
    .build();
    let short_cues = CompileOptions {
        cue_length: Some(24_000),
        ..CompileOptions::default()
    };
    assert!(plan_with(&song, "default", short_cues).cues.is_empty());
    assert_eq!(
        plan_with(
            &song,
            "default",
            CompileOptions {
                cue_timing: CueTiming::AtSectionStart,
                ..short_cues
            }
        )
        .cues,
        [
            cue(0, "count", SectionType::CountOff, None),
            cue(48_000, "verse", SectionType::Verse, Some(1)),
        ]
    );
}

#[test]
fn cue_is_skipped_when_its_section_is_shorter_than_the_cue() {
    // The tag lasts half a bar; the default cue lasts one bar.
    let song = SongSpec::new(
        432_000,
        vec![
            marker("intro", SectionType::Intro, None, 0),
            marker("tag", SectionType::Tag, None, 192_000),
            marker("chorus", SectionType::Chorus, None, 240_000),
        ],
    )
    .build();
    assert_eq!(
        plan_with(&song, "default", at_section_start()).cues,
        [
            cue(0, "intro", SectionType::Intro, None),
            cue(240_000, "chorus", SectionType::Chorus, None),
        ]
    );
    assert_eq!(
        plan(&song, "default").cues,
        [cue(144_000, "chorus", SectionType::Chorus, None)]
    );
    let half_bar_cues = CompileOptions {
        cue_length: Some(48_000),
        ..at_section_start()
    };
    assert_eq!(
        plan_with(&song, "default", half_bar_cues).cues,
        [
            cue(0, "intro", SectionType::Intro, None),
            cue(192_000, "tag", SectionType::Tag, None),
            cue(240_000, "chorus", SectionType::Chorus, None),
        ]
    );
}

#[test]
fn cue_that_overlaps_the_previous_cue_is_dropped() {
    // The verse lasts one bar at 120 bpm; the chorus is one bar at 60 bpm,
    // so its cue would start inside the verse cue.
    let song = SongSpec::new(
        480_000,
        vec![
            marker("intro", SectionType::Intro, None, 0),
            marker("verse", SectionType::Verse, Some(1), 192_000),
            marker("chorus", SectionType::Chorus, None, 288_000),
        ],
    )
    .tempo(&[(0, 120.0), (288_000, 60.0)])
    .build();
    assert_eq!(
        plan(&song, "default").cues,
        [cue(96_000, "verse", SectionType::Verse, Some(1))]
    );
    assert_eq!(
        plan_with(&song, "default", at_section_start()).cues,
        [
            cue(0, "intro", SectionType::Intro, None),
            cue(192_000, "verse", SectionType::Verse, Some(1)),
            cue(288_000, "chorus", SectionType::Chorus, None),
        ]
    );
}

/// Deterministic xorshift so the property test needs no extra dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }
}

#[test]
fn any_arrangement_compiles_to_contiguous_segments_and_non_overlapping_cues() {
    let ids = ["intro", "verse", "chorus", "outro"];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut arrangements: Vec<Vec<&str>> = vec![
        vec!["chorus"],
        vec!["outro", "intro"],
        vec!["chorus", "chorus", "chorus"],
    ];
    for _ in 0..40 {
        let len = 1 + rng.next(12) as usize;
        arrangements.push((0..len).map(|_| ids[rng.next(4) as usize]).collect());
    }
    let mut spec = four_section_spec();
    for (i, sections) in arrangements.iter().enumerate() {
        spec = spec.arrangement(&format!("a{i}"), sections);
    }
    let song = spec.build();

    for (i, sections) in arrangements.iter().enumerate() {
        let id = format!("a{i}");
        for cue_length in [None, Some(48_000)] {
            for cue_timing in [CueTiming::OneBarBefore, CueTiming::AtSectionStart] {
                let options = CompileOptions {
                    cue_timing,
                    cue_length,
                    ..CompileOptions::default()
                };
                let plan = plan_with(&song, &id, options);
                assert_eq!(
                    plan,
                    plan_with(&song, &id, options),
                    "compile is deterministic"
                );
                assert_contiguous(&plan);
                assert_eq!(plan.segments.len(), sections.len());

                let mut previous_src_end = None;
                for (segment, section_id) in plan.segments.iter().zip(sections) {
                    let section = song.section(section_id).unwrap();
                    assert_eq!(segment.section_id, *section_id);
                    assert_eq!(
                        (segment.src_start, segment.len),
                        (section.start, section.length())
                    );
                    let jumps = previous_src_end.is_some_and(|end| end != section.start);
                    assert_eq!(segment.crossfade_in, if jumps { 256 } else { 0 });
                    previous_src_end = Some(section.end);
                }

                assert_clicks_sorted(&plan);
                assert!(plan.clicks.iter().all(|c| c.at < plan.length));
                // Bar-aligned sections at a constant tempo keep the grid.
                assert_eq!(plan.clicks.len() as u64, plan.length / 6_000);

                let reserved = cue_length.unwrap_or(96_000);
                for pair in plan.cues.windows(2) {
                    assert!(pair[0].at + reserved <= pair[1].at, "cues overlap in {id}");
                }
                assert!(plan.cues.iter().all(|c| c.at < plan.length));
            }
        }
    }
}
