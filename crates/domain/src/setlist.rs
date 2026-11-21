//! Setlist format v1: "An ordered list of songs, each with its arrangement and
//! a transition to the next", plus click-only tiles (F5.1–F5.5).
//!
//! A setlist is a plain JSON file, like the song package, so other tools can
//! read and write it. This module only sees the setlist and what a
//! [`SongLookup`] says about the songs it names; it never touches files.
//!
//! # Start and end points
//!
//! A song item's start and end points are **arrangement positions**: indexes
//! into the arrangement's section list, counted from 0, both inclusive. A song
//! item with start 1 and end 2 plays the second and third sections of its
//! arrangement; without them it plays the whole arrangement. Positions are what
//! the players already apply exactly: [`crate::compile()`] lays out one
//! [`crate::Segment`] per position, and the live player addresses its
//! arrangement by position, entering a position only at its first sample and
//! leaving it at its last. A sample offset would start or stop inside a section,
//! which neither does, and would point somewhere else once the song's section
//! markers move.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::hash::BuildHasher;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::package::{
    DEFAULT_ARRANGEMENT_ID, MAX_BPM, MIN_BPM, StemPathProblem, VersionHeader, bpm_in_range,
    denominator_valid, numerator_valid, stem_path_problem,
};
use crate::song::Song;

/// The only setlist `formatVersion` this crate reads. Setlists and song
/// packages are versioned separately.
pub const SETLIST_FORMAT_VERSION: u32 = 1;

/// Length of a song item's fade-out at its end (F5.3).
pub const SONG_FADE_OUT_SECONDS: u32 = 5;

/// Length of the [`Transition::Crossfade`] blend over the end of a song (F5.4).
pub const CROSSFADE_SECONDS: u32 = 5;

/// A setlist file, exactly as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setlist {
    /// File format version; must be [`SETLIST_FORMAT_VERSION`].
    pub format_version: u32,
    /// Display name. **Assumption:** it must not be blank.
    pub name: String,
    /// Songs and click-only tiles in play order.
    pub items: Vec<SetlistItem>,
}

/// One entry of a setlist, stored with `"type": "song"` or `"type": "click"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SetlistItem {
    /// A song from the library.
    Song(SongItem),
    /// A click-only tile (F5.5).
    Click(ClickItem),
}

/// A song of the setlist with how it plays there (F5.2–F5.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongItem {
    /// Path of the song folder, relative to the library folder, such as
    /// `Hymns/Example Song.song`. [`SongLookup`] receives it as written.
    pub song: String,
    /// Id of the arrangement to play.
    pub arrangement: String,
    /// First arrangement position to play (see the [module docs](self));
    /// `None` starts at position 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_position: Option<usize>,
    /// Last arrangement position to play, inclusive; `None` plays to the end
    /// of the arrangement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_position: Option<usize>,
    /// Whether the song fades out over [`SONG_FADE_OUT_SECONDS`] at its end.
    #[serde(default)]
    pub fade_out: bool,
    /// What happens when the song ends.
    #[serde(default)]
    pub transition: Transition,
}

/// A click-only tile: a click at one tempo and time signature (F5.5).
///
/// **Assumption:** without a bar count it plays until the operator moves on;
/// with one it ends after that many bars and then follows its transition.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClickItem {
    /// Beats per minute, counting the time-signature denominator note, from
    /// [`MIN_BPM`] to [`MAX_BPM`] like a song's tempo.
    pub bpm: f64,
    /// Time signature, with the rules of a song's time-signature map.
    pub time_signature: TimeSignature,
    /// Number of bars to play; `None` plays until the operator moves on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<u32>,
    /// What happens when the tile ends.
    #[serde(default)]
    pub transition: Transition,
}

/// A time signature without a position: the meter of a click-only tile. It
/// follows the rules of [`crate::TimeSignaturePoint`], which also carries the
/// sample where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    /// Beats per bar; at least 1.
    pub numerator: u32,
    /// Note value of one beat (4 = quarter note): 1, 2, 4, 8, 16, or 32.
    pub denominator: u32,
}

/// What happens when an item ends (F5.4). Stored in camelCase (`cueNext`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transition {
    /// Stop and select the next item. **Assumption:** the default, as the
    /// first transition F5.4 lists.
    #[default]
    CueNext,
    /// Stop in the song.
    Stay,
    /// Start the next item immediately.
    AutoLink,
    /// Blend into the next item over the last [`CROSSFADE_SECONDS`].
    Crossfade,
}

