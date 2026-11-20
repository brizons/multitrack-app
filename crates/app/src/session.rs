//! A session: one song playing on one audio device.

use std::mem;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use domain::{DEFAULT_ARRANGEMENT_ID, DEFAULT_SEAM_CROSSFADE};
use engine::MixSettings;
use engine::live::{
    self, Controller, DECLICK_FRAMES, DEFAULT_FADE_SECONDS, FadeState, JumpKind, JumpTiming,
    PANIC_MILLISECONDS, Player, PlayerConfig, SendError, Transport,
};
use library::{DiskMonitor, Gate, StreamConfig, StreamingSong, open_streaming};
use rtrb::Consumer;

use crate::error::{OpenError, RuntimeError};
use crate::output::{
    AudioOutput, DEFAULT_BUFFER_FRAMES, MAX_BUFFER_FRAMES, MIN_BUFFER_FRAMES, OutputRequest,
    OutputStream, StreamHealth, StreamInfo,
};
use crate::renderer::{AudioRenderer, AudioState, Report};
use crate::song::SongInfo;

/// How long the session waits for a closed stream to hand the player back.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// How a session opens; [`Default`] gives the usual values.
#[derive(Clone)]
pub struct SessionOptions {
    /// Arrangement to play. Default: `default`.
    pub arrangement: String,
    /// Output device name; `None` for the system default output. Default:
    /// `None`.
    pub device: Option<String>,
    /// Frames per audio callback, [`MIN_BUFFER_FRAMES`] to
    /// [`MAX_BUFFER_FRAMES`] (F8.5). Default: [`DEFAULT_BUFFER_FRAMES`].
    pub buffer_frames: u32,
    /// Crossfade at seams where the source jumps, in frames. The player and
    /// the disk stream both take it from here, so the stream always keeps
    /// the audio a fading-out tail reads (advisory A3). Default:
    /// [`DEFAULT_SEAM_CROSSFADE`].
    pub seam_crossfade: u64,
    /// How long play waits for the disk to load the audio at the playhead
    /// before it gives up with [`RuntimeError::StartTimeout`]. Default: 5 s.
    pub start_timeout: Duration,
    /// How often a session whose device was lost tries to open it again.
    /// Default: 500 ms.
    pub reconnect_interval: Duration,
    /// Test and diagnostics hook passed to the disk thread; see
    /// [`library::Gate`]. Default: `None`.
    pub disk_gate: Option<Gate>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            arrangement: DEFAULT_ARRANGEMENT_ID.to_owned(),
            device: None,
            buffer_frames: DEFAULT_BUFFER_FRAMES,
            seam_crossfade: DEFAULT_SEAM_CROSSFADE,
            start_timeout: Duration::from_secs(5),
            reconnect_interval: Duration::from_millis(500),
            disk_gate: None,
        }
    }
}

/// A command a shell sends with [`Session::send`] (S1, F6).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// Starts or resumes playing. From stopped or paused, it first waits
    /// until the disk has the audio at the playhead (the snapshot says
    /// [`TransportState::Starting`]), at most
    /// [`SessionOptions::start_timeout`].
    Play,
    /// Ramps out and holds the position.
    Pause,
    /// Ramps out and returns to the start.
    Stop,
    /// Moves to the start of the arrangement.
    BackToStart,
    /// Queues a jump to arrangement position `position` (F6.2).
    QueueJump {
        /// Index into [`SongInfo::positions`].
        position: usize,
    },
    /// Drops the queued jump or loop.
    CancelJump,
    /// Chooses when queued jumps execute: at the section end or the next bar.
    SetJumpTiming(JumpTiming),
    /// Repeats the current section once more (F6.3).
    LiveLoop,
    /// Repeats the current section until switched off (F6.3).
    SetInfinite(bool),
    /// Fades the track stems out, or back in (F6.4).
    ToggleFade,
    /// How long a full fade takes.
    SetFadeDuration(Duration),
    /// Fades everything out in 0.5 s and stops (F6.6).
    Panic,
}

/// What the transport is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// Silent at the start of the arrangement.
    Stopped,
    /// Play was pressed; waiting for the disk to load the audio at the
    /// playhead.
    Starting,
    /// Playing.
    Playing,
    /// Ramping out before holding the position.
    Pausing,
    /// Silent, holding the position.
    Paused,
    /// Ramping out before returning to the start.
    Stopping,
    /// Fading everything out before stopping.
    Panicking,
}

