//! Key files: the default key order and loading a key with its passphrase.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use russh::keys::{Error as KeyError, PrivateKey, decode_secret_key};

use crate::SshEnv;

/// The key files looked for in `~/.ssh` when the agent has no identities, in this order.
pub const DEFAULT_KEYS: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// Key files are small (a 16384-bit RSA key is about 12 KiB); anything larger is not read.
const MAX_KEY_FILE: u64 = 1 << 20;

/// The first of [`DEFAULT_KEYS`] that is a file in `env.ssh_dir`.
pub fn default_key(env: &SshEnv) -> Option<PathBuf> {
    let dir = env.ssh_dir.as_ref()?;
    DEFAULT_KEYS.iter().map(|name| dir.join(name)).find(|p| p.is_file())
}

/// Why a key file could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoadError {
    /// The key is encrypted and no passphrase was given.
    NeedPassphrase,
    /// The key is encrypted and the passphrase does not open it.
    WrongPassphrase,
    /// Missing, not a regular file, too large, or not a key this client reads. The text names no
    /// key material.
    Unreadable(String),
}

fn read_key_file(path: &Path) -> Result<String, LoadError> {
    let bad = |m: String| LoadError::Unreadable(m);
    let meta = std::fs::metadata(path).map_err(|e| bad(e.to_string()))?;
    if !meta.is_file() {
        return Err(bad("not a regular file".into()));
    }
    if meta.len() > MAX_KEY_FILE {
        return Err(bad("larger than 1 MiB, so not a key file".into()));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path).and_then(|f| f.take(MAX_KEY_FILE + 1).read_to_end(&mut bytes)).map_err(|e| bad(e.to_string()))?;
    String::from_utf8(bytes).map_err(|_| bad("not a text key file".into()))
}

/// PEM forms that are encrypted but that the decoder only reports as malformed without a
/// passphrase: PKCS#8 `ENCRYPTED PRIVATE KEY` and the legacy `Proc-Type: 4,ENCRYPTED` header.
fn looks_encrypted(text: &str) -> bool {
    text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") || text.contains("Proc-Type: 4,ENCRYPTED")
}

/// Load a private key, opening it with `passphrase` only when it is encrypted. Blocking: the
/// key derivation of an encrypted key takes a noticeable time, so async callers use [`load_async`].
pub(crate) fn load(path: &Path, passphrase: Option<&str>) -> Result<PrivateKey, LoadError> {
    let text = read_key_file(path)?;
    match decode_secret_key(&text, None) {
        Ok(key) => Ok(key),
        Err(KeyError::KeyIsEncrypted) => decrypt(&text, passphrase),
        Err(_) if looks_encrypted(&text) => decrypt(&text, passphrase),
        Err(e) => Err(LoadError::Unreadable(e.to_string())),
    }
}

fn decrypt(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, LoadError> {
    let passphrase = passphrase.ok_or(LoadError::NeedPassphrase)?;
    decode_secret_key(text, Some(passphrase)).map_err(|_| LoadError::WrongPassphrase)
}

/// [`load`] on the blocking pool. A panic in the key decoder ends up as `Unreadable`.
pub(crate) async fn load_async(path: &Path, passphrase: Option<&str>) -> Result<PrivateKey, LoadError> {
    let (path, passphrase) = (path.to_path_buf(), passphrase.map(str::to_owned));
    tokio::task::spawn_blocking(move || load(&path, passphrase.as_deref()))
        .await
        .unwrap_or_else(|_| Err(LoadError::Unreadable("the key decoder failed".into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentSource, SshEnv};
    use dbee_testkit::temp_home;

    #[test]
    fn key_order_is_ed25519_ecdsa_rsa() {
        assert_eq!(DEFAULT_KEYS, ["id_ed25519", "id_ecdsa", "id_rsa"]);
        let home = temp_home(None, None);
        let ssh = home.ssh_dir();
        let env = SshEnv { ssh_dir: Some(ssh.clone()), agent: AgentSource::None };
        assert_eq!(default_key(&env), None);
        std::fs::write(ssh.join("id_rsa"), "x").unwrap();
        std::fs::write(ssh.join("id_ecdsa"), "x").unwrap();
        assert_eq!(default_key(&env), Some(ssh.join("id_ecdsa")));
        std::fs::write(ssh.join("id_ed25519"), "x").unwrap();
        assert_eq!(default_key(&env), Some(ssh.join("id_ed25519")));
        std::fs::remove_file(ssh.join("id_ed25519")).unwrap();
        std::fs::remove_file(ssh.join("id_ecdsa")).unwrap();
        assert_eq!(default_key(&env), Some(ssh.join("id_rsa")));
        // A directory with a key's name is not a key; no `.ssh` at all means no key.
        std::fs::create_dir(ssh.join("id_ed25519")).unwrap();
        assert_eq!(default_key(&env), Some(ssh.join("id_rsa")));
        assert_eq!(default_key(&SshEnv { ssh_dir: None, agent: AgentSource::None }), None);
    }

    #[test]
    fn unreadable_key_files() {
        let home = temp_home(None, None);
        assert!(matches!(load(&home.home.join("nope"), None), Err(LoadError::Unreadable(_))));
        let junk = home.home.join("junk");
        std::fs::write(&junk, "not a key").unwrap();
        assert!(matches!(load(&junk, Some("pp")), Err(LoadError::Unreadable(_))));
        std::fs::write(&junk, [0xff, 0xfe, 0x00]).unwrap();
        assert!(matches!(load(&junk, None), Err(LoadError::Unreadable(_))));
        assert!(matches!(load(&home.home, None), Err(LoadError::Unreadable(_))));
    }

    #[test]
    fn loading_tells_passphrase_problems_apart() {
        let db = dbee_testkit::require_testdb!();
        assert!(load(&db.keys.plain, None).is_ok());
        assert!(load(&db.keys.plain, Some("ignored")).is_ok());
        assert!(matches!(load(&db.keys.encrypted, None), Err(LoadError::NeedPassphrase)));
        assert!(matches!(load(&db.keys.encrypted, Some("wrong")), Err(LoadError::WrongPassphrase)));
        assert!(matches!(load(&db.keys.encrypted, Some("")), Err(LoadError::WrongPassphrase)));
        assert!(load(&db.keys.encrypted, Some(&db.keys.passphrase)).is_ok());
        assert!(load(&db.keys.rsa, None).is_ok_and(|k| k.algorithm().is_rsa()));
    }
}
