use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One byte range of the file, downloaded by one connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Segment {
    /// First byte of the range.
    pub start: u64,
    /// One past the last byte of the range. It can shrink while downloading,
    /// when another connection takes over the second half.
    pub end: u64,
    /// Bytes already written, counted from `start`.
    pub downloaded: u64,
    /// Whether a connection is working on this segment right now.
    #[serde(skip)]
    pub active: bool,
}

impl Segment {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end, downloaded: 0, active: false }
    }

    /// Next byte to download.
    pub fn position(&self) -> u64 {
        self.start + self.downloaded
    }

    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.position())
    }

    pub fn is_done(&self) -> bool {
        self.remaining() == 0
    }
}

/// Everything needed to resume a download, saved next to the partial file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadState {
    pub url: String,
    pub size: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub segments: Vec<Segment>,
}

impl DownloadState {
    pub fn downloaded(&self) -> u64 {
        self.segments.iter().map(|s| s.downloaded).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.segments.iter().all(Segment::is_done)
    }

    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// Writes to a temporary file first, so a crash never leaves a broken state file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(io::Error::other)?)?;
        std::fs::rename(&tmp, path)
    }

    /// Picks work for a free connection: an untouched segment if there is one,
    /// otherwise the second half of the biggest segment still in progress.
    /// Returns the index of the segment to download.
    pub fn claim(&mut self, min_split: u64) -> Option<usize> {
        if let Some(i) = self.segments.iter().position(|s| !s.active && !s.is_done()) {
            self.segments[i].active = true;
            return Some(i);
        }

        let (i, seg) = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, s)| s.active)
            .max_by_key(|(_, s)| s.remaining())?;
        if seg.remaining() < min_split * 2 {
            return None;
        }
        let mid = seg.position() + seg.remaining() / 2;
        let mut new = Segment::new(mid, seg.end);
        new.active = true;
        self.segments[i].end = mid;
        self.segments.push(new);
        Some(self.segments.len() - 1)
    }
}

/// Splits `size` bytes into at most `parts` segments of at least `min_size` bytes.
pub fn split_into_segments(size: u64, parts: usize, min_size: u64) -> Vec<Segment> {
    if size == 0 {
        return Vec::new();
    }
    let max_parts = (size / min_size.max(1)).max(1);
    let parts = (parts.max(1) as u64).min(max_parts);
    let chunk = size / parts;
    (0..parts)
        .map(|i| {
            let start = i * chunk;
            let end = if i == parts - 1 { size } else { start + chunk };
            Segment::new(start, end)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_evenly_and_covers_everything() {
        let segs = split_into_segments(1000, 3, 1);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].start, 0);
        assert_eq!(segs.last().unwrap().end, 1000);
        for w in segs.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }

    #[test]
    fn small_files_get_fewer_segments() {
        assert_eq!(split_into_segments(100, 8, 60).len(), 1);
        assert_eq!(split_into_segments(0, 8, 60).len(), 0);
    }

    #[test]
    fn claim_splits_the_biggest_active_segment() {
        let mut state = DownloadState {
            url: String::new(),
            size: 1000,
            etag: None,
            last_modified: None,
            segments: split_into_segments(1000, 1, 1),
        };
        assert_eq!(state.claim(100), Some(0));
        state.segments[0].downloaded = 200;
        // 800 bytes left: the new connection takes the last 400.
        assert_eq!(state.claim(100), Some(1));
        assert_eq!(state.segments[0].end, 600);
        assert_eq!(state.segments[1].start, 600);
        assert_eq!(state.segments[1].end, 1000);
        // 400 left in each: too small to split with min 300.
        assert_eq!(state.claim(300), None);
    }
}
