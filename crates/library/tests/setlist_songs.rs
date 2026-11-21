//! S2: a setlist is checked against the songs in the library folder; a song
//! the library lacks is reported by its path and the rest stays usable.

mod common;

use std::fs;
use std::path::Path;

use common::{manifest, write_int, write_manifest};
use domain::{
    ArrangementOutline, ClickItem, Setlist, SetlistItem, SetlistValidationError, SongItem,
    SongLookup, SongOutline, StemPathProblem, StemRole, TimeSignature,
};
use library::{Library, LoadError, SongProblem, SongUnavailable};

const RATE: u32 = 1_000;

/// A valid one-second song folder with the Default arrangement of one
/// position.
fn write_song(folder: &Path) {
    fs::create_dir_all(folder).unwrap();
    write_int(&folder.join("track.wav"), 16, 1, RATE, &[0; 1_000]);
    write_manifest(
        folder,
        &manifest(RATE, &[("track", "track.wav", StemRole::Track)]),
    );
}

fn song(path: &str) -> SetlistItem {
    SetlistItem::Song(SongItem::new(path))
}

fn setlist(items: Vec<SetlistItem>) -> Setlist {
    let mut setlist = Setlist::new("Sunday");
    setlist.items = items;
    setlist
}

fn unknown_song(item: usize, song: &str) -> SetlistValidationError {
    SetlistValidationError::UnknownSong {
        item,
        song: song.to_owned(),
    }
}

#[test]
fn a_missing_song_is_reported_by_path_and_the_rest_of_the_setlist_still_validates() {
    let dir = tempfile::tempdir().unwrap();
    write_song(&dir.path().join("Songs/Present.song"));
    let library = Library::new(dir.path());
    let setlist = setlist(vec![
        song("Songs/Present.song"),
        song("Songs/Missing.song"),
        SetlistItem::Click(ClickItem::new(
            120.0,
            TimeSignature {
                numerator: 4,
                denominator: 4,
            },
        )),
        SetlistItem::Song(SongItem {
            arrangement: "acoustic".to_owned(),
            ..SongItem::new("Songs/Present.song")
        }),
        song("Songs/Missing.song"),
    ]);

    let songs = library.setlist_songs(&setlist).unwrap();

    assert_eq!(
        setlist.validate(&songs),
        Err(vec![
            unknown_song(1, "Songs/Missing.song"),
            SetlistValidationError::UnknownArrangement {
                item: 3,
                song: "Songs/Present.song".to_owned(),
                arrangement: "acoustic".to_owned(),
            },
            unknown_song(4, "Songs/Missing.song"),
        ])
    );
    assert_eq!(
        songs.outline("Songs/Present.song"),
        Some(SongOutline {
            arrangements: vec![ArrangementOutline {
                id: "default".to_owned(),
                positions: 1,
            }],
        })
    );
    let unavailable = songs.unavailable();
    assert_eq!(unavailable.len(), 1, "{unavailable:?}");
    assert_eq!(unavailable[0].song(), "Songs/Missing.song");
    assert!(
        matches!(
            unavailable[0],
            SongUnavailable::Load {
                source: LoadError::SongFolder { .. },
                ..
            }
        ),
        "{}",
        unavailable[0]
    );
    assert!(unavailable[0].to_string().contains("Songs/Missing.song"));
}

