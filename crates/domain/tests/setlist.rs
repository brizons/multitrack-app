//! S1: setlist model, its JSON file, validation, and editing.

mod common;

use std::collections::{BTreeMap, HashMap};

use common::four_section_spec;
use domain::{
    ArrangementOutline, ClickItem, PositionBound, SETLIST_FORMAT_VERSION, Setlist,
    SetlistEditError, SetlistFileError, SetlistItem, SetlistValidationError, SongItem, SongLookup,
    SongOutline, StemPathProblem, TimeSignature, Transition,
};

/// A setlist exercising every field of format v1.
const EXAMPLE: &str = r#"{
  "formatVersion": 1,
  "name": "Sunday Morning",
  "items": [
    {
      "type": "song",
      "song": "Example Song.song",
      "arrangement": "default",
      "startPosition": 1,
      "endPosition": 2,
      "fadeOut": true,
      "transition": "crossfade"
    },
    {
      "type": "click",
      "bpm": 72.0,
      "timeSignature": { "numerator": 6, "denominator": 8 },
      "bars": 4,
      "transition": "autoLink"
    },
    {
      "type": "song",
      "song": "Hymns/Another Song.song",
      "arrangement": "acoustic",
      "fadeOut": false,
      "transition": "cueNext"
    }
  ]
}"#;

fn song_item(song: &str, arrangement: &str) -> SongItem {
    SongItem {
        arrangement: arrangement.to_owned(),
        ..SongItem::new(song)
    }
}

fn click_item(bpm: f64, numerator: u32, denominator: u32) -> ClickItem {
    ClickItem::new(
        bpm,
        TimeSignature {
            numerator,
            denominator,
        },
    )
}

fn example() -> Setlist {
    Setlist {
        format_version: 1,
        name: "Sunday Morning".to_owned(),
        items: vec![
            SetlistItem::Song(SongItem {
                start_position: Some(1),
                end_position: Some(2),
                fade_out: true,
                transition: Transition::Crossfade,
                ..song_item("Example Song.song", "default")
            }),
            SetlistItem::Click(ClickItem {
                bars: Some(4),
                transition: Transition::AutoLink,
                ..click_item(72.0, 6, 8)
            }),
            SetlistItem::Song(song_item("Hymns/Another Song.song", "acoustic")),
        ],
    }
}

fn outline(arrangements: &[(&str, usize)]) -> SongOutline {
    SongOutline {
        arrangements: arrangements
            .iter()
            .map(|&(id, positions)| ArrangementOutline {
                id: id.to_owned(),
                positions,
            })
            .collect(),
    }
}

/// The library the example setlist refers to.
fn library() -> HashMap<String, SongOutline> {
    HashMap::from([
        (
            "Example Song.song".to_owned(),
            outline(&[("default", 4), ("short", 2)]),
        ),
        (
            "Hymns/Another Song.song".to_owned(),
            outline(&[("default", 3), ("acoustic", 2)]),
        ),
    ])
}

fn errors_of(setlist: &Setlist, songs: &impl SongLookup) -> Vec<SetlistValidationError> {
    setlist
        .validate(songs)
        .expect_err("setlist must be rejected")
}

fn assert_message_names(error: &impl std::fmt::Display, parts: &[&str]) {
    let message = error.to_string();
    for part in parts {
        assert!(
            message.contains(part),
            "message {message:?} does not name {part:?}"
        );
    }
}

fn three_songs() -> Setlist {
    let mut setlist = Setlist::new("Set");
    setlist.items = ["a.song", "b.song", "c.song"]
        .iter()
        .map(|song| SetlistItem::Song(SongItem::new(*song)))
        .collect();
    setlist
}

fn songs_of(setlist: &Setlist) -> Vec<&str> {
    setlist
        .items
        .iter()
        .map(|item| match item {
            SetlistItem::Song(song) => song.song.as_str(),
            SetlistItem::Click(_) => "click",
        })
        .collect()
}

// --- JSON file -------------------------------------------------------------

