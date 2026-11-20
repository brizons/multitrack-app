//! Use cases and ports for live playback: the one API every shell calls (S1).
//!
//! A [`Session`] opens one song package for streaming, builds the live player
//! for one arrangement, and plays it on an audio device through the
//! [`AudioOutput`] port. Shells only send [`Command`]s and draw the
//! [`Snapshot`]s [`Session::poll`] returns, about 30 times a second.
//! [`Session::open`] is two steps a shell can also take apart:
//! [`Session::prepare`] checks the song and builds its player with no
//! device, and [`PreparedSong::start`] opens the device, so the next song
//! can be checked while another one still plays on that device.
//!
//! # Threads
//!
//! - The **control thread** owns the [`Session`]: commands travel to the
//!   audio thread over the player's lock-free queue, and every snapshot is
//!   read from a `triple_buffer` the audio thread writes.
//! - The **audio thread** belongs to the output adapter. It calls
//!   [`AudioRenderer::render`], which renders one buffer and publishes the
//!   player's state: no allocation, no lock, no I/O.
//! - The **disk thread** belongs to the streamed song (`library`).
//!
//! The player is only ever dropped on the control thread: a closed stream's
//! renderer hands it back instead of dropping it, because dropping it joins
//! the disk thread.
//!
//! # Rules the session adds to the engine
//!
//! - Play from stopped or paused waits until the disk has the audio at the
//!   playhead ([`TransportState::Starting`]), at most
//!   [`SessionOptions::start_timeout`]; a cold start never plays a silent
//!   first buffer.
//! - The output runs at the song's sample rate; a device that cannot is an
//!   error, since nothing resamples.
//! - The seam crossfade of the player and of the disk stream come from one
//!   option, and the window the disk keeps ahead of the playhead must cover
//!   [`MAX_BUFFER_FRAMES`].
//! - The device that opens is pinned by its id ([`DeviceInfo::id`]): the
//!   session only ever comes back to that device, never to the new default
//!   output or to another device with the same name.
//! - A lost device pauses the transport with a message that says why the
//!   stream stopped; when it comes back, the stream opens again with the
//!   transport still paused, and a device that is back but refuses to open
//!   says why.
//! - A queued jump waiting for its audio shows in [`Snapshot::last_error`]
//!   until it is over; so do a play that gave up waiting for the disk and a
//!   lost device. A decode failure stays until a newer problem replaces it,
//!   and comes back when that problem clears, because the failed audio
//!   plays as silence every time it comes round.
//! - Play decides whether the disk is ready from a report that describes the
//!   playhead every command sent before it left, never from an older one.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod error;
mod output;
mod renderer;
mod session;
mod song;
pub mod testing;

pub use engine::live::{FadeState, JumpKind, JumpTiming};
pub use error::{OpenError, RuntimeError};
pub use library::{LoadError, SongProblem};
pub use output::{
    AudioOutput, DEFAULT_BUFFER_FRAMES, DeviceChoice, DeviceInfo, LossReason, MAX_BUFFER_FRAMES,
    MIN_BUFFER_FRAMES, OutputError, OutputRequest, OutputStream, StreamHealth, StreamInfo,
};
pub use renderer::AudioRenderer;
pub use session::{
    Command, DeviceStatus, FadeView, PreparedSong, QueuedView, Session, SessionOptions, Snapshot,
    TransportState,
};
pub use song::{PositionInfo, SongInfo};
