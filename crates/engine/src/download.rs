use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, RANGE};
use reqwest::{Client, StatusCode};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::filename::sanitize_filename;
use crate::probe::probe;
use crate::state::{split_into_segments, DownloadState};
use crate::{Error, Result};

/// Data is collected in memory up to this size before each disk write.
const WRITE_BUFFER: usize = 512 * 1024;
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// The resume file is saved every this many progress ticks.
const SAVE_EVERY_TICKS: u32 = 4;

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Folder for the finished file.
    pub dir: PathBuf,
    /// Folder for the partial file; `dir` when unset.
    pub temp_dir: Option<PathBuf>,
    /// File name to use instead of the one the server suggests.
    pub filename: Option<String>,
    /// Parallel connections.
    pub connections: usize,
    /// Segments are never split below this size.
    pub min_split_size: u64,
    /// Extra request headers, such as `Cookie` and `Referer` from the browser.
    pub headers: Vec<(String, String)>,
    pub user_agent: Option<String>,
    /// Retries per segment for network errors.
    pub max_retries: u32,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            temp_dir: None,
            filename: None,
            connections: 8,
            min_split_size: 1024 * 1024,
            headers: Vec::new(),
            user_agent: None,
            max_retries: 5,
        }
    }
}

/// Something that happened during a download.
#[derive(Debug, Clone)]
pub enum Event {
    /// The server answered; the file name and size are now known.
    Started { filename: String, total: Option<u64>, resumable: bool },
    Progress(Progress),
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    /// `None` when the server does not report the size.
    pub total: Option<u64>,
    /// Connections currently receiving data.
    pub connections: usize,
}

/// Counters shared between the connections and the progress reporter.
#[derive(Default)]
struct Counters {
    downloaded: AtomicU64,
    active: AtomicUsize,
}

/// Downloads `url` and returns the path of the finished file.
///
/// If the partial file and its resume state from an earlier attempt are
/// found, the download continues where it stopped. `on_progress` is called
/// about four times a second. Cancelling `cancel` stops the download and
/// keeps the partial file so it can be resumed later.
pub async fn download<F>(
    url: &str,
    opts: &DownloadOptions,
    on_progress: F,
    cancel: CancellationToken,
) -> Result<PathBuf>
where
    F: Fn(Progress) + Send + Sync + 'static,
{
    let on_event = move |event| {
        if let Event::Progress(p) = event {
            on_progress(p)
        }
    };
    download_with_events(url, opts, on_event, cancel).await
}

/// Like [`download`], but also reports when the download has started.
pub async fn download_with_events<F>(
    url: &str,
    opts: &DownloadOptions,
    on_event: F,
    cancel: CancellationToken,
) -> Result<PathBuf>
where
    F: Fn(Event) + Send + Sync + 'static,
{
    let client = build_client(opts)?;
    let info = probe(&client, url).await?;
    let name = sanitize_filename(opts.filename.as_deref().unwrap_or(&info.filename));

    tokio::fs::create_dir_all(&opts.dir).await?;
    let work_dir = opts.work_dir().to_path_buf();
    tokio::fs::create_dir_all(&work_dir).await?;
    let (part, meta) = part_paths(&work_dir, &name);

    on_event(Event::Started {
        filename: name.clone(),
        total: info.size,
        resumable: info.resumable,
    });
    let on_event = Arc::new(on_event);
    let on_progress = {
        let on_event = on_event.clone();
        Arc::new(move |p| on_event(Event::Progress(p)))
    };
    let counters = Arc::new(Counters::default());
    let url = info.final_url.as_str();

    match info.size.filter(|_| info.resumable) {
        Some(size) if size > 0 => {
            let resumed = DownloadState::load(&meta).filter(|s| {
                s.size == size
                    && s.etag == info.etag
                    && s.last_modified == info.last_modified
                    && part.exists()
            });
            let state = match resumed {
                Some(state) => state,
                None => {
                    let file = tokio::fs::File::create(&part).await?;
                    file.set_len(size).await?;
                    DownloadState {
                        url: url.to_string(),
                        size,
                        etag: info.etag.clone(),
                        last_modified: info.last_modified.clone(),
                        segments: split_into_segments(size, opts.connections, opts.min_split_size),
                    }
                }
            };
            counters.downloaded.store(state.downloaded(), Ordering::Relaxed);
            state.save(&meta)?;

            let state = Arc::new(Mutex::new(state));
            let reporter = Reporter::start(
                counters.clone(),
                Some(size),
                on_progress.clone(),
                Some((state.clone(), meta.clone())),
            );
            let result =
                run_segmented(&client, url, &part, &state, &counters, opts, &cancel).await;
            reporter.stop().await;

            let snapshot = state.lock().unwrap().clone();
            snapshot.save(&meta)?;
            on_progress(progress(&counters, Some(size)));

            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            if !snapshot.is_complete() {
                return Err(result.err().unwrap_or(Error::Incomplete {
                    got: snapshot.downloaded(),
                    expected: size,
                }));
            }
        }
        _ => {
            // The server can't do ranges (or the file is empty): one connection,
            // starting from scratch.
            let reporter = Reporter::start(counters.clone(), info.size, on_progress.clone(), None);
            let result = run_single(&client, url, &part, &counters, &cancel).await;
            reporter.stop().await;
            on_progress(progress(&counters, info.size));
            result?;
            if let Some(expected) = info.size {
                let got = counters.downloaded.load(Ordering::Relaxed);
                if got != expected {
                    return Err(Error::Incomplete { got, expected });
                }
            }
        }
    }

    let target = unique_path(&opts.dir, &name);
    move_file(&part, &target).await?;
    let _ = tokio::fs::remove_file(&meta).await;
    Ok(target)
}

