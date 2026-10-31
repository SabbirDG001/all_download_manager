<div align="center">

<img src="crates/app/icons/128x128.png" width="96" alt="">

# All Download Manager

**A free, light and fast download manager for Windows, in the spirit of Internet Download Manager (IDM).**

Faster downloads with multiple connections · pause and resume · YouTube and 1,000+ video sites with quality choice · whole playlists, FTP and server folders · Chrome, Brave and Edge integration

[**⬇ Download for Windows**](#download) · [Setup guide](#setup-guide) · [Features](#features) · [FAQ](#faq--troubleshooting)

[![Build](https://github.com/SabbirDG001/all_download_manager/actions/workflows/build.yml/badge.svg)](https://github.com/SabbirDG001/all_download_manager/actions/workflows/build.yml)
[![Latest release](https://img.shields.io/github/v/release/SabbirDG001/all_download_manager?label=latest)](https://github.com/SabbirDG001/all_download_manager/releases/latest)

<img src="docs/screenshots/main-dark.png" alt="All Download Manager main window" width="900">

</div>

---

## Download

| File | What it is |
|---|---|
| [**AllDownloadManager-Setup.exe**](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/AllDownloadManager-Setup.exe) | **Installer (recommended).** Choose "only for me" or "everyone", and the install folder. |
| [AllDownloadManager-Portable.zip](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/AllDownloadManager-Portable.zip) | **No installation.** Unzip anywhere (another drive, a USB stick) and run it. |
| [AllDownloadManager-Extension.zip](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/AllDownloadManager-Extension.zip) | The browser extension on its own (it's also inside both downloads above). |
| [SHA256SUMS.txt](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/SHA256SUMS.txt) | Checksums, to check your download is genuine (optional). |

All versions are on the [Releases page](https://github.com/SabbirDG001/all_download_manager/releases).
Works on **Windows 10 and 11** (64-bit). Free, with no ads, no account and no tracking.

---

## Features

### ⚡ Fast, reliable downloads
- **Multiple connections per file** (8 by default, up to 32) for much higher speed, like IDM.
- **Smart splitting:** when one connection finishes, it helps the slowest one, so none sit idle.
- **Pause and resume** any time, even after closing the app, a crash or a lost connection.
- **Automatic retries** when the network hiccups.
- **Queue:** a set number of downloads run at once; the rest wait their turn.

### 🌐 Browser integration (Chrome, Brave, Edge)
- **Catches your downloads** automatically and hands them to the app.
- **Download button over videos** while you watch, with a menu of all the qualities right there.
- **Right-click → "Download with All Download Manager"** on any link, image, video or audio.
- **Finds video and audio** playing on a page (including HLS/DASH streams) and lists them in the extension's popup.
- **Keeps you logged in:** downloads that need a login work, because the browser's cookies come along.
- **Never loses a download:** if the app isn't available, the browser simply downloads as usual.

### 🎬 YouTube and 1,000+ video sites
- **Choose the exact quality**, like IDM: 4K, 1440p, 1080p60, 720p … 144p, each with its size.
- **Audio only:** MP3 at 320, 192 or 128 kbps, or the original M4A/Opus.
- Works with YouTube, Facebook, Instagram, TikTok, X/Twitter, Vimeo, Dailymotion, Twitch, SoundCloud and [many more](https://github.com/yt-dlp/yt-dlp/blob/master/supportedsites.md).

<p align="center">
<img src="docs/screenshots/page-menu.png" alt="Quality menu over a video in the browser" width="49%">
<img src="docs/screenshots/quality-picker.png" alt="Quality picker in the app" width="49%">
</p>

### 📚 Whole playlists, channels and folders
- **YouTube playlists and channels:** tick the videos you want and pick one quality for all.
- **FTP servers** (`ftp://…`), with all subfolders, and a login if needed.
- **Server folders** ("Index of /" pages and h5ai, as used by many media and BDIX servers), with all subfolders.
- Everything lands in a folder named after the playlist or folder, keeping its structure.

<p align="center"><img src="docs/screenshots/playlist.png" alt="Downloading a whole playlist" width="80%"></p>

### 🪟 IDM-style windows
- **Download File Info** window for each link from the browser: folder, file name or quality, size, **Start Download** or **Download Later**.
- **Progress window** per download: speed, time left, resume capability and each connection's progress.
- **Download complete**, with **Open** and **Open folder**.

<p align="center"><img src="docs/screenshots/progress.png" alt="Progress window with connection details" width="460"></p>

### 🗓 Scheduler, speed limit and more
- **Scheduler:** download at night (start and stop times, days of the week), then **sleep or shut down** the PC when done.
- **Speed limit**, switched on and off from the toolbar or the tray icon.
- **Categories:** Video, Music, Documents, Compressed, Programs, Images, optionally saved into matching folders.
- **System tray:** keeps downloading in the background; **start with Windows** if you like.
- **Notifications** when downloads finish or fail.
- **Light and dark theme**, following Windows.
- **Your choice of folders:** download folder, temporary folder and install folder, or run it portable.

<p align="center"><img src="docs/screenshots/scheduler.png" alt="Scheduler" width="80%"></p>

---

## Setup guide

### 1. Install the app
1. Download **[AllDownloadManager-Setup.exe](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/AllDownloadManager-Setup.exe)** and run it.
2. Windows may say **"Windows protected your PC"**. This is because the app isn't code-signed (that needs a paid certificate). Click **More info → Run anyway**.
3. Choose **Only for me** (no admin rights needed) or **Anyone who uses this computer**, then the **install folder** (any drive is fine).
4. Finish, and start **All Download Manager** from the Start menu.

> **Portable instead?** Download **[AllDownloadManager-Portable.zip](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/AllDownloadManager-Portable.zip)**, unzip it where you want (for example `D:\Apps\ADM`), and run `AllDownloadManager.exe`. Keep the `settings.json` next to it: it keeps your settings and download list in that folder.

### 2. First start
**Open the app once.** This also connects it to your browsers. Then click **Settings** (top right):

1. **Download folder:** where your files go.
2. Under **Video and audio**, click **Install** next to each helper. They're needed for video sites:

   | Helper | Needed for | Size |
   |---|---|---|
   | **yt-dlp** | YouTube and other video sites | ~18 MB |
   | **ffmpeg** | 1080p and higher, and MP3 | ~130 MB |
   | **Deno** | YouTube (yt-dlp needs it) | ~40 MB |

   If a row says *"already on your computer, ready to use"*, nothing is needed for it.
3. Choose any other options you like (tray, start with Windows, notifications…) and click **Save**.

### 3. Add the browser extension
The extension isn't in the Chrome Web Store, so it's added once by hand. It takes a minute:

1. In the app, open **Settings → Show extension folder**. That folder is the extension.
   *(It's also in the install folder, inside the portable folder, and in [AllDownloadManager-Extension.zip](https://github.com/SabbirDG001/all_download_manager/releases/latest/download/AllDownloadManager-Extension.zip).)*
2. Open your browser's extensions page:

   | Chrome | Brave | Edge |
   |---|---|---|
   | `chrome://extensions` | `brave://extensions` | `edge://extensions` |

3. Turn on **Developer mode** (top right; in Edge, on the left).
4. Click **Load unpacked** and choose the **extension** folder from step 1.
5. Click the puzzle-piece icon in the toolbar and **pin** All Download Manager.

Don't move or delete that folder afterwards: the browser keeps using it.

### 4. Check it works
- Click the extension's icon: it should say **"Connected to the app"** (or *"App installed; it starts when needed"*).
- Download any file in the browser: the **Download File Info** window should appear.
- On a YouTube video, point at the video and click **Download**: the list of qualities should appear.

---

## How to use

| To… | Do this |
|---|---|
| Download a file | Just download it in the browser. Or click **Add URL** (or press **Ctrl+V** in the app) and paste the link. |
| Download a video | Point at the video in the browser → **Download** → pick a quality. Or paste the link into **Add URL**. |
| Get only the audio | Pick **MP3** or **M4A** in the quality list. |
| Download a playlist, channel or folder | **Playlist / Folder** in the app, or **Whole playlist** in the extension's popup on YouTube. |
| Download from an FTP server | **Playlist / Folder** → `ftp://server/folder/` (or `ftp://user:password@server/folder/`). |
| Download later or at night | **Download Later**, or right-click → **Add to the scheduler**, then set the time in **Scheduler**. |
| Limit the speed | Set it in **Settings**, then switch it on and off with the **Limit** button or the tray icon. |
| See why something failed | The reason is shown in red under its name; right-click → **Copy error**. |

Keyboard: **Ctrl+N** add, **Ctrl+V** paste a link, **Delete** delete, **Ctrl+A** select all, **Enter** open, **Esc** deselect.

---

## FAQ / troubleshooting

<details>
<summary><b>Windows says "Windows protected your PC"</b></summary>

The app isn't code-signed, so SmartScreen warns the first time. Click **More info → Run anyway**. Only download it from this page, and compare it with `SHA256SUMS.txt` if you want to be sure (in PowerShell: `Get-FileHash .\AllDownloadManager-Setup.exe`).
</details>

<details>
<summary><b>The extension says "App not found"</b></summary>

Open the app once (it connects itself to the browsers when it starts), then reopen the browser. With the portable version, open it again after moving its folder.
</details>

<details>
<summary><b>YouTube videos fail, or the qualities never appear</b></summary>

Install **yt-dlp**, **ffmpeg** and **Deno** in **Settings → Video and audio**. If it worked before and then stopped, click **Update** next to yt-dlp: sites change often. The first video after starting the app can take a little longer.
</details>

<details>
<summary><b>Downloads aren't caught from the browser</b></summary>

Check that **Catch browser downloads** is on in the extension's popup. Downloads in incognito windows always stay in the browser.
</details>

<details>
<summary><b>Where did the app go when I closed it?</b></summary>

It keeps downloading in the **system tray** (bottom right, maybe under the **^** arrow). Right-click the icon → **Quit** to exit. You can turn this off in Settings.
</details>

<details>
<summary><b>Can it download from Netflix, Spotify or Disney+?</b></summary>

No. Those services use DRM copy protection, which no download manager (IDM included) can get around.
</details>

<details>
<summary><b>How do I update?</b></summary>

Quit the app from the tray, then download the newest installer and run it over the old version. Your settings and download list are kept. For the extension, replace its folder with the new one and click **↻ reload** on its card in the extensions page.
</details>

<details>
<summary><b>How do I uninstall?</b></summary>

Windows **Settings → Apps → All Download Manager → Uninstall** (portable: just delete its folder). Remove the extension from the browser's extensions page. Your settings and download list stay in `%APPDATA%\AllDownloadManager`; delete that folder to remove them too.
</details>

---

## Security and privacy

- **Nothing leaves your computer** except the downloads themselves: no account, no telemetry, no servers of its own.
- **Browser cookies** are used only so files behind a login still download. Only the cookies the browser itself would send are used, and they're removed from the saved download list as soon as a download finishes.
- **The browser connection** only accepts this project's extension, and the app only listens on your own computer (`127.0.0.1`) behind a random password.
- **Helpers** (yt-dlp, ffmpeg, Deno) come from their official GitHub releases and are checked against their published SHA-256 checksums before they're installed.
- **Downloaded files** get safe names, are never overwritten, and are never opened or run by themselves.
- **Sharing tip:** share this page or the files from the Releases page, not your own portable folder (it contains your settings and download list).

---

## Credits

All Download Manager is built with [Rust](https://www.rust-lang.org) and [Tauri](https://tauri.app). Video downloads use [yt-dlp](https://github.com/yt-dlp/yt-dlp), [FFmpeg](https://ffmpeg.org) and [Deno](https://deno.com), which the app downloads from their official releases when you ask for them.

---

## For developers

<details>
<summary><b>Project layout, building, tests and releases</b></summary>

### Layout
| Path | What it is |
|---|---|
| `crates/engine` | Download engine (library and `adm` command-line tool): segmented HTTP downloads, resume, FTP, yt-dlp, playlists and folders, scheduler, speed limit. |
| `crates/app` | The desktop app (Tauri 2): Rust backend in `src/`, plain HTML/CSS/JS interface in `ui/`. |
| `extension` | The Chrome/Brave/Edge extension (Manifest V3). |
| `docs/screenshots` | Pictures used in this README. |
| `.github/workflows` | `build.yml` builds and tests every push; `release.yml` publishes a release for each `v*` tag. |

### Build
You need [Rust](https://rustup.rs) (on Windows, the MSVC toolchain) and [Node.js](https://nodejs.org) (only for the Tauri CLI).

```bash
cd crates/app
npx @tauri-apps/cli@2 build
```

The installer ends up in `target\release\bundle\nsis\`. While working on the app: `npx @tauri-apps/cli@2 dev`.

The engine also has a command-line tool, `adm`, which uses the same settings:

```bash
cargo run --release -p adm-engine -- get https://example.com/big.zip -c 16
```

### Tests

```bash
cargo test
```

They run against local test servers: segmented downloads, resume, the queue, speed limit, scheduler and web folders, plus a real FTP server (if Python's `pyftpdlib` is installed) and yt-dlp with ffmpeg (if they're on `PATH`). `cargo test -- --ignored` also installs yt-dlp from GitHub.

### Publishing a release
Update the version in `Cargo.toml` and `crates/app/tauri.conf.json`, commit, then push a tag:

```bash
git tag v0.1.0
```

```bash
git push origin v0.1.0
```

Or, without a tag: **Actions → Release → Run workflow**, and enter the version (such as `v0.1.0`).

The **Release** workflow builds the app and attaches `AllDownloadManager-Setup.exe`, `AllDownloadManager-Portable.zip`, `AllDownloadManager-Extension.zip` and `SHA256SUMS.txt` to a GitHub Release. The download links in this README always point at the latest one.

### How the browser connects
The extension talks to the app through Chrome's [Native Messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging). The browser starts `AllDownloadManager.exe` as a short-lived host, which passes the request to the running app over `127.0.0.1` (protected by a random token in the data folder), starting the app first if needed. The extension's ID is fixed by the `key` in `extension/manifest.json`, so this works wherever the folder is unpacked.
</details>
