//! The download list: a queue of downloads, a few running at a time, saved to
//! disk so it survives restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

use crate::download::{download_with_events, part_paths, DownloadOptions, Event};
use crate::ftp::download_ftp;
use crate::media::{delete_partial_media, download_media, MediaFormat, MediaJob};
use crate::tools::find_tools;
use crate::limit::RateLimiter;
use crate::{Error, Quality, Settings, SpeedMeter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
    /// Waiting for the scheduler's time window.
    Scheduled,
}

/// How a download is fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A plain file, with the engine's parallel connections.
    #[default]
    File,
    /// A video or audio page or stream, through yt-dlp.
    Media,
}

/// One row of the download list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: u64,
    pub url: String,
    /// Known once the server has answered (or when chosen by the user).
    pub filename: Option<String>,
    /// Folder the finished file goes to.
    pub dir: PathBuf,
    pub status: Status,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Whether the server supports pausing and resuming this download.
    pub resumable: bool,
    /// Where the finished file ended up.
    pub path: Option<PathBuf>,
    pub error: Option<String>,
    /// Seconds since 1970.
    pub added_at: u64,
    /// Request headers from the browser, such as cookies.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub kind: Kind,
    /// Belongs to the scheduler: it runs only while the schedule is active.
    #[serde(default)]
    pub in_schedule: bool,
    /// The folder wasn't chosen by the user, so it may be sorted by type.
    #[serde(default)]
    pub auto_dir: bool,
    /// Video/audio downloads: chosen quality (older lists only have this).
    #[serde(default)]
    pub quality: Quality,
    /// Video/audio downloads: the exact choice from the quality picker.
    #[serde(default)]
    pub format: Option<MediaFormat>,
    /// Short name of the choice, such as "1080p" or "MP3 320 kbps".
    #[serde(default)]
    pub format_label: Option<String>,
    /// Video/audio downloads: the media's title and id, once known.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub media_id: Option<String>,
    /// A step after downloading, such as "Merging".
    #[serde(skip_deserializing)]
    pub stage: Option<String>,
    /// Bytes per second, while downloading.
    #[serde(skip_deserializing)]
    pub speed: f64,
    #[serde(skip_deserializing)]
    pub connections: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AddRequest {
    pub url: String,
    /// Defaults to the download folder from settings.
    pub dir: Option<PathBuf>,
    pub filename: Option<String>,
    pub headers: Vec<(String, String)>,
    /// Add it paused instead of queuing it.
    pub paused: bool,
    /// Fetch it with yt-dlp as video/audio.
    pub media: bool,
    /// Video/audio quality; the default from settings when unset.
    pub quality: Option<Quality>,
    /// Exact choice from the quality picker; overrides `quality`.
    pub format: Option<MediaFormat>,
    /// Title of the page the link came from, shown until the real one is known.
    pub title: Option<String>,
    /// Add it to the scheduler instead of starting it.
    pub scheduled: bool,
    /// Folder below `dir` (or the download folder), "/"-separated: playlists
    /// and folders keep their structure.
    pub subfolder: Option<String>,
}

/// What the list's listeners are told.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum Change {
    /// The whole list, after something was added, removed or finished.
    List(Vec<Entry>),
    /// Progress of one download.
    Updated(Box<Entry>),
    /// The scheduled downloads are finished; time for the "when done" action.
    ScheduleDone,
}

type Listener = dyn Fn(Change) + Send + Sync;

/// Handle to the download list. Cheap to clone.
#[derive(Clone)]
pub struct Manager {
    shared: Arc<Shared>,
}

struct Shared {
    inner: Mutex<Inner>,
    listener: Box<Listener>,
    runtime: Handle,
    store_path: PathBuf,
}

struct Inner {
    entries: Vec<Entry>,
    next_id: u64,
    running: HashMap<u64, Running>,
    settings: Settings,
    /// Where yt-dlp and ffmpeg are installed.
    tools_dir: PathBuf,
    limiter: Arc<RateLimiter>,
    /// Scheduled downloads may run now.
    schedule_active: bool,
    /// A scheduled download started during this active period.
    schedule_ran: bool,
}

