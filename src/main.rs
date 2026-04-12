#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use internet_uptime_monitor::{
    model::{Config, State},
    monitor::{App, seed_demo},
    platform, probes, server,
    storage::Store,
    tray, updates,
};
use std::{fs::OpenOptions, path::PathBuf, sync::Arc};

const HELP: &str = "Quiet, local connection monitoring. No account or session limits.

Usage: internet-uptime-monitor.exe [OPTIONS]

  --no-browser       Start quietly without opening the dashboard
  --no-tray          Run without a Windows notification-area icon
  --port NUMBER      Local dashboard port (default: 8765)
  --data-dir PATH    Directory for the SQLite history and preferences
  --portable         Keep data next to this executable in a data folder
  --router IPV4      Override automatic router discovery
  --once             Print one real ICMP check as JSON, without writing history
  --demo             Explore labeled, isolated example data in memory
  --stop             Gracefully stop the monitor on the selected port
  --enable-startup   Start quietly when you sign in to Windows
  --disable-startup  Remove automatic Windows startup
  -h, --help         Show this help
  -V, --version      Show the version

Double-click to start. Open http://127.0.0.1:8765 or double-click the tray icon.
Right-click the tray icon to open, pause, resume, configure startup, or quit.
";

struct Options {
    port: u16,
    data: PathBuf,
    no_browser: bool,
    no_tray: bool,
    once: bool,
    demo: bool,
    stop: bool,
    start_paused: bool,
    startup: Option<bool>,
    router: Option<String>,
    help: bool,
    version: bool,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut options = Self {
            port: 8765,
            data: platform::data_dir(),
            no_browser: false,
            no_tray: false,
            once: false,
            demo: false,
            stop: false,
            start_paused: false,
            startup: None,
            router: None,
            help: false,
            version: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-browser" => options.no_browser = true,
                "--no-tray" => options.no_tray = true,
                "--once" => options.once = true,
                "--demo" => options.demo = true,
                "--stop" => options.stop = true,
                "--start-paused" => options.start_paused = true,
                "--enable-startup" => options.startup = Some(true),
                "--disable-startup" => options.startup = Some(false),
                "--router" => {
                    options.router = Some(args.next().context("--router needs an IPv4 address.")?)
                }
                "--port" => {
                    options.port = args
                        .next()
                        .context("--port needs a number.")?
                        .parse()
                        .context("Port must be between 1 and 65535.")?
                }
                "--data-dir" => {
                    options.data = PathBuf::from(args.next().context("--data-dir needs a path.")?)
                }
                "--portable" => {
                    options.data = std::env::current_exe()?
                        .parent()
                        .context("No executable directory.")?
                        .join("data")
                }
                "-h" | "--help" => options.help = true,
                "-V" | "--version" => options.version = true,
                _ => bail!("Unknown option: {arg}. Use --help for options."),
            }
        }
        if options.port == 0 {
            bail!("Port must be between 1 and 65535.");
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
        let path = PathBuf::from(internal.next().context("Missing update plan.")?);
        anyhow::ensure!(internal.next().is_none(), "Unexpected update arguments.");
        updates::apply_update(&path)?;
        return Ok(0);
    }
    let options = Options::parse()?;
    if options.help {
        println!("Internet Uptime Monitor {}\n{HELP}", updates::VERSION);
        return Ok(0);
    }
    if options.version {
        println!("Internet Uptime Monitor {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if let Some(enable) = options.startup {
        platform::set_startup(enable, &options.data, options.port)?;
        println!(
            "Automatic Windows startup {}.",
            if enable { "enabled" } else { "disabled" }
        );
        return Ok(0);
    }
    if options.stop {
        if !platform::monitor_running(options.port) {
            bail!(
                "No Internet Uptime Monitor is running on port {}.",
                options.port
            );
        }
        platform::local_request(options.port, "POST", "/api/shutdown")?;
        println!("Monitor stopped. Your history is saved.");
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
    std::fs::create_dir_all(&options.data).context("Could not create the data directory.")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(options.data.join(if options.demo {
            "demo.lock"
        } else {
            "monitor.lock"
        }))?;
    if lock.try_lock_exclusive().is_err() {
        if platform::monitor_running(options.port) {
            if !options.no_browser {
                platform::open_dashboard(options.port)?;
            }
            return Ok(0);
        }
        bail!(
            "A monitor is already using this data directory. Open its dashboard or use --stop with its port."
        );
    }
    let listener = server::bind(options.port)?;
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
    app.updates.configure(updates::Runtime {
        executable: std::env::current_exe()?.canonicalize()?,
        data: options.data.clone(),
        port: options.port,
        no_tray: options.no_tray,
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
    let signal_app = Arc::clone(&app);
    ctrlc::set_handler(move || signal_app.stop())?;
    let monitor = app.start();
    let workers = server::start(listener, &app);
    updates::Updater::start_automatic(&app);
    println!(
        "Internet Uptime Monitor: http://127.0.0.1:{}\nData: {}",
        options.port,
        options.data.display()
    );
    if !options.no_browser {
        let _ = platform::open_dashboard(options.port);
    }
    let outcome = if options.no_tray {
        while !app.is_stopping() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Ok(())
    } else {
        tray::run(app.clone(), options.port, options.data)
    };
    app.stop();
    if let Some(handle) = monitor {
        let _ = handle.join();
    }
    for worker in workers {
        let _ = worker.join();
    }
    app.store.lock().expect("store lock").interrupt()?;
    outcome?;
    Ok(0)
}

fn main() {
    #[cfg(windows)]
    if std::env::args_os().len() > 1 {
        // Keep terminal commands usable while double-click startup has no console window.
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
            let message = format!("Cannot start the monitor: {error:#}");
            if std::env::args_os().len() > 1 {
                eprintln!("{message}");
            } else {
                platform::show_error(&message);
            }
            1
        }
    };
    std::process::exit(code);
}
