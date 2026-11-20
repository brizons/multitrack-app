//! S3: two players on the audio thread. A `Deck` plays the current setlist
//! item, holds the next one cued, and hands over at the current item's end:
//! AutoLink on the next frame, Crossfade as a linear blend over the frames
//! before the end. Players it no longer needs go back to the control
//! thread.

mod common;

use common::{SongSpec, bits, mono, noise, onsets, plan, stereo};
use domain::{SectionType, Song, StemRole, TimeSignature};
use engine::live::{
    Command, DECLICK_FRAMES, Deck, DeckController, Handoff, Player, PlayerConfig, Transport,
    memory_sources,
};
use engine::{MixSettings, StemAudio, render_offline};

const RATE: u32 = 8_000;
/// −1 dBFS.
const PEAK: f32 = 0.891_250_9;
/// Five seconds at [`RATE`].
const BLEND: u64 = 5 * RATE as u64;

/// Song A: 3 s, two sections, noise on a mono and a stereo track.
fn song_a() -> (Song, Vec<StemAudio>) {
    let frames = 24_000;
    let song = SongSpec::new(
        RATE,
        frames,
        vec![
            ("intro", SectionType::Intro, 0),
            ("verse", SectionType::Verse, 12_000),
        ],
        vec![("drums", StemRole::Track), ("keys", StemRole::Track)],
    )
    .build();
    let stems = vec![
        mono("drums", StemRole::Track, noise(1, frames as usize)),
        stereo("keys", StemRole::Track, noise(2, 2 * frames as usize)),
    ];
    (song, stems)
}

/// Song B: 2.5 s, two sections, other noise.
fn song_b() -> (Song, Vec<StemAudio>) {
    let frames = 20_000;
    let song = SongSpec::new(
        RATE,
        frames,
        vec![
            ("chorus", SectionType::Chorus, 0),
            ("outro", SectionType::Outro, 10_000),
        ],
        vec![("bass", StemRole::Track)],
    )
    .build();
    let stems = vec![mono("bass", StemRole::Track, noise(7, frames as usize))];
    (song, stems)
}

/// A 7-second song whose output is `track` on both channels, plus `guide`:
/// a silent click stem replaces the generated click, so the song peaks at
/// exactly `track + guide`.
fn steady(track: f32, guide: f32) -> (Song, Vec<StemAudio>) {
    let frames = 56_000;
    let song = SongSpec::new(
        RATE,
        frames,
        vec![("verse", SectionType::Verse, 0)],
        vec![
            ("track", StemRole::Track),
            ("guide", StemRole::Guide),
            ("click", StemRole::Click),
        ],
    )
    .build();
    let n = frames as usize;
    let stems = vec![
        mono("track", StemRole::Track, vec![track; n]),
        mono("guide", StemRole::Guide, vec![guide; n]),
        mono("click", StemRole::Click, vec![0.0; n]),
    ];
    (song, stems)
}

fn player(song: &Song, stems: Vec<StemAudio>, max_frames: usize) -> Player {
    let (player, _controller) = Player::new(
        song,
        "default",
        &MixSettings::default(),
        &PlayerConfig::new(RATE, max_frames),
        memory_sources(stems).unwrap(),
    )
    .unwrap();
    player
}

fn offline(song: &Song, stems: &[StemAudio]) -> Vec<f32> {
    render_offline(&plan(song, "default"), stems, &MixSettings::default()).unwrap()
}

/// Renders `frames` frames through `deck` in buffers of `buffer` frames.
fn run(deck: &mut Deck, buffer: usize, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(2 * frames);
    let mut chunk = vec![0.0f32; 2 * buffer];
    while out.len() < 2 * frames {
        deck.process(&mut chunk);
        out.extend_from_slice(&chunk);
    }
    out.truncate(2 * frames);
    out
}

fn left(out: &[f32], frame: u64) -> f32 {
    out[2 * frame as usize]
}

fn deck_of(song: &(Song, Vec<StemAudio>), buffer: usize) -> (Deck, DeckController) {
    Deck::new(player(&song.0, song.1.clone(), buffer), 1, buffer)
}

