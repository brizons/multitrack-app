//! S2-S4: arrangement compiler, click events, and guide cue events.
//!
//! Unless a test says otherwise the song runs at 120 bpm in 4/4 at 48 kHz:
//! one sixteenth is 6 000 samples, one beat 24 000, one bar 96 000.

mod common;

use common::{RATE, SongSpec, four_section_spec, info, marker};
use domain::{
    ClickEvent, ClickLevel, CompileError, CompileOptions, CueEvent, CueTiming,
    DEFAULT_SEAM_CROSSFADE, RenderPlan, SectionType, Segment, Song, compile, validate,
};

fn seg(id: &str, out_start: u64, src_start: u64, len: u64, fades: (u64, u64)) -> Segment {
    Segment {
        section_id: id.to_owned(),
        out_start,
        src_start,
        len,
        fade_in: fades.0,
        fade_out: fades.1,
    }
}

const NO_FADE: (u64, u64) = (0, 0);

fn cue(at: u64, len: u64, id: &str, section_type: SectionType, number: Option<u32>) -> CueEvent {
    CueEvent {
        at,
        section_id: id.to_owned(),
        section_type,
        number,
        len,
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

fn crossfade(seam_crossfade: u64) -> CompileOptions {
    CompileOptions {
        seam_crossfade,
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

fn has_accent_at(plan: &RenderPlan, at: u64) -> bool {
    plan.clicks
        .iter()
        .any(|c| c.at == at && c.level == ClickLevel::Downbeat)
}

const BEATS: [ClickLevel; 2] = [ClickLevel::Downbeat, ClickLevel::Beat];

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

/// Fades exactly at jumps, per the S2 overlap rule.
fn assert_fades(song: &Song, plan: &RenderPlan, seam_crossfade: u64) {
    let segments = &plan.segments;
    assert_eq!(segments[0].fade_in, 0, "the first segment does not fade in");
    assert_eq!(
        segments.last().unwrap().fade_out,
        0,
        "the last segment does not fade out"
    );
    for pair in segments.windows(2) {
        let (outgoing, incoming) = (&pair[0], &pair[1]);
        let outgoing_end = outgoing.src_start + outgoing.len;
        let expected = if outgoing_end == incoming.src_start {
            0
        } else {
            seam_crossfade
                .min(incoming.len)
                .min(song.length() - outgoing_end)
        };
        assert_eq!(
            incoming.fade_in, expected,
            "fade-in of {}",
            incoming.section_id
        );
        assert_eq!(
            outgoing.fade_out, incoming.fade_in,
            "fades match at the seam"
        );
    }
}

/// Kept cues never overlap and never announce a section shorter than the cue.
fn assert_cues(song: &Song, plan: &RenderPlan) {
    for pair in plan.cues.windows(2) {
        assert!(
            pair[0].at + pair[0].len <= pair[1].at,
            "cues overlap: {:?}",
            plan.cues
        );
    }
    for cue in &plan.cues {
        assert!(song.section(&cue.section_id).unwrap().length() >= cue.len);
        assert!(cue.at < plan.length);
    }
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
            seg("intro", 0, 0, 192_000, NO_FADE),
            seg("verse", 192_000, 192_000, 384_000, NO_FADE),
            seg("chorus", 576_000, 576_000, 384_000, (0, 256)),
            seg("chorus", 960_000, 576_000, 384_000, (256, 0)),
            seg("outro", 1_344_000, 960_000, 192_000, NO_FADE),
        ]
    );
    assert_eq!(plan.segments[3].src_start, plan.segments[2].src_start);
    assert_eq!(plan.length, 1_536_000);
    assert_contiguous(&plan);
    assert_fades(&song, &plan, DEFAULT_SEAM_CROSSFADE);

    // No gap in the click either: every bar-aligned section keeps the grid.
    assert_eq!(plan.clicks.len(), 1_536_000 / 6_000);
    assert!(
        plan.clicks
            .iter()
            .enumerate()
            .all(|(i, c)| c.at == i as u64 * 6_000)
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Downbeat]),
        (0..16).map(|bar| bar * 96_000).collect::<Vec<_>>()
    );
    assert!(has_accent_at(&plan, plan.segments[3].out_start));
}

#[test]
fn default_arrangement_plays_the_song_straight_through() {
    let song = four_section_spec().build();
    let plan = plan(&song, "default");
    assert_eq!(
        plan.segments,
        [
            seg("intro", 0, 0, 192_000, NO_FADE),
            seg("verse", 192_000, 192_000, 384_000, NO_FADE),
            seg("chorus", 576_000, 576_000, 384_000, NO_FADE),
            seg("outro", 960_000, 960_000, 192_000, NO_FADE),
        ]
    );
    assert_eq!(plan.length, song.length());
}

