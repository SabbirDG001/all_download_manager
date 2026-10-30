//! Whole playlists and folders: YouTube (and other sites') playlists and
//! channels through yt-dlp, FTP folders, and web folder listings such as
//! Apache/nginx "Index of" pages and h5ai, which many media servers use.

use std::collections::{HashSet, VecDeque};
use std::process::Stdio;

use percent_encoding::percent_decode_str;
use reqwest::Url;
use serde::Serialize;

use crate::media::{looks_like_media_page, MediaTools};
use crate::{Error, Result};

/// Most items listed from one link.
pub const MAX_ITEMS: usize = 5000;
/// Deepest subfolder level that is followed.
const MAX_DEPTH: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectionKind {
    Playlist,
    Folder,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionItem {
    pub url: String,
    /// Video title or file name.
    pub name: String,
    /// Subfolder inside the collection ("" for the top), with "/" between levels.
    pub folder: String,
    pub size: Option<u64>,
    /// Length in seconds, for videos.
    pub duration: Option<f64>,
    /// Download with yt-dlp (playlist videos) rather than as a plain file.
    pub media: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub title: String,
    pub kind: CollectionKind,
    pub items: Vec<CollectionItem>,
    /// More items exist than were listed.
    pub truncated: bool,
}

/// Whether a link probably points at a playlist, channel or folder.
pub fn looks_like_collection(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    let path = u.path();
    match u.scheme() {
        "ftp" => path.ends_with('/') || !path.rsplit('/').next().unwrap_or("").contains('.'),
        "http" | "https" => {
            if looks_like_media_page(url) {
                u.query_pairs().any(|(k, _)| k == "list")
                    || path.starts_with("/playlist")
                    || path.starts_with("/@")
                    || ["/channel/", "/c/", "/user/", "/showcase/", "/album/", "/sets/"].iter().any(|p| path.contains(p))
            } else {
                path.ends_with('/')
            }
        }
        _ => false,
    }
}

/// Lists everything behind a playlist, channel or folder link.
pub async fn list_collection(url: &str, headers: &[(String, String)], tools: &MediaTools) -> Result<Collection> {
    let parsed = Url::parse(url.trim()).map_err(|_| Error::Settings(format!("not a valid link: {url}")))?;
    match parsed.scheme() {
        "ftp" => list_ftp(&parsed).await,
        "http" | "https" if looks_like_media_page(url) => list_playlist(url, headers, tools).await,
        "http" | "https" => match list_web_folder(&parsed, headers).await {
            Ok(folder) => Ok(folder),
            // Not a folder listing: maybe a page with several videos.
            Err(folder_err) if tools.yt_dlp.is_some() => list_playlist(url, headers, tools).await.map_err(|_| folder_err),
            Err(e) => Err(e),
        },
        other => Err(Error::Settings(format!("{other} links are not supported"))),
    }
}

// ---------- Playlists (yt-dlp) ----------

async fn playlist_json(url: &str, headers: &[(String, String)], tools: &MediaTools) -> Result<serde_json::Value> {
    let yt_dlp = tools.yt_dlp.as_ref().ok_or(Error::ToolMissing("yt-dlp"))?;
    let mut cmd = tokio::process::Command::new(yt_dlp);
    cmd.args(["--flat-playlist", "--yes-playlist", "--dump-single-json", "--no-warnings", "--encoding", "utf-8"])
        .args(["--playlist-items", &format!("1:{MAX_ITEMS}")])
        .args(tools.runtime_args())
        .args(crate::media::header_args(headers))
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::tools::hide_console(&mut cmd);
    let out = tokio::time::timeout(std::time::Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| Error::Tool("yt-dlp took too long to list the playlist".into()))??;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().rev().find_map(|l| l.strip_prefix("ERROR: ")).unwrap_or("yt-dlp couldn't read this playlist");
        return Err(Error::Tool(crate::media::explain_error(msg, url, tools, &[])));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| Error::Tool(e.to_string()))
}

