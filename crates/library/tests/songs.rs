//! F10.2: the songs of the library folder are listed from their manifests
//! and stem headers, with the paths a setlist stores, broken songs apart,
//! and nothing outside the library.

mod common;

use std::fs;
use std::path::Path;

use common::{manifest, write_int, write_manifest};
use domain::{
    Arrangement, SectionMarker, SectionType, Setlist, SetlistItem, SongItem, SongPackage, StemRole,
};
use library::{
    ArrangementEntry, Library, LoadError, PositionEntry, SetlistError, SongEntry, SongListing,
    SongUnavailable, load_song,
};

const RATE: u32 = 1_000;

/// A valid one-second song folder titled `title`, with the Default
/// arrangement of one position.
fn write_song(folder: &Path, title: &str) {
    fs::create_dir_all(folder).unwrap();
    write_int(&folder.join("track.wav"), 16, 1, RATE, &[0; 1_000]);
    let mut package = manifest(RATE, &[("track", "track.wav", StemRole::Track)]);
    package.title = title.to_owned();
    write_manifest(folder, &package);
}

/// A four-second song with two stems, a key, an artist, three sections and
/// two arrangements.
fn rich_package() -> SongPackage {
    let mut package = manifest(
        RATE,
        &[
            ("drums", "stems/drums.wav", StemRole::Track),
            ("click", "stems/click.wav", StemRole::Click),
        ],
    );
    package.title = "Example Song".to_owned();
    package.artist = Some("The Examples".to_owned());
    package.key = Some("A".to_owned());
    package.tempo_map[0].bpm = 132.0;
    let section = |id: &str, section_type, number, start| SectionMarker {
        id: id.to_owned(),
        section_type,
        number,
        start,
    };
    package.sections = vec![
        section("intro", SectionType::Intro, None, 0),
        section("verse", SectionType::Verse, Some(2), 1_000),
        section("pre", SectionType::PreChorus, None, 2_500),
    ];
    let arrangement = |id: &str, name: &str, sections: &[&str]| Arrangement {
        id: id.to_owned(),
        name: name.to_owned(),
        sections: sections.iter().map(|&s| s.to_owned()).collect(),
        infinite_loop: Vec::new(),
    };
    package.arrangements = vec![
        arrangement("default", "Default", &["intro", "verse", "pre"]),
        arrangement("short", "Short", &["verse", "verse"]),
    ];
    package
}

fn write_rich_song(folder: &Path) {
    fs::create_dir_all(folder).unwrap();
    write_int(&folder.join("stems/drums.wav"), 16, 1, RATE, &[0; 4_000]);
    write_int(&folder.join("stems/click.wav"), 16, 1, RATE, &[0; 3_000]);
    write_manifest(folder, &rich_package());
}

fn titles(listing: &SongListing) -> Vec<&str> {
    listing.songs.iter().map(|s| s.title.as_str()).collect()
}

fn paths(listing: &SongListing) -> Vec<&str> {
    listing.songs.iter().map(|s| s.song.as_str()).collect()
}

fn unreadable_paths(listing: &SongListing) -> Vec<&str> {
    listing.unreadable.iter().map(|s| s.song.as_str()).collect()
}

fn position(section_id: &str, label: &str, length_frames: u64) -> PositionEntry {
    PositionEntry {
        section_id: section_id.to_owned(),
        label: label.to_owned(),
        length_frames,
    }
}

#[test]
fn a_song_is_listed_with_what_its_manifest_and_stem_headers_hold() {
    let dir = tempfile::tempdir().unwrap();
    write_rich_song(&dir.path().join("Hymns/Example Song.song"));

    let listing = Library::new(dir.path()).list_songs().unwrap();

    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    assert_eq!(
        listing.songs,
        [SongEntry {
            song: "Hymns/Example Song.song".to_owned(),
            title: "Example Song".to_owned(),
            artist: Some("The Examples".to_owned()),
            key: Some("A".to_owned()),
            bpm: 132.0,
            sample_rate: RATE,
            stems: 2,
            length_frames: 4_000,
            arrangements: vec![
                ArrangementEntry {
                    id: "default".to_owned(),
                    name: "Default".to_owned(),
                    positions: vec![
                        position("intro", "Intro", 1_000),
                        position("verse", "Verse 2", 1_500),
                        position("pre", "Pre-chorus", 1_500),
                    ],
                },
                ArrangementEntry {
                    id: "short".to_owned(),
                    name: "Short".to_owned(),
                    positions: vec![
                        position("verse", "Verse 2", 1_500),
                        position("verse", "Verse 2", 1_500),
                    ],
                },
            ],
        }]
    );
    assert_eq!(listing.songs[0].arrangements[0].length_frames(), 4_000);
    assert_eq!(listing.songs[0].arrangements[1].length_frames(), 3_000);
}

