//! Tests of the listener's file host, spoken to as a RustDesk client speaks
//! (`src/client.rs`, `src/client/io_loop.rs`): the real listener on a
//! loopback port, its files a folder made for the test. The connector's
//! file client runs its own tests against the same listener
//! (`connector/rustdesk_files_tests.rs`, [`start_listener`]).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use bytes::BytesMut;
use prost::Message as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::approval::{ApproveMode, Decision, Kind};
use crate::rustdesk::proto::{
    self, login_request, login_response, misc, permission_info::Permission, FileTransferSendRequest, LoginRequest, Misc, PeerInfo,
};
use crate::rustdesk::clipboard::ClipboardLink;
use crate::rustdesk::session::{Asker, Codec, Display, Screen, Video, NO_FILE_PERMISSION};
use crate::rustdesk::{frame, login, serve_listener, RustdeskConfig};

// ---------------------------------------------------------- the listener

/// A computer without a screen: a file-transfer connection asks for none.
struct NoScreen;

#[async_trait::async_trait]
impl Screen for NoScreen {
    async fn displays(&self) -> anyhow::Result<Vec<Display>> {
        anyhow::bail!("no screen in this test")
    }

    async fn encodes_h265(&self, _display: &Display) -> bool {
        false
    }

    fn start(&self, _display: &Display, _codec: Codec, _bitrate_kbps: u32) -> Video {
        unreachable!("no picture in a file test")
    }

    fn injector(&self) -> Option<Box<dyn crate::rustdesk::input::Injector>> {
        None
    }

    fn clipboard(&self) -> Option<ClipboardLink> {
        None
    }
}

pub(crate) struct Listener {
    pub(crate) address: SocketAddr,
    /// Who the person here was asked about, and for what.
    pub(crate) asked: Arc<Mutex<Vec<(String, Kind)>>>,
}

/// The agent's RustDesk listener on a loopback port, with these files:
/// `access` (the agent's `file_access`) and the folder a client starts in.
/// The person here accepts whoever they are asked about.
pub(crate) async fn start_listener(password: &str, mode: &str, file_transfer: bool, access: Access, home: Option<PathBuf>) -> Listener {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let config = RustdeskConfig {
        port: address.port(),
        password: password.into(),
        approve_mode: ApproveMode::parse(mode),
        allow_clipboard: false,
        agent_id: "test-agent".into(),
        name: "máy-thử".into(),
        file_transfer,
        file_access: access,
        file_home: home,
    };
    let asked: Arc<Mutex<Vec<(String, Kind)>>> = Arc::default();
    let record = asked.clone();
    let asker: Asker = Arc::new(move |requester, kind, _withdrawn| {
        record.lock().unwrap().push((requester, kind));
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Some(Decision::Accepted)
        })
    });
    tokio::spawn(serve_listener(listener, config, Arc::new(NoScreen), asker, Arc::new(|| true), Arc::default()));
    Listener { address, asked }
}

// ------------------------------------------------------------ the client

const PASSWORD: &str = "mật khẩu";
const T0: u64 = 1_700_000_000;

/// What the host says on a file-transfer connection.
#[derive(Debug)]
enum Reply {
    Dir(FileDirectory),
    Block(FileTransferBlock),
    Error(FileTransferError),
    Done(FileTransferDone),
    Digest(FileTransferDigest),
    EmptyDirs(ReadEmptyDirsResponse),
    /// `FileAction.send_confirm`, the host's answer to an upload's digest.
    Confirm(FileTransferSendConfirmRequest),
    Closed,
}

struct Client {
    stream: TcpStream,
    decoder: frame::Decoder,
    buf: BytesMut,
    hash: proto::Hash,
    /// The permissions the host said it does not give.
    denied: Vec<i32>,
}

impl Client {
    /// Connects and reads the host's `Hash`.
    async fn connect(address: SocketAddr) -> Client {
        let stream = TcpStream::connect(address).await.unwrap();
        let mut client = Client { stream, decoder: frame::Decoder::new(usize::MAX), buf: BytesMut::new(), hash: Default::default(), denied: Vec::new() };
        match client.next().await.and_then(|m| m.union) {
            Some(message::Union::Hash(hash)) => client.hash = hash,
            other => panic!("expected Hash, got {:?}", other),
        }
        client
    }

    async fn send(&mut self, msg: Message) {
        let mut out = BytesMut::new();
        frame::put_header(msg.encoded_len(), &mut out).unwrap();
        msg.encode(&mut out).unwrap();
        self.stream.write_all(&out).await.unwrap();
    }

    /// The next message that is not a delay probe or a permission notice;
    /// `None` when the host closed the connection.
    async fn next(&mut self) -> Option<Message> {
        loop {
            let msg = loop {
                if let Some(payload) = self.decoder.decode(&mut self.buf).unwrap() {
                    break Message::decode(payload.freeze()).unwrap();
                }
                let read = tokio::time::timeout(Duration::from_secs(20), self.stream.read_buf(&mut self.buf)).await.expect("the host answers in time");
                if read.unwrap_or(0) == 0 {
                    return None;
                }
            };
            match &msg.union {
                Some(message::Union::TestDelay(_)) => {}
                Some(message::Union::Misc(Misc { union: Some(misc::Union::PermissionInfo(info)) })) => {
                    if !info.enabled {
                        self.denied.push(info.permission);
                    }
                }
                _ => return Some(msg),
            }
        }
    }

    /// client.rs `create_login_msg` for `ConnType::FILE_TRANSFER`.
    async fn login(&mut self, password: &str, dir: &str) -> Result<PeerInfo, String> {
        let proof = if password.is_empty() {
            bytes::Bytes::new()
        } else {
            bytes::Bytes::copy_from_slice(&login::proof(&login::h1(password, &self.hash.salt), &self.hash.challenge))
        };
        self.send(Message {
            union: Some(message::Union::LoginRequest(LoginRequest {
                username: "127.0.0.1".into(),
                password: proof,
                my_id: "123456789".into(),
                my_name: "Người thử".into(),
                my_platform: "Linux".into(),
                version: "1.5.3".into(),
                union: Some(login_request::Union::FileTransfer(proto::FileTransfer { dir: dir.into(), show_hidden: true })),
                ..Default::default()
            })),
        })
        .await;
        match self.next().await.and_then(|m| m.union) {
            Some(message::Union::LoginResponse(proto::LoginResponse { union: Some(login_response::Union::PeerInfo(info)), .. })) => Ok(info),
            Some(message::Union::LoginResponse(proto::LoginResponse { union: Some(login_response::Union::Error(error)), .. })) => Err(error),
            other => panic!("expected LoginResponse, got {:?}", other),
        }
    }

    /// Logged in, and past the listing the host sends by itself.
    async fn open(address: SocketAddr) -> (Client, FileDirectory) {
        let mut client = Client::connect(address).await;
        client.login(PASSWORD, "").await.expect("logged in");
        match client.reply().await {
            Reply::Dir(dir) => (client, dir),
            other => panic!("expected the first listing, got {:?}", other),
        }
    }

    async fn act(&mut self, union: file_action::Union) {
        self.send(Message { union: Some(message::Union::FileAction(FileAction { union: Some(union) })) }).await;
    }

    async fn respond(&mut self, union: file_response::Union) {
        self.send(response(union)).await;
    }

    async fn reply(&mut self) -> Reply {
        match self.next().await.map(|m| m.union) {
            None => Reply::Closed,
            Some(Some(message::Union::FileResponse(FileResponse { union: Some(union) }))) => match union {
                file_response::Union::Dir(dir) => Reply::Dir(dir),
                file_response::Union::Block(block) => Reply::Block(block),
                file_response::Union::Error(error) => Reply::Error(error),
                file_response::Union::Done(done) => Reply::Done(done),
                file_response::Union::Digest(digest) => Reply::Digest(digest),
                file_response::Union::EmptyDirs(dirs) => Reply::EmptyDirs(dirs),
            },
            Some(Some(message::Union::FileAction(FileAction { union: Some(file_action::Union::SendConfirm(confirm)) }))) => Reply::Confirm(confirm),
            Some(other) => panic!("unexpected message {:?}", other),
        }
    }

