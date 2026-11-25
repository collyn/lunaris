//! Updating this agent when the Lunaris server's administrators ask for it
//! from the web (`ServerToAgentMessage::Update`).
//!
//! The server names a version, nothing else. This agent then
//!
//! 1. refuses unless the computer allows it (`allow_server_update`, on by
//!    default, `lunaris-agent set server-update deny`), the agent was
//!    installed in a way it can redo by itself (below), and the version is
//!    a release newer than its own (never a downgrade);
//! 2. downloads `SHA256SUMS`, `SHA256SUMS.sig` and the installer of that
//!    version from the project's GitHub releases (`updater::DOWNLOAD_PREFIX`,
//!    HTTPS), and nowhere else;
//! 3. checks the Ed25519 signature of `SHA256SUMS` with the release key
//!    built into this program, that the signed list is for that version,
//!    and the installer's SHA-256 against its line. Anything wrong: the
//!    file is deleted and nothing is installed;
//! 4. starts the installer and answers. The installer replaces the program
//!    and restarts the agent, as `agent.ps1 -Update` does.
//!
//! So a server, or whoever got hold of an administrator's account on it,
//! can make this computer run a newer release of the project and nothing
//! else.
//!
//! Where: Windows, installed by the installer (Program Files, the service:
//! the agent runs as SYSTEM and can run the silent installer). The Linux
//! agent runs without root (a user service, or the `lunaris-agent` system
//! account) and cannot install a package: there the update command is run
//! on the computer, as before.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use common::AgentMessage;
use tracing::{info, warn};

/// The release file that updates a Windows agent.
pub const WINDOWS_INSTALLER: &str = "lunaris-agent-installer-windows.exe";
/// Nothing larger is downloaded (the installer is about 60 MB).
const MAX_INSTALLER_BYTES: u64 = 400 * 1024 * 1024;
const MAX_LIST_BYTES: u64 = 256 * 1024;

/// Why not, for `UpdateAnswer`.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub text: String,
}

fn refuse(code: &'static str, text: impl Into<String>) -> Refusal {
    Refusal { code, text: text.into() }
}

/// Whether an agent installed as `install` can install a release by itself here.
pub fn can_install(install: &str) -> bool {
    cfg!(windows) && install == common::INSTALL_WINDOWS
}

/// What `AgentBuild.remote_update` says: this computer allows it and this
/// agent can do it (and has a release key to check with).
pub fn offered() -> bool {
    let allowed = crate::pairing::load_config(crate::pairing::config_path()).map(|c| c.allow_server_update).unwrap_or(false);
    allowed && can_install(&crate::build_info::current_install()) && updater::verify::signing_configured(updater::verify::RELEASE_SIGNING_PUBLIC_KEY_B64)
}

/// "0.8.3" from "0.8.3" or "v0.8.3": three numbers and nothing else, as it
/// goes into the download address.
fn release_version(wanted: &str) -> Option<String> {
    let v = wanted.trim().trim_start_matches('v');
    let parts: Vec<&str> = v.split('.').collect();
    let numbers = parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()));
    numbers.then(|| v.to_string())
}

/// What is decided before anything is downloaded. `trusted`: the request
/// came from the server this agent is connected to, or over a link whose
/// server proved the password. Answers the version to install.
pub fn check_request(own: &str, wanted: &str, allowed: bool, can: bool, trusted: bool) -> Result<String, Refusal> {
    if !trusted {
        return Err(refuse(common::UPDATE_NOT_ALLOWED, "Only a server that proved this computer's password may update its agent."));
    }
    if !allowed {
        return Err(refuse(
            common::UPDATE_NOT_ALLOWED,
            "This computer does not allow its agent to be updated from the server (`lunaris-agent set server-update allow` there turns it on).",
        ));
    }
    if !can {
        return Err(refuse(
            common::UPDATE_NOT_SUPPORTED,
            "This agent cannot install an update by itself (how it was installed, or the system it runs on): run the update command on the computer.",
        ));
    }
    let Some(version) = release_version(wanted) else {
        return Err(refuse(common::UPDATE_INVALID, "That is not a release version."));
    };
    if !updater::version::is_newer(own, &version) {
        return Err(refuse(common::UPDATE_INVALID, format!("Version {version} is not newer than this agent ({own}): nothing to install.")));
    }
    Ok(version)
}

