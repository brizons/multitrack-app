//! S6: `render-cli <SONG_DIR> --output <FILE> [--arrangement <ID>] [--no-click]`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use hound::{SampleFormat, WavSpec, WavWriter};

const RATE: u32 = 8_000;
/// Two bars at 120 bpm: four seconds.
const LENGTH: usize = 32_000;

/// A song folder with an `intro` and a `verse` (one bar each), the Default
/// arrangement and a `reversed` one, and two stems of `LENGTH` frames.
fn song_folder(dir: &Path, drums: &[i32]) -> PathBuf {
    let song = dir.join("song");
    fs::create_dir_all(song.join("stems")).unwrap();
    let package = SongPackage {
        format_version: 1,
        title: "Cli Test".to_owned(),
        artist: None,
        sample_rate: RATE,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![stem("drums"), stem("bass")],
        sections: vec![
            marker("intro", SectionType::Intro, 0),
            marker("verse", SectionType::Verse, 16_000),
        ],
        arrangements: vec![
            arrangement("default", &["intro", "verse"]),
            arrangement("reversed", &["verse", "intro", "verse"]),
        ],
    };
    fs::write(song.join("song.json"), package.to_json().unwrap()).unwrap();
    write_wav(&song.join("stems/drums.wav"), drums);
    write_wav(&song.join("stems/bass.wav"), &vec![0; LENGTH]);
    song
}

fn stem(id: &str) -> Stem {
    Stem {
        id: id.to_owned(),
        file: format!("stems/{id}.wav"),
        part: id.to_owned(),
        role: StemRole::Track,
    }
}

fn marker(id: &str, section_type: SectionType, start: u64) -> SectionMarker {
    SectionMarker {
        id: id.to_owned(),
        section_type,
        number: None,
        start,
    }
}

fn arrangement(id: &str, sections: &[&str]) -> Arrangement {
    Arrangement {
        id: id.to_owned(),
        name: id.to_owned(),
        sections: sections.iter().map(|s| (*s).to_owned()).collect(),
        infinite_loop: Vec::new(),
    }
}

fn write_wav(path: &Path, samples: &[i32]) {
    let spec = WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn render_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_render-cli"))
        .args(args)
        .output()
        .expect("render-cli runs")
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// Every sample of a stereo 32-bit float WAV file.
fn read_wav(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    assert_eq!(
        (
            spec.channels,
            spec.sample_rate,
            spec.bits_per_sample,
            spec.sample_format
        ),
        (2, RATE, 32, SampleFormat::Float)
    );
    reader.samples::<f32>().map(Result::unwrap).collect()
}

#[test]
fn renders_the_default_arrangement_and_prints_one_summary_line() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![1_000; LENGTH]);
    let out = dir.path().join("out.wav");

    let output = render_cli(&[path(&song), "--output", path(&out)]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        format!(
            "rendered \"Cli Test\" (arrangement \"default\"): 2 segments, 0:04, 8 clicks -> {}\n",
            out.display()
        )
    );
    assert_eq!(stderr(&output), "");
    let samples = read_wav(&out);
    assert_eq!(samples.len(), 2 * LENGTH);
}

#[test]
fn renders_the_requested_arrangement() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![0; LENGTH]);
    let out = dir.path().join("reversed.wav");

    let output = render_cli(&[
        path(&song),
        "--output",
        path(&out),
        "--arrangement",
        "reversed",
    ]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("(arrangement \"reversed\"): 3 segments, 0:06, 12 clicks"),
        "{}",
        stdout(&output)
    );
    assert_eq!(read_wav(&out).len(), 2 * 48_000);
}

