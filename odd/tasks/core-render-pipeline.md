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
- **Assumption (hardening, from verification):** `formatVersion` is checked before the rest of the shape, so a future version reports "unsupported formatVersion"; a package needs at least one section and every arrangement at least one section; song length is capped at 24 hours; `stems[].file` must be a non-empty relative path with no `..` component.

### S2 — Arrangement compiler (`crates/domain`)
- Decision 2: "the `domain` layer **compiles** a song, an arrangement, and its settings into a `RenderPlan`. The engine just plays that plan against a single sample counter."
- "`compile()` is a pure function."
- Plan shape: "segments: [out_start, src_start, length, fade_in, fade_out]" and "events: click ticks, guide cues, count-ins (output sample time)".
- F2.3 acceptance: "An arrangement `Intro, Verse, Chorus, Chorus, Outro` renders the chorus twice with no gap or click at the seam."
- Testing strategy: "Arrangements produce contiguous, non-overlapping segments; cues never overlap".
- **Assumption (from verification):** at a seam where the source jumps, the incoming segment has `fade_in = n` and the outgoing segment has `fade_out = n`: during the first `n` output samples of the incoming segment, the outgoing segment keeps reading its source past its end and fades out while the incoming one fades in (equal power). `n` is capped by the incoming length and by the source audio left after the outgoing end. Contiguous seams have both fades at 0.

### S3 — Click events (`crates/domain`)
- F3.1: "Generate the click from the tempo and time-signature maps, with an accent on beat 1."
- F3.2: "Mix four click levels independently: accent, quarter, eighth, and sixteenth notes."
- F3.1 acceptance: "Every click lands within ±1 sample of its computed beat position, including across tempo changes."
- **Assumption:** BPM counts the time-signature denominator note; the beat grid is continuous across tempo changes; bars restart at each time-signature point. Clicks inside a segment follow the *source* position, so they stay aligned with the audio after rearranging.
- **Assumption (from verification):** the four levels are relative to the beat (the denominator note): beat 1 of the bar, other beats, half-beats, quarter-beats. In 4/4 they are the spec's accent, quarter, eighth and sixteenth notes.

### S4 — Guide cue events (`crates/domain`)
- F4.1: "Generate guide cues from the arrangement: before each section, play the cue for its type and number."
- F4.2: "Default timing is **one bar before the section starts**; the user can switch to **at section start**."
- F4.3: "Cues follow the arrangement, so reordering sections updates the cues (dynamic guide)."
- Acceptance: "Reordering `Verse → Chorus` to `Chorus → Verse` makes the cues say "Chorus" then "Verse"." and "A cue never overlaps the next cue; if a section is shorter than the cue, the cue is skipped."
- **Assumption:** this feature emits cue *events* only (no voice samples exist yet); a cue that would start before sample 0 is dropped.
- **Clarification (from verification):** "one bar before the section starts" is the bar the musician hears just before the section in the *output*: one bar of the preceding segment, measured with the tempo and meter at the preceding segment's source end. The first segment has no preceding bar and uses its own. Each cue event carries its length (`cue_length`, default one bar as measured above).

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
| T1 | S1 | delegated (context backstop) | built, verification pending | 0162561 |
| T2 | S2, S3, S4 | delegated (context backstop) | built, verification pending | 0162561 |
| T3 | S5, S6 | delegated (context backstop) | pending | — |

Route evidence: the parent session is past the ~150k-token context backstop, so each unit goes to one bounded writer; units are sequential (T2 needs T1's types, T3 needs T2's plan). Forecast: ~1,500 authored lines in 3 work-unit commits.

## Log

- **L1** (2026-10-26, user, verbatim): "quiero hacer una app para todas las plataformas que hago lo mismo que la aplicacion de playback pero opensource. Necesito que investigues como funciona"
- **L2** (2026-10-30, user, verbatim): "por ahora hagamoslo sin la conexion de nube, eso sera en una nueva entrega"
- **L3** (2026-10-30, user, verbatim): "comienza pues"
- **L4** (2026-10-30): UI shell (Decision 1) and project license (Decision 5) are still open; this feature only builds the Rust core, which every option discussed shares. No license file is added until Decision 5 is made.
- **L5** (2026-10-30): T1+T2 delivered in commit 0162561 (44 tests, fmt and clippy clean, RED observed before GREEN). Writer decisions beyond the brief, to confirm: bpm bounded to 1–999; tempo and meter maps must start at 0; a package with no stems is an error; a section starting at the song end is rejected; `crossfade_in` capped at the segment length; `infiniteLoop` ids are validated but ignored by `compile`; a cue that starts before the previous kept cue ends is dropped; each stem's sample rate is checked against the package rate. Dependencies: serde 1.0.229, serde_json 1.0.154 (transitive zmij 1.0.23, MIT, dtolnay), thiserror 2.0.21. Risk: item 5 (first third-party crates) — independent verifier launched; native review assessed `high`, START preflight ready (lenses risk, reliability, readability), waiting for the verifier before START.
- **L6** (2026-10-30): independent verifier on 304de8b — S1 pass, S3 pass (max error 0.49999 samples over 60 min against an exact rational oracle), S2 fail (B2: plan has no `fade_out`), S4 fail (B1: "one bar before" measured with the announced section's bar, off the bar line at tempo or meter changes). Advisories A1 (no length cap → compile exhausts memory), A3 (level names), A4 (empty arrangement validates), A5 (version checked late), A6 (stem paths not confined), A7 (cue events carry no length). Dependencies all from dtolnay/serde-rs/BurntSushi; unicode-ident adds a Unicode-3.0 term to note for Decision 5. Decision: B1 resolved as the preceding output bar (musical meaning of a guide cue; recorded as a clarification in S4); B2, A1, A4–A7 fixed in one correction batch; A3 recorded as an S3 assumption; A2 (meter point off the grid) left to the future editor. T1/T2 reopened for that batch.