/// What setlist validation needs to know about one song: its arrangements and
/// how many positions each plays.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SongOutline {
    /// Arrangements in package order.
    pub arrangements: Vec<ArrangementOutline>,
}

/// One arrangement of a [`SongOutline`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrangementOutline {
    /// Arrangement id.
    pub id: String,
    /// Number of positions: the length of the arrangement's section list,
    /// counting every repeat.
    pub positions: usize,
}

/// Answers which songs exist for [`Setlist::validate`], without I/O: the
/// library layer reads the songs first and hands over what it found.
pub trait SongLookup {
    /// The outline of the song at `song`, a path relative to the library
    /// folder exactly as the setlist writes it, or `None` when the library has
    /// no such song.
    fn outline(&self, song: &str) -> Option<SongOutline>;
}

impl<S: BuildHasher> SongLookup for HashMap<String, SongOutline, S> {
    fn outline(&self, song: &str) -> Option<SongOutline> {
        self.get(song).cloned()
    }
}

impl SongLookup for BTreeMap<String, SongOutline> {
    fn outline(&self, song: &str) -> Option<SongOutline> {
        self.get(song).cloned()
    }
}

impl From<&Song> for SongOutline {
    fn from(song: &Song) -> Self {
        Self {
            arrangements: song
                .arrangements()
                .iter()
                .map(|arrangement| ArrangementOutline {
                    id: arrangement.id.clone(),
                    positions: arrangement.sections.len(),
                })
                .collect(),
        }
    }
}

impl SongOutline {
    /// Number of positions of the arrangement `arrangement`, or `None` when
    /// the song has no arrangement with that id.
    pub fn positions(&self, arrangement: &str) -> Option<usize> {
        self.arrangements
            .iter()
            .find(|outline| outline.id == arrangement)
            .map(|outline| outline.positions)
    }
}

/// Which of a song item's points an error refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionBound {
    /// [`SongItem::start_position`].
    Start,
    /// [`SongItem::end_position`].
    End,
}

impl fmt::Display for PositionBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Start => "start",
            Self::End => "end",
        })
    }
}

/// A setlist file could not be parsed or written.
#[derive(Debug, Error)]
pub enum SetlistFileError {
    /// The file declares a `formatVersion` this crate cannot read; the rest of
    /// its shape is not looked at.
    #[error(
        "setlist: unsupported formatVersion {found}; this version reads formatVersion {SETLIST_FORMAT_VERSION}"
    )]
    UnsupportedFormatVersion {
        /// The version in the file.
        found: u32,
    },
    /// The JSON is malformed or does not have the v1 shape.
    #[error("setlist: {0}")]
    Json(serde_json::Error),
    /// A click-only tile's tempo is not a finite number; JSON cannot hold it,
    /// so the setlist is not written. `item` is the index into
    /// [`Setlist::items`].
    #[error("setlist: item {} has a tempo that is not a finite number", .item + 1)]
    NonFiniteBpm {
        /// The index of the click-only tile.
        item: usize,
    },
}

