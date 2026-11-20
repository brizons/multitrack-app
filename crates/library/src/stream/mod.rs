//! Stems streamed from disk for the live player (S3).
//!
//! [`open_streaming`] checks a song folder like [`crate::load_song`], then
//! starts one disk thread for the song and returns one [`StemSource`] per
//! stem. Nothing is decoded up front, and memory does not grow with the song.
//!
//! # How it works
//!
//! Every stem has a fixed pool of blocks of [`StreamConfig::block_frames`]
//! stereo `f32` frames. Block `k` always holds source frames
//! `k * block_frames ..` of its stem, so a block can serve any reader that
//! needs those frames. Blocks travel between the threads through three
//! lock-free `rtrb` queues per stem, so neither side ever waits for the other
//! and the audio thread never allocates or frees:
//!
//! - audio to disk, *requests*: indices of the blocks the source wants;
//! - disk to audio, *filled*: decoded blocks;
//! - audio to disk, *returns*: blocks the source no longer needs.
//!
//! The source wants the blocks of a few windows, all of them bounded:
//!
//! - ahead of the playhead, [`StreamConfig::ahead_ms`] of audio;
//! - at each prefetch target (a queued jump, the current position's start
//!   for a loop, the next position when the arrangement jumps there, the
//!   start), [`StreamConfig::head_ms`];
//! - just past the last few cuts, where a seam's fading-out tail still
//!   reads, [`StreamConfig::seam_crossfade`] frames.
//!
//! It is ready for a jump target when the target's head window is in memory.
//! A read whose frames are not in memory yet plays silence for that stem and
//! the player counts an underrun.
//!
//! Dropping the last source stops the disk thread and waits for it.

mod disk;
mod source;

use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use domain::{DEFAULT_SEAM_CROSSFADE, Song, ValidationWarning};
use engine::live::{MAX_PREFETCH_TARGETS, StemSource};
use rtrb::RingBuffer;

use crate::{LoadError, open_package};
use disk::{Decoder, DiskStem};
use source::{StreamingSource, Wiring};

/// Fading-out tails a source keeps audio for after the playhead jumps.
const RETIRED_CURSORS: usize = 4;

/// A test and diagnostics hook, called on the disk thread before it decodes
/// a block, with the stem id and the source frames of the block. Returning
/// `false` holds the block back; the disk thread asks again later. It can
/// simulate a slow or stalled disk.
pub type Gate = Arc<dyn Fn(&str, Range<u64>) -> bool + Send + Sync>;

/// Buffer sizes of a streamed song.
#[derive(Clone)]
pub struct StreamConfig {
    /// Frames in one block, the unit the disk thread decodes. Default 2 048.
    pub block_frames: usize,
    /// Audio kept decoded ahead of the playhead, in milliseconds. It must
    /// cover the largest buffer the player processes. Default 1 000 ms.
    pub ahead_ms: u32,
    /// Audio kept decoded at every prefetch target, in milliseconds; a jump
    /// waits until all of it is in memory. Default 500 ms.
    pub head_ms: u32,
    /// Frames a fading-out tail reads past a cut: at least the player's
    /// seam crossfade. Default [`DEFAULT_SEAM_CROSSFADE`].
    pub seam_crossfade: u64,
    /// See [`Gate`]. Default none.
    pub gate: Option<Gate>,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            block_frames: 2_048,
            ahead_ms: 1_000,
            head_ms: 500,
            seam_crossfade: DEFAULT_SEAM_CROSSFADE,
            gate: None,
        }
    }
}

impl fmt::Debug for StreamConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamConfig")
            .field("block_frames", &self.block_frames)
            .field("ahead_ms", &self.ahead_ms)
            .field("head_ms", &self.head_ms)
            .field("seam_crossfade", &self.seam_crossfade)
            .field("gate", &self.gate.as_ref().map(|_| "set"))
            .finish()
    }
}

