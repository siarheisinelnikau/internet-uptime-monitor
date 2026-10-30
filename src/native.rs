//! Native egui desktop interface. No browser, webview, or localhost listener.
#[cfg(windows)]
use crate::platform;
use crate::{
    instance::Instance,
    model::{Config, State, now},
    monitor::App,
    reports,
    storage::Report,
    updates,
};
use anyhow::Result;
use chrono::{Local, TimeZone};
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

const INK: Color32 = Color32::from_rgb(32, 67, 53);
const MUTED: Color32 = Color32::from_rgb(112, 128, 113);
const GREEN: Color32 = Color32::from_rgb(133, 176, 110);
const ORANGE: Color32 = Color32::from_rgb(223, 157, 105);
const RED: Color32 = Color32::from_rgb(190, 106, 102);
const LINE: Color32 = Color32::from_rgb(226, 233, 221);

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Overview,
    History,
    Reports,
    Settings,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Period {
    Today,
    Week,
    Month,
}
impl Period {
    fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
        }
    }
    fn range(self) -> (f64, f64) {
        let end = now();
        let start = match self {
            Self::Today => Local::now()
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .and_then(|t| Local.from_local_datetime(&t).earliest())
                .map(|t| t.timestamp() as f64)
                .unwrap_or(end - 86400.0),
            Self::Week => end - 7.0 * 86400.0,
            Self::Month => end - 30.0 * 86400.0,
        };
        (start, end.max(start + 0.001))
    }
}
fn duration(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}
fn timestamp(seconds: f64) -> String {
    Local
        .timestamp_opt(seconds as i64, 0)
        .single()
        .map(|t| t.format("%d %b %H:%M:%S").to_string())
        .unwrap_or_default()
}
fn muted(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text.into()).size(12.0).color(MUTED));
}
fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(8)
        .inner_margin(18)
}
fn state_color(state: &str) -> Color32 {
    match state {
        "online" => GREEN,
        "upstream" => ORANGE,
        "local" => RED,
        "undetermined" => Color32::from_rgb(185, 160, 126),
        _ => LINE,
    }
}
pub fn icon() -> egui::IconData {
    let mut rgba = vec![0; 64 * 64 * 4];
    let points = [
        (12.0f32, 35.0f32),
        (23.0, 35.0),
        (29.0, 17.0),
        (36.0, 48.0),
        (43.0, 31.0),
        (53.0, 31.0),
    ];
    for y in 0..64 {
        for x in 0..64 {
            let offset = (y * 64 + x) * 4;
            let mut color = [32, 76, 64, 255];
            let p = egui::pos2(x as f32, y as f32);
            if points.windows(2).any(|s| {
                let a = egui::pos2(s[0].0, s[0].1);
                let b = egui::pos2(s[1].0, s[1].1);
                let t = ((p - a).dot(b - a) / (b - a).length_sq()).clamp(0.0, 1.0);
                p.distance(a + (b - a) * t) < 2.0
            }) {
                color = [194, 237, 184, 255];
            }
            rgba[offset..offset + 4].copy_from_slice(&color);
        }
    }
    egui::IconData {
        rgba,
        width: 64,
        height: 64,
    }
}

