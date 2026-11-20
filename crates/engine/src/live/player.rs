//! The live player: walks an arrangement in real time.
//!
//! Every output frame is summed in the same order as [`crate::render_offline`]
//! sums it: for each stem in order, the fading-out tails of earlier seams
//! (oldest first) and then the current section; then the generated click
//! bursts in the order they started; then the master gain. The envelopes and
//! bursts are the offline renderer's own, and every live gain is exactly 1.0
//! at rest, so with no live command the samples are bit-identical. Seams use
//! [`domain::seam_fade`], the rule `compile` uses.

use domain::{BeatGrid, Song, StemRole, seam_fade};
use rtrb::{Consumer, RingBuffer};

use super::ramp::Ramp;
use super::source::StemSource;
use super::status::{ClickSource, FadeState, FadeStatus, JumpKind, QueuedJump, Status, Transport};
use super::{
    COMMAND_CAPACITY, Command, Controller, DECLICK_FRAMES, DEFAULT_FADE_SECONDS, JumpTiming,
    LiveError, MAX_PREFETCH_TARGETS, PANIC_MILLISECONDS, PlayerConfig,
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
    /// Length of the fade-in this voice plays (see `domain::Segment`).
    fade_in: u64,
    /// Fade-in frames already played when this position was entered: a
    /// contiguous seam carries the fade-in on instead of restarting it.
    env_start: u64,
}

impl Voice {
    const START: Self = Self {
        position: 0,
        offset: 0,
        fade_in: 0,
        env_start: 0,
    };

    /// Frame of the fade-in envelope the next frame plays.
    fn envelope_position(&self) -> u64 {
        self.env_start + self.offset
    }
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
    /// First output frame the jump may execute on; after a deferral it is
    /// one past the boundary it missed.
    not_before: u64,
    deferrals: u32,
}

impl Queued {
    fn new(target: usize, kind: JumpKind) -> Self {
        Self {
            target,
            kind,
            not_before: 0,
            deferrals: 0,
        }
    }
}

/// The last prefetch hint sent to the sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hint {
    voice_epoch: u64,
    targets: [u64; MAX_PREFETCH_TARGETS],
    count: usize,
}

/// Plays one arrangement in real time; see the [module docs](super).
pub struct Player {
    entries: Vec<Entry>,
    /// `suffix[p]`: frames from the start of position `p` to the end of the
    /// arrangement; one more entry than `entries`, holding 0.
    suffix: Vec<u64>,
    song_length: u64,
    grid: BeatGrid,
    max_frames: usize,
    seam_crossfade: u64,
    panic_frames: u64,
    stems: Vec<Stem>,
    master_gain: f32,
    click_levels: [f32; 4],
    /// `None` when a `click` stem replaces the generated click.
    bursts: Option<ClickBursts>,
    /// Most click bursts that sound at once: at most one tick starts per
    /// output frame and a burst lasts its table length, plus one for the
    /// frame where the oldest ends.
    max_bursts: usize,
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
    /// Counts every re-entry of a position, so hints follow playhead jumps.
    voice_epoch: u64,
    last_hint: Option<Hint>,
    /// Commands taken from the queue so far.
    applied: u64,
    /// Times the arrangement played to its end (not a stop or a panic).
    ends: u64,
    /// Frames before the end over which the track stems fade out.
    end_fade: Option<u64>,
    /// Whether the end fade began since the last return to the start.
    end_fade_started: bool,
    /// Whether the infinite loop is on after every return to the start.
    infinite_at_start: bool,
}

