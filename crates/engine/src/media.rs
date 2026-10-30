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

/// Audio file types the user can pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    Mp3,
    M4a,
    Opus,
    /// Whatever the site has, unconverted.
    Original,
}

/// Exactly what to download from a video page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MediaFormat {
    /// Video with sound, at most `height` pixels tall (the best when unset).
    Video { height: Option<u32> },
    /// Sound only. `kbps` sets the MP3 bitrate (best variable bitrate when unset).
    Audio { codec: AudioCodec, kbps: Option<u32> },
}

impl From<Quality> for MediaFormat {
    fn from(q: Quality) -> Self {
        match q {
            Quality::Best => MediaFormat::Video { height: None },
            Quality::P1080 => MediaFormat::Video { height: Some(1080) },
            Quality::P720 => MediaFormat::Video { height: Some(720) },
            Quality::P480 => MediaFormat::Video { height: Some(480) },
            Quality::Audio => MediaFormat::Audio { codec: AudioCodec::Mp3, kbps: None },
        }
    }
}

impl MediaFormat {
    /// Short name for the list, such as "1080p" or "MP3 320 kbps".
    pub fn label(&self) -> String {
        match self {
            MediaFormat::Video { height: None } => "Best video".into(),
            MediaFormat::Video { height: Some(h) } => format!("{h}p"),
            MediaFormat::Audio { codec, kbps } => {
                let name = match codec {
                    AudioCodec::Mp3 => "MP3",
                    AudioCodec::M4a => "M4A",
                    AudioCodec::Opus => "Opus",
                    AudioCodec::Original => "Audio",
                };
                match kbps {
                    Some(k) if *codec == AudioCodec::Mp3 => format!("{name} {k} kbps"),
                    _ => name.to_string(),
                }
            }
        }
    }

    pub fn is_audio(&self) -> bool {
        matches!(self, MediaFormat::Audio { .. })
    }
}

#[derive(Debug, Clone)]
pub struct MediaJob {
    /// Page or stream address.
    pub url: String,
    pub dir: PathBuf,
    pub temp_dir: Option<PathBuf>,
    pub format: MediaFormat,
    pub headers: Vec<(String, String)>,
    /// Speed limit in bytes per second.
    pub rate_limit: Option<u64>,
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

    if let Some(ffmpeg) = ffmpeg {
        push("--ffmpeg-location", ffmpeg.display().to_string());
    }
    args.extend(format_args(job.format, ffmpeg.is_some()));
    if let Some(limit) = job.rate_limit.filter(|l| *l > 0) {
        args.push("--limit-rate".into());
        args.push(limit.to_string());
    }
    args.extend(header_args(&job.headers));
    args.push("--".into());
    args.push(job.url.clone());
    args
}

/// yt-dlp options that pick (and convert to) the chosen format. Without
/// ffmpeg nothing can be merged or converted, so a ready-made file is taken.
fn format_args(format: MediaFormat, has_ffmpeg: bool) -> Vec<String> {
    let pair = |a: &str, b: String| [a.to_string(), b];
    match format {
        MediaFormat::Video { height } => {
            let pick = match (height, has_ffmpeg) {
                (Some(h), true) => format!("bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"),
                (None, true) => "bv*+ba/b".into(),
                (Some(h), false) => format!("b[height<={h}]/b"),
                (None, false) => "b".into(),
            };
            let mut args = pair("--format", pick).to_vec();
            if has_ffmpeg {
                args.extend(pair("--merge-output-format", "mp4/mkv".into()));
            }
            args
        }
        MediaFormat::Audio { codec, kbps } => {
            let pick = match codec {
                AudioCodec::M4a => "ba[ext=m4a]/ba/b",
                AudioCodec::Opus => "ba[acodec^=opus]/ba/b",
                AudioCodec::Mp3 | AudioCodec::Original => "ba/b",
            };
            let mut args = pair("--format", pick.into()).to_vec();
            if has_ffmpeg {
                match codec {
                    AudioCodec::Mp3 => {
                        args.extend(pair("--audio-format", "mp3".into()));
                        args.extend(pair("--audio-quality", kbps.map_or("0".into(), |k| format!("{k}K"))));
                    }
                    AudioCodec::M4a => args.extend(pair("--audio-format", "m4a".into())),
                    AudioCodec::Opus => args.extend(pair("--audio-format", "opus".into())),
                    AudioCodec::Original => {}
                }
                args.push("--extract-audio".into());
            }
            args
        }
    }
}

