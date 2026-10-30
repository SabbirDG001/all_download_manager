use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Quality, Result, Schedule};

const FILE_NAME: &str = "settings.json";

/// User settings, stored as JSON.
///
/// The settings file and the download list live in `%APPDATA%\AllDownloadManager`,
/// unless a `settings.json` sits next to the program itself. That is
/// "portable mode": everything stays in the folder you installed or unzipped
/// the app to, which suits a USB stick or a non-system drive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where finished downloads are saved.
    pub download_dir: PathBuf,
    /// Where partial downloads are kept while downloading. Defaults to the
    /// download folder. Pick a folder on the same drive for instant moves.
    pub temp_dir: Option<PathBuf>,
    /// Parallel connections per download.
    pub connections: usize,
    /// Downloads running at the same time; the rest wait in the queue.
    pub max_active: usize,
    /// User-Agent header to send; the app's own name when unset.
    pub user_agent: Option<String>,
    /// Show the download dialog for links sent by the browser extension,
    /// instead of starting them straight away.
    pub confirm_browser_downloads: bool,
    /// Default quality for video and audio downloads.
    pub video_quality: Quality,
    /// Speed limit in KB/s, used while `speed_limit_on` is set.
    pub speed_limit_kbps: u64,
    pub speed_limit_on: bool,
    /// Save finished downloads in subfolders by type (Video, Music…).
    pub sort_into_folders: bool,
    pub schedule: Schedule,
    /// Closing the window keeps the app running in the system tray.
    pub close_to_tray: bool,
    /// Start (in the tray) when signing in to Windows.
    pub start_with_windows: bool,
    /// A Windows notification when downloads finish or fail.
    pub notify_on_finish: bool,
    /// IDM-style "Download complete" window with Open / Open folder buttons.
    pub show_complete_window: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let download_dir = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            download_dir,
            temp_dir: None,
            connections: 8,
            max_active: 3,
            user_agent: None,
            confirm_browser_downloads: true,
            video_quality: Quality::Best,
            speed_limit_kbps: 1024,
            speed_limit_on: false,
            sort_into_folders: false,
            schedule: Schedule::default(),
            close_to_tray: true,
            start_with_windows: false,
            notify_on_finish: true,
            show_complete_window: false,
        }
    }
}

impl Settings {
    /// Folder for the settings file and the download list.
    pub fn data_dir() -> PathBuf {
        if let Some(dir) = portable_dir() {
            return dir;
        }
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("AllDownloadManager")
    }

    /// Whether settings are kept next to the program (portable mode).
    pub fn is_portable() -> bool {
        portable_dir().is_some()
    }

    /// The speed limit in bytes per second, or 0 when it's off.
    pub fn speed_limit_bytes(&self) -> u64 {
        if self.speed_limit_on {
            self.speed_limit_kbps.saturating_mul(1024)
        } else {
            0
        }
    }

    /// Folder for yt-dlp and ffmpeg.
    pub fn tools_dir() -> PathBuf {
        Self::data_dir().join("tools")
    }

    /// Where the settings file is read from and written to.
    pub fn path() -> PathBuf {
        Self::data_dir().join(FILE_NAME)
    }

    /// Loads the settings, falling back to defaults when there is no file yet.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data)
                .map_err(|e| Error::Settings(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.connections) {
            return Err(Error::Settings("connections must be between 1 and 32".into()));
        }
        if !(1..=10).contains(&self.max_active) {
            return Err(Error::Settings("downloads at once must be between 1 and 10".into()));
        }
        if self.download_dir.as_os_str().is_empty() {
            return Err(Error::Settings("choose a download folder".into()));
        }
        if self.speed_limit_kbps == 0 {
            return Err(Error::Settings("the speed limit must be at least 1 KB/s".into()));
        }
        self.schedule.validate().map_err(Error::Settings)?;
        Ok(())
    }
}

fn portable_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join(FILE_NAME).exists().then_some(dir)
}