#[test]
fn a_song_folder_that_does_not_load_is_unavailable_with_its_problems() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("Broken.song");
    fs::create_dir(&broken).unwrap();
    write_manifest(
        &broken,
        &manifest(RATE, &[("gone", "gone.wav", StemRole::Track)]),
    );
    fs::create_dir(dir.path().join("Empty.song")).unwrap();
    let library = Library::new(dir.path());
    let setlist = setlist(vec![song("Broken.song"), song("Empty.song")]);

    let songs = library.setlist_songs(&setlist).unwrap();

    assert_eq!(
        setlist.validate(&songs),
        Err(vec![
            unknown_song(0, "Broken.song"),
            unknown_song(1, "Empty.song")
        ])
    );
    match songs.unavailable() {
        [
            SongUnavailable::Load {
                song: broken,
                source: LoadError::Invalid(problems),
            },
            SongUnavailable::Load {
                song: empty,
                source: LoadError::ReadManifest { .. },
            },
        ] => {
            assert_eq!(
                (broken.as_str(), empty.as_str()),
                ("Broken.song", "Empty.song")
            );
            assert!(
                matches!(&problems[..], [SongProblem::StemUnreadable { id, .. }] if id == "gone"),
                "{problems:?}"
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn song_paths_that_leave_the_library_folder_are_not_read() {
    let outer = tempfile::tempdir().unwrap();
    write_song(&outer.path().join("Outside.song"));
    let root = outer.path().join("Library");
    fs::create_dir(&root).unwrap();
    let library = Library::new(&root);
    let setlist = setlist(vec![song("../Outside.song")]);

    let songs = library.setlist_songs(&setlist).unwrap();

    assert_eq!(
        setlist.validate(&songs),
        Err(vec![SetlistValidationError::InvalidSongPath {
            item: 0,
            song: "../Outside.song".to_owned(),
            problem: StemPathProblem::ParentDirectory,
        }])
    );
    assert!(matches!(
        songs.unavailable(),
        [SongUnavailable::InvalidPath { song }] if song == "../Outside.song"
    ));
    assert!(matches!(
        library.song_folder("../Outside.song"),
        Err(SongUnavailable::InvalidPath { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_song_linking_out_of_the_library_folder_is_not_read() {
    let outer = tempfile::tempdir().unwrap();
    write_song(&outer.path().join("Outside.song"));
    let root = outer.path().join("Library");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(outer.path().join("Outside.song"), root.join("Linked.song"))
        .unwrap();
    let library = Library::new(&root);
    let setlist = setlist(vec![song("Linked.song")]);

    let songs = library.setlist_songs(&setlist).unwrap();

    assert_eq!(
        setlist.validate(&songs),
        Err(vec![unknown_song(0, "Linked.song")])
    );
    assert!(matches!(
        songs.unavailable(),
        [SongUnavailable::OutsideLibrary { song }] if song == "Linked.song"
    ));
    assert!(matches!(
        library.song_folder("Linked.song"),
        Err(SongUnavailable::OutsideLibrary { .. })
    ));
}

#[test]
fn song_folder_resolves_a_song_inside_the_library() {
    let dir = tempfile::tempdir().unwrap();
    write_song(&dir.path().join("Songs/Present.song"));
    let library = Library::new(dir.path());

    assert_eq!(
        library.song_folder("Songs/Present.song").unwrap(),
        fs::canonicalize(dir.path().join("Songs/Present.song")).unwrap()
    );
    assert!(matches!(
        library.song_folder("Songs/Missing.song"),
        Err(SongUnavailable::Load {
            source: LoadError::SongFolder { .. },
            ..
        })
    ));
}

/// S3: a setlist plays on one device rate, so the songs' rates are known
/// before it starts.
#[test]
fn each_song_that_loads_reports_its_sample_rate_and_a_missing_one_none() {
    let dir = tempfile::tempdir().unwrap();
    write_song(&dir.path().join("Songs/Present.song"));
    let other = dir.path().join("Songs/Other.song");
    fs::create_dir_all(&other).unwrap();
    write_int(&other.join("track.wav"), 16, 1, 2_000, &[0; 2_000]);
    write_manifest(
        &other,
        &manifest(2_000, &[("track", "track.wav", StemRole::Track)]),
    );
    let library = Library::new(dir.path());
    let setlist = setlist(vec![
        song("Songs/Present.song"),
        song("Songs/Other.song"),
        song("Songs/Missing.song"),
    ]);

    let songs = library.setlist_songs(&setlist).unwrap();

    assert_eq!(songs.sample_rate("Songs/Present.song"), Some(RATE));
    assert_eq!(songs.sample_rate("Songs/Other.song"), Some(2_000));
    assert_eq!(songs.sample_rate("Songs/Missing.song"), None);
    assert_eq!(songs.sample_rate("Songs/Never Named.song"), None);
}