/// The jump or loop waiting for its boundary (F6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedView {
    /// Arrangement position it goes to.
    pub target: usize,
    /// Label of that position's section.
    pub target_label: String,
    /// A jump or a live loop.
    pub kind: JumpKind,
    /// Output frame where it executes.
    pub executes_at: u64,
    /// Output frames until it executes.
    pub frames_until: u64,
    /// Boundaries it has passed because its audio was not loaded.
    pub deferrals: u32,
}

/// The fade of the track stems.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeView {
    /// Direction and progress.
    pub state: FadeState,
    /// Gain on the track stems, 0.0 to 1.0.
    pub level: f32,
    /// How long a full fade takes.
    pub duration: Duration,
}

/// Where the audio goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStatus {
    /// A device plays the session.
    Connected(StreamInfo),
    /// The device was lost; the transport is paused until it is back.
    Disconnected {
        /// The device that played.
        device: String,
        /// Why it was lost.
        reason: String,
    },
}

/// The state of a session, for a shell to draw; [`Session::poll`] returns
/// one, and shells call it about 30 times a second.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Transport state.
    pub transport: TransportState,
    /// Output frames played since the last start.
    pub output_frame: u64,
    /// Next source frame to play: the playhead in samples.
    pub source_frame: u64,
    /// Bar of the playhead, from 1.
    pub bar: u64,
    /// Beat of the playhead within its bar, from 1.
    pub beat: u32,
    /// Current arrangement position.
    pub position: usize,
    /// Label of the current section.
    pub section: String,
    /// The queued jump or loop, if any.
    pub queued: Option<QueuedView>,
    /// When queued jumps execute.
    pub jump_timing: JumpTiming,
    /// Whether the current section repeats until switched off.
    pub infinite: bool,
    /// The fade of the track stems.
    pub fade: FadeView,
    /// Where the audio goes.
    pub device: DeviceStatus,
    /// Stem reads the disk had not served in time; each played silence.
    pub underruns: u64,
    /// Blocks the disk thread could not decode.
    pub decode_errors: u64,
    /// The most recent problem, until a newer one replaces it.
    pub last_error: Option<RuntimeError>,
}

impl Snapshot {
    /// Whether a live loop is queued.
    pub fn live_loop(&self) -> bool {
        self.queued
            .as_ref()
            .is_some_and(|queued| queued.kind == JumpKind::Loop)
    }
}

/// Where the player is.
enum Link {
    /// A device calls the renderer that holds it.
    Streaming {
        stream: Box<dyn OutputStream>,
        home: Consumer<Box<AudioState>>,
    },
    /// The device was lost; the control thread holds it until the device is
    /// back.
    Held {
        state: Box<AudioState>,
        device: String,
        reason: String,
        retry_at: Instant,
    },
    /// The output kept the player; nothing plays any more.
    Lost,
}

/// Where a play request stands.
#[derive(Debug, Clone, Copy)]
enum Start {
    /// Waiting for the sources to have the playhead ready.
    Waiting { deadline: Instant },
    /// Play is on its way to the audio thread.
    Sent,
}

/// One song package playing on one audio device: the use cases a shell
/// calls (S1). Shells send [`Command`]s and draw the [`Snapshot`]s
/// [`Self::poll`] returns; they never touch the audio thread.
pub struct Session {
    song: SongInfo,
    output: Box<dyn AudioOutput>,
    request: OutputRequest,
    controller: Controller,
    reports: triple_buffer::Output<Report>,
    monitor: DiskMonitor,
    link: Link,
    start: Option<Start>,
    start_timeout: Duration,
    reconnect_interval: Duration,
    fade_duration: Duration,
    last_error: Option<RuntimeError>,
    decode_errors_seen: u64,
    deferral_seen: Option<(usize, u32)>,
}

