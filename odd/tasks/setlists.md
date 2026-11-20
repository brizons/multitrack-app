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
| T1 | S1 | delegated (context backstop) | done (review approved, one fix; L2–L3) | 594bacd, 6c47f66, 107e906 |
| T2 | S2 | delegated (context backstop) | verified, one correction (L5); recheck and native review pending | 789707d, 5a09571, 49859c3, 3eee6bc, 12f2c7c |
| T3 | S3 | delegated (context backstop) | pending | — |
| T4 | S4 | delegated (context backstop) | pending | — |

Route evidence: the parent session is long and past the ~150k-token backstop. Order: the pure model first, then storage, then playback (highest risk: two songs on the audio thread at once), then the apps. Each needs the previous one. Forecast: ~4,000 authored lines. Risk forecast: T3 item 4 (two song players and their disk threads during AutoLink and Crossfade) gets an independent verifier and the native review; the others get the native review when the assessment says so.

## Log

- **L1** (2026-11-19, user, verbatim): "mergea los PRs en orden y arranca con lo que sigue". The parent merged PRs #1–#10 into `main` in order (merge commits, `main` at fe8c742; CI green on Linux, macOS and Windows before the merge) and started this feature, the next one in docs/architecture.md "Next step" and the MVP spec (F5).
- **L2** (2026-11-19): T1 delivered by one delegated writer as 594bacd (`refactor(domain)`: stem-path, BPM and time-signature checks shared as `pub(crate)` helpers, pinned by a regression test of the package's exact errors) and 6c47f66 (`feat(domain)`: `Setlist`, `SetlistItem::{Song, Click}`, `SongItem`, `ClickItem`, `TimeSignature`, `Transition`, `PositionBound`, `SongOutline`/`ArrangementOutline` and the `SongLookup` trait, `SetlistFileError`, `SetlistValidationError`, `SetlistEditError`; JSON format v1 with `formatVersion`, camelCase fields, `type: "song" | "click"`, transitions `cueNext`/`stay`/`autoLink`/`crossfade`; editing: rename, duplicate, insert, remove, move, set arrangement, set start/end, set fade-out, set transition; the index-based ones return an out-of-range error, rename and duplicate reject a blank or unchanged name). 24 setlist tests (RED on `todo!()` stubs, then GREEN); 311 tests pass, 5 ignored; fmt, clippy and docs clean; the parent re-ran the suite (311 passed). Writer decisions: start and end points are **arrangement positions** (section indexes, both inclusive), because `compile` and the live player only enter a position at its first sample; a click tile uses a new position-free `TimeSignature` (same validation as `TimeSignaturePoint`); a blank setlist name is rejected (S2 needs names for files); `CueNext` is the default transition; changing an item's arrangement clears its start and end. Constants `SONG_FADE_OUT_SECONDS` and `CROSSFADE_SECONDS` = 5.
- **L3** (2026-11-19): native review of T1 (f8a7c8e..T1 docs, lenses risk, readability, reliability) granted by the user, approved with no blocker, acknowledged (review-ee581982a0be0c37). Fixed inline by the parent: a click tile whose tempo is not finite was written as `null` and the file could not be read back; `Setlist::to_json` now refuses it with `SetlistFileError::NonFiniteBpm` (RED compile error, then GREEN; domain 90 tests). Open advisories: unknown or misspelled JSON keys are ignored and dropped on save (no `deny_unknown_fields`; the song package behaves the same); the `Transition::Stay` doc says "in the song" though it also applies to click tiles; L2 overstated the edit errors (corrected above).
- **L4** (2026-11-19): T2 delivered by one delegated writer as 789707d (`refactor(library)`: `resolve_inside` and `open_regular_file` shared by stems and setlists; stem messages pinned by the existing symlink and FIFO tests), 5a09571 (`library::Library`: `list_setlists`, `load_setlist`, `save_setlist`, `rename_setlist`, `duplicate_setlist`; `Setlists/<stem>.setlist.json`; atomic save through the existing temp-file helper; 1 MiB read limit; broken files listed apart without hiding the rest) and 49859c3 (`Library::setlist_songs` builds a `SongLookup` from each named song's manifest; `SongUnavailable` reports missing or broken songs by path; `song_folder` stays inside the library). 25 tests (RED on `todo!()` stubs, then GREEN); 337 tests pass, 5 ignored; fmt and clippy clean (clippy also for the Linux target); the parent re-ran the suite (337 passed). Name-to-file mapping: `/ \ : * ? " < > |`, control characters, `%` and a leading dot written as `%XX`; Windows device names (CON, PRN, AUX, NUL, COM0–9, LPT0–9 and the superscript variants) get their first letter escaped; stems cut at 200 bytes on a character boundary; names landing on the same file are refused with `FileTaken`, never overwritten; a case-only rename rewrites then renames. Delete left out (not in S2). Writer notes: song manifests are still read by `load_song`'s unbounded `read_to_string` without the FIFO guard (unchanged); a check-then-rename race assumes one program writes a library at a time (documented); a failure mid-write was not simulated. Not run on Windows locally. Risk: item 2 (path confinement and FIFO refusal in the user-visible library folder), item 1 (rename rewrites the user's setlist file), item 3 checked (shared stem-opening code, behaviour pinned). Independent verifier next.
- **L5** (2026-11-20): independent verifier on 49859c3 vs c8e218f — every S2 rule passes except one: escaping, Windows device names and 14 Unicode names round-trip; same-file names (cut, case, NFC vs NFD, symlinks) get `FileTaken` with originals byte-identical; atomic save survives a read-only folder, a locked file and a real failure mid-write (`ulimit -f`); the 1 MiB limit is exact; FIFOs and symlinks out are refused for setlist files and the `Setlists` folder; missing songs are reported by path; `render-cli`/`play-cli` outputs and stem load messages byte-identical to the baseline; 337 tests. **B1:** on HFS+ and exFAT (names stored decomposed) a case-only rename such as "Domingo Mañana" → "domingo mañana" was refused with `FileTaken` naming a setlist that does not exist. Fixed inline by the parent in 12f2c7c: on unix the file's identity decides alone; the verifier's test is committed (RED `FileTaken`, then GREEN; library 53 tests; clippy also for the Windows target). Also 3eee6bc: an import used only by unix-only tests broke clippy on Windows CI (Auto-fix). Advisories: song manifests are still read by `open_package` without confinement, size limit or FIFO guard (baseline behaviour; the `setlist_songs` doc now says so); a device-name escape after the 200-byte cut can give a 202-byte stem; re-saving drops permission bits (like the WAV writer); unclear errors for a dangling symlink at the target, a `Setlists` file, and a dangling `Setlists` symlink (lists nothing); save is keyed by name, so a hand-named file is saved to a second file (matters for T4); a case-only rename whose final rename fails reports `Write` after rewriting; backslash song paths resolve only on Windows (T4 should write `/`); Windows same-file detection relies on size and time. Windows CI must confirm: device-name files, `CON.setlist.json` returning promptly, case-only renames on NTFS, a 200-byte stem under a >260-char path, the `:` refusal, read-only attribute saves; no junction confinement test exists on Windows. Verifier scratch removed by the parent.
