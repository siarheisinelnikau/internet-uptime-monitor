//! Single-instance activation and stop commands use local files, never a TCP port.
use crate::{model::now, monitor::App, updates::VERSION};
use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub pid: u32,
    pub version: String,
    pub id: String,
    pub ready: bool,
    pub demo: bool,
}
#[derive(Serialize, Deserialize)]
struct Command {
    session: String,
    action: String,
}
pub struct Instance {
    _lock: File,
    data: PathBuf,
    session: Mutex<Session>,
}
fn filename(demo: bool) -> &'static str {
    if demo {
        "demo-instance.json"
    } else {
        "instance.json"
    }
}
pub fn read(data: &Path, demo: bool) -> Option<Session> {
    let path = data.join(filename(demo));
    if fs::metadata(&path).ok()?.len() > 16384 {
        return None;
    }
    let session: Session = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (session.pid > 0
        && !session.id.is_empty()
        && session.id.len() < 100
        && session
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-'))
    .then_some(session)
}
pub fn is_locked(data: &Path, demo: bool) -> bool {
    let path = data.join(if demo { "demo.lock" } else { "monitor.lock" });
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    file.try_lock_exclusive().is_err()
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::Builder::new()
        .prefix(".pending-")
        .tempfile_in(path.parent().context("Missing data folder")?)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
impl Instance {
    pub fn acquire(data: &Path, demo: bool) -> Result<Option<Arc<Self>>> {
        fs::create_dir_all(data)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(data.join(if demo { "demo.lock" } else { "monitor.lock" }))?;
        if lock.try_lock_exclusive().is_err() {
            return Ok(None);
        }
        let instance = Arc::new(Self {
            _lock: lock,
            data: data.to_path_buf(),
            session: Mutex::new(Session {
                pid: std::process::id(),
                version: VERSION.into(),
                id: format!("{}-{}", std::process::id(), (now() * 1_000_000.0) as u64),
                ready: false,
                demo,
            }),
        });
        instance.publish()?;
        Ok(Some(instance))
    }
    fn publish(&self) -> Result<()> {
        let session = self.session.lock().expect("session");
        atomic_write(
            &self.data.join(filename(session.demo)),
            &serde_json::to_vec(&*session)?,
        )
    }
    pub fn ready(&self) -> Result<()> {
        self.session.lock().expect("session").ready = true;
        self.publish()
    }
    pub fn poll(&self, app: &App) {
        let session = self.session.lock().expect("session").clone();
        let prefix = format!("command-{}-", session.id);
        let Ok(entries) = fs::read_dir(&self.data) else {
            return;
        };
        for entry in entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        {
            if let Ok(bytes) = fs::read(entry.path())
                && let Ok(command) = serde_json::from_slice::<Command>(&bytes)
                && command.session == session.id
            {
                match command.action.as_str() {
                    "open" => app.request_window(),
                    "stop" => app.stop(),
                    _ => {}
                }
            }
            let _ = fs::remove_file(entry.path());
        }
    }
    pub fn start(self: &Arc<Self>, app: &Arc<App>) -> thread::JoinHandle<()> {
        let instance = self.clone();
        let app = app.clone();
        thread::spawn(move || {
            while !app.is_stopping() {
                instance.poll(&app);
                thread::sleep(Duration::from_millis(150));
            }
        })
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        let session = self.session.lock().expect("session");
        if read(&self.data, session.demo).is_some_and(|s| s.id == session.id) {
            let _ = fs::remove_file(self.data.join(filename(session.demo)));
        }
    }
}
pub fn send(data: &Path, demo: bool, action: &str) -> Result<bool> {
    let Some(session) = read(data, demo) else {
        return Ok(false);
    };
    #[cfg(windows)]
    if action == "open" {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(session.pid);
        }
    }
    let path = data.join(format!(
        "command-{}-{}-{}.json",
        session.id,
        std::process::id(),
        (now() * 1_000_000.0) as u64
    ));
    atomic_write(
        &path,
        &serde_json::to_vec(&Command {
            session: session.id,
            action: action.into(),
        })?,
    )?;
    Ok(true)
}

/// The 0.1 updater expects an HTTP readiness reply. Only an active legacy update
/// gets this short-lived compatibility listener; normal native startup binds nothing.
pub fn legacy_update_port(data: &Path, target: &Path) -> Option<u16> {
    let target = target.canonicalize().ok()?;
    for entry in fs::read_dir(data.join("updates")).ok()?.flatten() {
        if entry.path().join("completed").exists() {
            continue;
        }
        let Some(plan) = fs::read(entry.path().join("plan.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<crate::updates::UpdatePlan>(&b).ok())
        else {
            continue;
        };
        if plan.protocol == 1
            && plan.version == VERSION
            && plan.target.canonicalize().ok().as_ref() == Some(&target)
        {
            return Some(plan.port);
        }
    }
    None
}
pub fn legacy_ready(port: u16, app: &Arc<App>, instance: &Arc<Instance>) {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
        return;
    };
    let _ = listener.set_nonblocking(true);
    let app = app.clone();
    let instance = instance.clone();
    thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(12);
        while Instant::now() < until && !app.is_stopping() {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                let mut request = [0; 2048];
                if let Ok(n) = stream.read(&mut request)
                    && request[..n].starts_with(b"GET /api/health ")
                    && instance.session.lock().expect("session").ready
                {
                    let body=serde_json::json!({"application":"internet-uptime-monitor","version":VERSION,"pid":std::process::id(),"ok":true}).to_string();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
            } else {
                thread::sleep(Duration::from_millis(50));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Store;
    #[test]
    fn duplicate_launch_activates_existing_instance_without_a_socket() {
        let data = tempfile::tempdir().unwrap();
        let first = Instance::acquire(data.path(), false).unwrap().unwrap();
        assert!(Instance::acquire(data.path(), false).unwrap().is_none());
        let app = App::new(Store::memory().unwrap(), false).unwrap();
        app.window_visible
            .store(false, std::sync::atomic::Ordering::Release);
        send(data.path(), false, "open").unwrap();
        first.poll(&app);
        assert!(
            app.window_visible
                .load(std::sync::atomic::Ordering::Acquire)
        );
        send(data.path(), false, "stop").unwrap();
        first.poll(&app);
        assert!(app.is_stopping());
    }
    #[test]
    fn stale_session_is_replaced_and_readiness_is_explicit() {
        let data = tempfile::tempdir().unwrap();
        let first = Instance::acquire(data.path(), false).unwrap().unwrap();
        assert!(!read(data.path(), false).unwrap().ready);
        first.ready().unwrap();
        assert!(read(data.path(), false).unwrap().ready);
        drop(first);
        assert!(read(data.path(), false).is_none());
        assert!(Instance::acquire(data.path(), false).unwrap().is_some());
    }
}