/// Where a file of release `version` is downloaded from.
pub fn release_url(version: &str, name: &str) -> String {
    format!("{}v{}/{}", updater::DOWNLOAD_PREFIX, version, name)
}

/// How the files are fetched (the network; forged in tests).
pub trait Fetch {
    /// A small file, whole.
    fn small(&self, url: &str) -> Result<Vec<u8>, String>;
    /// A large file written to `to`; answers its SHA-256.
    fn file(&self, url: &str, to: &Path) -> Result<[u8; 32], String>;
}

/// Downloads release file `name` of `version` into `dir` and checks it:
/// the signature of the release's checksum list (`key_b64`), the version
/// the list is for, and the file's SHA-256. The path of the checked file,
/// or why not (then no file is left).
pub fn fetch_verified(key_b64: &str, version: &str, name: &str, fetch: &dyn Fetch, dir: &Path) -> Result<PathBuf, Refusal> {
    let failed = |what: String| refuse(common::UPDATE_FAILED, what);
    let sums = fetch
        .small(&release_url(version, updater::verify::SUMS_NAME))
        .map_err(|e| failed(format!("The checksum list of release {version} could not be downloaded ({e}): nothing is installed.")))?;
    let sig = fetch
        .small(&release_url(version, updater::verify::SIG_NAME))
        .map_err(|e| failed(format!("The signature of release {version} could not be downloaded ({e}): nothing is installed.")))?;
    let not_valid = |e: updater::verify::VerifyError| failed(format!("{e}: nothing is installed."));
    updater::verify::verify_sums_signature(key_b64, &sums, &sig).map_err(not_valid)?;
    updater::verify::check_sums_version(&sums, version).map_err(not_valid)?;
    // Before the large download: the list must name the file.
    updater::verify::expected_digest(&sums, name).map_err(not_valid)?;
    let path = dir.join(name);
    let digest = match fetch.file(&release_url(version, name), &path) {
        Ok(digest) => digest,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(failed(format!("{name} of release {version} could not be downloaded ({e}): nothing is installed.")));
        }
    };
    if let Err(e) = updater::verify::check_digest(&sums, name, &digest) {
        let _ = std::fs::remove_file(&path);
        return Err(not_valid(e));
    }
    Ok(path)
}

/// The network: HTTPS only, with limits.
struct Https {
    client: reqwest::blocking::Client,
}

impl Https {
    fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(updater::USER_AGENT)
            .https_only(true)
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(10 * 60))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client })
    }

    fn get(&self, url: &str) -> Result<reqwest::blocking::Response, String> {
        // Only ever the project's releases (they redirect to GitHub's storage;
        // what comes from there is checked against the signed list).
        if !url.starts_with(updater::DOWNLOAD_PREFIX) {
            return Err("not a release address".into());
        }
        let response = self.client.get(url).send().map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        Ok(response)
    }
}

