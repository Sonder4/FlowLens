//! Live per-adapter network I/O read from OS counters (sysinfo):
//! works without administrator privileges and without the packet capture.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Serialize;
use sysinfo::Networks;
use tauri::{AppHandle, Emitter};

/// Live input/output rates of one network adapter, in bytes per second.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterIo {
    pub name: String,
    pub rx_speed: u64,
    pub tx_speed: u64,
    pub total_rx: u64,
    pub total_tx: u64,
}

struct Inner {
    networks: Networks,
    last_refresh: Option<Instant>,
    last_totals: std::collections::HashMap<String, (u64, u64)>,
}

fn inner() -> &'static Mutex<Inner> {
    static INNER: OnceLock<Mutex<Inner>> = OnceLock::new();
    INNER.get_or_init(|| {
        Mutex::new(Inner {
            networks: Networks::new(),
            last_refresh: None,
            last_totals: std::collections::HashMap::new(),
        })
    })
}

/// Refreshes the OS counters and computes per-adapter rates. Call at a
/// regular cadence (once per second).
pub fn snapshot() -> Vec<AdapterIo> {
    let mut guard = inner().lock().unwrap_or_else(|p| p.into_inner());
    guard.networks.refresh(true);

    let now = Instant::now();
    let elapsed = guard
        .last_refresh
        .replace(now)
        .map_or(1.0, |t| now.duration_since(t).as_secs_f64().max(0.001));

    let previous = std::mem::take(&mut guard.last_totals);
    let mut current = std::collections::HashMap::new();
    let mut out = Vec::new();

    for (name, data) in guard.networks.iter() {
        let total_rx = data.total_received();
        let total_tx = data.total_transmitted();
        current.insert(name.clone(), (total_rx, total_tx));

        let (prev_rx, prev_tx) = previous
            .get(name)
            .copied()
            .unwrap_or((total_rx, total_tx));
        let secs = elapsed;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rx_speed = (total_rx.saturating_sub(prev_rx) as f64 / secs) as u64;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tx_speed = (total_tx.saturating_sub(prev_tx) as f64 / secs) as u64;

        out.push(AdapterIo {
            name: name.clone(),
            rx_speed,
            tx_speed,
            total_rx,
            total_tx,
        });
    }
    guard.last_totals = current;

    out.sort_by(|a, b| {
        (b.total_rx + b.total_tx).cmp(&(a.total_rx + a.total_tx))
    });
    out
}

/// Background thread emitting the `io-tick` event once per second.
pub fn spawn_emitter(app: AppHandle) {
    std::thread::Builder::new()
        .name("io-emitter".into())
        .spawn(move || loop {
            let snapshot = snapshot();
            let _ = app.emit("io-tick", &snapshot);
            std::thread::sleep(std::time::Duration::from_secs(1));
        })
        .ok();
}
