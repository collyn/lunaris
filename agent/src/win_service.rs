//! Windows service that runs the Lunaris host with the SYSTEM token in the
//! console session.
//!
//! Function-by-function port of Sunshine's `tools/sunshinesvc.cpp`
//! (`/mnt/nvme/projects/_ref/Sunshine/tools/sunshinesvc.cpp`; "sunshinesvc.cpp:N"
//! below always means that file), so that the host can operate system
//! applications (Task Manager, UAC prompt, lock screen). Windows-only.
//!
//! The only adaptations to Lunaris:
//!  1. The child is not `Sunshine.exe` with no arguments but
//!     `"<exe>" [agent] --cli --config "<ProgramData>\Lunaris\agent_config.json"`
//!     (`[agent]` when this program is the unified app, see [`agent_prefix`]),
//!     and its log file is `<ProgramData>\Lunaris\agent-service.log`
//!     (Sunshine: `sunshine.log` in the temp folder, sunshinesvc.cpp:131-146).
//!  2. `--install-service` copies and migrates nothing; it only says how to
//!     import the calling user's settings.
//!  3. Connection approval is unattended (nobody to ask on stdin).
//! Every other difference is listed at the function that has it.
//!
//! Command lines (the same for the standalone agent and, with the leading
//! `agent`, for the unified app):
//!   service binPath: `"<exe>" [agent] --service-run`
//!   child:           `"<exe>" [agent] --cli --config "<ProgramData>\Lunaris\agent_config.json"`
//!   terminate:       `"<exe>" [agent] --terminate <pid>`

use std::ffi::{c_void, OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_CALL_NOT_IMPLEMENTED, ERROR_PROCESS_ABORTED, ERROR_SERVICE_DOES_NOT_EXIST,
    ERROR_SHUTDOWN_IN_PROGRESS,
    HANDLE, NO_ERROR, WAIT_OBJECT_0,
};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId,
    SECURITY_ATTRIBUTES, TOKEN_ALL_ACCESS, TOKEN_DUPLICATE,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, CREATE_ALWAYS,
};
use windows::Win32::System::Console::{
    AttachConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_C_EVENT,
};
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_BREAKAWAY_OK,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows::Win32::System::Memory::{GetProcessHeap, HeapAlloc, HeapFree, HEAP_FLAGS};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::UI::WindowsAndMessaging::WTS_CONSOLE_CONNECT;
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW,
    SERVICE_ACCEPT_PRESHUTDOWN, SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_STOP,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_PRESHUTDOWN, SERVICE_CONTROL_SESSIONCHANGE,
    SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::System::Threading::{
    CreateEventA, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess,
    InitializeProcThreadAttributeList, OpenProcessToken, SetEvent, TerminateProcess,
    UpdateProcThreadAttribute, WaitForMultipleObjects, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
};

/// sunshinesvc.cpp:25 (`SERVICE_NAME = "SunshineService"`).
const SERVICE_NAME: &str = "LunarisAgentService";
/// Display name; Sunshine: `DisplayName= "Sunshine Service"`
/// (src_assets/windows/misc/service/install-service.bat:61).
const DISPLAY_NAME: &str = "Lunaris Agent Service";
/// Sunshine: `sc description` (install-service.bat:64).
const DESCRIPTION: &str = "Lunaris is a remote desktop host: runs it in the console session as SYSTEM.";

// ---- Globals, sunshinesvc.cpp:20-23 ----

