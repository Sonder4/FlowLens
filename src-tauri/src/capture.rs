//! Packet capture engine: one thread per device, per-second aggregation into
//! {rx,tx} × {IPv4,IPv6} buckets plus a per-flow table attributed to
//! applications (local port → owning process), pushed to the frontend as
//! `traffic-tick` events and persisted into the SQLite history.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use etherparse::{LaxPacketHeaders, NetHeaders, TransportHeader};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::port_map;
use crate::traffic_history::{self, Dir, Family};

/// One application-attributed flow of the live connection table.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowInfo {
    pub device: String,
    pub remote: String,
    pub remote_port: u16,
    pub local_port: u16,
    pub proto: String,
    pub family: String,
    pub rx: u64,
    pub tx: u64,
    pub program: String,
}

/// Presentation info of a capture device.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub desc: Option<String>,
    pub addresses: Vec<String>,
}

impl DeviceInfo {
    fn display(&self) -> String {
        self.desc.clone().unwrap_or_else(|| self.name.clone())
    }
}

/// Virtual adapters that duplicate real traffic or carry none: hidden from
/// the device picker to avoid confusing entries (user report: multiple Wi-Fi).
fn is_noise_device(desc: &str) -> bool {
    let lower = desc.to_lowercase();
    ["wi-fi direct", "bluetooth", "wan miniport", "microsoft kernel"]
        .iter()
        .any(|kw| lower.contains(kw))
}

pub fn list_devices() -> Vec<DeviceInfo> {
    // 描述相同的网卡（多块 TAP / 虚拟网卡重命名前后）只保留首个：
    // 前端以 display 作为设备标识与列表键，重复会导致键控循环崩溃
    let mut seen = std::collections::HashSet::new();
    pcap::Device::list()
        .unwrap_or_default()
        .into_iter()
        .map(|d| DeviceInfo {
            addresses: d.addresses.iter().map(|a| a.addr.to_string()).collect(),
            desc: d.desc,
            name: d.name,
        })
        .filter(|d| !d.addresses.is_empty())
        .filter(|d| !is_noise_device(&d.display()))
        .filter(|d| seen.insert(d.display()))
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    remote: IpAddr,
    remote_port: u16,
    local_port: u16,
    is_tcp: bool,
}

#[derive(Default)]
struct FlowStat {
    rx: u64,
    tx: u64,
    last_seen: u64,
    program: Arc<str>,
}

/// 应用流量按秒聚合的键：进程名 + IP 族（收/发分开计入 rx/tx）。
#[derive(PartialEq, Eq, Hash)]
struct AppKey {
    program: Arc<str>,
    family: Family,
}

struct DeviceState {
    display: String,
    adapter: Arc<str>,
    local_addrs: Vec<IpAddr>,
    cur: [u64; 4], // rx_v4, rx_v6, tx_v4, tx_v6
    flows: HashMap<FlowKey, FlowStat>,
    app_cur: HashMap<AppKey, (u64, u64)>, // (rx, tx)，每秒批量落历史
    cur_sec: u64,
}

struct Running {
    stop: Arc<AtomicBool>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Stops the current capture (if any) and starts one thread per selected
/// device. `device` filters by display name; `None` captures every device
/// that has at least one active address.
pub fn start(app: &AppHandle, device: Option<String>) {
    stop();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut spawned = false;

    for info in list_devices() {
        if let Some(filter) = &device {
            if &info.display() != filter && &info.name != filter {
                continue;
            }
        }
        let display = info.display();
        let state = DeviceState {
            display: display.clone(),
            adapter: Arc::from(display.as_str()),
            local_addrs: info
                .addresses
                .iter()
                .filter_map(|a| a.parse().ok())
                .collect(),
            cur: [0; 4],
            flows: HashMap::new(),
            app_cur: HashMap::new(),
            cur_sec: now_secs(),
        };
        let stop = Arc::clone(&stop_flag);
        let app = app.clone();
        let pcap_name = info.name.clone();
        let _ = std::thread::Builder::new()
            .name(format!("capture-{display}"))
            .spawn(move || {
                run_device(app, pcap_name, state, stop);
            });
        spawned = true;
    }

    if spawned {
        RUNNING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .replace(Running { stop: stop_flag });
    }
    let _ = app.emit("capture-state", serde_json::json!({ "running": spawned }));
}

pub fn stop() {
    if let Some(running) = RUNNING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        running.stop.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(120));
    }
}

