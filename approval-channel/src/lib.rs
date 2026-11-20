//! The approval helper's channel: lets the person at a computer answer the
//! "accept this connection?" questions of the Lunaris agent running there
//! as a daemon (the Windows service as SYSTEM; on Linux `--cli`, e.g. a
//! systemd user service, in the user's session), which shows nothing
//! itself. Without Qt or anything else of a desktop: the agent uses the
//! listening side ([`spawn_listener`], with its question state behind
//! [`Answerer`]), the optional tray program `lunaris-approval-helper`
//! (approval-helper/) the connecting side ([`connect`], [`run_answerer`]).
//!
//! The lines are those of the agent's stdio protocol (agent approval.rs),
//! after a first line saying the helper may answer:
//!
//! ```text
//! agent -> helper   @lunaris-approval-ready
//! agent -> helper   @lunaris-approval {"id":1,"requester":"huy (server x)","kind":"control","timeout_s":30}
//! agent -> helper   @lunaris-approval-cancel {"id":1}
//! helper -> agent   {"id":1,"accept":true}
//! ```
//!
//! One helper answers at a time (others are disconnected at once); when it
//! goes away, the questions waiting count as declined and the agent is
//! unattended again (only the password lets anyone in) until a helper
//! connects again. With no helper at all (a server, a VPS) nothing changes.
//!
//! The channels and who may use them:
//!
//! - Windows: the named pipe [`PIPE_NAME`], created by the agent with
//!   `FILE_FLAG_FIRST_PIPE_INSTANCE` (it fails when someone else holds the
//!   name) and remote clients rejected. Owner SYSTEM; SYSTEM has full access
//!   and the user logged on in the agent's session (the console) only read
//!   and write: not `FILE_CREATE_PIPE_INSTANCE`, so nobody else can serve
//!   on the name while the agent holds it. The DACL follows that user (no
//!   one at the logon screen; then whoever logs on). A client is let in
//!   when it runs in the agent's session, as that user, from an executable
//!   in the agent's own folder (the install folder). The helper in turn
//!   only talks to a pipe owned by SYSTEM, Administrators or its own user,
//!   served from its session, and opens it at identification level (the
//!   server cannot act as the helper's user).
//! - Linux: the Unix socket `$XDG_RUNTIME_DIR/lunaris/approval.sock`
//!   (folder 0700, socket 0600), for the agent running as the user (a
//!   systemd user service or the login autostart, `--cli`). Both ends check
//!   that the other runs as the same user (`SO_PEERCRED`). An agent running
//!   as root or without `XDG_RUNTIME_DIR` (a system service) offers no
//!   channel: nobody is asked there.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{info, warn};

use serde::{Deserialize, Serialize};

/// A question put to the person here (also the JSON of the protocol).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    pub id: u64,
    /// Who asks, as the other side says of itself.
    pub requester: String,
    /// "control" or "files".
    pub kind: &'static str,
    /// Seconds until it counts as declined.
    pub timeout_s: u64,
}

/// What the one showing the questions is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Show this question.
    Ask(Question),
    /// Take question `id` down: answered, timed out, or withdrawn.
    Cancel(u64),
}

#[derive(Deserialize)]
struct Answer {
    id: u64,
    accept: bool,
}

/// The line of the protocol for `event` (without the newline).
pub fn event_line(event: &Event) -> String {
    match event {
        Event::Ask(q) => format!("@lunaris-approval {}", serde_json::to_string(q).unwrap_or_default()),
        Event::Cancel(id) => format!("@lunaris-approval-cancel {{\"id\":{}}}", id),
    }
}

/// The event of a line written by [`event_line`]; None for anything else.
pub fn parse_event_line(line: &str) -> Option<Event> {
    #[derive(Deserialize)]
    struct Asked {
        id: u64,
        requester: String,
        kind: String,
        timeout_s: u64,
    }
    #[derive(Deserialize)]
    struct Cancelled {
        id: u64,
    }
    let line = line.trim();
    if let Some(json) = line.strip_prefix("@lunaris-approval ") {
        let q: Asked = serde_json::from_str(json).ok()?;
        Some(Event::Ask(Question {
            id: q.id,
            requester: q.requester,
            kind: if q.kind == "files" { "files" } else { "control" },
            timeout_s: q.timeout_s,
        }))
    } else if let Some(json) = line.strip_prefix("@lunaris-approval-cancel ") {
        serde_json::from_str::<Cancelled>(json).ok().map(|c| Event::Cancel(c.id))
    } else {
        None
    }
}

/// The answer line of the protocol (without the newline).
pub fn answer_line(id: u64, accept: bool) -> String {
    format!("{{\"id\":{},\"accept\":{}}}", id, accept)
}

/// The answer in a line read from whoever answers: Ok(None) for an empty
/// line, Err for anything that is not an answer.
pub fn parse_answer_line(line: &str) -> Result<Option<(u64, bool)>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<Answer>(line).map(|a| Some((a.id, a.accept))).map_err(|e| e.to_string())
}

