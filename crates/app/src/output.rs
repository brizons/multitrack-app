//! The audio-output port: what a session needs from an audio device.
//!
//! An adapter implements [`AudioOutput`] for real hardware (`io-cpal`);
//! [`crate::testing::FakeOutput`] implements it for tests. The session asks
//! for one stream at the song's sample rate and hands it an
//! [`AudioRenderer`], which the adapter calls from its audio thread.

use std::fmt;
use std::time::Duration;

use thiserror::Error;

use crate::AudioRenderer;

/// Smallest buffer a session opens a device with, in frames (F8.5).
pub const MIN_BUFFER_FRAMES: u32 = 64;

/// Largest buffer a session opens a device with, in frames (F8.5).
pub const MAX_BUFFER_FRAMES: u32 = 1_024;

/// Buffer size a session uses unless the shell chooses one.
pub const DEFAULT_BUFFER_FRAMES: u32 = 256;

/// An audio output device (F8.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Name, as [`OutputRequest::device`] takes it.
    pub name: String,
    /// Most output channels the device offers.
    pub output_channels: u16,
    /// Whether it is the system's default output.
    pub is_default: bool,
}

/// The stream a session asks a port to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputRequest {
    /// Device name, as [`DeviceInfo::name`]; `None` for the system default
    /// output.
    pub device: Option<String>,
    /// Frames per second: the song's rate, since nothing resamples.
    pub sample_rate: u32,
    /// Frames per audio callback.
    pub buffer_frames: u32,
}

/// The stream a port opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    /// Name of the device it plays on.
    pub device: String,
    /// Frames per second.
    pub sample_rate: u32,
    /// Frames per audio callback.
    pub buffer_frames: u32,
    /// Channels the stream opened on the device; the song plays on the
    /// first two, the others get silence.
    pub channels: u16,
}

impl StreamInfo {
    /// Latency one buffer adds: its frames at the sample rate (F8.5).
    pub fn latency(&self) -> Duration {
        Duration::from_secs_f64(f64::from(self.buffer_frames) / f64::from(self.sample_rate))
    }
}

/// Whether an open stream still plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamHealth {
    /// The device calls the renderer.
    Running,
    /// The device stopped calling the renderer for good: it was unplugged,
    /// or the system invalidated the stream.
    Lost {
        /// What the port knows about why.
        reason: String,
    },
}

/// An open output stream. Dropping it stops the stream and drops the
/// [`AudioRenderer`] it was given.
pub trait OutputStream: Send {
    /// What was opened.
    fn info(&self) -> &StreamInfo;

    /// Whether the stream still plays. Called from the control thread.
    fn health(&self) -> StreamHealth;
}

/// The audio-output port.
pub trait AudioOutput: Send {
    /// The output devices (F8.1).
    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError>;

    /// Opens `request.device` at exactly `request.sample_rate` and
    /// `request.buffer_frames`, plays the renderer's stereo frames on its
    /// outputs 1 and 2, and starts calling [`AudioRenderer::render`] from the
    /// audio thread.
    ///
    /// The port calls the renderer from one thread at a time, and drops it
    /// when the returned stream is dropped, or before returning an error.
    fn open(
        &mut self,
        request: &OutputRequest,
        renderer: AudioRenderer,
    ) -> Result<Box<dyn OutputStream>, OutputError>;
}

/// Why a port could not list devices or open a stream.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OutputError {
    /// The system has no output device.
    #[error("no audio output device is available")]
    NoDevice,
    /// No device has the requested name.
    #[error(
        "no audio output device is named \"{name}\"; available: {}",
        Names(available)
    )]
    DeviceNotFound {
        /// The requested name.
        name: String,
        /// Names of the devices there are.
        available: Vec<String>,
    },
    /// The device cannot run at the song's rate (no resampling).
    #[error(
        "audio device \"{device}\" cannot play at {requested} Hz, the song's sample rate (nothing resamples); it supports {}",
        Rates(supported)
    )]
    UnsupportedSampleRate {
        /// Device name.
        device: String,
        /// The song's rate.
        requested: u32,
        /// Inclusive ranges of rates the device supports.
        supported: Vec<(u32, u32)>,
    },
    /// The device cannot use the requested buffer size.
    #[error(
        "audio device \"{device}\" cannot use a buffer of {requested} frames; it supports {min} to {max}"
    )]
    UnsupportedBufferSize {
        /// Device name.
        device: String,
        /// Requested frames.
        requested: u32,
        /// Smallest buffer the device takes.
        min: u32,
        /// Largest buffer the device takes.
        max: u32,
    },
    /// The device has fewer than two output channels.
    #[error("audio device \"{device}\" has {channels} output channel(s); stereo playback needs 2")]
    NotStereo {
        /// Device name.
        device: String,
        /// Its output channels.
        channels: u16,
    },
    /// The device takes no sample format the adapter can write.
    #[error("audio device \"{device}\" takes no sample format this player can write")]
    UnsupportedSampleFormat {
        /// Device name.
        device: String,
    },
    /// The device refused or failed.
    #[error("audio device \"{device}\": {message}")]
    Device {
        /// Device name.
        device: String,
        /// What the system said.
        message: String,
    },
    /// The audio system itself failed, before any device was chosen.
    #[error("audio system: {message}")]
    Host {
        /// What the system said.
        message: String,
    },
}

struct Names<'a>(&'a [String]);

impl fmt::Display for Names<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("none");
        }
        for (index, name) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            write!(f, "\"{name}\"")?;
        }
        Ok(())
    }
}

struct Rates<'a>(&'a [(u32, u32)]);

impl fmt::Display for Rates<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("no rate");
        }
        for (index, &(min, max)) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            if min == max {
                write!(f, "{min} Hz")?;
            } else {
                write!(f, "{min} to {max} Hz")?;
            }
        }
        Ok(())
    }
}
