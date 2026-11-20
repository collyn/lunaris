//! Restarting after an update.
//!
//! * The app (and the agent, for its restart): [`remember_launch`] first thing in
//!   `main` keeps the program path, arguments and environment as they were
//!   started. After the update the window quits; when Qt's event loop has
//!   returned and the children are stopped, [`relaunch_if_requested`]
//!   starts the new program (Linux) with `LUNARIS_UPDATE_WAIT_PID`, and the
//!   new one waits in [`wait_for_previous`] until this process is gone
//!   (the app's single-instance port must be free). On Windows the helper
//!   that runs the installer starts the program again (see `install`).
//! * The headless agent (`--cli`, e.g. started at login): it does not get
//!   restarted by a package. [`watch_executable`] notices that its program
//!   file was replaced and re-executes it (same pid, arguments and
//!   environment); a replaced file is also what the capture service checks
//!   (`/proc/<pid>/exe`), so the old process could not use it anyway.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Set in the restarted program: the pid of the one that started it.
pub const WAIT_PID_ENV: &str = "LUNARIS_UPDATE_WAIT_PID";

/// Variables of the AppImage runtime, set again by the new AppImage.
const APPIMAGE_VARS: &[&str] = &["APPDIR", "APPIMAGE", "ARGV0", "OWD"];

#[derive(Clone, Debug)]
pub struct Launch {
    pub exe: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: Option<PathBuf>,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();
static RESTART_REQUESTED: AtomicBool = AtomicBool::new(false);
static RELAUNCH_ARGS_FILTER: OnceLock<fn(&[OsString]) -> Vec<OsString>> = OnceLock::new();
static EXTRA_ENV: std::sync::Mutex<Vec<(OsString, OsString)>> = std::sync::Mutex::new(Vec::new());

static CONSUMED_ENV: std::sync::Mutex<Vec<OsString>> = std::sync::Mutex::new(Vec::new());

/// Reads and removes a one-time variable (one [`set_relaunch_env`] set);
/// a later restart does not pass it on.
pub fn consume_env(key: &str) -> Option<OsString> {
    let value = std::env::var_os(key);
    std::env::remove_var(key);
    CONSUMED_ENV.lock().unwrap_or_else(|e| e.into_inner()).push(key.into());
    value
}

/// A variable for the restarted program only (e.g. "sharing was on").
/// Linux; the Windows installer helper starts the program without it.
pub fn set_relaunch_env(key: &str, value: &str) {
    EXTRA_ENV.lock().unwrap_or_else(|e| e.into_inner()).push((key.into(), value.into()));
}

/// Keeps how this process was started. Call it first in `main`, before
/// anything changes the environment.
pub fn remember_launch() {
    let _ = LAUNCH.set(Launch {
        exe: current_exe_path(),
        args: std::env::args_os().skip(1).collect(),
        env: std::env::vars_os().filter(|(k, _)| k != WAIT_PID_ENV).collect(),
        cwd: std::env::current_dir().ok(),
    });
}

fn launch() -> Launch {
    LAUNCH.get().cloned().unwrap_or_else(|| Launch {
        exe: current_exe_path(),
        args: std::env::args_os().skip(1).collect(),
        env: std::env::vars_os().filter(|(k, _)| k != WAIT_PID_ENV).collect(),
        cwd: std::env::current_dir().ok(),
    })
}

/// The program file as started (Linux: without the " (deleted)" that
/// /proc/self/exe shows once a package replaced it).
pub fn launch_exe() -> PathBuf {
    launch().exe
}

fn current_exe_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    strip_deleted(&exe)
}

/// `/usr/bin/x (deleted)` → `/usr/bin/x`.
pub fn strip_deleted(p: &Path) -> PathBuf {
    match p.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(s) => PathBuf::from(s),
        None => p.to_path_buf(),
    }
}

/// Which arguments the restarted program gets (default: the same).
pub fn set_relaunch_args_filter(f: fn(&[OsString]) -> Vec<OsString>) {
    let _ = RELAUNCH_ARGS_FILTER.set(f);
}

/// Asks for a restart once the window has quit (see [`relaunch_if_requested`]).
pub fn request_restart() {
    RESTART_REQUESTED.store(true, Ordering::SeqCst);
}

