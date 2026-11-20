//! S8: an output checks a request before any stream opens, so a shell can
//! leave the current song playing when the device cannot play the next
//! one. A check refuses exactly what opening would refuse before a stream
//! is built, and opens nothing.

mod common;

use app::testing::{FAKE_DEVICE_ID, FakeOutput};
use app::{
    AudioOutput, AudioRenderer, OpenError, OutputCheck, OutputError, OutputRequest, OutputStream,
    Session, SessionOptions,
};
use common::{RATE, start, write_song};

fn song(rate: u32) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), rate);
    dir
}

fn options(device: Option<&str>) -> SessionOptions {
    SessionOptions {
        buffer_frames: 256,
        device: device.map(str::to_owned),
        ..SessionOptions::default()
    }
}

/// What a check says and what opening the same song on the same output
/// does: the check refuses exactly the requests opening refuses.
fn check_then_open(fake: &FakeOutput, device: Option<&str>) -> (OutputCheck, Option<OutputError>) {
    let dir = song(RATE);
    let check = Session::prepare(dir.path(), options(device))
        .unwrap()
        .check(fake);
    let opened = Session::prepare(dir.path(), options(device))
        .unwrap()
        .start(Box::new(fake.clone()));
    let error = match opened {
        Ok(_) => None,
        Err(OpenError::Output(error)) => Some(error),
        Err(other) => panic!("only the device can refuse here: {other}"),
    };
    (check, error)
}

#[test]
fn the_fake_output_refuses_what_its_open_refuses_and_opens_nothing() {
    let cases = [
        ("a stereo device at any rate", FakeOutput::new(), None),
        (
            "a device without the song's rate",
            FakeOutput::without_devices().with_device("Studio", 2, Some(&[48_000])),
            None,
        ),
        (
            "a mono device",
            FakeOutput::without_devices().with_device("Mono", 1, None),
            None,
        ),
        ("no device at all", FakeOutput::without_devices(), None),
        ("a device nobody has", FakeOutput::new(), Some("Elsewhere")),
    ];
    for (case, fake, device) in cases {
        let (check, opened) = check_then_open(&fake, device);
        match &opened {
            None => assert_eq!(check, OutputCheck::Fits, "{case}"),
            Some(error) => assert_eq!(check, OutputCheck::Refused(error.clone()), "{case}"),
        }
    }

    // The refusals are the expected ones.
    let studio = FakeOutput::without_devices().with_device("Studio", 2, Some(&[48_000]));
    assert!(matches!(
        check_then_open(&studio, None).0,
        OutputCheck::Refused(OutputError::UnsupportedSampleRate {
            requested: RATE,
            ..
        })
    ));
    assert_eq!(
        check_then_open(&FakeOutput::without_devices(), None).0,
        OutputCheck::Refused(OutputError::NoDevice)
    );
}

#[test]
fn a_check_counts_as_no_open_and_no_stream() {
    let dir = song(RATE);
    let fake = FakeOutput::new();
    let prepared = Session::prepare(dir.path(), options(None)).unwrap();
    for _ in 0..3 {
        assert_eq!(prepared.check(&fake), OutputCheck::Fits);
    }
    assert_eq!(fake.checks(), 3);
    assert_eq!(fake.open_attempts(), 0);
    assert!(fake.opened().is_empty());
    assert!(!fake.is_streaming());

    let refusing = FakeOutput::without_devices().with_device("Studio", 2, Some(&[48_000]));
    assert!(matches!(prepared.check(&refusing), OutputCheck::Refused(_)));
    assert_eq!(refusing.open_attempts(), 0);

    // Opening counts every attempt, accepted or refused.
    assert!(prepared.start(Box::new(refusing.clone())).is_err());
    assert_eq!(refusing.open_attempts(), 1);
    assert!(refusing.opened().is_empty());
}

#[test]
fn an_unplugged_device_is_refused_by_name_and_as_the_default() {
    let fake = FakeOutput::new();
    fake.unplug(FAKE_DEVICE_ID);
    assert_eq!(
        check_then_open(&fake, None).0,
        OutputCheck::Refused(OutputError::NoDevice)
    );
    assert!(matches!(
        check_then_open(&fake, Some(FAKE_DEVICE_ID)).0,
        OutputCheck::Refused(OutputError::DeviceNotFound { .. })
    ));
    fake.plug(FAKE_DEVICE_ID);
    assert_eq!(check_then_open(&fake, None).0, OutputCheck::Fits);
}

#[test]
fn a_check_leaves_out_a_stream_already_playing_on_the_device() {
    // The device may be busy only with the song about to be replaced; a
    // one-stream device still fits, and opening tells later.
    let first = song(RATE);
    let second = song(RATE);
    let shared = FakeOutput::new();
    let mut playing = Session::open(first.path(), Box::new(shared.clone()), options(None)).unwrap();
    start(&mut playing, &shared, 256);
    let prepared = Session::prepare(second.path(), options(None)).unwrap();
    assert_eq!(prepared.check(&shared), OutputCheck::Fits);
    assert!(shared.is_streaming(), "the playing stream is untouched");
    assert_eq!(shared.open_attempts(), 1, "only the first song's open");
}

/// A port that does not override `check`.
struct Unchecked(FakeOutput);

impl AudioOutput for Unchecked {
    fn devices(&self) -> Result<Vec<app::DeviceInfo>, OutputError> {
        self.0.devices()
    }

    fn open(
        &mut self,
        request: &OutputRequest,
        renderer: AudioRenderer,
    ) -> Result<Box<dyn OutputStream>, OutputError> {
        self.0.open(request, renderer)
    }
}

#[test]
fn a_port_without_a_check_cannot_tell_and_opening_still_decides() {
    let dir = song(RATE);
    let refusing = FakeOutput::without_devices().with_device("Studio", 2, Some(&[48_000]));
    let prepared = Session::prepare(dir.path(), options(None)).unwrap();
    let check = prepared.check(&Unchecked(refusing.clone()));
    let OutputCheck::Unknown(reason) = &check else {
        panic!("a port with no check cannot tell: {check:?}");
    };
    assert!(!reason.is_empty());
    assert!(check.goes_ahead());
    assert!(OutputCheck::Fits.goes_ahead());
    assert!(!OutputCheck::Refused(OutputError::NoDevice).goes_ahead());
    assert!(prepared.start(Box::new(Unchecked(refusing))).is_err());
}
