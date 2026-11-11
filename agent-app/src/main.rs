#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! The standalone Lunaris agent: the agent library plus its desktop window
//! (`--cli` runs it headless). The Lunaris app runs the same agent with
//! `lunaris agent`.

#[cfg(feature = "gui")]
mod agent_gui;

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
    #[cfg(feature = "gui")]
    {
        agent::run_with_gui(
            std::env::args_os(),
            Some(agent::Gui {
                log_writer: agent_gui::ChannelMakeWriter,
                run: agent_gui::run_gui,
            }),
        )
    }
    #[cfg(not(feature = "gui"))]
    {
        agent::run(std::env::args_os())
    }
}
