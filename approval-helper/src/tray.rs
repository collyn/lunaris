//! What the tray (qt_app.cpp, a QSystemTrayIcon with a QMenu) asks of the
//! helper: whether an agent is connected, its web page, and its restart.
//! Plain C functions: the tray is C++ that runs on the Qt thread.

use std::sync::atomic::Ordering;

/// Whether an agent lets this helper answer now (the tray shows itself).
#[no_mangle]
pub extern "C" fn lunaris_helper_connected() -> bool {
    crate::helper::CONNECTED.load(Ordering::SeqCst)
}

/// Writes the agent's web page (UTF-8, no terminating zero) into `buf`
/// when it fits in `len` bytes; returns its length (0: none).
///
/// # Safety
/// `buf` must point at `len` writable bytes (or `len` be 0).
#[no_mangle]
pub unsafe extern "C" fn lunaris_helper_web_url(buf: *mut u8, len: usize) -> usize {
    let Some(url) = crate::pending::web_url() else { return 0 };
    let bytes = url.as_bytes();
    if !buf.is_null() && bytes.len() <= len {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, bytes.len());
    }
    bytes.len()
}

/// The tray's "Restart the agent".
#[no_mangle]
pub extern "C" fn lunaris_helper_restart() {
    tracing::info!("Asking the agent to restart");
    crate::pending::restart_agent();
}
