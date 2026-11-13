//! Connection commands: the saved and recent list (save, delete, duplicate), URL parsing, the
//! SSH key preview, test, the server's database list, open, opening a database file and saving
//! the open connection, reconnect, close, and the schema refresh.
//!
//! Secrets (the database password, the SSH password and the key passphrase) come from the
//! params first (`password`, `sshPassword`, `sshPassphrase`; an empty string counts as not
//! given), then the secret store, then nowhere. The store keeps secrets only for saved
//! connections, under their id, each with the settings it was saved for ([`SavedFor`]). What the
//! store says decides where a secret may go, at the read in the job ([`read_secret`], [`reuse`]),
//! never the connections file or a window's copy of it, so a stale window, an edited file or a
//! move while a save writes never send it elsewhere:
//! - the database password: the same database type, host, port and user, whichever database the
//!   settings name, the same tunnel (no tunnel on either side, or the same SSH host and port), TLS
//!   at least as strong as the saved one (disable < prefer < require < verify-ca < verify-full, with
//!   `require` and a CA file as `verify-ca`) and the same CA file where either side checks with one;
//! - the SSH password: the same SSH host, port and user;
//! - the key passphrase: the same SSH host, port and user, and the same key file.
//!
//! A secret stored by an older build says nothing: the saved connection decides this once, as it
//! did then, and the secret is stored again with those settings.
//!
//! So an edited form or an agent that sends a saved id with another server never gets the saved
//! secrets, and saving new settings forgets the stored secrets they no longer apply to (unless
//! the save gives them again) and saves the others for the new settings. Secrets are
//! kept only in the store and in the crate-private [`Connected::credentials`] (for reconnects),
//! never in the config, the connections file, `inspect()`, `Debug`, logs, errors or the journal
//! (the journal drops secret params by key, and `connection.parseUrl`, whose URL may carry a
//! password, is not journaled; nor are `connection.test` and `connection.listDatabases`). Store
//! reads run in the job, so a slow keyring never holds up the UI thread; writes
//! (`connection.save`, `savePassword`) and what a save or a copy reads for them run on the caller's
//! thread with the saved-connection change they belong to.
//!
//! An isolated run's allowed targets ([`crate::allowed`]) are checked by test, the database list,
//! open and reconnect before anything touches the network. Its allowed folders go to the driver
//! (`dbee_driver::Target::allowed_dirs`): a file driver checks its file inside the open, so the
//! refusal is the job's error (`config`), and keeps the connection's SQL from other files. The
//! job checks an SSH key file and a CA file the same way before it opens anything
//! ([`allowed::check_files`]).
//!
//! Connections open through the session's driver for the config's database type
//! ([`SessionOptions::drivers`](crate::SessionOptions::drivers)); test, the database list, open
//! and reconnect refuse a type without one (`disabled`, "this build has no MySQL driver") before
//! any job starts.
//!
//! Connecting is a job: the tunnel, the browse connection and, for open and reconnect, the
//! catalog, whose load is bounded ([`CATALOG_TIMEOUT`]: past it, it is cancelled, the connection
//! is discarded and the job fails with `timeout`). `connection.cancel` stops the attempts of open,
//! openFile, reconnect and test wherever they are: the step that runs is dropped (with the socket
//! it holds), what the attempt opened before it is closed, the tunnel last, and the job ends
//! [`ATTEMPT_CANCELLED`]; so does one whose work had ended but whose result was not applied yet,
//! and what it opened is closed. The prompts come back as
//! [`EngineError::Needs`]: an unknown host key, a key passphrase, the SSH password, or the
//! database password when none was given; the caller asks the user and runs the command again
//! with the answer in its params.
//!
//! A lost connection: when a browse operation fails with `connectionLost` (or the connection
//! has closed), the session is marked [`Connected::lost`] and the commands that need the
//! database are disabled with [`LOST`] until `connection.reconnect` replaces the connections.
//! Database work is stopped only through the connections' `Cancel`, never by dropping a running
//! apply; a job whose connection was closed or replaced meanwhile is ignored (`{"ignored": true}`,
//! see [`super`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dbee_config::{
    ConfigError, ConnectionConfig, ConnectionStore, DbKind, SavedFor, SecretKind, SecretStore, Settings, SshAuth, SslMode, StoredSecret, new_id,
    parse_url,
};
use dbee_core::Catalog;
use dbee_driver::{Cancel, Connection, DbError, DbFuture, Driver, Drivers, Purpose, Target, tls};
use dbee_ssh::{KeyPlan, SshEnv, SshError, SshSecrets, Tunnel};
use serde::Deserialize as _;
use serde_json::json;

use super::query::{self, End};
use super::{CommandSpec, LOST, MAX_OPEN, NOT_CONNECTED, always, bad, connected, disabled, ignored, needed_connection, needs_db, not_connected, p};
use crate::allowed;
use crate::jobs::{Stop, unless_stopped};
use crate::state::{Connected, Credentials, CurrentDatabase, server_text};
use crate::turns::{self, InTurn};
use crate::{Apply, ConnectionId, EndAbout, EngineError, Json, Need, Outcome, Screen, Session, Tab, UiRequest, Which};

const LIST: &str = "connection.list";
const SAVE: &str = "connection.save";
const DELETE: &str = "connection.delete";
const DUPLICATE: &str = "connection.duplicate";
const PARSE_URL: &str = "connection.parseUrl";
const SSH_KEY: &str = "connection.sshKey";
const TEST: &str = "connection.test";
const LIST_DATABASES: &str = "connection.listDatabases";
/// The command that opens a connection.
pub const OPEN: &str = "connection.open";
/// The command that opens a database file.
pub const OPEN_FILE: &str = "connection.openFile";
const SAVE_OPEN: &str = "connection.saveOpen";
/// The command that opens the open connection again.
pub const RECONNECT: &str = "connection.reconnect";
const CANCEL: &str = "connection.cancel";
const REFRESH: &str = "schema.refresh";
const CLOSE: &str = "connection.close";

/// How a connection attempt that `connection.cancel` stopped ends (code `cancelled`).
pub const ATTEMPT_CANCELLED: &str = "the connection attempt was cancelled";

/// What `connection.cancel` says when no attempt runs.
pub const NO_ATTEMPT: &str = "no connection attempt is running";

pub(crate) const SPECS: &[CommandSpec] = &[
    CommandSpec {
        id: LIST,
        label: "List Connections",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: always,
        run: list,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: SAVE,
        label: "Save Connection",
        menu: &[],
        shortcut: None,
        params: "{config, password?, sshPassword?, sshPassphrase?}",
        enabled: always,
        run: save,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: DELETE,
        label: "Delete Connection",
        menu: &[],
        shortcut: None,
        params: "{id}",
        enabled: always,
        run: delete,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: DUPLICATE,
        label: "Duplicate Connection",
        menu: &[],
        shortcut: None,
        params: "{id}",
        enabled: always,
        run: duplicate,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: PARSE_URL,
        label: "Fill from URL",
        menu: &[],
        shortcut: None,
        params: "{url}",
        enabled: always,
        run: parse,
        // The URL may carry a password, which the journal's key-based stripping would miss.
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: SSH_KEY,
        label: "Show SSH Key",
        menu: &[],
        shortcut: None,
        params: "{config}",
        enabled: always,
        run: ssh_key,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: TEST,
        label: "Test Connection",
        menu: &[],
        shortcut: None,
        params: "{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?, createFile?}",
        enabled: always,
        run: test,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: LIST_DATABASES,
        label: "List Databases",
        menu: &[],
        shortcut: None,
        params: "{config, password?, sshPassword?, sshPassphrase?, trustHostKey?}",
        enabled: always,
        run: list_databases,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: OPEN,
        label: "Connect",
        menu: &["File"],
        shortcut: None,
        params: "{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?, savePassword?, createFile?}",
        enabled: not_connected,
        run: open,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: OPEN_FILE,
        label: "Open Database File",
        menu: &[],
        shortcut: None,
        params: "{path, password?}",
        enabled: not_connected,
        run: open_file,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: SAVE_OPEN,
        label: "Save Open Connection",
        menu: &[],
        shortcut: None,
        params: "{connection?}",
        enabled: unsaved_open,
        run: save_open,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: RECONNECT,
        label: "Reconnect",
        menu: &[],
        shortcut: None,
        params: "{connection?, password?, sshPassword?, sshPassphrase?, trustHostKey?}",
        enabled: connected,
        run: reconnect,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: CANCEL,
        label: "Cancel Connecting",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: always,
        run: cancel,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: REFRESH,
        label: "Refresh Schema",
        menu: &["View"],
        shortcut: None,
        params: "{connection?}",
        enabled: needs_db,
        run: refresh,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: CLOSE,
        label: "Disconnect",
        menu: &["File"],
        shortcut: Some("Ctrl+Shift+Q"),
        params: "{connection?, force?, end?: \"commit\" or \"rollback\"}",
        enabled: connected,
        run: close,
        journal: true,
        undoable: false,
    },
];

/// The longest a catalog load may take inside a job (open, reconnect, refresh).
const CATALOG_TIMEOUT: Duration = Duration::from_secs(60);

/// After a timed-out catalog load is cancelled, how long it gets to end before it is given up.
const CANCEL_GRACE: Duration = Duration::from_secs(5);

// ---- params ----------------------------------------------------------------------------------

/// `params[key]` as a [`ConnectionConfig`]; `None` when missing or null. A key the config does not
/// have is refused, so a misspelt one (`hots`) is not saved quietly.
fn parse_config(params: &Json, key: &str) -> Result<Option<ConnectionConfig>, EngineError> {
    let Some(v) = params.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    if !v.is_object() {
        return Err(bad(format!("`{key}` must be a connection object")));
    }
    let config = ConnectionConfig::deserialize(v).map_err(|e| bad(format!("`{key}` is not a connection: {e}")))?;
    if let Some(unknown) = config.unknown_fields().first() {
        return Err(bad(format!("`{unknown}` in `{key}` is not a connection setting")));
    }
    // The tunnel's login keeps no unknown fields: its keys are those it serializes back to.
    let given_auth = v.get("ssh").and_then(|ssh| ssh.get("auth")).and_then(Json::as_object);
    if let (Some(ssh), Some(given_auth)) = (&config.ssh, given_auth) {
        let known = serde_json::to_value(&ssh.auth).ok();
        if let Some(unknown) = given_auth.keys().find(|k| !known.as_ref().and_then(Json::as_object).is_some_and(|m| m.contains_key(*k))) {
            return Err(bad(format!("`ssh.auth.{unknown}` in `{key}` is not a connection setting")));
        }
    }
    Ok(Some(config))
}

/// `config` for a command that connects or saves: a server's must name its `host` (so a bare `{}`
/// can never reach the default `localhost:5432`), and it must be valid (a file's names its file).
fn connect_config(params: &Json) -> Result<Option<ConnectionConfig>, EngineError> {
    let names_host = params.get("config").is_some_and(|v| v.get("host").is_some_and(Json::is_string));
    let Some(config) = parse_config(params, "config")? else { return Ok(None) };
    if !config.kind.uses_file() && !names_host {
        return Err(bad("`config` must name its `host`"));
    }
    config.validate().map_err(|e| bad(format!("`config`: {e}")))?;
    Ok(Some(config))
}

