//! The setlists of a library folder (F5.1): listed in their stored order
//! with their item counts, created, duplicated, moved and removed (to the
//! system trash) through the `library` crate, which writes each file
//! atomically, never overwrites another setlist and keeps the order of the
//! list. Renaming goes through the editor
//! ([`crate::editor::SetlistEditor::rename`]), which holds the setlist.

use domain::Setlist;
use library::{Library, SetlistEntry, SetlistError};

/// The name a new setlist gets, made unique with a number.
pub const NEW_SETLIST_NAME: &str = "New setlist";

/// One setlist of the shelf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfEntry {
    /// Its name and file.
    pub entry: SetlistEntry,
    /// How many items it holds.
    pub items: usize,
}

/// The setlists of a library folder, as [`Library::list_setlists`] lists
/// them: in the stored order, the rest by name.
#[derive(Debug)]
pub struct Shelf {
    library: Library,
    /// The setlists that could be read.
    pub setlists: Vec<ShelfEntry>,
    /// Setlist files that cannot be read, one line each with the reason.
    pub unreadable: Vec<String>,
    /// Why the stored order cannot be used, so the setlists are listed by
    /// name.
    pub order_problem: Option<String>,
}

impl Shelf {
    /// Lists the setlists of `library`.
    pub fn open(library: Library) -> Result<Self, SetlistError> {
        let mut shelf = Self {
            library,
            setlists: Vec::new(),
            unreadable: Vec::new(),
            order_problem: None,
        };
        shelf.refresh()?;
        Ok(shelf)
    }

    /// The library folder.
    pub fn library(&self) -> &Library {
        &self.library
    }

    /// Lists the setlists again.
    pub fn refresh(&mut self) -> Result<(), SetlistError> {
        let listing = self.library.list_setlists()?;
        self.setlists = listing
            .setlists
            .into_iter()
            .map(|entry| {
                let items = self
                    .library
                    .load_setlist(&entry.file)
                    .map_or(0, |setlist| setlist.items.len());
                ShelfEntry { entry, items }
            })
            .collect();
        self.unreadable = listing
            .unreadable
            .iter()
            .map(|unreadable| format!("{}: {}", unreadable.file, unreadable.error))
            .collect();
        self.order_problem = listing
            .order_error
            .map(|error| format!("{error}; the setlists are listed by name"));
        Ok(())
    }

    /// Moves the setlist at `index` of the list to place `to`, stores the
    /// order, and lists the setlists again.
    pub fn move_setlist(&mut self, index: usize, to: usize) -> Result<(), SetlistError> {
        let Some(file) = self.setlists.get(index).map(|s| s.entry.file.clone()) else {
            return Ok(());
        };
        self.library.move_setlist(&file, to)?;
        self.refresh()
    }

    /// The index of the setlist in `file`.
    pub fn position(&self, file: &str) -> Option<usize> {
        self.setlists.iter().position(|s| s.entry.file == file)
    }

    /// Creates an empty setlist named [`NEW_SETLIST_NAME`], numbered when
    /// the name is taken, and lists it.
    ///
    /// A warning comes with the entry when the setlist was saved but its
    /// place in the stored order was not ([`saved`]).
    pub fn create(&mut self) -> Result<(SetlistEntry, Option<String>), SetlistError> {
        let name = unique_name(NEW_SETLIST_NAME, &self.names());
        let created = saved(self.library.save_setlist(&Setlist::new(name)))?;
        self.refresh()?;
        Ok(created)
    }

    /// Copies the setlist in `file` to a new setlist named "NAME copy"
    /// (numbered when taken) and lists it.
    /// A warning comes with the entry as for [`Self::create`].
    pub fn duplicate(
        &mut self,
        file: &str,
    ) -> Result<(SetlistEntry, Option<String>), SetlistError> {
        let original = self.library.load_setlist(file)?;
        let name = unique_name(&format!("{} copy", original.name), &self.names());
        let copied = saved(self.library.duplicate_setlist(file, &name))?;
        self.refresh()?;
        Ok(copied)
    }

    /// Removes the setlist in `file` from the list by moving its file to
    /// the trash ([`Library::remove_setlist`]), and lists the setlists
    /// again. A warning comes with the entry when the file went to the
    /// trash but the stored order kept its name.
    pub fn remove(&mut self, file: &str) -> Result<(SetlistEntry, Option<String>), SetlistError> {
        let removed = match self.library.remove_setlist(file) {
            Ok(entry) => (entry, None),
            Err(SetlistError::RemovedOrderNotUpdated { removed, reason }) => {
                let warning = SetlistError::RemovedOrderNotUpdated {
                    removed: removed.clone(),
                    reason,
                }
                .to_string();
                (removed, Some(warning))
            }
            Err(error) => return Err(error),
        };
        self.refresh()?;
        Ok(removed)
    }

    fn names(&self) -> Vec<&str> {
        self.setlists
            .iter()
            .map(|s| s.entry.name.as_str())
            .collect()
    }
}

/// The outcome of a library call that writes a setlist: its entry, with a
/// warning when the setlist was written but its place in the stored order
/// was not ([`SetlistError::OrderNotUpdated`]), which is not a failure.
pub fn saved(
    result: Result<SetlistEntry, SetlistError>,
) -> Result<(SetlistEntry, Option<String>), SetlistError> {
    match result {
        Ok(entry) => Ok((entry, None)),
        Err(SetlistError::OrderNotUpdated { saved, reason }) => {
            let warning = SetlistError::OrderNotUpdated {
                saved: saved.clone(),
                reason,
            }
            .to_string();
            Ok((saved, Some(warning)))
        }
        Err(error) => Err(error),
    }
}

/// `base`, or `base 2`, `base 3`… the first that is not in `taken`,
/// ignoring case.
pub fn unique_name(base: &str, taken: &[&str]) -> String {
    let free = |name: &str| {
        !taken
            .iter()
            .any(|t| t.to_lowercase() == name.to_lowercase())
    };
    if free(base) {
        return base.to_owned();
    }
    (2..)
        .map(|number| format!("{base} {number}"))
        .find(|name| free(name))
        .expect("some number is free")
}