#[test]
fn jump_seams_fade_out_the_outgoing_and_fade_in_the_incoming_segment() {
    let song = four_section_spec()
        .arrangement("back", &["chorus", "intro", "verse"])
        .build();
    assert_eq!(
        plan_with(&song, "back", crossfade(1_024)).segments,
        [
            seg("chorus", 0, 576_000, 384_000, (0, 1_024)),
            seg("intro", 384_000, 0, 192_000, (1_024, 0)),
            seg("verse", 576_000, 192_000, 384_000, NO_FADE),
        ]
    );
    let no_fade = plan_with(&song, "back", crossfade(0));
    assert!(
        no_fade
            .segments
            .iter()
            .all(|s| s.fade_in == 0 && s.fade_out == 0)
    );
}

#[test]
fn seam_fade_is_capped_by_the_incoming_length_and_the_source_left() {
    let song = four_section_spec()
        .arrangement("into_chorus", &["intro", "chorus"])
        .arrangement("into_verse", &["chorus", "verse"])
        .arrangement("from_the_end", &["outro", "verse"])
        .build();
    let huge = crossfade(u64::MAX);

    // Intro ends at 192 000, so 960 000 samples are left; the chorus is 384 000.
    let plan = plan_with(&song, "into_chorus", huge);
    assert_eq!(
        plan.segments,
        [
            seg("intro", 0, 0, 192_000, (0, 384_000)),
            seg("chorus", 192_000, 576_000, 384_000, (384_000, 0)),
        ]
    );
    // The chorus ends at 960 000: only 192 000 source samples are left.
    let plan = plan_with(&song, "into_verse", huge);
    assert_eq!(
        plan.segments,
        [
            seg("chorus", 0, 576_000, 384_000, (0, 192_000)),
            seg("verse", 384_000, 192_000, 384_000, (192_000, 0)),
        ]
    );
    // The outro ends at the song end: there is nothing left to fade out.
    let plan = plan_with(&song, "from_the_end", huge);
    assert!(
        plan.segments
            .iter()
            .all(|s| s.fade_in == 0 && s.fade_out == 0)
    );
}

#[test]
fn rejects_unknown_arrangement() {
    let song = four_section_spec().build();
    let error =
        compile(&song, "nope", &CompileOptions::default()).expect_err("unknown arrangement");
    assert_eq!(
        error,
        CompileError::UnknownArrangement { id: "nope".into() }
    );
    assert!(error.to_string().contains("\"nope\""));
}

#[test]
fn constant_tempo_clicks_land_on_exact_samples() {
    let song = SongSpec::new(192_000, vec![marker("all", SectionType::Verse, None, 0)]).build();
    let plan = plan(&song, "default");

    assert_eq!(plan.clicks.len(), 32);
    assert_clicks_sorted(&plan);
    assert_eq!(positions(&plan, &[ClickLevel::Downbeat]), [0, 96_000]);
    assert_eq!(
        positions(&plan, &[ClickLevel::Beat]),
        [24_000, 48_000, 72_000, 120_000, 144_000, 168_000]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::HalfBeat]),
        (0..8).map(|n| 12_000 + n * 24_000).collect::<Vec<_>>()
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::QuarterBeat]),
        (0..16).map(|n| 6_000 + n * 12_000).collect::<Vec<_>>()
    );
}

