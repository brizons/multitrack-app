//! S5: `play-cli <SONG_DIR> [--device <NAME>] [--buffer <FRAMES>] [--list-devices]`.
//!
//! These runs never start playback: each one stops before a device opens,
//! or at the device lookup.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use hound::{SampleFormat, WavSpec, WavWriter};

fn play_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_play-cli"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("play-cli runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// A one-second song with one silent stem; `stem_file` names its file.
fn song_folder(dir: &Path, stem_file: &str) -> PathBuf {
    let song = dir.join("song");
    fs::create_dir_all(song.join("stems")).unwrap();
    let package = SongPackage {
        format_version: 1,
        title: "Cli Test".to_owned(),
        artist: None,
        sample_rate: 48_000,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![Stem {
            id: "pad".to_owned(),
            file: stem_file.to_owned(),
            part: "pad".to_owned(),
            role: StemRole::Track,
        }],
        sections: vec![SectionMarker {
            id: "intro".to_owned(),
            section_type: SectionType::Intro,
            number: None,
            start: 0,
        }],
        arrangements: vec![Arrangement {
            id: "default".to_owned(),
            name: "Default".to_owned(),
            sections: vec!["intro".to_owned()],
            infinite_loop: Vec::new(),
        }],
    };
    fs::write(song.join("song.json"), package.to_json().unwrap()).unwrap();
    let spec = WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(song.join("stems/pad.wav"), spec).unwrap();
    for _ in 0..48_000 {
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    song
}

#[test]
fn help_shows_the_command_line_and_the_controls() {
    let output = play_cli(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    assert!(help.contains(
        "Usage: play-cli <SONG_DIR> [--device <NAME>] [--buffer <FRAMES>] [--list-devices]"
    ));
    for line in [
        "--device <NAME>",
        "--buffer <FRAMES>",
        "--list-devices",
        "space  play / pause",
        "1-9    queue a jump to arrangement position 1-9",
        "n      jump timing: at the section end or at the next bar",
        "x      panic: fade everything out in 0.5 s and stop",
        "q      quit (Ctrl-C too)",
    ] {
        assert!(help.contains(line), "help mentions {line:?}:\n{help}");
    }
}

#[test]
fn usage_errors_exit_with_code_2() {
    for args in [
        &[][..],
        &["song", "--buffer", "32"],
        &["song", "--buffer", "2048"],
        &["song", "--buffer", "many"],
        &["song", "--loop"],
    ] {
        let output = play_cli(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr(&output).starts_with("error: "), "{args:?}");
        assert!(stdout(&output).is_empty());
    }
    assert!(stderr(&play_cli(&["song", "--buffer", "32"])).contains("64..=1024"));
}

#[test]
fn a_missing_song_folder_exits_with_code_1() {
    let output = play_cli(&["/nonexistent"]);
    assert_eq!(output.status.code(), Some(1));
    // The path separator and the OS error text differ by platform.
    let expected = format!(
        "error: cannot read {}: ",
        Path::new("/nonexistent").join("song.json").display()
    );
    let stderr = stderr(&output);
    assert!(stderr.starts_with(&expected), "{stderr}");
    assert!(
        stderr.ends_with(")\n") && stderr.lines().count() == 1,
        "{stderr}"
    );
    assert!(stdout(&output).is_empty());
}

#[test]
fn every_problem_of_an_invalid_song_is_listed_with_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), "stems/missing.wav");
    let output = play_cli(&[song.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let errors = stderr(&output);
    assert!(
        errors.starts_with("error: stem \"pad\" file \"stems/missing.wav\" cannot be read:"),
        "{errors}"
    );
    assert!(errors.lines().all(|line| line.starts_with("error: ")));
}

#[test]
fn an_unknown_device_exits_with_code_1_before_anything_plays() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), "stems/pad.wav");
    let output = play_cli(&[song.to_str().unwrap(), "--device", "No Such Device"]);
    assert_eq!(output.status.code(), Some(1));
    let errors = stderr(&output);
    // Without any audio system, the host error is reported instead.
    assert!(
        errors.starts_with(
            "error: no audio output device has the name or id \"No Such Device\"; available: "
        ) || errors.starts_with("error: audio system: "),
        "{errors}"
    );
}

/// Run with `cargo test -p play-cli --test cli -- --ignored` on a machine
/// with an audio output: the song and the device are checked first, so the
/// default output opens for an instant (silence; the transport never
/// starts) before play-cli refuses.
#[test]
#[ignore = "opens the default audio output for an instant"]
fn without_a_terminal_it_refuses_and_exits_with_code_1() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), "stems/pad.wav");
    let output = play_cli(&[song.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr(&output),
        "error: play-cli reads single keys, so its standard input must be a terminal\n"
    );
}

/// Run with `cargo test -p play-cli --test cli -- --ignored` on a machine
/// with an audio system; it only lists devices and opens none.
#[test]
#[ignore = "needs an audio system"]
fn list_devices_prints_one_line_per_device_and_exits_with_code_0() {
    let output = play_cli(&["--list-devices"]);
    assert_eq!(output.status.code(), Some(0));
    let list = stdout(&output);
    assert!(!list.is_empty());
    assert!(list.lines().all(|line| {
        (line.starts_with("* ") || line.starts_with("  "))
            && (line.contains(" output channels), id ") || line.contains(" output channel), id "))
    }));
}

#[test]
fn list_devices_refuses_a_song_folder_a_device_or_a_buffer_with_code_2() {
    for args in [
        &["some/song", "--list-devices"][..],
        &["--list-devices", "--device", "Studio"],
        &["--list-devices", "--buffer", "512"],
    ] {
        let output = play_cli(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stdout(&output).is_empty(), "{args:?}: nothing is listed");
        assert!(
            stderr(&output).contains("cannot be used with"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}
