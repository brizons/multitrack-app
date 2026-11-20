//! Generated click: one synthesized burst per click level, placed at the
//! plan's click events.

use std::f64::consts::TAU;

use domain::ClickLevel;

use crate::level_index;

/// Burst pitch per level, in the order Downbeat, Beat, HalfBeat, QuarterBeat:
/// the accent is the highest.
const FREQUENCIES: [f64; 4] = [1_760.0, 1_320.0, 1_100.0, 880.0];

/// Burst length in seconds.
const BURST_SECONDS: f64 = 0.015;

/// Time constant of the exponential decay, in seconds.
const DECAY_SECONDS: f64 = 0.004;

/// Peak of a burst at level 1.0.
const AMPLITUDE: f64 = 0.5;

/// The four burst tables, computed once before rendering.
pub(crate) struct ClickBursts {
    tables: [Vec<f32>; 4],
}

impl ClickBursts {
    /// Each burst is a decaying cosine, so its first sample is the peak
    /// ([`AMPLITUDE`]) and never 0: onsets are detectable sample-exactly.
    pub(crate) fn new(sample_rate: u32) -> Self {
        let rate = f64::from(sample_rate);
        let frames = ((rate * BURST_SECONDS).round() as usize).max(1);
        let tables = FREQUENCIES.map(|frequency| {
            (0..frames)
                .map(|n| {
                    let t = n as f64 / rate;
                    (AMPLITUDE * (TAU * frequency * t).cos() * (-t / DECAY_SECONDS).exp()) as f32
                })
                .collect()
        });
        Self { tables }
    }

    /// Adds the burst for `level`, scaled by `gain`, to both channels from
    /// output frame `at`; a gain of 0.0 adds nothing.
    pub(crate) fn add(&self, output: &mut [f32], at: u64, level: ClickLevel, gain: f32) {
        if gain == 0.0 {
            return;
        }
        let out_frames = (output.len() / 2) as u64;
        let burst = &self.tables[level_index(level)];
        let len = (burst.len() as u64).min(out_frames.saturating_sub(at));
        for (i, sample) in burst.iter().take(len as usize).enumerate() {
            let value = sample * gain;
            let out = 2 * (at as usize + i);
            output[out] += value;
            output[out + 1] += value;
        }
    }
}