async fn list_playlist(url: &str, headers: &[(String, String)], tools: &MediaTools) -> Result<Collection> {
    let json = playlist_json(url, headers, tools).await?;
    if json["_type"] != "playlist" {
        return Err(Error::Tool("This link is a single video, not a playlist. Use Add URL for it.".into()));
    }
    let mut collection = playlist_from_json(&json);
    // A channel's main page lists its tabs (Videos, Shorts, Live) as playlists: open them.
    let tabs: Vec<(String, String)> = collection
        .items
        .iter()
        .filter(|i| is_nested_playlist(&i.url))
        .map(|i| (i.url.clone(), i.name.clone()))
        .collect();
    if !tabs.is_empty() {
        collection.items.retain(|i| !is_nested_playlist(&i.url));
        for (tab_url, tab_name) in tabs {
            if collection.items.len() >= MAX_ITEMS {
                collection.truncated = true;
                break;
            }
            if let Ok(tab) = playlist_json(&tab_url, headers, tools).await {
                let mut inner = playlist_from_json(&tab);
                let folder = crate::sanitize_filename(&tab_name);
                for item in &mut inner.items {
                    item.folder = folder.clone();
                }
                collection.items.extend(inner.items.into_iter().filter(|i| !is_nested_playlist(&i.url)));
            }
        }
        collection.items.truncate(MAX_ITEMS);
    }
    Ok(collection)
}

fn is_nested_playlist(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else { return false };
    let p = u.path();
    p.starts_with("/playlist") || ["/videos", "/shorts", "/streams", "/live", "/podcasts", "/releases"].iter().any(|t| p.ends_with(t))
}

/// Turns yt-dlp's flat playlist listing into items.
pub fn playlist_from_json(json: &serde_json::Value) -> Collection {
    let entries = json["entries"].as_array().cloned().unwrap_or_default();
    let items = entries
        .iter()
        .filter_map(|e| {
            let url = e["url"].as_str().or(e["webpage_url"].as_str())?;
            let url = if url.starts_with("http") {
                url.to_string()
            } else if e["ie_key"] == "Youtube" {
                format!("https://www.youtube.com/watch?v={}", e["id"].as_str()?)
            } else {
                return None;
            };
            let name = e["title"].as_str().unwrap_or(e["id"].as_str().unwrap_or("video")).to_string();
            let unavailable = name == "[Private video]" || name == "[Deleted video]";
            (!unavailable).then(|| CollectionItem {
                url,
                name,
                folder: String::new(),
                size: None,
                duration: e["duration"].as_f64(),
                media: true,
            })
        })
        .collect::<Vec<_>>();
    let truncated = json["playlist_count"].as_u64().is_some_and(|n| n as usize > entries.len());
    Collection {
        title: json["title"].as_str().unwrap_or("Playlist").to_string(),
        kind: CollectionKind::Playlist,
        items,
        truncated,
    }
}

// ---------- FTP folders ----------

async fn list_ftp(root: &Url) -> Result<Collection> {
    let mut ftp = crate::ftp::connect(root).await?;
    let root_path = crate::ftp::ftp_path(root).trim_end_matches('/').to_string();
    let title = root_path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(root.host_str().unwrap_or("FTP")).to_string();

    let mut items = Vec::new();
    let mut truncated = false;
    let mut queue = VecDeque::from([(root_path.clone(), String::new(), 0usize)]);
    while let Some((path, folder, depth)) = queue.pop_front() {
        let dir = if path.is_empty() { "/".to_string() } else { path.clone() };
        for entry in crate::ftp::list_dir(&mut ftp, &dir).await? {
            let child = format!("{}/{}", path, entry.name);
            let child_folder = if folder.is_empty() { entry.name.clone() } else { format!("{folder}/{}", entry.name) };
            if entry.is_dir {
                if depth < MAX_DEPTH {
                    queue.push_back((child, child_folder, depth + 1));
                }
                continue;
            }
            if items.len() >= MAX_ITEMS {
                truncated = true;
                break;
            }
            let mut url = root.clone();
            url.set_path(&child);
            items.push(CollectionItem {
                url: url.to_string(),
                name: entry.name,
                folder: folder.clone(),
                size: Some(entry.size),
                duration: None,
                media: false,
            });
        }
    }
    let _ = ftp.quit().await;
    Ok(Collection { title, kind: CollectionKind::Folder, items, truncated })
}

// ---------- Web folder listings ----------

