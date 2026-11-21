//! What a shell shows about the open song.

use domain::{BeatGrid, Song};

/// One position of the arrangement being played: a section in play order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionInfo {
    /// Section id, as in the song package.
    pub section_id: String,
    /// Display name: the section type and its number, such as "Verse 2".
    pub label: String,
    /// First source frame of the section.
    pub start_frame: u64,
    /// Frames in the section.
    pub length_frames: u64,
    /// Bar the section starts in, counted from 1.
    pub start_bar: u64,
}

/// The open song and the arrangement a session plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongInfo {
    /// Song title.
    pub title: String,
    /// Artist, when the package names one.
    pub artist: Option<String>,
    /// Frames per second of the song and of the output.
    pub sample_rate: u32,
    /// Id of the arrangement being played.
    pub arrangement: String,
    /// The arrangement's positions, in play order; [`crate::Command::QueueJump`]
    /// takes an index into it.
    pub positions: Vec<PositionInfo>,
    /// Findings in the package that do not block playback.
    pub warnings: Vec<String>,
}

impl SongInfo {
    /// Describes arrangement `arrangement` of `song`; `None` when the song
    /// has no such arrangement.
    pub(crate) fn new(song: &Song, arrangement: &str, warnings: Vec<String>) -> Option<Self> {
        let grid = BeatGrid::new(song);
        let positions = song
            .arrangement(arrangement)?
            .sections
            .iter()
            .filter_map(|id| song.section(id))
            .map(|section| PositionInfo {
                section_id: section.id.clone(),
                label: section.section_type.label(section.number),
                start_frame: section.start,
                length_frames: section.length(),
                start_bar: grid.bar_and_beat(section.start).bar,
            })
            .collect();
        Some(Self {
            title: song.title().to_owned(),
            artist: song.artist().map(str::to_owned),
            sample_rate: song.sample_rate(),
            arrangement: arrangement.to_owned(),
            positions,
            warnings,
        })
    }

    /// Label of arrangement position `position`, or an empty string past
    /// the end.
    pub(crate) fn label(&self, position: usize) -> &str {
        self.positions
            .get(position)
            .map_or("", |position| position.label.as_str())
    }
}
