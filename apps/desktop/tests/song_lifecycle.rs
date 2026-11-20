//! S8: a pick that cannot open, or that the device cannot play, never ends
//! the current song; a pick that can open stops the current song with its
//! ramp and frees its device before the new song opens the device; and
//! closing a song always ramps down, even when the player refuses Stop at
//! first. A real `Session` on the app's `FakeOutput`.

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::time::Duration;

use app::testing::{FAKE_DEVICE_ID, FakeOutput};
use app::{
    AudioOutput, AudioRenderer, Command, DeviceInfo, DeviceStatus, JumpTiming, OpenError,
    OutputError, OutputRequest, OutputStream, RuntimeError, SessionOptions, TransportState,
};
use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use hound::{SampleFormat, WavSpec, WavWriter};
use openvamp_desktop::song::{Closing, STOP_POLL, STOP_WAIT, Song, close, replace, stop_and_wait};

const RATE: u32 = 8_000;
const FRAMES: usize = 256;

/// `title` at `rate`, 120 bpm in 4/4: intro and verse, one bar each, one
/// mono track stem that is never silent.
fn write_song_at(dir: &Path, title: &str, rate: u32) {
    let bar = 2 * u64::from(rate);
    let section = |id: &str, section_type, at: u64| SectionMarker {
        id: id.to_owned(),
        section_type,
        number: None,
        start: at * bar,
    };
    let package = SongPackage {
        format_version: 1,
        title: title.to_owned(),
        artist: None,
        sample_rate: rate,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![Stem {
            id: "keys".to_owned(),
            file: "stems/keys.wav".to_owned(),
            part: "keys".to_owned(),
            role: StemRole::Track,
        }],
        sections: vec![
            section("intro", SectionType::Intro, 0),
            section("verse", SectionType::Verse, 1),
        ],
        arrangements: vec![Arrangement {
            id: "default".to_owned(),
            name: "Default".to_owned(),
            sections: vec!["intro".to_owned(), "verse".to_owned()],
            infinite_loop: Vec::new(),
        }],
    };
    fs::create_dir_all(dir.join("stems")).unwrap();
    fs::write(dir.join("song.json"), package.to_json().unwrap()).unwrap();
    let spec = WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(dir.join("stems/keys.wav"), spec).unwrap();
    for frame in 0..2 * bar {
        writer
            .write_sample(2_000 + (frame % 200) as i16 * 50)
            .unwrap();
    }
    writer.finalize().unwrap();
}

fn write_song(dir: &Path, title: &str) {
    write_song_at(dir, title, RATE);
}

/// A song on `fake`, the screen's way.
fn open_on(dir: &Path, fake: &FakeOutput) -> Result<Song, OpenError> {
    Song::open(dir, &SessionOptions::default(), Box::new(fake.clone()))
}

/// One audio callback once the disk has caught up.
fn step(song: &mut Song, fake: &FakeOutput) -> Vec<f32> {
    assert!(
        song.session
            .disk_monitor()
            .wait_idle(Duration::from_secs(10))
    );
    let audio = fake.render(FRAMES).expect("a stream plays");
    song.poll();
    audio
}

fn play(song: &mut Song, fake: &FakeOutput) {
    song.session.send(Command::Play).unwrap();
    for _ in 0..200 {
        step(song, fake);
        if song.snapshot.transport == TransportState::Playing {
            return;
        }
    }
    panic!("the song never started");
}

/// Sends commands nobody takes until the player's queue refuses one.
fn fill_the_queue(song: &mut Song) -> RuntimeError {
    for _ in 0..10_000 {
        if let Err(error) = song
            .session
            .send(Command::SetJumpTiming(JumpTiming::NextBar))
        {
            return error;
        }
    }
    panic!("the command queue never filled");
}

/// An output factory that always hands out `fake` and counts its calls.
fn outputs<'a>(
    fake: &'a FakeOutput,
    calls: &'a Cell<usize>,
) -> impl FnMut() -> Box<dyn AudioOutput> + 'a {
    move || {
        calls.set(calls.get() + 1);
        Box::new(fake.clone())
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0, |peak, sample| peak.max(sample.abs()))
}

/// Whether the callbacks `heard` while closing end in a ramp: sound at the
/// start of the last one, silence at its end, and silence after it.
fn ramped_down(heard: &[Vec<f32>], after: &[f32]) -> bool {
    let Some(last) = heard.last() else {
        return false;
    };
    let (head, tail) = last.split_at(last.len() / 4);
    peak(head) > 0.01 && tail.last() == Some(&0.0) && peak(after) == 0.0
}