/// `SERVICE_STATUS_HANDLE service_status_handle` (sunshinesvc.cpp:20).
static SERVICE_STATUS_HANDLE_G: AtomicIsize = AtomicIsize::new(0);
/// `SERVICE_STATUS service_status` (sunshinesvc.cpp:21). A mutex because the
/// control handler runs on another thread than `ServiceMain`.
static SERVICE_STATUS_G: Mutex<SERVICE_STATUS> = Mutex::new(SERVICE_STATUS {
    dwServiceType: SERVICE_WIN32_OWN_PROCESS,
    dwCurrentState: SERVICE_STOPPED,
    dwControlsAccepted: 0,
    dwWin32ExitCode: 0,
    dwServiceSpecificExitCode: 0,
    dwCheckPoint: 0,
    dwWaitHint: 0,
});
/// `HANDLE stop_event` (sunshinesvc.cpp:22), stored as the handle value.
static STOP_EVENT: AtomicIsize = AtomicIsize::new(0);
/// `HANDLE session_change_event` (sunshinesvc.cpp:23), stored as the handle value.
static SESSION_CHANGE_EVENT: AtomicIsize = AtomicIsize::new(0);

fn handle_of(slot: &AtomicIsize) -> HANDLE {
    HANDLE(slot.load(Ordering::SeqCst) as *mut c_void)
}

/// `SetServiceStatus(service_status_handle, &service_status)` after changing
/// the global status with `change` (the pattern repeated all over
/// sunshinesvc.cpp:52-55, 208-216, 220-225, ...).
fn report_status(change: impl FnOnce(&mut SERVICE_STATUS)) {
    let mut status = SERVICE_STATUS_G.lock().unwrap_or_else(|e| e.into_inner());
    change(&mut status);
    unsafe {
        let _ = SetServiceStatus(
            SERVICE_STATUS_HANDLE(SERVICE_STATUS_HANDLE_G.load(Ordering::SeqCst) as *mut c_void),
            &*status,
        );
    }
}

/// The Win32 error behind a failed `windows` call (`GetLastError()` in the
/// C++); the crate turns it into an HRESULT of the form 0x8007xxxx.
fn win32_error(e: &windows::core::Error) -> u32 {
    let code = e.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}

fn wide_z(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

// ---- Command lines (adaptation 1) ----

/// `["agent"]` when this process was started as `<exe> agent ...`, that is
/// when the executable is the unified app (`client-desktop`, see
/// client-desktop/src/main.rs), `[]` for the standalone agent. Every command
/// line below (service binPath, child, terminate helper) is built with it, so
/// that the same code works through both executables; the service itself is
/// started by the SCM with the arguments installed by [`install_service`],
/// so the prefix survives all re-invocations.
pub fn agent_prefix() -> Vec<OsString> {
    match std::env::args_os().nth(1) {
        Some(first) if first == "agent" => vec![OsString::from("agent")],
        _ => Vec::new(),
    }
}

/// Quotes one argument for a Windows command line.
fn quote(s: &OsStr) -> OsString {
    let mut out = OsString::from("\"");
    out.push(s);
    out.push("\"");
    out
}

/// `"<exe>" [agent] <args...>` (sunshinesvc.cpp:150-156 for the terminate helper).
fn command_line(exe: &OsStr, args: &[OsString]) -> OsString {
    let mut cmd = quote(exe);
    for prefix in agent_prefix() {
        cmd.push(" ");
        cmd.push(prefix);
    }
    for a in args {
        cmd.push(" ");
        cmd.push(a);
    }
    cmd
}

/// `%ProgramData%\Lunaris`: the same three lines as `data_dir` in
/// server/src/service.rs (`imp::data_dir`); duplicated because the `agent`
/// crate does not depend on the `server` crate.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into());
    Path::new(&base).join("Lunaris")
}

fn service_config_path() -> PathBuf {
    data_dir().join("agent_config.json")
}

// ---- HandlerEx, sunshinesvc.cpp:35-64 ----

