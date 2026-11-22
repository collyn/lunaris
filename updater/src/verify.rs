//! Release signature: CI writes `SHA256SUMS` (the SHA-256 of every release
//! file, `sha256sum` format) and `SHA256SUMS.sig`, a raw 64-byte Ed25519
//! signature over the exact bytes of `SHA256SUMS`
//! (`openssl pkeyutl -sign -rawin`, see docs/release-signing.md). The app
//! checks the signature with the public key below, then the downloaded
//! file's SHA-256 against its line. Any mismatch: nothing is installed.

use std::collections::HashMap;

use base64::Engine as _;

/// The release signing public key: the raw 32-byte Ed25519 key, base64
/// (`openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64`).
/// While it is the placeholder, this build installs no update (it still
/// says when one exists). The release workflow refuses to publish when the
/// key here does not match the signing key. See docs/release-signing.md.
pub const RELEASE_SIGNING_PUBLIC_KEY_B64: &str = "epmKZg8tkNrvsbcR/E/LM3ONIvy30YoUjgEhLdkZqI0=";

/// The value of [`RELEASE_SIGNING_PUBLIC_KEY_B64`] before a key is set.
pub const PLACEHOLDER_PUBLIC_KEY: &str = "UNSET-RELEASE-SIGNING-PUBLIC-KEY";

pub const SUMS_NAME: &str = "SHA256SUMS";
pub const SIG_NAME: &str = "SHA256SUMS.sig";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// The placeholder key: this build cannot check updates.
    NotSigned,
    /// The built-in key is not a 32-byte base64 key.
    BadKey,
    /// The signature does not match SHA256SUMS (or is not 64 bytes).
    BadSignature,
    MalformedSums(String),
    MissingEntry(String),
    DuplicateEntry(String),
    HashMismatch(String),
    /// The signed list names another version than the release being
    /// installed (an old signed list re-published under a new tag).
    WrongVersion { signed: Option<String>, release: String },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::NotSigned => write!(f, "Updates are not signed in this build"),
            VerifyError::BadKey => write!(f, "The update signing key of this build is not valid"),
            VerifyError::BadSignature => write!(f, "The release signature is not valid"),
            VerifyError::MalformedSums(l) => write!(f, "The release checksum list is not valid: {l}"),
            VerifyError::MissingEntry(n) => write!(f, "The release checksum list has no entry for: {n}"),
            VerifyError::DuplicateEntry(n) => write!(f, "The release checksum list has two entries for: {n}"),
            VerifyError::HashMismatch(n) => write!(f, "The downloaded file does not match its checksum: {n}"),
            VerifyError::WrongVersion { signed, release } => write!(
                f,
                "The signed checksum list is for version {} but the release is {release}",
                signed.as_deref().unwrap_or("(none)")
            ),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Whether `key_b64` is a real key (not the placeholder): updates may be installed.
pub fn signing_configured(key_b64: &str) -> bool {
    public_key(key_b64).is_ok()
}

/// The raw 32-byte public key.
pub fn public_key(key_b64: &str) -> Result<[u8; 32], VerifyError> {
    let key_b64 = key_b64.trim();
    if key_b64.is_empty() || key_b64 == PLACEHOLDER_PUBLIC_KEY {
        return Err(VerifyError::NotSigned);
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .map_err(|_| VerifyError::BadKey)?;
    raw.try_into().map_err(|_| VerifyError::BadKey)
}

/// Checks the Ed25519 signature `sig` (64 raw bytes) over `sums`.
pub fn verify_sums_signature(key_b64: &str, sums: &[u8], sig: &[u8]) -> Result<(), VerifyError> {
    let key = public_key(key_b64)?;
    if sig.len() != 64 {
        return Err(VerifyError::BadSignature);
    }
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(sums, sig)
        .map_err(|_| VerifyError::BadSignature)
}

/// `sha256sum` output: `<64 hex>  <name>` (or ` *<name>`, binary mode) per line.
pub fn parse_sums(sums: &[u8]) -> Result<HashMap<String, [u8; 32]>, VerifyError> {
    let text = std::str::from_utf8(sums).map_err(|_| VerifyError::MalformedSums("not UTF-8".into()))?;
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (hex, rest) = line
            .split_once(' ')
            .ok_or_else(|| VerifyError::MalformedSums(line.to_string()))?;
        let name = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*')).unwrap_or(rest);
        let name = name.strip_prefix("./").unwrap_or(name);
        let digest = decode_hex(hex).ok_or_else(|| VerifyError::MalformedSums(line.to_string()))?;
        if name.is_empty() {
            return Err(VerifyError::MalformedSums(line.to_string()));
        }
        if out.insert(name.to_string(), digest).is_some() {
            return Err(VerifyError::DuplicateEntry(name.to_string()));
        }
    }
    Ok(out)
}

fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// The SHA-256 that SHA256SUMS gives for `name`.
pub fn expected_digest(sums: &[u8], name: &str) -> Result<[u8; 32], VerifyError> {
    parse_sums(sums)?
        .remove(name)
        .ok_or_else(|| VerifyError::MissingEntry(name.to_string()))
}

/// Checks a downloaded file's SHA-256 (`actual`) against its line.
pub fn check_digest(sums: &[u8], name: &str, actual: &[u8]) -> Result<(), VerifyError> {
    if expected_digest(sums, name)?.as_slice() == actual {
        Ok(())
    } else {
        Err(VerifyError::HashMismatch(name.to_string()))
    }
}

/// Header line CI writes first into SHA256SUMS: `# lunaris <version>`. The
/// signature covers it, which binds the list to one version.
pub const VERSION_LINE_PREFIX: &str = "# lunaris ";

/// The version the signed list declares (its `# lunaris <version>` line).
pub fn sums_version(sums: &[u8]) -> Option<String> {
    std::str::from_utf8(sums)
        .ok()?
        .lines()
        .find_map(|l| l.trim_end_matches('\r').strip_prefix(VERSION_LINE_PREFIX))
        .map(|v| v.trim().trim_start_matches('v').to_string())
        .filter(|v| !v.is_empty())
}

/// The signed list must be for `release_version` (without a leading `v`).
/// Call after [`verify_sums_signature`].
pub fn check_sums_version(sums: &[u8], release_version: &str) -> Result<(), VerifyError> {
    let release = release_version.trim().trim_start_matches('v').to_string();
    match sums_version(sums) {
        Some(signed) if signed == release => Ok(()),
        signed => Err(VerifyError::WrongVersion { signed, release }),
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, data);
    d.as_ref().try_into().expect("SHA-256 is 32 bytes")
}

