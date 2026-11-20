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

    /// Saves a setlist of `items` and opens it on `fake` with `options`.
    fn open_with(
        &self,
        items: Vec<SetlistItem>,
        fake: &FakeOutput,
        options: SessionOptions,
    ) -> SetlistSession {
        let mut setlist = Setlist::new("Sunday");
        setlist.items = items;
        let entry = self.library.save_setlist(&setlist).unwrap();
        SetlistSession::open(&self.library, &entry.file, Box::new(fake.clone()), options)
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

fn shelf_ab(transition: Transition) -> (Shelf, FakeOutput, SetlistSession) {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let fake = FakeOutput::new();
    let session = shelf.open(
        vec![
            song("Songs/A.song", transition),
            song("Songs/B.song", Transition::CueNext),
        ],
        &fake,
    );
    (shelf, fake, session)
}

/// the jump timing and fade duration the operator sets while the next
/// item is cued must hold when that item starts.
#[test]
fn operator_settings_sent_while_the_next_item_is_cued_hold_when_it_starts() {
    use engine::live::JumpTiming;
    use std::time::Duration;
    let (shelf, fake, mut session) = shelf_ab(Transition::AutoLink);
    let a = shelf.offline("Songs/A.song");
    wait_next(&mut session);
    assert!(session.poll().setlist.next_ready, "B is cued");
    session
        .send(SetlistCommand::Item(Command::SetJumpTiming(
            JumpTiming::NextBar,
        )))
        .unwrap();
    session
        .send(SetlistCommand::Item(Command::SetFadeDuration(
            Duration::from_millis(500),
        )))
        .unwrap();
    let (_, during) = step(&mut session, &fake);
    assert_eq!(during.song.jump_timing, JumpTiming::NextBar, "A takes it");
    let (_, snapshot) = play(&mut session, &fake, a.len() / 2 + 2_000);
    assert_eq!(snapshot.setlist.current, 1, "B followed on");
    assert_eq!(snapshot.song.transport, TransportState::Playing);
    // B's fade: toggle it and count the frames to silence of the tracks.
    session
        .send(SetlistCommand::Item(Command::ToggleFade))
        .unwrap();
    let mut frames = 0;
    let mut last = session.poll();
    while last.song.fade.level > 0.0 && frames < 100_000 {
        let (out, snapshot) = step(&mut session, &fake);
        frames += out.len() / 2;
        last = snapshot;
    }
    assert_eq!(
        (snapshot.song.jump_timing, frames <= 4_000 + BUFFER),
        (JumpTiming::NextBar, true),
        "B started with jump timing {:?} and faded in {frames} frames (0.5 s is 4 000); \
         the snapshot said {:?}",
        snapshot.song.jump_timing,
        snapshot.song.fade.duration
    );
}

/// a panic in the last half second of an item that AutoLinks must not
/// start the next item.
#[test]
fn a_panic_in_the_last_half_second_does_not_start_the_next_item() {
    let (shelf, fake, mut session) = shelf_ab(Transition::AutoLink);
    let a = shelf.offline("Songs/A.song");
    wait_next(&mut session);
    let la = a.len() / 2;
    let (_, _) = play(&mut session, &fake, la - 2_048);
    session.send(SetlistCommand::Item(Command::Panic)).unwrap();
    let mut after = Vec::new();
    let mut last = session.poll();
    while after.len() < 2 * 16_000 {
        let (out, snapshot) = step(&mut session, &fake);
        after.extend_from_slice(&out);
        last = snapshot;
    }
    let loud = after[2 * 6_000..].iter().filter(|s| s.abs() > 0.0).count();
    assert_eq!(
        (last.song.transport, loud),
        (TransportState::Stopped, 0),
        "after the panic: item {} {:?}, {loud} non-zero samples",
        last.setlist.current,
        last.song.transport
    );
}

/// A pause whose ramp runs past an AutoLink item's end stops there; the
/// next item does not start.
#[test]
fn a_pause_right_before_the_end_does_not_start_the_next_item() {
    let (shelf, fake, mut session) = shelf_ab(Transition::AutoLink);
    let a = shelf.offline("Songs/A.song");
    wait_next(&mut session);
    let la = a.len() / 2;
    play(&mut session, &fake, la - 128);
    session.send(SetlistCommand::Item(Command::Pause)).unwrap();
    let mut after = Vec::new();
    let mut last = session.poll();
    while after.len() < 2 * 8_000 {
        let (out, snapshot) = step(&mut session, &fake);
        after.extend_from_slice(&out);
        last = snapshot;
    }
    let loud = after[2 * 1_000..].iter().filter(|s| s.abs() > 0.0).count();
    assert_eq!(
        (last.setlist.current, loud),
        (0, 0),
        "after the pause: item {} {:?}, {loud} non-zero samples",
        last.setlist.current,
        last.song.transport
    );
}

/// The late path: the next item was not buffered when an AutoLink item
/// ended, and the operator stopped before the session saw the end. The
/// next item is selected (Cue next) but does not start.
#[test]
fn an_operator_stop_before_a_late_autolink_is_seen_keeps_the_next_item_stopped() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let la = shelf.offline("Songs/A.song").len() / 2;
    let (gate, open) = common::closed_gate();
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut setlist = Setlist::new("Late");
    setlist.items = vec![
        song("Songs/A.song", Transition::AutoLink),
        song("Songs/B.song", Transition::CueNext),
    ];
    let entry = shelf.library.save_setlist(&setlist).unwrap();
    let fake = FakeOutput::new();
    let mut session = SetlistSession::open(
        &shelf.library,
        &entry.file,
        Box::new(fake.clone()),
        SessionOptions {
            disk_gate: Some(gate),
            ..options()
        },
    )
    .unwrap();
    wait_disks(&session);
    // A is buffered; B's audio is held back from now on.
    open.store(false, std::sync::atomic::Ordering::SeqCst);
    let snapshot = session.poll();
    assert!(!snapshot.setlist.next_ready, "B is not buffered");

    start(&mut session, &fake);
    // No poll from here on until the operator stops: the session has not
    // seen A end.
    let mut frames = BUFFER;
    while frames < la + 1_000 {
        wait_disks(&session);
        fake.render(BUFFER).expect("a stream plays");
        frames += BUFFER;
    }
    session.send(SetlistCommand::Item(Command::Stop)).unwrap();
    let snapshot = session.poll();
    assert_eq!(snapshot.setlist.current, 1, "B is selected");
    open.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut after = Vec::new();
    for _ in 0..40 {
        after.extend(step(&mut session, &fake).0);
    }
    assert!(after.iter().all(|&s| s == 0.0), "B does not start");
    assert_eq!(session.poll().song.transport, TransportState::Stopped);
}