/// A rule the setlist breaks; [`Setlist::validate`] reports all of them at
/// once. `item` is the index into [`Setlist::items`]; messages count items
/// from 1, as an operator does.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SetlistValidationError {
    /// `formatVersion` is not [`SETLIST_FORMAT_VERSION`].
    #[error(
        "unsupported formatVersion {found}; this version reads formatVersion {SETLIST_FORMAT_VERSION}"
    )]
    UnsupportedFormatVersion {
        /// The version in the setlist.
        found: u32,
    },
    /// The setlist name is empty or only whitespace.
    #[error("the setlist name is empty")]
    EmptyName,
    /// A song path is empty, absolute, or leaves the library folder; the
    /// library is not asked about it.
    #[error(
        "item {} song \"{song}\" {problem}; it must be a relative path inside the library folder",
        .item + 1
    )]
    InvalidSongPath {
        /// Index of the item.
        item: usize,
        /// The rejected path.
        song: String,
        /// What is wrong with it.
        problem: StemPathProblem,
    },
    /// The library has no song at this path.
    #[error("item {} names song \"{song}\", which is not in the library", .item + 1)]
    UnknownSong {
        /// Index of the item.
        item: usize,
        /// The song path.
        song: String,
    },
    /// The song has no arrangement with this id.
    #[error(
        "item {} names arrangement \"{arrangement}\", which song \"{song}\" does not have",
        .item + 1
    )]
    UnknownArrangement {
        /// Index of the item.
        item: usize,
        /// The song path.
        song: String,
        /// The unknown arrangement id.
        arrangement: String,
    },
    /// The start position comes after the end position.
    #[error(
        "item {} has start position {start} after its end position {end}",
        .item + 1
    )]
    StartAfterEnd {
        /// Index of the item.
        item: usize,
        /// The start position.
        start: usize,
        /// The end position.
        end: usize,
    },
    /// A start or end position is past the last position of the arrangement.
    #[error(
        "item {} {bound} position {position} is outside arrangement \"{arrangement}\" of song \"{song}\", which has {positions} positions counted from 0",
        .item + 1
    )]
    PositionOutsideArrangement {
        /// Index of the item.
        item: usize,
        /// Which point is outside.
        bound: PositionBound,
        /// The rejected position.
        position: usize,
        /// The song path.
        song: String,
        /// The arrangement id.
        arrangement: String,
        /// Number of positions the arrangement has.
        positions: usize,
    },
    /// A click tile's tempo is not a finite number from [`MIN_BPM`] to
    /// [`MAX_BPM`].
    #[error(
        "item {} (click-only) has bpm {bpm}; bpm must be from {MIN_BPM} to {MAX_BPM}",
        .item + 1
    )]
    InvalidClickBpm {
        /// Index of the item.
        item: usize,
        /// The rejected tempo.
        bpm: f64,
    },
    /// A click tile's time signature has numerator 0.
    #[error(
        "item {} (click-only) has time-signature numerator 0; it must be at least 1",
        .item + 1
    )]
    InvalidClickNumerator {
        /// Index of the item.
        item: usize,
    },
    /// A click tile's time-signature denominator is not 1, 2, 4, 8, 16, or 32.
    #[error(
        "item {} (click-only) has time-signature denominator {denominator}; it must be 1, 2, 4, 8, 16, or 32",
        .item + 1
    )]
    InvalidClickDenominator {
        /// Index of the item.
        item: usize,
        /// The rejected denominator.
        denominator: u32,
    },
    /// A click tile asks for 0 bars.
    #[error(
        "item {} (click-only) has 0 bars; give at least 1, or no bar count to play until the operator moves on",
        .item + 1
    )]
    ZeroClickBars {
        /// Index of the item.
        item: usize,
    },
}

/// Why an edit was refused; the setlist is left unchanged. `index` is the
/// index into [`Setlist::items`], counted from 0.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SetlistEditError {
    /// No item has this index (for an insert: the index is past the end).
    #[error("item index {index} is out of range for a setlist of {len} items")]
    IndexOutOfRange {
        /// The rejected index.
        index: usize,
        /// Number of items in the setlist.
        len: usize,
    },
    /// The edit applies to song items only.
    #[error(
        "item index {index} is a click-only tile; only songs have an arrangement, start and end positions, and a fade-out"
    )]
    NotASong {
        /// Index of the click tile.
        index: usize,
    },
    /// The new name is empty or only whitespace.
    #[error("a setlist name cannot be empty")]
    EmptyName,
    /// A duplicate was asked to keep the original's name.
    #[error("the duplicate needs a name other than \"{name}\"")]
    SameName {
        /// The original's name.
        name: String,
    },
}

impl SongItem {
    /// The song at `song` playing the Default arrangement in full, with no
    /// fade-out and the default [`Transition`].
    pub fn new(song: impl Into<String>) -> Self {
        Self {
            song: song.into(),
            arrangement: DEFAULT_ARRANGEMENT_ID.to_owned(),
            start_position: None,
            end_position: None,
            fade_out: false,
            transition: Transition::default(),
        }
    }
}

impl ClickItem {
    /// A click tile without a bar count and with the default [`Transition`].
    pub fn new(bpm: f64, time_signature: TimeSignature) -> Self {
        Self {
            bpm,
            time_signature,
            bars: None,
            transition: Transition::default(),
        }
    }
}

impl SetlistItem {
    /// What happens when the item ends.
    pub fn transition(&self) -> Transition {
        match self {
            Self::Song(song) => song.transition,
            Self::Click(click) => click.transition,
        }
    }
}