pub struct NativeApp {
    #[cfg(feature = "screenshots")]
    frames: u32,
    app: Arc<App>,
    instance: Arc<Instance>,
    data: PathBuf,
    view: View,
    period: Period,
    report: Option<Report>,
    config: Config,
    cause: usize,
    notice: Option<String>,
    last_refresh: Instant,
    fetching: bool,
    generation: u64,
    sender: mpsc::Sender<(u64, std::result::Result<Report, String>)>,
    receiver: mpsc::Receiver<(u64, std::result::Result<Report, String>)>,
}
impl NativeApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        app: Arc<App>,
        instance: Arc<Instance>,
        data: PathBuf,
    ) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = Color32::from_rgb(247, 248, 245);
        style.visuals.override_text_color = Some(INK);
        style.visuals.selection.bg_fill = Color32::from_rgb(211, 232, 201);
        style.visuals.selection.stroke = Stroke::new(1.0_f32, INK);
        style.spacing.item_spacing = Vec2::new(12.0, 12.0);
        style.spacing.button_padding = Vec2::new(14.0, 9.0);
        cc.egui_ctx.set_style(style);
        let ctx = cc.egui_ctx.clone();
        #[cfg(windows)]
        let window = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match cc.window_handle().map(|handle| handle.as_raw()) {
                Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
                _ => None,
            }
        };
        app.set_ui_waker(Box::new(move |show| {
            #[cfg(windows)]
            if let Some(window) = window {
                use windows_sys::Win32::UI::WindowsAndMessaging::*;
                // A hidden winit window does not receive paint events, so queued egui
                // visibility commands alone cannot restore it. Wake the actual HWND
                // asynchronously: the tray/IPC thread must never wait on the UI thread.
                unsafe {
                    let window = window as windows_sys::Win32::Foundation::HWND;
                    if show {
                        ShowWindowAsync(
                            window,
                            if IsIconic(window) != 0 {
                                SW_RESTORE
                            } else {
                                SW_SHOW
                            },
                        );
                    } else if IsWindowVisible(window) == 0 || IsIconic(window) != 0 {
                        // Closing needs a final UI pass too. Do not activate a hidden
                        // window or steal focus while flushing its Close command.
                        ShowWindowAsync(window, SW_SHOWNOACTIVATE);
                    }
                }
            }
            if show {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            ctx.request_repaint();
        }));
        let (sender, receiver) = mpsc::channel();
        let config = app.snapshot().config;
        let mut native = Self {
            #[cfg(feature = "screenshots")]
            frames: 0,
            app,
            instance,
            data,
            view: View::Overview,
            period: Period::Today,
            report: None,
            config,
            cause: 0,
            notice: None,
            last_refresh: Instant::now() - Duration::from_secs(10),
            fetching: false,
            generation: 0,
            sender,
            receiver,
        };
        if let Err(error) = native.instance.ready() {
            native.notice = Some(format!("Could not publish application readiness: {error}"));
        }
        if native.app.window_visible.load(Ordering::Acquire) {
            native.app.request_window();
        }
        native.refresh(&cc.egui_ctx);
        #[cfg(feature = "screenshots")]
        if std::env::var_os("UPTIME_SCREENSHOT_TO").is_some() {
            if let Ok((_, Ok(report))) = native.receiver.recv_timeout(Duration::from_secs(5)) {
                native.report = Some(report);
                native.fetching = false;
            }
            native.view = match std::env::var("UPTIME_SCREENSHOT_VIEW").as_deref() {
                Ok("history") => View::History,
                Ok("reports") => View::Reports,
                Ok("settings") => View::Settings,
                _ => View::Overview,
            };
        }
        native
    }
    fn refresh(&mut self, ctx: &egui::Context) {
        if self.fetching {
            return;
        }
        self.fetching = true;
        self.last_refresh = Instant::now();
        let app = self.app.clone();
        let sender = self.sender.clone();
        let generation = self.generation;
        let (start, end) = self.period.range();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let minimum = app.snapshot().config.minimum_outage_seconds;
            let result = app
                .store
                .lock()
                .expect("store")
                .report(start, end, minimum, 500)
                .map_err(|e| e.to_string());
            let _ = sender.send((generation, result));
            ctx.request_repaint();
        });
    }
    fn export(&mut self, format: &str) {
        let (start, end) = self.period.range();
        let minimum = self.app.snapshot().config.minimum_outage_seconds;
        let result = (|| -> Result<()> {
            let report = self
                .app
                .store
                .lock()
                .expect("store")
                .report(start, end, minimum, 0)?;
            let demo = self.app.snapshot().demo;
            let bytes = match format {
                "csv" => reports::csv_report(&report, demo).into_bytes(),
                "json" => {
                    let mut value = serde_json::to_value(&report)?;
                    value["demo"] = demo.into();
                    serde_json::to_vec_pretty(&value)?
                }
                _ => reports::text_report(&report, demo).into_bytes(),
            };
            if let Some(path) = rfd::FileDialog::new()
                .set_title("Save connection report")
                .add_filter("Connection report", &[format])
                .set_file_name(format!(
                    "uptime-{}.{}",
                    Local::now().format("%Y-%m-%d"),
                    format
                ))
                .save_file()
            {
                std::fs::write(&path, bytes)?;
                self.notice = Some(format!("Report saved to {}", path.display()));
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.notice = Some(format!("Could not save the report: {error}"));
        }
    }
    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .exact_width(180.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(INK).inner_margin(18))
            .show(ctx, |ui| {
                ui.add_space(16.0);
                ui.label(
                    RichText::new("uptime")
                        .color(Color32::WHITE)
                        .size(29.0)
                        .strong(),
                );
                ui.label(
                    RichText::new("M O N I T O R")
                        .size(10.0)
                        .color(Color32::from_rgb(194, 221, 188)),
                );
                ui.add_space(38.0);
                for (view, name) in [
                    (View::Overview, "Overview"),
                    (View::History, "Outage history"),
                    (View::Reports, "Reports"),
                    (View::Settings, "Settings"),
                ] {
                    let active = self.view == view;
                    let button = egui::Button::new(RichText::new(name).color(if active {
                        INK
                    } else {
                        Color32::from_rgb(193, 211, 196)
                    }))
                    .fill(if active {
                        Color32::from_rgb(211, 232, 201)
                    } else {
                        Color32::TRANSPARENT
                    })
                    .stroke(Stroke::NONE)
                    .min_size(Vec2::new(144.0, 40.0));
                    if ui.add(button).clicked() {
                        self.view = view;
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(14.0);
                    ui.label(
                        RichText::new(format!("Native desktop · v{}", updates::VERSION))
                            .size(10.0)
                            .color(Color32::from_rgb(169, 195, 174)),
                    );
                    ui.label(
                        RichText::new("Your history stays here.")
                            .size(11.0)
                            .color(Color32::from_rgb(193, 211, 196)),
                    );
                    ui.add_space(12.0);
                    if ui.button("Quit monitor").clicked() {
                        self.app.stop();
                    }
                    if self.app.tray_available.load(Ordering::Acquire)
                        && ui.button("Hide to tray").clicked()
                    {
                        self.app.window_visible.store(false, Ordering::Release);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    }
                });
            });
    }
    fn overview(&mut self, ui: &mut egui::Ui) {
        let status = self.app.snapshot();
        let state = status.latest.as_ref().map(|s| s.state);
        let (title, description) = if status.paused {
            (
                "Monitoring is paused.",
                "Paused time is excluded from downtime. Resume whenever you’re ready.",
            )
        } else {
            match state {
                Some(State::Online) => (
                    "You’re connected. We’re keeping watch.",
                    "Quietly checking your router and independent internet targets.",
                ),
                Some(State::Upstream) => (
                    "The internet isn’t responding.",
                    "Your router replied, but all public targets failed. The issue may be upstream.",
                ),
                Some(State::Local) => (
                    "Your local connection needs a look.",
                    "Neither your router nor the internet targets replied. Check your Wi-Fi, cables, and router.",
                ),
                Some(State::Undetermined) => (
                    "The internet isn’t responding.",
                    "Public targets failed; router reachability is unknown.",
                ),
                _ => (
                    "Getting to know your connection…",
                    "The first connection checks are on their way. Unobserved time is excluded.",
                ),
            }
        };
        egui::Frame::new()
            .fill(if state.is_some_and(State::is_down) {
                Color32::from_rgb(251, 237, 224)
            } else {
                Color32::from_rgb(235, 244, 227)
            })
            .inner_margin(20)
            .corner_radius(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(title).size(18.0).strong());
                muted(ui, description);
            });
        ui.add_space(16.0);
        let Some(report) = self.report.as_ref() else {
            ui.spinner();
            muted(ui, "Loading your local history…");
            return;
        };
        let stats = [
            (
                "Connection uptime",
                report
                    .uptime_percent
                    .map(|v| format!("{v:.2}%"))
                    .unwrap_or("—".into()),
                "Of monitored time".to_string(),
            ),
            (
                "Outages",
                report.outage_count.to_string(),
                format!("Lasting {}s or longer", report.minimum_outage_seconds),
            ),
            (
                "Total downtime",
                duration(report.downtime_seconds),
                format!("Longest: {}", duration(report.longest_outage_seconds)),
            ),
            (
                "Average latency",
                report
                    .average_latency_ms
                    .map(|v| format!("{v:.0} ms"))
                    .unwrap_or("—".into()),
                "Fastest public target per check".into(),
            ),
        ];
        ui.columns(4, |columns| {
            for (column, (label, value, detail)) in columns.iter_mut().zip(stats) {
                card().show(column, |ui| {
                    ui.set_min_width(ui.available_width());
                    muted(ui, label);
                    ui.label(RichText::new(value).size(29.0).strong());
                    muted(ui, detail);
                });
            }
        });
        ui.add_space(16.0);
        card().show(ui, |ui| {
            ui.heading("Connection timeline");
            muted(ui, "Only observed time is counted. Gaps stay unmonitored.");
            ui.add_space(10.0);
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 4, LINE);
            let width = report.end - report.start;
            for online in [true, false] {
                for span in &report.spans {
                    if (span.state == "online" || span.state == "unknown") != online {
                        continue;
                    }
                    let x = rect.left()
                        + ((span.started_at - report.start) / width) as f32 * rect.width();
                    let w = ((span.ended_at - span.started_at) / width) as f32 * rect.width();
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(x, rect.top()),
                            Vec2::new(
                                w.max(if online { 0.5 } else { 2.0 }).min(rect.right() - x),
                                rect.height(),
                            ),
                        ),
                        0,
                        state_color(&span.state),
                    );
                }
            }
            if let Some(point) = response.hover_pos() {
                let t = report.start + ((point.x - rect.left()) / rect.width()) as f64 * width;
                let state = report
                    .spans
                    .iter()
                    .find(|s| s.started_at <= t && s.ended_at >= t)
                    .map(|s| s.state.as_str())
                    .unwrap_or("unmonitored");
                response.on_hover_text(format!("{} · {state}", timestamp(t)));
            }
            ui.horizontal(|ui| {
                muted(ui, timestamp(report.start));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    muted(ui, timestamp(report.end));
                });
            });
            ui.horizontal_wrapped(|ui| {
                for (label, color) in [
                    ("Online", GREEN),
                    ("Internet issue", ORANGE),
                    ("Local network", RED),
                    ("Unmonitored", MUTED),
                ] {
                    ui.horizontal(|ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::splat(6.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 3.0, color);
                        ui.label(RichText::new(label).size(11.0).color(color));
                    });
                }
            });
            muted(
                ui,
                format!(
                    "{} monitored · {} unmonitored",
                    duration(report.monitored_seconds),
                    duration(report.unmonitored_seconds)
                ),
            );
        });
        ui.add_space(16.0);
        ui.columns(2, |columns| {
            card().show(&mut columns[0], |ui| {
                ui.heading("Where’s the problem?");
                muted(ui, "Your router and the internet are checked separately.");
                ui.add_space(16.0);
                if let Some(sample) = status.latest.as_ref() {
                    egui::Grid::new("network")
                        .spacing([24.0, 14.0])
                        .show(ui, |ui| {
                            ui.strong("Your router");
                            ui.monospace(
                                sample.router.address.as_deref().unwrap_or("Not detected"),
                            );
                            ui.end_row();
                            ui.label("Router response");
                            ui.label(match sample.router.ok {
                                Some(true) => {
                                    format!("{} ms", sample.router.latency_ms.unwrap_or(0.0))
                                }
                                Some(false) => "No reply".into(),
                                None => "Unknown".into(),
                            });
                            ui.end_row();
                            ui.strong("The internet");
                            ui.label(format!(
                                "{} of {} targets replying",
                                sample.targets.iter().filter(|t| t.ok == Some(true)).count(),
                                sample.targets.len()
                            ));
                            ui.end_row();
                        });
                    for target in &sample.targets {
                        muted(
                            ui,
                            format!(
                                "{} · {}",
                                target.name.as_deref().unwrap_or("Target"),
                                target
                                    .latency_ms
                                    .map(|v| format!("{v:.0} ms"))
                                    .unwrap_or_else(|| target
                                        .error
                                        .clone()
                                        .unwrap_or("No reply".into()))
                            ),
                        );
                    }
                } else {
                    muted(
                        ui,
                        if status.paused {
                            "Monitoring paused"
                        } else {
                            "Waiting for the first check"
                        },
                    );
                }
            });
            card().show(&mut columns[1], |ui| {
                ui.heading("Latency at a glance");
                muted(ui, "Fastest public target per check, in milliseconds.");
                let (rect, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 150.0),
                    egui::Sense::hover(),
                );
                let painter = ui.painter();
                let maximum = report
                    .latency
                    .iter()
                    .filter_map(|p| p.latency_ms)
                    .fold(40.0, f64::max)
                    * 1.2;
                for i in 0..4 {
                    let y = rect.top() + i as f32 * rect.height() / 3.0;
                    painter.line_segment(
                        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                        Stroke::new(1.0_f32, LINE),
                    );
                }
                let mut previous = None;
                for point in &report.latency {
                    if let Some(value) = point.latency_ms {
                        let position = egui::pos2(
                            rect.left()
                                + ((point.checked_at - report.start) / (report.end - report.start))
                                    as f32
                                    * rect.width(),
                            rect.bottom() - (value / maximum) as f32 * rect.height(),
                        );
                        if let Some((bucket, last)) = previous
                            && point.bucket == bucket + 1
                        {
                            painter.line_segment([last, position], Stroke::new(2.0_f32, GREEN));
                        }
                        painter.circle_filled(position, 1.6, GREEN);
                        previous = Some((point.bucket, position));
                    } else {
                        previous = None;
                    }
                }
                muted(ui, format!("{} checks in this period", report.check_count));
            });
        });
        ui.add_space(16.0);
        card().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Recent outages");
                if ui.small_button("View history").clicked() {
                    self.view = View::History;
                }
            });
            outage_table(ui, report, 0, 4);
        });
        muted(
            ui,
            "Close this window to keep recording in the tray. Quit monitor to stop.",
        );
    }
    fn history(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Show");
            egui::ComboBox::from_id_salt("cause")
                .selected_text(
                    [
                        "All causes",
                        "Internet issues",
                        "Local network",
                        "Other / mixed",
                    ][self.cause],
                )
                .show_ui(ui, |ui| {
                    for (i, label) in [
                        "All causes",
                        "Internet issues",
                        "Local network",
                        "Other / mixed",
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui.selectable_value(&mut self.cause, i, *label);
                    }
                });
        });
        if let Some(report) = &self.report {
            card().show(ui, |ui| {
                ui.heading(format!(
                    "{} outages · {} of downtime",
                    report.outage_count,
                    duration(report.downtime_seconds)
                ));
                outage_table(ui, report, self.cause, 500);
                if report.outages_truncated {
                    muted(
                        ui,
                        "Showing the latest 500 events. Export a report for the complete list.",
                    );
                }
            });
        }
        muted(
            ui,
            "Interrupted means observation stopped before recovery was confirmed. Router evidence suggests a location; it does not prove ISP responsibility.",
        );
    }
    fn reports(&mut self, ui: &mut egui::Ui) {
        if let Some(report) = &self.report {
            card().show(ui, |ui| {
                muted(ui, "INTERNET UPTIME MONITOR · CONNECTION REPORT");
                ui.add_space(14.0);
                ui.label(
                    RichText::new(format!(
                        "{} outages. {} of downtime.",
                        report.outage_count,
                        duration(report.downtime_seconds)
                    ))
                    .size(27.0)
                    .strong(),
                );
                muted(
                    ui,
                    format!("{} — {}", timestamp(report.start), timestamp(report.end)),
                );
                ui.add_space(14.0);
                egui::Grid::new("report")
                    .spacing([50.0, 14.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("Monitored time", duration(report.monitored_seconds)),
                            ("Unmonitored time", duration(report.unmonitored_seconds)),
                            (
                                "Connection availability",
                                report
                                    .uptime_percent
                                    .map(|v| format!("{v:.3}%"))
                                    .unwrap_or("No monitored time".into()),
                            ),
                            (
                                "Longest interruption",
                                duration(report.longest_outage_seconds),
                            ),
                            (
                                "Minimum outage length",
                                format!("{}s", report.minimum_outage_seconds),
                            ),
                        ] {
                            muted(ui, label);
                            ui.strong(value);
                            ui.end_row();
                        }
                    });
            });
            ui.add_space(16.0);
        }
        ui.horizontal(|ui| {
            if ui.button("Save readable report…").clicked() {
                self.export("txt");
            }
            if ui.button("Export CSV…").clicked() {
                self.export("csv");
            }
            if ui.button("Export JSON…").clicked() {
                self.export("json");
            }
        });
        ui.add_space(12.0);
        muted(
            ui,
            "Exports use UTC timestamps and include monitoring coverage. ICMP filtering can look like an outage; DNS, HTTPS, and bandwidth are not tested.",
        );
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        let demo = self.app.snapshot().demo;
        ui.columns(2,|columns| {
        {
        let ui=&mut columns[0];
        card().show(ui,|ui| {ui.heading("Monitoring preferences");ui.add_space(10.0);ui.add_enabled_ui(!demo,|ui| {
            egui::Grid::new("preferences").spacing([30.0,14.0]).show(ui,|ui| {
                ui.label("Check interval");ui.add(egui::DragValue::new(&mut self.config.interval_seconds).range(2..=300).suffix(" seconds"));ui.end_row();
                ui.label("Response timeout");ui.add(egui::DragValue::new(&mut self.config.timeout_ms).range(200..=5000).suffix(" ms"));ui.end_row();
                ui.label("Minimum outage length");ui.add(egui::DragValue::new(&mut self.config.minimum_outage_seconds).range(0..=300).suffix(" seconds"));ui.end_row();
                ui.label("Router IPv4 address");ui.add(egui::TextEdit::singleline(&mut self.config.router).hint_text("Auto-detect").desired_width(180.0));ui.end_row();
            });ui.add_space(8.0);muted(ui,"Short interruptions affect availability even when excluded from outage counts.");
            ui.checkbox(&mut self.config.check_updates_automatically,"Check for updates on startup");muted(ui,"Also checks once a day. Downloads and installation are always your choice.");
            ui.add_space(12.0);ui.strong("Independent internet targets");let mut remove=None;
            for (i,target) in self.config.targets.iter_mut().enumerate() {ui.horizontal(|ui| {ui.add(egui::TextEdit::singleline(&mut target.name).desired_width(130.0));ui.add(egui::TextEdit::singleline(&mut target.address).desired_width(150.0));if ui.small_button("Remove").clicked() {remove=Some(i);}});}
            if let Some(i)=remove && self.config.targets.len()>2 {self.config.targets.remove(i);}
            if self.config.targets.len()<5 && ui.small_button("Add target").clicked() {self.config.targets.push(crate::model::Target {name:"New target".into(),address:String::new()});}
            ui.add_space(10.0);ui.horizontal(|ui| {if ui.button("Save preferences").clicked() {match self.app.configure(self.config.clone()) {Ok(())=>{self.notice=Some("Preferences saved. The next check will use them.".into());self.last_refresh=Instant::now()-Duration::from_secs(10);},Err(error)=>self.notice=Some(error.to_string())}}
            if ui.button("Revert").clicked() {self.config=self.app.snapshot().config;}});
        });});
        }
        let ui=&mut columns[1];
        self.updates(ui);
        ui.add_space(16.0);
        card().show(ui,|ui| {ui.heading("On this computer");muted(ui,"History folder"); ui.add(egui::Label::new(self.data.display().to_string()).truncate()).on_hover_text(self.data.display().to_string());muted(ui,"Raw checks are retained for 30 days. Outages and connection intervals are retained indefinitely.");
            #[cfg(windows)] {let mut enabled=platform::startup_enabled();if ui.checkbox(&mut enabled,"Start quietly with Windows").changed() && let Err(error)=platform::set_startup(enabled,&self.data) {self.notice=Some(error.to_string());}}
        });
        });
    }
    fn updates(&mut self, ui: &mut egui::Ui) {
        let update = self.app.updates.snapshot();
        let busy = matches!(
            update.phase.as_str(),
            "checking" | "downloading" | "restarting"
        );
        card().show(ui, |ui| {
            ui.heading("Application updates");
            muted(
                ui,
                format!(
                    "Installed version {} · native desktop",
                    update.current_version
                ),
            );
            ui.add_space(10.0);
            let message = match update.phase.as_str() {
                "checking" => "Checking GitHub. Monitoring continues in the background.".into(),
                "available" => format!(
                    "Version {} is available.",
                    update.latest_version.as_deref().unwrap_or("latest")
                ),
                "up_to_date" => "You’re up to date.".into(),
                "downloading" => "Downloading and verifying the new executable…".into(),
                "restarting" => "Update verified. Restarting the application…".into(),
                "error" => update
                    .error
                    .clone()
                    .unwrap_or("The update check failed. Try again later.".into()),
                _ => "Check GitHub for the latest stable release.".into(),
            };
            ui.label(message);
            if update.phase == "downloading" {
                ui.add(
                    egui::ProgressBar::new(if update.total_bytes > 0 {
                        update.downloaded_bytes as f32 / update.total_bytes as f32
                    } else {
                        0.0
                    })
                    .show_percentage(),
                );
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Check for updates"))
                    .clicked()
                    && let Err(error) = self.app.updates.check()
                {
                    self.notice = Some(error.to_string());
                }
                if update.phase == "available"
                    && update.install_supported
                    && !self.app.snapshot().demo
                    && ui.button("Download and install").clicked()
                    && let Err(error) = self.app.updates.install(&self.app)
                {
                    self.notice = Some(error.to_string());
                }
                if let Some(url) = &update.release_url {
                    ui.hyperlink_to("Release notes", url);
                }
            });
            if let Some(checked) = update.checked_at {
                muted(ui, format!("Last checked: {}", timestamp(checked)));
            }
            muted(
                ui,
                "Downloads are verified with SHA-256. Your history and preferences are preserved.",
            );
        });
    }
}
fn outage_table(ui: &mut egui::Ui, report: &Report, cause: usize, limit: usize) {
    let events: Vec<_> = report
        .outages
        .iter()
        .filter(|e| match cause {
            1 => e.cause == "upstream",
            2 => e.cause == "local",
            3 => e.cause != "upstream" && e.cause != "local",
            _ => true,
        })
        .take(limit)
        .collect();
    if events.is_empty() {
        ui.add_space(20.0);
        ui.strong("No outages on the record.");
        muted(ui, "Interruptions will appear here when they are observed.");
        ui.add_space(14.0);
        return;
    }
    ui.add_space(12.0);
    egui::Grid::new(ui.id().with("outages"))
        .striped(true)
        .spacing([18.0, 12.0])
        .show(ui, |ui| {
            for name in [
                "Started",
                "Duration",
                "Likely location",
                "Router evidence",
                "Status",
            ] {
                muted(ui, name);
            }
            ui.end_row();
            for event in events {
                ui.label(timestamp(event.started_at));
                ui.strong(duration(event.duration_seconds));
                ui.colored_label(
                    state_color(&event.cause),
                    match event.cause.as_str() {
                        "upstream" => "Internet issue",
                        "local" => "Local network",
                        "mixed" => "Mixed evidence",
                        _ => "Undetermined",
                    },
                );
                ui.label(match event.cause.as_str() {
                    "upstream" => "Responded",
                    "local" => "No reply",
                    "mixed" => "Changed",
                    _ => "Unknown",
                });
                ui.label(match event.status.as_str() {
                    "recovered" => "Recovered",
                    "ongoing" => "Ongoing",
                    _ => "Observation ended",
                });
                ui.end_row();
            }
        });
}
impl eframe::App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.app.window_visible.load(Ordering::Acquire) {
            // eframe shows the first rendered frame even with a hidden viewport.
            // Keep the framework state aligned with background startup as well.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        #[cfg(feature = "screenshots")]
        if let Some(path) = std::env::var_os("UPTIME_SCREENSHOT_TO") {
            let captured = ctx.input(|input| {
                input.events.iter().find_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(pixels) = captured {
                let rgba: Vec<u8> = pixels.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Err(error) = image::save_buffer(
                    PathBuf::from(path),
                    &rgba,
                    pixels.size[0] as u32,
                    pixels.size[1] as u32,
                    image::ColorType::Rgba8,
                ) {
                    self.notice = Some(error.to_string());
                } else {
                    self.app.stop();
                }
            }
            self.frames += 1;
            if self.frames == 3 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            }
            ctx.request_repaint();
        }
        if self.app.is_stopping() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.app.tray_available.load(Ordering::Acquire) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                self.app.window_visible.store(false, Ordering::Release);
            } else {
                self.app.stop();
            }
        }
        while let Ok((generation, result)) = self.receiver.try_recv() {
            self.fetching = false;
            if generation == self.generation {
                match result {
                    Ok(report) => self.report = Some(report),
                    Err(error) => self.notice = Some(format!("Could not load history: {error}")),
                }
            }
        }
        if self.last_refresh.elapsed() > Duration::from_secs(2) {
            self.refresh(ctx);
        }
        if let Some(message) = self.app.ui_notice.lock().expect("notice").take() {
            self.notice = Some(message);
        }
        self.sidebar(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(247, 248, 245))
                    .inner_margin(28),
            )
            .show(ctx, |ui| {
                let before = self.period;
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(match self.view {
                            View::Overview => "Your internet, on record.",
                            View::History => "Every disconnect, documented.",
                            View::Reports => "A clear story. Ready to share.",
                            View::Settings => "Make yourself comfortable.",
                        })
                        .size(27.0)
                        .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("period")
                            .selected_text(self.period.label())
                            .show_ui(ui, |ui| {
                                for period in [Period::Today, Period::Week, Period::Month] {
                                    ui.selectable_value(&mut self.period, period, period.label());
                                }
                            });
                    });
                });
                if before != self.period {
                    self.generation += 1;
                    self.last_refresh = Instant::now() - Duration::from_secs(10);
                }
                let status = self.app.snapshot();
                ui.horizontal(|ui| {
                    muted(
                        ui,
                        if status.paused {
                            "Monitoring paused"
                        } else {
                            "Quiet monitoring · no session limit"
                        },
                    );
                    if !status.demo
                        && ui
                            .small_button(if status.paused { "Resume" } else { "Pause" })
                            .clicked()
                        && let Err(error) =
                            self.app
                                .control(if status.paused { "resume" } else { "pause" })
                    {
                        self.notice = Some(error.to_string());
                    }
                    if status.demo {
                        ui.colored_label(ORANGE, "DEMO DATA — not real measurements");
                    }
                });
                ui.add_space(16.0);
                if let Some(message) = self.notice.clone() {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(245, 239, 218))
                        .inner_margin(12)
                        .corner_radius(5)
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(message);
                                if ui.small_button("Dismiss").clicked() {
                                    self.notice = None;
                                }
                            });
                        });
                    ui.add_space(12.0);
                }
                if let Some(error) = status.error {
                    ui.colored_label(RED, error);
                }
                if self.app.updates.snapshot().phase == "available" && self.view != View::Settings {
                    ui.horizontal(|ui| {
                        ui.label("A new version is available.");
                        if ui.small_button("View update").clicked() {
                            self.view = View::Settings;
                        }
                    });
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        match self.view {
                            View::Overview => self.overview(ui),
                            View::History => self.history(ui),
                            View::Reports => self.reports(ui),
                            View::Settings => self.settings(ui),
                        }
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(750));
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.app.stop();
        self.app.clear_ui_waker();
    }
}
pub fn run(app: Arc<App>, instance: Arc<Instance>, data: PathBuf, hidden: bool) -> Result<()> {
    let hidden = hidden && !app.window_visible.load(Ordering::Acquire);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1160.0, 890.0])
            .with_min_inner_size([850.0, 620.0])
            .with_visible(!hidden)
            .with_icon(icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Internet Uptime Monitor",
        options,
        Box::new(move |cc| Ok(Box::new(NativeApp::new(cc, app, instance, data)))),
    )
    .map_err(|e| anyhow::anyhow!("Could not create the native window: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_report_periods_are_valid() {
        for period in [Period::Today, Period::Week, Period::Month] {
            let (start, end) = period.range();
            assert!(end > start && end - start <= 31.0 * 86400.0);
        }
    }
    #[test]
    fn durations_are_human_readable() {
        assert_eq!(duration(480.0), "8m 0s");
        assert_eq!(duration(5.0), "5s");
    }
}
