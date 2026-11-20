//! Song folders and helpers shared by the session tests.
#![allow(dead_code)]

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use app::testing::FakeOutput;
use app::{Command, Session, SessionOptions, Snapshot};
use domain::{
    Arrangement, CompileOptions, SectionMarker, SectionType, SongPackage, Stem, StemRole,
    TempoPoint, TimeSignaturePoint, compile,
};
use engine::{MixSettings, render_offline};
use hound::{SampleFormat, WavSpec, WavWriter};
use library::{DiskMonitor, Gate, load_song};

/// The usual song rate: 120 bpm in 4/4, so a bar is two seconds.
pub const RATE: u32 = 8_000;
pub const BAR: u64 = 2 * RATE as u64;
/// Positions of every arrangement: intro, verse, chorus, outro.
pub const POS_CHORUS: usize = 2;
pub const POS_OUTRO: usize = 3;
pub const IDLE: Duration = Duration::from_secs(10);

/// Writes a song at `rate` into `dir`: intro (2 bars), verse 1 (2 bars),
/// chorus (2 bars), outro (1 bar); a mono 16-bit drums stem, a stereo float
/// keys stem, and a mono 16-bit guide. The `default` arrangement plays the
/// sections in order; `jumpy` plays intro, chorus, verse, outro, so every
/// seam jumps in the source.
pub fn write_song(dir: &Path, rate: u32) {
    let bar = 2 * u64::from(rate);
    let length = 7 * bar;
    let section = |id: &str, section_type, number, start| SectionMarker {
        id: id.to_owned(),
        section_type,
        number,
        start,
    };
    let arrangement = |id: &str, sections: &[&str]| Arrangement {
        id: id.to_owned(),
        name: id.to_owned(),
        sections: sections.iter().map(|s| (*s).to_owned()).collect(),
        infinite_loop: Vec::new(),
    };
    let stem = |id: &str, role| Stem {
        id: id.to_owned(),
        file: format!("stems/{id}.wav"),
        part: id.to_owned(),
        role,
    };
    let package = SongPackage {
        format_version: 1,
        title: "Session Test".to_owned(),
        artist: Some("The Testers".to_owned()),
        sample_rate: rate,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: vec![
            stem("drums", StemRole::Track),
            stem("keys", StemRole::Track),
            stem("guide", StemRole::Guide),
        ],
        sections: vec![
            section("intro", SectionType::Intro, None, 0),
            section("verse", SectionType::Verse, Some(1), 2 * bar),
            section("chorus", SectionType::Chorus, None, 4 * bar),
            section("outro", SectionType::Outro, None, 6 * bar),
        ],
        arrangements: vec![
            arrangement("default", &["intro", "verse", "chorus", "outro"]),
            arrangement("jumpy", &["intro", "chorus", "verse", "outro"]),
        ],
    };
    fs::create_dir_all(dir.join("stems")).unwrap();
    fs::write(dir.join("song.json"), package.to_json().unwrap()).unwrap();
    let frames = length as usize;
    write_wav(&dir.join("stems/drums.wav"), 1, rate, &noise(1, frames));
    write_float(&dir.join("stems/keys.wav"), rate, &noise(2, 2 * frames));
    write_wav(&dir.join("stems/guide.wav"), 1, rate, &noise(3, frames));
}

fn noise(seed: u64, count: usize) -> Vec<i16> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 48) as i16
        })
        .collect()
}

fn write_wav(path: &Path, channels: u16, rate: u32, samples: &[i16]) {
    let spec = WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(sample).unwrap();
    }
    writer.finalize().unwrap();
}

fn write_float(path: &Path, rate: u32, samples: &[i16]) {
    let spec = WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).unwrap();
    for &sample in samples {
        writer.write_sample(f32::from(sample) / 65_536.0).unwrap();
    }
    writer.finalize().unwrap();
}

/// The offline render of `arrangement` with `seam_crossfade`, as the live
/// session should play it.
pub fn offline(dir: &Path, arrangement: &str, seam_crossfade: u64) -> Vec<f32> {
    let loaded = load_song(dir).unwrap();
    let options = CompileOptions {
        seam_crossfade,
        ..CompileOptions::default()
    };
    let plan = compile(&loaded.song, arrangement, &options).unwrap();
    render_offline(&plan, &loaded.stems, &MixSettings::default()).unwrap()
}

pub fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}

pub fn is_silent(samples: &[f32]) -> bool {
    samples.iter().all(|&sample| sample == 0.0)
}

/// A gate that holds back every block until the returned flag is set.
pub fn closed_gate() -> (Gate, Arc<AtomicBool>) {
    let open = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&open);
    let gate: Gate = Arc::new(move |_: &str, _| flag.load(Ordering::SeqCst));
    (gate, open)
}

/// A session on `fake` with `options`.
pub fn open(dir: &Path, fake: &FakeOutput, options: SessionOptions) -> Session {
    Session::open(dir, Box::new(fake.clone()), options).expect("the session opens")
}

/// One audio callback of `frames` frames once the disk has served every
/// request it may serve, then a poll.
pub fn step(
    session: &mut Session,
    fake: &FakeOutput,
    monitor: &DiskMonitor,
    frames: usize,
) -> (Vec<f32>, Snapshot) {
    assert!(monitor.wait_idle(IDLE), "the disk thread keeps up");
    let out = fake.render(frames).expect("a stream plays");
    (out, session.poll())
}

/// Sends play and runs callbacks until the transport has started; returns
/// the audio of the first callback that played. The callbacks before it
/// must be silent.
pub fn start(session: &mut Session, fake: &FakeOutput, frames: usize) -> (Vec<f32>, Snapshot) {
    let monitor = session.disk_monitor();
    session.send(Command::Play).expect("play is sent");
    for _ in 0..100 {
        let (out, snapshot) = step(session, fake, &monitor, frames);
        if snapshot.output_frame > 0 {
            return (out, snapshot);
        }
        assert!(is_silent(&out), "nothing plays before the transport starts");
    }
    panic!("the transport never started");
}

/// Runs callbacks of `frames` frames until `total` frames played; returns
/// their audio and the last snapshot.
pub fn play_for(
    session: &mut Session,
    fake: &FakeOutput,
    frames: usize,
    total: usize,
) -> (Vec<f32>, Snapshot) {
    let monitor = session.disk_monitor();
    let mut audio = Vec::with_capacity(2 * total);
    let mut last = session.poll();
    while audio.len() < 2 * total {
        let (out, snapshot) = step(session, fake, &monitor, frames);
        audio.extend_from_slice(&out);
        last = snapshot;
    }
    (audio, last)
}
