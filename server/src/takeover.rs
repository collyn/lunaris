//! The service is how the server runs on a Linux computer with systemd
//! (`lunaris-server install`, or the .deb). A server run by hand there
//! (`lunaris-server`, `lunaris-server run`, data in its working folder or
//! `LUNARIS_DATA_DIR`, often `~/.local/share/lunaris-server`) is taken over
//! when the service is installed or upgraded:
//!
//! - its data is found (the usual folders of every user, and the folders of
//!   servers running by hand now);
//! - when the service has no account yet, that server and its gateway are
//!   stopped and its data is imported (import.rs: database, keys, the
//!   gateway's computers; what the service had is kept as a backup);
//! - when the service already has accounts nothing is replaced: the command
//!   to do it is printed.
//!
//! And on such a computer the server is no longer started by hand: without
//! a command it says how to install the service, and `run` refuses while
//! the service runs with the same data or port. Containers and computers
//! without systemd run it in the foreground as before.

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "linux")]
mod linux {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime};

    const SERVICE: &str = "lunaris-server";
    pub const DATA_DIR: &str = "/var/lib/lunaris";
    const ENV_FILE: &str = "/etc/lunaris/server.env";

    /// systemd runs this computer, and this process is not the service's
    /// (by its cgroup: INVOCATION_ID is also set in a desktop session's
    /// terminals, which systemd starts as user units).
    fn systemd_host() -> bool {
        Path::new("/run/systemd/system").is_dir()
            && !std::fs::read_to_string("/proc/self/cgroup").is_ok_and(|c| in_service_cgroup(&c))
    }

    /// Whether `/proc/self/cgroup` says the process runs in the system's
    /// lunaris-server unit.
    fn in_service_cgroup(cgroup: &str) -> bool {
        cgroup.lines().any(|l| l.ends_with(&format!("/system.slice/{SERVICE}.service")))
    }

    /// Whether the service runs: installed here, and a process in its
    /// cgroup (readable by anyone, no bus needed; a container sharing the
    /// host's cgroups has no unit of its own), else what systemctl says
    /// (cgroup v1).
    fn service_active() -> bool {
        if !service_installed() {
            return false;
        }
        match std::fs::read_to_string(format!("/sys/fs/cgroup/system.slice/{SERVICE}.service/cgroup.procs")) {
            Ok(procs) => !procs.trim().is_empty(),
            Err(_) => std::process::Command::new("systemctl")
                .args(["is-active", "--quiet", SERVICE])
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
        }
    }

    /// Its unit: `lunaris-server install`'s or the package's.
    fn service_installed() -> bool {
        [format!("/etc/systemd/system/{SERVICE}.service"), format!("/usr/lib/systemd/system/{SERVICE}.service"), format!("/lib/systemd/system/{SERVICE}.service")]
            .iter()
            .any(|p| Path::new(p).exists())
    }

    /// Left in a folder whose data the service took over: not found again.
    const MOVED_MARK: &str = "MOVED-TO-SERVICE.txt";

    /// The service's port (its settings file), 8090 by default.
    fn service_port() -> u16 {
        std::fs::read_to_string(ENV_FILE)
            .ok()
            .and_then(|text| {
                text.lines()
                    .filter_map(|l| l.trim().strip_prefix("SERVER_PORT="))
                    .last()
                    .and_then(|v| v.trim().parse().ok())
            })
            .unwrap_or(8090)
    }

    fn same_path(a: &Path, b: &Path) -> bool {
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        }
    }

    /// `lunaris-server` without a command: the service on a systemd
    /// computer, the foreground elsewhere.
    pub fn refuse_without_command() -> Result<(), String> {
        if !systemd_host() {
            return Ok(());
        }
        let state = if service_installed() {
            format!("The {SERVICE} service is installed here: systemctl status {SERVICE}")
        } else {
            "Install it as the service (it starts at boot, and takes over the data of a server run by hand here):\n  \
             sudo lunaris-server install"
                .to_string()
        };
        Err(format!(
            "On a computer with systemd the server runs as a service.\n{state}\n\
             `lunaris-server run` runs it in the foreground anyway (development; with its own \
             LUNARIS_DATA_DIR and SERVER_PORT while the service runs)."
        ))
    }

    /// `lunaris-server run` while the service runs: only with another data
    /// folder and another port (two servers on the same data or port break
    /// each other).
    pub fn refuse_beside_service() -> Result<(), String> {
        if !systemd_host() || !service_active() {
            return Ok(());
        }
        let dir = std::env::var_os("LUNARIS_DATA_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
        let port = std::env::var("SERVER_PORT").ok().and_then(|p| p.trim().parse::<u16>().ok());
        let own_dir = dir.is_some_and(|d| !same_path(&d, Path::new(DATA_DIR)));
        let own_port = port.is_some_and(|p| p != service_port());
        if own_dir && own_port {
            return Ok(());
        }
        Err(format!(
            "The {SERVICE} service already runs this server here (systemctl status {SERVICE}). \
             Run another one by hand only with its own LUNARIS_DATA_DIR and SERVER_PORT."
        ))
    }

    /// A process of a server or of its gateway, with the data folder it uses.
    #[derive(Debug, Clone)]
    struct Running {
        pid: i32,
        exe: String,
        gateway: bool,
        data_dir: PathBuf,
    }

    fn args_of(pid: i32) -> Vec<String> {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|raw| raw.split(|b| *b == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
            .unwrap_or_default()
    }

    fn env_of(pid: i32, key: &str) -> Option<String> {
        let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let prefix = format!("{key}=");
        raw.split(|b| *b == 0)
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .find_map(|v| v.strip_prefix(&prefix).map(str::to_owned))
            .filter(|v| !v.is_empty())
    }

    /// What a command line and executable name say a process is: a server
    /// (no command, or `run`) with its data folder, or a gateway (`lunaris-agent
    /// --bridge-only --config <data>/gateway/agent_config.json`).
    fn classify(exe_name: &str, args: &[String], env_dir: Option<String>, cwd: Option<PathBuf>) -> Option<(bool, PathBuf)> {
        let rest = args.get(1..).unwrap_or(&[]);
        if matches!(exe_name, "lunaris-server" | "server") && (rest.is_empty() || (rest.len() == 1 && rest[0] == "run")) {
            let dir = env_dir.map(PathBuf::from).or(cwd)?;
            return Some((false, dir));
        }
        if exe_name == "lunaris-agent" && rest.iter().any(|a| a == "--bridge-only") {
            let config = rest.iter().position(|a| a == "--config").and_then(|i| rest.get(i + 1))?;
            let gateway_dir = Path::new(config).parent()?;
            if gateway_dir.file_name()? != "gateway" {
                return None;
            }
            return Some((true, gateway_dir.parent()?.to_path_buf()));
        }
        None
    }

    fn running() -> Vec<Running> {
        let me = std::process::id() as i32;
        let mut found = Vec::new();
        let Ok(entries) = std::fs::read_dir("/proc") else { return found };
        for entry in entries.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            if pid == me {
                continue;
            }
            let args = args_of(pid);
            let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
                .map(|p| p.to_string_lossy().trim_end_matches(" (deleted)").to_string())
                .unwrap_or_else(|_| args.first().cloned().unwrap_or_default());
            let name = Path::new(&exe).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
            if let Some((gateway, data_dir)) = classify(&name, &args, env_of(pid, "LUNARIS_DATA_DIR"), cwd) {
                found.push(Running { pid, exe, gateway, data_dir });
            }
        }
        found
    }

    /// A data folder of a server run by hand, with accounts.
    #[derive(Debug, Clone)]
    struct Found {
        dir: PathBuf,
        users: i64,
        hosts: i64,
        modified: SystemTime,
    }

    fn modified(dir: &Path) -> SystemTime {
        ["lunaris.db", "lunaris.db-wal"]
            .iter()
            .filter_map(|n| std::fs::metadata(dir.join(n)).and_then(|m| m.modified()).ok())
            .max()
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }

    /// The folders a server run by hand keeps its data in: the usual one of
    /// every user, those of servers running now, and `extra`.
    fn candidate_dirs(extra: &[PathBuf], now_running: &[Running]) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut homes: Vec<PathBuf> = vec![PathBuf::from("/root")];
        if let Ok(entries) = std::fs::read_dir("/home") {
            homes.extend(entries.flatten().map(|e| e.path()));
        }
        dirs.extend(homes.iter().map(|h| h.join(".local/share/lunaris-server")));
        dirs.extend(now_running.iter().filter(|r| !r.gateway).map(|r| r.data_dir.clone()));
        dirs.extend(extra.iter().cloned());
        let mut unique: Vec<PathBuf> = Vec::new();
        for d in dirs {
            if same_path(&d, Path::new(DATA_DIR)) || d.starts_with("/var/lib/private") {
                continue;
            }
            if !unique.iter().any(|u| same_path(u, &d)) {
                unique.push(d);
            }
        }
        unique
    }

    async fn found_in(dirs: &[PathBuf]) -> Vec<Found> {
        let mut found = Vec::new();
        for dir in dirs {
            if dir.join(MOVED_MARK).exists() {
                continue;
            }
            let db = dir.join("lunaris.db");
            if !std::fs::symlink_metadata(&db).map(|m| m.is_file()).unwrap_or(false) {
                continue;
            }
            let (Ok(users), Ok(hosts)) = (crate::import::count(&db, "users").await, crate::import::count(&db, "hosts").await) else {
                continue;
            };
            if users > 0 {
                found.push(Found { dir: dir.clone(), users, hosts, modified: modified(dir) });
            }
        }
        // The most recently used first.
        found.sort_by(|a, b| b.modified.cmp(&a.modified));
        found
    }

    fn alive(pid: i32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    /// Stops processes: SIGTERM, then SIGKILL after a while.
    fn stop(procs: &[Running]) {
        for p in procs {
            println!(
                "Stopping the {} run by hand (pid {}, {}): the service takes its place.",
                if p.gateway { "gateway of the server" } else { "server" },
                p.pid,
                p.exe
            );
            unsafe {
                libc::kill(p.pid, libc::SIGCONT);
                libc::kill(p.pid, libc::SIGTERM);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while procs.iter().any(|p| alive(p.pid)) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        for p in procs.iter().filter(|p| alive(p.pid)) {
            unsafe {
                libc::kill(p.pid, libc::SIGKILL);
            }
        }
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let status = std::process::Command::new("systemctl").args(args).status().map_err(|e| format!("systemctl: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("systemctl {} failed", args.join(" ")))
        }
    }

    /// Takes over the data of a server run by hand into the service's
    /// folder `target` (see the module comment). Run as root, the service's
    /// unit installed. `extra`: further folders to look in (where `install`
    /// was run from). Returns whether something was imported; the service
    /// is left stopped then, for the caller to start.
    pub fn take_over(target: &Path, extra: &[PathBuf]) -> Result<bool, String> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
        let now_running: Vec<Running> =
            running().into_iter().filter(|r| !same_path(&r.data_dir, target) && !r.data_dir.starts_with("/var/lib/private")).collect();
        let found = rt.block_on(found_in(&candidate_dirs(extra, &now_running)));
        let Some(newest) = found.first().cloned() else { return Ok(false) };

        let target_db = target.join("lunaris.db");
        let target_users = if target_db.exists() { rt.block_on(crate::import::count(&target_db, "users"))? } else { 0 };
        if target_users > 0 {
            for f in &found {
                println!(
                    "Found the data of a server run by hand in {} ({} account(s), {} computer(s)). The service \
                     already has accounts, so it was not imported. To use it instead:\n  \
                     sudo systemctl stop {SERVICE} && sudo lunaris-server import {} --replace && sudo systemctl start {SERVICE}",
                    f.dir.display(),
                    f.users,
                    f.hosts,
                    f.dir.display()
                );
            }
            return Ok(false);
        }

        println!(
            "Taking over the server run by hand in {} ({} account(s), {} computer(s)) into the service.",
            newest.dir.display(),
            newest.users,
            newest.hosts
        );
        for other in found.iter().skip(1) {
            println!(
                "Also found {} ({} account(s), last used earlier): not imported; `lunaris-server import` it by hand if needed.",
                other.dir.display(),
                other.users
            );
        }
        // The service's folder exists once the service has started (with
        // DynamicUser systemd makes it, owned right); it must be stopped
        // while its database is replaced.
        if !target.exists() {
            match crate::import::service_user() {
                // The package's account: the folder made for it.
                Some((uid, gid)) => {
                    std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
                    std::os::unix::fs::chown(target, Some(uid), Some(gid)).map_err(|e| format!("{}: {e}", target.display()))?;
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o700));
                }
                None => systemctl(&["start", SERVICE])?,
            }
        }
        if service_active() {
            systemctl(&["stop", SERVICE])?;
        }
        // Stopped first, so its last writes are in the copy.
        let using: Vec<Running> = now_running.iter().filter(|r| same_path(&r.data_dir, &newest.dir)).cloned().collect();
        stop(&using);
        for r in now_running.iter().filter(|r| !r.gateway && !same_path(&r.data_dir, &newest.dir)) {
            println!(
                "A server run by hand still runs (pid {}, data {}); stop it if it uses the service's port.",
                r.pid,
                r.data_dir.display()
            );
        }

        let imported = rt.block_on(crate::import::import_into(&newest.dir, target, false))?;
        crate::import::give_to_service(target, &imported.copied)?;
        println!("Imported {} account(s) and {} computer(s).", imported.users, imported.hosts);
        if imported.gateway_sources > 0 {
            println!("{} computer(s) of the gateway added to its config.", imported.gateway_sources);
        }
        if let Some(backup) = &imported.backup {
            println!("What the service had is kept as {}.", backup.display());
        }
        mark_moved(&newest.dir, target);
        println!(
            "The old folder {} is left as it was (with {MOVED_MARK} in it); don't start that server again \
             (sign in to the service with the same accounts).",
            newest.dir.display()
        );
        Ok(true)
    }

    /// Notes in the old folder that the service has its data now, owned
    /// like the folder (only a file is added; nothing there is changed).
    fn mark_moved(dir: &Path, target: &Path) {
        use std::os::unix::fs::MetadataExt;
        let path = dir.join(MOVED_MARK);
        let text = format!(
            "The data of this Lunaris server was taken over by the lunaris-server service on {}\n\
             (into {}). Its accounts and computers are there now: sign in to the service.\n\
             This folder is kept as it was; delete it once you no longer need it.\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            target.display()
        );
        if std::fs::symlink_metadata(&path).is_ok() || std::fs::write(&path, text).is_err() {
            return;
        }
        if let Ok(m) = std::fs::metadata(dir) {
            let _ = std::os::unix::fs::lchown(&path, Some(m.uid()), Some(m.gid()));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn args(a: &[&str]) -> Vec<String> {
            a.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn the_service_is_told_by_its_cgroup() {
            assert!(in_service_cgroup("0::/system.slice/lunaris-server.service\n"));
            // A terminal of the desktop session (a user unit, with INVOCATION_ID).
            assert!(!in_service_cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-term.service\n"));
            assert!(!in_service_cgroup("0::/system.slice/lunaris-agent.service\n"));
        }

        #[test]
        fn servers_and_gateways_run_by_hand_are_recognised() {
            let cwd = Some(PathBuf::from("/home/u/srv"));
            // No command or `run`: the data folder from LUNARIS_DATA_DIR, else the working folder.
            assert_eq!(classify("lunaris-server", &args(&["lunaris-server"]), None, cwd.clone()), Some((false, PathBuf::from("/home/u/srv"))));
            assert_eq!(
                classify("server", &args(&["/x/server", "run"]), Some("/home/u/.local/share/lunaris-server".into()), cwd.clone()),
                Some((false, PathBuf::from("/home/u/.local/share/lunaris-server")))
            );
            // Other commands are not a running server.
            assert_eq!(classify("lunaris-server", &args(&["lunaris-server", "install"]), None, cwd.clone()), None);
            assert_eq!(classify("lunaris-server", &args(&["lunaris-server", "import", "/x"]), None, cwd.clone()), None);
            assert_eq!(classify("bash", &args(&["bash"]), None, cwd.clone()), None);
            // A gateway: its server's data folder from its config's path.
            assert_eq!(
                classify(
                    "lunaris-agent",
                    &args(&["lunaris-agent", "--cli", "--bridge-only", "--config", "/home/u/.local/share/lunaris-server/gateway/agent_config.json"]),
                    None,
                    cwd.clone()
                ),
                Some((true, PathBuf::from("/home/u/.local/share/lunaris-server")))
            );
            // An agent that is not a gateway.
            assert_eq!(classify("lunaris-agent", &args(&["lunaris-agent", "--cli"]), None, cwd), None);
        }
    }
}