/// Browser headers as yt-dlp options.
fn header_args(headers: &[(String, String)]) -> Vec<String> {
    let mut args = Vec::new();
    for (name, value) in headers {
        let (flag, value) = match name.to_ascii_lowercase().as_str() {
            "user-agent" => ("--user-agent", value.clone()),
            "referer" => ("--referer", value.clone()),
            _ => ("--add-header", format!("{name}:{value}")),
        };
        args.push(flag.to_string());
        args.push(value);
    }
    args
}

// ---------- Listing the qualities a page offers ----------

/// What a video page offers, for the quality picker.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub title: String,
    pub uploader: Option<String>,
    /// Length in seconds.
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub video: Vec<FormatOption>,
    pub audio: Vec<FormatOption>,
    /// Whether HD choices are limited because ffmpeg is missing.
    pub needs_ffmpeg: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatOption {
    pub format: MediaFormat,
    /// Such as "1080p60" or "MP3 320 kbps".
    pub label: String,
    /// Such as "Full HD" or "original".
    pub detail: String,
    /// Estimated size in bytes.
    pub size: Option<u64>,
}

/// Asks yt-dlp which formats `url` has.
pub async fn probe_media(url: &str, headers: &[(String, String)], tools: &MediaTools) -> Result<MediaInfo> {
    let yt_dlp = tools.yt_dlp.as_ref().ok_or(Error::ToolMissing("yt-dlp"))?;
    let mut cmd = tokio::process::Command::new(yt_dlp);
    cmd.args(["--dump-single-json", "--no-playlist", "--flat-playlist", "--no-warnings", "--encoding", "utf-8"])
        .args(header_args(headers))
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::tools::hide_console(&mut cmd);
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), cmd.output())
        .await
        .map_err(|_| Error::Tool("yt-dlp took too long to answer".into()))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().rev().find_map(|l| l.strip_prefix("ERROR: ")).unwrap_or("yt-dlp couldn't read this page");
        return Err(Error::Tool(msg.to_string()));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| Error::Tool(e.to_string()))?;
    media_info_from_json(&json, tools.ffmpeg.is_some())
}

