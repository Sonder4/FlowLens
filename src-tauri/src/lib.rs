//! GlassNet backend: packet capture, persistent traffic history and
//! live adapter I/O, exposed to the Svelte frontend via commands and events.

mod adapter_io;
mod capture;
mod port_map;
mod traffic_history;

use std::net::{Ipv4Addr, Ipv6Addr};
use tauri::menu::{MenuBuilder, MenuItem};
use tauri::{AppHandle, Emitter, Manager};

use capture::DeviceInfo;
use traffic_history::{Granularity, HistBucket};

static SETUP_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 前端初始化前等待后端 setup 完成，避免查询与事件在初始化前竞态
#[tauri::command]
fn setup_done() -> bool {
    SETUP_DONE.load(std::sync::atomic::Ordering::Relaxed)
}

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
    let result = traffic_history::query(granularity, adapter.as_deref());
    eprintln!("[glassnet] history -> {} buckets", result.len());
    result
}

/// 右键悬浮窗弹出的原生菜单：本机地址（只读）+ 主面板 + 关闭
#[tauri::command]
fn popup_floating_menu(app: AppHandle) {
    let addrs = local_addresses_impl();
    let v4 = addrs.iter().find(|a| a.parse::<Ipv4Addr>().is_ok());
    let v6 = addrs.iter().find(|a| a.parse::<Ipv6Addr>().is_ok());

    let Some(win) = app.get_webview_window("floating") else {
        return;
    };

    let text_item = |id: &str, text: String, enabled: bool| -> MenuItem<tauri::Wry> {
        MenuItem::with_id(&app, id, text, enabled, None::<&str>).ok().unwrap()
    };

    let v4_item = text_item("gm_v4", format!("v4  {}", v4.unwrap_or(&"不可用".to_string())), false);
    let v6_item = text_item("gm_v6", format!("v6  {}", v6.unwrap_or(&"不可用".to_string())), false);
    let open_item = text_item("gm_open", "打开主面板".into(), true);
    let close_item = text_item("gm_close", "关闭悬浮窗".into(), true);

    let Ok(sep) = tauri::menu::PredefinedMenuItem::separator(&app) else {
        return;
    };
    let menu = MenuBuilder::new(&app)
        .item(&v4_item)
        .item(&v6_item)
        .item(&sep)
        .item(&open_item)
        .item(&close_item)
        .build();
    if let Ok(menu) = menu {
        let _ = win.popup_menu(&menu);
    }
}

fn local_addresses_impl() -> Vec<String> {
    capture::list_devices()
        .into_iter()
        .flat_map(|d| d.addresses)
        .collect()
}

#[tauri::command]
fn local_addresses() -> Vec<String> {
    local_addresses_impl()
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
            eprintln!("[glassnet] setup: begin");
            traffic_history::init();
            eprintln!("[glassnet] setup: history ready");
            port_map::spawn_refresher();
            adapter_io::spawn_emitter(app.handle().clone());
            // resume capturing all adapters automatically
            capture::start(app.handle(), None);
            eprintln!(
                "[glassnet] setup: capture running={}",
                capture::is_running()
            );
            SETUP_DONE.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_devices,
            start_capture,
            stop_capture,
            capture_running,
            io_snapshot,
            history,
            setup_done,
            local_addresses,
            popup_floating_menu,
            show_window,
            hide_window,
        ])
        .on_menu_event(|app, event| match event.id().as_ref() {
            "gm_open" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "gm_close" => {
                if let Some(w) = app.get_webview_window("floating") {
                    let _ = w.hide();
                }
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running GlassNet");
}