async fn list_web_folder(root: &Url, headers: &[(String, String)]) -> Result<Collection> {
    let mut map = reqwest::header::HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), reqwest::header::HeaderValue::from_str(v)) {
            map.insert(k, v);
        }
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("AllDownloadManager/", env!("CARGO_PKG_VERSION")))
        .default_headers(map)
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut items = Vec::new();
    let mut truncated = false;
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from([(root.clone(), String::new(), 0usize)]);
    let mut title = None;
    while let Some((url, folder, depth)) = queue.pop_front() {
        let resp = client.get(url.clone()).send().await?;
        if !resp.status().is_success() {
            if depth == 0 {
                return Err(Error::BadStatus(resp.status()));
            }
            continue;
        }
        let is_html = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|t| t.contains("html"));
        // Folder pages are addressed with a trailing slash (servers redirect to it).
        let mut base = resp.url().clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        if !is_html {
            if depth == 0 {
                return Err(Error::Tool("This link isn't a folder listing.".into()));
            }
            continue;
        }
        let html = resp.text().await?;
        if title.is_none() {
            title = Some(last_segment(&base).unwrap_or_else(|| base.host_str().unwrap_or("Folder").to_string()));
        }
        let links = folder_links(&html, &base);
        if depth == 0 && links.is_empty() {
            return Err(Error::Tool("No files or folders were found at this link.".into()));
        }
        for link in links {
            if !seen.insert(link.url.to_string()) {
                continue;
            }
            let name = last_segment(&link.url).unwrap_or_default();
            let child_folder = if folder.is_empty() { name.clone() } else { format!("{folder}/{name}") };
            if link.url.path().ends_with('/') {
                if depth < MAX_DEPTH {
                    queue.push_back((link.url, child_folder, depth + 1));
                }
                continue;
            }
            if items.len() >= MAX_ITEMS {
                truncated = true;
                break;
            }
            items.push(CollectionItem {
                url: link.url.to_string(),
                name,
                folder: folder.clone(),
                size: link.size,
                duration: None,
                media: false,
            });
        }
    }
    Ok(Collection { title: title.unwrap_or_else(|| "Folder".into()), kind: CollectionKind::Folder, items, truncated })
}

fn last_segment(url: &Url) -> Option<String> {
    let seg = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    Some(percent_decode_str(seg).decode_utf8_lossy().to_string())
}

#[derive(Debug)]
pub(crate) struct FolderLink {
    pub url: Url,
    pub size: Option<u64>,
}

/// Links inside a folder listing that point below the folder itself (not to
/// parent folders, sort links, or other sites).
pub(crate) fn folder_links(html: &str, base: &Url) -> Vec<FolderLink> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in html.split_inclusive('\n') {
        let hrefs = hrefs_in(line);
        for href in &hrefs {
            if href.starts_with('?') || href.starts_with('#') || href.starts_with("javascript:") || href.starts_with("mailto:") {
                continue;
            }
            let Ok(url) = base.join(&html_unescape(href)) else { continue };
            let below = url.host_str() == base.host_str()
                && url.path().len() > base.path().len()
                && url.path().starts_with(base.path())
                && url.query().is_none();
            if !below || !seen.insert(url.path().to_string()) {
                continue;
            }
            let name = url.path().trim_end_matches('/').rsplit('/').next().unwrap_or("").to_ascii_lowercase();
            if name.starts_with('_') && url.path().ends_with('/') && name.contains("h5ai") {
                continue;
            }
            // Sizes are only trusted on lines with a single link (Apache, nginx tables).
            let size = if hrefs.len() == 1 && !url.path().ends_with('/') { size_after_link(line) } else { None };
            out.push(FolderLink { url, size });
        }
    }
    out
}

fn hrefs_in(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = line.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find("href=") {
        let start = from + pos + 5;
        let rest = &line[start..];
        let (quote, rest) = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => (Some(q), &rest[1..]),
            _ => (None, rest),
        };
        let end = match quote {
            Some(q) => rest.find(q),
            None => rest.find(|c: char| c.is_whitespace() || c == '>'),
        }
        .unwrap_or(rest.len());
        out.push(rest[..end].to_string());
        from = start + end.max(1);
        if from >= line.len() {
            break;
        }
    }
    out
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&#39;", "'").replace("&quot;", "\"")
}

/// The size shown after the link on a listing line, such as "12M", "1.5G" or "4096".
fn size_after_link(line: &str) -> Option<u64> {
    let after = &line[line.to_ascii_lowercase().rfind("</a>")? + 4..];
    let mut text = String::new();
    let mut in_tag = false;
    for c in after.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                text.push(' ');
            }
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    let token = text.split_whitespace().rev().find(|t| *t != "-")?;
    parse_size(token)
}

