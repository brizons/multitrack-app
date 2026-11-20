//! The parts of `play-cli` that do not touch the terminal or a device: which
//! key does what, the one-line status, and the device list (S5), and the
//! same for its setlist mode (S4): the item keys, the status with the
//! setlist's state, the problems printed before a setlist plays, and where
//! a setlist file's library folder is.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt::Write as _;
use std::path::{self, Component, Path, PathBuf};

use app::{
    Command, DeviceInfo, DeviceStatus, FadeState, JumpKind, JumpTiming, SetlistCommand,
    SetlistCommandError, SetlistInfo, SetlistSnapshot, Snapshot, SongInfo, TransportState,
};
use domain::{SetlistItem, Transition};
use library::SETLISTS_FOLDER;

/// The controls, as `--help` lists them.
pub const CONTROLS: &str = "\
Controls (single keys, no Enter):
  space  play / pause
  s      stop and return to the start
  b      back to the start
  1-9    queue a jump to arrangement position 1-9
  n      jump timing: at the section end or at the next bar
  l      live loop: repeat the current section once more
  i      infinite loop on / off
  f      fade the track stems out / back in
  c      cancel the queued jump or loop
  x      panic: fade everything out in 0.5 s and stop
  q      quit (Ctrl-C too)

Setlist controls (with --setlist; the keys above act on the current item):
  .      next item: the item playing stops with its ramp, the next waits
  ,      previous item
  g      go to item: type its number, then Enter (Esc cancels)";

/// What a key asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Send this command to the session.
    Send(Command),
    /// Stop and leave.
    Quit,
}

/// The action for `key` given the session's state, or `None` for a key with
/// no control. Letters work in either case.
pub fn action_for_key(key: char, snapshot: &Snapshot) -> Option<Action> {
    let send = |command| Some(Action::Send(command));
    match key.to_ascii_lowercase() {
        ' ' => match snapshot.transport {
            TransportState::Playing | TransportState::Starting => send(Command::Pause),
            _ => send(Command::Play),
        },
        's' => send(Command::Stop),
        'b' => send(Command::BackToStart),
        digit @ '1'..='9' => send(Command::QueueJump {
            position: digit as usize - '1' as usize,
        }),
        'n' => send(Command::SetJumpTiming(match snapshot.jump_timing {
            JumpTiming::SectionEnd => JumpTiming::NextBar,
            JumpTiming::NextBar => JumpTiming::SectionEnd,
        })),
        'l' => send(Command::LiveLoop),
        'i' => send(Command::SetInfinite(!snapshot.infinite)),
        'f' => send(Command::ToggleFade),
        'c' => send(Command::CancelJump),
        'x' => send(Command::Panic),
        'q' => Some(Action::Quit),
        _ => None,
    }
}

/// The status line, fields separated by ` | `: transport, bar.beat, section
/// and its position, the queued jump, jump timing, infinite loop, fade,
/// device, then underruns and the last error when there are any.
pub fn status_line(snapshot: &Snapshot, song: &SongInfo) -> String {
    let transport = match snapshot.transport {
        TransportState::Stopped => "STOPPED",
        TransportState::Starting => "LOADING",
        TransportState::Playing => "PLAYING",
        TransportState::Pausing | TransportState::Paused => "PAUSED",
        TransportState::Stopping => "STOPPING",
        TransportState::Panicking => "PANIC",
    };
    let mut fields = vec![
        format!("{transport:<8}"),
        format!("bar {}.{}", snapshot.bar, snapshot.beat),
        format!(
            "{} ({}/{})",
            snapshot.section,
            snapshot.position + 1,
            song.positions.len()
        ),
    ];
    fields.push(match &snapshot.queued {
        None => "next: -".to_owned(),
        Some(queued) => {
            let what = match queued.kind {
                JumpKind::Jump => "jump to",
                JumpKind::Loop => "loop",
            };
            let seconds = queued.frames_until as f64 / f64::from(song.sample_rate);
            let mut text = format!("next: {what} {} in {seconds:.1} s", queued.target_label);
            if queued.deferrals > 0 {
                let _ = write!(text, ", waited {}", queued.deferrals);
            }
            text
        }
    });
    fields.push(
        match snapshot.jump_timing {
            JumpTiming::SectionEnd => "timing: section end",
            JumpTiming::NextBar => "timing: next bar",
        }
        .to_owned(),
    );
    if snapshot.infinite {
        fields.push("infinite".to_owned());
    }
    let percent = (snapshot.fade.level * 100.0).round();
    fields.push(match snapshot.fade.state {
        FadeState::In => "fade: in".to_owned(),
        FadeState::Out => "fade: out".to_owned(),
        FadeState::FadingOut => format!("fade: out {percent}%"),
        FadeState::FadingIn => format!("fade: in {percent}%"),
    });
    fields.push(match &snapshot.device {
        DeviceStatus::Connected(info) => format!(
            "{} Hz, {} frames, {:.1} ms on \"{}\"",
            info.sample_rate,
            info.buffer_frames,
            info.latency().as_secs_f64() * 1_000.0,
            info.device
        ),
        DeviceStatus::Disconnected {
            device,
            reopen_error: Some(error),
            ..
        } => format!("NO DEVICE: \"{device}\" (cannot open it again: {error})"),
        DeviceStatus::Disconnected { device, reason, .. } => {
            format!("NO DEVICE: \"{device}\" ({reason})")
        }
        DeviceStatus::PlayerLost { device } => {
            format!("NO DEVICE: \"{device}\" (the output kept the player; open the song again)")
        }
    });
    if snapshot.underruns > 0 {
        fields.push(format!("underruns: {}", snapshot.underruns));
    }
    if let Some(error) = &snapshot.last_error {
        fields.push(format!("! {error}"));
    }
    fields.join(" | ")
}

