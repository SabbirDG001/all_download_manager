//! System tray icon and "download finished" notifications.

use std::time::Duration;

use adm_engine::{Entry, Manager, Settings, Status};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager as _, WebviewUrl, WebviewWindowBuilder, Wry};
use tauri_plugin_notification::NotificationExt;

const TRAY_ID: &str = "main";

/// Menu items whose state changes while the app runs.
pub struct TrayItems {
    limit: CheckMenuItem<Wry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let settings = app.state::<Manager>().settings();
    let open = MenuItem::with_id(app, "open", "Open All Download Manager", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause_all", "Pause all", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume_all", "Resume all", true, None::<&str>)?;
    let limit = CheckMenuItem::with_id(app, "limit", "Speed limit", true, settings.speed_limit_on, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &resume,
            &limit,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("All Download Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => crate::show_main(app),
            "pause_all" => app.state::<Manager>().pause_all(),
            "resume_all" => app.state::<Manager>().resume_all(),
            "limit" => toggle_limit(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                crate::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    app.manage(TrayItems { limit });

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            update_tooltip(&handle);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
    Ok(())
}

fn toggle_limit(app: &AppHandle) {
    let manager = app.state::<Manager>();
    let mut settings = manager.settings();
    settings.speed_limit_on = !settings.speed_limit_on;
    if settings.save().is_ok() {
        manager.set_settings(settings);
    }
    sync(app);
    let _ = app.emit("settings-changed", ());
}

/// Brings the tray menu in line with the settings.
pub fn sync(app: &AppHandle) {
    let on = app.state::<Manager>().settings().speed_limit_on;
    if let Some(items) = app.try_state::<TrayItems>() {
        let _ = items.limit.set_checked(on);
    }
}

fn update_tooltip(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let entries = app.state::<Manager>().entries();
    let running: Vec<&Entry> = entries.iter().filter(|e| e.status == Status::Downloading).collect();
    let waiting = entries.iter().filter(|e| e.status == Status::Queued).count();
    let text = if running.is_empty() {
        if waiting > 0 { format!("All Download Manager\n{waiting} waiting") } else { "All Download Manager".into() }
    } else {
        let speed: f64 = running.iter().map(|e| e.speed).sum();
        format!("All Download Manager\n{} downloading · {}/s", running.len(), human_bytes(speed))
    };
    let _ = tray.set_tooltip(Some(text));
}

fn human_bytes(n: f64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

fn display_name(e: &Entry) -> String {
    e.filename.clone().or_else(|| e.title.clone()).unwrap_or_else(|| e.url.clone())
}

/// Collects finished downloads for a moment, so a playlist finishing gives
/// one notification instead of dozens.
pub async fn run_notifier(app: AppHandle, mut finished: tokio::sync::mpsc::UnboundedReceiver<Entry>) {
    while let Some(first) = finished.recv().await {
        let mut batch = vec![first];
        let deadline = tokio::time::Instant::now() + Duration::from_millis(2500);
        while let Ok(Some(more)) = tokio::time::timeout_at(deadline, finished.recv()).await {
            batch.push(more);
        }
        let settings: Settings = app.state::<Manager>().settings();
        if settings.notify_on_finish && !main_window_in_use(&app) {
            notify(&app, &batch);
        }
        if settings.show_complete_window {
            for entry in batch.iter().filter(|e| e.status == Status::Completed).take(3) {
                open_complete_window(&app, entry);
            }
        }
    }
}

/// The user is looking at the list already: no need to notify.
fn main_window_in_use(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false) && !w.is_minimized().unwrap_or(true))
}

fn notify(app: &AppHandle, batch: &[Entry]) {
    let done: Vec<&Entry> = batch.iter().filter(|e| e.status == Status::Completed).collect();
    let failed: Vec<&Entry> = batch.iter().filter(|e| e.status == Status::Failed).collect();
    let (title, body) = match (done.len(), failed.len()) {
        (1, 0) => ("Download complete".to_string(), display_name(done[0])),
        (0, 1) => (
            "Download failed".to_string(),
            format!("{}\n{}", display_name(failed[0]), failed[0].error.clone().unwrap_or_default()),
        ),
        (d, 0) => (format!("{d} downloads complete"), names(&done)),
        (0, f) => (format!("{f} downloads failed"), names(&failed)),
        (d, f) => (format!("{d} downloads complete, {f} failed"), names(&done)),
    };
    let _ = app.notification().builder().title(title).body(body).show();
}

fn names(entries: &[&Entry]) -> String {
    let mut list: Vec<String> = entries.iter().take(3).map(|e| display_name(e)).collect();
    if entries.len() > 3 {
        list.push(format!("and {} more", entries.len() - 3));
    }
    list.join("\n")
}

/// IDM-style small window with Open / Open folder / Close.
pub fn open_complete_window(app: &AppHandle, entry: &Entry) {
    // Its progress window already turned into a "complete" window.
    if app.get_webview_window(&format!("progress-{}", entry.id)).is_some() {
        return;
    }
    let label = format!("done-{}", entry.id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("done.html?id={}", entry.id).into()))
        .title("Download complete")
        .inner_size(460.0, 230.0)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .always_on_top(true)
        .center()
        .focused(true)
        .build();
}
