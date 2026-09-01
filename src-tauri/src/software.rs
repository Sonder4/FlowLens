//! Installed-software catalog and traffic categorization.
//!
//! Traffic attribution upstream is per-process (see `port_map`); here each
//! program name is mapped to one of four user-facing categories:
//!
//! - `system`   — OS itself: kernel/system processes, svchost services
//!                (updates, telemetry, logs, Delivery Optimization…)
//! - `dev`      — developer tooling: node/npm/git/cargo/python/docker…
//! - `software` — installed applications (matched against the Windows
//!                uninstall registry + a well-known process map)
//! - `other`    — unrecognized traffic ("其他" and unmatched processes)
//!
//! Categories are computed at query/display time, never persisted, so the
//! rule sets can evolve without database migrations.

use std::{collections::HashSet, sync::OnceLock, sync::RwLock};

use serde::Serialize;

/// One entry of the installed-programs catalog (uninstall registry).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub name: String,
    pub publisher: Option<String>,
    pub install_location: Option<String>,
}

/// OS-owned processes (kernel, shell, service hosts, Defender, indexer…).
const SYSTEM_PROCESSES: &[&str] = &[
    "system", "smss", "csrss", "wininit", "winlogon", "lsass", "services",
    "svchost", "dwm", "fontdrvhost", "sihost", "taskhostw", "explorer",
    "conhost", "openconsole", "dllhost", "runtimebroker", "spoolsv",
    "searchapp", "searchindexer", "searchprotocolhost", "searchfilterhost",
    "memory compression", "vmmem", "wmiprvse", "wudfhost", "msmpeng",
    "nissrv", "securityhealthservice", "securityhealthsystray",
    "comptelrunner", "dusmhost", "werfault", "wermgr", "smartscreen",
    "ctfmon", "applicationframehost", "lockapp", "textinputhost",
    "startmenuexperiencehost", "shellexperiencehost", "systemsettings",
    "mousocoreworker", "tiworker", "trustedinstaller", "usocoreworker",
    "audiodg", "nvdisplay.container", "msedgewebview2",
];

/// Developer tooling: package managers, VCS, toolchains, containers.
const DEV_PROCESSES: &[&str] = &[
    "node", "npm", "npx", "yarn", "pnpm", "cnpm", "tnpm", "bun", "deno",
    "git", "git-remote-https", "git-remote-http", "git-remote-ssh", "ssh",
    "scp", "cargo", "rustc", "rustup", "clippy-driver", "python", "pythonw",
    "python3", "pip", "pip3", "conda", "go", "gofmt", "docker",
    "docker-compose", "dockerd", "kubectl", "helm", "terraform", "dotnet",
    "java", "javaw", "mvn", "gradle", "code", "winget", "scoop", "choco",
    "hugo", "zcode",
];

/// Well-known applications whose uninstall entries don't match their
/// process name; forces the `software` category.
const WELL_KNOWN_SOFTWARE: &[&str] = &[
    "msedge", "chrome", "firefox", "brave", "opera", "qq", "weixin",
    "wechat", "wxwork", "dingtalk", "feishu", "baidunetdisk", "thunder",
    "steam", "epicgameslauncher", "cloudmusic", "kugou", "potplayer",
    "everything", "ugreen nas", "syncspace_pro", "ugagent", "ima.copilot",
    "snipaste", "obsidian", "typora",
];

/// Lowercase process stem: `"C:\...\MsEdge.exe"` -> `"msedge"`;
/// svchost 服务标签（`svchost:xxx`）整体保留小写。
fn stem(program: &str) -> String {
    let base = program.strip_suffix(".exe").unwrap_or(program);
    if base.starts_with("svchost:") {
        return base.to_lowercase();
    }
    let file = base.rsplit(['\\', '/']).next().unwrap_or(base);
    file.to_lowercase()
}

