//! S8: opening another song never ends the current one unless the new one
//! opened, and closing a song always ramps down, even when the player
//! refuses Stop at first. A real `Session` on the app's `FakeOutput`.

use std::fs;
use std::path::Path;
use std::time::Duration;

use app::testing::FakeOutput;
use app::{Command, JumpTiming, OpenError, RuntimeError, Session, SessionOptions, TransportState};
use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use hound::{SampleFormat, WavSpec, WavWriter};
use multitrack_desktop::song::{Closing, STOP_POLL, STOP_WAIT, open_in_place, stop_and_wait};

const RATE: u32 = 8_000;
/// 120 bpm in 4/4: a bar is two seconds.
const BAR: u64 = 2 * RATE as u64;
const FRAMES: usize = 256;

/// `title`: intro and verse, one bar each, one mono track stem that is
/// never silent.
fn write_song(dir: &Path, title: &str) {
    let section = |id: &str, section_type, bar: u64| SectionMarker {
        id: id.to_owned(),
        section_type,
        number: None,
        start: bar * BAR,
    };
    let package = SongPackage {
        format_version: 1,
        title: title.to_owned(),
        artist: None,
        sample_rate: RATE,
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
        sample_rate: RATE,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(dir.join("stems/keys.wav"), spec).unwrap();
    for frame in 0..2 * BAR {
        writer
            .write_sample(2_000 + (frame % 200) as i16 * 50)
            .unwrap();
    }
    writer.finalize().unwrap();
}

/// A song playing on its own fake output.
struct Playing {
    session: Session,
    fake: FakeOutput,
}

impl Playing {
    fn open(dir: &Path) -> Result<Self, OpenError> {
        let fake = FakeOutput::new();
        let session = Session::open(dir, Box::new(fake.clone()), SessionOptions::default())?;
        Ok(Self { session, fake })
    }

    fn step(&mut self) -> Vec<f32> {
        assert!(
            self.session
                .disk_monitor()
                .wait_idle(Duration::from_secs(10))
        );
        self.fake.render(FRAMES).expect("a stream plays")
    }

    fn play(&mut self) {
        self.session.send(Command::Play).unwrap();
        for _ in 0..200 {
            self.step();
            if self.session.poll().transport == TransportState::Playing {
                return;
            }
        }
        panic!("the song never started");
    }

    fn playhead(&mut self) -> u64 {
        self.session.poll().output_frame
    }

    /// Sends commands nobody takes until the player's queue refuses one.
    fn fill_the_queue(&mut self) -> RuntimeError {
        for _ in 0..10_000 {
            if let Err(error) = self
                .session
                .send(Command::SetJumpTiming(JumpTiming::NextBar))
            {
                return error;
            }
        }
        panic!("the command queue never filled");
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
    let mut current = Some(Playing::open(dir.path()).unwrap());
    current.as_mut().unwrap().play();

    let missing = dir.path().join("no-such-song");
    let refused = open_in_place(&mut current, || Playing::open(&missing));
    assert!(refused.is_err(), "a missing folder does not open");

    // The first song is still there, and still plays.
    let playing = current.as_mut().expect("the current song stays loaded");
    assert_eq!(playing.session.song().title, "First");
    let before = playing.playhead();
    let audio = playing.step();
    assert_eq!(playing.session.poll().transport, TransportState::Playing);
    assert!(playing.playhead() > before, "the playhead moves on");
    assert!(peak(&audio) > 0.01, "the song is still heard");
}

#[test]
fn a_pick_that_opens_replaces_the_current_song_and_hands_it_back() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_song(first.path(), "First");
    write_song(second.path(), "Second");
    let mut current = Some(Playing::open(first.path()).unwrap());
    current.as_mut().unwrap().play();

    let replaced = open_in_place(&mut current, || Playing::open(second.path()))
        .unwrap_or_else(|error| panic!("the second song opens: {error}"))
        .expect("the first song comes back to be stopped");
    assert_eq!(current.as_ref().unwrap().session.song().title, "Second");
    assert_eq!(replaced.session.song().title, "First");

    // With nothing loaded, the pick simply loads.
    let mut empty: Option<Playing> = None;
    let none = open_in_place(&mut empty, || Playing::open(first.path())).unwrap();
    assert!(none.is_none() && empty.is_some());
}

#[test]
fn opening_runs_the_opener_before_touching_the_current_song() {
    let mut current = Some("first");
    let failed: Result<Option<&str>, &str> = open_in_place(&mut current, || Err("invalid"));
    assert_eq!(failed, Err("invalid"));
    assert_eq!(current, Some("first"));
    assert_eq!(
        open_in_place(&mut current, || Ok::<_, &str>("second")),
        Ok(Some("first"))
    );
    assert_eq!(current, Some("second"));
}

#[test]
fn closing_stops_with_the_ramp() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let mut playing = Playing::open(dir.path()).unwrap();
    playing.play();

    let fake = playing.fake.clone();
    let mut heard = Vec::new();
    let closing = stop_and_wait(&mut playing.session, |wait| {
        assert_eq!(wait, STOP_POLL);
        heard.push(fake.render(FRAMES).expect("a stream plays"));
    });
    assert_eq!(closing, Closing::Stopped { refused: None });
    assert_eq!(closing.problem(), None);
    assert_eq!(playing.session.poll().transport, TransportState::Stopped);
    assert!(ramped_down(&heard, &playing.step()), "a ramp, not a cut");
}

#[test]
fn closing_sends_a_refused_stop_again_and_still_ramps_down() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let mut playing = Playing::open(dir.path()).unwrap();
    playing.play();
    let full = playing.fill_the_queue();
    assert!(matches!(full, RuntimeError::QueueFull { .. }), "{full:?}");

    // The audio thread takes the queue on its next callback, then Stop.
    let fake = playing.fake.clone();
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
    assert!(ramped_down(&heard, &playing.step()), "a ramp, not a cut");
    let problem = closing.problem().expect("a refusal is reported");
    assert!(problem.contains("refused Stop"), "{problem}");
    assert!(problem.contains("stopped with its ramp"), "{problem}");
}

#[test]
fn closing_reports_a_stop_the_player_never_takes() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), "First");
    let mut playing = Playing::open(dir.path()).unwrap();
    playing.play();
    playing.fill_the_queue();

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
    let mut playing = Playing::open(dir.path()).unwrap();
    playing.play();

    // Stop is taken, but no callback runs to play the ramp.
    let closing = stop_and_wait(&mut playing.session, |_| {});
    assert_eq!(closing, Closing::RampUnfinished { refused: None });
    let problem = closing.problem().expect("an unfinished ramp is reported");
    assert!(problem.contains("ramp had not ended"), "{problem}");
}
