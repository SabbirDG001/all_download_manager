//! File types, for the sidebar and for sorting downloads into folders.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Video,
    Music,
    Documents,
    Compressed,
    Programs,
    Images,
    Other,
}

impl Category {
    pub fn of(file_name: &str) -> Self {
        let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        match ext.as_str() {
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "flv" | "wmv" | "m4v" | "ts" | "3gp" | "mpg" | "mpeg" => Self::Video,
            "mp3" | "m4a" | "flac" | "wav" | "aac" | "ogg" | "opus" | "wma" | "aiff" => Self::Music,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "epub" | "csv" | "odt" | "rtf" | "md" => {
                Self::Documents
            }
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" | "iso" | "tgz" => Self::Compressed,
            "exe" | "msi" | "msix" | "appx" | "apk" | "dmg" | "deb" | "rpm" | "appimage" => Self::Programs,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "heic" | "avif" | "tif" | "tiff" => Self::Images,
            _ => Self::Other,
        }
    }

    /// Subfolder of the download folder, when sorting by type. "Other" files
    /// stay in the download folder itself.
    pub fn folder(self) -> Option<&'static str> {
        match self {
            Self::Video => Some("Video"),
            Self::Music => Some("Music"),
            Self::Documents => Some("Documents"),
            Self::Compressed => Some("Compressed"),
            Self::Programs => Some("Programs"),
            Self::Images => Some("Images"),
            Self::Other => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_by_extension() {
        assert_eq!(Category::of("Movie.MKV"), Category::Video);
        assert_eq!(Category::of("song.mp3"), Category::Music);
        assert_eq!(Category::of("setup.exe"), Category::Programs);
        assert_eq!(Category::of("backup.tar.gz"), Category::Compressed);
        assert_eq!(Category::of("README"), Category::Other);
        assert_eq!(Category::Other.folder(), None);
    }
}
