//! S5 real-time checklist: "No memory allocation or deallocation" in the
//! render loop. `render_offline` allocates its output and scratch once, before
//! the loop, so its allocation count does not grow with the plan.
//!
//! This test binary installs a counting global allocator; `unsafe` is needed
//! only to forward to the system allocator and stays in this test.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use common::{SongSpec, four_sections, mono, noise, plan};
use domain::{SectionType, StemRole};
use engine::{MixSettings, StemAudio, render_offline};

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

/// Allocations and deallocations made by one `render_offline` call; the
/// output is dropped after counting stops.
fn allocations(song: &SongSpec, arrangement: &str, stems: &[StemAudio]) -> usize {
    let song = song.build();
    let plan = plan(&song, arrangement);
    let settings = MixSettings::default();
    COUNT.with(|count| count.set(Some(0)));
    let output = render_offline(&plan, stems, &settings);
    let counted = COUNT.with(|count| count.replace(None)).unwrap_or(0);
    assert!(output.is_ok());
    counted
}

#[test]
fn render_loop_allocates_nothing() {
    let stems = |frames: usize| {
        vec![
            mono("drums", StemRole::Track, noise(1, frames)),
            mono("bass", StemRole::Track, noise(2, frames)),
        ]
    };
    let tiny = SongSpec::new(
        48_000,
        48_000,
        vec![("intro", SectionType::Intro, 0)],
        vec![("drums", StemRole::Track), ("bass", StemRole::Track)],
    );
    // Many more frames, segments, jump seams, and clicks.
    let long = four_sections(vec![("drums", StemRole::Track), ("bass", StemRole::Track)])
        .arrangement(
            "long",
            &[
                "intro", "chorus", "verse", "chorus", "chorus", "intro", "verse", "outro",
            ],
        );

    let small = allocations(&tiny, "default", &stems(48_000));
    let large = allocations(&long, "long", &stems(1_152_000));

    assert!(small > 0, "the output buffer is allocated");
    assert_eq!(small, large, "allocations do not grow with the plan");
}
