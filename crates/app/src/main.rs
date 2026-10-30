// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use adm_engine::{
    find_tools, install_ffmpeg, install_yt_dlp, list_collection, looks_like_collection, looks_like_media_page,
    probe_media, tools_status, AddRequest, CancellationToken, Collection, Entry, Manager, MediaInfo, Progress,
    Settings, ToolsStatus,
};
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager as _, State, WebviewWindow, WindowEvent};
use tauri_plugin_autostart::ManagerExt as _;
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

/// Adds a whole playlist or folder at once.
#[tauri::command]
fn add_many(manager: State<Manager>, requests: Vec<AddRequest>) -> CmdResult<usize> {
    manager.add_many(requests).map(|ids| ids.len()).map_err(|e| e.to_string())
}

/// Lists the videos of a playlist or channel, or the files of a folder (FTP or web).
#[tauri::command]
async fn get_collection(url: String, headers: Vec<(String, String)>) -> CmdResult<Collection> {
    let tools = find_tools(&Settings::tools_dir());
    list_collection(&url, &headers, &tools).await.map_err(|e| e.to_string())
}

#[tauri::command]
fn is_collection(url: String) -> bool {
    looks_like_collection(&url)
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
fn save_settings(app: AppHandle, manager: State<Manager>, settings: Settings) -> CmdResult<()> {
    settings.save().map_err(|e| e.to_string())?;
    manager.set_settings(settings);
    sync_autostart(&app);
    tray::sync(&app);
    Ok(())
}

/// One download, for the "Download complete" window.
#[tauri::command]
fn get_entry(manager: State<Manager>, id: u64) -> Option<Entry> {
    manager.entry(id)
}

#[tauri::command]
fn close_this_window(window: WebviewWindow) {
    let _ = window.close();
}

/// Turns "start with Windows" on or off to match the settings.
fn sync_autostart(app: &AppHandle) {
    let want = app.state::<Manager>().settings().start_with_windows;
    let launcher = app.autolaunch();
    if launcher.is_enabled().unwrap_or(false) != want {
        let result = if want { launcher.enable() } else { launcher.disable() };
        if let Err(e) = result {
            eprintln!("couldn't change start with Windows: {e}");
        }
    }
}

/// Shows the main window (from the tray, a second launch or the browser).
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        bring_to_front(&window);
    }
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

#[tauri::command]
fn add_to_schedule(manager: State<Manager>, ids: Vec<u64>) {
    manager.add_to_schedule(&ids);
}

/// Starts (or stops) the scheduler's downloads now, without waiting for the time window.
#[tauri::command]
fn set_schedule_running(manager: State<Manager>, running: bool) {
    manager.set_schedule_active(running);
}

#[tauri::command]
fn schedule_running(manager: State<Manager>) -> bool {
    manager.schedule_active()
}

/// Puts the computer to sleep or shuts it down.
#[tauri::command]
fn power_action(action: String) -> CmdResult<()> {
    let (program, args): (&str, &[&str]) = match (action.as_str(), cfg!(windows)) {
        ("shutdown", true) => ("shutdown", &["/s", "/t", "0"]),
        ("sleep", true) => ("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"]),
        ("shutdown", false) => ("systemctl", &["poweroff"]),
        ("sleep", false) => ("systemctl", &["suspend"]),
        _ => return Err(format!("unknown action '{action}'")),
    };
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Opens and closes the scheduler's time window as the clock passes it.
async fn run_scheduler_clock(manager: Manager) {
    use chrono::{Datelike, Timelike};
    let mut was_open = false;
    loop {
        let schedule = manager.settings().schedule;
        let now = chrono::Local::now();
        let open = schedule.is_open(now.weekday().num_days_from_monday() as usize, now.hour() * 60 + now.minute());
        if open && !was_open {
            manager.set_schedule_active(true);
        } else if !open && was_open && schedule.stop.is_some() {
            // Without a stop time, downloads carry on until they're done.
            manager.set_schedule_active(false);
        }
        was_open = open;
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }
}

/// Lists the video and audio qualities a page offers.
#[tauri::command]
async fn get_media_formats(url: String, headers: Vec<(String, String)>) -> CmdResult<MediaInfo> {
    let tools = find_tools(&Settings::tools_dir());
    probe_media(&url, &headers, &tools).await.map_err(|e| e.to_string())
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
    /// Open the playlist / folder dialog for this link.
    playlist: bool,
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
            if manager.settings().confirm_browser_downloads || req.playlist {
                app.state::<PendingFromBrowser>().0.lock().unwrap().push(BrowserDownload {
                    url: req.url.clone(),
                    filename,
                    media,
                    playlist: req.playlist,
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

/// The first time the window is closed to the tray, say so.
fn tell_once_about_tray(app: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri_plugin_notification::NotificationExt;
    static TOLD: AtomicBool = AtomicBool::new(false);
    if !TOLD.swap(true, Ordering::Relaxed) {
        let _ = app
            .notification()
            .builder()
            .title("Still running in the tray")
            .body("Downloads continue in the background. Click the tray icon to open the window, or right-click it to quit.")
            .show();
    }
}

fn main() {
    if bridge::is_host_launch() {
        bridge::run_host();
        return;
    }

    tauri::Builder::default()
        // A second launch just brings the open window to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                if app.state::<Manager>().settings().close_to_tray {
                    // Keep downloading in the background; the tray icon brings it back.
                    api.prevent_close();
                    let _ = window.hide();
                    tell_once_about_tray(app);
                } else {
                    app.exit(0);
                }
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let settings = Settings::load().unwrap_or_default();
            let runtime = tauri::async_runtime::block_on(async { tokio::runtime::Handle::current() });
            let handle = app.handle().clone();
            let (finished_tx, finished_rx) = tokio::sync::mpsc::unbounded_channel();
            let manager = Manager::new(
                settings,
                Settings::data_dir().join("downloads.json"),
                runtime,
                move |change| {
                    if let adm_engine::Change::Finished(entry) = &change {
                        let _ = finished_tx.send((**entry).clone());
                    }
                    let _ = handle.emit("downloads", change);
                },
            );
            app.manage(manager.clone());
            app.manage(PendingFromBrowser::default());
            tauri::async_runtime::spawn(run_scheduler_clock(manager));
            tauri::async_runtime::spawn(tray::run_notifier(app.handle().clone(), finished_rx));
            if let Err(e) = tray::create(app.handle()) {
                eprintln!("couldn't create the tray icon: {e}");
            }
            sync_autostart(app.handle());
            // Started with Windows: stay in the tray. Otherwise show the window.
            if !std::env::args().any(|a| a == "--minimized") {
                show_main(app.handle());
            }

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
            get_media_formats,
            add_many,
            get_collection,
            is_collection,
            add_to_schedule,
            set_schedule_running,
            schedule_running,
            power_action,
            get_entry,
            close_this_window,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start All Download Manager");
}