fn parse_size(token: &str) -> Option<u64> {
    let t = token.trim().trim_end_matches(['B', 'b', 'i']);
    let (num, mult) = match t.chars().last()? {
        'K' | 'k' => (&t[..t.len() - 1], 1024f64),
        'M' | 'm' => (&t[..t.len() - 1], 1024f64.powi(2)),
        'G' | 'g' => (&t[..t.len() - 1], 1024f64.powi(3)),
        'T' | 't' => (&t[..t.len() - 1], 1024f64.powi(4)),
        _ => (t, 1.0),
    };
    let value: f64 = num.trim().parse().ok()?;
    (value >= 0.0).then_some((value * mult) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_collections() {
        assert!(looks_like_collection("https://www.youtube.com/playlist?list=PL123"));
        assert!(looks_like_collection("https://www.youtube.com/watch?v=x&list=PL123"));
        assert!(looks_like_collection("https://www.youtube.com/@channel"));
        assert!(!looks_like_collection("https://www.youtube.com/watch?v=x"));
        assert!(looks_like_collection("http://server.local/Movies/2024/"));
        assert!(!looks_like_collection("http://server.local/Movies/film.mkv"));
        assert!(looks_like_collection("ftp://ftp.example.com/pub/"));
        assert!(looks_like_collection("ftp://ftp.example.com/pub/linux"));
        assert!(!looks_like_collection("ftp://ftp.example.com/pub/a.iso"));
    }

    #[test]
    fn reads_apache_listing() {
        let base = Url::parse("http://server.local/Movies/").unwrap();
        let html = r#"<html><body><h1>Index of /Movies</h1><table>
<tr><th><a href="?C=N;O=D">Name</a></th><th><a href="?C=S;O=A">Size</a></th></tr>
<tr><td><a href="/">Parent Directory</a></td><td>-</td></tr>
<tr><td><a href="Season%201/">Season 1/</a></td><td>2024-01-01 10:00</td><td>-</td></tr>
<tr><td><a href="The%20Film%20(2024).mkv">The Film (2024).mkv</a></td><td>2024-01-01 10:00</td><td align="right">1.4G</td></tr>
<tr><td><a href="poster.jpg">poster.jpg</a></td><td>2024-01-01 10:00</td><td align="right">512K</td></tr>
<tr><td><a href="http://other.site/x.zip">elsewhere</a></td></tr>
</table></body></html>"#;
        let links = folder_links(html, &base);
        let paths: Vec<_> = links.iter().map(|l| l.url.path()).collect();
        assert_eq!(paths, ["/Movies/Season%201/", "/Movies/The%20Film%20(2024).mkv", "/Movies/poster.jpg"]);
        assert_eq!(links[0].size, None);
        assert_eq!(links[1].size, Some((1.4 * 1024f64.powi(3)) as u64));
        assert_eq!(links[2].size, Some(512 * 1024));
    }

    #[test]
    fn reads_nginx_listing() {
        let base = Url::parse("http://server.local/files/").unwrap();
        let html = "<pre><a href=\"../\">../</a>\n<a href=\"sub/\">sub/</a>                 01-Jan-2024 10:00       -\n<a href=\"a.zip\">a.zip</a>                01-Jan-2024 10:00   1048576\n</pre>";
        let links = folder_links(html, &base);
        assert_eq!(links.len(), 2);
        assert_eq!(links[1].size, Some(1_048_576));
    }

    #[test]
    fn reads_flat_playlist() {
        let json = serde_json::json!({
            "_type": "playlist", "title": "Rust Course", "playlist_count": 3,
            "entries": [
                {"_type": "url", "ie_key": "Youtube", "id": "aaa", "url": "https://www.youtube.com/watch?v=aaa", "title": "Lesson 1", "duration": 600.0},
                {"_type": "url", "ie_key": "Youtube", "id": "bbb", "url": "bbb", "title": "Lesson 2", "duration": 720.0},
                {"_type": "url", "ie_key": "Youtube", "id": "ccc", "url": "https://www.youtube.com/watch?v=ccc", "title": "[Private video]"}
            ]
        });
        let c = playlist_from_json(&json);
        assert_eq!(c.title, "Rust Course");
        assert_eq!(c.items.len(), 2);
        assert_eq!(c.items[1].url, "https://www.youtube.com/watch?v=bbb");
        assert!(c.items.iter().all(|i| i.media));
        assert!(!c.truncated);
    }
}