/// Sends item commands until the audio thread's queue refuses one: nothing
/// is rendered meanwhile, so the next message the session sends is refused
/// too.
fn fill_the_queue(session: &mut SetlistSession) {
    let timing = Command::SetJumpTiming(app::JumpTiming::SectionEnd);
    for _ in 0..1_000 {
        if let Err(error) = session.send(SetlistCommand::Item(timing)) {
            assert!(matches!(
                error,
                SetlistCommandError::Runtime(app::RuntimeError::QueueFull { .. })
            ));
            return;
        }
    }
    panic!("the queue never filled");
}

fn queue_full(error: &SetlistCommandError) -> bool {
    matches!(
        error,
        SetlistCommandError::Runtime(app::RuntimeError::QueueFull { .. })
    )
}

/// A at the device's rate, then Hi at twice it.
fn shelf_other_rate() -> (Shelf, FakeOutput, SetlistSession) {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/Hi.song", 2 * RATE, 10);
    let fake = FakeOutput::new();
    let session = shelf.open(
        vec![
            song("Songs/A.song", Transition::CueNext),
            song("Songs/Hi.song", Transition::CueNext),
        ],
        &fake,
    );
    (shelf, fake, session)
}

/// Steps until the device reopened for item 1, then a few buffers more.
fn until_reopened(session: &mut SetlistSession, fake: &FakeOutput) -> Vec<f32> {
    for _ in 0..100 {
        let (_, snapshot) = step(session, fake);
        if snapshot.setlist.current == 1 {
            let mut after = Vec::new();
            for _ in 0..20 {
                after.extend(step(session, fake).0);
            }
            return after;
        }
    }
    panic!("the device never reopened for item 1");
}

