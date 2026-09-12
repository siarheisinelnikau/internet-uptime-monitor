//! CPU rendering with a GDI bitmap presentation on Windows. No graphics context.
use crate::{instance::Instance, monitor::App, native::NativeApp};
use anyhow::{Context, Result};
use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, OwnedDisplayHandle},
    platform::windows::WindowAttributesExtWindows,
    window::{Icon, Window, WindowId},
};

#[path = "render_diagnostic.rs"]
mod diagnostic;

const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

struct Repaint {
    when: Instant,
    pass: u64,
}

struct SoftwareWindow {
    native: NativeApp,
    surface: softbuffer::Surface<OwnedDisplayHandle, Arc<Window>>,
    _context: softbuffer::Context<OwnedDisplayHandle>,
    window: Arc<Window>,
    input: egui_winit::State,
    info: egui::ViewportInfo,
    renderer: EguiSoftwareRender,
    pending_textures: egui::TexturesDelta,
    has_frame: bool,
}

struct SoftwareApp {
    monitor: Arc<App>,
    instance: Arc<Instance>,
    data: PathBuf,
    context: egui::Context,
    window: Option<SoftwareWindow>,
    next_frame: Option<Instant>,
    last_frame: Option<Instant>,
    error: Option<anyhow::Error>,
    trace: Option<FrameTrace>,
    diagnostic: Option<diagnostic::Diagnostic>,
    counts: [u64; 3],
}