    /// `ReadDir`: the listing, or the host's error (id 0).
    async fn list(&mut self, path: &str) -> Result<FileDirectory, String> {
        self.act(file_action::Union::ReadDir(proto::ReadDir { path: path.into(), include_hidden: true })).await;
        match self.reply().await {
            Reply::Dir(dir) => {
                assert_eq!(dir.id, 0);
                Ok(dir)
            }
            Reply::Error(e) => {
                assert_eq!((e.id, e.file_num), (0, -1), "the error of a listing has no job");
                Err(e.error)
            }
            other => panic!("{:?}", other),
        }
    }

    /// An action answered with `Done` or an error for job `id`.
    async fn done(&mut self, id: i32, union: file_action::Union) -> Result<(), String> {
        self.act(union).await;
        match self.reply().await {
            Reply::Done(d) if d.id == id => Ok(()),
            Reply::Error(e) if e.id == id => Err(e.error),
            other => panic!("{:?}", other),
        }
    }

    /// `Send` (a download) of one file: the job's listing, the digest, the
    /// confirm (`offset`, or skip when `None`), the blocks to `Done`.
    async fn download(&mut self, id: i32, path: &str, offset: Option<u32>) -> Result<(FileTransferDigest, Vec<u8>), String> {
        self.act(file_action::Union::Send(FileTransferSendRequest { id, path: path.into(), include_hidden: true, file_num: 0, file_type: 0 })).await;
        match self.reply().await {
            Reply::Dir(dir) => assert_eq!((dir.id, dir.path.as_str(), dir.entries.len()), (id, path, 1)),
            Reply::Error(e) if e.id == id => return Err(e.error),
            other => panic!("{:?}", other),
        }
        let digest = match self.reply().await {
            Reply::Digest(digest) => digest,
            Reply::Error(e) if e.id == id => return Err(e.error),
            other => panic!("{:?}", other),
        };
        assert_eq!((digest.id, digest.file_num, digest.is_upload), (id, 0, false));
        let union = match offset {
            Some(offset) => confirm::Union::OffsetBlk(offset),
            None => confirm::Union::Skip(true),
        };
        self.send(new_send_confirm(id, 0, union)).await;
        let mut data = Vec::new();
        loop {
            match self.reply().await {
                Reply::Block(block) => {
                    assert_eq!((block.id, block.file_num), (id, 0));
                    if block.compressed {
                        data.extend_from_slice(&cursor::decompress(&block.data, MAX_DECOMPRESSED_SIZE).expect("zstd"));
                    } else {
                        data.extend_from_slice(&block.data);
                    }
                }
                Reply::Done(done) => {
                    assert_eq!(done.id, id);
                    return Ok((digest, data));
                }
                Reply::Error(e) if e.id == id => return Err(e.error),
                other => panic!("{:?}", other),
            }
        }
    }

    /// `Receive` of one file to `path` and its digest (io_loop.rs: the
    /// client's read job sends the digest of the file it is about to send).
    async fn offer(&mut self, id: i32, path: &str, size: u64, modified: u64, is_resume: bool) -> Reply {
        self.act(file_action::Union::Receive(proto::FileTransferReceiveRequest {
            id,
            path: path.into(),
            files: vec![FileEntry { entry_type: FileType::File as i32, size, modified_time: modified, ..Default::default() }],
            file_num: 0,
            total_size: size,
        }))
        .await;
        self.respond(file_response::Union::Digest(FileTransferDigest { id, file_num: 0, last_modified: modified, file_size: size, is_resume, ..Default::default() })).await;
        self.reply().await
    }

    /// The blocks of an upload, then `Done`.
    async fn send_blocks(&mut self, id: i32, data: &[u8]) -> Result<(), String> {
        for chunk in data.chunks(BLOCK) {
            self.respond(file_response::Union::Block(block_of(id, 0, chunk, true))).await;
        }
        // fs.rs `TransferJob::read`: the end of a file is an empty block.
        self.respond(file_response::Union::Block(block_of(id, 0, &[], true))).await;
        self.respond(file_response::Union::Done(FileTransferDone { id, file_num: 0 })).await;
        match self.reply().await {
            Reply::Done(d) if d.id == id => Ok(()),
            Reply::Error(e) if e.id == id => Err(e.error),
            other => panic!("{:?}", other),
        }
    }

    /// A whole upload of a file that is not there.
    async fn upload(&mut self, id: i32, path: &str, data: &[u8], modified: u64) -> Result<(), String> {
        match self.offer(id, path, data.len() as u64, modified, false).await {
            Reply::Confirm(c) => assert_eq!((c.id, c.file_num, c.union), (id, 0, Some(confirm::Union::OffsetBlk(0)))),
            Reply::Error(e) if e.id == id => return Err(e.error),
            other => panic!("{:?}", other),
        }
        self.send_blocks(id, data).await
    }
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lunaris-rd-host-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // As the agent makes the folder of its `file_access`
    // (`Access::from_config`): on Windows without the `\\?\` that
    // `fs::canonicalize` puts before a drive, which is a device path when a
    // client names it.
    crate::files::canonical(&dir).unwrap()
}

/// Removes the test's folders however it ends.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for dir in &self.0 {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// The path as a client of this system writes it: with its separator only
/// (`root.join("a/b")` has both on Windows).
fn text(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text
    }
}

/// A name from a folder, with this system's separator.
fn rel(name: &str) -> String {
    name.replace('/', std::path::MAIN_SEPARATOR_STR)
}

/// The system's words for what is not there ("No such file or directory";
/// on Windows "The system cannot find the file / the path specified").
fn says_not_there(error: &str) -> bool {
    error.contains("No such file") || error.contains("cannot find the")
}

/// Hidden as this system has it: a name with a dot in front, and on Windows
/// the attribute (libs/base/src/fs.rs `read_dir` reads only that there).
fn hide(_path: &Path) {
    #[cfg(windows)]
    assert!(std::process::Command::new("attrib").arg("+h").arg(_path).status().unwrap().success());
}

/// A link to a folder: a symbolic link, and on Windows a junction (which
/// needs no privilege; a link to a file does there, so those are tested on
/// Unix only).
fn link_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let out = std::process::Command::new("cmd").arg("/c").arg("mklink").arg("/J").arg(link).arg(target).output().unwrap();
        assert!(out.status.success(), "mklink /J: {}", String::from_utf8_lossy(&out.stderr));
    }
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn set_mtime(path: &Path, secs: u64) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(UNIX_EPOCH + Duration::from_secs(secs)).unwrap();
}

/// Bytes that do not compress (xorshift), `n` of them.
fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    let mut out = Vec::with_capacity(n + 8);
    while out.len() < n {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(n);
    out
}

fn names(dir: &FileDirectory) -> Vec<&str> {
    let mut names: Vec<&str> = dir.entries.iter().map(|e| e.name.as_str()).collect();
    names.sort();
    names
}

/// Every path under `dir` with what it is, to see that nothing changed.
fn snapshot(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let meta = std::fs::symlink_metadata(entry.path()).unwrap();
            out.push((entry.path(), if meta.is_dir() { u64::MAX } else { meta.len() }));
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    out.sort();
    out
}

async fn confined(root: &Path) -> Listener {
    start_listener(PASSWORD, "password", true, Access::Root(root.to_path_buf()), None).await
}

// ------------------------------------------------------------------ tests

