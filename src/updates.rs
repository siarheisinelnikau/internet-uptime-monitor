//! GitHub releases are the sole update source. Installation is always user initiated.
use crate::{model::now, monitor::App};
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = "siarheisinelnikau/internet-uptime-monitor";
const LEGACY_EXE: &str = "internet-uptime-monitor.exe";
const MAX_EXE: u64 = 100 * 1024 * 1024;
const API: &str =
    "https://api.github.com/repos/siarheisinelnikau/internet-uptime-monitor/releases/latest";

#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    pub current_version: String,
    pub phase: String,
    pub latest_version: Option<String>,
    pub release_url: Option<String>,
    pub checked_at: Option<f64>,
    pub error: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub install_supported: bool,
}
impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            current_version: VERSION.into(),
            phase: "idle".into(),
            latest_version: None,
            release_url: None,
            checked_at: None,
            error: None,
            downloaded_bytes: 0,
            total_bytes: 0,
            install_supported: cfg!(all(windows, target_arch = "x86_64")),
        }
    }
}
impl UpdateStatus {
    fn busy(&self) -> bool {
        matches!(
            self.phase.as_str(),
            "checking" | "downloading" | "restarting"
        )
    }
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub executable: PathBuf,
    pub data: PathBuf,
    pub port: u16,
    pub no_tray: bool,
    pub headless: bool,
}
#[derive(Clone, Debug)]
struct Release {
    version: String,
    url: String,
    executable_url: String,
    executable_name: String,
    checksum_url: String,
    size: u64,
    digest: Option<String>,
}
#[derive(Default)]
pub struct Updater {
    state: Mutex<UpdateStatus>,
    available: Mutex<Option<Release>>,
    runtime: Mutex<Option<Runtime>>,
}