/// The agent's questions, as the listening side sees them.
pub trait Answerer: Send + Sync + 'static {
    /// A helper answers from now on: each question and withdrawal goes to
    /// `sink` (which must not block). Returns what [`Answerer::detach`]
    /// takes.
    fn attach(&self, sink: Box<dyn Fn(Event) + Send + Sync>) -> u64;
    /// The helper answered question `id`.
    fn answer(&self, id: u64, accept: bool);
    /// The helper of `attach`'s `token` went away: its questions count as
    /// declined, unless another one attached since.
    fn detach(&self, token: u64);
}

/// The agent's first line to a helper it lets answer (one turned away
/// gets nothing: the channel just closes).
pub const READY_LINE: &str = "@lunaris-approval-ready";

/// The named pipe on Windows.
pub const PIPE_NAME: &str = r"\\.\pipe\lunaris-approval";

/// The longest line read from the other end (questions carry what the
/// other side says of itself; answers are short).
const MAX_LINE: usize = 64 * 1024;

/// How often the helper tries again to reach the agent.
pub const RETRY: Duration = Duration::from_secs(3);

/// Reads one line (without its end) of at most `max` bytes; Ok(None) at the
/// end of the stream; Err for a longer line or one that is not UTF-8.
pub async fn read_line_capped<R: AsyncBufRead + Unpin>(reader: &mut R, max: usize) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let (take, done) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (buf.len(), false),
        };
        if line.len() + take > max + 1 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
        line.extend_from_slice(&buf[..take]);
        reader.consume(take);
        if done {
            break;
        }
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).map(Some).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))
}

/// Agent side: the helper on `reader`/`writer` answers from now on, until
/// it goes away (the end of `reader`, an error, or a line that is not an
/// answer); then nobody answers any more ([`Answerer::detach`]).
pub async fn serve_answerer<R, W>(reader: R, mut writer: W, answerer: &dyn Answerer) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();
    writer.write_all(format!("{READY_LINE}\n").as_bytes()).await?;
    writer.flush().await?;
    let generation = answerer.attach(Box::new(move |event| {
        let _ = tx.send(event);
    }));
    let mut reader = BufReader::new(reader);
    let read = async {
        loop {
            let Some(line) = read_line_capped(&mut reader, MAX_LINE).await? else {
                return Ok(());
            };
            match parse_answer_line(&line) {
                Ok(Some((id, accept))) => answerer.answer(id, accept),
                Ok(None) => {}
                Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("not an answer: {e}"))),
            }
        }
    };
    let write = async {
        while let Some(event) = rx.recv().await {
            let mut line = event_line(&event);
            line.push('\n');
            writer.write_all(line.as_bytes()).await?;
            writer.flush().await?;
        }
        Ok::<(), io::Error>(())
    };
    let result = tokio::select! {
        r = read => r,
        w = write => w,
    };
    answerer.detach(generation);
    result
}

/// Helper side: once the agent said it lets this helper answer, calls
/// `on_ready`, then shows each question and withdrawal with `on_event` and
/// sends the answers given on `answers`, until the agent goes away (Ok) or
/// sends something else (Err). Err(NotConnected) when the agent turned
/// this helper away (another one answers).
pub async fn run_answerer<R, W>(
    reader: R,
    mut writer: W,
    on_ready: impl FnOnce(),
    on_event: impl Fn(Event),
    answers: &mut mpsc::UnboundedReceiver<(u64, bool)>,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(reader);
    match read_line_capped(&mut reader, MAX_LINE).await? {
        Some(line) if line == READY_LINE => on_ready(),
        Some(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "not the agent")),
        None => return Err(io::Error::new(io::ErrorKind::NotConnected, "turned away: another helper answers")),
    }
    let read = async {
        loop {
            let Some(line) = read_line_capped(&mut reader, MAX_LINE).await? else {
                return Ok(());
            };
            if line.trim().is_empty() {
                continue;
            }
            match parse_event_line(&line) {
                Some(event) => on_event(event),
                None => return Err(io::Error::new(io::ErrorKind::InvalidData, "not a question")),
            }
        }
    };
    let write = async {
        while let Some((id, accept)) = answers.recv().await {
            let mut line = answer_line(id, accept);
            line.push('\n');
            writer.write_all(line.as_bytes()).await?;
            writer.flush().await?;
        }
        Ok::<(), io::Error>(())
    };
    tokio::select! {
        r = read => r,
        w = write => w,
    }
}

/// Agent side: offers the channel for the helper in the background (in a
/// tokio runtime); `answerer` holds the agent's questions.
pub fn spawn_listener(answerer: std::sync::Arc<dyn Answerer>) {
    #[cfg(target_os = "linux")]
    tokio::spawn(async move {
        if let Err(e) = unix::listen(answerer).await {
            warn!("Approval helper: {}; only the password lets anyone in", e);
        }
    });
    #[cfg(target_os = "windows")]
    tokio::spawn(async move {
        if let Err(e) = pipe::listen(answerer).await {
            warn!("Approval helper: {}; only the password lets anyone in", e);
        }
    });
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    drop(answerer);
}

