//! `lunaris-agent`: the Lunaris agent, a command-line program and daemon
//! without any window. It runs
//!
//! - on Linux as a systemd *user* service in a desktop session
//!   (`lunaris-agent.service`, shares that session's screen), or as the
//!   *system* service `lunaris-agent-system.service` on a headless machine
//!   (a VPS: connectors and direct access, no screen);
//! - on Windows as the `LunarisAgentService` service (win_service.rs).
//!
//! Everything else is managed from the Lunaris server's web page; the
//! optional tray program `lunaris-approval-helper` lets the person at the
//! computer accept connections. `lunaris-agent --help` lists the commands.

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static mut __cpu_model: [u32; 4] = [0; 4];

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
#[no_mangle]
pub extern "C" fn __cpu_indicator_init() -> i32 {
    0
}

fn main() -> std::process::ExitCode {
    // First of all, before any DLL is loaded on demand: Windows looks for
    // DLLs only in System32 and next to this program, not in the current
    // folder or the PATH (agent::harden_dll_search).
    #[cfg(target_os = "windows")]
    agent::harden_dll_search();
    // How this program was started, for the restart after its file is
    // replaced by an update (restart.rs); a restarted one first waits for
    // the old one to exit.
    updater::restart::remember_launch();
    updater::restart::wait_for_previous();
    agent::run(std::env::args_os())
}