impl Setlist {
    /// An empty setlist named `name`, at [`SETLIST_FORMAT_VERSION`].
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: SETLIST_FORMAT_VERSION,
            name: name.into(),
            items: Vec::new(),
        }
    }

    /// Parses a setlist file. `formatVersion` is checked first, so a newer
    /// file reports its version instead of a shape mismatch. Only the JSON
    /// shape is checked here; call [`Setlist::validate`] for the rules.
    pub fn from_json(json: &str) -> Result<Self, SetlistFileError> {
        let header: VersionHeader = serde_json::from_str(json).map_err(SetlistFileError::Json)?;
        if header.format_version != SETLIST_FORMAT_VERSION {
            return Err(SetlistFileError::UnsupportedFormatVersion {
                found: header.format_version,
            });
        }
        serde_json::from_str(json).map_err(SetlistFileError::Json)
    }

    /// Writes the setlist as pretty-printed JSON. A click-only tile whose
    /// tempo is not a finite number is refused, since JSON cannot hold it.
    pub fn to_json(&self) -> Result<String, SetlistFileError> {
        // serde_json writes a non-finite float as `null`, which `from_json`
        // then refuses: refuse to write it instead.
        if let Some(item) = self
            .items
            .iter()
            .position(|item| matches!(item, SetlistItem::Click(click) if !click.bpm.is_finite()))
        {
            return Err(SetlistFileError::NonFiniteBpm { item });
        }
        serde_json::to_string_pretty(self).map_err(SetlistFileError::Json)
    }

    /// Checks every setlist rule against the songs `songs` knows and returns
    /// all broken rules at once, in item order. A missing song is reported for
    /// its item and hides nothing else.
    pub fn validate<L: SongLookup + ?Sized>(
        &self,
        songs: &L,
    ) -> Result<(), Vec<SetlistValidationError>> {
        let mut errors = Vec::new();
        if self.format_version != SETLIST_FORMAT_VERSION {
            errors.push(SetlistValidationError::UnsupportedFormatVersion {
                found: self.format_version,
            });
        }
        if self.name.trim().is_empty() {
            errors.push(SetlistValidationError::EmptyName);
        }
        for (index, item) in self.items.iter().enumerate() {
            match item {
                SetlistItem::Song(song) => check_song(index, song, songs, &mut errors),
                SetlistItem::Click(click) => check_click(index, click, &mut errors),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Renames the setlist (F5.1).
    pub fn rename(&mut self, name: impl Into<String>) -> Result<(), SetlistEditError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SetlistEditError::EmptyName);
        }
        self.name = name;
        Ok(())
    }

    /// A copy of the setlist named `name`, which must differ from this
    /// setlist's name (F5.1).
    pub fn duplicate(&self, name: impl Into<String>) -> Result<Setlist, SetlistEditError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(SetlistEditError::EmptyName);
        }
        if name == self.name {
            return Err(SetlistEditError::SameName {
                name: self.name.clone(),
            });
        }
        Ok(Setlist {
            name,
            ..self.clone()
        })
    }

    /// Inserts `item` at `index`, from 0 to the number of items (the end).
    pub fn insert_item(&mut self, index: usize, item: SetlistItem) -> Result<(), SetlistEditError> {
        if index > self.items.len() {
            return Err(self.out_of_range(index));
        }
        self.items.insert(index, item);
        Ok(())
    }

    /// Removes and returns the item at `index`.
    pub fn remove_item(&mut self, index: usize) -> Result<SetlistItem, SetlistEditError> {
        self.check_index(index)?;
        Ok(self.items.remove(index))
    }

    /// Moves the item at `from` so it ends up at index `to`, shifting the
    /// items in between (F5.1).
    pub fn move_item(&mut self, from: usize, to: usize) -> Result<(), SetlistEditError> {
        self.check_index(from)?;
        self.check_index(to)?;
        let item = self.items.remove(from);
        self.items.insert(to, item);
        Ok(())
    }

    /// Chooses the arrangement of the song at `index` (F5.2). Its start and
    /// end positions are cleared, because they count positions of the
    /// arrangement it played before.
    pub fn set_arrangement(
        &mut self,
        index: usize,
        arrangement: impl Into<String>,
    ) -> Result<(), SetlistEditError> {
        let song = self.song_mut(index)?;
        song.arrangement = arrangement.into();
        song.start_position = None;
        song.end_position = None;
        Ok(())
    }

    /// Sets the start and end positions of the song at `index` (F5.3); `None`
    /// plays from the first or to the last position. [`Setlist::validate`]
    /// checks them against the arrangement.
    pub fn set_start_end(
        &mut self,
        index: usize,
        start_position: Option<usize>,
        end_position: Option<usize>,
    ) -> Result<(), SetlistEditError> {
        let song = self.song_mut(index)?;
        song.start_position = start_position;
        song.end_position = end_position;
        Ok(())
    }

    /// Switches the fade-out at the end of the song at `index` (F5.3).
    pub fn set_fade_out(&mut self, index: usize, fade_out: bool) -> Result<(), SetlistEditError> {
        self.song_mut(index)?.fade_out = fade_out;
        Ok(())
    }

    /// Chooses the transition of the song or click tile at `index` (F5.4).
    pub fn set_transition(
        &mut self,
        index: usize,
        transition: Transition,
    ) -> Result<(), SetlistEditError> {
        self.check_index(index)?;
        match &mut self.items[index] {
            SetlistItem::Song(song) => song.transition = transition,
            SetlistItem::Click(click) => click.transition = transition,
        }
        Ok(())
    }

    fn out_of_range(&self, index: usize) -> SetlistEditError {
        SetlistEditError::IndexOutOfRange {
            index,
            len: self.items.len(),
        }
    }

    fn check_index(&self, index: usize) -> Result<(), SetlistEditError> {
        if index < self.items.len() {
            Ok(())
        } else {
            Err(self.out_of_range(index))
        }
    }

    fn song_mut(&mut self, index: usize) -> Result<&mut SongItem, SetlistEditError> {
        self.check_index(index)?;
        match &mut self.items[index] {
            SetlistItem::Song(song) => Ok(song),
            SetlistItem::Click(_) => Err(SetlistEditError::NotASong { index }),
        }
    }
}

