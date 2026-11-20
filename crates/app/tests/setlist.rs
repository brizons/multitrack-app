//! S3: a setlist plays its items one after another on one device, with
//! each item's transition (Cue next, Stay, AutoLink, Crossfade), start and
//! end points, fade-out, and click-only tiles (F6.1: previous or next song).

mod common;

use app::testing::FakeOutput;
use app::{
    Command, SessionOptions, SetlistCommand, SetlistCommandError, SetlistSession, SetlistSnapshot,
    TransportState,
};
use common::{IDLE, RATE, bits, offline, write_song_with, write_steady};
use domain::{
    ClickItem, CompileOptions, DEFAULT_SEAM_CROSSFADE, Setlist, SetlistItem, SongItem,
    TimeSignature, Transition, compile,
};
use engine::live::DECLICK_FRAMES;
use engine::{MixSettings, render_offline};
use library::{Library, load_song};

/// −1 dBFS.
const PEAK: f32 = 0.891_250_9;
/// The crossfade, five seconds at [`RATE`].
const BLEND: usize = 5 * RATE as usize;
const BUFFER: usize = 256;

struct Shelf {
    dir: tempfile::TempDir,
    library: Library,
}

impl Shelf {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path());
        Self { dir, library }
    }

    fn path(&self, song: &str) -> std::path::PathBuf {
        self.dir.path().join(song)
    }

    /// A [`common::write_song`] song, 7 bars (14 s at 8 kHz), at `song`.
    fn song(&self, song: &str, rate: u32, seed: u64) -> &Self {
        write_song_with(&self.path(song), rate, seed);
        self
    }

    fn steady(&self, song: &str, seconds: usize, track: f32, guide: f32) -> &Self {
        write_steady(
            &self.path(song),
            RATE,
            seconds * RATE as usize,
            track,
            guide,
        );
        self
    }

    /// Saves a setlist of `items` and opens it on `fake`.
    fn open(&self, items: Vec<SetlistItem>, fake: &FakeOutput) -> SetlistSession {
        let mut setlist = Setlist::new("Sunday");
        setlist.items = items;
        let entry = self.library.save_setlist(&setlist).unwrap();
        SetlistSession::open(
            &self.library,
            &entry.file,
            Box::new(fake.clone()),
            options(),
        )
        .expect("the setlist opens")
    }

    fn offline(&self, song: &str) -> Vec<f32> {
        offline(&self.path(song), "default", DEFAULT_SEAM_CROSSFADE)
    }
}

fn options() -> SessionOptions {
    SessionOptions {
        buffer_frames: BUFFER as u32,
        ..SessionOptions::default()
    }
}

fn song(path: &str, transition: Transition) -> SetlistItem {
    SetlistItem::Song(SongItem {
        transition,
        ..SongItem::new(path)
    })
}

fn click(bars: Option<u32>, transition: Transition) -> SetlistItem {
    SetlistItem::Click(ClickItem {
        bars,
        transition,
        ..ClickItem::new(
            120.0,
            TimeSignature {
                numerator: 4,
                denominator: 4,
            },
        )
    })
}

fn wait_disks(session: &SetlistSession) {
    for monitor in session.disk_monitors() {
        assert!(monitor.wait_idle(IDLE), "the disk thread keeps up");
    }
}

fn step(session: &mut SetlistSession, fake: &FakeOutput) -> (Vec<f32>, SetlistSnapshot) {
    wait_disks(session);
    let out = fake.render(BUFFER).expect("a stream plays");
    (out, session.poll())
}

/// Polls until the next item is loaded.
fn wait_next(session: &mut SetlistSession) {
    for _ in 0..100 {
        wait_disks(session);
        if session.poll().setlist.next_ready {
            return;
        }
    }
    panic!("the next item never loaded");
}

/// Sends play and returns the audio from the first buffer that played on;
/// the buffers before it are silent.
fn start(session: &mut SetlistSession, fake: &FakeOutput) -> Vec<f32> {
    session
        .send(SetlistCommand::Item(Command::Play))
        .expect("play is sent");
    for _ in 0..100 {
        let (out, snapshot) = step(session, fake);
        if snapshot.song.output_frame > 0 {
            return out;
        }
        assert!(
            out.iter().all(|&s| s == 0.0),
            "nothing plays before the start"
        );
    }
    panic!("the transport never started");
}