/// The partial file and its resume state for a download named `name`.
pub fn part_paths(work_dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    (work_dir.join(format!("{name}.admpart")), work_dir.join(format!("{name}.admpart.json")))
}

impl DownloadOptions {
    /// Folder holding the partial file while downloading.
    pub fn work_dir(&self) -> &Path {
        self.temp_dir.as_deref().unwrap_or(&self.dir)
    }
}

fn build_client(opts: &DownloadOptions) -> Result<Client> {
    let mut headers = HeaderMap::new();
    for (key, value) in &opts.headers {
        let name = HeaderName::from_bytes(key.trim().as_bytes())
            .map_err(|_| Error::InvalidHeader(key.clone()))?;
        let value = HeaderValue::from_str(value.trim())
            .map_err(|_| Error::InvalidHeader(format!("{key}: {value}")))?;
        headers.insert(name, value);
    }
    let user_agent = opts
        .user_agent
        .clone()
        .unwrap_or_else(|| concat!("AllDownloadManager/", env!("CARGO_PKG_VERSION")).to_string());
    Ok(Client::builder()
        .default_headers(headers)
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()?)
}

async fn run_segmented(
    client: &Client,
    url: &str,
    part: &Path,
    state: &Arc<Mutex<DownloadState>>,
    counters: &Arc<Counters>,
    opts: &DownloadOptions,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut workers = JoinSet::new();
    for _ in 0..opts.connections.max(1) {
        let conn = Connection {
            client: client.clone(),
            url: url.to_string(),
            part: part.to_path_buf(),
            state: state.clone(),
            counters: counters.clone(),
            cancel: cancel.clone(),
            min_split: opts.min_split_size,
            max_retries: opts.max_retries,
        };
        workers.spawn(conn.run());
    }

    // A failed connection gives its segment back, and the others pick it up.
    // An error only matters if nobody manages to finish the file.
    let mut first_error = None;
    while let Some(joined) = workers.join_next().await {
        let result = joined.unwrap_or_else(|e| Err(Error::Io(io::Error::other(e.to_string()))));
        if let Err(e) = result {
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// One parallel connection. It keeps claiming segments until none are left.
struct Connection {
    client: Client,
    url: String,
    part: PathBuf,
    state: Arc<Mutex<DownloadState>>,
    counters: Arc<Counters>,
    cancel: CancellationToken,
    min_split: u64,
    max_retries: u32,
}

impl Connection {
    async fn run(self) -> Result<()> {
        let mut file = tokio::fs::OpenOptions::new().write(true).open(&self.part).await?;
        loop {
            let Some(index) = self.state.lock().unwrap().claim(self.min_split) else {
                return Ok(());
            };
            let mut attempt = 0;
            let outcome = loop {
                match self.fetch(index, &mut file).await {
                    Ok(()) => break Ok(()),
                    Err(e) if e.is_retryable() && attempt < self.max_retries => {
                        attempt += 1;
                        let wait = Duration::from_secs(1 << attempt.min(5));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = self.cancel.cancelled() => break Err(Error::Cancelled),
                        }
                    }
                    Err(e) => break Err(e),
                }
            };
            self.state.lock().unwrap().segments[index].active = false;
            match outcome {
                Ok(()) => {}
                Err(Error::Cancelled) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }

    /// Downloads the rest of segment `index`. The segment's end may move
    /// earlier while this runs, when another connection takes over part of it.
    async fn fetch(&self, index: usize, file: &mut tokio::fs::File) -> Result<()> {
        let (start, end) = {
            let state = self.state.lock().unwrap();
            let seg = &state.segments[index];
            (seg.position(), seg.end)
        };
        if start >= end {
            return Ok(());
        }

        let request = self
            .client
            .get(&self.url)
            .header(RANGE, format!("bytes={start}-{}", end - 1))
            .send();
        let resp = tokio::select! {
            r = request => r?,
            _ = self.cancel.cancelled() => return Err(Error::Cancelled),
        };
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {}
            StatusCode::OK => return Err(Error::RangeNotHonored),
            s => return Err(Error::BadStatus(s)),
        }

        self.counters.active.fetch_add(1, Ordering::Relaxed);
        let result = self.stream_to_file(resp, index, file).await;
        self.counters.active.fetch_sub(1, Ordering::Relaxed);
        result?;

        if self.state.lock().unwrap().segments[index].is_done() {
            Ok(())
        } else {
            Err(Error::ConnectionClosed)
        }
    }

    async fn stream_to_file(
        &self,
        resp: reqwest::Response,
        index: usize,
        file: &mut tokio::fs::File,
    ) -> Result<()> {
        let mut stream = resp.bytes_stream();
        let mut buffer = Vec::with_capacity(WRITE_BUFFER);

        loop {
            let chunk = tokio::select! {
                c = stream.next() => c,
                _ = self.cancel.cancelled() => None,
            };
            let finished = match chunk {
                Some(Ok(bytes)) => {
                    buffer.extend_from_slice(&bytes);
                    false
                }
                Some(Err(e)) => {
                    // Keep what already arrived, then let the caller retry.
                    self.flush(index, file, &mut buffer).await?;
                    return Err(e.into());
                }
                None => true,
            };

            if finished || buffer.len() >= WRITE_BUFFER {
                let reached_end = self.flush(index, file, &mut buffer).await?;
                if self.cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if finished || reached_end {
                    return Ok(());
                }
            }
        }
    }

    /// Writes the buffer to disk, then records it in the segment. Bytes past
    /// the segment's (possibly moved) end are dropped: another connection owns
    /// them. Returns whether the segment is now complete.
    async fn flush(
        &self,
        index: usize,
        file: &mut tokio::fs::File,
        buffer: &mut Vec<u8>,
    ) -> Result<bool> {
        let (pos, end) = {
            let state = self.state.lock().unwrap();
            let seg = &state.segments[index];
            (seg.position(), seg.end)
        };
        let take = (buffer.len() as u64).min(end.saturating_sub(pos)) as usize;
        file.seek(SeekFrom::Start(pos)).await?;
        file.write_all(&buffer[..take]).await?;
        // Make sure the bytes really reached the file before counting them,
        // so the resume state never claims more than is on disk.
        file.flush().await?;

        // Only this connection advances this segment; another one can only
        // move its end, and never below what's downloaded, so `take` still fits.
        let reached_end = {
            let mut state = self.state.lock().unwrap();
            let seg = &mut state.segments[index];
            let take = (take as u64).min(seg.remaining());
            seg.downloaded += take;
            self.counters.downloaded.fetch_add(take, Ordering::Relaxed);
            seg.is_done()
        };
        buffer.clear();
        Ok(reached_end)
    }
}

/// Downloads with a single connection, for servers that don't support ranges.
async fn run_single(
    client: &Client,
    url: &str,
    part: &Path,
    counters: &Counters,
    cancel: &CancellationToken,
) -> Result<()> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(Error::BadStatus(resp.status()));
    }
    let mut file = tokio::io::BufWriter::with_capacity(WRITE_BUFFER, tokio::fs::File::create(part).await?);
    let mut stream = resp.bytes_stream();
    counters.active.store(1, Ordering::Relaxed);
    loop {
        let chunk = tokio::select! {
            c = stream.next() => c,
            _ = cancel.cancelled() => return Err(Error::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        counters.downloaded.fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    counters.active.store(0, Ordering::Relaxed);
    file.flush().await?;
    Ok(())
}

fn progress(counters: &Counters, total: Option<u64>) -> Progress {
    Progress {
        downloaded: counters.downloaded.load(Ordering::Relaxed),
        total,
        connections: counters.active.load(Ordering::Relaxed),
    }
}

/// Background task that reports progress and saves the resume state.
struct Reporter {
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Reporter {
    fn start<F>(
        counters: Arc<Counters>,
        total: Option<u64>,
        on_progress: Arc<F>,
        saver: Option<(Arc<Mutex<DownloadState>>, PathBuf)>,
    ) -> Self
    where
        F: Fn(Progress) + Send + Sync + 'static,
    {
        let stop = CancellationToken::new();
        let token = stop.clone();
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PROGRESS_INTERVAL);
            let mut ticks = 0u32;
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = token.cancelled() => return,
                }
                on_progress(progress(&counters, total));
                ticks += 1;
                if ticks.is_multiple_of(SAVE_EVERY_TICKS) {
                    if let Some((state, path)) = &saver {
                        let snapshot = state.lock().unwrap().clone();
                        let _ = snapshot.save(path);
                    }
                }
            }
        });
        Self { stop, task }
    }

    async fn stop(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

/// `name` in `dir`, or `name (1)`, `name (2)`… if that file already exists.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("ran out of file names")
}

/// Renames, or copies and deletes when the folders are on different drives.
async fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if tokio::fs::rename(from, to).await.is_ok() {
        return Ok(());
    }
    tokio::fs::copy(from, to).await?;
    tokio::fs::remove_file(from).await
}
