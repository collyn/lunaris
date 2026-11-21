//! The host's side of RustDesk's file transfer: a RustDesk client's file
//! manager on this computer's files, through the listener.
//!
//! Ported from RustDesk 1.5.3 (AGPL-3.0, https://github.com/rustdesk/rustdesk):
//! `src/server/connection.rs` (the file-transfer login and the `FileAction`
//! and `FileResponse` branches of `on_message`), `src/ui_cm_interface.rs`
//! (`handle_fs` and its helpers) and `libs/base/src/fs.rs` (`read_dir`,
//! `TransferJob`, `is_write_need_confirmation`, `handle_read_jobs`, ...).
//! Each step names what it follows.
//!
//! A file-transfer connection is one of its own: the client logs in with
//! `LoginRequest.file_transfer` set, hears `PeerInfo` and the listing of the
//! folder it starts in, then sends `FileAction`s that are answered with
//! `FileResponse`s (and, for an upload, the other way round for the data).
//!
//! Off unless the agent's `rustdesk_file_transfer` is on (and its
//! `allow_file_transfer`): then the login is refused as a RustDesk host with
//! "Enable file transfer" off refuses it (session.rs).
//!
//! **Stricter than RustDesk.** A RustDesk host gives whoever is let in the
//! whole disk. Here every path the client names goes through the checks of
//! Lunaris's own file manager (`crate::files`: `resolve`, `resolve_entry`,
//! `safe_name`) before the file system sees it, so the agent's `file_access`
//! holds for RustDesk clients too:
//!
//! - with `file_access` "home" (the default) or a folder, nothing outside
//!   that folder is listed, read, written, renamed or removed: no `..`, no
//!   absolute path elsewhere, no link that leads out (such links are not
//!   listed either), not the folder itself for remove and rename;
//! - device paths (`\\.\`, `\\?\`), network paths (unless the whole disk is
//!   allowed), names ending in a dot or a space and Windows device names
//!   (CON, NUL, COM1, ...) are refused on every system;
//! - a path with a NUL is refused;
//! - the name of a file that is written or renamed to is cleaned as
//!   `safe_name` cleans it (control characters, `: * ? " < > |`, leading
//!   dots, device names), and a rename does not replace what is there;
//! - a listing of everything under a folder stops at `files::MAX_WALK`
//!   entries;
//! - a link is never written through (`.download`, `.digest` and the file
//!   itself) when access is limited to a folder.
//!
//! What is refused is answered with `FileResponse.error` (RustDesk's
//! `fs::new_error`), which the client's file manager shows: for a listing
//! with id 0 (the pending "read dir" fails at once instead of timing out),
//! for a job with the job's id.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;
use tracing::debug;

use super::clipboard::get_version_number;
use super::cursor;
use super::proto::{
    file_action, file_response, file_transfer_send_confirm_request as confirm, file_transfer_send_request, message, FileAction,
    FileDirectory, FileEntry, FileResponse, FileTransferBlock, FileTransferDigest, FileTransferDone, FileTransferError,
    FileTransferSendConfirmRequest, FileType, Message, ReadEmptyDirsResponse,
};
use crate::files::{self, Access};

/// libs/base/src/fs.rs `TransferJob::read`: `BUF_SIZE`, the size of a block.
const BLOCK: usize = 128 * 1024;
/// hbb_common/src/compress.rs `MAX_DECOMPRESSED_SIZE`.
const MAX_DECOMPRESSED_SIZE: usize = 256 * 1024 * 1024;
/// libs/base/src/fs.rs `can_enable_overwrite_detection`.
const MIN_VER_OVERWRITE_DETECTION: &str = "1.1.10";
/// The error of a recursive listing that grew past `files::MAX_WALK`.
const TOO_MANY: &str = "file transfer rejected: too many files";

// ------------------------------------------------------------- messages

fn response(union: file_response::Union) -> Message {
    Message { union: Some(message::Union::FileResponse(FileResponse { union: Some(union) })) }
}

/// libs/base/src/fs.rs `new_error`.
fn new_error(id: i32, error: impl ToString, file_num: i32) -> Message {
    response(file_response::Union::Error(FileTransferError { id, error: error.to_string(), file_num }))
}

/// libs/base/src/fs.rs `new_done`.
fn new_done(id: i32, file_num: i32) -> Message {
    response(file_response::Union::Done(FileTransferDone { id, file_num }))
}

/// libs/base/src/fs.rs `new_dir`.
fn new_dir(id: i32, path: String, entries: Vec<FileEntry>) -> Message {
    response(file_response::Union::Dir(FileDirectory { id, path, entries }))
}

/// libs/base/src/fs.rs `new_send_confirm`.
fn new_send_confirm(id: i32, file_num: i32, union: confirm::Union) -> Message {
    Message {
        union: Some(message::Union::FileAction(FileAction {
            union: Some(file_action::Union::SendConfirm(FileTransferSendConfirmRequest { id, file_num, union: Some(union) })),
        })),
    }
}

// ------------------------------------------------------------ fs.rs parts

/// libs/base/src/fs.rs `is_compressed_file`: blocks of these are sent as
/// they are.
fn is_compressed_file(name: &str) -> bool {
    let ext = name.rfind('.').map(|i| &name[i + 1..]).unwrap_or("");
    ["xz", "gz", "zip", "7z", "rar", "bz2", "tgz", "png", "jpg"].contains(&ext)
}

/// libs/base/src/fs.rs `get_string`.
fn get_string(path: &Path) -> String {
    path.to_str().unwrap_or("").to_owned()
}

