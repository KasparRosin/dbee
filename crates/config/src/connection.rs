//! Connection settings and URL parsing. There is no password field: passwords live in a
//! [`SecretStore`](crate::SecretStore).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::kind;

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

/// What a database type's settings consist of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settings {
    /// Host and port, with optional TLS and SSH.
    Server { tls: bool, ssh: bool },
    /// A single file, opened by these extensions.
    File { extensions: &'static [&'static str] },
}

/// The fixed facts of one database type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub default_port: Option<u16>,
    pub settings: Settings,
    pub url_schemes: &'static [&'static str],
    /// This type's own TLS-mode names in URLs, besides libpq's.
    pub ssl_aliases: &'static [(&'static str, SslMode)],
    /// The type's own on/off settings, kept in [`ConnectionConfig::options`].
    pub options: &'static [KindOption],
}

/// An on/off setting of one database type, saved under `id` in [`ConnectionConfig::options`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KindOption {
    pub id: &'static str,
    pub label: &'static str,
    /// On, it protects the password less: a stored password is not used for settings that turn it
    /// on where the saved ones had it off.
    pub weakens: bool,
}

/// The database types dbee can open. Saved as the type's id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "&'static str")]
pub enum DbKind {
    #[default]
    Postgres,
    MySql,
    MariaDb,
    Sqlite,
}

impl DbKind {
    pub const ALL: [DbKind; 4] = [DbKind::Postgres, DbKind::MySql, DbKind::MariaDb, DbKind::Sqlite];

    pub const fn info(self) -> &'static KindInfo {
        match self {
            DbKind::Postgres => &kind::postgres::INFO,
            DbKind::MySql => &kind::mysql::INFO,
            DbKind::MariaDb => &kind::mysql::MARIADB,
            DbKind::Sqlite => &kind::sqlite::INFO,
        }
    }

    pub const fn id(self) -> &'static str {
        self.info().id
    }

    pub fn from_id(id: &str) -> Option<DbKind> {
        DbKind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub const fn label(self) -> &'static str {
        self.info().label
    }

    pub const fn default_port(self) -> Option<u16> {
        self.info().default_port
    }

    pub const fn uses_file(self) -> bool {
        matches!(self.info().settings, Settings::File { .. })
    }

    /// Its connections may go through an SSH tunnel.
    pub const fn takes_ssh(self) -> bool {
        matches!(self.info().settings, Settings::Server { ssh: true, .. })
    }
}

impl From<DbKind> for &'static str {
    fn from(k: DbKind) -> Self {
        k.id()
    }
}

impl TryFrom<String> for DbKind {
    type Error = String;
    fn try_from(id: String) -> Result<Self, String> {
        DbKind::from_id(&id).ok_or_else(|| format!("unknown database type {id:?}"))
    }
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
    /// Fields this build does not know, read from the connections file and written back to it;
    /// never serialized anywhere else.
    #[serde(flatten, skip_serializing)]
    pub extra: Unknown,
}

impl Default for SshConfig {
    fn default() -> Self {
        SshConfig { host: String::new(), port: 22, user: String::new(), auth: SshAuth::default(), extra: Unknown::new() }
    }
}

/// Fields of a saved connection that this build does not know (a newer build's), by name.
pub type Unknown = serde_json::Map<String, serde_json::Value>;

/// One saved connection. Never holds a password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub kind: DbKind,
    pub file: Option<PathBuf>,
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
    /// The type's own settings ([`KindInfo::options`]), by id; keys this build does not know
    /// are kept.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub options: BTreeMap<String, String>,
    /// Fields this build does not know, read from the connections file and written back to it;
    /// never serialized anywhere else. A command's config has none: one would be refused.
    #[serde(flatten, skip_serializing)]
    pub extra: Unknown,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        ConnectionConfig {
            id: String::new(),
            name: String::new(),
            kind: DbKind::default(),
            file: None,
            color: ConnectionColor::None,
            host: "localhost".into(),
            port: DbKind::default().default_port().unwrap_or(0),
            user: String::new(),
            database: String::new(),
            ssl_mode: SslMode::Prefer,
            ca_file: None,
            ssh: None,
            read_only: false,
            save_secrets: true,
            options: BTreeMap::new(),
            extra: Unknown::new(),
        }
    }
}

impl ConnectionConfig {
    /// The names of the fields this build does not know, its tunnel's as `ssh.<name>`.
    pub fn unknown_fields(&self) -> Vec<String> {
        let ssh = self.ssh.iter().flat_map(|ssh| ssh.extra.keys().map(|k| format!("ssh.{k}")));
        self.extra.keys().cloned().chain(ssh).collect()
    }

    /// Takes the fields this build does not know from `old`, the same connection as the file held
    /// it, where this one lacks them (a config from a form or a script has none).
    pub(crate) fn keep_unknown(&mut self, old: &ConnectionConfig) {
        keep(&mut self.extra, &old.extra);
        if let (Some(ssh), Some(old)) = (self.ssh.as_mut(), old.ssh.as_ref()) {
            keep(&mut ssh.extra, &old.extra);
        }
    }

    /// The option `id` ([`KindInfo::options`]) is on: its value is `"true"`.
    pub fn option_on(&self, id: &str) -> bool {
        self.options.get(id).is_some_and(|v| v == "true")
    }

