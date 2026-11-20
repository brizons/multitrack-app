//! A validated song: the only input [`crate::compile()`] accepts.

use crate::package::{Arrangement, SectionType, Stem, TempoPoint, TimeSignaturePoint};

/// A song whose package passed [`crate::validate`].
///
/// Fields are private so every `Song` upholds the package rules: maps start at
/// 0 and are strictly increasing, sections are sorted and end where the next
/// one starts, and every arrangement references existing sections.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    pub(crate) title: String,
    pub(crate) artist: Option<String>,
    pub(crate) key: Option<String>,
    pub(crate) sample_rate: u32,
    pub(crate) length: u64,
    pub(crate) tempo_map: Vec<TempoPoint>,
    pub(crate) time_signature_map: Vec<TimeSignaturePoint>,
    pub(crate) stems: Vec<Stem>,
    pub(crate) sections: Vec<Section>,
    pub(crate) arrangements: Vec<Arrangement>,
}

/// A section with its resolved end position, in source samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Section id, unique within the song.
    pub id: String,
    /// What the section is (verse, chorus, ...).
    pub section_type: SectionType,
    /// Optional number, such as the 2 in "Verse 2".
    pub number: Option<u32>,
    /// First sample of the section.
    pub start: u64,
    /// First sample after the section: the next section start or the song end.
    pub end: u64,
}

impl Section {
    /// Number of samples in the section.
    pub fn length(&self) -> u64 {
        self.end - self.start
    }
}

impl Song {
    /// Song title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Artist, when the package names one.
    pub fn artist(&self) -> Option<&str> {
        self.artist.as_deref()
    }

    /// Musical key, when the package names one.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// Sample rate shared by the package and every stem.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Song length in samples: the length of the longest stem.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Tempo map, starting at 0 and strictly increasing.
    pub fn tempo_map(&self) -> &[TempoPoint] {
        &self.tempo_map
    }

    /// Time-signature map, starting at 0 and strictly increasing.
    pub fn time_signature_map(&self) -> &[TimeSignaturePoint] {
        &self.time_signature_map
    }

    /// Stems in package order.
    pub fn stems(&self) -> &[Stem] {
        &self.stems
    }

    /// Sections sorted by start.
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// The section with this id.
    pub fn section(&self, id: &str) -> Option<&Section> {
        self.sections.iter().find(|section| section.id == id)
    }

    /// Arrangements in package order; one of them has id
    /// [`crate::package::DEFAULT_ARRANGEMENT_ID`].
    pub fn arrangements(&self) -> &[Arrangement] {
        &self.arrangements
    }

    /// The arrangement with this id.
    pub fn arrangement(&self, id: &str) -> Option<&Arrangement> {
        self.arrangements
            .iter()
            .find(|arrangement| arrangement.id == id)
    }
}
