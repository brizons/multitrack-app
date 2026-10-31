//! Arrangement compiler: a song, an arrangement, and options become a
//! [`RenderPlan`] the engine plays against a single sample counter.
//!
//! "`compile()` is a pure function": same input, same plan, no I/O.

use thiserror::Error;

use crate::grid::BeatGrid;
use crate::package::SectionType;
use crate::song::{Section, Song};
use crate::timeline::Timeline;

/// Default length of the crossfade recorded at seams where the source jumps.
pub const DEFAULT_SEAM_CROSSFADE: u64 = 256;

/// When a guide cue plays relative to the section it announces (F4.2).
///
/// "One bar" is the bar the musician hears just before the section in the
/// output: one bar of the preceding segment, at the tempo and meter of its last
/// source sample. The first segment has no preceding bar and uses its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CueTiming {
    /// One bar before the section starts.
    #[default]
    OneBarBefore,
    /// Exactly at the section start.
    AtSectionStart,
}

/// Settings that shape the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompileOptions {
    /// Crossfade length, in samples, at seams where the source jumps (see
    /// [`Segment`]). Default [`DEFAULT_SEAM_CROSSFADE`].
    pub seam_crossfade: u64,
    /// When cues play. Default [`CueTiming::OneBarBefore`].
    pub cue_timing: CueTiming,
    /// Length every cue reserves, in samples; `None` means one bar, measured
    /// as in [`CueTiming`].
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
///
/// Segments are contiguous: each `out_start` is the previous `out_start + len`.
/// At a seam where the source jumps (the incoming `src_start` is not the
/// outgoing `src_start + len`), the incoming segment has `fade_in = n` and the
/// outgoing segment has `fade_out = n`. During the first `n` output samples of
/// the incoming segment the outgoing segment keeps reading its source past its
/// end, `src_start + len .. src_start + len + n`, fading out while the incoming
/// one fades in (equal power). `n` is [`CompileOptions::seam_crossfade`] capped
/// by the incoming `len` and by the source audio left after the outgoing end.
/// Contiguous seams have both fades at 0, as do the first `fade_in` and the
/// last `fade_out`.
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
    /// Samples at the start of this segment that fade in over the previous
    /// segment's fade-out; 0 unless the source jumps here.
    pub fade_in: u64,
    /// Samples this segment keeps playing past its end, fading out under the
    /// next segment's fade-in; equal to the next segment's `fade_in`.
    pub fade_out: u64,
}

/// One click tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClickEvent {
    /// Sample of the tick: an output sample in a [`RenderPlan`], a source
    /// sample in [`crate::BeatGrid::clicks_in`].
    pub at: u64,
    /// Which of the four independently mixed click levels plays it (F3.2).
    pub level: ClickLevel,
}

/// Click level, relative to the beat. BPM counts the time-signature
/// denominator note, so a beat is one denominator note: the levels are the bar
/// start, the other beats, the half-beats, and the quarter-beats. In 4/4 they
/// are the accent, quarter, eighth, and sixteenth notes of F3.2; in 6/8 a
/// `Beat` tick falls on every eighth note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClickLevel {
    /// Beat 1 of a bar (the accent).
    Downbeat,
    /// Every other beat.
    Beat,
    /// Halfway between two beats.
    HalfBeat,
    /// A quarter and three quarters of the way between two beats.
    QuarterBeat,
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
    /// Samples the cue reserves from `at`; the next cue never starts earlier
    /// than `at + len`.
    pub len: u64,
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
    let sections: Vec<&Section> = arrangement
        .sections
        .iter()
        .map(|id| {
            song.section(id).expect(
                "`validate` guarantees every arrangement section id names a section of the song",
            )
        })
        .collect();
    let segments = segments(&sections, options.seam_crossfade, song.length());
    let timeline = Timeline::new(song);
    let clicks = clicks(&BeatGrid::new(song), &segments);
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

/// Lays the sections end to end from output sample 0 and records the
/// crossfade at every seam where the source jumps (see [`Segment`]).
fn segments(sections: &[&Section], seam_crossfade: u64, song_length: u64) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::with_capacity(sections.len());
    let mut out_start = 0u64;
    for section in sections {
        let len = section.length();
        let mut fade_in = 0;
        if let Some(outgoing) = segments.last_mut() {
            let outgoing_end = outgoing.src_start + outgoing.len;
            if outgoing_end != section.start {
                let source_left = song_length.saturating_sub(outgoing_end);
                fade_in = seam_crossfade.min(len).min(source_left);
                outgoing.fade_out = fade_in;
            }
        }
        segments.push(Segment {
            section_id: section.id.clone(),
            out_start,
            src_start: section.start,
            len,
            fade_in,
            fade_out: 0,
        });
        out_start = out_start.saturating_add(len);
    }
    segments
}

/// Maps the source ticks inside each segment to output positions, so clicks
/// stay aligned with the audio after rearranging.
fn clicks(grid: &BeatGrid, segments: &[Segment]) -> Vec<ClickEvent> {
    let mut clicks = Vec::new();
    for segment in segments {
        let src_end = segment.src_start + segment.len;
        clicks.extend(
            grid.clicks_in(segment.src_start..src_end)
                .iter()
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
    for (index, (section, segment)) in sections.iter().zip(segments).enumerate() {
        let bar = bar_before(timeline, segments, index);
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
            len: cue_length,
        });
    }
    cues
}

/// The bar heard just before segment `index` in the output: one bar at the
/// tempo and meter of the preceding segment's last source sample. The first
/// segment has nothing before it and uses its own first bar.
fn bar_before(timeline: &Timeline<'_>, segments: &[Segment], index: usize) -> u64 {
    match index
        .checked_sub(1)
        .and_then(|previous| segments.get(previous))
    {
        Some(previous) => {
            let last_sample = (previous.src_start + previous.len).saturating_sub(1);
            timeline.bar_length_at(last_sample)
        }
        None => timeline.bar_length_at(segments[index].src_start),
    }
}