    /// The name; when empty, a file type's file name, else `user@host/database` (`user@host`
    /// without a database).
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        let file_name = self.file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
        match (self.kind.uses_file(), file_name) {
            (true, Some(n)) => n,
            _ => format!("{}@{}{}", self.user, self.host, self.database_suffix()),
        }
    }

    /// `/database`, or nothing when no database is set.
    pub fn database_suffix(&self) -> String {
        if self.database.is_empty() { String::new() } else { format!("/{}", self.database) }
    }

    /// A file type needs a file and takes no SSH tunnel. A server type needs a host and a port, and
    /// for SSH (where its type takes it) a host, a port and a user.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_for(self.kind.info().settings)
    }

    /// [`ConnectionConfig::validate`] for a type whose settings are `settings`.
    fn validate_for(&self, settings: Settings) -> Result<(), ConfigError> {
        let bad = |m: &str| Err(ConfigError::Invalid(m.to_string()));
        let ssh_allowed = match settings {
            Settings::File { .. } if self.ssh.is_some() => return bad("a database file has no SSH tunnel"),
            Settings::File { .. } => {
                return match &self.file {
                    Some(f) if !f.as_os_str().is_empty() => Ok(()),
                    _ => bad("file is empty"),
                };
            }
            Settings::Server { ssh, .. } => ssh,
        };
        if self.ssh.is_some() && !ssh_allowed {
            return bad("this database type takes no SSH tunnel");
        }
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

/// Adds the fields of `old` that `extra` lacks.
pub(crate) fn keep(extra: &mut Unknown, old: &Unknown) {
    for (k, v) in old {
        extra.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// What a connection URL fills in. Absent parts stay `None`. `Debug` hides the password.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct UrlParts {
    pub kind: DbKind,
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
            .field("kind", &self.kind)
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

/// libpq's TLS-mode names, which every type takes.
fn libpq_ssl_mode(v: &str) -> Option<SslMode> {
    Some(match v {
        "disable" => SslMode::Disable,
        "allow" | "prefer" => SslMode::Prefer,
        "require" => SslMode::Require,
        "verify-ca" => SslMode::VerifyCa,
        "verify-full" => SslMode::VerifyFull,
        _ => return None,
    })
}

/// Parses a server type's URL (the schemes in the type table): userinfo and path are percent-decoded,
/// `[v6]` hosts work, and `?sslmode=` or `?ssl-mode=` is read. Other query parameters are ignored.
pub fn parse_url(url: &str) -> Result<UrlParts, ConfigError> {
    let err = |m: &str| ConfigError::Url(m.to_string());
    let url = url.trim();
    let (kind, rest) = DbKind::ALL
        .into_iter()
        .flat_map(|k| k.info().url_schemes.iter().map(move |s| (k, *s)))
        .find_map(|(k, scheme)| {
            let head = url.get(..scheme.len())?;
            let tail = url.get(scheme.len()..)?.strip_prefix("://")?;
            head.eq_ignore_ascii_case(scheme).then_some((k, tail))
        })
        .ok_or_else(|| {
            let schemes: Vec<String> = DbKind::ALL.iter().flat_map(|k| k.info().url_schemes).map(|s| format!("{s}://")).collect();
            ConfigError::Url(format!("expected a {} URL", schemes.join(", ")))
        })?;
    let rest = rest.split('#').next().unwrap_or("");
    let (before_query, query) = match rest.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match before_query.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (before_query, None),
    };
    let mut parts = UrlParts { kind, ..UrlParts::default() };
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
        if let Some((key, v)) = pair.split_once('=')
            && (key == "sslmode" || key == "ssl-mode")
        {
            let v = percent_decode(v)?;
            let alias = kind.info().ssl_aliases.iter().find(|(name, _)| name.eq_ignore_ascii_case(&v)).map(|(_, m)| *m);
            parts.ssl_mode = Some(libpq_ssl_mode(&v).or(alias).ok_or_else(|| err("unknown sslmode"))?);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_holds_every_kind_once() {
        // Each kind and the next, by an exhaustive match: a new variant joins the chain here.
        let after = |k: DbKind| match k {
            DbKind::Postgres => Some(DbKind::MySql),
            DbKind::MySql => Some(DbKind::MariaDb),
            DbKind::MariaDb => Some(DbKind::Sqlite),
            DbKind::Sqlite => None,
        };
        let every: Vec<DbKind> = std::iter::successors(Some(DbKind::Postgres), |k| after(*k)).collect();
        assert_eq!(DbKind::ALL.to_vec(), every);
        let mut ids: Vec<&str> = DbKind::ALL.iter().map(|k| k.info().id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DbKind::ALL.len(), "each id once");
    }

    #[test]
    fn a_server_type_without_ssh_refuses_a_tunnel() {
        let ssh = SshConfig { host: "jump.example".into(), port: 22, user: "u".into(), auth: SshAuth::Password, ..SshConfig::default() };
        let c = ConnectionConfig { ssh: Some(ssh), ..ConnectionConfig::default() };
        assert_eq!(c.validate_for(Settings::Server { tls: true, ssh: true }), Ok(()));
        let refused = c.validate_for(Settings::Server { tls: true, ssh: false });
        assert_eq!(refused, Err(ConfigError::Invalid("this database type takes no SSH tunnel".into())));
        assert_eq!(ConnectionConfig { ssh: None, ..c }.validate_for(Settings::Server { tls: true, ssh: false }), Ok(()));
    }
}
