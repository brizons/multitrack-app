//! S2 (L24), F5.1: the order of the setlist list lives in
//! `Setlists/order.json`; setlists it does not name follow by name, an order
//! file that cannot be used falls back to name order and says why, and no
//! setlist is ever hidden, lost or changed by the order.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use domain::Setlist;
use library::{Library, SETLIST_ORDER_FILE, SETLISTS_FOLDER, SetlistError, SetlistListing};
use tempfile::TempDir;

fn new_library() -> (TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path());
    (dir, library)
}

fn folder(dir: &TempDir) -> PathBuf {
    dir.path().join(SETLISTS_FOLDER)
}

fn order_path(dir: &TempDir) -> PathBuf {
    folder(dir).join(SETLIST_ORDER_FILE)
}

/// Saves setlists named `names` and returns the library.
fn library_with(names: &[&str]) -> (TempDir, Library) {
    let (dir, library) = new_library();
    for name in names {
        library.save_setlist(&Setlist::new(*name)).unwrap();
    }
    (dir, library)
}

fn names(listing: &SetlistListing) -> Vec<&str> {
    listing.setlists.iter().map(|s| s.name.as_str()).collect()
}

fn listed(library: &Library) -> Vec<String> {
    let listing = library.list_setlists().unwrap();
    assert!(listing.order_error.is_none(), "{:?}", listing.order_error);
    names(&listing).into_iter().map(str::to_owned).collect()
}

/// Every setlist file and its bytes, to show the order never changes one.
fn setlist_files(dir: &TempDir) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(folder(dir))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(".setlist.json")
        })
        .map(|entry| {
            let name = entry.file_name().into_string().unwrap();
            let bytes = fs::read(entry.path()).unwrap_or_default();
            (name, bytes)
        })
        .collect()
}

/// File names in the setlists folder, sorted.
fn entries(dir: &TempDir) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder(dir))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn write_order(dir: &TempDir, json: &str) {
    fs::write(order_path(dir), json).unwrap();
}

#[test]
fn without_an_order_file_setlists_are_listed_by_name_as_before() {
    let (dir, library) = library_with(&["cherry", "Banana", "apple"]);
    fs::write(folder(&dir).join("broken.setlist.json"), "{ not json").unwrap();

    let listing = library.list_setlists().unwrap();

    assert_eq!(names(&listing), ["apple", "Banana", "cherry"]);
    assert_eq!(listing.unreadable.len(), 1);
    assert!(listing.order_error.is_none());
    // Neither saving nor listing writes an order file.
    assert!(!order_path(&dir).exists());
}

#[test]
fn the_stored_order_comes_first_then_the_rest_by_name_and_stale_names_are_ignored() {
    let (dir, library) = library_with(&["A", "B", "C", "D"]);
    write_order(
        &dir,
        r#"{"formatVersion": 1, "setlists": ["C.setlist.json", "Gone.setlist.json", "A.setlist.json", "C.setlist.json", "../A.setlist.json"]}"#,
    );

    assert_eq!(listed(&library), ["C", "A", "B", "D"]);
}

#[test]
fn a_hand_written_order_with_escapes_and_other_keys_is_read() {
    let (dir, library) = library_with(&["Café", "Zebra", "Alpha"]);
    write_order(
        &dir,
        "{\n  \"comment\": {\"a\": [1, -2.5e3, true, false, null, {\"b\": \"x\\\"y\"}]},\n  \"setlists\" : [ \"Zebra.setlist.json\" , \"Caf\\u00e9.setlist.json\" ],\n  \"formatVersion\": 1\n}\n",
    );

    assert_eq!(listed(&library), ["Zebra", "Café", "Alpha"]);
}

/// Lists `library`, which must fall back to name order and report a
/// problem, and returns it.
fn listed_by_name_with_a_problem(dir: &TempDir, library: &Library) -> SetlistError {
    let before = setlist_files(dir);
    let mut listing = library.list_setlists().unwrap();
    assert_eq!(names(&listing), ["A", "B", "C"], "name order");
    assert!(listing.unreadable.is_empty(), "no setlist is hidden");
    assert_eq!(setlist_files(dir), before, "no setlist file changes");
    listing.order_error.take().expect("the problem is reported")
}