#[tokio::test]
async fn the_file_transfer_login_follows_the_permission() {
    let root = tmp("login");
    let _cleanup = Cleanup(vec![root.clone()]);
    std::fs::create_dir(root.join("Tài liệu")).unwrap();
    std::fs::write(root.join("ghi chú.txt"), b"xin chao").unwrap();

    // Off (the default): the host says so as a RustDesk host with "Enable
    // file transfer" off does: `PermissionInfo { File, false }` when the
    // connection opens, the login error, and the connection is closed.
    let off = start_listener(PASSWORD, "password", false, Access::Root(root.clone()), None).await;
    let mut client = Client::connect(off.address).await;
    assert_eq!(client.login(PASSWORD, "").await.unwrap_err(), NO_FILE_PERMISSION);
    assert!(client.denied.contains(&(Permission::File as i32)));
    // A file action sent all the same is not answered; the host hangs up.
    client.act(file_action::Union::ReadDir(proto::ReadDir { path: text(&root), include_hidden: true })).await;
    assert!(matches!(client.reply().await, Reply::Closed));

    // On: a wrong password is refused as for a control connection, the
    // right one lets in on the same connection.
    let on = confined(&root).await;
    let mut client = Client::connect(on.address).await;
    assert_eq!(client.login("sai", "").await.unwrap_err(), login::LOGIN_MSG_PASSWORD_WRONG);
    let info = client.login(PASSWORD, "").await.expect("logged in");
    assert!(!client.denied.contains(&(Permission::File as i32)));
    assert_eq!((info.hostname.as_str(), info.version.as_str()), ("máy-thử", "1.5.3"));
    assert!(info.displays.is_empty() && info.encoding.is_none());
    // The listing the client expects without asking: the folder it starts in.
    match client.reply().await {
        Reply::Dir(dir) => {
            assert_eq!((dir.id, dir.path.as_str()), (0, text(&root).as_str()));
            assert_eq!(names(&dir), ["Tài liệu", "ghi chú.txt"]);
        }
        other => panic!("{:?}", other),
    }
    assert!(on.asked.lock().unwrap().is_empty());

    // `FileTransfer.dir`: a folder the client may see is listed first; one
    // it may not, or that is not there, gives the starting folder.
    let mut client = Client::connect(on.address).await;
    client.login(PASSWORD, &text(&root.join("Tài liệu"))).await.unwrap();
    assert!(matches!(client.reply().await, Reply::Dir(dir) if dir.path == text(&root.join("Tài liệu")) && dir.entries.is_empty()));
    let mut client = Client::connect(on.address).await;
    client.login(PASSWORD, if cfg!(windows) { r"C:\Windows" } else { "/etc" }).await.unwrap();
    assert!(matches!(client.reply().await, Reply::Dir(dir) if dir.path == text(&root)));

    // "click": the person here is asked, about files, and lets in.
    let click = start_listener(PASSWORD, "click", true, Access::Root(root.clone()), None).await;
    let mut client = Client::connect(click.address).await;
    assert_eq!(client.login(PASSWORD, "").await.unwrap_err(), login::LOGIN_MSG_NO_PASSWORD_ACCESS);
    match client.next().await.and_then(|m| m.union) {
        Some(message::Union::LoginResponse(proto::LoginResponse { union: Some(login_response::Union::PeerInfo(_)), .. })) => {}
        other => panic!("expected to be let in, got {:?}", other),
    }
    assert!(matches!(client.reply().await, Reply::Dir(dir) if dir.path == text(&root)));
    let asked = click.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, Kind::Files);

    // A setting that names no folder: refused in words.
    let nowhere = start_listener(PASSWORD, "password", true, Access::from_config(&text(&root.join("không có/thư mục"))), None).await;
    let mut client = Client::connect(nowhere.address).await;
    assert!(client.login(PASSWORD, "").await.unwrap_err().contains("does not exist"));
}

#[tokio::test]
async fn lists_folders_with_vietnamese_names() {
    let root = tmp("list");
    let outside = tmp("list-outside");
    let _cleanup = Cleanup(vec![root.clone(), outside.clone()]);
    std::fs::create_dir_all(root.join("Tài liệu/Ảnh cưới/trống")).unwrap();
    std::fs::create_dir_all(root.join("Tài liệu/rỗng")).unwrap();
    std::fs::write(root.join("Tài liệu/Báo cáo quý 3.docx"), b"12345").unwrap();
    std::fs::write(root.join("Tài liệu/Ảnh cưới/ảnh 1.jpg"), b"jpg").unwrap();
    std::fs::write(root.join("Tài liệu/.ẩn"), b"x").unwrap();
    hide(&root.join("Tài liệu/.ẩn"));
    std::fs::write(outside.join("bí mật.txt"), b"ngoai").unwrap();
    set_mtime(&root.join("Tài liệu/Báo cáo quý 3.docx"), T0);
    link_dir(&outside, &root.join("Tài liệu/ra ngoài"));
    link_dir(&root.join("Tài liệu/Ảnh cưới"), &root.join("Tài liệu/lối tắt"));
    let host = confined(&root).await;
    let (mut c, first) = Client::open(host.address).await;
    assert_eq!(names(&first), ["Tài liệu"]);

    // The listing is named as the client asked for it (its file manager
    // waits for that path), a slash at the end included.
    let asked = format!("{}{}", text(&root.join("Tài liệu")), std::path::MAIN_SEPARATOR);
    let dir = c.list(&asked).await.unwrap();
    assert_eq!(dir.path, asked);
    // The link that leads out of the folder is not there; the one that
    // stays inside is, as a link to a folder.
    assert_eq!(names(&dir), [".ẩn", "Báo cáo quý 3.docx", "lối tắt", "rỗng", "Ảnh cưới"]);
    let report = dir.entries.iter().find(|e| e.name == "Báo cáo quý 3.docx").unwrap();
    assert_eq!((report.entry_type, report.size, report.modified_time, report.is_hidden), (FileType::File as i32, 5, T0, false));
    assert_eq!(dir.entries.iter().find(|e| e.name == "Ảnh cưới").unwrap().entry_type, FileType::Dir as i32);
    assert!(dir.entries.iter().find(|e| e.name == ".ẩn").unwrap().is_hidden);
    assert_eq!(dir.entries.iter().find(|e| e.name == "lối tắt").unwrap().entry_type, FileType::DirLink as i32);
    // Through the link that stays inside.
    assert_eq!(names(&c.list(&text(&root.join("Tài liệu/lối tắt"))).await.unwrap()), ["trống", "ảnh 1.jpg"]);
    // The other separator is read too (a client of another system).
    assert_eq!(names(&c.list(&text(&root.join("Tài liệu/Ảnh cưới")).replace('\\', "/")).await.unwrap()), ["trống", "ảnh 1.jpg"]);
    // Hidden entries only when asked for.
    c.act(file_action::Union::ReadDir(proto::ReadDir { path: text(&root.join("Tài liệu")), include_hidden: false })).await;
    assert!(matches!(c.reply().await, Reply::Dir(dir) if !names(&dir).contains(&".ẩn")));
    // "" is the folder the client starts in.
    assert_eq!(c.list("").await.unwrap().path, text(&root));
    // What is not there: the system's words, at once.
    let error = c.list(&text(&root.join("không có"))).await.unwrap_err();
    assert!(says_not_there(&error), "{}", error);

    // Everything under a folder (`ReadAllFiles`): files, named from it;
    // links are not followed.
    c.act(file_action::Union::AllFiles(proto::ReadAllFiles { id: 7, path: text(&root.join("Tài liệu")), include_hidden: true })).await;
    match c.reply().await {
        Reply::Dir(dir) => {
            assert_eq!((dir.id, dir.path.as_str()), (7, text(&root.join("Tài liệu")).as_str()));
            assert_eq!(names(&dir), [".ẩn", "Báo cáo quý 3.docx", rel("Ảnh cưới/ảnh 1.jpg").as_str()]);
        }
        other => panic!("{:?}", other),
    }
    // The folders with nothing in them (`ReadEmptyDirs`).
    c.act(file_action::Union::ReadEmptyDirs(proto::ReadEmptyDirs { path: text(&root.join("Tài liệu")), include_hidden: true })).await;
    match c.reply().await {
        Reply::EmptyDirs(dirs) => {
            let mut paths: Vec<String> = dirs.empty_dirs.iter().map(|d| d.path.clone()).collect();
            paths.sort();
            assert_eq!(paths, [text(&root.join("Tài liệu/rỗng")), text(&root.join("Tài liệu/Ảnh cưới/trống"))]);
        }
        other => panic!("{:?}", other),
    }
}

