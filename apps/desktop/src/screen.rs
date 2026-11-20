//! The Perform screen's container: owns the session of the song or setlist
//! that plays, polls it on a timer, turns pointer and key events into
//! view-model actions, and hands a [`PerformView`] (and, for a setlist, a
//! [`openvamp_desktop::strip::SetlistView`]) to the presentational code in
//! [`crate::ui`].

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use app::SessionOptions;
use gpui::{
    App, AppContext, Context, FocusHandle, KeyDownEvent, KeyUpEvent, Modifiers,
    ModifiersChangedEvent, PathPromptOptions, PromptButton, PromptLevel, Render, Subscription,
    Task, Window, prelude::*,
};
use io_cpal::CpalOutput;
use openvamp_desktop::action::{Intent, Page, UiAction};
use openvamp_desktop::ending::{Dialog, EndRequest, Ending, control_acts, end_request};
use openvamp_desktop::hold::PanicHold;
use openvamp_desktop::keys::{HeldKeys, KeyAction, KeyPress, Mods, Typed, is_panic_key, typed};
use openvamp_desktop::library_page::{LibraryPage, Opened, library_to_open};
use openvamp_desktop::show::{LoadedSetlist, Show, Source, close, replace};
use openvamp_desktop::view::PerformView;

use crate::{library_ui, ui};

/// How often the session is polled: about 30 Hz (Decision 3). The timer
/// keeps running while the window is hidden, since a waiting play starts
/// only through `Session::poll` (L16).
const POLL_EVERY: Duration = Duration::from_millis(33);

/// The Perform screen.
pub struct Perform {
    focus: FocusHandle,
    loaded: Option<Show>,
    /// Problems to show, one line each: why the last pick did not open (the
    /// song that was playing goes on), or how the song it replaced closed.
    problems: Vec<String>,
    hold: PanicHold,
    keys: HeldKeys,
    /// Whether the question before ending the show is on screen.
    asking: bool,
    /// The screen of the side navigation on show.
    page: Page,
    /// The library folder the Library screen shows, once one is open.
    library: Option<LibraryPage>,
    /// How [`Self::library`] was opened, which decides whether it follows
    /// the setlist on Perform.
    opened: Opened,
    /// Why the last library folder picked did not open.
    library_problem: Option<String>,
    _poll: Task<()>,
    _quit: Subscription,
    _deactivated: Subscription,
    _blurred: Subscription,
}

