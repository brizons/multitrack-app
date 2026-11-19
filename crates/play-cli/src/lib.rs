//! The parts of `play-cli` that do not touch the terminal or a device: which
//! key does what, the one-line status, and the device list (S5).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::fmt::Write as _;

use app::{
    Command, DeviceInfo, DeviceStatus, FadeState, JumpKind, JumpTiming, Snapshot, SongInfo,
    TransportState,
};

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
  q      quit (Ctrl-C too)";

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