/// Turns yt-dlp's description of a page into quality choices.
pub fn media_info_from_json(v: &serde_json::Value, has_ffmpeg: bool) -> Result<MediaInfo> {
    if v["_type"] == "playlist" {
        return Err(Error::Tool("This link is a playlist. Open one of its videos and use that link.".into()));
    }
    let duration = v["duration"].as_f64();
    let formats: Vec<&serde_json::Value> = v["formats"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let has = |f: &serde_json::Value, key: &str| f[key].as_str().is_some_and(|c| c != "none");
    let height = |f: &serde_json::Value| f["height"].as_u64().unwrap_or(0) as u32;
    let size = |f: &serde_json::Value| -> Option<u64> {
        f["filesize"].as_u64().or(f["filesize_approx"].as_u64()).or_else(|| {
            let kbps = f["tbr"].as_f64()?;
            Some((kbps * 1000.0 / 8.0 * duration?) as u64)
        })
    };
    let rate = |f: &serde_json::Value, key: &str| f[key].as_f64().unwrap_or(0.0);

    // Best audio-only track, preferring M4A (it merges into MP4 cleanly).
    let audio_only: Vec<_> = formats.iter().copied().filter(|f| has(f, "acodec") && !has(f, "vcodec")).collect();
    let best_audio = |pred: &dyn Fn(&serde_json::Value) -> bool| {
        audio_only.iter().copied().filter(|f| pred(f)).max_by(|a, b| rate(a, "abr").total_cmp(&rate(b, "abr")))
    };
    let m4a = best_audio(&|f| f["ext"] == "m4a");
    let opus = best_audio(&|f| f["acodec"].as_str().is_some_and(|c| c.starts_with("opus")));
    let any_audio = best_audio(&|_| true);
    let merge_audio_size = m4a.or(any_audio).and_then(size).unwrap_or(0);
    // Streams often don't say what they carry: unknown counts as "may have sound".
    let has_any_sound = formats.is_empty() || formats.iter().any(|f| f["acodec"] != "none");

    // One choice per height: separate video tracks need ffmpeg to add sound.
    let mut by_height: std::collections::BTreeMap<u32, (&serde_json::Value, bool)> = Default::default();
    for f in formats.iter().copied().filter(|f| has(f, "vcodec") && height(f) > 0) {
        let progressive = has(f, "acodec");
        if !has_ffmpeg && !progressive {
            continue;
        }
        let better = match by_height.get(&height(f)) {
            None => true,
            Some((cur, _)) => (rate(f, "fps"), rate(f, "tbr")) > (rate(cur, "fps"), rate(cur, "tbr")),
        };
        if better {
            by_height.insert(height(f), (f, progressive));
        }
    }
    let video = by_height
        .iter()
        .rev()
        .map(|(&h, &(f, progressive))| {
            let fps = rate(f, "fps").round() as u32;
            let label = if fps > 30 { format!("{h}p{fps}") } else { format!("{h}p") };
            let detail = match h {
                h if h >= 2160 => "4K Ultra HD",
                h if h >= 1440 => "2K Quad HD",
                h if h >= 1080 => "Full HD",
                h if h >= 720 => "HD",
                _ => "",
            };
            let size = size(f).map(|s| if progressive { s } else { s + merge_audio_size });
            FormatOption { format: MediaFormat::Video { height: Some(h) }, label, detail: detail.into(), size }
        })
        .collect::<Vec<_>>();

    let mut audio = Vec::new();
    if has_any_sound {
        if has_ffmpeg {
            for kbps in [320, 192, 128] {
                audio.push(FormatOption {
                    format: MediaFormat::Audio { codec: AudioCodec::Mp3, kbps: Some(kbps) },
                    label: format!("MP3 {kbps} kbps"),
                    detail: String::new(),
                    size: duration.map(|d| (d * kbps as f64 * 1000.0 / 8.0) as u64),
                });
            }
        }
        let original = |f: &serde_json::Value, codec: AudioCodec, name: &str| {
            let abr = rate(f, "abr").round() as u32;
            FormatOption {
                format: MediaFormat::Audio { codec, kbps: None },
                label: if abr > 0 { format!("{name} {abr} kbps") } else { name.to_string() },
                detail: "original".into(),
                size: size(f),
            }
        };
        if let Some(f) = m4a {
            audio.push(original(f, AudioCodec::M4a, "M4A"));
        }
        if let Some(f) = opus.filter(|_| has_ffmpeg) {
            audio.push(original(f, AudioCodec::Opus, "Opus"));
        }
        if m4a.is_none() && opus.is_none() {
            if let Some(f) = any_audio {
                audio.push(original(f, AudioCodec::Original, "Audio"));
            }
        }
    }

    // Pages and streams without resolution details: just "the best there is".
    let video = if video.is_empty() && formats.iter().all(|f| f["vcodec"] != "none") {
        vec![FormatOption { format: MediaFormat::Video { height: None }, label: "Best".into(), detail: String::new(), size: v["filesize"].as_u64() }]
    } else {
        video
    };

    Ok(MediaInfo {
        title: v["title"].as_str().unwrap_or("").to_string(),
        uploader: v["uploader"].as_str().or(v["channel"].as_str()).map(str::to_string),
        duration,
        thumbnail: v["thumbnail"].as_str().filter(|t| t.starts_with("https://")).map(str::to_string),
        video,
        audio,
        needs_ffmpeg: !has_ffmpeg,
    })
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

    fn job(quality: impl Into<MediaFormat>) -> MediaJob {
        MediaJob {
            url: "https://www.youtube.com/watch?v=abc".into(),
            dir: PathBuf::from("/dl"),
            temp_dir: None,
            format: quality.into(),
            headers: vec![("Cookie".into(), "a=b".into()), ("Referer".into(), "https://r/".into())],
            title_hint: Some("Page: 100% \"live\"".into()),
            rate_limit: None,
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
        assert_eq!(value_after(&args, "--format"), Some("bv*[height<=720]+ba/b[height<=720]/bv*+ba/b"));
        assert_eq!(value_after(&args, "--merge-output-format"), Some("mp4/mkv"));
        assert_eq!(value_after(&args, "--referer"), Some("https://r/"));
        assert_eq!(value_after(&args, "--add-header"), Some("Cookie:a=b"));
        assert_eq!(args.last().unwrap(), "https://www.youtube.com/watch?v=abc");

        let audio = yt_dlp_args(&job(Quality::Audio), Some(&ffmpeg));
        assert!(audio.contains(&"--extract-audio".to_string()));
        assert_eq!(value_after(&audio, "--audio-quality"), Some("0"));

        let mp3 = yt_dlp_args(&job(MediaFormat::Audio { codec: AudioCodec::Mp3, kbps: Some(192) }), Some(&ffmpeg));
        assert_eq!(value_after(&mp3, "--audio-quality"), Some("192K"));
        let m4a = yt_dlp_args(&job(MediaFormat::Audio { codec: AudioCodec::M4a, kbps: None }), Some(&ffmpeg));
        assert_eq!(value_after(&m4a, "--format"), Some("ba[ext=m4a]/ba/b"));
        assert_eq!(value_after(&m4a, "--audio-format"), Some("m4a"));

        let low = yt_dlp_args(&job(MediaFormat::Video { height: Some(360) }), None);
        assert_eq!(value_after(&low, "--format"), Some("b[height<=360]/b"));

        // No ffmpeg: a single ready-made file, nothing to merge or convert.
        let plain = yt_dlp_args(&job(Quality::Best), None);
        assert_eq!(value_after(&plain, "--format"), Some("b"));
        assert!(!plain.contains(&"--extract-audio".to_string()));
    }

    /// Trimmed-down yt-dlp output for a YouTube-like page.
    fn sample_page() -> serde_json::Value {
        serde_json::json!({
            "title": "Rust in 100 Seconds", "uploader": "Fireship", "duration": 150.0,
            "thumbnail": "https://i.ytimg.com/vi/x/maxresdefault.jpg",
            "formats": [
                {"format_id": "139", "ext": "m4a", "acodec": "mp4a.40.5", "vcodec": "none", "abr": 48.8, "filesize": 900000},
                {"format_id": "140", "ext": "m4a", "acodec": "mp4a.40.2", "vcodec": "none", "abr": 129.5, "filesize": 2400000},
                {"format_id": "251", "ext": "webm", "acodec": "opus", "vcodec": "none", "abr": 135.2, "filesize": 2500000},
                {"format_id": "18", "ext": "mp4", "acodec": "mp4a.40.2", "vcodec": "avc1", "height": 360, "fps": 30, "filesize": 9000000},
                {"format_id": "134", "ext": "mp4", "acodec": "none", "vcodec": "avc1", "height": 360, "fps": 30, "filesize": 5000000},
                {"format_id": "136", "ext": "mp4", "acodec": "none", "vcodec": "avc1", "height": 720, "fps": 30, "filesize": 20000000},
                {"format_id": "299", "ext": "mp4", "acodec": "none", "vcodec": "avc1", "height": 1080, "fps": 60, "filesize_approx": 60000000},
                {"format_id": "303", "ext": "webm", "acodec": "none", "vcodec": "vp9", "height": 1080, "fps": 30, "filesize": 50000000},
                {"format_id": "313", "ext": "webm", "acodec": "none", "vcodec": "vp9", "height": 2160, "fps": 30, "tbr": 16000.0},
                {"format_id": "sb0", "ext": "mhtml", "acodec": "none", "vcodec": "none"}
            ]
        })
    }

    #[test]
    fn lists_qualities_like_idm() {
        let info = media_info_from_json(&sample_page(), true).unwrap();
        assert_eq!(info.title, "Rust in 100 Seconds");
        let labels: Vec<_> = info.video.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["2160p", "1080p60", "720p", "360p"]);
        assert_eq!(info.video[0].detail, "4K Ultra HD");
        // 4K size from bitrate (16000 kbps for 150 s) plus the M4A audio.
        assert_eq!(info.video[0].size, Some(300_000_000 + 2_400_000));
        // 1080p60 beats 1080p30, and its approximate size is used.
        assert_eq!(info.video[1].size, Some(60_000_000 + 2_400_000));
        // 360p exists as a file with sound and as video only; either way it has a size.
        assert!(info.video[3].size.is_some());

        let audio: Vec<_> = info.audio.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(audio, ["MP3 320 kbps", "MP3 192 kbps", "MP3 128 kbps", "M4A 130 kbps", "Opus 135 kbps"]);
        assert_eq!(info.audio[0].size, Some(6_000_000));
        assert_eq!(info.audio[3].format, MediaFormat::Audio { codec: AudioCodec::M4a, kbps: None });
    }

    #[test]
    fn without_ffmpeg_only_ready_made_files() {
        let info = media_info_from_json(&sample_page(), false).unwrap();
        let labels: Vec<_> = info.video.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["360p"]);
        let audio: Vec<_> = info.audio.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(audio, ["M4A 130 kbps"]);
        assert!(info.needs_ffmpeg);
    }

    #[test]
    fn playlists_are_refused() {
        let err = media_info_from_json(&serde_json::json!({"_type": "playlist"}), true).unwrap_err();
        assert!(err.to_string().contains("playlist"));
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
