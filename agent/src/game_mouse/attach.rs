//! Plugging the game mouse in with usbip-win2's command-line client.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use super::cli::{install_location, port_of};

/// No console window for the child (the agent may run in a user's session).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// usbip-win2's uninstall key (its Inno Setup AppId), which holds where it is.
const UNINSTALL_KEY: &str =
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{199505b0-b93d-4521-a8c7-897818e0205a}_is1";

/// usbip-win2's `usbip.exe`, when it is installed.
pub fn usbip_exe() -> Option<PathBuf> {
    let from_registry = Command::new("reg")
        .args(["query", UNINSTALL_KEY, "/v", "InstallLocation"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| install_location(&String::from_utf8_lossy(&o.stdout)));
    let default = std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("USBip"));
    from_registry
        .into_iter()
        .chain(default)
        .map(|dir| dir.join("usbip.exe"))
        .find(|exe| exe.is_file())
}

/// Attaches the device `bus_id` of the USB/IP server on 127.0.0.1:`port`;
/// returns the hub port it was plugged into.
pub fn plug_in(exe: &PathBuf, port: u16, bus_id: &str) -> Result<String, String> {
    let output = Command::new(exe)
        .args(["--tcp-port", &port.to_string(), "attach", "--remote", "127.0.0.1", "--bus-id", bus_id])
        // No retries in the background once the agent is gone; the mouse's
        // reports as they come (it sends a few bytes at a time).
        .args(["--once", "--receive-mode", "low-latency", "--terse"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("usbip attach: {} {}", stdout, stderr).trim().to_string())
    }
}

/// The hub port the device `bus_id` of 127.0.0.1:`port` is plugged into,
/// when it is (usbip-win2 may have plugged it back in by itself).
pub fn plugged_port(exe: &PathBuf, port: u16, bus_id: &str) -> Option<String> {
    let output = Command::new(exe).arg("port").creation_flags(CREATE_NO_WINDOW).output().ok()?;
    port_of(&String::from_utf8_lossy(&output.stdout), port, bus_id)
}

/// Unplugs what is plugged into `hub_port` (from `plug_in`). usbip-win2
/// plugs a device back in by itself when its server is still there
/// (measured: about a minute after a detach), so its attempts for the
/// device are stopped first.
pub fn unplug(exe: &PathBuf, port: u16, bus_id: &str, hub_port: &str) -> Result<(), String> {
    let _ = Command::new(exe)
        .args(["--tcp-port", &port.to_string(), "attach", "--remote", "127.0.0.1", "--bus-id", bus_id, "--stop"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let output = Command::new(exe)
        .args(["detach", "--port", hub_port.trim()])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        let all = format!("{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        Err(format!("usbip detach: {}", all.trim()))
    }
}
