Internet Uptime Monitor now opens as a **native Rust desktop application**.

- Native overview, connection timeline, latency chart, outage history, reports, settings, and in-app updates.
- No browser, WebView runtime, or localhost port required for normal startup.
- An occupied port cannot prevent the app from starting. Repeated launches activate the existing window through local IPC.
- Close the window to keep monitoring in the tray. Double-click the EXE or tray icon to reopen it; choose Quit monitor to stop.
- Native save dialogs for readable text, CSV, and JSON reports.
- Existing monitoring history and preferences are preserved. Portable mode and Windows sign-in startup remain supported.
- GitHub update downloads are verified with SHA-256. Native readiness checks support restart and recovery without a dashboard server.

Download **internet-uptime-monitor.exe** and double-click it. The single EXE includes the native UI and SQLite. Use `--portable` to keep history beside it.

Version 0.1.1 can install this release using Settings → Application updates. A brief compatibility handshake supports that older updater; normal native startup binds no port. The EXE is unsigned and measures IPv4 ICMP reachability.
