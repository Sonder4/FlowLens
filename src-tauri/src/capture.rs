//! Packet capture engine: one thread per device, per-second aggregation into
//! {rx,tx} × {IPv4,IPv6} buckets plus a small per-remote-IP connection table,
//! pushed to the frontend as `traffic-tick` events and persisted into the
//! SQLite history (minute buckets, permanent day/month rollups).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use etherparse::{LaxPacketHeaders, NetHeaders};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::traffic_history::{self, Dir, Family};

/// One remote endpoint of the live connection table.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnInfo {
    pub remote: String,
    pub family: String,
    pub rx: u64,
    pub tx: u64,
    pub service: String,
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

#[derive(Default)]
struct ConnStat {
    rx: u64,
    tx: u64,
    last_seen: u64,
}

struct DeviceState {
    display: String,
    adapter: Arc<str>,
    local_addrs: Vec<IpAddr>,
    cur: [u64; 4], // rx_v4, rx_v6, tx_v4, tx_v6
    conns: HashMap<IpAddr, ConnStat>,
    cur_sec: u64,
}

pub fn list_devices() -> Vec<DeviceInfo> {
    pcap::Device::list()
        .unwrap_or_default()
        .into_iter()
        .map(|d| DeviceInfo {
            addresses: d.addresses.iter().map(|a| a.addr.to_string()).collect(),
            desc: d.desc,
            name: d.name,
        })
        .filter(|d| !d.addresses.is_empty())
        .collect()
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
            conns: HashMap::new(),
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
    let _ = app.emit(
        "capture-state",
        serde_json::json!({ "running": spawned }),
    );
}

pub fn stop() {
    if let Some(running) = RUNNING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        running.stop.store(true, Ordering::Relaxed);
        // give the threads a moment to observe the flag and close their channels
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
    let Ok(mut cap) = pcap::Capture::from_device(pcap_name.as_str())
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
        // keep the per-second cadence even when no packets arrive
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

                traffic_history::record(&state.adapter, family, dir, bytes);

                let idx = match (dir, family) {
                    (Dir::Rx, Family::V4) => 0,
                    (Dir::Rx, Family::V6) => 1,
                    (Dir::Tx, Family::V4) => 2,
                    (Dir::Tx, Family::V6) => 3,
                };
                state.cur[idx] += bytes;

                let remote = if outgoing { dest } else { source };
                let entry = state.conns.entry(remote).or_default();
                if dir == Dir::Rx {
                    entry.rx += bytes;
                } else {
                    entry.tx += bytes;
                }
                entry.last_seen = now_secs();
                // bound the table: drop the least recently seen entry
                if state.conns.len() > 128 {
                    if let Some(oldest) = state
                        .conns
                        .iter()
                        .min_by_key(|(_, s)| s.last_seen)
                        .map(|(k, _)| *k)
                    {
                        state.conns.remove(&oldest);
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
    let [rx_v4, rx_v6, tx_v4, tx_v6] = state.cur;
    state.cur = [0; 4];

    let mut conns: Vec<ConnInfo> = state
        .conns
        .iter()
        .map(|(ip, s)| ConnInfo {
            remote: ip.to_string(),
            family: if ip.is_ipv4() { "v4" } else { "v6" }.into(),
            rx: s.rx,
            tx: s.tx,
            service: "-".into(),
        })
        .collect();
    conns.sort_by(|a, b| (b.rx + b.tx).cmp(&(a.rx + a.tx)));
    conns.truncate(8);

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
            "conns": conns,
        }),
    );
}

/// Ethernet and raw-IP link types cover virtually all desktop adapters.
fn sniffable_headers(
    packet: &[u8],
    datalink: pcap::Linktype,
) -> Option<LaxPacketHeaders<'_>> {
    match datalink {
        pcap::Linktype(1) => LaxPacketHeaders::from_ethernet(packet).ok(), // Ethernet
        pcap::Linktype(101) | pcap::Linktype(12) => {
            LaxPacketHeaders::from_ip(packet).ok()
        }
        _ => LaxPacketHeaders::from_ethernet(packet).ok(),
    }
}
