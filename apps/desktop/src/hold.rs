//! Hold-to-panic: panic fires only after the control is held for
//! [`PANIC_HOLD`], so a click or a brush never stops the show.

use std::time::{Duration, Instant};

/// How long the panic control must be held before panic fires.
pub const PANIC_HOLD: Duration = Duration::from_millis(1_000);

/// The state of the panic control. The screen calls [`Self::press`] and
/// [`Self::release`] from the pointer or the key, [`Self::tick`] from its
/// timer, and [`Self::cancel`] when the window loses focus; whichever of
/// tick and release sees the full hold first fires, once per hold.
#[derive(Debug, Clone, Default)]
pub struct PanicHold {
    since: Option<Instant>,
    fired: bool,
}

impl PanicHold {
    /// A control nobody holds.
    pub fn new() -> Self {
        Self::default()
    }

    /// The control went down at `now`. Pressing it again while held (a key
    /// repeating) changes nothing.
    pub fn press(&mut self, now: Instant) {
        if self.since.is_none() {
            self.since = Some(now);
            self.fired = false;
        }
    }

    /// The control went up at `now`; returns whether panic fires, because
    /// the hold was complete and no tick had fired it yet.
    pub fn release(&mut self, now: Instant) -> bool {
        let fires = self.tick(now);
        self.since = None;
        self.fired = false;
        fires
    }

    /// Returns whether panic fires at `now`: the first tick after the hold
    /// is complete.
    pub fn tick(&mut self, now: Instant) -> bool {
        let fires = self.is_counting() && self.fill(now) >= 1.0;
        self.fired |= fires;
        fires
    }

    /// The press was lost before its release (the window lost focus, so
    /// the release may never reach it): the hold ends without panic, and a
    /// release that still arrives later fires nothing.
    pub fn cancel(&mut self) {
        self.since = None;
        self.fired = false;
    }

    /// Whether a hold is counting toward panic: the control is down and
    /// panic has not fired yet. False once panic fired, even while the
    /// control is still down.
    pub fn is_counting(&self) -> bool {
        self.since.is_some() && !self.fired
    }

    /// How far the hold that is counting is toward panic at `now`, 0.0 to
    /// 1.0, as the panic button's fill shows it; 0.0 when nothing counts
    /// (the control is up, or panic fired while it is still down).
    pub fn fill(&self, now: Instant) -> f32 {
        match self.since {
            Some(since) if !self.fired => {
                let held = now.saturating_duration_since(since);
                (held.as_secs_f32() / PANIC_HOLD.as_secs_f32()).min(1.0)
            }
            _ => 0.0,
        }
    }
}
