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
- **Decided (L3):** the first shell is `apps/desktop` (Tauri 2 + React), developed and run on the development Mac; it ships for Windows and Linux.

Out of scope: setlists and transitions, mixer UI, routing matrix, MIDI, guide-voice playback, pads, key and tempo change, the other two shells.

## Tasks

| ID | Specs | Route | Status | Commit |
|----|-------|-------|--------|--------|
| T1 | S2 (+ public domain helpers for bar and click queries in source time) | delegated (context backstop) | verified; native review pending | 64638b4, 1e35bd3 |
| T2 | S3 + S2 defer-the-jump criterion + T1 advisories A2–A4, A6 | delegated (context backstop) | pending | — |
| T3 | S1, S4, S5 | delegated (context backstop) | pending | — |
| T4 | S6 | delegated (context backstop) | pending | — |

Route evidence: the parent session is past the ~150k-token context backstop. Order: the live transport first (highest risk, testable offline with in-memory sources), then streaming behind the same source interface, then the `app` API with real threads, `cpal` and `play-cli`, then the shell. Each task needs the previous one, so they run sequentially. Forecast: ~2,500 authored lines. Risk forecast: item 4 (concurrency: audio thread, disk thread, lock-free queues) for T2 and T3, item 5 (new crates `cpal`, `rtrb`) for T3 — each gets an independent verifier and the native review.

## Log

- **L1** (2026-10-30, user, verbatim): "Native per platform"
- **L2** (2026-10-30): Decision 1 recorded in docs/architecture.md: Rust core + SwiftUI (iPhone, iPad, Mac) + Compose (Android) + Tauri with React (Windows, Linux); `uniffi` 0.32.2 (MPL-2.0) for Swift and Kotlin, `tauri` 2.12.5 (Apache-2.0 OR MIT). Implementation of this feature waits for the user's go; the shell order (S6) is the only open product decision, and it is needed only before T4.
- **L3** (2026-10-30, user, verbatim): "Desktop first (Tauri)"
- **L4** (2026-10-30): branch `feat/live-playback` created from `feat/core-render-pipeline` (cc2235a). Tasks reordered: T1 live transport (S2), T2 streaming (S3), T3 `app` API + `cpal` + `play-cli` (S1, S4, S5), T4 desktop shell (S6).
- **L5** (2026-10-30): T1 delivered as 64638b4 (domain `BeatGrid`: `clicks_in`, `next_bar_line`, `bar_and_beat`; `compile` now uses it, output unchanged) and 1e35bd3 (`engine::live`: `Player::process`, `Controller` over `rtrb` 0.4.1, `StemSource` + `MemorySource`, `Status`). 125 tests; parent re-ran the suite. Live output equals `render_offline` bit-exactly with no commands, for buffer sizes 1/7/64/256/1024. Writer decisions to confirm: a queued jump overrides and switches off infinite; next bar = first bar line at or after the next frame, else section end; live loop always waits for the section end; pause/stop/resume/click ramp over 256 frames; stop, end of panic and end of arrangement reset to position 0; fade is linear, 3 s default, track stems only; click on/off affects only the generated click (a recorded click stem keeps playing — product call); back to start while playing crossfades; up to 8 overlapping tails; output rate must equal the song rate. Open for T2: `StemSource` needs a readiness query and a prefetch hint for the defer-the-jump rule. Risk: item 5 (`rtrb`), item 4 (SPSC queue is the cross-thread channel), item 3 checked (shared `compile` and mixing code refactored with unchanged output).
- **L6** (2026-10-30): independent verifier on 1e35bd3 vs 1bd8d23 — no blockers; S2 passes everything built (bit-exact vs `render_offline` across sizes 1–4096 and tempo/meter changes; jumps, next bar, loop, infinite, fade scope, panic, click grid; zero allocations; `compile` and `render-cli` output byte-identical to the baseline). Open: A1 the "move the jump to the next boundary when the target is not buffered" criterion is not built (needs a readiness query on `StemSource`) — moved to T2; A2 the player walks the arrangement and copies compile's seam rule instead of recompiling a plan — share one helper and update Decision 2 wording; A3 two seams on the same frame cancel the first fade-in (audible bump); A4 a live loop clears infinite when it executes — it should not; A5 click off with a recorded click stem changes nothing (product call pending with the user); A6 fade duration 0 gives an unsmoothed step — enforce a minimum; A7 a ninth overlapping tail is dropped abruptly; A9 song and device rates must match (T3). `rtrb` 0.4.1: MIT OR Apache-2.0, no dependencies, no build script. Verifier scratch removed by the parent.