impl Session {
    /// Opens the song folder `song_dir` for streaming, builds the player
    /// for `options.arrangement`, and opens the output device at the song's
    /// sample rate. The transport starts stopped.
    pub fn open(
        song_dir: &Path,
        mut output: Box<dyn AudioOutput>,
        options: SessionOptions,
    ) -> Result<Self, OpenError> {
        let buffer_frames = options.buffer_frames;
        if !(MIN_BUFFER_FRAMES..=MAX_BUFFER_FRAMES).contains(&buffer_frames) {
            return Err(OpenError::BufferSize {
                frames: buffer_frames,
            });
        }
        // One value feeds both configurations, so the stream always keeps
        // what the player's seams read (advisory A3).
        let stream_config = StreamConfig {
            seam_crossfade: options.seam_crossfade,
            gate: options.disk_gate.clone(),
            ..StreamConfig::default()
        };
        let StreamingSong {
            song,
            warnings,
            sources,
            monitor,
            capacity,
        } = open_streaming(song_dir, &stream_config)?;
        let sample_rate = song.sample_rate();
        if capacity.ahead_frames < u64::from(MAX_BUFFER_FRAMES) {
            return Err(OpenError::RateTooLow {
                sample_rate,
                ahead_frames: capacity.ahead_frames,
            });
        }
        let player_config = PlayerConfig {
            sample_rate,
            max_frames: buffer_frames as usize,
            seam_crossfade: options.seam_crossfade,
        };
        let (player, controller) = Player::new(
            &song,
            &options.arrangement,
            &MixSettings::default(),
            &player_config,
            sources,
        )?;
        let warnings = warnings.iter().map(ToString::to_string).collect();
        let info = SongInfo::new(&song, &options.arrangement, warnings)
            .expect("`Player::new` accepted the arrangement");

        let first = Report {
            status: player.status(),
            ready: false,
        };
        let (input, reports) = triple_buffer::triple_buffer(&first);
        let mut state = Box::new(AudioState {
            player,
            reports: input,
        });
        state.publish();
        let request = OutputRequest {
            device: options.device.clone(),
            sample_rate,
            buffer_frames,
        };
        let (renderer, home) = AudioRenderer::new(state);
        let (stream, request) = match output.open(&request, renderer) {
            Ok(stream) => {
                // Come back to this very device when it is lost, not to
                // whatever the default output is by then.
                let request = OutputRequest {
                    device: Some(stream.info().device.clone()),
                    ..request
                };
                (stream, request)
            }
            Err(error) => {
                drop(take_back(home));
                return Err(error.into());
            }
        };

        Ok(Self {
            song: info,
            output,
            request,
            controller,
            reports,
            monitor,
            link: Link::Streaming { stream, home },
            start: None,
            start_timeout: options.start_timeout,
            reconnect_interval: options.reconnect_interval,
            fade_duration: Duration::from_secs(u64::from(DEFAULT_FADE_SECONDS)),
            last_error: None,
            decode_errors_seen: 0,
            deferral_seen: None,
        })
    }

    /// The open song and the arrangement being played.
    pub fn song(&self) -> &SongInfo {
        &self.song
    }

    /// The output devices (F8.1).
    pub fn devices(&self) -> Result<Vec<crate::DeviceInfo>, crate::OutputError> {
        self.output.devices()
    }

    /// Watches the disk thread; for tests and diagnostics.
    pub fn disk_monitor(&self) -> DiskMonitor {
        self.monitor.clone()
    }

    /// Sends `command`. An error is also kept as
    /// [`Snapshot::last_error`].
    pub fn send(&mut self, command: Command) -> Result<(), RuntimeError> {
        let result = self.dispatch(command);
        if let Err(error) = &result {
            self.last_error = Some(error.clone());
        }
        result
    }

    /// Moves the session on and returns its state: starts a play that waited
    /// for the disk, notices a lost device (pausing the transport) and opens
    /// it again when it is back, and records decode failures and waiting
    /// jumps as [`Snapshot::last_error`]. Shells call it about 30 times a
    /// second; nothing advances between calls.
    pub fn poll(&mut self) -> Snapshot {
        let now = Instant::now();
        self.watch_device(now);
        let report = *self.reports.read();
        self.advance_start(&report, now);
        self.watch_disk(&report);
        self.snapshot(&report)
    }

    fn dispatch(&mut self, command: Command) -> Result<(), RuntimeError> {
        match command {
            Command::Play => return self.play(),
            Command::Pause | Command::Stop | Command::Panic => self.start = None,
            _ => {}
        }
        let engine_command = match command {
            Command::Play => live::Command::Play,
            Command::Pause => live::Command::Pause,
            Command::Stop => live::Command::Stop,
            Command::BackToStart => live::Command::BackToStart,
            Command::QueueJump { position } => live::Command::QueueJump { position },
            Command::CancelJump => live::Command::CancelJump,
            Command::SetJumpTiming(timing) => live::Command::SetJumpTiming(timing),
            Command::LiveLoop => live::Command::LiveLoop,
            Command::SetInfinite(on) => live::Command::SetInfinite(on),
            Command::ToggleFade => live::Command::ToggleFade,
            Command::SetFadeDuration(duration) => live::Command::SetFadeDuration {
                frames: self.fade_frames(duration),
            },
            Command::Panic => live::Command::Panic,
        };
        self.forward(engine_command)?;
        if let Command::SetFadeDuration(duration) = command {
            let frames = self.fade_frames(duration).max(DECLICK_FRAMES);
            self.fade_duration =
                Duration::from_secs_f64(frames as f64 / f64::from(self.song.sample_rate));
        }
        Ok(())
    }