/// Port of `HandlerEx` (sunshinesvc.cpp:35-64): INTERROGATE, the one
/// session-change event (WTS_CONSOLE_CONNECT), and STOP/PRESHUTDOWN.
unsafe extern "system" fn handler_ex(
    dw_control: u32,
    dw_event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match dw_control {
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,

        SERVICE_CONTROL_SESSIONCHANGE => {
            // If a new session connects to the console, restart the host
            // to allow it to spawn inside the new console session.
            if dw_event_type == WTS_CONSOLE_CONNECT {
                let _ = SetEvent(handle_of(&SESSION_CHANGE_EVENT));
            }
            NO_ERROR.0
        }

        // PRESHUTDOWN: the system is shutting down (falls through to STOP,
        // sunshinesvc.cpp:48-50).
        SERVICE_CONTROL_PRESHUTDOWN | SERVICE_CONTROL_STOP => {
            // Let SCM know we're stopping in up to 30 seconds
            report_status(|s| {
                s.dwCurrentState = SERVICE_STOP_PENDING;
                s.dwControlsAccepted = 0;
                s.dwWaitHint = 30 * 1000;
            });

            // Trigger ServiceMain() to start cleanup
            let _ = SetEvent(handle_of(&STOP_EVENT));
            NO_ERROR.0
        }

        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

// ---- CreateJobObjectForChildProcess, sunshinesvc.cpp:66-88 ----

unsafe fn create_job_object_for_child_process() -> Option<HANDLE> {
    let job_handle = CreateJobObjectW(None, PCWSTR::null()).ok()?;

    let mut job_limit_info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();

    // Kill the host when the final job object handle is closed (which will
    // happen if we terminate unexpectedly). This ensures we don't leave an
    // orphaned host running with an inherited handle to our log file.
    job_limit_info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

    // Allow the host to use CREATE_BREAKAWAY_FROM_JOB when spawning processes
    // to ensure they can live beyond the lifetime of the service.
    job_limit_info.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_BREAKAWAY_OK;

    if SetInformationJobObject(
        job_handle,
        JobObjectExtendedLimitInformation,
        &job_limit_info as *const _ as *const c_void,
        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
    )
    .is_err()
    {
        let _ = CloseHandle(job_handle);
        return None;
    }

    Some(job_handle)
}

// ---- AllocateProcThreadAttributeList, sunshinesvc.cpp:90-105 ----

unsafe fn allocate_proc_thread_attribute_list(
    attribute_count: u32,
) -> Option<LPPROC_THREAD_ATTRIBUTE_LIST> {
    let mut size: usize = 0;
    // Fails with ERROR_INSUFFICIENT_BUFFER; only `size` matters.
    let _ = InitializeProcThreadAttributeList(
        Some(LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut())),
        attribute_count,
        Some(0),
        &mut size,
    );

    let heap = GetProcessHeap().ok()?;
    let list = HeapAlloc(heap, HEAP_FLAGS(0), size);
    if list.is_null() {
        return None;
    }
    let list = LPPROC_THREAD_ATTRIBUTE_LIST(list);

    if InitializeProcThreadAttributeList(Some(list), attribute_count, Some(0), &mut size).is_err() {
        let _ = HeapFree(heap, HEAP_FLAGS(0), Some(list.0));
        return None;
    }

    Some(list)
}

// ---- DuplicateTokenForSession, sunshinesvc.cpp:107-129 ----

unsafe fn duplicate_token_for_session(console_session_id: u32) -> Option<HANDLE> {
    let mut current_token = HANDLE::default();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE, &mut current_token).is_err() {
        return None;
    }

    // Duplicate our own LocalSystem token
    let mut new_token = HANDLE::default();
    if DuplicateTokenEx(
        current_token,
        TOKEN_ALL_ACCESS,
        None,
        SecurityImpersonation,
        TokenPrimary,
        &mut new_token,
    )
    .is_err()
    {
        let _ = CloseHandle(current_token);
        return None;
    }

    let _ = CloseHandle(current_token);

    // Change the duplicated token to the console session ID
    if SetTokenInformation(
        new_token,
        TokenSessionId,
        &console_session_id as *const u32 as *const c_void,
        std::mem::size_of::<u32>() as u32,
    )
    .is_err()
    {
        let _ = CloseHandle(new_token);
        return None;
    }

    Some(new_token)
}

// ---- OpenLogFileHandle, sunshinesvc.cpp:131-146 ----

