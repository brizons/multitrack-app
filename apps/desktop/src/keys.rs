//! The keyboard: the same single keys as `play-cli`, so the screen can be
//! driven without a mouse.

use crate::action::UiAction;
use crate::line_edit::LineKey;

/// The panic key: held for [`crate::hold::PANIC_HOLD`], it fires panic; its
/// release ends the hold. Letters work in either case.
pub const PANIC_KEY: &str = "x";

/// Whether the platform's secondary modifier (the one in "Open…" and
/// "Quit") is Command (macOS) rather than Control.
pub const SECONDARY_IS_PLATFORM: bool = cfg!(target_os = "macos");

/// Modifier keys held with a key; Shift is left out, since letters work in
/// either case.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    /// Control.
    pub control: bool,
    /// Alt or Option.
    pub alt: bool,
    /// Command on macOS, the Windows or Super key elsewhere.
    pub platform: bool,
    /// Fn.
    pub function: bool,
}

/// A key that went down: its name as GPUI reports it (`"space"`,
/// `"escape"`, `"x"`, `"1"`) and the modifiers held.
#[derive(Debug, Clone, Copy)]
pub struct KeyPress<'a> {
    /// Key name.
    pub key: &'a str,
    /// Modifiers held.
    pub mods: Mods,
}

/// What a key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// The control this key stands for.
    Ui(UiAction),
    /// The panic key ([`PANIC_KEY`]) went down; panic fires only if it is
    /// held for [`crate::hold::PANIC_HOLD`] (see [`is_panic_key`] for its
    /// release).
    PanicDown,
    /// Open a song folder (secondary modifier + O).
    OpenSong,
    /// Quit (secondary modifier + Q).
    Quit,
}

/// What `press` asks for, or `None` for a key with no control. A plain
/// key only acts with no modifier other than Shift, so Ctrl-X never acts
/// as X.
pub fn key_down(press: &KeyPress<'_>) -> Option<KeyAction> {
    let Mods {
        control,
        alt,
        platform,
        function,
    } = press.mods;
    let key = press.key.to_ascii_lowercase();
    let (secondary, other) = if SECONDARY_IS_PLATFORM {
        (platform, control || alt || function)
    } else {
        (control, platform || alt || function)
    };
    if other {
        return None;
    }
    if secondary {
        return match key.as_str() {
            "o" => Some(KeyAction::OpenSong),
            "q" => Some(KeyAction::Quit),
            _ => None,
        };
    }
    let ui = |action| Some(KeyAction::Ui(action));
    match key.as_str() {
        "space" => ui(UiAction::TogglePlay),
        "s" => ui(UiAction::Stop),
        "b" => ui(UiAction::BackToStart),
        "n" => ui(UiAction::ToggleJumpTiming),
        "l" => ui(UiAction::ToggleLiveLoop),
        "i" => ui(UiAction::ToggleInfinite),
        "f" => ui(UiAction::ToggleFade),
        "c" | "escape" => ui(UiAction::CancelJump),
        "." => ui(UiAction::NextSong),
        "," => ui(UiAction::PreviousSong),
        PANIC_KEY => Some(KeyAction::PanicDown),
        digit => match digit.as_bytes() {
            &[byte @ b'1'..=b'9'] => ui(UiAction::QueueSection(usize::from(byte - b'1'))),
            _ => None,
        },
    }
}

/// What a key does while a text field has the focus ([`typed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typed<'a> {
    /// It goes to the field.
    Line(LineKey<'a>),
    /// The secondary modifier with V: paste into the field.
    Paste,
    /// A shortcut with the secondary modifier, such as Quit.
    Key(KeyAction),
    /// Nothing.
    Ignored,
}

