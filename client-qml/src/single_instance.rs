//! One running app per user: a second copy (a `lunaris://` link opened while
//! the app runs) hands its link to the running one and exits.
//!
//! The channel is private to the user, because a link carries a one-time
//! ticket worth a session for 60 s:
//!
//! * Linux / macOS: a Unix socket in `$XDG_RUNTIME_DIR` (0700, the user's),
//!   or in `~/.lunaris/run` (0700) when that is not set. Both ends check the
//!   other's user id (`SO_PEERCRED` / `getpeereid`) before anything is sent
//!   or acted on.
//! * Windows: TCP on 127.0.0.1 with a mutual challenge-response on a random
//!   key only this user can read (`~/.lunaris/instance.key`, in the user
//!   profile). The copy with the link sends it only after the listener has
//!   proven it knows the key, and the listener acts only on a message whose
//!   proof checks.
//!
//! When the other end fails the check the copy runs on its own: it never
//! sends the link to it.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a peer may take to answer.
const IO_TIMEOUT: Duration = Duration::from_secs(2);
/// A message is a `lunaris://` link at most (deeplink::MAX_URL_LEN) plus the word before it.
const MAX_LINE: usize = 16 * 1024;

/// Reads one `\n`-ended line of at most `MAX_LINE` bytes.
fn read_line<R: Read>(r: &mut BufReader<R>) -> std::io::Result<String> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 || buf.last() != Some(&b'\n') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "no complete line"));
    }
    buf.pop();
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "not UTF-8"))
}

// ───────────────────────── Unix: socket + peer uid ─────────────────────────

#[cfg(unix)]
pub mod unix {
    use super::*;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    pub fn my_uid() -> u32 {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() as u32 }
    }

    /// The user id of the process at the other end of `s`.
    pub fn peer_uid(s: &UnixStream) -> std::io::Result<u32> {
        let fd = s.as_raw_fd();
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            // SAFETY: cred/len are valid for writes of the sizes given.
            let r = unsafe {
                libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len)
            };
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(cred.uid as u32)
        }
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let mut uid: libc::uid_t = 0;
            let mut gid: libc::gid_t = 0;
            // SAFETY: uid/gid are valid for writes.
            if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(uid as u32)
        }
    }

    /// A directory only `uid` can use: owned by it, nobody else may write
    /// to or list it.
    fn private_dir_of(dir: &Path, uid: u32) -> bool {
        std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o077 == 0)
    }

    /// Where the socket is: `$XDG_RUNTIME_DIR/lunaris-client.sock` when that
    /// directory is the user's own, else `<app dir>/run/instance.sock`.
    pub fn socket_path(app_dir: &Path) -> Option<PathBuf> {
        let uid = my_uid();
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
            if runtime.is_absolute() && private_dir_of(&runtime, uid) {
                return Some(runtime.join("lunaris-client.sock"));
            }
        }
        let run = app_dir.join("run");
        crate::secure_fs::create_private_dir(&run).ok()?;
        private_dir_of(&run, uid).then(|| run.join("instance.sock"))
    }

    /// Sends `message` (one line) to the instance listening at `path`, if
    /// one runs there and it is `expected_uid`'s. Ok(true): sent. Ok(false):
    /// nobody listens. Err: someone listens but is not to be trusted (the
    /// message is not sent).
    pub fn send(path: &Path, message: &str, expected_uid: u32) -> std::io::Result<bool> {
        let mut s = match UnixStream::connect(path) {
            Ok(s) => s,
            Err(_) => return Ok(false),
        };
        let peer = peer_uid(&s)?;
        if peer != expected_uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("the listener is user {peer}, not this user"),
            ));
        }
        s.set_write_timeout(Some(IO_TIMEOUT))?;
        s.write_all(message.trim_end().as_bytes())?;
        s.write_all(b"\n")?;
        s.flush()?;
        Ok(true)
    }

    /// Listens at `path`; a stale socket (nobody listens) is replaced.
    pub fn bind(path: &Path) -> std::io::Result<UnixListener> {
        match UnixListener::bind(path) {
            Ok(l) => Ok(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                // Our own leftover socket only: never another kind of file.
                let m = std::fs::symlink_metadata(path)?;
                if !m.file_type().is_socket() || m.uid() != my_uid() {
                    return Err(e);
                }
                if UnixStream::connect(path).is_ok() {
                    return Err(e); // someone listens after all
                }
                std::fs::remove_file(path)?;
                UnixListener::bind(path)
            }
            Err(e) => Err(e),
        }
    }

    /// The message of one accepted connection, if its peer is `expected_uid`.
    pub fn receive(s: UnixStream, expected_uid: u32) -> std::io::Result<String> {
        let peer = peer_uid(&s)?;
        if peer != expected_uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("a connection from user {peer} was refused"),
            ));
        }
        s.set_read_timeout(Some(IO_TIMEOUT))?;
        read_line(&mut BufReader::new(s))
    }
}