struct Running {
    cancel: CancellationToken,
    /// Set when the entry is removed with "delete file" while running.
    delete_files: bool,
    /// File name in use, for cleaning up the partial file.
    filename: Option<String>,
    /// Video/audio downloads: id used in the partial files' names.
    media_id: Option<String>,
    speed: SpeedMeter,
}

impl Manager {
    /// Loads the list from `store_path` and starts queued downloads on `runtime`.
    pub fn new(
        settings: Settings,
        store_path: PathBuf,
        runtime: Handle,
        listener: impl Fn(Change) + Send + Sync + 'static,
    ) -> Self {
        let mut entries: Vec<Entry> = std::fs::read(&store_path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        // Downloads that were running when the app closed come back paused.
        for e in &mut entries {
            if e.status == Status::Downloading {
                e.status = Status::Paused;
            }
        }
        let next_id = entries.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let limiter = Arc::new(RateLimiter::new(settings.speed_limit_bytes()));

        let manager = Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    entries,
                    next_id,
                    running: HashMap::new(),
                    settings,
                    tools_dir: Settings::tools_dir(),
                    limiter,
                    schedule_active: false,
                    schedule_ran: false,
                }),
                listener: Box::new(listener),
                runtime,
                store_path,
            }),
        };
        manager.changed(|inner| manager.schedule(inner));
        manager
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.shared.inner.lock().unwrap().entries.clone()
    }

    pub fn settings(&self) -> Settings {
        self.shared.inner.lock().unwrap().settings.clone()
    }

    /// Uses yt-dlp and ffmpeg from `dir` (default: the data folder's `tools`).
    pub fn set_tools_dir(&self, dir: PathBuf) {
        self.shared.inner.lock().unwrap().tools_dir = dir;
    }

    /// Uses new settings for downloads started from now on. The speed limit
    /// also applies to running downloads at once.
    pub fn set_settings(&self, settings: Settings) {
        self.changed(|inner| {
            inner.limiter.set(settings.speed_limit_bytes());
            inner.settings = settings;
            self.schedule(inner);
        });
    }

    pub fn add(&self, req: AddRequest) -> crate::Result<u64> {
        Ok(self.add_many(vec![req])?[0])
    }

    /// Adds several downloads at once (a playlist or folder), saving the list once.
    /// Nothing is added if any link is invalid.
    pub fn add_many(&self, reqs: Vec<AddRequest>) -> crate::Result<Vec<u64>> {
        for req in &reqs {
            let url = req.url.trim();
            let parsed = reqwest::Url::parse(url).map_err(|_| Error::Settings(format!("not a valid link: {url}")))?;
            if !matches!(parsed.scheme(), "http" | "https" | "ftp") {
                return Err(Error::Settings("only http, https and ftp links are supported".into()));
            }
        }
        let mut ids = Vec::with_capacity(reqs.len());
        self.changed(|inner| {
            for req in reqs {
                let id = inner.next_id;
                inner.next_id += 1;
                ids.push(id);
                let entry = new_entry(id, req, &inner.settings);
                inner.entries.push(entry);
            }
            self.schedule(inner);
        });
        Ok(ids)
    }

    pub fn pause(&self, ids: &[u64]) {
        self.changed(|inner| {
            for &id in ids {
                let Some(e) = inner.entries.iter_mut().find(|e| e.id == id) else { continue };
                if matches!(e.status, Status::Queued | Status::Downloading | Status::Scheduled) {
                    e.status = Status::Paused;
                    e.in_schedule = false;
                    e.speed = 0.0;
                    e.connections = 0;
                }
                if let Some(r) = inner.running.get(&id) {
                    r.cancel.cancel();
                }
            }
            self.schedule(inner);
        });
    }

    /// Queues paused or failed downloads again.
    pub fn resume(&self, ids: &[u64]) {
        self.changed(|inner| {
            for e in inner.entries.iter_mut().filter(|e| ids.contains(&e.id)) {
                if matches!(e.status, Status::Paused | Status::Failed | Status::Scheduled) {
                    e.status = Status::Queued;
                    e.in_schedule = false;
                    e.error = None;
                }
            }
            self.schedule(inner);
        });
    }

    /// Pauses everything that is running or waiting (the scheduler's
    /// downloads stay with the scheduler).
    pub fn pause_all(&self) {
        let ids: Vec<u64> = self
            .entries()
            .iter()
            .filter(|e| matches!(e.status, Status::Queued | Status::Downloading) && !e.in_schedule)
            .map(|e| e.id)
            .collect();
        self.pause(&ids);
    }

    /// Hands downloads to the scheduler. Running ones stop until it's time.
    pub fn add_to_schedule(&self, ids: &[u64]) {
        self.changed(|inner| {
            let active = inner.schedule_active;
            for e in inner.entries.iter_mut().filter(|e| ids.contains(&e.id)) {
                if e.status == Status::Completed {
                    continue;
                }
                e.in_schedule = true;
                e.error = None;
                if e.status == Status::Downloading && active {
                    continue;
                }
                e.status = Status::Scheduled;
                e.speed = 0.0;
                if let Some(r) = inner.running.get(&e.id) {
                    r.cancel.cancel();
                }
            }
            self.schedule(inner);
        });
    }

    /// Lets the scheduler's downloads run (`true`) or stops them (`false`).
    pub fn set_schedule_active(&self, active: bool) {
        self.changed(|inner| {
            if inner.schedule_active == active {
                return;
            }
            inner.schedule_active = active;
            inner.schedule_ran = false;
            if !active {
                let Inner { entries, running, .. } = &mut *inner;
                for e in entries.iter_mut().filter(|e| e.in_schedule && e.status == Status::Downloading) {
                    e.status = Status::Scheduled;
                    e.speed = 0.0;
                    if let Some(r) = running.get(&e.id) {
                        r.cancel.cancel();
                    }
                }
            }
            self.schedule(inner);
        });
    }

    pub fn schedule_active(&self) -> bool {
        self.shared.inner.lock().unwrap().schedule_active
    }

    pub fn resume_all(&self) {
        let ids: Vec<u64> = self.entries().iter().map(|e| e.id).collect();
        self.resume(&ids);
    }

    /// Removes entries from the list, and optionally their files from disk.
    pub fn remove(&self, ids: &[u64], delete_files: bool) {
        self.changed(|inner| {
            let temp_dir = inner.settings.temp_dir.clone();
            for &id in ids {
                let Some(pos) = inner.entries.iter().position(|e| e.id == id) else { continue };
                let entry = inner.entries.remove(pos);
                if let Some(r) = inner.running.get_mut(&id) {
                    // The download task cleans up once it has stopped.
                    r.delete_files = delete_files;
                    r.cancel.cancel();
                } else if delete_files {
                    delete_entry_files(&entry, temp_dir.as_deref());
                }
            }
            self.schedule(inner);
        });
    }

    /// Removes finished downloads from the list (the files stay).
    pub fn clear_completed(&self) {
        self.changed(|inner| inner.entries.retain(|e| e.status != Status::Completed));
    }

    /// Runs `f` on the list, then saves it and tells the listener.
    fn changed(&self, f: impl FnOnce(&mut Inner)) {
        let (snapshot, schedule_done) = {
            let mut inner = self.shared.inner.lock().unwrap();
            f(&mut inner);
            let done = self.check_schedule_done(&mut inner);
            (inner.entries.clone(), done)
        };
        self.save(&snapshot);
        (self.shared.listener)(Change::List(snapshot));
        if schedule_done {
            (self.shared.listener)(Change::ScheduleDone);
        }
    }

    /// Ends the active period once the scheduler's downloads are all done.
    /// Returns whether something ran, so the "when done" action applies.
    fn check_schedule_done(&self, inner: &mut Inner) -> bool {
        if !inner.schedule_active {
            return false;
        }
        let waiting = inner
            .entries
            .iter()
            .any(|e| e.in_schedule && matches!(e.status, Status::Scheduled | Status::Downloading));
        if waiting {
            return false;
        }
        inner.schedule_active = false;
        std::mem::take(&mut inner.schedule_ran)
    }

    fn save(&self, entries: &[Entry]) {
        let path = &self.shared.store_path;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(json) = serde_json::to_vec_pretty(entries) else { return };
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// Starts queued downloads while there is room.
    fn schedule(&self, inner: &mut Inner) {
        while inner.running.len() < inner.settings.max_active {
            let next = inner
                .entries
                .iter()
                .find(|e| {
                    let ready = e.status == Status::Queued || (e.status == Status::Scheduled && inner.schedule_active);
                    ready && !inner.running.contains_key(&e.id)
                })
                .map(|e| e.id);
            match next {
                Some(id) => self.start(inner, id),
                None => break,
            }
        }
    }

    fn start(&self, inner: &mut Inner, id: u64) {
        let settings = inner.settings.clone();
        let tools_dir = inner.tools_dir.clone();
        let limiter = inner.limiter.clone();
        let Some(entry) = inner.entries.iter_mut().find(|e| e.id == id) else { return };
        if entry.in_schedule {
            inner.schedule_ran = true;
        }
        entry.status = Status::Downloading;
        entry.error = None;
        entry.stage = None;

        let opts = DownloadOptions {
            dir: entry.dir.clone(),
            temp_dir: settings.temp_dir.clone(),
            filename: entry.filename.clone(),
            connections: settings.connections,
            headers: entry.headers.clone(),
            user_agent: settings.user_agent.clone(),
            limiter: Some(limiter),
            sort_into_folders: settings.sort_into_folders && entry.auto_dir,
            ..Default::default()
        };
        let url = entry.url.clone();
        let cancel = CancellationToken::new();
        inner.running.insert(
            id,
            Running {
                cancel: cancel.clone(),
                delete_files: false,
                filename: opts.filename.clone(),
                media_id: entry.media_id.clone(),
                speed: SpeedMeter::default(),
            },
        );

        let this = self.clone();
        if entry.kind == Kind::Media {
            let format = entry.format.unwrap_or_else(|| entry.quality.into());
            let mut dir = opts.dir.clone();
            if opts.sort_into_folders {
                dir.push(if format.is_audio() { "Music" } else { "Video" });
            }
            let job = MediaJob {
                url,
                dir,
                temp_dir: opts.temp_dir.clone(),
                format,
                headers: opts.headers.clone(),
                title_hint: entry.title.clone(),
                rate_limit: Some(settings.speed_limit_bytes()),
            };
            self.shared.runtime.spawn(async move {
                let tools = find_tools(&tools_dir);
                let events = this.clone();
                let result = download_media(&job, &tools, move |ev| events.on_event(id, ev), cancel).await;
                this.finish(id, result, &opts);
            });
        } else if url.starts_with("ftp://") {
            self.shared.runtime.spawn(async move {
                let events = this.clone();
                let result = download_ftp(&url, &opts, move |ev| events.on_event(id, ev), cancel).await;
                this.finish(id, result, &opts);
            });
        } else {
            self.shared.runtime.spawn(async move {
                let events = this.clone();
                let result =
                    download_with_events(&url, &opts, move |ev| events.on_event(id, ev), cancel).await;
                this.finish(id, result, &opts);
            });
        }
    }

    fn on_event(&self, id: u64, event: Event) {
        let updated = {
            let mut inner = self.shared.inner.lock().unwrap();
            let Inner { entries, running, .. } = &mut *inner;
            match (&event, running.get_mut(&id)) {
                (Event::Started { filename, .. }, Some(r)) => r.filename = Some(filename.clone()),
                (Event::Media { id, .. }, Some(r)) => r.media_id = Some(id.clone()),
                _ => {}
            }
            let Some(entry) = entries.iter_mut().find(|e| e.id == id) else { return };
            if entry.status != Status::Downloading {
                return;
            }
            match event {
                Event::Started { filename, total, resumable } => {
                    entry.filename = Some(filename);
                    entry.total = total;
                    entry.resumable = resumable;
                }
                Event::Media { id, title } => {
                    entry.media_id = Some(id);
                    entry.title = Some(title);
                }
                Event::Stage(stage) => {
                    entry.stage = Some(stage);
                    entry.speed = 0.0;
                }
                Event::Progress(p) => {
                    entry.stage = None;
                    entry.downloaded = p.downloaded;
                    entry.total = p.total;
                    entry.connections = p.connections;
                    if let Some(r) = running.get_mut(&id) {
                        entry.speed = r.speed.update(p.downloaded);
                    }
                }
            }
            entry.clone()
        };
        (self.shared.listener)(Change::Updated(Box::new(updated)));
    }

    fn finish(&self, id: u64, result: crate::Result<PathBuf>, opts: &DownloadOptions) {
        self.changed(|inner| {
            let running = inner.running.remove(&id);
            match inner.entries.iter_mut().find(|e| e.id == id) {
                None => {
                    // Removed while running.
                    if let Some(r) = running.filter(|r| r.delete_files) {
                        if let Ok(path) = &result {
                            let _ = std::fs::remove_file(path);
                        }
                        if let Some(name) = &r.filename {
                            delete_part_files(opts.work_dir(), name);
                        }
                        if let Some(media_id) = &r.media_id {
                            delete_partial_media(&[&opts.dir, opts.work_dir()], media_id);
                        }
                    }
                }
                Some(entry) => {
                    entry.speed = 0.0;
                    entry.connections = 0;
                    entry.stage = None;
                    match result {
                        Ok(path) => {
                            entry.status = Status::Completed;
                            if entry.kind == Kind::Media {
                                // The size of the merged or converted file.
                                if let Ok(meta) = std::fs::metadata(&path) {
                                    entry.downloaded = meta.len();
                                    entry.total = Some(meta.len());
                                }
                                entry.filename = path.file_name().map(|n| n.to_string_lossy().to_string());
                            } else if let Some(total) = entry.total {
                                entry.downloaded = total;
                            }
                            entry.path = Some(path);
                        }
                        Err(Error::Cancelled) => {
                            // Paused, or paused and already resumed again.
                            if entry.status == Status::Downloading {
                                entry.status = Status::Paused;
                            }
                        }
                        Err(e) => {
                            entry.status = Status::Failed;
                            entry.error = Some(e.to_string());
                        }
                    }
                }
            }
            self.schedule(inner);
        });
    }
}

