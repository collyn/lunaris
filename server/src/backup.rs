//! Backup and restore of the server's data.
//!
//! The data is a SQLite database (`lunaris.db`: accounts, computers, groups,
//! install tokens, TURN servers, settings) and a few files next to it in the
//! data folder: `server_token.txt`, `jwt_secret.txt`, `mfa_key.txt`,
//! `tls_cert.pem` + `tls_key.pem` and the gateway's config. A backup is one
//! `.tar.gz` with a consistent snapshot of the database (`VACUUM INTO`, also
//! while the server runs), those files and `backup.json` (what is in it,
//! with each file's SHA-256).
//!
//! **A backup holds secrets** (password hashes, the keys above): it is given
//! to administrators only and written readable by its owner only.
//!
//! Restoring is `import_into(.., replace = true)` (import.rs) from the
//! unpacked backup: what the server had is kept beside it as
//! `*.before-import-<time>`, nothing is deleted. A database is replaced
//! only while no server has it open:
//!
//! - `lunaris-server restore FILE` with the service stopped;
//! - from the web: the uploaded backup is checked whole, kept as
//!   `restore-pending.tar.gz` in the data folder and applied when the server
//!   starts next (before the database is opened). Under systemd or Docker
//!   the server ends itself for that and is started again.
//!
//! Only for the usual layout (the database is `lunaris.db` in the data
//! folder), and never a backup made by a newer server.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::import::{is_plain_file, KEY_FILES};

const MANIFEST: &str = "backup.json";
const DB: &str = "lunaris.db";
const GATEWAY_CONFIG: &str = "gateway/agent_config.json";
/// The uploaded backup that the next start applies.
pub const PENDING: &str = "restore-pending.tar.gz";
const FORMAT: u32 = 1;
const MAX_DB_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SMALL_BYTES: u64 = 4 * 1024 * 1024;
/// The largest backup taken from the web.
pub const MAX_UPLOAD_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// `backup.json`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub format: u32,
    /// The server that made it.
    pub version: String,
    /// UTC, RFC 3339.
    pub created_at: String,
    pub users: i64,
    pub hosts: i64,
    pub files: Vec<FileEntry>,
}

fn sha256_file(path: &Path) -> Result<(u64, String), String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash = sha2::Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hash.update(&buffer[..n]);
    }
    Ok((size, hex::encode(hash.finalize())))
}