/// The connection `{id}` (saved, then recent) or `{config}` names: exactly one of them. There is
/// no default: tests and agents must name a server.
fn target_config(s: &Session, params: &Json) -> Result<ConnectionConfig, EngineError> {
    let id = p::opt_str(params, "id")?;
    let config = connect_config(params)?;
    match (id, config) {
        (Some(_), Some(_)) => Err(bad("give `id` or `config`, not both")),
        (Some(id), None) => {
            let c = s.state.store.get(id).cloned().ok_or_else(|| bad("no saved or recent connection has this `id`"))?;
            c.validate().map_err(|e| EngineError::Config(format!("the saved connection cannot be used: {e}")))?;
            Ok(c)
        }
        (None, Some(c)) => Ok(c),
        (None, None) => Err(bad("name the connection with `id`, or give its `config`")),
    }
}

/// A secret param; an empty string counts as not given.
fn secret(params: &Json, key: &str) -> Result<Option<String>, EngineError> {
    Ok(p::opt_str(params, key)?.filter(|v| !v.is_empty()).map(str::to_owned))
}

/// The secrets and the trusted host key fingerprint the params give.
fn given(params: &Json) -> Result<Credentials, EngineError> {
    Ok(Credentials {
        password: secret(params, "password")?,
        ssh: SshSecrets { password: secret(params, "sshPassword")?, passphrase: secret(params, "sshPassphrase")? },
        trust: p::opt_str(params, "trustHostKey")?.filter(|v| !v.is_empty()).map(str::to_owned),
    })
}

// ---- secrets in the store --------------------------------------------------------------------

const SECRET_KINDS: [SecretKind; 3] = [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase];

/// The secret stored as `kind` of `config.id`, if `config` may use it: what the store says it was
/// saved for decides ([`reuse`]), at this read, never the connections file or a window's copy of it.
/// A failing store counts as none (the user is then asked).
///
/// An older build's secret says nothing, so `saved`, the saved connection, decides this once, and
/// the secret is stored again for it.
fn read_secret(store: &dyn SecretStore, config: &ConnectionConfig, saved: &SavedFor, kind: SecretKind) -> Option<String> {
    let text = match store.get(&config.id, kind) {
        Ok(text) => text?,
        Err(_) => {
            log::warn!("the secret store could not be read; going on without the saved secret");
            return None;
        }
    };
    let secret = match StoredSecret::parse(&text) {
        StoredSecret::For { secret, saved_for } => reuse(&saved_for, config).allows(kind).then_some(secret),
        StoredSecret::Bare(secret) => reuse(saved, config).allows(kind).then(|| {
            if !store_secret(store, &config.id, kind, &secret, saved) {
                log::warn!("the secret store refused a change; the saved secret stays as an older build stored it");
            }
            secret
        }),
        StoredSecret::Unreadable => None,
    };
    secret.filter(|v| !v.is_empty())
}

/// Stores `secret` for `saved_for`; `false` when the store refused.
fn store_secret(store: &dyn SecretStore, id: &str, kind: SecretKind, secret: &str, saved_for: &SavedFor) -> bool {
    StoredSecret::text(secret, saved_for).and_then(|text| store.set(id, kind, &text)).is_ok()
}

/// Stores each secret `c` holds under `id`, for `saved_for`; `false` when the store refused one.
fn store_secrets(store: &dyn SecretStore, id: &str, c: &Credentials, saved_for: &SavedFor) -> bool {
    let mut ok = true;
    for (kind, value) in
        [(SecretKind::DbPassword, &c.password), (SecretKind::SshPassword, &c.ssh.password), (SecretKind::SshPassphrase, &c.ssh.passphrase)]
    {
        if let Some(v) = value {
            ok &= store_secret(store, id, kind, v, saved_for);
        }
    }
    ok
}

/// What a save with the settings `now` does to the stored secrets `given` does not hold again: each
/// that `now` may still use is saved for `now` from here on, and the others are deleted. `before`,
/// the settings saved before, stands in for what an older build's secret says nothing about. With
/// no setting changed, nothing is touched. `false` when the store refused.
fn carry_secrets(store: &dyn SecretStore, config: &ConnectionConfig, now: &SavedFor, before: Option<&SavedFor>, given: &Credentials) -> bool {
    if before == Some(now) {
        return true;
    }
    let mut ok = true;
    for (kind, again) in [
        (SecretKind::DbPassword, given.password.is_some()),
        (SecretKind::SshPassword, given.ssh.password.is_some()),
        (SecretKind::SshPassphrase, given.ssh.passphrase.is_some()),
    ] {
        if !again {
            ok &= carry_secret(store, config, now, before, kind);
        }
    }
    ok
}

/// [`carry_secrets`] for one secret.
fn carry_secret(store: &dyn SecretStore, config: &ConnectionConfig, now: &SavedFor, before: Option<&SavedFor>, kind: SecretKind) -> bool {
    let id = config.id.as_str();
    let text = match store.get(id, kind) {
        Ok(Some(text)) => text,
        Ok(None) => return true,
        Err(_) => return false,
    };
    let stored = StoredSecret::parse(&text);
    let (secret, saved_for) = match &stored {
        StoredSecret::For { secret, saved_for } => (secret.as_str(), Some(saved_for.as_ref())),
        StoredSecret::Bare(secret) => (secret.as_str(), before),
        StoredSecret::Unreadable => ("", None),
    };
    let applies = saved_for.is_some_and(|saved_for| reuse(saved_for, config).allows(kind));
    if !applies {
        return store.delete(id, kind).is_ok();
    }
    saved_for == Some(now) || store_secret(store, id, kind, secret, now)
}

/// Deletes every secret of `id`; `false` when the store refused.
fn forget_secrets(store: &dyn SecretStore, id: &str) -> bool {
    let mut ok = true;
    for kind in SECRET_KINDS {
        ok &= store.delete(id, kind).is_ok();
    }
    ok
}

/// Tells the user (and the log, without details) that the secret store failed.
fn secret_store_failed(s: &mut Session, what: &str) {
    log::warn!("the secret store refused a change");
    s.ui_requests.push(UiRequest::Notice(format!("The secret store refused to {what}; dbee will ask when a secret is needed.")));
}

/// Which stored secrets of a saved connection a config may use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Reuse {
    db: bool,
    ssh_password: bool,
    passphrase: bool,
}

impl Reuse {
    fn allows(self, kind: SecretKind) -> bool {
        match kind {
            SecretKind::DbPassword => self.db,
            SecretKind::SshPassword => self.ssh_password,
            SecretKind::SshPassphrase => self.passphrase,
        }
    }
}

/// How strong a TLS mode is with its CA file: disable < prefer < require < verify-ca < verify-full,
/// where `require` with a CA file checks what `verify-ca` checks.
fn tls_strength(mode: SslMode, ca_file: Option<&Path>) -> u8 {
    match mode {
        SslMode::Disable => 0,
        SslMode::Prefer => 1,
        SslMode::Require if ca_file.is_none() => 2,
        SslMode::Require | SslMode::VerifyCa => 3,
        SslMode::VerifyFull => 4,
    }
}

/// The secrets saved for `saved` that `given` may use: each only where it was saved for.
/// - The database password: the same database type, host, port and user, as it reaches the same
///   server and user whichever database the settings name; the same tunnel
///   (no tunnel on either side, or the same SSH host and port, since behind a tunnel the database
///   host is usually `localhost` of another machine); TLS at least as strong as the saved one
///   ([`tls_strength`]), and the same CA file where either side checks the certificate with one; and
///   no option that protects it less ([`KindOption::weakens`](dbee_config::KindOption::weakens)) on
///   where the saved settings had it off.
/// - The SSH password: the same SSH host, port and user.
/// - The key passphrase: the same SSH host, port and user, and the same key file.
fn reuse(saved: &SavedFor, given: &ConnectionConfig) -> Reuse {
    let same_tunnel = match (&saved.ssh, &given.ssh) {
        (None, None) => true,
        (Some(a), Some(b)) => a.host == b.host && a.port == b.port,
        _ => false,
    };
    let (saved_ca, given_ca) = (saved.ca_file.as_deref(), given.ca_file.as_deref());
    let checks = tls::checks(saved.ssl_mode, saved_ca) || tls::checks(given.ssl_mode, given_ca);
    let weaker = given.kind.info().options.iter().any(|o| o.weakens && given.option_on(o.id) && !saved.weakening.iter().any(|on| on == o.id));
    let db = saved.kind == given.kind
        && saved.host == given.host
        && saved.port == given.port
        && saved.user == given.user
        && same_tunnel
        && tls_strength(given.ssl_mode, given_ca) >= tls_strength(saved.ssl_mode, saved_ca)
        && (!checks || given_ca == saved_ca)
        && !weaker;
    let (ssh_password, passphrase) = match (&saved.ssh, &given.ssh) {
        (Some(a), Some(b)) if a.host == b.host && a.port == b.port && a.user == b.user => {
            let same_key = matches!((&a.auth, &b.auth), (SshAuth::Key { path: x }, SshAuth::Key { path: y }) if x == y);
            (true, same_key)
        }
        _ => (false, false),
    };
    Reuse { db, ssh_password, passphrase }
}

/// The settings of the saved connection with `config`'s id, for [`fill_from_store`]; `None` when no
/// saved connection has that id.
fn saved_settings(s: &Session, config: &ConnectionConfig) -> Option<SavedFor> {
    s.state.store.saved.iter().find(|c| c.id == config.id).map(SavedFor::of)
}

/// `c` with what it lacks filled from the store under `config.id`: params first, then the store,
/// then none. Only what the config's login can use is read, and only for a saved connection
/// (`saved`, its settings). [`read_secret`] decides whether each may be used.
async fn fill_from_store(mut c: Credentials, store: Option<Arc<dyn SecretStore>>, config: &ConnectionConfig, saved: Option<SavedFor>) -> Credentials {
    let (Some(store), Some(saved)) = (store, saved) else { return c };
    let want_db = c.password.is_none();
    let (want_ssh_password, want_passphrase) = match config.ssh.as_ref().map(|ssh| &ssh.auth) {
        Some(SshAuth::Password) => (c.ssh.password.is_none(), false),
        Some(SshAuth::Key { .. }) => (false, c.ssh.passphrase.is_none()),
        Some(SshAuth::Agent) | None => (false, false),
    };
    if !(want_db || want_ssh_password || want_passphrase) {
        return c;
    }
    let config = config.clone();
    let reading = tokio::task::spawn_blocking(move || {
        let get = |want: bool, kind| if want { read_secret(store.as_ref(), &config, &saved, kind) } else { None };
        (get(want_db, SecretKind::DbPassword), get(want_ssh_password, SecretKind::SshPassword), get(want_passphrase, SecretKind::SshPassphrase))
    });
    match reading.await {
        Ok((db, ssh_password, passphrase)) => {
            c.password = c.password.or(db);
            c.ssh.password = c.ssh.password.or(ssh_password);
            c.ssh.passphrase = c.ssh.passphrase.or(passphrase);
        }
        Err(_) => log::warn!("reading the secret store failed; going on without the saved secrets"),
    }
    c
}

// ---- errors ----------------------------------------------------------------------------------

