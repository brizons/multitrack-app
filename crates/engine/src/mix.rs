//! Stem playback: copies each segment's source range into the output, with
//! the equal-power crossfade at seams where the source jumps. The live player
//! reuses [`Envelope`] and [`frame_of`], so both paths compute the same gains.

use std::f64::consts::FRAC_PI_2;

use domain::Segment;

use crate::StemAudio;

/// Gain shape over a run of frames.
#[derive(Clone, Copy)]
pub(crate) enum Envelope {
    /// Fades in over the first `n` frames, then stays at 1.
    FadeIn(u64),
    /// Fades out over all `n` frames.
    FadeOut(u64),
}

impl Envelope {
    /// Gain at frame `i` of the run. Over a fade of `n` frames the gains are
    /// `sin` (in) and `cos` (out) of `π/2 · t` with `t = (i + 0.5) / n`, so the
    /// two sides of a seam always sum to unit power.
    pub(crate) fn at(self, i: u64) -> f32 {
        let phase = |n: u64| FRAC_PI_2 * ((i as f64 + 0.5) / n as f64);
        match self {
            Self::FadeIn(n) if i < n => phase(n).sin() as f32,
            Self::FadeIn(_) => 1.0,
            Self::FadeOut(n) => phase(n).cos() as f32,
        }
    }
}

/// Adds `stem`'s part of `segment` to `output`: the segment from its
/// `src_start`, fading in over `fade_in`, and the `fade_out` frames it keeps
/// reading past its end, fading out under the next segment (see [`Segment`]).
pub(crate) fn add_segment(output: &mut [f32], stem: &StemAudio, gain: f32, segment: &Segment) {
    add_frames(
        output,
        stem,
        gain,
        (segment.out_start, segment.src_start, segment.len),
        Envelope::FadeIn(segment.fade_in),
    );
    if segment.fade_out > 0 {
        add_frames(
            output,
            stem,
            gain,
            (
                segment.out_start.saturating_add(segment.len),
                segment.src_start.saturating_add(segment.len),
                segment.fade_out,
            ),
            Envelope::FadeOut(segment.fade_out),
        );
    }
}

/// Adds `len` frames of `stem` from source frame `src_start` to the output
/// from frame `out_start`. Source frames past the stem's end are silence.
fn add_frames(
    output: &mut [f32],
    stem: &StemAudio,
    gain: f32,
    (out_start, src_start, len): (u64, u64, u64),
    envelope: Envelope,
) {
    let out_frames = (output.len() / 2) as u64;
    let len = len
        .min(stem.frames.saturating_sub(src_start))
        .min(out_frames.saturating_sub(out_start));
    for i in 0..len {
        let frame_gain = gain * envelope.at(i);
        let (left, right) = frame_of(stem, (src_start + i) as usize);
        let out = 2 * (out_start + i) as usize;
        output[out] += left * frame_gain;
        output[out + 1] += right * frame_gain;
    }
}

/// Left and right sample of a mono or stereo stem; mono plays on both sides.
pub(crate) fn frame_of(stem: &StemAudio, frame: usize) -> (f32, f32) {
    if stem.channels == 1 {
        let sample = stem.samples[frame];
        (sample, sample)
    } else {
        (stem.samples[2 * frame], stem.samples[2 * frame + 1])
    }
}
