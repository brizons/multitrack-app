//! S2 (L33), F5.1: a setlist is removed from the library's list by moving
//! its file to the system trash, and the stored order loses its entry.
//! These tests never touch the real trash: the library is given a trash
//! that moves the file into a `Trash` folder of the temporary library.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use domain::Setlist;
use library::{Library, SETLIST_ORDER_FILE, SETLISTS_FOLDER, SetlistError, SetlistListing};
use tempfile::TempDir;

/// The test trash: `LIBRARY/Trash`, beside the `Setlists` folder of the
/// file it is given.
fn test_trash(path: &Path) -> io::Result<()> {
    let library = path
        .parent()
        .and_then(Path::parent)
        .expect("a setlist file is in LIBRARY/Setlists");
    let trash = library.join("Trash");
    fs::create_dir_all(&trash)?;
    fs::rename(path, trash.join(path.file_name().expect("a file name")))
}

fn refusing_trash(_: &Path) -> io::Result<()> {
    Err(io::Error::other("the trash is not available"))
}

fn library_with(names: &[&str]) -> (TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path()).with_trash(test_trash);
    for name in names {
        library.save_setlist(&Setlist::new(*name)).unwrap();
    }
    (dir, library)
}

fn folder(dir: &TempDir) -> PathBuf {
    dir.path().join(SETLISTS_FOLDER)
}

fn names(listing: &SetlistListing) -> Vec<&str> {
    listing.setlists.iter().map(|s| s.name.as_str()).collect()
}

fn listed(library: &Library) -> Vec<String> {
    let listing = library.list_setlists().unwrap();
    assert!(listing.order_error.is_none(), "{:?}", listing.order_error);
    names(&listing).into_iter().map(str::to_owned).collect()
}

/// Every file of the library folder, at any depth, with its bytes.
fn every_file(dir: &TempDir) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut folders = vec![dir.path().to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            let name = path.strip_prefix(dir.path()).unwrap().display().to_string();
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file() {
                files.insert(name, fs::read(&path).unwrap());
            } else {
                files.insert(name, Vec::new());
            }
        }
    }
    files
}

#[test]
fn removing_moves_the_file_through_the_trash_and_it_is_no_longer_listed() {
    let (dir, library) = library_with(&["A", "B", "C"]);
    let bytes = fs::read(folder(&dir).join("B.setlist.json")).unwrap();

    let removed = library.remove_setlist("B.setlist.json").unwrap();

    assert_eq!(removed.name, "B");
    assert_eq!(removed.file, "B.setlist.json");
    assert_eq!(listed(&library), ["A", "C"]);
    assert!(!folder(&dir).join("B.setlist.json").exists());
    assert_eq!(
        fs::read(dir.path().join("Trash/B.setlist.json")).unwrap(),
        bytes,
        "the file went to the trash as it was"
    );
    // Without a stored order, removing writes none.
    assert!(!folder(&dir).join(SETLIST_ORDER_FILE).exists());
}

#[test]
fn a_stored_order_loses_the_removed_setlist_and_keeps_the_rest() {
    let (dir, library) = library_with(&["A", "B", "C"]);
    library.move_setlist("C.setlist.json", 0).unwrap();
    assert_eq!(listed(&library), ["C", "A", "B"]);

    library.remove_setlist("A.setlist.json").unwrap();

    assert_eq!(listed(&library), ["C", "B"]);
    let order = fs::read_to_string(folder(&dir).join(SETLIST_ORDER_FILE)).unwrap();
    assert!(!order.contains("A.setlist.json"), "{order}");
    // A setlist created afterwards still goes at the end.
    library.save_setlist(&Setlist::new("A")).unwrap();
    assert_eq!(listed(&library), ["C", "B", "A"]);
}

#[test]
fn names_that_are_not_removable_setlists_are_refused_and_nothing_moves() {
    let (dir, library) = library_with(&["A", "B"]);
    library.move_setlist("B.setlist.json", 0).unwrap();
    fs::write(folder(&dir).join("Broken.setlist.json"), "{ not json").unwrap();
    fs::write(folder(&dir).join(".hidden.setlist.json"), "{}").unwrap();
    let before = every_file(&dir);

    for (file, refused) in [
        ("../A.setlist.json", "InvalidFileName"),
        ("Setlists/A.setlist.json", "InvalidFileName"),
        (SETLIST_ORDER_FILE, "InvalidFileName"),
        (".hidden.setlist.json", "InvalidFileName"),
        ("", "InvalidFileName"),
        ("Gone.setlist.json", "NotListed"),
        // Names are compared exactly, case included, even where the disk
        // ignores case.
        ("a.setlist.json", "NotListed"),
        ("BROKEN.setlist.json", "NotListed"),
    ] {
        let error = library.remove_setlist(file).unwrap_err();
        let kind = match error {
            SetlistError::InvalidFileName { .. } => "InvalidFileName",
            SetlistError::Read { .. } => "Read",
            SetlistError::NotListed { .. } => "NotListed",
            ref other => panic!("{file:?}: {other}"),
        };
        assert_eq!(kind, refused, "{file:?}: {error}");
    }

    assert_eq!(every_file(&dir), before, "every file is byte-identical");
    assert!(!dir.path().join("Trash").exists());
}