/// A tunnel error: the prompts become `Needs`, everything else `Db { code: "ssh" }` with a
/// `reason` (and the `known_hosts` line for a changed host key).
pub(crate) fn ssh_error(e: &SshError) -> EngineError {
    let message = e.to_string();
    let data = match e {
        SshError::HostKeyUnknown { host, key_type, fingerprint } => {
            let need = Need::HostKey { host: host.clone(), key_type: key_type.clone(), fingerprint: fingerprint.clone() };
            return EngineError::Needs { need, message };
        }
        SshError::NeedPassphrase { key } => return EngineError::Needs { need: Need::Passphrase { key: key.display().to_string() }, message },
        SshError::NeedPassword => return EngineError::Needs { need: Need::SshPassword, message },
        SshError::HostKeyChanged { host, line } => json!({ "reason": "hostKeyChanged", "host": host, "line": line }),
        SshError::HostKeyOtherType { host, key_type, recorded, lines } => {
            json!({ "reason": "hostKeyOtherType", "host": host, "keyType": key_type, "recorded": recorded, "lines": lines })
        }
        SshError::HostKeyRevoked { host, line } => json!({ "reason": "hostKeyRevoked", "host": host, "line": line }),
        SshError::WrongPassphrase { key } => json!({ "reason": "wrongPassphrase", "key": key.display().to_string() }),
        SshError::AuthFailed { tried } => json!({ "reason": "authFailed", "tried": tried }),
        SshError::NoKey => json!({ "reason": "noKey" }),
        SshError::Connect(_) => json!({ "reason": "connect" }),
        SshError::KnownHosts(_) => json!({ "reason": "knownHosts" }),
        SshError::Channel(_) => json!({ "reason": "channel" }),
    };
    EngineError::Db { code: "ssh", message, data }
}

/// A database or tunnel error as the engine reports it: `Db` with the `DbError` code, its text
/// (which can quote the server, so it is shown, never logged) and details; SSH prompts become
/// `Needs`. `connectionLost` names the command that fixes it in `data.action`.
pub(crate) fn db_error(e: &DbError) -> EngineError {
    let data = match e {
        DbError::Ssh(ssh) => return ssh_error(ssh),
        DbError::Server(s) => json!({ "serverCode": s.code, "detail": s.detail, "hint": s.hint, "position": s.position }),
        DbError::ConnectionLost => json!({ "action": RECONNECT }),
        DbError::RowNotFound { statement } => json!({ "statement": statement }),
        _ => json!({}),
    };
    EngineError::Db { code: e.code(), message: e.to_string(), data }
}

impl Session {
    /// The browse connection of the connection `which` names, for a command that needs the
    /// database. `Disabled` when there is none or it is lost; when it or the tunnel it runs
    /// through has closed since its last use, the connection is marked lost and the error is
    /// `connectionLost`.
    pub(crate) fn browse(&mut self, which: Which) -> Result<Arc<dyn Connection>, EngineError> {
        Ok(self.browse_in_turn(which)?.1)
    }

    /// [`Session::browse`] as the queue it is, for a job that waits for its turn itself
    /// ([`stoppable_in_turn`](crate::turns::stoppable_in_turn)), with the connection's id.
    pub(crate) fn browse_in_turn(&mut self, which: Which) -> Result<(ConnectionId, Arc<InTurn>), EngineError> {
        let c = self.state.connection_mut(which).ok_or_else(|| disabled(NOT_CONNECTED))?;
        if c.lost {
            return Err(disabled(LOST));
        }
        if c.browse.is_closed() || c.tunnel.as_ref().is_some_and(|t| t.is_closed()) {
            c.lost = true;
            log::info!("the browse connection or its tunnel has closed; the session is marked lost");
            return Err(db_error(&DbError::ConnectionLost));
        }
        Ok((c.id, Arc::clone(&c.browse)))
    }

    /// `conn` is still the browse connection of open connection `id`; a job that used another one
    /// (closed or replaced since) must drop its result.
    pub(crate) fn is_current_browse(&self, id: ConnectionId, conn: &Arc<dyn Connection>) -> bool {
        self.state.connection(Which::Id(id)).is_some_and(|c| c.is_browse(conn))
    }

    /// An operation on `conn`, of open connection `id`, failed with `e`. When the connection is
    /// gone, the session lets go of it: a lost browse connection marks the connection lost, and a
    /// lost query connection is dropped (the next query run opens a new one).
    pub(crate) fn db_failed(&mut self, id: ConnectionId, conn: &Arc<dyn Connection>, e: &DbError) -> EngineError {
        if (*e == DbError::ConnectionLost || conn.is_closed())
            && let Some(c) = self.state.connection_mut(Which::Id(id))
        {
            if c.is_browse(conn) && !c.lost {
                c.lost = true;
                log::info!("the browse connection was lost; the session is marked lost");
            }
            if c.query.as_ref().is_some_and(|q| Arc::ptr_eq(q, conn)) {
                c.query = None;
            }
        }
        db_error(e)
    }
}

// ---- connecting ------------------------------------------------------------------------------

/// The driver that opens `kind`; `Disabled` when this build has none.
pub(crate) fn driver_for(drivers: &Drivers, kind: DbKind) -> Result<Arc<dyn Driver>, EngineError> {
    drivers.for_kind(kind).cloned().ok_or_else(|| disabled(format!("this build has no {} driver", kind.label())))
}

/// An open connection and the tunnel it runs through.
struct Opened {
    conn: Arc<dyn Connection>,
    tunnel: Option<Arc<Tunnel>>,
}

impl Opened {
    /// `direct`, or `ssh ` and how the tunnel logged in (`agent`, `key <path>`, `password`).
    fn via(&self) -> String {
        match &self.tunnel {
            None => "direct".into(),
            Some(t) => format!("ssh {}", t.used()),
        }
    }

    /// Closes the connection, then the tunnel.
    async fn close(self) {
        drop(self.conn);
        if let Some(t) = self.tunnel {
            t.close().await;
        }
    }
}

/// `Ok` when this run may reach `config`'s target (an isolated run's allowed targets,
/// [`allowed`]); then the session's allowed folders, which go to the driver.
fn may_reach(s: &Session, config: &ConnectionConfig) -> Result<Option<Vec<PathBuf>>, EngineError> {
    allowed::check(s.allowed_targets.as_deref(), config)?;
    Ok(s.allowed_dirs.clone())
}

/// The tunnel `config` goes through, opened with `creds`: `None` when it has none or its type
/// takes none.
async fn open_tunnel(config: &ConnectionConfig, env: &SshEnv, creds: &Credentials) -> Result<Option<Arc<Tunnel>>, EngineError> {
    match config.ssh.as_ref().filter(|_| config.kind.takes_ssh()) {
        None => Ok(None),
        Some(ssh) => Ok(Some(Arc::new(Tunnel::open(ssh, env, &creds.ssh, creds.trust.as_deref()).await.map_err(|e| ssh_error(&e))?))),
    }
}

/// A driver's failure to reach the database, as the engine reports it: a refused login without a
/// password asks for one.
fn connect_error(e: DbError, creds: &Credentials) -> EngineError {
    match e {
        DbError::Auth { .. } if creds.password.is_none() => EngineError::Needs { need: Need::Password, message: e.to_string() },
        other => db_error(&other),
    }
}

/// The error of a connection attempt that `connection.cancel` stopped.
fn attempt_cancelled() -> EngineError {
    EngineError::Db { code: DbError::Cancelled.code(), message: ATTEMPT_CANCELLED.into(), data: json!({}) }
}

/// Checks the config's key and CA files against `allowed_dirs` (an isolated run's folders), then
/// opens the tunnel (when the config has one and its type takes one) and a connection through it
/// with `driver`, which checks a file against the same folders and makes a missing one only with
/// `create_file`. Once `stop` is set, the step that runs is dropped and the tunnel closed.
async fn open_connection(
    driver: &dyn Driver,
    config: &ConnectionConfig,
    allowed_dirs: Option<&[PathBuf]>,
    create_file: bool,
    env: &SshEnv,
    creds: &Credentials,
    stop: &Stop,
) -> Result<Opened, EngineError> {
    allowed::check_files(allowed_dirs, config).map_err(|e| db_error(&e))?;
    let tunnel = unless_stopped(stop, open_tunnel(config, env, creds)).await.ok_or_else(attempt_cancelled)??;
    let target = Target { config, password: creds.password.as_deref(), tunnel: tunnel.clone(), purpose: Purpose::Browse, create_file, allowed_dirs };
    let failed = match unless_stopped(stop, driver.connect(target)).await {
        Some(Ok(conn)) => return Ok(Opened { conn, tunnel }),
        Some(Err(e)) => connect_error(e, creds),
        None => attempt_cancelled(),
    };
    if let Some(t) = &tunnel {
        t.close().await;
    }
    Err(failed)
}

/// The databases `driver` lists on `config`'s server, through the tunnel when there is one, with
/// why a host key the user trusted was not saved, when it was not. The driver's connect is bounded
/// by its own timeout and the read by [`CATALOG_TIMEOUT`], past which it is cancelled as a catalog
/// read is. The connection, then the tunnel, close before this returns.
async fn list_on(
    driver: &dyn Driver,
    config: &ConnectionConfig,
    allowed_dirs: Option<&[PathBuf]>,
    env: &SshEnv,
    creds: &Credentials,
) -> Result<(Vec<String>, Option<String>), EngineError> {
    allowed::check_files(allowed_dirs, config).map_err(|e| db_error(&e))?;
    let tunnel = open_tunnel(config, env, creds).await?;
    let target =
        Target { config, password: creds.password.as_deref(), tunnel: tunnel.clone(), purpose: Purpose::Browse, create_file: false, allowed_dirs };
    let listed = match driver.list_connection(target).await {
        Ok(None) => Ok(Vec::new()),
        Ok(Some(conn)) => read_within(conn.as_ref(), CATALOG_TIMEOUT, conn.list_databases()).await.map_err(|e| match e {
            CatalogLoad::Failed(e) => db_error(&e),
            CatalogLoad::TimedOut => EngineError::Db {
                code: "timeout",
                message: format!("the server did not list its databases within {} s", CATALOG_TIMEOUT.as_secs()),
                data: json!({}),
            },
        }),
        Err(e) => Err(connect_error(e, creds)),
    };
    let unsaved = unsaved_host_key(tunnel.as_ref());
    if let Some(t) = tunnel {
        t.close().await;
    }
    listed.map(|names| (names, unsaved))
}

/// Why a bounded catalog read (the catalog, the database list) gave nothing.
enum CatalogLoad {
    Failed(DbError),
    /// It took longer than its limit; it was cancelled, and the connection must be discarded.
    TimedOut,
}

/// `read`, a catalog read on `conn`, bounded by `limit`. Past it the statement is cancelled
/// through the connection's `Cancel` and the read gets [`CANCEL_GRACE`] to end (its result is
/// discarded either way); then it is `TimedOut`, and the caller discards the connection.
async fn read_within<T>(conn: &dyn Connection, limit: Duration, mut read: DbFuture<'_, T>) -> Result<T, CatalogLoad> {
    match tokio::time::timeout(limit, &mut read).await {
        Ok(result) => result.map_err(CatalogLoad::Failed),
        Err(_) => {
            log::info!("a catalog read did not end within {} s; cancelling it", limit.as_secs());
            if conn.canceller().cancel().await.is_err() {
                log::info!("the cancel request was not delivered");
            }
            let _ = tokio::time::timeout(CANCEL_GRACE, &mut read).await;
            Err(CatalogLoad::TimedOut)
        }
    }
}

/// The connection's catalog, bounded by `limit` ([`read_within`]).
async fn load_catalog_within(conn: &dyn Connection, limit: Duration) -> Result<Catalog, CatalogLoad> {
    read_within(conn, limit, conn.load_catalog()).await
}

