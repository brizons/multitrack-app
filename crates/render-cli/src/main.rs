//! `render-cli`: the headless renderer for tests and CI.
//!
//! Renders one arrangement of a song folder offline to a stereo 32-bit float
//! WAV file. Exit codes: 0 on success, 1 on load, validation, or render
//! errors (all listed on stderr, one per line), 2 on usage errors.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use domain::{CompileOptions, DEFAULT_ARRANGEMENT_ID, RenderPlan, compile};
use engine::{ClickSettings, MixSettings, audible_click_count, render_offline};
use library::{LoadError, check_wav_stereo_f32_frames, load_song, write_wav_stereo_f32};

/// Render a song package offline to a stereo 32-bit float WAV file.
#[derive(Debug, Parser)]
#[command(name = "render-cli", version)]
struct Args {
    /// Song folder: holds song.json and the stem files it lists.
    #[arg(value_name = "SONG_DIR")]
    song_dir: PathBuf,

    /// WAV file to write (stereo, 32-bit float, at the song sample rate).
    #[arg(long, value_name = "FILE")]
    output: PathBuf,

    /// Id of the arrangement to render.
    #[arg(long, value_name = "ID", default_value = DEFAULT_ARRANGEMENT_ID)]
    arrangement: String,

    /// Do not generate the click. A recorded click stem still plays.
    #[arg(long)]
    no_click: bool,
}

fn main() -> ExitCode {
    // Usage errors print clap's message and exit with code 2.
    let args = Args::parse();
    match run(&args) {
        Ok(summary) => {
            // The file is written: a closed stdout does not make this a failure.
            print_line(&mut io::stdout().lock(), &summary);
            ExitCode::SUCCESS
        }
        Err(errors) => {
            let mut stderr = io::stderr().lock();
            for error in errors {
                print_line(&mut stderr, &format!("error: {error}"));
            }
            ExitCode::from(1)
        }
    }
}

/// Writes `text` as one line, with control characters (newlines included)
/// escaped. Write errors, such as a closed pipe, are ignored: the exit code
/// already says how the run went.
fn print_line(out: &mut impl Write, text: &str) {
    let line: String = text
        .chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect();
    let _ = writeln!(out, "{line}");
}

/// Renders as `args` asks and returns the summary line, or every error.
fn run(args: &Args) -> Result<String, Vec<String>> {
    let loaded = load_song(&args.song_dir).map_err(load_errors)?;
    let mut stderr = io::stderr().lock();
    for warning in &loaded.warnings {
        print_line(&mut stderr, &format!("warning: {warning}"));
    }
    drop(stderr);
    let plan = compile(&loaded.song, &args.arrangement, &CompileOptions::default())
        .map_err(|error| vec![error.to_string()])?;
    // Reject before rendering: the output buffer would be as large as the file.
    check_wav_stereo_f32_frames(plan.length, plan.sample_rate)
        .map_err(|error| vec![error.to_string()])?;
    let settings = MixSettings {
        click: ClickSettings {
            enabled: !args.no_click,
            ..ClickSettings::default()
        },
        ..MixSettings::default()
    };
    let audio =
        render_offline(&plan, &loaded.stems, &settings).map_err(|error| vec![error.to_string()])?;
    write_wav_stereo_f32(&args.output, plan.sample_rate, &audio)
        .map_err(|error| vec![error.to_string()])?;
    let clicks = audible_click_count(&plan, &loaded.stems, &settings);
    Ok(summary(
        loaded.song.title(),
        &args.arrangement,
        &plan,
        clicks,
        &args.output,
    ))
}

fn load_errors(error: LoadError) -> Vec<String> {
    match error {
        LoadError::Invalid(problems) => problems.iter().map(ToString::to_string).collect(),
        other => vec![other.to_string()],
    }
}

/// `rendered "<title>" (arrangement "<id>"): <n> segments, <m:ss>, <n> clicks -> <file>`
fn summary(
    title: &str,
    arrangement: &str,
    plan: &RenderPlan,
    clicks: usize,
    output: &Path,
) -> String {
    let seconds = plan.length / u64::from(plan.sample_rate);
    format!(
        "rendered \"{title}\" (arrangement \"{arrangement}\"): {} segments, {}:{:02}, {clicks} clicks -> {}",
        plan.segments.len(),
        seconds / 60,
        seconds % 60,
        output.display()
    )
}
