Internet Uptime Monitor 0.2.1 fixes reopening the native Windows window after it has been hidden in the tray.

## Fixed

- Tray clicks and repeated EXE launches wake the actual native window before queuing UI commands, so a hidden window can reopen.
- A single left click now opens the monitor, alongside double-click and the existing Open monitor menu action.
- Minimized windows are restored, background startup keeps the window hidden, and Quit can finish when the window is hidden.
- Monitoring continues while the window is hidden, using the same instance and history.

## Improved

- The standalone Windows x64 download is now named **InternetUptimeMonitor-v0.2.1.exe**. SHA256SUMS.txt and the in-app updater use the same versioned name.
- Settings clearly label **Check for updates on startup** and **Download and install**. Startup checks remain optional and do not block launch; downloads and installation require your action.
- README now includes fresh screenshots of the application and its settings, tray behavior, and upgrade instructions.
- Added an opt-in Windows regression that exercises real GUI/tray window handles, repeated hide/reopen cycles, minimization, background startup, duplicate launch, continued recording, and hidden shutdown. All 30 standard tests and the interactive-desktop regression passed locally, along with formatting and Clippy checks. The regression dispatches tray callbacks; it does not substitute for manual menu/focus testing.

## Upgrade from 0.2.0 or earlier

Download and run **InternetUptimeMonitor-v0.2.1.exe** manually once. Older updaters hardcode the previous unversioned asset name and cannot find this newly named download. Existing settings and history are preserved. For portable installations, place the new EXE beside your existing data folder and stop the old instance before starting it. Version 0.2.1 supports versioned assets for future in-app updates.

One unsigned Windows 10/11 x64 executable; no installer or browser runtime. SHA-256 checksums are included.

[Changes since v0.2.0](https://github.com/siarheisinelnikau/internet-uptime-monitor/compare/v0.2.0...v0.2.1)