impl Updater {
    pub fn snapshot(&self) -> UpdateStatus {
        self.state.lock().expect("update state").clone()
    }
    pub fn configure(&self, runtime: Runtime) {
        if let Ok(bytes) = fs::read(runtime.data.join("update-result.json")) {
            if let Ok(result) = serde_json::from_slice::<ValueResult>(&bytes)
                && !result.ok
            {
                let mut state = self.state.lock().expect("update state");
                state.phase = "error".into();
                state.error = Some(result.message);
            }
            let _ = fs::remove_file(runtime.data.join("update-result.json"));
        }
        *self.runtime.lock().expect("update runtime") = Some(runtime);
    }
    pub fn check(self: &Arc<Self>) -> Result<()> {
        {
            let mut state = self.state.lock().expect("update state");
            ensure!(!state.busy(), "An update operation is already in progress.");
            state.phase = "checking".into();
            state.error = None;
        }
        let updater = self.clone();
        thread::spawn(move || {
            let result = fetch_latest();
            let mut state = updater.state.lock().expect("update state");
            state.checked_at = Some(now());
            match result {
                Ok(release) => {
                    state.latest_version = Some(release.version.clone());
                    state.release_url = Some(release.url.clone());
                    if newer(&release.version, VERSION).unwrap_or(false) {
                        state.phase = "available".into();
                        *updater.available.lock().expect("available release") = Some(release);
                    } else {
                        state.phase = "up_to_date".into();
                        *updater.available.lock().expect("available release") = None;
                    }
                }
                Err(error) => {
                    state.phase = "error".into();
                    state.error = Some(format!("Could not check GitHub: {error:#}"));
                }
            }
        });
        Ok(())
    }
    pub fn install(self: &Arc<Self>, app: &Arc<App>) -> Result<()> {
        ensure!(
            cfg!(all(windows, target_arch = "x86_64")),
            "In-app installation currently supports Windows x64 only."
        );
        ensure!(
            !app.snapshot().demo,
            "Updates cannot be installed from demo mode."
        );
        let runtime = self
            .runtime
            .lock()
            .expect("update runtime")
            .clone()
            .context("The updater is not configured.")?;
        let release;
        {
            let mut state = self.state.lock().expect("update state");
            ensure!(!state.busy(), "An update operation is already in progress.");
            release = self
                .available
                .lock()
                .expect("available release")
                .clone()
                .context("Check for a newer release first.")?;
            state.phase = "downloading".into();
            state.error = None;
            state.downloaded_bytes = 0;
            state.total_bytes = release.size;
        }
        let updater = self.clone();
        let app = app.clone();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let parent = runtime
                    .executable
                    .parent()
                    .context("Cannot locate the application folder.")?;
                let _probe=tempfile::NamedTempFile::new_in(parent).context("The application folder is not writable. Move the EXE to a writable folder before updating.")?;
                let updates = runtime.data.join("updates");
                fs::create_dir_all(&updates)?;
                let stage = tempfile::Builder::new()
                    .prefix("update-")
                    .tempdir_in(&updates)?;
                let checksum = download(&release.checksum_url, 64 * 1024, 30)?;
                let expected = parse_checksum(
                    std::str::from_utf8(&checksum).context("Invalid checksum file.")?,
                    &release.executable_name,
                )?;
                if let Some(digest) = &release.digest {
                    ensure!(
                        digest == &expected,
                        "GitHub's asset digest disagrees with SHA256SUMS.txt."
                    );
                }
                let new_exe = stage.path().join("new.exe");
                download_executable(&release, &new_exe, &updater)?;
                verify_executable(&new_exe, &expected)?;
                let helper = stage.path().join("helper.exe");
                fs::copy(&runtime.executable, &helper)?;
                let plan = UpdatePlan {
                    protocol: 2,
                    parent_pid: std::process::id(),
                    target: runtime.executable.clone(),
                    data: runtime.data.clone(),
                    port: runtime.port,
                    no_tray: runtime.no_tray,
                    paused: app.snapshot().paused,
                    version: release.version,
                    sha256: expected,
                    headless: runtime.headless,
                    start_hidden: !app
                        .window_visible
                        .load(std::sync::atomic::Ordering::Acquire),
                };
                let manifest = stage.path().join("plan.json");
                let mut file = File::create(&manifest)?;
                file.write_all(&serde_json::to_vec(&plan)?)?;
                file.sync_all()?;
                let mut command = Command::new(&helper);
                command.arg("--apply-update").arg(&manifest);
                hide(&mut command);
                command.spawn().context(
                    "Could not start the update helper. The current version is still running.",
                )?;
                let _ = stage.keep();
                updater.state.lock().expect("update state").phase = "restarting".into();
                // Let the browser receive its response before closing the local server.
                thread::sleep(Duration::from_millis(500));
                app.stop();
                Ok(())
            })();
            if let Err(error) = result {
                let mut state = updater.state.lock().expect("update state");
                state.phase = "error".into();
                state.error = Some(format!("Update failed: {error:#}"));
            }
        });
        Ok(())
    }
    pub fn start_automatic(app: &Arc<App>) {
        let weak = Arc::downgrade(app);
        thread::spawn(move || {
            let mut next = Instant::now() + Duration::from_secs(30);
            loop {
                thread::sleep(Duration::from_secs(1));
                let Some(app) = weak.upgrade() else { break };
                if app.is_stopping() {
                    break;
                }
                if Instant::now() >= next {
                    let status = app.snapshot();
                    if !status.demo && status.config.check_updates_automatically {
                        let _ = app.updates.check();
                    }
                    next = Instant::now() + Duration::from_secs(86400);
                }
            }
        });
    }
}

