//! The keyboard: the same single keys as `play-cli`, so the screen can be
//! driven without a mouse.

use crate::action::UiAction;

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
    /// The panic key went down; panic fires only if it is held for
    /// [`crate::hold::PANIC_HOLD`] (see [`is_panic_key`] for its release).
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
        "x" => Some(KeyAction::PanicDown),
        digit => match digit.as_bytes() {
            &[byte @ b'1'..=b'9'] => ui(UiAction::QueueSection(usize::from(byte - b'1'))),
            _ => None,
        },
    }
}

/// Whether `key` is the panic key, whose release ends a hold.
pub fn is_panic_key(key: &str) -> bool {
    key.eq_ignore_ascii_case("x")
}