#[test]
fn rendering_twice_writes_byte_identical_files() {
    let dir = tempfile::tempdir().unwrap();
    let drums: Vec<i32> = (0..LENGTH as i32)
        .map(|n| (n * 7_919) % 20_000 - 10_000)
        .collect();
    let song = song_folder(dir.path(), &drums);
    let first = dir.path().join("first.wav");
    let second = dir.path().join("second.wav");

    for out in [&first, &second] {
        let output = render_cli(&[
            path(&song),
            "--output",
            path(out),
            "--arrangement",
            "reversed",
        ]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    }

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
}

#[test]
fn no_click_removes_the_generated_click() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![0; LENGTH]);
    let with_click = dir.path().join("click.wav");
    let without_click = dir.path().join("silent.wav");

    let clicked = render_cli(&[path(&song), "--output", path(&with_click)]);
    let silent = render_cli(&[path(&song), "--output", path(&without_click), "--no-click"]);

    assert_eq!(clicked.status.code(), Some(0), "{}", stderr(&clicked));
    assert_eq!(silent.status.code(), Some(0), "{}", stderr(&silent));
    assert!(
        read_wav(&with_click).iter().any(|&s| s != 0.0),
        "the click plays"
    );
    assert!(
        read_wav(&without_click).iter().all(|&s| s == 0.0),
        "only silent stems"
    );
    assert!(
        stdout(&silent).contains(", 0 clicks -> "),
        "{}",
        stdout(&silent)
    );
}

#[test]
fn lists_every_validation_error_on_its_own_line_and_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![0; LENGTH]);
    let manifest = fs::read_to_string(song.join("song.json")).unwrap();
    let broken = manifest
        .replace("\"id\": \"default\"", "\"id\": \"live\"")
        .replace("\"bpm\": 120.0", "\"bpm\": 0.0");
    fs::write(song.join("song.json"), broken).unwrap();
    fs::remove_file(song.join("stems/bass.wav")).unwrap();
    let out = dir.path().join("out.wav");

    let output = render_cli(&[path(&song), "--output", path(&out)]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    let errors = stderr(&output);
    let lines: Vec<&str> = errors.lines().collect();
    assert_eq!(lines.len(), 3, "{errors}");
    assert!(
        lines.iter().all(|line| line.starts_with("error: ")),
        "{errors}"
    );
    assert!(lines[0].contains("\"bass\""), "{errors}");
    assert!(lines[1].contains("bpm"), "{errors}");
    assert!(lines[2].contains("Default arrangement"), "{errors}");
    assert!(!out.exists(), "nothing is written");
}

#[test]
fn an_unknown_arrangement_exits_1() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![0; LENGTH]);
    let out = dir.path().join("out.wav");

    let output = render_cli(&[path(&song), "--output", path(&out), "--arrangement", "nope"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert_eq!(stderr(&output), "error: unknown arrangement \"nope\"\n");
    assert!(!out.exists());
}

#[test]
fn a_missing_output_is_a_usage_error_exiting_2() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![0; LENGTH]);

    let output = render_cli(&[path(&song)]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("--output"), "{}", stderr(&output));
}

#[test]
fn warnings_go_to_stderr_and_the_render_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let song = song_folder(dir.path(), &vec![0; LENGTH]);
    write_wav(
        &song.join("stems/bass.wav"),
        &vec![0; LENGTH - 2 * RATE as usize],
    );
    let out = dir.path().join("out.wav");

    let output = render_cli(&[path(&song), "--output", path(&out)]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let warnings = stderr(&output);
    assert_eq!(warnings.lines().count(), 1, "{warnings}");
    assert!(
        warnings.starts_with("warning: stem lengths differ"),
        "{warnings}"
    );
    assert_eq!(stdout(&output).lines().count(), 1);
}

#[test]
fn help_describes_every_flag() {
    let output = render_cli(&["--help"]);

    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    for part in [
        "<SONG_DIR>",
        "--output <FILE>",
        "--arrangement <ID>",
        "[default: default]",
        "--no-click",
        "song.json",
        "32-bit float",
    ] {
        assert!(
            help.contains(part),
            "help does not mention {part:?}:\n{help}"
        );
    }
}
