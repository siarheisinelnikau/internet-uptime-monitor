use crate::{
    model::{Config, Probe, Sample, State, Status, now},
    probes,
    storage::Store,
};
use anyhow::{Result, bail};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub type UiWaker = Box<dyn Fn(bool) + Send + Sync>;
pub struct App {
    pub tray_available: AtomicBool,
    pub window_visible: AtomicBool,
    show_requested: AtomicBool,
    pub ui_notice: Mutex<Option<String>>,
    ui_waker: Mutex<Option<UiWaker>>,
    ui_refresh_waker: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub updates: Arc<crate::updates::Updater>,
    pub cleanup_update: Mutex<Option<std::path::PathBuf>>,
    pub store: Mutex<Store>,
    pub status: Mutex<Status>,
    pub stopping: AtomicBool,
    wake: Condvar,
}

impl App {
    pub fn new(store: Store, demo: bool) -> Result<Arc<Self>> {
        let config = store.config()?;
        let latest = if demo { store.latest()? } else { None };
        let gateway = latest.as_ref().and_then(|s| s.router.address.clone());
        Ok(Arc::new(Self {
            tray_available: AtomicBool::new(false),
            window_visible: AtomicBool::new(true),
            show_requested: AtomicBool::new(false),
            ui_notice: Mutex::new(None),
            ui_waker: Mutex::new(None),
            ui_refresh_waker: Mutex::new(None),
            updates: Arc::new(crate::updates::Updater::default()),
            cleanup_update: Mutex::new(None),
            store: Mutex::new(store),
            status: Mutex::new(Status {
                paused: false,
                latest,
                error: None,
                gateway,
                config,
                demo,
                generation: 0,
            }),
            stopping: AtomicBool::new(false),
            wake: Condvar::new(),
        }))
    }
    pub fn snapshot(&self) -> Status {
        self.status.lock().expect("status lock").clone()
    }
    pub fn configure(&self, config: Config) -> Result<()> {
        config.validate()?;
        let mut status = self.status.lock().expect("status lock");
        if status.demo {
            bail!("Demo settings are read-only. Start without --demo to monitor your connection.");
        }
        let mut store = self.store.lock().expect("store lock");
        store.save_config(&config)?;
        store.interrupt()?;
        status.config = config;
        status.generation += 1;
        status.latest = None;
        self.wake.notify_all();
        self.refresh_ui();
        Ok(())
    }
    /// Appearance changes preserve the active observation and pending monitoring edits.
    pub fn set_appearance(
        &self,
        language: crate::i18n::Language,
        theme: crate::i18n::Theme,
    ) -> Result<()> {
        let mut status = self.status.lock().expect("status lock");
        let mut config = status.config.clone();
        config.language = language;
        config.theme = theme;
        if !status.demo {
            self.store
                .lock()
                .expect("store lock")
                .save_config(&config)?;
        }
        status.config = config;
        drop(status);
        self.refresh_ui();
        Ok(())
    }
    pub fn control(&self, action: &str) -> Result<()> {
        let mut status = self.status.lock().expect("status lock");
        if status.demo {
            bail!("This is a read-only demo. Start without --demo for live monitoring.");
        }
        status.paused = match action {
            "pause" => true,
            "resume" => false,
            _ => bail!("Choose pause or resume."),
        };
        self.store.lock().expect("store lock").interrupt()?;
        status.generation += 1;
        status.latest = None;
        self.wake.notify_all();
        self.refresh_ui();
        Ok(())
    }
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.wake.notify_all();
        if let Some(wake) = self.ui_waker.lock().expect("UI waker").as_ref() {
            wake(false);
        }
    }
    pub fn set_ui_waker(&self, wake: UiWaker) {
        *self.ui_waker.lock().expect("UI waker") = Some(wake);
    }
    pub fn clear_ui_waker(&self) {
        *self.ui_waker.lock().expect("UI waker") = None;
        *self.ui_refresh_waker.lock().expect("UI refresh waker") = None;
    }
    pub fn set_ui_refresh_waker(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.ui_refresh_waker.lock().expect("UI refresh waker") = Some(Box::new(wake));
    }
    fn refresh_ui(&self) {
        if self.window_visible.load(Ordering::Acquire)
            && let Some(wake) = self
                .ui_refresh_waker
                .lock()
                .expect("UI refresh waker")
                .as_ref()
        {
            wake();
        }
    }
    pub fn request_window(&self) {
        self.show_requested.store(true, Ordering::Release);
        self.window_visible.store(true, Ordering::Release);
        if let Some(wake) = self.ui_waker.lock().expect("UI waker").as_ref() {
            wake(true);
        }
    }
    #[cfg(windows)]
    pub(crate) fn take_show_request(&self) -> bool {
        self.show_requested.swap(false, Ordering::AcqRel)
    }
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }
    #[cfg(windows)]
    pub(crate) fn acknowledge_update_startup(&self) {
        if let Some(directory) = self.cleanup_update.lock().expect("update startup").take() {
            crate::updates::cleanup_after_update(directory);
        }
    }
    pub fn start(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        if self.snapshot().demo {
            return None;
        }
        let app = Arc::clone(self);
        Some(thread::spawn(move || app.run()))
    }
    fn run(&self) {
        let mut gateway = None;
        let mut refreshed = None::<Instant>;
        let mut last_generation = u64::MAX;
        while !self.is_stopping() {
            let status = self.snapshot();
            if status.paused {
                let guard = self.status.lock().expect("status lock");
                let _ = self.wake.wait_timeout(guard, Duration::from_secs(1));
                continue;
            }
            let started = Instant::now();
            if last_generation != status.generation {
                refreshed = None;
            }
            if !status.config.router.is_empty() {
                gateway = Some(status.config.router.clone());
            } else if refreshed.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                gateway = probes::discover_gateway();
                refreshed = Some(Instant::now());
            }
            last_generation = status.generation;
            let sample = probes::check(&status.config, gateway.as_deref());
            let mut current = self.status.lock().expect("status lock");
            if current.generation != status.generation || self.is_stopping() {
                continue;
            }
            let result = self.store.lock().expect("store lock").record(
                &sample,
                (status.config.interval_seconds as f64 * 3.0).max(30.0),
            );
            current.gateway = gateway.clone();
            match result {
                Ok(()) => {
                    current.error = (sample.state == State::Unknown).then(|| {
                        "A probe could not run. Check ICMP permissions and your targets.".into()
                    });
                    current.latest = Some(sample);
                }
                Err(error) => {
                    current.error = Some(format!("Could not save the check: {error}"));
                    current.latest = None;
                }
            }
            let wait = Duration::from_secs(status.config.interval_seconds as u64)
                .saturating_sub(started.elapsed());
            self.refresh_ui();
            let _ = self.wake.wait_timeout_while(current, wait, |s| {
                s.generation == status.generation && !self.is_stopping()
            });
        }
        let _ = self.store.lock().expect("store lock").interrupt();
    }
}

