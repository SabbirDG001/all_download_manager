//! Video and audio downloads through yt-dlp (with ffmpeg for merging and
//! converting). yt-dlp knows how to get the media out of YouTube and well
//! over a thousand other sites, and out of HLS/DASH streams.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::download::{Event, Progress};
use crate::{Error, Result};

/// Quality choices offered to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    #[default]
    Best,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "480p")]
    P480,
    /// Audio only, converted to MP3.
    Audio,
}

#[derive(Debug, Clone)]
pub struct MediaJob {
    /// Page or stream address.
    pub url: String,
    pub dir: PathBuf,
    pub temp_dir: Option<PathBuf>,
    pub quality: Quality,
    pub headers: Vec<(String, String)>,
    /// Page title from the browser. Used to name bare streams (HLS/DASH
    /// playlists), which have no title of their own.
    pub title_hint: Option<String>,
}

impl MediaJob {
    /// The name to use instead of yt-dlp's title, if any.
    fn name_override(&self) -> Option<String> {
        let path = reqwest::Url::parse(&self.url).ok()?.path().to_ascii_lowercase();
        let stream = path.ends_with(".m3u8") || path.ends_with(".mpd");
        let hint = self.title_hint.as_deref().map(str::trim).filter(|t| !t.is_empty())?;
        stream.then(|| crate::sanitize_filename(hint).chars().take(120).collect())
    }
}

/// Where the helper programs are.
#[derive(Debug, Clone, Default)]
pub struct MediaTools {
    pub yt_dlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

// Marker prefixes in yt-dlp's output, so our lines are easy to pick out.
const INFO: &str = "ADM-INFO ";
const FILE: &str = "ADM-FILE ";
const PROGRESS: &str = "ADM-PROGRESS ";

/// The yt-dlp command line for a job.
pub fn yt_dlp_args(job: &MediaJob, ffmpeg: Option<&Path>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--newline",
        "--no-colors",
        "--progress",
        "--no-quiet",
        "--no-playlist",
        "--no-mtime",
        "--windows-filenames",
        "--continue",
        "--encoding",
        "utf-8",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let mut push = |a: &str, b: String| {
        args.push(a.to_string());
        args.push(b);
    };
    push("--paths", job.dir.display().to_string());
    if let Some(temp) = &job.temp_dir {
        push("--paths", format!("temp:{}", temp.display()));
    }
    let output = match job.name_override() {
        // yt-dlp output templates treat % specially.
        Some(name) => format!("{} [%(id)s].%(ext)s", name.replace('%', "%%")),
        None => "%(title).120B [%(id)s].%(ext)s".into(),
    };
    push("--output", output);
    push("--print", format!("before_dl:{INFO}%(id)s %(title)s"));
    push("--print", format!("after_move:{FILE}%(filepath)s"));
    push(
        "--progress-template",
        format!(
            "download:{PROGRESS}%(progress.downloaded_bytes)s %(progress.total_bytes)s \
             %(progress.total_bytes_estimate)s %(info.format_id)s"
        ),
    );

    let has_ffmpeg = ffmpeg.is_some();
    let mut extract_audio = false;
    if let Some(ffmpeg) = ffmpeg {
        push("--ffmpeg-location", ffmpeg.display().to_string());
    }
    match (job.quality, has_ffmpeg) {
        (Quality::Audio, true) => {
            push("--format", "ba/b".into());
            push("--audio-format", "mp3".into());
            push("--audio-quality", "0".into());
            extract_audio = true;
        }
        // Without ffmpeg nothing can be converted: take the audio as it comes.
        (Quality::Audio, false) => push("--format", "ba/b".into()),
        // Without ffmpeg separate video and audio can't be merged: one file only.
        (_, false) => push("--format", "b".into()),
        (quality, true) => {
            push("--format", "bv*+ba/b".into());
            push("--merge-output-format", "mp4/mkv".into());
            let limit = match quality {
                Quality::P1080 => Some(1080),
                Quality::P720 => Some(720),
                Quality::P480 => Some(480),
                _ => None,
            };
            if let Some(height) = limit {
                push("--format-sort", format!("res:{height}"));
            }
        }
    }

    for (name, value) in &job.headers {
        match name.to_ascii_lowercase().as_str() {
            "user-agent" => push("--user-agent", value.clone()),
            "referer" => push("--referer", value.clone()),
            _ => push("--add-header", format!("{name}:{value}")),
        }
    }
    if extract_audio {
        args.push("--extract-audio".into());
    }
    args.push("--".into());
    args.push(job.url.clone());
    args
}

/// Combines yt-dlp's per-file progress into one running total, because a
/// video is often fetched as two files (video, then audio).
#[derive(Debug, Default)]
pub(crate) struct ProgressTracker {
    finished: u64,
    current: u64,
    current_total: Option<u64>,
    current_format: String,
}

impl ProgressTracker {
    /// Reads one progress line (without the marker) and returns the totals.
    pub(crate) fn update(&mut self, line: &str) -> Option<(u64, Option<u64>)> {
        let mut parts = line.split_whitespace();
        let downloaded = parse_num(parts.next()?)?;
        let total = parse_num(parts.next().unwrap_or("NA"));
        let estimate = parse_num(parts.next().unwrap_or("NA"));
        let format = parts.next().unwrap_or("").to_string();

        if format != self.current_format || downloaded < self.current {
            // A new file started: bank the previous one.
            if !self.current_format.is_empty() || self.current > 0 {
                self.finished += self.current_total.unwrap_or(self.current).max(self.current);
            }
            self.current_format = format;
        }
        self.current = downloaded;
        self.current_total = total.or(estimate);
        let total = self.current_total.map(|t| self.finished + t.max(downloaded));
        Some((self.finished + downloaded, total))
    }
}

fn parse_num(s: &str) -> Option<u64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64)
}

