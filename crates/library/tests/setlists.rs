//! S2: setlists live as `*.setlist.json` files in the library's `Setlists/`
//! folder; saving is atomic and never overwrites another setlist.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use domain::{
    ClickItem, Setlist, SetlistEditError, SetlistFileError, SetlistItem, SongItem, TimeSignature,
    Transition,
};
use library::{
    Library, MAX_SETLIST_FILE_BYTES, SETLISTS_FOLDER, SetlistEntry, SetlistError, SetlistListing,
};
// Only the unix-only symlink and FIFO tests list unreadable setlists.
#[cfg(unix)]
use library::UnreadableSetlist;
use tempfile::TempDir;

fn new_library() -> (TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path());
    (dir, library)
}

fn setlists_folder(dir: &TempDir) -> PathBuf {
    dir.path().join(SETLISTS_FOLDER)
}

/// File names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn entry(name: &str, file: &str) -> SetlistEntry {
    SetlistEntry {
        name: name.to_owned(),
        file: file.to_owned(),
    }
}

/// A setlist using every field of the format.
fn full_setlist(name: &str) -> Setlist {
    let mut setlist = Setlist::new(name);
    setlist.items = vec![
        SetlistItem::Song(SongItem {
            song: "Hymns/Example Song.song".to_owned(),
            arrangement: "short".to_owned(),
            start_position: Some(1),
            end_position: Some(3),
            fade_out: true,
            transition: Transition::Crossfade,
        }),
        SetlistItem::Click(ClickItem {
            bpm: 92.5,
            time_signature: TimeSignature {
                numerator: 6,
                denominator: 8,
            },
            bars: Some(4),
            transition: Transition::AutoLink,
        }),
        SetlistItem::Song(SongItem::new("Second.song")),
    ];
    setlist
}

fn names(listing: &SetlistListing) -> Vec<(&str, &str)> {
    listing
        .setlists
        .iter()
        .map(|entry| (entry.name.as_str(), entry.file.as_str()))
        .collect()
}

#[test]
fn a_library_without_a_setlists_folder_lists_no_setlists() {
    let (_dir, library) = new_library();

    let listing = library.list_setlists().unwrap();

    assert!(listing.setlists.is_empty());
    assert!(listing.unreadable.is_empty());
}

#[test]
fn listing_returns_readable_setlists_sorted_by_name_and_reports_broken_files() {
    let (dir, library) = new_library();
    for name in ["cherry", "Banana", "apple"] {
        library.save_setlist(&Setlist::new(name)).unwrap();
    }
    let folder = setlists_folder(&dir);
    fs::write(folder.join("notes.txt"), "not a setlist").unwrap();
    fs::write(folder.join("._apple.setlist.json"), "macOS metadata").unwrap();
    fs::write(folder.join("broken.setlist.json"), "{ not json").unwrap();
    fs::create_dir(folder.join("folder.setlist.json")).unwrap();

    let listing = library.list_setlists().unwrap();

    assert_eq!(
        names(&listing),
        [
            ("apple", "apple.setlist.json"),
            ("Banana", "Banana.setlist.json"),
            ("cherry", "cherry.setlist.json"),
        ]
    );
    let unreadable: Vec<&str> = listing
        .unreadable
        .iter()
        .map(|broken| broken.file.as_str())
        .collect();
    assert_eq!(unreadable, ["broken.setlist.json", "folder.setlist.json"]);
    assert!(matches!(
        listing.unreadable[0].error,
        SetlistError::Format {
            source: SetlistFileError::Json(_),
            ..
        }
    ));
    assert!(matches!(
        listing.unreadable[1].error,
        SetlistError::NotAFile { .. }
    ));
}

#[test]
fn loading_parses_a_setlist_file_written_by_hand() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    fs::create_dir(&folder).unwrap();
    fs::write(
        folder.join("By Hand.setlist.json"),
        r#"{
            "formatVersion": 1,
            "name": "By Hand",
            "items": [
                { "type": "song", "song": "Songs/A.song", "arrangement": "default", "transition": "autoLink" },
                { "type": "click", "bpm": 90, "timeSignature": { "numerator": 3, "denominator": 4 } }
            ]
        }"#,
    )
    .unwrap();

    let setlist = library.load_setlist("By Hand.setlist.json").unwrap();

    let mut expected = Setlist::new("By Hand");
    expected.items = vec![
        SetlistItem::Song(SongItem {
            transition: Transition::AutoLink,
            ..SongItem::new("Songs/A.song")
        }),
        SetlistItem::Click(ClickItem::new(
            90.0,
            TimeSignature {
                numerator: 3,
                denominator: 4,
            },
        )),
    ];
    assert_eq!(setlist, expected);
}