/// libs/base/src/fs.rs `TransferJob::join`.
fn join(base: &Path, name: &str) -> PathBuf {
    if name.is_empty() {
        base.to_path_buf()
    } else {
        base.join(name)
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text = path.as_os_str().to_owned();
    text.push(".");
    text.push(suffix);
    PathBuf::from(text)
}

/// libs/base/src/fs.rs `validate_fs_path_argument`, and no NUL in any path
/// (RustDesk checks it where it calls this).
fn validate_path_argument(path: &str, arg_name: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err(format!("{} cannot be empty", arg_name));
    }
    no_nul(path, arg_name)
}

fn no_nul(path: &str, arg_name: &str) -> Result<(), String> {
    if path.bytes().any(|b| b == 0) {
        return Err(format!("{} contains null bytes", arg_name));
    }
    Ok(())
}

/// libs/base/src/fs.rs `validate_file_name_no_traversal`. The Windows
/// forms are refused on every system: the client may be one that writes
/// them, and nothing here is named that way.
fn validate_file_name_no_traversal(name: &str) -> Result<(), String> {
    if name.bytes().any(|b| b == 0) {
        return Err("file name contains null bytes".into());
    }
    if name.split(['/', '\\']).filter(|s| !s.is_empty()).any(|s| s == "..") {
        return Err("path traversal detected in file name".into());
    }
    let bytes = name.as_bytes();
    if (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':') || name.starts_with('/') || name.starts_with('\\') {
        return Err("absolute path detected in file name".into());
    }
    Ok(())
}

/// libs/base/src/fs.rs `validate_transfer_file_names`.
fn validate_transfer_file_names(files: &[FileEntry]) -> Result<(), String> {
    // "Single-file transfer may use an empty relative name, because the
    // destination file path is carried by transfer metadata."
    if files.len() == 1 && files[0].name.is_empty() {
        return Ok(());
    }
    for file in files {
        if file.name.is_empty() {
            return Err("empty file name in multi-file transfer".into());
        }
        validate_file_name_no_traversal(&file.name)?;
    }
    Ok(())
}

/// libs/base/src/fs.rs `validate_no_symlink_components`: no part of a
/// relative name that exists is a link.
fn validate_no_symlink_components(base: &Path, name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Ok(());
    }
    let mut current = base.to_path_buf();
    for component in Path::new(name).components() {
        match component {
            std::path::Component::Normal(seg) => {
                current.push(seg);
                match std::fs::symlink_metadata(&current) {
                    Ok(meta) if meta.file_type().is_symlink() => return Err("symlink path component is not allowed".into()),
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(format!("failed to validate path component: {}", err)),
                }
            }
            std::path::Component::CurDir => {}
            _ => return Err("invalid file name component".into()),
        }
    }
    Ok(())
}

/// The drives of a Windows computer, as libs/base/src/fs.rs `read_dir`
/// lists them for "/" (`FileType::DirDrive`, named `C:`); found as
/// `crate::files` finds them.
#[cfg(windows)]
fn drives() -> Vec<FileEntry> {
    (b'A'..=b'Z')
        .filter(|letter| Path::new(&format!("{}:\\", *letter as char)).exists())
        .map(|letter| FileEntry { name: format!("{}:", letter as char), entry_type: FileType::DirDrive as i32, ..Default::default() })
        .collect()
}

