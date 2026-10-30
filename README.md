# All Download Manager

A light, fast download manager for Windows, in the spirit of Internet Download
Manager, with a browser extension for Chrome and Brave.

## Status

| Step | Part | State |
|---|---|---|
| 1 | Download engine + command-line tool (`crates/engine`) | done |
| 2 | Desktop app (Tauri) with download list, queue, settings, installer | next |
| 3 | Chrome / Brave extension + Native Messaging bridge | planned |
| 4 | Video and audio detection (yt-dlp + ffmpeg) | planned |
| 5 | Scheduler, categories, speed limit | planned |

## The engine

- **Parallel connections:** splits a file into segments and downloads them at
  the same time using HTTP `Range` requests (8 by default, up to 32).
- **Dynamic splitting:** when a connection finishes its segment, it takes over
  the second half of the biggest segment still running, so no connection sits idle.
- **Resume:** progress is saved to `<name>.admpart.json` next to the partial
  file `<name>.admpart`. Run the same download again (after Ctrl+C, a crash or
  a lost connection) and it continues where it stopped. If the file changed on
  the server (different size or ETag), it starts over.
- **Retries:** network errors are retried with backoff. A failed connection's
  segment is picked up by the others.
- **Fallback:** servers without range support get one plain connection.
- **Safe file names:** characters Windows rejects are replaced, and an
  existing file is never overwritten (`file (1).zip` is used instead).

## Storage and install location

- **Download folder:** set once with `adm config set download-dir D:\Downloads`,
  or per download with `-d`.
- **Temp folder:** partial files can live elsewhere, e.g. on a faster drive:
  `adm config set temp-dir E:\Temp`. Use the same drive as the download folder
  for an instant move when finished.
- **Settings file:** `%APPDATA%\AllDownloadManager\settings.json`.
- **Portable mode:** put an empty-object `settings.json` (`{}`) next to
  `adm.exe` and all settings are kept in that folder instead, so the app can
  live anywhere, such as a USB stick or `D:\Apps`.
- **Install location:** the Windows installer (step 2) will let you choose the
  install folder.

## Build and use

You need [Rust](https://rustup.rs). On Windows, use the MSVC toolchain.

```
cargo build --release
```

The program is `target\release\adm.exe`.

```
adm get https://example.com/big.zip
adm get https://example.com/big.zip -d D:\Downloads -c 16
adm get https://example.com/file -o name.zip -H "Cookie: session=abc" -H "Referer: https://example.com/"
adm config show
adm config set connections 16
```

Press Ctrl+C to pause. Run the same command again to resume.

## Tests

```
cargo test
```

The tests run against a small local HTTP server and check segmented downloads,
the single-connection fallback, not overwriting files, and resuming after a pause.
