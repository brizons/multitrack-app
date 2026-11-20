//! The songs of the library folder (F10.2): every song folder in it, read
//! without decoding any audio, so a shell can offer them for a setlist.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::Path;

use domain::{SectionType, Song};

use crate::{
    Library, LoadError, MANIFEST_FILE, OpenFileError, SETLISTS_FOLDER, SetlistError,
    SongUnavailable, open_package, open_regular_file, resolve_inside,
};

/// A song folder that [`Library::list_songs`] could read, with what its
/// `song.json` holds.
#[derive(Debug, Clone, PartialEq)]
pub struct SongEntry {
    /// Path of the song folder relative to the library folder, with `/`
    /// between folders, such as `Hymns/Example Song.song`: what a setlist
    /// item stores ([`domain::SongItem::song`]).
    pub song: String,
    /// Song title.
    pub title: String,
    /// Artist, when the package names one.
    pub artist: Option<String>,
    /// Musical key, when the package names one.
    pub key: Option<String>,
    /// Tempo at the start of the song, in beats per minute.
    pub bpm: f64,
    /// Frames per second of the song.
    pub sample_rate: u32,
    /// Number of stems.
    pub stems: usize,
    /// Song length in frames: the length of the longest stem.
    pub length_frames: u64,
    /// Arrangements in package order.
    pub arrangements: Vec<ArrangementEntry>,
}

/// One arrangement of a [`SongEntry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrangementEntry {
    /// Arrangement id, what a setlist item stores
    /// ([`domain::SongItem::arrangement`]).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Its positions in play order; a setlist item's start and end points
    /// index into them.
    pub positions: Vec<PositionEntry>,
}

impl ArrangementEntry {
    /// Frames the whole arrangement plays.
    pub fn length_frames(&self) -> u64 {
        self.positions.iter().map(|p| p.length_frames).sum()
    }
}

/// One position of an [`ArrangementEntry`]: a section in play order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionEntry {
    /// Section id, as in the song package.
    pub section_id: String,
    /// The section type and its number, such as "Verse 2" or "Pre-chorus",
    /// in the words the live player's song description uses.
    pub label: String,
    /// Frames in the section.
    pub length_frames: u64,
}

/// A song folder that [`Library::list_songs`] could not read, or a folder
/// it could not look into.
#[derive(Debug)]
pub struct UnreadableSong {
    /// Path of the folder relative to the library folder, with `/` between
    /// folders.
    pub song: String,
    /// Why it cannot be read.
    pub error: SongUnavailable,
}

/// What [`Library::list_songs`] found.
#[derive(Debug, Default)]
pub struct SongListing {
    /// Readable songs, sorted by title ignoring case, then by title, then by
    /// path.
    pub songs: Vec<SongEntry>,
    /// Song folders that cannot be read, and folders that could not be
    /// looked into, sorted by path; one broken song hides no other.
    pub unreadable: Vec<UnreadableSong>,
}

impl Library {
    /// Lists the song folders of the library: every folder, at any depth,
    /// that holds a `song.json`. A song folder is not looked into further.
    ///
    /// Skipped: hidden entries (starting with `.`), names that are not
    /// UTF-8 (a setlist could not name them), the [`SETLISTS_FOLDER`], and
    /// symlinked folders without a `song.json` (never followed, so a link
    /// cannot lead the listing out of the library or round in a circle).
    ///
    /// Each song is read as [`Library::setlist_songs`] reads it: its folder
    /// must resolve inside the library folder, symlinks included; its
    /// `song.json` too, and it must be a regular file (a FIFO is refused
    /// without being opened); then the package is checked with its stem
    /// headers, without decoding any audio. A song that fails is listed in
    /// [`SongListing::unreadable`] with its reason, as is a folder that
    /// cannot be read.
    pub fn list_songs(&self) -> Result<SongListing, SetlistError> {
        let root = self.canonical_root()?;
        let mut listing = SongListing::default();
        self.list_folder(&root, &root, "", &mut listing)
            .map_err(|source| SetlistError::Folder {
                path: root.clone(),
                source,
            })?;
        listing.songs.sort_by(by_title);
        listing.unreadable.sort_by(|a, b| a.song.cmp(&b.song));
        Ok(listing)
    }

