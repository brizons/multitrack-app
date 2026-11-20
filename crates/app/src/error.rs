//! Why a session could not open, and what went wrong while it runs.

use std::time::Duration;

use engine::live::LiveError;
use library::LoadError;
use thiserror::Error;

use crate::output::{MAX_BUFFER_FRAMES, MIN_BUFFER_FRAMES, OutputError};

/// Why [`crate::Session::open`] failed.
#[derive(Debug, Error)]
pub enum OpenError {
    /// The buffer size is outside what F8.5 allows.
    #[error(
        "the buffer size must be {MIN_BUFFER_FRAMES} to {MAX_BUFFER_FRAMES} frames, not {frames}"
    )]
    BufferSize {
        /// The requested frames.
        frames: u32,
    },
    /// The song folder could not be read or checked; [`LoadError::Invalid`]
    /// lists every problem.
    #[error(transparent)]
    Load(#[from] LoadError),
    /// The window the disk keeps decoded ahead of the playhead would not
    /// cover the largest buffer: the song's rate is too low.
    #[error(
        "the song's sample rate ({sample_rate} Hz) is too low to stream: {ahead_frames} frames kept ahead of the playhead do not cover a buffer of {MAX_BUFFER_FRAMES} frames"
    )]
    RateTooLow {
        /// The song's rate.
        sample_rate: u32,
        /// Frames the stream keeps ahead of the playhead at that rate.
        ahead_frames: u64,
    },
    /// The player could not be built, such as for an unknown arrangement.
    #[error(transparent)]
    Live(#[from] LiveError),
    /// The audio device could not be opened.
    #[error(transparent)]
    Output(#[from] OutputError),
}

/// What went wrong while a session runs: returned by
/// [`crate::Session::send`] and kept as [`crate::Snapshot::last_error`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeError {
    /// Play waited for the disk to load the audio at the playhead, and it
    /// did not arrive in time (advisory A4).
    #[error(
        "the audio at the playhead did not load from disk within {:.1} s; playback did not start",
        .waited.as_secs_f64()
    )]
    StartTimeout {
        /// How long play waited.
        waited: Duration,
    },
    /// The device stopped (F8 acceptance).
    #[error(
        "audio device \"{device}\" is disconnected ({reason}); playback is paused until it is plugged back in"
    )]
    DeviceDisconnected {
        /// The device that played.
        device: String,
        /// What the port said.
        reason: String,
    },
    /// The disk thread could not decode some blocks (advisory A5).
    #[error("the disk could not decode {blocks} block(s) of audio; they play as silence")]
    DecodeFailed {
        /// Blocks that failed so far.
        blocks: u64,
    },
    /// A queued jump passed boundaries because its audio was not loaded
    /// (R1-001): the operator sees that it waits, and for how long.
    #[error(
        "the jump to {section} is waiting for its audio from disk; it has moved {deferrals} boundary(ies) on"
    )]
    JumpWaiting {
        /// Label of the target section.
        section: String,
        /// Boundaries it has passed.
        deferrals: u32,
    },
    /// The command queue to the audio thread is full.
    #[error("the audio thread has not taken the last commands yet; {command} was not sent")]
    QueueFull {
        /// The command that was not sent.
        command: String,
    },
    /// A jump to a position outside the arrangement.
    #[error("position {position} is outside the arrangement, which has {positions} positions")]
    UnknownPosition {
        /// The requested position.
        position: usize,
        /// Positions in the arrangement.
        positions: usize,
    },
    /// The audio output never handed the player back after its stream
    /// closed, so this session cannot play any more.
    #[error("the audio output did not release the player; open the song again")]
    PlayerLost,
}
