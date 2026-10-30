# All Download Manager

A light, fast download manager for Windows, in the spirit of Internet Download
Manager, with a browser extension for Chrome and Brave.

## Status

| Step | Part | State |
|---|---|---|
| 1 | Download engine + command-line tool (`crates/engine`) | done |
| 2 | Desktop app (`crates/app`, Tauri) with download list, queue, settings, installer | done |
| 3 | Chrome / Brave extension + Native Messaging bridge | next |
| 4 | Video and audio detection (yt-dlp + ffmpeg) | planned |
| 5 | Scheduler, categories, speed limit | planned |

## Getting the Windows app

Every push builds the app on GitHub (see `.github/workflows/build.yml`).
Open the repository's **Actions** tab, pick the latest **Build Windows app**
run, and download one of:

- **AllDownloadManager-setup**: the installer. It asks whether to install for
  you or for everyone, and lets you choose the install folder.
- **AllDownloadManager-portable**: no install. Unzip anywhere (another drive,
  a USB stick) and run `AllDownloadManager.exe`. Settings and the download
  list stay in that folder.

The app uses Microsoft Edge WebView2, which Windows 10 and 11 already have.

## The desktop app

- Download list with progress, speed, time left and status, sorted newest first.
- Filters: All, Downloading, Unfinished, Finished.
- Queue: a set number of downloads run at once (3 by default), the rest wait.
- Pause, resume, delete (optionally with the file), pause all, resume all,
  clear finished. Paused downloads survive closing the app.
- Add links with **Add URL**, by pressing Ctrl+V anywhere in the window, or by
  dragging a link from the browser. Several links at once (one per line) work too.
- Double-click a finished download to open it; right-click for more options.
- Settings: download folder, temporary folder, connections per download,
  downloads at the same time, user agent.
- Follows the Windows light or dark theme.
- Keyboard: Ctrl+N add, Delete delete, Ctrl+A select all, Enter open, Esc deselect.

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
- **Portable mode:** put an empty-object `settings.json` (`{}`) next to the program
  (`AllDownloadManager.exe` or `adm.exe`) and settings and the download list are kept in that folder instead, so the app can
  live anywhere, such as a USB stick or `D:\Apps`.
- **Install location:** the installer lets you choose the install folder, or
  use the portable version and put it wherever you like.

## Building it yourself

You need [Rust](https://rustup.rs) (on Windows, the MSVC toolchain) and
[Node.js](https://nodejs.org) (only for the Tauri build tool).

```
cd crates/app
npx @tauri-apps/cli@2 build
```

The installer ends up in `target\release\bundle\nsis\`. To run the app
while working on it: `npx @tauri-apps/cli@2 dev`.

## Command-line tool

`adm` uses the same engine and settings as the app.

```
cargo build --release -p adm-engine
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
the single-connection fallback, not overwriting files, resuming after a pause,
and the download list (queue limit, pause/resume, delete, surviving a restart).
