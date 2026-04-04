# Internet Uptime Monitor

**Show when your internet actually went down.**

Calls drop. Videos freeze. By the time support arrives, everything works again.
This local monitor quietly records interruptions and gives you a clear report:
**“12 outages. 8 minutes of downtime.”**

Written in **Rust**. Distributed as **one Windows EXE**, with SQLite, the dashboard,
icons, and the C runtime bundled in. No installer, account, subscription, session
limit, Python, Node.js, or WebView runtime is needed to use it.

![Dashboard with clearly labeled demo data](docs/screenshots/overview.png)

## Run it

Download the EXE from [GitHub Releases](https://github.com/siarheisinelnikau/internet-uptime-monitor/releases/latest).
Double-click `internet-uptime-monitor.exe`. The monitor starts in your Windows
notification area and opens [the local dashboard](http://127.0.0.1:8765).
Close the browser tab whenever you like; the background process keeps recording.

Double-click the tray icon to reopen the dashboard. Right-click it to pause or
resume checks, enable startup at Windows sign-in, or **Quit monitor**. Starting
the executable again opens the existing instance instead of duplicating checks.

To build from source, install Rust and the Windows C++ build tools, then run:

```powershell
cargo build --locked --release
.\target\release\internet-uptime-monitor.exe
```

Only the resulting `target/release/internet-uptime-monitor.exe` needs to be copied
to another Windows computer. Its interface is served by the executable over
loopback and uses your existing browser. It does not extract or download assets.
Windows 10/11 x64 is the primary target. The executable is currently unsigned.
Source builds and automated tests also target Linux and macOS; their probes use
system `ping` plus `ip` (Linux) or `route` (macOS), without a tray menu.

## What you get

- **Overview:** live connection status, availability, outage count, total downtime,
  a connection timeline, latency history, and separate router evidence.
- **Outage history:** start time, duration, recovery status, and likely problem
  location. Filter local-network and upstream incidents.
- **Reports:** today, the last 7 days, or the last 30 days. Export readable text,
  a CSV outage spreadsheet, or complete JSON report data.
- **Settings:** check interval, response timeout, minimum reportable outage length,
  router address, and two to five independent IPv4 targets.
- **Quiet operation:** a native tray menu, pause/resume, persistent SQLite history,
  optional Windows sign-in startup, and duplicate-instance protection.
- **Offline interface:** all assets are embedded in the executable. The dashboard
  remains available when the internet goes down.
- **In-app updates:** check this repository’s GitHub releases, verify the download,
  and restart into the newer version while keeping your history and preferences.

### Updates

Open **Settings → Application updates** to check manually. A new version also
appears in a dashboard notice. Click **Update & restart** to download and install
it. Connection checks continue during download; the restart gap stays unmonitored.
The dashboard reconnects automatically.

Automatic checks run about 30 seconds after startup and once a day. Disable them
in Settings if you prefer manual checks. Nothing installs without your click.
Only published stable releases from this repository are accepted. The download
must match `SHA256SUMS.txt` and, when supplied, GitHub’s asset digest. A temporary
copy of the current EXE waits for shutdown, replaces the EXE at its existing path,
and restores the previous binary if the new process cannot start successfully.
Portable data paths, ports, tray preferences, and paused state are preserved.

The application folder must be writable. Completed staging files are removed on
the next startup. In-app installation supports Windows x64; other source builds
can check release information and download manually. Checksums verify integrity;
they do not replace code signing or protect against a compromised release account.

### Portable mode

```powershell
.\internet-uptime-monitor.exe --portable
```

This creates a `data` directory beside the EXE. Without `--portable`, evidence is
saved in `%LOCALAPPDATA%\InternetUptimeMonitor`. Moving the EXE does not remove
history. Single EXE describes the distributed application; recording observations
naturally creates a database.

### Background operation and startup

```powershell
.\internet-uptime-monitor.exe --no-browser
.\internet-uptime-monitor.exe --stop
```

Enable **Start with Windows** from the tray menu, or use:

```powershell
.\internet-uptime-monitor.exe --enable-startup
.\internet-uptime-monitor.exe --disable-startup
```

This changes only the current user’s `InternetUptimeMonitor` value in
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. Administrator rights are
not normally required. Keep the executable in place after enabling startup.
Pass `--portable` or `--data-dir` when enabling startup to preserve that location.
Startup is opt-in and is never enabled just by running the application.

### Explore example data

```powershell
.\internet-uptime-monitor.exe --demo --port 8766
```

Demo mode is visibly labeled, uses an in-memory database, sends no probes, and
never mixes sample outages into real history. Choose **Last 7 days** to see all
12 example incidents. Preferences and monitoring controls are read-only.

### More command-line options

```text
--no-browser       Start without opening a browser tab
--no-tray          Run without a Windows tray icon
--port 9000        Use a different localhost port
--data-dir PATH    Choose the directory for history and preferences
--portable         Keep data beside the executable
--router IPV4      Override automatic gateway detection
--once             Print one real check as JSON, without writing history
--demo             Explore isolated, labeled example data
--stop             Stop the monitor on the selected port
--enable-startup   Start quietly at Windows sign-in
--disable-startup  Remove automatic Windows startup
--help             Show all options
```

`--once` returns `0` if any public target replies, `1` if all are unreachable,
and `2` if a probe could not execute. Use `--stop --port 9000` to stop an instance
on another port. On Windows, pipe `--once` to `Out-String` if your terminal does
not wait for a GUI executable’s output.

## How the evidence works

By default, the monitor checks your router and three independent public
addresses concurrently every five seconds: Cloudflare (`1.1.1.1`), Google
(`8.8.8.8`), and Quad9 (`9.9.9.9`). Windows probes and gateway discovery use
native APIs, without spawning a shell or requiring a separate ping utility.

| Public targets | Router | Recorded observation |
| --- | --- | --- |
| Any target replies | Any state | Online |
| All fail | Replies | Internet unreachable; an upstream issue is possible |
| All fail | No reply | Router and internet unreachable; check the local network |
| All fail | Unavailable | Internet unreachable; location undetermined |
| No replies, and a public probe could not execute | Any state | Unknown; excluded from availability |

A single failed target does **not** create an outage. A change in router
reachability during a continuous outage updates its evidence without counting
a second outage. An outage ends on the first successful public reply. Ongoing
incidents appear once they reach the minimum length configured in Settings.

Durations are estimates at the sampling resolution. The interval between checks
is attributed to the previous observation. Start and recovery times can differ
from the actual event by approximately one interval; shorter outages may be
missed. Tiny failure markers are widened on the timeline for visibility;
tooltips and reports show observed durations.

Sleep, restarts, pauses, probe execution failures, and gaps longer than
`max(30 seconds, 3 × check interval)` do not become fabricated downtime. If
observation stops during an outage, it is marked **interrupted**, ending at the
last observed check. Recovery is not assumed. After a backwards clock correction,
recording waits until time advances past existing coverage.

**Availability** is successful observed time divided by known observed time,
including interruptions shorter than the report threshold. **Outage count and
total downtime** include events meeting the threshold (five seconds by default).
Reports clip outages crossing period boundaries and show monitoring coverage.
A few minutes of checks do not claim a full day of uptime.

Router replies are useful evidence, not proof of ISP responsibility. ICMP
filtering can resemble connectivity failure. This version checks IPv4 ICMP;
it does not test DNS, HTTPS, captive portals, IPv6, bandwidth, or whether a
specific application works. Some routers ignore ICMP. Set a manual gateway
address in Settings if automatic detection chooses the wrong interface.

## Data and privacy

The dashboard listens only on `127.0.0.1`. Host and Origin checks reject ordinary
cross-origin writes and DNS-rebinding access. There is no cloud service,
analytics, account, or remote management. Update checks contact GitHub for public
release metadata, and installing an update downloads its EXE and checksums. Your
connection history and preferences are not sent. Other applications on the same machine
are trusted; this is a personal local tool, not a hardened multi-user service.

| System | Default data directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\InternetUptimeMonitor` |
| macOS | `~/Library/Application Support/InternetUptimeMonitor` |
| Linux | `$XDG_DATA_HOME/internet-uptime-monitor` or `~/.local/share/internet-uptime-monitor` |

`history.sqlite3` stores preferences, connection intervals, outages, and raw check
evidence. Raw checks are retained for 30 days and pruned at startup and daily.
Outages and connection intervals are retained indefinitely. Stop the monitor
before backing up its database. Browser times use your device’s timezone;
export timestamps are UTC. Probe and storage errors appear in the dashboard
instead of being counted as connectivity failures.

## Development

The Rust application embeds the plain HTML, CSS, and JavaScript in `ui/` at
compile time. Editing the dashboard requires rebuilding the executable.
SQLite and the Windows C runtime are statically linked for Windows x64 builds.
No frontend build step is required.

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

Tests cover classification, validation, event merging, thresholds, report
boundaries, sleep gaps, pause/resume, unknown observations, crash recovery,
SQLite persistence, localhost HTTP controls, exports, and origin checks. Unit
tests mock connectivity and do not require public targets to be reachable.
GitHub Actions runs checks on Windows, Linux, and macOS and uploads the Windows
EXE. Pushing a `v*` tag builds a release with the EXE and a SHA-256 checksum.

| Path | Responsibility |
| --- | --- |
| `src/probes.rs` | Native Windows ICMP and gateway discovery; Unix ping |
| `src/monitor.rs` | Background sampling, controls, and isolated demo mode |
| `src/storage.rs` | SQLite evidence, observed intervals, and reports |
| `src/server.rs` | Loopback HTTP API, embedded UI, and exports |
| `src/tray.rs` | Native Windows tray menu and status |
| `src/platform.rs` | Data locations, browser launching, and sign-in startup |
| `src/updates.rs` | GitHub release checks, download verification, replacement, and recovery |
| `ui/` | Responsive English-language dashboard |
| `tests/http.rs` | End-to-end localhost API tests |

An independent open-source implementation inspired by
[Net Uptime Monitor](https://netuptimemonitor.com/details/). Not affiliated with
that product. Licensed under the [MIT License](LICENSE).
