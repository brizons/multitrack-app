//! Audio output through `cpal` (S4): the [`app::AudioOutput`] port on real
//! devices.
//!
//! [`CpalOutput`] lists the output devices with their channel counts (F8.1)
//! and opens one stream per session: at the song's sample rate (a device that
//! cannot run at it is an error; nothing resamples), with a fixed buffer of
//! the size the session asks for (F8.5), stereo on outputs 1 and 2 of one
//! device. A session that asks for the default output gets the device that
//! is the default when it opens, pinned, so the stream never moves to
//! another device on its own.
//!
//! The data callback obeys the real-time checklist: the stereo scratch buffer
//! is allocated when the stream is built, and the callback only renders into
//! it and converts samples; no allocation, lock, I/O or logging. The error
//! callback, which some systems call on the audio thread, only stores an
//! atomic. A stream reports [`StreamHealth::Lost`] once its device is gone
//! or the system invalidated it; the session then pauses and reopens the
//! device when it is back.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use app::{
    AudioOutput, AudioRenderer, DeviceInfo, LossReason, OutputError, OutputRequest, OutputStream,
    StreamHealth, StreamInfo,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, ErrorKind, FromSample, Sample, SampleFormat, SizedSample, SupportedBufferSize,
    SupportedStreamConfigRange,
};

/// The [`AudioOutput`] port on the system's default audio host.
pub struct CpalOutput {
    host: cpal::Host,
}

impl Default for CpalOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalOutput {
    /// Uses the system's default audio host.
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// The output devices, as `cpal` devices with their names.
    fn output_devices(&self) -> Result<Vec<(String, cpal::Device)>, OutputError> {
        let devices = self
            .host
            .output_devices()
            .map_err(|error| OutputError::Host {
                message: error.to_string(),
            })?;
        Ok(devices.map(|device| (device.to_string(), device)).collect())
    }

    /// The device named `name`, or the default output pinned by its name
    /// (the default device itself when no listed device has that name).
    fn find(&self, name: Option<&str>) -> Result<(String, cpal::Device), OutputError> {
        let mut devices = self.output_devices()?;
        let mut default = None;
        let wanted = match name {
            Some(name) => name.to_owned(),
            None => match self.host.default_output_device() {
                Some(device) => default.insert(device).to_string(),
                None => return Err(OutputError::NoDevice),
            },
        };
        let index = devices
            .iter()
            .position(|(candidate, _)| *candidate == wanted)
            .or_else(|| {
                devices
                    .iter()
                    .position(|(candidate, _)| candidate.eq_ignore_ascii_case(&wanted))
            });
        match (index, default) {
            (Some(index), _) => Ok(devices.swap_remove(index)),
            (None, Some(device)) => Ok((wanted, device)),
            (None, None) => Err(OutputError::DeviceNotFound {
                name: wanted,
                available: devices.into_iter().map(|(name, _)| name).collect(),
            }),
        }
    }
}

impl AudioOutput for CpalOutput {
    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        let default = self
            .host
            .default_output_device()
            .map(|device| device.to_string());
        Ok(self
            .output_devices()?
            .into_iter()
            .map(|(name, device)| DeviceInfo {
                output_channels: device
                    .supported_output_configs()
                    .map(|ranges| ranges.map(|range| range.channels()).max().unwrap_or(0))
                    .unwrap_or(0),
                is_default: default.as_deref() == Some(name.as_str()),
                name,
            })
            .collect())
    }

    fn open(
        &mut self,
        request: &OutputRequest,
        renderer: AudioRenderer,
    ) -> Result<Box<dyn OutputStream>, OutputError> {
        let (name, device) = self.find(request.device.as_deref())?;
        let device_error = |error: cpal::Error| OutputError::Device {
            device: name.clone(),
            message: error.to_string(),
        };
        let ranges: Vec<SupportedStreamConfigRange> = device
            .supported_output_configs()
            .map_err(device_error)?
            .collect();
        let choice = choose_config(&name, &ranges, request.sample_rate, request.buffer_frames)?;
        let config = cpal::StreamConfig {
            channels: choice.channels,
            sample_rate: request.sample_rate,
            buffer_size: BufferSize::Fixed(request.buffer_frames),
        };
        let health = Arc::new(AtomicU8::new(RUNNING));
        let max_frames = request.buffer_frames as usize;
        let built = match choice.format {
            SampleFormat::F32 => build::<f32>(&device, &config, renderer, &health, max_frames),
            SampleFormat::I32 => build::<i32>(&device, &config, renderer, &health, max_frames),
            SampleFormat::I16 => build::<i16>(&device, &config, renderer, &health, max_frames),
            SampleFormat::F64 => build::<f64>(&device, &config, renderer, &health, max_frames),
            SampleFormat::U16 => build::<u16>(&device, &config, renderer, &health, max_frames),
            _ => unreachable!("`choose_config` picks only formats `rank` knows"),
        };
        let stream = built.map_err(device_error)?;
        stream.play().map_err(device_error)?;
        let buffer_frames = stream.buffer_size().unwrap_or(request.buffer_frames);
        Ok(Box::new(CpalStream {
            _stream: stream,
            info: StreamInfo {
                device: name.clone(),
                sample_rate: request.sample_rate,
                buffer_frames,
                channels: choice.channels,
            },
            health,
        }))
    }
}

