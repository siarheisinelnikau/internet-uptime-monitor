//! Product settings and status adapter for the shared portable updater.
use crate::{model::now, monitor::App};
use anyhow::{Context, Result, ensure};
use rust_auto_updator::{Config, Event, Release, Service};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = "siarheisinelnikau/internet-uptime-monitor";

pub fn config() -> Result<Config> {
    let mut config = Config::new(
        "siarheisinelnikau",
        "internet-uptime-monitor",
        VERSION,
        "InternetUptimeMonitor",
    )
    .map_err(anyhow::Error::msg)?;
    config.max_executable_size = 100 * 1024 * 1024;
    config.expected_subsystem = Some(2);
    config.install.stage_prefix = ".uptime-update-".into();
    config.install.startup_health_timeout = Some(Duration::from_secs(30));
    Ok(config)
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    pub current_version: String,
    pub phase: String,
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub checked_at: Option<f64>,
    pub error: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub install_supported: bool,
}
impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            current_version: VERSION.into(),
            phase: "idle".into(),
            latest_version: None,
            release_url: None,
            checked_at: None,
            error: None,
            downloaded_bytes: 0,
            total_bytes: 0,
            install_supported: cfg!(windows),
        }
    }
}
impl UpdateStatus {
    fn busy(&self) -> bool {
        matches!(
            self.phase.as_str(),
            "checking" | "downloading" | "restarting"
        )
    }
}
#[derive(Clone, Debug)]
pub struct Runtime {
    pub data: PathBuf,
    pub no_tray: bool,
    pub headless: bool,
}
#[derive(Default)]
pub struct Updater {
    state: Mutex<UpdateStatus>,
    available: Mutex<Option<Box<Release>>>,
    runtime: Mutex<Option<Runtime>>,
    waker: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}
