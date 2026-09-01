//! Windows IP 协议策略管理（IPv6 优先 / IPv4 优先 / IPv6-only / IPv4-only）。
//!
//! 内嵌自外部脚本 neu-ipv6-diagnostic.ps1：
//! - prefer_ipv6 : netsh 前缀策略 ::/0=40、::ffff:0:0/96=35（双栈 + IPv6 优先）
//! - prefer_ipv4 : 前缀策略 ::ffff:0:0/96=46（IPv4 优先，IPv6 仍可回退）
//! - ipv6_only   : 禁用活动无线网卡的 IPv4 绑定（ms_tcpip），阻止 IPv4 回退
//! - ipv4_only   : 禁用活动无线网卡的 IPv6 绑定（ms_tcpip6）
//! - restore_dual: 恢复双栈绑定并重置默认 IPv6 优先策略
//!
//! 前缀策略持久化在 netsh 持久存储中，重启后保留（与脚本行为一致）。

#[cfg(windows)]
use serde::Serialize;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn set_creation_flags(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(windows)]
fn run_netsh(args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("netsh");
    cmd.args(args);
    set_creation_flags(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("无法启动 netsh: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(if err.trim().is_empty() {
            text
        } else {
            err.to_string()
        })
    }
}

#[cfg(windows)]
fn run_powershell(script: &str) -> Result<String, String> {
    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    set_creation_flags(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("无法启动 PowerShell: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(if err.trim().is_empty() {
            text
        } else {
            err.to_string()
        })
    }
}

/// net session 需要管理员权限：以此探测当前进程是否提升。
#[cfg(windows)]
fn is_elevated() -> bool {
    let mut cmd = std::process::Command::new("net");
    cmd.arg("session");
    set_creation_flags(&mut cmd);
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// 当前策略状态（读取，不需要管理员权限）。
#[cfg(windows)]
pub fn status() -> PolicyStatus {
    let mut st = PolicyStatus::default();

    match run_netsh(&["interface", "ipv6", "show", "prefixpolicies"]) {
        Ok(text) => {
            for line in text.lines() {
                let mut tokens = line.trim().split_whitespace();
                match (tokens.next(), tokens.next()) {
                    (Some("::/0"), Some(prec)) => {
                        st.v6_precedence = prec.parse().unwrap_or(-1);
                    }
                    (Some("::ffff:0:0/96"), Some(prec)) => {
                        st.v4_precedence = prec.parse().unwrap_or(-1);
                    }
                    _ => {}
                }
            }
            st.prefer_ipv6 = st.v6_precedence > st.v4_precedence && st.v6_precedence >= 40;
        }
        Err(e) => {
            st.error = Some(format!("读取 IPv6 前缀策略失败: {}", first_line(&e)));
        }
    }

    match run_powershell(BINDINGS_SCRIPT) {
        Ok(text) => {
            for line in text.lines() {
                let parts: Vec<&str> = line.trim().split('|').collect();
                if parts.len() == 3 {
                    st.adapters.push(AdapterBinding {
                        name: parts[0].to_string(),
                        ipv4: parts[1] == "True",
                        ipv6: parts[2] == "True",
                    });
                }
            }
        }
        Err(e) => {
            let msg = format!("读取网卡协议绑定失败: {}", first_line(&e));
            st.error = Some(match st.error.take() {
                Some(prev) => format!("{prev}；{msg}"),
                None => msg,
            });
        }
    }

    st.elevated = is_elevated();
    st
}

#[cfg(windows)]
fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// 应用一个策略模式，返回给前端的说明文本。
#[cfg(windows)]
pub fn apply(mode: &str) -> Result<String, String> {
    if !is_elevated() {
        return Err("需要管理员权限：请以管理员身份运行 FlowLens 后再设置 IP 协议策略。".into());
    }
    match mode {
        "prefer_ipv6" => {
            set_policy("::/0", 40, 1)?;
            set_policy("::ffff:0:0/96", 35, 4)?;
            Ok("已设置 IPv6 优先策略（::/0=40 > IPv4=35）。双栈目标优先走 IPv6，IPv4 仍可回退；策略重启后保留。".into())
        }
        "prefer_ipv4" => {
            set_policy("::/0", 40, 1)?;
            set_policy("::ffff:0:0/96", 46, 4)?;
            Ok("已设置 IPv4 优先策略（IPv4=46 > ::/0=40）。双栈目标优先走 IPv4，无 IPv4 的目标仍走 IPv6。".into())
        }
        "ipv6_only" => adapter_binding("ms_tcpip", false, "IPv6-only：已禁用活动无线网卡的 IPv4 协议绑定"),
        "ipv4_only" => adapter_binding("ms_tcpip6", false, "IPv4-only：已禁用活动无线网卡的 IPv6 协议绑定"),
        "restore_dual" => {
            let mut msg = adapter_binding("ms_tcpip", true, "已启用 IPv4 协议绑定")?;
            msg.push_str("；");
            msg.push_str(&adapter_binding("ms_tcpip6", true, "已启用 IPv6 协议绑定")?);
            set_policy("::/0", 40, 1)?;
            set_policy("::ffff:0:0/96", 35, 4)?;
            msg.push_str("；已重置为默认 IPv6 优先策略");
            Ok(msg)
        }
        _ => Err(format!("未知模式: {mode}")),
    }
}

/// 设置单条前缀策略（`set` 失败时条目可能不存在，回退 `add`）。
#[cfg(windows)]
fn set_policy(prefix: &str, precedence: u32, label: u32) -> Result<(), String> {
    let prec = precedence.to_string();
    let label = label.to_string();
    if run_netsh(&["interface", "ipv6", "set", "prefixpolicy", prefix, &prec, &label]).is_ok() {
        return Ok(());
    }
    run_netsh(&["interface", "ipv6", "add", "prefixpolicy", prefix, &prec, &label])
        .map(|_| ())
        .map_err(|e| {
            format!(
                "netsh 设置前缀策略 {prefix}={prec} 失败: {}",
                first_line(&e)
            )
        })
}

/// 切换活动无线网卡的协议绑定（与脚本的 Set-WlanBindingState 等价）。
#[cfg(windows)]
fn adapter_binding(component: &str, enable: bool, ok_msg: &str) -> Result<String, String> {
    let verb = if enable { "Enable" } else { "Disable" };
    let want = if enable { "$true" } else { "$false" };
    let script = WLAN_BINDING_SCRIPT
        .replace("__COMP__", component)
        .replace("__VERB__", verb)
        .replace("__WANT__", want)
        .replace("__MSG__", ok_msg);
    let out = run_powershell(&script)?;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("OK|") {
            return Ok(rest.to_string());
        }
        if let Some(rest) = line.strip_prefix("ERR|") {
            return Err(rest.to_string());
        }
    }
    if out.trim().is_empty() {
        Err("命令执行完成但没有返回结果，请重试或检查网卡状态。".into())
    } else {
        Ok(out.trim().to_string())
    }
}

