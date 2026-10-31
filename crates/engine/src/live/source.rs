//! Where the live player reads stem audio from.

use std::slice;

use domain::StemRole;

use crate::mix::frame_of;
use crate::{RenderError, StemAudio, check_stems};

/// A stem the live player can read at any source position.
///
/// [`StemSource::read`] runs on the audio thread: it must not allocate,
/// lock, block, or do I/O.
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
}

/// Wraps every stem in a [`MemorySource`], in order.
pub fn memory_sources(stems: Vec<StemAudio>) -> Result<Vec<Box<dyn StemSource>>, RenderError> {
    stems
        .into_iter()
        .map(|stem| MemorySource::new(stem).map(|source| Box::new(source) as Box<dyn StemSource>))
        .collect()
}