/// Port of `logging::rotate_log_file` (Sunshine src/logging.h:54-78, keeps
/// `retained_log_file_count` = 5 previous logs): `<log>.5` is removed, `.4` ->
/// `.5` ... `.1` -> `.2`, and the current log becomes `.1`. As there, the
/// first filesystem error stops the rotation, and the caller ignores it
/// (sunshinesvc.cpp:139 discards the returned error code).
fn rotate_log_file(log_file: &Path) -> std::io::Result<()> {
    const RETAINED: usize = 5;
    let rotated = |generation: usize| -> PathBuf {
        let mut p = log_file.as_os_str().to_owned();
        p.push(format!(".{generation}"));
        PathBuf::from(p)
    };

    // std::filesystem::remove() does not fail for a missing file.
    match std::fs::remove_file(rotated(RETAINED)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    for generation in (2..=RETAINED).rev() {
        let previous = rotated(generation - 1);
        if previous.exists() {
            std::fs::rename(&previous, rotated(generation))?;
        }
    }
    if log_file.exists() {
        std::fs::rename(log_file, rotated(1))?;
    }
    Ok(())
}

/// Port of `OpenLogFileHandle` (sunshinesvc.cpp:131-146). Adaptation 1: the log
/// is `<ProgramData>\Lunaris\agent-service.log`, not `sunshine.log` in the
/// temp folder (:134-136); the folder is created if missing.
unsafe fn open_log_file_handle() -> windows::core::Result<HANDLE> {
    let _ = std::fs::create_dir_all(data_dir());
    let log_file_name = data_dir().join("agent-service.log");

    // Preserve previous service output before opening the current log.
    let _ = rotate_log_file(&log_file_name);

    // The file handle must be inheritable for our child process to use it
    let security_attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };

    // Create the current log
    let wide = wide_z(log_file_name.as_os_str());
    CreateFileW(
        PCWSTR(wide.as_ptr()),
        windows::Win32::Foundation::GENERIC_WRITE.0,
        FILE_SHARE_READ,
        Some(&security_attributes),
        CREATE_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
        None,
    )
}

// ---- RunTerminationHelper, sunshinesvc.cpp:148-182 ----

unsafe fn run_termination_helper(console_token: HANDLE, pid: u32) -> bool {
    let module_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    // `"<exe>" [agent] --terminate <pid>` (:150-156)
    let command = command_line(
        module_path.as_os_str(),
        &[OsString::from("--terminate"), OsString::from(pid.to_string())],
    );
    let module_wide = wide_z(module_path.as_os_str());
    let mut command_wide = wide_z(&command);

    let startup_info = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(w!("winsta0\\default").as_ptr() as *mut u16),
        ..Default::default()
    };

    // Execute ourselves as a detached process in the user session with the
    // --terminate argument. This will allow us to attach to the host's console
    // and send it a Ctrl-C event.
    let mut process_info = PROCESS_INFORMATION::default();
    if CreateProcessAsUserW(
        Some(console_token),
        PCWSTR(module_wide.as_ptr()),
        Some(PWSTR(command_wide.as_mut_ptr())),
        None,
        None,
        false,
        CREATE_UNICODE_ENVIRONMENT | DETACHED_PROCESS,
        None,
        PCWSTR::null(),
        &startup_info,
        &mut process_info,
    )
    .is_err()
    {
        return false;
    }

    // Wait for the termination helper to complete
    WaitForSingleObject(process_info.hProcess, u32::MAX);

    // Check the exit status of the helper process
    let mut exit_code: u32 = 0;
    let _ = GetExitCodeProcess(process_info.hProcess, &mut exit_code);

    // Cleanup handles
    let _ = CloseHandle(process_info.hProcess);
    let _ = CloseHandle(process_info.hThread);

    // If the helper process returned 0, it succeeded
    exit_code == 0
}

// ---- TerminateSunshine, sunshinesvc.cpp:184-194 ----

/// Port of `TerminateSunshine`: terminate the host gracefully, forcing
/// termination if it does not exit within 20 seconds.
unsafe fn terminate_agent(console_token: HANDLE, process_info: &PROCESS_INFORMATION) {
    if !run_termination_helper(console_token, process_info.dwProcessId)
        || WaitForSingleObject(process_info.hProcess, 20000) != WAIT_OBJECT_0
    {
        let _ = TerminateProcess(process_info.hProcess, ERROR_PROCESS_ABORTED.0);
    }
}

