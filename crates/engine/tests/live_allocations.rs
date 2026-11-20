//! S3 real-time checklist for the live player: "No memory allocation or
//! deallocation." in `Player::process`, whatever the commands do.
//!
//! This test binary installs a counting global allocator; `unsafe` is needed
//! only to forward to the system allocator and stays in this test.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::{four_sections, mono, noise, stereo};
use domain::StemRole;
use engine::MixSettings;
use engine::live::{Command, JumpTiming, Player, PlayerConfig, memory_sources};

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