/// Builds a stream of samples of type `T` that renders through `renderer`.
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut renderer: AudioRenderer,
    health: &Arc<AtomicU8>,
    max_frames: usize,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32> + Send + 'static,
{
    let channels = usize::from(config.channels);
    // Allocated here, never in the callback.
    let mut stereo = vec![0.0f32; 2 * max_frames];
    let health = Arc::clone(health);
    device.build_output_stream::<T, _, _>(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            write_interleaved(data, channels, &mut stereo, |piece| renderer.render(piece));
        },
        move |error: cpal::Error| {
            if let Some(reason) = loss_reason(error.kind()) {
                health.store(encode(reason), Ordering::SeqCst);
            }
        },
        None,
    )
}

/// A playing `cpal` stream; dropping it stops the stream and drops the
/// renderer, which hands the player back to the session.
struct CpalStream {
    _stream: cpal::Stream,
    info: StreamInfo,
    health: Arc<AtomicU8>,
}

impl OutputStream for CpalStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn health(&self) -> StreamHealth {
        match decode(self.health.load(Ordering::SeqCst)) {
            None => StreamHealth::Running,
            Some(reason) => StreamHealth::Lost { reason },
        }
    }
}

/// The stream configuration [`choose_config`] picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamChoice {
    /// Channels to open; the song plays on the first two.
    pub channels: u16,
    /// Sample format to write.
    pub format: SampleFormat,
}

/// Picks, among the configurations a device supports (`ranges`), one with at
/// least two channels that runs at `sample_rate` with a buffer of `frames`
/// frames: a float format first, then the fewest channels. Says why there is
/// none: a device with fewer than two outputs, without the song's rate (with
/// the rates it has), without the buffer size (with the sizes it takes), or
/// without a format this adapter writes.
pub fn choose_config(
    device: &str,
    ranges: &[SupportedStreamConfigRange],
    sample_rate: u32,
    frames: u32,
) -> Result<StreamChoice, OutputError> {
    let stereo: Vec<&SupportedStreamConfigRange> = ranges
        .iter()
        .filter(|range| range.channels() >= 2)
        .collect();
    if stereo.is_empty() {
        return Err(OutputError::NotStereo {
            device: device.to_owned(),
            channels: ranges
                .iter()
                .map(|range| range.channels())
                .max()
                .unwrap_or(0),
        });
    }
    let at_rate: Vec<&SupportedStreamConfigRange> = stereo
        .iter()
        .copied()
        .filter(|range| {
            range.min_sample_rate() <= sample_rate && sample_rate <= range.max_sample_rate()
        })
        .collect();
    if at_rate.is_empty() {
        let mut supported: Vec<(u32, u32)> = stereo
            .iter()
            .map(|range| (range.min_sample_rate(), range.max_sample_rate()))
            .collect();
        supported.sort_unstable();
        supported.dedup();
        return Err(OutputError::UnsupportedSampleRate {
            device: device.to_owned(),
            requested: sample_rate,
            supported,
        });
    }
    let writable: Vec<&SupportedStreamConfigRange> = at_rate
        .into_iter()
        .filter(|range| rank(range.sample_format()).is_some())
        .collect();
    if writable.is_empty() {
        return Err(OutputError::UnsupportedSampleFormat {
            device: device.to_owned(),
        });
    }
    let fits = |range: &SupportedStreamConfigRange| match *range.buffer_size() {
        SupportedBufferSize::Range { min, max } => min <= frames && frames <= max,
        SupportedBufferSize::Unknown => true,
    };
    let Some(best) = writable
        .iter()
        .copied()
        .filter(|range| fits(range))
        .min_by_key(|range| (rank(range.sample_format()), range.channels()))
    else {
        let bounds = writable
            .iter()
            .filter_map(|range| match *range.buffer_size() {
                SupportedBufferSize::Range { min, max } => Some((min, max)),
                SupportedBufferSize::Unknown => None,
            });
        let (min, max) = bounds.fold((u32::MAX, 0), |(lo, hi), (min, max)| {
            (lo.min(min), hi.max(max))
        });
        return Err(OutputError::UnsupportedBufferSize {
            device: device.to_owned(),
            requested: frames,
            min,
            max,
        });
    };
    Ok(StreamChoice {
        channels: best.channels(),
        format: best.sample_format(),
    })
}

