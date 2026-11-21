//! The updater of a running app: one per process, driven by the window
//! through [`check_now`], [`install`], [`restart_now`] and read back with
//! [`status_json`] (the windows poll it).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::install::{self, InstallError};
use crate::kind::{self, App, InstallKind, Plan};
use crate::release::Release;
use crate::verify::{self, RELEASE_SIGNING_PUBLIC_KEY_B64, SIG_NAME, SUMS_NAME};

/// First automatic check after the start, then every day.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// Set (to anything) to turn the automatic check off, e.g. in tests.
pub const NO_CHECK_ENV: &str = "LUNARIS_NO_UPDATE_CHECK";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Verifying,
    Installing,
    /// Installed: the window quits and the program starts again.
    Restart,
    Error,
}

impl Phase {
    fn id(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::UpToDate => "upToDate",
            Phase::Available => "available",
            Phase::Downloading => "downloading",
            Phase::Verifying => "verifying",
            Phase::Installing => "installing",
            Phase::Restart => "restart",
            Phase::Error => "error",
        }
    }

    fn busy(self) -> bool {
        matches!(self, Phase::Checking | Phase::Downloading | Phase::Verifying | Phase::Installing | Phase::Restart)
    }
}

struct Config {
    app: App,
    settings_file: PathBuf,
    result_file: PathBuf,
    kind: InstallKind,
    exe_name: String,
}

struct Inner {
    phase: Phase,
    release: Option<Release>,
    plan: Option<Plan>,
    progress: f64,
    error: String,
    /// "check" | "cancelled" | "failed" | "verify" | ""
    error_kind: &'static str,
    settings: Settings,
}

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    #[serde(default = "yes")]
    auto_check: bool,
    #[serde(default)]
    last_check: u64,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_check: true, last_check: 0 }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static INNER: Mutex<Inner> = Mutex::new(Inner {
    phase: Phase::Idle,
    release: None,
    plan: None,
    progress: 0.0,
    error: String::new(),
    error_kind: "",
    settings: Settings { auto_check: true, last_check: 0 },
});

fn inner() -> std::sync::MutexGuard<'static, Inner> {
    INNER.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Starts the updater of `app`. `settings_dir` holds `update_settings.json`
/// ("Check for updates automatically") and, on Windows, the installer's
/// outcome for the next start. Starts the automatic check.
pub fn init(app: App, settings_dir: &Path) {
    let exe = crate::restart::launch_exe();
    let kind = kind::classify(&kind::probe(app, &exe));
    let exe_name = exe.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
    let config = Config {
        app,
        settings_file: settings_dir.join("update_settings.json"),
        result_file: settings_dir.join("update_result.txt"),
        kind,
        exe_name,
    };
    let settings: Settings = std::fs::read(&config.settings_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    // The Windows installer helper's outcome of the last update.
    if let Ok(text) = std::fs::read_to_string(&config.result_file) {
        let _ = std::fs::remove_file(&config.result_file);
        if let Err(e) = install::parse_windows_result(&text) {
            let mut g = inner();
            g.phase = Phase::Error;
            g.error_kind = if e == InstallError::Cancelled { "cancelled" } else { "failed" };
            g.error = e.to_string();
        }
    }
    inner().settings = settings;
    if CONFIG.set(config).is_err() {
        return; // already started
    }
    std::thread::Builder::new()
        .name("updater".into())
        .spawn(|| {
            install::remove_old_dirs(cfg().app);
            std::thread::sleep(FIRST_CHECK_DELAY);
            let mut last_auto: Option<std::time::Instant> = None;
            loop {
                let due = last_auto.is_none_or(|t| t.elapsed() >= CHECK_INTERVAL);
                if due && auto_check() && std::env::var_os(NO_CHECK_ENV).is_none() {
                    last_auto = Some(std::time::Instant::now());
                    check(true);
                }
                std::thread::sleep(Duration::from_secs(600));
            }
        })
        .ok();
}

fn cfg() -> &'static Config {
    CONFIG.get().expect("updater::service::init was not called")
}

fn save_settings(s: &Settings) {
    let Some(c) = CONFIG.get() else { return };
    if let Some(dir) = c.settings_file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(&c.settings_file, json);
    }
}

pub fn auto_check() -> bool {
    inner().settings.auto_check
}

pub fn set_auto_check(on: bool) {
    let s = {
        let mut g = inner();
        g.settings.auto_check = on;
        g.settings.clone()
    };
    save_settings(&s);
}