impl Player {
    /// Builds a stopped player for the arrangement `arrangement_id` of
    /// `song`, and the [`Controller`] that drives it.
    ///
    /// Stems are mixed in `sources` order with `settings`, as
    /// [`crate::render_offline`] mixes them. Everything the audio thread
    /// needs is allocated here, and every source gets its first
    /// [`StemSource::prefetch`] hint: the start of the arrangement.
    pub fn new(
        song: &Song,
        arrangement_id: &str,
        settings: &MixSettings,
        config: &PlayerConfig,
        sources: Vec<Box<dyn StemSource>>,
    ) -> Result<(Self, Controller), LiveError> {
        let PlayerConfig {
            sample_rate,
            max_frames,
            seam_crossfade,
        } = *config;
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
        let max_bursts = bursts.as_ref().map_or(0, |bursts| bursts.len() + 1);
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
        let mut suffix = vec![0; positions + 1];
        for (index, entry) in entries.iter().enumerate().rev() {
            suffix[index] = suffix[index + 1] + entry.len;
        }

        let mut player = Self {
            entries,
            suffix,
            song_length: song.length(),
            grid: BeatGrid::new(song),
            max_frames,
            seam_crossfade,
            panic_frames: u64::from(sample_rate) * PANIC_MILLISECONDS / 1_000,
            stems,
            master_gain: settings.master_gain,
            click_levels: settings.click.levels,
            bursts,
            max_bursts,
            commands,
            read: vec![0.0; 2 * max_frames],
            track_gain: vec![0.0; max_frames],
            click_gain: vec![0.0; max_frames],
            master: vec![0.0; max_frames],
            active: Vec::with_capacity(max_bursts),
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
            voice_epoch: 0,
            last_hint: None,
            applied: 0,
            ends: 0,
            end_fade: None,
            end_fade_started: false,
            infinite_at_start: false,
        };
        player.update_hints();
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
            self.applied += 1;
        }
        self.update_hints();
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
                executes_at: self.jump_frame(queued),
                deferrals: queued.deferrals,
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
            click_source: if self.bursts.is_some() {
                ClickSource::Generated
            } else {
                ClickSource::Recorded
            },
            underruns: self.underruns,
        }
    }

    /// Fades the `track` stems out over the last `frames` frames before the
    /// arrangement ends, so they are silent on its last frame: the fade of
    /// [`Command::ToggleFade`], started on its own (F5.3). Click and guide
    /// stems and the generated click keep playing to the end. The fade
    /// starts while playing, at most once per run from the start; pressing
    /// [`Command::ToggleFade`] during it fades the stems back in. `None`, the
    /// default, plays the end as it is. Call it before the player plays.
    pub fn set_end_fade(&mut self, frames: Option<u64>) {
        self.end_fade = frames;
    }

    /// Switches the infinite loop on or off now and at every return to the
    /// start (a stop, a panic), as [`Command::SetInfinite`] would right after
    /// it. A click-only tile without a bar count uses it to repeat its one
    /// bar until the operator moves on. Off by default.
    pub fn set_infinite_at_start(&mut self, on: bool) {
        self.infinite_at_start = on;
        self.infinite = on;
    }

    /// Applies `command` at once, as if it had come through the queue: for
    /// the thread that owns the player, such as a [`super::Deck`] driving
    /// it. It is not counted by [`Self::applied_commands`]. Real-time safe.
    pub(crate) fn command(&mut self, command: Command) {
        self.apply(command);
    }

    /// Output frames the arrangement plays before it ends if no other
    /// command arrives: the rest of the current position and the positions
    /// after it, or, with a jump or loop queued, the frames to its boundary
    /// and the positions from its target on. `None` while the infinite loop
    /// is on. An estimate when a queued jump is deferred: the arrangement
    /// then plays on. Real-time safe.
    pub(crate) fn frames_to_end(&self) -> Option<u64> {
        if self.infinite {
            return None;
        }
        Some(match self.queued {
            Some(queued) => {
                (self.jump_frame(queued) - self.output_frame) + self.suffix[queued.target]
            }
            None => {
                let entry = self.entries[self.voice.position];
                (entry.len - self.voice.offset) + self.suffix[self.voice.position + 1]
            }
        })
    }

    /// Times the arrangement played to its end and returned to the start
    /// there, not counting stops and panics.
    pub(crate) fn ends(&self) -> u64 {
        self.ends
    }

    /// The transport state.
    pub(crate) fn transport(&self) -> Transport {
        self.transport
    }

    /// Output frames played since the last start.
    pub(crate) fn output_frame(&self) -> u64 {
        self.output_frame
    }

    /// How many commands the player has taken from its [`Controller`]'s
    /// queue and applied so far. A [`Status`] read after a call that returns
    /// as many as the controller sent describes the state those commands
    /// left, such as the playhead after [`Command::BackToStart`]. Real-time
    /// safe.
    pub fn applied_commands(&self) -> u64 {
        self.applied
    }

    /// Whether every source can start playing from the playhead now without
    /// running dry: [`StemSource::is_ready`] for the rest of the current
    /// position, the question a queued jump asks for its target. Real-time
    /// safe; a streaming source also takes in the blocks that arrived. The
    /// control side asks it (through whatever thread owns the player) while
    /// the transport is stopped or paused, so a start from a cold stream
    /// does not play a silent first buffer.
    pub fn playhead_ready(&mut self) -> bool {
        let at = self.source_frame();
        let entry = self.entries[self.voice.position];
        let frames = entry.len - self.voice.offset;
        self.stems
            .iter_mut()
            .all(|stem| stem.source.is_ready(at, frames))
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
                    self.voice_epoch += 1;
                    self.tails.clear();
                    self.active.clear();
                }
                Stopped | Stopping | Panicking => {}
            },
            Command::QueueJump { position } => {
                if position < self.entries.len() {
                    self.queued = Some(Queued::new(position, JumpKind::Jump));
                }
            }
            Command::CancelJump => self.queued = None,
            Command::SetJumpTiming(timing) => self.jump_timing = timing,
            Command::LiveLoop => {
                self.queued = Some(Queued::new(self.voice.position, JumpKind::Loop));
            }
            Command::SetInfinite(on) => self.infinite = on,
            Command::ToggleFade => {
                self.fade_out = !self.fade_out;
                let target = if self.fade_out { 0.0 } else { 1.0 };
                self.fade.retarget(target, self.fade_frames);
            }
            Command::SetFadeDuration { frames } => self.fade_frames = frames.max(DECLICK_FRAMES),
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
            self.start_end_fade();
            self.update_hints();
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
                && self.jump_frame(queued) == self.output_frame
            {
                if self.target_ready(queued.target) {
                    self.queued = None;
                    if queued.kind == JumpKind::Jump && queued.target != self.voice.position {
                        self.infinite = false;
                    }
                    self.seam_to(queued.target);
                } else {
                    self.queued = Some(Queued {
                        not_before: self.output_frame + 1,
                        deferrals: queued.deferrals + 1,
                        ..queued
                    });
                }
                continue;
            }
            if self.voice.offset == self.entries[self.voice.position].len {
                let Some(next) = self.natural_next() else {
                    self.ends += 1;
                    self.reset();
                    return;
                };
                // A jump deferred at this boundary whose target the
                // arrangement enters on its own has done its job; executing
                // it later would play the target again.
                if self.queued.is_some_and(|queued| queued.target == next) {
                    self.queued = None;
                }
                self.seam_to(next);
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
            frames = frames.min(self.jump_frame(queued) - self.output_frame);
        }
        if matches!(
            self.transport,
            Transport::Pausing | Transport::Stopping | Transport::Panicking
        ) {
            frames = frames.min(self.transport_gain.remaining());
        }
        if let Some(due) = self.end_fade_due()
            && due > 0
        {
            frames = frames.min(due);
        }
        frames
    }

    /// Frames until the end fade must start, while it is set, not started
    /// yet, and the transport plays; 0 when it is due now.
    fn end_fade_due(&self) -> Option<u64> {
        let fade = self.end_fade?;
        if self.end_fade_started || self.transport != Transport::Playing {
            return None;
        }
        Some(self.frames_to_end()?.saturating_sub(fade))
    }

    /// Starts the end fade when it is due: the track stems ramp from where
    /// the fade stands to silence over exactly the frames left, so the last
    /// frame of the arrangement is silent for them.
    fn start_end_fade(&mut self) {
        if self.end_fade_due() != Some(0) {
            return;
        }
        let left = self.frames_to_end().unwrap_or(0);
        self.end_fade_started = true;
        self.fade_out = true;
        self.fade.retarget_over(0.0, left);
    }

    /// Output frame of the first boundary, at or after `queued.not_before`,
    /// where `queued` may execute: the end of the current position, or with
    /// [`JumpTiming::NextBar`] the next bar line inside it.
    fn jump_frame(&self, queued: Queued) -> u64 {
        let entry = self.entries[self.voice.position];
        let at_end = self.output_frame + (entry.len - self.voice.offset);
        let earliest = queued.not_before.max(self.output_frame);
        if queued.kind == JumpKind::Loop || self.jump_timing == JumpTiming::SectionEnd {
            // Past `earliest` only for the instant between a deferral at a
            // section end and the seam that follows it.
            return at_end.max(earliest);
        }
        let source = entry.src_start + self.voice.offset;
        let from = source + (earliest - self.output_frame);
        let boundary = match self.grid.next_bar_line(from) {
            Some(bar) if bar < entry.src_start + entry.len => self.output_frame + (bar - source),
            _ => at_end,
        };
        boundary.max(earliest)
    }

    /// Whether every source has the start of position `target` ready.
    fn target_ready(&mut self, target: usize) -> bool {
        let entry = self.entries[target];
        self.stems
            .iter_mut()
            .all(|stem| stem.source.is_ready(entry.src_start, entry.len))
    }

    /// Position the arrangement enters at the end of the current one when no
    /// queued jump executes: the current one again with the infinite loop
    /// on, else the next one; `None` at the end of the arrangement.
    fn natural_next(&self) -> Option<usize> {
        let position = self.voice.position;
        if self.infinite {
            Some(position)
        } else {
            (position + 1 < self.entries.len()).then_some(position + 1)
        }
    }

    /// Source start of `position` when entering it at the end of the current
    /// position jumps in the source; `None` when it follows on contiguously,
    /// where the window ahead of the playhead already covers it.
    fn seam_target(&self, position: usize) -> Option<u64> {
        let current = self.entries[self.voice.position];
        let start = self.entries[position].src_start;
        (start != current.src_start + current.len).then_some(start)
    }

    /// Sends the sources a new prefetch hint when the playhead jumped or the
    /// targets changed: the queued target; both places the end of the
    /// current position can go, the next position and its own start (a
    /// loop), the one the infinite loop selects first; and the start of the
    /// arrangement (for back to start and stop). Hinting both keeps a loop
    /// switched on or off just before the end from meeting audio the source
    /// has not buffered.
    fn update_hints(&mut self) {
        let mut targets = [0; MAX_PREFETCH_TARGETS];
        let mut count = 0;
        let position = self.voice.position;
        let next = (position + 1 < self.entries.len()).then_some(position + 1);
        let (likely, other) = if self.infinite {
            (Some(position), next)
        } else {
            (next, Some(position))
        };
        let candidates = [
            self.queued
                .map(|queued| self.entries[queued.target].src_start),
            likely.and_then(|position| self.seam_target(position)),
            other.and_then(|position| self.seam_target(position)),
            Some(self.entries[0].src_start),
        ];
        for target in candidates.into_iter().flatten() {
            if !targets[..count].contains(&target) {
                targets[count] = target;
                count += 1;
            }
        }
        let hint = Hint {
            voice_epoch: self.voice_epoch,
            targets,
            count,
        };
        if self.last_hint == Some(hint) {
            return;
        }
        self.last_hint = Some(hint);
        let playhead = self.source_frame();
        for stem in &mut self.stems {
            stem.source.prefetch(playhead, &targets[..count]);
        }
    }

    fn source_frame(&self) -> u64 {
        self.entries[self.voice.position].src_start + self.voice.offset
    }

    /// Leaves the current position for the start of `target`. When the
    /// source jumps, the outgoing side keeps playing as a tail and the
    /// incoming side fades in, as [`seam_fade`] says. When it does not, the
    /// same audio plays on, a fade-in still running included.
    fn seam_to(&mut self, target: usize) {
        let outgoing_end = self.source_frame();
        let incoming = self.entries[target];
        self.voice_epoch += 1;
        if outgoing_end == incoming.src_start {
            self.voice = Voice {
                position: target,
                offset: 0,
                env_start: self.voice.envelope_position(),
                ..self.voice
            };
            return;
        }
        let fade = seam_fade(
            self.seam_crossfade,
            outgoing_end,
            incoming.src_start,
            incoming.len,
            self.song_length,
        );
        if fade > 0 {
            if self.tails.len() == MAX_TAILS {
                self.tails.remove(0);
            }
            self.tails.push(Tail {
                src_pos: outgoing_end,
                len: fade,
                played: 0,
                fade_in: self.voice.fade_in,
                cut_offset: self.voice.envelope_position(),
            });
        }
        self.voice = Voice {
            position: target,
            offset: 0,
            fade_in: fade,
            env_start: 0,
        };
    }

    /// Back to the start, stopped.
    fn reset(&mut self) {
        self.transport = Transport::Stopped;
        self.transport_gain = Ramp::at(1.0);
        self.output_frame = 0;
        self.voice = Voice::START;
        self.voice_epoch += 1;
        self.tails.clear();
        self.active.clear();
        self.queued = None;
        self.infinite = self.infinite_at_start;
        self.fade = Ramp::at(1.0);
        self.fade_out = false;
        self.end_fade_started = false;
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
                    * main_envelope.at(self.voice.envelope_position() + i as u64);
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
            if k < table.len() && self.active.len() < self.max_bursts {
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
