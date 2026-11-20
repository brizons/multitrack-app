//! The Perform screen's container: owns the session, polls it on a timer,
//! turns pointer and key events into view-model actions, and hands a
//! [`PerformView`] to the presentational code in [`crate::ui`].

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use app::{Command, Session, SessionOptions, Snapshot, SongInfo, TransportState};
use gpui::{
    App, AppContext, Context, FocusHandle, KeyDownEvent, KeyUpEvent, PathPromptOptions, Render,
    Subscription, Task, Window, prelude::*,
};
use io_cpal::CpalOutput;
use multitrack_desktop::action::{UiAction, command_for};
use multitrack_desktop::hold::PanicHold;
use multitrack_desktop::keys::{KeyAction, KeyPress, Mods, is_panic_key, key_down};
use multitrack_desktop::view::{PerformView, open_error_lines};

use crate::ui;

/// How often the session is polled: about 30 Hz (Decision 3). The timer
/// keeps running while the window is hidden, since a waiting play starts
/// only through `Session::poll` (L16).
const POLL_EVERY: Duration = Duration::from_millis(33);
/// How long closing waits for the stop ramp before the device closes.
const STOP_WAIT: Duration = Duration::from_millis(500);

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
}

/// A song playing on the default output.
struct Loaded {
    session: Session,
    song: SongInfo,
    snapshot: Snapshot,
}

/// The Perform screen.
pub struct Perform {
    focus: FocusHandle,
    loaded: Option<Loaded>,
    /// Why the last song did not open, one line per problem.
    open_errors: Vec<String>,
    hold: PanicHold,
    _poll: Task<()>,
    _quit: Subscription,
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
        let mut screen = Self {
            focus,
            loaded: None,
            open_errors: Vec::new(),
            hold: PanicHold::new(),
            _poll: poll,
            _quit: quit,
        };
        if let Some(dir) = song_dir {
            screen.open_song(&dir);
        }
        screen
    }

    /// Stops with the usual ramp, then closes the session and its device.
    pub fn close_song(&mut self) {
        let Some(mut loaded) = self.loaded.take() else {
            return;
        };
        if loaded.session.send(Command::Stop).is_ok() {
            let deadline = Instant::now() + STOP_WAIT;
            while loaded.session.poll().transport != TransportState::Stopped
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
        }
        drop(loaded);
    }

    fn open_song(&mut self, dir: &Path) {
        self.close_song();
        self.hold = PanicHold::new();
        match Session::open(dir, Box::new(CpalOutput::new()), SessionOptions::default()) {
            Ok(mut session) => {
                let snapshot = session.poll();
                let song = session.song().clone();
                self.open_errors.clear();
                self.loaded = Some(Loaded {
                    session,
                    song,
                    snapshot,
                });
            }
            Err(error) => {
                self.open_errors = open_error_lines(&error);
                self.open_errors
                    .insert(0, format!("Could not open {}:", dir.display()));
            }
        }
    }

    /// One turn of the timer: moves the session on and fires a panic hold
    /// that is complete.
    fn tick(&mut self, cx: &mut Context<Self>) {
        let held = self.hold.is_held();
        let fires = self.hold.tick(Instant::now());
        let Some(loaded) = &mut self.loaded else {
            // Nothing moves without a song, except a hold's fill.
            if held {
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
                        screen.open_errors = vec![message];
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
        // A held key repeats; only its first press acts, so a held space
        // does not toggle play over and over.
        if event.is_held {
            return;
        }
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
        let intent = match key_down(&press) {
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
                ui::perform(&view, self.hold.progress(Instant::now()), cx).into_any_element()
            }
            None => ui::no_song(&self.open_errors, cx).into_any_element(),
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
