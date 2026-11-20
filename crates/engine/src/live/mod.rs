//! Real-time live player (S2).
//!
//! A [`Player`] walks one arrangement of a [`domain::Song`] buffer by buffer
//! from [`Player::process`], which the audio callback calls. It keeps one
//! output frame counter, the current arrangement position, and the offset
//! into it. With no live command it plays exactly what [`crate::render_offline`]
//! renders for the compiled plan, seam crossfades and generated click
//! included, whatever the buffer size.
//!
//! A [`Controller`] sends [`Command`]s through a lock-free single-producer,
//! single-consumer queue; the player applies every pending command at the
//! start of the next [`Player::process`] call. A queued jump executes on the
//! exact boundary frame, splitting the buffer there when it falls inside one.
//!
//! Real-time rules: `process` allocates nothing, takes no locks, and does no
//! I/O. Every buffer is sized at construction from the maximum buffer size,
//! and every gain change (fade, click on/off, pause, stop, panic) ramps per
//! frame.

mod player;
mod ramp;
mod source;
mod status;

use std::error::Error;
use std::fmt;

pub use player::Player;
pub use source::{MemorySource, StemSource, memory_sources};
pub use status::{FadeState, FadeStatus, JumpKind, QueuedJump, Status, Transport};

/// Frames over which pause, stop, resume, and click on/off ramp, so the
/// transport never cuts a waveform mid-cycle.
pub const DECLICK_FRAMES: u64 = 256;

/// Default fade duration, in seconds of output ([`Command::SetFadeDuration`]
/// changes it).
pub const DEFAULT_FADE_SECONDS: u32 = 3;

/// Commands the queue holds before [`Controller::send`] reports it full.
pub const COMMAND_CAPACITY: usize = 256;

/// When a queued jump executes (F6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JumpTiming {
    /// At the end of the current arrangement position.
    #[default]
    SectionEnd,
    /// At the next bar line of the current section's source grid, or at its
    /// end when no bar line is left in it.
    NextBar,
}

/// A live command for the [`Player`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Starts playing. From [`Transport::Stopped`] the song starts at full
    /// level, like the offline render; from a pause it ramps back in over
    /// [`DECLICK_FRAMES`]. Ignored during a panic.
    Play,
    /// Ramps out over [`DECLICK_FRAMES`] and holds the position.
    Pause,
    /// Ramps out over [`DECLICK_FRAMES`], then returns to the start: position
    /// 0, output frame 0, no queued jump, infinite loop off, fade back in.
    Stop,
    /// Moves to the start of the arrangement. While audible it is a seam to
    /// position 0 with the equal-power crossfade; while paused it moves there
    /// silently.
    BackToStart,
    /// Queues a jump to arrangement position `position`, replacing any queued
    /// jump. It executes at the boundary [`JumpTiming`] selects.
    QueueJump {
        /// Index into the arrangement's section list.
        position: usize,
    },
    /// Drops the queued jump or loop.
    CancelJump,
    /// Chooses when queued jumps execute; a jump already queued follows the
    /// new timing.
    SetJumpTiming(JumpTiming),
    /// Repeats the current position once more: a queued jump to it at the
    /// end of the current position, whatever the [`JumpTiming`].
    LiveLoop,
    /// Repeats the current position at each of its ends until switched off.
    /// A queued jump takes priority and switches it off when it executes.
    SetInfinite(bool),
    /// Fades the `track` stems out, or back in when they are faded or
    /// fading out. Click and guide stems and the generated click keep
    /// playing.
    ToggleFade,
    /// Sets how long a full fade takes, in frames; a fade from a partial
    /// level takes proportionally less.
    SetFadeDuration {
        /// Frames for a full fade.
        frames: u64,
    },
    /// Fades everything, click included, to silence within 0.5 seconds, then
    /// stops like [`Command::Stop`].
    Panic,
    /// Turns the generated click on or off, ramping over [`DECLICK_FRAMES`].
    /// Like [`crate::ClickSettings::enabled`], it does not affect a `click`
    /// stem.
    SetClick(bool),
}

/// Sends [`Command`]s to its [`Player`] from the control thread.
#[derive(Debug)]
pub struct Controller {
    producer: rtrb::Producer<Command>,
    positions: usize,
}

impl Controller {
    /// Queues `command` for the player's next buffer.
    pub fn send(&mut self, command: Command) -> Result<(), SendError> {
        if let Command::QueueJump { position } = command
            && position >= self.positions
        {
            return Err(SendError::UnknownPosition {
                position,
                positions: self.positions,
            });
        }
        self.producer
            .push(command)
            .map_err(|rtrb::PushError::Full(command)| SendError::Full(command))
    }

    /// Number of positions in the player's arrangement.
    pub fn positions(&self) -> usize {
        self.positions
    }
}

/// Why a command was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// The queue holds [`COMMAND_CAPACITY`] commands the player has not
    /// applied yet.
    Full(Command),
    /// A jump target outside the arrangement.
    UnknownPosition {
        /// The requested position.
        position: usize,
        /// Number of positions in the arrangement.
        positions: usize,
    },
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full(command) => write!(f, "the command queue is full; {command:?} was not sent"),
            Self::UnknownPosition {
                position,
                positions,
            } => write!(
                f,
                "position {position} is outside the arrangement, which has {positions} positions"
            ),
        }
    }
}

impl Error for SendError {}

/// Why a [`Player`] could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveError {
    /// The song has no arrangement with this id.
    UnknownArrangement {
        /// The requested id.
        id: String,
    },
    /// The output runs at another rate than the song; there is no resampling.
    SampleRateMismatch {
        /// The song's sample rate.
        song: u32,
        /// The requested output rate.
        output: u32,
    },
    /// The maximum buffer size is 0.
    ZeroBufferSize,
    /// [`crate::MixSettings::stem_gains`] names a stem with no source.
    UnknownStemGain {
        /// The id with a gain but no source.
        id: String,
    },
}

impl fmt::Display for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArrangement { id } => write!(f, "unknown arrangement \"{id}\""),
            Self::SampleRateMismatch { song, output } => write!(
                f,
                "the song runs at {song} Hz but the output at {output} Hz; resampling is not supported"
            ),
            Self::ZeroBufferSize => write!(f, "the maximum buffer size must be at least one frame"),
            Self::UnknownStemGain { id } => write!(
                f,
                "a gain is set for stem \"{id}\", but no stem source has that id"
            ),
        }
    }
}

impl Error for LiveError {}
