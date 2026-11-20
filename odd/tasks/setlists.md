# Feature: setlists and transitions

Objective: build, store and play an ordered setlist of songs with per-song arrangement, start and end points, fade-out and transitions, plus click-only tiles, driven through the `app` API and shown in the desktop app.
Branch: `feat/setlists` (from `main` at fe8c742) · Delivery: work-unit commits, stacked PRs to `main` (strategy chosen in live-playback L35) · Test runner: `cargo test --workspace` with Rust 1.93.0 (`rust-toolchain.toml`).

## Specs

Requirements quote `docs/mvp-spec.md` verbatim. Items marked **Assumption** are not in that document and are the parent's call until the user says otherwise.

### S1 — Setlist model (`crates/domain`)
- Glossary: "**Setlist** | An ordered list of songs, each with its arrangement and a transition to the next."
- F5.1: "Create, rename, duplicate, and reorder setlists and the songs in them."
- F5.2: "Pick an arrangement per song in the setlist."
- F5.3: "Set start and end points per song, with an optional five-second fade-out at the end."
- F5.4: "Choose a transition per song: **Cue next** (stop and select next), **Stay** (stop in song), **AutoLink** (start the next song immediately), **Crossfade** (blend over the last five seconds)."
- F5.5: "Insert a **click-only** tile at any tempo and time signature."
- **Assumption:** a setlist is a plain JSON file (format v1, like the song package) holding its name and an ordered list of items; a song item names its song folder by a path relative to the library folder, its arrangement id, optional start and end points as positions in that arrangement, the fade-out flag and its transition; a click-only item holds tempo, time signature, an optional bar count and its transition. Validation reports every problem at once, like the song package.
- **Assumption:** a click-only tile without a bar count plays until the operator moves on; with one, it ends after that many bars and then follows its transition.

### S2 — Setlists in the library folder (`crates/library`)
- F10.2: "Songs and setlists live in a user-visible library folder."
- F10.1: "All features work with no network connection and no account."
- **Assumption:** setlists live as `*.setlist.json` files in a `Setlists/` folder of the library; saving is atomic (temporary file then rename), like WAV output; a song a setlist names but the library lacks is reported by name, and the rest of the setlist stays usable.

### S3 — Setlist playback (`crates/engine`, `crates/app`)
- F6.1: "Play, pause, stop, back to start, previous or next song."
- Acceptance: "AutoLink starts the next song on the sample after the previous one ends."
- Acceptance: "Crossfade keeps the combined output below 0 dBFS when both songs peak at −1 dBFS."
- **Assumption (technical, from the acceptance above):** the crossfade is a linear (constant-sum) blend over the last five seconds: an equal-power curve sums two −1 dBFS peaks above 0 dBFS.
- **Assumption:** the device opens at the setlist's first song's rate; a later song at another rate is reported before the setlist starts and cannot AutoLink or crossfade (no resampling in the MVP, as in live-playback A9).
- **Assumption:** the next item is prepared (opened and its start buffered) while the current one plays, so AutoLink and Crossfade never wait for the disk.

### S4 — Setlist in the apps (`crates/play-cli`, `apps/desktop`)
- The design canvas's setlist strip and previous/next song controls on the Perform screen (left out of live-playback S6).
- **Assumption:** `play-cli` gains a setlist mode for development and hardware spikes; editing setlists (F5.1) happens in the desktop app.

Out of scope: mixer, routing matrix, MIDI, guide-voice playback, pads, key and tempo change, cloud sources, setlist export and import (v0.2), waveforms.

## Tasks

| ID | Specs | Route | Status | Commit |
|----|-------|-------|--------|--------|
| T1 | S1 | delegated (context backstop) | in progress | — |
| T2 | S2 | delegated (context backstop) | pending | — |
| T3 | S3 | delegated (context backstop) | pending | — |
| T4 | S4 | delegated (context backstop) | pending | — |

Route evidence: the parent session is long and past the ~150k-token backstop. Order: the pure model first, then storage, then playback (highest risk: two songs on the audio thread at once), then the apps. Each needs the previous one. Forecast: ~4,000 authored lines. Risk forecast: T3 item 4 (two song players and their disk threads during AutoLink and Crossfade) gets an independent verifier and the native review; the others get the native review when the assessment says so.

## Log

- **L1** (2026-11-19, user, verbatim): "mergea los PRs en orden y arranca con lo que sigue". The parent merged PRs #1–#10 into `main` in order (merge commits, `main` at fe8c742; CI green on Linux, macOS and Windows before the merge) and started this feature, the next one in docs/architecture.md "Next step" and the MVP spec (F5).
