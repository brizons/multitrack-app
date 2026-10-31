//! Arrangement compiler: a song, an arrangement, and options become a
//! [`RenderPlan`] the engine plays against a single sample counter.
//!
//! "`compile()` is a pure function": same input, same plan, no I/O.

use thiserror::Error;

use crate::package::SectionType;
use crate::song::{Section, Song};
use crate::timeline::Timeline;

/// Default length of the crossfade recorded at seams where the source jumps.
pub const DEFAULT_SEAM_CROSSFADE: u64 = 256;

/// When a guide cue plays relative to the section it announces (F4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CueTiming {
    /// One bar before the section starts, at the section's start tempo and meter.
    #[default]
    OneBarBefore,
    /// Exactly at the section start.
    AtSectionStart,
}

/// Settings that shape the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileOptions {
    /// Crossfade length, in samples, recorded on segments whose source position
    /// jumps. Default [`DEFAULT_SEAM_CROSSFADE`].
    pub seam_crossfade: u64,
    /// When cues play. Default [`CueTiming::OneBarBefore`].
    pub cue_timing: CueTiming,
    /// Length every cue reserves, in samples; `None` means one bar at the
    /// announced section's start tempo and meter.
    pub cue_length: Option<u64>,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            seam_crossfade: DEFAULT_SEAM_CROSSFADE,
            cue_timing: CueTiming::default(),
            cue_length: None,
        }
    }
}

/// What the engine plays. Every position is in output samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderPlan {
    /// Sample rate of the song and the output.
    pub sample_rate: u32,
    /// Output length: the sum of the segment lengths.
    pub length: u64,
    /// Contiguous source ranges in play order, starting at output sample 0.
    pub segments: Vec<Segment>,
    /// Click ticks, sorted by position, then level.
    pub clicks: Vec<ClickEvent>,
    /// Guide cues in play order; they never overlap.
    pub cues: Vec<CueEvent>,
}

/// One section played from the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Id of the section this segment plays.
    pub section_id: String,
    /// First output sample.
    pub out_start: u64,
    /// First source sample.
    pub src_start: u64,
    /// Number of samples.
    pub len: u64,
    /// Crossfade the renderer applies at the start of this segment, in samples;
    /// non-zero only where the source jumps (capped at `len`).
    pub crossfade_in: u64,
}

/// One click tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClickEvent {
    /// Output sample of the tick.
    pub at: u64,
    /// Which of the four independently mixed click levels plays it (F3.2).
    pub level: ClickLevel,
}

/// Click level. BPM counts the time-signature denominator note, so a "beat"
/// below is one denominator note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClickLevel {
    /// Beat 1 of a bar.
    Accent,
    /// Every other beat.
    Quarter,
    /// Halfway between two beats.
    Eighth,
    /// A quarter and three quarters of the way between two beats.
    Sixteenth,
}

/// One guide cue announcing a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueEvent {
    /// Output sample where the cue starts.
    pub at: u64,
    /// Id of the announced section.
    pub section_id: String,
    /// Type of the announced section.
    pub section_type: SectionType,
    /// Number of the announced section.
    pub number: Option<u32>,
}

/// Why a plan could not be compiled.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CompileError {
    /// The song has no arrangement with this id.
    #[error("unknown arrangement \"{id}\"")]
    UnknownArrangement {
        /// The requested id.
        id: String,
    },
    /// The arrangement lists no sections.
    #[error("arrangement \"{id}\" has no sections")]
    EmptyArrangement {
        /// The arrangement id.
        id: String,
    },
}

/// Compiles `song` with the arrangement `arrangement_id` into a render plan.
pub fn compile(
    song: &Song,
    arrangement_id: &str,
    options: &CompileOptions,
) -> Result<RenderPlan, CompileError> {
    let arrangement =
        song.arrangement(arrangement_id)
            .ok_or_else(|| CompileError::UnknownArrangement {
                id: arrangement_id.to_owned(),
            })?;
    if arrangement.sections.is_empty() {
        return Err(CompileError::EmptyArrangement {
            id: arrangement_id.to_owned(),
        });
    }
    // `validate` guarantees every reference resolves, so nothing is skipped.
    let sections: Vec<&Section> = arrangement
        .sections
        .iter()
        .filter_map(|id| song.section(id))
        .collect();
    let segments = segments(&sections, options.seam_crossfade);
    let timeline = Timeline::new(song);
    let clicks = clicks(&timeline, &segments);
    let cues = cues(&timeline, &sections, &segments, options);
    let length = segments
        .last()
        .map_or(0, |last| last.out_start.saturating_add(last.len));
    Ok(RenderPlan {
        sample_rate: song.sample_rate(),
        length,
        segments,
        clicks,
        cues,
    })
}

/// Lays the sections end to end from output sample 0.
fn segments(sections: &[&Section], seam_crossfade: u64) -> Vec<Segment> {
    let mut segments = Vec::with_capacity(sections.len());
    let mut out_start = 0u64;
    let mut previous_src_end = None;
    for section in sections {
        let len = section.length();
        let jumps = previous_src_end.is_some_and(|end| end != section.start);
        segments.push(Segment {
            section_id: section.id.clone(),
            out_start,
            src_start: section.start,
            len,
            crossfade_in: if jumps { seam_crossfade.min(len) } else { 0 },
        });
        out_start = out_start.saturating_add(len);
        previous_src_end = Some(section.end);
    }
    segments
}

/// Maps the source ticks inside each segment to output positions, so clicks
/// stay aligned with the audio after rearranging.
fn clicks(timeline: &Timeline<'_>, segments: &[Segment]) -> Vec<ClickEvent> {
    let ticks = timeline.ticks();
    let mut clicks = Vec::with_capacity(ticks.len());
    for segment in segments {
        let src_end = segment.src_start + segment.len;
        let first = ticks.partition_point(|tick| tick.at < segment.src_start);
        clicks.extend(
            ticks[first..]
                .iter()
                .take_while(|tick| tick.at < src_end)
                .map(|tick| ClickEvent {
                    at: segment
                        .out_start
                        .saturating_add(tick.at - segment.src_start),
                    level: tick.level,
                }),
        );
    }
    clicks.sort_by_key(|click| (click.at, click.level));
    clicks
}

/// One cue per segment, announcing its section; cues that would start before
/// 0, announce a section shorter than the cue, or start before the previous
/// kept cue ends are left out.
fn cues(
    timeline: &Timeline<'_>,
    sections: &[&Section],
    segments: &[Segment],
    options: &CompileOptions,
) -> Vec<CueEvent> {
    let mut cues = Vec::new();
    let mut previous_end = 0u64;
    for (section, segment) in sections.iter().zip(segments) {
        let bar = timeline.bar_length_at(segment.src_start);
        let cue_length = options.cue_length.unwrap_or(bar);
        if segment.len < cue_length {
            continue;
        }
        let at = match options.cue_timing {
            CueTiming::AtSectionStart => segment.out_start,
            CueTiming::OneBarBefore => match segment.out_start.checked_sub(bar) {
                Some(at) => at,
                None => continue,
            },
        };
        if at < previous_end {
            continue;
        }
        previous_end = at.saturating_add(cue_length);
        cues.push(CueEvent {
            at,
            section_id: section.id.clone(),
            section_type: section.section_type,
            number: section.number,
        });
    }
    cues
}