// ---- ServiceMain, sunshinesvc.cpp:196-351 ----

/// Report SERVICE_STOPPED with the last error and return (the repeated
/// "Tell SCM we failed to start" blocks, sunshinesvc.cpp:220-225, 230-235, ...).
fn fail_to_start(error: u32) {
    report_status(|s| {
        s.dwWin32ExitCode = error;
        s.dwCurrentState = SERVICE_STOPPED;
    });
}

/// Port of `ServiceMain`: run the host in the active console session until the
/// Windows service is stopped.
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide_z(OsStr::new(SERVICE_NAME));
    match RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(handler_ex), None) {
        Ok(handle) => SERVICE_STATUS_HANDLE_G.store(handle.0 as isize, Ordering::SeqCst),
        Err(e) => {
            // Nothing we can really do here but terminate ourselves
            std::process::exit(win32_error(&e) as i32);
        }
    }

    // Tell SCM we're starting
    report_status(|s| {
        s.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
        s.dwServiceSpecificExitCode = 0;
        s.dwWin32ExitCode = NO_ERROR.0;
        s.dwWaitHint = 0;
        s.dwControlsAccepted = 0;
        s.dwCheckPoint = 0;
        s.dwCurrentState = SERVICE_START_PENDING;
    });

    // Create a manual-reset stop event
    match CreateEventA(None, true, false, windows::core::PCSTR::null()) {
        Ok(h) => STOP_EVENT.store(h.0 as isize, Ordering::SeqCst),
        Err(e) => {
            // Tell SCM we failed to start
            fail_to_start(win32_error(&e));
            return;
        }
    }

    // Create an auto-reset session change event
    match CreateEventA(None, false, false, windows::core::PCSTR::null()) {
        Ok(h) => SESSION_CHANGE_EVENT.store(h.0 as isize, Ordering::SeqCst),
        Err(e) => {
            fail_to_start(win32_error(&e));
            return;
        }
    }
    let stop_event = handle_of(&STOP_EVENT);
    let session_change_event = handle_of(&SESSION_CHANGE_EVENT);

    // `log_file_handle` lives in this frame: the handle-list attribute below
    // keeps a pointer to it (UpdateProcThreadAttribute copies nothing).
    let log_file_handle = match open_log_file_handle() {
        Ok(h) => h,
        Err(e) => {
            fail_to_start(win32_error(&e));
            return;
        }
    };

    // We can use a single STARTUPINFOEXW for all the processes that we launch
    let mut startup_info = STARTUPINFOEXW::default();
    startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_info.StartupInfo.lpDesktop = PWSTR(w!("winsta0\\default").as_ptr() as *mut u16);
    startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup_info.StartupInfo.hStdInput = HANDLE::default();
    startup_info.StartupInfo.hStdOutput = log_file_handle;
    startup_info.StartupInfo.hStdError = log_file_handle;

    // Allocate an attribute list with space for 2 entries
    startup_info.lpAttributeList = match allocate_proc_thread_attribute_list(2) {
        Some(list) => list,
        None => {
            // Tell SCM we failed to start. (C++ reports GetLastError() here;
            // after the failed HeapAlloc / Initialize... it is whatever those
            // left, the nearest equivalent is the last OS error.)
            fail_to_start(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32);
            return;
        }
    };

    // Only allow the host to inherit the log file handle, not all inheritable
    // handles
    let _ = UpdateProcThreadAttribute(
        startup_info.lpAttributeList,
        0,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        Some(&log_file_handle as *const HANDLE as *const c_void),
        std::mem::size_of::<HANDLE>(),
        None,
        None,
    );

    // Tell SCM we're running (and stoppable now)
    report_status(|s| {
        s.dwControlsAccepted =
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_PRESHUTDOWN | SERVICE_ACCEPT_SESSIONCHANGE;
        s.dwCurrentState = SERVICE_RUNNING;
    });

    // Difference from the C++: the job handle lives in this slot, declared
    // once, because the job-list attribute stores the ADDRESS of the handle
    // (sunshinesvc.cpp:295) and, as UpdateProcThreadAttribute refuses to set
    // the same non-additive attribute twice, from the second launch on it
    // keeps pointing at the slot of the first one. The C++ loop body reuses
    // the same stack slot by luck; here it is explicit.
    let mut job_slot: HANDLE;

    // Loop every 3 seconds until the stop event is set or the host is running
    while WaitForSingleObject(stop_event, 3000) != WAIT_OBJECT_0 {
        let console_session_id = WTSGetActiveConsoleSessionId();
        if console_session_id == 0xFFFF_FFFF {
            // No console session yet
            continue;
        }

        let console_token = match duplicate_token_for_session(console_session_id) {
            Some(t) => t,
            None => continue,
        };

        // Job objects cannot span sessions, so we must create one for each process
        let job_handle = match create_job_object_for_child_process() {
            Some(j) => j,
            None => {
                let _ = CloseHandle(console_token);
                continue;
            }
        };
        job_slot = job_handle;

        // Start the host inside our job object
        let _ = UpdateProcThreadAttribute(
            startup_info.lpAttributeList,
            0,
            PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
            Some(&job_slot as *const HANDLE as *const c_void),
            std::mem::size_of::<HANDLE>(),
            None,
            None,
        );

        // Adaptation 1: the child is the host itself, with its arguments,
        // instead of `L"Sunshine.exe"` with none (:298). The application name
        // is the full path of this executable, like the terminate helper's
        // (:165); Sunshine's relative name relies on the working directory.
        let module_path = match std::env::current_exe() {
            Ok(p) => p,
            Err(_) => {
                let _ = CloseHandle(console_token);
                let _ = CloseHandle(job_handle);
                continue;
            }
        };
        let command = command_line(
            module_path.as_os_str(),
            &[
                OsString::from("--cli"),
                OsString::from("--config"),
                quote(service_config_path().as_os_str()),
            ],
        );
        let module_wide = wide_z(module_path.as_os_str());
        let mut command_wide = wide_z(&command);

        let mut process_info = PROCESS_INFORMATION::default();
        if CreateProcessAsUserW(
            Some(console_token),
            PCWSTR(module_wide.as_ptr()),
            Some(PWSTR(command_wide.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            None,
            PCWSTR::null(),
            &startup_info as *const STARTUPINFOEXW as *const STARTUPINFOW,
            &mut process_info,
        )
        .is_err()
        {
            let _ = CloseHandle(console_token);
            let _ = CloseHandle(job_handle);
            continue;
        }

        loop {
            // Wait for the stop event to be set, the host to terminate, or the
            // console session to change
            let wait_objects = [stop_event, process_info.hProcess, session_change_event];
            let still_running;
            match WaitForMultipleObjects(&wait_objects, false, u32::MAX).0 {
                // WAIT_OBJECT_0 + 2 (:309-315), falling through to WAIT_OBJECT_0
                // when the console session really changed.
                x if x == WAIT_OBJECT_0.0 + 2 => {
                    if WTSGetActiveConsoleSessionId() == console_session_id {
                        // The active console session didn't actually change.
                        // Let the host keep running.
                        still_running = true;
                    } else {
                        // Terminate the host and start it again.
                        terminate_agent(console_token, &process_info);
                        still_running = false;
                    }
                }
                x if x == WAIT_OBJECT_0.0 => {
                    // The service is shutting down, so try to gracefully
                    // terminate the host.
                    terminate_agent(console_token, &process_info);
                    still_running = false;
                }
                x if x == WAIT_OBJECT_0.0 + 1 => {
                    // The host terminated itself.
                    let mut exit_code: u32 = 0;
                    if GetExitCodeProcess(process_info.hProcess, &mut exit_code).is_ok()
                        && exit_code == ERROR_SHUTDOWN_IN_PROGRESS.0
                    {
                        // The host is asking for us to shut down, so
                        // gracefully stop ourselves.
                        let _ = SetEvent(stop_event);
                    }
                    still_running = false;
                }
                _ => {
                    // Stop the service rather than orphaning the host after an
                    // unexpected wait failure.
                    let _ = SetEvent(stop_event);
                    terminate_agent(console_token, &process_info);
                    still_running = false;
                }
            }
            if !still_running {
                break;
            }
        }

        let _ = CloseHandle(process_info.hThread);
        let _ = CloseHandle(process_info.hProcess);
        let _ = CloseHandle(console_token);
        let _ = CloseHandle(job_handle);
    }

    // Let SCM know we've stopped
    report_status(|s| s.dwCurrentState = SERVICE_STOPPED);
}

// ---- DoGracefulTermination, sunshinesvc.cpp:353-369 ----

/// Port of `DoGracefulTermination`; this runs in a child process in the user
/// session. Returns the process exit code.
unsafe fn do_graceful_termination(pid: u32) -> i32 {
    // Attach to the host's console
    if let Err(e) = AttachConsole(pid) {
        return win32_error(&e) as i32;
    }

    // Disable our own Ctrl-C handling
    let _ = SetConsoleCtrlHandler(None, true);

    // Send a Ctrl-C event to the host
    if let Err(e) = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) {
        return win32_error(&e) as i32;
    }

    0
}