#[tokio::test]
async fn a_download_is_digest_confirm_blocks_done() {
    let root = tmp("down");
    let _cleanup = Cleanup(vec![root.clone()]);
    let words = "Con đường dài nhất là con đường chưa đi. ".repeat(20_000).into_bytes();
    let random = noise(700_000, 7);
    std::fs::write(root.join("văn bản.txt"), &words).unwrap();
    std::fs::write(root.join("nhiễu.bin"), &random).unwrap();
    std::fs::write(root.join("rỗng"), b"").unwrap();
    set_mtime(&root.join("văn bản.txt"), T0);
    let host = confined(&root).await;
    let (mut c, _) = Client::open(host.address).await;

    // Whole: the digest names the file, the blocks are it.
    let (digest, data) = c.download(1, &text(&root.join("văn bản.txt")), Some(0)).await.unwrap();
    assert_eq!((digest.file_size, digest.last_modified), (words.len() as u64, T0));
    assert!(data == words);
    assert!(c.download(2, &text(&root.join("nhiễu.bin")), Some(0)).await.unwrap().1 == random);
    assert!(c.download(3, &text(&root.join("rỗng")), Some(0)).await.unwrap().1.is_empty());
    // From an offset (the client has the start of it).
    assert!(c.download(4, &text(&root.join("nhiễu.bin")), Some(123_457)).await.unwrap().1 == random[123_457..]);
    // Skipped by the client: the job ends as RustDesk's does.
    assert_eq!(c.download(5, &text(&root.join("nhiễu.bin")), None).await.unwrap_err(), "skipped");
    // What is not there.
    assert_eq!(c.download(6, &text(&root.join("không có")), Some(0)).await.unwrap_err(), "Not exists");

    // A folder: every file in turn, each with its digest.
    std::fs::create_dir(root.join("thư mục")).unwrap();
    std::fs::write(root.join("thư mục/a.txt"), b"mot").unwrap();
    std::fs::write(root.join("thư mục/b.txt"), b"hai").unwrap();
    c.act(file_action::Union::Send(FileTransferSendRequest { id: 8, path: text(&root.join("thư mục")), include_hidden: true, file_num: 0, file_type: 0 })).await;
    let listed = match c.reply().await {
        Reply::Dir(dir) => dir.entries.iter().map(|e| e.name.clone()).collect::<Vec<_>>(),
        other => panic!("{:?}", other),
    };
    assert_eq!(listed.len(), 2);
    let mut got: Vec<(String, Vec<u8>)> = listed.iter().map(|name| (name.clone(), Vec::new())).collect();
    loop {
        match c.reply().await {
            Reply::Digest(d) => c.send(new_send_confirm(8, d.file_num, confirm::Union::OffsetBlk(0))).await,
            Reply::Block(b) => got[b.file_num as usize].1.extend_from_slice(&b.data),
            Reply::Done(d) => {
                assert_eq!((d.id, d.file_num), (8, 2));
                break;
            }
            other => panic!("{:?}", other),
        }
    }
    got.sort();
    assert_eq!(got, [("a.txt".to_string(), b"mot".to_vec()), ("b.txt".to_string(), b"hai".to_vec())]);

    // Cancelled in the middle: the blocks stop, and the connection goes on.
    let big = noise(24 * 1024 * 1024, 9);
    std::fs::write(root.join("lớn.bin"), &big).unwrap();
    c.act(file_action::Union::Send(FileTransferSendRequest { id: 9, path: text(&root.join("lớn.bin")), include_hidden: true, file_num: 0, file_type: 0 })).await;
    assert!(matches!(c.reply().await, Reply::Dir(_)));
    assert!(matches!(c.reply().await, Reply::Digest(_)));
    c.send(new_send_confirm(9, 0, confirm::Union::OffsetBlk(0))).await;
    let mut received = 0usize;
    for _ in 0..4 {
        match c.reply().await {
            Reply::Block(b) => received += b.data.len(),
            other => panic!("{:?}", other),
        }
    }
    c.act(file_action::Union::Cancel(proto::FileTransferCancel { id: 9 })).await;
    c.act(file_action::Union::ReadDir(proto::ReadDir { path: text(&root), include_hidden: true })).await;
    loop {
        match c.reply().await {
            // What was on its way when the cancel was sent.
            Reply::Block(b) => received += b.data.len(),
            Reply::Dir(dir) => {
                assert_eq!(dir.id, 0);
                break;
            }
            other => panic!("{:?}", other),
        }
    }
    assert!(received < big.len(), "the download stopped ({} of {})", received, big.len());
}