impl SoftwareApp {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let icon = crate::native::icon();
        let icon = Icon::from_rgba(icon.rgba, icon.width, icon.height)?;
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("Internet Uptime Monitor")
                    .with_inner_size(LogicalSize::new(1160.0, 890.0))
                    .with_min_inner_size(LogicalSize::new(850.0, 620.0))
                    .with_visible(self.monitor.window_visible.load(Ordering::Acquire))
                    .with_window_icon(Some(icon.clone()))
                    .with_taskbar_icon(Some(icon)),
            )?,
        );
        let context = softbuffer::Context::new(event_loop.owned_display_handle())
            .map_err(|e| anyhow::anyhow!("Could not create the GDI drawing context: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| anyhow::anyhow!("Could not create the GDI bitmap surface: {e}"))?;
        let input = egui_winit::State::new(
            self.context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let native = NativeApp::new(
            &self.context,
            self.monitor.clone(),
            self.instance.clone(),
            self.data.clone(),
        );
        self.window = Some(SoftwareWindow {
            native,
            surface,
            _context: context,
            window,
            input,
            info: Default::default(),
            renderer: EguiSoftwareRender::new(ColorFieldOrder::Bgra),
            pending_textures: Default::default(),
            has_frame: false,
        });
        self.request_frame(Instant::now());
        if !self.monitor.window_visible.load(Ordering::Acquire) {
            self.monitor.acknowledge_update_startup();
        }
        Ok(())
    }

    fn request_frame(&mut self, when: Instant) {
        let earliest = self
            .last_frame
            .map(|last| last + FRAME_INTERVAL)
            .unwrap_or(when);
        let when = when.max(earliest);
        self.next_frame = Some(self.next_frame.map_or(when, |pending| pending.min(when)));
    }

    fn draw(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let started = Instant::now();
        self.last_frame = Some(started);
        self.next_frame = None;
        let Some(state) = self.window.as_mut() else {
            return Ok(());
        };
        if !self.monitor.window_visible.load(Ordering::Acquire) {
            state.window.set_visible(false);
            return Ok(());
        }
        // A requested show is handled independently of layout while hidden.
        if self.monitor.take_show_request() || !state.window.is_visible().unwrap_or(true) {
            state.window.set_visible(true);
            state.window.set_minimized(false);
        }
        if state.window.is_minimized() == Some(true) {
            return Ok(());
        }
        egui_winit::update_viewport_info(&mut state.info, &self.context, &state.window, false);
        let mut input = state.input.take_egui_input(&state.window);
        input
            .viewports
            .insert(egui::ViewportId::ROOT, state.info.clone());
        state.info.events.clear();
        self.counts[0] += 1;
        let mut output = self.context.run(input, |ctx| state.native.draw(ctx));
        // Keep font/image uploads from hidden frames until the next presentation.
        state.pending_textures.append(output.textures_delta);
        state
            .input
            .handle_platform_output(&state.window, output.platform_output);
        let mut close = false;
        let mut commands = Vec::new();
        let mut screenshots = Vec::new();
        if let Some(viewport) = output.viewport_output.get_mut(&egui::ViewportId::ROOT) {
            if viewport.repaint_delay != Duration::MAX {
                self.next_frame = started.checked_add(viewport.repaint_delay.max(FRAME_INTERVAL));
            }
            for command in viewport.commands.drain(..) {
                match command {
                    egui::ViewportCommand::Close => close = true,
                    egui::ViewportCommand::CancelClose => close = false,
                    egui::ViewportCommand::Screenshot(data) => screenshots.push(data),
                    command => commands.push(command),
                }
            }
        }
        let mut actions = Vec::new();
        egui_winit::process_viewport_commands(
            &self.context,
            &mut state.info,
            commands,
            &state.window,
            &mut actions,
        );
        if close || self.monitor.is_stopping() {
            self.monitor.stop();
            event_loop.exit();
            return Ok(());
        }
        let visible =
            state.window.is_visible().unwrap_or(true) && state.window.is_minimized() != Some(true);
        let size = state.window.inner_size();
        if !visible || size.width == 0 || size.height == 0 {
            // Wakeups still process show/quit commands while hidden, without rasterizing.
            self.next_frame = None;
            return Ok(());
        }
        state
            .surface
            .resize(
                NonZeroU32::new(size.width).context("Missing bitmap width")?,
                NonZeroU32::new(size.height).context("Missing bitmap height")?,
            )
            .map_err(|e| anyhow::anyhow!("Could not resize the GDI bitmap: {e}"))?;
        let primitives = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        let mut buffer = state
            .surface
            .buffer_mut()
            .map_err(|e| anyhow::anyhow!("Could not access the GDI bitmap: {e}"))?;
        buffer.fill(0);
        self.counts[1] += 1;
        state.renderer.render(
            &mut BufferMutRef::new(
                bytemuck::cast_slice_mut(&mut buffer),
                size.width as usize,
                size.height as usize,
            ),
            &primitives,
            &state.pending_textures,
            output.pixels_per_point,
        );
        state.pending_textures = Default::default();
        if !screenshots.is_empty() {
            let pixels = buffer
                .iter()
                .map(|pixel| {
                    egui::Color32::from_rgb((pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8)
                })
                .collect();
            let image = Arc::new(egui::ColorImage::new(
                [size.width as usize, size.height as usize],
                pixels,
            ));
            for data in screenshots {
                state
                    .input
                    .egui_input_mut()
                    .events
                    .push(egui::Event::Screenshot {
                        viewport_id: egui::ViewportId::ROOT,
                        user_data: data,
                        image: image.clone(),
                    });
            }
            self.next_frame = Some(started + FRAME_INTERVAL);
        }
        buffer
            .present()
            .map_err(|e| anyhow::anyhow!("Could not display the GDI bitmap: {e}"))?;
        self.counts[2] += 1;
        state.has_frame = true;
        self.monitor.acknowledge_update_startup();
        if let Some(trace) = &mut self.trace {
            trace.record(started.elapsed(), &self.context);
        }
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }
}

impl ApplicationHandler<Repaint> for SoftwareApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.initialize(event_loop)
        {
            self.fail(event_loop, error);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(state) = self.window.as_mut() else {
            return;
        };
        if state.window.id() != id {
            return;
        }
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
        ) {
            state.has_frame = false;
        }
        if matches!(event, WindowEvent::RedrawRequested) && state.has_frame {
            if !state.window.is_visible().unwrap_or(true)
                || state.window.is_minimized() == Some(true)
            {
                return;
            }
            // An OS expose/paint event only needs the retained bitmap. Rebuilding
            // egui here would turn unrelated Windows paint traffic into UI frames.
            let result = state
                .surface
                .buffer_mut()
                .and_then(|buffer| buffer.present());
            if let Err(error) = result {
                self.fail(
                    event_loop,
                    anyhow::anyhow!("Could not restore the GDI bitmap: {error}"),
                );
            }
            self.counts[2] += 1;
            return;
        }
        if matches!(event, WindowEvent::CloseRequested) {
            state.info.events.push(egui::ViewportEvent::Close);
        }
        let response = state.input.on_window_event(&state.window, &event);
        if response.repaint
            || matches!(
                event,
                WindowEvent::RedrawRequested | WindowEvent::CloseRequested
            )
        {
            self.request_frame(Instant::now());
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, repaint: Repaint) {
        let pass = self.context.cumulative_pass_nr();
        if pass == repaint.pass || pass == repaint.pass + 1 {
            self.request_frame(repaint.when);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(diagnostic) = &mut self.diagnostic {
            match diagnostic.poll(self.counts) {
                diagnostic::Action::Hide => {
                    if let Some(state) = &self.window {
                        state.window.set_visible(false);
                    }
                    self.monitor.window_visible.store(false, Ordering::Release);
                    self.next_frame = None;
                }
                diagnostic::Action::Show => {
                    self.monitor.window_visible.store(true, Ordering::Release);
                    if let Some(state) = &mut self.window {
                        state.window.set_visible(true);
                        state.window.set_minimized(false);
                        state.has_frame = false;
                    }
                    self.request_frame(Instant::now());
                }
                diagnostic::Action::Exit => {
                    event_loop.exit();
                    return;
                }
                diagnostic::Action::None => {}
            }
        }
        if self.monitor.is_stopping() {
            event_loop.exit();
            return;
        }
        if self.next_frame.is_some_and(|when| when <= Instant::now())
            && let Err(error) = self.draw(event_loop)
        {
            self.fail(event_loop, error);
        }
        let deadline = match (
            self.next_frame,
            self.diagnostic
                .as_ref()
                .map(|diagnostic| diagnostic.deadline()),
        ) {
            (Some(frame), Some(diagnostic)) => Some(frame.min(diagnostic)),
            (frame, diagnostic) => frame.or(diagnostic),
        };
        event_loop.set_control_flow(deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.window {
            state.native.shutdown();
        }
    }
}

/// Opt-in local timings include UI layout, CPU rasterization and GDI presentation.
struct FrameTrace {
    file: File,
    started: Instant,
    frames_ms: Vec<f64>,
}

impl FrameTrace {
    fn new() -> Option<Self> {
        let path = std::env::var_os("UPTIME_RENDER_TRACE")?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        let _ = writeln!(
            file,
            "{}",
            serde_json::json!({"renderer": "CPU / Windows GDI"})
        );
        Some(Self {
            file,
            started: Instant::now(),
            frames_ms: Vec::new(),
        })
    }
    fn record(&mut self, elapsed: Duration, context: &egui::Context) {
        self.frames_ms.push(elapsed.as_secs_f64() * 1000.0);
        if self.started.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.frames_ms.sort_unstable_by(f64::total_cmp);
        let count = self.frames_ms.len();
        let _ = writeln!(
            self.file,
            "{}",
            serde_json::json!({
                "seconds": self.started.elapsed().as_secs_f64(), "frames": count,
                "frame_median_ms": self.frames_ms[count / 2],
                "frame_p95_ms": self.frames_ms[(count - 1) * 95 / 100],
                "frame_max_ms": self.frames_ms[count - 1],
            "repaint_causes": context.repaint_causes().iter().map(ToString::to_string).collect::<Vec<_>>(),
            })
        );
        self.started = Instant::now();
        self.frames_ms.clear();
    }
}

pub fn run(monitor: Arc<App>, instance: Arc<Instance>, data: PathBuf, _hidden: bool) -> Result<()> {
    let event_loop = EventLoop::<Repaint>::with_user_event().build()?;
    let context = egui::Context::default();
    let proxy = event_loop.create_proxy();
    context.set_request_repaint_callback(move |info| {
        if let Some(when) = Instant::now().checked_add(info.delay) {
            let _ = proxy.send_event(Repaint {
                when,
                pass: info.current_cumulative_pass_nr,
            });
        }
    });
    let mut app = SoftwareApp {
        monitor,
        instance,
        data,
        context,
        window: None,
        next_frame: None,
        last_frame: None,
        error: None,
        trace: FrameTrace::new(),
        diagnostic: diagnostic::Diagnostic::from_env("UPTIME_RENDER_DIAGNOSTIC"),
        counts: [0; 3],
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
