use anyhow::{Result, bail};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    return PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
        .join("InternetUptimeMonitor");
    #[cfg(target_os = "macos")]
    return PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
        .join("Library/Application Support/InternetUptimeMonitor");
    #[cfg(all(not(windows), not(target_os = "macos")))]
    return std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
                .join(".local/share")
        })
        .join("internet-uptime-monitor");
}
pub fn open_dashboard(port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &url])
            .creation_flags(0x08000000)
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(url).spawn()?;
    #[cfg(all(not(windows), not(target_os = "macos")))]
    std::process::Command::new("xdg-open").arg(url).spawn()?;
    Ok(())
}
pub fn local_request(port: u16, method: &str, path: &str) -> Result<serde_json::Value> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    let body = if method == "POST" { "{}" } else { "" };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut response = String::new();
    stream.take(1_000_000).read_to_string(&mut response)?;
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("Invalid local HTTP response."))?;
    if !headers
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 "))
    {
        bail!("The local monitor returned an error: {body}");
    }
    Ok(serde_json::from_str(body)?)
}
pub fn monitor_running(port: u16) -> bool {
    local_request(port, "GET", "/api/health")
        .is_ok_and(|v| v["application"] == "internet-uptime-monitor")
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
pub fn set_startup(enable: bool, data: &Path, port: u16) -> Result<()> {
    use std::ptr;
    use windows_sys::Win32::System::Registry::*;
    let key_name = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = wide("InternetUptimeMonitor");
    unsafe {
        let mut key = ptr::null_mut();
        let code = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            key_name.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if code != 0 {
            bail!("Could not open your startup preferences ({code}).");
        }
        let code = if enable {
            let exe = std::env::current_exe()?;
            let command = wide(&format!(
                "\"{}\" --no-browser --data-dir \"{}\" --port {port}",
                exe.display(),
                data.display()
            ));
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(key, name.as_ptr())
        };
        RegCloseKey(key);
        if code != 0 && (code != 2 || enable) {
            bail!("Could not update automatic startup ({code}).");
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn startup_enabled() -> bool {
    use std::ptr;
    use windows_sys::Win32::System::Registry::*;
    unsafe {
        let mut key = ptr::null_mut();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) != 0
        {
            return false;
        }
        let mut size = 0;
        let code = RegQueryValueExW(
            key,
            wide("InternetUptimeMonitor").as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut size,
        );
        RegCloseKey(key);
        code == 0 && size > 0
    }
}

#[cfg(not(windows))]
pub fn set_startup(_enable: bool, _data: &Path, _port: u16) -> Result<()> {
    bail!("Automatic sign-in startup is currently available on Windows only.")
}

pub fn show_error(message: &str) {
    eprintln!("{message}");
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            wide(message).as_ptr(),
            wide("Internet Uptime Monitor").as_ptr(),
            0x10,
        );
    }
}
