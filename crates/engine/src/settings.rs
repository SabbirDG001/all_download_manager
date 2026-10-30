use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const FILE_NAME: &str = "settings.json";

/// User settings, stored as JSON.
///
/// The settings file lives in `%APPDATA%\AllDownloadManager\settings.json`,
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
    /// User-Agent header to send; the app's own name when unset.
    pub user_agent: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let download_dir = dirs::download_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self { download_dir, temp_dir: None, connections: 8, user_agent: None }
    }
}

impl Settings {
    /// Where the settings file is read from and written to.
    pub fn path() -> PathBuf {
        if let Some(portable) = portable_path() {
            if portable.exists() {
                return portable;
            }
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AllDownloadManager")
            .join(FILE_NAME)
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
        Ok(())
    }
}

fn portable_path() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join(FILE_NAME))
}