#[test]
fn example_round_trips_through_json() {
    let parsed = Setlist::from_json(EXAMPLE).expect("example parses");
    assert_eq!(parsed, example());

    let json = parsed.to_json().expect("serializes");
    assert_eq!(Setlist::from_json(&json).expect("reparses"), parsed);
    let written: serde_json::Value = serde_json::from_str(&json).unwrap();
    let original: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
    assert_eq!(written, original, "the written file keeps every field");
}

#[test]
fn optional_fields_may_be_omitted() {
    let json = r#"{
      "formatVersion": 1,
      "name": "Minimal",
      "items": [
        { "type": "song", "song": "a.song", "arrangement": "default" },
        { "type": "click", "bpm": 120, "timeSignature": { "numerator": 4, "denominator": 4 } }
      ]
    }"#;
    let setlist = Setlist::from_json(json).expect("parses");
    assert_eq!(
        setlist.items,
        [
            SetlistItem::Song(SongItem::new("a.song")),
            SetlistItem::Click(click_item(120.0, 4, 4)),
        ]
    );
    let SetlistItem::Song(song) = &setlist.items[0] else {
        unreachable!()
    };
    assert_eq!(song.arrangement, "default");
    assert_eq!((song.start_position, song.end_position), (None, None));
    assert!(!song.fade_out);
    assert_eq!(song.transition, Transition::CueNext);
    let SetlistItem::Click(click) = &setlist.items[1] else {
        unreachable!()
    };
    assert_eq!(click.bars, None);
    assert_eq!(click.transition, Transition::CueNext);
}

#[test]
fn transitions_are_stored_in_camel_case() {
    for (transition, name) in [
        (Transition::CueNext, "cueNext"),
        (Transition::Stay, "stay"),
        (Transition::AutoLink, "autoLink"),
        (Transition::Crossfade, "crossfade"),
    ] {
        let mut setlist = Setlist::new("Set");
        setlist.items = vec![SetlistItem::Song(SongItem {
            transition,
            ..SongItem::new("a.song")
        })];
        let json = setlist.to_json().expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["items"][0]["transition"], name);
        assert_eq!(Setlist::from_json(&json).unwrap(), setlist);
    }
}

