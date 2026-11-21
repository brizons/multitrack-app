//! S3 / F5.3: an item's fade-out covers only its last five seconds as the
//! player predicts them. A jump back or a loop that moves the end away
//! cancels it: the stems return to the operator's level with a declick
//! ramp, and the fade starts again exactly five seconds before the new end.
//! Click and guide are never touched by it.
//!
//! The track stem plays only on the left channel and the guide only on the
//! right, so each can be read on its own.

mod common;

use common::{SongSpec, stereo};
use domain::{SectionType, StemRole};
use engine::MixSettings;
use engine::live::{
    Command, Controller, DECLICK_FRAMES, Player, PlayerConfig, Transport, memory_sources,
};

const RATE: u32 = 8_000;
/// Five seconds at [`RATE`].
const WINDOW: u64 = 5 * RATE as u64;
const TRACK: f32 = 0.5;
const GUIDE: f32 = 0.25;

/// A player whose `track` stem is `TRACK` on the left and whose `guide` is
/// `GUIDE` on the right; a silent click stem replaces the generated click.
fn player(sections: Vec<(&'static str, SectionType, u64)>, length: u64) -> (Player, Controller) {
    let song = SongSpec::new(
        RATE,
        length,
        sections,
        vec![
            ("track", StemRole::Track),
            ("guide", StemRole::Guide),
            ("click", StemRole::Click),
        ],
    )
    .build();
    let n = length as usize;
    let stems = vec![
        stereo("track", StemRole::Track, [TRACK, 0.0].repeat(n)),
        stereo("guide", StemRole::Guide, [0.0, GUIDE].repeat(n)),
        stereo("click", StemRole::Click, vec![0.0; 2 * n]),
    ];
    let (mut player, controller) = Player::new(
        &song,
        "default",
        &MixSettings::default(),
        &PlayerConfig::new(RATE, 256),
        memory_sources(stems).unwrap(),
    )
    .unwrap();
    player.set_end_fade(Some(WINDOW));
    (player, controller)
}

/// Intro, chorus and outro of three seconds each: 72 000 frames. The
/// window starts in the chorus, at 32 000.
fn three_sections() -> (Player, Controller) {
    player(
        vec![
            ("intro", SectionType::Intro, 0),
            ("chorus", SectionType::Chorus, 24_000),
            ("outro", SectionType::Outro, 48_000),
        ],
        72_000,
    )
}

/// A two-second intro and a seven-second outro: 72 000 frames. The window
/// starts in the outro, at 32 000.
fn long_outro() -> (Player, Controller) {
    player(
        vec![
            ("intro", SectionType::Intro, 0),
            ("outro", SectionType::Outro, 16_000),
        ],
        72_000,
    )
}

/// Renders exactly `frames` frames into `out`, as left/right pairs.
fn render(player: &mut Player, out: &mut Vec<(f32, f32)>, frames: usize) {
    let mut left = frames;
    let mut chunk = vec![0.0f32; 2 * 256];
    while left > 0 {
        let n = left.min(256);
        player.process(&mut chunk[..2 * n]);
        out.extend(chunk[..2 * n].chunks_exact(2).map(|f| (f[0], f[1])));
        left -= n;
    }
}

/// The stems' gain at frame `i` of a fade starting at full level over the
/// whole window.
fn faded(i: u64) -> f32 {
    TRACK * (1.0 - (i + 1) as f32 / WINDOW as f32)
}

fn assert_fade_from(out: &[(f32, f32)], start: u64, what: &str) {
    for i in 0..WINDOW {
        let (track, guide) = out[(start + i) as usize];
        assert!(
            (track - faded(i)).abs() < 1e-6,
            "{what}: fade frame {i} at {}: track {track}, expected {}",
            start + i,
            faded(i)
        );
        assert_eq!(
            guide,
            GUIDE,
            "{what}: the guide plays on, frame {}",
            start + i
        );
    }
    let last = out[(start + WINDOW - 1) as usize];
    assert_eq!(
        last,
        (0.0, GUIDE),
        "{what}: the stems are silent on the last frame"
    );
}

fn assert_full(out: &[(f32, f32)], frames: std::ops::Range<u64>, what: &str) {
    for frame in frames {
        assert_eq!(
            out[frame as usize],
            (TRACK, GUIDE),
            "{what}: full level at frame {frame}"
        );
    }
}

#[test]
fn a_jump_back_after_the_end_fade_began_brings_the_stems_back_and_fades_at_the_new_end() {
    let (mut player, mut controller) = three_sections();
    let mut out = Vec::new();
    controller.send(Command::Play).unwrap();
    render(&mut player, &mut out, 40_000);
    assert!(out[39_999].0 < TRACK * 0.81, "the end fade began at 32 000");

    // Back to the intro at the end of the chorus (48 000).
    controller.send(Command::QueueJump { position: 0 }).unwrap();
    render(&mut player, &mut out, 120_000 + 1_000 - 40_000);

    let back = 48_000 + DECLICK_FRAMES;
    assert_full(&out, back..80_000, "after the jump and the declick ramp");
    // The arrangement plays again from the intro: it ends at 48 000 + 72 000.
    assert_fade_from(&out, 120_000 - WINDOW, "the new end");
    assert!(out[120_000..].iter().all(|&frame| frame == (0.0, 0.0)));
    assert_eq!(player.status().transport, Transport::Stopped);
}

#[test]
fn with_the_operators_fade_engaged_the_stems_stay_faded_after_a_jump_back() {
    let (mut player, mut controller) = three_sections();
    let mut out = Vec::new();
    controller.send(Command::Play).unwrap();
    render(&mut player, &mut out, 40_000);
    controller
        .send(Command::SetFadeDuration { frames: 1_000 })
        .unwrap();
    controller.send(Command::ToggleFade).unwrap();
    controller.send(Command::QueueJump { position: 0 }).unwrap();
    render(&mut player, &mut out, 120_000 + 1_000 - 40_000);

    for (frame, &(track, guide)) in out.iter().enumerate().take(120_000).skip(41_000) {
        assert_eq!(track, 0.0, "the operator's fade holds, frame {frame}");
        if !(48_000..48_000 + DECLICK_FRAMES as usize).contains(&frame) {
            assert_eq!(guide, GUIDE, "the guide plays on, frame {frame}");
        }
    }
}

#[test]
fn the_infinite_loop_brings_the_stems_back_and_its_release_fades_at_the_end() {
    let (mut player, mut controller) = long_outro();
    let mut out = Vec::new();
    controller.send(Command::Play).unwrap();
    render(&mut player, &mut out, 40_000);
    assert!(out[39_999].0 < TRACK * 0.81, "the end fade began at 32 000");

    controller.send(Command::SetInfinite(true)).unwrap();
    render(&mut player, &mut out, 42_000);
    // The outro repeats from 72 000 (source 16 000) while the loop is on.
    assert_full(&out, 40_000 + DECLICK_FRAMES..82_000, "while looping");

    // Released 10 000 frames into the repeat: 46 000 frames are left.
    controller.send(Command::SetInfinite(false)).unwrap();
    render(&mut player, &mut out, 128_000 + 1_000 - 82_000);
    assert_full(&out, 82_000..128_000 - WINDOW, "before the new window");
    assert_fade_from(&out, 128_000 - WINDOW, "after the release");
    assert!(out[128_000..].iter().all(|&frame| frame == (0.0, 0.0)));
}

#[test]
fn a_live_loop_queued_in_the_window_repeats_with_the_stems_back_and_fades_at_the_new_end() {
    let (mut player, mut controller) = long_outro();
    let mut out = Vec::new();
    controller.send(Command::Play).unwrap();
    render(&mut player, &mut out, 40_000);
    controller.send(Command::LiveLoop).unwrap();
    render(&mut player, &mut out, 128_000 + 1_000 - 40_000);

    // Until the loop executes at the outro's end the item could still end
    // there (a loop waiting for its audio plays on), so the fade runs on.
    assert_eq!(out[71_999].0, 0.0, "the outro faded to its end");
    assert_full(
        &out,
        72_000 + DECLICK_FRAMES..128_000 - WINDOW,
        "while it repeats",
    );
    assert_fade_from(&out, 128_000 - WINDOW, "the repeat's end");
    assert!(out[128_000..].iter().all(|&frame| frame == (0.0, 0.0)));
}

#[test]
fn a_jump_ahead_in_the_window_speeds_the_fade_up_to_the_new_last_frame() {
    // Four two-second sections: 64 000 frames; the window starts at 24 000.
    let (mut player, mut controller) = player(
        vec![
            ("intro", SectionType::Intro, 0),
            ("verse", SectionType::Verse, 16_000),
            ("chorus", SectionType::Chorus, 32_000),
            ("outro", SectionType::Outro, 48_000),
        ],
        64_000,
    );
    let mut out = Vec::new();
    controller.send(Command::Play).unwrap();
    render(&mut player, &mut out, 26_000);
    // Skips the chorus at the end of the verse: the item now ends at 48 000.
    controller.send(Command::QueueJump { position: 3 }).unwrap();
    render(&mut player, &mut out, 49_000 - 26_000);

    assert_eq!(
        out[47_999],
        (0.0, GUIDE),
        "silent stems on the new last frame"
    );
    assert!(out[48_000..].iter().all(|&frame| frame == (0.0, 0.0)));
}