impl Fetch for Https {
    fn small(&self, url: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let mut out = Vec::new();
        self.get(url)?.take(MAX_LIST_BYTES + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
        if out.len() as u64 > MAX_LIST_BYTES {
            return Err("too large".into());
        }
        Ok(out)
    }

    fn file(&self, url: &str, to: &Path) -> Result<[u8; 32], String> {
        use std::io::{Read, Write};
        let mut response = self.get(url)?;
        let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
        let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buffer = vec![0u8; 256 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = response.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_INSTALLER_BYTES {
                return Err("too large".into());
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(hash.finish().as_ref().try_into().expect("SHA-256 is 32 bytes"))
    }
}

/// The folder a release is downloaded into: `update` beside the config
/// (under the service that is `%ProgramData%\Lunaris`, which only SYSTEM and
/// administrators can write: nobody else can swap the checked file).
fn update_dir(config_path: &str) -> PathBuf {
    Path::new(config_path).parent().unwrap_or(Path::new(".")).join("update")
}

/// The scheduled task that runs the installer (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
const TASK_NAME: &str = "LunarisAgentUpdate";

#[cfg(windows)]
fn schtasks(args: &[&str]) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let output = std::process::Command::new(Path::new(&system_root).join("System32").join("schtasks.exe"))
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("schtasks could not be run: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let said = String::from_utf8_lossy(&output.stderr).trim().chars().take(200).collect::<String>();
        Err(format!("schtasks {} failed ({}): {}", args.first().copied().unwrap_or(""), output.status, said))
    }
}

/// Makes the task that runs the checked installer silently, as SYSTEM. A
/// task and not a child process: the installer stops the agent's service,
/// and the service ends everything it started.
#[cfg(windows)]
fn prepare_installer(installer: &Path) -> Result<(), String> {
    let run = format!("\"{}\" /S", installer.display());
    schtasks(&["/Create", "/TN", TASK_NAME, "/TR", &run, "/SC", "ONCE", "/ST", "00:00", "/RU", "SYSTEM", "/RL", "HIGHEST", "/F"])
}

#[cfg(windows)]
fn start_installer() -> Result<(), String> {
    schtasks(&["/Run", "/TN", TASK_NAME])
}

#[cfg(not(windows))]
fn prepare_installer(_installer: &Path) -> Result<(), String> {
    Err("not on this system".into())
}

#[cfg(not(windows))]
fn start_installer() -> Result<(), String> {
    Err("not on this system".into())
}

/// One update at a time; stays set once the installer was started (this
/// program ends within moments).
static BUSY: AtomicBool = AtomicBool::new(false);

/// Does it, and answers once. Blocking (a download): not on the async loop.
pub fn reply(request_id: String, wanted: &str, requester: Option<&str>, trusted: bool, config_path: String) -> AgentMessage {
    let who = requester.unwrap_or("an administrator");
    let result = update(wanted, trusted, &config_path, who);
    match &result {
        Ok(version) => info!("Update to {} asked for by {} on the server: checked, its installer starts", version, who),
        Err(r) => warn!("Update to {:?} asked for by {} on the server: not done ({}): {}", wanted.chars().take(32).collect::<String>(), who, r.code, r.text),
    }
    let (error, code) = match result {
        Ok(_) => (None, None),
        Err(r) => (Some(r.text), Some(r.code.to_string())),
    };
    AgentMessage::UpdateAnswer { request_id, error, code }
}

fn update(wanted: &str, trusted: bool, config_path: &str, who: &str) -> Result<String, Refusal> {
    let allowed = crate::pairing::load_config(config_path).map(|c| c.allow_server_update).unwrap_or(false);
    let version = check_request(env!("CARGO_PKG_VERSION"), wanted, allowed, can_install(&crate::build_info::current_install()), trusted)?;
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err(refuse(common::UPDATE_BUSY, "An update is already being installed on this computer."));
    }
    let result = (|| {
        let failed = |what: String| refuse(common::UPDATE_FAILED, what);
        let dir = update_dir(config_path);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{} could not be made ({e}).", dir.display())))?;
        info!("Update to {} asked for by {} on the server: downloading from {}", version, who, release_url(&version, WINDOWS_INSTALLER));
        let https = Https::new().map_err(|e| failed(format!("The download could not be set up ({e}).")))?;
        let installer = fetch_verified(updater::verify::RELEASE_SIGNING_PUBLIC_KEY_B64, &version, WINDOWS_INSTALLER, &https, &dir)?;
        info!("Release {}: signature and checksum of {} are right", version, WINDOWS_INSTALLER);
        prepare_installer(&installer).map_err(|e| failed(format!("The installer could not be started ({e}): nothing is installed.")))?;
        Ok(())
    })();
    if let Err(refusal) = result {
        BUSY.store(false, Ordering::SeqCst);
        return Err(refusal);
    }
    // After the answer is on its way: the installer ends this program.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(3));
        if let Err(e) = start_installer() {
            warn!("The update's installer did not start: {}", e);
            BUSY.store(false, Ordering::SeqCst);
        }
    });
    Ok(version)
}

/// At start: what an earlier update left (its downloaded installer, its
/// task) goes, once that installer has surely ended.
pub fn clean_up_later(config_path: String) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let dir = update_dir(&config_path);
    if !dir.exists() {
        return;
    }
    ONCE.call_once(move || {
        clean_up_after(dir, std::time::Duration::from_secs(120));
    });
}