fn client(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(seconds)))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}
fn download(url: &str, limit: u64, seconds: u64) -> Result<Vec<u8>> {
    Ok(client(seconds)
        .get(url)
        .header("User-Agent", format!("InternetUptimeMonitor/{VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()?)
}
fn fetch_latest() -> Result<Release> {
    let bytes = download(API, 1024 * 1024, 20)?;
    parse_release(&serde_json::from_slice(&bytes)?)
}
fn newer(remote: &str, current: &str) -> Result<bool> {
    Ok(Version::parse(remote.trim_start_matches('v'))?
        > Version::parse(current.trim_start_matches('v'))?)
}
fn parse_release(value: &serde_json::Value) -> Result<Release> {
    ensure!(
        value["draft"] == false && value["prerelease"] == false,
        "The release is not a published stable release."
    );
    let tag = value["tag_name"].as_str().context("Missing release tag.")?;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag))?;
    ensure!(
        version.pre.is_empty(),
        "Pre-release updates are not installed automatically."
    );
    let url = format!("https://github.com/{REPOSITORY}/releases/tag/{tag}");
    ensure!(
        value["html_url"].as_str() == Some(url.as_str()),
        "Unexpected release repository."
    );
    let assets = value["assets"]
        .as_array()
        .context("Missing release assets.")?;
    let asset = |name: &str| -> Result<&serde_json::Value> {
        let matches = assets
            .iter()
            .filter(|v| v["name"] == name)
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "The release must contain exactly one {name} asset."
        );
        Ok(matches[0])
    };
    let executable_name = if version < Version::new(0, 2, 1) {
        LEGACY_EXE.to_owned()
    } else {
        format!("InternetUptimeMonitor-v{version}.exe")
    };
    let exe = asset(&executable_name)?;
    let sums = asset("SHA256SUMS.txt")?;
    let prefix = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/");
    let executable_url = format!("{prefix}{executable_name}");
    let checksum_url = format!("{prefix}SHA256SUMS.txt");
    ensure!(
        exe["browser_download_url"] == executable_url
            && sums["browser_download_url"] == checksum_url,
        "Unexpected update download URL."
    );
    let size = exe["size"].as_u64().context("Missing executable size.")?;
    ensure!(
        (1024..=MAX_EXE).contains(&size),
        "Unexpected executable size."
    );
    let digest = exe["digest"]
        .as_str()
        .map(|s| {
            s.strip_prefix("sha256:")
                .context("Unsupported asset digest.")
                .and_then(valid_hash)
        })
        .transpose()?;
    Ok(Release {
        version: version.to_string(),
        url,
        executable_url,
        executable_name,
        checksum_url,
        size,
        digest,
    })
}
fn valid_hash(value: &str) -> Result<String> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid SHA-256 checksum."
    );
    Ok(value.to_ascii_lowercase())
}
fn parse_checksum(text: &str, executable_name: &str) -> Result<String> {
    let mut found = None;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() == 2 && fields[1].trim_start_matches('*') == executable_name {
            ensure!(found.is_none(), "Duplicate executable checksum.");
            found = Some(valid_hash(fields[0])?);
        }
    }
    found.context("The release has no SHA-256 checksum for this executable.")
}
fn download_executable(release: &Release, path: &Path, updater: &Updater) -> Result<()> {
    let mut response = client(300)
        .get(&release.executable_url)
        .header("User-Agent", format!("InternetUptimeMonitor/{VERSION}"))
        .call()?;
    let mut reader = response.body_mut().with_config().limit(MAX_EXE).reader();
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut buffer = [0u8; 65536];
    let mut count = 0;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])?;
        count += n as u64;
        updater.state.lock().expect("update state").downloaded_bytes = count;
    }
    file.sync_all()?;
    ensure!(count == release.size, "The update download was incomplete.");
    Ok(())
}
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn verify_executable(path: &Path, expected: &str) -> Result<()> {
    ensure!(
        hash_file(path)? == valid_hash(expected)?,
        "SHA-256 verification failed. The current executable has not been changed."
    );
    let mut file = File::open(path)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)?;
    ensure!(
        &header[..2] == b"MZ",
        "The update is not a Windows executable."
    );
    use std::io::{Seek, SeekFrom};
    let offset = u32::from_le_bytes(header[60..64].try_into()?);
    file.seek(SeekFrom::Start(offset as u64))?;
    let mut signature = [0u8; 6];
    file.read_exact(&mut signature)?;
    ensure!(
        &signature[..4] == b"PE\0\0" && u16::from_le_bytes(signature[4..6].try_into()?) == 0x8664,
        "The update is not a Windows x64 executable."
    );
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePlan {
    pub protocol: u32,
    pub parent_pid: u32,
    pub target: PathBuf,
    pub data: PathBuf,
    pub port: u16,
    pub no_tray: bool,
    pub paused: bool,
    pub version: String,
    pub sha256: String,
    #[serde(default)]
    pub headless: bool,
    #[serde(default)]
    pub start_hidden: bool,
}
#[derive(Serialize, Deserialize)]
struct ValueResult {
    ok: bool,
    message: String,
}
fn hide(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(not(windows))]
    let _ = command;
}
fn launch(plan: &UpdatePlan) -> Result<Child> {
    let mut command = Command::new(&plan.target);
    command
        .arg("--data-dir")
        .arg(&plan.data)
        .arg("--port")
        .arg(plan.port.to_string());
    if plan.start_hidden {
        command.arg("--background");
    }
    if plan.headless {
        command.arg("--headless");
    }
    if plan.no_tray {
        command.arg("--no-tray");
    }
    if plan.paused {
        command.arg("--start-paused");
    }
    hide(&mut command);
    Ok(command.spawn()?)
}
#[cfg(windows)]
fn wait_for_parent(pid: u32) -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, WAIT_OBJECT_0},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            ensure!(
                GetLastError() == 87,
                "Cannot wait for the running monitor to exit."
            );
            return Ok(());
        }
        let result = WaitForSingleObject(process, 60000);
        CloseHandle(process);
        ensure!(
            result == WAIT_OBJECT_0,
            "The running monitor did not exit. No files were replaced."
        );
    }
    Ok(())
}
#[cfg(not(windows))]
fn wait_for_parent(_pid: u32) -> Result<()> {
    bail!("In-app installation currently supports Windows x64 only.")
}