#[test]
fn the_last_transport_command_wins_while_a_reopen_waits() {
    for halt in [Command::Stop, Command::Pause, Command::Panic] {
        let (_shelf, fake, mut session) = shelf_other_rate();
        play(&mut session, &fake, 4_000);
        // Hi runs at another rate: the device reopens once A has stopped.
        session.send(SetlistCommand::Next).unwrap();
        session.send(SetlistCommand::Item(Command::Play)).unwrap();
        session.send(SetlistCommand::Item(halt)).unwrap();
        let after = until_reopened(&mut session, &fake);
        assert!(
            after.iter().all(|&s| s == 0.0),
            "{halt:?} after Play: the reopened item stays silent"
        );
        assert_eq!(session.poll().song.transport, TransportState::Stopped);
        let (played, _) = play(&mut session, &fake, 2_000);
        assert!(played.iter().any(|&s| s != 0.0), "Play still plays it");
    }

    let (_shelf, fake, mut session) = shelf_other_rate();
    play(&mut session, &fake, 4_000);
    session.send(SetlistCommand::Next).unwrap();
    for command in [Command::Play, Command::Stop, Command::Play] {
        session.send(SetlistCommand::Item(command)).unwrap();
    }
    let after = until_reopened(&mut session, &fake);
    assert!(after.iter().any(|&s| s != 0.0), "Play after Stop plays it");
}

#[test]
fn a_select_refused_before_a_reopen_keeps_the_item_the_rate_and_the_stream() {
    let (_shelf, fake, mut session) = shelf_other_rate();
    session.poll();
    fill_the_queue(&mut session);
    let error = session.send(SetlistCommand::Next).unwrap_err();
    assert!(queue_full(&error), "the refusal is reported: {error}");

    let snapshot = session.poll();
    assert_eq!(snapshot.setlist.current, 0, "A is still the item");
    assert_eq!(fake.opened().len(), 1, "the device was not reopened");
    let app::DeviceStatus::Connected(stream) = &snapshot.song.device else {
        panic!("the stream is still open: {:?}", snapshot.song.device);
    };
    assert_eq!(stream.sample_rate, RATE);
    assert_eq!(session.song().sample_rate, RATE);
    assert!(matches!(
        snapshot.song.last_error,
        Some(app::RuntimeError::QueueFull { .. })
    ));

    for _ in 0..4 {
        step(&mut session, &fake);
    }
    session.send(SetlistCommand::Next).unwrap();
    assert_eq!(session.poll().setlist.current, 1, "once taken in, it works");
    assert_eq!(fake.opened().last().unwrap().sample_rate, 2 * RATE);
}

/// [A AutoLink, B, C], B cued.
fn shelf_abc() -> (Shelf, FakeOutput, SetlistSession) {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10)
        .song("Songs/C.song", RATE, 20);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::AutoLink),
            song("Songs/B.song", Transition::CueNext),
            song("Songs/C.song", Transition::CueNext),
        ],
        &fake,
    );
    wait_next(&mut session);
    (shelf, fake, session)
}

/// After a refused message: the B cued first (watched through `cued_b`)
/// keeps its disk thread, so no second copy replaced it, and A's AutoLink
/// hands over to it with the setlist following.
fn assert_still_follows_b(
    shelf: &Shelf,
    fake: &FakeOutput,
    session: &mut SetlistSession,
    cued_b: &library::DiskMonitor,
) {
    assert_eq!(session.disk_monitors().len(), 2, "A and the cued B");
    for _ in 0..4 {
        step(session, fake);
    }
    let la = shelf.offline("Songs/A.song").len() / 2;
    let (_, snapshot) = play(session, fake, la + 4_000);
    assert_eq!(
        snapshot.setlist.current, 1,
        "the setlist follows A's AutoLink"
    );
    assert_eq!(snapshot.song.transport, TransportState::Playing);
    assert!(
        cued_b.is_running(),
        "the B that plays is the one cued first"
    );
}

#[test]
fn a_refused_select_of_the_cued_item_keeps_following_its_hand_over() {
    let (shelf, fake, mut session) = shelf_abc();
    let cued_b = session.disk_monitors()[1].clone();
    fill_the_queue(&mut session);
    let error = session.send(SetlistCommand::Next).unwrap_err();
    assert!(queue_full(&error), "{error}");
    assert_eq!(session.poll().setlist.current, 0);
    assert_still_follows_b(&shelf, &fake, &mut session, &cued_b);
}

#[test]
fn a_refused_uncue_keeps_following_the_cued_items_hand_over() {
    let (shelf, fake, mut session) = shelf_abc();
    let cued_b = session.disk_monitors()[1].clone();
    fill_the_queue(&mut session);
    let error = session.send(SetlistCommand::Select(2)).unwrap_err();
    assert!(queue_full(&error), "{error}");
    assert_eq!(session.poll().setlist.current, 0);
    assert_still_follows_b(&shelf, &fake, &mut session, &cued_b);
}

