//! The Audio output screen without a window (S3, F8): the output devices
//! with their output counts, the rate the device runs at, the buffer size
//! with its latency, split mode for 2-output devices, and the routing
//! matrix of buses by outputs with its presets. Every change applies to
//! the song playing at once and is saved for its device (keyed by the
//! device's id); with nothing playing, the chosen device's routing is
//! edited and saved for the next session.

use app::{DeviceInfo, DeviceStatus, LossReason, OutputError, RuntimeError};
use domain::{Bus, Destination, Routing};

use crate::audio_settings::AudioSettings;
use crate::show::Show;

/// The buffer sizes offered, in frames (F8.5).
pub const BUFFER_SIZES: [u32; 5] = [64, 128, 256, 512, 1024];

/// The output counts with a preset of their own in the Preset menu.
const PRESETS: [u16; 6] = [1, 2, 3, 4, 6, 8];

/// What a click on output `output` of bus `bus`'s row makes of `routing`.
/// A bus on one output gets a stereo pair with the output clicked; a bus on
/// a pair keeps the other side alone when one side is clicked, and a
/// third output starts over: the bus plays on that output alone. The only
/// output of a bus stays, since a bus always plays somewhere. In split
/// mode a click turns split mode off and edits the buses as stored.
pub fn click_cell(routing: &Routing, bus: Bus, output: u16) -> Routing {
    let mut routing = routing.clone();
    routing.split_mode = false;
    let destination = match routing.destination(bus) {
        Destination::Mono(alone) if alone == output => Destination::Mono(alone),
        Destination::Mono(alone) => Destination::Stereo {
            left: alone.min(output),
            right: alone.max(output),
        },
        Destination::Stereo { left, right } if output == left => Destination::Mono(right),
        Destination::Stereo { left, right } if output == right => Destination::Mono(left),
        Destination::Stereo { .. } => Destination::Mono(output),
    };
    routing.buses.insert(bus, destination);
    routing
}

/// A problem the session reports that the screen shows:
/// [`RuntimeError::RoutingRefitted`].
pub fn problem_notice(error: &RuntimeError) -> Option<String> {
    matches!(error, RuntimeError::RoutingRefitted { .. }).then(|| error.to_string())
}

/// Why a device could not be opened, when the screen shows it:
/// [`OutputError::TooFewChannels`].
pub fn output_notice(error: &OutputError) -> Option<String> {
    matches!(error, OutputError::TooFewChannels { .. }).then(|| error.to_string())
}

/// Why `show` did not take a change that `result` reports: an error other
/// than a full queue (which still reaches the players), or a device that
/// refused to open again with the new settings and is held as a lost one.
fn refusal(result: Result<(), RuntimeError>, show: &mut Show) -> Option<String> {
    if let Err(error) = result
        && !matches!(error, RuntimeError::QueueFull { .. })
    {
        return Some(error.to_string());
    }
    show.poll();
    match &show.snapshot().device {
        DeviceStatus::Disconnected {
            reason: LossReason::Reconfigured,
            reopen_error,
            device,
        } => Some(reopen_error.as_ref().map_or_else(
            || format!("{device} refuses the new settings"),
            ToString::to_string,
        )),
        _ => None,
    }
}

/// The preset `routing` is for a device with `outputs` outputs, such as
/// "8 outputs" (the device's own first, then the Preset menu's), or
/// "Custom" once it is none of them.
fn preset_name(routing: &Routing, outputs: u16) -> String {
    std::iter::once(outputs)
        .chain(PRESETS)
        .find(|&preset| Routing::preset(preset).fit(outputs) == *routing)
        .map_or_else(|| "Custom".to_owned(), outputs_text)
}

/// "8 kHz", "44.1 kHz".
fn rate_text(rate: u32) -> String {
    if rate.is_multiple_of(1_000) {
        format!("{} kHz", rate / 1_000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1_000.0)
    }
}

fn outputs_text(outputs: u16) -> String {
    format!("{outputs} output{}", if outputs == 1 { "" } else { "s" })
}

/// One device of the device menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// Such as "Interface · 8 outputs".
    pub label: String,
    /// Whether it is the device in use (or chosen, with nothing playing).
    pub chosen: bool,
}

/// One bus's row of the routing matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingRow {
    /// The bus.
    pub bus: Bus,
    /// Its name.
    pub name: &'static str,
    /// Whether it plays on each output, from output 1.
    pub cells: Vec<bool>,
}

