//! S3 (F3.1): every click lands within ±1 sample of its exact beat position,
//! checked against an exact rational oracle (ported from the verifier).

mod common;

use common::{RATE, SongSpec, marker};
use domain::{ClickLevel, CompileOptions, SectionType, compile};

/// Exact beat grid. Tempos are integer milli-BPM, so with `D = rate * 60_000`
/// every beat position is `N / D` beats for an integer `N`.
struct Oracle {
    d: i128,
    /// Tempo regions as (start sample, start beat * D, milli-BPM).
    regions: Vec<(i128, i128, i128)>,
    /// Meter points as (start sample, numerator).
    meters: Vec<(u64, u32)>,
    end: u64,
}

impl Oracle {
    fn new(tempo_milli: &[(u64, i128)], meters: &[(u64, u32, u32)], end: u64) -> Self {
        let mut regions: Vec<(i128, i128, i128)> = Vec::new();
        for &(at, bpm) in tempo_milli {
            let n = regions
                .last()
                .map_or(0, |&(previous_at, previous_n, previous_bpm)| {
                    previous_n + (at as i128 - previous_at) * previous_bpm
                });
            regions.push((at as i128, n, bpm));
        }
        Self {
            d: i128::from(RATE) * 60_000,
            regions,
            meters: meters
                .iter()
                .map(|&(at, numerator, _)| (at, numerator))
                .collect(),
            end,
        }
    }

    fn n_at(&self, at: u64) -> i128 {
        let region = self
            .regions
            .iter()
            .rev()
            .find(|r| r.0 <= at as i128)
            .unwrap();
        region.1 + (at as i128 - region.0) * region.2
    }

    /// Exact sample position of beat `n / D` as a fraction.
    fn position(&self, n: i128) -> (i128, i128) {
        let region = self.regions.iter().rev().find(|r| r.1 <= n).unwrap();
        (region.0 * region.2 + (n - region.1), region.2)
    }

    /// Expected ticks as (exact position, level), sorted by position.
    fn ticks(&self) -> Vec<(f64, ClickLevel)> {
        let mut ticks = Vec::new();
        for (index, &(meter_at, numerator)) in self.meters.iter().enumerate() {
            let region_end = self
                .meters
                .get(index + 1)
                .map_or(self.end, |next| next.0.min(self.end));
            let origin = self.n_at(meter_at);
            let per_bar = 4 * u64::from(numerator);
            for tick in 0u64.. {
                let (numerator, denominator) =
                    self.position(origin + i128::from(tick) * self.d / 4);
                if numerator >= i128::from(region_end) * denominator {
                    break;
                }
                let level = if tick % per_bar == 0 {
                    ClickLevel::Accent
                } else if tick % 4 == 0 {
                    ClickLevel::Quarter
                } else if tick % 2 == 0 {
                    ClickLevel::Eighth
                } else {
                    ClickLevel::Sixteenth
                };
                ticks.push((numerator as f64 / denominator as f64, level));
            }
        }
        ticks.sort_by(|a, b| a.0.total_cmp(&b.0));
        ticks
    }
}

/// Compiles a one-section song straight through and compares its clicks with
/// the oracle; returns the largest position error in samples.
fn max_click_error(tempo_milli: &[(u64, i128)], meters: &[(u64, u32, u32)], end: u64) -> f64 {
    let tempo: Vec<(u64, f64)> = tempo_milli
        .iter()
        .map(|&(at, milli)| (at, milli as f64 / 1_000.0))
        .collect();
    let song = SongSpec::new(end, vec![marker("all", SectionType::Verse, None, 0)])
        .tempo(&tempo)
        .meters(meters)
        .build();
    let plan = compile(&song, "default", &CompileOptions::default()).unwrap();
    let expected = Oracle::new(tempo_milli, meters, end).ticks();
    assert_eq!(
        plan.clicks.len(),
        expected.len(),
        "one click per expected tick"
    );
    let mut max_error: f64 = 0.0;
    for (click, &(position, level)) in plan.clicks.iter().zip(&expected) {
        assert_eq!(click.level, level, "level of the tick at {position}");
        max_error = max_error.max((click.at as f64 - position).abs());
    }
    max_error
}

#[test]
fn tempo_and_meter_changes_at_odd_positions_match_the_exact_oracle() {
    let tempo = [(0, 120_000), (1_234_567, 97_500), (3_000_001, 140_000)];
    let meters = [(0, 4, 4), (2_000_003, 3, 4), (4_000_000, 4, 4)];
    let max_error = max_click_error(&tempo, &meters, 6_000_000);
    assert!(
        max_error <= 0.5 + 1e-6,
        "rounded to the nearest sample, got {max_error}"
    );

    let tempo_f64: Vec<(u64, f64)> = tempo
        .iter()
        .map(|&(a, m)| (a, m as f64 / 1_000.0))
        .collect();
    let song = SongSpec::new(6_000_000, vec![marker("all", SectionType::Verse, None, 0)])
        .tempo(&tempo_f64)
        .meters(&meters)
        .build();
    let plan = compile(&song, "default", &CompileOptions::default()).unwrap();
    for meter_at in [2_000_003, 4_000_000] {
        assert!(
            plan.clicks
                .iter()
                .any(|c| c.at == meter_at && c.level == ClickLevel::Accent),
            "bars restart with an accent at {meter_at}"
        );
    }
}

#[test]
fn sixty_minutes_of_tempo_and_meter_changes_stay_within_one_sample() {
    let end = 60 * 60 * u64::from(RATE);
    let tempo = [
        (0, 72_000),
        (13_337_777, 133_333),
        (40_000_123, 90_500),
        (77_777_771, 140_000),
        (120_000_017, 60_250),
        (150_123_457, 180_000),
        (170_000_001, 99_999),
    ];
    let meters = [
        (0, 4, 4),
        (30_000_000, 3, 4),
        (60_000_000, 6, 8),
        (100_000_000, 7, 8),
        (140_000_000, 4, 4),
    ];
    let max_error = max_click_error(&tempo, &meters, end);
    assert!(max_error <= 1.0, "F3.1: within ±1 sample, got {max_error}");
}
