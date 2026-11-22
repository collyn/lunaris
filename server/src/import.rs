//! `lunaris-server import <dir> [--replace]`: takes over what the desktop
//! app's built-in server had (accounts, computers, groups, keys) into this
//! service's data folder.
//!
//! The app kept that server's data in `<per-user data dir>/lunaris-server`
//! (client-qml/src/local_server.rs: `~/.local/share/lunaris-server` on Linux,
//! `%APPDATA%\lunaris-server` on Windows, `~/Library/Application
//! Support/lunaris-server` on macOS).
//!
//! Copied: the database (as one consistent snapshot, also while the app's
//! server runs), and the files that make it usable: `server_token.txt`
//! (agents keep connecting with their token), `mfa_key.txt` (two-factor
//! secrets are encrypted with it), `jwt_secret.txt` (sign-ins stay valid),
//! `tls_cert.pem` + `tls_key.pem` (browsers keep the certificate they
//! accepted), and the computers added through the server's gateway (the
//! `sources` of `gateway/agent_config.json`, gateway.rs), added to the
//! service's gateway config. Not copied: logs, `server.env`, and old
//! first-admin password or setup code files.

use std::path::{Path, PathBuf};

/// Files copied next to the database when present (see the module comment).
const KEY_FILES: &[&str] = &["server_token.txt", "mfa_key.txt", "jwt_secret.txt", "tls_cert.pem", "tls_key.pem"];

const USAGE: &str = "usage: lunaris-server import <data folder of the app's server> [--replace]";

struct Args {
    source: PathBuf,
    replace: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut source = None;
    let mut replace = false;
    for a in args {
        match a.as_str() {
            "--replace" => replace = true,
            s if s.starts_with('-') => return Err(format!("unknown option: {s}\n{USAGE}")),
            s if source.is_none() => source = Some(PathBuf::from(s)),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Args { source: source.ok_or(USAGE)?, replace })
}

#[derive(Debug)]
pub struct Imported {
    /// Names written in the target folder.
    pub copied: Vec<String>,
    /// The target's earlier database, kept under this name.
    pub backup: Option<PathBuf>,
    /// The target's earlier key files, kept as `<name>.before-import-<time>`
    /// next to it.
    pub kept: Vec<String>,
    pub users: i64,
    pub hosts: i64,
    /// Computers of the gateway taken over (`gateway/agent_config.json`).
    pub gateway_sources: usize,
}

/// The gateway's config, relative to a server data folder (gateway.rs).
const GATEWAY_CONFIG: &str = "gateway/agent_config.json";

/// Adds the computers of `source`'s gateway to `target`'s gateway config
/// (made when there is none; the rest of it is kept, and so is the earlier
/// file, as `agent_config.json.before-import-<stamp>`). The gateway's id
/// comes from the imported database (gateway.rs `host_id`). Returns how
/// many were added and the names written.
fn import_gateway_sources(source: &Path, target: &Path, stamp: &str) -> Result<(usize, Vec<String>, Option<String>), String> {
    let from = source.join(GATEWAY_CONFIG);
    if !is_plain_file(&from) {
        return Ok((0, Vec::new(), None));
    }
    let read = |path: &Path| -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        match serde_json::from_str(&text) {
            Ok(serde_json::Value::Object(map)) => Ok(map),
            _ => Err(format!("{} is not a gateway config", path.display())),
        }
    };
    let theirs = read(&from)?;
    let sources: Vec<serde_json::Value> = theirs.get("sources").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if sources.is_empty() {
        return Ok((0, Vec::new(), None));
    }
    let dir = target.join("gateway");
    let mut written = Vec::new();
    if std::fs::symlink_metadata(&dir).is_err() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        written.push("gateway".to_string());
    } else if !std::fs::symlink_metadata(&dir).map(|m| m.file_type().is_dir()).unwrap_or(false) {
        // Never through a link.
        return Err(format!("{} is not a folder", dir.display()));
    }
    let to = target.join(GATEWAY_CONFIG);
    let mut ours = if std::fs::symlink_metadata(&to).is_ok() {
        if !is_plain_file(&to) {
            return Err(format!("{} is not a plain file", to.display()));
        }
        read(&to)?
    } else {
        serde_json::Map::new()
    };
    let mut list: Vec<serde_json::Value> = ours.get("sources").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let id_of = |s: &serde_json::Value| s.get("id").and_then(|v| v.as_str()).map(str::to_owned);
    let mut added = 0;
    for s in sources {
        let id = id_of(&s);
        if id.is_some() && list.iter().any(|have| id_of(have) == id) {
            continue;
        }
        list.push(s);
        added += 1;
    }
    if added == 0 {
        return Ok((0, written, None));
    }
    let mut kept = None;
    if std::fs::symlink_metadata(&to).is_ok() {
        let aside_name = format!("{GATEWAY_CONFIG}.before-import-{stamp}");
        std::fs::rename(&to, target.join(&aside_name)).map_err(|e| format!("{}: {e}", to.display()))?;
        kept = Some(aside_name);
    }
    ours.insert("sources".into(), serde_json::Value::Array(list));
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(ours)).map_err(|e| e.to_string())?;
    crate::paths::write_private(&to, &text).map_err(|e| format!("{}: {e}", to.display()))?;
    written.push(GATEWAY_CONFIG.to_string());
    Ok((added, written, kept))
}