// ─────────────── Windows (and tests): TCP + challenge-response ───────────────

/// Mutual proof of a shared per-user key over any byte stream.
#[cfg(any(windows, test))]
pub mod keyed {
    use super::*;
    use ring::hmac;
    use ring::rand::{SecureRandom, SystemRandom};

    const HELLO: &str = "LUNARIS-INSTANCE1";
    pub const KEY_FILE: &str = "instance.key";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn unhex(s: &str) -> Option<Vec<u8>> {
        if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
    }

    fn random32() -> std::io::Result<[u8; 32]> {
        let mut b = [0u8; 32];
        SystemRandom::new().fill(&mut b).map_err(|_| std::io::Error::other("no random source"))?;
        Ok(b)
    }

    fn bad(what: &str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, what.to_string())
    }

    /// The user's key in `dir/instance.key`, made on first use (only this
    /// user can read the folder: unix 0600 / Windows profile ACL).
    pub fn load_or_create_key(dir: &Path) -> std::io::Result<Vec<u8>> {
        crate::secure_fs::create_private_dir(dir)?;
        let path = dir.join(KEY_FILE);
        let parse = |data: String| unhex(data.trim()).filter(|k| k.len() == 32);
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Some(k) = parse(data) {
                return Ok(k);
            }
        }
        let key = hex(&random32()?);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        match o.open(&path) {
            Ok(mut f) => {
                f.write_all(key.as_bytes())?;
                f.sync_all()?;
            }
            // Another copy made it at the same moment: theirs counts.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(k) = std::fs::read_to_string(&path).ok().and_then(parse) {
                    return Ok(k);
                }
                // A broken file: replaced.
                crate::secure_fs::write_private(&path, &key)?;
            }
            Err(e) => return Err(e),
        }
        parse(key).ok_or_else(|| std::io::Error::other("key"))
    }

    fn tag(key: &[u8], who: &[u8], cn: &[u8], sn: &[u8], message: &[u8]) -> hmac::Tag {
        let k = hmac::Key::new(hmac::HMAC_SHA256, key);
        let mut ctx = hmac::Context::with_key(&k);
        for part in [who, cn, sn, message] {
            ctx.update(&(part.len() as u32).to_be_bytes());
            ctx.update(part);
        }
        ctx.sign()
    }

    fn check(key: &[u8], who: &[u8], cn: &[u8], sn: &[u8], message: &[u8], proof: &str) -> bool {
        let Some(proof) = unhex(proof) else { return false };
        let k = hmac::Key::new(hmac::HMAC_SHA256, key);
        let mut data = Vec::new();
        for part in [who, cn, sn, message] {
            data.extend_from_slice(&(part.len() as u32).to_be_bytes());
            data.extend_from_slice(part);
        }
        hmac::verify(&k, &data, &proof).is_ok()
    }

    /// The copy with the link: hello with a nonce, check the listener's
    /// proof, and only then send `message` with our own proof.
    pub fn client<S: Read + Write>(stream: S, key: &[u8], message: &str) -> std::io::Result<()> {
        let mut r = BufReader::new(stream);
        let cn = random32()?;
        r.get_mut().write_all(format!("{HELLO} {}\n", hex(&cn)).as_bytes())?;
        r.get_mut().flush()?;
        let line = read_line(&mut r)?;
        let (sn_hex, proof) = line.split_once(' ').ok_or_else(|| bad("not a Lunaris app"))?;
        let sn = unhex(sn_hex).filter(|n| n.len() == 32).ok_or_else(|| bad("not a Lunaris app"))?;
        if !check(key, b"server", &cn, &sn, b"", proof) {
            return Err(bad("the listener does not know this user's key"));
        }
        let message = message.trim_end();
        let mine = tag(key, b"client", &cn, &sn, message.as_bytes());
        r.get_mut().write_all(format!("{} {}\n", hex(mine.as_ref()), message).as_bytes())?;
        r.get_mut().flush()
    }

    /// The running app: answer the hello with our proof, then take the
    /// message only if its proof checks.
    pub fn server<S: Read + Write>(stream: S, key: &[u8]) -> std::io::Result<String> {
        let mut r = BufReader::new(stream);
        let hello = read_line(&mut r)?;
        let cn = hello
            .strip_prefix(HELLO)
            .and_then(|rest| rest.strip_prefix(' '))
            .and_then(unhex)
            .filter(|n| n.len() == 32)
            .ok_or_else(|| bad("not a Lunaris hello"))?;
        let sn = random32()?;
        let proof = tag(key, b"server", &cn, &sn, b"");
        r.get_mut().write_all(format!("{} {}\n", hex(&sn), hex(proof.as_ref())).as_bytes())?;
        r.get_mut().flush()?;
        let line = read_line(&mut r)?;
        let (proof, message) = line.split_once(' ').ok_or_else(|| bad("no proof"))?;
        if !check(key, b"client", &cn, &sn, message.as_bytes(), proof) {
            return Err(bad("the sender does not know this user's key"));
        }
        Ok(message.to_string())
    }
}

