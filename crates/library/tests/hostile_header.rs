//! A WAV header that claims far more samples than its file holds must not make
//! the loader reserve memory for them.
//!
//! This test binary installs a global allocator that records the largest
//! single allocation; `unsafe` is needed only to forward to the system
//! allocator and stays in this test.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;

use common::{manifest, write_int, write_manifest};
use domain::StemRole;
use library::{LoadError, load_song};

struct Tracking;

thread_local! {
    /// Largest allocation on this thread while tracking.
    static LARGEST: Cell<Option<usize>> = const { Cell::new(None) };
}

fn record(size: usize) {
    let _ = LARGEST.try_with(|largest| {
        if let Some(current) = largest.get() {
            largest.set(Some(current.max(size)));
        }
    });
}

// SAFETY: every call forwards to `System` with the caller's arguments.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged; `ptr` came from `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

#[test]
fn a_header_claiming_billions_of_samples_reserves_only_what_the_file_holds() {
    // 192 kHz keeps the claimed 2 147 483 640 frames under the 24-hour cap,
    // so the stem passes validation and reaches the decoder.
    let rate = 192_000;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    write_manifest(
        path,
        &manifest(rate, &[("hostile", "hostile.wav", StemRole::Track)]),
    );
    let file = path.join("hostile.wav");
    write_int(&file, 16, 1, rate, &[0; 1_000]);
    let mut bytes = fs::read(&file).unwrap();
    // 16-bit mono uses the 44-byte PCM header: the data size is at 40..44.
    bytes[40..44].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    fs::write(&file, bytes).unwrap();

    LARGEST.with(|largest| largest.set(Some(0)));
    let result = load_song(path);
    let largest = LARGEST.with(|largest| largest.replace(None)).unwrap_or(0);

    assert!(
        matches!(&result, Err(LoadError::Decode { id, .. }) if id == "hostile"),
        "{result:?}"
    );
    assert!(
        largest < 1 << 20,
        "largest allocation was {largest} bytes for a 2 KB file"
    );
}