#[cfg(windows)]
const BINDINGS_SCRIPT: &str = r#"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Get-NetAdapter -Physical -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'Up' } | ForEach-Object {
  $v4 = Get-NetAdapterBinding -Name $_.Name -ComponentID 'ms_tcpip' -ErrorAction SilentlyContinue
  $v6 = Get-NetAdapterBinding -Name $_.Name -ComponentID 'ms_tcpip6' -ErrorAction SilentlyContinue
  '{0}|{1}|{2}' -f $_.Name, [bool]($v4.Enabled), [bool]($v6.Enabled)
}
"#;

/// 与脚本 Get-WlanAdapter + Set-WlanBindingState 等价的绑定切换。
#[cfg(windows)]
const WLAN_BINDING_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$adapters = @(Get-NetAdapter -Physical -ErrorAction SilentlyContinue | Where-Object { $_.Status -eq 'Up' })
$wireless = @($adapters | Where-Object { $_.Name -match '(?i)wi-?fi|wlan|wireless|802\.11' -or $_.InterfaceDescription -match '(?i)wi-?fi|wlan|wireless|802\.11' })
if ($wireless.Count -eq 0) {
  $wireless = @($adapters | Where-Object { Get-NetRoute -InterfaceIndex $_.ifIndex -AddressFamily IPv6 -DestinationPrefix '::/0' -ErrorAction SilentlyContinue | Where-Object { $_.State -ne 'Invalid' } })
}
if ($wireless.Count -eq 0) { Write-Output 'ERR|未找到已连接的物理网卡'; exit 2 }
$a = $wireless[0]
$binding = Get-NetAdapterBinding -Name $a.Name -ComponentID '__COMP__' -ErrorAction SilentlyContinue
if ($binding -and ([bool]$binding.Enabled) -eq __WANT__) {
  Write-Output "OK|$($a.Name)|__MSG__（当前已处于目标状态）"
  exit 0
}
__VERB__-NetAdapterBinding -Name $a.Name -ComponentID '__COMP__' -Confirm:$false
Write-Output "OK|$($a.Name)|__MSG__。如当前连接中断，请重新连接 Wi-Fi 后生效"
"#;

#[cfg(windows)]
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterBinding {
    pub name: String,
    pub ipv4: bool,
    pub ipv6: bool,
}

#[cfg(windows)]
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyStatus {
    pub v6_precedence: i64,
    pub v4_precedence: i64,
    pub prefer_ipv6: bool,
    pub adapters: Vec<AdapterBinding>,
    pub elevated: bool,
    pub error: Option<String>,
}

// ---- 非 Windows 平台的桩，保证可编译 ----

#[cfg(not(windows))]
#[derive(Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyStatus {
    pub v6_precedence: i64,
    pub v4_precedence: i64,
    pub prefer_ipv6: bool,
    pub adapters: Vec<serde_json::Value>,
    pub elevated: bool,
    pub error: Option<String>,
}

#[cfg(not(windows))]
pub fn status() -> PolicyStatus {
    PolicyStatus {
        error: Some("IP 协议策略功能仅支持 Windows。".into()),
        ..Default::default()
    }
}

#[cfg(not(windows))]
pub fn apply(_mode: &str) -> Result<String, String> {
    Err("IP 协议策略功能仅支持 Windows。".into())
}