#[tokio::test]
async fn an_upload_is_written_beside_the_file_and_asks_before_it_replaces() {
    let root = tmp("up");
    let _cleanup = Cleanup(vec![root.clone()]);
    let host = confined(&root).await;
    let (mut c, _) = Client::open(host.address).await;
    let words = "Trăm năm trong cõi người ta. ".repeat(30_000).into_bytes();
    let target = root.join("Tải lên/thơ Kiều.txt");
    let (download, digest) = (root.join("Tải lên/thơ Kiều.txt.download"), root.join("Tải lên/thơ Kiều.txt.digest"));

    // A new file: the host confirms (offset 0); what arrives is in
    // `.download` with a `.digest` beside it until `Done`.
    match c.offer(1, &text(&target), words.len() as u64, T0, false).await {
        Reply::Confirm(r) => assert_eq!((r.id, r.file_num, r.union), (1, 0, Some(confirm::Union::OffsetBlk(0)))),
        other => panic!("{:?}", other),
    }
    c.respond(file_response::Union::Block(block_of(1, 0, &words[..BLOCK], true))).await;
    // The host has taken the block once it answered what came after it.
    c.list(&text(&root)).await.unwrap();
    assert!(download.exists() && digest.exists() && !target.exists());
    assert_eq!(std::fs::metadata(&download).unwrap().len(), BLOCK as u64);
    for chunk in words[BLOCK..].chunks(BLOCK) {
        c.respond(file_response::Union::Block(block_of(1, 0, chunk, true))).await;
    }
    c.respond(file_response::Union::Block(block_of(1, 0, &[], true))).await;
    c.respond(file_response::Union::Done(FileTransferDone { id: 1, file_num: 0 })).await;
    assert!(matches!(c.reply().await, Reply::Done(d) if d.id == 1));
    assert!(std::fs::read(&target).unwrap() == words);
    assert_eq!(mtime(&target), T0);
    assert!(!download.exists() && !digest.exists());

    // The same file again: the host says it has it (`is_identical`), and
    // the client skips.
    match c.offer(2, &text(&target), words.len() as u64, T0, false).await {
        Reply::Digest(d) => assert_eq!((d.id, d.is_upload, d.is_identical, d.file_size, d.last_modified), (2, true, true, words.len() as u64, T0)),
        other => panic!("{:?}", other),
    }
    c.send(new_send_confirm(2, 0, confirm::Union::Skip(true))).await;
    c.act(file_action::Union::Cancel(proto::FileTransferCancel { id: 2 })).await;

    // Another file of that name: the host asks (not identical). Skipped,
    // the file stays; confirmed, it is replaced.
    let other = b"mot noi dung khac".to_vec();
    match c.offer(3, &text(&target), other.len() as u64, T0 + 60, false).await {
        Reply::Digest(d) => assert_eq!((d.is_upload, d.is_identical, d.file_size), (true, false, words.len() as u64)),
        other => panic!("{:?}", other),
    }
    c.send(new_send_confirm(3, 0, confirm::Union::Skip(true))).await;
    c.act(file_action::Union::Cancel(proto::FileTransferCancel { id: 3 })).await;
    c.list(&text(&root)).await.unwrap();
    assert!(std::fs::read(&target).unwrap() == words);
    assert!(matches!(c.offer(4, &text(&target), other.len() as u64, T0 + 60, false).await, Reply::Digest(d) if !d.is_identical));
    c.send_blocks(4, &other).await.unwrap();
    assert!(std::fs::read(&target).unwrap() == other);
    assert_eq!(mtime(&target), T0 + 60);

    // Resume: a connection that ends in the middle leaves the part; the
    // next one hears how much is there and sends the rest.
    let data = noise(3 * 1024 * 1024, 3);
    let part = root.join("dở dang.bin");
    {
        let (mut cut, _) = Client::open(host.address).await;
        assert!(matches!(cut.offer(5, &text(&part), data.len() as u64, T0, false).await, Reply::Confirm(_)));
        for chunk in data[..8 * BLOCK].chunks(BLOCK) {
            cut.respond(file_response::Union::Block(block_of(5, 0, chunk, true))).await;
        }
        cut.list(&text(&root)).await.unwrap();
    }
    let have = std::fs::metadata(root.join("dở dang.bin.download")).unwrap().len();
    assert_eq!(have, 8 * BLOCK as u64);
    assert!(!part.exists());
    match c.offer(6, &text(&part), data.len() as u64, T0, true).await {
        Reply::Digest(d) => assert_eq!((d.is_upload, d.is_identical, d.transferred_size), (true, true, have)),
        other => panic!("{:?}", other),
    }
    c.send(new_send_confirm(6, 0, confirm::Union::OffsetBlk(have as u32))).await;
    c.send_blocks(6, &data[have as usize..]).await.unwrap();
    assert!(std::fs::read(&part).unwrap() == data);
    assert!(!root.join("dở dang.bin.download").exists() && !root.join("dở dang.bin.digest").exists());
    // A part of another file is not gone on from.
    std::fs::write(root.join("khác.bin.download"), b"cu").unwrap();
    std::fs::write(root.join("khác.bin.digest"), serde_json::json!({"size": 999, "modified": 1}).to_string()).unwrap();
    assert!(matches!(c.offer(7, &text(&root.join("khác.bin")), 4, T0, true).await, Reply::Confirm(r) if r.union == Some(confirm::Union::OffsetBlk(0))));
    c.send_blocks(7, b"moi!").await.unwrap();
    assert_eq!(std::fs::read(root.join("khác.bin")).unwrap(), b"moi!");

    // Cancelled in the middle: the part goes.
    assert!(matches!(c.offer(8, &text(&root.join("bỏ dở.bin")), data.len() as u64, T0, false).await, Reply::Confirm(_)));
    c.respond(file_response::Union::Block(block_of(8, 0, &data[..BLOCK], true))).await;
    c.act(file_action::Union::Cancel(proto::FileTransferCancel { id: 8 })).await;
    // A block that was on its way is not an error.
    c.respond(file_response::Union::Block(block_of(8, 0, &data[..BLOCK], true))).await;
    assert_eq!(names(&c.list(&text(&root)).await.unwrap()), ["Tải lên", "dở dang.bin", "khác.bin"]);

    // Several files in one job, into folders that are made on the way.
    c.act(file_action::Union::Receive(proto::FileTransferReceiveRequest {
        id: 9,
        path: text(&root.join("Nhiều")),
        files: vec![
            FileEntry { name: "a.txt".into(), modified_time: T0, ..Default::default() },
            FileEntry { name: "trong/b.txt".into(), modified_time: T0 + 1, ..Default::default() },
        ],
        file_num: 0,
        total_size: 6,
    }))
    .await;
    for (n, data) in [(0, b"mot"), (1, b"hai")] {
        c.respond(file_response::Union::Digest(FileTransferDigest { id: 9, file_num: n, last_modified: T0 + n as u64, file_size: 3, ..Default::default() })).await;
        assert!(matches!(c.reply().await, Reply::Confirm(r) if r.file_num == n));
        c.respond(file_response::Union::Block(FileTransferBlock { id: 9, file_num: n, data: bytes::Bytes::from_static(data), ..Default::default() })).await;
        c.respond(file_response::Union::Block(FileTransferBlock { id: 9, file_num: n, ..Default::default() })).await;
    }
    c.respond(file_response::Union::Done(FileTransferDone { id: 9, file_num: 1 })).await;
    assert!(matches!(c.reply().await, Reply::Done(d) if d.id == 9));
    assert_eq!(std::fs::read(root.join("Nhiều/a.txt")).unwrap(), b"mot");
    assert_eq!(std::fs::read(root.join("Nhiều/trong/b.txt")).unwrap(), b"hai");
    assert_eq!((mtime(&root.join("Nhiều/a.txt")), mtime(&root.join("Nhiều/trong/b.txt"))), (T0, T0 + 1));

    // A block that says it is compressed and is not: an error, no file.
    assert!(matches!(c.offer(10, &text(&root.join("hỏng.bin")), 9, T0, false).await, Reply::Confirm(_)));
    c.respond(file_response::Union::Block(FileTransferBlock { id: 10, file_num: 0, data: bytes::Bytes::from_static(b"khong phai zstd"), compressed: true, blk_id: 0 })).await;
    assert!(matches!(c.reply().await, Reply::Error(e) if e.id == 10 && e.error.contains("could not be read")));
    assert!(!root.join("hỏng.bin").exists());
}

#[tokio::test]
async fn makes_renames_and_removes() {
    let root = tmp("ops");
    let outside = tmp("ops-outside");
    let _cleanup = Cleanup(vec![root.clone(), outside.clone()]);
    std::fs::write(outside.join("giữ lại.txt"), b"khong xoa").unwrap();
    let host = confined(&root).await;
    let (mut c, _) = Client::open(host.address).await;
    let at = |name: &str| text(&root.join(name));

    // New folders, the ones above them too.
    c.done(1, file_action::Union::Create(proto::FileDirCreate { id: 1, path: at("Dự án/2026/Bản vẽ") })).await.unwrap();
    assert!(root.join("Dự án/2026/Bản vẽ").is_dir());
    std::fs::write(root.join("Dự án/ghi chú.txt"), b"3").unwrap();
    std::fs::write(root.join("Dự án/khác.txt"), b"4").unwrap();

    // Rename: a file and a folder; not onto a name that is taken (RustDesk
    // would replace it), not what is not there, not without a name.
    let rename = |id: i32, path: String, new_name: &str| file_action::Union::Rename(proto::FileRename { id, path, new_name: new_name.into() });
    c.done(2, rename(2, at("Dự án/ghi chú.txt"), "Ghi chú mới.txt")).await.unwrap();
    assert!(root.join("Dự án/Ghi chú mới.txt").exists() && !root.join("Dự án/ghi chú.txt").exists());
    c.done(3, rename(3, at("Dự án/2026"), "Năm 2026")).await.unwrap();
    assert!(root.join("Dự án/Năm 2026/Bản vẽ").is_dir());
    assert_eq!(c.done(4, rename(4, at("Dự án/khác.txt"), "Ghi chú mới.txt")).await.unwrap_err(), "Ghi chú mới.txt already exists");
    assert_eq!(std::fs::read(root.join("Dự án/Ghi chú mới.txt")).unwrap(), b"3");
    assert_eq!(c.done(5, rename(5, at("Dự án/khác.txt"), "")).await.unwrap_err(), "new file name cannot be empty");
    assert!(c.done(6, rename(6, at("Dự án/không có"), "x")).await.unwrap_err().contains("not exists"));
    // A new name is cleaned as Lunaris's file manager cleans it.
    c.done(7, rename(7, at("Dự án/khác.txt"), "bản*nháp?.txt")).await.unwrap();
    assert!(root.join("Dự án/bản_nháp_.txt").exists());

    // Remove: a file; a link (the link goes, not what it leads to); a
    // folder as the client removes one (its files, then
    // `remove_dir { recursive }`); what is not there; not the folder itself.
    let remove_file = |id: i32, path: String| file_action::Union::RemoveFile(proto::FileRemoveFile { id, path, file_num: 0 });
    let remove_dir = |id: i32, path: String, recursive: bool| file_action::Union::RemoveDir(proto::FileRemoveDir { id, path, recursive });
    c.done(8, remove_file(8, at("Dự án/bản_nháp_.txt"))).await.unwrap();
    assert!(!root.join("Dự án/bản_nháp_.txt").exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.join("giữ lại.txt"), root.join("Dự án/tệp tắt")).unwrap();
        c.done(9, remove_file(9, at("Dự án/tệp tắt"))).await.unwrap();
        assert!(std::fs::symlink_metadata(root.join("Dự án/tệp tắt")).is_err());
    }
    // A link to a folder outside, removed as a client removes one (on
    // Windows as a folder, elsewhere as a file): the link goes, what it
    // leads to stays.
    link_dir(&outside, &root.join("Dự án/lối ra"));
    if cfg!(windows) {
        c.done(19, remove_dir(19, at("Dự án/lối ra"), false)).await.unwrap();
    } else {
        c.done(19, remove_file(19, at("Dự án/lối ra"))).await.unwrap();
    }
    assert!(std::fs::symlink_metadata(root.join("Dự án/lối ra")).is_err());
    assert_eq!(std::fs::read(outside.join("giữ lại.txt")).unwrap(), b"khong xoa");
    // Such a link in a folder that is removed: it goes with the folder.
    link_dir(&outside, &root.join("Dự án/Năm 2026/lối tắt"));
    c.done(10, remove_file(10, at("Dự án/Ghi chú mới.txt"))).await.unwrap();
    c.done(11, remove_dir(11, at("Dự án"), true)).await.unwrap();
    assert!(!root.join("Dự án").exists());
    assert_eq!(std::fs::read(outside.join("giữ lại.txt")).unwrap(), b"khong xoa");
    let error = c.done(12, remove_file(12, at("không có"))).await.unwrap_err();
    assert!(says_not_there(&error), "{}", error);
    assert!(c.done(13, remove_dir(13, text(&root), true)).await.is_err());
    assert_eq!(c.done(14, remove_file(14, String::new())).await.unwrap_err(), "file path cannot be empty");
    // An empty folder, not recursively; one that is not empty stays.
    std::fs::create_dir_all(root.join("trống")).unwrap();
    c.done(15, remove_dir(15, at("trống"), false)).await.unwrap();
    assert!(!root.join("trống").exists());
    std::fs::create_dir_all(root.join("đầy")).unwrap();
    std::fs::write(root.join("đầy/x"), b"x").unwrap();
    assert!(c.done(16, remove_dir(16, at("đầy"), false)).await.is_err());
    assert!(root.join("đầy/x").exists());
    assert!(root.is_dir());
}

