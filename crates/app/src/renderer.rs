//! The audio-thread side of a session.
//!
//! The [`Deck`] (the current item's player, and the next one when a setlist
//! cues it) and the writing end of the state snapshot travel together as
//! one [`AudioState`]. While a device plays, an [`AudioRenderer`] owns it
//! on the audio thread; when the renderer is dropped (the stream closed or
//! the device was lost), it hands the state back to the session through a
//! one-slot `rtrb` queue instead of dropping it. The players are therefore
//! only ever dropped on the control thread: dropping one drops its last
//! streaming source, which joins the disk thread (advisory A6). The deck
//! sends the players it no longer needs back the same way.

use std::fmt;

use engine::live::{Deck, DeckStatus, Status};
use rtrb::{Consumer, Producer, RingBuffer};

/// What the audio thread publishes after every buffer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Report {
    /// The deck's snapshot.
    pub(crate) deck: DeckStatus,
    /// Whether every source of the current item can start playing from the
    /// playhead; always true while the transport runs.
    pub(crate) ready: bool,
    /// Messages the deck had applied: the report describes the state every
    /// message sent up to this count left.
    pub(crate) applied: u64,
}

impl Report {
    /// The current item's player.
    pub(crate) fn status(&self) -> &Status {
        &self.deck.current
    }
}

/// The deck and the writing end of its snapshot.
pub(crate) struct AudioState {
    pub(crate) deck: Deck,
    pub(crate) reports: triple_buffer::Input<Report>,
}

impl AudioState {
    /// Renders `out` and publishes the result. Real-time safe.
    pub(crate) fn render(&mut self, out: &mut [f32]) {
        self.deck.process(out);
        self.publish();
    }

    /// Publishes the deck's state. Real-time safe: `Report` is plain data,
    /// and the triple buffer neither allocates nor locks.
    pub(crate) fn publish(&mut self) {
        let deck = self.deck.status();
        let ready = deck.current.transport.is_running() || self.deck.playhead_ready();
        let applied = self.deck.applied_messages();
        self.reports.write(Report {
            deck,
            ready,
            applied,
        });
    }
}

/// Renders a session's audio. An [`crate::AudioOutput`] gets one with every
/// stream it opens and calls [`Self::render`] from its audio thread.
pub struct AudioRenderer {
    state: Option<Box<AudioState>>,
    home: Producer<Box<AudioState>>,
}

impl AudioRenderer {
    /// A renderer for `state`, and the end the session takes it back from.
    pub(crate) fn new(state: Box<AudioState>) -> (Self, Consumer<Box<AudioState>>) {
        let (home, back) = RingBuffer::new(1);
        let renderer = Self {
            state: Some(state),
            home,
        };
        (renderer, back)
    }

    /// Fills `out` with interleaved stereo frames (a trailing odd sample is
    /// left at 0.0) and publishes the session's state. Real-time safe: no
    /// allocation, no lock, no I/O.
    pub fn render(&mut self, out: &mut [f32]) {
        match self.state.as_mut() {
            Some(state) => state.render(out),
            None => out.fill(0.0),
        }
    }
}

impl Drop for AudioRenderer {
    /// Hands the deck back to its session rather than dropping it here,
    /// on what may be the audio thread. Pushing a box moves a pointer into a
    /// slot that already exists: nothing is allocated or freed.
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            // The slot holds the only state there is, so it is free.
            let _ = self.home.push(state);
        }
    }
}

impl fmt::Debug for AudioRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioRenderer").finish_non_exhaustive()
    }
}
