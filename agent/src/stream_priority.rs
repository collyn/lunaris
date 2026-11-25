//! A Windows host streams at a higher CPU and GPU priority.
//!
//! A game that loads the CPU and the GPU left the agent's capture, encoder
//! and RTP writer waiting at normal priority: under heavy effects frames
//! piled up ("RTP frame queue full", "Discarding N queued video frames"),
//! the audio underran and the picture stuttered (2026-11-25, Windows host
//! with an RX 6600 XT). Sunshine does the same: its process runs at
//! `HIGH_PRIORITY_CLASS` and asks the GPU scheduler for a higher class
//! (display_base.cpp, `D3DKMTSetProcessSchedulingPriorityClass`). HIGH, not
//! REALTIME, for the GPU: REALTIME can starve the desktop compositor with
//! hardware-accelerated GPU scheduling.
//!
//! Raised once, when the first stream starts, and kept for the process's
//! life (a later stream wants it again; an idle agent uses next to nothing).

/// Raises the priorities, once per process.
pub fn raise_for_streaming() {
    #[cfg(target_os = "windows")]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(windows_impl::raise);
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use windows::core::{s, w};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass, HIGH_PRIORITY_CLASS};

    /// D3DKMT_SCHEDULINGPRIORITYCLASS_HIGH (d3dkmthk.h).
    const GPU_PRIORITY_HIGH: i32 = 4;

    type SetGpuPriority = unsafe extern "system" fn(HANDLE, i32) -> i32;

    pub fn raise() {
        let process = unsafe { GetCurrentProcess() };
        match unsafe { SetPriorityClass(process, HIGH_PRIORITY_CLASS) } {
            Ok(()) => tracing::info!("Streaming: process priority set to high"),
            Err(e) => tracing::warn!("Streaming: could not raise the process priority: {e}"),
        }
        // In gdi32.dll since Windows Vista; looked up so that a system
        // without it only loses this.
        let Ok(gdi32) = (unsafe { LoadLibraryW(w!("gdi32.dll")) }) else {
            tracing::warn!("Streaming: gdi32.dll not loaded; GPU priority unchanged");
            return;
        };
        let Some(proc) = (unsafe { GetProcAddress(gdi32, s!("D3DKMTSetProcessSchedulingPriorityClass")) }) else {
            tracing::warn!("Streaming: no D3DKMTSetProcessSchedulingPriorityClass; GPU priority unchanged");
            return;
        };
        let set: SetGpuPriority = unsafe { std::mem::transmute(proc) };
        let status = unsafe { set(process, GPU_PRIORITY_HIGH) };
        if status == 0 {
            tracing::info!("Streaming: GPU scheduling priority set to high");
        } else {
            tracing::warn!("Streaming: could not raise the GPU priority (NTSTATUS {status:#x})");
        }
    }
}