#[tokio::test]
async fn nothing_outside_the_allowed_folder_is_reached() {
    let root = tmp("confine");
    let outside = tmp("confine-outside");
    let _cleanup = Cleanup(vec![root.clone(), outside.clone()]);
    std::fs::create_dir(outside.join("thư mục")).unwrap();
    std::fs::write(outside.join("bí mật.txt"), b"khong duoc doc").unwrap();
    std::fs::write(outside.join("thư mục/trong.txt"), b"khong duoc xoa").unwrap();
    std::fs::create_dir(root.join("trong")).unwrap();
    std::fs::write(root.join("trong/của tôi.txt"), b"duoc").unwrap();
    link_dir(&outside, &root.join("lối ra"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.join("bí mật.txt"), root.join("tệp ra")).unwrap();
        std::os::unix::fs::symlink(outside.join("chưa có.txt"), root.join("treo")).unwrap();
    }
    let before = snapshot(&outside);
    let host = confined(&root).await;
    let (mut c, first) = Client::open(host.address).await;
    // The links that lead out are not even listed.
    assert_eq!(names(&first), ["trong"]);

    let secret = text(&outside.join("bí mật.txt"));
    let up = format!("{}/../{}/bí mật.txt", text(&root), outside.file_name().unwrap().to_string_lossy());
    let mut escapes: Vec<String> = vec![
        // An absolute path outside, `..` in every place, and what is above.
        secret.clone(),
        text(&outside),
        up,
        "../bí mật.txt".into(),
        format!("{}/trong/../../x", text(&root)),
        "/".into(),
        "/etc/passwd".into(),
        text(root.parent().unwrap()),
        // Windows forms, as strings: device and network paths.
        r"\\.\C:\Windows\win.ini".into(),
        r"\\?\C:\Windows\win.ini".into(),
        r"\??\C:\Windows\win.ini".into(),
        r"\\máy\chia sẻ\tệp.txt".into(),
        "//máy/chia sẻ/tệp.txt".into(),
        // Names Windows reads as something else.
        format!("{}/CON", text(&root)),
        format!("{}/trong/NUL.txt", text(&root)),
        format!("{}/COM1", text(&root)),
        format!("{}/tên. ", text(&root)),
        format!("{}/tên...", text(&root)),
        // A NUL in the path.
        format!("{}/trong\0/../../x", text(&root)),
    ];
    // Links inside the folder that lead out of it, and what is behind them.
    escapes.extend([text(&root.join("lối ra")), text(&root.join("lối ra/bí mật.txt")), text(&root.join("lối ra/thư mục"))]);
    #[cfg(unix)]
    escapes.extend([text(&root.join("tệp ra")), text(&root.join("treo"))]);

    let mut id = 100;
    for path in &escapes {
        let shown = path.replace('\0', "\\0");
        // Listing: an error with id 0 (the file manager's pending listing
        // fails at once), in words.
        let error = c.list(path).await.expect_err(&shown);
        assert!(!error.is_empty(), "{}", shown);
        // Everything under it; a download; a new folder: errors by job id.
        id += 1;
        c.act(file_action::Union::AllFiles(proto::ReadAllFiles { id, path: path.clone(), include_hidden: true })).await;
        assert!(matches!(c.reply().await, Reply::Error(e) if e.id == id && !e.error.is_empty()), "{}", shown);
        id += 1;
        assert!(c.download(id, path, Some(0)).await.is_err(), "{}", shown);
        id += 1;
        assert!(c.done(id, file_action::Union::Create(proto::FileDirCreate { id, path: format!("{}/mới", path) })).await.is_err(), "{}", shown);
        // An upload to it, and into it: refused before any data is asked for.
        for target in [path.clone(), format!("{}/tải lên.txt", path)] {
            id += 1;
            match c.offer(id, &target, 4, T0, false).await {
                Reply::Error(e) => assert!(e.id == id && !e.error.is_empty(), "{}", shown),
                other => panic!("{}: {:?}", shown, other),
            }
        }
    }
    // Removing and renaming what a link leads to, or what is outside. (The
    // link itself, inside the folder, may be removed: see the other test.)
    let mut entries: Vec<String> = vec![secret.clone(), text(&outside), text(&outside.join("thư mục")), "../x".into(), "/".into(), text(&root), r"\\.\C:\x".into(), r"\\máy\chia sẻ\x".into()];
    entries.extend([text(&root.join("lối ra/bí mật.txt")), text(&root.join("lối ra/thư mục"))]);
    for path in &entries {
        id += 1;
        assert!(c.done(id, file_action::Union::RemoveFile(proto::FileRemoveFile { id, path: path.clone(), file_num: 0 })).await.is_err(), "{}", path);
        for recursive in [false, true] {
            id += 1;
            assert!(c.done(id, file_action::Union::RemoveDir(proto::FileRemoveDir { id, path: path.clone(), recursive })).await.is_err(), "{}", path);
        }
        id += 1;
        assert!(c.done(id, file_action::Union::Rename(proto::FileRename { id, path: path.clone(), new_name: "đổi".into() })).await.is_err(), "{}", path);
    }
    // A new name that is a path out, for an entry inside.
    let mine = text(&root.join("trong/của tôi.txt"));
    let absolute = format!("{}/ra.txt", text(&outside));
    for name in ["../../ra.txt", "..", absolute.as_str(), r"..\ra.txt", r"C:\ra.txt", "a\0b"] {
        id += 1;
        let error = c.done(id, file_action::Union::Rename(proto::FileRename { id, path: mine.clone(), new_name: name.into() })).await.unwrap_err();
        assert!(error.contains("traversal") || error.contains("absolute path") || error.contains("null bytes"), "{:?}: {}", name, error);
    }
    assert!(root.join("trong/của tôi.txt").exists());
    // The files of an upload named as paths out of where it goes.
    for name in ["../ra.txt", "a/../../ra.txt", absolute.as_str(), r"..\ra.txt", r"C:\ra.txt", r"\\máy\x", "a\0b"] {
        id += 1;
        c.act(file_action::Union::Receive(proto::FileTransferReceiveRequest {
            id,
            path: text(&root.join("trong")),
            files: vec![FileEntry { name: "được.txt".into(), ..Default::default() }, FileEntry { name: name.into(), ..Default::default() }],
            file_num: 0,
            total_size: 2,
        }))
        .await;
        assert!(matches!(c.reply().await, Reply::Error(e) if e.id == id && !e.error.is_empty()), "{:?}", name);
    }
    // ... and through a link that is a part of the name (RustDesk's own rule).
    {
        link_dir(&root.join("trong"), &root.join("vòng"));
        id += 1;
        c.act(file_action::Union::Receive(proto::FileTransferReceiveRequest {
            id,
            path: text(&root),
            files: vec![FileEntry { name: "vòng/x.txt".into(), ..Default::default() }, FileEntry { name: "y.txt".into(), ..Default::default() }],
            file_num: 0,
            total_size: 2,
        }))
        .await;
        assert!(matches!(c.reply().await, Reply::Error(e) if e.id == id && e.error.contains("symlink")));
    }
    // A block for a job that was refused goes nowhere.
    c.respond(file_response::Union::Block(FileTransferBlock { id, file_num: 0, data: bytes::Bytes::from_static(b"du lieu"), ..Default::default() })).await;

    // Other Windows forms mean nothing outside the folder here: a drive
    // letter and a stream name are refused or stay names inside it.
    for path in [r"C:\Windows\..\..\x", r"C:..\x", "tệp.txt:luồng"] {
        id += 1;
        let _ = c.done(id, file_action::Union::Create(proto::FileDirCreate { id, path: path.into() })).await;
        id += 1;
        if let Reply::Confirm(_) = c.offer(id, path, 2, T0, false).await {
            c.send_blocks(id, b"ok").await.unwrap();
        }
    }

    // The connection is still good, and nothing outside was touched or made.
    assert_eq!(c.download(999, &mine, Some(0)).await.unwrap().1, b"duoc");
    c.upload(998, &text(&root.join("trong/mới.txt")), b"moi", T0).await.unwrap();
    assert_eq!(std::fs::read(root.join("trong/mới.txt")).unwrap(), b"moi");
    assert_eq!(snapshot(&outside), before);
    assert_eq!(std::fs::read(outside.join("bí mật.txt")).unwrap(), b"khong duoc doc");
    assert!(!root.parent().unwrap().join("x").exists() && !root.parent().unwrap().join("ra.txt").exists());
    // Everything that was made is inside the folder.
    for (path, _) in snapshot(&root) {
        assert!(path.starts_with(&root));
    }
}

