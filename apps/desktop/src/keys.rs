//! The keyboard: the same single keys as `play-cli`, so the screen can be
//! driven without a mouse.

use crate::action::UiAction;

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
        PANIC_KEY => Some(KeyAction::PanicDown),
        digit => match digit.as_bytes() {
            &[byte @ b'1'..=b'9'] => ui(UiAction::QueueSection(usize::from(byte - b'1'))),
            _ => None,
        },
    }
}

/// Whether `key` is the panic key, whose release ends a hold.
pub fn is_panic_key(key: &str) -> bool {
    key.eq_ignore_ascii_case(PANIC_KEY)
}

/// The keys that are down, so a held key acts once: its repeats are
/// ignored until it is released. gpui 0.2.2 marks a repeat as held on
/// macOS and Windows but reports every repeat on X11 as a fresh press, so
/// the screen keeps track itself, on every platform.
#[derive(Debug, Clone, Default)]
pub struct HeldKeys {
    /// Lowercase names of the plain keys that acted and are still down.
    down: Vec<String>,
}

impl HeldKeys {
    /// No key down.
    pub fn new() -> Self {
        Self::default()
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

    /// The modifiers held changed to `mods`. When Command (the platform
    /// key) is down, every key counts as up: macOS never delivers the
    /// release of a key let go while Command is held, so a key that went
    /// down before Command would otherwise swallow its next press. Other
    /// platforms have no such loss; there it only means that a repeat of
    /// a key held through a Super or Windows key press acts once more.
    pub fn modifiers_changed(&mut self, mods: Mods) {
        if mods.platform {
            self.forget_all();
        }
    }

    /// Forgets every key: the window lost focus, so their releases may
    /// never arrive.
    pub fn forget_all(&mut self) {
        self.down.clear();
    }
}