#[test]
fn loading_reports_a_missing_file_and_a_newer_format() {
    let (dir, library) = new_library();

    let missing = library.load_setlist("Missing.setlist.json").unwrap_err();
    assert!(
        matches!(&missing, SetlistError::Read { source, .. } if source.kind() == io::ErrorKind::NotFound),
        "{missing}"
    );
    assert!(
        missing.to_string().contains("Missing.setlist.json"),
        "{missing}"
    );

    let folder = setlists_folder(&dir);
    fs::create_dir(&folder).unwrap();
    fs::write(
        folder.join("Future.setlist.json"),
        r#"{ "formatVersion": 2, "name": "Future", "items": [] }"#,
    )
    .unwrap();
    let error = library.load_setlist("Future.setlist.json").unwrap_err();
    assert!(
        matches!(
            error,
            SetlistError::Format {
                source: SetlistFileError::UnsupportedFormatVersion { found: 2 },
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_setlist_file_over_the_size_limit_is_refused() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    fs::create_dir(&folder).unwrap();
    let mut json = r#"{ "formatVersion": 1, "name": "Big", "items": [] }"#.to_owned();
    json.push_str(&" ".repeat(MAX_SETLIST_FILE_BYTES as usize));
    fs::write(folder.join("Big.setlist.json"), json).unwrap();

    let error = library.load_setlist("Big.setlist.json").unwrap_err();

    assert!(
        matches!(error, SetlistError::TooLarge { max_bytes, .. } if max_bytes == MAX_SETLIST_FILE_BYTES),
        "{error}"
    );
}

#[test]
fn a_saved_setlist_loads_back_unchanged() {
    let (dir, library) = new_library();
    let setlist = full_setlist("Sunday Morning");

    let saved = library.save_setlist(&setlist).unwrap();

    assert_eq!(
        saved,
        entry("Sunday Morning", "Sunday Morning.setlist.json")
    );
    assert!(setlists_folder(&dir).join(&saved.file).is_file());
    assert_eq!(library.load_setlist(&saved.file).unwrap(), setlist);
}

#[test]
fn saving_replaces_the_same_setlist_and_leaves_no_temporary_file() {
    let (dir, library) = new_library();
    let mut setlist = full_setlist("Evening");
    library.save_setlist(&setlist).unwrap();

    setlist.remove_item(0).unwrap();
    let saved = library.save_setlist(&setlist).unwrap();

    assert_eq!(library.load_setlist(&saved.file).unwrap(), setlist);
    assert_eq!(entries(&setlists_folder(&dir)), ["Evening.setlist.json"]);
}

#[test]
fn a_failed_save_leaves_the_existing_file_untouched_and_no_temporary_file() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    library.save_setlist(&full_setlist("Evening")).unwrap();
    let before = fs::read(folder.join("Evening.setlist.json")).unwrap();

    // JSON cannot hold a tempo that is not a number.
    let mut unwritable = full_setlist("Evening");
    unwritable
        .insert_item(
            0,
            SetlistItem::Click(ClickItem::new(
                f64::NAN,
                TimeSignature {
                    numerator: 4,
                    denominator: 4,
                },
            )),
        )
        .unwrap();
    let error = library.save_setlist(&unwritable).unwrap_err();
    assert!(
        matches!(
            error,
            SetlistError::Format {
                source: SetlistFileError::NonFiniteBpm { item: 0 },
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(
        fs::read(folder.join("Evening.setlist.json")).unwrap(),
        before
    );

    // A folder holds the file name: nothing is written over it.
    fs::create_dir(folder.join("Taken.setlist.json")).unwrap();
    fs::write(folder.join("Taken.setlist.json/inside"), "kept").unwrap();
    let error = library.save_setlist(&Setlist::new("Taken")).unwrap_err();
    assert!(matches!(error, SetlistError::NotAFile { .. }), "{error}");
    assert_eq!(
        entries(&folder.join("Taken.setlist.json")),
        ["inside".to_owned()]
    );

    assert_eq!(
        entries(&folder),
        ["Evening.setlist.json", "Taken.setlist.json"]
    );
}

#[test]
fn a_blank_name_is_refused_before_anything_is_written() {
    let (dir, library) = new_library();

    let error = library.save_setlist(&Setlist::new("  ")).unwrap_err();

    assert!(
        matches!(error, SetlistError::Edit(SetlistEditError::EmptyName)),
        "{error}"
    );
    assert!(!setlists_folder(&dir).exists());
}

#[test]
fn setlist_names_map_to_file_names_that_are_safe_on_every_platform() {
    let (dir, library) = new_library();
    let cases = [
        ("Sunday Morning", "Sunday Morning.setlist.json"),
        ("Café Night", "Café Night.setlist.json"),
        ("AC/DC: Live?", "AC%2FDC%3A Live%3F.setlist.json"),
        (r#"a\b*c<d>e|f"g"#, "a%5Cb%2Ac%3Cd%3Ee%7Cf%22g.setlist.json"),
        ("100% live", "100%25 live.setlist.json"),
        ("tab\there", "tab%09here.setlist.json"),
        (".hidden", "%2Ehidden.setlist.json"),
        ("..", "%2E..setlist.json"),
        ("CON", "%43ON.setlist.json"),
        ("nul.backup", "%6Eul.backup.setlist.json"),
        ("Com1 ", "%43om1 .setlist.json"),
        ("LPT²", "%4CPT².setlist.json"),
        ("Con Brio", "Con Brio.setlist.json"),
        ("Auxiliary", "Auxiliary.setlist.json"),
    ];
    for (name, file) in cases {
        let saved = library.save_setlist(&Setlist::new(name)).unwrap();

        assert_eq!(saved, entry(name, file), "{name:?}");
        assert!(setlists_folder(&dir).join(file).is_file(), "{name:?}");
        assert_eq!(library.load_setlist(file).unwrap().name, name, "{name:?}");
    }
}

#[test]
fn long_names_are_cut_to_200_bytes_at_a_character_boundary() {
    let (_dir, library) = new_library();

    let ascii = library
        .save_setlist(&Setlist::new("a".repeat(300)))
        .unwrap();
    assert_eq!(ascii.file, format!("{}.setlist.json", "a".repeat(200)));

    // "é" is two bytes in UTF-8, "%2F" three: neither is split.
    let accented = library
        .save_setlist(&Setlist::new("é".repeat(150)))
        .unwrap();
    assert_eq!(accented.file, format!("{}.setlist.json", "é".repeat(100)));
    let escaped = library
        .save_setlist(&Setlist::new(format!("{}/x", "b".repeat(198))))
        .unwrap();
    assert_eq!(escaped.file, format!("{}.setlist.json", "b".repeat(198)));
}

#[test]
fn saving_never_overwrites_another_setlist_that_maps_to_the_same_file() {
    let (dir, library) = new_library();
    let first = format!("{}first", "x".repeat(200));
    let second = format!("{}second", "x".repeat(200));
    let saved = library.save_setlist(&full_setlist(&first)).unwrap();
    let before = fs::read(setlists_folder(&dir).join(&saved.file)).unwrap();

    let error = library.save_setlist(&Setlist::new(&second)).unwrap_err();

    assert!(
        matches!(&error, SetlistError::FileTaken { name, file } if *name == second && *file == saved.file),
        "{error}"
    );
    assert_eq!(
        fs::read(setlists_folder(&dir).join(&saved.file)).unwrap(),
        before
    );
    assert_eq!(entries(&setlists_folder(&dir)), [saved.file]);
}

#[test]
fn renaming_moves_the_file_and_the_name_together() {
    let (dir, library) = new_library();
    let saved = library.save_setlist(&full_setlist("Old Name")).unwrap();

    let renamed = library.rename_setlist(&saved.file, "New: Name").unwrap();

    assert_eq!(renamed, entry("New: Name", "New%3A Name.setlist.json"));
    assert_eq!(
        entries(&setlists_folder(&dir)),
        ["New%3A Name.setlist.json"]
    );
    assert_eq!(
        library.load_setlist(&renamed.file).unwrap(),
        full_setlist("New: Name")
    );
}

#[test]
fn renaming_can_change_only_the_case_of_the_name() {
    let (dir, library) = new_library();
    let saved = library.save_setlist(&full_setlist("sunday")).unwrap();

    let renamed = library.rename_setlist(&saved.file, "Sunday").unwrap();

    assert_eq!(renamed, entry("Sunday", "Sunday.setlist.json"));
    assert_eq!(entries(&setlists_folder(&dir)), ["Sunday.setlist.json"]);
    assert_eq!(
        names(&library.list_setlists().unwrap()),
        [("Sunday", "Sunday.setlist.json")]
    );
}

#[test]
fn renaming_never_overwrites_another_setlist() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    let a = library.save_setlist(&full_setlist("A")).unwrap();
    let b = library.save_setlist(&Setlist::new("B")).unwrap();
    let (a_bytes, b_bytes) = (
        fs::read(folder.join(&a.file)).unwrap(),
        fs::read(folder.join(&b.file)).unwrap(),
    );

    let taken = library.rename_setlist(&a.file, "B").unwrap_err();
    assert!(
        matches!(&taken, SetlistError::FileTaken { name, file } if name == "B" && file == "B.setlist.json"),
        "{taken}"
    );
    let blank = library.rename_setlist(&a.file, " ").unwrap_err();
    assert!(
        matches!(blank, SetlistError::Edit(SetlistEditError::EmptyName)),
        "{blank}"
    );

    assert_eq!(fs::read(folder.join(&a.file)).unwrap(), a_bytes);
    assert_eq!(fs::read(folder.join(&b.file)).unwrap(), b_bytes);
    assert_eq!(entries(&folder), ["A.setlist.json", "B.setlist.json"]);
}

#[test]
fn duplicating_copies_the_setlist_under_a_new_name() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    let original = library.save_setlist(&full_setlist("Morning")).unwrap();
    let before = fs::read(folder.join(&original.file)).unwrap();

    let copy = library
        .duplicate_setlist(&original.file, "Morning (copy)")
        .unwrap();

    assert_eq!(copy, entry("Morning (copy)", "Morning (copy).setlist.json"));
    assert_eq!(
        library.load_setlist(&copy.file).unwrap(),
        full_setlist("Morning (copy)")
    );
    assert_eq!(fs::read(folder.join(&original.file)).unwrap(), before);
    assert_eq!(
        entries(&folder),
        ["Morning (copy).setlist.json", "Morning.setlist.json"]
    );
}

#[test]
fn duplicating_never_overwrites_another_setlist() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    let a = library.save_setlist(&full_setlist("A")).unwrap();
    let b = library.save_setlist(&Setlist::new("B")).unwrap();
    let b_bytes = fs::read(folder.join(&b.file)).unwrap();

    let taken = library.duplicate_setlist(&a.file, "B").unwrap_err();
    assert!(
        matches!(&taken, SetlistError::FileTaken { name, file } if name == "B" && file == "B.setlist.json"),
        "{taken}"
    );
    let same = library.duplicate_setlist(&a.file, "A").unwrap_err();
    assert!(
        matches!(same, SetlistError::Edit(SetlistEditError::SameName { .. })),
        "{same}"
    );

    assert_eq!(fs::read(folder.join(&b.file)).unwrap(), b_bytes);
    assert_eq!(entries(&folder), ["A.setlist.json", "B.setlist.json"]);
}

#[test]
fn file_names_that_are_not_plain_setlist_file_names_are_refused() {
    let (dir, library) = new_library();
    library.save_setlist(&Setlist::new("Real")).unwrap();
    fs::write(dir.path().join("outside.setlist.json"), "{}").unwrap();

    for file in [
        "",
        "../outside.setlist.json",
        "sub/x.setlist.json",
        r"sub\x.setlist.json",
        "Real.json",
        ".setlist.json",
        ".hidden.setlist.json",
    ] {
        let load = library.load_setlist(file).unwrap_err();
        assert!(
            matches!(&load, SetlistError::InvalidFileName { file: rejected } if rejected == file),
            "{file:?}: {load}"
        );
        let rename = library.rename_setlist(file, "Other").unwrap_err();
        assert!(
            matches!(rename, SetlistError::InvalidFileName { .. }),
            "{file:?}"
        );
        let duplicate = library.duplicate_setlist(file, "Other").unwrap_err();
        assert!(
            matches!(duplicate, SetlistError::InvalidFileName { .. }),
            "{file:?}"
        );
    }
    // On Windows a colon names a drive or an alternate data stream.
    #[cfg(windows)]
    assert!(matches!(
        library.load_setlist("Real:stream.setlist.json"),
        Err(SetlistError::InvalidFileName { .. })
    ));
    assert_eq!(entries(&setlists_folder(&dir)), ["Real.setlist.json"]);
}

#[cfg(unix)]
#[test]
fn symlinks_that_leave_the_library_are_refused() {
    use std::os::unix::fs::symlink;

    let outside = tempfile::tempdir().unwrap();
    let json = Setlist::new("Outside").to_json().unwrap();
    fs::write(outside.path().join("Outside.setlist.json"), &json).unwrap();

    // A setlist file that links out of the setlists folder.
    let (dir, library) = new_library();
    library.save_setlist(&Setlist::new("Inside")).unwrap();
    let folder = setlists_folder(&dir);
    symlink(
        outside.path().join("Outside.setlist.json"),
        folder.join("Link.setlist.json"),
    )
    .unwrap();
    let error = library.load_setlist("Link.setlist.json").unwrap_err();
    assert!(
        matches!(error, SetlistError::OutsideLibrary { .. }),
        "{error}"
    );
    let listing = library.list_setlists().unwrap();
    assert_eq!(names(&listing), [("Inside", "Inside.setlist.json")]);
    assert!(matches!(
        listing.unreadable[..],
        [UnreadableSetlist {
            error: SetlistError::OutsideLibrary { .. },
            ..
        }]
    ));

    // A setlists folder that links out of the library.
    let (linked, linked_library) = new_library();
    symlink(outside.path(), setlists_folder(&linked)).unwrap();
    assert!(matches!(
        linked_library.list_setlists(),
        Err(SetlistError::OutsideLibrary { .. })
    ));
    assert!(matches!(
        linked_library.load_setlist("Outside.setlist.json"),
        Err(SetlistError::OutsideLibrary { .. })
    ));
    assert!(matches!(
        linked_library.save_setlist(&Setlist::new("Written")),
        Err(SetlistError::OutsideLibrary { .. })
    ));
    assert_eq!(entries(outside.path()), ["Outside.setlist.json"]);
}

/// Runs `run` on another thread and gives up after five seconds, so code
/// that opens a FIFO (and blocks) fails the test instead of hanging it.
#[cfg(unix)]
fn within_five_seconds<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(run());
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("returns without opening a FIFO")
}

#[cfg(unix)]
#[test]
fn a_fifo_setlist_file_is_refused_without_blocking() {
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    fs::create_dir(&folder).unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(folder.join("Pipe.setlist.json"))
        .status()
        .unwrap();
    assert!(status.success());

    let loader = library.clone();
    let error = within_five_seconds(move || loader.load_setlist("Pipe.setlist.json")).unwrap_err();
    assert!(matches!(error, SetlistError::NotAFile { .. }), "{error}");

    let listing = within_five_seconds(move || library.list_setlists()).unwrap();
    assert!(listing.setlists.is_empty());
    assert!(matches!(
        listing.unreadable[..],
        [UnreadableSetlist {
            error: SetlistError::NotAFile { .. },
            ..
        }]
    ));
}

#[test]
fn renaming_only_the_case_works_when_the_file_name_is_stored_decomposed() {
    // HFS+ and exFAT on macOS store file names decomposed (NFD): a setlist
    // saved as "Domingo Mañana" is listed as "Domingo Man\u{303}ana.setlist.json".
    let (dir, library) = new_library();
    let folder = setlists_folder(&dir);
    fs::create_dir(&folder).unwrap();
    fs::write(
        folder.join("Domingo Man\u{303}ana.setlist.json"),
        full_setlist("Domingo Ma\u{f1}ana").to_json().unwrap(),
    )
    .unwrap();
    let file = library.list_setlists().unwrap().setlists[0].file.clone();

    let renamed = library
        .rename_setlist(&file, "Domingo ma\u{f1}ana")
        .unwrap();

    assert_eq!(renamed.name, "Domingo ma\u{f1}ana");
    let listing = library.list_setlists().unwrap();
    assert_eq!(names(&listing).len(), 1, "{:?}", names(&listing));
    assert_eq!(
        library.load_setlist(&listing.setlists[0].file).unwrap(),
        full_setlist("Domingo ma\u{f1}ana")
    );
}
