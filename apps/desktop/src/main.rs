//! `multitrack-desktop`: the desktop shell for Mac, Windows and Linux (S6).
//!
//! Opens the Perform screen, with the song folder given on the command line
//! when there is one, on the system's default audio output.

#![forbid(unsafe_code)]

mod screen;
mod theme;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use gpui::Application;

const USAGE: &str = "\
usage: multitrack-desktop [SONG_DIR]

Opens the Perform screen. SONG_DIR is a song folder: song.json and the WAV
stems it lists; without it, use \"Open song…\" (Cmd/Ctrl-O). The song plays
on the system's default audio output, at the song's sample rate.";

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let song_dir = args.next().map(PathBuf::from);
    if args.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    if let Some(dir) = &song_dir
        && let Some(flag) = dir.to_str().filter(|arg| arg.starts_with('-'))
    {
        if flag == "-h" || flag == "--help" {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        eprintln!("unknown option {flag}\n\n{USAGE}");
        return ExitCode::from(2);
    }

    Application::new().with_assets(theme::Icons).run(move |cx| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        screen::open_window(song_dir, cx);
        cx.activate(true);
    });
    ExitCode::SUCCESS
}