/// One line per device, the default marked with `*`, each with its output
/// channel count (F8.1) and its id, which `--device` takes when two devices
/// share a name.
pub fn device_list(devices: &[DeviceInfo]) -> String {
    if devices.is_empty() {
        return "no audio output device found\n".to_owned();
    }
    let mut list = String::new();
    for device in devices {
        let mark = if device.is_default { '*' } else { ' ' };
        let plural = if device.output_channels == 1 { "" } else { "s" };
        let _ = writeln!(
            list,
            "{mark} {} ({} output channel{plural}), id {}",
            device.name, device.output_channels, device.id
        );
    }
    list
}

/// The key a terminal's Enter gives.
pub const KEY_ENTER: char = '\r';
/// The key a terminal's Escape gives.
pub const KEY_ESCAPE: char = '\u{1b}';
/// The key a terminal's Backspace gives.
pub const KEY_BACKSPACE: char = '\u{8}';
/// DEL, which many terminals send for Backspace; it deletes a digit too.
const KEY_DELETE: char = '\u{7f}';
/// Line feed, which some terminals send for Enter; it selects too.
const KEY_LINE_FEED: char = '\n';

/// Most digits an item number takes.
const ITEM_DIGITS: usize = 4;

/// What a key asks for in setlist mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SetlistAction {
    /// Send this command to the setlist session.
    Send(SetlistCommand),
    /// Stop and leave.
    Quit,
}

/// The keys of setlist mode: `.` and `,` for the next and previous item,
/// `g` to type an item's number (Enter selects it, Escape cancels,
/// Backspace deletes a digit), and every key of [`action_for_key`] for the
/// current item. Digits typed after `g` name an item; otherwise they queue
/// sections, as in single-song mode.
#[derive(Debug, Default)]
pub struct SetlistKeys {
    /// The digits typed after `g`, while an item number is being typed.
    entry: Option<String>,
}

impl SetlistKeys {
    /// The action for `key` given the session's state, or `None` for a key
    /// with no control or one that only types. Letters work in either case.
    pub fn key(&mut self, key: char, snapshot: &SetlistSnapshot) -> Option<SetlistAction> {
        if let Some(entry) = &mut self.entry {
            match key {
                '0'..='9' => {
                    if entry.len() < ITEM_DIGITS {
                        entry.push(key);
                    }
                    return None;
                }
                KEY_BACKSPACE | KEY_DELETE => {
                    entry.pop();
                    return None;
                }
                KEY_ENTER | KEY_LINE_FEED => {
                    let number = entry.parse::<usize>().ok().filter(|&number| number > 0);
                    self.entry = None;
                    return number
                        .map(|number| SetlistAction::Send(SetlistCommand::Select(number - 1)));
                }
                KEY_ESCAPE => {
                    self.entry = None;
                    return None;
                }
                // Any other key ends the number and does its own job.
                _ => self.entry = None,
            }
        }
        match key.to_ascii_lowercase() {
            '.' => Some(SetlistAction::Send(SetlistCommand::Next)),
            ',' => Some(SetlistAction::Send(SetlistCommand::Previous)),
            'g' => {
                self.entry = Some(String::new());
                None
            }
            _ => match action_for_key(key, &snapshot.song)? {
                Action::Send(command) => Some(SetlistAction::Send(SetlistCommand::Item(command))),
                Action::Quit => Some(SetlistAction::Quit),
            },
        }
    }

    /// The digits typed after `g`, while an item number is being typed.
    pub fn entry(&self) -> Option<&str> {
        self.entry.as_deref()
    }
}

/// A setlist item's name: a song's folder name without `.song`, or a
/// click-only tile's tempo, meter and bar count.
pub fn item_label(item: &SetlistItem) -> String {
    match item {
        SetlistItem::Song(song) => {
            let name = song.song.rsplit(['/', '\\']).next().unwrap_or(&song.song);
            name.strip_suffix(".song").unwrap_or(name).to_owned()
        }
        SetlistItem::Click(click) => {
            let meter = click.time_signature;
            let mut label = format!(
                "Click {} BPM {}/{}",
                click.bpm, meter.numerator, meter.denominator
            );
            if let Some(bars) = click.bars {
                let plural = if bars == 1 { "" } else { "s" };
                let _ = write!(label, ", {bars} bar{plural}");
            }
            label
        }
    }
}

