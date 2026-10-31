//! The live player: walks an arrangement in real time.
//!
//! Every output frame is summed in the same order as [`crate::render_offline`]
//! sums it: for each stem in order, the fading-out tails of earlier seams
//! (oldest first) and then the current section; then the generated click
//! bursts in the order they started; then the master gain. The envelopes and
//! bursts are the offline renderer's own, and every live gain is exactly 1.0
//! at rest, so with no live command the samples are bit-identical.

use domain::{BeatGrid, DEFAULT_SEAM_CROSSFADE, Song, StemRole};
use rtrb::{Consumer, RingBuffer};

use super::ramp::Ramp;
use super::source::StemSource;
use super::status::{FadeState, FadeStatus, JumpKind, QueuedJump, Status, Transport};
use super::{
    COMMAND_CAPACITY, Command, Controller, DECLICK_FRAMES, DEFAULT_FADE_SECONDS, JumpTiming,
    LiveError,
};
use crate::click::ClickBursts;
use crate::mix::Envelope;
use crate::{MixSettings, level_index};

/// Seams whose fade-out can overlap; a further seam drops the oldest tail.
const MAX_TAILS: usize = 8;

/// One arrangement position: a section's source range.
#[derive(Debug, Clone, Copy)]
struct Entry {
    src_start: u64,
    len: u64,
}

/// The section being played.
#[derive(Debug, Clone, Copy)]
struct Voice {
    position: usize,
    /// Frames played since entering this position.
    offset: u64,
    /// Seam crossfade at the start of this position (see `domain::Segment`).
    fade_in: u64,
}

impl Voice {
    const START: Self = Self {
        position: 0,
        offset: 0,
        fade_in: 0,
    };
}

/// The outgoing side of a seam: it keeps reading its source past the cut,
/// fading out under the incoming section.
#[derive(Debug, Clone, Copy)]
struct Tail {
    /// Next source frame.
    src_pos: u64,
    /// Fade-out length.
    len: u64,
    /// Frames of the fade-out played.
    played: u64,
    /// Fade-in of the cut voice and how far it had played, so a voice cut
    /// before its own fade-in finished fades out from where it was.
    fade_in: u64,
    cut_offset: u64,
}

impl Tail {
    fn remaining(&self) -> u64 {
        self.len - self.played
    }

    /// Envelope at frame `i` of the fade-out. A voice that finished its
    /// fade-in contributes exactly 1.0, as in a compiled plan.
    fn gain(&self, i: u64) -> f32 {
        Envelope::FadeIn(self.fade_in).at(self.cut_offset + i) * Envelope::FadeOut(self.len).at(i)
    }
}

struct Stem {
    source: Box<dyn StemSource>,
    gain: f32,
    /// Whether the fade applies: only `track` stems fade.
    fades: bool,
}

impl Stem {
    fn gain_at(&self, track_gain: &[f32], i: usize) -> f32 {
        if self.fades {
            self.gain * track_gain[i]
        } else {
            self.gain
        }
    }
}

/// A click burst still sounding.
#[derive(Debug, Clone, Copy)]
struct ActiveBurst {
    level: domain::ClickLevel,
    gain: f32,
    /// Next sample of the burst table.
    offset: usize,
}

#[derive(Debug, Clone, Copy)]
struct Queued {
    target: usize,
    kind: JumpKind,
}

/// Plays one arrangement in real time; see the [module docs](super).
pub struct Player {
    entries: Vec<Entry>,
    song_length: u64,
    grid: BeatGrid,
    max_frames: usize,
    panic_frames: u64,
    stems: Vec<Stem>,
    master_gain: f32,
    click_levels: [f32; 4],
    /// `None` when a `click` stem replaces the generated click.
    bursts: Option<ClickBursts>,
    commands: Consumer<Command>,

    // Scratch, sized at construction.
    read: Vec<f32>,
    track_gain: Vec<f32>,
    click_gain: Vec<f32>,
    master: Vec<f32>,
    active: Vec<ActiveBurst>,
    tails: Vec<Tail>,