#[test]
fn songs_at_any_depth_are_listed_by_title_and_other_folders_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_song(&root.join("Zebra.song"), "zebra");
    write_song(&root.join("Hymns/Old/Amazing.song"), "Amazing");
    write_song(&root.join("Hymns/Beta.song"), "beta");
    write_song(&root.join("Second Beta.song"), "beta");
    // A song folder is not looked into: a song inside it is not listed.
    write_song(&root.join("Zebra.song/Nested.song"), "Nested");
    // Hidden folders and the setlists folder are skipped.
    write_song(&root.join(".Trash/Gone.song"), "Gone");
    write_song(&root.join("Setlists/Odd.song"), "Odd");
    fs::create_dir_all(root.join("Notes")).unwrap();
    fs::write(root.join("Notes/readme.txt"), "not a song").unwrap();
    fs::write(root.join("loose.wav"), "not a song").unwrap();

    let listing = Library::new(root).list_songs().unwrap();

    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    assert_eq!(titles(&listing), ["Amazing", "beta", "beta", "zebra"]);
    assert_eq!(
        paths(&listing),
        [
            "Hymns/Old/Amazing.song",
            "Hymns/Beta.song",
            "Second Beta.song",
            "Zebra.song"
        ]
    );
}

#[test]
fn the_listed_paths_are_what_a_setlist_stores_and_the_library_resolves() {
    let dir = tempfile::tempdir().unwrap();
    write_song(&dir.path().join("Hymns/Old/Amazing.song"), "Amazing");
    write_rich_song(&dir.path().join("Example.song"));
    let library = Library::new(dir.path());

    let listing = library.list_songs().unwrap();
    let mut setlist = Setlist::new("Sunday");
    for song in &listing.songs {
        setlist.items.push(SetlistItem::Song(SongItem {
            arrangement: song.arrangements.last().unwrap().id.clone(),
            end_position: Some(song.arrangements.last().unwrap().positions.len() - 1),
            ..SongItem::new(song.song.clone())
        }));
    }
    let songs = library.setlist_songs(&setlist).unwrap();

    assert_eq!(setlist.validate(&songs), Ok(()));
    for song in &listing.songs {
        assert_eq!(songs.sample_rate(&song.song), Some(song.sample_rate));
        assert_eq!(
            library.song_folder(&song.song).unwrap(),
            fs::canonicalize(dir.path().join(&song.song)).unwrap()
        );
    }
    // Saved and read back, the setlist still names the same folders.
    let entry = library.save_setlist(&setlist).unwrap();
    assert_eq!(library.load_setlist(&entry.file).unwrap(), setlist);
}

#[test]
fn listing_reads_stem_headers_but_decodes_no_audio() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("Cut.song");
    write_song(&folder, "Cut short");
    // The header still says 1 000 frames, but only 50 are left: decoding
    // fails, reading the header does not.
    let stem = folder.join("track.wav");
    let bytes = fs::read(&stem).unwrap();
    fs::write(&stem, &bytes[..44 + 100]).unwrap();
    assert!(matches!(load_song(&folder), Err(LoadError::Decode { .. })));

    let listing = Library::new(dir.path()).list_songs().unwrap();

    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    assert_eq!(titles(&listing), ["Cut short"]);
    assert_eq!(listing.songs[0].length_frames, 1_000);
}

#[test]
fn a_broken_song_is_listed_apart_with_its_reason_and_hides_no_other() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_song(&root.join("Good.song"), "Good");
    fs::create_dir_all(root.join("Bad JSON.song")).unwrap();
    fs::write(root.join("Bad JSON.song/song.json"), "{ not json").unwrap();
    write_song(&root.join("Hymns/No Stem.song"), "No stem");
    fs::remove_file(root.join("Hymns/No Stem.song/track.wav")).unwrap();

    let listing = Library::new(root).list_songs().unwrap();

    assert_eq!(titles(&listing), ["Good"]);
    assert_eq!(
        unreadable_paths(&listing),
        ["Bad JSON.song", "Hymns/No Stem.song"]
    );
    assert!(matches!(
        &listing.unreadable[0].error,
        SongUnavailable::Load { song, source: LoadError::Manifest(_) } if song == "Bad JSON.song"
    ));
    assert!(matches!(
        &listing.unreadable[1].error,
        SongUnavailable::Load { song, source: LoadError::Invalid(_) } if song == "Hymns/No Stem.song"
    ));
    assert!(
        listing.unreadable[1]
            .error
            .to_string()
            .starts_with("song \"Hymns/No Stem.song\" cannot be loaded"),
        "{}",
        listing.unreadable[1].error
    );
}

