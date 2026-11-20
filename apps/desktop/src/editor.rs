//! The setlist editor (F5.1–F5.5): changes one setlist of a library folder
//! with the `domain` crate's edits, checks it against the library's songs
//! after every change, and saves it through the `library` crate, which
//! writes the file atomically.

use std::fmt;

use domain::{
    ClickItem, Setlist, SetlistEditError, SetlistItem, SetlistValidationError, SongItem,
    TimeSignature, Transition,
};
use library::{Library, SetlistError};

use crate::shelf::saved;

/// A click-only tile as the editor adds it: 120 BPM in 4/4, until the
/// operator moves on.
pub fn new_click() -> ClickItem {
    ClickItem::new(
        120.0,
        TimeSignature {
            numerator: 4,
            denominator: 4,
        },
    )
}

/// A change to the setlist; indexes count items from 0.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Adds the song at this path (relative to the library folder) at the
    /// end, playing its Default arrangement in full (F5.1).
    AddSong(String),
    /// Adds a click-only tile at the end ([`new_click`], F5.5).
    AddClick,
    /// Removes an item.
    Remove(usize),
    /// Moves an item so it ends up at index `to` (F5.1).
    Move {
        /// The item.
        from: usize,
        /// Where it goes.
        to: usize,
    },
    /// Chooses a song's arrangement, which clears its start and end (F5.2).
    Arrangement(usize, String),
    /// Sets a song's start and end positions; `None` plays from the first
    /// or to the last (F5.3).
    StartEnd(usize, Option<usize>, Option<usize>),
    /// Switches a song's five-second fade-out at its end (F5.3).
    FadeOut(usize, bool),
    /// Chooses an item's transition (F5.4).
    Transition(usize, Transition),
    /// Sets a click-only tile's tempo, meter and bar count (F5.5).
    Click(usize, ClickItem),
}

/// Why a change was not made; the setlist and its file are as they were.
#[derive(Debug)]
pub enum EditError {
    /// The setlist refused the edit.
    Edit(SetlistEditError),
    /// The item is a song, not a click-only tile.
    NotAClick(usize),
    /// The setlist could not be saved.
    Save(SetlistError),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Edit(error) => error.fmt(f),
            Self::NotAClick(item) => {
                write!(f, "item {} is a song, not a click-only tile", item + 1)
            }
            Self::Save(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for EditError {}

impl From<SetlistEditError> for EditError {
    fn from(error: SetlistEditError) -> Self {
        Self::Edit(error)
    }
}

impl From<SetlistError> for EditError {
    fn from(error: SetlistError) -> Self {
        Self::Save(error)
    }
}

/// One setlist being edited.
#[derive(Debug)]
pub struct SetlistEditor {
    library: Library,
    file: String,
    setlist: Setlist,
    problems: Vec<SetlistValidationError>,
    /// Whether the file has been saved under the name the setlist maps to;
    /// a file named by hand is moved there before the first save, so the
    /// setlist never ends up in two files.
    in_place: bool,
}

impl SetlistEditor {
    /// Opens the setlist in `file` of `library` and checks it.
    pub fn open(library: Library, file: &str) -> Result<Self, SetlistError> {
        let setlist = library.load_setlist(file)?;
        let mut editor = Self {
            library,
            file: file.to_owned(),
            setlist,
            problems: Vec::new(),
            in_place: false,
        };
        editor.check();
        Ok(editor)
    }

    /// The setlist file inside the library's setlists folder.
    pub fn file(&self) -> &str {
        &self.file
    }

    /// The setlist as saved.
    pub fn setlist(&self) -> &Setlist {
        &self.setlist
    }

    /// Every rule the setlist breaks against the library's songs, in item
    /// order, as the `domain` crate reports them.
    pub fn problems(&self) -> &[SetlistValidationError] {
        &self.problems
    }

    /// Makes `edit` and saves the setlist; on an error nothing changes. A
    /// warning comes back when the setlist was saved but its place in the
    /// stored order was not ([`crate::shelf::saved`]).
    pub fn apply(&mut self, edit: Edit) -> Result<Option<String>, EditError> {
        let mut changed = self.setlist.clone();
        apply(&mut changed, edit)?;
        let warning = self.save(&changed)?;
        self.setlist = changed;
        self.check();
        Ok(warning)
    }

    /// Renames the setlist (F5.1), moving its file to the one the new name
    /// maps to; a warning comes back as for [`Self::apply`].
    pub fn rename(&mut self, name: &str) -> Result<Option<String>, SetlistError> {
        let (entry, warning) = saved(self.library.rename_setlist(&self.file, name))?;
        self.file = entry.file;
        self.setlist.name = entry.name;
        self.in_place = true;
        self.check();
        Ok(warning)
    }

    /// Writes `setlist` to the file its name maps to. The first time, a
    /// file named otherwise (by hand) is moved there first, so saving does
    /// not leave the old file behind.
    fn save(&mut self, setlist: &Setlist) -> Result<Option<String>, SetlistError> {
        let mut moved = None;
        if !self.in_place {
            let (entry, warning) =
                saved(self.library.rename_setlist(&self.file, &self.setlist.name))?;
            self.file = entry.file;
            self.in_place = true;
            moved = warning;
        }
        let (entry, warning) = saved(self.library.save_setlist(setlist))?;
        self.file = entry.file;
        Ok(warning.or(moved))
    }

    /// Checks the setlist against the songs the library holds now.
    fn check(&mut self) {
        self.problems = match self.library.setlist_songs(&self.setlist) {
            Ok(songs) => self.setlist.validate(&songs).err().unwrap_or_default(),
            // Without the library folder every song is missing.
            Err(_) => self
                .setlist
                .validate(&std::collections::BTreeMap::new())
                .err()
                .unwrap_or_default(),
        };
    }
}

/// Makes `edit` to `setlist` with the `domain` crate's edits.
fn apply(setlist: &mut Setlist, edit: Edit) -> Result<(), EditError> {
    let end = setlist.items.len();
    match edit {
        Edit::AddSong(song) => setlist.insert_item(end, SetlistItem::Song(SongItem::new(song)))?,
        Edit::AddClick => setlist.insert_item(end, SetlistItem::Click(new_click()))?,
        Edit::Remove(index) => {
            setlist.remove_item(index)?;
        }
        Edit::Move { from, to } => setlist.move_item(from, to)?,
        Edit::Arrangement(index, arrangement) => setlist.set_arrangement(index, arrangement)?,
        Edit::StartEnd(index, start, end) => setlist.set_start_end(index, start, end)?,
        Edit::FadeOut(index, fade_out) => setlist.set_fade_out(index, fade_out)?,
        Edit::Transition(index, transition) => setlist.set_transition(index, transition)?,
        Edit::Click(index, click) => match setlist.items.get_mut(index) {
            Some(SetlistItem::Click(tile)) => *tile = click,
            Some(SetlistItem::Song(_)) => return Err(EditError::NotAClick(index)),
            None => {
                return Err(SetlistEditError::IndexOutOfRange { index, len: end }.into());
            }
        },
    }
    Ok(())
}