#[cfg(unix)]
#[test]
fn a_fifo_or_a_symlink_out_of_the_library_is_refused_and_nothing_moves() {
    let (dir, library) = library_with(&["A"]);
    let status = std::process::Command::new("mkfifo")
        .arg(folder(&dir).join("Pipe.setlist.json"))
        .status()
        .unwrap();
    assert!(status.success());
    let outside = tempfile::tempdir().unwrap();
    let elsewhere = outside.path().join("Elsewhere.setlist.json");
    fs::write(&elsewhere, Setlist::new("Elsewhere").to_json().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, folder(&dir).join("Linked.setlist.json")).unwrap();
    let before = every_file(&dir);

    let (sender, receiver) = std::sync::mpsc::channel();
    let remover = library.clone();
    std::thread::spawn(move || {
        let _ = sender.send(remover.remove_setlist("Pipe.setlist.json"));
    });
    let error = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("returns without opening a FIFO")
        .unwrap_err();
    assert!(matches!(error, SetlistError::NotAFile { .. }), "{error}");
    let error = library.remove_setlist("Linked.setlist.json").unwrap_err();
    assert!(
        matches!(error, SetlistError::OutsideLibrary { .. }),
        "{error}"
    );

    assert_eq!(every_file(&dir), before);
    assert!(elsewhere.exists());
}

#[test]
fn a_trash_that_refuses_leaves_the_file_and_the_order_as_they_were() {
    let (dir, library) = library_with(&["A", "B"]);
    library.move_setlist("B.setlist.json", 0).unwrap();
    let before = every_file(&dir);
    let library = library.with_trash(refusing_trash);

    let error = library.remove_setlist("A.setlist.json").unwrap_err();

    assert!(matches!(error, SetlistError::Trash { .. }), "{error}");
    assert!(
        error
            .to_string()
            .ends_with("to the trash: the trash is not available"),
        "{error}"
    );
    assert_eq!(every_file(&dir), before);
    assert_eq!(listed(&library), ["B", "A"]);
}

/// Fills every name the temporary file for `order.json` can take, so
/// storing the order fails while the rest still works.
fn block_order_writes(dir: &TempDir) {
    for attempt in 0..100 {
        let name = format!(".{SETLIST_ORDER_FILE}.{}-{attempt}.tmp", std::process::id());
        fs::create_dir(folder(dir).join(name)).unwrap();
    }
}

#[test]
fn a_setlist_removed_whose_entry_stays_in_the_order_says_it_was_removed() {
    let (dir, library) = library_with(&["A", "B"]);
    library.move_setlist("B.setlist.json", 0).unwrap();
    let order = fs::read(folder(&dir).join(SETLIST_ORDER_FILE)).unwrap();
    block_order_writes(&dir);

    let error = library.remove_setlist("A.setlist.json").unwrap_err();

    match &error {
        SetlistError::RemovedOrderNotUpdated { removed, .. } => {
            assert_eq!(removed.file, "A.setlist.json");
        }
        other => panic!("RemovedOrderNotUpdated expected: {other}"),
    }
    assert!(
        error.to_string().starts_with(
            "setlist \"A\" was moved to the trash, but the order of the list could not be updated: "
        ),
        "{error}"
    );
    assert!(dir.path().join("Trash/A.setlist.json").exists());
    assert_eq!(
        listed(&library),
        ["B"],
        "a name left in the order is ignored"
    );
    assert_eq!(
        fs::read(folder(&dir).join(SETLIST_ORDER_FILE)).unwrap(),
        order
    );
}

/// Uses the real system trash: run by hand with `cargo test -p library
/// --test setlist_remove -- --ignored`, then empty the trash.
#[test]
#[ignore = "moves a file into the real system trash"]
fn removing_with_the_system_trash_takes_the_file_out_of_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path());
    library
        .save_setlist(&Setlist::new("OpenVamp trash test"))
        .unwrap();

    library
        .remove_setlist("OpenVamp trash test.setlist.json")
        .unwrap();

    assert!(library.list_setlists().unwrap().setlists.is_empty());
    assert!(
        !dir.path()
            .join(SETLISTS_FOLDER)
            .join("OpenVamp trash test.setlist.json")
            .exists()
    );
}

#[test]
fn a_setlist_file_that_cannot_be_read_can_still_be_removed_from_the_list() {
    let (dir, library) = library_with(&["A"]);
    fs::write(folder(&dir).join("Broken.setlist.json"), "{ not json").unwrap();
    assert_eq!(library.list_setlists().unwrap().unreadable.len(), 1);

    let removed = library.remove_setlist("Broken.setlist.json").unwrap();

    assert_eq!(removed.file, "Broken.setlist.json");
    assert_eq!(removed.name, "Broken.setlist.json", "named by its file");
    let listing = library.list_setlists().unwrap();
    assert!(listing.unreadable.is_empty());
    assert_eq!(names(&listing), ["A"]);
    assert_eq!(
        fs::read_to_string(dir.path().join("Trash/Broken.setlist.json")).unwrap(),
        "{ not json"
    );
}

#[test]
fn a_setlist_file_name_is_found_as_the_folder_lists_it() {
    let (dir, library) = library_with(&["Bravo"]);
    assert_eq!(
        library.listed_setlist_file("Bravo.setlist.json").unwrap(),
        Some("Bravo.setlist.json".to_owned())
    );
    assert_eq!(
        library.listed_setlist_file("Gone.setlist.json").unwrap(),
        None
    );
    let other_case = library.listed_setlist_file("bravo.setlist.json").unwrap();
    if folder(&dir).join("bravo.setlist.json").exists() {
        // A file system that ignores case: the same file, as listed.
        assert_eq!(other_case, Some("Bravo.setlist.json".to_owned()));
    } else {
        assert_eq!(other_case, None);
    }
    assert!(matches!(
        library.listed_setlist_file("../Bravo.setlist.json"),
        Err(SetlistError::InvalidFileName { .. })
    ));
}