#[test]
fn a_deck_with_one_item_plays_exactly_what_its_player_plays() {
    let a = song_a();
    let buffer = 256;
    let (mut alone, mut controller) = Player::new(
        &a.0,
        "default",
        &MixSettings::default(),
        &PlayerConfig::new(RATE, buffer),
        memory_sources(a.1.clone()).unwrap(),
    )
    .unwrap();
    let (mut deck, mut control) = deck_of(&a, buffer);
    let script = [
        (0, Command::Play),
        (5, Command::QueueJump { position: 1 }),
        (40, Command::ToggleFade),
        (60, Command::BackToStart),
    ];
    let mut expected = Vec::new();
    let mut played = Vec::new();
    let mut chunk = vec![0.0f32; 2 * buffer];
    for index in 0..120 {
        for (_, command) in script.iter().filter(|(at, _)| *at == index) {
            controller.send(*command).unwrap();
            control.send(*command).unwrap();
        }
        alone.process(&mut chunk);
        expected.extend_from_slice(&chunk);
        deck.process(&mut chunk);
        played.extend_from_slice(&chunk);
    }
    assert_eq!(bits(&played), bits(&expected));
    assert_eq!(deck.status().current, alone.status());
    assert_eq!(deck.applied_messages(), script.len() as u64);
}

#[test]
fn autolink_starts_the_cued_item_on_the_frame_after_the_current_one_ends() {
    let (a, b) = (song_a(), song_b());
    let expected_a = offline(&a.0, &a.1);
    let expected_b = offline(&b.0, &b.1);
    let (la, lb) = (expected_a.len() / 2, expected_b.len() / 2);
    for buffer in [1, 7, 256, 1_000] {
        let (mut deck, mut control) = deck_of(&a, buffer);
        control
            .cue(player(&b.0, b.1.clone(), buffer), 2, Handoff::AutoLink)
            .unwrap();
        control.send(Command::Play).unwrap();
        let out = run(&mut deck, buffer, la + lb + 500);

        assert_eq!(
            bits(&out[..2 * la]),
            bits(&expected_a),
            "A plays to its last frame, {la} (buffer {buffer})"
        );
        assert_eq!(
            bits(&out[2 * la..2 * (la + lb)]),
            bits(&expected_b),
            "B's first frame is frame {la}, right after A's last (buffer {buffer})"
        );
        assert!(out[2 * (la + lb)..].iter().all(|&s| s == 0.0));
        let status = deck.status();
        assert_eq!(status.item, 2, "B is the current item");
        assert_eq!(status.cued, None);
        assert_eq!(status.finished, 1, "B ended with nothing cued");
        assert_eq!(status.finished_item, Some(2));
        assert!(control.returned().is_some(), "A comes back to be dropped");
        assert!(control.returned().is_none());
    }
}

#[test]
fn a_crossfade_blends_linearly_over_the_last_frames_of_the_current_item() {
    let (a, b) = (steady(PEAK, 0.0), steady(-PEAK, 0.0));
    let la = a.0.length();
    let start = la - BLEND;
    let (mut deck, mut control) = deck_of(&a, 333);
    control
        .cue(
            player(&b.0, b.1.clone(), 333),
            2,
            Handoff::Crossfade { frames: BLEND },
        )
        .unwrap();
    control.send(Command::Play).unwrap();
    let out = run(&mut deck, 333, (start + b.0.length() + 1_000) as usize);

    for frame in 0..start {
        assert_eq!(
            left(&out, frame),
            PEAK,
            "A alone before the blend, frame {frame}"
        );
    }
    for t in 0..BLEND {
        let gain = (t as f64 + 0.5) / BLEND as f64;
        let expected = f64::from(PEAK) * (1.0 - gain) - f64::from(PEAK) * gain;
        let got = f64::from(left(&out, start + t));
        assert!(
            (got - expected).abs() < 1e-6,
            "blend frame {t}: {got} instead of {expected}"
        );
    }
    let end = start + b.0.length();
    for frame in la..end {
        assert_eq!(
            left(&out, frame),
            -PEAK,
            "B alone after the blend, frame {frame}"
        );
    }
    assert!((end..end + 1_000).all(|frame| left(&out, frame) == 0.0));
    assert_eq!(deck.status().item, 2);
    assert!(control.returned().is_some(), "A comes back after the blend");
}