/// The memory a streamed stem uses: fixed by the configuration and the
/// sample rate, whatever the song length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamCapacity {
    /// Frames per block.
    pub block_frames: usize,
    /// Blocks each stem owns.
    pub blocks_per_stem: usize,
    /// Frames kept ahead of the playhead.
    pub ahead_frames: u64,
    /// Frames kept at each prefetch target.
    pub head_frames: u64,
    /// Prefetch targets kept at once.
    pub targets: usize,
    /// Frames kept past each recent cut.
    pub tail_frames: u64,
}

impl StreamCapacity {
    fn new(config: &StreamConfig, sample_rate: u32) -> Self {
        let block_frames = config.block_frames.max(1);
        let millis = |ms: u32| (u64::from(ms) * u64::from(sample_rate) / 1_000).max(1);
        let ahead_frames = millis(config.ahead_ms);
        let head_frames = millis(config.head_ms);
        let tail_frames = config.seam_crossfade;
        // A window of `frames` frames starting anywhere touches at most this
        // many blocks.
        let blocks = |frames: u64| (frames.div_ceil(block_frames as u64) + 1) as usize;
        let blocks_per_stem = blocks(ahead_frames)
            + (MAX_PREFETCH_TARGETS + 1) * blocks(head_frames)
            + RETIRED_CURSORS * blocks(tail_frames);
        Self {
            block_frames,
            blocks_per_stem,
            ahead_frames,
            head_frames,
            targets: MAX_PREFETCH_TARGETS,
            tail_frames,
        }
    }

    /// Frames of audio each stem can hold.
    pub fn frames_per_stem(&self) -> u64 {
        self.blocks_per_stem as u64 * self.block_frames as u64
    }

    /// Bytes of sample memory each stem holds: stereo 32-bit float frames.
    pub fn bytes_per_stem(&self) -> u64 {
        self.frames_per_stem() * 2 * 4
    }
}

/// A validated song whose stems stream from disk.
pub struct StreamingSong {
    /// The validated song.
    pub song: Song,
    /// Findings that do not block playback.
    pub warnings: Vec<ValidationWarning>,
    /// One source per stem, in package order, for
    /// [`engine::live::Player::new`]. Dropping them all stops the disk
    /// thread.
    pub sources: Vec<Box<dyn StemSource>>,
    /// Watches the disk thread.
    pub monitor: DiskMonitor,
    /// Memory per stem.
    pub capacity: StreamCapacity,
}

impl fmt::Debug for StreamingSong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingSong")
            .field("song", &self.song.title())
            .field("sources", &self.sources.len())
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

/// Counters the sources, the disk thread, and the monitor share.
#[derive(Debug, Default)]
struct Counters {
    /// Block requests the sources sent.
    requested: AtomicU64,
    /// Requests the disk thread answered with a block.
    completed: AtomicU64,
    /// Requests the gate held back in the disk thread's last pass.
    held: AtomicU64,
    /// Passes the disk thread started and finished over its requests.
    passes_started: AtomicU64,
    passes_finished: AtomicU64,
    /// Blocks the disk thread could not decode.
    errors: AtomicU64,
    /// Set by the last source when it is dropped.
    stop: AtomicBool,
    /// Cleared by the disk thread when it exits.
    running: AtomicBool,
}

/// Watches the disk thread of a streamed song, from any thread. The
/// [`Default`] monitor watches no thread: it is idle and never running, for
/// something that streams nothing, such as a click-only tile.
#[derive(Debug, Clone, Default)]
pub struct DiskMonitor {
    counters: Arc<Counters>,
}

impl DiskMonitor {
    /// Whether the disk thread is still running.
    pub fn is_running(&self) -> bool {
        self.counters.running.load(Ordering::SeqCst)
    }