// ---- main, sunshinesvc.cpp:371-396 ----

/// `main`'s first branch (sunshinesvc.cpp:372-375): the re-invocation of
/// ourselves to send Ctrl-C to the host. Never returns.
pub fn terminate_main(pid: u32) -> ! {
    std::process::exit(unsafe { do_graceful_termination(pid) });
}

/// `main`'s second part (sunshinesvc.cpp:377-395): set the working directory
/// and trigger `ServiceMain` through the service control dispatcher. Never
/// returns.
pub fn service_run_main() -> ! {
    // By default, services have their current directory set to
    // %SYSTEMROOT%\System32. We want to use the directory where the host
    // executable is located instead of system32.
    // Difference: Sunshine strips 2 path components (file name and `tools`
    // folder, :380-387) because sunshinesvc.exe sits in `<root>\tools` and the
    // host in `<root>`. Here the service and the host are the same file, so
    // only the file name is stripped.
    let module_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => std::process::exit(e.raw_os_error().unwrap_or(1)),
    };
    let dir = module_path.parent().map(Path::to_path_buf).unwrap_or(module_path);
    if let Err(e) = std::env::set_current_dir(&dir) {
        std::process::exit(e.raw_os_error().unwrap_or(1));
    }

    // Trigger our ServiceMain()
    let mut service_name = wide_z(OsStr::new(SERVICE_NAME));
    let service_table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(service_name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR::null(),
            lpServiceProc: None,
        },
    ];
    // C++ returns the BOOL of StartServiceCtrlDispatcher as the exit code
    // (:395), i.e. 1 on success and 0 on failure. Same here. Addition: say why
    // on stderr when the dispatcher fails (typically: started from a console
    // instead of by the Service Control Manager).
    match unsafe { StartServiceCtrlDispatcherW(service_table.as_ptr()) } {
        Ok(()) => std::process::exit(1),
        Err(e) => {
            eprintln!(
                "error: {e} (--service-run is used by the Windows service manager; \
                 use --install-service instead)"
            );
            std::process::exit(0)
        }
    }
}