/// The whole check for a file held in memory: the signature of `sums`,
/// then the file's hash against its line.
pub fn verify_release_file(
    key_b64: &str,
    sums: &[u8],
    sig: &[u8],
    name: &str,
    file: &[u8],
) -> Result<(), VerifyError> {
    verify_sums_signature(key_b64, sums, sig)?;
    check_digest(sums, name, &sha256(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    /// A throwaway key pair, made for this test run only.
    fn keypair() -> (Ed25519KeyPair, String) {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = base64::engine::general_purpose::STANDARD.encode(pair.public_key().as_ref());
        (pair, public)
    }

    fn hex(d: &[u8]) -> String {
        d.iter().map(|b| format!("{b:02x}")).collect()
    }

    const FILE: &[u8] = b"pretend this is lunaris-server-linux-x86_64.tar.gz";
    const NAME: &str = "lunaris-server-linux-x86_64.tar.gz";

    fn sums_for(file: &[u8]) -> Vec<u8> {
        format!(
            "# lunaris 0.6.4\n{}  {}\n{}  lunaris-agent-installer-windows.exe\n",
            hex(&sha256(file)),
            NAME,
            hex(&sha256(b"other"))
        )
        .into_bytes()
    }

    #[test]
    fn a_good_signature_and_file_pass() {
        let (pair, public) = keypair();
        let sums = sums_for(FILE);
        let sig = pair.sign(&sums);
        assert_eq!(verify_release_file(&public, &sums, sig.as_ref(), NAME, FILE), Ok(()));
    }

    #[test]
    fn a_tampered_sums_file_fails() {
        let (pair, public) = keypair();
        let sums = sums_for(FILE);
        let sig = pair.sign(&sums);
        // The attacker's own file, and its hash put into SHA256SUMS.
        let evil = b"evil package";
        let tampered = sums_for(evil);
        assert_eq!(
            verify_release_file(&public, &tampered, sig.as_ref(), NAME, evil),
            Err(VerifyError::BadSignature)
        );
        // One byte changed.
        let mut flipped = sums.clone();
        flipped[0] ^= 1;
        assert_eq!(verify_sums_signature(&public, &flipped, sig.as_ref()), Err(VerifyError::BadSignature));
    }

    #[test]
    fn a_signature_by_another_key_fails() {
        let (pair, _) = keypair();
        let (_, other_public) = keypair();
        let sums = sums_for(FILE);
        let sig = pair.sign(&sums);
        assert_eq!(verify_sums_signature(&other_public, &sums, sig.as_ref()), Err(VerifyError::BadSignature));
        assert_eq!(verify_sums_signature(&other_public, &sums, &[0u8; 10]), Err(VerifyError::BadSignature));
    }

    #[test]
    fn a_tampered_file_fails() {
        let (pair, public) = keypair();
        let sums = sums_for(FILE);
        let sig = pair.sign(&sums);
        let mut file = FILE.to_vec();
        file.push(b'!');
        assert_eq!(
            verify_release_file(&public, &sums, sig.as_ref(), NAME, &file),
            Err(VerifyError::HashMismatch(NAME.into()))
        );
    }

    #[test]
    fn a_missing_entry_fails() {
        let (pair, public) = keypair();
        let sums = sums_for(FILE);
        let sig = pair.sign(&sums);
        assert_eq!(
            verify_release_file(&public, &sums, sig.as_ref(), "lunaris-agent_0.6.4_amd64.deb", FILE),
            Err(VerifyError::MissingEntry("lunaris-agent_0.6.4_amd64.deb".into()))
        );
    }

    #[test]
    fn the_placeholder_key_refuses() {
        let (pair, _) = keypair();
        let sums = sums_for(FILE);
        let sig = pair.sign(&sums);
        assert!(!signing_configured(PLACEHOLDER_PUBLIC_KEY));
        assert_eq!(
            verify_release_file(PLACEHOLDER_PUBLIC_KEY, &sums, sig.as_ref(), NAME, FILE),
            Err(VerifyError::NotSigned)
        );
        assert_eq!(verify_sums_signature("", &sums, sig.as_ref()), Err(VerifyError::NotSigned));
        assert_eq!(verify_sums_signature("AAAA", &sums, sig.as_ref()), Err(VerifyError::BadKey));
        assert_eq!(VerifyError::NotSigned.to_string(), "Updates are not signed in this build");
    }

    #[test]
    fn the_built_in_key_is_a_real_key() {
        // A valid 32-byte key: builds can install signed updates.
        let r = public_key(RELEASE_SIGNING_PUBLIC_KEY_B64);
        assert!(r.is_ok(), "{r:?}");
        assert!(signing_configured(RELEASE_SIGNING_PUBLIC_KEY_B64));
    }

    /// The commands of docs/release-signing.md and release.yml, with a
    /// throwaway key in a temporary folder (skipped without `openssl`).
    #[cfg(unix)]
    #[test]
    fn openssl_signature_interop() {
        use std::process::Command;
        let sh = |script: &str, dir: &std::path::Path| {
            Command::new("sh").arg("-c").arg(script).current_dir(dir).output().expect("sh")
        };
        let dir = std::env::temp_dir().join(format!("lunaris-sign-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        if !sh("command -v openssl", &dir).status.success() {
            eprintln!("openssl not found: skipped");
            return;
        }
        std::fs::write(dir.join(NAME), FILE).unwrap();
        let out = sh(
            &format!(
                "set -e; umask 077; openssl genpkey -algorithm ed25519 -out key.pem; \
                 sha256sum -- {NAME} > SHA256SUMS; \
                 openssl pkeyutl -sign -rawin -inkey key.pem -in SHA256SUMS -out SHA256SUMS.sig; \
                 openssl pkey -in key.pem -pubout -outform DER | tail -c 32 | base64"
            ),
            &dir,
        );
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let public = String::from_utf8(out.stdout).unwrap().trim().to_string();
        let sums = std::fs::read(dir.join("SHA256SUMS")).unwrap();
        let sig = std::fs::read(dir.join("SHA256SUMS.sig")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(sig.len(), 64);
        assert_eq!(verify_release_file(&public, &sums, &sig, NAME, FILE), Ok(()));
        assert_eq!(verify_release_file(&public, &sums, &sig, NAME, b"tampered"), Err(VerifyError::HashMismatch(NAME.into())));
    }

    #[test]
    fn sums_format() {
        let d = hex(&sha256(b"x"));
        let text = format!("{d}  a.deb\n{d} *b.exe\r\n\n{d}  ./c.AppImage\n");
        let map = parse_sums(text.as_bytes()).unwrap();
        assert_eq!(map.len(), 3);
        assert!(map.contains_key("a.deb") && map.contains_key("b.exe") && map.contains_key("c.AppImage"));
        assert!(matches!(parse_sums(format!("{d}  a\n{d}  a\n").as_bytes()), Err(VerifyError::DuplicateEntry(_))));
        assert!(matches!(parse_sums(b"zz  a\n"), Err(VerifyError::MalformedSums(_))));
        assert!(matches!(parse_sums(b"nospace\n"), Err(VerifyError::MalformedSums(_))));
    }
}

#[cfg(test)]
mod version_binding_tests {
    use super::*;

    #[test]
    fn the_signed_list_is_bound_to_its_version() {
        let sums = b"# lunaris 0.6.4\n";
        assert_eq!(sums_version(sums).as_deref(), Some("0.6.4"));
        assert_eq!(check_sums_version(sums, "0.6.4"), Ok(()));
        assert_eq!(check_sums_version(sums, "v0.6.4"), Ok(()));
        // An old signed list re-published under a newer tag is refused.
        assert!(matches!(check_sums_version(sums, "9.9.9"), Err(VerifyError::WrongVersion { .. })));
        // A list without the line is refused too.
        let bare = b"0000000000000000000000000000000000000000000000000000000000000000  x\n";
        assert!(matches!(check_sums_version(bare, "0.6.4"), Err(VerifyError::WrongVersion { signed: None, .. })));
        // The header is not taken for a file entry.
        assert_eq!(parse_sums(b"# lunaris 0.6.4\n").unwrap().len(), 0);
    }
}
