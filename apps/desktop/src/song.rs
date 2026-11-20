//! Opening and closing a song while the show runs (S8): a pick is checked
//! before the current song stops, so a pick that cannot open leaves the
//! current song loaded and playing; a pick that can open stops the current
//! song with its ramp and frees its device before the new song opens it;
//! and closing always ramps down, sending a refused Stop again, and says
//! what went wrong.

use std::path::{Path, PathBuf};
use std::time::Duration;

use app::{
    AudioOutput, Command, OpenError, PreparedSong, RuntimeError, Session, SessionOptions, Snapshot,
    TransportState,
};

/// How long closing waits for the player to take Stop and end its ramp
/// before the device closes.
pub const STOP_WAIT: Duration = Duration::from_millis(500);

/// How often closing sends a refused Stop again and polls for the end of
/// the ramp.
pub const STOP_POLL: Duration = Duration::from_millis(5);

/// A song loaded on a device: the folder it was opened from, so it can be
/// opened again, its session, and the last snapshot [`Self::poll`] took.
pub struct Song {
    /// The song folder.
    pub dir: PathBuf,
    /// The session playing it.
    pub session: Session,
    /// The session's state at the last poll.
    pub snapshot: Snapshot,
}

impl Song {
    /// Opens the song folder `dir` on `output`, stopped at the start.
    pub fn open(
        dir: &Path,
        options: &SessionOptions,
        output: Box<dyn AudioOutput>,
    ) -> Result<Self, OpenError> {
        Self::start(dir, Session::prepare(dir, options.clone())?, output)
    }

    fn start(
        dir: &Path,
        prepared: PreparedSong,
        output: Box<dyn AudioOutput>,
    ) -> Result<Self, OpenError> {
        let mut session = prepared.start(output)?;
        let snapshot = session.poll();
        Ok(Self {
            dir: dir.to_owned(),
            session,
            snapshot,
        })
    }

    /// Moves the session on and keeps its snapshot.
    pub fn poll(&mut self) {
        self.snapshot = self.session.poll();
    }
}

/// A song that closed, and how.
#[derive(Debug)]
pub struct Closed {
    /// Its title.
    pub title: String,
    /// Its folder, to open it again.
    pub dir: PathBuf,
    /// How its stop went.
    pub closing: Closing,
}

impl Closed {
    /// What went wrong, in one line that names the song; `None` when it
    /// stopped with its ramp at the first Stop.
    pub fn problem(&self) -> Option<String> {
        let problem = self.closing.problem()?;
        Some(format!("Closing \"{}\": {problem}", self.title))
    }
}

/// Stops `song` with its ramp ([`stop_and_wait`]), then closes its session,
/// which closes its device.
pub fn close(mut song: Song, sleep: impl FnMut(Duration)) -> Closed {
    let closing = stop_and_wait(&mut song.session, sleep);
    let closed = Closed {
        title: song.session.song().title.clone(),
        dir: song.dir.clone(),
        closing,
    };
    drop(song);
    closed
}

/// What [`replace`] did once the picked song had prepared.
#[derive(Debug)]
pub struct Replaced {
    /// The folder picked.
    pub dir: PathBuf,
    /// The song that was loaded before, and how it closed.
    pub previous: Option<Closed>,
    /// Why the picked song's device did not open, if it did not. The
    /// current song is then the previous one, opened again and stopped at
    /// the start, or none.
    pub start_error: Option<OpenError>,
    /// Why the previous song could not be opened again after that.
    pub reopen_error: Option<OpenError>,
}

impl Replaced {
    /// What to show and report, one line each; empty when the previous song
    /// stopped with its ramp and the picked one opened.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self.previous.iter().filter_map(Closed::problem).collect();
        if let Some(error) = &self.start_error {
            problems.push(format!(
                "Could not start {} on the audio device: {error}",
                self.dir.display()
            ));
            if let Some(previous) = &self.previous {
                problems.push(match &self.reopen_error {
                    None => format!(
                        "\"{}\" is loaded again, stopped at the start.",
                        previous.title
                    ),
                    Some(error) => {
                        format!("\"{}\" could not be loaded again: {error}", previous.title)
                    }
                });
            }
        }
        problems
    }
}