pub fn is_running() -> bool {
    RUNNING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some()
}

fn run_device(app: AppHandle, pcap_name: String, mut state: DeviceState, stop: Arc<AtomicBool>) {
    // 读超时让阻塞的 next_packet 定期返回：网卡空闲时主循环也能
    // 按秒推进（advance_second 发 tick），否则要等到下一个包才更新
    let Ok(mut cap) = pcap::Capture::from_device(pcap_name.as_str())
        .map(|c| c.timeout(400))
        .and_then(|c| c.open())
    else {
        let _ = app.emit(
            "capture-error",
            serde_json::json!({ "device": state.display, "message": "无法打开网卡（权限不足或已被占用）" }),
        );
        return;
    };
    let datalink = cap.get_datalink();

    while !stop.load(Ordering::Relaxed) {
        if now_secs() != state.cur_sec {
            advance_second(&app, &mut state);
        }

        match cap.next_packet() {
            Ok(packet) => {
                let Some(headers) = sniffable_headers(packet.data, datalink) else {
                    continue;
                };
                let family = match &headers.net {
                    Some(NetHeaders::Ipv4(..)) => Some(Family::V4),
                    Some(NetHeaders::Ipv6(..)) => Some(Family::V6),
                    _ => None,
                };
                let Some(family) = family else { continue };

                let bytes = u64::try_from(packet.data.len()).unwrap_or(u64::MAX);
                let (source, dest) = match &headers.net {
                    Some(NetHeaders::Ipv4(h, _)) => {
                        (IpAddr::from(h.source), IpAddr::from(h.destination))
                    }
                    Some(NetHeaders::Ipv6(h, _)) => {
                        (IpAddr::from(h.source), IpAddr::from(h.destination))
                    }
                    _ => continue,
                };
                let outgoing = state.local_addrs.contains(&source);
                let dir = if outgoing { Dir::Tx } else { Dir::Rx };

                // 历史计数改为 advance_second 每秒批量记录（见下），
                // 抓包热路径不再做每包的哈希查找与 Arc 克隆
                let idx = match (dir, family) {
                    (Dir::Rx, Family::V4) => 0,
                    (Dir::Rx, Family::V6) => 1,
                    (Dir::Tx, Family::V4) => 2,
                    (Dir::Tx, Family::V6) => 3,
                };
                state.cur[idx] += bytes;

                // flow bookkeeping (ports + owning application)
                let (is_tcp, sport, dport) = match &headers.transport {
                    Some(TransportHeader::Tcp(h)) => (true, h.source_port, h.destination_port),
                    Some(TransportHeader::Udp(h)) => (false, h.source_port, h.destination_port),
                    _ => continue,
                };
                let remote = if outgoing { dest } else { source };
                let local_port = if outgoing { sport } else { dport };
                let key = FlowKey {
                    remote,
                    remote_port: if outgoing { dport } else { sport },
                    local_port,
                    is_tcp,
                };
                let seen = now_secs();
                let program = {
                    let entry = state.flows.entry(key).or_insert_with(|| FlowStat {
                        program: port_map::program_for(is_tcp, family == Family::V4, local_port)
                            .map(|s| Arc::<str>::from(s))
                            .unwrap_or_else(|| Arc::from("其他")),
                        last_seen: seen,
                        ..FlowStat::default()
                    });
                    if dir == Dir::Rx {
                        entry.rx += bytes;
                    } else {
                        entry.tx += bytes;
                    }
                    entry.last_seen = seen;
                    Arc::clone(&entry.program)
                };
                // 按进程/协议族聚合到秒级桶，advance_second 时批量落历史：
                // 避免每包做日期/进程名字符串分配与哈希（高频流量下的热点）
                let agg = state
                    .app_cur
                    .entry(AppKey { program, family })
                    .or_insert((0, 0));
                if dir == Dir::Rx {
                    agg.0 += bytes;
                } else {
                    agg.1 += bytes;
                }
                if state.flows.len() > 256 {
                    if let Some(oldest) = state
                        .flows
                        .iter()
                        .min_by_key(|(_, st)| st.last_seen)
                        .map(|(k, _)| *k)
                    {
                        state.flows.remove(&oldest);
                    }
                }
            }
            Err(pcap::Error::TimeoutExpired) | Err(pcap::Error::NoMorePackets) => {}
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn advance_second(app: &AppHandle, state: &mut DeviceState) {
    state.cur_sec = now_secs();

    // re-resolve program names for flows still marked unknown: the OS port
    // table refreshes every 3s, so connections missed at creation may match
    // now; retry every unknown flow once per second
    let keys: Vec<FlowKey> = state
        .flows
        .iter()
        .filter(|(_, s)| s.program.as_ref() == "其他")
        .map(|(k, _)| *k)
        .collect();
    for k in keys {
        if let Some(name) = port_map::program_for(k.is_tcp, k.remote.is_ipv4(), k.local_port) {
            if let Some(st) = state.flows.get_mut(&k) {
                st.program = name.into();
            }
        }
    }
    let [rx_v4, rx_v6, tx_v4, tx_v6] = state.cur;
    state.cur = [0; 4];

    // 每秒批量记录历史：原来每包调用一次 record/record_app，
    // 高频流量下（每包 Arc 克隆 + 哈希 + 字符串分配）是抓包线程热点
    traffic_history::record(&state.adapter, Family::V4, Dir::Rx, rx_v4);
    traffic_history::record(&state.adapter, Family::V6, Dir::Rx, rx_v6);
    traffic_history::record(&state.adapter, Family::V4, Dir::Tx, tx_v4);
    traffic_history::record(&state.adapter, Family::V6, Dir::Tx, tx_v6);
    let app_drained: HashMap<AppKey, (u64, u64)> = std::mem::take(&mut state.app_cur);
    for (k, (rx, tx)) in app_drained {
        traffic_history::record_app_totals(&k.program, k.family, rx, tx);
    }

    let mut flows: Vec<FlowInfo> = state
        .flows
        .iter()
        .map(|(k, st)| FlowInfo {
            device: state.display.clone(),
            remote: k.remote.to_string(),
            remote_port: k.remote_port,
            local_port: k.local_port,
            proto: if k.is_tcp { "TCP" } else { "UDP" }.into(),
            family: if k.remote.is_ipv4() { "v4" } else { "v6" }.into(),
            rx: st.rx,
            tx: st.tx,
            program: st.program.to_string(),
        })
        .collect();
    flows.sort_by(|a, b| (b.rx + b.tx).cmp(&(a.rx + a.tx)));
    flows.truncate(24);

    let _ = app.emit(
        "traffic-tick",
        serde_json::json!({
            "device": state.display,
            "rxV4": rx_v4,
            "rxV6": rx_v6,
            "txV4": tx_v4,
            "txV6": tx_v6,
            "totalRx": rx_v4 + rx_v6,
            "totalTx": tx_v4 + tx_v6,
            "flows": flows,
        }),
    );
}

/// Ethernet and raw-IP link types cover virtually all desktop adapters.
fn sniffable_headers(
    packet: &[u8],
    datalink: pcap::Linktype,
) -> Option<LaxPacketHeaders<'_>> {
    match datalink {
        pcap::Linktype(1) => LaxPacketHeaders::from_ethernet(packet).ok(),
        pcap::Linktype(101) | pcap::Linktype(12) => LaxPacketHeaders::from_ip(packet).ok(),
        _ => LaxPacketHeaders::from_ethernet(packet).ok(),
    }
}
