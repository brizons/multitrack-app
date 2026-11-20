//! The Perform screen's container: owns the session, polls it on a timer,
//! turns pointer and key events into view-model actions, and hands a
//! [`PerformView`] to the presentational code in [`crate::ui`].

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use app::SessionOptions;
use gpui::{
    App, AppContext, Context, FocusHandle, KeyDownEvent, KeyUpEvent, PathPromptOptions,
    PromptButton, PromptLevel, Render, Subscription, Task, Window, prelude::*,
};
use io_cpal::CpalOutput;
use multitrack_desktop::action::{Intent, UiAction, command_for};
use multitrack_desktop::ending::{Dialog, EndRequest, Ending, control_acts, end_request};
use multitrack_desktop::hold::PanicHold;
use multitrack_desktop::keys::{HeldKeys, KeyAction, KeyPress, Mods, is_panic_key};
use multitrack_desktop::song::{Song, close, replace};
use multitrack_desktop::view::{PerformView, open_error_lines};

use crate::ui;

/// How often the session is polled: about 30 Hz (Decision 3). The timer
/// keeps running while the window is hidden, since a waiting play starts
/// only through `Session::poll` (L16).
const POLL_EVERY: Duration = Duration::from_millis(33);

/// The Perform screen.
pub struct Perform {
    focus: FocusHandle,
    loaded: Option<Song>,
    /// Problems to show, one line each: why the last pick did not open (the
    /// song that was playing goes on), or how the song it replaced closed.
    problems: Vec<String>,
    hold: PanicHold,
    keys: HeldKeys,
    /// Whether the question before ending the show is on screen.
    asking: bool,
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
            asking: false,
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

    /// Stops the song with its ramp, sending a Stop the player refuses
    /// again within [`multitrack_desktop::song::STOP_WAIT`], then closes
    /// the session and its device; what went wrong goes to stderr.
    pub fn close_song(&mut self) {
        if let Some(song) = self.loaded.take()
            && let Some(problem) = close(song, thread::sleep).problem()
        {
            eprintln!("warning: {problem}");
        }
    }

    /// Quits or closes the window: at once when nothing plays (the song,
    /// if any, stops with its ramp first); while a song plays, only once
    /// the person confirms ([`end_request`]). Returns whether it ended at
    /// once, which lets the window close.
    fn end_show(&mut self, ending: Ending, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let transport = self.loaded.as_mut().map(|song| {
            song.poll();
            song.snapshot.transport
        });
        match end_request(transport, self.asking) {
            EndRequest::EndNow => {
                self.close_song();
                if ending == Ending::Quit {
                    cx.quit();
                }
                true
            }
            EndRequest::Ask => {
                self.ask_before_ending(ending, window, cx);
                false
            }
            EndRequest::Wait => false,
        }
    }

    fn ask_before_ending(&mut self, ending: Ending, window: &mut Window, cx: &mut Context<Self>) {
        let question = ending.question(Dialog::HERE);
        let buttons = question.answers.map(|answer| {
            if answer.keeps_playing {
                PromptButton::cancel(answer.label)
            } else {
                PromptButton::new(answer.label)
            }
        });
        let answer = window.prompt(
            PromptLevel::Warning,
            question.message,
            Some(question.detail),
            &buttons,
            cx,
        );
        self.asking = true;
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await.ok();
            let _ = this.update_in(cx, |screen, window, cx| {
                screen.asking = false;
                if question.ends(answer) {
                    screen.close_song();
                    match ending {
                        Ending::Quit => cx.quit(),
                        Ending::CloseWindow => window.remove_window(),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the song folder `dir` in place of the current song
    /// ([`replace`]): a folder that does not open leaves the current song
    /// loaded and playing, and the reasons show on screen; otherwise the
    /// current song stops with its ramp and closes its device before the
    /// new song opens the device.
    fn open_song(&mut self, dir: &Path) {
        let replaced = replace(
            &mut self.loaded,
            dir,
            &SessionOptions::default(),
            || Box::new(CpalOutput::new()),
            thread::sleep,
        );
        match replaced {
            Ok(replaced) => {
                self.hold = PanicHold::new();
                self.problems = replaced.problems();
                for problem in &self.problems {
                    eprintln!("warning: {problem}");
                }
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
        let Some(song) = &mut self.loaded else {
            // Nothing moves without a song, except a hold's fill.
            if counting {
                cx.notify();
            }
            return;
        };
        if fires {
            send(song, UiAction::Panic);
        }
        song.poll();
        cx.notify();
    }

    /// Carries out what the presentational code or a key asks for; while
    /// the question before ending the show is open, the controls do
    /// nothing ([`control_acts`]).
    pub fn handle(&mut self, intent: Intent, window: &mut Window, cx: &mut Context<Self>) {
        if !control_acts(intent, self.asking) {
            return;
        }
        let now = Instant::now();
        match intent {
            Intent::Ui(action) => {
                if let Some(song) = &mut self.loaded {
                    send(song, action);
                    song.poll();
                }
            }
            Intent::PanicDown => self.hold.press(now),
            Intent::PanicUp => {
                if self.hold.release(now)
                    && let Some(song) = &mut self.loaded
                {
                    send(song, UiAction::Panic);
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
                self.end_show(Ending::Quit, window, cx);
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
fn send(song: &mut Song, action: UiAction) {
    if let Some(command) = command_for(action, &song.snapshot, song.session.song()) {
        let _ = song.session.send(command);
    }
}

impl Render for Perform {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.loaded {
            Some(song) => {
                let view = PerformView::new(&song.snapshot, song.session.song());
                ui::perform(&view, self.hold.fill(Instant::now()), &self.problems, cx)
                    .into_any_element()
            }
            None => ui::no_song(&self.problems, cx).into_any_element(),
        };
        ui::root(&self.focus, cx)
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .child(body)
            .when(self.asking, |root| root.child(ui::shield()))
    }
}

/// Opens the Perform window for `song_dir`, if any. An error means no
/// window opened, and so no song or device either.
pub fn open_window(song_dir: Option<PathBuf>, cx: &mut App) -> gpui::Result<()> {
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
    cx.open_window(options, |window, cx| {
        let screen = cx.new(|cx| Perform::new(song_dir, window, cx));
        // Closing the window while a song plays asks first; otherwise the
        // song stops with its ramp before the window closes.
        let closing = screen.downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            closing
                .update(cx, |screen, cx| {
                    screen.end_show(Ending::CloseWindow, window, cx)
                })
                .unwrap_or(true)
        });
        screen
    })?;
    Ok(())
}
