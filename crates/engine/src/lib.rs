//! All Download Manager engine.
//!
//! Downloads a file over several parallel HTTP connections (using `Range`
//! requests), keeps its progress on disk so it can resume after a crash or a
//! dropped connection, and re-splits slow segments when a connection frees up.

mod download;
mod error;
mod filename;
mod probe;
mod settings;
mod state;

pub use download::{download, DownloadOptions, Progress};
pub use error::{Error, Result};
pub use filename::sanitize_filename;
pub use probe::{probe, ProbeInfo};
pub use settings::Settings;
pub use state::{split_into_segments, DownloadState, Segment};
pub use tokio_util::sync::CancellationToken;