/// Preference among the formats this adapter writes; `None` for the others.
fn rank(format: SampleFormat) -> Option<u8> {
    match format {
        SampleFormat::F32 => Some(0),
        SampleFormat::F64 => Some(1),
        SampleFormat::I32 => Some(2),
        SampleFormat::I16 => Some(3),
        SampleFormat::U16 => Some(4),
        _ => None,
    }
}

/// Fills `data`, interleaved frames of `channels` samples, from stereo the
/// renderer writes into `stereo` in pieces of at most `stereo.len() / 2`
/// frames: left on output 1, right on output 2, silence on the others.
/// Real-time safe: it only uses the buffers it is given.
pub fn write_interleaved<T>(
    data: &mut [T],
    channels: usize,
    stereo: &mut [f32],
    mut render: impl FnMut(&mut [f32]),
) where
    T: Sample + FromSample<f32>,
{
    let piece_frames = stereo.len() / 2;
    if channels < 2 || piece_frames == 0 {
        data.fill(T::EQUILIBRIUM);
        return;
    }
    let whole = data.len() - data.len() % channels;
    let (frames, rest) = data.split_at_mut(whole);
    rest.fill(T::EQUILIBRIUM);
    for chunk in frames.chunks_mut(piece_frames * channels) {
        let piece = &mut stereo[..2 * (chunk.len() / channels)];
        render(piece);
        for (frame, pair) in chunk.chunks_exact_mut(channels).zip(piece.chunks_exact(2)) {
            frame[0] = T::from_sample(pair[0]);
            frame[1] = T::from_sample(pair[1]);
            frame[2..].fill(T::EQUILIBRIUM);
        }
    }
}

/// A stream that still plays.
const RUNNING: u8 = 0;

/// Every loss reason, in the order the health atomic encodes them from 1.
const LOSSES: [LossReason; 6] = [
    LossReason::Unplugged,
    LossReason::StreamInvalidated,
    LossReason::DeviceChanged,
    LossReason::DeviceBusy,
    LossReason::HostStopped,
    LossReason::Failed,
];

/// What a stream error means for the session: why the stream is lost, or
/// `None` when it plays on (an xrun, or real-time priority refused).
pub fn loss_reason(kind: ErrorKind) -> Option<LossReason> {
    match kind {
        ErrorKind::Xrun | ErrorKind::RealtimeDenied => None,
        ErrorKind::DeviceNotAvailable => Some(LossReason::Unplugged),
        ErrorKind::StreamInvalidated => Some(LossReason::StreamInvalidated),
        ErrorKind::DeviceChanged => Some(LossReason::DeviceChanged),
        ErrorKind::DeviceBusy => Some(LossReason::DeviceBusy),
        ErrorKind::HostUnavailable => Some(LossReason::HostStopped),
        _ => Some(LossReason::Failed),
    }
}

/// The health atomic's value for a stream lost for `reason`.
fn encode(reason: LossReason) -> u8 {
    let index = LOSSES
        .iter()
        .position(|&loss| loss == reason)
        .expect("`LOSSES` lists every reason");
    index as u8 + 1
}

/// The loss reason the health atomic holds; `None` while the stream plays.
fn decode(code: u8) -> Option<LossReason> {
    code.checked_sub(1)
        .and_then(|index| LOSSES.get(usize::from(index)).copied())
}
