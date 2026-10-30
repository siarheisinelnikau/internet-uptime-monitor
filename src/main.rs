#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
use anyhow::{Context, Result, bail};
use internet_uptime_monitor::{
    instance::{self, Instance},
    model::{Config, State},
    monitor::{App, seed_demo},
    native, platform, probes,
    storage::Store,
    tray, updates,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const HELP: &str = "Native desktop internet outage monitoring. No browser or local port is needed.

Options:
  --background       Start quietly in the Windows tray
  --portable         Keep history beside the executable in a data folder
  --data-dir PATH    Choose a history directory
  --demo             Explore labeled sample data without probing
  --once             Print one connection check as JSON
  --router IPV4      Override automatic gateway discovery
  --stop             Stop the instance using this history directory
  --status           Print native instance information as JSON
  --start-paused     Start with monitoring paused
  --no-tray          Close the app when its window closes
  --headless         Run monitoring without a window or tray
  --enable-startup   Start quietly when you sign in to Windows
  --disable-startup  Disable Windows sign-in startup
  --help             Show help
  --version          Show the installed version

Close the window to keep monitoring in the tray. Click the tray icon or
double-click the EXE to open it again. Use Quit monitor to stop recording.";

struct Options {
    data: PathBuf,
    background: bool,
    no_tray: bool,
    headless: bool,
    once: bool,
    demo: bool,
    stop: bool,
    status: bool,
    start_paused: bool,
    startup: Option<bool>,
    router: Option<String>,
    help: bool,
    version: bool,
    legacy_port: u16,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            data: platform::data_dir(),
            background: false,
            no_tray: false,
            headless: false,
            once: false,
            demo: false,
            stop: false,
            status: false,
            start_paused: false,
            startup: None,
            router: None,
            help: false,
            version: false,
            legacy_port: 8765,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--background" | "--no-browser" => options.background = true,
                "--no-tray" => options.no_tray = true,
                "--headless" => {
                    options.headless = true;
                    options.no_tray = true;
                }
                "--once" => options.once = true,
                "--demo" => options.demo = true,
                "--stop" => options.stop = true,
                "--status" => options.status = true,
                "--start-paused" => options.start_paused = true,
                "--enable-startup" => options.startup = Some(true),
                "--disable-startup" => options.startup = Some(false),
                "--router" => {
                    options.router = Some(args.next().context("--router needs an IPv4 address.")?)
                }
                "--data-dir" => {
                    options.data = PathBuf::from(args.next().context("--data-dir needs a path.")?)
                }
                "--portable" => {
                    options.data = std::env::current_exe()?
                        .parent()
                        .context("Missing application folder.")?
                        .join("data")
                }
                // Accepted for existing startup shortcuts and the 0.1 update helper only.
                "--port" => {
                    options.legacy_port =
                        args.next().context("Missing legacy port value.")?.parse()?
                }
                "-h" | "--help" => options.help = true,
                "-V" | "--version" => options.version = true,
                _ => bail!("Unknown option: {arg}. Use --help for options."),
            }
        }
        if options.data.is_relative() {
            options.data = std::env::current_dir()?.join(options.data);
        }
        Ok(options)
    }
}