/// libs/base/src/fs.rs `read_dir`: the entries of `real`, listed as the
/// folder `shown` (the path as the client wrote it: its file manager waits
/// for a listing by that path). Stricter: a link that leads out of the
/// allowed folder is not listed (`files::inside`), as in Lunaris's own
/// listing.
fn read_dir(access: &Access, real: &Path, shown: &str, include_hidden: bool) -> std::io::Result<FileDirectory> {
    let mut dir = FileDirectory { id: 0, path: shown.to_owned(), entries: Vec::new() };
    for entry in real.read_dir()?.flatten() {
        let p = entry.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_owned();
        if name.is_empty() {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        let link = meta.file_type().is_symlink();
        if link && !files::inside(access, &p) {
            continue;
        }
        // docs.microsoft.com/en-us/windows/win32/fileio/file-attribute-constants
        #[cfg(windows)]
        let is_hidden = {
            use std::os::windows::fs::MetadataExt;
            meta.file_attributes() & 0x2 != 0
        };
        #[cfg(not(windows))]
        let is_hidden = name.starts_with('.');
        if is_hidden && !include_hidden {
            continue;
        }
        let (entry_type, size) = if p.is_dir() {
            (if link { FileType::DirLink } else { FileType::Dir }, 0)
        } else if link {
            (FileType::FileLink, 0)
        } else {
            (FileType::File, meta.len())
        };
        dir.entries.push(FileEntry { entry_type: entry_type as i32, name, is_hidden, size, modified_time: files::mtime_of(&meta) });
    }
    Ok(dir)
}

/// libs/base/src/fs.rs `read_dir_recursive` (`get_recursive_files`): the
/// files under `path`, named relative to it; links are not followed and
/// not listed. Stricter: not more than `files::MAX_WALK` of them.
fn read_dir_recursive(access: &Access, path: &Path, prefix: &Path, include_hidden: bool, count: &mut usize) -> Result<Vec<FileEntry>, String> {
    let mut out = Vec::new();
    if path.is_dir() {
        let dir = read_dir(access, path, "", include_hidden).map_err(|e| e.to_string())?;
        for entry in dir.entries {
            if entry.entry_type == FileType::File as i32 {
                *count += 1;
                if *count > files::MAX_WALK {
                    return Err(TOO_MANY.into());
                }
                let mut entry = entry;
                entry.name = get_string(&prefix.join(&entry.name));
                out.push(entry);
            } else if entry.entry_type == FileType::Dir as i32 {
                match read_dir_recursive(access, &path.join(&entry.name), &prefix.join(&entry.name), include_hidden, count) {
                    Ok(mut inner) => out.append(&mut inner),
                    Err(e) if e == TOO_MANY => return Err(e),
                    // fs.rs: a folder that cannot be read is left out.
                    Err(_) => {}
                }
            }
        }
        Ok(out)
    } else if path.is_file() {
        let (size, modified_time) = std::fs::metadata(path).map(|m| (m.len(), files::mtime_of(&m))).unwrap_or((0, 0));
        out.push(FileEntry { entry_type: FileType::File as i32, size, modified_time, ..Default::default() });
        Ok(out)
    } else {
        Err("Not exists".into())
    }
}

/// libs/base/src/fs.rs `read_empty_dirs_recursive`: the folders under
/// `path` that have nothing in them, each named from `shown`.
fn read_empty_dirs_recursive(access: &Access, path: &Path, shown: &Path, include_hidden: bool, count: &mut usize) -> Result<Vec<FileDirectory>, String> {
    let mut dirs = Vec::new();
    if path.is_dir() {
        *count += 1;
        if *count > files::MAX_WALK {
            return Err(TOO_MANY.into());
        }
        let dir = read_dir(access, path, &get_string(shown), include_hidden).map_err(|e| e.to_string())?;
        if dir.entries.is_empty() {
            dirs.push(dir);
        } else {
            for entry in &dir.entries {
                if entry.entry_type == FileType::Dir as i32 {
                    match read_empty_dirs_recursive(access, &path.join(&entry.name), &shown.join(&entry.name), include_hidden, count) {
                        Ok(mut inner) => dirs.append(&mut inner),
                        Err(e) if e == TOO_MANY => return Err(e),
                        Err(_) => {}
                    }
                }
            }
        }
        Ok(dirs)
    } else if path.is_file() {
        Ok(dirs)
    } else {
        Err("Not exists".into())
    }
}

/// libs/base/src/fs.rs `remove_all_empty_dir`: the folders under `path`
/// and the links in them are removed (a link itself, never what it leads
/// to), then `path`; files stay, and so does a folder that has one.
fn remove_all_empty_dir(path: &Path) -> Result<(), String> {
    // Every entry, the links that lead elsewhere too: they are removed.
    let dir = read_dir(&Access::Full, path, "", true).map_err(|e| e.to_string())?;
    for entry in &dir.entries {
        if entry.entry_type == FileType::Dir as i32 {
            let _ = remove_all_empty_dir(&path.join(&entry.name));
        } else if entry.entry_type == FileType::DirLink as i32 || entry.entry_type == FileType::FileLink as i32 {
            let link = path.join(&entry.name);
            // A link to a folder is a folder on Windows (a junction, a
            // directory link): `remove_file` refuses it, `remove_dir`
            // takes the link and leaves what it leads to.
            if std::fs::remove_file(&link).is_err() && cfg!(windows) && entry.entry_type == FileType::DirLink as i32 {
                let _ = std::fs::remove_dir(&link);
            }
        }
    }
    let _ = std::fs::remove_dir(path);
    Ok(())
}

/// libs/base/src/fs.rs `DigestCheckResult` (`IsSame` is never made there).
enum DigestCheck {
    NeedConfirm(FileTransferDigest),
    NoSuchFile,
}

/// libs/base/src/fs.rs `is_write_need_confirmation`: what the host has of
/// the file a client is about to send, at `target`.
fn is_write_need_confirmation(is_resume: bool, target: &Target, digest: &FileTransferDigest) -> Result<DigestCheck, String> {
    if is_resume && target.digest.exists() && target.download.exists() {
        // "If the digest file exists, it means the file was transferred
        // before. We can use the digest file to check whether the file is
        // the same."
        let local = std::fs::read_to_string(&target.digest).ok().and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok());
        if let Some(local) = local {
            let is_identical = local["modified"].as_u64() == Some(digest.last_modified) && local["size"].as_u64() == Some(digest.file_size);
            if is_identical {
                let transferred_size = std::fs::metadata(&target.download).map(|m| m.len()).unwrap_or(0);
                // "Only send confirmation if the file is not empty."
                if transferred_size > 0 {
                    return Ok(DigestCheck::NeedConfirm(FileTransferDigest {
                        id: digest.id,
                        file_num: digest.file_num,
                        last_modified: digest.last_modified,
                        file_size: digest.file_size,
                        is_identical,
                        transferred_size,
                        ..Default::default()
                    }));
                }
            }
        }
    }
    if target.path.exists() && target.path.is_file() {
        let meta = std::fs::metadata(&target.path).map_err(|e| e.to_string())?;
        let local = files::mtime_of(&meta);
        Ok(DigestCheck::NeedConfirm(FileTransferDigest {
            id: digest.id,
            file_num: digest.file_num,
            last_modified: local,
            file_size: meta.len(),
            is_identical: local == digest.last_modified && digest.file_size == meta.len(),
            ..Default::default()
        }))
    } else {
        Ok(DigestCheck::NoSuchFile)
    }
}

