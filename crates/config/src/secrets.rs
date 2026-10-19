//! Secret stores: the OS keyring and an in-memory one for tests and isolated runs.

use keyring_core::{CredentialStore, Entry, Error};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const SERVICE: &str = "dev.rosin.dbee";

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