fn run() -> Result<i32> {
    let mut internal = std::env::args_os().skip(1);
    if internal.next().as_deref() == Some(std::ffi::OsStr::new("--apply-update")) {
        updates::apply_update(&PathBuf::from(
            internal.next().context("Missing update plan.")?,
        ))?;
        return Ok(0);
    }
    let options = Options::parse()?;
    if options.help {
        println!("Internet Uptime Monitor {}\n{HELP}", updates::VERSION);
        return Ok(0);
    }
    if options.version {
        println!("Internet Uptime Monitor {}", updates::VERSION);
        return Ok(0);
    }
    if let Some(enable) = options.startup {
        platform::set_startup(enable, &options.data)?;
        println!(
            "Automatic Windows startup {}.",
            if enable { "enabled" } else { "disabled" }
        );
        return Ok(0);
    }
    if options.status {
        println!(
            "{}",
            serde_json::to_string_pretty(&instance::read(&options.data, options.demo))?
        );
        return Ok(0);
    }
    if options.stop {
        if !instance::send(&options.data, options.demo, "stop")?
            && instance::is_locked(&options.data, options.demo)
            && platform::monitor_running(options.legacy_port)
        {
            platform::local_request(options.legacy_port, "POST", "/api/shutdown")?;
        }
        println!("Stop requested. Your monitoring history is preserved.");
        return Ok(0);
    }
    let config = Config {
        router: options.router.clone().unwrap_or_default(),
        ..Config::default()
    };
    config.validate()?;
    if options.once {
        let gateway = options.router.or_else(probes::discover_gateway);
        let sample = probes::check(&config, gateway.as_deref());
        println!("{}", serde_json::to_string_pretty(&sample)?);
        return Ok(match sample.state {
            State::Online => 0,
            State::Unknown => 2,
            _ => 1,
        });
    }
    let mut instance = Instance::acquire(&options.data, options.demo)?;
    if instance.is_none() {
        // Briefly wait for a concurrent first launch to publish its IPC endpoint.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && instance::read(&options.data, options.demo).is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if instance::read(&options.data, options.demo).is_some() {
            if !options.background {
                instance::send(&options.data, options.demo, "open")?;
            }
            return Ok(0);
        }
        // Adopt an already running 0.1 instance without exposing its port conflict.
        if platform::monitor_running(options.legacy_port) {
            let _ = platform::local_request(options.legacy_port, "POST", "/api/shutdown");
            let deadline = Instant::now() + Duration::from_secs(10);
            while instance.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
                instance = Instance::acquire(&options.data, options.demo)?;
            }
        }
    }
    let Some(instance) = instance else {
        return Ok(0);
    };
    let mut store = if options.demo {
        Store::memory()?
    } else {
        Store::open(&options.data.join("history.sqlite3"))?
    };
    if options.demo {
        seed_demo(&mut store)?;
    }
    let app = App::new(store, options.demo)?;
    updates::cleanup(&options.data);
    let executable = std::env::current_exe()?.canonicalize()?;
    let legacy = instance::legacy_update_port(&options.data, &executable);
    app.updates.configure(updates::Runtime {
        executable,
        data: options.data.clone(),
        port: 0,
        no_tray: options.no_tray,
        headless: options.headless,
    });
    if let Some(router) = options.router {
        app.configure(Config {
            router,
            ..app.snapshot().config
        })?;
    }
    if options.start_paused && !options.demo {
        app.control("pause")?;
    }
    let signal_app = app.clone();
    ctrlc::set_handler(move || signal_app.stop())?;
    app.window_visible.store(
        !options.background || legacy.is_some(),
        std::sync::atomic::Ordering::Release,
    );
    let monitor = app.start();
    let ipc = instance.start(&app);
    updates::Updater::start_automatic(&app);
    if let Some(port) = legacy {
        instance::legacy_ready(port, &app, &instance);
    }
    let tray = if cfg!(windows) && !options.no_tray {
        let app = app.clone();
        let data = options.data.clone();
        Some(std::thread::spawn(move || {
            if let Err(error) = tray::run(app.clone(), data) {
                *app.ui_notice.lock().expect("notice") = Some(format!(
                    "Tray unavailable: {error}. Keep the window open to continue monitoring."
                ));
                app.request_window();
            }
        }))
    } else {
        None
    };
    let result = if options.headless {
        instance.ready()?;
        while !app.is_stopping() {
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    } else {
        native::run(
            app.clone(),
            instance.clone(),
            options.data,
            options.background && legacy.is_none(),
        )
    };
    app.stop();
    if let Some(monitor) = monitor {
        let _ = monitor.join();
    }
    let _ = ipc.join();
    if let Some(tray) = tray {
        let _ = tray.join();
    }
    app.store.lock().expect("store").interrupt()?;
    result?;
    Ok(0)
}
fn main() {
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        unsafe {
            use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, System::Console::*};
            let output = GetStdHandle(STD_OUTPUT_HANDLE);
            if output.is_null() || output == INVALID_HANDLE_VALUE {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            platform::show_error(&format!("Could not start monitoring: {error:#}"));
            1
        }
    };
    std::process::exit(code);
}