    // Transport state.
    transport: Transport,
    transport_gain: Ramp,
    output_frame: u64,
    voice: Voice,
    queued: Option<Queued>,
    jump_timing: JumpTiming,
    infinite: bool,
    fade: Ramp,
    fade_out: bool,
    fade_frames: u64,
    click: Ramp,
    click_on: bool,
    underruns: u64,
}

impl Player {
    /// Builds a stopped player for the arrangement `arrangement_id` of
    /// `song`, and the [`Controller`] that drives it.
    ///
    /// `sample_rate` must be the song's rate. `max_frames` is the largest
    /// buffer [`Self::process`] will be given; larger buffers are processed
    /// in pieces of that size. Stems are mixed in `sources` order with
    /// `settings`, as [`crate::render_offline`] mixes them. Everything the
    /// audio thread needs is allocated here.
    pub fn new(
        song: &Song,
        arrangement_id: &str,
        settings: &MixSettings,
        sample_rate: u32,
        max_frames: usize,
        sources: Vec<Box<dyn StemSource>>,
    ) -> Result<(Self, Controller), LiveError> {
        let arrangement =
            song.arrangement(arrangement_id)
                .ok_or_else(|| LiveError::UnknownArrangement {
                    id: arrangement_id.to_owned(),
                })?;
        if sample_rate != song.sample_rate() {
            return Err(LiveError::SampleRateMismatch {
                song: song.sample_rate(),
                output: sample_rate,
            });
        }
        if max_frames == 0 {
            return Err(LiveError::ZeroBufferSize);
        }
        if let Some(id) = settings
            .stem_gains
            .keys()
            .find(|id| !sources.iter().any(|source| source.id() == id.as_str()))
        {
            return Err(LiveError::UnknownStemGain { id: id.clone() });
        }

        let entries: Vec<Entry> = arrangement
            .sections
            .iter()
            .map(|id| {
                let section = song.section(id).expect(
                    "`validate` guarantees every arrangement section id names a section of the song",
                );
                Entry {
                    src_start: section.start,
                    len: section.length(),
                }
            })
            .collect();
        let has_click_stem = sources
            .iter()
            .any(|source| source.role() == StemRole::Click);
        let bursts = (!has_click_stem).then(|| ClickBursts::new(sample_rate));
        let burst_capacity = bursts.as_ref().map_or(0, |bursts| bursts.len() + 1);
        let stems = sources
            .into_iter()
            .map(|source| Stem {
                gain: settings.stem_gain(source.id()),
                fades: source.role() == StemRole::Track,
                source,
            })
            .collect();
        let (producer, commands) = RingBuffer::new(COMMAND_CAPACITY);
        let click_level = if settings.click.enabled { 1.0 } else { 0.0 };
        let positions = entries.len();

        let player = Self {
            entries,
            song_length: song.length(),
            grid: BeatGrid::new(song),
            max_frames,
            panic_frames: u64::from(sample_rate) / 2,
            stems,
            master_gain: settings.master_gain,
            click_levels: settings.click.levels,
            bursts,
            commands,
            read: vec![0.0; 2 * max_frames],
            track_gain: vec![0.0; max_frames],
            click_gain: vec![0.0; max_frames],
            master: vec![0.0; max_frames],
            active: Vec::with_capacity(burst_capacity),
            tails: Vec::with_capacity(MAX_TAILS),
            transport: Transport::Stopped,
            transport_gain: Ramp::at(1.0),
            output_frame: 0,
            voice: Voice::START,
            queued: None,
            jump_timing: JumpTiming::default(),
            infinite: false,
            fade: Ramp::at(1.0),
            fade_out: false,
            fade_frames: u64::from(DEFAULT_FADE_SECONDS) * u64::from(sample_rate),
            click: Ramp::at(click_level),
            click_on: settings.click.enabled,
            underruns: 0,
        };
        Ok((
            player,
            Controller {
                producer,
                positions,
            },
        ))
    }

