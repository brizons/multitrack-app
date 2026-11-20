//! What the Perform screen plays: one song folder, or a setlist from a
//! library folder (S4). Either one opens in place of the other with the
//! same care as a song in place of a song ([`crate::song::replace`]): the
//! pick and the device are checked before what plays stops, and the device
//! opens only once the old show has closed it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use app::{
    AudioOutput, OpenError, OutputCheck, PreparedSetlist, PreparedSong, Session, SessionOptions,
    SetlistCommand, SetlistCommandError, SetlistOpenError, SetlistSession, SetlistSnapshot,
    Snapshot, SongInfo,
};
use library::{Library, SETLIST_EXTENSION, SETLISTS_FOLDER};

use crate::action::{UiAction, command_for, setlist_command_for};
use crate::catalog::Catalog;
use crate::song::{Closed, Song, stop_and_wait};
use crate::strip::SetlistView;
use crate::view::open_error_lines;

/// What the screen can open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A song folder.
    Song(PathBuf),
    /// A setlist file of a library folder.
    Setlist {
        /// The library folder: the one holding the [`SETLISTS_FOLDER`].
        library: PathBuf,
        /// The file name inside the [`SETLISTS_FOLDER`], as
        /// [`Library::load_setlist`] takes it.
        file: String,
    },
}

impl Source {
    /// The setlist file at `path`: its library is the folder holding the
    /// [`SETLISTS_FOLDER`] it is in (the folder's name may be in any case).
    /// `None` when `path` is not inside such a folder.
    pub fn setlist_file(path: &Path) -> Option<Self> {
        let folder = path.parent()?;
        if !folder.file_name().is_some_and(is_setlists_folder) {
            return None;
        }
        Some(Self::Setlist {
            library: folder.parent()?.to_owned(),
            file: path.file_name()?.to_str()?.to_owned(),
        })
    }

    /// What `path` opens: a file whose name ends in
    /// [`library::SETLIST_EXTENSION`] is a setlist ([`Self::setlist_file`]),
    /// anything else a song folder. Why not, for a setlist file outside a
    /// [`SETLISTS_FOLDER`].
    pub fn for_path(path: &Path) -> Result<Self, String> {
        let is_setlist = path
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| name.ends_with(SETLIST_EXTENSION));
        if !is_setlist {
            return Ok(Self::Song(path.to_owned()));
        }
        Self::setlist_file(path).ok_or_else(|| {
            format!(
                "{} is not in the {SETLISTS_FOLDER} folder of a library folder, so its songs cannot be found",
                path.display()
            )
        })
    }

    /// The setlist file picked at `path`; why not, for any other file.
    pub fn picked_setlist(path: &Path) -> Result<Self, String> {
        match Self::for_path(path)? {
            Self::Song(_) => Err(format!(
                "{} is not a setlist: pick a file ending in {SETLIST_EXTENSION} in the {SETLISTS_FOLDER} folder of a library folder",
                path.display()
            )),
            setlist => Ok(setlist),
        }
    }

    /// The folder or file, for messages.
    pub fn describe(&self) -> String {
        match self {
            Self::Song(dir) => dir.display().to_string(),
            Self::Setlist { library, file } => library
                .join(SETLISTS_FOLDER)
                .join(file)
                .display()
                .to_string(),
        }
    }

    /// Checks and prepares the show with no device ([`Session::prepare`],
    /// [`SetlistSession::prepare`]); a setlist also lists the library's
    /// songs to name its items.
    fn prepare(&self, options: &SessionOptions) -> Result<Prepared, OpenFailure> {
        match self {
            Self::Song(dir) => Session::prepare(dir, options.clone())
                .map(|prepared| Prepared::Song(dir.clone(), Box::new(prepared)))
                .map_err(OpenFailure::Song),
            Self::Setlist { library, file } => {
                let catalog = Library::new(library)
                    .list_songs()
                    .map(|listing| Catalog::from_listing(&listing))
                    .unwrap_or_default();
                let prepared =
                    SetlistSession::prepare(&Library::new(library), file, options.clone())
                        .map_err(OpenFailure::Setlist)?;
                Ok(Prepared::Setlist {
                    library: library.clone(),
                    file: file.clone(),
                    prepared: Box::new(prepared),
                    catalog,
                })
            }
        }
    }
}

/// Why a show did not open.
#[derive(Debug)]
pub enum OpenFailure {
    /// The song folder or its device.
    Song(OpenError),
    /// The setlist, its first item or its device.
    Setlist(SetlistOpenError),
}

impl OpenFailure {
    /// Why, one line per problem, as the `app` API words them: every
    /// problem of an invalid song package, and every reason each item of a
    /// setlist that cannot play at all cannot play.
    pub fn lines(&self) -> Vec<String> {
        match self {
            Self::Song(error) | Self::Setlist(SetlistOpenError::Open(error)) => {
                open_error_lines(error)
            }
            Self::Setlist(error @ SetlistOpenError::NothingPlayable { problems, .. }) => {
                let mut lines = vec![error.to_string()];
                for (index, reasons) in problems.iter().enumerate() {
                    for reason in reasons {
                        lines.push(format!("item {} cannot play: {reason}", index + 1));
                    }
                }
                lines
            }
            Self::Setlist(error) => vec![error.to_string()],
        }
    }
}