#[test]
fn rejects_unsupported_format_version_before_the_rest_of_the_shape() {
    let error = Setlist::from_json(r#"{ "formatVersion": 2, "items": 7 }"#)
        .expect_err("version 2 is rejected");
    assert!(
        matches!(
            error,
            SetlistFileError::UnsupportedFormatVersion { found: 2 }
        ),
        "unexpected error {error:?}"
    );
    assert_message_names(&error, &["setlist", "formatVersion 2", "formatVersion 1"]);
    assert_eq!(SETLIST_FORMAT_VERSION, 1);
}

#[test]
fn rejects_malformed_json_and_wrong_shapes() {
    for json in [
        "not json",
        r#"{ "name": "No version", "items": [] }"#,
        r#"{ "formatVersion": 1, "name": "Set", "items": [{ "type": "pad" }] }"#,
        r#"{ "formatVersion": 1, "name": "Set", "items": [{ "type": "song" }] }"#,
    ] {
        let error = Setlist::from_json(json).expect_err("malformed setlist is rejected");
        assert!(
            matches!(error, SetlistFileError::Json(_)),
            "{json}: {error:?}"
        );
        assert!(error.to_string().starts_with("setlist: "), "{error}");
    }
}

// --- Validation ------------------------------------------------------------

#[test]
fn a_new_setlist_is_empty_and_valid() {
    let setlist = Setlist::new("Sunday Morning");
    assert_eq!(setlist.format_version, SETLIST_FORMAT_VERSION);
    assert_eq!(setlist.name, "Sunday Morning");
    assert!(setlist.items.is_empty());
    assert_eq!(setlist.validate(&HashMap::new()), Ok(()));
}

#[test]
fn the_example_is_valid_against_its_library() {
    assert_eq!(example().validate(&library()), Ok(()));
    let ordered: BTreeMap<String, SongOutline> = library().into_iter().collect();
    assert_eq!(
        example().validate(&ordered),
        Ok(()),
        "a BTreeMap is a lookup"
    );
}

#[test]
fn rejects_unknown_song() {
    let mut setlist = Setlist::new("Set");
    setlist.items = vec![SetlistItem::Song(SongItem::new("Missing.song"))];
    let errors = errors_of(&setlist, &library());
    assert_eq!(
        errors,
        [SetlistValidationError::UnknownSong {
            item: 0,
            song: "Missing.song".into()
        }]
    );
    assert_message_names(&errors[0], &["item 1", "\"Missing.song\"", "library"]);
}

#[test]
fn rejects_unknown_arrangement() {
    let mut setlist = Setlist::new("Set");
    setlist.items = vec![SetlistItem::Song(song_item("Example Song.song", "live"))];
    let errors = errors_of(&setlist, &library());
    assert_eq!(
        errors,
        [SetlistValidationError::UnknownArrangement {
            item: 0,
            song: "Example Song.song".into(),
            arrangement: "live".into()
        }]
    );
    assert_message_names(&errors[0], &["item 1", "\"live\"", "\"Example Song.song\""]);
}

#[test]
fn rejects_start_after_end() {
    let mut setlist = Setlist::new("Set");
    setlist.items = vec![SetlistItem::Song(SongItem {
        start_position: Some(3),
        end_position: Some(1),
        ..song_item("Example Song.song", "default")
    })];
    let errors = errors_of(&setlist, &library());
    assert_eq!(
        errors,
        [SetlistValidationError::StartAfterEnd {
            item: 0,
            start: 3,
            end: 1
        }]
    );
    assert_message_names(
        &errors[0],
        &["item 1", "start position 3", "end position 1"],
    );

    for (start, end) in [(Some(2), Some(2)), (Some(3), None), (None, Some(0))] {
        setlist.items = vec![SetlistItem::Song(SongItem {
            start_position: start,
            end_position: end,
            ..song_item("Example Song.song", "default")
        })];
        assert_eq!(setlist.validate(&library()), Ok(()), "{start:?}..={end:?}");
    }
}

#[test]
fn rejects_positions_outside_the_arrangement() {
    let mut setlist = Setlist::new("Set");
    setlist.items = vec![
        SetlistItem::Song(SongItem {
            start_position: Some(4),
            end_position: Some(9),
            ..song_item("Example Song.song", "default")
        }),
        SetlistItem::Song(SongItem {
            end_position: Some(2),
            ..song_item("Example Song.song", "short")
        }),
    ];
    let errors = errors_of(&setlist, &library());
    assert_eq!(
        errors,
        [
            SetlistValidationError::PositionOutsideArrangement {
                item: 0,
                bound: PositionBound::Start,
                position: 4,
                song: "Example Song.song".into(),
                arrangement: "default".into(),
                positions: 4,
            },
            SetlistValidationError::PositionOutsideArrangement {
                item: 0,
                bound: PositionBound::End,
                position: 9,
                song: "Example Song.song".into(),
                arrangement: "default".into(),
                positions: 4,
            },
            SetlistValidationError::PositionOutsideArrangement {
                item: 1,
                bound: PositionBound::End,
                position: 2,
                song: "Example Song.song".into(),
                arrangement: "short".into(),
                positions: 2,
            },
        ]
    );
    assert_message_names(
        &errors[0],
        &["item 1", "start position 4", "\"default\"", "4 positions"],
    );
    assert_message_names(&errors[2], &["item 2", "end position 2", "\"short\""]);

    setlist.items = vec![SetlistItem::Song(SongItem {
        start_position: Some(3),
        end_position: Some(3),
        ..song_item("Example Song.song", "default")
    })];
    assert_eq!(
        setlist.validate(&library()),
        Ok(()),
        "the last position is inside"
    );
}

#[test]
fn rejects_invalid_click_tiles() {
    let bpms = [0.0, -10.0, f64::NAN, f64::INFINITY, 1_000.0, 0.5];
    let mut setlist = Setlist::new("Set");
    setlist.items = bpms
        .iter()
        .map(|&bpm| SetlistItem::Click(click_item(bpm, 4, 4)))
        .collect();
    let errors = errors_of(&setlist, &library());
    assert_eq!(errors.len(), bpms.len());
    for (i, error) in errors.iter().enumerate() {
        assert!(
            matches!(error, SetlistValidationError::InvalidClickBpm { item, bpm }
                if *item == i && bpm.to_bits() == bpms[i].to_bits()),
            "unexpected error {error:?}"
        );
    }
    assert_message_names(&errors[1], &["item 2", "click-only", "-10", "1", "999"]);

    setlist.items = vec![
        SetlistItem::Click(click_item(120.0, 0, 4)),
        SetlistItem::Click(click_item(120.0, 4, 3)),
        SetlistItem::Click(click_item(120.0, 0, 64)),
        SetlistItem::Click(ClickItem {
            bars: Some(0),
            ..click_item(120.0, 4, 4)
        }),
    ];
    let errors = errors_of(&setlist, &library());
    assert_eq!(
        errors,
        [
            SetlistValidationError::InvalidClickNumerator { item: 0 },
            SetlistValidationError::InvalidClickDenominator {
                item: 1,
                denominator: 3
            },
            SetlistValidationError::InvalidClickNumerator { item: 2 },
            SetlistValidationError::InvalidClickDenominator {
                item: 2,
                denominator: 64
            },
            SetlistValidationError::ZeroClickBars { item: 3 },
        ]
    );
    assert_message_names(&errors[0], &["item 1", "numerator 0"]);
    assert_message_names(
        &errors[1],
        &["item 2", "denominator 3", "1, 2, 4, 8, 16, or 32"],
    );
    assert_message_names(&errors[4], &["item 4", "0 bars"]);

    let mut valid: Vec<SetlistItem> = [1, 2, 4, 8, 16, 32]
        .iter()
        .map(|&denominator| SetlistItem::Click(click_item(999.0, 7, denominator)))
        .collect();
    valid.push(SetlistItem::Click(ClickItem {
        bars: Some(1),
        ..click_item(1.0, 1, 4)
    }));
    setlist.items = valid;
    assert_eq!(
        setlist.validate(&library()),
        Ok(()),
        "range bounds are valid"
    );
}

#[test]
fn rejects_song_paths_outside_the_library_with_the_stem_path_rules() {
    let cases = [
        ("", StemPathProblem::Empty),
        ("/Songs/a.song", StemPathProblem::Absolute),
        (r"\\server\share\a.song", StemPathProblem::Absolute),
        (r"C:\Songs\a.song", StemPathProblem::Absolute),
        ("c:a.song", StemPathProblem::Absolute),
        ("../a.song", StemPathProblem::ParentDirectory),
        ("Hymns/../../a.song", StemPathProblem::ParentDirectory),
        (r"Hymns\..\..\a.song", StemPathProblem::ParentDirectory),
        ("Hymns/..", StemPathProblem::ParentDirectory),
    ];
    let mut setlist = Setlist::new("Set");
    setlist.items = cases
        .iter()
        .map(|&(song, _)| SetlistItem::Song(SongItem::new(song)))
        .collect();
    // The lookup knows every path, so only the path rule can reject them.
    let everything: HashMap<String, SongOutline> = cases
        .iter()
        .map(|&(song, _)| (song.to_owned(), outline(&[("default", 1)])))
        .collect();
    let errors = errors_of(&setlist, &everything);
    let expected: Vec<_> = cases
        .iter()
        .enumerate()
        .map(
            |(item, &(song, problem))| SetlistValidationError::InvalidSongPath {
                item,
                song: song.into(),
                problem,
            },
        )
        .collect();
    assert_eq!(errors, expected);
    assert_message_names(
        &errors[1],
        &["item 2", "/Songs/a.song", "absolute", "library folder"],
    );
    assert_message_names(&errors[5], &["item 6", "..", "library folder"]);
    assert_message_names(&errors[0], &["item 1", "empty"]);

    for song in [
        "a.song",
        "./Hymns/a.song",
        r"Hymns\a.song",
        "Hymns/..a.song",
        "a..b.song",
    ] {
        setlist.items = vec![SetlistItem::Song(SongItem::new(song))];
        let songs = HashMap::from([(song.to_owned(), outline(&[("default", 1)]))]);
        assert_eq!(
            setlist.validate(&songs),
            Ok(()),
            "{song} is inside the library"
        );
    }
}

#[test]
fn a_missing_song_does_not_hide_problems_in_other_items() {
    let mut setlist = example();
    setlist.format_version = 3;
    setlist.name = "  ".into();
    setlist.items = vec![
        SetlistItem::Song(SongItem {
            start_position: Some(2),
            end_position: Some(1),
            ..SongItem::new("Missing.song")
        }),
        SetlistItem::Click(click_item(0.0, 4, 4)),
        SetlistItem::Song(song_item("Example Song.song", "live")),
        SetlistItem::Song(SongItem::new("../Outside.song")),
        SetlistItem::Song(SongItem {
            start_position: Some(7),
            ..song_item("Hymns/Another Song.song", "acoustic")
        }),
        SetlistItem::Song(SongItem::new("Example Song.song")),
    ];
    let errors = errors_of(&setlist, &library());
    assert_eq!(
        errors,
        [
            SetlistValidationError::UnsupportedFormatVersion { found: 3 },
            SetlistValidationError::EmptyName,
            SetlistValidationError::UnknownSong {
                item: 0,
                song: "Missing.song".into()
            },
            SetlistValidationError::StartAfterEnd {
                item: 0,
                start: 2,
                end: 1
            },
            SetlistValidationError::InvalidClickBpm { item: 1, bpm: 0.0 },
            SetlistValidationError::UnknownArrangement {
                item: 2,
                song: "Example Song.song".into(),
                arrangement: "live".into()
            },
            SetlistValidationError::InvalidSongPath {
                item: 3,
                song: "../Outside.song".into(),
                problem: StemPathProblem::ParentDirectory
            },
            SetlistValidationError::PositionOutsideArrangement {
                item: 4,
                bound: PositionBound::Start,
                position: 7,
                song: "Hymns/Another Song.song".into(),
                arrangement: "acoustic".into(),
                positions: 2,
            },
        ]
    );
    assert_message_names(&errors[0], &["formatVersion 3", "formatVersion 1"]);
    assert_message_names(&errors[1], &["name", "empty"]);
}

#[test]
fn song_outline_counts_the_positions_of_each_arrangement() {
    let song = four_section_spec()
        .arrangement("short", &["verse", "chorus", "chorus"])
        .build();
    let outline = SongOutline::from(&song);
    assert_eq!(
        outline.arrangements,
        [
            ArrangementOutline {
                id: "default".into(),
                positions: 4
            },
            ArrangementOutline {
                id: "short".into(),
                positions: 3
            },
        ]
    );
    assert_eq!(outline.positions("short"), Some(3));
    assert_eq!(outline.positions("live"), None);
}

// --- Editing ---------------------------------------------------------------

#[test]
fn rename_changes_the_name_and_rejects_blank_names() {
    let mut setlist = example();
    setlist.rename("Sunday Evening").expect("renames");
    assert_eq!(setlist.name, "Sunday Evening");
    assert_eq!(setlist.items, example().items, "items are untouched");

    for blank in ["", "   "] {
        let error = setlist.rename(blank).expect_err("blank name is rejected");
        assert_eq!(error, SetlistEditError::EmptyName);
        assert_eq!(setlist.name, "Sunday Evening", "the name is kept");
    }
    assert_message_names(&SetlistEditError::EmptyName, &["name", "empty"]);
}

#[test]
fn duplicate_copies_the_setlist_under_a_distinct_name() {
    let original = example();
    let copy = original
        .duplicate("Sunday Morning (2)")
        .expect("duplicates");
    assert_eq!(copy.name, "Sunday Morning (2)");
    assert_eq!(copy.items, original.items);
    assert_eq!(copy.format_version, original.format_version);
    assert_eq!(original, example(), "the original is untouched");

    let error = original
        .duplicate("Sunday Morning")
        .expect_err("the same name is rejected");
    assert_eq!(
        error,
        SetlistEditError::SameName {
            name: "Sunday Morning".into()
        }
    );
    assert_message_names(&error, &["\"Sunday Morning\""]);
    assert_eq!(
        original.duplicate(" "),
        Err(SetlistEditError::EmptyName),
        "a blank name is rejected"
    );
}

#[test]
fn move_item_reorders_and_rejects_out_of_range_indices() {
    let mut setlist = three_songs();
    setlist.move_item(0, 2).expect("moves forward");
    assert_eq!(songs_of(&setlist), ["b.song", "c.song", "a.song"]);
    setlist.move_item(2, 0).expect("moves backward");
    assert_eq!(songs_of(&setlist), ["a.song", "b.song", "c.song"]);
    setlist.move_item(1, 1).expect("moving in place is a no-op");
    assert_eq!(songs_of(&setlist), ["a.song", "b.song", "c.song"]);

    assert_eq!(
        setlist.move_item(3, 0),
        Err(SetlistEditError::IndexOutOfRange { index: 3, len: 3 })
    );
    assert_eq!(
        setlist.move_item(0, 3),
        Err(SetlistEditError::IndexOutOfRange { index: 3, len: 3 })
    );
    assert_eq!(
        songs_of(&setlist),
        ["a.song", "b.song", "c.song"],
        "unchanged"
    );
    assert_eq!(
        Setlist::new("Empty").move_item(0, 0),
        Err(SetlistEditError::IndexOutOfRange { index: 0, len: 0 })
    );
    assert_message_names(
        &SetlistEditError::IndexOutOfRange { index: 3, len: 3 },
        &["index 3", "3 items"],
    );
}

#[test]
fn insert_item_accepts_any_index_up_to_the_end() {
    let mut setlist = three_songs();
    setlist
        .insert_item(1, SetlistItem::Click(click_item(90.0, 3, 4)))
        .expect("inserts in the middle");
    setlist
        .insert_item(4, SetlistItem::Song(SongItem::new("d.song")))
        .expect("appends at the end");
    setlist
        .insert_item(0, SetlistItem::Song(SongItem::new("z.song")))
        .expect("inserts at the start");
    assert_eq!(
        songs_of(&setlist),
        ["z.song", "a.song", "click", "b.song", "c.song", "d.song"]
    );

    assert_eq!(
        setlist.insert_item(7, SetlistItem::Song(SongItem::new("x.song"))),
        Err(SetlistEditError::IndexOutOfRange { index: 7, len: 6 })
    );
    assert_eq!(setlist.items.len(), 6, "unchanged");
}

#[test]
fn remove_item_returns_it_and_rejects_out_of_range_indices() {
    let mut setlist = three_songs();
    assert_eq!(
        setlist.remove_item(1),
        Ok(SetlistItem::Song(SongItem::new("b.song")))
    );
    assert_eq!(songs_of(&setlist), ["a.song", "c.song"]);
    assert_eq!(
        setlist.remove_item(2),
        Err(SetlistEditError::IndexOutOfRange { index: 2, len: 2 })
    );
    assert_eq!(songs_of(&setlist), ["a.song", "c.song"], "unchanged");
}

#[test]
fn set_arrangement_changes_it_and_clears_the_start_and_end() {
    let mut setlist = example();
    setlist
        .set_arrangement(0, "short")
        .expect("changes the arrangement");
    let SetlistItem::Song(song) = &setlist.items[0] else {
        unreachable!()
    };
    assert_eq!(song.arrangement, "short");
    assert_eq!(
        (song.start_position, song.end_position),
        (None, None),
        "positions of the old arrangement are cleared"
    );
    assert!(song.fade_out, "the fade-out is kept");
    assert_eq!(
        song.transition,
        Transition::Crossfade,
        "the transition is kept"
    );

    assert_eq!(
        setlist.set_arrangement(1, "default"),
        Err(SetlistEditError::NotASong { index: 1 })
    );
    assert_eq!(
        setlist.set_arrangement(3, "default"),
        Err(SetlistEditError::IndexOutOfRange { index: 3, len: 3 })
    );
    assert_message_names(
        &SetlistEditError::NotASong { index: 1 },
        &["index 1", "click-only"],
    );
}

#[test]
fn set_start_end_changes_both_positions() {
    let mut setlist = example();
    setlist
        .set_start_end(2, Some(0), Some(1))
        .expect("sets both");
    let SetlistItem::Song(song) = &setlist.items[2] else {
        unreachable!()
    };
    assert_eq!((song.start_position, song.end_position), (Some(0), Some(1)));
    setlist.set_start_end(2, None, None).expect("clears both");
    let SetlistItem::Song(song) = &setlist.items[2] else {
        unreachable!()
    };
    assert_eq!((song.start_position, song.end_position), (None, None));

    assert_eq!(
        setlist.set_start_end(1, Some(0), None),
        Err(SetlistEditError::NotASong { index: 1 })
    );
    assert_eq!(
        setlist.set_start_end(9, Some(0), None),
        Err(SetlistEditError::IndexOutOfRange { index: 9, len: 3 })
    );
}

#[test]
fn set_fade_out_switches_the_five_second_fade() {
    let mut setlist = example();
    setlist.set_fade_out(0, false).expect("switches off");
    setlist.set_fade_out(2, true).expect("switches on");
    let fades: Vec<Option<bool>> = setlist
        .items
        .iter()
        .map(|item| match item {
            SetlistItem::Song(song) => Some(song.fade_out),
            SetlistItem::Click(_) => None,
        })
        .collect();
    assert_eq!(fades, [Some(false), None, Some(true)]);

    assert_eq!(
        setlist.set_fade_out(1, true),
        Err(SetlistEditError::NotASong { index: 1 })
    );
    assert_eq!(
        setlist.set_fade_out(3, true),
        Err(SetlistEditError::IndexOutOfRange { index: 3, len: 3 })
    );
}

#[test]
fn set_transition_works_for_songs_and_click_tiles() {
    let mut setlist = example();
    setlist.set_transition(0, Transition::Stay).expect("song");
    setlist
        .set_transition(1, Transition::Crossfade)
        .expect("click tile");
    let transitions: Vec<Transition> = setlist.items.iter().map(SetlistItem::transition).collect();
    assert_eq!(
        transitions,
        [Transition::Stay, Transition::Crossfade, Transition::CueNext]
    );
    assert_eq!(
        setlist.set_transition(3, Transition::AutoLink),
        Err(SetlistEditError::IndexOutOfRange { index: 3, len: 3 })
    );
}

#[test]
fn a_tempo_that_is_not_a_finite_number_is_refused_when_saving() {
    for bpm in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut setlist = Setlist::new("Sunday");
        setlist
            .insert_item(0, SetlistItem::Song(SongItem::new("a.song")))
            .unwrap();
        setlist
            .insert_item(1, SetlistItem::Click(click_item(bpm, 4, 4)))
            .unwrap();

        let error = setlist.to_json().unwrap_err();

        assert!(
            matches!(error, SetlistFileError::NonFiniteBpm { item: 1 }),
            "{bpm}: {error:?}"
        );
        assert_message_names(&error, &["item 2", "tempo"]);
    }
    // A finite tempo out of range still saves; validation reports it.
    let mut setlist = Setlist::new("Sunday");
    setlist
        .insert_item(0, SetlistItem::Click(click_item(5_000.0, 4, 4)))
        .unwrap();
    assert!(setlist.to_json().is_ok());
}
