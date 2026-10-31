//! dbee config (layer L1): connection settings, URL parsing, the saved/recent connections file
//! and secret stores. No password is ever part of [`ConnectionConfig`] or the file.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod connection;
pub mod secrets;
pub mod store;

pub use connection::{
    ALLOW_PUBLIC_KEY, ConfigError, ConnectionColor, ConnectionConfig, DbKind, KindInfo, KindOption, Settings, SshAuth, SshConfig, SslMode, UrlParts,
    home_dir, new_id, parse_url,
};
pub use secrets::{KeyringStore, MemoryStore, SecretError, SecretKind, SecretStore, keyring_store};
pub use store::{ConnectionStore, RecentEntry};