/// Reports the song item's problems: its path, its song and arrangement, then
/// its start and end. The start-after-end rule needs no lookup, so it is
/// checked even when the song is missing.
fn check_song<L: SongLookup + ?Sized>(
    item: usize,
    entry: &SongItem,
    songs: &L,
    errors: &mut Vec<SetlistValidationError>,
) {
    let positions = song_positions(item, entry, songs, errors);
    if let (Some(start), Some(end)) = (entry.start_position, entry.end_position)
        && start > end
    {
        errors.push(SetlistValidationError::StartAfterEnd { item, start, end });
    }
    let Some(positions) = positions else {
        return;
    };
    for (bound, position) in [
        (PositionBound::Start, entry.start_position),
        (PositionBound::End, entry.end_position),
    ] {
        if let Some(position) = position
            && position >= positions
        {
            errors.push(SetlistValidationError::PositionOutsideArrangement {
                item,
                bound,
                position,
                song: entry.song.clone(),
                arrangement: entry.arrangement.clone(),
                positions,
            });
        }
    }
}

/// The number of positions of the item's arrangement, or `None` after
/// reporting why it is unknown.
fn song_positions<L: SongLookup + ?Sized>(
    item: usize,
    entry: &SongItem,
    songs: &L,
    errors: &mut Vec<SetlistValidationError>,
) -> Option<usize> {
    if let Some(problem) = stem_path_problem(&entry.song) {
        errors.push(SetlistValidationError::InvalidSongPath {
            item,
            song: entry.song.clone(),
            problem,
        });
        return None;
    }
    let Some(outline) = songs.outline(&entry.song) else {
        errors.push(SetlistValidationError::UnknownSong {
            item,
            song: entry.song.clone(),
        });
        return None;
    };
    let positions = outline.positions(&entry.arrangement);
    if positions.is_none() {
        errors.push(SetlistValidationError::UnknownArrangement {
            item,
            song: entry.song.clone(),
            arrangement: entry.arrangement.clone(),
        });
    }
    positions
}

fn check_click(item: usize, click: &ClickItem, errors: &mut Vec<SetlistValidationError>) {
    if !bpm_in_range(click.bpm) {
        errors.push(SetlistValidationError::InvalidClickBpm {
            item,
            bpm: click.bpm,
        });
    }
    if !numerator_valid(click.time_signature.numerator) {
        errors.push(SetlistValidationError::InvalidClickNumerator { item });
    }
    if !denominator_valid(click.time_signature.denominator) {
        errors.push(SetlistValidationError::InvalidClickDenominator {
            item,
            denominator: click.time_signature.denominator,
        });
    }
    if click.bars == Some(0) {
        errors.push(SetlistValidationError::ZeroClickBars { item });
    }
}