/// "Check now": in the background; nothing when busy.
pub fn check_now() {
    if CONFIG.get().is_none() || inner().phase.busy() {
        return;
    }
    std::thread::Builder::new().name("updater-check".into()).spawn(|| check(false)).ok();
}

fn check(automatic: bool) {
    {
        let mut g = inner();
        if g.phase.busy() {
            return;
        }
        g.phase = Phase::Checking;
    }
    let result = crate::net::fetch_latest();
    let mut g = inner();
    g.settings.last_check = now_secs();
    let settings = g.settings.clone();
    match result {
        Ok(release) if crate::version::is_newer(crate::CURRENT_VERSION, &release.tag) => {
            let c = cfg();
            let names = release.asset_names();
            let companions: Vec<App> = match c.kind {
                InstallKind::Package(m) if kind::package_installed(m, c.app.other().package_name()) => vec![c.app.other()],
                _ => vec![],
            };
            let plan = kind::plan(
                c.app,
                &c.kind,
                &c.exe_name,
                &release.version,
                std::env::consts::ARCH,
                &names,
                verify::signing_configured(RELEASE_SIGNING_PUBLIC_KEY_B64),
                &companions,
            );
            g.plan = Some(plan);
            g.release = Some(release);
            g.phase = Phase::Available;
            g.error.clear();
            g.error_kind = "";
        }
        Ok(_) => {
            g.release = None;
            g.plan = None;
            g.phase = Phase::UpToDate;
            g.error.clear();
            g.error_kind = "";
        }
        Err(e) => {
            // A failed automatic check keeps what was known and says nothing loud.
            if g.release.is_some() {
                g.phase = Phase::Available;
            } else {
                g.phase = if automatic { Phase::Idle } else { Phase::Error };
            }
            g.error = format!("Could not check for updates: {e}");
            g.error_kind = "check";
        }
    }
    drop(g);
    save_settings(&settings);
}

/// The Update button: download, verify, install, in the background.
pub fn install() {
    let (release, assets) = {
        let mut g = inner();
        if g.phase.busy() {
            return;
        }
        let (Some(release), Some(Plan::Install { assets })) = (g.release.clone(), g.plan.clone()) else {
            return;
        };
        g.phase = Phase::Downloading;
        g.progress = 0.0;
        g.error.clear();
        g.error_kind = "";
        (release, assets)
    };
    std::thread::Builder::new()
        .name("updater-install".into())
        .spawn(move || {
            let outcome = run_install(&release, &assets);
            let mut g = inner();
            match outcome {
                Ok(()) => g.phase = Phase::Restart,
                Err((kind, message)) => {
                    g.phase = Phase::Error;
                    g.error_kind = kind;
                    g.error = message;
                }
            }
        })
        .ok();
}

fn set_phase(p: Phase) {
    inner().phase = p;
}

