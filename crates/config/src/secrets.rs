//! Secret stores: the OS keyring and an in-memory one for tests and isolated runs.

use keyring_core::{CredentialStore, Entry, Error};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::connection::{ConnectionConfig, DbKind, SshConfig, SslMode, Unknown};

const SERVICE: &str = "dev.rosin.dbee";

/// The form of [`StoredSecret`] in the store that this build writes and reads.
const FORM: u64 = 1;

/// Which secret of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    DbPassword,
    SshPassword,
    SshPassphrase,
}

impl SecretKind {
    fn suffix(self) -> &'static str {
        match self {
            SecretKind::DbPassword => "db-password",
            SecretKind::SshPassword => "ssh-password",
            SecretKind::SshPassphrase => "ssh-passphrase",
        }
    }
}

/// The settings a stored secret was saved for. They are kept with it, in the store, because the
/// connections file and a window's copy of it can change without the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedFor {
    pub kind: DbKind,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub ssl_mode: SslMode,
    pub ca_file: Option<PathBuf>,
    pub ssh: Option<SshConfig>,
    /// The ids of the options on that protect the password less ([`KindOption::weakens`](crate::KindOption::weakens)).
    pub weakening: Vec<String>,
}

impl SavedFor {
    /// The settings of `config` that decide where a secret may go.
    pub fn of(config: &ConnectionConfig) -> SavedFor {
        SavedFor {
            kind: config.kind,
            host: config.host.clone(),
            port: config.port,
            user: config.user.clone(),
            database: config.database.clone(),
            ssl_mode: config.ssl_mode,
            ca_file: config.ca_file.clone(),
            ssh: config.ssh.as_ref().map(|ssh| SshConfig { extra: Unknown::new(), ..ssh.clone() }),
            weakening: config.kind.info().options.iter().filter(|o| o.weakens && config.option_on(o.id)).map(|o| o.id.to_owned()).collect(),
        }
    }
}

/// What the store's text for one secret says.
#[derive(Clone, PartialEq, Eq)]
pub enum StoredSecret {
    /// The secret with the settings it was saved for.
    For { secret: String, saved_for: Box<SavedFor> },
    /// An older build's secret: the text itself, saved for nothing it says.
    Bare(String),
    /// A form this build can't read, a later build's. It is no password to send.
    Unreadable,
}

impl std::fmt::Debug for StoredSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StoredSecret::For { .. } => "StoredSecret::For",
            StoredSecret::Bare(_) => "StoredSecret::Bare",
            StoredSecret::Unreadable => "StoredSecret::Unreadable",
        })
    }
}

#[derive(Serialize)]
struct Written<'a> {
    v: u64,
    secret: &'a str,
    #[serde(rename = "for")]
    saved_for: &'a SavedFor,
}

impl StoredSecret {
    /// The store's text for `secret` saved for `saved_for`.
    pub fn text(secret: &str, saved_for: &SavedFor) -> Result<String, SecretError> {
        serde_json::to_string(&Written { v: FORM, secret, saved_for })
            .map_err(|_| SecretError("the settings can't be written beside the secret".to_string()))
    }

    /// Reads the store's text. A password can be any text, so only an object with all three of the
    /// form's fields counts as the form.
    pub fn parse(text: &str) -> StoredSecret {
        let bare = || StoredSecret::Bare(text.to_owned());
        let Ok(serde_json::Value::Object(mut fields)) = serde_json::from_str(text) else { return bare() };
        let (Some(form), Some(secret), Some(saved_for)) = (fields.remove("v"), fields.remove("secret"), fields.remove("for")) else { return bare() };
        match (form.as_u64() == Some(FORM), secret, serde_json::from_value(saved_for)) {
            (true, serde_json::Value::String(secret), Ok(saved_for)) => StoredSecret::For { secret, saved_for: Box::new(saved_for) },
            _ => StoredSecret::Unreadable,
        }
    }
}

/// A secret store failure. The message never contains a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretError(pub String);

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "secret store: {}", self.0)
    }
}

