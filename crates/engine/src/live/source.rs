//! Where the live player reads stem audio from.

use std::slice;

use domain::StemRole;

use crate::mix::frame_of;
use crate::{RenderError, StemAudio, check_stems};

/// A stem the live player can read at any source position.
///
/// Every method runs on the audio thread: it must not allocate, lock, block,
/// or do I/O.
pub trait StemSource: Send {
    /// Stem id, as in the song package; [`crate::MixSettings::stem_gains`]
    /// uses it.
    fn id(&self) -> &str;

    /// What the stem is used for: only `track` stems fade, and a `click` stem
    /// replaces the generated click.
    fn role(&self) -> StemRole;

    /// Fills `out` with `out.len() / 2` interleaved stereo frames starting at
    /// source frame `at`: a mono stem plays on both sides, and frames past
    /// the stem's end are silence. Returns `false` when the frames are not
    /// available; the player then plays silence for this stem in that range,
    /// ignores what was written to `out`, and counts an underrun.
    fn read(&mut self, at: u64, out: &mut [f32]) -> bool;

    /// Whether playing `frames` frames from source frame `at` can start now
    /// without running dry, as long as the source keeps reading ahead while
    /// they play. A streaming source answers for the frames it keeps buffered
    /// at a jump target; the player executes a queued jump only when every
    /// source is ready for it.
    fn is_ready(&mut self, at: u64, frames: u64) -> bool;

    /// Says where playback stands, so the source can buffer in time: the
    /// current section plays on from source frame `playhead`, and it may jump
    /// next to one of `targets` (most likely first, at most
    /// [`super::MAX_PREFETCH_TARGETS`]). Each call replaces the previous one;
    /// the player calls it again whenever the playhead jumps or the targets
    /// change, not while it plays on contiguously.
    fn prefetch(&mut self, playhead: u64, targets: &[u64]);
}

/// A stem decoded into memory; every read is available.
#[derive(Debug, Clone, PartialEq)]
pub struct MemorySource {
    stem: StemAudio,
}

impl MemorySource {
    /// Wraps `stem` after the same checks [`crate::render_offline`] makes.
    pub fn new(stem: StemAudio) -> Result<Self, RenderError> {
        check_stems(slice::from_ref(&stem))?;
        Ok(Self { stem })
    }
}

impl StemSource for MemorySource {
    fn id(&self) -> &str {
        &self.stem.id
    }

    fn role(&self) -> StemRole {
        self.stem.role
    }

    fn read(&mut self, at: u64, out: &mut [f32]) -> bool {
        for (i, frame) in out.chunks_exact_mut(2).enumerate() {
            let source_frame = at.saturating_add(i as u64);
            let (left, right) = if source_frame < self.stem.frames {
                frame_of(&self.stem, source_frame as usize)
            } else {
                (0.0, 0.0)
            };
            frame[0] = left;
            frame[1] = right;
        }
        true
    }

    /// Always: the whole stem is in memory.
    fn is_ready(&mut self, _at: u64, _frames: u64) -> bool {
        true
    }

    /// Nothing to do: the whole stem is in memory.
    fn prefetch(&mut self, _playhead: u64, _targets: &[u64]) {}
}

/// Wraps every stem in a [`MemorySource`], in order.
pub fn memory_sources(stems: Vec<StemAudio>) -> Result<Vec<Box<dyn StemSource>>, RenderError> {
    stems
        .into_iter()
        .map(|stem| MemorySource::new(stem).map(|source| Box::new(source) as Box<dyn StemSource>))
        .collect()
}
