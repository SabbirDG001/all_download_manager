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
use crate::{Error, Settings, SpeedMeter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
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
}

/// What the list's listeners are told.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase")]
pub enum Change {
    /// The whole list, after something was added, removed or finished.
    List(Vec<Entry>),
    /// Progress of one download.
    Updated(Entry),
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
}

struct Running {
    cancel: CancellationToken,
    /// Set when the entry is removed with "delete file" while running.
    delete_files: bool,
    /// File name in use, for cleaning up the partial file.
    filename: Option<String>,
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

        let manager = Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner { entries, next_id, running: HashMap::new(), settings }),
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

    /// Uses new settings for downloads started from now on.
    pub fn set_settings(&self, settings: Settings) {
        self.changed(|inner| {
            inner.settings = settings;
            self.schedule(inner);
        });
    }

    pub fn add(&self, req: AddRequest) -> crate::Result<u64> {
        let url = req.url.trim().to_string();
        let parsed = reqwest::Url::parse(&url).map_err(|_| Error::Settings(format!("not a valid link: {url}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::Settings("only http and https links are supported".into()));
        }
        let mut id = 0;
        self.changed(|inner| {
            id = inner.next_id;
            inner.next_id += 1;
            inner.entries.push(Entry {
                id,
                url,
                filename: req.filename.filter(|f| !f.trim().is_empty()),
                dir: req.dir.clone().unwrap_or_else(|| inner.settings.download_dir.clone()),
                status: if req.paused { Status::Paused } else { Status::Queued },
                downloaded: 0,
                total: None,
                resumable: false,
                path: None,
                error: None,
                added_at: SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
                headers: req.headers,
                speed: 0.0,
                connections: 0,
            });
            self.schedule(inner);
        });
        Ok(id)
    }

    pub fn pause(&self, ids: &[u64]) {
        self.changed(|inner| {
            for &id in ids {
                let Some(e) = inner.entries.iter_mut().find(|e| e.id == id) else { continue };
                if matches!(e.status, Status::Queued | Status::Downloading) {
                    e.status = Status::Paused;
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
                if matches!(e.status, Status::Paused | Status::Failed) {
                    e.status = Status::Queued;
                    e.error = None;
                }
            }
            self.schedule(inner);
        });
    }

    pub fn pause_all(&self) {
        let ids: Vec<u64> = self.entries().iter().map(|e| e.id).collect();
        self.pause(&ids);
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
        let snapshot = {
            let mut inner = self.shared.inner.lock().unwrap();
            f(&mut inner);
            inner.entries.clone()
        };
        self.save(&snapshot);
        (self.shared.listener)(Change::List(snapshot));
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
                .find(|e| e.status == Status::Queued && !inner.running.contains_key(&e.id))
                .map(|e| e.id);
            match next {
                Some(id) => self.start(inner, id),
                None => break,
            }
        }
    }

    fn start(&self, inner: &mut Inner, id: u64) {
        let settings = inner.settings.clone();
        let Some(entry) = inner.entries.iter_mut().find(|e| e.id == id) else { return };
        entry.status = Status::Downloading;
        entry.error = None;

        let opts = DownloadOptions {
            dir: entry.dir.clone(),
            temp_dir: settings.temp_dir.clone(),
            filename: entry.filename.clone(),
            connections: settings.connections,
            headers: entry.headers.clone(),
            user_agent: settings.user_agent.clone(),
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
                speed: SpeedMeter::default(),
            },
        );

        let this = self.clone();
        self.shared.runtime.spawn(async move {
            let events = this.clone();
            let result =
                download_with_events(&url, &opts, move |ev| events.on_event(id, ev), cancel).await;
            this.finish(id, result, &opts);
        });
    }

    fn on_event(&self, id: u64, event: Event) {
        let updated = {
            let mut inner = self.shared.inner.lock().unwrap();
            let Inner { entries, running, .. } = &mut *inner;
            if let (Event::Started { filename, .. }, Some(r)) = (&event, running.get_mut(&id)) {
                r.filename = Some(filename.clone());
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
                Event::Progress(p) => {
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
        (self.shared.listener)(Change::Updated(updated));
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
                    }
                }
                Some(entry) => {
                    entry.speed = 0.0;
                    entry.connections = 0;
                    match result {
                        Ok(path) => {
                            entry.status = Status::Completed;
                            if let Some(total) = entry.total {
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

fn delete_entry_files(entry: &Entry, temp_dir: Option<&Path>) {
    if let Some(path) = &entry.path {
        let _ = std::fs::remove_file(path);
    }
    if let Some(name) = &entry.filename {
        delete_part_files(temp_dir.unwrap_or(&entry.dir), name);
    }
}

fn delete_part_files(work_dir: &Path, name: &str) {
    let (part, meta) = part_paths(work_dir, name);
    let _ = std::fs::remove_file(part);
    let _ = std::fs::remove_file(meta);
}
