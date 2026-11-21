//! F8.5 through the `app` API: the buffer size changes while a session
//! plays, by opening the stream again, without rebuilding the players.

mod common;

use std::time::Duration;

use app::testing::FakeOutput;
use app::{
    Command, DeviceStatus, MAX_BUFFER_FRAMES, RuntimeError, SessionOptions, SetlistCommand,
    SetlistSession, TransportState,
};
use common::{bits, offline, open, play_for, start, write_song, write_song_with};
use domain::{DEFAULT_SEAM_CROSSFADE, Setlist, SetlistItem, SongItem, Transition};
use library::Library;

#[test]
fn the_buffer_size_changes_while_playing_and_the_audio_goes_on_sample_exact() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), common::RATE);
    let expected = offline(dir.path(), "default", DEFAULT_SEAM_CROSSFADE);
    let fake = FakeOutput::new();
    let mut session = open(
        dir.path(),
        &fake,
        SessionOptions {
            buffer_frames: 64,
            ..SessionOptions::default()
        },
    );
    let (first, _) = start(&mut session, &fake, 64);
    let (before, _) = play_for(&mut session, &fake, 64, 20_000);

    session.set_buffer_frames(MAX_BUFFER_FRAMES).unwrap();
    assert_eq!(fake.opened().len(), 2, "the stream opened again");
    assert_eq!(fake.opened()[1].buffer_frames, MAX_BUFFER_FRAMES);
    let snapshot = session.poll();
    let DeviceStatus::Connected(info) = &snapshot.device else {
        panic!("the device plays");
    };
    assert_eq!(info.buffer_frames, MAX_BUFFER_FRAMES);
    assert_eq!(info.latency().as_secs_f64(), 1_024.0 / 8_000.0, "128 ms");
    assert_eq!(snapshot.transport, TransportState::Playing, "it plays on");

    let played = first.len() + before.len();
    let (after, _) = play_for(
        &mut session,
        &fake,
        MAX_BUFFER_FRAMES as usize,
        (expected.len() - played) / 2,
    );
    let all = [first, before, after].concat();
    assert_eq!(
        bits(&all[..expected.len()]),
        bits(&expected),
        "no frame lost or repeated across the change"
    );

    // The same size again opens nothing.
    session.set_buffer_frames(MAX_BUFFER_FRAMES).unwrap();
    assert_eq!(fake.opened().len(), 2);
}

#[test]
fn a_buffer_size_outside_64_to_1024_frames_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), common::RATE);
    let fake = FakeOutput::new();
    let mut session = open(dir.path(), &fake, SessionOptions::default());
    for frames in [0, 63, 1_025] {
        let error = session.set_buffer_frames(frames).unwrap_err();
        assert_eq!(error, RuntimeError::BufferSize { frames });
        assert_eq!(session.poll().last_error, Some(error));
    }
    assert_eq!(
        RuntimeError::BufferSize { frames: 32 }.to_string(),
        "the buffer size must be 64 to 1024 frames, not 32"
    );
    assert_eq!(fake.opened().len(), 1, "nothing opened again");
}

#[test]
fn a_setlist_changes_the_buffer_size_for_the_items_that_follow_too() {
    let dir = tempfile::tempdir().unwrap();
    write_song_with(&dir.path().join("first"), common::RATE, 0);
    write_song_with(&dir.path().join("second"), common::RATE, 9);
    let library = Library::new(dir.path());
    let mut setlist = Setlist::new("Sunday");
    setlist.items = ["first", "second"]
        .into_iter()
        .map(|path| {
            SetlistItem::Song(SongItem {
                transition: Transition::AutoLink,
                ..SongItem::new(path)
            })
        })
        .collect();
    let file = library.save_setlist(&setlist).unwrap().file;
    let fake = FakeOutput::new();
    let mut session = SetlistSession::open(
        &library,
        &file,
        Box::new(fake.clone()),
        SessionOptions::default(),
    )
    .unwrap();
    session.send(SetlistCommand::Item(Command::Play)).unwrap();
    session.set_buffer_frames(512).unwrap();
    assert_eq!(fake.opened()[1].buffer_frames, 512);
    let second = offline(
        &dir.path().join("second"),
        "default",
        DEFAULT_SEAM_CROSSFADE,
    );
    let mut heard = Vec::new();
    for _ in 0..1_000 {
        for monitor in session.disk_monitors() {
            assert!(monitor.wait_idle(Duration::from_secs(10)));
        }
        let out = fake.render(512).expect("a stream plays");
        if session.poll().setlist.current == 1 {
            heard.extend(out);
            if heard.len() >= second.len() / 2 {
                break;
            }
        }
    }
    assert!(
        heard.len() >= second.len() / 2,
        "the second song plays at 512 frames"
    );
    assert!(heard.iter().any(|&sample| sample != 0.0));
}
