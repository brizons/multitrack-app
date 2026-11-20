//! The songs a setlist names, read from the library folder so
//! [`Setlist::validate`] can check the setlist against them (S2).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use domain::{Setlist, SetlistItem, SongLookup, SongOutline};
use thiserror::Error;

use crate::{Library, LoadError, SetlistError, is_plain_relative, open_package, resolve_inside};

/// What the library holds for the songs a setlist names: the outline of
/// each song that loads, which [`Setlist::validate`] reads through
/// [`SongLookup`], and why each other song cannot be used. A song the library
/// lacks makes only its own items invalid; the rest of the setlist stays
/// usable.
#[derive(Debug, Default)]
pub struct SetlistSongs {
    outlines: BTreeMap<String, SongOutline>,
    unavailable: Vec<SongUnavailable>,
}

/// Why a song a setlist names cannot be used. `song` is the path as the
/// setlist writes it, relative to the library folder.
#[derive(Debug, Error)]
pub enum SongUnavailable {
    /// The path is empty, absolute, or has a `..` component, as this platform
    /// reads paths; nothing on disk was touched. [`Setlist::validate`]
    /// reports it as [`domain::SetlistValidationError::InvalidSongPath`].
    #[error("song \"{song}\" is not a relative path inside the library folder")]
    InvalidPath {
        /// The song path.
        song: String,
    },
    /// The path resolves, through symlinks or otherwise, outside the library
    /// folder; the song was not read.
    #[error("song \"{song}\" resolves outside the library folder")]
    OutsideLibrary {
        /// The song path.
        song: String,
    },
    /// The song folder is missing, or is not a valid song package; `source`
    /// lists every problem, as [`crate::load_song`] does.
    #[error("song \"{song}\" cannot be loaded: {source}")]
    Load {
        /// The song path.
        song: String,
        /// Why the folder does not load.
        source: LoadError,
    },
}

impl SongUnavailable {
    /// The song path, as the setlist writes it.
    pub fn song(&self) -> &str {
        match self {
            Self::InvalidPath { song }
            | Self::OutsideLibrary { song }
            | Self::Load { song, .. } => song,
        }
    }
}

impl SetlistSongs {
    /// The songs the setlist names that the library cannot provide, once
    /// each, in the order the setlist first names them.
    pub fn unavailable(&self) -> &[SongUnavailable] {
        &self.unavailable
    }
}

impl SongLookup for SetlistSongs {
    fn outline(&self, song: &str) -> Option<SongOutline> {
        self.outlines.get(song).cloned()
    }
}

impl Library {
    /// The folder of the song at `song`, a path relative to the library
    /// folder as a setlist writes it. The result is canonical and inside the
    /// library folder, symlinks included.
    pub fn song_folder(&self, song: &str) -> Result<PathBuf, SongUnavailable> {
        let root = fs::canonicalize(self.root()).map_err(|source| SongUnavailable::Load {
            song: song.to_owned(),
            source: LoadError::SongFolder {
                path: self.root().to_owned(),
                source,
            },
        })?;
        locate_song(&root, song)
    }

    /// Reads every song `setlist` names, once each, with the checks
    /// [`crate::load_song`] makes but without decoding any audio, so
    /// [`Setlist::validate`] can check the setlist against them. Song paths
    /// stay inside the library folder, symlinks included.
    pub fn setlist_songs(&self, setlist: &Setlist) -> Result<SetlistSongs, SetlistError> {
        let root = self.canonical_root()?;
        let mut songs = SetlistSongs::default();
        let mut seen = HashSet::new();
        for item in &setlist.items {
            let SetlistItem::Song(item) = item else {
                continue;
            };
            if !seen.insert(item.song.as_str()) {
                continue;
            }
            match song_outline(&root, &item.song) {
                Ok(outline) => {
                    songs.outlines.insert(item.song.clone(), outline);
                }
                Err(unavailable) => songs.unavailable.push(unavailable),
            }
        }
        Ok(songs)
    }
}

/// The canonical folder of `song` inside `root` (canonical). Nothing on disk
/// is touched for a path that is not plain relative.
fn locate_song(root: &Path, song: &str) -> Result<PathBuf, SongUnavailable> {
    if !is_plain_relative(song) {
        return Err(SongUnavailable::InvalidPath {
            song: song.to_owned(),
        });
    }
    match resolve_inside(root, song) {
        Ok(Some(folder)) => Ok(folder),
        Ok(None) => Err(SongUnavailable::OutsideLibrary {
            song: song.to_owned(),
        }),
        Err(source) => Err(SongUnavailable::Load {
            song: song.to_owned(),
            source: LoadError::SongFolder {
                path: root.join(song),
                source,
            },
        }),
    }
}

/// Reads and checks the song package at `song` as [`crate::load_song`]
/// does, opening its stems only to read their headers.
fn song_outline(root: &Path, song: &str) -> Result<SongOutline, SongUnavailable> {
    let folder = locate_song(root, song)?;
    let (validated, _stems) = open_package(&folder).map_err(|source| SongUnavailable::Load {
        song: song.to_owned(),
        source,
    })?;
    Ok(SongOutline::from(&validated.song))
}
