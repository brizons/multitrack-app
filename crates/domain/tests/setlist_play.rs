//! S3: the songs a setlist item plays. A song item with start and end points
//! plays an excerpt of its arrangement; a click-only tile plays a song with
//! no stems whose generated click is the tile's tempo and meter.

mod common;

use common::{RATE, four_section_spec};
use domain::{
    ClickLevel, CompileOptions, DEFAULT_SEAM_CROSSFADE, MAX_SONG_HOURS, SectionType, Song,
    TimeSignature, compile,
};

fn song() -> Song {
    four_section_spec()
        .arrangement("repeat", &["intro", "verse", "chorus", "chorus", "outro"])
        .build()
}

fn four_four() -> TimeSignature {
    TimeSignature {
        numerator: 4,
        denominator: 4,
    }
}

#[test]
fn an_excerpt_plays_only_the_positions_from_start_to_end() {
    let song = song();
    let excerpt = song
        .excerpt("repeat", 1, 3)
        .expect("the range is inside the arrangement");

    assert_eq!(excerpt.arrangements().len(), 1, "only the excerpt is left");
    let arrangement = &excerpt.arrangements()[0];
    assert_eq!(arrangement.id, "repeat");
    assert_eq!(arrangement.sections, ["verse", "chorus", "chorus"]);
    assert_eq!(excerpt.sections(), song.sections());
    assert_eq!(excerpt.length(), song.length());
    assert_eq!(excerpt.tempo_map(), song.tempo_map());
    assert_eq!(excerpt.stems(), song.stems());

    let plan = compile(&excerpt, "repeat", &CompileOptions::default()).unwrap();
    let full = compile(&song, "repeat", &CompileOptions::default()).unwrap();
    let starts: Vec<(u64, u64)> = plan
        .segments
        .iter()
        .map(|segment| (segment.src_start, segment.len))
        .collect();
    let expected: Vec<(u64, u64)> = full.segments[1..=3]
        .iter()
        .map(|segment| (segment.src_start, segment.len))
        .collect();
    assert_eq!(starts, expected, "the same source ranges, in order");
    assert_eq!(plan.segments[0].out_start, 0, "the excerpt starts at once");
    assert_eq!(plan.segments[0].fade_in, 0, "nothing plays before it");
    assert_eq!(
        plan.segments[2].fade_in, DEFAULT_SEAM_CROSSFADE,
        "the chorus repeat is still a seam"
    );
    assert_eq!(plan.segments[2].fade_out, 0, "nothing plays after the end");
    assert_eq!(plan.length, 192_000 * 2 + 384_000 * 2);
}

#[test]
fn the_whole_range_is_the_arrangement_itself() {
    let song = song();
    let excerpt = song.excerpt("repeat", 0, 4).unwrap();
    assert_eq!(
        compile(&excerpt, "repeat", &CompileOptions::default()),
        compile(&song, "repeat", &CompileOptions::default())
    );
}

#[test]
fn an_excerpt_outside_the_arrangement_is_refused() {
    let song = song();
    assert!(
        song.excerpt("acoustic", 0, 0).is_none(),
        "unknown arrangement"
    );
    assert!(song.excerpt("repeat", 3, 2).is_none(), "start after end");
    assert!(
        song.excerpt("repeat", 0, 5).is_none(),
        "end past the last position"
    );
}

#[test]
fn a_click_tile_with_a_bar_count_lasts_that_many_bars_of_its_meter() {
    let song = Song::click_only(RATE, 120.0, four_four(), Some(3)).expect("a valid tile");
    // 120 bpm at 48 kHz: a beat is 24 000 samples, a 4/4 bar 96 000.
    assert_eq!(song.length(), 3 * 96_000);
    assert_eq!(song.sample_rate(), RATE);
    assert!(song.stems().is_empty(), "the click is generated");
    assert_eq!(song.sections().len(), 1);
    assert_eq!(song.sections()[0].section_type, SectionType::CountOff);
    assert_eq!(song.sections()[0].end, song.length());
    assert_eq!(song.arrangements().len(), 1);

    let plan = compile(
        &song,
        &song.arrangements()[0].id,
        &CompileOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.length, 3 * 96_000);
    let at = |level| -> Vec<u64> {
        plan.clicks
            .iter()
            .filter(|click| click.level == level)
            .map(|click| click.at)
            .collect()
    };
    assert_eq!(at(ClickLevel::Downbeat), [0, 96_000, 192_000]);
    let beats = at(ClickLevel::Beat);
    assert_eq!(beats.len(), 9, "three beats after each downbeat");
    assert!(beats.iter().all(|at| at % 24_000 == 0));
}

#[test]
fn a_click_tile_counts_the_time_signature_denominator_note() {
    let six_eight = TimeSignature {
        numerator: 6,
        denominator: 8,
    };
    // 72 eighth notes a minute: an eighth is 40 000 samples, a bar 240 000.
    let song = Song::click_only(RATE, 72.0, six_eight, Some(2)).unwrap();
    assert_eq!(song.length(), 480_000);
    assert_eq!(song.time_signature_map()[0].numerator, 6);
    assert_eq!(song.time_signature_map()[0].denominator, 8);
    assert_eq!(song.tempo_map()[0].bpm, 72.0);
}

#[test]
fn a_click_tile_without_a_bar_count_is_one_bar_to_loop() {
    let song = Song::click_only(RATE, 120.0, four_four(), None).unwrap();
    assert_eq!(song.length(), 96_000);
    assert_eq!(song.arrangements()[0].sections.len(), 1);
}

#[test]
fn a_click_tile_the_setlist_rules_refuse_builds_no_song() {
    let tile = |rate, bpm, numerator, denominator, bars| {
        Song::click_only(
            rate,
            bpm,
            TimeSignature {
                numerator,
                denominator,
            },
            bars,
        )
    };
    assert!(tile(RATE, 0.5, 4, 4, None).is_none(), "bpm below the range");
    assert!(
        tile(RATE, f64::NAN, 4, 4, None).is_none(),
        "bpm not a number"
    );
    assert!(tile(RATE, 120.0, 0, 4, None).is_none(), "no beats per bar");
    assert!(tile(RATE, 120.0, 4, 3, None).is_none(), "denominator 3");
    assert!(tile(RATE, 120.0, 4, 4, Some(0)).is_none(), "zero bars");
    assert!(tile(0, 120.0, 4, 4, None).is_none(), "no sample rate");
    // 24 hours at 120 bpm in 4/4 are 43 200 bars.
    let hours = u32::try_from(MAX_SONG_HOURS).unwrap();
    assert!(tile(RATE, 120.0, 4, 4, Some(1_800 * hours)).is_some());
    assert!(
        tile(RATE, 120.0, 4, 4, Some(1_800 * hours + 1)).is_none(),
        "longer than a song may be"
    );
}
