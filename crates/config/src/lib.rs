//! dbee config (layer L1): connection settings, URL parsing, the saved/recent connections file
//! and secret stores. No password is ever part of [`ConnectionConfig`] or the file.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod connection;
mod kind;
pub mod secrets;
pub mod settings;
pub mod store;

pub use connection::{
    ConfigError, ConnectionColor, ConnectionConfig, DbKind, KindInfo, KindOption, Settings, SshAuth, SshConfig, SslMode, Unknown, UrlParts, home_dir,
    is_database_host, is_host_name, new_id, parse_url,
};
pub use kind::mysql::ALLOW_PUBLIC_KEY;
pub use secrets::{KeyringStore, MemoryStore, SavedFor, SecretError, SecretKind, SecretStore, StoredSecret, keyring_store};
pub use settings::{
    SETTINGS_FILE, SETTINGS_VERSION, SaveError, ThemeChoice, load_theme, load_update_check, read_small_file, save_theme, save_update_check,
    write_small_file,
};
pub use store::{ConnectionStore, RecentEntry};