pub fn seed_demo(store: &mut Store) -> Result<()> {
    let config = Config::default();
    let now = (now() / 20.0).floor() as i64 * 20;
    let durations = [20, 40, 20, 40, 60, 20, 40, 60, 20, 40, 80, 40];
    for timestamp in ((now - 86400)..=now).step_by(20) {
        let state = durations
            .iter()
            .enumerate()
            .find_map(|(i, &duration)| {
                let start = now - (20 + i as i64 * 37) * 60;
                (timestamp >= start && timestamp < start + duration).then_some(
                    if i == 2 || i == 7 {
                        State::Local
                    } else {
                        State::Upstream
                    },
                )
            })
            .unwrap_or(State::Online);
        let targets = config
            .targets
            .iter()
            .enumerate()
            .map(|(i, t)| Probe {
                name: Some(t.name.clone()),
                address: Some(t.address.clone()),
                ok: Some(state == State::Online),
                latency_ms: (state == State::Online).then_some(
                    18.0 + i as f64 * 5.0 + 8.0 * (timestamp as f64 / 370.0).sin().abs(),
                ),
                error: (state != State::Online).then(|| "No reply".into()),
            })
            .collect();
        store.record(
            &Sample {
                checked_at: timestamp as f64,
                state,
                targets,
                router: Probe {
                    name: None,
                    address: Some("192.168.1.1".into()),
                    ok: Some(state != State::Local),
                    latency_ms: (state != State::Local).then_some(1.0),
                    error: (state == State::Local).then(|| "No reply".into()),
                },
            },
            60.0,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_is_isolated_and_controls_are_read_only() {
        let mut store = Store::memory().unwrap();
        seed_demo(&mut store).unwrap();
        let r = store.report(now() - 86400.0, now(), 5, 0).unwrap();
        assert_eq!(r.outage_count, 12);
        assert_eq!(r.downtime_seconds, 480.0);
        let app = App::new(store, true).unwrap();
        assert!(app.snapshot().demo);
        assert!(app.control("pause").is_err());
        assert!(app.configure(Config::default()).is_err());
    }
    #[test]
    fn preferences_and_pause_resume_are_persisted() {
        let app = App::new(Store::memory().unwrap(), false).unwrap();
        app.control("pause").unwrap();
        assert!(app.snapshot().paused);
        app.control("resume").unwrap();
        assert!(!app.snapshot().paused);
        let config = Config {
            interval_seconds: 10,
            ..Config::default()
        };
        app.configure(config.clone()).unwrap();
        assert_eq!(app.store.lock().unwrap().config().unwrap(), config);
        assert!(app.control("invalid").is_err());
    }
    #[test]
    fn appearance_persists_without_resetting_observation_or_monitoring_state() {
        let app = App::new(Store::memory().unwrap(), false).unwrap();
        app.control("pause").unwrap();
        let generation = app.snapshot().generation;
        app.set_appearance(crate::i18n::Language::Russian, crate::i18n::Theme::Dark)
            .unwrap();
        let status = app.snapshot();
        assert!(status.paused);
        assert_eq!(status.generation, generation);
        assert_eq!(status.config.interval_seconds, 5);
        let persisted = app.store.lock().unwrap().config().unwrap();
        assert_eq!(persisted.language, crate::i18n::Language::Russian);
        assert_eq!(persisted.theme, crate::i18n::Theme::Dark);
    }
}