fn run_install(release: &Release, assets: &[String]) -> Result<(), (&'static str, String)> {
    let c = cfg();
    let failed = |m: String| ("failed", m);
    let dir = install::private_dir(c.app).map_err(|e| failed(format!("cannot create a download folder: {e}")))?;

    // 1. The checksum list and its signature, checked before anything else.
    let url = |name: &str| {
        release
            .asset(name)
            .map(|a| a.url.clone())
            .ok_or_else(|| ("verify", verify::VerifyError::MissingEntry(name.to_string()).to_string()))
    };
    let sums = crate::net::fetch_small(&url(SUMS_NAME)?, 1 << 20).map_err(|e| failed(format!("Download failed: {e}")))?;
    let sig = crate::net::fetch_small(&url(SIG_NAME)?, 4096).map_err(|e| failed(format!("Download failed: {e}")))?;
    verify::verify_sums_signature(RELEASE_SIGNING_PUBLIC_KEY_B64, &sums, &sig).map_err(|e| ("verify", e.to_string()))?;
    // The signature covers the list's version line: an old signed list
    // re-published under a newer tag cannot downgrade this copy.
    verify::check_sums_version(&sums, &release.version).map_err(|e| ("verify", e.to_string()))?;

    // 2. The files, each checked against its line.
    let total: u64 = assets.iter().filter_map(|n| release.asset(n)).map(|a| a.size).sum::<u64>().max(1);
    let mut done: u64 = 0;
    let mut files = Vec::new();
    for name in assets {
        let asset = release
            .asset(name)
            .ok_or_else(|| failed(format!("the release has no {name}")))?;
        // Unknown names never get downloaded (the list is from `plan`).
        verify::expected_digest(&sums, name).map_err(|e| ("verify", e.to_string()))?;
        let path = dir.join(name);
        let base = done;
        let digest = crate::net::download_to(&asset.url, &path, asset.size, &|n| {
            inner().progress = ((base + n) as f64 / total as f64).min(1.0);
        })
        .map_err(|e| failed(format!("Download failed: {e}")))?;
        done += asset.size;
        set_phase(Phase::Verifying);
        if let Err(e) = verify::check_digest(&sums, name, &digest) {
            let _ = std::fs::remove_file(&path);
            return Err(("verify", e.to_string()));
        }
        set_phase(Phase::Downloading);
        files.push((path, digest));
    }

    // 3. Install.
    set_phase(Phase::Installing);
    let map = |e: InstallError| match e {
        InstallError::Cancelled => ("cancelled", e.to_string()),
        InstallError::Failed(_) => ("failed", e.to_string()),
    };
    let result = match &c.kind {
        InstallKind::Package(m) => install::install_packages(*m, &files).map_err(map),
        InstallKind::AppImage { path } => install::replace_appimage(&files[0].0, path).map_err(map),
        InstallKind::WindowsInstaller { dir: install_dir } => {
            windows_handover(&files[0].0, &files[0].1, install_dir, &c.result_file).map_err(map)
        }
        _ => Err(failed("this copy cannot be updated in place".into())),
    };
    // The Windows helper still needs the installer; the next start removes the folder.
    if !matches!(c.kind, InstallKind::WindowsInstaller { .. }) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

#[cfg(windows)]
fn windows_handover(installer: &Path, sha256: &[u8; 32], install_dir: &Path, result_file: &Path) -> Result<(), InstallError> {
    let args: Vec<String> = crate::restart::relaunch_args().iter().map(|a| a.to_string_lossy().into_owned()).collect();
    install::launch_windows_installer(installer, sha256, install_dir, &crate::restart::launch_exe(), &args, result_file)
}

#[cfg(not(windows))]
fn windows_handover(_: &Path, _: &[u8; 32], _: &Path, _: &Path) -> Result<(), InstallError> {
    Err(InstallError::Failed("the Windows installer runs on Windows only".into()))
}

/// After the install: true when the window should quit now. On Linux the
/// program then starts again (`restart::relaunch_if_requested`); on
/// Windows the installer helper starts it.
pub fn restart_now() -> bool {
    if inner().phase != Phase::Restart {
        return false;
    }
    if !matches!(CONFIG.get().map(|c| &c.kind), Some(InstallKind::WindowsInstaller { .. })) {
        crate::restart::request_restart();
    }
    true
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status<'a> {
    state: &'a str,
    current: &'a str,
    latest: &'a str,
    notes: &'a str,
    release_url: &'a str,
    can_install: bool,
    manual_reason: &'a str,
    manual_message: &'a str,
    progress: f64,
    error: &'a str,
    error_kind: &'a str,
    auto_check: bool,
    last_checked: u64,
    install_kind: &'a str,
}

/// The state for the window, as JSON:
/// `{state, current, latest, notes, releaseUrl, canInstall, manualReason,
///   manualMessage, progress (0..1), error, errorKind, autoCheck,
///   lastChecked (unix s), installKind}`; `state` is one of idle, checking,
/// upToDate, available, downloading, verifying, installing, restart, error.
pub fn status_json() -> String {
    let g = inner();
    let (manual_reason, manual_message) = match &g.plan {
        Some(Plan::Manual(r)) => (r.id(), r.message()),
        _ => ("", ""),
    };
    let release = g.release.as_ref();
    let status = Status {
        state: g.phase.id(),
        current: crate::CURRENT_VERSION,
        latest: release.map(|r| r.version.as_str()).unwrap_or(""),
        notes: release.map(|r| r.notes.as_str()).unwrap_or(""),
        release_url: release.map(|r| r.page_url.as_str()).unwrap_or(crate::RELEASES_PAGE),
        can_install: matches!(g.plan, Some(Plan::Install { .. })),
        manual_reason,
        manual_message,
        progress: g.progress,
        error: &g.error,
        error_kind: g.error_kind,
        auto_check: g.settings.auto_check,
        last_checked: g.settings.last_check,
        install_kind: CONFIG.get().map(|c| c.kind.id()).unwrap_or(""),
    };
    serde_json::to_string(&status).unwrap_or_default()
}