    fn fade_frames(&self, duration: Duration) -> u64 {
        (duration.as_secs_f64() * f64::from(self.song.sample_rate)).round() as u64
    }

    /// Play from stopped or paused waits until the sources have the playhead
    /// ready, so the first buffer is never a silent underrun (advisory A4).
    fn play(&mut self) -> Result<(), RuntimeError> {
        match &self.link {
            Link::Held { device, reason, .. } => {
                return Err(RuntimeError::DeviceDisconnected {
                    device: device.clone(),
                    reason: reason.clone(),
                });
            }
            Link::Lost => return Err(RuntimeError::PlayerLost),
            Link::Streaming { .. } => {}
        }
        let report = *self.reports.read();
        let silent = matches!(
            report.status.transport,
            Transport::Stopped | Transport::Paused
        );
        if silent && !report.ready {
            if !matches!(self.start, Some(Start::Waiting { .. })) {
                self.start = Some(Start::Waiting {
                    deadline: Instant::now() + self.start_timeout,
                });
            }
            return Ok(());
        }
        self.forward(live::Command::Play)?;
        self.start = silent.then_some(Start::Sent);
        Ok(())
    }

    fn advance_start(&mut self, report: &Report, now: Instant) {
        let Some(start) = self.start else {
            return;
        };
        if report.status.transport.is_running() {
            self.start = None;
            return;
        }
        if let Start::Waiting { deadline } = start {
            if report.ready {
                self.start = Some(Start::Sent);
                if let Err(error) = self.forward(live::Command::Play) {
                    self.start = None;
                    self.last_error = Some(error);
                }
            } else if now >= deadline {
                self.start = None;
                self.last_error = Some(RuntimeError::StartTimeout {
                    waited: self.start_timeout,
                });
            }
        }
    }

    /// Sends `command` to the player. While the device is lost the control
    /// thread holds the player, so it applies the command at once.
    fn forward(&mut self, command: live::Command) -> Result<(), RuntimeError> {
        self.controller.send(command).map_err(|error| match error {
            SendError::Full(command) => RuntimeError::QueueFull {
                command: format!("{command:?}"),
            },
            SendError::UnknownPosition {
                position,
                positions,
            } => RuntimeError::UnknownPosition {
                position,
                positions,
            },
        })?;
        if let Link::Held { state, .. } = &mut self.link {
            state.player.process(&mut []);
            state.publish();
        }
        Ok(())
    }

    fn watch_device(&mut self, now: Instant) {
        let (lost, retry) = match &self.link {
            Link::Streaming { stream, .. } => match stream.health() {
                StreamHealth::Lost { reason } => (Some(reason), false),
                StreamHealth::Running => (None, false),
            },
            Link::Held { retry_at, .. } => (None, now >= *retry_at),
            Link::Lost => (None, false),
        };
        if let Some(reason) = lost {
            self.device_lost(reason, now);
        } else if retry {
            self.reconnect(now);
        }
    }

    /// Takes the player back from the dead stream, pauses it at once, and
    /// holds it until the device is back (F8 acceptance).
    fn device_lost(&mut self, reason: String, now: Instant) {
        let Link::Streaming { stream, home } = mem::replace(&mut self.link, Link::Lost) else {
            return;
        };
        let device = stream.info().device.clone();
        drop(stream);
        self.start = None;
        let Some(mut state) = take_back(home) else {
            self.last_error = Some(RuntimeError::PlayerLost);
            return;
        };
        self.pause_now(&mut state);
        self.last_error = Some(RuntimeError::DeviceDisconnected {
            device: device.clone(),
            reason: reason.clone(),
        });
        self.link = Link::Held {
            state,
            device,
            reason,
            retry_at: now + self.reconnect_interval,
        };
    }

    /// Pauses the held player on the control thread: no device is calling
    /// back, so the pause ramp renders into a buffer nobody hears. A stop or
    /// a panic under way finishes the same way.
    fn pause_now(&mut self, state: &mut AudioState) {
        state.player.process(&mut []);
        let _ = self.controller.send(live::Command::Pause);
        let max_frames = self.request.buffer_frames as usize;
        let mut scratch = vec![0.0f32; 2 * max_frames];
        let longest =
            u64::from(self.song.sample_rate) * PANIC_MILLISECONDS / 1_000 + DECLICK_FRAMES;
        for _ in 0..=longest.div_ceil(max_frames as u64) {
            state.player.process(&mut scratch);
            if !state.player.status().transport.is_running() {
                break;
            }
        }
        state.publish();
    }

