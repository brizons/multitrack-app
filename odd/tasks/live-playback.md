# Feature: live playback (app API, real-time engine, first shell)

Objective: play one song package live on an audio device with transport, queued section jumps, loops, fade and panic — driven through the `app` API, first from a headless `play-cli`, then from the first native shell.
Branch: `feat/live-playback` (from `feat/core-render-pipeline`) · Delivery: work-unit commits, `ask-on-risk` · Test runner: `cargo test --workspace`.

## Specs

Requirements quote `docs/mvp-spec.md` and `docs/architecture.md` verbatim. Items marked **Assumption** are not in those documents.

### S1 — `app` API (`crates/app`)
- Decision 3: the UI "Calls the `app` API; polls snapshots at about 30 Hz".
- Decision 1: "The `app` crate exposes one API of commands and state snapshots; shells only render snapshots and send commands."
- F6.1: "Play, pause, stop, back to start, previous or next song."
- **Assumption:** this feature loads one song (setlists, previous and next song come with the setlist feature). The snapshot carries: transport state, playhead in samples and bars, current section, queued jump (target and when it executes), loop and infinite flags, fade state, device status, and the last error.

### S2 — Real-time transport and live changes (`crates/engine`)
- F6.2: "**Jump to section:** queue any section; the jump happens at the end of the current section by default, or at the next bar if the user chooses."
- F6.3: "**Live loop:** repeat the current section once more. **Infinite loop:** repeat until switched off."
- F6.4: "**Fade:** fade out all stems over a configurable time while click and guide keep playing; press again to fade back in."
- F6.5: "Show the queued action on the song map until it executes or is cancelled."
- F6.6: "**Panic:** fade everything out in 0.5 seconds and stop."
- Acceptance: "A queued jump executes on the exact boundary sample." and "If the target audio is not buffered in time, the jump moves to the next boundary instead of producing silence or a glitch."
- Decision 2: "A live change (jump, loop, fade) recompiles the plan from a boundary and swaps it at that boundary sample."

### S3 — Streaming and real-time rules (`crates/engine`, `crates/library`)
- Decision 3 threads: audio callback renders the plan; disk thread "Decodes stems ahead of the playhead and prefetches jump targets"; control thread "Runs use cases, compiles plans"; commands reach the audio thread "over a lock-free queue".
- Real-time checklist: "No memory allocation or deallocation.", "No locks, no blocking calls, no file or network I/O, no logging.", "All buffers are preallocated when the plan is loaded.", "Gain changes are smoothed per sample to avoid zipper noise."
- NFR: "Audio streams from disk; memory use does not grow with song length."
- NFR (measured later on reference hardware): "Zero buffer underruns in a 60-minute run with 32 stems at 256 frames, 48 kHz."

### S4 — Audio output adapter (`crates/io-cpal`)
- F8.1: "List the audio devices available and their output channel count."
- F8.5: "Choose the buffer size (64 to 1024 frames) and show the resulting latency."
- F8 acceptance: "Unplugging the device pauses playback with a clear message; plugging it back in restores the routing."
- **Assumption:** this feature plays stereo to outputs 1–2 of one device; the routing matrix (F8.2–F8.4) comes later.

### S5 — `play-cli` (`crates/play-cli`)
- **Assumption:** a headless live player for development and hardware spikes: `play-cli <song-dir> [--device <name>] [--buffer <frames>] [--list-devices]`, with single-key controls (play/pause, stop, back to start, queue section 1–9, toggle next-bar timing, loop, infinite, fade, cancel queue, panic, quit) and a one-line status refreshed about 10 times per second.

### S6 — First shell (`apps/…`)
- The Perform screen from the design canvas, reduced to: song map with playhead and queued-jump marker, section pads with Now/Next, transport, fade, hold-to-panic, jump timing toggle, loop and infinite.
- **Decision needed:** which shell goes first. Recommendation: `apps/desktop` (Tauri + React) — it reuses the team's React skills, runs on the development Mac, and the desktop `cpal` backends are the most mature, so it validates the engine with the least platform risk. Alternative: `apps/apple` (SwiftUI), closer to the main live-use device (iPad) but adds UniFFI and audio-session work up front.

Out of scope: setlists and transitions, mixer UI, routing matrix, MIDI, guide-voice playback, pads, key and tempo change, the other two shells.

## Tasks

| ID | Specs | Route | Status | Commit |
|----|-------|-------|--------|--------|
| T1 | S1 | delegated (context backstop) | pending | — |
| T2 | S2, S3 | delegated (context backstop) | pending | — |
| T3 | S4, S5 | delegated (context backstop) | pending | — |
| T4 | S6 | delegated (context backstop) | blocked on the shell decision | — |

Route evidence: the parent session is past the ~150k-token context backstop. T1 and T2 share the snapshot and command types, so they run sequentially; T3 needs T2's engine. Forecast: ~2,500 authored lines. Risk forecast: item 4 (concurrency: audio thread, disk thread, lock-free queues) for T2 and T3, item 5 (new crates `cpal`, `rtrb`) for T3 — each gets an independent verifier and the native review.

## Log

- **L1** (2026-10-30, user, verbatim): "Native per platform"
- **L2** (2026-10-30): Decision 1 recorded in docs/architecture.md: Rust core + SwiftUI (iPhone, iPad, Mac) + Compose (Android) + Tauri with React (Windows, Linux); `uniffi` 0.32.2 (MPL-2.0) for Swift and Kotlin, `tauri` 2.12.5 (Apache-2.0 OR MIT). Implementation of this feature waits for the user's go; the shell order (S6) is the only open product decision, and it is needed only before T4.