/// Everything the Audio output screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputScreen {
    /// The devices, in the order the system lists them.
    pub devices: Vec<DeviceRow>,
    /// The device in use, as its row reads; "No output device" without one.
    pub device: String,
    /// Whether the device menu is open.
    pub device_menu: bool,
    /// The rate the device runs at, such as "48 kHz".
    pub sample_rate: String,
    /// The buffer sizes offered.
    pub buffer_sizes: [u32; 5],
    /// The buffer size in use.
    pub buffer: u32,
    /// The latency the buffer adds, such as "5.3 ms"; empty with nothing
    /// playing, since it depends on the rate.
    pub latency: String,
    /// Whether split mode is on, for a 2-output device; `None` for others.
    pub split_mode: Option<bool>,
    /// The line beside the matrix's title.
    pub hint: &'static str,
    /// The device's outputs: the matrix's columns.
    pub outputs: u16,
    /// The matrix, a row per bus in mixer order.
    pub rows: Vec<RoutingRow>,
    /// Such as "Preset: 8 outputs".
    pub preset: String,
    /// Whether the Preset menu is open.
    pub preset_menu: bool,
    /// The presets the device can play, as (outputs, label).
    pub presets: Vec<(u16, String)>,
    /// Why a settings file is not used, a routing was refitted or refused,
    /// a device has too few outputs, or a change could not be saved.
    pub notices: Vec<String>,
}

/// What an Audio output screen control asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputAction {
    /// The device button: shows the devices, or hides them.
    OpenDeviceMenu,
    /// Plays on the device at this index of the menu.
    ChooseDevice(usize),
    /// A buffer size.
    Buffer(u32),
    /// The split mode switch.
    ToggleSplit,
    /// A cell of the matrix ([`click_cell`]).
    Cell {
        /// Its row.
        bus: Bus,
        /// Its output, from 1.
        output: u16,
    },
    /// "Preset": shows the presets, or hides them.
    OpenPresetMenu,
    /// The preset for this many outputs, fitted to the device.
    Preset(u16),
    /// "Reset": the device's own preset.
    Reset,
}

/// The device whose routing the screen edits.
struct Current {
    id: String,
    outputs: u16,
    routing: Routing,
}

/// The Audio output screen's own state: the devices listed, the menus,
/// the preset last applied and the last problem.
#[derive(Debug, Default)]
pub struct OutputPage {
    devices: Vec<DeviceInfo>,
    device_menu: bool,
    preset_menu: bool,
    problem: Option<String>,
}

impl OutputPage {
    /// The screen with no device listed yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The output devices the system lists now.
    pub fn set_devices(&mut self, devices: Vec<DeviceInfo>) {
        self.devices = devices;
    }

    /// The device in use: the one `show` plays on, else the one `settings`
    /// opens the next session on.
    fn current(&self, show: Option<&Show>, settings: &AudioSettings) -> Option<Current> {
        if let Some(show) = show {
            let snapshot = show.snapshot();
            let routing = snapshot.mixer.routing.clone();
            if let DeviceStatus::Connected(stream) = &snapshot.device {
                let outputs = self
                    .devices
                    .iter()
                    .find(|device| device.id == stream.device_id)
                    .map_or(routing.outputs, |device| device.output_channels);
                return Some(Current {
                    id: stream.device_id.clone(),
                    outputs,
                    routing,
                });
            }
        }
        settings.device(&self.devices).map(|device| Current {
            id: device.id.clone(),
            outputs: device.output_channels,
            routing: settings.routing_for(device),
        })
    }

    /// Does what `action` asks of the song `show` plays and the settings;
    /// returns whether the song must open again, on another device.
    pub fn act(
        &mut self,
        action: OutputAction,
        show: Option<&mut Show>,
        settings: &mut AudioSettings,
    ) -> bool {
        let current = self.current(show.as_deref(), settings);
        let routing = match action {
            OutputAction::OpenDeviceMenu => {
                self.device_menu = !self.device_menu;
                return false;
            }
            OutputAction::OpenPresetMenu => {
                self.preset_menu = !self.preset_menu;
                return false;
            }
            OutputAction::ChooseDevice(index) => {
                self.device_menu = false;
                let Some(device) = self.devices.get(index) else {
                    return false;
                };
                let changed = current.is_none_or(|current| current.id != device.id);
                settings.choose(device);
                return changed && show.is_some();
            }
            OutputAction::Buffer(frames) => {
                let refused = match show {
                    Some(show) => refusal(show.set_buffer_frames(frames), show),
                    None => (!(BUFFER_SIZES[0]..=BUFFER_SIZES[4]).contains(&frames))
                        .then(|| RuntimeError::BufferSize { frames }.to_string()),
                };
                // The next sessions keep the size the device last took.
                self.problem = match refused {
                    Some(why) => Some(format!("The buffer size could not be changed: {why}")),
                    None => {
                        settings.set_buffer_frames(frames);
                        None
                    }
                };
                return false;
            }
            OutputAction::ToggleSplit => current.as_ref().map(|current| {
                let mut routing = current.routing.clone();
                routing.split_mode = !routing.split_mode;
                routing
            }),
            OutputAction::Cell { bus, output } => current
                .as_ref()
                .map(|current| click_cell(&current.routing, bus, output)),
            OutputAction::Preset(outputs) => {
                self.preset_menu = false;
                current
                    .as_ref()
                    .map(|current| Routing::preset(outputs).fit(current.outputs))
            }
            OutputAction::Reset => current
                .as_ref()
                .map(|current| Routing::preset(current.outputs)),
        };
        if let (Some(current), Some(routing)) = (current, routing) {
            self.apply(&current.id, routing, show, settings);
        }
        false
    }