/// Plays `total` frames on from a start; returns them and the last snapshot.
fn play(
    session: &mut SetlistSession,
    fake: &FakeOutput,
    total: usize,
) -> (Vec<f32>, SetlistSnapshot) {
    let mut audio = start(session, fake);
    let mut last = session.poll();
    while audio.len() < 2 * total {
        let (out, snapshot) = step(session, fake);
        audio.extend_from_slice(&out);
        last = snapshot;
    }
    audio.truncate(2 * total);
    (audio, last)
}

fn left(out: &[f32], frame: usize) -> f32 {
    out[2 * frame]
}

#[test]
fn a_one_song_setlist_plays_exactly_as_the_offline_render() {
    let shelf = Shelf::new();
    shelf.song("Songs/A.song", RATE, 0);
    let expected = shelf.offline("Songs/A.song");
    let fake = FakeOutput::new();
    let mut session = shelf.open(vec![song("Songs/A.song", Transition::CueNext)], &fake);

    let (played, snapshot) = play(&mut session, &fake, expected.len() / 2 + 1_000);
    assert_eq!(bits(&played[..expected.len()]), bits(&expected));
    assert!(played[expected.len()..].iter().all(|&s| s == 0.0));
    assert_eq!(snapshot.setlist.current, 0);
    assert_eq!(snapshot.setlist.next, None);
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
}

#[test]
fn autolink_starts_the_next_song_on_the_sample_after_the_previous_one_ends() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let (a, b) = (shelf.offline("Songs/A.song"), shelf.offline("Songs/B.song"));
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::AutoLink),
            song("Songs/B.song", Transition::CueNext),
        ],
        &fake,
    );
    wait_next(&mut session);

    let (la, lb) = (a.len() / 2, b.len() / 2);
    let (played, snapshot) = play(&mut session, &fake, la + lb + 1_000);
    assert_eq!(
        left(&played, la - 1),
        left(&a, la - 1),
        "frame {} is A's last",
        la - 1
    );
    assert_ne!(left(&b, 0), 0.0);
    assert_eq!(left(&played, la), left(&b, 0), "frame {la} is B's first");
    assert_eq!(bits(&played[..2 * la]), bits(&a));
    assert_eq!(bits(&played[2 * la..2 * (la + lb)]), bits(&b));
    assert!(played[2 * (la + lb)..].iter().all(|&s| s == 0.0));
    assert_eq!(snapshot.setlist.current, 1, "B is the current item");
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
}

#[test]
fn crossfades_blend_over_five_seconds_and_stay_below_0_dbfs_at_minus_1_dbfs_peaks() {
    let shelf = Shelf::new();
    shelf
        .steady("Songs/A.song", 12, PEAK, 0.0)
        .steady("Songs/B.song", 12, PEAK, 0.0)
        .steady("Songs/C.song", 12, -PEAK, 0.0);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::Crossfade),
            song("Songs/B.song", Transition::Crossfade),
            song("Songs/C.song", Transition::CueNext),
        ],
        &fake,
    );
    wait_next(&mut session);
    let song_frames = 12 * RATE as usize;
    // A: 0..96 000; B from 56 000 to 152 000; C from 112 000 to 208 000.
    let b_start = song_frames - BLEND;
    let c_start = b_start + song_frames - BLEND;
    let end = c_start + song_frames;

    let mut audio = start(&mut session, &fake);
    let mut blends = Vec::new();
    while audio.len() < 2 * (end + 1_000) {
        let (out, snapshot) = step(&mut session, &fake);
        audio.extend_from_slice(&out);
        if let Some(blend) = snapshot.setlist.blend {
            blends.push(blend);
        }
    }

    let peak = audio.iter().fold(0.0f32, |peak, &s| peak.max(s.abs()));
    assert!(peak < 1.0, "the output reached {peak}, 0 dBFS or more");
    assert!(
        peak <= PEAK * (1.0 + 1e-6),
        "two −1 dBFS songs summed to {peak}"
    );
    for frame in b_start..song_frames {
        let got = left(&audio, frame);
        assert!(
            (got - PEAK).abs() < 1e-6,
            "A and B in phase sum to {got} at {frame}"
        );
    }

    for frame in 0..b_start {
        assert_eq!(left(&audio, frame), PEAK, "A alone, frame {frame}");
    }
    for frame in song_frames..c_start {
        assert_eq!(left(&audio, frame), PEAK, "B alone, frame {frame}");
    }
    for t in 0..BLEND {
        let gain = (t as f64 + 0.5) / BLEND as f64;
        let expected = f64::from(PEAK) * (1.0 - 2.0 * gain);
        let got = f64::from(left(&audio, c_start + t));
        assert!((got - expected).abs() < 1e-6, "blend frame {t}: {got}");
    }
    for frame in c_start + BLEND..end {
        assert_eq!(left(&audio, frame), -PEAK, "C alone, frame {frame}");
    }
    assert!((end..end + 1_000).all(|frame| left(&audio, frame) == 0.0));

    let first = blends.first().expect("the snapshot shows the blend");
    assert_eq!((first.from, first.to, first.frames), (0, 1, BLEND as u64));
    let last = blends.last().unwrap();
    assert_eq!((last.from, last.to), (1, 2));
    assert!(
        blends
            .windows(2)
            .any(|pair| pair[0].played < pair[1].played)
    );
    assert!(
        blends
            .iter()
            .all(|blend| (0.0..=1.0).contains(&blend.progress()))
    );
    assert_eq!(session.poll().setlist.current, 2);
}

