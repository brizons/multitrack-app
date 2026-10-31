//! S3 real-time checklist with streaming sources: "No memory allocation or
//! deallocation." in `Player::process`, while the disk thread streams, jumps
//! are prefetched, and blocks come and go.
//!
//! This test binary installs a counting global allocator that counts only on
//! the test thread, which is the one calling `process`; `unsafe` is needed
//! only to forward to the system allocator and stays in this test.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::Duration;

use common::{write_int, write_manifest};
use domain::{
    Arrangement, SectionMarker, SectionType, SongPackage, Stem, StemRole, TempoPoint,
    TimeSignaturePoint,
};
use engine::MixSettings;
use engine::live::{Command, JumpTiming, Player, PlayerConfig};
use library::{StreamConfig, open_streaming};

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

const RATE: u32 = 8_000;

fn write_song(dir: &std::path::Path) {
    let length = 112_000u64;
    let marker = |id: &str, start| SectionMarker {
        id: id.to_owned(),
        section_type: SectionType::Verse,
        number: None,
        start,
    };
    let package = SongPackage {
        format_version: 1,
        title: "Allocations".to_owned(),
        artist: None,
        sample_rate: RATE,
        key: None,
        tempo_map: vec![TempoPoint { at: 0, bpm: 120.0 }],
        time_signature_map: vec![TimeSignaturePoint {
            at: 0,
            numerator: 4,
            denominator: 4,
        }],
        stems: ["drums", "bass"]
            .iter()
            .map(|id| Stem {
                id: (*id).to_owned(),
                file: format!("stems/{id}.wav"),
                part: (*id).to_owned(),
                role: StemRole::Track,
            })
            .collect(),
        sections: vec![
            marker("a", 0),
            marker("b", 32_000),
            marker("c", 64_000),
            marker("d", 96_000),
        ],
        arrangements: vec![Arrangement {
            id: "default".to_owned(),
            name: "Default".to_owned(),
            sections: ["a", "c", "c", "b", "d"].map(str::to_owned).to_vec(),
            infinite_loop: Vec::new(),
        }],
    };
    write_manifest(dir, &package);
    let samples: Vec<i32> = (0..length as i32).map(|i| (i % 2_000) - 1_000).collect();
    for id in ["drums", "bass"] {
        write_int(&dir.join(format!("stems/{id}.wav")), 16, 1, RATE, &samples);
    }
}

#[test]
fn process_allocates_nothing_with_streaming_sources() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path());
    let streams = open_streaming(dir.path(), &StreamConfig::default()).unwrap();
    let monitor = streams.monitor.clone();
    let buffer = 256;
    let (mut player, mut controller) = Player::new(
        &streams.song,
        "default",
        &MixSettings::default(),
        &PlayerConfig::new(RATE, buffer),
        streams.sources,
    )
    .unwrap();
    let script = [
        (0, Command::Play),
        (20, Command::QueueJump { position: 3 }),
        (60, Command::SetJumpTiming(JumpTiming::NextBar)),
        (60, Command::QueueJump { position: 1 }),
        (200, Command::LiveLoop),
        (300, Command::SetInfinite(true)),
        (400, Command::ToggleFade),
        (450, Command::BackToStart),
        (500, Command::SetInfinite(false)),
        (520, Command::Pause),
        (540, Command::Play),
        (600, Command::Stop),
        (620, Command::Play),
    ];
    let mut output = vec![0.0f32; 2 * buffer];
    let mut allocations = 0;
    for call in 0..1_000 {
        for &(_, command) in script.iter().filter(|(at, _)| *at == call) {
            controller.send(command).unwrap();
        }
        assert!(monitor.wait_idle(Duration::from_secs(10)));
        COUNT.with(|count| count.set(Some(0)));
        player.process(&mut output);
        allocations += COUNT.with(|count| count.replace(None)).unwrap_or(0);
    }

    assert_eq!(allocations, 0, "process allocated or freed memory");
    assert_eq!(player.status().underruns, 0);
    assert!(player.status().output_frame > 0);
}