pub fn restart_requested() -> bool {
    RESTART_REQUESTED.load(Ordering::SeqCst)
}

/// The environment for the new program (pure): the AppImage runtime's
/// variables removed, and entries of path lists that point into the old
/// AppImage mount (`LD_LIBRARY_PATH` from its AppRun, …) dropped.
pub fn filtered_env(env: &[(OsString, OsString)], appdir: Option<&OsStr>) -> Vec<(OsString, OsString)> {
    let appdir = appdir.and_then(OsStr::to_str).filter(|d| !d.is_empty());
    env.iter()
        .filter(|(k, _)| {
            k != WAIT_PID_ENV && !(appdir.is_some() && APPIMAGE_VARS.iter().any(|v| k == v))
        })
        .filter_map(|(k, v)| {
            let Some(dir) = appdir else { return Some((k.clone(), v.clone())) };
            match v.to_str() {
                Some(s) if s.contains(dir) => {
                    let kept: Vec<&str> = s.split(':').filter(|e| !e.starts_with(dir)).collect();
                    if kept.iter().all(|e| e.is_empty()) {
                        None
                    } else {
                        Some((k.clone(), OsString::from(kept.join(":"))))
                    }
                }
                _ => Some((k.clone(), v.clone())),
            }
        })
        .collect()
}

/// The program to start again: the AppImage file for an AppImage (the
/// running program is inside its old mount), else the program as started.
pub fn relaunch_target() -> PathBuf {
    let l = launch();
    let get = |k: &str| l.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    if let (Some(image), Some(dir)) = (get("APPIMAGE"), get("APPDIR")) {
        if !image.is_empty() && l.exe.starts_with(&dir) {
            return PathBuf::from(image);
        }
    }
    l.exe
}

/// The arguments of the restarted program (see [`set_relaunch_args_filter`]).
pub fn relaunch_args() -> Vec<OsString> {
    let l = launch();
    match RELAUNCH_ARGS_FILTER.get() {
        Some(f) => f(&l.args),
        None => l.args,
    }
}

// Used on Unix only (Windows: the installer helper starts the program).
#[cfg_attr(not(unix), allow(dead_code))]
fn relaunch_command() -> std::process::Command {
    let l = launch();
    let appdir = l.env.iter().find(|(k, _)| k == "APPDIR").map(|(_, v)| v.clone());
    let target = relaunch_target();
    let args = relaunch_args();
    let mut cmd = std::process::Command::new(&target);
    cmd.args(args);
    cmd.env_clear();
    let consumed = CONSUMED_ENV.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let env: Vec<_> = l.env.iter().filter(|(k, _)| !consumed.contains(k)).cloned().collect();
    cmd.envs(filtered_env(&env, appdir.as_deref()));
    cmd.envs(EXTRA_ENV.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
    // The AppImage runtime remembers the original folder in $OWD.
    let owd = l.env.iter().find(|(k, _)| k == "OWD").map(|(_, v)| PathBuf::from(v));
    if let Some(dir) = owd.or(l.cwd).filter(|d| d.is_dir()) {
        cmd.current_dir(dir);
    }
    cmd
}

/// Starts the new program when [`request_restart`] was called. Linux only:
/// on Windows the installer helper does it. Call after the window quit and
/// the children were stopped, right before the process exits.
pub fn relaunch_if_requested() {
    if !restart_requested() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = relaunch_command();
        cmd.env(WAIT_PID_ENV, std::process::id().to_string());
        // Its own process group: not stopped with ours by a terminal.
        cmd.process_group(0);
        cmd.stdin(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(child) => eprintln!("updater: restarted as pid {}", child.id()),
            Err(e) => eprintln!("updater: cannot start {}: {e}", relaunch_target().display()),
        }
    }
}

/// In a restarted program: waits (up to 30 s) until the program that
/// started it has exited. Call first in `main`, after [`remember_launch`].
pub fn wait_for_previous() {
    let Some(pid) = std::env::var(WAIT_PID_ENV).ok().and_then(|p| p.parse::<u32>().ok()) else {
        return;
    };
    std::env::remove_var(WAIT_PID_ENV);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while process_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    // A zombie (exited, not yet reaped by its parent) counts as gone.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat.rsplit_once(')').map(|(_, rest)| !rest.trim_start().starts_with('Z')).unwrap_or(true),
        Err(_) => false,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

/// Identity of a program file: replaced when any of these changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
    pub len: u64,
    pub mtime: i64,
}

