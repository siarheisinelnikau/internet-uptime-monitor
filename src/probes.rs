use crate::model::{Config, Probe, Sample, classify, now};
use std::thread;

pub fn check(config: &Config, gateway: Option<&str>) -> Sample {
    let (targets, router) = thread::scope(|scope| {
        let handles: Vec<_> = config
            .targets
            .iter()
            .map(|target| {
                scope.spawn(move || {
                    let mut result = ping(&target.address, config.timeout_ms);
                    result.name = Some(target.name.clone());
                    result
                })
            })
            .collect();
        let router = scope.spawn(|| {
            gateway
                .map(|address| ping(address, config.timeout_ms))
                .unwrap_or_else(|| {
                    Probe::unknown(
                        None,
                        "No gateway detected. Set your router address in Settings.",
                    )
                })
        });
        (
            handles
                .into_iter()
                .map(|h| h.join().expect("probe thread"))
                .collect::<Vec<_>>(),
            router.join().expect("router probe"),
        )
    });
    Sample {
        checked_at: now(),
        state: classify(&targets, &router),
        targets,
        router,
    }
}

#[cfg(windows)]
pub fn ping(address: &str, timeout_ms: u32) -> Probe {
    use std::{ffi::c_void, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, INVALID_HANDLE_VALUE},
        NetworkManagement::IpHelper::{
            ICMP_ECHO_REPLY, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
        },
    };
    let ip = match address.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip,
        Err(e) => return Probe::unknown(Some(address.into()), e.to_string()),
    };
    // Each probe owns its handle and aligned reply storage; no pointers escape this call.
    unsafe {
        let handle = IcmpCreateFile();
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Probe::unknown(
                Some(address.into()),
                format!("Cannot open the Windows ICMP driver ({})", GetLastError()),
            );
        }
        let payload = b"uptime-monitor";
        let size = std::mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 8;
        let mut reply = vec![0u64; size.div_ceil(8)];
        let count = IcmpSendEcho(
            handle,
            u32::from_ne_bytes(ip.octets()),
            payload.as_ptr() as *const c_void,
            payload.len() as u16,
            ptr::null(),
            reply.as_mut_ptr().cast(),
            (reply.len() * 8) as u32,
            timeout_ms,
        );
        let error = GetLastError();
        IcmpCloseHandle(handle);
        if count > 0 {
            let result = &*reply.as_ptr().cast::<ICMP_ECHO_REPLY>();
            return Probe {
                name: None,
                address: Some(address.into()),
                ok: Some(result.Status == 0),
                latency_ms: (result.Status == 0).then_some(result.RoundTripTime as f64),
                error: (result.Status != 0).then(|| format!("ICMP status {}", result.Status)),
            };
        }
        if matches!(error, 11002..=11005 | 11010 | 1231 | 1232) {
            Probe {
                name: None,
                address: Some(address.into()),
                ok: Some(false),
                latency_ms: None,
                error: Some(format!("No reply ({error})")),
            }
        } else {
            Probe::unknown(
                Some(address.into()),
                format!("Windows could not execute the ICMP probe ({error})"),
            )
        }
    }
}

#[cfg(windows)]
pub fn discover_gateway() -> Option<String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIpForwardTable, MIB_IPFORWARDTABLE};
    // GetIpForwardTable includes the interface metric in dwForwardMetric1.
    unsafe {
        let mut size = 0;
        GetIpForwardTable(std::ptr::null_mut(), &mut size, 0);
        if size == 0 {
            return None;
        }
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        if GetIpForwardTable(buffer.as_mut_ptr().cast(), &mut size, 0) != 0 {
            return None;
        }
        let table = &*buffer.as_ptr().cast::<MIB_IPFORWARDTABLE>();
        let count = table.dwNumEntries as usize;
        let max_rows = (size as usize).saturating_sub(4) / std::mem::size_of_val(&table.table[0]);
        if count > max_rows {
            return None;
        }
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), count);
        rows.iter()
            .filter(|r| {
                r.dwForwardDest == 0
                    && r.dwForwardMask == 0
                    && r.dwForwardNextHop != 0
                    && r.Anonymous1.dwForwardType != 2
            })
            .min_by_key(|r| r.dwForwardMetric1)
            .map(|r| std::net::Ipv4Addr::from(r.dwForwardNextHop.to_ne_bytes()).to_string())
    }
}

#[cfg(not(windows))]
pub fn ping(address: &str, timeout_ms: u32) -> Probe {
    use std::process::Command;
    let timeout = if cfg!(target_os = "macos") {
        timeout_ms.to_string()
    } else {
        timeout_ms.div_ceil(1000).to_string()
    };
    match Command::new("ping")
        .env("LC_ALL", "C")
        .args(["-n", "-c", "1", "-W", &timeout, address])
        .output()
    {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            let error = String::from_utf8_lossy(&output.stderr);
            if error.contains("Operation not permitted") || error.contains("Permission denied") {
                return Probe::unknown(Some(address.into()), error.trim());
            }
            let latency = text
                .split("time=")
                .nth(1)
                .or_else(|| text.split("time<").nth(1))
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f64>().ok());
            let ok = output.status.success() && latency.is_some();
            Probe {
                name: None,
                address: Some(address.into()),
                ok: Some(ok),
                latency_ms: if ok { latency } else { None },
                error: if ok { None } else { Some("No reply".into()) },
            }
        }
        Err(error) => Probe::unknown(
            Some(address.into()),
            format!("Could not execute ping: {error}"),
        ),
    }
}

#[cfg(not(windows))]
pub fn discover_gateway() -> Option<String> {
    use std::process::Command;
    let value = if cfg!(target_os = "macos") {
        let output = Command::new("route")
            .args(["-n", "get", "default"])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("gateway:")
                    .map(|s| s.trim().to_string())
            })?
    } else {
        let output = Command::new("ip")
            .args(["-j", "-4", "route", "show", "default"])
            .output()
            .ok()?;
        let routes: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).ok()?;
        routes
            .iter()
            .filter(|r| r["gateway"].is_string())
            .min_by_key(|r| r["metric"].as_u64().unwrap_or(0))?["gateway"]
            .as_str()?
            .to_string()
    };
    value
        .parse::<std::net::Ipv4Addr>()
        .ok()
        .map(|ip| ip.to_string())
}