/// Runs yt-dlp for `job` and returns the finished file's path.
///
/// Reports `Event::Media` once the title is known, `Event::Progress` while
/// downloading and `Event::Stage` while ffmpeg merges or converts.
pub async fn download_media<F>(
    job: &MediaJob,
    tools: &MediaTools,
    on_event: F,
    cancel: CancellationToken,
) -> Result<PathBuf>
where
    F: Fn(Event) + Send + Sync,
{
    let yt_dlp = tools.yt_dlp.as_ref().ok_or(Error::ToolMissing("yt-dlp"))?;
    tokio::fs::create_dir_all(&job.dir).await?;

    let mut cmd = tokio::process::Command::new(yt_dlp);
    cmd.args(yt_dlp_args(job, tools.ffmpeg.as_deref()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::tools::hide_console(&mut cmd);
    let mut child = cmd.spawn()?;

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let errors = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut last_error = None;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(msg) = line.strip_prefix("ERROR: ") {
                last_error = Some(msg.to_string());
            }
        }
        last_error
    });

    let mut lines = BufReader::new(stdout).lines();
    let mut tracker = ProgressTracker::default();
    let name_override = job.name_override();
    let mut file = None;
    loop {
        let line = tokio::select! {
            l = lines.next_line() => l?,
            _ = cancel.cancelled() => {
                crate::tools::kill_tree(&mut child).await;
                return Err(Error::Cancelled);
            }
        };
        let Some(line) = line else { break };
        if let Some(rest) = line.strip_prefix(PROGRESS) {
            if let Some((downloaded, total)) = tracker.update(rest) {
                on_event(Event::Progress(Progress { downloaded, total, connections: 1 }));
            }
        } else if let Some(rest) = line.strip_prefix(INFO) {
            let (id, title) = rest.split_once(' ').unwrap_or((rest, ""));
            let title = name_override.clone().unwrap_or_else(|| title.to_string());
            on_event(Event::Media { id: id.to_string(), title });
        } else if let Some(path) = line.strip_prefix(FILE) {
            file = Some(PathBuf::from(path.trim()));
        } else if let Some(stage) = stage_of(&line) {
            on_event(Event::Stage(stage.to_string()));
        }
    }

    let status = child.wait().await?;
    let error = errors.await.ok().flatten();
    if !status.success() {
        return Err(Error::Tool(error.unwrap_or_else(|| format!("yt-dlp stopped ({status})"))));
    }
    file.ok_or_else(|| Error::Tool(error.unwrap_or_else(|| "yt-dlp didn't save a file".into())))
}

/// Recognises yt-dlp's post-processing steps.
fn stage_of(line: &str) -> Option<&'static str> {
    if line.starts_with("[Merger]") {
        Some("Merging")
    } else if line.starts_with("[ExtractAudio]") {
        Some("Converting")
    } else if line.starts_with("[Fixup") || line.starts_with("[VideoConvertor]") {
        Some("Finishing")
    } else {
        None
    }
}

