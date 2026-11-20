//! S4 end to end without a window or hardware: a setlist from a temporary
//! library folder opens in place of a song on the app's `FakeOutput`, moves
//! through its items with the screen's controls, and shows its problems.

use std::fs;
use std::path::Path;
use std::time::Duration;

use app::testing::FakeOutput;
use app::{AudioOutput, SessionOptions};
use domain::{
    Arrangement, ClickItem, SectionMarker, SectionType, Setlist, SetlistItem, SongItem,
    SongPackage, Stem, StemRole, TempoPoint, TimeSignature, TimeSignaturePoint,
};
use hound::{SampleFormat, WavSpec, WavWriter};
use library::Library;
use openvamp_desktop::action::UiAction;
use openvamp_desktop::show::{Show, Source, replace};
use openvamp_desktop::strip::ItemState;
use openvamp_desktop::view::PerformView;

/// A two-bar song (intro and outro, 120 bpm in 4/4) titled `title` at
/// `rate`, with one quiet mono stem.
fn write_song(dir: &Path, title: &str, rate: u32) {
    let bar = 2 * u64::from(rate);
    let section = |id: &str, section_type, start| SectionMarker {
        id: id.to_owned(),
        section_type,
        number: None,
        start,
    };
    let package = SongPackage {
        format_version: 1,
        title: title.to_owned(),
        artist: None,
        sample_rate: rate,
        key: Some("E".to_owned()),
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![Stem {
            id: "keys".to_owned(),
            file: "keys.wav".to_owned(),
            part: "keys".to_owned(),
            role: StemRole::Track,
        }],
        sections: vec![
            section("intro", SectionType::Intro, 0),
            section("outro", SectionType::Outro, bar),
        ],
        arrangements: vec![Arrangement {
            id: "default".to_owned(),
            name: "Default".to_owned(),
            sections: vec!["intro".to_owned(), "outro".to_owned()],
            infinite_loop: Vec::new(),
        }],
    };
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("song.json"), package.to_json().unwrap()).unwrap();
    let spec = WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(dir.join("keys.wav"), spec).unwrap();
    for frame in 0..2 * bar {
        writer.write_sample((frame % 100) as i16 * 10).unwrap();
    }
    writer.finalize().unwrap();
}

fn song(path: &str) -> SetlistItem {
    SetlistItem::Song(SongItem::new(path))
}

/// Saves a setlist of `items` named `name` in `library` and returns where
/// it is.
fn save_setlist(library: &Path, name: &str, items: Vec<SetlistItem>) -> Source {
    let mut setlist = Setlist::new(name);
    setlist.items = items;
    let entry = Library::new(library).save_setlist(&setlist).unwrap();
    Source::Setlist {
        library: library.to_owned(),
        file: entry.file,
    }
}

fn output(fake: &FakeOutput) -> impl FnMut() -> Box<dyn AudioOutput> + '_ {
    || Box::new(fake.clone())
}

/// A few audio callbacks and screen polls.
fn run(show: &mut Show, fake: &FakeOutput) {
    for _ in 0..8 {
        fake.render(256);
        show.poll();
    }
}

fn setlist_view(show: &Show) -> openvamp_desktop::strip::SetlistView {
    show.setlist().expect("a setlist plays").view()
}

#[test]
fn a_setlist_opens_in_place_of_a_song_and_moves_through_its_items() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path();
    write_song(&library.join("Songs/One.song"), "First", 8_000);
    write_song(&library.join("Songs/Two.song"), "Second", 8_000);
    let source = save_setlist(
        library,
        "Sunday",
        vec![
            song("Songs/One.song"),
            song("Songs/Missing.song"),
            song("Songs/Two.song"),
            SetlistItem::Click(ClickItem {
                bars: Some(2),
                ..ClickItem::new(
                    90.0,
                    TimeSignature {
                        numerator: 3,
                        denominator: 4,
                    },
                )
            }),
        ],
    );
    let fake = FakeOutput::new();
    let options = SessionOptions::default();
    let mut current = None;
    replace(
        &mut current,
        &Source::Song(library.join("Songs/Two.song")),
        &options,
        output(&fake),
        |_| {},
    )
    .unwrap();
    assert!(matches!(current, Some(Show::Song(_))));

    let replaced = replace(&mut current, &source, &options, output(&fake), |_| {}).unwrap();

    assert_eq!(replaced.problems(), Vec::<String>::new());
    let (closed, previous) = replaced.previous.as_ref().unwrap();
    assert_eq!(closed.title, "Second");
    assert_eq!(previous, &Source::Song(library.join("Songs/Two.song")));
    let show = current.as_mut().unwrap();
    run(show, &fake);
    let view = setlist_view(show);
    assert_eq!(view.name, "Sunday");
    assert_eq!(view.position, "1 of 4");
    let titles: Vec<&str> = view.items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["First", "Missing", "Second", "Click"]);
    assert_eq!(view.items[0].meta, "E · 120 BPM · Default");
    assert_eq!(view.items[3].meta, "90 BPM · 3/4 · 2 bars");
    assert_eq!(view.items[0].state, ItemState::Current);
    assert!(view.items[2].is_next, "the missing item is skipped");
    assert!(!view.items[1].playable);
    assert!(
        view.problems[0].starts_with("Item 2 \"Missing\" cannot play: song \"Songs/Missing.song\""),
        "{:?}",
        view.problems
    );
    let perform = PerformView::new(show.snapshot(), show.song());
    assert_eq!(perform.title, "First");

    // Nothing comes before the first item: the refusal shows until a
    // control is accepted.
    show.send(UiAction::PreviousSong);
    run(show, &fake);
    assert_eq!(
        setlist_view(show).problems.last().map(String::as_str),
        Some("Refused: no item before the current one can play")
    );

    show.send(UiAction::NextSong);
    run(show, &fake);
    let view = setlist_view(show);
    assert_eq!(view.position, "3 of 4");
    assert_eq!(view.items[0].state, ItemState::Played);
    assert_eq!(view.items[2].state, ItemState::Current);
    assert!(!view.problems.iter().any(|line| line.starts_with("Refused")));
    assert_eq!(
        PerformView::new(show.snapshot(), show.song()).title,
        "Second"
    );

    // A tile selects its item; back to start acts on the current item.
    show.send(UiAction::SelectItem(3));
    run(show, &fake);
    assert_eq!(setlist_view(show).position, "4 of 4");
    assert_eq!(show.song().title, "Click");
    show.send(UiAction::BackToStart);
    run(show, &fake);
    assert_eq!(show.snapshot().last_error, None);

    // A song opens in place of the setlist again.
    let replaced = replace(
        &mut current,
        &Source::Song(library.join("Songs/One.song")),
        &options,
        output(&fake),
        |_| {},
    )
    .unwrap();
    assert_eq!(replaced.previous.as_ref().unwrap().0.title, "Sunday");
    assert!(matches!(current, Some(Show::Song(_))));
}

