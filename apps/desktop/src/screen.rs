//! The Perform screen's container: owns the session, polls it on a timer,
//! turns pointer and key events into view-model actions, and hands a
//! [`PerformView`] to the presentational code in [`crate::ui`].

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use app::{Session, SessionOptions, Snapshot, SongInfo};
use gpui::{
    App, AppContext, Context, FocusHandle, KeyDownEvent, KeyUpEvent, PathPromptOptions, Render,
    Subscription, Task, Window, prelude::*,
};
use io_cpal::CpalOutput;
use multitrack_desktop::action::{UiAction, command_for};
use multitrack_desktop::hold::PanicHold;
use multitrack_desktop::keys::{HeldKeys, KeyAction, KeyPress, Mods, is_panic_key};
use multitrack_desktop::song::{open_in_place, stop_and_wait};
use multitrack_desktop::view::{PerformView, open_error_lines};

use crate::ui;

/// How often the session is polled: about 30 Hz (Decision 3). The timer
/// keeps running while the window is hidden, since a waiting play starts
/// only through `Session::poll` (L16).
const POLL_EVERY: Duration = Duration::from_millis(33);

/// What the presentational code asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// A control of the Perform screen.
    Ui(UiAction),
    /// The panic button or key went down.
    PanicDown,
    /// The panic button or key went up.
    PanicUp,
    /// "Open song…".
    OpenSong,
    /// Hides the problems shown above a playing song.
    DismissProblems,
}

/// A song playing on the default output.
struct Loaded {
    session: Session,
    song: SongInfo,
    snapshot: Snapshot,
}

impl Loaded {
    fn new(mut session: Session) -> Self {
        let snapshot = session.poll();
        let song = session.song().clone();
        Self {
            session,
            song,
            snapshot,
        }
    }
}

/// Stops `loaded` with its ramp ([`stop_and_wait`]: a Stop the player
/// refuses is sent again until it takes it, within
/// [`multitrack_desktop::song::STOP_WAIT`]), then closes the session and
/// its device. What went wrong goes to stderr and comes back as a line for
/// the screen.
fn close(mut loaded: Loaded) -> Option<String> {
    let closing = stop_and_wait(&mut loaded.session, thread::sleep);
    let title = loaded.song.title.clone();
    drop(loaded);
    let problem = closing
        .problem()
        .map(|problem| format!("Closing \"{title}\": {problem}"));
    if let Some(problem) = &problem {
        eprintln!("warning: {problem}");
    }
    problem
}

/// The Perform screen.
pub struct Perform {
    focus: FocusHandle,
    loaded: Option<Loaded>,
    /// Problems to show, one line each: why the last pick did not open (the
    /// song that was playing goes on), or how the song it replaced closed.
    problems: Vec<String>,
    hold: PanicHold,
    keys: HeldKeys,
    _poll: Task<()>,
    _quit: Subscription,
    _deactivated: Subscription,
    _blurred: Subscription,
}