#[test]
fn an_order_file_that_cannot_be_used_falls_back_to_name_order_and_says_why() {
    for (json, expected) in [
        ("{ not json", "is not a valid setlist order"),
        (
            r#"{"formatVersion": 1, "setlists": [1, 2]}"#,
            "is not a valid setlist order",
        ),
        (
            r#"{"formatVersion": 1, "setlists": "C.setlist.json"}"#,
            "is not a valid setlist order",
        ),
        (r#"{"setlists": []}"#, "is not a valid setlist order"),
        (
            r#"{"formatVersion": 1, "setlists": []} trailing"#,
            "is not a valid setlist order",
        ),
        (
            r#"{"formatVersion": 2, "setlists": ["C.setlist.json"], "groups": {}}"#,
            "unsupported formatVersion 2",
        ),
    ] {
        let (dir, library) = library_with(&["C", "B", "A"]);
        write_order(&dir, json);
        let error = listed_by_name_with_a_problem(&dir, &library);
        assert!(error.to_string().contains(expected), "{json}: {error}");
    }

    // Deep nesting is refused, not followed.
    let (dir, library) = library_with(&["C", "B", "A"]);
    let deep = format!(
        r#"{{"formatVersion": 1, "x": {}{}, "setlists": []}}"#,
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    write_order(&dir, &deep);
    let error = listed_by_name_with_a_problem(&dir, &library);
    assert!(matches!(error, SetlistError::OrderFormat { .. }), "{error}");

    // A folder in its place.
    let (dir, library) = library_with(&["C", "B", "A"]);
    fs::create_dir(order_path(&dir)).unwrap();
    let error = listed_by_name_with_a_problem(&dir, &library);
    assert!(matches!(error, SetlistError::NotAFile { .. }), "{error}");
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
fn a_fifo_or_a_symlink_out_of_the_library_as_the_order_file_is_not_read() {
    let (dir, library) = library_with(&["C", "B", "A"]);
    let status = std::process::Command::new("mkfifo")
        .arg(order_path(&dir))
        .status()
        .unwrap();
    assert!(status.success());
    let lister = library.clone();
    within_five_seconds(move || lister.list_setlists().unwrap());
    let error = listed_by_name_with_a_problem(&dir, &library);
    assert!(matches!(error, SetlistError::NotAFile { .. }), "{error}");

    let outside = tempfile::tempdir().unwrap();
    let elsewhere = outside.path().join("order.json");
    let order = r#"{"formatVersion": 1, "setlists": ["C.setlist.json"]}"#;
    fs::write(&elsewhere, order).unwrap();
    let (dir, library) = library_with(&["C", "B", "A"]);
    std::os::unix::fs::symlink(&elsewhere, order_path(&dir)).unwrap();
    let error = listed_by_name_with_a_problem(&dir, &library);
    assert!(
        matches!(error, SetlistError::OutsideLibrary { .. }),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&elsewhere).unwrap(), order);
}

#[test]
fn moving_a_setlist_stores_the_order_and_it_lasts() {
    let (dir, library) = library_with(&["A", "B", "C", "D"]);
    let before = setlist_files(&dir);

    library.move_setlist("D.setlist.json", 0).unwrap();
    assert_eq!(listed(&library), ["D", "A", "B", "C"]);
    library.move_setlist("A.setlist.json", 3).unwrap();
    assert_eq!(listed(&library), ["D", "B", "C", "A"]);
    library.move_setlist("C.setlist.json", 1).unwrap();

    assert_eq!(listed(&Library::new(dir.path())), ["D", "C", "B", "A"]);
    assert_eq!(setlist_files(&dir), before, "no setlist file changes");
    assert_eq!(
        entries(&dir),
        [
            "A.setlist.json",
            "B.setlist.json",
            "C.setlist.json",
            "D.setlist.json",
            "order.json"
        ]
    );
    let stored = fs::read_to_string(order_path(&dir)).unwrap();
    assert!(stored.contains("\"formatVersion\": 1"), "{stored}");

    // A place past the end, an unknown file and a bad name change nothing.
    let order = fs::read(order_path(&dir)).unwrap();
    let error = library.move_setlist("A.setlist.json", 4).unwrap_err();
    assert!(
        matches!(error, SetlistError::MoveOutOfRange { to: 4, len: 4 }),
        "{error}"
    );
    assert_eq!(
        error.to_string(),
        "cannot move a setlist to place 5: the library lists 4 setlists"
    );
    let error = library.move_setlist("Gone.setlist.json", 0).unwrap_err();
    assert!(matches!(error, SetlistError::Read { .. }), "{error}");
    let error = library.move_setlist("../A.setlist.json", 0).unwrap_err();
    assert!(
        matches!(error, SetlistError::InvalidFileName { .. }),
        "{error}"
    );
    assert_eq!(fs::read(order_path(&dir)).unwrap(), order);
}

#[test]
fn a_stored_order_keeps_new_renamed_and_duplicated_setlists_in_place() {
    let (dir, library) = library_with(&["B", "C", "D"]);
    library.move_setlist("D.setlist.json", 0).unwrap();
    assert_eq!(listed(&library), ["D", "B", "C"]);

    // Created: at the end, not in name order.
    library.save_setlist(&Setlist::new("A")).unwrap();
    assert_eq!(listed(&library), ["D", "B", "C", "A"]);
    // Saving it again changes nothing.
    library.save_setlist(&Setlist::new("A")).unwrap();
    assert_eq!(listed(&library), ["D", "B", "C", "A"]);

    // Renamed, also in case only: in its place.
    library.rename_setlist("B.setlist.json", "Zed").unwrap();
    assert_eq!(listed(&library), ["D", "Zed", "C", "A"]);
    library.rename_setlist("Zed.setlist.json", "zed").unwrap();
    assert_eq!(listed(&library), ["D", "zed", "C", "A"]);

    // Duplicated: right after the original.
    library
        .duplicate_setlist("D.setlist.json", "D copy")
        .unwrap();
    assert_eq!(listed(&library), ["D", "D copy", "zed", "C", "A"]);
    library
        .duplicate_setlist("A.setlist.json", "A copy")
        .unwrap();
    assert_eq!(listed(&library), ["D", "D copy", "zed", "C", "A", "A copy"]);

    assert_eq!(
        listed(&Library::new(dir.path())),
        ["D", "D copy", "zed", "C", "A", "A copy"]
    );
}

#[test]
fn without_a_stored_order_nothing_writes_one_but_a_move() {
    let (dir, library) = library_with(&["B", "C"]);
    library.save_setlist(&Setlist::new("A")).unwrap();
    library.rename_setlist("B.setlist.json", "E").unwrap();
    library
        .duplicate_setlist("C.setlist.json", "C copy")
        .unwrap();

    assert!(!order_path(&dir).exists());
    assert_eq!(listed(&library), ["A", "C", "C copy", "E"]);
}

#[test]
fn a_move_replaces_a_broken_order_but_never_one_from_a_newer_version() {
    let (dir, library) = library_with(&["A", "B", "C"]);
    write_order(&dir, "{ not json");
    library.move_setlist("C.setlist.json", 0).unwrap();
    assert_eq!(listed(&library), ["C", "A", "B"]);

    let newer = r#"{"formatVersion": 2, "setlists": ["B.setlist.json"]}"#;
    write_order(&dir, newer);
    let error = library.move_setlist("A.setlist.json", 2).unwrap_err();
    assert!(
        matches!(error, SetlistError::OrderVersion { found: 2, .. }),
        "{error}"
    );
    assert_eq!(fs::read_to_string(order_path(&dir)).unwrap(), newer);
}

#[test]
fn a_setlist_that_cannot_be_read_stays_in_the_stored_order() {
    let (dir, library) = library_with(&["A", "B", "C"]);
    library.move_setlist("C.setlist.json", 0).unwrap();
    let broken = fs::read(folder(&dir).join("B.setlist.json")).unwrap();
    fs::write(folder(&dir).join("B.setlist.json"), "{ not json").unwrap();

    library.move_setlist("A.setlist.json", 0).unwrap();
    assert_eq!(listed(&library), ["A", "C"]);

    // Fixed again, it comes back where it was in the stored order.
    fs::write(folder(&dir).join("B.setlist.json"), broken).unwrap();
    assert_eq!(listed(&library), ["A", "C", "B"]);
}

#[test]
fn a_failed_order_write_leaves_the_order_file_and_no_temporary_file() {
    let (dir, library) = library_with(&["A", "B"]);
    // A folder in the order file's place cannot be written over.
    fs::create_dir(order_path(&dir)).unwrap();
    fs::write(order_path(&dir).join("inside"), "kept").unwrap();
    let before = entries(&dir);

    let error = library.move_setlist("B.setlist.json", 0).unwrap_err();

    assert!(matches!(error, SetlistError::Write { .. }), "{error}");
    assert_eq!(entries(&dir), before, "no temporary file is left");
    assert_eq!(
        fs::read_to_string(order_path(&dir).join("inside")).unwrap(),
        "kept"
    );
    let listing = library.list_setlists().unwrap();
    assert_eq!(names(&listing), ["A", "B"]);
}

#[cfg(unix)]
#[test]
fn a_move_in_a_read_only_folder_fails_and_leaves_the_order_as_it_was() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, library) = library_with(&["A", "B"]);
    library.move_setlist("B.setlist.json", 0).unwrap();
    let order = fs::read(order_path(&dir)).unwrap();
    let before = entries(&dir);
    fs::set_permissions(folder(&dir), fs::Permissions::from_mode(0o555)).unwrap();
    let probe = fs::write(folder(&dir).join(".probe"), "");
    let result = library.move_setlist("A.setlist.json", 0);
    fs::set_permissions(folder(&dir), fs::Permissions::from_mode(0o755)).unwrap();
    if probe.is_ok() {
        // Running as root: the folder stays writable.
        return;
    }

    assert!(
        matches!(result, Err(SetlistError::Write { .. })),
        "{result:?}"
    );
    assert_eq!(fs::read(order_path(&dir)).unwrap(), order);
    assert_eq!(entries(&dir), before);
}