// ---- Install / uninstall ----
//
// What Sunshine's scripts do (src_assets/windows/misc/):
//   service/install-service.bat   create (or `config`, after `net stop`) the
//       service with binPath = the quoted launcher, start= demand,
//       DisplayName; `sc description`; `net start`        (:20-30, 58-67)
//   autostart/autostart-service.bat   `sc config <service> start= auto`  (:4)
//   service/uninstall-service.bat   `net stop`, `sc delete`  (:106-108)
// done with the `windows-service` crate like server/src/service.rs does.
// Not ported: the legacy `sunshinesvc` removal (:15-17, there is no legacy
// Lunaris service) and keeping the previous start type across reinstalls
// (install-service.bat:32-56, uninstall-service.bat:3-39).

use windows_service::{
    service::{
        Service, ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
        ServiceType,
    },
    service_manager::{ServiceManager, ServiceManagerAccess},
};

/// The two commands change the Service Control Manager's database, which only
/// an elevated prompt may do.
fn scm_err(e: windows_service::Error) -> String {
    format!("{e} (run this from an elevated Administrator prompt)")
}

/// `net stop`: stop and wait up to 10 s (the same loop as `stop_and_wait` in
/// server/src/service.rs).
fn stop_and_wait(service: &Service) {
    if let Ok(s) = service.query_status() {
        if s.current_state != ServiceState::Stopped {
            let _ = service.stop();
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(250));
                if matches!(service.query_status(), Ok(s) if s.current_state == ServiceState::Stopped)
                {
                    break;
                }
            }
        }
    }
}