fn clean_up_after(dir: PathBuf, wait: std::time::Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(wait);
        if BUSY.load(Ordering::SeqCst) {
            return;
        }
        #[cfg(windows)]
        let _ = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]);
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => info!("Removed what the last update downloaded ({})", dir.display()),
            Err(e) => warn!("{} could not be removed: {}", dir.display(), e),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn keypair() -> (Ed25519KeyPair, String) {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = base64::engine::general_purpose::STANDARD.encode(pair.public_key().as_ref());
        (pair, public)
    }

    /// An empty folder of this test's own; removed when dropped.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("lunaris-update-{}-{}", name, std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A pretend release: what each address answers, and what was asked.
    struct Release {
        files: HashMap<String, Vec<u8>>,
        asked: RefCell<Vec<String>>,
    }

    impl Fetch for Release {
        fn small(&self, url: &str) -> Result<Vec<u8>, String> {
            self.asked.borrow_mut().push(url.to_string());
            self.files.get(url).cloned().ok_or_else(|| "HTTP 404".to_string())
        }
        fn file(&self, url: &str, to: &Path) -> Result<[u8; 32], String> {
            self.asked.borrow_mut().push(url.to_string());
            let bytes = self.files.get(url).ok_or_else(|| "HTTP 404".to_string())?;
            std::fs::write(to, bytes).map_err(|e| e.to_string())?;
            Ok(updater::verify::sha256(bytes))
        }
    }

    const NAME: &str = WINDOWS_INSTALLER;

    /// Release `version` with `installer`, its list signed by `pair` for `listed_version`.
    fn release(pair: &Ed25519KeyPair, version: &str, listed_version: &str, listed: &[u8], installer: &[u8]) -> Release {
        let sums = format!("# lunaris {listed_version}\n{}  {NAME}\n", hex(&updater::verify::sha256(listed)));
        let sig = pair.sign(sums.as_bytes()).as_ref().to_vec();
        let files = HashMap::from([
            (release_url(version, "SHA256SUMS"), sums.into_bytes()),
            (release_url(version, "SHA256SUMS.sig"), sig),
            (release_url(version, NAME), installer.to_vec()),
        ]);
        Release { files, asked: RefCell::new(Vec::new()) }
    }

    #[test]
    fn a_release_that_checks_out_is_kept() {
        let (pair, key) = keypair();
        let dir = TempDir::new("kept");
        let r = release(&pair, "0.9.0", "0.9.0", b"the installer", b"the installer");
        let path = fetch_verified(&key, "0.9.0", NAME, &r, dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"the installer");
        assert_eq!(path, dir.path().join(NAME));
        // Only the project's release addresses were asked, the list first.
        let asked = r.asked.borrow();
        assert!(asked.iter().all(|u| u.starts_with("https://github.com/collyn/lunaris/releases/download/v0.9.0/")), "{asked:?}");
        assert!(asked[0].ends_with("/SHA256SUMS") && asked[2].ends_with(NAME));
    }

    #[test]
    fn anything_wrong_installs_nothing_and_leaves_no_file() {
        let (pair, key) = keypair();
        let (other_pair, other_key) = keypair();
        let dir = TempDir::new("wrong");
        let left = || std::fs::read_dir(dir.path()).unwrap().count();

        // Another file than the one the signed list names.
        let swapped = release(&pair, "0.9.0", "0.9.0", b"the installer", b"something else");
        let e = fetch_verified(&key, "0.9.0", NAME, &swapped, dir.path()).unwrap_err();
        assert_eq!(e.code, common::UPDATE_FAILED);
        assert!(e.text.contains("does not match its checksum"), "{}", e.text);
        assert_eq!(left(), 0);

        // A list signed by somebody else: the installer is not even asked for.
        let forged = release(&other_pair, "0.9.0", "0.9.0", b"evil", b"evil");
        let e = fetch_verified(&key, "0.9.0", NAME, &forged, dir.path()).unwrap_err();
        assert!(e.text.contains("signature is not valid"), "{}", e.text);
        assert!(!forged.asked.borrow().iter().any(|u| u.ends_with(NAME)));
        assert_eq!(left(), 0);
        let _ = other_key;

        // An old, rightly signed list put under a newer version's address.
        let replayed = release(&pair, "0.9.0", "0.8.0", b"old installer", b"old installer");
        let e = fetch_verified(&key, "0.9.0", NAME, &replayed, dir.path()).unwrap_err();
        assert!(e.text.contains("is for version 0.8.0"), "{}", e.text);
        assert_eq!(left(), 0);

        // A list that does not name the installer.
        let mut nameless = release(&pair, "0.9.0", "0.9.0", b"x", b"x");
        let sums = b"# lunaris 0.9.0\n".to_vec();
        nameless.files.insert(release_url("0.9.0", "SHA256SUMS.sig"), pair.sign(&sums).as_ref().to_vec());
        nameless.files.insert(release_url("0.9.0", "SHA256SUMS"), sums);
        let e = fetch_verified(&key, "0.9.0", NAME, &nameless, dir.path()).unwrap_err();
        assert!(e.text.contains("no entry"), "{}", e.text);

        // No such release; a build without a key.
        let none = Release { files: HashMap::new(), asked: RefCell::new(Vec::new()) };
        assert!(fetch_verified(&key, "0.9.0", NAME, &none, dir.path()).unwrap_err().text.contains("could not be downloaded"));
        let good = release(&pair, "0.9.0", "0.9.0", b"i", b"i");
        assert!(fetch_verified(updater::verify::PLACEHOLDER_PUBLIC_KEY, "0.9.0", NAME, &good, dir.path()).unwrap_err().text.contains("not signed"));
        assert_eq!(left(), 0);
    }

    #[test]
    fn a_request_is_refused_before_anything_is_downloaded() {
        let ok = |own, wanted| check_request(own, wanted, true, true, true);
        assert_eq!(ok("0.8.3", "0.8.4").as_deref(), Ok("0.8.4"));
        assert_eq!(ok("0.8.3", "v0.9.0").as_deref(), Ok("0.9.0"));
        // Never the same version or a downgrade.
        assert_eq!(ok("0.8.3", "0.8.3").unwrap_err().code, common::UPDATE_INVALID);
        assert_eq!(ok("0.8.3", "0.8.2").unwrap_err().code, common::UPDATE_INVALID);
        // Nothing but three numbers goes into the address.
        for bad in ["", "latest", "0.9", "0.9.0-rc.1", "0.9.0/../../x", "0.9.0?x=1", "0.9.0 x", "1.2.3.4", "../0.9.0", "0.9.0#", "0.9.x", "999999.0.0", "0..9"] {
            assert_eq!(ok("0.8.3", bad).unwrap_err().code, common::UPDATE_INVALID, "{bad:?}");
        }
        // The computer's switch, how it was installed, and who asks.
        assert_eq!(check_request("0.8.3", "0.9.0", false, true, true).unwrap_err().code, common::UPDATE_NOT_ALLOWED);
        assert_eq!(check_request("0.8.3", "0.9.0", true, false, true).unwrap_err().code, common::UPDATE_NOT_SUPPORTED);
        assert_eq!(check_request("0.8.3", "0.9.0", true, true, false).unwrap_err().code, common::UPDATE_NOT_ALLOWED);
    }

    #[test]
    fn only_the_windows_installer_install_updates_itself() {
        assert_eq!(can_install(common::INSTALL_WINDOWS), cfg!(windows));
        for kind in [common::INSTALL_DEB, common::INSTALL_APPIMAGE, common::INSTALL_PACKAGE, common::INSTALL_MANUAL, ""] {
            assert!(!can_install(kind), "{kind}");
        }
        assert_eq!(release_url("0.9.0", NAME), "https://github.com/collyn/lunaris/releases/download/v0.9.0/lunaris-agent-installer-windows.exe");
        assert_eq!(update_dir(r"/var/lib/x/agent_config.json"), Path::new("/var/lib/x/update"));
    }

    /// The real thing, over the network (run by hand: `cargo test -p agent
    /// real_release -- --ignored`): a published release checks out with the
    /// key built into this program, and a file that is not the listed one
    /// does not.
    #[test]
    #[ignore]
    fn real_release_checks_out() {
        let dir = TempDir::new("real");
        let https = Https::new().unwrap();
        let key = updater::verify::RELEASE_SIGNING_PUBLIC_KEY_B64;
        let name = "lunaris-approval-helper_0.8.2_amd64.deb";
        let path = fetch_verified(key, "0.8.2", name, &https, dir.path()).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 100_000);
        // The same release's list does not vouch for another version's address.
        let e = fetch_verified(key, "0.8.1", "lunaris-approval-helper_0.8.2_amd64.deb", &https, dir.path()).unwrap_err();
        assert!(e.text.contains("no entry"), "{}", e.text);
        // Not a release address: refused before any request.
        assert_eq!(https.small("https://example.com/SHA256SUMS").unwrap_err(), "not a release address");
    }
}
