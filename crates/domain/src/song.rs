//! A validated song: the only input [`crate::compile()`] accepts, plus the
//! songs a setlist item plays ([`Song::excerpt`], [`Song::click_only`]).

use crate::package::{
    Arrangement, DEFAULT_ARRANGEMENT_ID, MAX_SONG_HOURS, SectionType, Stem, TempoPoint,
    TimeSignaturePoint, bpm_in_range, denominator_valid, numerator_valid,
};
use crate::setlist::TimeSignature;

/// Id of the one section of a click-only tile's song ([`Song::click_only`]).
pub const CLICK_SECTION_ID: &str = "click";

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

    /// The song as a setlist item with start and end points plays it: its
    /// only arrangement is `arrangement` cut to positions `first..=last`
    /// (counted from 0, both inclusive), under the same id; everything else
    /// is unchanged. `None` when the song has no such arrangement or the
    /// range is not inside it. The players need no seek: they play the
    /// excerpt from its first position as they play any arrangement.
    pub fn excerpt(&self, arrangement: &str, first: usize, last: usize) -> Option<Song> {
        let source = self.arrangement(arrangement)?;
        if first > last || last >= source.sections.len() {
            return None;
        }
        let sections = source.sections[first..=last].to_vec();
        let infinite_loop = source
            .infinite_loop
            .iter()
            .filter(|id| sections.contains(id))
            .cloned()
            .collect();
        let excerpt = Arrangement {
            id: source.id.clone(),
            name: source.name.clone(),
            sections,
            infinite_loop,
        };
        Some(Song {
            arrangements: vec![excerpt],
            ..self.clone()
        })
    }

    /// The song a click-only tile plays (F5.5): no stems, so the players
    /// generate the click at `bpm` (counting the denominator note) in
    /// `time_signature`. It is one section, [`CLICK_SECTION_ID`] of type
    /// [`SectionType::CountOff`], in the arrangement
    /// [`crate::DEFAULT_ARRANGEMENT_ID`]: `bars` bars long, or one bar
    /// without a bar count, which the player repeats until the operator moves
    /// on (the section is listed as the arrangement's infinite loop).
    ///
    /// `None` when the tile breaks a setlist rule (tempo, meter, zero bars),
    /// the rate is 0, the tile is shorter than one sample, or it is longer
    /// than [`crate::MAX_SONG_HOURS`].
    pub fn click_only(
        sample_rate: u32,
        bpm: f64,
        time_signature: TimeSignature,
        bars: Option<u32>,
    ) -> Option<Song> {
        let TimeSignature {
            numerator,
            denominator,
        } = time_signature;
        if sample_rate == 0
            || !bpm_in_range(bpm)
            || !numerator_valid(numerator)
            || !denominator_valid(denominator)
            || bars == Some(0)
        {
            return None;
        }
        // Rounded once from the start, as the timeline places a bar line:
        // the tile ends exactly where its next downbeat would be.
        let samples_per_beat = f64::from(sample_rate) * 60.0 / bpm;
        let beats = f64::from(bars.unwrap_or(1)) * f64::from(numerator);
        let length = (beats * samples_per_beat).round() as u64;
        let max = MAX_SONG_HOURS * 3_600 * u64::from(sample_rate);
        if length == 0 || length > max {
            return None;
        }
        let looped = if bars.is_none() {
            vec![CLICK_SECTION_ID.to_owned()]
        } else {
            Vec::new()
        };
        Some(Song {
            title: "Click".to_owned(),
            artist: None,
            key: None,
            sample_rate,
            length,
            tempo_map: vec![TempoPoint { at: 0, bpm }],
            time_signature_map: vec![TimeSignaturePoint {
                at: 0,
                numerator,
                denominator,
            }],
            stems: Vec::new(),
            sections: vec![Section {
                id: CLICK_SECTION_ID.to_owned(),
                section_type: SectionType::CountOff,
                number: None,
                start: 0,
                end: length,
            }],
            arrangements: vec![Arrangement {
                id: DEFAULT_ARRANGEMENT_ID.to_owned(),
                name: "Default".to_owned(),
                sections: vec![CLICK_SECTION_ID.to_owned()],
                infinite_loop: looped,
            }],
        })
    }
}
