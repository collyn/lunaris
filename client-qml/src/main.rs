#![windows_subsystem = "windows"]

//! The Lunaris app: the low-latency window of a remote computer's stream,
//! opened from the web page of a Lunaris server (a `lunaris://connect` link).
//! Hosting this computer is the Lunaris agent's (lunaris-agent), the server
//! is its own program (lunaris-server).
//!
//!   lunaris-client                     says to open it from the web page
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
    client_qml::run();
    std::process::ExitCode::SUCCESS
}
