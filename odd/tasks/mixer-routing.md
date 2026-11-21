# Feature: mixer, buses and output routing

Objective: mix stems through buses with per-stem, per-bus and master levels, and route each bus to the interface's outputs, saved per arrangement and per device, in the engine, the `app` API and the desktop app.
Branch: `feat/mixer-routing` (from `setlists/apps` at a3b3e12 while PR #14 waits for CI; PRs stack on #14 until it merges) · Delivery: work-unit commits, stacked PRs to `main` · Test runner: `cargo test --workspace` with Rust 1.93.0 (`rust-toolchain.toml`).

## Specs

Requirements quote `docs/mvp-spec.md` verbatim. Items marked **Assumption** are not in that document and are the parent's call until the user says otherwise.

### S1 — Mixer and buses (F7)
- F7.1: "Each stem has volume, mute, and solo; each bus has volume and mute; there is one master volume."
- F7.2: "Stems go to buses automatically by part (F1.2); the user can reassign a stem to another bus."
- F1.2: "Assign a part to each stem automatically from its file name, using our own rules table; the user can override it."
- F7.3: "Solo never mutes click or guide."
- F7.4: "Volume changes ramp smoothly, with no zipper noise."
- F7.5: "The mix is saved per arrangement; bus settings are global unless the user enables per-song buses."
- Acceptance: "Muting a bus silences every stem in it and restores their levels on unmute."
- **Decision (user, L2):** the mix of each arrangement (stem levels, mutes, solos, bus assignments) is saved in a `mix.json` beside `song.json` in the song folder; `song.json` is never rewritten. Global bus settings live in the app's configuration folder.
- **Assumption:** six fixed buses as in the design canvas: Click & Guide, Drums, Bass, Keys & Pads, Guitars, Vocals; the generated click and click/guide stems go to Click & Guide.
- **Assumption:** solo applies to stems only (no bus solo, as in the design); a muted bus stays silent even with a soloed stem in it.
- **Assumption:** in a setlist each song plays with its own saved mix; bus levels are global, so they carry from song to song.

### S2 — Output routing (F8)
- F8.1: "List the audio devices available and their output channel count."
- F8.2: "Route each bus to a mono output or a stereo pair, up to 32 outputs."
- F8.3: "Apply a default routing preset based on the device's output count; the user can edit it and the routing is saved per device."
- F8.4: "**Split mode** for 2-output devices: click and guide on the left, the stems summed to mono on the right."
- F8.5: "Choose the buffer size (64 to 1024 frames) and show the resulting latency."
- Acceptance: "Unplugging the device pauses playback with a clear message; plugging it back in restores the routing."
- **Decision (user, L2):** routing is saved per device in the app's configuration folder.
- **Assumption:** a mono output receives the bus's left and right summed; several buses may share an output (they are summed), as the design's bus layer shows.
- **Assumption:** default presets by output count follow the design (2 outputs: split mode on; 4, 8, 16+: Click & Guide on its own pair and the other buses spread over the rest); buses beyond the device's outputs fall back to outputs 1/2.
- **Assumption:** at unity gain with the stereo routing on outputs 1/2, the output stays bit-identical to today's (offline renders, `render-cli`, live null tests unchanged).

### S3 — In the apps
- The design canvas's Mixer screen (stems and buses layers, "Save mix to arrangement", "Reset to song mix") and Audio output screen (device, sample rate shown, buffer 64–1024 with latency, split mode, routing matrix with presets and reset).
- **Assumption:** `play-cli` and `render-cli` keep today's stereo behaviour; `play-cli` may gain options to pick a routing preset for hardware spikes.

Out of scope: MIDI (F9), guide-voice cues (F4), per-song buses (F7.5's opt-in), meters beyond what the screens show, effects.

## Tasks

| ID | Specs | Route | Status | Commit |
|----|-------|-------|--------|--------|
| T1 | S1, S2 | delegated (context backstop) | in progress | — |
| T2 | S1, S2 | delegated (context backstop) | pending | — |
| T3 | S1, S2 | delegated (context backstop) | pending | — |
| T4 | S1, S2 | delegated (context backstop) | pending | — |
| T5 | S3 | delegated (context backstop) | pending | — |

T1 `domain`: buses, part-to-bus rules table, arrangement mix model and `mix.json` v1, routing model, presets and split mode. T2 `library` and app config: read and write `mix.json` atomically, per-device routing and global bus settings in the config folder. T3 `engine`: per-stem, per-bus and master gain ramps, mute and solo rules, bus rendering and the router into N channels, allocation-free, unity bit-exact. T4 `io-cpal` and `app`: multichannel open, routing in the output request (restored on reopen), mixer and routing commands, snapshot levels, setlist carry. T5 desktop: Mixer and Audio output screens.

Route evidence: the parent session is long and past the ~150k-token backstop. Order: model, storage, engine, app, screens; each needs the previous one. Forecast: ~4,700 authored lines. Risk forecast: T1 item 3 (a new format others may read, `mix.json`, additive); T2 item 1 (writes into song folders and a config folder); T3 item 4 and 6 (audio thread, bit-exact goldens); T4 item 3 (the output port contract changes from stereo to N channels); T5 item 6 (GPUI drawing).

## Log

- **L1** (2026-11-20, user, verbatim): "ahora si está bien, creo que podemos pasar a la siguiente fase". Asked which phase follows the setlists merge, the user chose verbatim "Mezcla + ruteo (F7+F8) (Recommended)". The parent merged PRs #11, #12 and #13 into `main` in order (merge commits; CI green on Linux, macOS and Windows), retargeted #14 to `main` and waits for its CI (the repository does not allow auto-merge). A read-only explorer mapped the code: the engine, the `AudioRenderer` port and `io-cpal` carry one interleaved stereo pair and `write_interleaved` writes silence above output 2; `io-cpal::choose_config` opens the fewest channels at or above two (pinned by tests); `domain::Stem.part` is unvalidated free text and the package has no bus, mix or routing fields; the app persists no settings; summing through buses would reorder float additions and break the bit-exact tests unless the stereo mixdown keeps a stem-order sum with effective gains.
- **L2** (2026-11-20, user, verbatim): asked where the mix of each arrangement and the per-device routing are saved, the user chose "mix.json en la carpeta (Recommended)": the mix per arrangement in a `mix.json` beside `song.json` in the song folder (`song.json` is never rewritten); global bus settings and per-device routing in the app's configuration folder.
