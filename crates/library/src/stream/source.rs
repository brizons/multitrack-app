//! The audio-thread side of a streamed stem.
//!
//! Every method here runs on the audio thread: it only moves blocks through
//! `rtrb` queues and searches small vectors that never grow past their
//! capacity, so it never allocates, frees, locks, or does I/O.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use domain::StemRole;
use engine::live::{MAX_PREFETCH_TARGETS, StemSource};
use rtrb::{Consumer, Producer};

use super::{Block, Counters, RETIRED_CURSORS, Shutdown, StreamCapacity};

/// The queue ends a source owns.
pub(super) struct Wiring {
    pub(super) id: String,
    pub(super) role: StemRole,
    /// Frames in the stem file.
    pub(super) frames: u64,
    pub(super) requests: Producer<u64>,
    pub(super) filled: Consumer<Block>,
    pub(super) returns: Producer<Block>,
}

/// A reader still busy past an earlier playhead: a seam's fading-out tail.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    next: u64,
    /// Frames past the cut it may still read.
    limit: u64,
}

/// Every window the source keeps in memory, as `[start, end)` frames.
type Windows = [Option<(u64, u64)>; 1 + MAX_PREFETCH_TARGETS + RETIRED_CURSORS];

pub(super) struct StreamingSource {
    wiring: Wiring,
    block_frames: u64,
    ahead: u64,
    head: u64,
    tail: u64,
    /// Blocks the source may hold or wait for at once.
    pool: usize,
    /// Next frame of the current section.
    main: u64,
    /// Block of `main` when the windows were last refreshed.
    main_block: u64,
    retired: [Option<Cursor>; RETIRED_CURSORS],
    /// Slot the next retired cursor replaces, oldest first.
    next_retired: usize,
    targets: [Option<u64>; MAX_PREFETCH_TARGETS],
    resident: Vec<Block>,
    pending: Vec<u64>,
    /// Blocks the disk thread could not decode; they are not asked again.
    failed: Vec<u64>,
    /// Whether the windows changed since the last refresh, or a refresh
    /// could not finish.
    dirty: bool,
    counters: Arc<Counters>,
    /// Kept alive by every source; the last one dropped stops the thread.
    _shutdown: Arc<Shutdown>,
}

impl StreamingSource {
    pub(super) fn new(
        wiring: Wiring,
        capacity: &StreamCapacity,
        counters: Arc<Counters>,
        shutdown: Arc<Shutdown>,
    ) -> Self {
        let pool = capacity.blocks_per_stem;
        Self {
            wiring,
            block_frames: capacity.block_frames as u64,
            ahead: capacity.ahead_frames,
            head: capacity.head_frames,
            tail: capacity.tail_frames,
            pool,
            main: 0,
            main_block: 0,
            retired: [None; RETIRED_CURSORS],
            next_retired: 0,
            targets: [None; MAX_PREFETCH_TARGETS],
            resident: Vec::with_capacity(pool),
            pending: Vec::with_capacity(pool),
            failed: Vec::with_capacity(pool),
            dirty: true,
            counters,
            _shutdown: shutdown,
        }
    }

    /// `[start, start + len)` clipped to the stem, or `None` when empty.
    fn clip(&self, start: u64, len: u64) -> Option<(u64, u64)> {
        let end = start.saturating_add(len).min(self.wiring.frames);
        (start < end).then_some((start, end))
    }

    fn windows(&self) -> Windows {
        let mut windows: Windows = [None; 1 + MAX_PREFETCH_TARGETS + RETIRED_CURSORS];
        windows[0] = self.clip(self.main, self.ahead);
        for (slot, target) in windows[1..].iter_mut().zip(&self.targets) {
            *slot = target.and_then(|target| self.clip(target, self.head));
        }
        for (slot, cursor) in windows[1 + MAX_PREFETCH_TARGETS..]
            .iter_mut()
            .zip(&self.retired)
        {
            *slot = cursor.and_then(|cursor| {
                self.clip(cursor.next, cursor.limit.saturating_sub(cursor.next))
            });
        }
        windows
    }

    fn wanted(&self, windows: &Windows, index: u64) -> bool {
        let start = index * self.block_frames;
        let end = start + self.block_frames;
        windows
            .iter()
            .flatten()
            .any(|&(from, to)| from < end && start < to)
    }

    fn resident_block(&self, index: u64) -> Option<&Block> {
        self.resident
            .iter()
            .find(|block| block.ok && block.index == index)
    }

    /// Gives a block back to the disk thread; if its queue were ever full,
    /// keeps it rather than freeing memory on the audio thread.
    fn give_back(&mut self, block: Block) {
        if let Err(rtrb::PushError::Full(block)) = self.wiring.returns.push(block) {
            self.resident.push(block);
            self.dirty = true;
        }
    }

