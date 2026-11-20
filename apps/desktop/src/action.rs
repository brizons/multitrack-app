//! The controls of the Perform screen, as the `app` commands they send.

use app::{Command, JumpKind, JumpTiming, Snapshot, SongInfo, TransportState};

/// A control on the Perform screen, or the key that stands for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiAction {
    /// The play/pause button: pauses while playing or loading, plays
    /// otherwise.
    TogglePlay,
    /// Stops and returns to the start.
    Stop,
    /// Moves to the start of the arrangement.
    BackToStart,
    /// A section pad: queues a jump to this arrangement position, or drops
    /// the jump when this position is the one already queued.
    QueueSection(usize),
    /// Drops the queued jump or loop.
    CancelJump,
    /// One side of the jump timing toggle.
    SetJumpTiming(JumpTiming),
    /// Switches the jump timing to the other side.
    ToggleJumpTiming,
    /// "Loop section": queues a live loop, or drops the one queued.
    ToggleLiveLoop,
    /// Infinite loop on or off.
    ToggleInfinite,
    /// Fades the track stems out, or back in.
    ToggleFade,
    /// Panic. The screen sends it only after a full hold
    /// ([`crate::hold::PanicHold`]), never on a click.
    Panic,
}

/// The command `action` sends, given the session's state; `None` when it
/// sends nothing, such as a pad past the end of the arrangement.
pub fn command_for(action: UiAction, snapshot: &Snapshot, song: &SongInfo) -> Option<Command> {
    Some(match action {
        UiAction::TogglePlay if is_playing(snapshot.transport) => Command::Pause,
        UiAction::TogglePlay => Command::Play,
        UiAction::Stop => Command::Stop,
        UiAction::BackToStart => Command::BackToStart,
        UiAction::QueueSection(position) if position >= song.positions.len() => return None,
        UiAction::QueueSection(position) => match &snapshot.queued {
            Some(queued) if queued.kind == JumpKind::Jump && queued.target == position => {
                Command::CancelJump
            }
            _ => Command::QueueJump { position },
        },
        UiAction::CancelJump => Command::CancelJump,
        UiAction::SetJumpTiming(timing) => Command::SetJumpTiming(timing),
        UiAction::ToggleJumpTiming => Command::SetJumpTiming(match snapshot.jump_timing {
            JumpTiming::SectionEnd => JumpTiming::NextBar,
            JumpTiming::NextBar => JumpTiming::SectionEnd,
        }),
        UiAction::ToggleLiveLoop if snapshot.live_loop() => Command::CancelJump,
        UiAction::ToggleLiveLoop => Command::LiveLoop,
        UiAction::ToggleInfinite => Command::SetInfinite(!snapshot.infinite),
        UiAction::ToggleFade => Command::ToggleFade,
        UiAction::Panic => Command::Panic,
    })
}

/// Whether `transport` counts as playing: playing, or waiting for the disk
/// to start (shown as "Loading…"). Play/pause then pauses and reads
/// "Pause", and quitting or closing the window asks first
/// ([`crate::ending`]).
pub fn is_playing(transport: TransportState) -> bool {
    matches!(
        transport,
        TransportState::Playing | TransportState::Starting
    )
}