fn new_entry(id: u64, req: AddRequest, settings: &Settings) -> Entry {
    let mut dir = req.dir.clone().unwrap_or_else(|| settings.download_dir.clone());
    // Playlists and folders keep their own folder structure.
    if let Some(sub) = req.subfolder.as_deref() {
        for part in sub.split(['/', '\\']).filter(|p| !p.trim().is_empty()) {
            dir.push(crate::sanitize_filename(part));
        }
    }
    let format = req.format.unwrap_or_else(|| req.quality.unwrap_or(settings.video_quality).into());
    Entry {
        id,
        url: req.url.trim().to_string(),
        filename: req.filename.filter(|f| !f.trim().is_empty()),
        dir,
        status: if req.scheduled {
            Status::Scheduled
        } else if req.paused {
            Status::Paused
        } else {
            Status::Queued
        },
        in_schedule: req.scheduled,
        auto_dir: req.dir.is_none() && req.subfolder.is_none(),
        downloaded: 0,
        total: None,
        resumable: false,
        path: None,
        error: None,
        added_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        headers: req.headers,
        kind: if req.media { Kind::Media } else { Kind::File },
        quality: req.quality.unwrap_or(settings.video_quality),
        format: req.format,
        format_label: req.media.then(|| format.label()),
        title: req.title.filter(|t| !t.trim().is_empty()),
        media_id: None,
        stage: None,
        speed: 0.0,
        connections: 0,
    }
}

fn delete_entry_files(entry: &Entry, temp_dir: Option<&Path>) {
    if let Some(path) = &entry.path {
        let _ = std::fs::remove_file(path);
    }
    if let Some(name) = &entry.filename {
        delete_part_files(temp_dir.unwrap_or(&entry.dir), name);
    }
    if let Some(media_id) = &entry.media_id {
        delete_partial_media(&[&entry.dir, temp_dir.unwrap_or(&entry.dir)], media_id);
    }
}

fn delete_part_files(work_dir: &Path, name: &str) {
    let (part, meta) = part_paths(work_dir, name);
    let _ = std::fs::remove_file(part);
    let _ = std::fs::remove_file(meta);
}