/// A show checked with no device.
enum Prepared {
    Song(PathBuf, Box<PreparedSong>),
    Setlist {
        library: PathBuf,
        file: String,
        prepared: Box<PreparedSetlist>,
        catalog: Catalog,
    },
}

impl Prepared {
    fn check(&self, output: &dyn AudioOutput) -> OutputCheck {
        match self {
            Self::Song(_, prepared) => prepared.check(output),
            Self::Setlist { prepared, .. } => prepared.check(output),
        }
    }

    fn start(self, output: Box<dyn AudioOutput>) -> Result<Show, OpenError> {
        match self {
            Self::Song(dir, prepared) => {
                Song::start(&dir, *prepared, output).map(|song| Show::Song(Box::new(song)))
            }
            Self::Setlist {
                library,
                file,
                prepared,
                catalog,
            } => {
                let mut session = prepared.start(output)?;
                let snapshot = session.poll();
                Ok(Show::Setlist(Box::new(LoadedSetlist {
                    library,
                    file,
                    session,
                    snapshot,
                    catalog,
                    refused: None,
                })))
            }
        }
    }
}

/// A setlist loaded on a device: where it came from, its session, the last
/// snapshot, the library's songs that name its items, and the last
/// setlist control it refused.
pub struct LoadedSetlist {
    /// The library folder.
    pub library: PathBuf,
    /// The setlist file inside its [`SETLISTS_FOLDER`].
    pub file: String,
    /// The session playing it.
    pub session: SetlistSession,
    /// The session's state at the last poll.
    pub snapshot: SetlistSnapshot,
    /// The library's songs, to name the items.
    pub catalog: Catalog,
    /// The last previous, next or select the setlist refused, until one is
    /// accepted.
    pub refused: Option<SetlistCommandError>,
}

impl LoadedSetlist {
    /// Opens the setlist `file` of the library folder `library` on
    /// `output`, at its first playable item, stopped at its start.
    pub fn open(
        library: &Path,
        file: &str,
        options: &SessionOptions,
        output: Box<dyn AudioOutput>,
    ) -> Result<Self, OpenFailure> {
        let source = Source::Setlist {
            library: library.to_owned(),
            file: file.to_owned(),
        };
        match source.prepare(options)?.start(output) {
            Ok(Show::Setlist(setlist)) => Ok(*setlist),
            Ok(Show::Song(_)) => unreachable!("a setlist source prepares a setlist"),
            Err(error) => Err(OpenFailure::Setlist(SetlistOpenError::Open(error))),
        }
    }

    /// Moves the session on and keeps its snapshot.
    pub fn poll(&mut self) {
        self.snapshot = self.session.poll();
    }

    /// Sends the command `action` stands for. A refused item command shows
    /// as the snapshot's last error; a refused previous, next or select is
    /// kept in [`Self::refused`] until one is accepted.
    pub fn send(&mut self, action: UiAction) {
        let Some(command) = setlist_command_for(action, &self.snapshot, self.session.song()) else {
            return;
        };
        match self.session.send(command) {
            Ok(()) => {
                if !matches!(command, SetlistCommand::Item(_)) {
                    self.refused = None;
                }
            }
            // The snapshot's last error says why.
            Err(SetlistCommandError::Runtime(_)) => {}
            Err(error) => self.refused = Some(error),
        }
    }

    /// What the screen shows of the setlist.
    pub fn view(&self) -> SetlistView {
        SetlistView::new(
            self.session.info(),
            &self.snapshot,
            &self.catalog,
            self.refused.as_ref(),
        )
    }
}

/// What plays: a song or a setlist.
pub enum Show {
    /// One song folder.
    Song(Box<Song>),
    /// A setlist.
    Setlist(Box<LoadedSetlist>),
}

impl Show {
    /// The current song's state at the last poll (a setlist's current
    /// item).
    pub fn snapshot(&self) -> &Snapshot {
        match self {
            Self::Song(song) => &song.snapshot,
            Self::Setlist(setlist) => &setlist.snapshot.song,
        }
    }

    /// The current song (a setlist's current item).
    pub fn song(&self) -> &SongInfo {
        match self {
            Self::Song(song) => song.session.song(),
            Self::Setlist(setlist) => setlist.session.song(),
        }
    }

    /// The setlist, when one plays.
    pub fn setlist(&self) -> Option<&LoadedSetlist> {
        match self {
            Self::Song(_) => None,
            Self::Setlist(setlist) => Some(setlist),
        }
    }

    /// Moves the session on and keeps its snapshot.
    pub fn poll(&mut self) {
        match self {
            Self::Song(song) => song.poll(),
            Self::Setlist(setlist) => setlist.poll(),
        }
    }

