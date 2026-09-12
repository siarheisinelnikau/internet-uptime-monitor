//! Finite, opt-in renderer diagnostic. No external input or desktop capture.
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub enum Action {
    None,
    Hide,
    Show,
    Exit,
}
pub struct Diagnostic {
    path: PathBuf,
    started: Instant,
    phase: u8,
    baseline: [u64; 3],
    cpu: Option<f64>,
    sample_started: Instant,
    samples: Vec<serde_json::Value>,
}
impl Diagnostic {
    pub fn from_env(name: &str) -> Option<Self> {
        let now = Instant::now();
        Some(Self {
            path: std::env::var_os(name)?.into(),
            started: now,
            phase: 0,
            baseline: [0; 3],
            cpu: None,
            sample_started: now,
            samples: Vec::new(),
        })
    }
    pub fn deadline(&self) -> Instant {
        self.started
            + Duration::from_secs(match self.phase {
                0 => 1,
                1 => 6,
                2 => 12,
                _ => 18,
            })
    }
    pub fn poll(&mut self, counts: [u64; 3]) -> Action {
        if self.phase >= 4 {
            return Action::Exit;
        }
        if Instant::now() < self.deadline() {
            return Action::None;
        }
        let now = Instant::now();
        if self.phase != 0 {
            let elapsed = now.duration_since(self.sample_started).as_secs_f64();
            self.samples.push(serde_json::json!({"state": match self.phase { 1 => "visible_idle", 2 => "hidden", _ => "restored" }, "seconds": elapsed, "layouts":counts[0]-self.baseline[0], "rasters":counts[1]-self.baseline[1], "presents":counts[2]-self.baseline[2], "cpu_one_core_percent": cpu_seconds().zip(self.cpu).map(|(current, previous)| 100.0*(current-previous)/elapsed)}));
        }
        self.baseline = counts;
        self.cpu = cpu_seconds();
        self.sample_started = now;
        self.phase += 1;
        match self.phase {
            1 => Action::None,
            2 => Action::Hide,
            3 => Action::Show,
            _ => {
                let report = serde_json::json!({"backend":"egui CPU rasterizer -> softbuffer Win32 GDI", "samples":self.samples, "scope":"Finite application-owned hide/show sequence; no OS input, screenshot capture, tray/menu or perceptual latency check."});
                if let Err(error) =
                    std::fs::write(&self.path, serde_json::to_vec_pretty(&report).unwrap())
                {
                    eprintln!("Cannot save renderer diagnostic: {error}");
                }
                Action::Exit
            }
        }
    }
}
fn cpu_seconds() -> Option<f64> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return None;
        }
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    Some((ticks(kernel) + ticks(user)) as f64 / 10_000_000.0)
}