    /// Plays `show` with `routing` and saves it for the device `device`.
    fn apply(
        &mut self,
        device: &str,
        routing: Routing,
        show: Option<&mut Show>,
        settings: &mut AudioSettings,
    ) {
        // Only a routing the session took is saved.
        let refused = show.and_then(|show| refusal(show.set_routing(routing.clone()), show));
        self.problem = match refused {
            Some(why) => Some(format!("The routing could not be applied: {why}")),
            None => settings
                .save_routing(device, &routing)
                .err()
                .map(|error| format!("The routing could not be saved: {error}")),
        };
    }

    /// What the screen shows of the song `show` plays and the settings.
    pub fn view(&self, show: Option<&Show>, settings: &AudioSettings) -> OutputScreen {
        let current = self.current(show, settings);
        let stream = show.and_then(|show| match &show.snapshot().device {
            DeviceStatus::Connected(stream) => Some(stream.clone()),
            _ => None,
        });
        let label = |device: &DeviceInfo| {
            format!("{} · {}", device.name, outputs_text(device.output_channels))
        };
        let devices = self
            .devices
            .iter()
            .map(|device| DeviceRow {
                label: label(device),
                chosen: current
                    .as_ref()
                    .is_some_and(|current| current.id == device.id),
            })
            .collect();
        let device = current
            .as_ref()
            .and_then(|current| self.devices.iter().find(|device| device.id == current.id))
            .map_or_else(|| "No output device".to_owned(), label);
        let outputs = current.as_ref().map_or(2, |current| current.outputs);
        let routing = current.as_ref().map_or_else(
            || Routing::stereo(outputs),
            |current| current.routing.clone(),
        );
        let split = routing.split_mode && outputs >= 2;
        let rows = Bus::ALL
            .into_iter()
            .map(|bus| {
                let on = routing.destination(bus).outputs();
                RoutingRow {
                    bus,
                    name: bus.name(),
                    cells: (1..=outputs).map(|output| on.contains(&output)).collect(),
                }
            })
            .collect();
        let presets = PRESETS
            .into_iter()
            .filter(|&preset| preset <= outputs)
            .map(|preset| (preset, outputs_text(preset)))
            .collect();
        let mut notices: Vec<String> = settings.notices().to_vec();
        if let Some(show) = show {
            let snapshot = show.snapshot();
            notices.extend(snapshot.last_error.as_ref().and_then(problem_notice));
            if let DeviceStatus::Disconnected {
                reopen_error: Some(error),
                ..
            } = &snapshot.device
            {
                notices.extend(output_notice(error));
            }
        }
        notices.extend(self.problem.clone());
        OutputScreen {
            devices,
            device,
            device_menu: self.device_menu,
            sample_rate: stream.as_ref().map_or_else(
                || "Set by the song that plays".to_owned(),
                |stream| rate_text(stream.sample_rate),
            ),
            buffer_sizes: BUFFER_SIZES,
            buffer: stream
                .as_ref()
                .map_or(settings.buffer_frames(), |stream| stream.buffer_frames),
            latency: stream.as_ref().map_or_else(String::new, |stream| {
                format!("{:.1} ms", stream.latency().as_secs_f64() * 1_000.0)
            }),
            split_mode: (outputs == 2).then_some(routing.split_mode),
            hint: if split {
                "Split mode is on: only outputs 1 and 2 are used"
            } else {
                "Tap a cell to send a bus to an output"
            },
            outputs,
            rows,
            preset: format!("Preset: {}", preset_name(&routing, outputs)),
            preset_menu: self.preset_menu,
            presets,
            notices,
        }
    }
}
