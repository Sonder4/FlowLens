//! Maps local ports to owning process names (Windows IP Helper API).
//! Used to attribute captured flows to applications.
//!
//! A background thread refreshes the TCP/UDP owner tables every 3 seconds;
//! lookups are lock-cheap reads against that cache.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[cfg(windows)]
use sysinfo::System;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortKey {
    pub is_tcp: bool,
    pub is_v4: bool,
    pub port: u16,
}

struct Cache {
    ports: HashMap<PortKey, u32>,
    names: HashMap<u32, String>,
}

static CACHE: RwLock<Option<Arc<Cache>>> = RwLock::new(None);

/// Returns the process name owning the given local port, if known.
pub fn program_for(is_tcp: bool, is_v4: bool, local_port: u16) -> Option<String> {
    let cache = CACHE
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()?;
    let pid = *cache.ports.get(&PortKey {
        is_tcp,
        is_v4,
        port: local_port,
    })?;
    cache.names.get(&pid).cloned()
}

/// Spawns the background refresher thread.
pub fn spawn_refresher() {
    std::thread::Builder::new()
        .name("port-map".into())
        .spawn(loop_fn)
        .ok();
}

#[cfg(windows)]
fn loop_fn() {
    loop {
        let cache = refresh_once();
        *CACHE.write().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(cache));
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

#[cfg(not(windows))]
fn loop_fn() {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

#[cfg(windows)]
fn refresh_once() -> Cache {
    let mut ports = HashMap::new();
    collect_tcp(&mut ports);
    collect_udp(&mut ports);

    let sys = System::new_all();
    let names: HashMap<u32, String> = ports
        .values()
        .filter_map(|pid| {
            sys.process(sysinfo::Pid::from_u32(*pid))
                .map(|p| (*pid, p.name().to_string_lossy().to_string()))
        })
        .collect();

    Cache { ports, names }
}

/// Reads the DWORD-stored port (network byte order in the high half).
#[cfg(windows)]
fn be_port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

#[cfg(windows)]
fn collect_tcp(ports: &mut HashMap<PortKey, u32>) {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPTABLE_OWNER_MODULE, TCP_TABLE_CLASS,
        TCP_TABLE_OWNER_MODULE_ALL,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    for (family, is_v4) in [(AF_INET.0 as u32, true), (AF_INET6.0 as u32, false)] {
        let mut size: u32 = 0;
        let _ = unsafe {
            GetExtendedTcpTable(
                None,
                &mut size,
                false,
                family,
                TCP_TABLE_CLASS(TCP_TABLE_OWNER_MODULE_ALL.0),
                0,
            )
        };
        if size == 0 {
            continue;
        }
        let mut buf = vec![0_u8; size as usize];
        let ret = unsafe {
            GetExtendedTcpTable(
                Some(buf.as_mut_ptr().cast()),
                &mut size,
                false,
                family,
                TCP_TABLE_CLASS(TCP_TABLE_OWNER_MODULE_ALL.0),
                0,
            )
        };
        if ret != 0 {
            continue;
        }
        unsafe {
            let table = &*buf.as_ptr().cast::<MIB_TCPTABLE_OWNER_MODULE>();
            let rows =
                std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
            for row in rows {
                ports.insert(
                    PortKey {
                        is_tcp: true,
                        is_v4,
                        port: be_port(row.dwLocalPort),
                    },
                    row.dwOwningPid,
                );
            }
        }
    }
}

#[cfg(windows)]
fn collect_udp(ports: &mut HashMap<PortKey, u32>) {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedUdpTable, MIB_UDPTABLE_OWNER_MODULE, UDP_TABLE_CLASS,
        UDP_TABLE_OWNER_MODULE,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    for (family, is_v4) in [(AF_INET.0 as u32, true), (AF_INET6.0 as u32, false)] {
        let mut size: u32 = 0;
        let _ = unsafe {
            GetExtendedUdpTable(
                None,
                &mut size,
                false,
                family,
                UDP_TABLE_CLASS(UDP_TABLE_OWNER_MODULE.0),
                0,
            )
        };
        if size == 0 {
            continue;
        }
        let mut buf = vec![0_u8; size as usize];
        let ret = unsafe {
            GetExtendedUdpTable(
                Some(buf.as_mut_ptr().cast()),
                &mut size,
                false,
                family,
                UDP_TABLE_CLASS(UDP_TABLE_OWNER_MODULE.0),
                0,
            )
        };
        if ret != 0 {
            continue;
        }
        unsafe {
            let table = &*buf.as_ptr().cast::<MIB_UDPTABLE_OWNER_MODULE>();
            let rows =
                std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
            for row in rows {
                ports.insert(
                    PortKey {
                        is_tcp: false,
                        is_v4,
                        port: be_port(row.dwLocalPort),
                    },
                    row.dwOwningPid,
                );
            }
        }
    }
}
