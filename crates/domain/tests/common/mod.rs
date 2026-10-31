//! Shared fixtures for the domain integration tests.
#![allow(dead_code)]

use domain::{
    Arrangement, SectionMarker, SectionType, Song, SongPackage, Stem, StemInfo, StemRole,
    TempoPoint, TimeSignaturePoint, validate,
};

pub const RATE: u32 = 48_000;

/// The example manifest from `docs/mvp-spec.md`, "Song package format v1".
pub const SPEC_EXAMPLE: &str = r#"{
  "formatVersion": 1,
  "title": "Example Song",
  "artist": "Example Artist",
  "sampleRate": 48000,
  "key": "G",
  "tempoMap": [{ "at": 0, "bpm": 72.0 }],
  "timeSignatureMap": [{ "at": 0, "numerator": 4, "denominator": 4 }],
  "stems": [
    { "id": "drums", "file": "stems/drums.flac", "part": "drums", "role": "track" },
    { "id": "click", "file": "stems/click.flac", "part": "click", "role": "click" }
  ],
  "sections": [
    { "id": "s1", "type": "intro", "number": null, "start": 0 },
    { "id": "s2", "type": "verse", "number": 1, "start": 640000 }
  ],
  "arrangements": [
    { "id": "default", "name": "Default", "sections": ["s1", "s2"], "infiniteLoop": [] }
  ]
}"#;

/// Length given to every stem of the spec example.
pub const SPEC_LENGTH: u64 = 1_280_000;

pub fn spec_package() -> SongPackage {
    SongPackage::from_json(SPEC_EXAMPLE).expect("spec example parses")
}

pub fn info(id: &str, sample_rate: u32, length_samples: u64) -> StemInfo {
    StemInfo {
        id: id.to_owned(),
        sample_rate,
        length_samples,
    }
}

pub fn spec_infos() -> Vec<StemInfo> {
    vec![
        info("drums", RATE, SPEC_LENGTH),
        info("click", RATE, SPEC_LENGTH),
    ]
}

pub fn stem(id: &str, role: StemRole) -> Stem {
    Stem {
        id: id.to_owned(),
        file: format!("stems/{id}.wav"),
        part: id.to_owned(),
        role,
    }
}

pub fn marker(
    id: &str,
    section_type: SectionType,
    number: Option<u32>,
    start: u64,
) -> SectionMarker {
    SectionMarker {
        id: id.to_owned(),
        section_type,
        number,
        start,
    }
}

pub fn arrangement(id: &str, sections: &[&str]) -> Arrangement {
    Arrangement {
        id: id.to_owned(),
        name: id.to_owned(),
        sections: sections.iter().map(|s| (*s).to_owned()).collect(),
        infinite_loop: Vec::new(),
    }
}

/// A song description for compile tests; `build` validates it.
pub struct SongSpec {
    pub length: u64,
    pub tempo: Vec<(u64, f64)>,
    pub meters: Vec<(u64, u32, u32)>,
    pub sections: Vec<SectionMarker>,
    pub arrangements: Vec<Arrangement>,
}

impl SongSpec {
    /// 120 bpm in 4/4 at 48 kHz: one beat is 24 000 samples, one bar 96 000.
    pub fn new(length: u64, sections: Vec<SectionMarker>) -> Self {
        let ids: Vec<String> = sections.iter().map(|s| s.id.clone()).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        Self {
            length,
            tempo: vec![(0, 120.0)],
            meters: vec![(0, 4, 4)],
            arrangements: vec![arrangement("default", &ids)],
            sections,
        }
    }

    pub fn tempo(mut self, tempo: &[(u64, f64)]) -> Self {
        self.tempo = tempo.to_vec();
        self
    }

    pub fn meters(mut self, meters: &[(u64, u32, u32)]) -> Self {
        self.meters = meters.to_vec();
        self
    }

    pub fn arrangement(mut self, id: &str, sections: &[&str]) -> Self {
        self.arrangements.push(arrangement(id, sections));
        self
    }

    pub fn build(self) -> Song {
        let package = SongPackage {
            format_version: 1,
            title: "Test Song".to_owned(),
            artist: None,
            sample_rate: RATE,
            key: None,
            tempo_map: self
                .tempo
                .iter()
                .map(|&(at, bpm)| TempoPoint { at, bpm })
                .collect(),
            time_signature_map: self
                .meters
                .iter()
                .map(|&(at, numerator, denominator)| TimeSignaturePoint {
                    at,
                    numerator,
                    denominator,
                })
                .collect(),
            stems: vec![stem("drums", StemRole::Track)],
            sections: self.sections,
            arrangements: self.arrangements,
        };
        validate(&package, &[info("drums", RATE, self.length)])
            .expect("test song is valid")
            .song
    }
}

/// Intro (2 bars), Verse 1 (4 bars), Chorus (4 bars), Outro (2 bars) at
/// 120 bpm 4/4: the song is 1 152 000 samples long.
pub fn four_section_spec() -> SongSpec {
    SongSpec::new(
        1_152_000,
        vec![
            marker("intro", SectionType::Intro, None, 0),
            marker("verse", SectionType::Verse, Some(1), 192_000),
            marker("chorus", SectionType::Chorus, None, 576_000),
            marker("outro", SectionType::Outro, None, 960_000),
        ],
    )
}
