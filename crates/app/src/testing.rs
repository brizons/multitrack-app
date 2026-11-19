//! An audio output with no hardware, for tests and headless tools.
//!
//! [`FakeOutput`] implements [`AudioOutput`]. Nothing runs on its own: the
//! test calls [`FakeOutput::render`] to run one audio callback, so the
//! player is driven deterministically. Devices can be unplugged and plugged
//! back in, lose their stream for another reason, and change the rates they
//! run at.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use crate::output::{
    AudioOutput, DeviceInfo, LossReason, OutputError, OutputRequest, OutputStream, StreamHealth,
    StreamInfo,
};
use crate::renderer::AudioRenderer;

/// Name of the device [`FakeOutput::new`] offers.
pub const FAKE_DEVICE: &str = "Fake Output";

/// An in-process audio output; clones share the same devices and stream.
#[derive(Clone)]
pub struct FakeOutput {
    shared: Arc<Shared>,
}

struct Shared {
    state: Mutex<State>,
    audio_thread_drops: AtomicUsize,
}

#[derive(Default)]
struct State {
    devices: Vec<Device>,
    stream: Option<Running>,
    next_id: u64,
    opened: Vec<OutputRequest>,
    drop_on_audio_thread: bool,
}

struct Device {
    name: String,
    channels: u16,
    /// `None` for any rate.
    rates: Option<Vec<u32>>,
    present: bool,
}

struct Running {
    id: u64,
    renderer: AudioRenderer,
    device: String,
    lost: Option<LossReason>,
}

impl Default for FakeOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeOutput {
    /// One stereo device, [`FAKE_DEVICE`], that runs at any rate.
    pub fn new() -> Self {
        Self::without_devices().with_device(FAKE_DEVICE, 2, None)
    }

