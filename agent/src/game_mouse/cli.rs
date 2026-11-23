//! Reading the output of usbip-win2's command line and of `reg query`
//! (used on Windows by attach.rs; tested everywhere).

use std::path::PathBuf;

/// The folder in `reg query ... /v InstallLocation` output.
pub fn install_location(output: &str) -> Option<PathBuf> {
    output.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("InstallLocation")?;
        let value = rest.trim_start().strip_prefix("REG_SZ")?.trim();
        (!value.is_empty()).then(|| PathBuf::from(value))
    })
}

/// In `usbip port` output, the "Port NN:" of the device at
/// usbip://127.0.0.1:`port`/`bus_id`.
pub fn port_of(output: &str, port: u16, bus_id: &str) -> Option<String> {
    let target = format!("usbip://127.0.0.1:{port}/{bus_id}");
    let mut current = None;
    for line in output.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Port ") {
            current = rest.split(':').next().map(|n| n.trim().trim_start_matches('0').to_string());
        } else if line.trim_start_matches("-> ").trim() == target {
            return current.filter(|n| !n.is_empty());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_hub_port_of_the_game_mouse_is_found_in_usbip_port() {
        let out = "Imported USB devices\r\n====================\r\nPort 01: device in use at Full Speed(12Mbps)\r\n         Generic : pid.codes Test PID (1209:0001)\r\n           -> usbip://192.168.1.5:3240/1-1\r\nPort 02: device in use at Full Speed(12Mbps)\r\n           -> usbip://127.0.0.1:37240/1-1\r\n           -> mode: low-latency\r\n";
        assert_eq!(port_of(out, 37240, "1-1").as_deref(), Some("2"));
        assert_eq!(port_of(out, 37241, "1-1"), None);
        assert_eq!(port_of("Imported USB devices\r\n====================\r\n", 37240, "1-1"), None);
    }

    #[test]
    fn the_install_folder_is_read_from_reg_output() {
        let out = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\...\\{1995}_is1\r\n    InstallLocation    REG_SZ    C:\\Program Files\\USBip\\\r\n\r\n";
        assert_eq!(install_location(out), Some(PathBuf::from("C:\\Program Files\\USBip\\")));
        assert_eq!(install_location("    InstallLocation    REG_SZ    \r\n"), None);
        assert_eq!(install_location("ERROR: not found"), None);
    }
}
