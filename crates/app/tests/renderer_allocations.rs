//! Real-time checklist for the audio side of a session: "No memory
//! allocation or deallocation." in `AudioRenderer::render`, which runs the
//! player, asks the sources whether the playhead is ready, and publishes the
//! snapshot, while the session is stopped, starting, playing, jumping,
//! fading and panicking.
//!
//! This test binary installs a counting global allocator that counts only on
//! the test thread while it calls `render`; `unsafe` is needed only to
//! forward to the system allocator and stays in this test.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Arc, Mutex};

use app::{
    AudioOutput, AudioRenderer, Command, DeviceInfo, JumpTiming, OutputError, OutputRequest,
    OutputStream, Session, SessionOptions, StreamHealth, StreamInfo,
};
use common::{IDLE, RATE, write_song};

struct Counting;

thread_local! {
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

/// A port that hands the renderer to the test.
#[derive(Clone, Default)]
struct Capture {
    renderer: Arc<Mutex<Option<AudioRenderer>>>,
}

struct Captured {
    renderer: Arc<Mutex<Option<AudioRenderer>>>,
    info: StreamInfo,
}

impl AudioOutput for Capture {
    fn devices(&self) -> Result<Vec<DeviceInfo>, OutputError> {
        Ok(Vec::new())
    }

    fn open(
        &mut self,
        request: &OutputRequest,
        renderer: AudioRenderer,
    ) -> Result<Box<dyn OutputStream>, OutputError> {
        *self.renderer.lock().unwrap() = Some(renderer);
        Ok(Box::new(Captured {
            renderer: Arc::clone(&self.renderer),
            info: StreamInfo {
                device: "capture".to_owned(),
                sample_rate: request.sample_rate,
                buffer_frames: request.buffer_frames,
                channels: 2,
            },
        }))
    }
}

impl OutputStream for Captured {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn health(&self) -> StreamHealth {
        StreamHealth::Running
    }
}

impl Drop for Captured {
    fn drop(&mut self) {
        drop(self.renderer.lock().unwrap().take());
    }
}

#[test]
fn rendering_a_session_allocates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_song(dir.path(), RATE);
    let capture = Capture::default();
    let mut session = Session::open(
        dir.path(),
        Box::new(capture.clone()),
        SessionOptions {
            buffer_frames: 256,
            ..SessionOptions::default()
        },
    )
    .unwrap();
    let monitor = session.disk_monitor();
    let script = [
        (2, Command::Play),
        (40, Command::QueueJump { position: 3 }),
        (60, Command::SetJumpTiming(JumpTiming::NextBar)),
        (60, Command::QueueJump { position: 1 }),
        (200, Command::ToggleFade),
        (260, Command::LiveLoop),
        (300, Command::SetInfinite(true)),
        (340, Command::Pause),
        (350, Command::Play),
        (380, Command::BackToStart),
        (400, Command::Panic),
        (430, Command::Play),
    ];
    let mut out = vec![0.0f32; 2 * 256];
    let mut allocations = 0;
    let mut renderer = capture.renderer.lock().unwrap();
    let renderer = renderer.as_mut().unwrap();
    for buffer in 0..500 {
        for (_, command) in script.iter().filter(|(at, _)| *at == buffer) {
            session.send(*command).unwrap();
        }
        assert!(monitor.wait_idle(IDLE));
        COUNT.with(|count| count.set(Some(0)));
        renderer.render(&mut out);
        allocations += COUNT.with(|count| count.replace(None)).unwrap();
        session.poll();
    }
    assert_eq!(allocations, 0);
    assert!(session.poll().output_frame > 0, "the song played");
}
