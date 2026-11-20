#![windows_subsystem = "windows"]

//! The Lunaris app: the low-latency window of a remote computer's stream,
//! opened from the web page of a Lunaris server (a `lunaris://connect` link).
//! Hosting this computer is the Lunaris agent's (lunaris-agent), the server
//! is its own program (lunaris-server).
//!
//!   lunaris-client                     the window that points to the web page
//!   lunaris-client lunaris://connect?… the stream of the link's computer

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
    // How this program was started (the restart after an update); a
    // restarted app first waits for the old one to exit (its
    // single-instance port must be free).
    updater::restart::remember_launch();
    updater::restart::wait_for_previous();
    // The app comes back as the plain window: no link.
    updater::restart::set_relaunch_args_filter(|args| {
        args.iter()
            .filter(|a| *a != "--minimized" && !a.to_string_lossy().starts_with("lunaris://"))
            .cloned()
            .collect()
    });
    client_qml::run();
    std::process::ExitCode::SUCCESS
}
