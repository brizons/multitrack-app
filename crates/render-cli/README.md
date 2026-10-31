# render-cli

Headless renderer for tests and CI. It renders one arrangement of a song folder offline to a stereo 32-bit float WAV file, with no audio hardware.

```text
render-cli <SONG_DIR> --output <FILE> [--arrangement <ID>] [--no-click]
```

| Flag | Meaning |
|------|---------|
| `<SONG_DIR>` | Song folder: `song.json` plus the WAV stems it lists (16-, 24-, or 32-bit integer or 32-bit float, mono or stereo). |
| `--output <FILE>` | WAV file to write, at the song sample rate. |
| `--arrangement <ID>` | Arrangement to render. Default: `default`. |
| `--no-click` | Do not generate the click. A recorded `click` stem still plays. |

## Example

```console
$ render-cli songs/example --output example.wav --arrangement live
rendered "Example Song" (arrangement "live"): 5 segments, 3:12, 230 clicks -> example.wav
```

The summary is the only line on stdout. The click count is the generated ticks that are audible: none with `--no-click` or when the song has a `click` stem.

## Exit codes

| Code | When | Output |
|------|------|--------|
| 0 | The file was written. | The summary line on stdout; warnings on stderr, each prefixed `warning:`. |
| 1 | The song folder cannot be loaded or validated, the arrangement does not exist, or rendering or writing fails. | Every error on its own stderr line, prefixed `error:`. Nothing is written. |
| 2 | Usage error, such as a missing `--output`. | clap's message on stderr. |

Rendering is deterministic: the same song and flags always write a byte-identical file.