impl Updater {
    pub fn snapshot(&self) -> UpdateStatus {
        self.state.lock().expect("update state").clone()
    }
    pub fn configure(&self, runtime: Runtime) {
        *self.runtime.lock().expect("update runtime") = Some(runtime);
    }
    pub fn set_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.waker.lock().expect("update waker") = Some(Box::new(wake));
    }
    pub fn clear_waker(&self) {
        *self.waker.lock().expect("update waker") = None;
    }
    fn changed(&self) {
        if let Some(wake) = self.waker.lock().expect("update waker").as_ref() {
            wake();
        }
    }
    fn fail(&self, error: String) {
        {
            let mut state = self.state.lock().expect("update state");
            state.phase = "error".into();
            state.error = Some(error);
        }
        self.changed();
    }
    pub fn check(self: &Arc<Self>) -> Result<()> {
        {
            let mut state = self.state.lock().expect("update state");
            ensure!(!state.busy(), "An update operation is already in progress.");
            state.phase = "checking".into();
            state.error = None;
        }
        self.changed();
        let updater = self.clone();
        thread::spawn(move || {
            let result = rust_auto_updator::check_latest(&match config() {
                Ok(config) => config,
                Err(error) => {
                    updater.fail(error.to_string());
                    return;
                }
            });
            {
                let mut state = updater.state.lock().expect("update state");
                state.checked_at = Some(now());
                match result {
                    Ok(Some(release)) => {
                        state.latest_version = Some(release.version.to_string());
                        state.release_url = Some(release.page.clone());
                        state.phase = "available".into();
                        *updater.available.lock().expect("available release") =
                            Some(Box::new(release));
                    }
                    Ok(None) => {
                        state.phase = "up_to_date".into();
                        *updater.available.lock().expect("available release") = None;
                    }
                    Err(error) => {
                        state.phase = "error".into();
                        state.error = Some(error);
                    }
                }
            }
            updater.changed();
        });
        Ok(())
    }
    pub fn install(self: &Arc<Self>, app: &Arc<App>) -> Result<()> {
        ensure!(
            !app.snapshot().demo,
            "Updates cannot be installed from demo mode."
        );
        let runtime = self
            .runtime
            .lock()
            .expect("update runtime")
            .clone()
            .context("The updater is not configured.")?;
        let release;
        {
            let mut state = self.state.lock().expect("update state");
            ensure!(!state.busy(), "An update operation is already in progress.");
            release = self
                .available
                .lock()
                .expect("available release")
                .clone()
                .context("Check for a newer release first.")?;
            state.phase = "downloading".into();
            state.error = None;
            state.downloaded_bytes = 0;
            state.total_bytes = release.size();
        }
        self.changed();
        let updater = self.clone();
        let app = app.clone();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut config = config()?;
                config.install.restart_args = restart_args(
                    &runtime,
                    app.snapshot().paused,
                    !app.window_visible
                        .load(std::sync::atomic::Ordering::Acquire),
                );
                config.install.rollback_args = config.install.restart_args.clone();
                let service = Service::new(config, || {}).map_err(anyhow::Error::msg)?;
                service.download(release).map_err(anyhow::Error::msg)?;
                while let Ok(event) = service.events.recv() {
                    match event {
                        Event::Progress { received, total } => {
                            {
                                let mut state = updater.state.lock().expect("update state");
                                state.downloaded_bytes = received;
                                state.total_bytes = total;
                            }
                            updater.changed();
                        }
                        Event::Ready(result) => {
                            let mut prepared = result.map_err(anyhow::Error::msg)?;
                            // Flush the monitor's store before allowing the helper to take over.
                            app.store.lock().expect("store").interrupt()?;
                            rust_auto_updator::handoff(&mut prepared)
                                .map_err(anyhow::Error::msg)?;
                            updater.state.lock().expect("update state").phase = "restarting".into();
                            app.stop();
                            return Ok(());
                        }
                        Event::Checked(_) => {}
                    }
                }
                anyhow::bail!("The update worker stopped before preparing the update.")
            })();
            if let Err(error) = result {
                updater.fail(format!("Update failed: {error:#}"));
            }
        });
        Ok(())
    }
    pub fn start_automatic(app: &Arc<App>) {
        let weak = Arc::downgrade(app);
        thread::spawn(move || {
            let mut next = Instant::now() + Duration::from_secs(30);
            loop {
                thread::sleep(Duration::from_secs(1));
                let Some(app) = weak.upgrade() else { break };
                if app.is_stopping() {
                    break;
                }
                if Instant::now() >= next {
                    let status = app.snapshot();
                    if !status.demo && status.config.check_updates_automatically {
                        let _ = app.updates.check();
                    }
                    next = Instant::now() + Duration::from_secs(86400);
                }
            }
        });
    }
}
fn restart_args(runtime: &Runtime, paused: bool, hidden: bool) -> Vec<String> {
    let mut args = vec![
        "--data-dir".into(),
        runtime.data.to_string_lossy().into_owned(),
    ];
    for (enabled, flag) in [
        (runtime.no_tray, "--no-tray"),
        (runtime.headless, "--headless"),
        (paused, "--start-paused"),
        (hidden, "--background"),
    ] {
        if enabled {
            args.push(flag.into());
        }
    }
    args
}

/// Read-only compatibility with readiness probes from the released 0.1 helper.
/// That release runs its own copied helper, so the old installation engine need not be duplicated.
#[derive(Debug, Deserialize)]
pub struct UpdatePlan {
    pub protocol: u32,
    pub target: PathBuf,
    pub port: u16,
    pub version: String,
}

pub fn cleanup_after_update(directory: PathBuf) {
    if let Ok(config) = config() {
        rust_auto_updator::cleanup_after_update(directory, config.install);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_preserves_data_monitoring_and_window_state() {
        let runtime = Runtime {
            data: PathBuf::from("C:/History with spaces"),
            no_tray: true,
            headless: true,
        };
        assert_eq!(
            restart_args(&runtime, true, true),
            [
                "--data-dir",
                "C:/History with spaces",
                "--no-tray",
                "--headless",
                "--start-paused",
                "--background"
            ]
        );
        assert_eq!(restart_args(&runtime, false, false).len(), 4);
    }
    #[test]
    fn legacy_readiness_plan_remains_readable() {
        let value = serde_json::json!({"protocol":1,"target":"C:/Monitor.exe","port":8765,"version":VERSION,"parent_pid":12,"sha256":"old","data":"C:/Data"});
        let plan: UpdatePlan = serde_json::from_value(value).unwrap();
        assert_eq!(plan.protocol, 1);
        assert_eq!(plan.port, 8765);
    }
}