    /// Sends the command `action` stands for; a single song ignores the
    /// setlist controls, and a refused command shows as the snapshot's last
    /// error (or, for a setlist control, in the setlist's problems).
    pub fn send(&mut self, action: UiAction) {
        match self {
            Self::Song(song) => {
                if let Some(command) = command_for(action, &song.snapshot, song.session.song()) {
                    let _ = song.session.send(command);
                }
            }
            Self::Setlist(setlist) => setlist.send(action),
        }
    }

    /// Where it was opened from, to open it again.
    pub fn source(&self) -> Source {
        match self {
            Self::Song(song) => Source::Song(song.dir.clone()),
            Self::Setlist(setlist) => Source::Setlist {
                library: setlist.library.clone(),
                file: setlist.file.clone(),
            },
        }
    }
}

/// Stops `show` with its ramp ([`stop_and_wait`]), then closes its session,
/// which closes its device. A setlist reads as its name.
pub fn close(show: Show, sleep: impl FnMut(Duration)) -> Closed {
    match show {
        Show::Song(song) => crate::song::close(*song, sleep),
        Show::Setlist(mut setlist) => {
            let closing = stop_and_wait(&mut setlist.session, sleep);
            let closed = Closed {
                title: setlist.session.info().name.clone(),
                dir: setlist.library.clone(),
                closing,
            };
            drop(setlist);
            closed
        }
    }
}

/// What [`replace`] did once the pick had prepared.
#[derive(Debug)]
pub struct Replaced {
    /// What was picked.
    pub source: Source,
    /// What was loaded before, how it closed, and where it came from.
    pub previous: Option<(Closed, Source)>,
    /// Why the pick's device did not open although its check went ahead,
    /// if it did not. What plays is then the previous show, opened again
    /// and stopped at the start, or nothing.
    pub start_error: Option<OpenError>,
    /// Why the previous show could not be opened again after that.
    pub reopen_error: Option<OpenFailure>,
}

impl Replaced {
    /// What to show and report, one line each; empty when the previous show
    /// stopped with its ramp and the pick opened.
    pub fn problems(&self) -> Vec<String> {
        let mut problems: Vec<String> = self
            .previous
            .iter()
            .filter_map(|(closed, _)| closed.problem())
            .collect();
        if let Some(error) = &self.start_error {
            problems.push(format!(
                "Could not start {} on the audio device: {error}",
                self.source.describe()
            ));
            if let Some((previous, _)) = &self.previous {
                match &self.reopen_error {
                    None => problems.push(format!(
                        "\"{}\" is loaded again, stopped at the start.",
                        previous.title
                    )),
                    Some(error) => {
                        let mut lines = error.lines().into_iter();
                        let first = lines.next().unwrap_or_default();
                        problems.push(format!(
                            "\"{}\" could not be loaded again: {first}",
                            previous.title
                        ));
                        problems.extend(lines);
                    }
                }
            }
        }
        problems
    }
}

/// Opens `source` in place of `current`, in the order
/// [`crate::song::replace`] follows, for a song or a setlist on either
/// side: the pick is prepared and the device asked first, and a pick that
/// does not open or that the device refuses leaves `current` playing; then
/// `current` stops with its ramp and closes its device; then the pick opens
/// the device, and when that fails the previous show opens again from its
/// source, stopped at the start.
pub fn replace(
    current: &mut Option<Show>,
    source: &Source,
    options: &SessionOptions,
    mut output: impl FnMut() -> Box<dyn AudioOutput>,
    sleep: impl FnMut(Duration),
) -> Result<Replaced, OpenFailure> {
    let prepared = source.prepare(options)?;
    let device = output();
    if let OutputCheck::Refused(error) = prepared.check(device.as_ref()) {
        return Err(failure(source, error.into()));
    }
    let previous = current.take().map(|show| {
        let previous_source = show.source();
        (close(show, sleep), previous_source)
    });
    let mut replaced = Replaced {
        source: source.clone(),
        previous,
        start_error: None,
        reopen_error: None,
    };
    match prepared.start(device) {
        Ok(show) => *current = Some(show),
        Err(error) => {
            replaced.start_error = Some(error);
            if let Some((_, previous)) = &replaced.previous {
                match previous.prepare(options).and_then(|prepared| {
                    prepared
                        .start(output())
                        .map_err(|error| failure(previous, error))
                }) {
                    Ok(show) => *current = Some(show),
                    Err(error) => replaced.reopen_error = Some(error),
                }
            }
        }
    }
    Ok(replaced)
}

/// `error` from opening `source`'s device.
fn failure(source: &Source, error: OpenError) -> OpenFailure {
    match source {
        Source::Song(_) => OpenFailure::Song(error),
        Source::Setlist { .. } => OpenFailure::Setlist(SetlistOpenError::Open(error)),
    }
}

/// Whether `name` is the setlists folder's name, in any case.
fn is_setlists_folder(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.eq_ignore_ascii_case(SETLISTS_FOLDER))
}