/// [`load_catalog_within`] on the browse connection, once its turn comes: the limit counts from
/// then, as waiting behind another tab's statement is no reason to give up on the connection.
async fn load_catalog_in_turn(conn: &InTurn, limit: Duration) -> Result<Catalog, CatalogLoad> {
    turns::in_turn(conn, |c| load_catalog_within(c, limit)).await
}

/// A refresh whose catalog load timed out: its connection was discarded and the session is lost,
/// so the error offers the reconnect like every lost connection. (`EngineError::Timeout` carries
/// no data, so this is a `Db` error with the same code.)
fn lost_by_timeout() -> EngineError {
    EngineError::Db {
        code: "timeout",
        message: format!("the catalog did not load within {} s, so the connection was dropped; reconnect to go on", CATALOG_TIMEOUT.as_secs()),
        data: json!({ "action": RECONNECT }),
    }
}

fn catalog_error(e: CatalogLoad) -> EngineError {
    match e {
        CatalogLoad::Failed(e) => db_error(&e),
        CatalogLoad::TimedOut => EngineError::Timeout,
    }
}

/// [`open_connection`], then the catalog (bounded by [`CATALOG_TIMEOUT`]). On any failure, and
/// once `stop` is set, the new connection and tunnel are closed.
async fn connect_and_load(
    driver: &dyn Driver,
    config: &ConnectionConfig,
    allowed_dirs: Option<&[PathBuf]>,
    create_file: bool,
    env: &SshEnv,
    creds: &Credentials,
    stop: &Stop,
) -> Result<(Opened, Catalog), EngineError> {
    let opened = open_connection(driver, config, allowed_dirs, create_file, env, creds, stop).await?;
    let failed = match unless_stopped(stop, load_catalog_within(opened.conn.as_ref(), CATALOG_TIMEOUT)).await {
        Some(Ok(catalog)) => return Ok((opened, catalog)),
        Some(Err(e)) => catalog_error(e),
        None => attempt_cancelled(),
    };
    opened.close().await;
    Err(failed)
}

/// `result`, an attempt's, unless `connection.cancel` stopped the attempt after its work ended and
/// before this apply: then what it opened is closed, and it ends cancelled all the same.
fn unless_cancelled(s: &Session, stop: &Stop, result: Result<(Opened, Catalog), EngineError>) -> Result<(Opened, Catalog), EngineError> {
    if !stop.is_set() {
        return result;
    }
    if let Ok((opened, _)) = result {
        s.jobs.spawn_detached(opened.close());
    }
    Err(attempt_cancelled())
}

/// `{connection, server, tables, warnings}` for an open or a reconnect; `saved` as
/// [`Connected::summary`] takes it.
fn opened_json(c: &Connected, saved: bool) -> Json {
    json!({
        "connection": c.summary(saved),
        "server": server_text(&c.info),
        "tables": c.catalog.table_count(),
        "warnings": c.info.warnings,
    })
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Makes one change to the connections file and the session's copy of it
/// ([`ConnectionStore::change`]); a warning about the file becomes the store warning.
fn change_store<R>(s: &mut Session, f: impl FnOnce(&mut ConnectionStore) -> R) -> Result<R, ConfigError> {
    let (r, warning) = s.state.store.change(f)?;
    if warning.is_some() {
        log::warn!("the connections file could not be used as it is");
        s.state.store_warning = warning;
    }
    Ok(r)
}

/// Puts `config` first in the recent list, in the file too; tells the user when the file could
/// not be written (the open stands).
fn record_use(s: &mut Session, config: &ConnectionConfig) {
    if let Err(e) = change_store(s, |store| store.touch_recent(config, unix_now())) {
        log::warn!("the connections file could not be written");
        let notice = match e {
            ConfigError::Locked => e.to_string(),
            e => format!("The recent connections could not be saved: {e}"),
        };
        s.ui_requests.push(UiRequest::Notice(notice));
    }
}

/// Sends a cancel request for the statement running on each of `conns` that is still open, in
/// a detached task. The jobs running them then end with an error (and their results are
/// dropped where their connection is gone).
pub(crate) fn cancel_statements(s: &Session, conns: impl IntoIterator<Item = Arc<dyn Connection>>) {
    let cancellers: Vec<Arc<dyn Cancel>> = conns.into_iter().filter(|c| !c.is_closed()).map(|c| c.canceller()).collect();
    if cancellers.is_empty() {
        return;
    }
    s.jobs.spawn_detached(async move {
        for c in cancellers {
            if c.cancel().await.is_err() {
                log::info!("a cancel request was not delivered");
            }
        }
    });
}

// ---- the commands ----------------------------------------------------------------------------

/// `{}` → `{saved: [config], recent: [{config, usedAt}]}`. Configs hold no secrets.
fn list(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let store = &s.state.store;
    Ok(Outcome::Done(json!({ "saved": store.saved, "recent": store.recent })))
}

/// `{config, password?, sshPassword?, sshPassphrase?}` → `{id}`: adds or replaces the saved
/// connection with the config's id (a new id when it is empty) and writes the file. With
/// `saveSecrets` and a secret store, the given secrets are stored for the new settings, and of the
/// ones not given those the new settings still allow ([`carry_secrets`]) are saved for them too
/// while the others are forgotten; without `saveSecrets`, every stored one is forgotten.
fn save(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let config = connect_config(params)?.ok_or_else(|| bad("`config` is required (a connection object)"))?;
    let secrets = given(params)?;
    let id = save_with_secrets(s, config, &secrets)?;
    Ok(Outcome::Done(json!({ "id": id })))
}

/// What [`save`] does once it has its config and secrets: the saved list and the file, then the
/// secret store as `saveSecrets` says. The config's id, a new one when it had none.
fn save_with_secrets(s: &mut Session, mut config: ConnectionConfig, secrets: &Credentials) -> Result<String, EngineError> {
    if config.id.is_empty() {
        config.id = new_id();
    }
    let before = change_store(s, |store| {
        let before = store.saved.iter().find(|c| c.id == config.id).cloned();
        store.upsert(config.clone());
        before
    })
    .map_err(|e| EngineError::Config(e.to_string()))?;
    if let Some(secret_store) = s.secrets.clone() {
        let ok = if config.save_secrets {
            let now = SavedFor::of(&config);
            let before = before.as_ref().map(SavedFor::of);
            let carried = carry_secrets(secret_store.as_ref(), &config, &now, before.as_ref(), secrets);
            store_secrets(secret_store.as_ref(), &config.id, secrets, &now) && carried
        } else {
            forget_secrets(secret_store.as_ref(), &config.id)
        };
        if !ok {
            secret_store_failed(s, "keep the connection's secrets");
        }
    }
    Ok(config.id)
}

/// `{id}` → `{id}`: removes the connection from the saved and recent lists, with its secrets.
fn delete(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let id = p::str(params, "id")?.to_owned();
    if s.state.store.get(&id).is_none() {
        return Err(bad("no saved or recent connection has this `id`"));
    }
    change_store(s, |store| {
        store.remove(&id);
        store.recent.retain(|r| r.config.id != id);
    })
    .map_err(|e| EngineError::Config(e.to_string()))?;
    if let Some(secret_store) = s.secrets.clone()
        && !forget_secrets(secret_store.as_ref(), &id)
    {
        secret_store_failed(s, "delete the connection's secrets");
    }
    Ok(Outcome::Done(json!({ "id": id })))
}

/// `{id}` → `{id}`: a saved copy named `<name> (copy)`, with a new id and copies of the stored
/// secrets, each with the settings it was saved for.
fn duplicate(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let id = p::str(params, "id")?;
    let original = s.state.store.get(id).cloned().ok_or_else(|| bad("no saved or recent connection has this `id`"))?;
    let copy = ConnectionConfig { id: new_id(), name: format!("{} (copy)", original.display_name()), ..original.clone() };
    change_store(s, |store| store.upsert(copy.clone())).map_err(|e| EngineError::Config(e.to_string()))?;
    if let Some(secret_store) = s.secrets.clone()
        && copy.save_secrets
    {
        let mut ok = true;
        for kind in SECRET_KINDS {
            match secret_store.get(&original.id, kind) {
                Ok(Some(v)) => ok &= secret_store.set(&copy.id, kind, &v).is_ok(),
                Ok(None) => {}
                Err(_) => ok = false,
            }
        }
        if !ok {
            secret_store_failed(s, "copy the connection's secrets");
        }
    }
    Ok(Outcome::Done(json!({ "id": copy.id })))
}

/// `params.createFile`, which a read-only `config` cannot take: a read-only connection makes no
/// file.
fn create_file(params: &Json, config: &ConnectionConfig) -> Result<bool, EngineError> {
    let create = p::bool_or(params, "createFile", false)?;
    if create && config.read_only {
        return Err(bad("A read-only connection can't make a new file. Untick Read-only for the first connect."));
    }
    Ok(create)
}

/// `{url}` → `{kind, host, port, user, passwordGiven, database, sslMode}`: the database type its
/// scheme names, then the parts, each `null` when the URL lacks it. The password is not in the
/// reply, only whether the URL had one. Not journaled.
fn parse(_: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let parts = parse_url(p::str(params, "url")?).map_err(|e| bad(e.to_string()))?;
    Ok(Outcome::Done(json!({
        "kind": parts.kind,
        "host": parts.host,
        "port": parts.port,
        "user": parts.user,
        "passwordGiven": parts.password.is_some(),
        "database": parts.database,
        "sslMode": parts.ssl_mode,
    })))
}

/// `{config}` → a job returning what the SSH login will use: `{method: "agent", identities}`,
/// `{method: "file", path}`, `{method: "password"}` or `{method: "none"}`. Reads no key.
fn ssh_key(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let config = parse_config(params, "config")?.ok_or_else(|| bad("`config` is required (a connection object)"))?;
    let ssh = config.ssh.ok_or_else(|| bad("`config` has no SSH tunnel (`ssh`)"))?;
    let env = s.ssh.clone();
    let job = s.spawn_job(SSH_KEY, async move {
        let result = match dbee_ssh::plan(&ssh, &env).await {
            KeyPlan::Agent { identities } => json!({ "method": "agent", "identities": identities }),
            KeyPlan::File(path) => json!({ "method": "file", "path": path.display().to_string() }),
            KeyPlan::Password => json!({ "method": "password" }),
            KeyPlan::Nothing => json!({ "method": "none" }),
        };
        let apply: Apply = Box::new(move |_: &mut Session| Ok(result));
        apply
    });
    Ok(Outcome::Started(job))
}

/// `{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?, createFile?}` → a job
/// returning `{server, via, warnings}`: connects (through the tunnel when there is one), then
/// closes. `via` is how it got there, `direct` or the tunnel's login; `null` for a type that takes
/// no tunnel, such as a file. The screen and the lists stay as they are. A trusted unknown host key is learned.
/// `createFile: true` lets the file driver make a missing file; an existing one opens as it is. It
/// is refused for a read-only connection ([`create_file`]).
fn test(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let config = target_config(s, params)?;
    let create_file = create_file(params, &config)?;
    let allowed_dirs = may_reach(s, &config)?;
    let driver = driver_for(&s.drivers, config.kind)?;
    let creds = given(params)?;
    let saved = saved_settings(s, &config);
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_attempt(TEST, move |stop| async move {
        let result = match unless_stopped(&stop, fill_from_store(creds, store, &config, saved)).await {
            None => Err(attempt_cancelled()),
            Some(creds) => match open_connection(driver.as_ref(), &config, allowed_dirs.as_deref(), create_file, &env, &creds, &stop).await {
                Ok(opened) => {
                    let info = opened.conn.info();
                    let via = config.kind.takes_ssh().then(|| opened.via());
                    let r = json!({ "server": server_text(info), "via": via, "warnings": info.warnings });
                    let unsaved = unsaved_host_key(opened.tunnel.as_ref());
                    opened.close().await;
                    Ok((r, unsaved))
                }
                Err(e) => Err(e),
            },
        };
        let apply: Apply = Box::new(move |s: &mut Session| {
            if stop.is_set() {
                return Err(attempt_cancelled());
            }
            let (r, unsaved) = result?;
            tell(s, unsaved);
            Ok(r)
        });
        apply
    });
    Ok(Outcome::Started(job))
}

/// `{config, password?, sshPassword?, sshPassphrase?, trustHostKey?}` → a job returning
/// `{databases}`: the names the server lists, for the connection form's Database field to choose
/// from. `config` is the form's, saved or not; the secrets, the prompts and an isolated run's
/// targets are as for [`test`]. It connects (through the tunnel when there is one), reads the list
/// and closes again, and leaves the session as it is, connected or not. Not journaled: its params
/// can carry a password. A type whose connection is one database file is refused.
fn list_databases(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    if let Some(kind) = parse_config(params, "config")?.map(|c| c.kind).filter(|k| k.uses_file()) {
        return Err(bad(format!("{} opens one database file, so there are no databases to list.", kind.label())));
    }
    let config = connect_config(params)?.ok_or_else(|| bad("`config` is required (a connection object)"))?;
    let allowed_dirs = may_reach(s, &config)?;
    let driver = driver_for(&s.drivers, config.kind)?;
    let creds = given(params)?;
    let saved = saved_settings(s, &config);
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_job(LIST_DATABASES, async move {
        let creds = fill_from_store(creds, store, &config, saved).await;
        let result = list_on(driver.as_ref(), &config, allowed_dirs.as_deref(), &env, &creds).await;
        let apply: Apply = Box::new(move |s: &mut Session| {
            let (databases, unsaved) = result?;
            tell(s, unsaved);
            Ok(json!({ "databases": databases }))
        });
        apply
    });
    Ok(Outcome::Started(job))
}

/// `{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?, savePassword?,
/// createFile?}` → a job returning `{connection, server, tables, warnings}`: the tunnel, the
/// browse connection and the catalog. On success the workspace shows, the connection goes to the
/// top of the recent list (an unsaved config gets an id for it) and the file is written. One open
/// at a time (`Busy`). `createFile` as for `connection.test`.
///
/// `savePassword: true` asks to keep the secrets given in the params: the result then has
/// `secretsSaved`, and `secretsNote` when they were not kept. They are kept only for a saved
/// connection whose settings match the saved ones ([`reuse`]); that turns its `saveSecrets` on
/// (in the file) and stores them. For an unsaved config nothing is stored.
fn open(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    not_opening(s)?;
    let config = target_config(s, params)?;
    let create_file = create_file(params, &config)?;
    start_open(s, config, create_file, params)
}

/// `Busy` while a connection is being opened: one open at a time.
fn not_opening(s: &Session) -> Result<(), EngineError> {
    if s.jobs.is_running_label(OPEN) {
        return Err(EngineError::Busy("a connection is being opened".into()));
    }
    Ok(())
}

/// The open job of [`open`] for `config`, with the secrets and `savePassword` that `params` give.
fn start_open(s: &mut Session, mut config: ConnectionConfig, create_file: bool, params: &Json) -> Result<Outcome, EngineError> {
    let allowed_dirs = may_reach(s, &config)?;
    let driver = driver_for(&s.drivers, config.kind)?;
    let typed = given(params)?;
    let keep = p::bool_or(params, "savePassword", false)?;
    if config.id.is_empty() {
        config.id = new_id();
    }
    let saved = saved_settings(s, &config);
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_attempt(OPEN, move |stop| async move {
        let creds = unless_stopped(&stop, fill_from_store(typed.clone(), store, &config, saved)).await;
        let result = match &creds {
            Some(creds) => connect_and_load(driver.as_ref(), &config, allowed_dirs.as_deref(), create_file, &env, creds, &stop).await,
            None => Err(attempt_cancelled()),
        };
        let typed = keep.then_some(typed);
        let apply: Apply = Box::new(move |s: &mut Session| apply_open(s, config, creds.unwrap_or_default(), result, typed, &stop));
        apply
    });
    Ok(Outcome::Started(job))
}

/// A database type that opens a file: its label and its file extensions.
struct FileType<K> {
    kind: K,
    label: &'static str,
    extensions: &'static [&'static str],
}

