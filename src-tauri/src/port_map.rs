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
    /// pid -> 承载的 Windows 服务名（把 svchost.exe 归属到具体服务）
    services: HashMap<u32, String>,
}

static CACHE: RwLock<Option<Arc<Cache>>> = RwLock::new(None);

/// Returns the process name owning the given local port, if known.
/// Shared svchost.exe processes are reported as `svchost:服务名[,服务名…]`.
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
    let name = cache.names.get(&pid)?;
    if name.eq_ignore_ascii_case("svchost.exe") {
        if let Some(svc) = cache.services.get(&pid) {
            return Some(format!("svchost:{}", svc));
        }
    }
    Some(name.clone())
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

    let services = collect_services();

    Cache {
        ports,
        names,
        services,
    }
}

/// Enumerates Win32 services and maps each hosting PID to its service names,
/// so traffic inside a shared svchost.exe can be attributed to the actual
/// service (e.g. Dnscache / WSearch) instead of a bare "svchost.exe".
#[cfg(windows)]
fn collect_services() -> HashMap<u32, String> {
    use windows::Win32::System::Services::{
        CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW,
        ENUM_SERVICE_STATUS_PROCESSW, SC_ENUM_PROCESS_INFO, SC_MANAGER_ENUMERATE_SERVICE,
        SERVICE_STATE_ALL, SERVICE_WIN32,
    };
    use windows::core::{PCWSTR, PWSTR};

    let mut map: HashMap<u32, Vec<String>> = HashMap::new();
    unsafe {
        let Ok(scm) = OpenSCManagerW(None, None, SC_MANAGER_ENUMERATE_SERVICE) else {
            return HashMap::new();
        };
        let mut needed = 0u32;
        let mut returned = 0u32;
        let mut resume = 0u32;
        // 首次调用故意用空缓冲：只为取得所需字节数（以 ERROR_MORE_DATA 失败）
        let _ = EnumServicesStatusExW(
            scm,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            None,
            &mut needed,
            &mut returned,
            Some(&mut resume),
            PCWSTR::null(),
        );
        if needed > 0 {
            let mut buf = vec![0u8; needed as usize];
            if EnumServicesStatusExW(
                scm,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                Some(buf.as_mut_slice()),
                &mut needed,
                &mut returned,
                Some(&mut resume),
                PCWSTR::null(),
            )
            .is_ok()
            {
                let stride = std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
                for i in 0..returned as usize {
                    let entry =
                        &*(buf.as_ptr().add(i * stride) as *const ENUM_SERVICE_STATUS_PROCESSW);
                    let pid = entry.ServiceStatusProcess.dwProcessId;
                    if pid == 0 || entry.lpServiceName.is_null() {
                        continue;
                    }
                    if let Ok(name) = PWSTR(entry.lpServiceName.as_ptr()).to_string() {
                        map.entry(pid).or_default().push(name);
                    }
                }
            }
        }
        let _ = CloseServiceHandle(scm);
    }
    map.into_iter()
        .map(|(pid, mut names)| {
            names.dedup();
            let mut s = names.join(",");
            if s.chars().count() > 48 {
                s = format!("{}…", s.chars().take(47).collect::<String>());
            }
            (pid, s)
        })
        .collect()
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
