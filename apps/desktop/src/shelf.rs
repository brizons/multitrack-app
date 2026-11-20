//! The setlists of a library folder (F5.1): listed with their item counts,
//! created and duplicated through the `library` crate, which writes each
//! file atomically and never overwrites another setlist. Renaming goes
//! through the editor ([`crate::editor::SetlistEditor::rename`]), which
//! holds the setlist.

use std::path::Path;

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
/// them (by name, ignoring case).
#[derive(Debug)]
pub struct Shelf {
    library: Library,
    /// The setlists that could be read.
    pub setlists: Vec<ShelfEntry>,
    /// Setlist files that cannot be read, one line each with the reason.
    pub unreadable: Vec<String>,
}

impl Shelf {
    /// Lists the setlists of the library folder `root`.
    pub fn open(root: &Path) -> Result<Self, SetlistError> {
        let mut shelf = Self {
            library: Library::new(root),
            setlists: Vec::new(),
            unreadable: Vec::new(),
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
        Ok(())
    }

    /// The index of the setlist in `file`.
    pub fn position(&self, file: &str) -> Option<usize> {
        self.setlists.iter().position(|s| s.entry.file == file)
    }

    /// Creates an empty setlist named [`NEW_SETLIST_NAME`], numbered when
    /// the name is taken, and lists it.
    pub fn create(&mut self) -> Result<SetlistEntry, SetlistError> {
        let name = unique_name(NEW_SETLIST_NAME, &self.names());
        let entry = self.library.save_setlist(&Setlist::new(name))?;
        self.refresh()?;
        Ok(entry)
    }

    /// Copies the setlist in `file` to a new setlist named "NAME copy"
    /// (numbered when taken) and lists it.
    pub fn duplicate(&mut self, file: &str) -> Result<SetlistEntry, SetlistError> {
        let original = self.library.load_setlist(file)?;
        let name = unique_name(&format!("{} copy", original.name), &self.names());
        let entry = self.library.duplicate_setlist(file, &name)?;
        self.refresh()?;
        Ok(entry)
    }

    fn names(&self) -> Vec<&str> {
        self.setlists
            .iter()
            .map(|s| s.entry.name.as_str())
            .collect()
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