#[test]
fn a_range_the_song_no_longer_has_is_reported_and_never_played_whole() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/B.song", RATE, 10);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::CueNext),
            SetlistItem::Song(SongItem {
                start_position: Some(2),
                end_position: Some(3),
                ..SongItem::new("Songs/B.song")
            }),
        ],
        &fake,
    );
    // The setlist was checked against four positions; the song now has two.
    let manifest = shelf.path("Songs/B.song").join("song.json");
    let mut package =
        domain::SongPackage::from_json(&std::fs::read_to_string(&manifest).unwrap()).unwrap();
    package.arrangements[0].sections.truncate(2);
    std::fs::write(&manifest, package.to_json().unwrap()).unwrap();

    let mut snapshot = session.poll();
    for _ in 0..10 {
        wait_disks(&session);
        snapshot = session.poll();
    }
    assert!(!snapshot.setlist.next_ready, "nothing of B is prepared");
    let Some(app::SetlistProblem::Prepare { item: 1, error }) = &snapshot.setlist.problem else {
        panic!("item 2's range is reported: {:?}", snapshot.setlist.problem);
    };
    assert!(
        error.contains("positions"),
        "the message names the range: {error}"
    );
    assert!(matches!(
        session.send(SetlistCommand::Next),
        Err(SetlistCommandError::Prepare { item: 1, .. })
    ));
    assert_eq!(session.poll().setlist.current, 0);
}

#[test]
fn a_transition_the_audio_thread_refuses_reads_as_refused_not_as_unprepared() {
    let (shelf, fake, mut session) = shelf_ab(Transition::CueNext);
    let la = shelf.offline("Songs/A.song").len() / 2;
    wait_next(&mut session);
    start(&mut session, &fake);
    // A plays to its end with no poll, then the queue fills before the
    // session sees the end.
    let mut frames = BUFFER;
    while frames < la + 1_000 {
        wait_disks(&session);
        fake.render(BUFFER).expect("a stream plays");
        frames += BUFFER;
    }
    fill_the_queue(&mut session);
    let snapshot = session.poll();
    let problem = snapshot.setlist.problem.expect("the transition failed");
    assert!(
        matches!(
            &problem,
            app::SetlistProblem::Transition {
                item: 1,
                error: SetlistCommandError::Runtime(app::RuntimeError::QueueFull { .. }),
            }
        ),
        "{problem:?}"
    );
    assert!(!problem.to_string().contains("prepared"), "{problem}");
    assert_eq!(snapshot.setlist.current, 0);
    for _ in 0..4 {
        step(&mut session, &fake);
    }
    session.send(SetlistCommand::Next).unwrap();
    assert_eq!(session.poll().setlist.current, 1, "B can still be selected");
}

#[test]
fn a_device_refusing_the_other_rate_holds_the_item_as_lost_until_a_rate_it_takes() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/Hi.song", 2 * RATE, 10)
        .song("Songs/C.song", RATE, 20);
    let fake = FakeOutput::new();
    fake.set_rates(app::testing::FAKE_DEVICE_ID, Some(&[RATE]));
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::CueNext),
            song("Songs/Hi.song", Transition::CueNext),
            song("Songs/C.song", Transition::CueNext),
        ],
        &fake,
    );
    session.send(SetlistCommand::Next).unwrap();
    let held = session.poll();
    assert_eq!(held.setlist.current, 1, "Hi is selected");
    assert!(
        matches!(
            &held.song.device,
            app::DeviceStatus::Disconnected {
                reopen_error: Some(app::OutputError::UnsupportedSampleRate { .. }),
                ..
            }
        ),
        "{:?}",
        held.song.device
    );
    assert!(matches!(
        held.song.last_error,
        Some(app::RuntimeError::DeviceReopenFailed { .. })
    ));
    assert!(fake.render(BUFFER).is_none(), "nothing plays");
    assert!(session.send(SetlistCommand::Item(Command::Play)).is_err());

    session.send(SetlistCommand::Next).unwrap();
    let back = session.poll();
    assert_eq!(back.setlist.current, 2);
    assert!(matches!(back.song.device, app::DeviceStatus::Connected(_)));
    assert_eq!(back.song.last_error, None);
    let c = shelf.offline("Songs/C.song");
    let (played, _) = play(&mut session, &fake, 8_000);
    assert_eq!(
        bits(&played),
        bits(&c[..2 * 8_000]),
        "C plays at the device's rate"
    );
}