/// Deletes the leftovers of an unfinished media download.
pub fn delete_partial_media(dirs: &[&Path], id: &str) {
    let tag = format!("[{id}]");
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let partial = name.ends_with(".part")
                || name.ends_with(".ytdl")
                || name.contains(".part-Frag")
                || name.contains(".temp.");
            if name.contains(&tag) && partial {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Whether a link is better handled by yt-dlp than as a plain file: known
/// video sites, and streaming playlists.
pub fn looks_like_media_page(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else { return false };
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".m3u8") || path.ends_with(".mpd") {
        return true;
    }
    let host = url.host_str().unwrap_or("").trim_start_matches("www.").trim_start_matches("m.");
    MEDIA_SITES.iter().any(|site| host == *site || host.ends_with(&format!(".{site}")))
}

/// Popular sites yt-dlp supports. Kept in sync with `extension/sites.js`.
pub const MEDIA_SITES: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "dailymotion.com",
    "twitch.tv", "x.com", "twitter.com", "facebook.com", "fb.watch", "instagram.com",
    "tiktok.com", "reddit.com", "soundcloud.com", "bandcamp.com", "bilibili.com",
    "ted.com", "streamable.com", "rumble.com", "kick.com", "odysee.com", "mixcloud.com",
    "9gag.com", "imgur.com", "pinterest.com", "linkedin.com", "threads.net",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn job(quality: Quality) -> MediaJob {
        MediaJob {
            url: "https://www.youtube.com/watch?v=abc".into(),
            dir: PathBuf::from("/dl"),
            temp_dir: None,
            quality,
            headers: vec![("Cookie".into(), "a=b".into()), ("Referer".into(), "https://r/".into())],
            title_hint: Some("Page: 100% \"live\"".into()),
        }
    }

    #[test]
    fn names_bare_streams_after_the_page() {
        let site = yt_dlp_args(&job(Quality::Best), None);
        assert_eq!(value_after(&site, "--output"), Some("%(title).120B [%(id)s].%(ext)s"));
        let mut stream = job(Quality::Best);
        stream.url = "https://cdn.example.com/hls/master.m3u8".into();
        let args = yt_dlp_args(&stream, None);
        assert_eq!(value_after(&args, "--output"), Some("Page_ 100%% _live_ [%(id)s].%(ext)s"));
    }

    fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.iter().position(|a| a == flag).map(|i| args[i + 1].as_str())
    }

    #[test]
    fn builds_arguments() {
        let ffmpeg = PathBuf::from("/tools/ffmpeg");
        let args = yt_dlp_args(&job(Quality::P720), Some(&ffmpeg));
        assert_eq!(value_after(&args, "--format"), Some("bv*+ba/b"));
        assert_eq!(value_after(&args, "--format-sort"), Some("res:720"));
        assert_eq!(value_after(&args, "--referer"), Some("https://r/"));
        assert_eq!(value_after(&args, "--add-header"), Some("Cookie:a=b"));
        assert_eq!(args.last().unwrap(), "https://www.youtube.com/watch?v=abc");

        let audio = yt_dlp_args(&job(Quality::Audio), Some(&ffmpeg));
        assert!(audio.contains(&"--extract-audio".to_string()));

        // No ffmpeg: a single ready-made file, nothing to merge or convert.
        let plain = yt_dlp_args(&job(Quality::Best), None);
        assert_eq!(value_after(&plain, "--format"), Some("b"));
        assert!(!plain.contains(&"--extract-audio".to_string()));
    }

    #[test]
    fn combines_progress_of_video_and_audio() {
        let mut t = ProgressTracker::default();
        assert_eq!(t.update("100 1000 NA 137"), Some((100, Some(1000))));
        assert_eq!(t.update("1000 1000 NA 137"), Some((1000, Some(1000))));
        // Audio file starts.
        assert_eq!(t.update("50 200 NA 140"), Some((1050, Some(1200))));
        assert_eq!(t.update("200 200 NA 140"), Some((1200, Some(1200))));
        // Unknown size, then an estimate.
        let mut t = ProgressTracker::default();
        assert_eq!(t.update("10 NA NA 0"), Some((10, None)));
        assert_eq!(t.update("20 NA 500.5 0"), Some((20, Some(500))));
    }

    #[tokio::test]
    async fn reports_missing_yt_dlp() {
        let result = download_media(&job(Quality::Best), &MediaTools::default(), |_| {}, CancellationToken::new()).await;
        let err = result.unwrap_err().to_string();
        assert!(err.contains("yt-dlp is not installed"), "{err}");
    }

    #[test]
    fn extension_knows_the_same_sites() {
        let js = include_str!("../../../extension/sites.js");
        for site in MEDIA_SITES {
            assert!(js.contains(&format!("\"{site}\"")), "{site} is missing from extension/sites.js");
        }
    }

    #[test]
    fn spots_media_pages() {
        assert!(looks_like_media_page("https://www.youtube.com/watch?v=x"));
        assert!(looks_like_media_page("https://m.youtube.com/watch?v=x"));
        assert!(looks_like_media_page("https://youtu.be/x"));
        assert!(looks_like_media_page("https://cdn.example.com/live/master.m3u8?t=1"));
        assert!(!looks_like_media_page("https://example.com/file.zip"));
        assert!(!looks_like_media_page("https://notyoutube.com/x"));
    }
}