/// A new folder only its owner can enter.
fn private_dir(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    let dir = parent.join(format!("{prefix}{}", crate::security::random_hex(6)));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// The names a backup may hold besides the manifest.
fn allowed(name: &str) -> bool {
    name == DB || name == GATEWAY_CONFIG || KEY_FILES.contains(&name)
}

/// Writes a backup of the data folder `data` to `out` (a `.tar.gz`,
/// readable by its owner only). Works while a server runs with the data.
pub async fn make(data: &Path, out: &Path) -> Result<Manifest, String> {
    let db = data.join(DB);
    if !is_plain_file(&db) {
        return Err(format!("{} has no {DB}: not a Lunaris server data folder", data.display()));
    }
    let work = private_dir(out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")), ".lunaris-backup-")?;
    let result = make_in(data, &db, &work, out).await;
    let _ = std::fs::remove_dir_all(&work);
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

async fn make_in(data: &Path, db: &Path, work: &Path, out: &Path) -> Result<Manifest, String> {
    // One consistent copy, whatever is in the WAL.
    let snapshot = work.join(DB);
    let mut conn = crate::import::open_read_only(db).await?;
    sqlx::query("VACUUM INTO ?")
        .bind(snapshot.to_string_lossy().into_owned())
        .execute(&mut conn)
        .await
        .map_err(|e| format!("cannot copy the database: {e}"))?;
    let _ = sqlx::Connection::close(conn).await;
    let users = crate::import::count(&snapshot, "users").await?;
    let hosts = crate::import::count(&snapshot, "hosts").await?;

    let mut names = vec![DB.to_string()];
    for name in KEY_FILES.iter().copied().chain([GATEWAY_CONFIG]) {
        let from = data.join(name);
        // Never through a link.
        if !is_plain_file(&from) {
            continue;
        }
        let to = work.join(name);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
        names.push(name.to_string());
    }
    let mut files = Vec::new();
    for name in &names {
        let (size, sha256) = sha256_file(&work.join(name))?;
        files.push(FileEntry { name: name.clone(), size, sha256 });
    }
    let manifest = Manifest {
        format: FORMAT,
        version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        users,
        hosts,
        files,
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;

    crate::paths::write_private(out, "").map_err(|e| format!("{}: {e}", out.display()))?;
    let file = std::fs::OpenOptions::new().write(true).truncate(true).open(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::default()));
    let failed = |e: std::io::Error| format!("{}: {e}", out.display());
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_json.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(chrono::Utc::now().timestamp().max(0) as u64);
    header.set_cksum();
    tar.append_data(&mut header, MANIFEST, manifest_json.as_slice()).map_err(failed)?;
    for name in &names {
        let mut source = std::fs::File::open(work.join(name)).map_err(failed)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(source.metadata().map_err(failed)?.len());
        header.set_mode(0o600);
        header.set_mtime(chrono::Utc::now().timestamp().max(0) as u64);
        header.set_cksum();
        tar.append_data(&mut header, name, &mut source).map_err(failed)?;
    }
    tar.into_inner().and_then(|gz| gz.finish()).and_then(|mut f| f.flush()).map_err(failed)?;
    Ok(manifest)
}

/// Unpacks the backup `archive` into the empty folder `into` and checks
/// it: only the files a backup holds, plain files, each as large and with
/// the SHA-256 its manifest says, a database among them, and not from a
/// newer server. Nothing of it is trusted before that.
pub fn unpack(archive: &Path, into: &Path) -> Result<Manifest, String> {
    let bad = |what: &str| format!("not a Lunaris backup: {what}");
    let file = std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut manifest: Option<Manifest> = None;
    let mut seen: Vec<String> = Vec::new();
    for entry in tar.entries().map_err(|e| bad(&e.to_string()))? {
        let entry = entry.map_err(|e| bad(&e.to_string()))?;
        let name = entry.path().map_err(|e| bad(&e.to_string()))?.to_string_lossy().replace('\\', "/");
        if !entry.header().entry_type().is_file() {
            return Err(bad(&format!("{name} is not a plain file")));
        }
        if name == MANIFEST {
            let mut text = String::new();
            entry.take(MAX_SMALL_BYTES).read_to_string(&mut text).map_err(|e| bad(&e.to_string()))?;
            manifest = Some(serde_json::from_str(&text).map_err(|e| bad(&format!("{MANIFEST}: {e}")))?);
            continue;
        }
        // Only these names: nothing is ever written outside `into`.
        if !allowed(&name) || seen.contains(&name) {
            return Err(bad(&format!("unexpected file {}", name.chars().take(80).collect::<String>())));
        }
        let limit = if name == DB { MAX_DB_BYTES } else { MAX_SMALL_BYTES };
        if entry.header().size().map_err(|e| bad(&e.to_string()))? > limit {
            return Err(bad(&format!("{name} is too large")));
        }
        let to = into.join(&name);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        crate::paths::write_private(&to, "").map_err(|e| format!("{}: {e}", to.display()))?;
        let mut out = std::fs::OpenOptions::new().write(true).truncate(true).open(&to).map_err(|e| format!("{}: {e}", to.display()))?;
        std::io::copy(&mut entry.take(limit), &mut out).map_err(|e| bad(&e.to_string()))?;
        seen.push(name);
    }
    let manifest = manifest.ok_or_else(|| bad(&format!("no {MANIFEST}")))?;
    if manifest.format != FORMAT {
        return Err(format!("this backup has format {}, which this server ({}) does not read: update the server", manifest.format, env!("CARGO_PKG_VERSION")));
    }
    if updater::version::is_newer(env!("CARGO_PKG_VERSION"), &manifest.version) {
        return Err(format!(
            "this backup was made by Lunaris {} and this server is {}: update the server before restoring it",
            manifest.version,
            env!("CARGO_PKG_VERSION")
        ));
    }
    if !seen.iter().any(|n| n == DB) {
        return Err(bad("no database in it"));
    }
    for name in &seen {
        let listed = manifest.files.iter().find(|f| &f.name == name).ok_or_else(|| bad(&format!("{name} is not in its list")))?;
        let (size, sha256) = sha256_file(&into.join(name))?;
        if size != listed.size || !sha256.eq_ignore_ascii_case(&listed.sha256) {
            return Err(bad(&format!("{name} is damaged (its checksum does not match)")));
        }
    }
    if let Some(missing) = manifest.files.iter().find(|f| !seen.contains(&f.name)) {
        return Err(bad(&format!("{} is listed but missing", missing.name)));
    }
    Ok(manifest)
}

/// What a restore did.
pub struct Restored {
    pub manifest: Manifest,
    pub imported: crate::import::Imported,
}

/// Replaces the data in `data` with the backup `archive`. No server may
/// have the database open. What was there is kept as `*.before-import-<time>`.
pub async fn restore(archive: &Path, data: &Path) -> Result<Restored, String> {
    std::fs::create_dir_all(data).map_err(|e| format!("{}: {e}", data.display()))?;
    let work = private_dir(data, ".lunaris-restore-")?;
    let result = async {
        let manifest = unpack(archive, &work)?;
        let imported = crate::import::import_into(&work, data, true).await?;
        Ok(Restored { manifest, imported })
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Checks an uploaded backup whole and keeps it for the next start.
pub async fn stage(upload: &Path, data: &Path) -> Result<Manifest, String> {
    let work = private_dir(data, ".lunaris-restore-check-")?;
    let checked = async {
        let manifest = unpack(upload, &work)?;
        if crate::import::count(&work.join(DB), "users").await? == 0 {
            return Err("this backup has no accounts: nothing to restore".to_string());
        }
        Ok(manifest)
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    let manifest = checked?;
    let pending = data.join(PENDING);
    let _ = std::fs::remove_file(&pending);
    std::fs::rename(upload, &pending).map_err(|e| format!("{}: {e}", pending.display()))?;
    Ok(manifest)
}

/// At start, before the database is opened: applies a backup kept by
/// [`stage`]. A backup that cannot be applied is set aside
/// (`restore-failed-<time>.tar.gz`) and the server starts with the data it had.
pub async fn apply_pending(data: &Path) {
    let pending = data.join(PENDING);
    if !is_plain_file(&pending) {
        return;
    }
    match restore(&pending, data).await {
        Ok(done) => {
            let _ = std::fs::remove_file(&pending);
            tracing::warn!(
                "Restored the backup of {} (Lunaris {}): {} account(s), {} computer(s). What this server had is kept as *.before-import-* in {}",
                done.manifest.created_at,
                done.manifest.version,
                done.imported.users,
                done.imported.hosts,
                data.display()
            );
        }
        Err(e) => {
            let aside = data.join(format!("restore-failed-{}.tar.gz", chrono::Utc::now().format("%Y%m%d-%H%M%S")));
            let _ = std::fs::rename(&pending, &aside);
            tracing::error!("The backup to restore could not be applied ({e}); the server keeps the data it had. The backup is kept as {}", aside.display());
        }
    }
}

/// The data folder when it has the usual layout (the database this server
/// uses is `lunaris.db` in it); else why backups are not made here.
pub fn data_folder(database_url: &str) -> Result<PathBuf, String> {
    let dir = crate::paths::data_dir().ok_or("this server has no data folder (LUNARIS_DATA_DIR is not set): backups need one")?;
    let path = database_url.strip_prefix("sqlite://").unwrap_or(database_url);
    let path = path.split('?').next().unwrap_or(path);
    let same = match (Path::new(path).canonicalize(), dir.join(DB).canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => Path::new(path) == dir.join(DB),
    };
    if !same {
        return Err(format!("this server's database is not {} (DATABASE_URL points elsewhere): back it up with the database's own tools", dir.join(DB).display()));
    }
    Ok(dir)
}

/// Whether something starts this server again when it ends (systemd,
/// Docker): then a restore from the web can restart it by itself.
pub fn restarts_by_itself() -> bool {
    supervised(std::process::id(), std::env::var("SYSTEMD_EXEC_PID").ok().as_deref())
}

/// This process is a systemd service's own process (`SYSTEMD_EXEC_PID` is
/// its pid: a program merely started from a service's shell inherits the
/// variable with another pid), or a container's first process.
fn supervised(pid: u32, systemd_exec_pid: Option<&str>) -> bool {
    cfg!(unix) && (pid == 1 || systemd_exec_pid.and_then(|p| p.trim().parse::<u32>().ok()) == Some(pid))
}

const USAGE: &str = "usage: lunaris-server backup [FILE.tar.gz]\n       lunaris-server restore FILE.tar.gz";

/// The data folder a command works on: LUNARIS_DATA_DIR, else the
/// installed service's (as root / Administrator).
fn command_data_dir(for_restore: bool) -> Result<(PathBuf, bool), String> {
    if let Some(dir) = std::env::var_os("LUNARIS_DATA_DIR").filter(|v| !v.is_empty()) {
        return Ok((PathBuf::from(dir), true));
    }
    #[cfg(unix)]
    if unsafe { libc::geteuid() } != 0 {
        return Err("this command needs root: run it with sudo".into());
    }
    if for_restore {
        crate::import::refuse_if_service_runs()?;
    }
    #[cfg(windows)]
    {
        Ok((crate::service::prepare_data_dir()?.0, false))
    }
    #[cfg(not(windows))]
    {
        Ok((PathBuf::from("/var/lib/lunaris"), false))
    }
}

/// `lunaris-server backup [FILE]`.
pub fn run_backup(args: &[String]) -> Result<(), String> {
    if args.len() > 1 || args.first().is_some_and(|a| a.starts_with('-')) {
        return Err(USAGE.into());
    }
    let (data, _) = command_data_dir(false)?;
    let out = args.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("lunaris-backup-{}.tar.gz", chrono::Utc::now().format("%Y%m%d-%H%M%S"))));
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let manifest = rt.block_on(make(&data, &out))?;
    println!("Backup of {} written to {}:", data.display(), out.display());
    println!("  {} account(s), {} computer(s), {} file(s), Lunaris {}", manifest.users, manifest.hosts, manifest.files.len(), manifest.version);
    println!("It holds this server's keys and password hashes: keep it somewhere safe.");
    println!("Restore it with: lunaris-server restore {}", out.display());
    Ok(())
}

/// `lunaris-server restore FILE`.
pub fn run_restore(args: &[String]) -> Result<(), String> {
    let [file] = args else { return Err(USAGE.into()) };
    if file.starts_with('-') {
        return Err(USAGE.into());
    }
    let (data, explicit) = command_data_dir(true)?;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let done = rt.block_on(restore(Path::new(file), &data))?;
    #[cfg(unix)]
    crate::import::give_to_service(&data, &done.imported.copied)?;
    #[cfg(windows)]
    if !explicit {
        crate::win_acl::secure_dir(&data)?;
    }
    println!(
        "Restored the backup of {} (Lunaris {}) into {}: {} account(s), {} computer(s).",
        done.manifest.created_at,
        done.manifest.version,
        data.display(),
        done.imported.users,
        done.imported.hosts
    );
    if let Some(backup) = &done.imported.backup {
        println!("The database that was there is kept as {}.", backup.display());
    }
    for name in &done.imported.kept {
        println!("A key file that was there is kept as {}.", data.join(name).display());
    }
    if !explicit {
        #[cfg(target_os = "linux")]
        println!("Start the service again: sudo systemctl start lunaris-server");
        #[cfg(windows)]
        println!("Start the service again: net start LunarisServer");
    }
    let _ = explicit;
    Ok(())
}

// ---- The web (administrators) -------------------------------------------------

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;

fn refused(status: StatusCode, text: &str) -> axum::response::Response {
    (status, Json(serde_json::json!({ "error": text }))).into_response()
}

/// GET /api/admin/server/backup: the backup, as a download.
pub async fn download(admin: crate::auth::AdminUser) -> axum::response::Response {
    let data = match data_folder(&crate::find_database_url()) {
        Ok(dir) => dir,
        Err(e) => return refused(StatusCode::CONFLICT, &e),
    };
    let name = format!("lunaris-backup-{}.tar.gz", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
    let out = data.join(format!(".backup-download-{}.tar.gz", crate::security::random_hex(6)));
    let made = make(&data, &out).await;
    let bytes = made.as_ref().ok().and_then(|_| std::fs::read(&out).ok());
    let _ = std::fs::remove_file(&out);
    match (made, bytes) {
        (Ok(manifest), Some(bytes)) => {
            tracing::info!("Admin {} downloaded a backup ({} account(s), {} computer(s), {} bytes)", admin.username, manifest.users, manifest.hosts, bytes.len());
            (
                [
                    (axum::http::header::CONTENT_TYPE, "application/gzip".to_string()),
                    (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
                    (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
                ],
                bytes,
            )
                .into_response()
        }
        (Err(e), _) => refused(StatusCode::INTERNAL_SERVER_ERROR, &format!("The backup could not be made: {e}")),
        (Ok(_), None) => refused(StatusCode::INTERNAL_SERVER_ERROR, "The backup could not be read back"),
    }
}

/// POST /api/admin/server/restore (the backup as the body): checked whole
/// and kept for the next start; answers what is in it and whether the
/// server restarts by itself now (`restart`: "now" or "manual").
pub async fn upload(admin: crate::auth::AdminUser, body: axum::body::Bytes) -> axum::response::Response {
    let data = match data_folder(&crate::find_database_url()) {
        Ok(dir) => dir,
        Err(e) => return refused(StatusCode::CONFLICT, &e),
    };
    if body.is_empty() {
        return refused(StatusCode::BAD_REQUEST, "Send the backup file");
    }
    let upload = data.join(format!(".restore-upload-{}.tar.gz", crate::security::random_hex(6)));
    if let Err(e) = crate::paths::write_private(&upload, "").and_then(|_| std::fs::write(&upload, &body)) {
        let _ = std::fs::remove_file(&upload);
        return refused(StatusCode::INTERNAL_SERVER_ERROR, &format!("The backup could not be kept: {e}"));
    }
    let manifest = match stage(&upload, &data).await {
        Ok(manifest) => manifest,
        Err(e) => {
            let _ = std::fs::remove_file(&upload);
            return refused(StatusCode::BAD_REQUEST, &e);
        }
    };
    let now = restarts_by_itself();
    tracing::warn!(
        "Admin {} uploaded a backup to restore (made {}, Lunaris {}, {} account(s), {} computer(s)); it is applied when the server starts next{}",
        admin.username,
        manifest.created_at,
        manifest.version,
        manifest.users,
        manifest.hosts,
        if now { ": restarting now" } else { "" }
    );
    if now {
        // After the answer is out: systemd / Docker start the server again.
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            std::process::exit(0);
        });
    }
    Json(serde_json::json!({
        "restart": if now { "now" } else { "manual" },
        "created_at": manifest.created_at,
        "version": manifest.version,
        "users": manifest.users,
        "hosts": manifest.hosts,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunaris-backup-{name}-{}", crate::security::random_hex(4)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A server data folder with one account named `user` and one computer.
    async fn server(dir: &Path, user: &str) {
        let url = format!("sqlite://{}?mode=rwc", dir.join(DB).display());
        let pool = crate::db::init_db(&url).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, 'h', 'admin')").bind(user).bind(user).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', ?, 'Offline')").bind(format!("pc of {user}")).execute(&pool).await.unwrap();
        pool.close().await;
        for name in KEY_FILES {
            std::fs::write(dir.join(name), format!("{name} of {user}")).unwrap();
        }
    }

    async fn users_in(dir: &Path) -> Vec<String> {
        let mut conn = crate::import::open_read_only(&dir.join(DB)).await.unwrap();
        sqlx::query_scalar("SELECT username FROM users ORDER BY username").fetch_all(&mut conn).await.unwrap()
    }

    #[tokio::test]
    async fn a_backup_restores_what_was_there_and_keeps_what_it_replaces() {
        let (old, new, out) = (temp("old"), temp("new"), temp("out"));
        server(&old, "alice").await;
        let archive = out.join("b.tar.gz");
        let manifest = make(&old, &archive).await.unwrap();
        assert_eq!((manifest.users, manifest.hosts, manifest.format), (1, 1, FORMAT));
        assert_eq!(manifest.files.len(), 1 + KEY_FILES.len());
        assert!(std::fs::read_dir(&out).unwrap().count() == 1, "no work folder left");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&archive).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // Into a server that has other data.
        server(&new, "bob").await;
        let done = restore(&archive, &new).await.unwrap();
        assert_eq!(done.manifest, manifest);
        assert_eq!(users_in(&new).await, ["alice"]);
        assert_eq!(std::fs::read_to_string(new.join("jwt_secret.txt")).unwrap(), "jwt_secret.txt of alice");
        // Bob's database and keys are still there, set aside.
        let kept = done.imported.backup.unwrap();
        let mut conn = crate::import::open_read_only(&kept).await.unwrap();
        let bob: Vec<String> = sqlx::query_scalar("SELECT username FROM users").fetch_all(&mut conn).await.unwrap();
        assert_eq!(bob, ["bob"]);
        assert_eq!(done.imported.kept.len(), KEY_FILES.len());
        assert!(!std::fs::read_dir(&new).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".lunaris-restore-")));
        for d in [old, new, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// A tar.gz with these entries (name, bytes).
    fn archive_of(path: &Path, entries: &[(&str, Vec<u8>)]) {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(std::fs::File::create(path).unwrap(), flate2::Compression::fast()));
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            // (`append_data` refuses `..`; the header is written as it is.)
            header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            tar.append(&header, bytes.as_slice()).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }

    #[tokio::test]
    async fn what_is_not_a_whole_backup_is_refused_and_changes_nothing() {
        let (old, new, out) = (temp("old2"), temp("new2"), temp("out2"));
        server(&old, "alice").await;
        server(&new, "bob").await;
        let good = out.join("good.tar.gz");
        let manifest = make(&old, &good).await.unwrap();
        let unpacked = temp("unpacked");
        unpack(&good, &unpacked).unwrap();
        let read = |name: &str| std::fs::read(unpacked.join(name)).unwrap();
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let cases: Vec<(&str, Vec<(&str, Vec<u8>)>, &str)> = vec![
            ("a file outside", vec![(MANIFEST, manifest_bytes.clone()), (DB, read(DB)), ("../evil.txt", b"x".to_vec())], "unexpected file"),
            ("another name", vec![(MANIFEST, manifest_bytes.clone()), (DB, read(DB)), ("server.env", b"x".to_vec())], "unexpected file"),
            ("a changed database", vec![(MANIFEST, manifest_bytes.clone()), (DB, b"not the database".to_vec())], "damaged"),
            ("no manifest", vec![(DB, read(DB))], "no backup.json"),
            ("no database", vec![(MANIFEST, manifest_bytes.clone())], "no database"),
            ("a listed file missing", vec![(MANIFEST, manifest_bytes.clone()), (DB, read(DB))], "listed but missing"),
        ];
        for (what, entries, says) in cases {
            let bad = out.join("bad.tar.gz");
            archive_of(&bad, &entries);
            let e = restore(&bad, &new).await.err().unwrap_or_else(|| panic!("{what} was restored"));
            assert!(e.contains(says), "{what}: {e}");
            assert_eq!(users_in(&new).await, ["bob"], "{what}");
            assert!(!out.join("evil.txt").exists() && !new.parent().unwrap().join("evil.txt").exists(), "{what}");
        }
        // Not a tar.gz at all; and a backup from a newer server.
        std::fs::write(out.join("junk.tar.gz"), b"hello").unwrap();
        assert!(restore(&out.join("junk.tar.gz"), &new).await.is_err());
        let newer = Manifest { version: "999.0.0".into(), ..manifest.clone() };
        archive_of(&out.join("newer.tar.gz"), &[(MANIFEST, serde_json::to_vec(&newer).unwrap()), (DB, read(DB))]);
        let e = restore(&out.join("newer.tar.gz"), &new).await.err().unwrap();
        assert!(e.contains("update the server"), "{e}");
        assert_eq!(users_in(&new).await, ["bob"]);
        assert_eq!(std::fs::read_to_string(new.join("jwt_secret.txt")).unwrap(), "jwt_secret.txt of bob");
        for d in [old, new, out, unpacked] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn only_a_supervised_server_ends_itself() {
        assert_eq!(supervised(4242, Some("4242")), cfg!(unix), "the service's own process");
        assert!(!supervised(4242, Some("17")), "started from a service's shell");
        assert!(!supervised(4242, None));
        assert!(!supervised(4242, Some("x")));
        assert_eq!(supervised(1, None), cfg!(unix), "a container's first process");
    }

    #[tokio::test]
    async fn an_uploaded_backup_waits_for_the_next_start() {
        let (old, new, out) = (temp("old3"), temp("new3"), temp("out3"));
        server(&old, "alice").await;
        server(&new, "bob").await;
        let archive = out.join("b.tar.gz");
        make(&old, &archive).await.unwrap();
        // Nothing waits: a start changes nothing.
        apply_pending(&new).await;
        assert_eq!(users_in(&new).await, ["bob"]);
        // Staged: still bob until the next start.
        let upload = new.join(".restore-upload-x.tar.gz");
        std::fs::copy(&archive, &upload).unwrap();
        let manifest = stage(&upload, &new).await.unwrap();
        assert_eq!(manifest.users, 1);
        assert!(new.join(PENDING).exists() && !upload.exists());
        assert_eq!(users_in(&new).await, ["bob"]);
        apply_pending(&new).await;
        assert_eq!(users_in(&new).await, ["alice"]);
        assert!(!new.join(PENDING).exists());
        // One that cannot be applied is set aside and the data stays.
        std::fs::write(new.join(PENDING), b"junk").unwrap();
        apply_pending(&new).await;
        assert_eq!(users_in(&new).await, ["alice"]);
        assert!(!new.join(PENDING).exists());
        assert!(std::fs::read_dir(&new).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("restore-failed-")));
        // A junk upload is refused at once and nothing waits.
        std::fs::write(&upload, b"junk").unwrap();
        assert!(stage(&upload, &new).await.is_err());
        assert!(!new.join(PENDING).exists());
        for d in [old, new, out] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
