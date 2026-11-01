Internet Uptime Monitor 0.3.0 adds six interface languages and a persistent dark theme while preserving your monitoring history and preferences.

## Added

- Choose English, Russian, French, German, Spanish, or Italian in **Settings → Appearance and language**. Navigation, connection status, outage history, settings, updates, and tray menus follow the selected language.
- Switch between **Light** and **Dark** in the same settings card. Both language and theme apply immediately and save automatically.
- Readable text exports follow the selected language. CSV and JSON retain their existing field names and UTC timestamps for compatibility.

## Improved

- Appearance changes preserve active monitoring and unsaved monitoring preferences. Upgrading from older settings uses English and Light without changing saved sampling or update preferences.
- Dark mode applies to cards, inputs, selection, notices, charts, and text. Status labels use readable colors in both themes.
- Local timestamps use an unambiguous numeric format. Addresses, target names, and technical OS/network details keep their original values.
- Embedded catalogs validate all translations and interpolation placeholders. Focused regressions cover preference migration, saved appearance without a monitoring reset, and translated readable reports.

## Download and upgrade

Download **InternetUptimeMonitor-v0.3.0.exe** for Windows 10/11 x64. This is one unsigned portable executable; no installer or browser runtime is required. **SHA256SUMS.txt** contains its SHA-256 checksum.

Version 0.2.1 or newer can install this release through **Settings → Application updates → Check for updates → Download and install**. When upgrading from 0.2.0 or earlier, download and run the new EXE manually once because those older updaters require the previous unversioned asset name. Existing settings and history are preserved; portable users should place the new EXE beside their existing `data` folder and stop the old instance before starting it.

[Changes since v0.2.2](https://github.com/siarheisinelnikau/internet-uptime-monitor/compare/v0.2.2...v0.3.0)
