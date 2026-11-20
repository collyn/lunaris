//! In-app updates of the Lunaris app (`client-desktop`), the restart of the
//! agent (`lunaris-agent`, no window) when its program file is replaced, and
//! the "a new version exists" notice of the standalone server.
//!
//! The flow (see [`service`]): ask GitHub for the latest stable release,
//! compare its version with this build's, work out how this copy was
//! installed ([`kind`]) and which release file updates it. When the user
//! presses Update: download `SHA256SUMS` and `SHA256SUMS.sig`, check the
//! Ed25519 signature with the key built into this program ([`verify`]),
//! download the file into a private temporary folder, check its SHA-256
//! against its line in `SHA256SUMS`, and only then install it ([`install`])
//! and restart ([`restart`]).
//!
//! Everything that decides something is a pure function with unit tests;
//! the Qt windows only call [`service`] and show its JSON status.

pub mod kind;
pub mod release;
pub mod restart;
pub mod verify;
pub mod version;

#[cfg(feature = "net")]
pub mod install;
#[cfg(feature = "net")]
mod net;
#[cfg(feature = "net")]
pub mod service;

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

pub use kind::App;