#[test]
fn a_pick_that_cannot_open_leaves_the_current_song_playing() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let mut current = Some(open_on(dir.path(), &fake).unwrap());
    play(current.as_mut().unwrap(), &fake);

    let calls = Cell::new(0);
    let missing = dir.path().join("no-such-song");
    let refused = replace(
        &mut current,
        &missing,
        &SessionOptions::default(),
        outputs(&fake, &calls),
        |_| panic!("nothing stops"),
    );
    assert!(matches!(refused, Err(OpenError::Load(_))), "{refused:?}");
    assert_eq!(calls.get(), 0, "no device is touched");

    // The first song is still there, and still plays.
    let song = current.as_mut().expect("the current song stays loaded");
    assert_eq!(song.session.song().title, "First");
    let before = song.snapshot.output_frame;
    let audio = step(song, &fake);
    assert_eq!(song.snapshot.transport, TransportState::Playing);
    assert!(song.snapshot.output_frame > before, "the playhead moves on");
    assert!(peak(&audio) > 0.01, "the song is still heard");
}

#[test]
fn a_pick_stops_the_current_song_with_its_ramp_before_it_opens_the_device() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song(second.path(), "Second");
    // One device that plays one stream at a time, as some interfaces do.
    let device = FakeOutput::new();
    let mut current = Some(open_on(first.path(), &device).unwrap());
    play(current.as_mut().unwrap(), &device);

    let mut heard = Vec::new();
    let replaced = replace(
        &mut current,
        second.path(),
        &SessionOptions::default(),
        // The device takes one stream: the second song can only open it
        // once the first song closed.
        || Box::new(device.clone()),
        |_| heard.push(device.render(FRAMES).expect("the first song plays")),
    )
    .unwrap_or_else(|error| panic!("the second song opens: {error}"));

    let previous = replaced.previous.as_ref().expect("a song was loaded");
    assert_eq!(previous.title, "First");
    assert_eq!(previous.closing, Closing::Stopped { refused: None });
    assert!(replaced.start_error.is_none() && replaced.reopen_error.is_none());
    assert!(replaced.problems().is_empty(), "{:?}", replaced.problems());
    let song = current.as_mut().expect("the second song is loaded");
    assert_eq!(song.session.song().title, "Second");
    assert_eq!(song.dir, second.path());
    assert_eq!(song.snapshot.transport, TransportState::Stopped);
    assert!(
        ramped_down(&heard, &step(song, &device)),
        "a ramp, not a cut"
    );
    assert_eq!(device.opened().len(), 2);
    assert_eq!(device.open_attempts(), 2, "one open per song, none refused");
    assert_eq!(device.checks(), 1, "the pick was checked first");
}

#[test]
fn with_nothing_loaded_a_pick_simply_loads() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let calls = Cell::new(0);
    let mut current = None;
    let replaced = replace(
        &mut current,
        dir.path(),
        &SessionOptions::default(),
        outputs(&fake, &calls),
        |_| panic!("nothing stops"),
    )
    .unwrap();
    assert!(replaced.previous.is_none() && replaced.problems().is_empty());
    assert_eq!(calls.get(), 1);
    assert_eq!(current.unwrap().session.song().title, "First");
}

#[test]
fn a_pick_the_device_cannot_play_leaves_the_current_song_playing() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song_at(second.path(), "Second", 16_000);
    let device = FakeOutput::without_devices().with_device("Only 8k", 2, Some(&[RATE]));
    let mut current = Some(open_on(first.path(), &device).unwrap());
    play(current.as_mut().unwrap(), &device);

    let refused = replace(
        &mut current,
        second.path(),
        &SessionOptions::default(),
        || Box::new(device.clone()),
        |_| panic!("nothing stops"),
    );
    let Err(OpenError::Output(OutputError::UnsupportedSampleRate {
        requested: 16_000, ..
    })) = &refused
    else {
        panic!("the device cannot play 16 kHz: {refused:?}");
    };
    assert_eq!(device.checks(), 1);
    assert_eq!(device.open_attempts(), 1, "only the first song's open");

    let song = current.as_mut().expect("the first song stays loaded");
    assert_eq!(song.session.song().title, "First");
    let audio = step(song, &device);
    assert_eq!(song.snapshot.transport, TransportState::Playing);
    assert!(peak(&audio) > 0.01, "the first song is still heard");
    assert_eq!(song.snapshot.last_error, None);
}

