//! Earlier versions of the app could start with the computer (to host it or
//! sit in the system tray): an entry in `~/.config/autostart` (Linux), a
//! launch agent (macOS), or a scheduled task and an older `Run` entry
//! (Windows), each starting `<app> --minimized`. Hosting is the Lunaris
//! agent's job now and the app only shows streams, so those entries are
//! taken away, and an app they still start (`--minimized`) quits at once.

/// The scheduled task's name in the Task Scheduler, and the old `Run` value.
#[cfg(target_os = "windows")]
const WINDOWS_ENTRY: &str = "LunarisClient";

/// Started by one of those entries.
pub fn started_by_old_entry() -> bool {
    std::env::args().skip(1).any(|arg| arg == "--minimized")
}

/// Takes the entries away (whatever is not there is left alone).
pub fn remove() {
    #[cfg(target_os = "linux")]
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::Path::new(&home).join(".config/autostart/lunaris-client.desktop");
        let _ = std::fs::remove_file(path);
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        let path = std::path::Path::new(&home).join("Library/LaunchAgents/com.lunaris.client.plist");
        let _ = std::fs::remove_file(path);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let quiet = |program: &str| {
            let mut cmd = std::process::Command::new(program);
            cmd.creation_flags(CREATE_NO_WINDOW);
            cmd
        };
        // The task was made with administrator rights: without them it may
        // stay (the installer removes it too), and the app it starts quits.
        let task_there = quiet("schtasks")
            .args(["/Query", "/TN", WINDOWS_ENTRY])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if task_there {
            let _ = quiet("schtasks").args(["/Delete", "/TN", WINDOWS_ENTRY, "/F"]).output();
        }
        let _ = quiet("reg")
            .args(["delete", "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run", "/v", WINDOWS_ENTRY, "/f"])
            .output();
    }
}