    /// Whether, as of its last pass, the disk thread has answered every
    /// request it may answer: all of them except those the [`Gate`] holds
    /// back.
    pub fn is_idle(&self) -> bool {
        let requested = self.counters.requested.load(Ordering::SeqCst);
        let completed = self.counters.completed.load(Ordering::SeqCst);
        let held = self.counters.held.load(Ordering::SeqCst);
        requested == completed + held
    }

    /// Waits until [`Self::is_idle`], at most `timeout`; returns whether it
    /// became idle. While the gate holds requests back, it also waits for a
    /// disk pass that started after the call, so a gate opened just before
    /// is taken into account. For tests and tools, never for the audio
    /// thread.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let fresh_pass = self.counters.passes_started.load(Ordering::SeqCst) + 1;
        loop {
            let held = self.counters.held.load(Ordering::SeqCst);
            let fresh = self.counters.passes_finished.load(Ordering::SeqCst) >= fresh_pass;
            if self.is_idle() && (held == 0 || fresh) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_micros(100));
        }
    }

    /// Blocks the disk thread could not decode; each one plays as an
    /// underrun.
    pub fn decode_errors(&self) -> u64 {
        self.counters.errors.load(Ordering::SeqCst)
    }
}

/// Stops the disk thread and waits for it when the last source is dropped.
struct Shutdown {
    counters: Arc<Counters>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.counters.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            // A panic on the disk thread has already cleared `running`.
            let _ = thread.join();
        }
    }
}

/// One block of decoded stereo frames.
struct Block {
    index: u64,
    /// `false` when the disk thread could not decode it.
    ok: bool,
    data: Box<[f32]>,
}

/// Checks the song folder `dir` like [`crate::load_song`], opens every stem
/// for streaming, and starts the disk thread.
pub fn open_streaming(dir: &Path, config: &StreamConfig) -> Result<StreamingSong, LoadError> {
    let (validated, opened) = open_package(dir)?;
    let capacity = StreamCapacity::new(config, validated.song.sample_rate());
    let pool = capacity.blocks_per_stem;
    let counters = Arc::new(Counters::default());
    let mut disk_stems = Vec::with_capacity(opened.len());
    let mut wirings = Vec::with_capacity(opened.len());
    for (stem, opened) in opened {
        let decoder = Decoder::new(&stem, opened)?;
        let (request_tx, request_rx) = RingBuffer::new(pool);
        let (filled_tx, filled_rx) = RingBuffer::new(pool);
        let (return_tx, return_rx) = RingBuffer::new(pool);
        wirings.push(Wiring {
            id: stem.id.clone(),
            role: stem.role,
            frames: decoder.frames(),
            requests: request_tx,
            filled: filled_rx,
            returns: return_tx,
        });
        disk_stems.push(DiskStem::new(
            stem.id,
            decoder,
            capacity.block_frames,
            pool,
            (request_rx, filled_tx, return_rx),
        ));
    }

    counters.running.store(true, Ordering::SeqCst);
    let thread = {
        let counters = Arc::clone(&counters);
        let gate = config.gate.clone();
        thread::Builder::new()
            .name("stem-disk".to_owned())
            .spawn(move || disk::run(disk_stems, &counters, gate.as_ref()))
    };
    let thread = match thread {
        Ok(thread) => thread,
        Err(source) => {
            counters.running.store(false, Ordering::SeqCst);
            return Err(LoadError::DiskThread { source });
        }
    };
    let shutdown = Arc::new(Shutdown {
        counters: Arc::clone(&counters),
        thread: Some(thread),
    });
    let sources = wirings
        .into_iter()
        .map(|wiring| {
            Box::new(StreamingSource::new(
                wiring,
                &capacity,
                Arc::clone(&counters),
                Arc::clone(&shutdown),
            )) as Box<dyn StemSource>
        })
        .collect();
    Ok(StreamingSong {
        song: validated.song,
        warnings: validated.warnings,
        sources,
        monitor: DiskMonitor { counters },
        capacity,
    })
}