#[test]
fn with_the_only_output_unplugged_a_pick_leaves_the_current_song_to_come_back() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song(second.path(), "Second");
    let fake = FakeOutput::new();
    let options = SessionOptions {
        reconnect_interval: Duration::ZERO,
        ..SessionOptions::default()
    };
    let mut current = Some(Song::open(first.path(), &options, Box::new(fake.clone())).unwrap());
    play(current.as_mut().unwrap(), &fake);
    fake.unplug(FAKE_DEVICE_ID);
    current.as_mut().unwrap().poll();

    let refused = replace(
        &mut current,
        second.path(),
        &options,
        || Box::new(fake.clone()),
        |_| panic!("nothing stops"),
    );
    assert!(
        matches!(refused, Err(OpenError::Output(OutputError::NoDevice))),
        "{refused:?}"
    );
    assert_eq!(fake.open_attempts(), 1, "only the first song's open");

    // The first song is still loaded, and plays on its device once it is
    // back.
    let song = current.as_mut().expect("the first song stays loaded");
    assert_eq!(song.session.song().title, "First");
    assert!(matches!(
        song.snapshot.device,
        DeviceStatus::Disconnected { .. }
    ));
    fake.plug(FAKE_DEVICE_ID);
    song.poll();
    assert!(
        matches!(song.snapshot.device, DeviceStatus::Connected(_)),
        "{:?}",
        song.snapshot.device
    );
}

/// A port without a check, so it cannot tell ([`app::OutputCheck::Unknown`]).
struct Unchecked(FakeOutput);

impl AudioOutput for Unchecked {
    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
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
fn a_device_that_cannot_tell_still_takes_the_pick() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song(second.path(), "Second");
    let device = FakeOutput::new();
    let mut current = Some(open_on(first.path(), &device).unwrap());
    play(current.as_mut().unwrap(), &device);

    let replaced = replace(
        &mut current,
        second.path(),
        &SessionOptions::default(),
        || Box::new(Unchecked(device.clone())),
        |_| {
            device.render(FRAMES);
        },
    )
    .expect("a check that cannot tell goes ahead");
    assert!(replaced.start_error.is_none(), "{:?}", replaced.start_error);
    assert_eq!(current.unwrap().session.song().title, "Second");
    assert_eq!(device.checks(), 0, "the wrapper has no check");
}

/// A device that passes the check but is busy with another stream when the
/// pick opens it: opening fails after the current song stopped.
fn busy_device(dir: &Path) -> (FakeOutput, Song) {
    let busy = FakeOutput::new();
    write_song(dir, "Elsewhere");
    let holder = open_on(dir, &busy).unwrap();
    (busy, holder)
}

#[test]
fn a_device_that_fails_to_open_after_the_check_gets_the_previous_song_back_stopped() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song(second.path(), "Second");
    let device = FakeOutput::new();
    let mut current = Some(open_on(first.path(), &device).unwrap());
    play(current.as_mut().unwrap(), &device);
    let (busy, _holder) = busy_device(other.path());

    // The pick is checked and opened on the busy device; the previous song
    // opens again on its own, which its close freed.
    let calls = Cell::new(0);
    let replaced = replace(
        &mut current,
        second.path(),
        &SessionOptions::default(),
        || {
            calls.set(calls.get() + 1);
            let output = if calls.get() == 1 { &busy } else { &device };
            Box::new(output.clone()) as Box<dyn AudioOutput>
        },
        |_| {
            device.render(FRAMES);
        },
    )
    .expect("the check lets the pick go ahead");

    assert_eq!(calls.get(), 2, "the pick, then the previous song again");
    assert_eq!(busy.checks(), 1);
    assert!(matches!(
        replaced.start_error,
        Some(OpenError::Output(OutputError::Device { .. }))
    ));
    assert!(replaced.reopen_error.is_none());
    let song = current.as_mut().expect("the first song is back");
    assert_eq!(song.session.song().title, "First");
    assert_eq!(song.snapshot.transport, TransportState::Stopped);
    assert_eq!(song.snapshot.output_frame, 0, "back at the start");
    let problems = replaced.problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].starts_with(&format!(
            "Could not start {} on the audio device: ",
            second.path().display()
        )),
        "{problems:?}"
    );
    assert_eq!(
        problems[1],
        "\"First\" is loaded again, stopped at the start."
    );
}

