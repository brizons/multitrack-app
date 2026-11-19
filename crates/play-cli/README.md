# play-cli

Headless live player for development and hardware spikes. It plays one song folder on an audio device through the `app` API, with single-key controls and a one-line status refreshed about 10 times per second. Nothing is resampled: the device runs at the song's sample rate.

```text
play-cli <SONG_DIR> [--device <NAME>] [--buffer <FRAMES>] [--list-devices]
```

| Flag | Meaning |
|------|---------|
| `<SONG_DIR>` | Song folder: `song.json` plus the WAV stems it lists. The `default` arrangement plays. |
| `--device <NAME>` | Output device, by the name `--list-devices` prints. Default: the system default output, pinned when the song opens. |
| `--buffer <FRAMES>` | Frames per audio buffer, 64 to 1024. Default: 256. The status line shows the resulting latency. |
| `--list-devices` | List the output devices with their output channel counts, the default marked `*`, then exit. |

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
* MacBook Pro Speakers (2 output channels)
  USB Audio Interface (18 output channels)
$ play-cli songs/example --device "USB Audio Interface" --buffer 128
"Example Song" (arrangement "default", 6 positions) on "USB Audio Interface": 48000 Hz, 128 frames per buffer, 2.7 ms latency
space: play/pause, 1-9: queue a section, q: quit; play-cli --help lists every key
PLAYING  | bar 12.3 | Chorus (3/6) | next: jump to Bridge in 4.2 s | timing: section end | fade: in | 48000 Hz, 128 frames, 2.7 ms on "USB Audio Interface"
```

The status line holds, separated by `|`: the transport (`STOPPED`, `LOADING`, `PLAYING`, `PAUSED`, `STOPPING`, `PANIC`), bar and beat, the section and its position in the arrangement, the queued jump or loop with the time until it executes (and how many boundaries it has waited when its audio was not loaded), the jump timing, `infinite` when the infinite loop is on, the fade, and the device. Underruns and the last problem, prefixed `!`, follow when there are any. The line is cut to the terminal width.

## Device lost

Unplugging the device pauses playback; the status line says `NO DEVICE` with the reason, and play is refused until the device is back. Other keys still work. When the same device is plugged back in, play-cli opens it again within half a second, with the transport still paused: press space to go on.

To check it by hand: start a song on a USB interface or headphones, unplug them while it plays, wait, plug them back in, and press space.

## Exit codes

| Code | When | Output |
|------|------|--------|
| 0 | Quit with `q` or Ctrl-C, or `--list-devices` listed the devices. | The device list on stdout for `--list-devices`. |
| 1 | The song folder cannot be loaded or validated, the device cannot be opened (not found, not at the song's sample rate, buffer size refused), standard input is not a terminal, or the terminal fails. | Every error on its own stderr line, prefixed `error:`; package warnings prefixed `warning:`. |
| 2 | Usage error, such as a missing `<SONG_DIR>` or `--buffer` outside 64–1024. | clap's message on stderr. |