#[test]
fn cue_next_stops_and_selects_the_next_item() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let (a, b) = (shelf.offline("Songs/A.song"), shelf.offline("Songs/B.song"));
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::CueNext),
            song("Songs/B.song", Transition::CueNext),
        ],
        &fake,
    );
    let (played, snapshot) = play(&mut session, &fake, a.len() / 2 + 4_000);
    assert_eq!(bits(&played[..a.len()]), bits(&a));
    assert!(played[a.len()..].iter().all(|&s| s == 0.0), "it stops");
    assert_eq!(snapshot.setlist.current, 1, "the next item is selected");
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
    assert_eq!(snapshot.song.output_frame, 0);

    let (played, _) = play(&mut session, &fake, b.len() / 2);
    assert_eq!(bits(&played), bits(&b), "play starts the next item");
}

#[test]
fn stay_stops_and_keeps_the_item_selected() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let a = shelf.offline("Songs/A.song");
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::Stay),
            song("Songs/B.song", Transition::CueNext),
        ],
        &fake,
    );
    let (played, snapshot) = play(&mut session, &fake, a.len() / 2 + 4_000);
    assert!(played[a.len()..].iter().all(|&s| s == 0.0), "it stops");
    assert_eq!(snapshot.setlist.current, 0, "the item stays selected");
    assert_eq!(snapshot.song.transport, TransportState::Stopped);

    let (played, _) = play(&mut session, &fake, a.len() / 2);
    assert_eq!(bits(&played), bits(&a), "play starts it again");
}

#[test]
fn start_and_end_points_play_exactly_those_positions() {
    let shelf = Shelf::new();
    shelf.song("Songs/A.song", RATE, 0);
    let loaded = load_song(&shelf.path("Songs/A.song")).unwrap();
    let excerpt = loaded.song.excerpt("default", 1, 2).unwrap();
    let plan = compile(&excerpt, "default", &CompileOptions::default()).unwrap();
    let expected = render_offline(&plan, &loaded.stems, &MixSettings::default()).unwrap();
    // Verse 1 and chorus: two bars each.
    assert_eq!(expected.len() / 2, 4 * common::BAR as usize);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![SetlistItem::Song(SongItem {
            start_position: Some(1),
            end_position: Some(2),
            ..SongItem::new("Songs/A.song")
        })],
        &fake,
    );
    let labels: Vec<&str> = session
        .song()
        .positions
        .iter()
        .map(|position| position.label.as_str())
        .collect();
    assert_eq!(labels, ["Verse 1", "Chorus"]);

    let (played, _) = play(&mut session, &fake, expected.len() / 2 + 1_000);
    assert_eq!(bits(&played[..expected.len()]), bits(&expected));
    assert!(played[expected.len()..].iter().all(|&s| s == 0.0));
}

#[test]
fn the_fade_out_silences_the_stems_on_the_items_last_frame_while_the_guide_plays_on() {
    let shelf = Shelf::new();
    shelf.steady("Songs/A.song", 7, 0.5, 0.25);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![SetlistItem::Song(SongItem {
            fade_out: true,
            ..SongItem::new("Songs/A.song")
        })],
        &fake,
    );
    let length = 7 * RATE as usize;
    let (played, _) = play(&mut session, &fake, length + 1_000);
    for frame in 0..length - BLEND {
        assert_eq!(left(&played, frame), 0.75, "full level, frame {frame}");
    }
    let middle = length - BLEND / 2;
    assert!((left(&played, middle) - 0.5).abs() < 1e-3, "half way down");
    assert_eq!(
        left(&played, length - 1),
        0.25,
        "the stems are silent at the end"
    );
    assert!((length..length + 1_000).all(|frame| left(&played, frame) == 0.0));
}

