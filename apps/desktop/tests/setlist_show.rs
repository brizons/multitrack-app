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
    // The library is kept canonical, the form the Library screen gives.
    let library = &std::fs::canonicalize(library).unwrap();

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

#[test]
fn a_setlist_keeps_its_library_folder_canonical() {
    let dir = tempfile::tempdir().unwrap();
    write_song(&dir.path().join("One.song"), "First", 8_000);
    save_setlist(dir.path(), "Sunday", vec![song("One.song")]);
    let fake = FakeOutput::new();
    let mut current = None;
    let source = Source::Setlist {
        library: dir.path().join("One.song/.."),
        file: "Sunday.setlist.json".to_owned(),
    };
    replace(
        &mut current,
        &source,
        &SessionOptions::default(),
        output(&fake),
        |_| {},
    )
    .unwrap();

    assert_eq!(
        current.unwrap().source(),
        Source::Setlist {
            library: std::fs::canonicalize(dir.path()).unwrap(),
            file: "Sunday.setlist.json".to_owned(),
        }
    );
}

#[test]
fn a_refused_control_is_forgotten_once_the_setlist_moves_on_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path();
    write_song(&library.join("One.song"), "First", 8_000);
    write_song(&library.join("Two.song"), "Second", 8_000);
    let mut first = SongItem::new("One.song");
    first.transition = domain::Transition::AutoLink;
    let source = save_setlist(
        library,
        "Sunday",
        vec![SetlistItem::Song(first), song("Two.song")],
    );
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
    show.send(UiAction::PreviousSong);
    run(show, &fake);
    assert!(
        setlist_view(show)
            .problems
            .iter()
            .any(|l| l.starts_with("Refused"))
    );

    show.send(UiAction::TogglePlay);
    for _ in 0..2_000 {
        if setlist_view(show).position == "2 of 2" {
            break;
        }
        for monitor in show.setlist().unwrap().session.disk_monitors() {
            assert!(monitor.wait_idle(Duration::from_secs(10)));
        }
        fake.render(256);
        show.poll();
    }

    let view = setlist_view(show);
    assert_eq!(view.position, "2 of 2", "AutoLink moved on");
    assert!(
        !view.problems.iter().any(|l| l.starts_with("Refused")),
        "{:?}",
        view.problems
    );
}

#[cfg(unix)]
#[test]
fn songs_the_library_cannot_list_show_as_a_note_and_items_keep_their_folder_names() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("Library");
    write_song(&library.join("One.song"), "First", 8_000);
    let source = save_setlist(&library, "Sunday", vec![song("One.song")]);
    // Searchable but not readable: paths inside resolve, the folder does
    // not list.
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o311)).unwrap();
    let listable = std::fs::read_dir(&library).is_ok();
    let fake = FakeOutput::new();
    let mut current = None;
    let opened = replace(
        &mut current,
        &source,
        &SessionOptions::default(),
        output(&fake),
        |_| {},
    );
    std::fs::set_permissions(&library, std::fs::Permissions::from_mode(0o755)).unwrap();
    if listable {
        // Running as root: the folder stays readable.
        return;
    }
    opened.unwrap();

    let view = setlist_view(current.as_ref().unwrap());
    assert_eq!(view.items[0].title, "One");
    assert!(
        view.problems.iter().any(|line| line.starts_with(
            "The library's songs could not be listed, so items show their folder names: "
        )),
        "{:?}",
        view.problems
    );
}

/// A device that is checked fine but held by another song, so opening a
/// pick on it fails after the check.
fn busy_device(dir: &Path) -> (FakeOutput, Option<Show>) {
    let busy = FakeOutput::new();
    write_song(&dir.join("Elsewhere.song"), "Elsewhere", 8_000);
    let mut holder = None;
    replace(
        &mut holder,
        &Source::Song(dir.join("Elsewhere.song")),
        &SessionOptions::default(),
        output(&busy),
        |_| {},
    )
    .unwrap();
    (busy, holder)
}

#[test]
fn a_setlist_comes_back_stopped_when_the_pick_cannot_start_on_its_device() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("Library");
    write_song(&library.join("One.song"), "First", 8_000);
    write_song(&dir.path().join("Pick.song"), "Pick", 8_000);
    let source = save_setlist(&library, "Sunday", vec![song("One.song")]);
    let device = FakeOutput::new();
    let mut current = None;
    replace(
        &mut current,
        &source,
        &SessionOptions::default(),
        output(&device),
        |_| {},
    )
    .unwrap();
    let (busy, _holder) = busy_device(&dir.path().join("other"));

    let calls = std::cell::Cell::new(0);
    let replaced = replace(
        &mut current,
        &Source::Song(dir.path().join("Pick.song")),
        &SessionOptions::default(),
        || {
            calls.set(calls.get() + 1);
            let output = if calls.get() == 1 { &busy } else { &device };
            Box::new(output.clone()) as Box<dyn AudioOutput>
        },
        |_| {
            device.render(256);
        },
    )
    .expect("the check lets the pick go ahead");

    assert!(replaced.start_error.is_some());
    assert!(replaced.reopen_error.is_none());
    let show = current.as_mut().expect("the setlist is back");
    assert_eq!(setlist_view(show).name, "Sunday");
    assert_eq!(show.snapshot().transport, app::TransportState::Stopped);
    let problems = replaced.problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].starts_with(&format!(
            "Could not start {} on the audio device: ",
            dir.path().join("Pick.song").display()
        )),
        "{problems:?}"
    );
    assert_eq!(
        problems[1],
        "\"Sunday\" is loaded again, stopped at the start."
    );
}

#[test]
fn when_the_setlist_cannot_come_back_nothing_is_loaded_and_both_reasons_show() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("Library");
    write_song(&library.join("One.song"), "First", 8_000);
    write_song(&dir.path().join("Pick.song"), "Pick", 8_000);
    let source = save_setlist(&library, "Sunday", vec![song("One.song")]);
    let device = FakeOutput::new();
    let mut current = None;
    replace(
        &mut current,
        &source,
        &SessionOptions::default(),
        output(&device),
        |_| {},
    )
    .unwrap();
    let (busy, _holder) = busy_device(&dir.path().join("other"));

    let replaced = replace(
        &mut current,
        &Source::Song(dir.path().join("Pick.song")),
        &SessionOptions::default(),
        || Box::new(busy.clone()) as Box<dyn AudioOutput>,
        |_| {
            device.render(256);
        },
    )
    .unwrap();

    assert!(current.is_none());
    let problems = replaced.problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[1].starts_with("\"Sunday\" could not be loaded again: "),
        "{problems:?}"
    );
}