/// The forms only Windows reads as paths: drive letters (with and without a
/// root), `\\?\` and `\\.\`, network paths, device names, names ending in a
/// dot or a space, alternate data streams and short (8.3) names. Whatever
/// names something outside the allowed folder is refused, by the rules
/// themselves (`files::resolve`, `files::resolve_entry`) and through the
/// listener; whatever is taken stays inside it. Nothing outside is read,
/// made, changed or removed.
#[cfg(windows)]
#[tokio::test]
async fn windows_forms_of_a_path_do_not_lead_out_of_the_allowed_folder() {
    let root = tmp("winforms");
    let outside = tmp("winforms-outside");
    let _cleanup = Cleanup(vec![root.clone(), outside.clone()]);
    std::fs::create_dir(outside.join("thư mục")).unwrap();
    std::fs::write(outside.join("bí mật.txt"), b"khong duoc doc").unwrap();
    std::fs::write(outside.join("thư mục/trong.txt"), b"khong duoc xoa").unwrap();
    std::fs::create_dir(root.join("trong")).unwrap();
    std::fs::write(root.join("trong/của tôi.txt"), b"duoc").unwrap();
    let before = snapshot(&outside);
    let secret_time = mtime(&outside.join("bí mật.txt"));

    let (r, o) = (text(&root), text(&outside));
    assert!(!r.starts_with(r"\\") && r.as_bytes()[1] == b':', "a drive path without \\\\?\\: {}", r);
    let drive = &r[..2];
    // The same folders without their drive (`\Users\...`), and by name.
    let (r_rooted, o_rooted) = (&r[2..], &o[2..]);
    let o_name = outside.file_name().unwrap().to_string_lossy().into_owned();
    let r_name = root.file_name().unwrap().to_string_lossy().into_owned();
    // What the test makes where a path is taken, to look for afterwards.
    let made = format!("thoát-{}", std::process::id());

    // The outside folder and the file in it, every way Windows would find
    // them. All of these are refused for everything.
    let mut out: Vec<String> = Vec::new();
    for target in ["", r"\bí mật.txt", r"\thư mục"] {
        out.extend([
            // The drive in the other case, the other separator, no drive.
            format!("{}{}{}", drive.to_lowercase(), o_rooted, target),
            format!("{}{}", o, target).replace('\\', "/"),
            format!("{}{}", o_rooted, target),
            format!("{}{}", o_rooted, target).replace('\\', "/"),
            // Out of the folder and back down, with what Windows drops.
            format!(r"{}\..\{}{}", r, o_name, target),
            format!(r"{}\trong\..\..\{}{}", r, o_name, target),
            format!(r"{}\.\..\{}{}", r, o_name, target),
            format!(r"{}\..\{}.{}", r, o_name, target),
            format!(r"{}\..\{} {}", r, o_name, target),
            format!(r"{}\...\{}{}", r, o_name, target),
            format!(r"{}\.. \{}{}", r, o_name, target),
            format!(r"{}\..\{}{}.", r, o_name, target),
            format!(r"{}\..\{}{} ", r, o_name, target),
            format!(r"{}{}.", o, target),
            format!(r"{}{}. . ", o, target),
            // Device paths: to the outside, and to the folder itself too.
            format!(r"\\?\{}{}", o, target),
            format!(r"\\.\{}{}", o, target),
            format!(r"\??\{}{}", o, target),
            format!(r"//?/{}{}", o, target).replace('\\', "/"),
            format!(r"\\?\{}\..\{}{}", r, o_name, target),
            format!(r"\\?\{}\trong", r),
            // Network paths to this computer's own disk.
            format!(r"\\localhost\{}${}{}", &drive[..1], o_rooted, target),
            format!(r"\\127.0.0.1\{}${}{}", &drive[..1], o_rooted, target),
            format!(r"\\?\UNC\localhost\{}${}{}", &drive[..1], o_rooted, target),
            format!(r"//localhost/{}${}{}", &drive[..1], o_rooted, target).replace('\\', "/"),
            // Streams: of the outside, and ones whose name is a way out.
            format!(r"{}{}::$DATA", o, target),
            format!(r"{}{}:luồng", o, target),
            format!(r"{}::$INDEX_ALLOCATION{}", o, target),
            format!(r"{}::$INDEX_ALLOCATION\..\{}{}", r, o_name, target),
            format!(r"{}:luồng\..\{}{}", r, o_name, target),
            format!(r"{}\trong:$I30:$INDEX_ALLOCATION\..\..\{}{}", r, o_name, target),
        ]);
    }
    out.extend([
        // A drive without a root is the folder the agent runs in.
        drive.to_string(),
        format!("{}{}", drive, made),
        format!(r"{}..\{}", drive, made),
        format!(r"{}.", drive),
        format!(r"{}\", drive),
        r"\".to_string(),
        // The folder's own streams and other names are not the folder.
        format!("{}:luồng", r),
        format!("{}::$DATA", r),
        format!("{}.", r),
        format!(r"{}\..\{}", r, r_name.to_uppercase() + "."),
        // Devices, by name inside the folder and by path.
        format!(r"{}\NUL", r),
        format!(r"{}\nul", r),
        format!(r"{}\trong\con.txt", r),
        format!(r"{}\COM1", r),
        format!(r"{}\LPT9.log", r),
        format!(r"{}\aux", r),
        format!(r"{}\PRN.tar.gz", r),
        format!(r"{}\CONIN$", r),
        format!(r"{}\conout$", r),
        format!(r"{}\NUL \x", r),
        format!(r"{}\NUL:x", r),
        format!(r"{}\con:", r),
        format!(r"{}\trong\COM1:9600", r),
        format!(r"{}\COM¹", r),
        format!(r"{}\lpt².txt", r),
        "NUL".to_string(),
        r"\\.\NUL".to_string(),
        r"\\.\PhysicalDrive0".to_string(),
        r"\\?\GLOBALROOT\Device\Null".to_string(),
        r"\\.\pipe\lunaris".to_string(),
        // Names Windows reads without their end.
        format!(r"{}\trong\của tôi.txt.", r),
        format!(r"{}\trong\của tôi.txt .", r),
        format!(r"{}\trong.\của tôi.txt", r),
        format!(r"{}\trong \của tôi.txt", r),
        format!(r"{}\trong\...", r),
    ]);
    // Read only, never written to: files of the system.
    let system: Vec<String> = vec![
        format!(r"{}\Windows\win.ini", drive),
        r"\Windows\win.ini".into(),
        "/Windows/win.ini".into(),
        format!(r"{}Windows\win.ini", drive),
        format!(r"{}\..\..\..\..\..\..\..\..\Windows\win.ini", r),
        r"\\?\C:\Windows\win.ini".into(),
        r"\\localhost\C$\Windows\win.ini".into(),
    ];

    // The rules themselves.
    let access = Access::Root(root.clone());
    for path in out.iter().chain(&system) {
        assert!(files::resolve(&access, path).is_err(), "resolve {}", path);
        assert!(files::resolve_entry(&access, path).is_err(), "resolve_entry {}", path);
        assert!(files::resolve(&access, &format!(r"{}\{}", path, made)).is_err(), "resolve under {}", path);
    }
    // A path of the folder that Windows knows under another name is the
    // folder's: the drive in the other case, the short name when it has one.
    assert_eq!(files::resolve(&access, &format!(r"{}{}\trong", drive.to_lowercase(), r_rooted)).unwrap(), root.join("trong"));
    for path in [format!(r"{}\TRONG\CỦA TÔI.TXT", r), format!(r"{}\TRONG~1\CATI~1.TXT", r)] {
        if let Ok(real) = files::resolve(&access, &path) {
            assert!(real.starts_with(&root), "{} -> {}", path, real.display());
        }
    }

    // Through the listener: every request a client has.
    let host = confined(&root).await;
    let (mut c, first) = Client::open(host.address).await;
    assert_eq!(first.path, r);
    let mut id = 100;
    for path in &system {
        assert!(c.list(path).await.is_err(), "{}", path);
        id += 1;
        c.act(file_action::Union::AllFiles(proto::ReadAllFiles { id, path: path.clone(), include_hidden: true })).await;
        assert!(matches!(c.reply().await, Reply::Error(e) if e.id == id), "{}", path);
        id += 1;
        assert!(c.download(id, path, Some(0)).await.is_err(), "{}", path);
    }
    for path in &out {
        assert!(c.list(path).await.is_err(), "{}", path);
        id += 1;
        c.act(file_action::Union::AllFiles(proto::ReadAllFiles { id, path: path.clone(), include_hidden: true })).await;
        assert!(matches!(c.reply().await, Reply::Error(e) if e.id == id && !e.error.is_empty()), "{}", path);
        id += 1;
        assert!(c.download(id, path, Some(0)).await.is_err(), "{}", path);
        id += 1;
        assert!(c.done(id, file_action::Union::Create(proto::FileDirCreate { id, path: format!(r"{}\{}", path, made) })).await.is_err(), "{}", path);
        for target in [path.clone(), format!(r"{}\{}.txt", path, made)] {
            id += 1;
            match c.offer(id, &target, 4, T0, false).await {
                Reply::Error(e) => assert!(e.id == id && !e.error.is_empty(), "{}", target),
                other => panic!("{}: {:?}", target, other),
            }
        }
        id += 1;
        assert!(c.done(id, file_action::Union::RemoveFile(proto::FileRemoveFile { id, path: path.clone(), file_num: 0 })).await.is_err(), "{}", path);
        for recursive in [false, true] {
            id += 1;
            assert!(c.done(id, file_action::Union::RemoveDir(proto::FileRemoveDir { id, path: path.clone(), recursive })).await.is_err(), "{}", path);
        }
        id += 1;
        assert!(c.done(id, file_action::Union::Rename(proto::FileRename { id, path: path.clone(), new_name: made.clone() })).await.is_err(), "{}", path);
    }
    // The names of an upload's files, and a new name, in these forms.
    let mine = text(&root.join("trong/của tôi.txt"));
    for name in [format!("{}{}", drive, made), format!(r"{}\{}", o, made), format!(r"\\?\{}\{}", o, made), format!(r"\\localhost\{}${}\{}", &drive[..1], o_rooted, made), format!(r"..\{}\{}", o_name, made), format!(r"\{}", made)] {
        id += 1;
        c.act(file_action::Union::Receive(proto::FileTransferReceiveRequest {
            id,
            path: text(&root.join("trong")),
            files: vec![FileEntry { name: "được.txt".into(), ..Default::default() }, FileEntry { name: name.clone(), ..Default::default() }],
            file_num: 0,
            total_size: 2,
        }))
        .await;
        assert!(matches!(c.reply().await, Reply::Error(e) if e.id == id && !e.error.is_empty()), "{}", name);
        id += 1;
        assert!(c.done(id, file_action::Union::Rename(proto::FileRename { id, path: mine.clone(), new_name: name.clone() })).await.is_err(), "{}", name);
    }

    // Forms that name something inside the folder may be taken or refused;
    // what they make is inside it. A stream's name is cleaned from a file's
    // name (`files::safe_name`), a device's too.
    for path in [
        format!(r"{}\trong\của tôi.txt:luồng", r),
        format!(r"{}\trong\của tôi.txt::$DATA", r),
        format!(r"{}\trong::$INDEX_ALLOCATION", r),
        format!(r"{}\trong:$I30:$INDEX_ALLOCATION\{}", r, made),
        format!(r"{}\mới:luồng", r),
        format!(r"{}\TRONG\{}", r, made),
    ] {
        let _ = c.list(&path).await;
        // (A download of what may be a folder is not one file's.)
        if path.contains(".txt:") {
            id += 1;
            let _ = c.download(id, &path, Some(0)).await;
        }
        id += 1;
        let _ = c.done(id, file_action::Union::Create(proto::FileDirCreate { id, path: path.clone() })).await;
        id += 1;
        if let Reply::Confirm(_) = c.offer(id, &format!("{}.txt", path), 2, T0, false).await {
            c.send_blocks(id, b"ok").await.unwrap();
        }
    }
    for name in ["a:b.txt", "NUL", "con.txt", "x.txt::$DATA", "tên. . "] {
        id += 1;
        let _ = c.done(id, file_action::Union::Rename(proto::FileRename { id, path: mine.clone(), new_name: name.into() })).await;
        // Back to its name, whatever it is called now.
        let now = std::fs::read_dir(root.join("trong")).unwrap().flatten().find(|e| std::fs::read(e.path()).map(|d| d == b"duoc").unwrap_or(false)).expect("the file is still in its folder");
        assert!(!now.file_name().to_string_lossy().contains(':') && !now.file_name().to_string_lossy().ends_with(['.', ' ']), "{:?}", now.file_name());
        std::fs::rename(now.path(), root.join("trong/của tôi.txt")).unwrap();
    }

    // The connection is still good; nothing outside was read, touched or made.
    assert_eq!(c.download(999, &mine, Some(0)).await.unwrap().1, b"duoc");
    assert_eq!(snapshot(&outside), before);
    assert_eq!(std::fs::read(outside.join("bí mật.txt")).unwrap(), b"khong duoc doc");
    assert_eq!(mtime(&outside.join("bí mật.txt")), secret_time);
    assert_eq!(std::fs::read(outside.join("thư mục/trong.txt")).unwrap(), b"khong duoc xoa");
    let cwd = std::env::current_dir().unwrap();
    for dir in [root.parent().unwrap().to_path_buf(), outside.clone(), cwd.clone(), PathBuf::from(format!(r"{}\", drive)), PathBuf::from(format!(r"{}\", &text(&cwd)[..2]))] {
        for name in [made.clone(), format!("{}.txt", made), format!("{}.txt.download", made), format!("{}.txt.digest", made)] {
            assert!(std::fs::symlink_metadata(dir.join(&name)).is_err(), "{} was made in {}", name, dir.display());
        }
    }
    for (path, _) in snapshot(&root) {
        assert!(path.starts_with(&root));
    }
    assert!(root.join("trong/của tôi.txt").is_file());
}