/// Frames where the left channel turns on after silence.
fn onsets(out: &[f32]) -> Vec<usize> {
    (0..out.len() / 2)
        .filter(|&f| left(out, f) != 0.0 && (f == 0 || left(out, f - 1) == 0.0))
        .collect()
}

#[test]
fn a_click_tile_with_a_bar_count_plays_its_bars_then_follows_its_transition() {
    let shelf = Shelf::new();
    shelf.song("Songs/A.song", RATE, 0);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            click(Some(2), Transition::CueNext),
            song("Songs/A.song", Transition::CueNext),
        ],
        &fake,
    );
    assert_eq!(session.song().title, "Click");
    let bar = common::BAR as usize;
    let (played, snapshot) = play(&mut session, &fake, 3 * bar);
    let beats: Vec<usize> = (0..8).map(|beat| beat * bar / 4).collect();
    assert_eq!(onsets(&played), beats, "two bars of 4/4, then silence");
    assert_eq!(snapshot.setlist.current, 1, "Cue next selected the song");
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
}

#[test]
fn a_click_tile_without_a_bar_count_plays_until_the_operator_moves_on() {
    let shelf = Shelf::new();
    shelf.song("Songs/A.song", RATE, 0);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            click(None, Transition::AutoLink),
            song("Songs/A.song", Transition::CueNext),
        ],
        &fake,
    );
    wait_next(&mut session);
    let bar = common::BAR as usize;
    let (played, snapshot) = play(&mut session, &fake, 5 * bar);
    let beats: Vec<usize> = (0..20).map(|beat| beat * bar / 4).collect();
    assert_eq!(onsets(&played), beats, "the bar repeats");
    assert_eq!(snapshot.song.transport, TransportState::Playing);
    assert_eq!(snapshot.setlist.current, 0);

    session.send(SetlistCommand::Next).unwrap();
    let (_, snapshot) = step(&mut session, &fake);
    assert_eq!(snapshot.setlist.current, 1);
    let mut after = Vec::new();
    for _ in 0..2 * bar / BUFFER {
        after.extend(step(&mut session, &fake).0);
    }
    assert!(after.iter().all(|&s| s == 0.0), "no click once moved on");
    assert_eq!(session.poll().song.transport, TransportState::Stopped);
}

#[test]
fn next_previous_and_select_move_while_stopped() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10)
        .song("Songs/C.song", RATE, 20);
    let b = shelf.offline("Songs/B.song");
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::CueNext),
            song("Songs/B.song", Transition::CueNext),
            song("Songs/C.song", Transition::CueNext),
        ],
        &fake,
    );
    let current = |session: &mut SetlistSession| session.poll().setlist.current;
    assert_eq!(current(&mut session), 0);
    session.send(SetlistCommand::Next).unwrap();
    assert_eq!(current(&mut session), 1);
    session.send(SetlistCommand::Next).unwrap();
    assert_eq!(current(&mut session), 2);
    assert_eq!(
        session.send(SetlistCommand::Next),
        Err(SetlistCommandError::NoNext)
    );
    session.send(SetlistCommand::Previous).unwrap();
    assert_eq!(current(&mut session), 1);
    session.send(SetlistCommand::Select(0)).unwrap();
    assert_eq!(current(&mut session), 0);
    assert_eq!(
        session.send(SetlistCommand::Previous),
        Err(SetlistCommandError::NoPrevious)
    );
    assert_eq!(
        session.send(SetlistCommand::Select(3)),
        Err(SetlistCommandError::OutOfRange { item: 3, items: 3 })
    );
    session.send(SetlistCommand::Select(1)).unwrap();
    assert_eq!(current(&mut session), 1);
    let (played, snapshot) = play(&mut session, &fake, b.len() / 2 + 1_000);
    assert_eq!(
        snapshot.setlist.current, 2,
        "B's Cue next selected C at its end"
    );
    let played = &played[..b.len()];
    assert_eq!(
        bits(played),
        bits(&b),
        "the selected item plays from its start"
    );
}