/// libs/base/src/fs.rs `TransferJob::read`: a block of a file, compressed
/// (hbb_common/src/compress.rs, zstd) when that makes it smaller.
fn block_of(id: i32, file_num: i32, data: &[u8], may_compress: bool) -> FileTransferBlock {
    let mut block = FileTransferBlock { id, file_num, data: Bytes::new(), compressed: false, blk_id: 0 };
    if may_compress && !data.is_empty() {
        let compressed = cursor::compress(data);
        if !compressed.is_empty() && compressed.len() < data.len() {
            block.data = Bytes::from(compressed);
            block.compressed = true;
            return block;
        }
    }
    block.data = Bytes::copy_from_slice(data);
    block
}

// ----------------------------------------------------------- confinement

/// Where the file of an upload goes: the file, and beside it what is
/// written until it is whole (libs/base/src/fs.rs `TransferJob::write`:
/// `<file>.download`) and what it will be (`<file>.digest`).
struct Target {
    path: PathBuf,
    download: PathBuf,
    digest: PathBuf,
}

/// A path the client names, for reading, listing or creating: checked as
/// Lunaris's file manager checks it (`files::resolve`: inside the allowed
/// folder, links followed and still inside). "" is the folder the client
/// starts in.
fn checked(access: &Access, start: &Path, path: &str) -> Result<PathBuf, String> {
    no_nul(path, "path")?;
    if path.is_empty() {
        return files::resolve(access, &start.to_string_lossy());
    }
    files::resolve(access, path)
}

/// The file `name` of an upload to `base` (`TransferJob::write`:
/// `join_validated_path(p, &entry.name)`), checked as `files.rs`
/// `begin_upload` checks an upload: the path inside the allowed folder, the
/// file's name cleaned (`files::safe_name`), and no link written through.
fn write_target(access: &Access, start: &Path, base: &str, name: &str) -> Result<Target, String> {
    validate_file_name_no_traversal(name)?;
    let base = checked(access, start, base)?;
    validate_no_symlink_components(&base, name)?;
    let path = checked(access, start, &join(&base, name).to_string_lossy())?;
    let dir = path.parent().ok_or("no folder")?;
    let name = files::safe_name(&path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let path = files::resolve(access, &dir.join(&name).to_string_lossy())?;
    if std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
        return Err(format!("{} is a folder", name));
    }
    let target = Target { download: with_suffix(&path, "download"), digest: with_suffix(&path, "digest"), path };
    if let Access::Root(_) = access {
        for p in [&target.path, &target.download, &target.digest] {
            if std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                return Err(format!("{} is a link: not written through", name));
            }
        }
    }
    Ok(target)
}

// ------------------------------------------------------------------ jobs

/// libs/base/src/fs.rs `TransferJob`, of a job that reads (the client
/// downloads).
struct ReadJob {
    id: i32,
    /// The folder or the file, checked (its real path).
    base: PathBuf,
    files: Vec<FileEntry>,
    file_num: i32,
    stream: Option<std::fs::File>,
    /// `enable_overwrite_detection`: each file waits for the client's word.
    overwrite_detection: bool,
    confirmed: bool,
    waiting: bool,
    skipped: bool,
}

/// What a turn of a reading job gave.
pub(crate) enum Step {
    /// A message for the client; there may be more.
    Send(Message),
    /// Nothing now: a job waits for the client's confirm.
    Idle,
}

impl ReadJob {
    /// The file in turn, checked again where it is now.
    fn open(&self, access: &Access) -> Result<std::fs::File, String> {
        let name = &self.files[self.file_num as usize].name;
        let path = files::resolve(access, &join(&self.base, name).to_string_lossy())?;
        files::open_inside(access, &path, std::fs::OpenOptions::new().read(true))
    }

    /// `TransferJob::set_file_skipped`.
    fn set_file_skipped(&mut self) {
        self.stream = None;
        self.confirmed = false;
        self.waiting = false;
        self.file_num += 1;
        self.skipped = true;
    }

    /// The next file after an error (`open_data_stream`, `read`).
    fn next_file(&mut self) {
        self.file_num += 1;
        self.stream = None;
        self.confirmed = false;
        self.waiting = false;
    }

    /// `TransferJob::confirm`, for a job that reads.
    fn confirm(&mut self, access: &Access, r: &FileTransferSendConfirmRequest) {
        if self.file_num != r.file_num {
            return;
        }
        match r.union {
            Some(confirm::Union::Skip(true)) => self.set_file_skipped(),
            // `set_file_confirmed(true)`.
            Some(confirm::Union::Skip(false)) => {
                self.confirmed = true;
                self.skipped = false;
            }
            Some(confirm::Union::OffsetBlk(offset)) => {
                self.confirmed = true;
                self.skipped = false;
                // `set_stream_offset`: the client has the start of the file
                // and wants the rest ("seek (reader) to the offset").
                if offset > 0 && (r.file_num as usize) < self.files.len() {
                    if let Ok(mut file) = self.open(access) {
                        if file.seek(std::io::SeekFrom::Start(offset as u64)).is_ok() {
                            self.stream = Some(file);
                        }
                    }
                }
            }
            None => {}
        }
    }

