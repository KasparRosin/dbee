//! dbee update (layer L2): everything the update check needs below the engine, with no UI.
//!
//! - [`manifest`]: a release's `SHA256SUMS.txt`, read as hostile input;
//! - [`version`]: which release a copy is offered;
//! - [`signature`]: the manifest's ed25519 signature against the keys compiled in;
//! - [`install`]: how this copy was installed, which asset is its own, and what a click does;
//! - [`fetch`]: where the manifest comes from, and the one HTTPS GET that reads it;
//! - [`cache`]: the last check's time and answer, and when the next check is due.
//!
//! The client never takes a URL from the network: every URL is built here from a fixed template
//! and names checked against the asset grammar.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cache;
pub mod fetch;
pub mod install;
pub mod manifest;
pub mod signature;
pub mod version;

pub use cache::{Cache, FIRST_CHECK_DELAY, FRESH, RECHECK, first_check_in};
pub use fetch::{DEFAULT_REPO, Fetch, HttpFetch, MANIFEST_URL_ENV, PRETEND_VERSION_ENV, Repo, Source, release_page};
pub use install::{Action, Install, InstallKind, Offer, Os, Platform, Probe, Progress, asset_name, is_development_build, offer};
pub use manifest::{Asset, MANIFEST_NAME, MAX_MANIFEST, Manifest};
pub use semver::Version;
pub use signature::{PINNED_KEYS, PublicKey, ReleaseCheck, SIGNATURE_LEN, SIGNATURE_NAME, check_release, verify};
pub use version::{is_newer, parse_version};

/// Why an update step failed. The texts are dbee's own words and never quote the bytes read from
/// the network beyond a file name that passed the asset grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateError {
    /// The manifest is not what a release's `SHA256SUMS.txt` looks like.
    Manifest(String),
    /// The signature does not belong to the manifest, or to none of the pinned keys.
    Signature(&'static str),
    /// The manifest could not be read from the network.
    Fetch(String),
    /// The install step failed.
    Install(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Manifest(why) => write!(f, "the release's file list could not be read: {why}"),
            UpdateError::Signature(why) => write!(f, "the release's signature was refused: {why}"),
            UpdateError::Fetch(why) => write!(f, "the update check could not reach the releases: {why}"),
            UpdateError::Install(why) => write!(f, "the update could not be installed: {why}"),
        }
    }
}

impl std::error::Error for UpdateError {}
