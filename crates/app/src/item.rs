//! What a session plays, with its player built and its start buffering,
//! ready to hand to the audio thread.

use std::path::Path;

use domain::{SONG_FADE_OUT_SECONDS, Song};
use engine::MixSettings;
use engine::live::{Player, PlayerConfig};
use library::{DiskMonitor, StreamConfig, StreamingSong, open_streaming};

use crate::error::OpenError;
use crate::output::MAX_BUFFER_FRAMES;
use crate::session::SessionOptions;
use crate::song::SongInfo;

/// A player and what the control thread keeps about it.
pub(crate) struct Item {
    pub(crate) info: SongInfo,
    pub(crate) player: Player,
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
        let (mut player, _controller) = Player::new(
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
            monitor,
        })
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
