//! A one-line text field without a window: what the keys typed into a
//! setlist's name or the song search do. Text goes in at the end and
//! Backspace takes the last character; there is no caret to move, and no
//! input-method composition (gpui 0.2.2 has no text field of its own).

/// A key that reaches a field while it has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKey<'a> {
    /// Text the key types.
    Text(&'a str),
    /// Deletes the last character.
    Backspace,
    /// Commits the text.
    Enter,
    /// Leaves the field without the change.
    Escape,
}

impl<'a> LineKey<'a> {
    /// The key gpui reports as `key` (`"backspace"`, `"enter"`, `"a"`) with
    /// the text it types (`key_char`), or `None` for a key that does
    /// nothing in a field, such as an arrow.
    pub fn from_key(key: &str, key_char: Option<&'a str>) -> Option<Self> {
        match key {
            "backspace" => Some(Self::Backspace),
            "enter" => Some(Self::Enter),
            "escape" => Some(Self::Escape),
            _ => key_char
                .filter(|text| !text.chars().any(char::is_control))
                .map(Self::Text),
        }
    }
}

/// What a key did to the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineOutcome {
    /// The field goes on.
    Editing,
    /// Enter: the text to keep.
    Commit(String),
    /// Escape: the field closes without a change.
    Cancel,
}

/// A one-line field's text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
}

/// Most characters a field holds.
pub const MAX_CHARS: usize = 200;

impl LineEdit {
    /// A field holding `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The text so far.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Applies `key`.
    pub fn key(&mut self, key: LineKey<'_>) -> LineOutcome {
        match key {
            LineKey::Text(text) => self.paste(text),
            LineKey::Backspace => {
                self.text.pop();
            }
            LineKey::Enter => return LineOutcome::Commit(self.text.clone()),
            LineKey::Escape => return LineOutcome::Cancel,
        }
        LineOutcome::Editing
    }

    /// Adds pasted `text`: its first line, without control characters.
    pub fn paste(&mut self, text: &str) {
        let line = text.lines().next().unwrap_or_default();
        let room = MAX_CHARS.saturating_sub(self.text.chars().count());
        self.text
            .extend(line.chars().filter(|c| !c.is_control()).take(room));
    }
}
