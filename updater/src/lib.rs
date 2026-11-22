//! What the agent and the server share about Lunaris releases: the restart
//! of the agent (`lunaris-agent`, no window) when its program file is
//! replaced ([`restart`]), and for the server the latest release on GitHub
//! ([`release`]), the version comparison ([`version`]) and the Ed25519
//! check of a release's `SHA256SUMS` ([`verify`]): the "a new version
//! exists" notice and the one-line installer's checksums. (The client is
//! updated by its package or installer: it opens only from links.)

pub mod release;
pub mod restart;
pub mod verify;
pub mod version;

/// GitHub repository the releases come from.
pub const REPO: &str = "collyn/lunaris";
/// The latest stable release (GitHub leaves drafts and prereleases out).
pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/collyn/lunaris/releases/latest";
/// Where a person downloads a release by hand.
pub const RELEASES_PAGE: &str = "https://github.com/collyn/lunaris/releases/latest";
/// Release files are only ever downloaded from under this address.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/collyn/lunaris/releases/download/";
/// Generic User-Agent: nothing about the user or the computer is sent.
pub const USER_AGENT: &str = "lunaris-updater";
/// This build's version (the workspace version, root Cargo.toml).
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