async fn open_read_only(path: &Path) -> Result<sqlx::SqliteConnection, String> {
    use sqlx::ConnectOptions;
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .connect()
        .await
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Rows in `table` of a database (0 when it has no such table).
async fn count(path: &Path, table: &str) -> Result<i64, String> {
    let mut conn = open_read_only(path).await?;
    let err = |e: sqlx::Error| format!("{}: {e}", path.display());
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?")
        .bind(table)
        .fetch_one(&mut conn)
        .await
        .map_err(err)?;
    let n = if exists > 0 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT COUNT(*) FROM \"{table}\"")))
            .fetch_one(&mut conn)
            .await
            .map_err(err)?
    } else {
        0
    };
    let _ = sqlx::Connection::close(conn).await;
    Ok(n)
}

/// Copies the app's server data in `source` into `target` (a server data
/// folder). Refuses when `target` already has accounts, unless `replace`;
/// an existing database is kept as `lunaris.db.before-import-<time>`. The
/// target's server must be stopped.
pub async fn import_into(source: &Path, target: &Path, replace: bool) -> Result<Imported, String> {
    let source_db = source.join("lunaris.db");
    if !std::fs::symlink_metadata(&source_db).map(|m| m.is_file()).unwrap_or(false) {
        return Err(format!("{} has no lunaris.db: not a Lunaris server data folder", source.display()));
    }
    let users = count(&source_db, "users").await?;
    if users == 0 {
        return Err(format!("{} has no accounts: nothing to import", source_db.display()));
    }
    let hosts = count(&source_db, "hosts").await?;
    std::fs::create_dir_all(target).map_err(|e| format!("{}: {e}", target.display()))?;
    if same_dir(source, target) {
        return Err("the source is the service's data folder itself".into());
    }

    let target_db = target.join("lunaris.db");
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let mut backup = None;
    // (from, to) of what was moved aside, to put back when the copy fails.
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    if target_db.exists() {
        let existing = count(&target_db, "users").await?;
        if existing > 0 && !replace {
            return Err(format!(
                "{} already has {existing} account(s); add --replace to replace its database \
                 (it is kept as a backup)",
                target_db.display()
            ));
        }
        let aside = target.join(format!("lunaris.db.before-import-{stamp}"));
        // SQLite's own files go with it, under the names it looks for.
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let from = target.join(format!("lunaris.db{suffix}"));
            if std::fs::symlink_metadata(&from).is_ok() {
                let to = PathBuf::from(format!("{}{suffix}", aside.display()));
                if let Err(e) = std::fs::rename(&from, &to) {
                    put_back(&moved);
                    return Err(format!("{}: {e}", from.display()));
                }
                moved.push((from, to));
            }
        }
        backup = Some(aside);
    }

    // One consistent copy of the database (the app's server may be running
    // with part of it in its WAL), made in a private file first.
    let tmp = target.join(format!(".lunaris.db.import-{}", crate::security::random_hex(4)));
    let copied_db = async {
        crate::paths::write_private(&tmp, "").map_err(|e| format!("{}: {e}", tmp.display()))?;
        let mut conn = open_read_only(&source_db).await?;
        sqlx::query("VACUUM INTO ?")
            .bind(tmp.to_string_lossy().into_owned())
            .execute(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        let _ = sqlx::Connection::close(conn).await;
        #[cfg(test)]
        if FAIL_COPY.with(|f| f.get()) {
            return Err("failed on purpose".to_string());
        }
        // The folder belongs to the service's account: what is renamed into
        // place must still be the file made above, not a link put there.
        if !is_plain_file(&tmp) {
            return Err(format!("{} is no longer a plain file", tmp.display()));
        }
        std::fs::rename(&tmp, &target_db).map_err(|e| e.to_string())
    }
    .await;
    if let Err(e) = copied_db {
        let _ = std::fs::remove_file(&tmp);
        // The service keeps the database it had.
        put_back(&moved);
        return Err(format!("cannot copy the database: {e}"));
    }
    restrict(&target_db)?;
    let mut copied = vec!["lunaris.db".to_string()];

    let mut kept = Vec::new();
    for name in KEY_FILES {
        let from = source.join(name);
        // Never through a link.
        if !std::fs::symlink_metadata(&from).map(|m| m.is_file()).unwrap_or(false) {
            continue;
        }
        let text = std::fs::read_to_string(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        let to = target.join(name);
        // The service's own key is kept, as its database is (a rename: a
        // link there is moved, never followed).
        if std::fs::symlink_metadata(&to).is_ok() {
            let aside_name = format!("{name}.before-import-{stamp}");
            let aside = target.join(&aside_name);
            std::fs::rename(&to, &aside).map_err(|e| format!("{}: {e}", to.display()))?;
            kept.push(aside_name);
        }
        crate::paths::write_private(&to, &text).map_err(|e| format!("{}: {e}", to.display()))?;
        copied.push(name.to_string());
    }
    let (gateway_sources, written, gateway_kept) = import_gateway_sources(source, target, &stamp)?;
    copied.extend(written);
    kept.extend(gateway_kept);
    // The target's own setup code is void now that there are accounts.
    let _ = std::fs::remove_file(target.join(crate::setup::CODE_FILE));
    Ok(Imported { copied, backup, kept, users, hosts, gateway_sources })
}

#[cfg(test)]
thread_local! {
    /// Makes the copy of the database fail (after the moves aside).
    static FAIL_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Undoes the moves aside (last first) after a failed copy.
fn put_back(moved: &[(PathBuf, PathBuf)]) {
    for (from, to) in moved.iter().rev() {
        if let Err(e) = std::fs::rename(to, from) {
            eprintln!("could not put {} back as {}: {e}", to.display(), from.display());
        }
    }
}

fn is_plain_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map(|m| m.file_type().is_file()).unwrap_or(false)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Owner-only (the database is written by SQLite, not write_private).
/// Refuses a link: permissions set as root would go to what it points at.
fn restrict(path: &Path) -> Result<(), String> {
    if !is_plain_file(path) {
        return Err(format!("{} is not a plain file", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Gives the copied files to the service's account: the `lunaris` user of
/// the package (scripts/packaging/linux/lunaris-server.sysusers) when it
/// exists, otherwise the owner of the data folder (the service made by
/// `lunaris-server install` runs as a dynamic user that owns it). Only as
/// root; None when not.
#[cfg(unix)]
fn give_to_service(target: &Path, names: &[String]) -> Result<Option<String>, String> {
    use std::os::unix::fs::MetadataExt;
    if unsafe { libc::geteuid() } != 0 {
        return Ok(None);
    }
    let (uid, gid, who) = match service_user() {
        Some((uid, gid)) => (uid, gid, "the lunaris user".to_string()),
        None => {
            let m = std::fs::metadata(target).map_err(|e| format!("{}: {e}", target.display()))?;
            (m.uid(), m.gid(), format!("uid {} (the owner of {})", m.uid(), target.display()))
        }
    };
    for name in names {
        let path = target.join(name);
        // The folder is the service account's: a link it put in place of a
        // copied file gets the new owner itself, never what it points at.
        std::os::unix::fs::lchown(&path, Some(uid), Some(gid)).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(Some(who))
}

/// uid and gid of the `lunaris` user.
#[cfg(unix)]
fn service_user() -> Option<(u32, u32)> {
    let name = std::ffi::CString::new("lunaris").ok()?;
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    unsafe { Some(((*pw).pw_uid, (*pw).pw_gid)) }
}

/// The installed service must be stopped: it has the database open.
fn refuse_if_service_runs() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let active = std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", "lunaris-server"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if active {
            return Err("the lunaris-server service is running: stop it first \
                        (sudo systemctl stop lunaris-server), and start it again after the import"
                .into());
        }
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("sc").args(["query", "LunarisServer"]).output();
        if matches!(out, Ok(o) if String::from_utf8_lossy(&o.stdout).contains("RUNNING")) {
            return Err("the LunarisServer service is running: stop it first (net stop LunarisServer), \
                        and start it again after the import"
                .into());
        }
    }
    Ok(())
}

pub fn run(args: &[String]) -> Result<(), String> {
    let args = parse(args)?;
    // LUNARIS_DATA_DIR: another server folder (a server run by hand);
    // otherwise the installed service's.
    let explicit = std::env::var_os("LUNARIS_DATA_DIR").filter(|v| !v.is_empty()).map(PathBuf::from);
    let target = match &explicit {
        Some(dir) => dir.clone(),
        None => {
            #[cfg(unix)]
            if unsafe { libc::geteuid() } != 0 {
                return Err("this command needs root: run it with sudo".into());
            }
            refuse_if_service_runs()?;
            #[cfg(windows)]
            {
                // Locked (and an older server's data moved) first; fails
                // without Administrator rights.
                crate::service::prepare_data_dir()?.0
            }
            #[cfg(not(windows))]
            {
                PathBuf::from("/var/lib/lunaris")
            }
        }
    };

    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let imported = rt.block_on(import_into(&args.source, &target, args.replace))?;

    #[cfg(unix)]
    let owner = give_to_service(&target, &imported.copied)?;
    #[cfg(windows)]
    if explicit.is_none() {
        crate::win_acl::secure_dir(&target)?;
    }

    println!(
        "Imported {} account(s) and {} computer(s) from {} into {}:",
        imported.users,
        imported.hosts,
        args.source.display(),
        target.display()
    );
    for name in &imported.copied {
        println!("  {name}");
    }
    if imported.gateway_sources > 0 {
        println!("{} computer(s) of the gateway added to its config.", imported.gateway_sources);
    }
    if let Some(backup) = &imported.backup {
        println!("The earlier database is kept as {}.", backup.display());
    }
    for name in &imported.kept {
        println!("An earlier key file is kept as {}.", target.join(name).display());
    }
    #[cfg(unix)]
    match owner {
        Some(who) => println!("Owned by {who}, readable by it only."),
        None => println!("Readable by their owner only (not run as root: ownership unchanged)."),
    }
    #[cfg(windows)]
    if explicit.is_none() {
        println!("Readable by SYSTEM and Administrators only.");
    }
    if explicit.is_none() {
        #[cfg(target_os = "linux")]
        println!("Start the service again: sudo systemctl start lunaris-server");
        #[cfg(windows)]
        println!("Start the service again: net start LunarisServer");
    }
    println!("Sign in with the accounts of the app's server, and turn that server off in the app.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunaris-import-{}-{}", name, crate::security::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// An app server's folder: a database with one account and one computer
    /// (left open, as a running server would), and its files.
    async fn app_server(dir: &Path) -> sqlx::SqlitePool {
        let db = crate::db::init_db(&format!("sqlite://{}", dir.join("lunaris.db").display())).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('u1', 'owner', 'x', 'admin', 1)")
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO hosts (id, name, status) VALUES ('h1', 'Office PC', 'Offline')").execute(&db).await.unwrap();
        for (name, text) in [
            ("server_token.txt", "agent-token"),
            ("mfa_key.txt", "mfa"),
            ("jwt_secret.txt", "jwt"),
            ("tls_cert.pem", "cert"),
            ("tls_key.pem", "key"),
            ("initial_admin_password.txt", "old"),
            ("server.log", "log"),
        ] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        db
    }

    async fn users_in(path: &Path) -> Vec<String> {
        let mut conn = open_read_only(path).await.unwrap();
        sqlx::query_scalar("SELECT username FROM users").fetch_all(&mut conn).await.unwrap()
    }

    #[tokio::test]
    async fn imports_into_a_new_service() {
        let src = temp("src");
        let dst = temp("dst");
        let app = app_server(&src).await;
        // A fresh service: its own token and a setup code, no account.
        let fresh = crate::db::init_db(&format!("sqlite://{}", dst.join("lunaris.db").display())).await.unwrap();
        crate::setup::prepare(&fresh, &dst.join(crate::setup::CODE_FILE), None).await.unwrap();
        fresh.close().await;
        std::fs::write(dst.join("server_token.txt"), "service-token").unwrap();

        let imported = import_into(&src, &dst, false).await.unwrap();
        assert_eq!((imported.users, imported.hosts), (1, 1));
        assert_eq!(
            imported.copied,
            ["lunaris.db", "server_token.txt", "mfa_key.txt", "jwt_secret.txt", "tls_cert.pem", "tls_key.pem"]
        );
        assert!(imported.backup.is_some(), "the empty database is kept aside too");
        assert_eq!(users_in(&dst.join("lunaris.db")).await, ["owner"]);
        // Usable by the server.
        let db = crate::db::init_db(&format!("sqlite://{}", dst.join("lunaris.db").display())).await.unwrap();
        let host: String = sqlx::query_scalar("SELECT name FROM hosts").fetch_one(&db).await.unwrap();
        assert_eq!(host, "Office PC");
        db.close().await;
        assert_eq!(std::fs::read_to_string(dst.join("server_token.txt")).unwrap(), "agent-token");
        assert!(!dst.join(crate::setup::CODE_FILE).exists());
        assert!(!dst.join("initial_admin_password.txt").exists());
        assert!(!dst.join("server.log").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in &imported.copied {
                assert_eq!(std::fs::metadata(dst.join(name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
            }
        }
        // The source is untouched.
        assert_eq!(users_in(&src.join("lunaris.db")).await, ["owner"]);
        app.close().await;
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[tokio::test]
    async fn refuses_a_service_with_accounts_unless_replace() {
        let src = temp("src2");
        let dst = temp("dst2");
        let app = app_server(&src).await;
        let service = crate::db::init_db(&format!("sqlite://{}", dst.join("lunaris.db").display())).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('s1', 'boss', 'x', 'admin', 1)")
            .execute(&service)
            .await
            .unwrap();
        service.close().await;

        let err = import_into(&src, &dst, false).await.unwrap_err();
        assert!(err.contains("--replace"), "{err}");
        assert_eq!(users_in(&dst.join("lunaris.db")).await, ["boss"]);

        let imported = import_into(&src, &dst, true).await.unwrap();
        assert_eq!(users_in(&dst.join("lunaris.db")).await, ["owner"]);
        assert_eq!(users_in(&imported.backup.unwrap()).await, ["boss"]);
        app.close().await;
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    /// A failed copy leaves the service's database where it was; its key
    /// files are kept next to it when replaced.
    #[tokio::test]
    async fn a_failed_copy_puts_the_database_back_and_keys_are_kept() {
        let src = temp("src4");
        let dst = temp("dst4");
        let app = app_server(&src).await;
        let service = crate::db::init_db(&format!("sqlite://{}", dst.join("lunaris.db").display())).await.unwrap();
        sqlx::query("INSERT INTO users (id, username, password_hash, role, is_active) VALUES ('s1', 'boss', 'x', 'admin', 1)")
            .execute(&service)
            .await
            .unwrap();
        service.close().await;
        std::fs::write(dst.join("jwt_secret.txt"), "service-jwt").unwrap();

        FAIL_COPY.with(|f| f.set(true));
        let err = import_into(&src, &dst, true).await.unwrap_err();
        FAIL_COPY.with(|f| f.set(false));
        assert!(err.contains("cannot copy"), "{err}");
        assert_eq!(users_in(&dst.join("lunaris.db")).await, ["boss"]);
        let names: Vec<String> =
            std::fs::read_dir(&dst).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.contains("before-import") || n.starts_with(".lunaris.db.import")), "{names:?}");
        assert_eq!(std::fs::read_to_string(dst.join("jwt_secret.txt")).unwrap(), "service-jwt");

        let imported = import_into(&src, &dst, true).await.unwrap();
        assert_eq!(imported.kept.len(), 1);
        assert!(imported.kept[0].starts_with("jwt_secret.txt.before-import-"));
        assert_eq!(std::fs::read_to_string(dst.join(&imported.kept[0])).unwrap(), "service-jwt");
        assert_eq!(std::fs::read_to_string(dst.join("jwt_secret.txt")).unwrap(), "jwt");
        let backup = imported.backup.unwrap().file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(backup.trim_start_matches("lunaris.db."), imported.kept[0].trim_start_matches("jwt_secret.txt."));
        app.close().await;
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    /// A link put where a key file goes is moved aside, never written through.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_in_the_target_is_not_written_through() {
        let src = temp("src5");
        let dst = temp("dst5");
        let outside = temp("outside5");
        let app = app_server(&src).await;
        std::fs::write(outside.join("victim"), "untouched").unwrap();
        std::os::unix::fs::symlink(outside.join("victim"), dst.join("mfa_key.txt")).unwrap();
        let imported = import_into(&src, &dst, false).await.unwrap();
        assert_eq!(std::fs::read_to_string(outside.join("victim")).unwrap(), "untouched");
        assert!(std::fs::symlink_metadata(dst.join("mfa_key.txt")).unwrap().file_type().is_file());
        assert_eq!(imported.kept.len(), 1);
        app.close().await;
        for d in [&src, &dst, &outside] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// The computers added through the app server's gateway go to the
    /// service's gateway config, beside what it has; its earlier file is kept.
    #[tokio::test]
    async fn the_gateways_computers_are_taken_over() {
        let src = temp("src6");
        let dst = temp("dst6");
        let app = app_server(&src).await;
        std::fs::create_dir_all(src.join("gateway")).unwrap();
        std::fs::write(
            src.join(GATEWAY_CONFIG),
            r#"{"client_unique_id":"GW","server_url":"ws://127.0.0.1:8090","sources":[{"id":"home","name":"home","connector":"rdp"},{"id":"both","name":"both","connector":"vnc"}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dst.join("gateway")).unwrap();
        std::fs::write(dst.join(GATEWAY_CONFIG), r#"{"client_unique_id":"GW","sources":[{"id":"both","name":"theirs"}]}"#).unwrap();

        let imported = import_into(&src, &dst, false).await.unwrap();
        assert_eq!(imported.gateway_sources, 1);
        assert!(imported.copied.contains(&GATEWAY_CONFIG.to_string()));
        let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dst.join(GATEWAY_CONFIG)).unwrap()).unwrap();
        let names: Vec<&str> = config["sources"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["theirs", "home"]);
        // Only the computers: not the app gateway's other settings.
        assert!(config.get("server_url").is_none());
        let aside = imported.kept.iter().find(|k| k.starts_with("gateway/agent_config.json.before-import-")).expect("kept");
        assert!(std::fs::read_to_string(dst.join(aside)).unwrap().contains("theirs"));

        // A service without a gateway folder yet gets one.
        let dst2 = temp("dst6b");
        let imported = import_into(&src, &dst2, false).await.unwrap();
        assert_eq!(imported.gateway_sources, 2);
        assert!(imported.copied.contains(&"gateway".to_string()));
        app.close().await;
        for d in [&src, &dst, &dst2] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[tokio::test]
    async fn refuses_a_folder_without_a_database_or_accounts() {
        let src = temp("src3");
        let dst = temp("dst3");
        assert!(import_into(&src, &dst, false).await.unwrap_err().contains("no lunaris.db"));
        let empty = crate::db::init_db(&format!("sqlite://{}", src.join("lunaris.db").display())).await.unwrap();
        empty.close().await;
        assert!(import_into(&src, &dst, false).await.unwrap_err().contains("no accounts"));
        assert!(!dst.join("lunaris.db").exists());
        assert!(import_into(&src, &src, true).await.is_err());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn parses_arguments() {
        let a = parse(&["/x".into(), "--replace".into()]).unwrap();
        assert_eq!(a.source, PathBuf::from("/x"));
        assert!(a.replace);
        assert!(!parse(&["/x".into()]).unwrap().replace);
        assert!(parse(&[]).is_err());
        assert!(parse(&["/x".into(), "/y".into()]).is_err());
        assert!(parse(&["--force".into()]).is_err());
    }
}