    /// Applies every pending command, then fills `out` with interleaved
    /// stereo frames (a trailing odd sample is left at 0.0). Real-time safe:
    /// no allocation, no lock, no I/O.
    pub fn process(&mut self, out: &mut [f32]) {
        while let Ok(command) = self.commands.pop() {
            self.apply(command);
        }
        out.fill(0.0);
        let usable = out.len() - out.len() % 2;
        let piece = 2 * self.max_frames;
        for chunk in out[..usable].chunks_mut(piece) {
            self.render(chunk);
        }
    }

    /// A snapshot of the transport.
    pub fn status(&self) -> Status {
        let source_frame = self.source_frame();
        Status {
            transport: self.transport,
            output_frame: self.output_frame,
            position: self.voice.position,
            source_frame,
            bar_beat: self.grid.bar_and_beat(source_frame),
            queued: self.queued.map(|queued| QueuedJump {
                target: queued.target,
                kind: queued.kind,
                executes_at: self.jump_frame(queued.kind),
            }),
            jump_timing: self.jump_timing,
            infinite: self.infinite,
            fade: FadeStatus {
                state: match (self.fade_out, self.fade.is_done()) {
                    (false, true) => FadeState::In,
                    (true, false) => FadeState::FadingOut,
                    (true, true) => FadeState::Out,
                    (false, false) => FadeState::FadingIn,
                },
                level: self.fade.level(),
            },
            click: self.click_on,
            underruns: self.underruns,
        }
    }

    fn apply(&mut self, command: Command) {
        use Transport::*;
        match command {
            Command::Play => match self.transport {
                Stopped => self.transport = Playing,
                Pausing | Paused | Stopping => {
                    self.transport = Playing;
                    self.transport_gain.retarget(1.0, DECLICK_FRAMES);
                }
                Playing | Panicking => {}
            },
            Command::Pause => {
                if self.transport == Playing {
                    self.transport = Pausing;
                    self.transport_gain.retarget(0.0, DECLICK_FRAMES);
                }
            }
            Command::Stop => match self.transport {
                Playing | Pausing => {
                    self.transport = Stopping;
                    self.transport_gain.retarget(0.0, DECLICK_FRAMES);
                }
                Paused | Stopped => self.reset(),
                Stopping | Panicking => {}
            },
            Command::BackToStart => match self.transport {
                Playing | Pausing => self.seam_to(0),
                Paused => {
                    self.voice = Voice::START;
                    self.tails.clear();
                    self.active.clear();
                }
                Stopped | Stopping | Panicking => {}
            },
            Command::QueueJump { position } => {
                if position < self.entries.len() {
                    self.queued = Some(Queued {
                        target: position,
                        kind: JumpKind::Jump,
                    });
                }
            }
            Command::CancelJump => self.queued = None,
            Command::SetJumpTiming(timing) => self.jump_timing = timing,
            Command::LiveLoop => {
                self.queued = Some(Queued {
                    target: self.voice.position,
                    kind: JumpKind::Loop,
                });
            }
            Command::SetInfinite(on) => self.infinite = on,
            Command::ToggleFade => {
                self.fade_out = !self.fade_out;
                let target = if self.fade_out { 0.0 } else { 1.0 };
                self.fade.retarget(target, self.fade_frames);
            }
            Command::SetFadeDuration { frames } => self.fade_frames = frames,
            Command::Panic => match self.transport {
                Playing | Pausing | Stopping => {
                    self.transport = Panicking;
                    self.transport_gain.retarget(0.0, self.panic_frames);
                }
                Paused | Stopped => self.reset(),
                Panicking => {}
            },
            Command::SetClick(on) => {
                self.click_on = on;
                self.click
                    .retarget(if on { 1.0 } else { 0.0 }, DECLICK_FRAMES);
            }
        }
    }

