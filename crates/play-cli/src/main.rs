//! `play-cli`: a headless live player for development and hardware spikes.
//!
//! Plays one song package on an audio device through the `app` API, with
//! single-key controls and a one-line status refreshed about 10 times a
//! second. Exit codes: 0 when quit, 1 on load, device or terminal errors
//! (each on its own stderr line, prefixed `error:`), 2 on usage errors.

#![forbid(unsafe_code)]

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use app::{
    AudioOutput, Command, DEFAULT_BUFFER_FRAMES, DeviceStatus, LoadError, MAX_BUFFER_FRAMES,
    MIN_BUFFER_FRAMES, OpenError, Session, SessionOptions, TransportState,
};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal;
use io_cpal::CpalOutput;
use play_cli::{Action, CONTROLS, action_for_key, device_list, status_line};

/// How often the status line is redrawn.
const STATUS_EVERY: Duration = Duration::from_millis(100);
/// How long one turn of the loop waits for a key.
const KEY_WAIT: Duration = Duration::from_millis(10);
/// How long quitting waits for the stop ramp before closing the device.
const STOP_WAIT: Duration = Duration::from_millis(500);

/// Play a song package live on an audio device, with single-key controls.
#[derive(Debug, Parser)]
#[command(
    name = "play-cli",
    version,
    override_usage = "play-cli <SONG_DIR> [--device <NAME>] [--buffer <FRAMES>] [--list-devices]",
    after_help = CONTROLS
)]
struct Args {
    /// Song folder: holds song.json and the stem files it lists.
    #[arg(value_name = "SONG_DIR", required_unless_present = "list_devices")]
    song_dir: Option<PathBuf>,

    /// Output device, by the name --list-devices prints. Default: the system
    /// default output.
    #[arg(long, value_name = "NAME")]
    device: Option<String>,

    /// Frames per audio buffer, 64 to 1024; the status line shows the
    /// resulting latency.
    #[arg(
        long,
        value_name = "FRAMES",
        default_value_t = DEFAULT_BUFFER_FRAMES,
        value_parser = clap::value_parser!(u32)
            .range(i64::from(MIN_BUFFER_FRAMES)..=i64::from(MAX_BUFFER_FRAMES))
    )]
    buffer: u32,

    /// List the output devices with their output channel counts (the default
    /// marked with *), then exit.
    #[arg(long)]
    list_devices: bool,
}

fn main() -> ExitCode {
    // Usage errors print clap's message and exit with code 2.
    let args = Args::parse();
    let result = if args.list_devices {
        list_devices()
    } else {
        play(&args)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            let mut stderr = io::stderr().lock();
            for error in errors {
                let _ = writeln!(stderr, "error: {}", escape(&error));
            }
            ExitCode::from(1)
        }
    }
}

fn list_devices() -> Result<(), Vec<String>> {
    let devices = CpalOutput::new()
        .devices()
        .map_err(|error| vec![error.to_string()])?;
    // A closed stdout does not make the listing a failure.
    let _ = io::stdout()
        .lock()
        .write_all(device_list(&devices).as_bytes());
    Ok(())
}

/// Opens the song and the device, then runs the controls until quit.
fn play(args: &Args) -> Result<(), Vec<String>> {
    let song_dir = args
        .song_dir
        .as_deref()
        .expect("clap requires SONG_DIR without --list-devices");
    let options = SessionOptions {
        device: args.device.clone(),
        buffer_frames: args.buffer,
        ..SessionOptions::default()
    };
    let mut session =
        Session::open(song_dir, Box::new(CpalOutput::new()), options).map_err(open_errors)?;
    if !io::stdin().is_terminal() {
        return Err(vec![
            "play-cli reads single keys, so its standard input must be a terminal".to_owned(),
        ]);
    }
    {
        let mut stderr = io::stderr().lock();
        for warning in &session.song().warnings {
            let _ = writeln!(stderr, "warning: {}", escape(warning));
        }
    }
    print_header(&mut session);

    let raw = RawMode::enable().map_err(|error| {
        vec![format!(
            "cannot read single keys from the terminal: {error}"
        )]
    })?;
    let result = run(&mut session);
    stop(&mut session);
    drop(raw);
    println!();
    result
}

fn print_header(session: &mut Session) {
    let song = session.song();
    let mut line = format!(
        "\"{}\" (arrangement \"{}\", {} positions)",
        song.title,
        song.arrangement,
        song.positions.len()
    );
    if let DeviceStatus::Connected(info) = session.poll().device {
        line += &format!(
            " on \"{}\": {} Hz, {} frames per buffer, {:.1} ms latency",
            info.device,
            info.sample_rate,
            info.buffer_frames,
            info.latency().as_secs_f64() * 1_000.0
        );
    }
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "{}", escape(&line));
    let _ = writeln!(
        stdout,
        "space: play/pause, 1-9: queue a section, q: quit; play-cli --help lists every key"
    );
}

/// Reads keys and redraws the status line until a quit key.
fn run(session: &mut Session) -> Result<(), Vec<String>> {
    let terminal_error = |error: io::Error| vec![format!("terminal: {error}")];
    let mut stdout = io::stdout();
    let mut next_status = Instant::now();
    loop {
        let snapshot = session.poll();
        if event::poll(KEY_WAIT).map_err(terminal_error)?
            && let Event::Key(key) = event::read().map_err(terminal_error)?
            && key.kind == KeyEventKind::Press
        {
            let action = match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(Action::Quit)
                }
                KeyCode::Char(key) => action_for_key(key, &snapshot),
                _ => None,
            };
            match action {
                Some(Action::Quit) => return Ok(()),
                // A refused command shows as the last error in the status.
                Some(Action::Send(command)) => drop(session.send(command)),
                None => {}
            }
        }
        let now = Instant::now();
        if now >= next_status {
            draw(&mut stdout, &status_line(&snapshot, session.song())).map_err(terminal_error)?;
            next_status = (next_status + STATUS_EVERY).max(now);
        }
    }
}

/// Rewrites the status line in place, cut to the terminal width.
fn draw(stdout: &mut io::Stdout, line: &str) -> io::Result<()> {
    let width = terminal::size().map_or(120, |(columns, _)| usize::from(columns));
    let line: String = escape(line).chars().take(width.saturating_sub(1)).collect();
    write!(stdout, "\r{line}")?;
    crossterm::execute!(stdout, terminal::Clear(terminal::ClearType::UntilNewLine))
}

/// Stops with the usual ramp before the device closes, so quitting does not
/// cut the audio mid-waveform.
fn stop(session: &mut Session) {
    if session.send(Command::Stop).is_err() {
        return;
    }
    let deadline = Instant::now() + STOP_WAIT;
    while session.poll().transport != TransportState::Stopped && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
}

fn open_errors(error: OpenError) -> Vec<String> {
    match error {
        OpenError::Load(LoadError::Invalid(problems)) => {
            problems.iter().map(ToString::to_string).collect()
        }
        other => vec![other.to_string()],
    }
}

/// `text` with control characters, newlines included, escaped.
fn escape(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Raw terminal mode for single keys, restored on drop, panics included.
struct RawMode;

impl RawMode {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}
