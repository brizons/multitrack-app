//! The order of the setlist list (F5.1, S2): the [`SETLIST_ORDER_FILE`] in
//! the [`crate::SETLISTS_FOLDER`], a small JSON file naming setlist files in
//! order:
//!
//! ```json
//! {
//!   "formatVersion": 1,
//!   "setlists": [
//!     "Sunday Morning.setlist.json",
//!     "Youth Night.setlist.json"
//!   ]
//! }
//! ```
//!
//! The file is read as any JSON value first, so `formatVersion` is checked
//! before the shape of `setlists` and a newer file reports its version; keys
//! the order does not know are ignored.

use std::io;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::setlists::{read_folder_file, write_atomically};
use crate::{SETLIST_ORDER_FILE, SETLIST_ORDER_FORMAT_VERSION, SetlistEntry, SetlistError};

/// The stored order: `None` when there is no order file.
pub(crate) fn read_order(folder: &Path) -> Result<Option<Vec<String>>, SetlistError> {
    let path = folder.join(SETLIST_ORDER_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(SetlistError::Read { path, source }),
    }
    let text = read_folder_file(folder, SETLIST_ORDER_FILE)?;
    match parse(&text) {
        Ok(files) => Ok(Some(files)),
        Err(Problem::Version(found)) => Err(SetlistError::OrderVersion { path, found }),
        Err(Problem::Format(reason)) => Err(SetlistError::OrderFormat { path, reason }),
    }
}

/// Writes `files` as the stored order, atomically.
pub(crate) fn write_order(folder: &Path, files: &[String]) -> Result<(), SetlistError> {
    let path = folder.join(SETLIST_ORDER_FILE);
    let file = OrderFile {
        format_version: SETLIST_ORDER_FORMAT_VERSION,
        setlists: files,
    };
    let mut json = serde_json::to_string_pretty(&file).map_err(|error| SetlistError::Write {
        path: path.clone(),
        source: io::Error::other(error),
    })?;
    json.push('\n');
    write_atomically(&path, &json)
}

/// Changes the stored order with `change`, when there is one that can be
/// used; nothing is written when there is none, when it cannot be used
/// (the listing reports it), or when `change` leaves it as it was.
pub(crate) fn update_order(
    folder: &Path,
    change: impl FnOnce(&mut Vec<String>),
) -> Result<(), SetlistError> {
    let Ok(Some(mut order)) = read_order(folder) else {
        return Ok(());
    };
    let before = order.clone();
    change(&mut order);
    if order == before {
        return Ok(());
    }
    write_order(folder, &order)
}

/// `setlists` (in name order) with the ones `order` names first, in its
/// order; a name listed twice counts once, and names of files that are not
/// in `setlists` are ignored.
pub(crate) fn apply_order(setlists: Vec<SetlistEntry>, order: &[String]) -> Vec<SetlistEntry> {
    let mut rest = setlists;
    let mut ordered = Vec::with_capacity(rest.len());
    for file in order {
        if let Some(index) = rest.iter().position(|entry| &entry.file == file) {
            ordered.push(rest.remove(index));
        }
    }
    ordered.extend(rest);
    ordered
}

/// The order file as written.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderFile<'a> {
    format_version: u32,
    setlists: &'a [String],
}

/// Why an order file cannot be used.
#[derive(Debug, PartialEq, Eq)]
enum Problem {
    /// Another `formatVersion` than this version reads.
    Version(u32),
    /// Not valid JSON, or not the v1 shape.
    Format(String),
}

/// The setlist file names an order file holds.
fn parse(text: &str) -> Result<Vec<String>, Problem> {
    let format = |reason: &str| Problem::Format(reason.to_owned());
    let value: Value =
        serde_json::from_str(text).map_err(|error| Problem::Format(error.to_string()))?;
    let Value::Object(fields) = value else {
        return Err(format("the order must be a JSON object"));
    };
    let version = fields
        .get("formatVersion")
        .ok_or_else(|| format("formatVersion is missing"))?;
    let version = version
        .as_u64()
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| Problem::Format(format!("formatVersion {version} is not a whole number")))?;
    // Versions start at 1, so 0 is malformed rather than newer.
    if version == 0 {
        return Err(format(
            "formatVersion 0 is not a version; versions start at 1",
        ));
    }
    if version != SETLIST_ORDER_FORMAT_VERSION {
        return Err(Problem::Version(version));
    }
    let not_names = || format("setlists must be a list of setlist file names");
    let Some(Value::Array(setlists)) = fields.get("setlists") else {
        return Err(not_names());
    };
    setlists
        .iter()
        .map(|file| file.as_str().map(str::to_owned).ok_or_else(not_names))
        .collect()
}
