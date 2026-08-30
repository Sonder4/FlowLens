//! GlassNet backend: packet capture, persistent traffic history and
//! live adapter I/O, exposed to the Svelte frontend via commands and events.

mod adapter_io;
mod capture;
mod traffic_history;

use tauri::{AppHandle, Emitter, Manager};

use capture::DeviceInfo;
use traffic_history::{Granularity, HistBucket};

#[tauri::command]
fn list_devices() -> Vec<DeviceInfo> {
    capture::list_devices()
}

#[tauri::command]
fn start_capture(app: AppHandle, device: Option<String>) {
    capture::start(&app, device);
}

#[tauri::command]
fn stop_capture(app: AppHandle) {
    capture::stop();
    let _ = app.emit("capture-state", serde_json::json!({ "running": false }));
}

#[tauri::command]
fn capture_running() -> bool {
    capture::is_running()
}

#[tauri::command]
fn io_snapshot() -> Vec<adapter_io::AdapterIo> {
    adapter_io::snapshot()
}

#[tauri::command]
fn history(granularity: String, adapter: Option<String>) -> Vec<HistBucket> {
    let granularity = match granularity.as_str() {
        "daily" => Granularity::Daily,
        "monthly" => Granularity::Monthly,
        _ => Granularity::Hourly,
    };
    traffic_history::query(granularity, adapter.as_deref())
}

#[tauri::command]
fn show_window(app: AppHandle, label: String) {
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn hide_window(app: AppHandle, label: String) {
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.hide();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            traffic_history::init();
            adapter_io::spawn_emitter(app.handle().clone());
            // resume capturing all adapters automatically
            capture::start(app.handle(), None);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            start_capture,
            stop_capture,
            capture_running,
            io_snapshot,
            history,
            show_window,
            hide_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running GlassNet");
}
