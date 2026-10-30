use reqwest::header::{
    HeaderMap, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG,
    LAST_MODIFIED, RANGE,
};
use reqwest::{Client, StatusCode, Url};

use crate::filename;
use crate::{Error, Result};

/// What the server told us about a file before downloading it.
#[derive(Debug, Clone)]
pub struct ProbeInfo {
    /// URL after following redirects.
    pub final_url: Url,
    /// Total size in bytes, when the server reports it.
    pub size: Option<u64>,
    /// Whether the server accepts `Range` requests (needed for parallel
    /// connections and for resuming).
    pub resumable: bool,
    /// Suggested file name (not yet sanitized).
    pub filename: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Asks the server for the first byte of the file. This works on servers that
/// reject `HEAD`, and tells us both the size and whether ranges work.
pub async fn probe(client: &Client, url: &str) -> Result<ProbeInfo> {
    let resp = client.get(url).header(RANGE, "bytes=0-0").send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::BadStatus(status));
    }
    let headers = resp.headers().clone();
    let final_url = resp.url().clone();
    // Don't download the body when the server ignored the range.
    drop(resp);

    let (size, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        (total_from_content_range(&headers), true)
    } else {
        // A 200 means the server ignored our range, whatever Accept-Ranges says.
        let len = header_str(&headers, CONTENT_LENGTH).and_then(|v| v.parse().ok());
        (len, false)
    };

    let name = header_str(&headers, CONTENT_DISPOSITION)
        .and_then(filename::from_content_disposition)
        .or_else(|| filename::from_url(&final_url))
        .unwrap_or_else(|| "download".to_string());

    Ok(ProbeInfo {
        final_url,
        size,
        resumable: resumable && size.is_some(),
        filename: name,
        etag: header_str(&headers, ETAG).map(str::to_string),
        last_modified: header_str(&headers, LAST_MODIFIED).map(str::to_string),
    })
}

fn header_str(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Parses `bytes 0-0/12345` into `12345`.
fn total_from_content_range(headers: &HeaderMap) -> Option<u64> {
    header_str(headers, CONTENT_RANGE)?
        .rsplit_once('/')?
        .1
        .trim()
        .parse()
        .ok()
}
