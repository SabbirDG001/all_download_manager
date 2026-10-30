# All Download Manager

A light, fast download manager for Windows, in the spirit of Internet Download
Manager, with a browser extension for Chrome and Brave.

## Status

| Step | Part | State |
|---|---|---|
| 1 | Download engine + command-line tool (`crates/engine`) | done |
| 2 | Desktop app (`crates/app`, Tauri) with download list, queue, settings, installer | done |
| 3 | Chrome / Brave / Edge extension (`extension/`) + Native Messaging bridge | done |
| 4 | Video and audio: yt-dlp + ffmpeg, media detection in the browser | done |
| 5 | Scheduler, categories, speed limit | done |

## Getting the Windows app

Every push builds the app on GitHub (see `.github/workflows/build.yml`).
Open the repository's **Actions** tab, pick the latest **Build Windows app**
run, and download one of:

- **AllDownloadManager-setup**: the installer. It asks whether to install for
  you or for everyone, and lets you choose the install folder.
- **AllDownloadManager-portable**: no install. Unzip anywhere (another drive,
  a USB stick) and run `AllDownloadManager.exe`. Settings and the download
  list stay in that folder.
- **AllDownloadManager-extension**: the browser extension on its own (it is
  also included in the installer and the portable folder).

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

## Speed limit, categories and scheduler

- **Speed limit:** the **No limit / Limit** button in the toolbar turns it on
  and off; the speed (KB/s) is set in Settings. It is shared by all
  downloads and changes running downloads straight away (video downloads
  pick up a new limit when they next start).
- **Categories:** the sidebar groups downloads by type: Video, Music,
  Documents, Compressed, Programs, Images and Other. Turn on **Sort finished
  downloads into folders by type** in Settings to save them in matching
  subfolders of the download folder (`Downloads\Video`, `Downloads\Music`…).
  Downloads saved to a folder you picked yourself are left where you put them.
- **Scheduler:** add downloads to the scheduler from the Add dialog
  ("When: Add to the scheduler") or by right-clicking them. In
  **Scheduler**, choose the start time, an optional stop time (windows past
  midnight work, such as 23:00–07:00) and the days. At the start time the
  scheduled downloads begin; at the stop time they pause until the next
  window. **Start now** runs them at once. When they are all done, the
  computer can **sleep** or **shut down**, after a 60-second warning you can
  cancel.

## Video and audio

The app downloads from YouTube and
[over a thousand other sites](https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md),
and from HLS/DASH streams, using two free helper programs:

- **yt-dlp** gets the video out of the site. Required for video downloads.
- **ffmpeg** joins separate video and audio (needed for 1080p and up on
  YouTube) and makes MP3s. Without it you get the best single file the site offers.

Install both from **Settings → Video and audio** (Install / Update buttons).
They go into a `tools` folder in the app's data folder (in portable mode,
next to the program), and copies already on your `PATH` are used too. Keep
yt-dlp updated: sites change often and new versions follow them.

To download a video:

- Paste the page's link into **Add URL**. Links from known video sites (and
  `.m3u8`/`.mpd` streams) are ticked as **Video or audio page** automatically.
- Pick the quality from the list, like in IDM. The app asks the site which
  versions it has and shows each with its estimated size:
  - **Video:** every resolution offered, such as 2160p (4K), 1440p,
    1080p60, 720p … 144p.
  - **Audio only:** MP3 at 320, 192 or 128 kbps, or the site's original
    M4A or Opus track at its real bitrate.

  The choice preselected follows the default quality in Settings (Best, up
  to 1080p/720p/480p, or audio only), so for most downloads you just click
  **Download**. A small tag in the list shows what was chosen.
- Or, in the browser, use the **Download** button on the video (see below).

Videos are named after their title. Pause and resume work too (yt-dlp keeps
its partial files). DRM-protected services (Netflix, Spotify, Disney+ and
similar) can't be downloaded.

## Browser extension (Chrome, Brave, Edge)

### Install

1. Install the app and **open it once**. On every start it registers itself
   with Chrome, Brave and Edge so the extension can reach it.
2. In the browser, open `chrome://extensions` (Brave: `brave://extensions`,
   Edge: `edge://extensions`) and turn on **Developer mode**.
3. Click **Load unpacked** and choose the `extension` folder. To find it, open
   the app's **Settings** and click **Show extension folder**. (It is in the
   install folder, or next to `AllDownloadManager.exe` in the portable version.)
4. Pin the extension. Its popup shows whether it can reach the app.

### What it does

- **Catches downloads:** when you download a file, the browser's download is
  stopped and the app takes over. Turn this off in the extension's popup.
- **Right-click menu:** "Download link with All Download Manager" on links,
  images, video and audio.
- **Download button on videos:** point at a video and a **Download** button
  appears in its corner. On video sites the page goes to yt-dlp; elsewhere the
  stream or file the page is playing is used. Can be turned off in the popup.
- **Finds media:** video, audio and HLS/DASH streams that pages load are
  counted on the extension icon and listed in its popup, each with a
  download button.
- **Keeps you logged in:** the browser's cookies, referrer and user agent are
  sent with the link, so downloads that need a login still work.
- **Asks first:** the app comes to the front with the download dialog, so you
  can change the folder or name. Turn off "Ask before downloading links from
  the browser" in Settings to start them straight away.
- **Starts the app** if it isn't running.
- **Never loses a download:** if the app can't be reached, the browser carries
  on downloading as usual and the extension icon shows a "!".

Downloads in incognito windows, and `blob:`/`data:` downloads made by web
pages themselves, stay in the browser.

### How it connects

The browser starts `AllDownloadManager.exe` as a
[Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging)
host. That short-lived process passes the link to the running app over a
local connection (127.0.0.1, protected by a random token stored in the data
folder), starting the app first if needed. The extension's ID is fixed by
the `key` in `extension/manifest.json`, so the registration works wherever
the folder is unpacked.

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
and the download list (queue limit, pause/resume, delete, surviving a restart,
speed limit, sorting into folders, and the scheduler's time windows).
If yt-dlp and ffmpeg are on `PATH`, a test also downloads a local HLS stream
and extracts MP3 audio with them. `cargo test -- --ignored` additionally
installs yt-dlp from GitHub.
