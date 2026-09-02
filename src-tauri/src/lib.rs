//! FlowLens backend: packet capture, persistent traffic history and
//! live adapter I/O, exposed to the Svelte frontend via commands and events.

mod adapter_io;
mod capture;
mod embedded_assets_server;
mod ip_policy;
mod port_map;
mod software;
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

// ── 开机自启：HKCU\...\Run 注册表项，值 = 当前 exe 路径 + --minimized ──
#[cfg(target_os = "windows")]
mod autostart {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
    use winreg::RegKey;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "FlowLens";
    const LAUNCH_ARG: &str = " --minimized";

    /// 自启命令行：exe 路径加引号 + 静默启动参数
    fn command_value() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        Some(format!("\"{}\"{}", exe.display(), LAUNCH_ARG))
    }

    /// 是否已启用：注册表项存在且指向当前 exe（exe 位置变更后视为未启用，重新开启即可覆盖）
    pub fn status() -> bool {
        let Some(expected) = command_value() else {
            return false;
        };
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(RUN_KEY)
            .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
            .ok()
            .map_or(false, |v| v == expected)
    }

    pub fn set(enable: bool) -> Result<(), String> {
        let key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE | KEY_QUERY_VALUE)
            .map_err(|e| e.to_string())?;
        if enable {
            let value = command_value().ok_or_else(|| "无法获取程序路径".to_string())?;
            key.set_value(VALUE_NAME, &value).map_err(|e| e.to_string())
        } else {
            // 幂等：项不存在也视为成功
            match key.delete_value(VALUE_NAME) {
                Ok(()) => Ok(()),
                Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.to_string()),
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod autostart {
    pub fn status() -> bool {
        false
    }
    pub fn set(_enable: bool) -> Result<(), String> {
        Err("仅支持 Windows".to_string())
    }
}

/// 开机自启是否已开启
#[tauri::command]
fn autostart_status() -> bool {
    autostart::status()
}

/// 开启/关闭开机自启（登录后静默启动到系统托盘）
#[tauri::command]
fn autostart_set(enable: bool) -> Result<(), String> {
    autostart::set(enable)
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

/// 触发 UAC 弹窗并以管理员权限重启 FlowLens。
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
        return Err("已取消 UAC 提权或启动失败，FlowLens 未重启".into());
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

/// 任意时间范围的总流量序列（后端按跨度自动选小时/天桶）
#[tauri::command]
fn history_range(
    since: i64,
    until: i64,
    adapter: Option<String>,
) -> Option<traffic_history::RangeSeries> {
    traffic_history::query_range(since, until, adapter.as_deref())
}

/// 任意时间范围内按应用的流量聚合（附带 分类 标记）
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AppCategoryRow {
    app: String,
    category: &'static str,
    rx_v4: u64,
    tx_v4: u64,
    rx_v6: u64,
    tx_v6: u64,
}

#[tauri::command]
fn history_app_range(since: i64, until: i64) -> Vec<AppCategoryRow> {
    traffic_history::query_app_range(since, until)
        .into_iter()
        .map(|r| AppCategoryRow {
            category: software::categorize(&r.app),
            app: r.app,
            rx_v4: r.rx_v4,
            tx_v4: r.tx_v4,
            rx_v6: r.rx_v6,
            tx_v6: r.tx_v6,
        })
        .collect()
}

/// 已安装软件目录（注册表 Uninstall 键枚举，启动时加载并每日刷新）
#[tauri::command]
fn list_installed_apps() -> Vec<software::InstalledApp> {
    software::installed_apps()
}

#[tauri::command]
fn history(granularity: String, adapter: Option<String>) -> Vec<HistBucket> {
    let granularity = match granularity.as_str() {
        "daily" => Granularity::Daily,
        "monthly" => Granularity::Monthly,
        _ => Granularity::Hourly,
    };
    let result = traffic_history::query(granularity, adapter.as_deref());
    eprintln!("[flowlens] history -> {} buckets", result.len());
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

/// Release builds use a real private loopback HTTP server for the bundled UI.
/// WebView2 then has no dependency on the unreliable tauri.localhost request hook.
fn create_frontend_windows(app: &tauri::App<tauri::Wry>) -> tauri::Result<()> {
    #[cfg(debug_assertions)]
    let frontend_base = app
        .config()
        .build
        .dev_url
        .as_ref()
        .expect("dev builds require build.devUrl")
        .as_str()
        .trim_end_matches('/')
        .to_string();

    #[cfg(not(debug_assertions))]
    let frontend_base = {
        let assets_server = embedded_assets_server::EmbeddedAssetsServer::start(app.handle())?;
        authorize_embedded_frontend(app, assets_server.url_pattern())?;
        assets_server.url_for("").trim_end_matches('/').to_string()
    };

    for configured_window in &app.config().app.windows {
        let mut window = configured_window.clone();
        let tauri::WebviewUrl::App(asset_path) = &window.url else {
            continue;
        };
        let url = format!("{frontend_base}/{}", asset_path.to_string_lossy());
        window.url = tauri::WebviewUrl::External(url.parse().expect("valid frontend URL"));
        tauri::WebviewWindowBuilder::from_config(app.handle(), &window)?.build()?;
    }

    Ok(())
}

/// The random loopback origin is known only after binding the listener.
/// Register it at runtime so Tauri authorizes exactly this app instance.
#[cfg(not(debug_assertions))]
fn authorize_embedded_frontend(app: &tauri::App<tauri::Wry>, url_pattern: String) -> tauri::Result<()> {
    app.add_capability(
        tauri::ipc::CapabilityBuilder::new("embedded-ui")
            .remote(url_pattern)
            .windows(["main", "floating", "settings"])
            .permission("allow-flowlens-ui")
            .permission("core:default")
            .permission("core:window:default")
            .permission("core:event:default")
            .permission("core:window:allow-start-dragging")
            .permission("core:window:allow-show")
            .permission("core:window:allow-hide")
            .permission("core:window:allow-set-focus"),
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例：应用已运行时再次双击 exe，不再起第二个进程，
        // 而是唤出已运行实例的主面板（否则用户会以为程序没打开）
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .setup(|app| {
            eprintln!("[flowlens] setup: begin");
            create_frontend_windows(app)?;
            traffic_history::init();
            eprintln!("[flowlens] setup: history ready");
            port_map::spawn_refresher();
            software::spawn_refresher();
            adapter_io::spawn_emitter(app.handle().clone());
            // resume capturing all adapters automatically
            capture::start(app.handle(), None);
            eprintln!(
                "[flowlens] setup: capture running={}",
                capture::is_running()
            );
            SETUP_DONE.store(true, std::sync::atomic::Ordering::Relaxed);

            // 开机自启（--minimized）：主面板隐藏到托盘，抓包已在上文自动恢复；
            // 悬浮窗保留显示，作为"应用已在运行"的可见提示
            //（用户在设置中关闭悬浮窗时，其前端加载后会自行隐藏）
            if std::env::args().any(|a| a == "--minimized") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
                eprintln!("[flowlens] setup: minimized launch, main hidden");
            }

            // 系统托盘：左键点击 = 切换主面板；右键菜单 = 显示主面板/悬浮窗/退出
            use tauri::tray::{TrayIconBuilder, TrayIconEvent};
            let tray_show = tauri::menu::MenuItem::with_id(app, "tray_show", "显示主面板", true, None::<&str>)?;
            let tray_float = tauri::menu::MenuItem::with_id(app, "tray_float", "显示悬浮窗", true, None::<&str>)?;
            let tray_quit = tauri::menu::MenuItem::with_id(app, "tray_quit", "退出", true, None::<&str>)?;
            let tray_menu = tauri::menu::Menu::with_items(app, &[&tray_show, &tray_float, &tray_quit])?;
            let _tray = TrayIconBuilder::with_id("flowlens-tray")
                .icon(app.default_window_icon().expect("missing window icon").clone())
                .tooltip("FlowLens — 网络流量监控")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.unminimize();
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

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
            history_range,
            history_app_range,
            list_installed_apps,
            setup_done,
            local_addresses,
            popup_floating_menu,
            show_window,
            hide_window,
            autostart_status,
            autostart_set,
        ])
        // 窗口关闭 = 最小化到托盘继续运行（真正退出请使用托盘菜单的「退出」）
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray_show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "tray_float" => {
                if let Some(w) = app.get_webview_window("floating") {
                    let _ = w.show();
                }
            }
            "tray_quit" => app.exit(0),
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
        .expect("error while building FlowLens")
        .run(|_app, event| {
            // 退出前把内存中未落盘的流量（含应用每日累计）写入数据库
            if matches!(event, tauri::RunEvent::Exit) {
                traffic_history::flush_now();
            }
        });
}