    /// One turn of libs/base/src/fs.rs `handle_read_jobs` for this job:
    /// `init_data_stream`, then `read`. `Err` when the job is over, with
    /// its last message.
    fn step(&mut self, access: &Access) -> Result<Step, Message> {
        let id = self.id;
        // `init_data_stream`: `open_data_stream`, then the digest.
        if (self.file_num as usize) < self.files.len() {
            if self.stream.is_none() {
                match self.open(access) {
                    Ok(file) => {
                        self.stream = Some(file);
                        self.confirmed = false;
                        self.waiting = false;
                    }
                    // "On open error, behave the same as validation
                    // failure: advance to next file and return the error."
                    Err(e) => {
                        self.next_file();
                        return Ok(Step::Send(new_error(id, e, self.file_num)));
                    }
                }
            }
            if self.overwrite_detection && !self.confirmed && !self.waiting {
                // `send_current_digest`.
                let meta = self.stream.as_ref().and_then(|f| f.metadata().ok());
                let Some(meta) = meta else {
                    self.next_file();
                    return Ok(Step::Send(new_error(id, "file is None", self.file_num)));
                };
                self.waiting = true;
                return Ok(Step::Send(response(file_response::Union::Digest(FileTransferDigest {
                    id,
                    file_num: self.file_num,
                    last_modified: files::mtime_of(&meta),
                    file_size: meta.len(),
                    ..Default::default()
                }))));
            }
        } else {
            // "job done"
            self.stream = None;
        }
        // `read`: nothing while the file is not confirmed, and nothing
        // after the last file.
        if (self.overwrite_detection && !self.confirmed) || (self.file_num as usize) >= self.files.len() {
            // `job_completed`: not waiting for a confirm.
            if !self.overwrite_detection || (!self.confirmed && !self.waiting) {
                // `job_error`: "skipped" when the one file was skipped.
                return Err(if self.skipped && self.files.len() == 1 {
                    new_error(id, "skipped", self.file_num)
                } else {
                    new_done(id, self.file_num)
                });
            }
            return Ok(Step::Idle);
        }
        let file_num = self.file_num;
        let name = if self.files.len() == 1 && self.files[0].name.is_empty() {
            self.base.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string()
        } else {
            self.files[file_num as usize].name.clone()
        };
        let mut buf = vec![0u8; BLOCK];
        let mut filled = 0;
        while filled < BLOCK {
            let read = match self.stream.as_mut() {
                Some(file) => file.read(&mut buf[filled..]),
                None => Err(std::io::Error::other("data stream is None")),
            };
            match read {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) => {
                    self.next_file();
                    return Ok(Step::Send(new_error(id, e, self.file_num)));
                }
            }
        }
        // The end of a file is an empty block, and the next file's turn.
        if filled == 0 {
            self.next_file();
        }
        let block = block_of(id, file_num, &buf[..filled], !is_compressed_file(&name));
        Ok(Step::Send(response(file_response::Union::Block(block))))
    }
}

/// libs/base/src/fs.rs `TransferJob`, of a job that writes (the client
/// uploads): ui_cm_interface.rs `FS::NewWrite`.
struct WriteJob {
    id: i32,
    /// `FileTransferReceiveRequest.path`, as the client wrote it; checked
    /// with each file's name (`write_target`).
    base: String,
    files: Vec<FileEntry>,
    file_num: i32,
    /// The file being written (its `.download`) and where it goes.
    stream: Option<(std::fs::File, Target)>,
    /// fs.rs `FileDigest`: size and modified time of the file in turn.
    digest: (u64, u64),
}

impl WriteJob {
    fn target(&self, access: &Access, start: &Path, file_num: i32) -> Result<Target, String> {
        let entry = self.files.get(file_num as usize).ok_or("Wrong file number")?;
        write_target(access, start, &self.base, &entry.name)
    }

    /// `TransferJob::modify_time`: the file that was being written is whole:
    /// its `.digest` goes, the `.download` becomes the file, with the
    /// client's time. (RustDesk also does this for a file it has not opened
    /// in this job, which would make a file of what an earlier, broken
    /// transfer left; not here.)
    fn finish_file(&mut self) {
        let Some((file, target)) = self.stream.take() else { return };
        let _ = file.sync_all();
        if let Some(entry) = self.files.get(self.file_num as usize) {
            if entry.modified_time > 0 {
                let _ = file.set_modified(UNIX_EPOCH + Duration::from_secs(entry.modified_time));
            }
        }
        drop(file);
        let _ = std::fs::remove_file(&target.digest);
        let _ = std::fs::rename(&target.download, &target.path);
    }

    /// `TransferJob::remove_download_file`.
    fn remove_download_file(&mut self, access: &Access, start: &Path) {
        let target = match self.stream.take() {
            Some((_, target)) => Some(target),
            None => self.target(access, start, self.file_num).ok(),
        };
        if let Some(target) = target {
            let _ = std::fs::remove_file(&target.download);
            let _ = std::fs::remove_file(&target.digest);
        }
    }

    /// `TransferJob::write`.
    fn write(&mut self, access: &Access, start: &Path, block: FileTransferBlock) -> Result<(), String> {
        if block.id != self.id {
            return Err("Wrong id".into());
        }
        if block.file_num < 0 || block.file_num as usize >= self.files.len() {
            return Err("Wrong file number".into());
        }
        if block.file_num != self.file_num || self.stream.is_none() {
            self.finish_file();
            self.file_num = block.file_num;
            let target = self.target(access, start, block.file_num)?;
            if let Some(parent) = target.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if target.digest.exists() {
                std::fs::remove_file(&target.digest).map_err(|e| e.to_string())?;
            }
            let file = files::open_inside(access, &target.download, std::fs::OpenOptions::new().write(true).create(true).truncate(true))?;
            if let Ok(mut digest) = files::open_inside(access, &target.digest, std::fs::OpenOptions::new().write(true).create(true).truncate(true)) {
                let _ = digest.write_all(serde_json::json!({"size": self.digest.0, "modified": self.digest.1}).to_string().as_bytes());
            }
            self.stream = Some((file, target));
        }
        let data = if block.compressed {
            // hbb_common `decompress` gives nothing for what it cannot
            // read, and RustDesk writes that nothing; a file with a hole is
            // not the file, so here the transfer fails.
            Bytes::from(cursor::decompress(&block.data, MAX_DECOMPRESSED_SIZE).ok_or("a block of the file could not be read")?)
        } else {
            block.data
        };
        let (file, _) = self.stream.as_mut().ok_or("file is None")?;
        file.write_all(&data).map_err(|e| e.to_string())
    }