/// Opens the song folder `dir` in place of `current`, never holding the
/// device twice:
///
/// 1. The picked song is prepared with no device ([`Session::prepare`]).
///    When it cannot open, the error comes back and `current` plays on,
///    untouched.
/// 2. The current song stops with its ramp ([`stop_and_wait`], `sleep`
///    between tries) and closes, which closes its device.
/// 3. The picked song starts on a fresh output from `output`. When the
///    device refuses it, the previous song opens again from its folder,
///    stopped at the start, if it can; [`Replaced`] says what happened.
///
/// The device opens only once the old song has closed because on macOS
/// opening a stream sets the device's sample rate, which would re-clock a
/// song still playing on it (it would drop out with no ramp), and some
/// devices take one stream at a time.
pub fn replace(
    current: &mut Option<Song>,
    dir: &Path,
    options: &SessionOptions,
    mut output: impl FnMut() -> Box<dyn AudioOutput>,
    sleep: impl FnMut(Duration),
) -> Result<Replaced, OpenError> {
    let prepared = Session::prepare(dir, options.clone())?;
    let previous = current.take().map(|song| close(song, sleep));
    let mut replaced = Replaced {
        dir: dir.to_owned(),
        previous,
        start_error: None,
        reopen_error: None,
    };
    match Song::start(dir, prepared, output()) {
        Ok(song) => *current = Some(song),
        Err(error) => {
            replaced.start_error = Some(error);
            if let Some(previous) = &replaced.previous {
                match Song::open(&previous.dir, options, output()) {
                    Ok(song) => *current = Some(song),
                    Err(error) => replaced.reopen_error = Some(error),
                }
            }
        }
    }
    Ok(replaced)
}

/// How closing a song went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Closing {
    /// The transport stopped with its ramp. `refused` holds the last
    /// reason the player refused Stop before it took it, if it did.
    Stopped {
        /// The last refusal before the player took Stop.
        refused: Option<RuntimeError>,
    },
    /// The player took Stop, but the ramp had not ended after
    /// [`STOP_WAIT`].
    RampUnfinished {
        /// The last refusal before the player took Stop.
        refused: Option<RuntimeError>,
    },
    /// The player refused Stop for the whole of [`STOP_WAIT`], so the song
    /// closes without its ramp.
    Refused(RuntimeError),
}

impl Closing {
    /// What went wrong, in one line; `None` when the player took the first
    /// Stop and the ramp ended.
    pub fn problem(&self) -> Option<String> {
        let wait = STOP_WAIT.as_millis();
        match self {
            Self::Stopped { refused: None } => None,
            Self::Stopped {
                refused: Some(reason),
            } => Some(format!(
                "the player refused Stop at first ({reason}); it took it on a later try and stopped with its ramp"
            )),
            Self::RampUnfinished { refused } => {
                let mut problem = format!(
                    "the stop ramp had not ended after {wait} ms; the song closed before it ended"
                );
                if let Some(reason) = refused {
                    problem.push_str(&format!(" (Stop was refused at first: {reason})"));
                }
                Some(problem)
            }
            Self::Refused(reason) => Some(format!(
                "the player refused Stop for {wait} ms ({reason}); the song closed without its ramp"
            )),
        }
    }
}

/// Sends Stop and waits for the ramp, at most [`STOP_WAIT`]. A refused
/// Stop (the command queue is full because the audio thread has not taken
/// the last commands yet) is sent again every [`STOP_POLL`] until the
/// player takes it; once taken, the session is polled every [`STOP_POLL`]
/// until the transport is stopped. `sleep` waits between tries
/// (`std::thread::sleep` in the app). The caller then drops the session,
/// which closes the device, and reports [`Closing::problem`].
pub fn stop_and_wait(session: &mut Session, mut sleep: impl FnMut(Duration)) -> Closing {
    let tries = STOP_WAIT.as_millis() / STOP_POLL.as_millis();
    let mut refused = None;
    let mut taken = false;
    for attempt in 0..=tries {
        if !taken {
            match session.send(Command::Stop) {
                Ok(()) => taken = true,
                Err(reason) => refused = Some(reason),
            }
        }
        if taken && session.poll().transport == TransportState::Stopped {
            return Closing::Stopped { refused };
        }
        if attempt < tries {
            sleep(STOP_POLL);
        }
    }
    match refused {
        Some(reason) if !taken => Closing::Refused(reason),
        refused => Closing::RampUnfinished { refused },
    }
}