#[test]
fn the_device_held_for_another_rate_shows_in_the_setlist_problems() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path();
    write_song(&library.join("Low.song"), "Low", 8_000);
    write_song(&library.join("High.song"), "High", 16_000);
    let source = save_setlist(library, "Rates", vec![song("Low.song"), song("High.song")]);
    let fake = FakeOutput::without_devices().with_device("Speakers", 2, Some(&[8_000]));
    let mut current = None;
    replace(
        &mut current,
        &source,
        &SessionOptions {
            reconnect_interval: Duration::from_millis(1),
            ..SessionOptions::default()
        },
        output(&fake),
        |_| {},
    )
    .unwrap();
    let show = current.as_mut().unwrap();
    run(show, &fake);
    assert!(
        setlist_view(show)
            .problems
            .contains(&"Item 2 \"High\" runs at 16000 Hz and the device at 8000 Hz: it plays once the device reopens at its rate, and no AutoLink or Crossfade leads into it".to_owned())
    );

    show.send(UiAction::NextSong);
    for _ in 0..20 {
        run(show, &fake);
    }

    let problems = setlist_view(show).problems;
    let held = problems.last().unwrap();
    assert!(
        held.starts_with("The device is held for item 2 \"High\" at 16000 Hz, which it refuses ("),
        "{problems:?}"
    );
    assert!(
        held.ends_with("choose an item at 8000 Hz to play again"),
        "{held}"
    );

    // Selecting an item at the device's rate brings it back.
    show.send(UiAction::PreviousSong);
    for _ in 0..20 {
        run(show, &fake);
    }
    let problems = setlist_view(show).problems;
    assert!(
        !problems
            .iter()
            .any(|line| line.starts_with("The device is held")),
        "{problems:?}"
    );
}

#[test]
fn a_setlist_with_nothing_to_play_leaves_the_song_playing_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path();
    write_song(&library.join("One.song"), "First", 8_000);
    let source = save_setlist(library, "Empty", vec![song("Gone.song")]);
    let fake = FakeOutput::new();
    let options = SessionOptions::default();
    let mut current = None;
    replace(
        &mut current,
        &Source::Song(library.join("One.song")),
        &options,
        output(&fake),
        |_| {},
    )
    .unwrap();

    let error = replace(&mut current, &source, &options, output(&fake), |_| {}).unwrap_err();

    let lines = error.lines();
    assert_eq!(lines[0], "setlist \"Empty\" has no item that can play");
    assert!(
        lines[1].starts_with("item 1 cannot play: song \"Gone.song\""),
        "{lines:?}"
    );
    assert!(
        matches!(current, Some(Show::Song(_))),
        "the song still plays"
    );
}

#[test]
fn a_playing_setlist_follows_its_file_when_it_moves() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path();
    write_song(&library.join("One.song"), "First", 8_000);
    let source = save_setlist(library, "Sunday", vec![song("One.song")]);
    let fake = FakeOutput::new();
    let mut current = None;
    replace(
        &mut current,
        &source,
        &SessionOptions::default(),
        output(&fake),
        |_| {},
    )
    .unwrap();
    let show = current.as_mut().unwrap();

    show.follow_move(library, "Sunday.setlist.json", "Zed.setlist.json");
    assert_eq!(
        show.source(),
        Source::Setlist {
            library: library.to_owned(),
            file: "Zed.setlist.json".to_owned(),
        }
    );
    // A move in another library, or of another file, changes nothing.
    show.follow_move(
        Path::new("/elsewhere"),
        "Zed.setlist.json",
        "Other.setlist.json",
    );
    show.follow_move(library, "Other.setlist.json", "Third.setlist.json");
    assert_eq!(
        show.source(),
        Source::Setlist {
            library: library.to_owned(),
            file: "Zed.setlist.json".to_owned(),
        }
    );
}