    /// `TransferJob::confirm`, for a job that writes.
    fn confirm(&mut self, access: &Access, start: &Path, r: &FileTransferSendConfirmRequest) {
        if self.file_num != r.file_num {
            return;
        }
        match r.union {
            // `set_file_skipped`.
            Some(confirm::Union::Skip(true)) => {
                self.stream = None;
                self.file_num += 1;
            }
            // `set_stream_offset`: "If both download and digest files
            // exist, seek (writer) to the offset".
            Some(confirm::Union::OffsetBlk(offset)) if offset > 0 => {
                let Ok(target) = self.target(access, start, r.file_num) else { return };
                if target.download.exists() && target.digest.exists() {
                    if let Ok(mut file) = std::fs::File::options().write(true).open(&target.download) {
                        if file.seek(std::io::SeekFrom::Start(offset as u64)).is_ok() {
                            self.stream = Some((file, target));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ host

/// The files of this computer for one file-transfer connection.
pub(crate) struct FileHost {
    access: Access,
    /// The folder the client starts in ("" from it).
    start: PathBuf,
    /// libs/base/src/fs.rs `can_enable_overwrite_detection` of the
    /// client's version.
    overwrite_detection: bool,
    read_jobs: Vec<ReadJob>,
    write_jobs: Vec<WriteJob>,
}

impl FileHost {
    /// `access`: the agent's `file_access`. `home`: the folder to start in
    /// when it is not the one `access` gives. `Err` (the words for the
    /// client) when `access` names no usable folder.
    pub(crate) fn new(access: Access, home: Option<PathBuf>, client_version: &str) -> Result<FileHost, String> {
        if let Access::Denied(reason) = &access {
            return Err(reason.clone());
        }
        let start = home.unwrap_or_else(|| access.start());
        Ok(FileHost {
            access,
            start,
            overwrite_detection: get_version_number(client_version) >= get_version_number(MIN_VER_OVERWRITE_DETECTION),
            read_jobs: Vec::new(),
            write_jobs: Vec::new(),
        })
    }

    pub(crate) fn has_read_jobs(&self) -> bool {
        !self.read_jobs.is_empty()
    }

    /// src/server/connection.rs `send_logon_response`, the file-transfer
    /// branch: the listing a client gets by itself after the login, of
    /// `FileTransfer.dir` when that is a folder it may see, else of the
    /// folder it starts in.
    pub(crate) async fn first_listing(&self, dir: &str, show_hidden: bool) -> Message {
        let allowed = !dir.is_empty() && checked(&self.access, &self.start, dir).map(|p| p.is_dir()).unwrap_or(false);
        self.read_dir(if allowed { dir } else { "" }, show_hidden).await
    }

    /// ui_cm_interface.rs `read_dir`: "" is the home folder (here: the
    /// folder the client starts in); the error has no job (id 0).
    async fn read_dir(&self, dir: &str, include_hidden: bool) -> Message {
        // libs/base/src/fs.rs `read_dir`: "/" on Windows is the list of
        // drives; where access is limited to a folder there is no such list.
        #[cfg(windows)]
        if dir == "/" && self.access == Access::Full {
            return new_dir(0, "/".into(), drives());
        }
        let shown = if dir.is_empty() { get_string(&self.start) } else { dir.to_owned() };
        let real = match checked(&self.access, &self.start, dir) {
            Ok(real) => real,
            Err(e) => return self.refused(0, e, -1),
        };
        let access = self.access.clone();
        match tokio::task::spawn_blocking(move || read_dir(&access, &real, &shown, include_hidden)).await {
            Ok(Ok(dir)) => response(file_response::Union::Dir(dir)),
            Ok(Err(e)) => new_error(0, e, -1),
            Err(e) => new_error(0, e, -1),
        }
    }

    /// A request this host does not serve where a RustDesk host would: the
    /// client is told why. Only the fact is logged, never the path.
    fn refused(&self, id: i32, reason: String, file_num: i32) -> Message {
        debug!("RustDesk file session: a request was refused (job {})", id);
        new_error(id, reason, file_num)
    }

    /// ui_cm_interface.rs `handle_result`.
    fn result(result: Result<(), String>, id: i32, file_num: i32) -> Message {
        match result {
            Ok(()) => new_done(id, file_num),
            Err(e) => new_error(id, e, file_num),
        }
    }

    /// The files under `path` (fs.rs `get_recursive_files`), away from the
    /// connection's task.
    async fn recursive_files(&self, real: PathBuf, include_hidden: bool) -> Result<Vec<FileEntry>, String> {
        let access = self.access.clone();
        tokio::task::spawn_blocking(move || read_dir_recursive(&access, &real, Path::new(""), include_hidden, &mut 0))
            .await
            .map_err(|e| format!("task failed: {}", e))?
    }

    /// src/server/connection.rs `on_message`, `Message::FileAction` on a
    /// file-transfer connection, with what ui_cm_interface.rs `handle_fs`
    /// does for each. What the client is to hear is returned.
    pub(crate) async fn on_action(&mut self, action: FileAction) -> Vec<Message> {
        let Some(union) = action.union else { return Vec::new() };
        let (access, start) = (self.access.clone(), self.start.clone());
        match union {
            file_action::Union::ReadDir(rd) => vec![self.read_dir(&rd.path, rd.include_hidden).await],
            // ui_cm_interface.rs `read_empty_dirs`. A RustDesk host says
            // nothing when it cannot; here the client hears of no empty
            // folders, so that it does not wait.
            file_action::Union::ReadEmptyDirs(rd) => {
                let empty_dirs = match checked(&access, &start, &rd.path) {
                    Ok(real) if !rd.path.is_empty() => {
                        let (shown, hidden) = (PathBuf::from(&rd.path), rd.include_hidden);
                        tokio::task::spawn_blocking(move || read_empty_dirs_recursive(&access, &real, &shown, hidden, &mut 0))
                            .await
                            .ok()
                            .and_then(Result::ok)
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                vec![response(file_response::Union::EmptyDirs(ReadEmptyDirsResponse { path: rd.path, empty_dirs }))]
            }
            // `fs::get_recursive_files`, then `fs::new_dir`.
            file_action::Union::AllFiles(f) => {
                let listed = match validate_path_argument(&f.path, "path").and_then(|_| checked(&access, &start, &f.path)) {
                    Ok(real) => self.recursive_files(real, f.include_hidden).await,
                    Err(e) => return vec![self.refused(f.id, e, -1)],
                };
                vec![match listed {
                    Ok(files) => new_dir(f.id, f.path, files),
                    Err(e) => new_error(f.id, e, -1),
                }]
            }
            // "server to client": `create_and_start_read_job`
            // (`TransferJob::new_read`) and `process_new_read_job`.
            file_action::Union::Send(s) => {
                // A printer job reads what this host printed: there is none.
                if s.file_type == file_transfer_send_request::FileType::Printer as i32 {
                    return Vec::new();
                }
                let real = match validate_path_argument(&s.path, "path").and_then(|_| checked(&access, &start, &s.path)) {
                    Ok(real) => real,
                    Err(e) => return vec![self.refused(s.id, e, 0)],
                };
                match self.recursive_files(real.clone(), s.include_hidden).await {
                    Err(e) if e == TOO_MANY => vec![new_error(s.id, e, -1)],
                    Err(e) => vec![new_error(s.id, e, 0)],
                    Ok(files) => {
                        debug!("RustDesk file session: read job {} ({} files)", s.id, files.len());
                        self.read_jobs.push(ReadJob {
                            id: s.id,
                            base: real,
                            files: files.clone(),
                            file_num: s.file_num,
                            stream: None,
                            overwrite_detection: self.overwrite_detection,
                            confirmed: false,
                            waiting: false,
                            skipped: false,
                        });
                        vec![new_dir(s.id, s.path, files)]
                    }
                }
            }
            // "client to server": ui_cm_interface.rs `FS::NewWrite`
            // (`TransferJob::new_write`, `set_files`). Every file's place
            // is checked now, before any of them is sent.
            file_action::Union::Receive(r) => {
                let job = WriteJob { id: r.id, base: r.path, files: r.files, file_num: r.file_num, stream: None, digest: (0, 0) };
                let valid = validate_transfer_file_names(&job.files)
                    .and_then(|_| (0..job.files.len() as i32).try_for_each(|n| job.target(&access, &start, n).map(|_| ())));
                if let Err(e) = valid {
                    return vec![self.refused(job.id, e, job.file_num)];
                }
                debug!("RustDesk file session: write job {} ({} files)", job.id, job.files.len());
                // A job of that id that was left is over.
                self.write_jobs.retain(|j| j.id != job.id);
                self.write_jobs.push(job);
                Vec::new()
            }
            // ui_cm_interface.rs `remove_dir`: `fs::remove_all_empty_dir`
            // or `std::fs::remove_dir`. The folder itself is the entry
            // (`files::resolve_entry`: a link is not followed, and the
            // allowed folder is not removed).
            file_action::Union::RemoveDir(d) => {
                let result = match validate_path_argument(&d.path, "directory path").and_then(|_| files::resolve_entry(&access, &d.path)) {
                    Err(e) => return vec![self.refused(d.id, e, 0)],
                    Ok(path) if d.recursive => tokio::task::spawn_blocking(move || remove_all_empty_dir(&path))
                        .await
                        .unwrap_or_else(|e| Err(format!("task failed: {}", e))),
                    Ok(path) => std::fs::remove_dir(&path).map_err(|e| e.to_string()),
                };
                vec![Self::result(result, d.id, 0)]
            }
            // ui_cm_interface.rs `remove_file`: `fs::remove_file`.
            file_action::Union::RemoveFile(f) => {
                let result = match validate_path_argument(&f.path, "file path").and_then(|_| files::resolve_entry(&access, &f.path)) {
                    Err(e) => return vec![self.refused(f.id, e, f.file_num)],
                    Ok(path) => std::fs::remove_file(&path).map_err(|e| e.to_string()),
                };
                vec![Self::result(result, f.id, f.file_num)]
            }
            // ui_cm_interface.rs `create_dir`: `fs::create_dir`.
            file_action::Union::Create(c) => {
                let result = match validate_path_argument(&c.path, "directory path").and_then(|_| checked(&access, &start, &c.path)) {
                    Err(e) => return vec![self.refused(c.id, e, 0)],
                    Ok(path) => std::fs::create_dir_all(&path).map_err(|e| e.to_string()),
                };
                vec![Self::result(result, c.id, 0)]
            }
            // ui_cm_interface.rs `rename_file`: `fs::rename_file`, with
            // what `files.rs` `rename` adds: the entry itself (not what a
            // link leads to), the new name cleaned, and nothing replaced.
            file_action::Union::Rename(r) => {
                let result = (|| {
                    validate_path_argument(&r.path, "path")?;
                    if r.new_name.is_empty() {
                        return Err("new file name cannot be empty".to_string());
                    }
                    validate_file_name_no_traversal(&r.new_name)?;
                    let path = files::resolve_entry(&access, &r.path)?;
                    if std::fs::symlink_metadata(&path).is_err() {
                        return Err(format!("{:?} not exists", path.file_name().unwrap_or_default()));
                    }
                    let name = files::safe_name(&r.new_name);
                    let dir = path.parent().ok_or("Parent directory not exists")?;
                    let dest = files::resolve(&access, &dir.join(&name).to_string_lossy())?;
                    if std::fs::symlink_metadata(&dest).is_ok() {
                        return Err(format!("{} already exists", name));
                    }
                    std::fs::rename(&path, dest).map_err(|e| e.to_string())
                })();
                vec![Self::result(result, r.id, 0)]
            }
            // `FS::CancelWrite` (`remove_download_file`) and `FS::CancelRead`.
            file_action::Union::Cancel(c) => {
                if let Some(i) = self.write_jobs.iter().position(|j| j.id == c.id) {
                    self.write_jobs.remove(i).remove_download_file(&access, &start);
                }
                self.read_jobs.retain(|j| j.id != c.id);
                Vec::new()
            }
            // `fs::get_job(r.id, &mut self.read_jobs)` and `job.confirm`,
            // else `FS::SendConfirm` for the job that writes.
            file_action::Union::SendConfirm(r) => {
                if let Some(job) = self.read_jobs.iter_mut().find(|j| j.id == r.id) {
                    job.confirm(&access, &r);
                } else if let Some(job) = self.write_jobs.iter_mut().find(|j| j.id == r.id) {
                    job.confirm(&access, &start, &r);
                }
                Vec::new()
            }
        }
    }

    /// src/server/connection.rs `on_message`, `Message::FileResponse` (the
    /// data of an upload), with ui_cm_interface.rs `FS::WriteBlock`,
    /// `WriteDone`, `WriteError` and `CheckDigest`.
    pub(crate) fn on_response(&mut self, fr: FileResponse) -> Vec<Message> {
        let (access, start) = (&self.access, &self.start);
        match fr.union {
            Some(file_response::Union::Block(block)) => {
                let (id, file_num) = (block.id, block.file_num);
                if let Some(i) = self.write_jobs.iter().position(|j| j.id == id) {
                    if let Err(e) = self.write_jobs[i].write(access, start, block) {
                        // RustDesk keeps the job and answers every block
                        // that follows with the error again; here the job
                        // is over with the first (its part stays, as when
                        // a connection breaks).
                        self.write_jobs.remove(i);
                        return vec![new_error(id, e, file_num)];
                    }
                }
                Vec::new()
            }
            Some(file_response::Union::Done(d)) => {
                if let Some(i) = self.write_jobs.iter().position(|j| j.id == d.id) {
                    let mut job = self.write_jobs.remove(i);
                    job.finish_file();
                    debug!("RustDesk file session: write job {} done", d.id);
                    return vec![new_done(d.id, d.file_num)];
                }
                Vec::new()
            }
            Some(file_response::Union::Error(e)) => {
                if let Some(i) = self.write_jobs.iter().position(|j| j.id == e.id) {
                    self.write_jobs.remove(i);
                    return vec![new_error(e.id, e.error, e.file_num)];
                }
                Vec::new()
            }
            Some(file_response::Union::Digest(d)) => {
                let Some(job) = self.write_jobs.iter_mut().find(|j| j.id == d.id) else { return Vec::new() };
                if job.files.get(d.file_num as usize).is_none() {
                    return Vec::new();
                }
                let check = job.target(access, start, d.file_num).and_then(|target| is_write_need_confirmation(d.is_resume, &target, &d));
                vec![match check {
                    Err(e) => new_error(d.id, e, d.file_num),
                    Ok(check) => {
                        // `job.set_digest(file_size, last_modified)`.
                        job.digest = (d.file_size, d.last_modified);
                        match check {
                            // "upload to server, but server has the same
                            // file, request"
                            DigestCheck::NeedConfirm(mut digest) => {
                                digest.is_upload = true;
                                response(file_response::Union::Digest(digest))
                            }
                            DigestCheck::NoSuchFile => new_send_confirm(d.id, d.file_num, confirm::Union::OffsetBlk(0)),
                        }
                    }
                }]
            }
            _ => Vec::new(),
        }
    }

    /// libs/base/src/fs.rs `handle_read_jobs`: one turn of the first job
    /// ("Break to handle jobs one by one").
    pub(crate) fn next_read(&mut self) -> Step {
        let Some(job) = self.read_jobs.first_mut() else { return Step::Idle };
        match job.step(&self.access) {
            Ok(step) => step,
            Err(last) => {
                debug!("RustDesk file session: read job {} over", job.id);
                self.read_jobs.remove(0);
                Step::Send(last)
            }
        }
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
pub(crate) mod tests;
