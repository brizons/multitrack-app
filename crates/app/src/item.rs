//! What a session plays: one song, or one setlist item, with its player
//! built and its start buffering, ready to hand to the audio thread.

use std::path::Path;
use std::time::Duration;

use domain::{ClickItem, DEFAULT_ARRANGEMENT_ID, SONG_FADE_OUT_SECONDS, Song};
use engine::MixSettings;
use engine::live::{self, Controller, JumpTiming, Player, PlayerConfig};
use library::{DiskMonitor, StreamConfig, StreamingSong, open_streaming};

use crate::error::OpenError;
use crate::output::MAX_BUFFER_FRAMES;
use crate::session::SessionOptions;
use crate::song::SongInfo;

/// A player and what the control thread keeps about it.
pub(crate) struct Item {
    pub(crate) info: SongInfo,
    pub(crate) player: Player,
    /// The player's own queue: commands sent here before the player is
    /// handed over apply when it first plays.
    pub(crate) controller: Controller,
    pub(crate) monitor: DiskMonitor,
}

impl Item {
    /// Opens the song folder `dir` for streaming and builds the player for
    /// `arrangement`, cut to positions `first..=last` when `positions` is
    /// given (clamped to the arrangement, which the setlist was checked
    /// against). With `end_fade`, the track stems fade out over the last
    /// [`SONG_FADE_OUT_SECONDS`] (F5.3). The player starts buffering at once.
    pub(crate) fn open_song(
        dir: &Path,
        arrangement: &str,
        positions: Option<(usize, usize)>,
        end_fade: bool,
        options: &SessionOptions,
    ) -> Result<Self, OpenError> {
        // One value feeds both configurations, so the stream always keeps
        // what the player's seams read (advisory A3).
        let stream_config = StreamConfig {
            seam_crossfade: options.seam_crossfade,
            gate: options.disk_gate.clone(),
            ..StreamConfig::default()
        };
        let StreamingSong {
            song,
            warnings,
            sources,
            monitor,
            capacity,
        } = open_streaming(dir, &stream_config)?;
        let sample_rate = song.sample_rate();
        if capacity.ahead_frames < u64::from(MAX_BUFFER_FRAMES) {
            return Err(OpenError::RateTooLow {
                sample_rate,
                ahead_frames: capacity.ahead_frames,
            });
        }
        let song = match positions {
            Some(range) => excerpt(song, arrangement, range),
            None => song,
        };
        let (mut player, controller) = Player::new(
            &song,
            arrangement,
            &MixSettings::default(),
            &config(sample_rate, options),
            sources,
        )?;
        if end_fade {
            let frames = u64::from(SONG_FADE_OUT_SECONDS) * u64::from(sample_rate);
            player.set_end_fade(Some(frames));
        }
        let warnings = warnings.iter().map(ToString::to_string).collect();
        let info = SongInfo::new(&song, arrangement, warnings)
            .expect("`Player::new` accepted the arrangement");
        Ok(Self {
            info,
            player,
            controller,
            monitor,
        })
    }

    /// The player of a click-only tile at `sample_rate` (F5.5): its bars,
    /// or one bar repeating until the operator moves on. `None` when the
    /// tile breaks a setlist rule.
    pub(crate) fn click(
        click: &ClickItem,
        sample_rate: u32,
        options: &SessionOptions,
    ) -> Option<Self> {
        let song = Song::click_only(sample_rate, click.bpm, click.time_signature, click.bars)?;
        let (mut player, controller) = Player::new(
            &song,
            DEFAULT_ARRANGEMENT_ID,
            &MixSettings::default(),
            &config(sample_rate, options),
            Vec::new(),
        )
        .ok()?;
        player.set_infinite_at_start(click.bars.is_none());
        let info = SongInfo::new(&song, DEFAULT_ARRANGEMENT_ID, Vec::new())?;
        Some(Self {
            info,
            player,
            controller,
            monitor: DiskMonitor::default(),
        })
    }

    /// Carries the operator's jump timing and fade duration over to this
    /// item, so a setting does not flip back when the next item starts.
    pub(crate) fn carry(&mut self, timing: JumpTiming, fade: Duration) {
        let frames = (fade.as_secs_f64() * f64::from(self.info.sample_rate)).round() as u64;
        // A fresh queue has room for both.
        let _ = self.controller.send(live::Command::SetJumpTiming(timing));
        let _ = self
            .controller
            .send(live::Command::SetFadeDuration { frames });
    }
}

fn config(sample_rate: u32, options: &SessionOptions) -> PlayerConfig {
    PlayerConfig {
        sample_rate,
        max_frames: options.buffer_frames as usize,
        seam_crossfade: options.seam_crossfade,
    }
}

/// `song` cut to positions `first..=last` of `arrangement`, clamped to it;
/// the song unchanged when it has no such arrangement, for
/// [`Player::new`] to report.
fn excerpt(song: Song, arrangement: &str, (first, last): (usize, usize)) -> Song {
    let Some(positions) = song
        .arrangement(arrangement)
        .map(|arrangement| arrangement.sections.len())
    else {
        return song;
    };
    let last = last.min(positions.saturating_sub(1));
    song.excerpt(arrangement, first.min(last), last)
        .unwrap_or(song)
}
