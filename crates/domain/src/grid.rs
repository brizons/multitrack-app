//! Bar, beat and click queries in source time, for players that walk an
//! arrangement live instead of reading a compiled plan.
//!
//! A [`BeatGrid`] holds every click tick of the song, computed once by the
//! same timeline (and the same rounding) that [`crate::compile()`] uses, so a
//! query over a source range returns exactly the ticks a plan places for a
//! segment covering that range. Building the grid allocates; every query is a
//! binary search over it and allocates nothing.

use std::ops::Range;

use crate::compile::ClickLevel;
use crate::song::Song;
use crate::timeline::Timeline;

/// A bar and a beat, both counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BarBeat {
    /// Bar number from the song start; bars keep counting across meter
    /// changes.
    pub bar: u64,
    /// Beat within the bar.
    pub beat: u32,
}

/// One click tick at a source sample. A plan places it at an output sample as
/// a [`crate::ClickEvent`]; the two types keep source and output time apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceClick {
    /// Source sample of the tick.
    pub at: u64,
    /// Which of the four click levels plays it.
    pub level: ClickLevel,
}

/// The song's tick, beat and bar grid, in source samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeatGrid {
    /// Every tick in `[0, song length)`, sorted by position.
    ticks: Vec<SourceClick>,
    /// Positions of the `Downbeat` and `Beat` ticks.
    beats: Vec<u64>,
    /// Positions of the `Downbeat` ticks: the bar lines.
    bars: Vec<u64>,
}

impl BeatGrid {
    /// Computes the grid of `song`.
    pub fn new(song: &Song) -> Self {
        let ticks: Vec<SourceClick> = Timeline::new(song)
            .ticks()
            .into_iter()
            .map(|tick| SourceClick {
                at: tick.at,
                level: tick.level,
            })
            .collect();
        let positions = |levels: &[ClickLevel]| -> Vec<u64> {
            ticks
                .iter()
                .filter(|tick| levels.contains(&tick.level))
                .map(|tick| tick.at)
                .collect()
        };
        let beats = positions(&[ClickLevel::Downbeat, ClickLevel::Beat]);
        let bars = positions(&[ClickLevel::Downbeat]);
        Self { ticks, beats, bars }
    }

    /// The click ticks whose source position lies in `range`, sorted by
    /// position. A plan segment playing `range` from output sample
    /// `out_start` places each tick at `out_start + (at - range.start)`.
    pub fn clicks_in(&self, range: Range<u64>) -> &[SourceClick] {
        let first = self.ticks.partition_point(|tick| tick.at < range.start);
        let end = self.ticks.partition_point(|tick| tick.at < range.end);
        &self.ticks[first..end.max(first)]
    }

    /// The first bar line at or after source sample `at`, or `None` when no
    /// bar line follows before the song end.
    pub fn next_bar_line(&self, at: u64) -> Option<u64> {
        let index = self.bars.partition_point(|&bar| bar < at);
        self.bars.get(index).copied()
    }

    /// The bar and beat that source sample `at` falls in.
    pub fn bar_and_beat(&self, at: u64) -> BarBeat {
        let bar = self.bars.partition_point(|&bar| bar <= at).max(1);
        let bar_start = self.bars.get(bar - 1).copied().unwrap_or(0);
        let first_beat = self.beats.partition_point(|&beat| beat < bar_start);
        let beats_so_far = self.beats.partition_point(|&beat| beat <= at);
        BarBeat {
            bar: bar as u64,
            beat: beats_so_far.saturating_sub(first_beat).max(1) as u32,
        }
    }
}