fn service_info(exe: PathBuf, start_type: ServiceStartType) -> ServiceInfo {
    let mut launch_arguments = agent_prefix();
    launch_arguments.push(OsString::from("--service-run"));
    ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments,
        dependencies: vec![],
        // LocalSystem: install-service.bat:61 has no `obj=`.
        account_name: None,
        account_password: None,
    }
}

/// `--install-service`. `user_config` is the calling user's default config
/// file (adaptation 2).
pub fn install_service(user_config: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(scm_err)?;

    let data = data_dir();
    std::fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;

    // install-service.bat:20-30: an existing service is stopped and
    // reconfigured, otherwise created. start= demand until autostart below.
    let access = ServiceAccess::QUERY_STATUS
        | ServiceAccess::STOP
        | ServiceAccess::START
        | ServiceAccess::CHANGE_CONFIG;
    let service = match manager.open_service(SERVICE_NAME, access) {
        Ok(svc) => {
            stop_and_wait(&svc);
            svc.change_config(&service_info(exe.clone(), ServiceStartType::OnDemand))
                .map_err(scm_err)?;
            svc
        }
        Err(_) => manager
            .create_service(&service_info(exe.clone(), ServiceStartType::OnDemand), access)
            .map_err(scm_err)?,
    };
    let _ = service.set_description(DESCRIPTION);

    // autostart-service.bat:4: `sc config ... start= auto`
    service
        .change_config(&service_info(exe.clone(), ServiceStartType::AutoStart))
        .map_err(scm_err)?;

    // Adaptation 2: nothing is copied or migrated.
    let service_config = service_config_path();
    if !service_config.exists() && Path::new(user_config).exists() {
        let mut cmd = quote(exe.as_os_str());
        for p in agent_prefix() {
            cmd.push(" ");
            cmd.push(p);
        }
        println!(
            "{} does not exist yet; to use the settings of this user, run once: {} --cli --config \"{}\" --import-config \"{}\" (stop it with Ctrl+C), then restart the service.",
            service_config.display(),
            cmd.to_string_lossy(),
            service_config.display(),
            user_config,
        );
    }

    // `net start`
    service.start(&[] as &[&OsStr]).map_err(scm_err)?;

    println!("Installed and started the `{SERVICE_NAME}` Windows service (starts at boot).");
    println!("Log: {}", data.join("agent-service.log").display());
    Ok(())
}

/// `--uninstall-service` (uninstall-service.bat:106-108).
pub fn uninstall_service() -> Result<(), String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(scm_err)?;
    match manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        Ok(svc) => {
            stop_and_wait(&svc);
            svc.delete().map_err(scm_err)?;
            println!("Stopped and removed the `{SERVICE_NAME}` Windows service.");
        }
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST.0 as i32) =>
        {
            println!("The `{SERVICE_NAME}` Windows service is not installed.")
        }
        Err(e) => return Err(scm_err(e)),
    }
    Ok(())
}