/// Invoked in a separate copy of the current EXE after a verified download.
pub fn apply_update(manifest: &Path) -> Result<()> {
    use fs2::FileExt;
    let plan: UpdatePlan = serde_json::from_slice(&fs::read(manifest)?)?;
    ensure!(
        matches!(plan.protocol, 1 | 2) && plan.parent_pid != std::process::id(),
        "Invalid update plan."
    );
    Version::parse(&plan.version)?;
    valid_hash(&plan.sha256)?;
    let stage = manifest
        .parent()
        .context("Missing staging folder.")?
        .canonicalize()?;
    let updates = plan.data.join("updates").canonicalize()?;
    ensure!(
        stage.parent() == Some(updates.as_path())
            && stage
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("update-")),
        "The update must be staged in the monitor's data directory."
    );
    ensure!(
        plan.target.is_absolute()
            && plan
                .target
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("exe")),
        "Invalid executable path."
    );
    let mut restored = false;
    let result = (|| -> Result<()> {
        let parent = plan
            .target
            .parent()
            .context("Missing application directory.")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(".uptime-update.lock"))?;
        lock.try_lock_exclusive()
            .context("Another update is already replacing this application.")?;
        let downloaded = stage.join("new.exe");
        verify_executable(&downloaded, &plan.sha256)?;
        wait_for_parent(plan.parent_pid)?;
        let candidate = tempfile::Builder::new()
            .prefix(".uptime-new-")
            .suffix(".exe")
            .tempfile_in(parent)?;
        fs::copy(&downloaded, candidate.path())?;
        verify_executable(candidate.path(), &plan.sha256)?;
        let candidate = candidate.into_temp_path();
        let backup = parent.join(format!(
            ".uptime-{}.backup.exe",
            stage.file_name().unwrap().to_string_lossy()
        ));
        ensure!(!backup.exists(), "An update backup already exists.");
        fs::rename(&plan.target, &backup).context(
            "Could not replace the running EXE. Another instance may still be using it.",
        )?;
        if let Err(error) = fs::rename(&candidate, &plan.target) {
            fs::rename(&backup, &plan.target).context("Could not restore the previous executable. The backup is in the application folder.")?;
            restored = true;
            return Err(error.into());
        }
        let outcome = (|| -> Result<()> {
            let mut child = launch(&plan).context("The updated executable could not start.")?;
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                if child.try_wait()?.is_some() {
                    bail!("The updated application exited during startup.");
                }
                if crate::instance::read(&plan.data, false)
                    .is_some_and(|v| v.ready && v.version == plan.version && v.pid == child.id())
                {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(300));
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!("The updated application did not become ready.")
        })();
        if let Err(error) = outcome {
            // Restore only the file we just installed, never an independently modified EXE.
            if hash_file(&plan.target)? == plan.sha256 {
                fs::remove_file(&plan.target)?;
                fs::rename(&backup, &plan.target)?;
                restored = true;
                bail!("{error:#} The previous executable was restored.");
            }
            bail!(
                "{error:#} The executable changed independently; its backup was left in the application folder."
            );
        }
        let _ = fs::remove_file(backup);
        Ok(())
    })();
    let record = ValueResult {
        ok: result.is_ok(),
        message: result
            .as_ref()
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| format!("Updated to {}.", plan.version)),
    };
    let _ = fs::write(
        plan.data.join("update-result.json"),
        serde_json::to_vec(&record)?,
    );
    let _ = fs::write(stage.join("completed"), b"1");
    if restored {
        let _ = launch(&plan);
    }
    result
}