    /// No device at all.
    pub fn without_devices() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                audio_thread_drops: AtomicUsize::new(0),
            }),
        }
    }

    /// Adds a device with `channels` outputs that runs at `rates` (any rate
    /// when `None`). The first device present is the default.
    pub fn with_device(self, name: &str, channels: u16, rates: Option<&[u32]>) -> Self {
        self.lock().devices.push(Device {
            name: name.to_owned(),
            channels,
            rates: rates.map(<[u32]>::to_vec),
            present: true,
        });
        self
    }

    /// Runs one audio callback of `frames` frames and returns its
    /// interleaved stereo output; `None` when no stream plays.
    pub fn render(&self, frames: usize) -> Option<Vec<f32>> {
        let mut state = self.lock();
        let running = state
            .stream
            .as_mut()
            .filter(|running| running.lost.is_none())?;
        let mut out = vec![0.0f32; 2 * frames];
        running.renderer.render(&mut out);
        Some(out)
    }

    /// Removes device `name`: a stream on it stops calling back and reports
    /// [`StreamHealth::Lost`] with [`LossReason::Unplugged`].
    pub fn unplug(&self, name: &str) {
        let mut state = self.lock();
        for device in state.devices.iter_mut().filter(|d| d.name == name) {
            device.present = false;
        }
        if let Some(running) = state.stream.as_mut().filter(|r| r.device == name) {
            running.lost = Some(LossReason::Unplugged);
        }
    }

    /// Ends the stream on device `name` for `reason`; the device stays.
    pub fn lose(&self, name: &str, reason: LossReason) {
        if let Some(running) = self.lock().stream.as_mut().filter(|r| r.device == name) {
            running.lost = Some(reason);
        }
    }

    /// From now on, device `name` runs at `rates` (any rate when `None`).
    pub fn set_rates(&self, name: &str, rates: Option<&[u32]>) {
        for device in self.lock().devices.iter_mut().filter(|d| d.name == name) {
            device.rates = rates.map(<[u32]>::to_vec);
        }
    }

    /// Puts device `name` back.
    pub fn plug(&self, name: &str) {
        for device in self.lock().devices.iter_mut().filter(|d| d.name == name) {
            device.present = true;
        }
    }

    /// Whether a stream is open and playing.
    pub fn is_streaming(&self) -> bool {
        self.lock()
            .stream
            .as_ref()
            .is_some_and(|r| r.lost.is_none())
    }

    /// Every request [`AudioOutput::open`] accepted, in order.
    pub fn opened(&self) -> Vec<OutputRequest> {
        self.lock().opened.clone()
    }

    /// From now on, a closed stream drops its renderer on a thread of its
    /// own, as a real audio thread might, without waiting for it.
    pub fn drop_renderers_on_audio_thread(&self) {
        self.lock().drop_on_audio_thread = true;
    }

    /// Renderers that thread has finished dropping.
    pub fn renderers_dropped_on_audio_thread(&self) -> usize {
        self.shared.audio_thread_drops.load(Ordering::SeqCst)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl AudioOutput for FakeOutput {
    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        let state = self.lock();
        let present: Vec<&Device> = state.devices.iter().filter(|d| d.present).collect();
        Ok(present
            .iter()
            .enumerate()
            .map(|(index, device)| DeviceInfo {
                name: device.name.clone(),
                output_channels: device.channels,
                is_default: index == 0,
            })
            .collect())
    }

    fn open(
        &mut self,
        request: &OutputRequest,
        renderer: AudioRenderer,
    ) -> Result<Box<dyn OutputStream>, OutputError> {
        let mut state = self.lock();
        let present: Vec<&Device> = state.devices.iter().filter(|d| d.present).collect();
        let device = match &request.device {
            Some(name) => present.iter().find(|d| &d.name == name).ok_or_else(|| {
                OutputError::DeviceNotFound {
                    name: name.clone(),
                    available: present.iter().map(|d| d.name.clone()).collect(),
                }
            })?,
            None => present.first().ok_or(OutputError::NoDevice)?,
        };
        if device.channels < 2 {
            return Err(OutputError::NotStereo {
                device: device.name.clone(),
                channels: device.channels,
            });
        }
        if let Some(rates) = &device.rates
            && !rates.contains(&request.sample_rate)
        {
            return Err(OutputError::UnsupportedSampleRate {
                device: device.name.clone(),
                requested: request.sample_rate,
                supported: rates.iter().map(|&rate| (rate, rate)).collect(),
            });
        }
        if state.stream.is_some() {
            return Err(OutputError::Device {
                device: device.name.clone(),
                message: "the fake output plays one stream at a time".to_owned(),
            });
        }
        let info = StreamInfo {
            device: device.name.clone(),
            sample_rate: request.sample_rate,
            buffer_frames: request.buffer_frames,
            channels: device.channels,
        };
        state.next_id += 1;
        let id = state.next_id;
        state.opened.push(request.clone());
        state.stream = Some(Running {
            id,
            renderer,
            device: info.device.clone(),
            lost: None,
        });
        Ok(Box::new(FakeStream {
            shared: Arc::clone(&self.shared),
            id,
            info,
        }))
    }
}

struct FakeStream {
    shared: Arc<Shared>,
    id: u64,
    info: StreamInfo,
}

impl FakeStream {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl OutputStream for FakeStream {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn health(&self) -> StreamHealth {
        match self.lock().stream.as_ref().filter(|r| r.id == self.id) {
            Some(Running { lost: None, .. }) => StreamHealth::Running,
            Some(Running {
                lost: Some(reason), ..
            }) => StreamHealth::Lost { reason: *reason },
            None => StreamHealth::Lost {
                reason: LossReason::Unplugged,
            },
        }
    }
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        let (running, on_audio_thread) = {
            let mut state = self.lock();
            let mine = state.stream.as_ref().is_some_and(|r| r.id == self.id);
            (
                if mine { state.stream.take() } else { None },
                state.drop_on_audio_thread,
            )
        };
        let Some(running) = running else {
            return;
        };
        if on_audio_thread {
            let shared = Arc::clone(&self.shared);
            thread::Builder::new()
                .name("fake-audio".to_owned())
                .spawn(move || {
                    drop(running);
                    shared.audio_thread_drops.fetch_add(1, Ordering::SeqCst);
                })
                .expect("the fake audio thread starts");
        } else {
            drop(running);
        }
    }
}