#[test]
fn subdivisions_split_each_beat_into_eighths_and_sixteenths() {
    let song = SongSpec::new(96_000, vec![marker("all", SectionType::Verse, None, 0)]).build();
    let plan = plan(&song, "default");
    let click = |at, level| ClickEvent { at, level };
    assert_eq!(
        plan.clicks[..9],
        [
            click(0, ClickLevel::Downbeat),
            click(6_000, ClickLevel::QuarterBeat),
            click(12_000, ClickLevel::HalfBeat),
            click(18_000, ClickLevel::QuarterBeat),
            click(24_000, ClickLevel::Beat),
            click(30_000, ClickLevel::QuarterBeat),
            click(36_000, ClickLevel::HalfBeat),
            click(42_000, ClickLevel::QuarterBeat),
            click(48_000, ClickLevel::Beat),
        ]
    );
    let count = |level| plan.clicks.iter().filter(|c| c.level == level).count();
    assert_eq!(count(ClickLevel::Downbeat), 1);
    assert_eq!(count(ClickLevel::Beat), 3);
    assert_eq!(count(ClickLevel::HalfBeat), 4);
    assert_eq!(count(ClickLevel::QuarterBeat), 8);
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
        positions(&plan, &[ClickLevel::Downbeat]),
        [0, 96_000, 224_000]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::HalfBeat])[4..6],
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
    assert_eq!(positions(&plan, &[ClickLevel::Downbeat]), [0, 156_000]);
    let first: Vec<(u64, ClickLevel)> = plan.clicks[..9].iter().map(|c| (c.at, c.level)).collect();
    assert_eq!(
        first,
        [
            (0, ClickLevel::Downbeat),
            (6_000, ClickLevel::QuarterBeat),
            (12_000, ClickLevel::HalfBeat),
            (18_000, ClickLevel::QuarterBeat),
            (24_000, ClickLevel::Beat),
            (30_000, ClickLevel::QuarterBeat),
            (36_000, ClickLevel::HalfBeat),
            (48_000, ClickLevel::QuarterBeat),
            (60_000, ClickLevel::Beat),
        ]
    );
}

