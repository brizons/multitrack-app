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
//! The library has no JSON dependency of its own, so this module reads and
//! writes that one shape itself: the reader accepts any valid JSON object,
//! ignores keys it does not know (nested at most [`MAX_DEPTH`] deep), and
//! checks `formatVersion` before the shape of `setlists`, so a newer file
//! reports its version.

use std::path::Path;

use crate::setlists::{read_folder_file, write_atomically};
use crate::{SETLIST_ORDER_FILE, SETLIST_ORDER_FORMAT_VERSION, SetlistEntry, SetlistError};

/// How deep values the reader skips may nest.
const MAX_DEPTH: usize = 128;

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
    write_atomically(&folder.join(SETLIST_ORDER_FILE), &to_json(files))
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

/// Why an order file cannot be used.
#[derive(Debug, PartialEq, Eq)]
enum Problem {
    /// Another `formatVersion` than this version reads.
    Version(u32),
    /// Not valid JSON, or not the v1 shape.
    Format(String),
}

impl From<String> for Problem {
    fn from(reason: String) -> Self {
        Self::Format(reason)
    }
}

/// The setlist file names an order file holds.
fn parse(text: &str) -> Result<Vec<String>, Problem> {
    let mut json = Json {
        text: text.as_bytes(),
        at: 0,
    };
    let mut version = None;
    let mut setlists_at = None;
    json.whitespace();
    json.expect(b'{')?;
    json.whitespace();
    if json.peek() == Some(b'}') {
        json.at += 1;
    } else {
        loop {
            json.whitespace();
            let key = json.string()?;
            json.whitespace();
            json.expect(b':')?;
            json.whitespace();
            match key.as_str() {
                "formatVersion" => version = Some(json.number()?.to_owned()),
                "setlists" => {
                    setlists_at = Some(json.at);
                    json.skip_value(0)?;
                }
                _ => json.skip_value(0)?,
            }
            json.whitespace();
            match json.next() {
                Some(b',') => {}
                Some(b'}') => break,
                _ => return Err(json.error("expected , or }").into()),
            }
        }
    }
    json.whitespace();
    if json.at < json.text.len() {
        return Err(json.error("unexpected text after the order").into());
    }
    let version = version.ok_or_else(|| "formatVersion is missing".to_owned())?;
    let version: u32 = version
        .parse()
        .map_err(|_| format!("formatVersion {version} is not a whole number"))?;
    if version != SETLIST_ORDER_FORMAT_VERSION {
        return Err(Problem::Version(version));
    }
    json.at = setlists_at.ok_or_else(|| "setlists is missing".to_owned())?;
    let not_names = || "setlists must be a list of setlist file names".to_owned();
    if json.next() != Some(b'[') {
        return Err(not_names().into());
    }
    let mut files = Vec::new();
    json.whitespace();
    if json.peek() == Some(b']') {
        return Ok(files);
    }
    loop {
        json.whitespace();
        if json.peek() != Some(b'"') {
            return Err(not_names().into());
        }
        files.push(json.string()?);
        json.whitespace();
        match json.next() {
            Some(b',') => {}
            Some(b']') => return Ok(files),
            _ => return Err(not_names().into()),
        }
    }
}