    /// Opens the lost device again; the transport stays paused.
    fn reconnect(&mut self, now: Instant) {
        let Link::Held {
            state,
            device,
            reason,
            ..
        } = mem::replace(&mut self.link, Link::Lost)
        else {
            return;
        };
        let (renderer, home) = AudioRenderer::new(state);
        match self.output.open(&self.request, renderer) {
            Ok(stream) => {
                self.link = Link::Streaming { stream, home };
                if matches!(
                    self.last_error,
                    Some(RuntimeError::DeviceDisconnected { .. })
                ) {
                    self.last_error = None;
                }
            }
            Err(_) => match take_back(home) {
                Some(state) => {
                    self.link = Link::Held {
                        state,
                        device,
                        reason,
                        retry_at: now + self.reconnect_interval,
                    };
                }
                None => self.last_error = Some(RuntimeError::PlayerLost),
            },
        }
    }

    /// Records new decode failures and new deferrals of the queued jump.
    fn watch_disk(&mut self, report: &Report) {
        let errors = self.monitor.decode_errors();
        if errors > self.decode_errors_seen {
            self.decode_errors_seen = errors;
            self.last_error = Some(RuntimeError::DecodeFailed { blocks: errors });
        }
        match report.status.queued {
            Some(queued) if queued.deferrals > 0 => {
                let seen = (queued.target, queued.deferrals);
                if self.deferral_seen != Some(seen) {
                    self.deferral_seen = Some(seen);
                    self.last_error = Some(RuntimeError::JumpWaiting {
                        section: self.song.label(queued.target).to_owned(),
                        deferrals: queued.deferrals,
                    });
                }
            }
            _ => self.deferral_seen = None,
        }
    }

    fn snapshot(&self, report: &Report) -> Snapshot {
        let status = &report.status;
        let transport = match status.transport {
            Transport::Stopped | Transport::Paused if self.start.is_some() => {
                TransportState::Starting
            }
            Transport::Stopped => TransportState::Stopped,
            Transport::Playing => TransportState::Playing,
            Transport::Pausing => TransportState::Pausing,
            Transport::Paused => TransportState::Paused,
            Transport::Stopping => TransportState::Stopping,
            Transport::Panicking => TransportState::Panicking,
        };
        let device = match &self.link {
            Link::Streaming { stream, .. } => DeviceStatus::Connected(stream.info().clone()),
            Link::Held { device, reason, .. } => DeviceStatus::Disconnected {
                device: device.clone(),
                reason: reason.clone(),
            },
            Link::Lost => DeviceStatus::Disconnected {
                device: self.request.device.clone().unwrap_or_default(),
                reason: RuntimeError::PlayerLost.to_string(),
            },
        };
        Snapshot {
            transport,
            output_frame: status.output_frame,
            source_frame: status.source_frame,
            bar: status.bar_beat.bar,
            beat: status.bar_beat.beat,
            position: status.position,
            section: self.song.label(status.position).to_owned(),
            queued: status.queued.map(|queued| QueuedView {
                target: queued.target,
                target_label: self.song.label(queued.target).to_owned(),
                kind: queued.kind,
                executes_at: queued.executes_at,
                frames_until: queued.executes_at.saturating_sub(status.output_frame),
                deferrals: queued.deferrals,
            }),
            jump_timing: status.jump_timing,
            infinite: status.infinite,
            fade: FadeView {
                state: status.fade.state,
                level: status.fade.level,
                duration: self.fade_duration,
            },
            device,
            underruns: status.underruns,
            decode_errors: self.monitor.decode_errors(),
            last_error: self.last_error.clone(),
        }
    }
}

impl Drop for Session {
    /// Stops the stream, takes the player back, and drops it here, on the
    /// control thread, which also waits for the disk thread to end.
    fn drop(&mut self) {
        match mem::replace(&mut self.link, Link::Lost) {
            Link::Streaming { stream, home } => {
                drop(stream);
                drop(take_back(home));
            }
            Link::Held { state, .. } => drop(state),
            Link::Lost => {}
        }
    }
}

/// Waits for a dropped renderer to hand its state back. When the output
/// keeps the renderer past [`RELEASE_TIMEOUT`], the queue is leaked so the
/// state can never be freed on the audio thread.
fn take_back(mut home: Consumer<Box<AudioState>>) -> Option<Box<AudioState>> {
    let deadline = Instant::now() + RELEASE_TIMEOUT;
    loop {
        if let Ok(state) = home.pop() {
            return Some(state);
        }
        if Instant::now() >= deadline {
            mem::forget(home);
            return None;
        }
        thread::sleep(Duration::from_millis(1));
    }
}
