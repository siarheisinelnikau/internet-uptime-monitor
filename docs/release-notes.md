Internet Uptime Monitor is a local Rust application that records when your connection fails and produces evidence you can share with your internet provider.

Version 0.1.1 fixes GitHub update checks by explicitly using the operating system’s trusted certificate store. Use this version instead of 0.1.0.

- Single Windows x64 EXE with an embedded dashboard, bundled SQLite, and static C runtime.
- Independent router checks and multiple public internet targets, with outage start times, durations, and recovery evidence.
- Daily, 7-day, and 30-day reports; readable text, CSV, and JSON exports.
- Native tray controls, portable data mode, pause/resume, and optional Windows sign-in startup.
- GitHub update checks and in-app download, SHA-256 verification, replacement, and restart. Failed startup attempts restore the previous executable.
- Sleep, pauses, stopped sessions, and probe execution errors stay separate from observed downtime.

Download **internet-uptime-monitor.exe** and double-click it. No installer, Python, Node.js, or WebView runtime is required. Use `--portable` to keep your history beside the EXE.

Updates are available in **Settings → Application updates**. Automatic checks can be disabled; updates install only when you click **Update & restart**.

This initial release measures IPv4 ICMP reachability, not DNS, HTTPS, bandwidth, or individual applications. The EXE is unsigned.