#[test]
fn a_crossfade_of_two_songs_peaking_at_minus_1_dbfs_stays_below_0_dbfs() {
    let (a, b) = (steady(PEAK, 0.0), steady(PEAK, 0.0));
    let la = a.0.length();
    let (mut deck, mut control) = deck_of(&a, 256);
    control
        .cue(
            player(&b.0, b.1.clone(), 256),
            2,
            Handoff::Crossfade { frames: BLEND },
        )
        .unwrap();
    control.send(Command::Play).unwrap();
    let mut blend_seen = None;
    let mut out = Vec::new();
    let mut chunk = vec![0.0f32; 512];
    while out.len() < 2 * (la + b.0.length() - BLEND) as usize {
        deck.process(&mut chunk);
        out.extend_from_slice(&chunk);
        blend_seen = blend_seen.or(deck.status().blend);
    }
    let peak = out.iter().fold(0.0f32, |peak, &s| peak.max(s.abs()));
    assert!(peak < 1.0, "the blend peaks at {peak}, over 0 dBFS");
    assert!(
        peak <= PEAK * (1.0 + 1e-6),
        "the blend never adds up: {peak}"
    );
    let blend = blend_seen.expect("a blend ran");
    assert_eq!(
        (blend.from, blend.frames),
        (1, BLEND),
        "five seconds from A"
    );
}

#[test]
fn pausing_during_a_crossfade_holds_both_items_and_the_blend() {
    let (a, b) = (steady(PEAK, 0.0), steady(-PEAK, 0.0));
    let (mut deck, mut control) = deck_of(&a, 100);
    control
        .cue(
            player(&b.0, b.1.clone(), 100),
            2,
            Handoff::Crossfade { frames: BLEND },
        )
        .unwrap();
    control.send(Command::Play).unwrap();
    run(&mut deck, 100, (a.0.length() - BLEND + 4_000) as usize);
    let before = deck.status().blend.expect("the blend runs").played;
    assert_eq!(before, 4_000);

    control.send(Command::Pause).unwrap();
    let ramp = run(&mut deck, 100, 300);
    assert!(ramp[..2 * 200].iter().any(|&s| s != 0.0), "both ramp out");
    let held = deck.status().blend.expect("the blend holds").played;
    assert_eq!(held, before + DECLICK_FRAMES, "the blend follows the ramp");
    let silent = run(&mut deck, 100, 5_000);
    assert!(silent.iter().all(|&s| s == 0.0));
    assert_eq!(deck.status().blend.unwrap().played, held);
    assert!(control.returned().is_none(), "A is still in the blend");

    control.send(Command::Play).unwrap();
    run(&mut deck, 100, (BLEND - held + 100) as usize);
    assert_eq!(deck.status().blend, None, "the blend finished");
    assert!(control.returned().is_some());
    assert_eq!(deck.status().current.transport, Transport::Playing);
}

#[test]
fn the_end_fade_silences_the_tracks_on_the_last_frame_while_the_guide_plays_on() {
    let (song, stems) = steady(0.5, 0.25);
    let length = song.length();
    let fade = BLEND;
    let mut player = player(&song, stems, 256);
    player.set_end_fade(Some(fade));
    let (mut deck, mut control) = Deck::new(player, 1, 256);
    control.send(Command::Play).unwrap();
    let out = run(&mut deck, 256, length as usize + 500);

    for frame in 0..length - fade {
        assert_eq!(
            left(&out, frame),
            0.75,
            "full level before the fade, frame {frame}"
        );
    }
    for i in 0..fade {
        let track = 0.5 * (1.0 - (i + 1) as f64 / fade as f64);
        let got = f64::from(left(&out, length - fade + i));
        assert!((got - (track + 0.25)).abs() < 1e-6, "fade frame {i}: {got}");
    }
    assert_eq!(
        left(&out, length - 1),
        0.25,
        "the tracks are silent on the last frame"
    );
    assert!((length..length + 500).all(|frame| left(&out, frame) == 0.0));
    assert_eq!(deck.status().current.transport, Transport::Stopped);
}

fn click_tile(bars: Option<u32>) -> Player {
    let tile = Song::click_only(
        RATE,
        120.0,
        TimeSignature {
            numerator: 4,
            denominator: 4,
        },
        bars,
    )
    .unwrap();
    let (mut player, _controller) = Player::new(
        &tile,
        "default",
        &MixSettings::default(),
        &PlayerConfig::new(RATE, 256),
        Vec::new(),
    )
    .unwrap();
    player.set_infinite_at_start(bars.is_none());
    player
}

/// 120 bpm in 4/4 at 8 kHz: a beat is 4 000 frames, a bar 16 000.
const BAR: u64 = 16_000;

#[test]
fn a_click_tile_with_a_bar_count_plays_that_many_bars_then_ends() {
    let (mut deck, mut control) = Deck::new(click_tile(Some(2)), 1, 256);
    control.send(Command::Play).unwrap();
    let out = run(&mut deck, 256, 3 * BAR as usize);
    let beats: Vec<u64> = (0..8).map(|beat| beat * 4_000).collect();
    assert_eq!(onsets(&out), beats, "eight beats, then silence");
    let status = deck.status();
    assert_eq!(status.current.transport, Transport::Stopped);
    assert_eq!((status.finished, status.finished_item), (1, Some(1)));
}

