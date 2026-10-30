//! Finding, installing and updating the helper programs (yt-dlp and ffmpeg).
//!
//! They are installed into the app's own `tools` folder, downloaded with the
//! engine itself. Copies on the system `PATH` are used when there are none.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::download::{download, DownloadOptions, Progress};
use crate::media::MediaTools;
use crate::{Error, Result};

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub path: PathBuf,
    pub version: Option<String>,
    /// Installed by the app (and so updatable by it), rather than found on PATH.
    pub managed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsStatus {
    pub yt_dlp: Option<ToolInfo>,
    pub ffmpeg: Option<ToolInfo>,
    /// Whether the app can install ffmpeg itself on this system.
    pub can_install_ffmpeg: bool,
}

/// Locates the helper programs.
pub fn find_tools(tools_dir: &Path) -> MediaTools {
    MediaTools { yt_dlp: find(tools_dir, "yt-dlp").map(|(p, _)| p), ffmpeg: find(tools_dir, "ffmpeg").map(|(p, _)| p) }
}

fn find(tools_dir: &Path, name: &str) -> Option<(PathBuf, bool)> {
    let own = tools_dir.join(format!("{name}{EXE}"));
    if own.is_file() {
        return Some((own, true));
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(format!("{name}{EXE}")))
        .find(|p| p.is_file())
        .map(|p| (p, false))
}

pub async fn tools_status(tools_dir: &Path) -> ToolsStatus {
    let info = |name: &'static str, version_arg: &'static str| async move {
        let (path, managed) = find(tools_dir, name)?;
        let version = version_of(&path, version_arg).await;
        Some(ToolInfo { path, version, managed })
    };
    ToolsStatus {
        yt_dlp: info("yt-dlp", "--version").await,
        ffmpeg: info("ffmpeg", "-version").await,
        can_install_ffmpeg: cfg!(windows),
    }
}

async fn version_of(path: &Path, arg: &str) -> Option<String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.arg(arg).stdin(Stdio::null()).stderr(Stdio::null());
    hide_console(&mut cmd);
    let out = tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?.trim();
    // "ffmpeg version 7.1-full_build-www.gyan.dev Copyright…" -> "7.1-full_build-www.gyan.dev"
    let version = first.strip_prefix("ffmpeg version ").map_or(first, |v| v.split(' ').next().unwrap_or(v));
    Some(version.to_string()).filter(|v| !v.is_empty())
}

fn yt_dlp_url() -> &'static str {
    if cfg!(windows) {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos"
    } else {
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"
    }
}

const FFMPEG_WINDOWS_URL: &str =
    "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";

/// Downloads the latest yt-dlp into `tools_dir` (also used to update it).
pub async fn install_yt_dlp<F>(tools_dir: &Path, on_progress: F, cancel: CancellationToken) -> Result<PathBuf>
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    let target = tools_dir.join(format!("yt-dlp{EXE}"));
    let fetched = fetch(tools_dir, yt_dlp_url(), "yt-dlp.download", on_progress, cancel).await?;
    replace(&fetched, &target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(target)
}

/// Downloads ffmpeg and ffprobe into `tools_dir`. Windows only; elsewhere
/// ffmpeg comes from the system's package manager.
pub async fn install_ffmpeg<F>(tools_dir: &Path, on_progress: F, cancel: CancellationToken) -> Result<PathBuf>
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    if !cfg!(windows) {
        return Err(Error::Tool("install ffmpeg with your system's package manager".into()));
    }
    let zip_path = fetch(tools_dir, FFMPEG_WINDOWS_URL, "ffmpeg.zip", on_progress, cancel).await?;
    let dir = tools_dir.to_path_buf();
    let result = tokio::task::spawn_blocking(move || extract_ffmpeg(&zip_path, &dir).map(|p| (p, zip_path)))
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    let (ffmpeg, zip_path) = result?;
    let _ = std::fs::remove_file(zip_path);
    Ok(ffmpeg)
}

/// Takes `bin/ffmpeg.exe` and `bin/ffprobe.exe` out of the zip.
fn extract_ffmpeg(zip_path: &Path, tools_dir: &Path) -> Result<PathBuf> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| Error::Tool(format!("bad ffmpeg download: {e}")))?;
    let mut found = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| Error::Tool(e.to_string()))?;
        let name = entry.name().replace('\\', "/");
        let Some(file_name) = name.rsplit('/').next() else { continue };
        if name.contains("/bin/") && (file_name == "ffmpeg.exe" || file_name == "ffprobe.exe") {
            let tmp = tools_dir.join(format!("{file_name}.new"));
            let mut out = std::fs::File::create(&tmp)?;
            std::io::copy(&mut entry, &mut out)?;
            drop(out);
            let target = tools_dir.join(file_name);
            replace(&tmp, &target)?;
            if file_name == "ffmpeg.exe" {
                found = Some(target);
            }
        }
    }
    found.ok_or_else(|| Error::Tool("ffmpeg.exe wasn't in the download".into()))
}

async fn fetch<F>(tools_dir: &Path, url: &str, name: &str, on_progress: F, cancel: CancellationToken) -> Result<PathBuf>
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    tokio::fs::create_dir_all(tools_dir).await?;
    let target = tools_dir.join(name);
    let _ = tokio::fs::remove_file(&target).await;
    let opts = DownloadOptions {
        dir: tools_dir.to_path_buf(),
        filename: Some(name.to_string()),
        connections: 4,
        ..Default::default()
    };
    download(url, &opts, on_progress, cancel).await
}

/// Moves `from` over `to`, even if `to` exists.
fn replace(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    std::fs::rename(from, to)?;
    Ok(())
}

/// Stops a console window from popping up for helper programs on Windows.
pub(crate) fn hide_console(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Stops a helper and the programs it started (yt-dlp runs ffmpeg).
pub(crate) async fn kill_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut cmd = tokio::process::Command::new("taskkill");
        cmd.args(["/T", "/F", "/PID", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null());
        hide_console(&mut cmd);
        let _ = cmd.status().await;
    }
    let _ = child.kill().await;
}