#[cfg(unix)]
pub fn file_id(p: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p).ok()?;
    Some(FileId { dev: m.dev(), ino: m.ino(), len: m.len(), mtime: m.mtime() })
}

/// What the executable watcher does after one look (pure): `start` is the
/// file at startup, `last` the previous look, `now` this one.
#[derive(Debug, PartialEq, Eq)]
pub enum WatchStep {
    Keep,
    /// Replaced, but still changing (a copy in progress): look again.
    Settling,
    /// Replaced and unchanged since the last look: re-execute.
    Reexec,
}

pub fn watch_step(start: &FileId, last: Option<&FileId>, now: Option<&FileId>) -> WatchStep {
    match now {
        None => WatchStep::Keep, // gone for a moment (being replaced): wait
        Some(now) if now == start => WatchStep::Keep,
        Some(now) if Some(now) == last => WatchStep::Reexec,
        Some(_) => WatchStep::Settling,
    }
}

/// Linux, headless agent: re-executes this program when its file is
/// replaced (a package upgrade, a new AppImage). Looks every `interval`.
#[cfg(target_os = "linux")]
pub fn watch_executable(interval: std::time::Duration) {
    let target = relaunch_target();
    let Some(start) = file_id(&target) else { return };
    std::thread::Builder::new()
        .name("exe-watch".into())
        .spawn(move || {
            use std::os::unix::process::CommandExt;
            let mut last: Option<FileId> = None;
            loop {
                std::thread::sleep(interval);
                let now = file_id(&target);
                match watch_step(&start, last.as_ref(), now.as_ref()) {
                    WatchStep::Reexec => {
                        eprintln!("updater: {} was replaced; starting the new version", target.display());
                        let err = relaunch_command().exec();
                        eprintln!("updater: cannot start the new version: {err}");
                        return;
                    }
                    WatchStep::Settling | WatchStep::Keep => last = now,
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn env_without_the_old_appimage() {
        let env = vec![
            (os("HOME"), os("/home/u")),
            (os("APPDIR"), os("/tmp/.mount_abc")),
            (os("APPIMAGE"), os("/home/u/L.AppImage")),
            (os("OWD"), os("/home/u")),
            (os("LD_LIBRARY_PATH"), os("/tmp/.mount_abc/usr/lib/libva-fallback:/opt/lib")),
            (os("QT_PLUGIN_PATH"), os("/tmp/.mount_abc/usr/plugins")),
            (os(WAIT_PID_ENV), os("12")),
        ];
        let out = filtered_env(&env, Some(OsStr::new("/tmp/.mount_abc")));
        assert_eq!(out, vec![(os("HOME"), os("/home/u")), (os("LD_LIBRARY_PATH"), os("/opt/lib"))]);
        // Not an AppImage: everything but the wait variable stays.
        let out = filtered_env(&env, None);
        assert_eq!(out.len(), env.len() - 1);
    }

    #[test]
    fn deleted_suffix() {
        assert_eq!(strip_deleted(Path::new("/usr/bin/lunaris-agent (deleted)")), PathBuf::from("/usr/bin/lunaris-agent"));
        assert_eq!(strip_deleted(Path::new("/usr/bin/lunaris-agent")), PathBuf::from("/usr/bin/lunaris-agent"));
    }

    #[test]
    fn watcher_waits_for_a_stable_new_file() {
        let a = FileId { dev: 1, ino: 10, len: 100, mtime: 5 };
        let b = FileId { dev: 1, ino: 11, len: 50, mtime: 9 };
        let b2 = FileId { len: 120, ..b.clone() };
        assert_eq!(watch_step(&a, None, Some(&a)), WatchStep::Keep);
        assert_eq!(watch_step(&a, Some(&a), None), WatchStep::Keep);
        assert_eq!(watch_step(&a, Some(&a), Some(&b)), WatchStep::Settling);
        assert_eq!(watch_step(&a, Some(&b), Some(&b2)), WatchStep::Settling);
        assert_eq!(watch_step(&a, Some(&b2), Some(&b2)), WatchStep::Reexec);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_is_alive() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(u32::MAX - 1));
    }
}