#[test]
fn a_late_autolink_or_crossfade_starts_the_next_item_once_buffered_and_says_so() {
    for transition in [Transition::AutoLink, Transition::Crossfade] {
        let shelf = Shelf::new();
        shelf.song("Songs/A.song", RATE, 0);
        // B's track stem is held back until released: only B has one.
        shelf.steady("Songs/B.song", 7, 0.5, 0.25);
        let (gate, open) = common::closed_gate();
        let held = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let hold = std::sync::Arc::clone(&held);
        let gate: library::Gate = std::sync::Arc::new(move |stem: &str, range| {
            !(stem == "track" && hold.load(std::sync::atomic::Ordering::SeqCst))
                && gate(stem, range)
        });
        open.store(true, std::sync::atomic::Ordering::SeqCst);
        let fake = FakeOutput::new();
        let mut session = shelf.open_with(
            vec![
                song("Songs/A.song", transition),
                song("Songs/B.song", Transition::CueNext),
            ],
            &fake,
            SessionOptions {
                disk_gate: Some(gate),
                ..options()
            },
        );
        let la = shelf.offline("Songs/A.song").len() / 2;
        let (_, at_end) = play(&mut session, &fake, la + 2_000);
        assert_eq!(at_end.setlist.current, 1, "{transition:?}: B is selected");
        assert_eq!(
            at_end.song.transport,
            TransportState::Starting,
            "{transition:?}"
        );
        assert_eq!(
            at_end.setlist.problem,
            Some(app::SetlistProblem::LateLink { item: 1 }),
            "{transition:?}"
        );

        held.store(false, std::sync::atomic::Ordering::SeqCst);
        let mut played = Vec::new();
        let mut last = session.poll();
        for _ in 0..40 {
            let (out, snapshot) = step(&mut session, &fake);
            played.extend(out);
            last = snapshot;
        }
        assert_eq!(
            last.song.transport,
            TransportState::Playing,
            "{transition:?}"
        );
        assert!(
            played.iter().any(|&s| (s - 0.75).abs() < 1e-6),
            "{transition:?}: B plays once buffered"
        );
        assert_eq!(
            last.setlist.problem,
            Some(app::SetlistProblem::LateLink { item: 1 })
        );
    }
}

/// An output that keeps every renderer it was given, even after its stream
/// closes, so a lost stream never hands the session's deck back: the
/// session ends up without a player ([`app::RuntimeError::PlayerLost`]).
#[derive(Clone, Default)]
struct KeepsRenderers {
    kept: std::sync::Arc<std::sync::Mutex<Vec<app::AudioRenderer>>>,
    lost: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct KeptStream {
    info: app::StreamInfo,
    lost: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl app::AudioOutput for KeepsRenderers {
    fn devices(&self) -> Result<Vec<app::DeviceInfo>, app::OutputError> {
        Ok(Vec::new())
    }

    fn open(
        &mut self,
        request: &app::OutputRequest,
        renderer: app::AudioRenderer,
    ) -> Result<Box<dyn app::OutputStream>, app::OutputError> {
        self.kept.lock().unwrap().push(renderer);
        Ok(Box::new(KeptStream {
            info: app::StreamInfo {
                device: "Keeper".to_owned(),
                device_id: "keeper".to_owned(),
                sample_rate: request.sample_rate,
                buffer_frames: request.buffer_frames,
                channels: 2,
            },
            lost: std::sync::Arc::clone(&self.lost),
        }))
    }
}

impl app::OutputStream for KeptStream {
    fn info(&self) -> &app::StreamInfo {
        &self.info
    }

    fn health(&self) -> app::StreamHealth {
        if self.lost.load(std::sync::atomic::Ordering::SeqCst) {
            app::StreamHealth::Lost {
                reason: app::LossReason::Unplugged,
            }
        } else {
            app::StreamHealth::Running
        }
    }
}

#[test]
fn a_reopen_with_no_player_left_sends_nothing_and_keeps_describing_the_item() {
    let shelf = Shelf::new();
    shelf
        .song("Songs/A.song", RATE, 0)
        .song("Songs/Hi.song", 2 * RATE, 10);
    let mut setlist = Setlist::new("Sunday");
    setlist.items = vec![
        song("Songs/A.song", Transition::CueNext),
        song("Songs/Hi.song", Transition::CueNext),
    ];
    let entry = shelf.library.save_setlist(&setlist).unwrap();
    let output = KeepsRenderers::default();
    let mut session = SetlistSession::open(
        &shelf.library,
        &entry.file,
        Box::new(output.clone()),
        options(),
    )
    .unwrap();
    session.poll();
    wait_disks(&session);
    let monitors = session.disk_monitors();
    assert_eq!(monitors.len(), 2, "A and Hi, prepared ahead");
    let hi = monitors[1].clone();
    assert!(hi.is_running());

    // The stream is lost and the output keeps the deck: no player is left.
    output.lost.store(true, std::sync::atomic::Ordering::SeqCst);
    let lost = session.poll();
    assert_eq!(lost.song.last_error, Some(app::RuntimeError::PlayerLost));

    let error = session.send(SetlistCommand::Next).unwrap_err();
    assert_eq!(
        error,
        SetlistCommandError::Runtime(app::RuntimeError::PlayerLost)
    );
    let snapshot = session.poll();
    assert_eq!(snapshot.setlist.current, 0, "the session still describes A");
    assert_eq!(session.song().sample_rate, RATE);
    assert!(
        !hi.is_running(),
        "Hi was dropped here, not sent to a deck nobody plays"
    );
}

#[test]
fn a_first_item_that_cannot_be_built_is_reported_and_the_setlist_opens_at_the_next() {
    let shelf = Shelf::new();
    // 1 kHz is too low to stream, though the song itself is valid.
    shelf
        .song("Songs/Low.song", 1_000, 0)
        .song("Songs/A.song", RATE, 10);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/Low.song", Transition::CueNext),
            song("Songs/A.song", Transition::CueNext),
        ],
        &fake,
    );
    let info = session.info().clone();
    assert_eq!(info.unavailable(), [0], "Low is reported");
    assert!(
        info.items[0].problems.iter().any(|p| p.contains("too low")),
        "{:?}",
        info.items[0].problems
    );
    assert_eq!(info.sample_rate, RATE, "the device opens at A's rate");
    assert_eq!(fake.opened()[0].sample_rate, RATE);
    assert_eq!(session.poll().setlist.current, 1);
    assert_eq!(
        session.send(SetlistCommand::Previous),
        Err(SetlistCommandError::NoPrevious),
        "Low is skipped"
    );
}

