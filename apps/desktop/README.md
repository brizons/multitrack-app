# OpenVamp desktop app

The desktop shell for Mac, Windows and Linux: the Perform screen from the design canvas, drawn with GPUI (`gpui` 0.2.2) and calling the `app` crate directly. It plays one song folder, or a setlist from a library folder, on the system's default audio output, at the song's sample rate.

```text
openvamp [SONG_DIR]
openvamp SETLIST_FILE
```

`SONG_DIR` is a song folder: `song.json` and the WAV stems it lists; its `default` arrangement plays. `SETLIST_FILE` is a file ending in `.setlist.json` in the `Setlists` folder of a library folder (the folder's name may be in any case); the library folder is the one holding `Setlists`, and the setlist's song paths are relative to it. Without either, the screen offers **Open song…** (also Cmd-O on the Mac, Ctrl-O elsewhere) and **Open setlist…**. `--help` prints the usage; any other option, or a second argument, is named in the error and exits with code 2. A window that cannot open exits with code 1.

To try it without a song of your own, write the demo song (48 kHz, 8 positions, 48 s, one quiet chord stem) and open it:

```console
$ cargo run -p openvamp --example demo_song -- /tmp/demo-song
$ cargo run -p openvamp -- /tmp/demo-song
```

## The Perform screen

| Area | Shows and does |
|------|----------------|
| Header | **Open song…** and **Open setlist…**, or with a setlist its name and the current item's place (`2 of 5`), which opens another setlist; back to start, previous song and next song (with a setlist), play/pause, stop; the time in the arrangement, bar and beat, and the transport state (`Loading…` while play waits for the disk); **Fade** (shows the direction and progress); **Panic** (hold). |
| Setlist strip | With a setlist: a tile per item, in play order, with its number, title and what it plays (key, tempo, arrangement, start and end, fade-out; a click-only tile's tempo, meter and bars), marked played, current or upcoming; the next item is marked **Next**, or **Loading…** until its start is buffered. Between tiles, each item's transition (**Cue**, **Stay**, **Auto**, **X-fade**). A tile selects its item. An item that cannot play, or a song at another sample rate, shows why instead of what it plays. |
| Setlist notes | With a setlist, when there is something to say: a crossfade under way (both items and its progress), every reason an item cannot play, each song at another sample rate than the device, the setlist's last problem (an item that could not be prepared, such as a start or end its song no longer has; a link that was late; a transition the audio thread refused), a previous or next that had nowhere to go, and the device held for an item at a rate it refuses. |
| Now and Next | The section playing and the time left in it; what plays next: the queued jump or loop and when it executes, the current section again under the infinite loop, the next section, or the end of the song. A queued jump can be cancelled here. |
| Song map | Every arrangement position in play order, sized by length and colored by section family; the playhead; the queued jump's target (dashed) and a marker where it executes, with the time until then. |
| Section controls | **Jump at**: section end or next bar; **Loop section** (repeat the current section once more; press again to drop it); **Infinite**. |
| Pads | One per arrangement position, marked **Now** and **Next**: a pad queues a jump to its position; the queued pad, pressed again, cancels it. |
| Status | The device and its stream (rate, buffer, latency) or why there is none; underruns; the last problem, in the `app` API's words; the song package's warnings; the keys. |

**Panic** fires only after the button, or the X key, is held for one second; the fill shows the hold. A click or a tap never fires it, and neither does a hold whose release the window cannot see: when the window loses focus during the hold (Cmd-Tab, a system dialog, the folder picker), the hold ends without panic.

The session is polled about 30 times a second on a timer that keeps running while the window is hidden, since a waiting play starts only through polling.

**Open setlist…** asks for a setlist file. The setlist is read and checked against the library's songs before anything stops; a setlist with no item that can play leaves what plays playing and says why for each item. Otherwise it opens like a song (below), at its first playable item, stopped at its start, on a device opened at the first playable song's rate; each song item is named by its title from the library, or by its folder name when the library does not have it. All the transport controls act on the current item; previous and next skip the items that cannot play. A setlist and a song replace each other the same way a song replaces a song.

**Open song…** first checks the new folder and loads it without the audio device while the current song keeps playing, then asks the device, without opening it, whether it can play the song: that the device is there and takes the song's sample rate, the buffer size and stereo. A folder that does not open, a device that cannot play the song, or a folder picker that fails, leaves the current song playing (or, with its device unplugged, loaded and waiting for it to come back) and shows why under the header until dismissed. A device that cannot tell without opening, such as a busy one, goes ahead. Otherwise the current song stops with its ramp and closes the device, and only then does the new song open it: on macOS, opening a stream sets the device's sample rate, which would cut off a song still playing on it, and some interfaces take one stream at a time. If opening still fails (the stream cannot be built or started), the previous song is loaded again, stopped at the start, and both reasons show.

Quitting or closing the window while a song plays or waits to start (`Loading…`) asks first: **Stop and quit** or **Keep playing**, so that neither Space nor Escape ends the show. On the Mac, Return picks Stop and quit, and Escape (or Space, on the focused button) keeps playing. On Windows, Keep playing comes first as the default: Enter, Space and Escape keep playing. On Linux, gpui draws the question inside the window and it is answered with a click; while it is open, the screen's controls do nothing. Stop, the transport control, stays immediate. With nothing playing, both quit at once. The app sets no menus of its own. Quitting from the Dock, or when the system logs out, cannot be asked about (gpui 0.2.2 cannot cancel it), so the song just stops with its ramp.

Closing a song (opening another one, closing the window, quitting) stops it with the usual short ramp before the device closes. If the player refuses Stop because its command queue is full, Stop is sent again every 5 ms for up to 500 ms. A refusal, or a ramp that has not ended by then, is reported on stderr, and on screen when another song replaced it.

## Keys

The same single keys as `play-cli`; letters work in either case, and a key held with Ctrl, Alt or Cmd does nothing (except Open and Quit). A key held down acts once: its repeats are ignored until it is released, on every platform (on X11, gpui reports each repeat as a new press, so the screen tracks the keys that are down itself). On the Mac, pressing Cmd counts every key as up, since macOS never reports a key released while Cmd is held; elsewhere the Super or Windows key changes nothing.

| Key | Does |
|-----|------|
| Space | Play or pause. |
| `s` | Stop and return to the start. |
| `b` | Back to the start. |
| `1`–`9` | Queue a jump to arrangement position 1–9 (again on the queued one: cancel). |
| `n` | Jump timing: section end or next bar. |
| `l` | Loop section, or drop the queued loop. |
| `i` | Infinite loop on or off. |
| `f` | Fade the track stems out, or back in. |
| `c`, Escape | Cancel the queued jump or loop. |
| `,` / `.` | With a setlist: previous / next song (as in `play-cli`). |
| `x` (hold 1 s) | Panic: fade everything out in 0.5 s and stop. |
| Cmd-O / Ctrl-O | Open a song folder. |
| Cmd-Q / Ctrl-Q | Quit; while a song plays, only after confirming. A plain `q` does nothing, unlike `play-cli`, so a stray key on stage cannot end the show. |

## Building

On macOS, GPUI's `runtime_shaders` feature compiles the Metal shaders when the app starts, so the build needs no Metal Toolchain (Xcode 26 ships it as a separate download). The screen uses the system UI font, and Menlo, Consolas or DejaVu Sans Mono for times and keys; the canvas's Barlow and JetBrains Mono are not bundled.

The view-model (`src/view.rs`, `src/strip.rs`, `src/catalog.rs`, `src/action.rs`, `src/hold.rs`, `src/keys.rs`, `src/song.rs`, `src/show.rs`, `src/ending.rs`) holds every decision the screen makes and is tested without a window (`cargo test -p openvamp`); the GPUI code (`src/screen.rs`, `src/ui.rs`) only draws it and dispatches, and has no automated test.
