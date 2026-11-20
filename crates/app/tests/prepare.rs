//! S8: a song is prepared without a device, then started on one, so a shell
//! can check a new song while another one plays and open the device only
//! once the old song has closed. `Session::open` is the two steps in one.

mod common;

use app::testing::FakeOutput;
use app::{OpenError, OutputError, Session, SessionOptions, TransportState};
use common::{RATE, bits, offline, play_for, start, write_song};
use domain::DEFAULT_SEAM_CROSSFADE;

fn song(rate: u32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), rate);
    dir
}

fn options() -> SessionOptions {
    SessionOptions {
        buffer_frames: 256,
        ..SessionOptions::default()
    }
}

#[test]
fn a_prepared_song_touches_no_device_until_it_starts() {
    let dir = song(RATE);
    let fake = FakeOutput::new();

    let prepared = Session::prepare(dir.path(), options()).expect("the song prepares");
    assert!(fake.opened().is_empty(), "no device opened while preparing");
    assert_eq!(prepared.song().sample_rate, RATE);
    let title = prepared.song().title.clone();

    let mut session = prepared
        .start(Box::new(fake.clone()))
        .expect("the device opens");
    assert_eq!(fake.opened().len(), 1);
    assert_eq!(fake.opened()[0].sample_rate, RATE);
    assert_eq!(session.song().title, title);
    assert_eq!(session.poll().transport, TransportState::Stopped);
}

#[test]
fn a_started_song_plays_exactly_as_an_opened_one() {
    let dir = song(RATE);
    let expected = offline(dir.path(), "default", DEFAULT_SEAM_CROSSFADE);
    let fake = FakeOutput::new();
    let mut session = Session::prepare(dir.path(), options())
        .unwrap()
        .start(Box::new(fake.clone()))
        .unwrap();

    let (first, _) = start(&mut session, &fake, 256);
    let (rest, _) = play_for(&mut session, &fake, 256, expected.len() / 2);
    let played = [first, rest].concat();
    assert_eq!(bits(&played[..expected.len()]), bits(&expected));
}

#[test]
fn preparing_fails_for_a_bad_folder_or_buffer_with_no_device_involved() {
    let dir = song(RATE);
    let missing = dir.path().join("no-such-song");
    assert!(matches!(
        Session::prepare(&missing, options()),
        Err(OpenError::Load(_))
    ));
    let tiny = SessionOptions {
        buffer_frames: 16,
        ..options()
    };
    assert!(matches!(
        Session::prepare(dir.path(), tiny),
        Err(OpenError::BufferSize { frames: 16 })
    ));
    let low = song(1_000);
    assert!(matches!(
        Session::prepare(low.path(), options()),
        Err(OpenError::RateTooLow { .. })
    ));
}

#[test]
fn starting_on_a_device_that_refuses_the_rate_says_why() {
    let dir = song(RATE);
    let fake = FakeOutput::without_devices().with_device("Studio", 2, Some(&[48_000]));
    let error = Session::prepare(dir.path(), options())
        .unwrap()
        .start(Box::new(fake.clone()))
        .err()
        .expect("the device does not run at 8 kHz");
    assert!(matches!(
        error,
        OpenError::Output(OutputError::UnsupportedSampleRate {
            requested: RATE,
            ..
        })
    ));
    assert!(fake.opened().is_empty(), "no stream opened");
}

#[test]
fn on_a_one_stream_device_the_next_song_prepares_while_one_plays_and_starts_after_it() {
    let first = song(RATE);
    let second = song(RATE);
    let shared = FakeOutput::new();
    let mut playing = Session::open(first.path(), Box::new(shared.clone()), options()).unwrap();
    start(&mut playing, &shared, 256);

    // The device plays one stream at a time: opening beside it fails.
    assert!(Session::open(second.path(), Box::new(shared.clone()), options()).is_err());

    // Preparing needs no device, so it succeeds while the first plays.
    let prepared = Session::prepare(second.path(), options()).unwrap();
    drop(playing);
    let mut next = prepared
        .start(Box::new(shared.clone()))
        .expect("the device is free once the first song closed");
    assert_eq!(next.poll().transport, TransportState::Stopped);
    assert!(shared.is_streaming());
}