#[test]
fn a_setlist_whose_items_all_fail_to_build_has_nothing_playable_with_every_reason() {
    let shelf = Shelf::new();
    shelf.song("Songs/Low.song", 1_000, 0);
    let mut setlist = Setlist::new("Low");
    setlist.items = vec![
        song("Songs/Missing.song", Transition::CueNext),
        song("Songs/Low.song", Transition::CueNext),
    ];
    let entry = shelf.library.save_setlist(&setlist).unwrap();
    let fake = FakeOutput::new();
    let result = SetlistSession::open(
        &shelf.library,
        &entry.file,
        Box::new(fake.clone()),
        options(),
    );
    let Err(app::SetlistOpenError::NothingPlayable { name, problems }) = result else {
        panic!("nothing can play: {:?}", result.err());
    };
    assert_eq!(name, "Low");
    assert!(problems[0].iter().any(|p| p.contains("Songs/Missing.song")));
    assert!(
        problems[1].iter().any(|p| p.contains("too low")),
        "{problems:?}"
    );
    assert!(fake.opened().is_empty());
}

/// Tests wait on these monitors before each buffer: the item a crossfade
/// fades out is still heard, so its disk is watched until the blend ends.
#[test]
fn the_item_a_crossfade_fades_out_stays_watched_until_the_blend_ends() {
    let shelf = Shelf::new();
    shelf
        .steady("Songs/A.song", 7, PEAK, 0.0)
        .steady("Songs/B.song", 7, PEAK, 0.0);
    let fake = FakeOutput::new();
    let mut session = shelf.open(
        vec![
            song("Songs/A.song", Transition::Crossfade),
            song("Songs/B.song", Transition::CueNext),
        ],
        &fake,
    );
    wait_next(&mut session);
    assert_eq!(session.disk_monitors().len(), 2, "A, and B cued");
    let mut during = None;
    let mut after = None;
    let mut audio = start(&mut session, &fake);
    while audio.len() < 2 * 14 * RATE as usize {
        let (out, snapshot) = step(&mut session, &fake);
        audio.extend(out);
        let watched = session.disk_monitors().len();
        if snapshot.setlist.blend.is_some() {
            during.get_or_insert(watched);
        } else if snapshot.setlist.current == 1 {
            after.get_or_insert(watched);
        }
    }
    assert_eq!(during, Some(2), "B, and A while it fades out");
    assert_eq!(after, Some(1), "B alone once the blend is over");
}