#[test]
fn next_and_previous_while_playing_ramp_the_item_out_and_select_the_other_stopped() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let (a, b) = (shelf.offline("Songs/A.song"), shelf.offline("Songs/B.song"));
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::AutoLink),
            song("Songs/B.song", Transition::CueNext),
        ],
        &fake,
    );
    wait_next(&mut session);
    play(&mut session, &fake, 20_000);

    session.send(SetlistCommand::Next).unwrap();
    let (ramp, snapshot) = step(&mut session, &fake);
    assert!(ramp[..64].iter().any(|&s| s != 0.0), "A ramps out");
    let silent_from = (0..BUFFER).rev().find(|&f| left(&ramp, f) != 0.0).unwrap() + 1;
    assert!(
        silent_from <= DECLICK_FRAMES as usize,
        "within the declick ramp"
    );
    let (after, _) = step(&mut session, &fake);
    assert!(after.iter().all(|&s| s == 0.0));
    assert_eq!(snapshot.setlist.current, 1);
    assert_eq!(snapshot.song.output_frame, 0, "B waits at its start");
    let (played, _) = play(&mut session, &fake, 20_000);
    assert_eq!(
        bits(&played),
        bits(&b[..2 * 20_000]),
        "B plays from its start"
    );

    session.send(SetlistCommand::Previous).unwrap();
    let (ramp, snapshot) = step(&mut session, &fake);
    assert!(ramp[..64].iter().any(|&s| s != 0.0), "B ramps out");
    let (after, _) = step(&mut session, &fake);
    assert!(after.iter().all(|&s| s == 0.0));
    assert_eq!(snapshot.setlist.current, 0);
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
    let (played, _) = play(&mut session, &fake, 20_000);
    assert_eq!(
        bits(&played),
        bits(&a[..2 * 20_000]),
        "A plays from its start"
    );
}

#[test]
fn unavailable_and_other_rate_songs_are_reported_before_playing_and_the_rest_still_plays() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/Hi.song", 2 * RATE, 10)
        .song("Songs/C.song", RATE, 20);
    let a = shelf.offline("Songs/A.song");
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/Missing.song", Transition::CueNext),
            song("Songs/A.song", Transition::AutoLink),
            song("Songs/Hi.song", Transition::CueNext),
            song("Songs/C.song", Transition::CueNext),
        ],
        &fake,
    );
    let info = session.info().clone();
    assert_eq!(
        info.sample_rate, RATE,
        "the first song that plays sets the rate"
    );
    assert_eq!(info.unavailable(), [0]);
    assert!(
        info.items[0]
            .problems
            .iter()
            .any(|p| p.contains("Songs/Missing.song"))
    );
    assert_eq!(info.other_rate(), [2]);
    assert_eq!(info.items[2].sample_rate, Some(2 * RATE));
    assert_eq!(fake.opened()[0].sample_rate, RATE);
    assert_eq!(session.poll().setlist.current, 1, "the first playable item");
    assert!(matches!(
        session.send(SetlistCommand::Select(0)),
        Err(SetlistCommandError::Unplayable { item: 0 })
    ));

    let (played, snapshot) = play(&mut session, &fake, a.len() / 2 + 4_000);
    assert_eq!(bits(&played[..a.len()]), bits(&a));
    assert!(
        played[a.len()..].iter().all(|&s| s == 0.0),
        "no AutoLink across rates"
    );
    assert_eq!(snapshot.setlist.current, 2, "it falls back to Cue next");
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
    assert_eq!(fake.opened().last().unwrap().sample_rate, 2 * RATE);

    let (played, _) = play(&mut session, &fake, 10_000);
    assert!(
        played.iter().any(|&s| s != 0.0),
        "the other-rate song plays"
    );
    session.send(SetlistCommand::Next).unwrap();
    for _ in 0..10 {
        step(&mut session, &fake);
    }
    let snapshot = session.poll();
    assert_eq!(snapshot.setlist.current, 3);
    assert_eq!(fake.opened().last().unwrap().sample_rate, RATE);
    assert_eq!(snapshot.song.transport, TransportState::Stopped);
}

#[test]
fn a_setlist_with_nothing_playable_does_not_open() {
    let shelf = Shelf::new();
    let mut setlist = Setlist::new("Empty");
    setlist.items = vec![song("Songs/Missing.song", Transition::CueNext)];
    let entry = shelf.library.save_setlist(&setlist).unwrap();
    let fake = FakeOutput::new();
    let result = SetlistSession::open(
        &shelf.library,
        &entry.file,
        Box::new(fake.clone()),
        options(),
    );
    assert!(matches!(
        result,
        Err(app::SetlistOpenError::NothingPlayable { .. })
    ));
    assert!(fake.opened().is_empty());
}