/// What `press`, typing `key_char`, does while a text field has the focus:
/// every key types into the field (Space, `x` and digits too, so typing
/// never plays, panics or queues), except the secondary modifier's
/// shortcuts: paste, open and quit.
pub fn typed<'a>(press: &KeyPress<'_>, key_char: Option<&'a str>) -> Typed<'a> {
    let secondary = if SECONDARY_IS_PLATFORM {
        press.mods.platform
    } else {
        press.mods.control
    };
    if secondary {
        if press.key.eq_ignore_ascii_case("v") {
            return Typed::Paste;
        }
        return match key_down(press) {
            Some(action @ (KeyAction::Quit | KeyAction::OpenSong)) => Typed::Key(action),
            _ => Typed::Ignored,
        };
    }
    LineKey::from_key(&press.key.to_ascii_lowercase(), key_char).map_or(Typed::Ignored, Typed::Line)
}

/// Whether `key` is the panic key, whose release ends a hold.
pub fn is_panic_key(key: &str) -> bool {
    key.eq_ignore_ascii_case(PANIC_KEY)
}

/// How the platform reports key releases, which decides when
/// [`HeldKeys`] forgets the keys that are down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPlatform {
    /// macOS: a key let go while Command is held never reports its
    /// release.
    Mac,
    /// Windows and Linux: every release arrives, whatever is held.
    Other,
}

impl KeyPlatform {
    /// This platform.
    pub const HERE: Self = if cfg!(target_os = "macos") {
        Self::Mac
    } else {
        Self::Other
    };
}

/// The keys that are down, so a held key acts once: its repeats are
/// ignored until it is released. gpui 0.2.2 marks a repeat as held on
/// macOS and Windows but reports every repeat on X11 as a fresh press, so
/// the screen keeps track itself, on every platform.
#[derive(Debug, Clone)]
pub struct HeldKeys {
    /// Lowercase names of the plain keys that acted and are still down.
    down: Vec<String>,
    platform: KeyPlatform,
}

impl Default for HeldKeys {
    fn default() -> Self {
        Self::new()
    }
}

impl HeldKeys {
    /// No key down, on this platform ([`KeyPlatform::HERE`]).
    pub fn new() -> Self {
        Self::on(KeyPlatform::HERE)
    }

    /// No key down, on `platform`.
    pub fn on(platform: KeyPlatform) -> Self {
        Self {
            down: Vec::new(),
            platform,
        }
    }

    /// What `press` asks for when it goes down, as [`key_down`] maps it, or
    /// `None` for a repeat: `repeat` is the platform's own repeat flag, and
    /// a plain key that acted counts as down until [`Self::release`]. Two
    /// kinds of key rely on other rules instead, so that a release the
    /// window never saw cannot swallow their next press: the panic key
    /// (every press reaches [`crate::hold::PanicHold`], which ignores a
    /// press while held), and keys held with a modifier (macOS drops the
    /// release of a key let go while Command is down; see also
    /// [`Self::modifiers_changed`]).
    pub fn press(&mut self, press: &KeyPress<'_>, repeat: bool) -> Option<KeyAction> {
        if repeat {
            return None;
        }
        let action = key_down(press)?;
        let tracked = press.mods == Mods::default() && action != KeyAction::PanicDown;
        if tracked {
            let key = press.key.to_ascii_lowercase();
            if self.down.contains(&key) {
                return None;
            }
            self.down.push(key);
        }
        Some(action)
    }

    /// `key` went up.
    pub fn release(&mut self, key: &str) {
        self.down.retain(|down| !down.eq_ignore_ascii_case(key));
    }

    /// The modifiers held changed to `mods`. On the Mac, while Command is
    /// down, every key counts as up: macOS never delivers the release of a
    /// key let go while Command is held, so a key that went down before
    /// Command would otherwise swallow its next press. Elsewhere every
    /// release arrives, so the Super or Windows key changes nothing.
    pub fn modifiers_changed(&mut self, mods: Mods) {
        if self.platform == KeyPlatform::Mac && mods.platform {
            self.forget_all();
        }
    }

    /// Forgets every key: the window lost focus, so their releases may
    /// never arrive.
    pub fn forget_all(&mut self) {
        self.down.clear();
    }
}
