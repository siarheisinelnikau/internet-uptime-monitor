Internet Uptime Monitor 0.2.2 moves Windows drawing to a CPU renderer with GDI presentation and fixes unnecessary event-loop work.

## Improved

- The Windows interface renders egui directly on the CPU and presents a bitmap through GDI, without creating an OpenGL or DirectX context. Linux and macOS continue to use the existing Glow renderer.
- Windows paint events reuse the last rendered bitmap. UI frames follow repaint deadlines and are limited to roughly 60 per second during interaction.
- Hidden and minimized windows skip rasterization and wait for events. Tray and duplicate-launch commands can still restore the window, and monitoring continues in the background.
- Added an opt-in Windows regression that measures CPU use with a populated Overview visible, hidden, restored, and receiving pointer events.
- Optional local frame timing is available through `UPTIME_RENDER_TRACE`; tracing is disabled by default.
- README screenshots now show the CPU-rendered Overview and version 0.2.2 settings.

## Validation

Formatting, Clippy, all 30 standard tests, and the release build passed locally on Windows. Both interactive-desktop regressions passed against the packaged executable, covering tray restoration, background recording, hidden shutdown, and CPU use. The CPU regression measured 0.39–3.91% of one core across its idle states and 18.34% during pointer events on the test machine; results vary by hardware and workload.

## Download and upgrade

Download **InternetUptimeMonitor-v0.2.2.exe** for Windows 10/11 x64. This is one unsigned portable executable; no installer or browser runtime is required. **SHA256SUMS.txt** contains its SHA-256 checksum.

Version 0.2.1 can install this release through **Settings → Application updates → Check for updates → Download and install**. When upgrading from 0.2.0 or earlier, download and run the new EXE manually once because those older updaters require the previous unversioned asset name. Existing settings and history are preserved; portable users should place the new EXE beside their existing `data` folder and stop the old instance before starting it.

[Changes since v0.2.1](https://github.com/siarheisinelnikau/internet-uptime-monitor/compare/v0.2.1...v0.2.2)