impl std::error::Error for SecretError {}

/// Where passwords and passphrases live.
pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str, kind: SecretKind) -> Result<Option<String>, SecretError>;
    fn set(&self, id: &str, kind: SecretKind, value: &str) -> Result<(), SecretError>;
    /// Deleting something that is not there is not an error.
    fn delete(&self, id: &str, kind: SecretKind) -> Result<(), SecretError>;
    fn name(&self) -> &'static str;
}

/// Secrets in process memory only.
#[derive(Default)]
pub struct MemoryStore {
    map: Mutex<HashMap<(String, SecretKind), String>>,
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MemoryStore")
    }
}

impl MemoryStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(String, SecretKind), String>> {
        self.map.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, id: &str, kind: SecretKind) -> Result<Option<String>, SecretError> {
        Ok(self.lock().get(&(id.to_string(), kind)).cloned())
    }
    fn set(&self, id: &str, kind: SecretKind, value: &str) -> Result<(), SecretError> {
        self.lock().insert((id.to_string(), kind), value.to_string());
        Ok(())
    }
    fn delete(&self, id: &str, kind: SecretKind) -> Result<(), SecretError> {
        self.lock().remove(&(id.to_string(), kind));
        Ok(())
    }
    fn name(&self) -> &'static str {
        "memory"
    }
}

/// The OS keyring (Secret Service, Keychain, Credential Manager).
#[derive(Clone)]
pub struct KeyringStore {
    store: Arc<CredentialStore>,
}

impl std::fmt::Debug for KeyringStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyringStore")
    }
}

/// Maps a keyring error to text that cannot carry the secret: only platform error text is kept.
fn describe(e: &Error) -> SecretError {
    let text = match e {
        Error::PlatformFailure(p) => format!("platform failure: {p}"),
        Error::NoStorageAccess(p) => format!("no access to the keyring: {p}"),
        Error::NoEntry => "no such entry".to_string(),
        Error::BadEncoding(_) => "stored value is not UTF-8".to_string(),
        Error::BadDataFormat(..) | Error::BadStoreFormat(_) => "stored value has an unexpected format".to_string(),
        Error::TooLong(attr, max) => format!("{attr} is longer than {max}"),
        Error::Invalid(attr, _) => format!("invalid {attr}"),
        Error::Ambiguous(_) => "more than one matching entry".to_string(),
        _ => "keyring error".to_string(),
    };
    SecretError(text)
}

impl KeyringStore {
    fn entry(&self, id: &str, kind: SecretKind) -> Result<Entry, SecretError> {
        self.store.build(SERVICE, &format!("{id}/{}", kind.suffix()), None).map_err(|e| describe(&e))
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, id: &str, kind: SecretKind) -> Result<Option<String>, SecretError> {
        match self.entry(id, kind)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(describe(&e)),
        }
    }
    fn set(&self, id: &str, kind: SecretKind, value: &str) -> Result<(), SecretError> {
        self.entry(id, kind)?.set_password(value).map_err(|e| describe(&e))
    }
    fn delete(&self, id: &str, kind: SecretKind) -> Result<(), SecretError> {
        match self.entry(id, kind)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(describe(&e)),
        }
    }
    fn name(&self) -> &'static str {
        "keyring"
    }
}

/// Connects to the platform keyring. `Err` when no backend answers, for example no Secret
/// Service on D-Bus. Tests must not call this: it touches the real keyring.
pub fn keyring_store() -> Result<KeyringStore, SecretError> {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let store: Arc<CredentialStore> = zbus_secret_service_keyring_store::Store::new().map_err(|e| describe(&e))?;
    #[cfg(target_os = "macos")]
    let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new().map_err(|e| describe(&e))?;
    #[cfg(target_os = "windows")]
    let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new().map_err(|e| describe(&e))?;
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows")))]
    return Err(SecretError("no keyring on this platform".to_string()));
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows"))]
    Ok(KeyringStore { store })
}