#[test]
fn a_library_without_songs_lists_nothing_and_a_missing_one_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let listing = Library::new(dir.path()).list_songs().unwrap();
    assert!(listing.songs.is_empty());
    assert!(listing.unreadable.is_empty());

    let missing = Library::new(dir.path().join("Missing"));
    assert!(matches!(
        missing.list_songs(),
        Err(SetlistError::Folder { .. })
    ));
}

#[cfg(unix)]
#[test]
fn songs_outside_the_library_are_never_listed_or_read() {
    use std::os::unix::fs::symlink;

    let outer = tempfile::tempdir().unwrap();
    write_song(&outer.path().join("Outside.song"), "Outside");
    write_song(&outer.path().join("Elsewhere/Far.song"), "Far");
    let root = outer.path().join("Library");
    write_song(&root.join("Songs/Real.song"), "Real");
    // A link to a song folder outside: listed as unreadable, not read.
    symlink(outer.path().join("Outside.song"), root.join("Linked.song")).unwrap();
    // A link to a folder of songs outside: never followed.
    symlink(outer.path().join("Elsewhere"), root.join("Elsewhere")).unwrap();
    // A link back up to the library folder: never followed, so no loop.
    symlink(&root, root.join("Songs/Loop")).unwrap();
    // A song whose manifest links out of the library: not read.
    fs::create_dir_all(root.join("Sneaky.song")).unwrap();
    symlink(
        outer.path().join("Outside.song/song.json"),
        root.join("Sneaky.song/song.json"),
    )
    .unwrap();
    // A link to a song folder inside the library is listed under its own
    // path, which the library resolves.
    symlink(root.join("Songs/Real.song"), root.join("Alias.song")).unwrap();

    let listing = Library::new(&root).list_songs().unwrap();

    assert_eq!(paths(&listing), ["Alias.song", "Songs/Real.song"]);
    assert_eq!(unreadable_paths(&listing), ["Linked.song", "Sneaky.song"]);
    assert!(matches!(
        &listing.unreadable[0].error,
        SongUnavailable::OutsideLibrary { song } if song == "Linked.song"
    ));
    assert!(matches!(
        &listing.unreadable[1].error,
        SongUnavailable::OutsideLibrary { song } if song == "Sneaky.song"
    ));
}

#[cfg(unix)]
#[test]
fn a_fifo_manifest_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    write_song(&root.join("Good.song"), "Good");
    fs::create_dir_all(root.join("Pipe.song")).unwrap();
    let status = std::process::Command::new("mkfifo")
        .arg(root.join("Pipe.song/song.json"))
        .status()
        .unwrap();
    assert!(status.success());

    let (sender, receiver) = std::sync::mpsc::channel();
    let library = Library::new(&root);
    std::thread::spawn(move || {
        let _ = sender.send(library.list_songs());
    });
    let listing = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("returns without opening a FIFO")
        .unwrap();

    assert_eq!(titles(&listing), ["Good"]);
    assert_eq!(unreadable_paths(&listing), ["Pipe.song"]);
    let message = listing.unreadable[0].error.to_string();
    assert!(message.contains("not a regular file"), "{message}");
    assert!(
        matches!(
            listing.unreadable[0].error,
            SongUnavailable::Load {
                source: LoadError::ManifestNotAFile { .. },
                ..
            }
        ),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_read_is_listed_apart() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_song(&root.join("Good.song"), "Good");
    write_song(&root.join("Locked/Hidden.song"), "Hidden");
    fs::set_permissions(root.join("Locked"), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(root.join("Locked")).is_ok() {
        // Running as root: the folder stays readable.
        fs::set_permissions(root.join("Locked"), fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let listing = Library::new(root).list_songs();
    fs::set_permissions(root.join("Locked"), fs::Permissions::from_mode(0o755)).unwrap();
    let listing = listing.unwrap();

    assert_eq!(titles(&listing), ["Good"]);
    assert_eq!(unreadable_paths(&listing), ["Locked"]);
    assert!(matches!(
        &listing.unreadable[0].error,
        SongUnavailable::Load {
            source: LoadError::SongFolder { .. },
            ..
        }
    ));
}