/// Helper side: connects to the agent on this computer, checked (see the
/// module); the two ends of the channel.
pub async fn connect() -> io::Result<(Box<dyn AsyncRead + Unpin + Send>, Box<dyn AsyncWrite + Unpin + Send>)> {
    #[cfg(target_os = "linux")]
    {
        let stream = unix::connect().await?;
        let (r, w) = stream.into_split();
        Ok((Box::new(r), Box::new(w)))
    }
    #[cfg(target_os = "windows")]
    {
        let client = pipe::connect()?;
        let (r, w) = tokio::io::split(client);
        Ok((Box::new(r), Box::new(w)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no approval helper on this system"))
    }
}

/// Whether there can be an agent here for the helper to answer: on Windows,
/// whether the agent's service is installed; on Linux, whether there is a
/// runtime folder (a desktop session).
pub fn agent_may_listen() -> bool {
    #[cfg(target_os = "windows")]
    {
        pipe::service_installed()
    }
    #[cfg(target_os = "linux")]
    {
        unix::socket_path().is_some()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        false
    }
}

/// Whether two executables are in the same folder (Windows paths: case does
/// not matter, `\\?\` prefixes are ignored).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    let folder = |p: &std::path::Path| {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
        s.rfind('\\').map(|i| s[..i].trim_end_matches('\\').to_lowercase()).unwrap_or_default()
    };
    let (a, b) = (folder(a), folder(b));
    !a.is_empty() && a == b
}

/// The pipe's security descriptor (SDDL): full access for SYSTEM (and, for
/// an agent not running as SYSTEM, its own user `own_sid`), read and write
/// for `answerer` (`FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES
/// | READ_CONTROL | SYNCHRONIZE`: no `FILE_CREATE_PIPE_INSTANCE`).
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn pipe_sddl(as_system: bool, own_sid: &str, answerer: Option<&str>) -> String {
    let mut sddl = if as_system {
        "O:SYD:P(A;;FA;;;SY)".to_string()
    } else {
        format!("D:P(A;;FA;;;SY)(A;;FA;;;{own_sid})")
    };
    if let Some(user) = answerer.filter(|u| as_system || *u != own_sid) {
        sddl.push_str(&format!("(A;;0x{:x};;;{})", CLIENT_ACCESS, user));
    }
    sddl
}

/// What the helper opens the pipe with, and all the answerer is granted.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
const CLIENT_ACCESS: u32 = 0x0001 /* FILE_READ_DATA */
    | 0x0002 /* FILE_WRITE_DATA */
    | 0x0080 /* FILE_READ_ATTRIBUTES */
    | 0x0002_0000 /* READ_CONTROL */
    | 0x0010_0000 /* SYNCHRONIZE */;

#[cfg(target_os = "linux")]
mod unix {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::net::{UnixListener, UnixStream};

    fn euid() -> u32 {
        unsafe { libc::geteuid() }
    }

    /// `$XDG_RUNTIME_DIR/lunaris/approval.sock`; None without that folder.
    pub fn socket_path() -> Option<PathBuf> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|d| d.is_absolute())?;
        Some(dir.join("lunaris").join("approval.sock"))
    }

    /// The socket's folder: made 0700, or an existing one taken when it is
    /// a real folder of this user (then made 0700).
    pub(super) fn prepare_dir(dir: &Path) -> io::Result<()> {
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != euid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is not a folder of this user", dir.display()),
            ));
        }
        if meta.mode() & 0o777 != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    /// Binds the socket at `path` (a stale one is replaced; a live one, of
    /// another agent, is left alone: Err).
    pub(super) async fn bind(path: &Path) -> io::Result<UnixListener> {
        prepare_dir(path.parent().unwrap_or(Path::new("/")))?;
        if std::fs::symlink_metadata(path).is_ok() {
            if UnixStream::connect(path).await.is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another agent already offers {}", path.display()),
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    pub async fn listen(answerer: Arc<dyn Answerer>) -> io::Result<()> {
        if euid() == 0 {
            info!("Approval helper: the agent runs as root, so nobody here is asked; only the password lets anyone in");
            return Ok(());
        }
        let Some(path) = socket_path() else {
            info!("Approval helper: no XDG_RUNTIME_DIR, so nobody here is asked; only the password lets anyone in");
            return Ok(());
        };
        let listener = bind(&path).await?;
        info!("Approval helper: waiting on {}", path.display());
        serve(listener, euid(), answerer).await
    }

    /// Whether the other end of `stream` runs as `uid`.
    pub(super) fn peer_is(stream: &UnixStream, uid: u32) -> bool {
        stream.peer_cred().map(|c| c.uid() == uid).unwrap_or(false)
    }

    /// Accepts helpers running as `uid`, one at a time.
    pub(super) async fn serve(listener: UnixListener, uid: u32, answerer: Arc<dyn Answerer>) -> io::Result<()> {
        let busy = Arc::new(AtomicBool::new(false));
        loop {
            let (stream, _) = listener.accept().await?;
            if !peer_is(&stream, uid) {
                warn!("Approval helper: refused a connection from another user");
                continue;
            }
            if busy.swap(true, Ordering::SeqCst) {
                // Another helper answers already.
                continue;
            }
            let busy = busy.clone();
            let answerer = answerer.clone();
            tokio::spawn(async move {
                info!("Approval helper connected: the person here is asked");
                let (r, w) = stream.into_split();
                if let Err(e) = serve_answerer(r, w, &*answerer).await {
                    warn!("Approval helper: {}", e);
                }
                info!("Approval helper gone: only the password lets anyone in");
                busy.store(false, Ordering::SeqCst);
            });
        }
    }

    pub async fn connect() -> io::Result<UnixStream> {
        let path = socket_path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no XDG_RUNTIME_DIR"))?;
        connect_to(&path, euid()).await
    }

    /// Connects to `path`, served by a process running as `uid`.
    pub(super) async fn connect_to(path: &Path, uid: u32) -> io::Result<UnixStream> {
        let stream = UnixStream::connect(path).await?;
        if !peer_is(&stream, uid) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the agent's socket is served by another user"));
        }
        Ok(stream)
    }
}