#[test]
fn a_click_tile_without_a_bar_count_repeats_its_bar_even_after_a_stop() {
    let (mut deck, mut control) = Deck::new(click_tile(None), 1, 256);
    control.send(Command::Play).unwrap();
    let out = run(&mut deck, 256, 5 * BAR as usize);
    let beats: Vec<u64> = (0..20).map(|beat| beat * 4_000).collect();
    assert_eq!(onsets(&out), beats, "the bar repeats");
    assert_eq!(deck.status().current.transport, Transport::Playing);
    assert!(deck.status().current.infinite);

    control.send(Command::Stop).unwrap();
    run(&mut deck, 256, 1_000);
    control.send(Command::Play).unwrap();
    run(&mut deck, 256, 3 * BAR as usize);
    let status = deck.status();
    assert_eq!(
        status.current.transport,
        Transport::Playing,
        "it loops again"
    );
    assert!(status.current.infinite);
    assert_eq!(status.finished, 0);
}

#[test]
fn selecting_while_playing_ramps_the_current_item_out_and_stops_at_the_new_one() {
    let (a, b) = (song_a(), song_b());
    let expected_b = offline(&b.0, &b.1);
    let (mut deck, mut control) = deck_of(&a, 64);
    control.send(Command::Play).unwrap();
    run(&mut deck, 64, 5_000);

    control.select(player(&b.0, b.1.clone(), 64), 2).unwrap();
    let ramp = run(&mut deck, 64, 1_000);
    let last = (0..1_000).rev().find(|&f| ramp[2 * f] != 0.0).unwrap();
    assert!(
        last < DECLICK_FRAMES as usize,
        "A ramps out within the declick"
    );
    assert!(
        last > DECLICK_FRAMES as usize / 2,
        "A ramps rather than cuts"
    );
    let status = deck.status();
    assert_eq!(status.item, 2);
    assert_eq!(status.current.transport, Transport::Stopped);
    assert_eq!(status.current.output_frame, 0, "B waits at its start");
    assert_eq!(status.leaving, 0);
    assert!(control.returned().is_some(), "A comes back once silent");

    control.send(Command::Play).unwrap();
    let played = run(&mut deck, 64, expected_b.len() / 2);
    assert_eq!(bits(&played), bits(&expected_b), "B plays from its start");
}

#[test]
fn an_item_with_a_stop_handoff_ends_stopped_and_its_cued_item_can_be_selected() {
    let (a, b) = (song_a(), song_b());
    let expected_b = offline(&b.0, &b.1);
    let (mut deck, mut control) = deck_of(&a, 256);
    control
        .cue(player(&b.0, b.1.clone(), 256), 2, Handoff::Stop)
        .unwrap();
    control.send(Command::Play).unwrap();
    let out = run(&mut deck, 256, a.0.length() as usize + 2_000);
    let la = a.0.length() as usize;
    assert!(out[2 * la..].iter().all(|&s| s == 0.0), "nothing follows A");
    let status = deck.status();
    assert_eq!((status.item, status.cued), (1, Some(2)));
    assert_eq!((status.finished, status.finished_item), (1, Some(1)));
    assert_eq!(status.current.transport, Transport::Stopped);

    control.select_cued().unwrap();
    run(&mut deck, 256, 10);
    let status = deck.status();
    assert_eq!((status.item, status.cued), (2, None));
    assert!(control.returned().is_some(), "A comes back");
    control.send(Command::Play).unwrap();
    let played = run(&mut deck, 256, expected_b.len() / 2);
    assert_eq!(bits(&played), bits(&expected_b));
}

#[test]
fn an_uncued_item_comes_back_and_the_current_one_just_ends() {
    let (a, b) = (song_a(), song_b());
    let (mut deck, mut control) = deck_of(&a, 256);
    control
        .cue(player(&b.0, b.1.clone(), 256), 2, Handoff::AutoLink)
        .unwrap();
    control.uncue().unwrap();
    control.send(Command::Play).unwrap();
    let out = run(&mut deck, 256, a.0.length() as usize + 2_000);
    assert!(control.returned().is_some(), "B comes back unplayed");
    let la = a.0.length() as usize;
    assert!(out[2 * la..].iter().all(|&s| s == 0.0));
    assert_eq!(deck.status().item, 1);
}