    /// Lists the songs in `folder`, at `prefix` inside the library folder
    /// `root` (canonical; `prefix` is empty for the library folder itself),
    /// and in the folders inside it. An error is about `folder` itself; one
    /// about a folder inside it is listed as unreadable.
    fn list_folder(
        &self,
        root: &Path,
        folder: &Path,
        prefix: &str,
        listing: &mut SongListing,
    ) -> io::Result<()> {
        for found in fs::read_dir(folder)? {
            let entry = found?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') || (prefix.is_empty() && name == SETLISTS_FOLDER) {
                continue;
            }
            let song = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let path = entry.path();
            if fs::symlink_metadata(path.join(MANIFEST_FILE)).is_ok() {
                match self.read_song(root, &song) {
                    Ok(entry) => listing.songs.push(entry),
                    Err(error) => listing.unreadable.push(UnreadableSong { song, error }),
                }
            } else if entry.file_type().is_ok_and(|kind| kind.is_dir())
                && let Err(source) = self.list_folder(root, &path, &song, listing)
            {
                listing.unreadable.push(UnreadableSong {
                    song: song.clone(),
                    error: SongUnavailable::Load {
                        song,
                        source: LoadError::SongFolder { path, source },
                    },
                });
            }
        }
        Ok(())
    }

    /// Reads the song folder at `song` inside the library folder `root`
    /// (canonical), as [`Library::setlist_songs`] reads it, after checking
    /// that its `song.json` resolves inside the library and is a regular
    /// file.
    fn read_song(&self, root: &Path, song: &str) -> Result<SongEntry, SongUnavailable> {
        let folder = self.song_folder(song)?;
        let load = |source| SongUnavailable::Load {
            song: song.to_owned(),
            source,
        };
        let manifest = match resolve_inside(root, Path::new(song).join(MANIFEST_FILE)) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => {
                return Err(SongUnavailable::OutsideLibrary {
                    song: song.to_owned(),
                });
            }
            Err(source) => {
                return Err(load(LoadError::ReadManifest {
                    path: folder.join(MANIFEST_FILE),
                    source,
                }));
            }
        };
        if let Err(error) = open_regular_file(&manifest) {
            let source = match error {
                OpenFileError::Io(error) => error,
                other => io::Error::new(io::ErrorKind::InvalidInput, other.to_string()),
            };
            return Err(load(LoadError::ReadManifest {
                path: manifest,
                source,
            }));
        }
        let (validated, _stems) = open_package(&folder).map_err(load)?;
        Ok(entry(song, &validated.song))
    }
}

/// What the listing shows of `song`, found at `path`.
fn entry(path: &str, song: &Song) -> SongEntry {
    let arrangements = song
        .arrangements()
        .iter()
        .map(|arrangement| ArrangementEntry {
            id: arrangement.id.clone(),
            name: arrangement.name.clone(),
            positions: arrangement
                .sections
                .iter()
                .filter_map(|id| song.section(id))
                .map(|section| PositionEntry {
                    section_id: section.id.clone(),
                    label: section_label(section.section_type, section.number),
                    length_frames: section.length(),
                })
                .collect(),
        })
        .collect();
    SongEntry {
        song: path.to_owned(),
        title: song.title().to_owned(),
        artist: song.artist().map(str::to_owned),
        key: song.key().map(str::to_owned),
        // A validated tempo map starts at 0, so it has a first point.
        bpm: song.tempo_map().first().map_or(0.0, |point| point.bpm),
        sample_rate: song.sample_rate(),
        stems: song.stems().len(),
        length_frames: song.length(),
        arrangements,
    }
}

/// "Verse 2", "Pre-chorus", "Count-off": the words the live player's song
/// description (`app::SongInfo`) uses.
fn section_label(section_type: SectionType, number: Option<u32>) -> String {
    let name = match section_type {
        SectionType::CountOff => "Count-off",
        SectionType::Intro => "Intro",
        SectionType::Verse => "Verse",
        SectionType::PreChorus => "Pre-chorus",
        SectionType::Chorus => "Chorus",
        SectionType::PostChorus => "Post-chorus",
        SectionType::Bridge => "Bridge",
        SectionType::Instrumental => "Instrumental",
        SectionType::Interlude => "Interlude",
        SectionType::Tag => "Tag",
        SectionType::Vamp => "Vamp",
        SectionType::Turnaround => "Turnaround",
        SectionType::Breakdown => "Breakdown",
        SectionType::Outro => "Outro",
        SectionType::Ending => "Ending",
    };
    match number {
        Some(number) => format!("{name} {number}"),
        None => name.to_owned(),
    }
}

/// Sorts entries by title ignoring case, then by title, then by path.
fn by_title(a: &SongEntry, b: &SongEntry) -> Ordering {
    a.title
        .to_lowercase()
        .cmp(&b.title.to_lowercase())
        .then_with(|| a.title.cmp(&b.title))
        .then_with(|| a.song.cmp(&b.song))
}