/// The types of `kinds` that open a file, in `kinds` order.
fn file_types(kinds: &[DbKind]) -> Vec<FileType<DbKind>> {
    kinds
        .iter()
        .filter_map(|k| match k.info().settings {
            Settings::File { extensions } => Some(FileType { kind: *k, label: k.label(), extensions }),
            Settings::Server { .. } => None,
        })
        .collect()
}

/// The first of `types` whose extensions hold `path`'s, compared case-insensitively: on a shared
/// extension the one listed first wins, as the first driver to claim a type wins it.
fn first_with_extension<K: Copy>(types: &[FileType<K>], path: &Path) -> Option<K> {
    let ext = path.extension()?.to_str()?;
    types.iter().find(|t| t.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))).map(|t| t.kind)
}

/// `a`, `a and b`, `a, b and c`; `a`, `a or b`, `a, b or c` with `or`.
fn listed(items: &[String], and: &str) -> String {
    match items.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// Why `path` does not open: its name, every extension of `types` (each once), and where a file
/// with another name opens.
fn no_file_type<K>(path: &Path, types: &[FileType<K>]) -> String {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let mut exts: Vec<String> = Vec::new();
    for e in types.iter().flat_map(|t| t.extensions.iter()) {
        let e = format!(".{e}");
        if !exts.contains(&e) {
            exts.push(e);
        }
    }
    if exts.is_empty() {
        return format!("Can't open {name}: this build of dbee opens no database files.");
    }
    let labels: Vec<String> = types.iter().map(|t| t.label.to_string()).collect();
    format!(
        "Can't open {name}: dbee opens {} files. To open a file with another name, use New connection and choose {}.",
        listed(&exts, "and"),
        listed(&labels, "or")
    )
}

/// The params of `connection.open` that `connection.openFile` does not take: a file has no
/// tunnel, and an unsaved connection keeps no password.
const NOT_FOR_A_FILE: [&str; 4] = ["savePassword", "sshPassword", "sshPassphrase", "trustHostKey"];

/// The database type of the file at `path` among `kinds`: the first whose file extensions hold the
/// path's ([`first_with_extension`]). `Err` is the sentence that says why it does not open:
/// its name, the extensions dbee opens and where a file with another name opens.
fn file_kind(kinds: &[DbKind], path: &Path) -> Result<DbKind, String> {
    let types = file_types(kinds);
    first_with_extension(&types, path).ok_or_else(|| no_file_type(path, &types))
}

/// The saved connection whose file is the file at `file`, once both are resolved (links
/// followed, `.` and `..` gone).
fn saved_for_file<'a>(saved: &'a [ConnectionConfig], file: &Path) -> Option<&'a ConnectionConfig> {
    let resolved = std::fs::canonicalize(file).ok()?;
    saved.iter().find(|c| c.kind.uses_file() && c.file.as_deref().and_then(|f| std::fs::canonicalize(f).ok()).is_some_and(|f| f == resolved))
}

/// `{path, password?}` → a job as [`open`]'s: the file at `path` (a relative path is the current
/// folder's). When a saved connection's file is that file (both resolved), that connection opens,
/// with its name, colour and read-only setting. Else the file opens with the first of the
/// session's types whose file extensions hold the path's ([`file_kind`]), and the password the
/// params give: the config is not saved and has no name, so the file's name shows
/// ([`ConnectionConfig::display_name`]), and the summary says `saved: false` until
/// `connection.saveOpen`. It never makes a file. A path with none of those extensions is refused
/// (`badParams`), naming the ones dbee opens and where a file with another name opens, as are
/// `savePassword` and the SSH params.
fn open_file(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    not_opening(s)?;
    if let Some(name) = NOT_FOR_A_FILE.iter().find(|name| params.get(**name).is_some()) {
        return Err(bad(format!("`{name}` is not a param of connection.openFile, which takes {{path, password?}}")));
    }
    let config = file_config(s, Path::new(p::str(params, "path")?))?;
    start_open(s, config, false, params)
}

/// The settings `connection.openFile` opens `path` with, or its `badParams` refusal; also while
/// a connection is open, when it would be refused only for that (a file dropped on the window
/// then waits for the open one to close only when it can open).
pub fn file_config(s: &Session, path: &Path) -> Result<ConnectionConfig, EngineError> {
    if path.as_os_str().is_empty() {
        return Err(bad("`path` is empty"));
    }
    let file = std::path::absolute(path).map_err(|e| bad(format!("{}: {e}", path.display())))?;
    let config = match saved_for_file(&s.state.store.saved, &file) {
        Some(saved) => saved.clone(),
        None => {
            let kind = file_kind(&s.kinds(), path).map_err(bad)?;
            ConnectionConfig { kind, file: Some(file), host: String::new(), port: 0, ..ConnectionConfig::default() }
        }
    };
    config.validate().map_err(|e| bad(e.to_string()))?;
    Ok(config)
}

/// The active tab's connection is open and the saved list doesn't hold it.
fn unsaved_open(s: &Session) -> Result<(), String> {
    match s.state.connection(Which::Active) {
        None => Err(NOT_CONNECTED.into()),
        Some(c) if s.state.store.is_saved(&c.config.id) => Err("the open connection is saved already".into()),
        Some(_) => Ok(()),
    }
}

/// `{connection?}` → `{id, connection}`: saves the open connection, which the saved list doesn't
/// hold, as `connection.save` would with the secrets it was opened with: its settings as opened
/// (its id, no name when it had none), and with `saveSecrets` and a secret store its password and
/// SSH secrets.
fn save_open(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let connection = needed_connection(s, params)?;
    let c = s.state.connection(Which::Id(connection)).ok_or_else(|| disabled(NOT_CONNECTED))?;
    let (config, secrets) = (c.config.clone(), c.credentials.clone());
    let id = save_with_secrets(s, config, &secrets)?;
    Ok(Outcome::Done(json!({ "id": id, "connection": connection })))
}

