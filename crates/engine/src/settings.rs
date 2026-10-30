use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

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
}

impl Default for Settings {
    fn default() -> Self {
        let download_dir = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self { download_dir, temp_dir: None, connections: 8, max_active: 3, user_agent: None }
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
        Ok(())
    }
}

fn portable_dir() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join(FILE_NAME).exists().then_some(dir)
}