/// A reader over JSON text.
struct Json<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Json<'a> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn error(&self, what: &str) -> String {
        format!("{what} at byte {}", self.at)
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected {}", byte as char)))
        }
    }

    /// A string, unescaped.
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut bytes = Vec::new();
        loop {
            match self.next() {
                None => return Err(self.error("unterminated string")),
                Some(b'"') => break,
                Some(b'\\') => {
                    let unescaped = match self.next() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{8}',
                        Some(b'f') => '\u{c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => self.unicode_escape()?,
                        _ => return Err(self.error("invalid escape")),
                    };
                    let mut buffer = [0; 4];
                    bytes.extend_from_slice(unescaped.encode_utf8(&mut buffer).as_bytes());
                }
                Some(byte) if byte < 0x20 => {
                    return Err(self.error("control character in a string"));
                }
                Some(byte) => bytes.push(byte),
            }
        }
        // Raw bytes come from valid UTF-8 text, escapes from chars.
        String::from_utf8(bytes).map_err(|_| self.error("invalid UTF-8"))
    }

    /// The character of a `\u` escape, after the `u`; a surrogate pair takes
    /// two escapes.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let first = self.hex4()?;
        let code = match first {
            0xD800..=0xDBFF => {
                if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                    return Err(self.error("unpaired surrogate"));
                }
                let second = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(self.error("unpaired surrogate"));
                }
                0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
            }
            0xDC00..=0xDFFF => return Err(self.error("unpaired surrogate")),
            code => code,
        };
        char::from_u32(code).ok_or_else(|| self.error("invalid escape"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u32::from_str_radix(digits, 16).ok())
            .ok_or_else(|| self.error("invalid \\u escape"))?;
        self.at += 4;
        Ok(digits)
    }

    /// A number, as written.
    fn number(&mut self) -> Result<&'a str, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => self.digits(),
            _ => return Err(self.error("expected a number")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            self.required_digits()?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            self.required_digits()?;
        }
        // Only ASCII was consumed.
        Ok(std::str::from_utf8(&self.text[start..self.at]).unwrap_or_default())
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
    }

    fn required_digits(&mut self) -> Result<(), String> {
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(self.error("expected a digit"));
        }
        self.digits();
        Ok(())
    }

    fn literal(&mut self, word: &str) -> Result<(), String> {
        if self.text[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(())
        } else {
            Err(self.error("invalid value"))
        }
    }

    /// Reads past one value, checking that it is valid JSON.
    fn skip_value(&mut self, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(self.error(&format!("values nested deeper than {MAX_DEPTH}")));
        }
        match self.peek() {
            Some(b'{') => self.skip_list(b'}', depth, true),
            Some(b'[') => self.skip_list(b']', depth, false),
            Some(b'"') => self.string().map(drop),
            Some(b't') => self.literal("true"),
            Some(b'f') => self.literal("false"),
            Some(b'n') => self.literal("null"),
            Some(b'-' | b'0'..=b'9') => self.number().map(drop),
            _ => Err(self.error("expected a value")),
        }
    }

    /// Reads past an object (`keyed`) or an array, from its opening byte.
    fn skip_list(&mut self, close: u8, depth: usize, keyed: bool) -> Result<(), String> {
        self.at += 1;
        self.whitespace();
        if self.peek() == Some(close) {
            self.at += 1;
            return Ok(());
        }
        loop {
            self.whitespace();
            if keyed {
                self.string()?;
                self.whitespace();
                self.expect(b':')?;
                self.whitespace();
            }
            self.skip_value(depth + 1)?;
            self.whitespace();
            match self.next() {
                Some(b',') => {}
                Some(byte) if byte == close => return Ok(()),
                _ => return Err(self.error("expected , or a closing bracket")),
            }
        }
    }
}

/// The order file for `files`, pretty-printed like the setlist files.
fn to_json(files: &[String]) -> String {
    let mut json =
        format!("{{\n  \"formatVersion\": {SETLIST_ORDER_FORMAT_VERSION},\n  \"setlists\": [");
    for (index, file) in files.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    ");
        push_string(&mut json, file);
    }
    if !files.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n}\n");
    json
}

/// `text` as a JSON string.
fn push_string(json: &mut String, text: &str) {
    json.push('"');
    for character in text.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            control if u32::from(control) < 0x20 => {
                json.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => json.push(other),
        }
    }
    json.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_reads_back() {
        let files = vec![
            "Plain.setlist.json".to_owned(),
            "Café \u{1F3B5} \"quoted\" back\\slash \u{1}.setlist.json".to_owned(),
        ];
        assert_eq!(parse(&to_json(&files)), Ok(files));
        assert_eq!(parse(&to_json(&[])), Ok(Vec::new()));
    }

    #[test]
    fn escapes_and_surrogate_pairs_are_read() {
        assert_eq!(
            parse(r#"{"formatVersion":1,"setlists":["🎵é\n\/"]}"#),
            Ok(vec!["\u{1F3B5}é\n/".to_owned()])
        );
        for bad in [
            r#"{"formatVersion":1,"setlists":["\ud83c"]}"#,
            r#"{"formatVersion":1,"setlists":["\udfb5"]}"#,
            r#"{"formatVersion":1,"setlists":["\x"]}"#,
            "{\"formatVersion\":1,\"setlists\":[\"a\u{1}\"]}",
            r#"{"formatVersion":1.0,"setlists":[]}"#,
            r#"{"formatVersion":-1,"setlists":[]}"#,
            r#"{"formatVersion":1,"setlists":[],}"#,
            r#"{"formatVersion":1 "setlists":[]}"#,
            r#"{"formatVersion":1,"x":01,"setlists":[]}"#,
            r#"{"formatVersion":1,"x":tru,"setlists":[]}"#,
            "",
        ] {
            assert!(matches!(parse(bad), Err(Problem::Format(_))), "{bad}");
        }
    }
}
