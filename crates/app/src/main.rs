// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;

use std::path::PathBuf;
use std::sync::Mutex;

use adm_engine::{
    install_ffmpeg, install_yt_dlp, looks_like_media_page, tools_status, AddRequest, CancellationToken, Entry,
    Manager, Progress, Settings, ToolsStatus,
};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager as _, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

type CmdResult<T> = Result<T, String>;

#[tauri::command]
fn list(manager: State<Manager>) -> Vec<Entry> {
    manager.entries()
}

#[tauri::command]
fn add(manager: State<Manager>, request: AddRequest) -> CmdResult<u64> {
    manager.add(request).map_err(|e| e.to_string())
}

#[tauri::command]
fn pause(manager: State<Manager>, ids: Vec<u64>) {
    manager.pause(&ids);
}

#[tauri::command]
fn resume(manager: State<Manager>, ids: Vec<u64>) {
    manager.resume(&ids);
}

#[tauri::command]
fn remove(manager: State<Manager>, ids: Vec<u64>, delete_files: bool) {
    manager.remove(&ids, delete_files);
}

#[tauri::command]
fn pause_all(manager: State<Manager>) {
    manager.pause_all();
}

#[tauri::command]
fn resume_all(manager: State<Manager>) {
    manager.resume_all();
}

#[tauri::command]
fn clear_completed(manager: State<Manager>) {
    manager.clear_completed();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsInfo {
    settings: Settings,
    /// Where the settings file is.
    path: PathBuf,
    portable: bool,
}

#[tauri::command]
fn get_settings(manager: State<Manager>) -> SettingsInfo {
    SettingsInfo { settings: manager.settings(), path: Settings::path(), portable: Settings::is_portable() }
}

#[tauri::command]
fn save_settings(manager: State<Manager>, settings: Settings) -> CmdResult<()> {
    settings.save().map_err(|e| e.to_string())?;
    manager.set_settings(settings);
    Ok(())
}

#[tauri::command]
async fn get_tools_status() -> ToolsStatus {
    tools_status(&Settings::tools_dir()).await
}

/// Installs or updates yt-dlp or ffmpeg, reporting "tool-progress" events.
#[tauri::command]
async fn install_tool(app: AppHandle, name: String) -> CmdResult<()> {
    let dir = Settings::tools_dir();
    let tool = name.clone();
    let on_progress = move |p: Progress| {
        let _ = app.emit("tool-progress", json!({ "name": tool, "downloaded": p.downloaded, "total": p.total }));
    };
    let result = match name.as_str() {
        "yt-dlp" => install_yt_dlp(&dir, on_progress, CancellationToken::new()).await,
        "ffmpeg" => install_ffmpeg(&dir, on_progress, CancellationToken::new()).await,
        other => return Err(format!("unknown tool '{other}'")),
    };
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// Whether a link should go to yt-dlp (video sites, stream playlists).
#[tauri::command]
fn looks_like_media(url: String) -> bool {
    looks_like_media_page(&url)
}

/// Shows the folder picker; returns `None` when cancelled.
#[tauri::command]
async fn pick_folder(app: AppHandle, start: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file();
    if let Some(start) = start.filter(|s| !s.is_empty()) {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let folder = rx.await.ok().flatten()?;
    folder.into_path().ok().map(|p| p.display().to_string())
}

#[tauri::command]
fn open_file(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_in_folder(app: AppHandle, path: String) -> CmdResult<()> {
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

/// A download sent by the browser, waiting for the user to confirm it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserDownload {
    url: String,
    filename: Option<String>,
    media: bool,
    title: String,
    referrer: String,
    headers: Vec<(String, String)>,
}

#[derive(Default)]
struct PendingFromBrowser(Mutex<Vec<BrowserDownload>>);

/// Hands the frontend the downloads that arrived from the browser.
#[tauri::command]
fn take_browser_downloads(pending: State<PendingFromBrowser>) -> Vec<BrowserDownload> {
    std::mem::take(&mut *pending.0.lock().unwrap())
}

/// Opens the folder with the browser extension, for "Load unpacked".
#[tauri::command]
fn open_extension_folder(app: AppHandle) -> CmdResult<String> {
    let dir = extension_dir(&app).ok_or("The extension folder wasn't found next to the app.")?;
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())?;
    Ok(dir.display().to_string())
}

fn extension_dir(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(res) = app.path().resource_dir() {
        candidates.push(res.join("extension"));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())) {
        candidates.push(dir.join("extension"));
    }
    // Running from a source checkout.
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extension"));
    candidates.into_iter().find(|d| d.join("manifest.json").exists()).and_then(|d| d.canonicalize().ok())
}

fn bring_to_front(window: &WebviewWindow) {
    let _ = window.show();
    let _ = window.unminimize();
    // Windows only lets a background app take focus in some cases; briefly
    // staying on top makes sure the window is seen.
    let _ = window.set_always_on_top(true);
    let _ = window.set_focus();
    let _ = window.set_always_on_top(false);
}

/// Answers a message from the browser extension.
fn handle_browser_request(app: &AppHandle, req: bridge::Request) -> Value {
    let show = || {
        if let Some(window) = app.get_webview_window("main") {
            bring_to_front(&window);
        }
    };
    match req.action.as_str() {
        "ping" => bridge::ping_reply(),
        "show" => {
            show();
            json!({ "ok": true })
        }
        "add" => {
            let manager = app.state::<Manager>();
            let filename = Some(req.filename.clone()).filter(|f| !f.is_empty());
            let media = req.media || looks_like_media_page(&req.url);
            if manager.settings().confirm_browser_downloads {
                app.state::<PendingFromBrowser>().0.lock().unwrap().push(BrowserDownload {
                    url: req.url.clone(),
                    filename,
                    media,
                    title: req.title.clone(),
                    referrer: req.referrer.clone(),
                    headers: req.headers(),
                });
                let _ = app.emit("browser-download", ());
                show();
                json!({ "ok": true })
            } else {
                let request = AddRequest {
                    url: req.url.clone(),
                    filename: filename.filter(|_| !media),
                    headers: req.headers(),
                    media,
                    title: Some(req.title.clone()),
                    ..Default::default()
                };
                match manager.add(request) {
                    Ok(_) => json!({ "ok": true }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                }
            }
        }
        other => json!({ "ok": false, "error": format!("unknown action '{other}'") }),
    }
}

fn main() {
    if bridge::is_host_launch() {
        bridge::run_host();
        return;
    }

    tauri::Builder::default()
        // A second launch just brings the open window to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                bring_to_front(&window);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let settings = Settings::load().unwrap_or_default();
            let runtime = tauri::async_runtime::block_on(async { tokio::runtime::Handle::current() });
            let handle = app.handle().clone();
            let manager = Manager::new(
                settings,
                Settings::data_dir().join("downloads.json"),
                runtime,
                move |change| {
                    let _ = handle.emit("downloads", change);
                },
            );
            app.manage(manager);
            app.manage(PendingFromBrowser::default());

            if let Err(e) = bridge::register() {
                eprintln!("couldn't register with the browsers: {e}");
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let app = handle.clone();
                if let Err(e) = bridge::serve(move |req| handle_browser_request(&app, req)).await {
                    eprintln!("browser bridge stopped: {e}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list,
            add,
            pause,
            resume,
            remove,
            pause_all,
            resume_all,
            clear_completed,
            get_settings,
            save_settings,
            pick_folder,
            open_file,
            show_in_folder,
            take_browser_downloads,
            open_extension_folder,
            get_tools_status,
            install_tool,
            looks_like_media,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start All Download Manager");
}
