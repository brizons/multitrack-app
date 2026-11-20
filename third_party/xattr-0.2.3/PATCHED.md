# xattr 0.2.3, patched for OpenVamp

This is the crates.io release of `xattr` 0.2.3 (MIT OR Apache-2.0, see `LICENSE-MIT` and `LICENSE-APACHE`) with two changes: `src/sys/mod.rs` reads `libc::ENODATA` instead of `libc::ENOATTR` on Linux and Android, and `src/lib.rs` allows the deprecated `libc::uint32_t` alias it uses on macOS, so builds stay free of warnings.

Why: GPUI 0.2.2 pulls in `xattr` 0.2.3 through `gpui_http_client` and `zed-async-tar` (default feature `xattr`). `libc` 0.2.190 and later no longer define `ENOATTR` on Linux, where it was always an alias of `ENODATA`, so `xattr` 0.2.3 does not compile there. `libc` cannot be held back, because `mio` 1.2.5 (via `crossterm`) requires `libc` 0.2.190 or later.

The root `Cargo.toml` applies it with `[patch.crates-io]` and excludes this folder from the workspace. Remove the patch once GPUI no longer depends on `xattr` 0.2.