// ─────────────────────────────── The app ───────────────────────────────

/// Sends `message` to the running instance. Ok(true): it took it (this copy
/// exits). Ok(false): none runs. Err: something listens that is not this
/// user's app (this copy runs on its own; nothing was sent).
pub fn send_to_running(app_dir: &Path, message: &str) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        let path = unix::socket_path(app_dir).ok_or_else(|| std::io::Error::other("no private folder for the socket"))?;
        unix::send(&path, message, unix::my_uid())
    }
    #[cfg(windows)]
    {
        use std::net::{SocketAddr, TcpStream};
        let addr: SocketAddr = ([127, 0, 0, 1], WINDOWS_PORT).into();
        let s = match TcpStream::connect_timeout(&addr, IO_TIMEOUT) {
            Ok(s) => s,
            Err(_) => return Ok(false),
        };
        s.set_read_timeout(Some(IO_TIMEOUT))?;
        s.set_write_timeout(Some(IO_TIMEOUT))?;
        let key = keyed::load_or_create_key(app_dir)?;
        keyed::client(s, &key, message).map(|_| true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (app_dir, message);
        Ok(false)
    }
}

#[cfg(windows)]
const WINDOWS_PORT: u16 = 28435;

/// Listens for the next copies' messages on its own thread; `on_message`
/// gets each one whose sender passed the check.
pub fn listen(app_dir: &Path, on_message: impl Fn(&str) + Send + 'static) {
    #[cfg(unix)]
    {
        let Some(path) = unix::socket_path(app_dir) else {
            tracing::error!("Single-instance: no private folder for the socket; links open new windows");
            return;
        };
        let listener = match unix::bind(&path) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Single-instance: cannot listen at {}: {}", path.display(), e);
                return;
            }
        };
        let uid = unix::my_uid();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                match unix::receive(stream, uid) {
                    Ok(msg) => on_message(msg.trim()),
                    Err(e) => tracing::warn!("Single-instance: ignored a message: {}", e),
                }
            }
        });
    }
    #[cfg(windows)]
    {
        let key = match keyed::load_or_create_key(app_dir) {
            Ok(k) => k,
            Err(e) => {
                tracing::error!("Single-instance: no key ({}); links open new windows", e);
                return;
            }
        };
        let listener = match std::net::TcpListener::bind(("127.0.0.1", WINDOWS_PORT)) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Single-instance: cannot listen: {:?}", e);
                return;
            }
        };
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                match keyed::server(stream, &key) {
                    Ok(msg) => on_message(msg.trim()),
                    Err(e) => tracing::warn!("Single-instance: ignored a message: {}", e),
                }
            }
        });
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (app_dir, on_message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    const LINK: &str = "CONNECT lunaris://connect?host_id=h&ticket=lct_secret&server=wss%3A%2F%2Fs";

    fn temp_dir(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let d = std::env::temp_dir().join(format!("lunaris-si-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The keyed exchange over a real local TCP connection: (what the client
    /// got, what the server got, every byte the server side received).
    fn exchange(client_key: Vec<u8>, server_key: Vec<u8>) -> (std::io::Result<()>, std::io::Result<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
            keyed::server(s, &server_key)
        });
        let c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
        let got = keyed::client(c, &client_key, LINK);
        (got, server.join().unwrap())
    }

    #[test]
    fn keyed_good_peer_receives_the_link() {
        let key = vec![7u8; 32];
        let (c, s) = exchange(key.clone(), key);
        c.unwrap();
        assert_eq!(s.unwrap(), LINK);
    }

    #[test]
    fn keyed_listener_without_the_key_never_receives_the_link() {
        // An impostor on the port: speaks the protocol with another key and
        // records everything that reaches it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let spy = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            // The hello, then a fake proof.
            let n = s.read(&mut buf).unwrap();
            seen.extend_from_slice(&buf[..n]);
            s.write_all(format!("{} {}\n", "ab".repeat(32), "cd".repeat(32)).as_bytes()).unwrap();
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            String::from_utf8_lossy(&seen).into_owned()
        });
        let c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
        assert!(keyed::client(c, &[1u8; 32], LINK).is_err());
        let seen = spy.join().unwrap();
        assert!(!seen.contains("lct_"), "the ticket leaked: {seen}");
        assert!(!seen.contains("CONNECT"), "the link leaked: {seen}");

        // Same with a real server that has another key.
        let (c, s) = exchange(vec![1u8; 32], vec![2u8; 32]);
        assert!(c.is_err());
        assert!(s.is_err());
    }

    #[test]
    fn keyed_sender_without_the_key_is_ignored() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
            keyed::server(s, &[9u8; 32])
        });
        // The old protocol (a bare line) and a forged proof are both refused.
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(format!("{LINK}\n").as_bytes()).unwrap();
        assert!(server.join().unwrap().is_err());

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
            keyed::server(s, &[9u8; 32])
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(format!("LUNARIS-INSTANCE1 {}\n", "00".repeat(32)).as_bytes()).unwrap();
        let mut line = String::new();
        BufReader::new(c.try_clone().unwrap()).read_line(&mut line).unwrap();
        c.write_all(format!("{} {LINK}\n", "00".repeat(32)).as_bytes()).unwrap();
        assert!(server.join().unwrap().is_err());
    }

    #[test]
    fn keyed_key_is_made_once_and_private() {
        let dir = temp_dir("key");
        let a = keyed::load_or_create_key(&dir).unwrap();
        let b = keyed::load_or_create_key(&dir).unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(a, b);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(keyed::KEY_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A broken key file is replaced.
        std::fs::write(dir.join(keyed::KEY_FILE), "junk").unwrap();
        let c = keyed::load_or_create_key(&dir).unwrap();
        assert_eq!(c.len(), 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_good_peer_receives_the_link() {
        let dir = temp_dir("sock");
        let path = dir.join("i.sock");
        let listener = unix::bind(&path).unwrap();
        let me = unix::my_uid();
        let server = std::thread::spawn(move || unix::receive(listener.accept().unwrap().0, me));
        assert!(unix::send(&path, LINK, me).unwrap());
        assert_eq!(server.join().unwrap().unwrap(), LINK);
        // Nobody listens any more: not sent, and the stale socket is replaced.
        assert!(!unix::send(&dir.join("none.sock"), LINK, me).unwrap());
        let _again = unix::bind(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_wrong_user_never_receives_the_link() {
        let dir = temp_dir("sockuid");
        let path = dir.join("i.sock");
        let listener = unix::bind(&path).unwrap();
        let me = unix::my_uid();
        let other = me.wrapping_add(1);
        // The sender expects another user than the listener's: it refuses
        // before writing anything.
        let spy = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
            let mut seen = Vec::new();
            let _ = s.read_to_end(&mut seen);
            (String::from_utf8_lossy(&seen).into_owned(), listener)
        });
        assert!(unix::send(&path, LINK, other).is_err());
        let (seen, listener) = spy.join().unwrap();
        assert!(seen.is_empty(), "sent to the wrong user: {seen}");
        // The listener refuses a sender that is not its user.
        let server = std::thread::spawn(move || unix::receive(listener.accept().unwrap().0, other));
        assert!(unix::send(&path, LINK, me).unwrap());
        assert!(server.join().unwrap().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_bind_never_removes_a_file_that_is_not_a_socket() {
        let dir = temp_dir("sockfile");
        let path = dir.join("i.sock");
        std::fs::write(&path, b"data").unwrap();
        assert!(unix::bind(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"data");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_lives_in_a_private_folder() {
        let dir = temp_dir("sockpath");
        let p = unix::socket_path(&dir).unwrap();
        let parent = p.parent().unwrap();
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let m = std::fs::metadata(parent).unwrap();
        assert_eq!(m.permissions().mode() & 0o077, 0, "{}", parent.display());
        assert_eq!(m.uid(), unix::my_uid());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