/// What to say about the setlist before it plays, one line per problem:
/// each reason an item cannot play, and each song at another sample rate
/// than the device opens at. The rest of the setlist plays.
pub fn setlist_problems(info: &SetlistInfo) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, item) in info.items.iter().enumerate() {
        let name = item_name(index, Some(&item.item));
        for problem in &item.problems {
            lines.push(item_cannot_play(index, Some(&item.item), problem));
        }
        if let Some(rate) = item.sample_rate
            && item.is_playable()
            && rate != info.sample_rate
        {
            lines.push(format!(
                "{name} runs at {rate} Hz and the device at {} Hz: it plays once the device reopens at its rate, and no AutoLink or Crossfade leads into it",
                info.sample_rate
            ));
        }
    }
    lines
}

/// An item as messages name it: its number from 1, and its label when the
/// item is known.
pub fn item_name(index: usize, item: Option<&SetlistItem>) -> String {
    match item {
        Some(item) => format!("item {} (\"{}\")", index + 1, item_label(item)),
        None => format!("item {}", index + 1),
    }
}

/// One reason item `index` cannot play, as play-cli prints it: as a
/// warning before a setlist plays, or as an error when nothing can.
pub fn item_cannot_play(index: usize, item: Option<&SetlistItem>, reason: &str) -> String {
    format!("{} cannot play: {reason}", item_name(index, item))
}

/// The setlist mode's status line: the item number being typed after `g`,
/// the current item and its place in the setlist, a crossfade under way,
/// what follows the item, then the current item's [`status_line`], then
/// the setlist's problem and the last refused setlist command, each
/// prefixed `!`.
pub fn setlist_status_line(
    snapshot: &SetlistSnapshot,
    info: &SetlistInfo,
    song: &SongInfo,
    keys: &SetlistKeys,
    refused: Option<&SetlistCommandError>,
) -> String {
    let state = &snapshot.setlist;
    let label = |index: usize| {
        info.items
            .get(index)
            .map_or_else(String::new, |item| item_label(&item.item))
    };
    let mut fields = Vec::new();
    if let Some(entry) = keys.entry() {
        fields.push(format!("go to item {entry}_ (Enter, Esc)"));
    }
    fields.push(format!(
        "item {}/{} \"{}\"",
        state.current + 1,
        info.items.len(),
        label(state.current)
    ));
    if let Some(blend) = &state.blend {
        let percent = (blend.progress() * 100.0).round();
        fields.push(format!(
            "crossfade from {} \"{}\" {percent}%",
            blend.from + 1,
            label(blend.from)
        ));
    }
    let follow = |name: &str, next: usize| {
        let loading = if state.next_ready { "" } else { " (loading)" };
        format!("then: {name} -> {} \"{}\"{loading}", next + 1, label(next))
    };
    fields.push(match (state.transition, state.next) {
        (Transition::Stay, _) => "then: Stay".to_owned(),
        (_, None) => "then: end of setlist".to_owned(),
        (Transition::CueNext, Some(next)) => follow("Cue next", next),
        (Transition::AutoLink, Some(next)) => follow("AutoLink", next),
        (Transition::Crossfade, Some(next)) => follow("Crossfade", next),
    });
    fields.push(status_line(&snapshot.song, song));
    if let Some(problem) = &state.problem {
        fields.push(format!("! {problem}"));
    }
    if let Some(error) = refused {
        fields.push(format!("! {error}"));
    }
    fields.join(" | ")
}

/// The library folder and the file name a setlist file `path` stands for:
/// a setlist lives in the [`SETLISTS_FOLDER`] of its library folder, so the
/// library is the folder holding that one (any case of its name). Relative
/// paths count from the current folder, and `.` and `..` are resolved by
/// name, without following symlinks; nothing on disk is read.
pub fn setlist_location(path: &Path) -> Result<(PathBuf, String), String> {
    let not_there = || {
        format!(
            "{} is not in a {SETLISTS_FOLDER} folder: a setlist file lives in the {SETLISTS_FOLDER} folder of a library folder, such as LIBRARY/{SETLISTS_FOLDER}/Sunday.setlist.json",
            path.display()
        )
    };
    // A path ending in `..` names a folder, never a setlist file.
    if path.ends_with(Component::ParentDir.as_os_str()) {
        return Err(not_there());
    }
    let absolute = path::absolute(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let absolute = resolve_dots(&absolute);
    let file = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(not_there)?;
    let folder = absolute.parent().ok_or_else(not_there)?;
    let in_setlists = folder
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(SETLISTS_FOLDER));
    let library = folder
        .parent()
        .filter(|_| in_setlists)
        .ok_or_else(not_there)?;
    Ok((library.to_owned(), file.to_owned()))
}

/// `path` (absolute) with `.` dropped and each `..` taking off the name
/// before it; `..` at the root stays at the root.
fn resolve_dots(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if resolved.parent().is_some() {
                    resolved.pop();
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
}
