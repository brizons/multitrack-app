# play-cli

Headless live player for development and hardware spikes. It plays one song folder, or a setlist from a library folder, on an audio device through the `app` API, with single-key controls and a one-line status refreshed about 10 times per second. Nothing is resampled: the device runs at the song's sample rate.

```text
play-cli <SONG_DIR> [--device <NAME>] [--buffer <FRAMES>] [--list-devices]
play-cli --setlist <FILE> [--device <NAME>] [--buffer <FRAMES>]
```

| Flag | Meaning |
|------|---------|
| `<SONG_DIR>` | Song folder: `song.json` plus the WAV stems it lists. The `default` arrangement plays. |
| `--setlist <FILE>` | Play a setlist instead: a file such as `LIBRARY/Setlists/Sunday.setlist.json`. See [Setlist mode](#setlist-mode). Cannot be combined with `<SONG_DIR>` (exit code 2). |
| `--device <NAME>` | Output device, by the name or the id `--list-devices` prints; names match exactly, then ignoring case. A name two devices share is refused: use the id. Default: the system default output. Either way the device is pinned by its id when the song opens. |
| `--buffer <FRAMES>` | Frames per audio buffer, 64 to 1024. Default: 256. The status line shows the resulting latency. |
| `--list-devices` | List the output devices with their output channel counts and ids, the default marked `*`, then exit. Cannot be combined with a song folder, a setlist, `--device` or `--buffer` (exit code 2). |

The song plays in stereo on outputs 1 and 2 of the device.

## Controls

Single keys, no Enter. Letters work in either case.

| Key | Does |
|-----|------|
| space | Play or pause. From stopped or paused, play first waits until the disk has the audio at the playhead (`LOADING`), at most 5 s. |
| `s` | Stop and return to the start. |
| `b` | Back to the start. |
| `1`–`9` | Queue a jump to arrangement position 1–9. |
| `n` | Jump timing: at the section end, or at the next bar. |
| `l` | Live loop: repeat the current section once more. |
| `i` | Infinite loop on or off. |
| `f` | Fade the track stems out, or back in. Click and guide keep playing. |
| `c` | Cancel the queued jump or loop. |
| `x` | Panic: fade everything out in 0.5 s and stop. |
| `q`, Ctrl-C | Quit. Playback stops with the usual short ramp first. |

## Example

```console
$ play-cli --list-devices
* MacBook Pro Speakers (2 output channels), id coreaudio:BuiltInSpeakerDevice
  USB Audio Interface (18 output channels), id coreaudio:AppleUSBAudioEngine:Vendor:USB Audio Interface:1100000:1
  USB Audio Interface (18 output channels), id coreaudio:AppleUSBAudioEngine:Vendor:USB Audio Interface:1200000:1
$ play-cli songs/example --device "coreaudio:AppleUSBAudioEngine:Vendor:USB Audio Interface:1200000:1" --buffer 128
"Example Song" (arrangement "default", 6 positions) on "USB Audio Interface": 48000 Hz, 128 frames per buffer, 2.7 ms latency
space: play/pause, 1-9: queue a section, q: quit; play-cli --help lists every key
PLAYING  | bar 12.3 | Chorus (3/6) | next: jump to Bridge in 4.2 s | timing: section end | fade: in | 48000 Hz, 128 frames, 2.7 ms on "USB Audio Interface"
```

The status line holds, separated by `|`: the transport (`STOPPED`, `LOADING`, `PLAYING`, `PAUSED`, `STOPPING`, `PANIC`), bar and beat, the section and its position in the arrangement, the queued jump or loop with the time until it executes (and how many boundaries it has waited when its audio was not loaded), the jump timing, `infinite` when the infinite loop is on, the fade, and the device. Underruns and the last problem, prefixed `!`, follow when there are any. The line is cut to the terminal width.

## Setlist mode

`--setlist` plays a setlist file from a library folder. A setlist lives in the `Setlists` folder of its library folder, and its song paths are relative to that library folder, so play-cli takes the library to be the folder holding `Setlists` (the folder's name may be in any case; `.` and `..` in the path are resolved by name, without following symlinks).

Before anything plays, play-cli reads the setlist, checks it against the library's songs and prepares its first playable item, then prints one `warning:` line on stderr for each reason an item cannot play (a song the library lacks or cannot read, an arrangement or start and end positions the song does not have, a click-only tile that breaks the setlist rules, a song that does not stream, such as one whose sample rate is too low) and for each song at another sample rate than the device. The device opens at the first playable song's rate; a song at another rate plays after the device reopens at its rate, and no AutoLink or Crossfade leads into it. The rest of the setlist plays. Then it checks the terminal and the device before any stream opens. A setlist with no item that can play is an error.

All the keys of [Controls](#controls) act on the current item, and three more move through the setlist:

| Key | Does |
|-----|------|
| `.` | Next item: the item playing stops with its ramp, and the next one that can play waits at its start. |
| `,` | Previous item, the same way. |
| `g` | Go to an item: type its number (up to four digits), then Enter. Escape cancels, Backspace deletes a digit, any other key cancels and does its own job. The digits typed after `g` name an item; otherwise `1`–`9` still queue sections. |

Each item follows on with its transition: Cue next stops and selects the next item, Stay stops and keeps the item selected, AutoLink starts the next item on the sample after the item ends, Crossfade blends the two over the item's last five seconds. A song item can play an excerpt of its arrangement and fade out over its last five seconds; a click-only tile plays its bars, or repeats its bar until you move on (`.`, or `i` to let it end at the bar line).

```console
$ play-cli --setlist ~/Music/Library/Setlists/Sunday.setlist.json
warning: item 3 ("Hymn") runs at 44100 Hz and the device at 48000 Hz: it plays once the device reopens at its rate, and no AutoLink or Crossfade leads into it
setlist "Sunday" (5 items) on "USB Audio Interface": 48000 Hz, 256 frames per buffer, 5.3 ms latency
space: play/pause, . and ,: next and previous item, g then a number and Enter: go to an item, q: quit; play-cli --help lists every key
item 2/5 "Opening" | then: AutoLink -> 3 "Hymn" | PLAYING  | bar 12.3 | Chorus (3/6) | next: - | timing: section end | fade: in | 48000 Hz, 256 frames, 5.3 ms on "USB Audio Interface"
```

The setlist's status line starts with `go to item 12_ (Enter, Esc)`, showing the digits typed so far, while a number is being typed after `g`, then the current item and its place in the setlist, `crossfade from N "…" P%` while a crossfade runs, and what follows the item: `then:` with its transition and the next item (`(loading)` until that item's start is buffered), `then: Stay`, or `then: end of setlist`. The current item's own status follows, as in single-song mode. At the end, prefixed `!`, come the setlist's last problem (an item that could not be prepared, a link that was late because the next item was not loaded in time, a transition the audio thread refused) and a key the setlist refused (no next or previous item, an item that cannot play). While the device is held for a song at another rate it refuses, the device field says `NO DEVICE` and why; choosing an item at a rate the device takes brings it back.

## Device lost

When the device's stream stops, playback pauses; the status line says `NO DEVICE` with the reason (unplugged, stream invalidated by the system, busy with another application, and so on), and play is refused until the device opens again. Other keys still work. play-cli tries to open the same device again every half second, found by its id, so an identical interface with the same name is never taken instead; the transport stays paused: press space to go on. When the device is there but refuses to open, for example because it came back at another sample rate, the status line says why instead (`cannot open it again: …`).

To check it by hand: start a song on a USB interface or headphones, unplug them while it plays, wait, plug them back in, and press space.

## Exit codes

| Code | When | Output |
|------|------|--------|
| 0 | Quit with `q` or Ctrl-C, or `--list-devices` listed the devices. | The device list on stdout for `--list-devices`. |
| 1 | The song folder cannot be loaded or validated; the setlist file is not in a `Setlists` folder, cannot be read, or is not a valid setlist, or no item of it can play (each item's reasons follow); the device cannot be opened (not found, a name two devices share, not at the song's sample rate, buffer size refused); standard input is not a terminal; or the terminal fails. | Every error on its own stderr line, prefixed `error:`; package warnings and the setlist's problems prefixed `warning:`. |
| 2 | Usage error, such as a missing `<SONG_DIR>`, `<SONG_DIR>` together with `--setlist`, or `--buffer` outside 64–1024. | clap's message on stderr. |