/// Fills every name the library's temporary file for the order file can
/// take (`.order.json.<pid>-<n>.tmp`, n from 0 to 99) with a folder, so
/// writing the order fails while setlists are still written.
fn block_order_writes(dir: &TempDir) {
    for attempt in 0..100 {
        let name = format!(".{SETLIST_ORDER_FILE}.{}-{attempt}.tmp", std::process::id());
        fs::create_dir(folder(dir).join(name)).unwrap();
    }
}

fn saved_file(error: &SetlistError) -> &str {
    match error {
        SetlistError::OrderNotUpdated { saved, .. } => &saved.file,
        other => panic!("OrderNotUpdated expected: {other}"),
    }
}

#[test]
fn a_setlist_saved_whose_place_cannot_be_stored_says_it_was_saved() {
    let (dir, library) = library_with(&["A", "B"]);
    library.move_setlist("B.setlist.json", 0).unwrap();
    let order = fs::read(order_path(&dir)).unwrap();
    block_order_writes(&dir);

    let error = library.save_setlist(&Setlist::new("C")).unwrap_err();
    assert_eq!(saved_file(&error), "C.setlist.json");
    assert!(
        error
            .to_string()
            .starts_with("setlist \"C\" was saved as C.setlist.json, but its place in the list could not be stored: "),
        "{error}"
    );
    assert_eq!(library.load_setlist("C.setlist.json").unwrap().name, "C");

    let error = library.rename_setlist("A.setlist.json", "Zed").unwrap_err();
    assert_eq!(saved_file(&error), "Zed.setlist.json");
    assert_eq!(
        library.load_setlist("Zed.setlist.json").unwrap().name,
        "Zed"
    );

    let error = library
        .duplicate_setlist("B.setlist.json", "B copy")
        .unwrap_err();
    assert_eq!(saved_file(&error), "B copy.setlist.json");

    // Every setlist is listed: the ones the order names first, the rest
    // by name; the order file is as it was.
    assert_eq!(listed(&library), ["B", "B copy", "C", "Zed"]);
    assert_eq!(fs::read(order_path(&dir)).unwrap(), order);
}

#[test]
fn a_move_far_past_the_end_is_refused_without_overflow() {
    let (_dir, library) = library_with(&["A", "B"]);
    let error = library
        .move_setlist("A.setlist.json", usize::MAX)
        .unwrap_err();
    assert!(matches!(error, SetlistError::MoveOutOfRange { .. }));
    assert!(
        error
            .to_string()
            .starts_with("cannot move a setlist to place ")
    );
}