fn apply_open(
    s: &mut Session,
    mut config: ConnectionConfig,
    credentials: Credentials,
    result: Result<(Opened, Catalog), EngineError>,
    keep: Option<Credentials>,
    stop: &Stop,
) -> Result<Json, EngineError> {
    let (opened, catalog) = unless_cancelled(s, stop, result)?;
    if s.state.connections.len() >= MAX_OPEN {
        // Open is disabled then and runs one at a time, so this cannot happen; the new connection
        // closes as it is dropped.
        return Err(EngineError::Busy("already connected".into()));
    }
    let kept = keep.map(|typed| keep_secrets(s, &mut config, &typed));
    let Opened { conn, tunnel } = opened;
    let (info, dialect) = (conn.info().clone(), conn.dialect());
    let used = config.clone();
    let c = Connected {
        id: s.state.take_connection_id(),
        config,
        info,
        dialect,
        catalog: Arc::new(catalog),
        lost: false,
        browse: InTurn::new(conn),
        query: None,
        query_database: CurrentDatabase::default(),
        query_run: None,
        transaction: None,
        tunnel,
        credentials,
        apply_gate: None,
    };
    let mut result = opened_json(&c, s.state.store.is_saved(&c.config.id));
    if let (Json::Object(map), Some(kept)) = (&mut result, kept) {
        map.insert("secretsSaved".into(), json!(kept.is_ok()));
        if let Err(note) = kept {
            map.insert("secretsNote".into(), json!(note));
        }
    }
    for w in &c.info.warnings {
        s.ui_requests.push(UiRequest::Notice(w.clone()));
    }
    tell(s, unsaved_host_key(c.tunnel.as_ref()));
    // Query tabs opened while no connection was open run on this one.
    for tab in &mut s.state.tabs {
        if let Tab::Query(q) = tab
            && q.connection.is_none()
        {
            q.connection = Some(c.id);
        }
    }
    s.state.connections.push(c);
    s.state.screen = Screen::Workspace;
    record_use(s, &used);
    Ok(result)
}

/// `savePassword` after a successful open: keeps `typed` for the saved connection `config.id`
/// when its settings match the saved ones, turning the saved `saveSecrets` on first (and
/// `config`'s, for the recent list and the session). It is stored with the settings the file held
/// at that check. `Err` says why nothing was kept.
fn keep_secrets(s: &mut Session, config: &mut ConnectionConfig, typed: &Credentials) -> Result<(), String> {
    let Some(secret_store) = s.secrets.clone() else { return Err("there is no secret store, so dbee asks each time".into()) };
    if !s.state.store.is_saved(&config.id) {
        return Err("the connection is not saved; save it to keep its secrets".into());
    }
    // Checked against the settings as the file holds them now, which another dbee may have changed.
    let given = config.clone();
    let kept = change_store(s, |store| {
        let Some(saved) = store.saved.iter_mut().find(|c| c.id == given.id) else {
            return Err("the connection is not saved any more; save it to keep its secrets");
        };
        let saved_for = SavedFor::of(saved);
        let r = reuse(&saved_for, &given);
        if (typed.password.is_some() && !r.db)
            || (typed.ssh.password.is_some() && !r.ssh_password)
            || (typed.ssh.passphrase.is_some() && !r.passphrase)
        {
            return Err("the settings differ from the saved connection; save them to keep the secrets");
        }
        saved.save_secrets = true;
        Ok(saved_for)
    })
    .map_err(|e| match e {
        ConfigError::Locked => "another dbee window was changing the connections file".to_owned(),
        e => format!("the connections file could not be written: {e}"),
    })?;
    let saved_for = kept?;
    config.save_secrets = true;
    if !store_secrets(secret_store.as_ref(), &config.id, typed, &saved_for) {
        secret_store_failed(s, "save the password");
        return Err("the secret store refused them".into());
    }
    Ok(())
}

/// `{connection?, password?, sshPassword?, sshPassphrase?, trustHostKey?}` → a job returning
/// `{connection, server, tables, warnings, querySessionReset?}` (`querySessionReset: true` when a
/// query connection was open, so its transaction and SETs are dropped): a new tunnel, browse
/// connection and catalog with the secrets kept from the open (the params answer a prompt). On success the old connections are let go (a
/// statement still running on one is cancelled, and a query run stops), the query connection is
/// opened again on the next run, and `lost` clears; the connection keeps its id, and its tabs and
/// their contents stay, but table tabs forget the loads and counts that ran on the old connection
/// (`TableTab::drop_dead_jobs`) and a query tab its run.
fn reconnect(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    if s.jobs.is_running_label(RECONNECT) {
        return Err(EngineError::Busy("a reconnect is in progress".into()));
    }
    let answers = given(params)?;
    let id = needed_connection(s, params)?;
    let c = s.state.connection(Which::Id(id)).ok_or_else(|| disabled(NOT_CONNECTED))?;
    let mut creds = c.credentials.clone();
    creds.password = answers.password.or(creds.password);
    creds.ssh.password = answers.ssh.password.or(creds.ssh.password);
    creds.ssh.passphrase = answers.ssh.passphrase.or(creds.ssh.passphrase);
    creds.trust = answers.trust.or(creds.trust);
    let config = c.config.clone();
    let allowed_dirs = may_reach(s, &config)?;
    let old = Arc::clone(&c.browse);
    let driver = driver_for(&s.drivers, config.kind)?;
    let saved = saved_settings(s, &config);
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_attempt(RECONNECT, move |stop| async move {
        let filled = unless_stopped(&stop, fill_from_store(creds, store, &config, saved)).await;
        let result = match &filled {
            Some(creds) => connect_and_load(driver.as_ref(), &config, allowed_dirs.as_deref(), false, &env, creds, &stop).await,
            None => Err(attempt_cancelled()),
        };
        let apply: Apply = Box::new(move |s: &mut Session| apply_reconnect(s, id, &old, filled.unwrap_or_default(), result, &stop));
        apply
    });
    Ok(Outcome::Started(job))
}

fn apply_reconnect(
    s: &mut Session,
    id: ConnectionId,
    old: &Arc<InTurn>,
    credentials: Credentials,
    result: Result<(Opened, Catalog), EngineError>,
    stop: &Stop,
) -> Result<Json, EngineError> {
    let saved = s.state.connection(Which::Id(id)).is_some_and(|c| s.state.store.is_saved(&c.config.id));
    if !s.state.connection(Which::Id(id)).is_some_and(|c| Arc::ptr_eq(&c.browse, old)) {
        // Closed or reconnected meanwhile: ignored, whatever the outcome (a new connection closes
        // as it is dropped, and a prompt is answered by nobody).
        return Ok(ignored());
    }
    let (opened, catalog) = unless_cancelled(s, stop, result)?;
    let Some(c) = s.state.connection_mut(Which::Id(id)) else { return Ok(ignored()) };
    let Opened { conn, tunnel } = opened;
    let unsaved = unsaved_host_key(tunnel.as_ref());
    c.info = conn.info().clone();
    c.dialect = conn.dialect();
    c.catalog = Arc::new(catalog);
    let old_browse = std::mem::replace(&mut c.browse, InTurn::new(conn));
    let query_reset = c.query.take().is_some();
    c.query_run = None;
    c.transaction = None;
    c.tunnel = tunnel;
    c.credentials = credentials;
    c.lost = false;
    let warnings = c.info.warnings.clone();
    let mut result = opened_json(c, saved);
    if query_reset && let Some(o) = result.as_object_mut() {
        // The query session (an open transaction, SETs, temporary tables) went with its connection.
        o.insert("querySessionReset".into(), Json::Bool(true));
    }
    // Every job of its tabs started before the swap ran on the old connections: it is dead.
    for tab in s.state.tabs.iter_mut().filter(|t| t.connection() == Some(id)) {
        match tab {
            Tab::Table(t) => t.drop_dead_jobs(),
            Tab::Query(q) => q.running = None,
        }
    }
    stop_statements(s, old_browse);
    for w in warnings {
        s.ui_requests.push(UiRequest::Notice(w));
    }
    tell(s, unsaved);
    Ok(result)
}

/// Stops what still runs on connections that are let go, `browse` and the query connection: the
/// jobs that stop themselves (a query run, page loads and counts) through their `Stop`, and for
/// the others (an Apply, a catalog load) a cancel request on `browse`.
fn stop_statements(s: &Session, browse: Arc<dyn Connection>) {
    s.jobs.stop_all();
    if s.jobs.unstoppable_running() {
        cancel_statements(s, std::iter::once(browse));
    }
}

/// Why the host key the user trusted was not saved to `known_hosts`, when that failed.
fn unsaved_host_key(tunnel: Option<&Arc<Tunnel>>) -> Option<String> {
    tunnel.and_then(|t| t.unsaved_host_key()).map(str::to_owned)
}

/// Shows `note` as a notice, when there is one.
fn tell(s: &mut Session, note: Option<String>) {
    if let Some(note) = note {
        s.ui_requests.push(UiRequest::Notice(note));
    }
}

/// `{}` → `{stopped: true}`: stops every connection attempt that runs (open, openFile, reconnect,
/// test), which then ends [`ATTEMPT_CANCELLED`] with nothing it opened left open (see the module
/// docs). Asked again before such an attempt has ended: `{stopped: false, stopping: true}`. With
/// none running it does nothing: `{stopped: false, note}`, no error.
fn cancel(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    Ok(Outcome::Done(match s.jobs.stop_attempts() {
        (0, 0) => json!({ "stopped": false, "note": NO_ATTEMPT }),
        (0, _) => json!({ "stopped": false, "stopping": true }),
        _ => json!({ "stopped": true }),
    }))
}

/// `{connection?}` → a job returning `{tables, connection}`: reloads the catalog on the browse
/// connection. Tabs keep their tables. A load past [`CATALOG_TIMEOUT`], counted from its turn on
/// the connection, is cancelled and the connection is discarded: the session is marked lost, and
/// the error (code `timeout`) names the reconnect in `data.action`.
fn refresh(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let (id, turned) = s.browse_in_turn(Which::Id(needed_connection(s, params)?))?;
    let job = s.spawn_job(REFRESH, async move {
        let result = load_catalog_in_turn(&turned, CATALOG_TIMEOUT).await;
        let conn: Arc<dyn Connection> = turned;
        let apply: Apply = Box::new(move |s: &mut Session| {
            if !s.is_current_browse(id, &conn) {
                return Ok(ignored());
            }
            let catalog = match result {
                Ok(catalog) => catalog,
                Err(CatalogLoad::Failed(e)) => return Err(s.db_failed(id, &conn, &e)),
                Err(CatalogLoad::TimedOut) => {
                    // The connection is discarded: it is lost until a reconnect.
                    if let Some(c) = s.state.connection_mut(Which::Id(id)) {
                        c.lost = true;
                    }
                    return Err(lost_by_timeout());
                }
            };
            let tables = catalog.table_count();
            if let Some(c) = s.state.connection_mut(Which::Id(id)) {
                c.catalog = Arc::new(catalog);
            }
            Ok(json!({ "tables": tables, "connection": id }))
        });
        apply
    });
    Ok(Outcome::Started(job))
}

/// `{connection?, force?, end?}` → `{closed, connection}` (`closed` being the saved connection's
/// id): back to the connection screen. Unless `force` or `end`, it asks first:
/// - when its query session has a transaction, open, failed or unknown: `Needs::EndTransaction`
///   ([`query::end_question`]), which comes before the question below and carries its sentence in
///   `lost`;
/// - when its query tabs have text or its table tabs staged edits: `Needs::Confirm`, `what` being
///   `queryText`, or `stagedEdits` when only edits would be lost.
///
/// With `end` (`"commit"` or `"rollback"`) it is a job that ends the transaction first, and closes
/// unless that fails ([`query::end_then_close`]). Its tabs close, which is every tab; statements
/// still running are cancelled through their connections' cancellers (an Apply then rolls back,
/// and a query run stops), and the connections are let go (they close once the jobs using them
/// end; those jobs' results are dropped). The saved and recent lists are read again from the file,
/// which another dbee may have changed meanwhile.
fn close(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let force = p::bool_or(params, "force", false)?;
    let end = End::param(params)?;
    let id = needed_connection(s, params)?;
    let c = s.state.connection(Which::Id(id)).ok_or_else(|| disabled(NOT_CONNECTED))?;
    if let Some(end) = end {
        return query::end_then_close(s, Some(id), end, CLOSE, move |s| close_now(s, id));
    }
    if !force {
        let name = c.config.display_name();
        let question = format!("Disconnect from {name}?");
        let lost = lost(s, id);
        if let Some(t) = c.transaction() {
            return Err(query::end_question(s, id, t.state, EndAbout::Connection, name, &question, lost.map(|(_, l)| l)));
        }
        if let Some((what, lost)) = lost {
            return Err(EngineError::Needs { need: Need::Confirm { what: what.into() }, message: format!("{question} {lost}") });
        }
    }
    close_now(s, id).map(Outcome::Done).ok_or_else(|| disabled(NOT_CONNECTED))
}

