//! Connection settings and URL parsing. There is no password field: passwords live in a
//! [`SecretStore`](crate::SecretStore).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Errors of this crate. The text never contains a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// A setting is not usable.
    Invalid(String),
    /// A connection URL could not be parsed.
    Url(String),
    /// The connections file could not be read or written.
    Io(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Invalid(m) => write!(f, "invalid connection settings: {m}"),
            ConfigError::Url(m) => write!(f, "invalid connection URL: {m}"),
            ConfigError::Io(m) => write!(f, "connections file: {m}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// TLS mode, named as in libpq.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SslMode {
    #[serde(rename = "disable")]
    Disable,
    #[default]
    #[serde(rename = "prefer")]
    Prefer,
    #[serde(rename = "require")]
    Require,
    #[serde(rename = "verify-ca")]
    VerifyCa,
    #[serde(rename = "verify-full")]
    VerifyFull,
}

/// The colour tag shown for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionColor {
    #[default]
    None,
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

/// How to authenticate to the SSH server. `Key { path: None }` is automatic: agent with
/// identities, then `~/.ssh/id_ed25519`, `id_ecdsa`, `id_rsa`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum SshAuth {
    Key { path: Option<PathBuf> },
    Agent,
    Password,
}

impl Default for SshAuth {
    fn default() -> Self {
        SshAuth::Key { path: None }
    }
}

/// An SSH jump host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SshConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
}

impl Default for SshConfig {
    fn default() -> Self {
        SshConfig { host: String::new(), port: 22, user: String::new(), auth: SshAuth::default() }
    }
}

/// One saved connection. Never holds a password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub color: ConnectionColor,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    pub ssl_mode: SslMode,
    pub ca_file: Option<PathBuf>,
    pub ssh: Option<SshConfig>,
    pub read_only: bool,
    pub save_secrets: bool,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        ConnectionConfig {
            id: String::new(),
            name: String::new(),
            color: ConnectionColor::None,
            host: "localhost".into(),
            port: 5432,
            user: String::new(),
            database: String::new(),
            ssl_mode: SslMode::Prefer,
            ca_file: None,
            ssh: None,
            read_only: false,
            save_secrets: true,
        }
    }
}

impl ConnectionConfig {
    /// The name, or `user@host/database` when the name is empty.
    pub fn display_name(&self) -> String {
        if self.name.is_empty() { format!("{}@{}/{}", self.user, self.host, self.database) } else { self.name.clone() }
    }

    /// Host non-empty, port non-zero, and for SSH a host, a port and a user.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: &str| Err(ConfigError::Invalid(m.to_string()));
        if self.host.trim().is_empty() {
            return bad("host is empty");
        }
        if self.port == 0 {
            return bad("port must not be 0");
        }
        if let Some(ssh) = &self.ssh {
            if ssh.host.trim().is_empty() {
                return bad("SSH host is empty");
            }
            if ssh.port == 0 {
                return bad("SSH port must not be 0");
            }
            if ssh.user.trim().is_empty() {
                return bad("SSH user is empty");
            }
        }
        Ok(())
    }
}

/// What a connection URL fills in. Absent parts stay `None`. `Debug` hides the password.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct UrlParts {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub ssl_mode: Option<SslMode>,
}

impl std::fmt::Debug for UrlParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UrlParts")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("database", &self.database)
            .field("ssl_mode", &self.ssl_mode)
            .finish()
    }
}

fn percent_decode(s: &str) -> Result<String, ConfigError> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
            match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => out.push(v),
                None => return Err(ConfigError::Url("bad percent escape".into())),
            }
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ConfigError::Url("not valid UTF-8 after decoding".into()))
}

fn some_nonempty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// Parses `postgres://` and `postgresql://` URLs: userinfo and path are percent-decoded, `[v6]`
/// hosts work, and `?sslmode=` is read. Other query parameters are ignored.
pub fn parse_url(url: &str) -> Result<UrlParts, ConfigError> {
    let err = |m: &str| ConfigError::Url(m.to_string());
    let url = url.trim();
    let rest = ["postgresql://", "postgres://"]
        .iter()
        .find_map(|p| url.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).map(|_| &url[p.len()..]))
        .ok_or_else(|| err("expected a postgres:// or postgresql:// URL"))?;
    let rest = rest.split('#').next().unwrap_or("");
    let (before_query, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match before_query.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (before_query, None),
    };
    let mut parts = UrlParts::default();
    let hostport = match authority.rsplit_once('@') {
        Some((userinfo, hp)) => {
            let (u, p) = match userinfo.split_once(':') {
                Some((u, p)) => (u, Some(p)),
                None => (userinfo, None),
            };
            parts.user = some_nonempty(percent_decode(u)?);
            parts.password = match p {
                Some(p) => Some(percent_decode(p)?),
                None => None,
            };
            hp
        }
        None => authority,
    };
    let (host, port) = if let Some(after) = hostport.strip_prefix('[') {
        let (h, tail) = after.split_once(']').ok_or_else(|| err("unterminated [ in host"))?;
        let port = match tail {
            "" => None,
            t => Some(t.strip_prefix(':').ok_or_else(|| err("junk after ]"))?),
        };
        (h, port)
    } else {
        match hostport.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    parts.host = some_nonempty(percent_decode(host)?);
    parts.port = match port {
        None | Some("") => None,
        Some(p) => Some(p.parse::<u16>().map_err(|_| err("bad port"))?),
    };
    if let Some(p) = path {
        parts.database = some_nonempty(percent_decode(p)?);
    }
    for pair in query.unwrap_or("").split('&') {
        if let Some(("sslmode", v)) = pair.split_once('=') {
            parts.ssl_mode = Some(match percent_decode(v)?.as_str() {
                "disable" => SslMode::Disable,
                "allow" | "prefer" => SslMode::Prefer,
                "require" => SslMode::Require,
                "verify-ca" => SslMode::VerifyCa,
                "verify-full" => SslMode::VerifyFull,
                _ => return Err(err("unknown sslmode")),
            });
        }
    }
    Ok(parts)
}

/// The user's home directory: `HOME`, then `USERPROFILE`.
pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"].iter().find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty())).map(PathBuf::from)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh connection id: hex of the time in nanoseconds plus a process-wide counter.
pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{nanos:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}