impl Perform {
    /// The screen, with the song folder `song_dir` open when one is given.
    pub fn new(song_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window);
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_EVERY).await;
                if this.update(cx, |screen, cx| screen.tick(cx)).is_err() {
                    break;
                }
            }
        });
        let quit = cx.on_app_quit(|screen, _| {
            screen.close_song();
            async {}
        });
        // A release that happens while the window is inactive, or while
        // something else has the focus, never reaches the screen.
        let deactivated = cx.observe_window_activation(window, |screen, window, cx| {
            if !window.is_window_active() {
                screen.focus_lost(cx);
            }
        });
        let blurred = cx.on_blur(&focus, window, |screen, _, cx| screen.focus_lost(cx));
        let mut screen = Self {
            focus,
            loaded: None,
            problems: Vec::new(),
            hold: PanicHold::new(),
            keys: HeldKeys::new(),
            _poll: poll,
            _quit: quit,
            _deactivated: deactivated,
            _blurred: blurred,
        };
        if let Some(dir) = song_dir {
            screen.open_song(&dir);
        }
        screen
    }

    /// Stops the song with its ramp, even when the player refuses Stop at
    /// first, then closes the session and its device (see [`close`]).
    pub fn close_song(&mut self) {
        if let Some(loaded) = self.loaded.take() {
            close(loaded);
        }
    }

    /// Opens the song folder `dir` while the current song keeps playing;
    /// only once it opened does the current song stop and close. A folder
    /// that does not open leaves the current song loaded and playing, and
    /// the reasons show on screen.
    fn open_song(&mut self, dir: &Path) {
        let opened = open_in_place(&mut self.loaded, || {
            Session::open(dir, Box::new(CpalOutput::new()), SessionOptions::default())
                .map(Loaded::new)
        });
        match opened {
            Ok(replaced) => {
                self.hold = PanicHold::new();
                self.problems = replaced.and_then(close).into_iter().collect();
            }
            Err(error) => {
                self.problems = open_error_lines(&error);
                self.problems
                    .insert(0, format!("Could not open {}:", dir.display()));
            }
        }
    }

    /// The window lost focus (Cmd-Tab, a system dialog, the folder
    /// picker): a panic hold ends without panic, and every key counts as
    /// up, since their releases go elsewhere.
    fn focus_lost(&mut self, cx: &mut Context<Self>) {
        self.hold.cancel();
        self.keys.forget_all();
        cx.notify();
    }

    /// One turn of the timer: moves the session on and fires a panic hold
    /// that is complete.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let counting = self.hold.is_counting();
        let fires = self.hold.tick(Instant::now());
        let Some(loaded) = &mut self.loaded else {
            // Nothing moves without a song, except a hold's fill.
            if counting {
                cx.notify();
            }
            return;
        };
        if fires {
            send(loaded, UiAction::Panic);
        }
        loaded.snapshot = loaded.session.poll();
        cx.notify();
    }

    /// Carries out what the presentational code or a key asks for.
    pub fn handle(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        match intent {
            Intent::Ui(action) => {
                if let Some(loaded) = &mut self.loaded {
                    send(loaded, action);
                    loaded.snapshot = loaded.session.poll();
                }
            }
            Intent::PanicDown => self.hold.press(now),
            Intent::PanicUp => {
                if self.hold.release(now)
                    && let Some(loaded) = &mut self.loaded
                {
                    send(loaded, UiAction::Panic);
                }
            }
            Intent::OpenSong => self.prompt_for_song(window, cx),
            Intent::DismissProblems => self.problems.clear(),
        }
        cx.notify();
    }

    fn prompt_for_song(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open song".into()),
        });
        cx.spawn(async move |this, cx| {
            let chosen = match paths.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(error)) => {
                    let message = format!("The folder picker failed: {error}");
                    eprintln!("error: {message}");
                    let _ = this.update(cx, |screen, cx| {
                        screen.problems = vec![message];
                        cx.notify();
                    });
                    None
                }
                _ => None,
            };
            if let Some(dir) = chosen {
                let _ = this.update(cx, |screen, cx| {
                    screen.open_song(&dir);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let press = KeyPress {
            key: &event.keystroke.key,
            mods: Mods {
                control: modifiers.control,
                alt: modifiers.alt,
                platform: modifiers.platform,
                function: modifiers.function,
            },
        };
        // A held key repeats; only its first press acts, so a held space
        // does not toggle play over and over.
        let intent = match self.keys.press(&press, event.is_held) {
            Some(KeyAction::Ui(action)) => Intent::Ui(action),
            Some(KeyAction::PanicDown) => Intent::PanicDown,
            Some(KeyAction::OpenSong) => Intent::OpenSong,
            Some(KeyAction::Quit) => {
                self.close_song();
                cx.quit();
                return;
            }
            None => return,
        };
        cx.stop_propagation();
        self.handle(intent, window, cx);
    }

    fn key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.keys.release(&event.keystroke.key);
        if is_panic_key(&event.keystroke.key) {
            self.handle(Intent::PanicUp, window, cx);
        }
    }
}

/// Sends the command `action` stands for; a refused command shows as the
/// snapshot's last error.
fn send(loaded: &mut Loaded, action: UiAction) {
    if let Some(command) = command_for(action, &loaded.snapshot, &loaded.song) {
        let _ = loaded.session.send(command);
    }
}

impl Render for Perform {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.loaded {
            Some(loaded) => {
                let view = PerformView::new(&loaded.snapshot, &loaded.song);
                ui::perform(&view, self.hold.fill(Instant::now()), &self.problems, cx)
                    .into_any_element()
            }
            None => ui::no_song(&self.problems, cx).into_any_element(),
        };
        ui::root(&self.focus, cx)
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .child(body)
    }
}

/// Opens the Perform window for `song_dir`, if any.
pub fn open_window(song_dir: Option<PathBuf>, cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, gpui::size(gpui::px(1366.), gpui::px(900.)), cx);
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("Multitrack — Perform".into()),
            ..Default::default()
        }),
        window_min_size: Some(gpui::size(gpui::px(960.), gpui::px(640.))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        let screen = cx.new(|cx| Perform::new(song_dir, window, cx));
        // Closing the window stops the song with its ramp first.
        let closing = screen.downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = closing.update(cx, |screen, _| screen.close_song());
            true
        });
        screen
    });
    if let Err(error) = opened {
        eprintln!("error: cannot open the window: {error}");
        cx.quit();
    }
}
