//! S3 real-time checklist for the live player: "No memory allocation or
//! deallocation." in `Player::process`, whatever the commands do, and in
//! `Deck::process` through an AutoLink, a crossfade, a selection and the
//! players it hands back.
//!
//! This test binary installs a counting global allocator; `unsafe` is needed
//! only to forward to the system allocator and stays in this test.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::{four_sections, mono, noise, stereo};
use domain::StemRole;
use engine::MixSettings;
use engine::live::{Command, Deck, Handoff, JumpTiming, Player, PlayerConfig, memory_sources};

struct Counting;

thread_local! {
    /// Allocations and deallocations on this thread while counting.
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}

fn record() {
    let _ = COUNT.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}

// SAFETY: every call forwards to `System` with the caller's arguments.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        // SAFETY: forwarded unchanged; `ptr` came from `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn process_allocates_nothing_through_every_live_command() {
    let frames = 1_152_000;
    let song = four_sections(vec![
        ("drums", StemRole::Track),
        ("keys", StemRole::Track),
        ("guide", StemRole::Guide),
    ])
    .build();
    let stems = vec![
        mono("drums", StemRole::Track, noise(1, frames)),
        stereo("keys", StemRole::Track, noise(2, 2 * frames)),
        mono("guide", StemRole::Guide, noise(3, frames)),
    ];
    let mut settings = MixSettings::default();
    settings.click.levels = [1.0; 4];
    let buffer = 256;
    let (mut player, mut controller) = Player::new(
        &song,
        "f23",
        &settings,
        &PlayerConfig::new(48_000, buffer),
        memory_sources(stems).unwrap(),
    )
    .unwrap();
    let script = [
        (0, Command::Play),
        (10, Command::SetJumpTiming(JumpTiming::NextBar)),
        (10, Command::QueueJump { position: 3 }),
        (400, Command::LiveLoop),
        (900, Command::SetInfinite(true)),
        (1_000, Command::ToggleFade),
        (1_100, Command::SetClick(false)),
        (1_200, Command::ToggleFade),
        (1_300, Command::SetClick(true)),
        (1_400, Command::QueueJump { position: 1 }),
        (1_500, Command::Pause),
        (1_600, Command::Play),
        (1_700, Command::BackToStart),
        (1_800, Command::SetJumpTiming(JumpTiming::SectionEnd)),
        (1_900, Command::Stop),
        (2_000, Command::Play),
        (2_100, Command::BackToStart),
        (2_200, Command::Panic),
        (2_400, Command::Play),
    ];
    let mut output = vec![0.0f32; 2 * buffer];
    let mut allocations = 0;
    for call in 0..3_000 {
        for &(_, command) in script.iter().filter(|(at, _)| *at == call) {
            controller.send(command).unwrap();
        }
        COUNT.with(|count| count.set(Some(0)));
        player.process(&mut output);
        allocations += COUNT.with(|count| count.replace(None)).unwrap_or(0);
    }

    assert_eq!(allocations, 0, "process allocated or freed memory");
    assert!(player.status().output_frame > 0, "the player kept playing");
}

#[test]
fn the_deck_allocates_nothing_through_an_autolink_a_crossfade_and_a_selection() {
    let frames = 1_152_000;
    let song = four_sections(vec![("drums", StemRole::Track), ("keys", StemRole::Track)]).build();
    let buffer = 256;
    let player = |seed: u64| {
        let stems = vec![
            mono("drums", StemRole::Track, noise(seed, frames)),
            stereo("keys", StemRole::Track, noise(seed + 1, 2 * frames)),
        ];
        let (mut player, _controller) = Player::new(
            &song,
            "default",
            &MixSettings::default(),
            &PlayerConfig::new(48_000, buffer),
            memory_sources(stems).unwrap(),
        )
        .unwrap();
        player.set_end_fade(Some(240_000));
        player
    };
    let (mut deck, mut control) = Deck::new(player(1), 1, buffer);
    control.cue(player(3), 2, Handoff::AutoLink).unwrap();
    control.send(Command::Play).unwrap();

    let calls = frames / buffer;
    let mut output = vec![0.0f32; 2 * buffer];
    let mut allocations = 0;
    let mut returned = Vec::new();
    let mut step = 0;
    for call in 0..3 * calls + 400 {
        // Messages carry boxed players, built and queued outside the count.
        let status = deck.status();
        if step == 0 && status.item == 2 {
            control
                .cue(player(5), 3, Handoff::Crossfade { frames: 240_000 })
                .unwrap();
            step = 1;
        } else if step == 1 && status.item == 3 && status.blend.is_none() {
            control.send(Command::QueueJump { position: 2 }).unwrap();
            step = 2;
        } else if step == 2 && call % 200 == 0 {
            control.select(player(7), 4).unwrap();
            control.send(Command::Play).unwrap();
            step = 3;
        }
        COUNT.with(|count| count.set(Some(0)));
        deck.process(&mut output);
        allocations += COUNT.with(|count| count.replace(None)).unwrap_or(0);
        while let Some(player) = control.returned() {
            returned.push(player);
        }
    }

    assert_eq!(allocations, 0, "the deck allocated or freed memory");
    assert_eq!(
        step, 3,
        "the script ran: AutoLink, crossfade, then a selection"
    );
    assert_eq!(returned.len(), 3, "every replaced player came back");
    assert_eq!(deck.status().item, 4);
}