#[cfg(target_os = "windows")]
mod pipe {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::{AsRawHandle, RawHandle};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, ServerOptions};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
        SetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
    };
    use windows::Win32::Security::{
        GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACL, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION,
        SECURITY_SQOS_PRESENT,
    };
    use windows::Win32::System::Pipes::{
        GetNamedPipeClientProcessId, GetNamedPipeClientSessionId, GetNamedPipeServerProcessId,
        GetNamedPipeServerSessionId,
    };
    use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn wide_z(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os_err(e: windows::core::Error) -> io::Error {
        io::Error::other(e.to_string())
    }

    /// Closes a handle when dropped.
    struct Owned(HANDLE);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Text form of a SID ("S-1-5-21-…").
    unsafe fn sid_string(sid: PSID) -> Option<String> {
        let mut out = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut out).ok()?;
        let s = out.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(out.0 as *mut c_void)));
        s
    }

    /// The user of a token, as a SID string.
    unsafe fn token_user(token: HANDLE) -> Option<String> {
        // TOKEN_USER and the SID after it (at most 68 bytes); u64 for alignment.
        let mut buf = [0u64; 32];
        let mut len = 0u32;
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            std::mem::size_of_val(&buf) as u32,
            &mut len,
        )
        .ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_string(user.User.Sid)
    }

    /// The user a process runs as.
    unsafe fn process_user(process: HANDLE) -> Option<String> {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let token = Owned(token);
        token_user(token.0)
    }

    /// Whether this process runs as LocalSystem (S-1-5-18), as the
    /// service's agent does.
    fn running_as_local_system() -> bool {
        own_user().as_deref() == Some("S-1-5-18")
    }

    /// The agent's Windows service (agent/src/win_service.rs `SERVICE_NAME`).
    const SERVICE_NAME: &str = "LunarisAgentService";

    /// Whether the agent's service is installed (any user may ask).
    pub fn service_installed() -> bool {
        use windows::Win32::System::Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
        };
        let name = wide_z(SERVICE_NAME);
        unsafe {
            let Ok(manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
                return false;
            };
            let found = match OpenServiceW(manager, PCWSTR(name.as_ptr()), SERVICE_QUERY_STATUS) {
                Ok(service) => {
                    let _ = CloseServiceHandle(service);
                    true
                }
                Err(_) => false,
            };
            let _ = CloseServiceHandle(manager);
            found
        }
    }

    fn own_session() -> Option<u32> {
        let mut session = 0u32;
        unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.ok()?;
        Some(session)
    }

    fn own_user() -> Option<String> {
        unsafe { process_user(GetCurrentProcess()) }
    }

    /// The person who may answer: the user logged on in this session (the
    /// console, for the service's agent); for an agent not running as
    /// SYSTEM (no right to ask), its own user.
    fn answerer() -> Option<String> {
        let session = own_session()?;
        let mut token = HANDLE::default();
        if unsafe { WTSQueryUserToken(session, &mut token) }.is_ok() {
            let token = Owned(token);
            return unsafe { token_user(token.0) };
        }
        if running_as_local_system() {
            None // nobody logged on (the logon screen)
        } else {
            own_user()
        }
    }

    /// The image path of process `pid`, if this process may see it.
    fn image_of(pid: u32) -> Option<PathBuf> {
        unsafe {
            let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
            let mut buf = vec![0u16; 32768];
            let mut len = buf.len() as u32;
            QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
            Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
        }
    }

    /// A security descriptor from SDDL (freed with LocalFree).
    struct Descriptor(PSECURITY_DESCRIPTOR);
    impl Descriptor {
        fn new(sddl: &str) -> io::Result<Self> {
            let wide = wide_z(sddl);
            let mut sd = PSECURITY_DESCRIPTOR::default();
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(wide.as_ptr()), SDDL_REVISION_1, &mut sd, None)
            }
            .map_err(os_err)?;
            Ok(Self(sd))
        }
    }
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0 .0)));
            }
        }
    }

    fn sddl_for(answerer: Option<&str>) -> io::Result<String> {
        let as_system = running_as_local_system();
        let own = if as_system { String::new() } else { own_user().ok_or_else(|| io::Error::other("no user of this process"))? };
        Ok(pipe_sddl(as_system, &own, answerer))
    }

    /// A new instance of the pipe; `first`: none may exist yet (someone
    /// else holding the name is an error, not a pipe shared with them).
    fn create(first: bool, answerer: Option<&str>) -> io::Result<NamedPipeServer> {
        let sd = Descriptor::new(&sddl_for(answerer)?)?;
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0 .0,
            bInheritHandle: false.into(),
        };
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                // So `allow` can change who may open it.
                .write_dac(true)
                // The one answering and the next one, told to go away.
                .max_instances(8)
                .create_with_security_attributes_raw(PIPE_NAME, &mut sa as *mut _ as *mut c_void)
        }
    }

    /// Lets `answerer` (only) open the pipe from now on.
    fn allow(server: &NamedPipeServer, answerer: Option<&str>) -> io::Result<()> {
        let sd = Descriptor::new(&sddl_for(answerer)?)?;
        unsafe {
            let mut present = windows::core::BOOL::default();
            let mut defaulted = windows::core::BOOL::default();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            GetSecurityDescriptorDacl(sd.0, &mut present, &mut dacl, &mut defaulted).map_err(os_err)?;
            let r = SetSecurityInfo(
                HANDLE(server.as_raw_handle() as *mut c_void),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl as *const ACL),
                None,
            );
            if r != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(r.0 as i32));
            }
        }
        Ok(())
    }

    /// Whether the client connected to `server` is the helper of the person
    /// who may answer: in this session, as `answerer`, run from this
    /// program's folder.
    fn client_allowed(server: &NamedPipeServer, answerer: Option<&str>) -> Result<(), String> {
        let handle = HANDLE(server.as_raw_handle() as *mut c_void);
        let answerer = answerer.ok_or("nobody is logged on")?;
        let mut session = 0u32;
        unsafe { GetNamedPipeClientSessionId(handle, &mut session) }.map_err(|e| e.to_string())?;
        if Some(session) != own_session() {
            return Err(format!("client in session {session}, not this one"));
        }
        let mut pid = 0u32;
        unsafe { GetNamedPipeClientProcessId(handle, &mut pid) }.map_err(|e| e.to_string())?;
        let user = unsafe {
            let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(|e| e.to_string())?);
            process_user(process.0)
        };
        if user.as_deref() != Some(answerer) {
            return Err("client runs as another user".into());
        }
        let image = image_of(pid).ok_or("cannot see the client's program")?;
        let own = std::env::current_exe().map_err(|e| e.to_string())?;
        if !same_folder(&image, &own) {
            return Err(format!("client program {} is not in this program's folder", image.display()));
        }
        Ok(())
    }

    pub async fn listen(answerer_state: Arc<dyn Answerer>) -> io::Result<()> {
        // Created at once (also before anyone logs on, then for SYSTEM
        // only) so the name is the agent's; someone else holding it is
        // told, and tried again.
        let mut allowed = answerer();
        let mut server = loop {
            match create(true, allowed.as_deref()) {
                Ok(s) => break s,
                Err(e) => {
                    warn!("Approval helper: cannot create {} ({}); trying again", PIPE_NAME, e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    allowed = answerer();
                }
            }
        };
        info!("Approval helper: waiting on {}", PIPE_NAME);
        let busy = Arc::new(AtomicBool::new(false));
        loop {
            tokio::select! {
                r = server.connect() => r?,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    // Someone logged on (or another user did).
                    let now = answerer();
                    if now != allowed {
                        match allow(&server, now.as_deref()) {
                            Ok(()) => allowed = now,
                            Err(e) => warn!("Approval helper: cannot change who may answer: {}", e),
                        }
                    }
                    continue;
                }
            }
            // The next instance first: the name stays the agent's.
            // A client (allowed or not) may hold an instance meanwhile: a
            // failure here waits and tries again instead of ending the
            // listener for good.
            let next = loop {
                match create(false, allowed.as_deref()) {
                    Ok(next) => break next,
                    Err(e) => {
                        warn!("Approval helper: cannot open the next pipe instance ({}); trying again", e);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            };
            let connected = std::mem::replace(&mut server, next);
            if let Err(e) = client_allowed(&connected, allowed.as_deref()) {
                warn!("Approval helper: refused a client ({})", e);
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            if busy.swap(true, Ordering::SeqCst) {
                // Another helper answers already.
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            let busy = busy.clone();
            let state = answerer_state.clone();
            tokio::spawn(async move {
                info!("Approval helper connected: the person here is asked");
                let (r, w) = tokio::io::split(connected);
                if let Err(e) = serve_answerer(r, w, &*state).await {
                    warn!("Approval helper: {}", e);
                }
                info!("Approval helper gone: only the password lets anyone in");
                busy.store(false, Ordering::SeqCst);
            });
        }
    }

    /// Whether the server end of `handle` is the agent: served from this
    /// session, the pipe owned by SYSTEM, Administrators or this user, and,
    /// when this process may see it, run from this program's folder.
    fn server_trusted(handle: HANDLE) -> Result<(), String> {
        let mut session = 0u32;
        unsafe { GetNamedPipeServerSessionId(handle, &mut session) }.map_err(|e| e.to_string())?;
        if Some(session) != own_session() {
            return Err(format!("served from session {session}, not this one"));
        }
        let owner = unsafe {
            let mut owner = PSID::default();
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let r = GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                None,
                None,
                Some(&mut sd),
            );
            if r != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(r.0 as i32).to_string());
            }
            let s = sid_string(owner);
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            s.ok_or("no owner")?
        };
        // LocalSystem, BUILTIN\Administrators, or this user.
        if owner != "S-1-5-18" && owner != "S-1-5-32-544" && Some(&owner) != own_user().as_ref() {
            return Err(format!("the pipe belongs to {owner}"));
        }
        let mut pid = 0u32;
        unsafe { GetNamedPipeServerProcessId(handle, &mut pid) }.map_err(|e| e.to_string())?;
        // A SYSTEM process is usually not open to this user: then the owner
        // check is what counts.
        if let (Some(image), Ok(own)) = (image_of(pid), std::env::current_exe()) {
            if !same_folder(&image, &own) {
                return Err(format!("served by {}", image.display()));
            }
        }
        Ok(())
    }

    pub fn connect() -> io::Result<NamedPipeClient> {
        let name = wide_z(PIPE_NAME);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                CLIENT_ACCESS,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0 & 0xFFFF))?;
        if let Err(e) = server_trusted(handle) {
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("not the agent's pipe: {e}")));
        }
        // Takes the handle over (closed on error too).
        unsafe { NamedPipeClient::from_raw_handle(handle.0 as RawHandle) }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::duplex;

    type Sink = Box<dyn Fn(Event) + Send + Sync>;

    /// The agent's question state, as small as can be: one sink, the
    /// answers given.
    #[derive(Default)]
    struct Questions {
        sink: Mutex<Option<(u64, Sink)>>,
        next: AtomicU64,
        answers: Mutex<Vec<(u64, bool)>>,
    }

    impl Questions {
        fn attended(&self) -> bool {
            self.sink.lock().unwrap().is_some()
        }
        fn emit(&self, event: Event) {
            if let Some((_, sink)) = self.sink.lock().unwrap().as_ref() {
                sink(event);
            }
        }
        async fn wait_attended(&self, attended: bool) {
            while self.attended() != attended {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        async fn wait_answer(&self) -> (u64, bool) {
            loop {
                if let Some(a) = self.answers.lock().unwrap().pop() {
                    return a;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
    }

    impl Answerer for Questions {
        fn attach(&self, sink: Sink) -> u64 {
            let token = self.next.fetch_add(1, Ordering::SeqCst) + 1;
            *self.sink.lock().unwrap() = Some((token, sink));
            token
        }
        fn answer(&self, id: u64, accept: bool) {
            self.answers.lock().unwrap().push((id, accept));
        }
        fn detach(&self, token: u64) {
            let mut sink = self.sink.lock().unwrap();
            if sink.as_ref().map(|(t, _)| *t) == Some(token) {
                *sink = None;
            }
        }
    }

    fn question(id: u64, kind: &'static str) -> Question {
        Question { id, requester: "huy (direct)".into(), kind, timeout_s: 30 }
    }

    #[test]
    fn lines_round_trip() {
        let q = Question { id: 4, requester: "a \"b\"\n".into(), kind: "files", timeout_s: 30 };
        let line = event_line(&Event::Ask(q.clone()));
        assert!(!line.contains('\n'));
        assert_eq!(line, r#"@lunaris-approval {"id":4,"requester":"a \"b\"\n","kind":"files","timeout_s":30}"#);
        assert_eq!(parse_event_line(&line), Some(Event::Ask(q)));
        assert_eq!(event_line(&Event::Cancel(4)), r#"@lunaris-approval-cancel {"id":4}"#);
        assert_eq!(parse_event_line(r#"@lunaris-approval-cancel {"id":4}"#), Some(Event::Cancel(4)));
        assert_eq!(parse_event_line("hello"), None);
        assert_eq!(parse_event_line("@lunaris-approval {}"), None);
        assert_eq!(answer_line(9, false), r#"{"id":9,"accept":false}"#);
        assert_eq!(parse_answer_line(" {\"id\":9,\"accept\":true}\r"), Ok(Some((9, true))));
        assert_eq!(parse_answer_line("  "), Ok(None));
        assert!(parse_answer_line("{\"id\":9}").is_err());
    }

    #[tokio::test]
    async fn capped_lines() {
        let data: &[u8] = b"one\r\ntwo\n\nlast";
        let mut r = BufReader::new(data);
        assert_eq!(read_line_capped(&mut r, 10).await.unwrap().as_deref(), Some("one"));
        assert_eq!(read_line_capped(&mut r, 10).await.unwrap().as_deref(), Some("two"));
        assert_eq!(read_line_capped(&mut r, 10).await.unwrap().as_deref(), Some(""));
        assert_eq!(read_line_capped(&mut r, 10).await.unwrap().as_deref(), Some("last"));
        assert_eq!(read_line_capped(&mut r, 10).await.unwrap(), None);
        let long = vec![b'x'; 100];
        let mut r = BufReader::with_capacity(8, &long[..]);
        assert!(read_line_capped(&mut r, 50).await.is_err());
    }

    /// Both sides over an in-memory pair: the agent's questions reach the
    /// helper, its answers come back, and its leaving detaches it.
    #[tokio::test]
    async fn agent_and_helper_over_a_pair() {
        let questions = Arc::new(Questions::default());
        let (agent_end, helper_end) = duplex(4096);
        let (ar, aw) = tokio::io::split(agent_end);
        let q2 = questions.clone();
        let served = tokio::spawn(async move { serve_answerer(ar, aw, &*q2).await });

        let (hr, hw) = tokio::io::split(helper_end);
        let (atx, mut arx) = mpsc::unbounded_channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let helper = tokio::spawn(async move {
            run_answerer(hr, hw, move || { let _ = ready_tx.send(()); }, move |e| s.lock().unwrap().push(e), &mut arx).await
        });
        ready_rx.await.unwrap();
        questions.wait_attended(true).await;

        questions.emit(Event::Ask(question(1, "control")));
        questions.emit(Event::Ask(question(2, "files")));
        questions.emit(Event::Cancel(2));
        while seen.lock().unwrap().len() < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec![Event::Ask(question(1, "control")), Event::Ask(question(2, "files")), Event::Cancel(2)]
        );
        atx.send((1, true)).unwrap();
        assert_eq!(questions.wait_answer().await, (1, true));

        // The helper goes away (its answers' sender dropped): detached.
        drop(atx);
        helper.await.unwrap().unwrap();
        served.await.unwrap().unwrap();
        assert!(!questions.attended());
    }

    #[tokio::test]
    async fn agent_side_drops_a_helper_that_talks_nonsense() {
        let questions = Questions::default();
        let (agent_end, mut helper_end) = duplex(4096);
        let (ar, aw) = tokio::io::split(agent_end);
        helper_end.write_all(b"\n{\"id\":3,\"accept\":false}\nhello\n").await.unwrap();
        assert!(serve_answerer(ar, aw, &questions).await.is_err());
        assert_eq!(*questions.answers.lock().unwrap(), vec![(3, false)]);
        assert!(!questions.attended());
    }

    /// A helper ending leaves a newer one attached.
    #[tokio::test]
    async fn old_helper_does_not_detach_a_newer_one() {
        let questions = Arc::new(Questions::default());
        let (agent_end, helper_end) = duplex(4096);
        let (ar, aw) = tokio::io::split(agent_end);
        let q2 = questions.clone();
        let served = tokio::spawn(async move { serve_answerer(ar, aw, &*q2).await });
        questions.wait_attended(true).await;
        let newer = questions.attach(Box::new(|_| {}));
        drop(helper_end);
        served.await.unwrap().unwrap();
        assert!(questions.attended());
        questions.detach(newer);
        assert!(!questions.attended());
    }

    /// A helper turned away (the channel closes at once) or talking to
    /// something else is told so, and never ready.
    #[tokio::test]
    async fn helper_side_turned_away() {
        for (sent, kind) in [(&b""[..], io::ErrorKind::NotConnected), (&b"hello\n"[..], io::ErrorKind::InvalidData)] {
            let (_atx, mut arx) = mpsc::unbounded_channel();
            let r = run_answerer(sent, tokio::io::sink(), || panic!("ready"), |_| {}, &mut arx).await;
            assert_eq!(r.unwrap_err().kind(), kind);
        }
        // An agent that is ready, then sends something else.
        let (_atx, mut arx) = mpsc::unbounded_channel();
        let sent = format!("{READY_LINE}\nnonsense\n");
        let r = run_answerer(sent.as_bytes(), tokio::io::sink(), || {}, |_| {}, &mut arx).await;
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn folders() {
        use std::path::Path;
        assert!(same_folder(
            Path::new(r"C:\Program Files\Lunaris Agent\lunaris-agent.exe"),
            Path::new(r"\\?\c:\program files\lunaris agent\Lunaris-Approval-Helper.exe")
        ));
        assert!(same_folder(Path::new("/opt/lunaris/agent"), Path::new("/opt/lunaris/other")));
        assert!(!same_folder(
            Path::new(r"C:\Users\x\Downloads\lunaris-approval-helper.exe"),
            Path::new(r"C:\Program Files\Lunaris Agent\lunaris-agent.exe")
        ));
        assert!(!same_folder(Path::new("agent"), Path::new("agent")));
    }

    #[test]
    fn pipe_permissions() {
        // The service's agent: SYSTEM owns it, the console user reads and
        // writes, and cannot create instances (0x4).
        assert_eq!(pipe_sddl(true, "", None), "O:SYD:P(A;;FA;;;SY)");
        assert_eq!(pipe_sddl(true, "", Some("S-1-5-21-1-2-3-1001")), "O:SYD:P(A;;FA;;;SY)(A;;0x120083;;;S-1-5-21-1-2-3-1001)");
        assert_eq!(CLIENT_ACCESS & 0x4, 0);
        // An agent run by a user: that user has it all.
        assert_eq!(pipe_sddl(false, "S-1-5-21-9", Some("S-1-5-21-9")), "D:P(A;;FA;;;SY)(A;;FA;;;S-1-5-21-9)");
        assert_eq!(
            pipe_sddl(false, "S-1-5-21-9", Some("S-1-5-21-8")),
            "D:P(A;;FA;;;SY)(A;;FA;;;S-1-5-21-9)(A;;0x120083;;;S-1-5-21-8)"
        );
    }

    #[cfg(target_os = "linux")]
    mod unix_socket {
        use super::super::unix;
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::{AsyncBufReadExt, AsyncReadExt};

        fn tmp(name: &str) -> std::path::PathBuf {
            let dir = std::env::temp_dir().join(format!("lunaris-approval-test-{}-{}", std::process::id(), name));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn uid() -> u32 {
            unsafe { libc::geteuid() }
        }

        #[tokio::test]
        async fn socket_is_private_and_replaces_a_stale_one() {
            let base = tmp("perm");
            let path = base.join("lunaris").join("approval.sock");
            let listener = unix::bind(&path).await.unwrap();
            let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(path.parent().unwrap()), 0o700);
            assert_eq!(mode(&path), 0o600);
            // A second agent leaves the live socket alone.
            assert_eq!(unix::bind(&path).await.unwrap_err().kind(), io::ErrorKind::AddrInUse);
            // A stale one (its agent gone) is replaced.
            drop(listener);
            let _again = unix::bind(&path).await.unwrap();
            // A folder that is a link is refused.
            let link = base.join("link");
            std::os::unix::fs::symlink(&base, &link).unwrap();
            assert!(unix::prepare_dir(&link).is_err());
            std::fs::remove_dir_all(&base).unwrap();
        }

        #[tokio::test]
        async fn peer_checks() {
            let base = tmp("peer");
            let path = base.join("lunaris").join("approval.sock");
            let questions = Arc::new(Questions::default());
            let mut buf = [0u8; 1];

            // Served for another user: this one is dropped at once.
            let listener = unix::bind(&path).await.unwrap();
            let server = tokio::spawn(unix::serve(listener, uid().wrapping_add(1), questions.clone()));
            let mut stream = unix::connect_to(&path, uid()).await.unwrap();
            assert_eq!(stream.read(&mut buf).await.unwrap(), 0);
            assert!(!questions.attended());
            server.abort();

            let _ = std::fs::remove_file(&path);
            let listener = unix::bind(&path).await.unwrap();
            let server = tokio::spawn(unix::serve(listener, uid(), questions.clone()));
            // The same user: answers; a second helper meanwhile is dropped.
            let mut first = unix::connect_to(&path, uid()).await.unwrap();
            let mut ready = vec![0u8; READY_LINE.len() + 1];
            first.read_exact(&mut ready).await.unwrap();
            assert_eq!(ready, format!("{READY_LINE}\n").into_bytes());
            questions.wait_attended(true).await;
            let mut second = unix::connect_to(&path, uid()).await.unwrap();
            assert_eq!(second.read(&mut buf).await.unwrap(), 0);
            assert!(questions.attended());
            questions.emit(Event::Ask(question(5, "control")));
            let (fr, mut fw) = first.into_split();
            let mut lines = BufReader::new(fr).lines();
            assert_eq!(parse_event_line(&lines.next_line().await.unwrap().unwrap()), Some(Event::Ask(question(5, "control"))));
            fw.write_all(format!("{}\n", answer_line(5, true)).as_bytes()).await.unwrap();
            assert_eq!(questions.wait_answer().await, (5, true));
            drop(fw);
            drop(lines);
            questions.wait_attended(false).await;
            // A helper expecting the agent to run as another user refuses.
            assert_eq!(
                unix::connect_to(&path, uid().wrapping_add(1)).await.unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
            server.abort();
            std::fs::remove_dir_all(&base).unwrap();
        }
    }
}
