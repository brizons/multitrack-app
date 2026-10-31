//! Shared fixtures for the engine integration tests: songs are built in
//! memory, validated, and compiled with the `domain` crate.
#![allow(dead_code)]

use domain::{
    Arrangement, CompileOptions, RenderPlan, SectionMarker, SectionType, Song, SongPackage, Stem,
    StemInfo, StemRole, TempoPoint, TimeSignaturePoint, compile, validate,
};
use engine::StemAudio;

/// A song description; [`SongSpec::build`] validates it.
pub struct SongSpec {
    pub rate: u32,
    pub length: u64,
    pub bpm: f64,
    pub sections: Vec<(&'static str, SectionType, u64)>,
    pub arrangements: Vec<(&'static str, Vec<&'static str>)>,
    pub stems: Vec<(&'static str, StemRole)>,
}

impl SongSpec {
    /// 120 bpm in 4/4; the Default arrangement plays every section in order.
    pub fn new(
        rate: u32,
        length: u64,
        sections: Vec<(&'static str, SectionType, u64)>,
        stems: Vec<(&'static str, StemRole)>,
    ) -> Self {
        let ids = sections.iter().map(|section| section.0).collect();
        Self {
            rate,
            length,
            bpm: 120.0,
            sections,
            arrangements: vec![("default", ids)],
            stems,
        }
    }

    pub fn bpm(mut self, bpm: f64) -> Self {
        self.bpm = bpm;
        self
    }

    pub fn arrangement(mut self, id: &'static str, sections: &[&'static str]) -> Self {
        self.arrangements.push((id, sections.to_vec()));
        self
    }

    /// Validates the package; every stem is `length` frames long.
    pub fn build(&self) -> Song {
        let package = SongPackage {
            format_version: 1,
            title: "Engine Test".to_owned(),
            artist: None,
            sample_rate: self.rate,
            key: None,
            tempo_map: vec![TempoPoint {
                at: 0,
                bpm: self.bpm,
            }],
            time_signature_map: vec![TimeSignaturePoint {
                at: 0,
                numerator: 4,
                denominator: 4,
            }],
            stems: self
                .stems
                .iter()
                .map(|&(id, role)| Stem {
                    id: id.to_owned(),
                    file: format!("stems/{id}.wav"),
                    part: id.to_owned(),
                    role,
                })
                .collect(),
            sections: self
                .sections
                .iter()
                .map(|&(id, section_type, start)| SectionMarker {
                    id: id.to_owned(),
                    section_type,
                    number: None,
                    start,
                })
                .collect(),
            arrangements: self
                .arrangements
                .iter()
                .map(|(id, sections)| Arrangement {
                    id: (*id).to_owned(),
                    name: (*id).to_owned(),
                    sections: sections.iter().map(|s| (*s).to_owned()).collect(),
                    infinite_loop: Vec::new(),
                })
                .collect(),
        };
        let infos: Vec<StemInfo> = self
            .stems
            .iter()
            .map(|&(id, _)| StemInfo {
                id: id.to_owned(),
                sample_rate: self.rate,
                length_samples: self.length,
            })
            .collect();
        validate(&package, &infos).expect("test song is valid").song
    }
}

pub fn plan(song: &Song, arrangement: &str) -> RenderPlan {
    compile(song, arrangement, &CompileOptions::default()).expect("arrangement compiles")
}

/// Intro (2 bars), Verse (4 bars), Chorus (4 bars), Outro (2 bars) at 120 bpm
/// in 4/4 at 48 kHz, plus the F2.3 arrangement `Intro, Verse, Chorus, Chorus,
/// Outro` with id `f23`.
pub fn four_sections(stems: Vec<(&'static str, StemRole)>) -> SongSpec {
    SongSpec::new(
        48_000,
        1_152_000,
        vec![
            ("intro", SectionType::Intro, 0),
            ("verse", SectionType::Verse, 192_000),
            ("chorus", SectionType::Chorus, 576_000),
            ("outro", SectionType::Outro, 960_000),
        ],
        stems,
    )
    .arrangement("f23", &["intro", "verse", "chorus", "chorus", "outro"])
}

pub fn mono(id: &str, role: StemRole, samples: Vec<f32>) -> StemAudio {
    StemAudio {
        id: id.to_owned(),
        role,
        channels: 1,
        frames: samples.len() as u64,
        samples,
    }
}

/// A stereo stem from interleaved samples.
pub fn stereo(id: &str, role: StemRole, samples: Vec<f32>) -> StemAudio {
    StemAudio {
        id: id.to_owned(),
        role,
        channels: 2,
        frames: samples.len() as u64 / 2,
        samples,
    }
}

/// Deterministic noise in [-0.5, 0.5) from a linear congruential generator.
pub fn noise(seed: u64, count: usize) -> Vec<f32> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 40) as f32 / (1u64 << 24) as f32) - 0.5
        })
        .collect()
}

pub fn sine(frequency: f64, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            (0.5 * (std::f64::consts::TAU * frequency * n as f64 / f64::from(rate)).sin()) as f32
        })
        .collect()
}

/// Frames where the left channel turns non-zero after silence: the onsets of
/// the generated click bursts in a render whose stems are silent.
pub fn onsets(output: &[f32]) -> Vec<u64> {
    let left: Vec<f32> = output.iter().step_by(2).copied().collect();
    (0..left.len())
        .filter(|&frame| left[frame] != 0.0 && (frame == 0 || left[frame - 1] == 0.0))
        .map(|frame| frame as u64)
        .collect()
}

pub fn bits(samples: &[f32]) -> Vec<u32> {
    samples.iter().map(|sample| sample.to_bits()).collect()
}
