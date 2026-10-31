//! Beat and bar grid derived from a song's tempo and time-signature maps.
//!
//! Tempo is piecewise constant and BPM counts the time-signature denominator
//! note. The beat grid runs on across tempo changes; bars restart at every
//! time-signature point. Every position is computed in `f64` from the start
//! of its constant-tempo region and rounded once, so errors never accumulate.

use crate::compile::ClickLevel;
use crate::package::TimeSignaturePoint;
use crate::song::Song;

/// Ticks per beat: the click subdivides each beat into sixteenths.
const TICKS_PER_BEAT: u64 = 4;

/// A click tick at a source position.
pub(crate) struct Tick {
    pub(crate) at: u64,
    pub(crate) level: ClickLevel,
}

/// A constant-tempo region: where it starts, in samples and in beats.
struct TempoRegion {
    at: u64,
    beat: f64,
    samples_per_beat: f64,
}

pub(crate) struct Timeline<'a> {
    tempo: Vec<TempoRegion>,
    meters: &'a [TimeSignaturePoint],
    end: u64,
}

impl<'a> Timeline<'a> {
    /// Relies on the `Song` invariants: both maps are non-empty, start at 0,
    /// strictly increase, and every bpm is positive and finite.
    pub(crate) fn new(song: &'a Song) -> Self {
        let sample_rate = f64::from(song.sample_rate());
        let mut tempo: Vec<TempoRegion> = Vec::with_capacity(song.tempo_map().len());
        for point in song.tempo_map() {
            let beat = tempo.last().map_or(0.0, |previous| {
                previous.beat + (point.at - previous.at) as f64 / previous.samples_per_beat
            });
            tempo.push(TempoRegion {
                at: point.at,
                beat,
                samples_per_beat: sample_rate * 60.0 / point.bpm,
            });
        }
        Self {
            tempo,
            meters: song.time_signature_map(),
            end: song.length(),
        }
    }

    /// Every click tick in `[0, song length)`, sorted by position, then level.
    pub(crate) fn ticks(&self) -> Vec<Tick> {
        let mut ticks = Vec::new();
        for (index, meter) in self.meters.iter().enumerate() {
            let region_end = self
                .meters
                .get(index + 1)
                .map_or(self.end, |next| next.at.min(self.end));
            let origin = self.beat_at(meter.at);
            let ticks_per_bar = TICKS_PER_BEAT * u64::from(meter.numerator);
            for tick in 0u64.. {
                let at = self.position_of(origin + tick as f64 / TICKS_PER_BEAT as f64);
                if at >= region_end {
                    break;
                }
                ticks.push(Tick {
                    at,
                    level: level_of(tick, ticks_per_bar),
                });
            }
        }
        ticks.sort_by_key(|tick| (tick.at, tick.level));
        ticks
    }

    /// Length of one bar at the tempo and meter in force at `at`.
    pub(crate) fn bar_length_at(&self, at: u64) -> u64 {
        let beats = f64::from(self.meter_at(at).numerator);
        (beats * self.tempo_at(at).samples_per_beat).round() as u64
    }

    fn tempo_at(&self, at: u64) -> &TempoRegion {
        let index = self.tempo.partition_point(|region| region.at <= at);
        &self.tempo[index.saturating_sub(1)]
    }

    fn meter_at(&self, at: u64) -> &TimeSignaturePoint {
        let index = self.meters.partition_point(|meter| meter.at <= at);
        &self.meters[index.saturating_sub(1)]
    }

    fn beat_at(&self, at: u64) -> f64 {
        let region = self.tempo_at(at);
        region.beat + (at - region.at) as f64 / region.samples_per_beat
    }

    /// Sample nearest to `beat`.
    fn position_of(&self, beat: f64) -> u64 {
        let index = self.tempo.partition_point(|region| region.beat <= beat);
        let region = &self.tempo[index.saturating_sub(1)];
        (region.at as f64 + (beat - region.beat) * region.samples_per_beat).round() as u64
    }
}

fn level_of(tick: u64, ticks_per_bar: u64) -> ClickLevel {
    if tick.is_multiple_of(ticks_per_bar) {
        ClickLevel::Accent
    } else if tick.is_multiple_of(TICKS_PER_BEAT) {
        ClickLevel::Quarter
    } else if tick.is_multiple_of(2) {
        ClickLevel::Eighth
    } else {
        ClickLevel::Sixteenth
    }
}
