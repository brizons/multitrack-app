//! S8: the command line. Every case here exits before the app opens a
//! window or an audio device; a run still going after a few seconds is
//! killed and fails the test.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_openvamp");
const EXIT_WITHIN: Duration = Duration::from_secs(10);

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn read(pipe: Option<impl Read>) -> String {
    let mut text = String::new();
    pipe.expect("the pipe is set up")
        .read_to_string(&mut text)
        .unwrap();
    text
}

/// Runs `openvamp args`, which must exit by itself within
/// [`EXIT_WITHIN`].
fn run(args: &[&str]) -> Run {
    let mut child: Child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + EXIT_WITHIN;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("openvamp {args:?} was still running after {EXIT_WITHIN:?}");
        }
        thread::sleep(Duration::from_millis(10));
    };
    Run {
        code: status.code(),
        stdout: read(child.stdout.take()),
        stderr: read(child.stderr.take()),
    }
}

#[test]
fn help_prints_the_usage_and_exits_0() {
    for flag in ["--help", "-h"] {
        let out = run(&[flag]);
        assert_eq!(out.code, Some(0), "{flag}: {}", out.stderr);
        assert!(
            out.stdout.starts_with("usage: openvamp [SONG_DIR]"),
            "{flag}: {}",
            out.stdout
        );
        assert_eq!(out.stderr, "", "{flag}");
    }
}

#[test]
fn an_unknown_option_is_named_and_exits_2() {
    let out = run(&["--bogus"]);
    assert_eq!(out.code, Some(2));
    assert!(
        out.stderr.starts_with("unknown option --bogus\n"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("usage: openvamp"));
    assert_eq!(out.stdout, "");
}

#[test]
fn an_extra_argument_is_named_and_exits_2() {
    let out = run(&["first-song", "second-song"]);
    assert_eq!(out.code, Some(2));
    assert!(
        out.stderr.starts_with(
            "unexpected argument \"second-song\" (one song folder or setlist file at most)\n"
        ),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("usage: openvamp"));
    assert_eq!(out.stdout, "");

    let out = run(&["a", "b", "c d"]);
    assert_eq!(out.code, Some(2));
    assert!(
        out.stderr.starts_with(
            "unexpected arguments \"b\" \"c d\" (one song folder or setlist file at most)\n"
        ),
        "{}",
        out.stderr
    );

    // An extra argument after a flag is named too.
    let out = run(&["--help", "more"]);
    assert_eq!(out.code, Some(2));
    assert!(
        out.stderr.starts_with("unexpected argument \"more\""),
        "{}",
        out.stderr
    );
}