/// Categorize one attributed program name into `system` / `dev` /
/// `software` / `other`.
pub fn categorize(program: &str) -> &'static str {
    if program == "其他" {
        return "other";
    }
    let s = stem(program);
    if s.starts_with("svchost:") || SYSTEM_PROCESSES.contains(&s.as_str()) {
        return "system";
    }
    if DEV_PROCESSES.contains(&s.as_str()) {
        return "dev";
    }
    if WELL_KNOWN_SOFTWARE.contains(&s.as_str()) {
        return "software";
    }
    // 目录匹配：安装目录包含同名 exe，或目录名与进程名一致
    let exe_suffix = format!("\\{s}.exe");
    let dir_suffix = format!("\\{s}");
    // 产品名前缀匹配：PotPlayer ↔ potplayermini64（exe 带变体后缀时仍可命中）
    let name_match = |n: &str| n.len() >= 3 && s.len() >= 4 && (s.starts_with(n) || n.starts_with(&s));
    let in_catalog = catalog()
        .read()
        .map(|c| {
            c.iter().any(|a| {
                if let Some(loc) = a.install_location.as_ref() {
                    let loc = loc.to_lowercase();
                    if loc.ends_with(&exe_suffix) || loc.ends_with(&dir_suffix) {
                        return true;
                    }
                }
                name_match(&a.name.to_lowercase())
            })
        })
        .unwrap_or(false);
    if in_catalog {
        return "software";
    }
    "other"
}

static CATALOG: OnceLock<RwLock<Vec<InstalledApp>>> = OnceLock::new();

fn catalog() -> &'static RwLock<Vec<InstalledApp>> {
    CATALOG.get_or_init(|| RwLock::new(Vec::new()))
}

/// Returns a snapshot of the installed-programs catalog.
pub fn installed_apps() -> Vec<InstalledApp> {
    catalog().read().map(|c| c.clone()).unwrap_or_default()
}

/// Scans the uninstall registry and replaces the cached catalog.
/// Returns the fresh snapshot.
pub fn refresh() -> Vec<InstalledApp> {
    let list = scan_registry();
    if let Ok(mut slot) = catalog().write() {
        *slot = list.clone();
    }
    list
}

/// Loads the catalog once and refreshes it daily in a background thread.
pub fn spawn_refresher() {
    let _ = std::thread::Builder::new()
        .name("software-catalog".into())
        .spawn(|| {
            refresh();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
                refresh();
            }
        });
}

#[cfg(windows)]
fn scan_registry() -> Vec<InstalledApp> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const HIVES: [(winreg::HKEY, &str); 3] = [
        (HKEY_LOCAL_MACHINE, UNINSTALL),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
        (HKEY_CURRENT_USER, UNINSTALL),
    ];

    let mut out: Vec<InstalledApp> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (hive, path) in HIVES {
        let Ok(root) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) else {
            continue;
        };
        for key_name in root.enum_keys().flatten() {
            let Ok(key) = root.open_subkey_with_flags(&key_name, KEY_READ) else {
                continue;
            };
            let name: String = key.get_value("DisplayName").unwrap_or_default();
            let name = name.trim().to_string();
            // 跳过纯更新补丁（KBxxxxxxx）：系统更新流量本身按进程归为 system
            if name.is_empty() || name.to_lowercase().starts_with("kb") {
                continue;
            }
            if !seen.insert(name.to_lowercase()) {
                continue;
            }
            let publisher: Option<String> = key.get_value("Publisher").ok();
            let install_location: Option<String> = key
                .get_value::<String, _>("InstallLocation")
                .ok()
                .filter(|s| !s.trim().is_empty());
            out.push(InstalledApp {
                name,
                publisher,
                install_location,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

#[cfg(not(windows))]
fn scan_registry() -> Vec<InstalledApp> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_categorize() {
        // 系统流量：svchost 服务、内核与系统进程
        assert_eq!(categorize("svchost:wuauserv"), "system");
        assert_eq!(categorize("svchost:DoSvc,BITS"), "system");
        assert_eq!(categorize("MsMpEng.exe"), "system");
        assert_eq!(categorize("system"), "system");
        assert_eq!(categorize("SearchIndexer.exe"), "system");
        // 开发流量
        assert_eq!(categorize("node.exe"), "dev");
        assert_eq!(categorize("git-remote-https.exe"), "dev");
        assert_eq!(categorize("cargo.exe"), "dev");
        assert_eq!(categorize("python.exe"), "dev");
        // 软件流量：知名映射
        assert_eq!(categorize("msedge.exe"), "software");
        assert_eq!(categorize("WeChat.exe"), "software");
        // 未归类
        assert_eq!(categorize("其他"), "other");
        assert_eq!(categorize("totally_unknown.exe"), "other");
    }

    #[test]
    fn test_categorize_by_install_location() {
        // 注入一条安装目录匹配（绕过注册表扫描，直接写缓存）
        if let Ok(mut slot) = catalog().write() {
            slot.push(InstalledApp {
                name: "PotPlayer".into(),
                publisher: None,
                install_location: Some("D:\\Tools\\PotPlayer\\".into()),
            });
        }
        assert_eq!(categorize("PotPlayerMini64.exe"), "software");
    }
}
