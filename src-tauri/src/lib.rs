//! GlassNet backend: packet capture, persistent traffic history and
//! live adapter I/O, exposed to the Svelte frontend via commands and events.

mod adapter_io;
mod capture;
mod ip_policy;
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

/// 当前 Windows IPv6 前缀策略与网卡协议绑定状态（设置页展示）
#[tauri::command]
fn ipv6_policy_status() -> ip_policy::PolicyStatus {
    ip_policy::status()
}

/// 应用 IP 协议策略（需管理员权限，与 neu-ipv6-diagnostic.ps1 等价）
#[tauri::command]
fn set_ipv6_policy(mode: String) -> Result<String, String> {
    ip_policy::apply(&mode)
}

/// 触发 UAC 弹窗并以管理员权限重启 GlassNet。
/// 用户在 UAC 中确认后，当前未提权实例自动退出；取消则保留当前实例并返回错误。
#[tauri::command]
fn restart_as_admin() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().map_err(|e| format!("获取程序路径失败: {e}"))?;
        let exe_escaped = exe.to_string_lossy().replace('\'', "''");
        let script = format!(
            "try {{ Start-Process -FilePath '{exe_escaped}' -Verb RunAs -ErrorAction Stop; exit 0 }} catch {{ exit 1 }}"
        );
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("powershell")
                .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &script])
                .creation_flags(CREATE_NO_WINDOW)
                .status()
                .map_err(|e| format!("无法发起提权请求: {e}"))?
        };
        if status.success() {
            // 新的提权实例已启动；先落盘再退出当前实例
            traffic_history::flush_now();
            std::process::exit(0);
        }
        return Err("已取消 UAC 提权或启动失败，GlassNet 未重启".into());
    }
    #[cfg(not(target_os = "windows"))]
    Err("仅 Windows 支持 UAC 提权重启".into())
}

/// 应用每日流量历史（仅保留单日合计 > 100MB 的应用，v4/v6 收发明细）
#[tauri::command]
fn history_app_day() -> Vec<traffic_history::AppDayRow> {
    traffic_history::query_app_days()
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
            ipv6_policy_status,
            set_ipv6_policy,
            restart_as_admin,
            io_snapshot,
            history,
            history_app_day,
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
        .build(tauri::generate_context!())
        .expect("error while building GlassNet")
        .run(|_app, event| {
            // 退出前把内存中未落盘的流量（含应用每日累计）写入数据库
            if matches!(event, tauri::RunEvent::Exit) {
                traffic_history::flush_now();
            }
        });
}
