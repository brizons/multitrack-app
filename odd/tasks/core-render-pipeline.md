# Feature: core render pipeline (Rust core, no hardware)

Objective: first slice of the Rust core from `docs/architecture.md` — song package model, timeline compiler, and a deterministic offline renderer driven by `render-cli`. Testable without audio hardware.
Branch: `feat/core-render-pipeline` · Delivery: work-unit commits, `ask-on-risk` (chain strategy asked before the first pull request) · Test runner: `cargo test --workspace`.

## Specs

Requirements quote `docs/mvp-spec.md` and `docs/architecture.md` verbatim where they exist. Items marked **Assumption** are not in those documents and must stay easy to change.

### S1 — Song package model and validation (`crates/domain`)
- Format: "A song is a folder with a manifest and its stems. The format is plain JSON so other tools can read and write it." Fields as in the spec example: `formatVersion`, `title`, `artist`, `sampleRate`, `key`, `tempoMap[{at,bpm}]`, `timeSignatureMap[{at,numerator,denominator}]`, `stems[{id,file,part,role}]`, `sections[{id,type,number,start}]`, `arrangements[{id,name,sections,infiniteLoop}]`.
- "Positions (`at`, `start`) are **sample offsets** at `sampleRate`."
- "A section ends where the next section starts; the last one ends at the song end."
- "`role` is `track`, `click`, or `guide`."
- F1.5: "Reject a package whose stems differ in sample rate, and warn when stem lengths differ by more than one second."
- F2.3: "A **Default** arrangement always exists and cannot be deleted."
- **Assumption:** the Default arrangement has id `default`; the first section starts at 0; section starts are strictly increasing; ids are unique; arrangements only reference existing sections; at most one `click` and one `guide` stem; `formatVersion` must be 1; the song end is the longest stem length.
- **Assumption:** section `type` values are kebab-case: `count-off`, `intro`, `verse`, `pre-chorus`, `chorus`, `post-chorus`, `bridge`, `instrumental`, `interlude`, `tag`, `vamp`, `turnaround`, `breakdown`, `outro`, `ending`.

### S2 — Arrangement compiler (`crates/domain`)
- Decision 2: "the `domain` layer **compiles** a song, an arrangement, and its settings into a `RenderPlan`. The engine just plays that plan against a single sample counter."
- "`compile()` is a pure function."
- Plan shape: "segments: [out_start, src_start, length, fade_in, fade_out]" and "events: click ticks, guide cues, count-ins (output sample time)".
- F2.3 acceptance: "An arrangement `Intro, Verse, Chorus, Chorus, Outro` renders the chorus twice with no gap or click at the seam."
- Testing strategy: "Arrangements produce contiguous, non-overlapping segments; cues never overlap".

### S3 — Click events (`crates/domain`)
- F3.1: "Generate the click from the tempo and time-signature maps, with an accent on beat 1."
- F3.2: "Mix four click levels independently: accent, quarter, eighth, and sixteenth notes."
- F3.1 acceptance: "Every click lands within ±1 sample of its computed beat position, including across tempo changes."
- **Assumption:** BPM counts the time-signature denominator note; the beat grid is continuous across tempo changes; bars restart at each time-signature point. Clicks inside a segment follow the *source* position, so they stay aligned with the audio after rearranging.

### S4 — Guide cue events (`crates/domain`)
- F4.1: "Generate guide cues from the arrangement: before each section, play the cue for its type and number."
- F4.2: "Default timing is **one bar before the section starts**; the user can switch to **at section start**."
- F4.3: "Cues follow the arrangement, so reordering sections updates the cues (dynamic guide)."
- Acceptance: "Reordering `Verse → Chorus` to `Chorus → Verse` makes the cues say "Chorus" then "Verse"." and "A cue never overlaps the next cue; if a section is shorter than the cue, the cue is skipped."
- **Assumption:** this feature emits cue *events* only (no voice samples exist yet); a cue that would start before sample 0 is dropped.

### S5 — Deterministic offline renderer (`crates/engine`, `crates/library`)
- NFR: "Rendering a song offline twice produces bit-identical output."
- NFR: "All stems stay sample-aligned for 60 minutes (zero drift)."
- Testing strategy: "Null test: engine output minus the plain sum of the stems" — "Mixing and seams add nothing"; "Impulse test on the click" — "Every tick lands on its computed sample".
- Real-time checklist applies to the render loop: "No memory allocation or deallocation." (buffers preallocated before rendering).
- **Assumption:** WAV input only in this feature (16/24/32-bit integer and 32-bit float, mono or stereo); stereo 32-bit float WAV output; a short equal-power crossfade is applied only at seams where the source position jumps, so contiguous plans stay bit-exact.

### S6 — `render-cli` (`crates/render-cli`)
- Repository layout: "`render-cli/` headless renderer for tests and CI".
- **Assumption:** `render-cli <song-dir> --output <file.wav> [--arrangement <id>] [--no-click]`; prints a one-line summary; exit code 0 on success, 1 on validation or render errors (all errors listed), 2 on usage errors.

Out of scope for this feature: real-time audio I/O, MIDI, UI, FLAC/MP3 decoding, count-in, key and tempo change, live jumps.

## Tasks

| ID | Specs | Route | Status | Commit |
|----|-------|-------|--------|--------|
| T0 | — | inline | done | docs commit |
| T1 | S1 | delegated (context backstop) | pending | — |
| T2 | S2, S3, S4 | delegated (context backstop) | pending | — |
| T3 | S5, S6 | delegated (context backstop) | pending | — |

Route evidence: the parent session is past the ~150k-token context backstop, so each unit goes to one bounded writer; units are sequential (T2 needs T1's types, T3 needs T2's plan). Forecast: ~1,500 authored lines in 3 work-unit commits.

## Log

- **L1** (2026-10-26, user, verbatim): "quiero hacer una app para todas las plataformas que hago lo mismo que la aplicacion de playback pero opensource. Necesito que investigues como funciona"
- **L2** (2026-10-30, user, verbatim): "por ahora hagamoslo sin la conexion de nube, eso sera en una nueva entrega"
- **L3** (2026-10-30, user, verbatim): "comienza pues"
- **L4** (2026-10-30): UI shell (Decision 1) and project license (Decision 5) are still open; this feature only builds the Rust core, which every option discussed shares. No license file is added until Decision 5 is made.