/// Only completed, direct child staging folders created by this application are removed.
pub fn cleanup(data: &Path) {
    let root = data.join("updates");
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("update-")
            && entry
                .file_type()
                .is_ok_and(|t| t.is_dir() && !t.is_symlink())
            && entry.path().join("completed").is_file()
            && fs::remove_dir_all(entry.path()).is_err()
            && entry.path().is_dir()
        {
            // A helper can still be running during restart; retain the retry marker.
            let _ = fs::write(entry.path().join("completed"), b"1");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const EXE: &str = "InternetUptimeMonitor-v0.2.1.exe";
    #[test]
    fn native_tls_explicitly_uses_the_platform_certificate_store() {
        assert!(matches!(
            client(1).config().tls_config().root_certs(),
            ureq::tls::RootCerts::PlatformVerifier
        ));
    }
    fn release() -> serde_json::Value {
        let prefix = format!("https://github.com/{REPOSITORY}/releases/download/v0.2.1/");
        json!({"tag_name":"v0.2.1","draft":false,"prerelease":false,"html_url":format!("https://github.com/{REPOSITORY}/releases/tag/v0.2.1"),"assets":[{"name":EXE,"size":2000,"browser_download_url":format!("{prefix}{EXE}")},{"name":"SHA256SUMS.txt","browser_download_url":format!("{prefix}SHA256SUMS.txt")}]})
    }
    #[test]
    fn compares_numeric_versions_and_never_downgrades() {
        assert!(newer("v0.10.0", "0.9.0").unwrap());
        assert!(!newer("v0.1.0", "0.1.0").unwrap());
        assert!(!newer("0.1.0", "0.2.0").unwrap());
        assert!(newer("../../bad", "0.1.0").is_err());
    }
    #[test]
    fn accepts_only_complete_stable_releases_from_our_repository() {
        assert!(parse_release(&release()).is_ok());
        for path in ["prerelease", "draft"] {
            let mut v = release();
            v[path] = json!(true);
            assert!(parse_release(&v).is_err());
        }
        let mut v = release();
        v["assets"][0]["browser_download_url"] = json!("https://attacker.example/file.exe");
        assert!(parse_release(&v).is_err());
        let mut v = release();
        v["assets"].as_array_mut().unwrap().pop();
        assert!(parse_release(&v).is_err());
        let mut v = release();
        v["assets"][0]["size"] = json!(MAX_EXE + 1);
        assert!(parse_release(&v).is_err());
    }
    #[test]
    fn checksums_are_exact_and_unambiguous() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_checksum(&format!("{hash}  {EXE}\n"), EXE).unwrap(),
            hash
        );
        assert_eq!(
            parse_checksum(&format!("\u{feff}{hash} *{EXE}\r\n"), EXE).unwrap(),
            hash
        );
        assert!(parse_checksum(&format!("{hash} other.exe"), EXE).is_err());
        assert!(parse_checksum(&format!("bad {EXE}"), EXE).is_err());
        assert!(parse_checksum(&format!("{hash} {EXE}\n{hash} {EXE}"), EXE).is_err());
        assert!(parse_checksum(&format!("{hash} {LEGACY_EXE}"), EXE).is_err());
    }
    #[test]
    fn legacy_assets_are_accepted_only_for_historical_versions() {
        let legacy = release()
            .to_string()
            .replace(EXE, LEGACY_EXE)
            .replace("0.2.1", "0.2.0");
        assert_eq!(
            parse_release(&serde_json::from_str(&legacy).unwrap())
                .unwrap()
                .executable_name,
            LEGACY_EXE
        );
        let wrong = release().to_string().replace(EXE, LEGACY_EXE);
        assert!(parse_release(&serde_json::from_str(&wrong).unwrap()).is_err());
    }
    #[test]
    fn rejects_tampered_downloads_before_replacing_anything() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.exe");
        fs::write(&path, b"not an executable").unwrap();
        assert!(verify_executable(&path, &"a".repeat(64)).is_err());
        assert!(verify_executable(&path, &hash_file(&path).unwrap()).is_err());
    }
    #[test]
    fn cleanup_preserves_unfinished_and_unrelated_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("updates");
        for name in ["update-done", "update-pending", "other"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::write(root.join("update-done/completed"), b"1").unwrap();
        fs::write(root.join("other/completed"), b"1").unwrap();
        cleanup(dir.path());
        assert!(!root.join("update-done").exists());
        assert!(root.join("update-pending").exists());
        assert!(root.join("other").exists());
    }
}
