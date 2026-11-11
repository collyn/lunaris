#![windows_subsystem = "windows"]

//! The Lunaris app. With no mode it opens the desktop app; the same binary is
//! also the host agent and the server:
//!
//!   lunaris                     desktop app (also handles lunaris:// links)
//!   lunaris agent [options]     host agent, headless (see `lunaris agent --help`)
//!   lunaris server [command]    signaling/web server (see `lunaris server help`)

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
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    match args.get(1).and_then(|a| a.to_str()) {
        Some("agent") => {
            attach_console();
            // clap sees "lunaris agent" as the program name.
            agent::run(std::iter::once(std::ffi::OsString::from("lunaris agent")).chain(args.into_iter().skip(2)))
        }
        Some("server") => {
            attach_console();
            server::run(args.iter().skip(2).map(|a| a.to_string_lossy().into_owned()).collect())
        }
        _ => {
            client_qml::run();
            std::process::ExitCode::SUCCESS
        }
    }
}

/// The app is a GUI program on Windows; give the command-line modes the
/// console they were started from so their output shows.
fn attach_console() {
    #[cfg(windows)]
    {
        extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