    /// Takes the decoded blocks that arrived.
    fn receive(&mut self) {
        while let Ok(block) = self.wiring.filled.pop() {
            if let Some(position) = self.pending.iter().position(|&i| i == block.index) {
                self.pending.swap_remove(position);
            }
            self.dirty = true;
            if block.ok {
                self.resident.push(block);
            } else {
                if self.failed.len() < self.failed.capacity() {
                    self.failed.push(block.index);
                }
                self.give_back(block);
            }
        }
    }

    /// Gives back the blocks no window needs and asks for the missing ones,
    /// the current section first.
    fn refresh(&mut self) {
        self.dirty = false;
        self.main_block = self.main / self.block_frames;
        let windows = self.windows();
        let mut position = 0;
        while position < self.resident.len() {
            if self.wanted(&windows, self.resident[position].index) {
                position += 1;
            } else {
                let block = self.resident.swap_remove(position);
                self.give_back(block);
                if self.dirty {
                    break;
                }
            }
        }
        for &(start, end) in windows.iter().flatten() {
            for index in start / self.block_frames..=(end - 1) / self.block_frames {
                if self.resident_block(index).is_some()
                    || self.pending.contains(&index)
                    || self.failed.contains(&index)
                {
                    continue;
                }
                if self.resident.len() + self.pending.len() >= self.pool
                    || self.wiring.requests.push(index).is_err()
                {
                    // Retried on the next call, when blocks have come back.
                    self.dirty = true;
                    return;
                }
                self.pending.push(index);
                self.counters.requested.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// The current section left `main`: keep the frames just past it for a
    /// fading-out tail.
    fn retire_main(&mut self) {
        self.retired[self.next_retired] = Some(Cursor {
            next: self.main,
            limit: self.main.saturating_add(self.tail),
        });
        self.next_retired = (self.next_retired + 1) % RETIRED_CURSORS;
        self.dirty = true;
    }

    /// Moves the reader that reads `at` on by `frames`: the current section,
    /// a tail, or, for a read the player did not announce, a new playhead.
    fn follow(&mut self, at: u64, frames: u64) {
        if at == self.main {
            self.main = at + frames;
            return;
        }
        for slot in &mut self.retired {
            if let Some(cursor) = slot
                && cursor.next == at
            {
                cursor.next = at + frames;
                if cursor.next >= cursor.limit {
                    *slot = None;
                    self.dirty = true;
                }
                return;
            }
        }
        self.retire_main();
        self.main = at + frames;
    }

    /// Copies `[at, at + out.len() / 2)` from the resident blocks; whether
    /// every frame was there.
    fn copy(&self, at: u64, out: &mut [f32]) -> bool {
        let frames = (out.len() / 2) as u64;
        let mut complete = true;
        let mut done = 0;
        while done < frames {
            let position = at + done;
            let index = position / self.block_frames;
            let offset = position - index * self.block_frames;
            let take = (self.block_frames - offset).min(frames - done);
            let destination = &mut out[2 * done as usize..2 * (done + take) as usize];
            if position >= self.wiring.frames {
                destination.fill(0.0);
            } else if let Some(block) = self.resident_block(index) {
                let offset = offset as usize;
                destination.copy_from_slice(&block.data[2 * offset..2 * (offset + take as usize)]);
            } else {
                complete = false;
            }
            done += take;
        }
        complete
    }
}

impl StemSource for StreamingSource {
    fn id(&self) -> &str {
        &self.wiring.id
    }

    fn role(&self) -> StemRole {
        self.wiring.role
    }

    fn read(&mut self, at: u64, out: &mut [f32]) -> bool {
        self.receive();
        let complete = self.copy(at, out);
        self.follow(at, (out.len() / 2) as u64);
        if self.dirty || self.main / self.block_frames != self.main_block {
            self.refresh();
        }
        complete
    }

    fn is_ready(&mut self, at: u64, frames: u64) -> bool {
        self.receive();
        if self.dirty {
            self.refresh();
        }
        let Some((start, end)) = self.clip(at, frames.min(self.head)) else {
            return true;
        };
        (start / self.block_frames..=(end - 1) / self.block_frames)
            .all(|index| self.resident_block(index).is_some())
    }

    fn prefetch(&mut self, playhead: u64, targets: &[u64]) {
        self.receive();
        if playhead != self.main {
            self.retire_main();
            self.main = playhead;
        }
        self.targets = [None; MAX_PREFETCH_TARGETS];
        for (slot, &target) in self.targets.iter_mut().zip(targets) {
            *slot = Some(target);
        }
        self.refresh();
    }
}
