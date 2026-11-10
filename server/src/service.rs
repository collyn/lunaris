#![cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]
//! `lunaris-server install | uninstall | status`: run the server as a system
//! service (systemd on Linux, a Windows service on Windows).

const USAGE_INSTALL: &str = "lunaris-server install [--port <port>]";

/// Parses `--port <n>` (the only install option).
fn parse_port(args: &[String]) -> Result<Option<u16>, String> {
    let mut port = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" | "-p" => {
                let v = it.next().ok_or("--port needs a value")?;
                port = Some(v.parse::<u16>().map_err(|_| format!("invalid port: {v}"))?);
            }
            other => return Err(format!("unknown option: {other}\nusage: {USAGE_INSTALL}")),
        }
    }
    Ok(port)
}

fn default_env_file(port: u16) -> String {
    format!(
        "# Lunaris server settings (KEY=value). Restart the service after editing.\n\
         SERVER_PORT={port}\n\
         # Token agents use to register (generated on first start when unset):\n\
         # LUNARIS_TOKEN=\n\
         # Log level:\n\
         # RUST_LOG=info\n"
    )
}

/// Waits (up to `secs`) for a file the freshly started service writes, then returns it.
fn wait_for_file(path: &std::path::Path, secs: u64) -> Option<String> {
    for _ in 0..secs * 4 {
        if let Ok(s) = std::fs::read_to_string(path) {
            if !s.trim().is_empty() {
                return Some(s.trim().to_string());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    None
}

fn print_summary(data_dir: &std::path::Path, env_file: &str, port: u16, existing_db: bool) {
    println!();
    println!("Lunaris server is running on port {port} (http://<this-machine>:{port}).");
    println!("  Settings: {env_file}");
    println!("  Data:     {}", data_dir.display());
    let pw_file = data_dir.join("initial_admin_password.txt");
    match wait_for_file(&pw_file, if existing_db { 2 } else { 15 }) {
        Some(pw) => {
            println!();
            println!("  Admin login:  admin / {pw}");
            println!("  (saved in {} — change it after signing in)", pw_file.display());
        }
        None if existing_db => {
            println!("  Existing database kept; sign in with your current admin account.")
        }
        None => println!("  The initial admin password will be written to {}.", pw_file.display()),
    }
    if let Some(token) = wait_for_file(&data_dir.join("server_token.txt"), 5) {
        println!("  Agent token:  {token}");
    }
}

// ───────────────────────────────── Linux (systemd) ─────────────────────────────────

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    const SERVICE: &str = "lunaris-server";
    const UNIT_PATH: &str = "/etc/systemd/system/lunaris-server.service";
    const BIN_PATH: &str = "/usr/local/bin/lunaris-server";
    const CONF_DIR: &str = "/etc/lunaris";
    const ENV_FILE: &str = "/etc/lunaris/server.env";
    const DATA_DIR: &str = "/var/lib/lunaris";

    pub fn unit_file() -> String {
        format!(
            "[Unit]\n\
             Description=Lunaris remote desktop server\n\
             Documentation=https://github.com/collyn/lunaris\n\
             After=network-online.target\n\
             Wants=network-online.target\n\
             \n\
             [Service]\n\
             ExecStart={BIN_PATH} run\n\
             EnvironmentFile=-{ENV_FILE}\n\
             Environment=LUNARIS_DATA_DIR={DATA_DIR}\n\
             DynamicUser=yes\n\
             StateDirectory=lunaris\n\
             StateDirectoryMode=0700\n\
             AmbientCapabilities=CAP_NET_BIND_SERVICE\n\
             NoNewPrivileges=yes\n\
             ProtectSystem=strict\n\
             ProtectHome=yes\n\
             PrivateTmp=yes\n\
             Restart=on-failure\n\
             RestartSec=3\n\
             \n\
             [Install]\n\
             WantedBy=multi-user.target\n"
        )
    }

    fn require_root() -> Result<(), String> {
        if unsafe { libc::geteuid() } != 0 {
            return Err("this command needs root: run it with sudo".into());
        }
        if !Path::new("/run/systemd/system").exists() {
            return Err("systemd is not running on this machine; run `lunaris-server` directly \
                        (or use the Docker image) instead"
                .into());
        }
        Ok(())
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let status = Command::new("systemctl")
            .args(args)
            .status()
            .map_err(|e| format!("systemctl: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`systemctl {}` failed", args.join(" ")))
        }
    }

    fn env_port() -> Option<u16> {
        std::fs::read_to_string(ENV_FILE).ok()?.lines().find_map(|l| {
            l.trim().strip_prefix("SERVER_PORT=").and_then(|v| v.trim().parse().ok())
        })
    }

    pub fn install(args: &[String]) -> Result<(), String> {
        let port_arg = parse_port(args)?;
        require_root()?;

        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if exe != Path::new(BIN_PATH) {
            // Copy to a temp name and rename, so upgrading a running binary works.
            let tmp = format!("{BIN_PATH}.new");
            std::fs::copy(&exe, &tmp).map_err(|e| format!("copy to {tmp}: {e}"))?;
            std::fs::rename(&tmp, BIN_PATH).map_err(|e| format!("install {BIN_PATH}: {e}"))?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(BIN_PATH, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }

        std::fs::create_dir_all(CONF_DIR).map_err(|e| format!("{CONF_DIR}: {e}"))?;
        if !Path::new(ENV_FILE).exists() {
            std::fs::write(ENV_FILE, default_env_file(port_arg.unwrap_or(8090)))
                .map_err(|e| format!("{ENV_FILE}: {e}"))?;
        } else if let Some(p) = port_arg {
            let text = std::fs::read_to_string(ENV_FILE).unwrap_or_default();
            let mut lines: Vec<String> = text
                .lines()
                .filter(|l| !l.trim().starts_with("SERVER_PORT="))
                .map(String::from)
                .collect();
            lines.push(format!("SERVER_PORT={p}"));
            std::fs::write(ENV_FILE, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
        }

        std::fs::write(UNIT_PATH, unit_file()).map_err(|e| format!("{UNIT_PATH}: {e}"))?;
        let existing_db = Path::new(DATA_DIR).join("lunaris.db").exists();
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", SERVICE])?;
        systemctl(&["restart", SERVICE])?;

        println!("Installed {BIN_PATH} and the `{SERVICE}` systemd service (starts at boot).");
        print_summary(Path::new(DATA_DIR), ENV_FILE, env_port().unwrap_or(8090), existing_db);
        println!();
        println!("Manage it with: systemctl status|restart|stop {SERVICE}   (logs: journalctl -u {SERVICE})");
        Ok(())
    }

    pub fn uninstall(args: &[String]) -> Result<(), String> {
        let purge = args.iter().any(|a| a == "--purge");
        require_root()?;
        let _ = systemctl(&["disable", "--now", SERVICE]);
        let _ = std::fs::remove_file(UNIT_PATH);
        let _ = systemctl(&["daemon-reload"]);
        let _ = std::fs::remove_file(BIN_PATH);
        if purge {
            // DynamicUser keeps the real directory under /var/lib/private.
            let _ = std::fs::remove_dir_all("/var/lib/private/lunaris");
            let _ = std::fs::remove_file(DATA_DIR);
            let _ = std::fs::remove_dir_all(DATA_DIR);
            let _ = std::fs::remove_dir_all(CONF_DIR);
            println!("Removed the service, {BIN_PATH}, {CONF_DIR} and all data.");
        } else {
            println!("Removed the service and {BIN_PATH}.");
            println!("Kept settings ({CONF_DIR}) and data ({DATA_DIR}); add --purge to delete them.");
        }
        Ok(())
    }

    pub fn status() -> Result<(), String> {
        let _ = Command::new("systemctl").args(["status", "--no-pager", SERVICE]).status();
        Ok(())
    }
}

// ───────────────────────────────── Windows (SCM) ─────────────────────────────────

#[cfg(windows)]
pub mod imp {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;
    use windows_service::{
        define_windows_service,
        service::{
            ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl,
            ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const SERVICE: &str = "LunarisServer";
    const DISPLAY: &str = "Lunaris Server";
    const FIREWALL_RULE: &str = "Lunaris Server";

    pub fn data_dir() -> PathBuf {
        let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
        Path::new(&base).join("Lunaris")
    }

    fn install_dir() -> PathBuf {
        let base = std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into());
        Path::new(&base).join("Lunaris")
    }

    fn scm_err(e: windows_service::Error) -> String {
        format!("{e} (run this from an elevated Administrator prompt)")
    }

    fn env_port(env_file: &Path) -> Option<u16> {
        std::fs::read_to_string(env_file).ok()?.lines().find_map(|l| {
            l.trim().strip_prefix("SERVER_PORT=").and_then(|v| v.trim().parse().ok())
        })
    }

    fn stop_and_wait(service: &windows_service::service::Service) {
        if let Ok(s) = service.query_status() {
            if s.current_state != ServiceState::Stopped {
                let _ = service.stop();
                for _ in 0..40 {
                    std::thread::sleep(Duration::from_millis(250));
                    if matches!(service.query_status(), Ok(s) if s.current_state == ServiceState::Stopped) {
                        break;
                    }
                }
            }
        }
    }

    fn netsh(args: &[&str]) {
        let _ = std::process::Command::new("netsh")
            .args(args)
            .stdout(std::process::Stdio::null())
            .status();
    }

    pub fn install(args: &[String]) -> Result<(), String> {
        let port_arg = parse_port(args)?;
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .map_err(scm_err)?;

        // Stop an existing installation first so its binary can be replaced.
        let existing = manager
            .open_service(SERVICE, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::START)
            .ok();
        if let Some(svc) = &existing {
            stop_and_wait(svc);
        }

        let dir = install_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let bin = dir.join("lunaris-server.exe");
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if exe != bin {
            std::fs::copy(&exe, &bin).map_err(|e| format!("copy to {}: {e}", bin.display()))?;
        }

        let data = data_dir();
        std::fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
        let env_file = data.join("server.env");
        if !env_file.exists() {
            std::fs::write(&env_file, default_env_file(port_arg.unwrap_or(8090)))
                .map_err(|e| e.to_string())?;
        } else if let Some(p) = port_arg {
            let text = std::fs::read_to_string(&env_file).unwrap_or_default();
            let mut lines: Vec<String> = text
                .lines()
                .filter(|l| !l.trim().starts_with("SERVER_PORT="))
                .map(String::from)
                .collect();
            lines.push(format!("SERVER_PORT={p}"));
            std::fs::write(&env_file, lines.join("\r\n") + "\r\n").map_err(|e| e.to_string())?;
        }

        let service = match existing {
            Some(svc) => svc,
            None => {
                let info = ServiceInfo {
                    name: OsString::from(SERVICE),
                    display_name: OsString::from(DISPLAY),
                    service_type: ServiceType::OWN_PROCESS,
                    start_type: ServiceStartType::AutoStart,
                    error_control: ServiceErrorControl::Normal,
                    executable_path: bin.clone(),
                    launch_arguments: vec![OsString::from("service")],
                    dependencies: vec![],
                    account_name: None, // LocalSystem
                    account_password: None,
                };
                let svc = manager
                    .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
                    .map_err(scm_err)?;
                let _ = svc.set_description("Lunaris remote desktop server (web UI, accounts, signaling)");
                svc
            }
        };

        let program = format!("program={}", bin.display());
        netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")]);
        netsh(&[
            "advfirewall", "firewall", "add", "rule", &format!("name={FIREWALL_RULE}"),
            "dir=in", "action=allow", &program, "enable=yes",
        ]);

        let existing_db = data.join("lunaris.db").exists();
        service.start(&[] as &[&OsStr]).map_err(scm_err)?;

        println!("Installed {} and the `{SERVICE}` Windows service (starts at boot).", bin.display());
        print_summary(&data, &env_file.display().to_string(), env_port(&env_file).unwrap_or(8090), existing_db);
        println!();
        println!("Logs: {}", data.join("server.log").display());
        Ok(())
    }

    pub fn uninstall(args: &[String]) -> Result<(), String> {
        let purge = args.iter().any(|a| a == "--purge");
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(scm_err)?;
        if let Ok(svc) = manager.open_service(
            SERVICE,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        ) {
            stop_and_wait(&svc);
            svc.delete().map_err(scm_err)?;
        }
        netsh(&["advfirewall", "firewall", "delete", "rule", &format!("name={FIREWALL_RULE}")]);
        let _ = std::fs::remove_dir_all(install_dir());
        if purge {
            let _ = std::fs::remove_dir_all(data_dir());
            println!("Removed the service, program files and all data.");
        } else {
            println!("Removed the service and program files.");
            println!("Kept settings and data in {}; add --purge to delete them.", data_dir().display());
        }
        Ok(())
    }

    pub fn status() -> Result<(), String> {
        let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .map_err(scm_err)?;
        match manager.open_service(SERVICE, ServiceAccess::QUERY_STATUS) {
            Ok(svc) => {
                let s = svc.query_status().map_err(scm_err)?;
                println!("{SERVICE}: {:?}", s.current_state);
            }
            Err(_) => println!("{SERVICE} is not installed."),
        }
        Ok(())
    }

    // ── Running under the Service Control Manager ──

    define_windows_service!(ffi_service_main, service_main);

    /// Entry point for `lunaris-server service` (what the SCM launches).
    pub fn run_dispatcher() -> Result<(), String> {
        service_dispatcher::start(SERVICE, ffi_service_main).map_err(|e| {
            format!("{e} — `service` is started by Windows; use `lunaris-server install` instead")
        })
    }

    fn service_main(_args: Vec<OsString>) {
        let _ = run_service();
    }

    fn run_service() -> windows_service::Result<()> {
        let data = data_dir();
        let _ = std::fs::create_dir_all(&data);
        if std::env::var_os("LUNARIS_DATA_DIR").is_none() {
            std::env::set_var("LUNARIS_DATA_DIR", &data);
        }
        crate::paths::load_env_file(&data.join("server.env"));
        crate::init_logging(Some(&data.join("server.log")));

        let stop = Arc::new(tokio::sync::Notify::new());
        let stop_handler = stop.clone();
        let status_handle = service_control_handler::register(SERVICE, move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                stop_handler.notify_one();
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;

        let set = |state, accept, code| {
            let _ = status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: state,
                controls_accepted: accept,
                exit_code: ServiceExitCode::Win32(code),
                checkpoint: 0,
                wait_hint: Duration::from_secs(10),
                process_id: None,
            });
        };
        set(
            ServiceState::Running,
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            0,
        );

        let code = match tokio::runtime::Runtime::new() {
            Ok(rt) => match rt.block_on(crate::run_server(async move { stop.notified().await })) {
                Ok(()) => 0,
                Err(e) => {
                    tracing::error!("Server stopped with an error: {e}");
                    1
                }
            },
            Err(e) => {
                tracing::error!("Cannot start the async runtime: {e}");
                1
            }
        };
        set(ServiceState::Stopped, ServiceControlAccept::empty(), code);
        Ok(())
    }
}

// ───────────────────────────────── Other platforms ─────────────────────────────────

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn install(_: &[String]) -> Result<(), String> {
        Err("installing as a service is supported on Linux (systemd) and Windows".into())
    }
    pub fn uninstall(_: &[String]) -> Result<(), String> {
        install(&[])
    }
    pub fn status() -> Result<(), String> {
        install(&[])
    }
}

pub use imp::{install, status, uninstall};
#[cfg(windows)]
pub use imp::run_dispatcher;

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn unit_file_runs_the_installed_binary() {
        let unit = super::imp::unit_file();
        assert!(unit.contains("ExecStart=/usr/local/bin/lunaris-server run"));
        assert!(unit.contains("Environment=LUNARIS_DATA_DIR=/var/lib/lunaris"));
        assert!(unit.contains("StateDirectory=lunaris"));
    }

    #[test]
    fn parses_port_option() {
        assert_eq!(super::parse_port(&["--port".into(), "9000".into()]), Ok(Some(9000)));
        assert_eq!(super::parse_port(&[]), Ok(None));
        assert!(super::parse_port(&["--bogus".into()]).is_err());
    }
}