/// What closing connection `id` loses: the `what` of its `Needs::Confirm`, and the sentence its
/// question says it in after asking ("The text of 1 query tab will be lost.").
fn lost(s: &Session, id: ConnectionId) -> Option<(&'static str, String)> {
    let its = || s.state.tabs.iter().filter(|t| t.connection() == Some(id));
    let with_text = its().filter(|t| matches!(t, Tab::Query(q) if !q.text.trim().is_empty())).count();
    let with_edits = its().filter(|t| matches!(t, Tab::Table(t) if t.has_staged_edits())).count();
    if with_text == 0 && with_edits == 0 {
        return None;
    }
    let tabs = |n: usize, what: &str| if n == 1 { format!("1 {what} tab") } else { format!("{n} {what} tabs") };
    let mut lost = Vec::new();
    if with_text > 0 {
        lost.push(format!("text of {}", tabs(with_text, "query")));
    }
    if with_edits > 0 {
        lost.push(format!("staged changes of {}", tabs(with_edits, "table")));
    }
    let what = if with_text > 0 { "queryText" } else { "stagedEdits" };
    Some((what, format!("The {} will be lost.", lost.join(" and the "))))
}

/// Closes connection `id` when it is still open (see [`close`]): `{closed, connection}`.
fn close_now(s: &mut Session, id: ConnectionId) -> Option<Json> {
    let at = s.state.connections.iter().position(|c| c.id == id)?;
    let c = s.state.connections.remove(at);
    if s.state.connections.is_empty() {
        s.state.screen = Screen::Connections;
    }
    if let Some(warning) = s.state.store.reload() {
        log::warn!("the connections file could not be used as it is");
        s.state.store_warning = Some(warning);
    }
    s.state.tabs.retain(|t| t.connection() != Some(id));
    if s.state.active.is_none_or(|a| s.state.tab(a).is_none()) {
        s.state.active = s.state.tabs.last().map(Tab::id);
    }
    stop_statements(s, c.browse);
    Some(json!({ "closed": c.config.id, "connection": id }))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use dbee_config::SshConfig;

    use super::*;

    /// [`super::reuse`] for a secret saved for the settings of `saved`.
    fn reuse(saved: &ConnectionConfig, given: &ConnectionConfig) -> Reuse {
        super::reuse(&SavedFor::of(saved), given)
    }

    fn saved() -> ConnectionConfig {
        ConnectionConfig {
            id: "a".into(),
            host: "db.example".into(),
            port: 5432,
            user: "ann".into(),
            database: "shop".into(),
            ssh: Some(SshConfig {
                host: "jump".into(),
                port: 22,
                user: "ann".into(),
                auth: SshAuth::Key { path: Some(PathBuf::from("/k/id")) },
                ..SshConfig::default()
            }),
            ..ConnectionConfig::default()
        }
    }

    fn with_ssh(f: impl FnOnce(&mut SshConfig)) -> ConnectionConfig {
        let mut c = saved();
        if let Some(ssh) = c.ssh.as_mut() {
            f(ssh);
        }
        c
    }

    #[test]
    fn stored_secrets_are_reused_only_for_the_same_server() {
        let all = Reuse { db: true, ssh_password: true, passphrase: true };
        let nothing = Reuse::default();
        assert_eq!(reuse(&saved(), &saved()), all);
        // The database name and the display settings do not matter.
        assert_eq!(reuse(&saved(), &ConnectionConfig { database: "other".into(), name: "x".into(), ..saved() }), all);
        // Another database host, port or user: no database password; the tunnel is the same.
        let ssh_only = Reuse { db: false, ssh_password: true, passphrase: true };
        assert_eq!(reuse(&saved(), &ConnectionConfig { host: "evil.example".into(), ..saved() }), ssh_only);
        assert_eq!(reuse(&saved(), &ConnectionConfig { port: 1, ..saved() }), ssh_only);
        assert_eq!(reuse(&saved(), &ConnectionConfig { user: "bob".into(), ..saved() }), ssh_only);
        // Another SSH host or port is another tunnel, so another machine behind the same DB host
        // name: no secret at all.
        assert_eq!(reuse(&saved(), &with_ssh(|s| s.host = "evil".into())), nothing);
        assert_eq!(reuse(&saved(), &with_ssh(|s| s.port = 2222)), nothing);
        // Another SSH user: the same tunnel endpoint, so the database password still applies.
        let db_only = Reuse { db: true, ssh_password: false, passphrase: false };
        assert_eq!(reuse(&saved(), &with_ssh(|s| s.user = "bob".into())), db_only);
        // Another key file (or another login method): no passphrase.
        let no_passphrase = Reuse { db: true, ssh_password: true, passphrase: false };
        assert_eq!(reuse(&saved(), &with_ssh(|s| s.auth = SshAuth::Key { path: Some("/k/other".into()) })), no_passphrase);
        assert_eq!(reuse(&saved(), &with_ssh(|s| s.auth = SshAuth::Key { path: None })), no_passphrase);
        assert_eq!(reuse(&saved(), &with_ssh(|s| s.auth = SshAuth::Password)), no_passphrase);
        // A tunnel on one side only: another route, so nothing.
        assert_eq!(reuse(&saved(), &ConnectionConfig { ssh: None, ..saved() }), nothing);
        assert_eq!(reuse(&ConnectionConfig { ssh: None, ..saved() }, &saved()), nothing);
        // No tunnel on either side: the database password by its own rule.
        let direct = ConnectionConfig { ssh: None, ..saved() };
        assert_eq!(reuse(&direct, &direct), Reuse { db: true, ssh_password: false, passphrase: false });
    }

    #[test]
    fn stored_password_needs_the_same_ca_file_to_verify_with() {
        use dbee_config::SslMode::{Prefer, Require, VerifyCa, VerifyFull};
        let with = |mode, ca: Option<&str>| ConnectionConfig { ssl_mode: mode, ca_file: ca.map(PathBuf::from), ..saved() };
        for mode in [VerifyCa, VerifyFull] {
            let was = with(mode, Some("/ca/ours.pem"));
            assert!(reuse(&was, &with(mode, Some("/ca/ours.pem"))).db, "the same CA file: {mode:?}");
            for other in [Some("/ca/theirs.pem"), None] {
                let r = reuse(&was, &with(mode, other));
                assert!(!r.db, "another CA file ({other:?}) under {mode:?}");
                assert!(r.ssh_password && r.passphrase, "the CA file does not concern the tunnel");
            }
            // Verifying now where the saved settings did not: by the CA file the saved ones had.
            assert!(reuse(&with(Require, Some("/ca/ours.pem")), &with(mode, Some("/ca/ours.pem"))).db, "{mode:?}");
            assert!(!reuse(&with(Require, None), &with(mode, Some("/ca/theirs.pem"))).db, "{mode:?}");
        }
        // `require` with a CA file checks the certificate as `verify-ca` does: the same CA file, or
        // nothing. Without one it checks nothing, so a CA file there is a change too.
        let ours = with(Require, Some("/ca/ours.pem"));
        for other in [Some("/ca/theirs.pem"), None] {
            assert!(!reuse(&ours, &with(Require, other)).db, "another CA file ({other:?}) under require");
        }
        assert!(reuse(&ours, &ours).db);
        assert!(
            reuse(&ours, &with(VerifyCa, Some("/ca/ours.pem"))).db && reuse(&with(VerifyCa, Some("/ca/ours.pem")), &ours).db,
            "they check the same"
        );
        assert!(!reuse(&ours, &with(Prefer, Some("/ca/ours.pem"))).db, "prefer reads no CA file");
        assert!(!reuse(&with(Require, None), &with(Require, Some("/ca/theirs.pem"))).db);
        // `prefer` reads no CA file, so it does not count.
        assert!(reuse(&with(Prefer, Some("/ca/ours.pem")), &with(Prefer, Some("/ca/theirs.pem"))).db);
        assert!(reuse(&with(Prefer, None), &with(Prefer, Some("/ca/theirs.pem"))).db);
    }

    #[test]
    fn stored_password_needs_the_same_database_type() {
        let on = |kind| ConnectionConfig { kind, ..saved() };
        for kind in [DbKind::MySql, DbKind::MariaDb] {
            let r = reuse(&on(DbKind::Postgres), &on(kind));
            assert!(!r.db, "saved for Postgres, given {kind:?}");
            assert!(r.ssh_password && r.passphrase, "the database type does not concern the tunnel");
        }
        assert!(!reuse(&on(DbKind::MySql), &on(DbKind::MariaDb)).db);
        assert!(reuse(&on(DbKind::MySql), &on(DbKind::MySql)).db);
    }

    #[test]
    fn stored_password_is_not_used_with_an_option_that_protects_it_less() {
        let mysql = |on: Option<&str>| {
            let mut c = ConnectionConfig { kind: DbKind::MySql, port: 3306, ..saved() };
            if let Some(v) = on {
                c.options.insert(dbee_config::ALLOW_PUBLIC_KEY.id.into(), v.into());
            }
            c
        };
        const { assert!(dbee_config::ALLOW_PUBLIC_KEY.weakens) };
        let off = mysql(None);
        let on = mysql(Some("true"));
        let r = reuse(&off, &on);
        assert!(!r.db, "allowPublicKey turned on where the saved settings had it off");
        assert!(r.ssh_password && r.passphrase, "the option does not concern the tunnel");
        // Saved with it on, or off on both sides, or turned off: the password is used.
        assert!(reuse(&on, &on).db && reuse(&off, &off).db && reuse(&on, &off).db);
        assert!(reuse(&off, &mysql(Some("false"))).db, "only \"true\" is on");
        // An option the type does not have does not count.
        let mut stray = saved();
        stray.options.insert(dbee_config::ALLOW_PUBLIC_KEY.id.into(), "true".into());
        assert!(reuse(&saved(), &stray).db, "Postgres has no allowPublicKey");
    }

    #[test]
    fn stored_password_needs_tls_at_least_as_strong() {
        use dbee_config::SslMode::{Disable, Prefer, Require, VerifyCa, VerifyFull};
        let order = [Disable, Prefer, Require, VerifyCa, VerifyFull];
        for (i, &was) in order.iter().enumerate() {
            for (j, &now) in order.iter().enumerate() {
                let r = reuse(&ConnectionConfig { ssl_mode: was, ..saved() }, &ConnectionConfig { ssl_mode: now, ..saved() });
                assert_eq!(r.db, j >= i, "saved {was:?}, given {now:?}");
                assert!(r.ssh_password && r.passphrase, "TLS does not concern the tunnel: saved {was:?}, given {now:?}");
            }
        }
    }

    /// The tunnel's secrets go by what the store says as the database password does; the integration
    /// tests can't reach this without an SSH server.
    #[test]
    fn stored_ssh_secrets_go_by_the_settings_kept_with_them() {
        let store = dbee_config::MemoryStore::default();
        let elsewhere = with_ssh(|s| s.host = "evil".into());
        let other_user = with_ssh(|s| s.user = "bob".into());
        let other_key = with_ssh(|s| s.auth = SshAuth::Key { path: Some("/k/other".into()) });
        let saved_for = SavedFor::of(&saved());
        // The file's settings, here the session's copy, don't decide.
        let file_says = SavedFor::of(&elsewhere);
        store_secret(&store, "a", SecretKind::SshPassword, "ssh", &saved_for);
        store_secret(&store, "a", SecretKind::SshPassphrase, "phrase", &saved_for);
        let read = |given: &ConnectionConfig, kind| read_secret(&store, given, &file_says, kind);
        assert_eq!(read(&saved(), SecretKind::SshPassword).as_deref(), Some("ssh"));
        assert_eq!(read(&saved(), SecretKind::SshPassphrase).as_deref(), Some("phrase"));
        assert_eq!(read(&elsewhere, SecretKind::SshPassword), None);
        assert_eq!(read(&elsewhere, SecretKind::SshPassphrase), None);
        assert_eq!(read(&other_user, SecretKind::SshPassword), None);
        assert_eq!(read(&other_user, SecretKind::SshPassphrase), None);
        assert_eq!(read(&other_key, SecretKind::SshPassword).as_deref(), Some("ssh"));
        assert_eq!(read(&other_key, SecretKind::SshPassphrase), None);
    }

    #[test]
    fn an_older_builds_ssh_secrets_are_stored_again_with_their_settings() {
        let saved_for = SavedFor::of(&saved());
        for kind in [SecretKind::SshPassword, SecretKind::SshPassphrase] {
            let store = dbee_config::MemoryStore::default();
            store.set("a", kind, "older").unwrap();
            assert_eq!(read_secret(&store, &with_ssh(|s| s.host = "evil".into()), &saved_for, kind), None, "{kind:?}");
            assert_eq!(store.get("a", kind).unwrap().as_deref(), Some("older"), "{kind:?}: not used, so as it was");
            assert_eq!(read_secret(&store, &saved(), &saved_for, kind).as_deref(), Some("older"), "{kind:?}");
            let text = store.get("a", kind).unwrap().unwrap();
            assert_eq!(StoredSecret::parse(&text), StoredSecret::For { secret: "older".into(), saved_for: Box::new(saved_for.clone()) }, "{kind:?}");
        }
    }

    /// A save goes by what each secret says, and moves it to the new settings.
    #[test]
    fn a_save_carries_each_secret_by_the_settings_kept_with_it() {
        let store = dbee_config::MemoryStore::default();
        let was = SavedFor::of(&saved());
        store_secret(&store, "a", SecretKind::DbPassword, "db", &was);
        store_secret(&store, "a", SecretKind::SshPassword, "ssh", &was);
        store_secret(&store, "a", SecretKind::SshPassphrase, "phrase", &was);
        let nothing = Credentials::default();
        let stored = |kind| store.get("a", kind).unwrap().map(|text| StoredSecret::parse(&text));

        // Another SSH user keeps the database password, now saved for the new settings.
        let now_config = with_ssh(|s| s.user = "bob".into());
        let now = SavedFor::of(&now_config);
        assert!(carry_secrets(&store, &now_config, &now, Some(&was), &nothing));
        assert_eq!(stored(SecretKind::DbPassword), Some(StoredSecret::For { secret: "db".into(), saved_for: Box::new(now.clone()) }));
        assert_eq!((stored(SecretKind::SshPassword), stored(SecretKind::SshPassphrase)), (None, None));

        // What the store says wins over the file: the password was saved for another server, whatever
        // the settings saved before say.
        let elsewhere = SavedFor::of(&ConnectionConfig { host: "elsewhere".into(), ..saved() });
        store_secret(&store, "a", SecretKind::DbPassword, "db", &elsewhere);
        let renamed = ConnectionConfig { port: 5433, ..saved() };
        assert!(carry_secrets(&store, &renamed, &SavedFor::of(&renamed), Some(&was), &nothing));
        assert_eq!(stored(SecretKind::DbPassword), None);

        // An older build's goes by the settings saved before, and comes out with the new ones.
        store.set("a", SecretKind::DbPassword, "older").unwrap();
        let stronger = ConnectionConfig { ssl_mode: SslMode::Require, ..saved() };
        let now = SavedFor::of(&stronger);
        assert!(carry_secrets(&store, &stronger, &now, Some(&was), &nothing));
        assert_eq!(stored(SecretKind::DbPassword), Some(StoredSecret::For { secret: "older".into(), saved_for: Box::new(now.clone()) }));
        store.set("a", SecretKind::DbPassword, "older").unwrap();
        assert!(carry_secrets(&store, &stronger, &now, None, &nothing), "no settings saved before");
        assert_eq!(stored(SecretKind::DbPassword), None);

        // A secret given again is left to the write that follows.
        store.set("a", SecretKind::DbPassword, "older").unwrap();
        let given = Credentials { password: Some("new".into()), ..Credentials::default() };
        assert!(carry_secrets(&store, &renamed, &SavedFor::of(&renamed), Some(&was), &given));
        assert_eq!(stored(SecretKind::DbPassword), Some(StoredSecret::Bare("older".into())));
    }

    fn file_type<K>(kind: K, label: &'static str, extensions: &'static [&'static str]) -> FileType<K> {
        FileType { kind, label, extensions }
    }

    #[test]
    fn the_first_kind_with_the_extension_wins() {
        let kinds = [file_type("first", "First", &["db", "one"]), file_type("second", "Second", &["db", "two"])];
        let kind = |path: &str| first_with_extension(&kinds, Path::new(path));
        assert_eq!(kind("/x/a.db"), Some("first"), "a shared extension is the first kind's");
        assert_eq!(kind("a.two"), Some("second"));
        // Any letter case; only the last extension counts.
        assert_eq!((kind("A.DB"), kind("a.Two"), kind("a.two.db")), (Some("first"), Some("second"), Some("first")));
        // No extension, or another one.
        for path in ["a.txt", "db", ".db", "a.db.txt", "a.", ""] {
            assert_eq!(kind(path), None, "{path:?}");
        }
    }

    #[test]
    fn the_refusal_lists_the_extensions_and_where_other_files_open() {
        let notes = Path::new("/home/ann/notes.txt");
        let hint = |labels: &str| format!(" To open a file with another name, use New connection and choose {labels}.");
        assert_eq!(no_file_type(notes, &[file_type(0, "A", &["a"])]), format!("Can't open notes.txt: dbee opens .a files.{}", hint("A")));
        let two = [file_type(0, "A", &["a"]), file_type(1, "B", &["b", "a"])];
        assert_eq!(no_file_type(notes, &two), format!("Can't open notes.txt: dbee opens .a and .b files.{}", hint("A or B")), "each once");
        let three = [file_type(0, "A", &["a", "b", "c"]), file_type(1, "B", &[]), file_type(2, "C", &[])];
        assert_eq!(no_file_type(notes, &three), format!("Can't open notes.txt: dbee opens .a, .b and .c files.{}", hint("A, B or C")));
        assert_eq!(no_file_type(notes, &[] as &[FileType<u8>]), "Can't open notes.txt: this build of dbee opens no database files.");
        // The types dbee knows, from their own data: every extension once, in order, and every label.
        let types = file_types(&DbKind::ALL);
        assert!(!types.is_empty() && types.iter().all(|t| t.kind.uses_file()));
        let text = no_file_type(notes, &types);
        let named: Vec<&str> = text.split_whitespace().filter(|w| w.starts_with('.')).map(|w| w.trim_end_matches(',')).collect();
        let mut want: Vec<String> = Vec::new();
        for e in types.iter().flat_map(|t| t.extensions.iter()).map(|e| format!(".{e}")) {
            if !want.contains(&e) {
                want.push(e);
            }
        }
        assert_eq!(named, want, "{text}");
        assert!(types.iter().all(|t| text.contains(t.label)), "{text}");
    }

    #[test]
    fn a_refresh_timeout_offers_the_reconnect() {
        let e = lost_by_timeout();
        assert_eq!(e.code(), "timeout");
        assert_eq!(e.data(), json!({ "action": "connection.reconnect" }));
        assert!(e.to_string().contains("60 s"), "{e}");
    }

    /// A refresh behind another tab's statement waits for its turn; its limit counts from then.
    #[tokio::test]
    async fn a_catalog_load_counts_its_time_from_its_turn() {
        let quick = Arc::new(crate::turns::tests::Quick::default());
        let conn = InTurn::new(Arc::clone(&quick) as Arc<dyn Connection>);
        let held = conn.turn().await;
        let loading = tokio::spawn({
            let conn = Arc::clone(&conn);
            async move { load_catalog_in_turn(&conn, Duration::from_millis(50)).await.is_ok() }
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        while conn.waiting() == 0 {
            assert!(Instant::now() < deadline, "the load never waited for its turn");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        // Longer than its limit, behind another operation: no reason to give up.
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(held);
        assert!(loading.await.unwrap(), "the load timed out while it waited for its turn");
        assert_eq!(quick.cancels(), 0, "no cancel request went out");
    }

    #[tokio::test]
    async fn catalog_load_times_out() {
        let db = dbee_testkit::require_testdb!();
        let took = catalog_load_timing_out(db).await;
        println!("a catalog load that timed out ended {took:?} after it started");
    }

    #[tokio::test]
    #[ignore = "timing; run with --ignored --nocapture"]
    async fn catalog_load_is_bounded() {
        let db = dbee_testkit::require_testdb!();
        let took = catalog_load_timing_out(db).await;
        println!("a catalog load that timed out ended {took:?} after it started");
        assert!(took < CANCEL_GRACE + Duration::from_secs(5), "{took:?}");
    }

    /// A catalog load with no time at all times out, is cancelled and gives the `timeout` error; a
    /// fresh connection loads within the real bound. How long the timed-out load took.
    async fn catalog_load_timing_out(db: &dbee_testkit::TestDb) -> Duration {
        let server = db.server("pg14").unwrap();
        let config = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: server.port,
            user: "postgres".into(),
            database: "dbee".into(),
            ..ConnectionConfig::default()
        };
        let driver = driver_for(&dbee_drivers::all(), config.kind).unwrap();
        let target = || Target {
            config: &config,
            password: Some(&db.password),
            tunnel: None,
            purpose: Purpose::Browse,
            create_file: false,
            allowed_dirs: None,
        };
        let conn = driver.connect(target()).await.unwrap();
        // No load can answer within no time at all: the first poll only sends the query.
        let started = Instant::now();
        assert!(matches!(load_catalog_within(conn.as_ref(), Duration::ZERO).await, Err(CatalogLoad::TimedOut)));
        let took = started.elapsed();
        let fresh = driver.connect(target()).await.unwrap();
        assert!(load_catalog_within(fresh.as_ref(), CATALOG_TIMEOUT).await.is_ok_and(|c| c.table_count() > 10));
        assert_eq!(catalog_error(CatalogLoad::TimedOut).code(), "timeout");
        took
    }
}