    /// Renders `out`, splitting it at every boundary inside it.
    fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let mut done = 0;
        loop {
            self.handle_boundaries();
            if done == frames || !self.transport.is_running() {
                return;
            }
            let n = ((frames - done) as u64).min(self.frames_to_next_event()) as usize;
            self.render_block(&mut out[2 * done..2 * (done + n)]);
            self.advance(n as u64);
            done += n;
        }
    }

    /// Executes everything due at the current output frame.
    fn handle_boundaries(&mut self) {
        loop {
            match self.transport {
                Transport::Stopped | Transport::Paused => return,
                Transport::Pausing if self.transport_gain.is_done() => {
                    self.transport = Transport::Paused;
                    return;
                }
                Transport::Stopping | Transport::Panicking if self.transport_gain.is_done() => {
                    self.reset();
                    return;
                }
                _ => {}
            }
            if let Some(queued) = self.queued
                && self.jump_frame(queued.kind) == self.output_frame
            {
                self.queued = None;
                self.infinite = false;
                self.seam_to(queued.target);
                continue;
            }
            if self.voice.offset == self.entries[self.voice.position].len {
                let position = self.voice.position;
                if self.infinite {
                    self.seam_to(position);
                } else if position + 1 < self.entries.len() {
                    self.seam_to(position + 1);
                } else {
                    self.reset();
                    return;
                }
                continue;
            }
            return;
        }
    }

    /// Frames until the next boundary: the end of the current position, the
    /// queued jump, or the end of a stopping ramp. At least 1 after
    /// [`Self::handle_boundaries`].
    fn frames_to_next_event(&self) -> u64 {
        let entry = self.entries[self.voice.position];
        let mut frames = entry.len - self.voice.offset;
        if let Some(queued) = self.queued {
            frames = frames.min(self.jump_frame(queued.kind) - self.output_frame);
        }
        if matches!(
            self.transport,
            Transport::Pausing | Transport::Stopping | Transport::Panicking
        ) {
            frames = frames.min(self.transport_gain.remaining());
        }
        frames
    }

    /// Output frame where a queued jump of `kind` executes.
    fn jump_frame(&self, kind: JumpKind) -> u64 {
        let entry = self.entries[self.voice.position];
        let at_end = self.output_frame + (entry.len - self.voice.offset);
        if kind == JumpKind::Loop || self.jump_timing == JumpTiming::SectionEnd {
            return at_end;
        }
        let source = entry.src_start + self.voice.offset;
        match self.grid.next_bar_line(source) {
            Some(bar) if bar < entry.src_start + entry.len => self.output_frame + (bar - source),
            _ => at_end,
        }
    }

    fn source_frame(&self) -> u64 {
        self.entries[self.voice.position].src_start + self.voice.offset
    }

    /// Leaves the current position for the start of `target`. When the
    /// source jumps, the outgoing side keeps playing as a tail and the
    /// incoming side fades in, with the length and caps `compile` uses.
    fn seam_to(&mut self, target: usize) {
        let outgoing_end = self.source_frame();
        let incoming = self.entries[target];
        let fade = if outgoing_end == incoming.src_start {
            0
        } else {
            DEFAULT_SEAM_CROSSFADE
                .min(incoming.len)
                .min(self.song_length.saturating_sub(outgoing_end))
        };
        if fade > 0 {
            if self.tails.len() == MAX_TAILS {
                self.tails.remove(0);
            }
            self.tails.push(Tail {
                src_pos: outgoing_end,
                len: fade,
                played: 0,
                fade_in: self.voice.fade_in,
                cut_offset: self.voice.offset,
            });
        }
        self.voice = Voice {
            position: target,
            offset: 0,
            fade_in: fade,
        };
    }

    /// Back to the start, stopped.
    fn reset(&mut self) {
        self.transport = Transport::Stopped;
        self.transport_gain = Ramp::at(1.0);
        self.output_frame = 0;
        self.voice = Voice::START;
        self.tails.clear();
        self.active.clear();
        self.queued = None;
        self.infinite = false;
        self.fade = Ramp::at(1.0);
        self.fade_out = false;
    }

    /// Renders `out.len() / 2` frames with no boundary inside them.
    fn render_block(&mut self, out: &mut [f32]) {
        let n = out.len() / 2;
        for i in 0..n {
            self.track_gain[i] = self.fade.next();
            self.click_gain[i] = self.click.next();
            self.master[i] = self.master_gain * self.transport_gain.next();
        }
        let source = self.source_frame();
        let main_envelope = Envelope::FadeIn(self.voice.fade_in);
        for stem in &mut self.stems {
            for tail in &self.tails {
                let k = (n as u64).min(tail.remaining()) as usize;
                let read = &mut self.read[..2 * k];
                if !stem.source.read(tail.src_pos, read) {
                    self.underruns += 1;
                    continue;
                }
                for i in 0..k {
                    let gain =
                        stem.gain_at(&self.track_gain, i) * tail.gain(tail.played + i as u64);
                    out[2 * i] += read[2 * i] * gain;
                    out[2 * i + 1] += read[2 * i + 1] * gain;
                }
            }
            let read = &mut self.read[..2 * n];
            if !stem.source.read(source, read) {
                self.underruns += 1;
                continue;
            }
            for i in 0..n {
                let gain = stem.gain_at(&self.track_gain, i)
                    * main_envelope.at(self.voice.offset + i as u64);
                out[2 * i] += read[2 * i] * gain;
                out[2 * i + 1] += read[2 * i + 1] * gain;
            }
        }
        self.add_clicks(out, source);
        for (frame, gain) in out.chunks_exact_mut(2).zip(&self.master) {
            frame[0] *= gain;
            frame[1] *= gain;
        }
    }

    /// Adds the bursts still sounding, then the bursts of the ticks in the
    /// source range being played, in the order they start.
    fn add_clicks(&mut self, out: &mut [f32], source: u64) {
        let Some(bursts) = &self.bursts else {
            return;
        };
        let n = out.len() / 2;
        let click_gain = &self.click_gain[..n];
        for burst in &mut self.active {
            let table = &bursts.table(burst.level)[burst.offset..];
            let k = n.min(table.len());
            add_burst(&mut out[..2 * k], &table[..k], burst.gain, &click_gain[..k]);
            burst.offset += k;
        }
        self.active
            .retain(|burst| burst.offset < bursts.table(burst.level).len());
        for click in self.grid.clicks_in(source..source + n as u64) {
            let gain = self.click_levels[level_index(click.level)];
            if gain == 0.0 {
                continue;
            }
            let start = (click.at - source) as usize;
            let table = bursts.table(click.level);
            let k = (n - start).min(table.len());
            add_burst(
                &mut out[2 * start..2 * (start + k)],
                &table[..k],
                gain,
                &click_gain[start..start + k],
            );
            if k < table.len() && self.active.len() < self.active.capacity() {
                self.active.push(ActiveBurst {
                    level: click.level,
                    gain,
                    offset: k,
                });
            }
        }
    }

    /// Moves every counter `n` frames on.
    fn advance(&mut self, n: u64) {
        self.voice.offset += n;
        self.output_frame += n;
        for tail in &mut self.tails {
            let k = n.min(tail.remaining());
            tail.played += k;
            tail.src_pos += k;
        }
        self.tails.retain(|tail| tail.remaining() > 0);
    }
}

/// Adds `table` scaled by `gain` and the per-frame click gain to both
/// channels of `out`.
fn add_burst(out: &mut [f32], table: &[f32], gain: f32, click_gain: &[f32]) {
    for ((frame, sample), click) in out.chunks_exact_mut(2).zip(table).zip(click_gain) {
        let value = sample * (gain * click);
        frame[0] += value;
        frame[1] += value;
    }
}
