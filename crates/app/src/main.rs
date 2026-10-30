// No console window behind the app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use adm_engine::{AddRequest, Entry, Manager, Settings};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager as _, State};
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

fn main() {
    tauri::Builder::default()
        // A second launch just brings the open window to the front.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
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
        ])
        .run(tauri::generate_context!())
        .expect("failed to start All Download Manager");
}