#[test]
fn when_the_previous_song_cannot_come_back_nothing_is_loaded_and_both_reasons_show() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song(second.path(), "Second");
    let fake = FakeOutput::new();
    let mut current = Some(open_on(first.path(), &fake).unwrap());
    play(current.as_mut().unwrap(), &fake);
    let (busy, _holder) = busy_device(other.path());

    let replaced = replace(
        &mut current,
        second.path(),
        &SessionOptions::default(),
        || Box::new(busy.clone()),
        |_| {
            fake.render(FRAMES);
        },
    )
    .unwrap();
    assert!(current.is_none());
    assert!(replaced.start_error.is_some() && replaced.reopen_error.is_some());
    let problems = replaced.problems();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[1].starts_with("\"First\" could not be loaded again: "),
        "{problems:?}"
    );
}

#[test]
fn closing_a_song_frees_its_device_and_names_it_in_its_problem() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let mut song = open_on(dir.path(), &fake).unwrap();
    play(&mut song, &fake);
    fill_the_queue(&mut song);

    let closed = close(song, |_| {});
    assert!(!fake.is_streaming(), "the device is free");
    assert_eq!(closed.title, "First");
    assert!(matches!(closed.closing, Closing::Refused(_)));
    let problem = closed.problem().expect("a refusal is reported");
    assert!(
        problem.starts_with("Closing \"First\": the player refused Stop"),
        "{problem}"
    );
}

#[test]
fn closing_stops_with_the_ramp() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let mut playing = open_on(dir.path(), &fake).unwrap();
    play(&mut playing, &fake);

    let mut heard = Vec::new();
    let closing = stop_and_wait(&mut playing.session, |wait| {
        assert_eq!(wait, STOP_POLL);
        heard.push(fake.render(FRAMES).expect("a stream plays"));
    });
    assert_eq!(closing, Closing::Stopped { refused: None });
    assert_eq!(closing.problem(), None);
    assert_eq!(playing.session.poll().transport, TransportState::Stopped);
    assert!(
        ramped_down(&heard, &step(&mut playing, &fake)),
        "a ramp, not a cut"
    );
}

#[test]
fn closing_sends_a_refused_stop_again_and_still_ramps_down() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let mut playing = open_on(dir.path(), &fake).unwrap();
    play(&mut playing, &fake);
    let full = fill_the_queue(&mut playing);
    assert!(matches!(full, RuntimeError::QueueFull { .. }), "{full:?}");

    // The audio thread takes the queue on its next callback, then Stop.
    let mut heard = Vec::new();
    let closing = stop_and_wait(&mut playing.session, |_| {
        heard.push(fake.render(FRAMES).expect("a stream plays"));
    });
    let Closing::Stopped {
        refused: Some(RuntimeError::QueueFull { command }),
    } = &closing
    else {
        panic!("Stop refused once, then taken: {closing:?}");
    };
    assert!(command.contains("Stop"), "{command}");
    assert_eq!(playing.session.poll().transport, TransportState::Stopped);
    assert!(heard.len() >= 2, "Stop waited for the queue to drain");
    assert!(
        ramped_down(&heard, &step(&mut playing, &fake)),
        "a ramp, not a cut"
    );
    let problem = closing.problem().expect("a refusal is reported");
    assert!(problem.contains("refused Stop"), "{problem}");
    assert!(problem.contains("stopped with its ramp"), "{problem}");
}

#[test]
fn closing_reports_a_stop_the_player_never_takes() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let mut playing = open_on(dir.path(), &fake).unwrap();
    play(&mut playing, &fake);
    fill_the_queue(&mut playing);

    // No callback runs, so the queue stays full for the whole wait.
    let mut waits = Duration::ZERO;
    let closing = stop_and_wait(&mut playing.session, |wait| waits += wait);
    assert!(
        matches!(&closing, Closing::Refused(RuntimeError::QueueFull { .. })),
        "{closing:?}"
    );
    assert_eq!(waits, STOP_WAIT, "the wait is bounded");
    let problem = closing.problem().expect("a refusal is reported");
    assert!(problem.contains("refused Stop"), "{problem}");
    assert!(problem.contains("without its ramp"), "{problem}");
}

#[test]
fn closing_reports_a_ramp_that_does_not_end_in_time() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let fake = FakeOutput::new();
    let mut playing = open_on(dir.path(), &fake).unwrap();
    play(&mut playing, &fake);

    // Stop is taken, but no callback runs to play the ramp.
    let closing = stop_and_wait(&mut playing.session, |_| {});
    assert_eq!(closing, Closing::RampUnfinished { refused: None });
    let problem = closing.problem().expect("an unfinished ramp is reported");
    assert!(problem.contains("ramp had not ended"), "{problem}");
}