#[test]
fn meter_change_restarts_bars_with_an_accent() {
    // Two bars of 4/4, then two bars of 3/4 (72 000 samples per bar).
    let song = SongSpec::new(336_000, vec![marker("all", SectionType::Verse, None, 0)])
        .meters(&[(0, 4, 4), (192_000, 3, 4)])
        .build();
    let plan = plan(&song, "default");
    assert_eq!(
        positions(&plan, &[ClickLevel::Downbeat]),
        [0, 96_000, 192_000, 264_000]
    );
    assert_eq!(
        positions(&plan, &[ClickLevel::Beat]),
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
            seg("b", 0, 30_000, 162_000, NO_FADE),
            seg("a", 162_000, 0, 30_000, NO_FADE),
        ],
        "b ends at the song end, so there is no source left to crossfade"
    );
    assert_eq!(
        swapped.clicks[..2],
        [
            ClickEvent {
                at: 0,
                level: ClickLevel::QuarterBeat
            },
            ClickEvent {
                at: 6_000,
                level: ClickLevel::HalfBeat
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
            cue(96_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(480_000, 96_000, "chorus", SectionType::Chorus, None),
        ]
    );
    assert_eq!(
        plan(&song, "cv").cues,
        [
            cue(96_000, 96_000, "chorus", SectionType::Chorus, None),
            cue(480_000, 96_000, "verse", SectionType::Verse, Some(1)),
        ]
    );
    assert_eq!(
        plan_with(&song, "cv", at_section_start()).cues,
        [
            cue(0, 96_000, "intro", SectionType::Intro, None),
            cue(192_000, 96_000, "chorus", SectionType::Chorus, None),
            cue(576_000, 96_000, "verse", SectionType::Verse, Some(1)),
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
            cue(96_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(480_000, 96_000, "chorus", SectionType::Chorus, None),
            cue(864_000, 96_000, "outro", SectionType::Outro, None),
        ]
    );
    assert_eq!(
        plan_with(&song, "default", at_section_start()).cues,
        [
            cue(0, 96_000, "intro", SectionType::Intro, None),
            cue(192_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(576_000, 96_000, "chorus", SectionType::Chorus, None),
            cue(960_000, 96_000, "outro", SectionType::Outro, None),
        ]
    );
}

#[test]
fn one_bar_before_is_the_preceding_output_bar() {
    // From the verse on the tempo is 60 bpm: one bar is 192 000 samples. The
    // verse cue still lasts one intro bar, the bar heard just before the verse.
    let slow = four_section_spec()
        .tempo(&[(0, 120.0), (192_000, 60.0)])
        .build();
    assert_eq!(
        plan(&slow, "default").cues,
        [
            cue(96_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(384_000, 192_000, "chorus", SectionType::Chorus, None),
            cue(768_000, 192_000, "outro", SectionType::Outro, None),
        ]
    );
    // The default cue length follows the same bar at section start too; the
    // first segment has no preceding bar and uses its own.
    assert_eq!(
        plan_with(&slow, "default", at_section_start()).cues,
        [
            cue(0, 96_000, "intro", SectionType::Intro, None),
            cue(192_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(576_000, 192_000, "chorus", SectionType::Chorus, None),
            cue(960_000, 192_000, "outro", SectionType::Outro, None),
        ]
    );

    // From the verse on the meter is 3/4: one bar is 72 000 samples.
    let waltz = four_section_spec()
        .meters(&[(0, 4, 4), (192_000, 3, 4)])
        .build();
    assert_eq!(
        plan(&waltz, "default").cues,
        [
            cue(96_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(504_000, 72_000, "chorus", SectionType::Chorus, None),
            cue(888_000, 72_000, "outro", SectionType::Outro, None),
        ]
    );
}

#[test]
fn cue_before_a_meter_change_starts_on_the_last_bar_line_of_the_verse() {
    // Verifier probe: verse 4/4 at 0, bridge 3/4 at 768 000.
    let song = SongSpec::new(
        768_000 + 8 * 72_000,
        vec![
            marker("verse", SectionType::Verse, Some(1), 0),
            marker("bridge", SectionType::Bridge, None, 768_000),
        ],
    )
    .meters(&[(0, 4, 4), (768_000, 3, 4)])
    .build();
    let plan = plan(&song, "default");
    assert_eq!(
        plan.cues,
        [cue(672_000, 96_000, "bridge", SectionType::Bridge, None)]
    );
    assert!(has_accent_at(&plan, 672_000));
}

#[test]
fn cue_before_a_tempo_change_starts_one_verse_bar_before_the_chorus() {
    // Verifier probe: 120 bpm, then 60 bpm from the chorus at 768 000.
    let song = SongSpec::new(
        768_000 + 8 * 192_000,
        vec![
            marker("verse", SectionType::Verse, Some(1), 0),
            marker("chorus", SectionType::Chorus, None, 768_000),
        ],
    )
    .tempo(&[(0, 120.0), (768_000, 60.0)])
    .build();
    let plan = plan(&song, "default");
    assert_eq!(
        plan.cues,
        [cue(672_000, 96_000, "chorus", SectionType::Chorus, None)]
    );
    assert!(has_accent_at(&plan, 672_000));
}

#[test]
fn cue_after_a_rearranged_section_uses_that_sections_last_bar() {
    // Source: chorus 4/4 (8 bars), bridge 3/4 (8 bars). Played bridge first,
    // the chorus cue starts on the last bridge bar: 576 000 - 72 000.
    let song = SongSpec::new(
        768_000 + 576_000,
        vec![
            marker("chorus", SectionType::Chorus, None, 0),
            marker("bridge", SectionType::Bridge, None, 768_000),
        ],
    )
    .meters(&[(0, 4, 4), (768_000, 3, 4)])
    .arrangement("bridge_first", &["bridge", "chorus"])
    .build();
    let plan = plan(&song, "bridge_first");
    assert_eq!(
        plan.cues,
        [cue(504_000, 72_000, "chorus", SectionType::Chorus, None)]
    );
    assert!(has_accent_at(&plan, 504_000));
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
    let at_start = CompileOptions {
        cue_timing: CueTiming::AtSectionStart,
        ..short_cues
    };
    assert_eq!(
        plan_with(&song, "default", at_start).cues,
        [
            cue(0, 24_000, "count", SectionType::CountOff, None),
            cue(48_000, 24_000, "verse", SectionType::Verse, Some(1)),
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
            cue(0, 96_000, "intro", SectionType::Intro, None),
            cue(240_000, 96_000, "chorus", SectionType::Chorus, None),
        ]
    );
    assert_eq!(
        plan(&song, "default").cues,
        [cue(144_000, 96_000, "chorus", SectionType::Chorus, None)]
    );
    let half_bar_cues = CompileOptions {
        cue_length: Some(48_000),
        ..at_section_start()
    };
    assert_eq!(
        plan_with(&song, "default", half_bar_cues).cues,
        [
            cue(0, 48_000, "intro", SectionType::Intro, None),
            cue(192_000, 48_000, "tag", SectionType::Tag, None),
            cue(240_000, 48_000, "chorus", SectionType::Chorus, None),
        ]
    );
}

#[test]
fn cue_that_overlaps_the_previous_cue_is_dropped() {
    // The tempo halves inside the verse, so the bar before the chorus lasts
    // 192 000 samples and the chorus cue would start inside the verse cue.
    let song = SongSpec::new(
        500_000,
        vec![
            marker("intro", SectionType::Intro, None, 0),
            marker("verse", SectionType::Verse, Some(1), 192_000),
            marker("chorus", SectionType::Chorus, None, 300_000),
        ],
    )
    .tempo(&[(0, 120.0), (240_000, 60.0)])
    .build();
    let plan_one_bar_before = plan(&song, "default");
    assert_eq!(
        plan_one_bar_before.cues,
        [cue(96_000, 96_000, "verse", SectionType::Verse, Some(1))]
    );
    assert_eq!(
        plan_with(&song, "default", at_section_start()).cues,
        [
            cue(0, 96_000, "intro", SectionType::Intro, None),
            cue(192_000, 96_000, "verse", SectionType::Verse, Some(1)),
            cue(300_000, 192_000, "chorus", SectionType::Chorus, None),
        ]
    );
}

/// Deterministic xorshift so the property tests need no extra dependency.
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
                assert_fades(&song, &plan, DEFAULT_SEAM_CROSSFADE);
                assert_eq!(plan.segments.len(), sections.len());
                for (segment, section_id) in plan.segments.iter().zip(sections) {
                    let section = song.section(section_id).unwrap();
                    assert_eq!(segment.section_id, *section_id);
                    assert_eq!(
                        (segment.src_start, segment.len),
                        (section.start, section.length())
                    );
                }

                assert_clicks_sorted(&plan);
                assert!(plan.clicks.iter().all(|c| c.at < plan.length));
                // Bar-aligned sections at a constant tempo keep the grid.
                assert_eq!(plan.clicks.len() as u64, plan.length / 6_000);

                assert_cues(&song, &plan);
                let reserved = cue_length.unwrap_or(96_000);
                assert!(plan.cues.iter().all(|c| c.len == reserved));
            }
        }
    }
}

#[test]
fn random_arrangements_over_tempo_and_meter_changes_keep_every_invariant() {
    // Verifier property test: sections off the bar grid, tempo and meter changes.
    let bar = 96_000u64;
    let length = 14 * bar;
    let base = SongSpec::new(
        length,
        vec![
            marker("a", SectionType::Intro, None, 0),
            marker("b", SectionType::Verse, Some(1), 2 * bar + 13),
            marker("c", SectionType::Chorus, Some(1), 2 * bar + 40_000),
            marker("d", SectionType::Bridge, None, 7 * bar),
            marker("e", SectionType::Outro, None, 11 * bar + 99),
        ],
    )
    .tempo(&[(0, 120.0), (6 * bar + 777, 87.25)])
    .meters(&[(0, 4, 4), (3 * bar, 3, 4), (9 * bar + 5, 6, 8)])
    .package();
    let ids = ["a", "b", "c", "d", "e"];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for case in 0..300 {
        let count = 1 + rng.next(9) as usize;
        let order: Vec<&str> = (0..count).map(|_| ids[rng.next(5) as usize]).collect();
        let mut package = base.clone();
        package.arrangements.push(common::arrangement("x", &order));
        let song = validate(&package, &[info("drums", RATE, length)])
            .unwrap()
            .song;
        for cue_timing in [CueTiming::OneBarBefore, CueTiming::AtSectionStart] {
            for cue_length in [None, Some(30_000 + rng.next(150_000))] {
                let options = CompileOptions {
                    cue_timing,
                    cue_length,
                    ..CompileOptions::default()
                };
                let plan = plan_with(&song, "x", options);
                assert_contiguous(&plan);
                assert_fades(&song, &plan, DEFAULT_SEAM_CROSSFADE);
                for (segment, section_id) in plan.segments.iter().zip(&order) {
                    let section = song.section(section_id).unwrap();
                    assert_eq!(
                        (segment.src_start, segment.len),
                        (section.start, section.length()),
                        "case {case} {order:?}"
                    );
                }
                assert_cues(&song, &plan);
                if let Some(reserved) = cue_length {
                    assert!(plan.cues.iter().all(|c| c.len == reserved), "case {case}");
                }
                assert_clicks_sorted(&plan);
                assert!(plan.clicks.iter().all(|c| c.at < plan.length));
            }
        }
    }
}

#[test]
fn tempo_and_meter_points_far_beyond_the_song_end_do_not_panic() {
    let song = SongSpec::new(480_000, vec![marker("all", SectionType::Intro, None, 0)])
        .tempo(&[(0, 120.0), (u64::MAX - 1, 999.0), (u64::MAX, 1.0)])
        .meters(&[(0, 4, 4), (u64::MAX - 1, 3, 4), (u64::MAX, 7, 8)])
        .build();
    let plan = plan(&song, "default");
    assert_eq!(plan.clicks.len(), 5 * 16, "five bars of 4/4 at 120 bpm");
    assert_eq!(positions(&plan, &[ClickLevel::Downbeat]).len(), 5);
}
