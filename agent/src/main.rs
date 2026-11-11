#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

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
    agent::run(std::env::args_os())
}
