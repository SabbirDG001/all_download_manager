//! All Download Manager engine.
//!
//! Downloads a file over several parallel HTTP connections (using `Range`
//! requests), keeps its progress on disk so it can resume after a crash or a
//! dropped connection, and re-splits slow segments when a connection frees up.

mod download;
mod error;
mod filename;
mod manager;
mod media;
mod probe;
mod settings;
mod speed;
mod state;
mod tools;

pub use download::{download, download_with_events, part_paths, DownloadOptions, Event, Progress};
pub use error::{Error, Result};
pub use filename::sanitize_filename;
pub use manager::{AddRequest, Change, Entry, Kind, Manager, Status};
pub use media::{
    download_media, looks_like_media_page, media_info_from_json, probe_media, yt_dlp_args, AudioCodec, FormatOption,
    MediaFormat, MediaInfo, MediaJob, MediaTools, Quality, MEDIA_SITES,
};
pub use probe::{probe, ProbeInfo};
pub use settings::Settings;
pub use speed::SpeedMeter;
pub use state::{split_into_segments, DownloadState, Segment};
pub use tokio_util::sync::CancellationToken;
pub use tools::{find_tools, install_ffmpeg, install_yt_dlp, tools_status, ToolInfo, ToolsStatus};