impl Perform {
    /// The screen, with `path` open when one is given: a song folder, or a
    /// setlist file ([`Source::for_path`]).
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            page: Page::Perform,
            library: None,
            opened: Opened::Followed,
            library_problem: None,
            _poll: poll,
            _quit: quit,
            _deactivated: deactivated,
            _blurred: blurred,
        };
        if let Some(path) = path {
            screen.open_path(&path);
        }
        screen
    }

    /// Stops the song or setlist with its ramp, sending a Stop the player
    /// refuses again within [`openvamp_desktop::song::STOP_WAIT`], then
    /// closes the session and its device; what went wrong goes to stderr.
    pub fn close_song(&mut self) {
        if let Some(show) = self.loaded.take()
            && let Some(problem) = close(show, thread::sleep).problem()
        {
            eprintln!("warning: {problem}");
        }
    }

    /// Quits or closes the window: at once when nothing plays (the song,
    /// if any, stops with its ramp first); while a song plays, only once
    /// the person confirms ([`end_request`]). Returns whether it ended at
    /// once, which lets the window close.
    fn end_show(&mut self, ending: Ending, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let transport = self.loaded.as_mut().map(|show| {
            show.poll();
            show.snapshot().transport
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

    /// Opens what `path` names ([`Source::for_path`]); one that cannot be
    /// opened says why.
    fn open_path(&mut self, path: &Path) {
        match Source::for_path(path) {
            Ok(source) => self.open(&source),
            Err(problem) => self.problems = vec![problem],
        }
    }

    /// Opens `source` in place of what plays ([`replace`]): a pick that
    /// does not open leaves the current song or setlist loaded and
    /// playing, and the reasons show on screen; otherwise what plays stops
    /// with its ramp and closes its device before the pick opens the
    /// device.
    fn open(&mut self, source: &Source) {
        let replaced = replace(
            &mut self.loaded,
            source,
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
                self.problems = error.lines();
                self.problems
                    .insert(0, format!("Could not open {}:", source.describe()));
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
        let Some(show) = &mut self.loaded else {
            // Nothing moves without a song, except a hold's fill.
            if counting {
                cx.notify();
            }
            return;
        };
        if fires {
            show.send(UiAction::Panic);
        }
        show.poll();
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
                if let Some(show) = &mut self.loaded {
                    show.send(action);
                    show.poll();
                }
            }
            Intent::PanicDown => self.hold.press(now),
            Intent::PanicUp => {
                if self.hold.release(now)
                    && let Some(show) = &mut self.loaded
                {
                    show.send(UiAction::Panic);
                }
            }
            Intent::OpenSong => self.prompt_for(Pick::Song, window, cx),
            Intent::OpenSetlist => self.prompt_for(Pick::Setlist, window, cx),
            Intent::OpenLibrary => self.prompt_for(Pick::Library, window, cx),
            Intent::ShowPage(page) => self.show_page(page),
            Intent::Library(action) => {
                // The setlist on Perform cannot be removed.
                let playing = self.playing_file().map(str::to_owned);
                if let Some(library) = &mut self.library {
                    library.set_playing(playing.as_deref());
                    library.act(action);
                }
                self.follow_library_moves();
            }
            Intent::PerformSetlist => self.perform_setlist(),
            Intent::DismissProblems => self.problems.clear(),
        }
        cx.notify();
    }

    /// Asks for a song folder or a setlist file, then opens it.
    fn prompt_for(&mut self, pick: Pick, _window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(match pick {
            Pick::Song => PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Open song".into()),
            },
            Pick::Setlist => PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some("Open setlist".into()),
            },
            Pick::Library => PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Open library".into()),
            },
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
            if let Some(path) = chosen {
                let _ = this.update(cx, |screen, cx| {
                    match pick {
                        Pick::Song => screen.open(&Source::Song(path)),
                        Pick::Setlist => match Source::picked_setlist(&path) {
                            Ok(source) => screen.open(&source),
                            Err(problem) => screen.problems = vec![problem],
                        },
                        Pick::Library => screen.open_library(&path, None, Opened::Picked),
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Shows `page`. The Library screen opens on the library of the
    /// setlist that plays, with that setlist in the editor, when no library
    /// is open yet.
    fn show_page(&mut self, page: Page) {
        self.page = page;
        if page != Page::Library {
            return;
        }
        let playing = match &self.loaded {
            Some(Show::Setlist(setlist)) => {
                Some((setlist.library.as_path(), setlist.file.as_str()))
            }
            _ => None,
        };
        let open = self
            .library
            .as_ref()
            .map(|library| (library.root(), self.opened));
        if let Some((root, file)) = library_to_open(open, playing) {
            self.open_library(&root, Some(&file), Opened::Followed);
        }
    }

    /// Lets the setlist on Perform follow its file when the Library screen
    /// moved it (a rename, or a file named by hand moved on its first
    /// change), so it opens again from where it now is and stays marked.
    fn follow_library_moves(&mut self) {
        let Some(library) = &mut self.library else {
            return;
        };
        let root = library.root().to_owned();
        for (from, to) in library.take_moves() {
            if let Some(show) = &mut self.loaded {
                show.follow_move(&root, &from, &to);
            }
        }
    }

    /// Opens the library folder `root` on the Library screen, with `file`
    /// in the editor when given.
    fn open_library(&mut self, root: &Path, file: Option<&str>, opened: Opened) {
        match LibraryPage::open(root, file) {
            Ok(library) => {
                self.library = Some(library);
                self.opened = opened;
                self.library_problem = None;
            }
            Err(error) => {
                self.library_problem = Some(format!("Could not open {}: {error}", root.display()));
            }
        }
    }

    /// Opens the setlist in the Library screen's editor on the Perform
    /// screen, as Open setlist… does.
    fn perform_setlist(&mut self) {
        let Some(library) = &self.library else {
            return;
        };
        let Some(file) = library.selected_file() else {
            return;
        };
        let source = Source::Setlist {
            library: library.root().to_owned(),
            file: file.to_owned(),
        };
        self.open(&source);
        self.page = Page::Perform;
    }

    /// The setlist file open on the Perform screen, when it is in the
    /// library the Library screen shows.
    fn playing_file(&self) -> Option<&str> {
        let library = self.library.as_ref()?;
        match &self.loaded {
            Some(Show::Setlist(setlist)) if setlist.library == library.root() => {
                Some(setlist.file.as_str())
            }
            _ => None,
        }
    }

    /// A key while a field of the Library screen has the focus: it types
    /// into the field, except the shortcuts ([`typed`]).
    fn type_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let press = KeyPress {
            key: &event.keystroke.key,
            mods: mods(&event.keystroke.modifiers),
        };
        cx.stop_propagation();
        match typed(&press, event.keystroke.key_char.as_deref()) {
            Typed::Line(key) => {
                if let Some(library) = &mut self.library {
                    library.type_key(key);
                }
                self.follow_library_moves();
            }
            Typed::Paste => {
                let text = cx.read_from_clipboard().and_then(|item| item.text());
                if let (Some(library), Some(text)) = (&mut self.library, text) {
                    library.paste(&text);
                }
            }
            Typed::Key(KeyAction::Quit) => {
                self.end_show(Ending::Quit, window, cx);
                return;
            }
            Typed::Key(KeyAction::OpenSong) => self.handle(Intent::OpenSong, window, cx),
            Typed::Key(_) | Typed::Ignored => {}
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.page == Page::Library
            && self
                .library
                .as_ref()
                .is_some_and(|library| library.typing().is_some())
        {
            self.type_key(event, window, cx);
            return;
        }
        let press = KeyPress {
            key: &event.keystroke.key,
            mods: mods(&event.keystroke.modifiers),
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

    fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.keys.modifiers_changed(mods(&event.modifiers));
    }

    fn key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.keys.release(&event.keystroke.key);
        if is_panic_key(&event.keystroke.key) {
            self.handle(Intent::PanicUp, window, cx);
        }
    }
}

/// The modifiers the keyboard rules read; Shift is left out.
fn mods(modifiers: &Modifiers) -> Mods {
    Mods {
        control: modifiers.control,
        alt: modifiers.alt,
        platform: modifiers.platform,
        function: modifiers.function,
    }
}

/// What a picker asks for.
#[derive(Debug, Clone, Copy)]
enum Pick {
    /// A song folder.
    Song,
    /// A setlist file in the Setlists folder of a library folder.
    Setlist,
    /// A library folder.
    Library,
}

impl Render for Perform {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (self.page, &self.loaded) {
            (Page::Library, _) => match &self.library {
                Some(library) => {
                    let view = library.view(self.playing_file());
                    library_ui::library(&view, cx).into_any_element()
                }
                None => {
                    library_ui::no_library(self.library_problem.as_deref(), cx).into_any_element()
                }
            },
            (Page::Perform, Some(show)) => {
                let view = PerformView::new(show.snapshot(), show.song());
                let setlist = show.setlist().map(LoadedSetlist::view);
                ui::perform(
                    &view,
                    setlist.as_ref(),
                    self.hold.fill(Instant::now()),
                    &self.problems,
                    cx,
                )
                .into_any_element()
            }
            (Page::Perform, None) => ui::no_song(&self.problems, cx).into_any_element(),
        };
        ui::root(&self.focus, cx)
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .child(ui::nav(self.page, cx))
            .child(ui::main().child(body))
            .when(self.asking, |root| root.child(ui::shield()))
    }
}

/// Opens the Perform window for `path` (a song folder or a setlist file),
/// if any. An error means no window opened, and so no song or device
/// either.
pub fn open_window(path: Option<PathBuf>, cx: &mut App) -> gpui::Result<()> {
    let bounds = gpui::Bounds::centered(None, gpui::size(gpui::px(1366.), gpui::px(900.)), cx);
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some("OpenVamp — Perform".into()),
            ..Default::default()
        }),
        window_min_size: Some(gpui::size(gpui::px(960.), gpui::px(640.))),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let screen = cx.new(|cx| Perform::new(path, window, cx));
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
