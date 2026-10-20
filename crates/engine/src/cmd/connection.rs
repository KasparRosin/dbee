//! Connection commands: the saved and recent list (save, delete, duplicate), URL parsing, the
//! SSH key preview, test, open, reconnect, close, and the schema refresh.
//!
//! Secrets (the database password, the SSH password and the key passphrase) come from the
//! params first (`password`, `sshPassword`, `sshPassphrase`; an empty string counts as not
//! given), then the secret store under the connection's id, then nowhere. They are kept only in
//! the store and in the crate-private [`Connected::credentials`] (for reconnects), never in the
//! config, the connections file, `inspect()`, `Debug`, logs, errors or the journal (the journal
//! drops secret params by key, and `connection.parseUrl`, whose URL may carry a password, is
//! not journaled). Store reads and the writes of `savePassword` run in the job, so a slow
//! keyring never holds up the UI thread.
//!
//! Connecting is a job: the tunnel, the browse connection and, for open and reconnect, the
//! catalog. The prompts come back as [`EngineError::Needs`]: an unknown host key, a key
//! passphrase, the SSH password, or the database password when none was given; the caller asks
//! the user and runs the command again with the answer in its params.
//!
//! A lost connection: when a browse operation fails with `connectionLost` (or the connection
//! has closed), the session is marked [`Connected::lost`] and the commands that need the
//! database are disabled with [`LOST`] until `connection.reconnect` replaces the connections.
//! Database work is stopped only through `Canceller`s, never by dropping a running future; a
//! job whose connection was closed or replaced meanwhile has its result dropped.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dbee_config::{ConnectionConfig, SecretKind, SecretStore, SshAuth, new_id, parse_url};
use dbee_core::Catalog;
use dbee_pg::catalog::load_catalog;
use dbee_pg::{Canceller, PgConnection, PgError, ServerInfo, Target};
use dbee_ssh::{KeyPlan, SshEnv, SshError, SshSecrets, Tunnel};
use serde::Deserialize as _;
use serde_json::json;

use super::{CommandSpec, LOST, NOT_CONNECTED, always, connected, needs_db, not_connected, p};
use crate::state::{Connected, Credentials};
use crate::{Apply, EngineError, Json, Need, Outcome, Screen, Session, Tab, UiRequest};

const LIST: &str = "connection.list";
const SAVE: &str = "connection.save";
const DELETE: &str = "connection.delete";
const DUPLICATE: &str = "connection.duplicate";
const PARSE_URL: &str = "connection.parseUrl";
const SSH_KEY: &str = "connection.sshKey";
const TEST: &str = "connection.test";
const OPEN: &str = "connection.open";
const RECONNECT: &str = "connection.reconnect";
const REFRESH: &str = "schema.refresh";
const CLOSE: &str = "connection.close";

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
        params: "{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?}",
        enabled: always,
        run: test,
        journal: false,
        undoable: false,
    },
    CommandSpec {
        id: OPEN,
        label: "Connect",
        menu: &["File"],
        shortcut: None,
        params: "{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?, savePassword?}",
        enabled: not_connected,
        run: open,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: RECONNECT,
        label: "Reconnect",
        menu: &[],
        shortcut: None,
        params: "{password?, sshPassword?, sshPassphrase?, trustHostKey?}",
        enabled: connected,
        run: reconnect,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: REFRESH,
        label: "Refresh Schema",
        menu: &[],
        shortcut: None,
        params: "{}",
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
        params: "{force?}",
        enabled: connected,
        run: close,
        journal: true,
        undoable: false,
    },
];

fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { id: String::new(), msg: msg.into() }
}

fn disabled(why: &str) -> EngineError {
    EngineError::Disabled { id: String::new(), why: why.to_string() }
}

/// The result of a job whose connection was closed or replaced while it ran.
fn stale(label: &str) -> EngineError {
    EngineError::Disabled { id: label.to_string(), why: "the connection was closed or replaced while this ran, so its result was dropped".into() }
}

// ---- params ----------------------------------------------------------------------------------

/// `params[key]` as a [`ConnectionConfig`]; `None` when missing or null. Unknown keys are
/// ignored without looking into them.
fn parse_config(params: &Json, key: &str) -> Result<Option<ConnectionConfig>, EngineError> {
    let Some(v) = params.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    if !v.is_object() {
        return Err(bad(format!("`{key}` must be a connection object")));
    }
    ConnectionConfig::deserialize(v).map(Some).map_err(|e| bad(format!("`{key}` is not a connection: {e}")))
}

/// `config` for a command that connects or saves: it must name its `host` (so a bare `{}` can
/// never reach the default `localhost:5432`) and be valid.
fn connect_config(params: &Json) -> Result<Option<ConnectionConfig>, EngineError> {
    if let Some(v) = params.get("config").filter(|v| !v.is_null())
        && !v.get("host").is_some_and(Json::is_string)
    {
        return Err(bad("`config` must name its `host`"));
    }
    let Some(config) = parse_config(params, "config")? else { return Ok(None) };
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

const KINDS: [SecretKind; 3] = [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase];

/// A secret from the store; a failing store counts as none (the user is then asked).
fn read_secret(store: &dyn SecretStore, id: &str, kind: SecretKind) -> Option<String> {
    match store.get(id, kind) {
        Ok(v) => v.filter(|v| !v.is_empty()),
        Err(_) => {
            log::warn!("the secret store could not be read; going on without the saved secret");
            None
        }
    }
}

/// Stores each secret `c` holds under `id`; `false` when the store refused one.
fn store_secrets(store: &dyn SecretStore, id: &str, c: &Credentials) -> bool {
    let mut ok = true;
    for (kind, value) in
        [(SecretKind::DbPassword, &c.password), (SecretKind::SshPassword, &c.ssh.password), (SecretKind::SshPassphrase, &c.ssh.passphrase)]
    {
        if let Some(v) = value {
            ok &= store.set(id, kind, v).is_ok();
        }
    }
    ok
}

/// Deletes every secret of `id`; `false` when the store refused.
fn forget_secrets(store: &dyn SecretStore, id: &str) -> bool {
    let mut ok = true;
    for kind in KINDS {
        ok &= store.delete(id, kind).is_ok();
    }
    ok
}

/// Tells the user (and the log, without details) that the secret store failed.
fn secret_store_failed(s: &mut Session, what: &str) {
    log::warn!("the secret store refused a change");
    s.ui_requests.push(UiRequest::Notice(format!("The secret store refused to {what}; dbee will ask when a secret is needed.")));
}

/// `c` with what it lacks filled from the store under `config.id`: params first, then the
/// store, then none. Only the secrets this config can use are read.
async fn fill_from_store(mut c: Credentials, store: Option<Arc<dyn SecretStore>>, config: &ConnectionConfig) -> Credentials {
    let Some(store) = store else { return c };
    if config.id.is_empty() {
        return c;
    }
    let want_db = c.password.is_none();
    let (want_ssh_password, want_passphrase) = match config.ssh.as_ref().map(|ssh| &ssh.auth) {
        Some(SshAuth::Password) => (c.ssh.password.is_none(), false),
        Some(SshAuth::Key { .. }) => (false, c.ssh.passphrase.is_none()),
        Some(SshAuth::Agent) | None => (false, false),
    };
    if !(want_db || want_ssh_password || want_passphrase) {
        return c;
    }
    let id = config.id.clone();
    let reading = tokio::task::spawn_blocking(move || {
        let get = |want: bool, kind| if want { read_secret(store.as_ref(), &id, kind) } else { None };
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
        SshError::WrongPassphrase { key } => json!({ "reason": "wrongPassphrase", "key": key.display().to_string() }),
        SshError::AuthFailed { tried } => json!({ "reason": "authFailed", "tried": tried }),
        SshError::NoKey => json!({ "reason": "noKey" }),
        SshError::Connect(_) => json!({ "reason": "connect" }),
        SshError::KnownHosts(_) => json!({ "reason": "knownHosts" }),
        SshError::Channel(_) => json!({ "reason": "channel" }),
    };
    EngineError::Db { code: "ssh", message, data }
}

/// A database or tunnel error as the engine reports it: `Db` with the `PgError` code, its text
/// (which can quote the server, so it is shown, never logged) and details; SSH prompts become
/// `Needs`. `connectionLost` names the command that fixes it in `data.action`.
pub(crate) fn db_error(e: &PgError) -> EngineError {
    let data = match e {
        PgError::Ssh(ssh) => return ssh_error(ssh),
        PgError::Server { code, detail, hint, position, .. } => json!({ "sqlstate": code, "detail": detail, "hint": hint, "position": position }),
        PgError::ConnectionLost => json!({ "action": RECONNECT }),
        PgError::RowNotFound { statement } => json!({ "statement": statement }),
        _ => json!({}),
    };
    EngineError::Db { code: e.code(), message: e.to_string(), data }
}

impl Session {
    /// The browse connection, for a command that needs the database. `Disabled` when there is
    /// none or it is lost; when it has closed since its last use, the session is marked lost and
    /// the error is `connectionLost`.
    pub(crate) fn browse(&mut self) -> Result<Arc<PgConnection>, EngineError> {
        let c = self.state.connected.as_mut().ok_or_else(|| disabled(NOT_CONNECTED))?;
        if c.lost {
            return Err(disabled(LOST));
        }
        if c.browse.is_closed() {
            c.lost = true;
            log::info!("the browse connection has closed; the session is marked lost");
            return Err(db_error(&PgError::ConnectionLost));
        }
        Ok(Arc::clone(&c.browse))
    }

    /// `conn` is still the session's browse connection; a job that used another one (closed or
    /// replaced since) must drop its result.
    pub(crate) fn is_current_browse(&self, conn: &Arc<PgConnection>) -> bool {
        self.state.connected.as_ref().is_some_and(|c| Arc::ptr_eq(&c.browse, conn))
    }

    /// An operation on `conn` failed with `e`. When the connection is gone, the session lets go
    /// of it: a lost browse connection marks the session lost, and a lost query connection is
    /// dropped (the next query run opens a new one).
    pub(crate) fn db_failed(&mut self, conn: &Arc<PgConnection>, e: &PgError) -> EngineError {
        if (*e == PgError::ConnectionLost || conn.is_closed())
            && let Some(c) = self.state.connected.as_mut()
        {
            if Arc::ptr_eq(&c.browse, conn) && !c.lost {
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

/// An open connection and the tunnel it runs through.
struct Opened {
    conn: PgConnection,
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

/// What the result texts call the server: `PostgreSQL 14.12`.
fn server_text(info: &ServerInfo) -> String {
    format!("PostgreSQL {}", info.version)
}

/// Opens the tunnel (when the config has one) and a connection through it.
async fn open_connection(config: &ConnectionConfig, env: &SshEnv, creds: &Credentials) -> Result<Opened, EngineError> {
    let tunnel = match &config.ssh {
        None => None,
        Some(ssh) => Some(Arc::new(Tunnel::open(ssh, env, &creds.ssh, creds.trust.as_deref()).await.map_err(|e| ssh_error(&e))?)),
    };
    let target = Target { config, password: creds.password.as_deref(), tunnel: tunnel.clone() };
    match dbee_pg::connect(target).await {
        Ok(conn) => Ok(Opened { conn, tunnel }),
        Err(e) => {
            if let Some(t) = &tunnel {
                t.close().await;
            }
            Err(match e {
                PgError::Auth { .. } if creds.password.is_none() => EngineError::Needs { need: Need::Password, message: e.to_string() },
                other => db_error(&other),
            })
        }
    }
}

/// [`open_connection`], then the catalog.
async fn connect_and_load(config: &ConnectionConfig, env: &SshEnv, creds: &Credentials) -> Result<(Opened, Catalog), EngineError> {
    let opened = open_connection(config, env, creds).await?;
    match load_catalog(&opened.conn).await {
        Ok(catalog) => Ok((opened, catalog)),
        Err(e) => {
            opened.close().await;
            Err(db_error(&e))
        }
    }
}

/// `{connection, server, tables, warnings}` for an open or a reconnect.
fn opened_json(c: &Connected) -> Json {
    json!({
        "connection": c.summary(),
        "server": server_text(&c.info),
        "tables": c.catalog.table_count(),
        "warnings": c.info.warnings,
    })
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Writes the connections file after a change that must not fail the command (a use); tells the
/// user when it could not be written.
fn save_store_or_tell(s: &mut Session) {
    if let Err(e) = s.state.store.save() {
        log::warn!("the connections file could not be written");
        s.ui_requests.push(UiRequest::Notice(format!("The recent connections could not be saved: {e}")));
    }
}

/// Sends a cancel request for the statement running on each of `conns` that is still open, in
/// a detached task. The jobs running them then end with an error (and their results are
/// dropped where their connection is gone).
fn cancel_statements(s: &Session, conns: impl IntoIterator<Item = Arc<PgConnection>>) {
    let cancellers: Vec<Canceller> = conns.into_iter().filter(|c| !c.is_closed()).map(|c| c.canceller()).collect();
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
/// `saveSecrets` and a secret store, the given secrets are stored (the ones not given are kept);
/// without `saveSecrets`, the stored ones are forgotten.
fn save(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let mut config = connect_config(params)?.ok_or_else(|| bad("`config` is required (a connection object)"))?;
    let secrets = given(params)?;
    if config.id.is_empty() {
        config.id = new_id();
    }
    let mut store = s.state.store.clone();
    store.upsert(config.clone());
    store.save().map_err(|e| EngineError::Config(e.to_string()))?;
    s.state.store = store;
    if let Some(secret_store) = s.secrets.clone() {
        let ok = if config.save_secrets {
            store_secrets(secret_store.as_ref(), &config.id, &secrets)
        } else {
            forget_secrets(secret_store.as_ref(), &config.id)
        };
        if !ok {
            secret_store_failed(s, "keep the connection's secrets");
        }
    }
    Ok(Outcome::Done(json!({ "id": config.id })))
}

/// `{id}` → `{id}`: removes the connection from the saved and recent lists, with its secrets.
fn delete(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let id = p::str(params, "id")?.to_owned();
    let mut store = s.state.store.clone();
    let saved = store.remove(&id);
    let recent = store.recent.len();
    store.recent.retain(|r| r.config.id != id);
    if !saved && store.recent.len() == recent {
        return Err(bad("no saved or recent connection has this `id`"));
    }
    store.save().map_err(|e| EngineError::Config(e.to_string()))?;
    s.state.store = store;
    if let Some(secret_store) = s.secrets.clone()
        && !forget_secrets(secret_store.as_ref(), &id)
    {
        secret_store_failed(s, "delete the connection's secrets");
    }
    Ok(Outcome::Done(json!({ "id": id })))
}

/// `{id}` → `{id}`: a saved copy named `<name> (copy)`, with a new id and copies of the stored
/// secrets.
fn duplicate(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let id = p::str(params, "id")?;
    let original = s.state.store.get(id).cloned().ok_or_else(|| bad("no saved or recent connection has this `id`"))?;
    let copy = ConnectionConfig { id: new_id(), name: format!("{} (copy)", original.display_name()), ..original.clone() };
    let mut store = s.state.store.clone();
    store.upsert(copy.clone());
    store.save().map_err(|e| EngineError::Config(e.to_string()))?;
    s.state.store = store;
    if let Some(secret_store) = s.secrets.clone()
        && copy.save_secrets
    {
        let mut ok = true;
        for kind in KINDS {
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

/// `{url}` → `{host, port, user, password, database, sslMode}`, each `null` when the URL lacks
/// it. Not journaled.
fn parse(_: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let parts = parse_url(p::str(params, "url")?).map_err(|e| bad(e.to_string()))?;
    Ok(Outcome::Done(json!({
        "host": parts.host,
        "port": parts.port,
        "user": parts.user,
        "password": parts.password,
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

/// `{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?}` → a job returning
/// `{server, via, warnings}`: connects (through the tunnel when there is one), then closes. The
/// screen and the lists stay as they are. A trusted unknown host key is learned.
fn test(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let config = target_config(s, params)?;
    let creds = given(params)?;
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_job(TEST, async move {
        let creds = fill_from_store(creds, store, &config).await;
        let result = match open_connection(&config, &env, &creds).await {
            Ok(opened) => {
                let info = opened.conn.info();
                let r = json!({ "server": server_text(info), "via": opened.via(), "warnings": info.warnings });
                opened.close().await;
                Ok(r)
            }
            Err(e) => Err(e),
        };
        let apply: Apply = Box::new(move |_: &mut Session| result);
        apply
    });
    Ok(Outcome::Started(job))
}

/// `{id | config, password?, sshPassword?, sshPassphrase?, trustHostKey?, savePassword?}` → a
/// job returning `{connection, server, tables, warnings}`: the tunnel, the browse connection and
/// the catalog. On success the workspace shows, the connection goes to the top of the recent
/// list (an unsaved config gets an id for it) and the file is written; with `savePassword` the
/// secrets given in the params are stored. One open at a time (`Busy`).
fn open(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    if s.jobs.is_running_label(OPEN) {
        return Err(EngineError::Busy("a connection is being opened".into()));
    }
    let mut config = target_config(s, params)?;
    let typed = given(params)?;
    let save_secrets = p::bool_or(params, "savePassword", false)?;
    if config.id.is_empty() {
        config.id = new_id();
    }
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_job(OPEN, async move {
        let creds = fill_from_store(typed.clone(), store.clone(), &config).await;
        let result = connect_and_load(&config, &env, &creds).await;
        let stored = match (&result, save_secrets, store) {
            (Ok(_), true, Some(store)) => {
                let id = config.id.clone();
                Some(tokio::task::spawn_blocking(move || store_secrets(store.as_ref(), &id, &typed)).await.unwrap_or(false))
            }
            _ => None,
        };
        let apply: Apply = Box::new(move |s: &mut Session| apply_open(s, config, creds, result, stored));
        apply
    });
    Ok(Outcome::Started(job))
}

fn apply_open(
    s: &mut Session,
    config: ConnectionConfig,
    credentials: Credentials,
    result: Result<(Opened, Catalog), EngineError>,
    stored: Option<bool>,
) -> Result<Json, EngineError> {
    let (opened, catalog) = result?;
    if s.state.connected.is_some() {
        // Open is disabled while connected and runs one at a time, so this cannot happen; the new
        // connection closes as it is dropped.
        return Err(EngineError::Busy("already connected".into()));
    }
    let Opened { conn, tunnel } = opened;
    let info = conn.info().clone();
    s.state.store.touch_recent(&config, unix_now());
    let c = Connected { config, info, catalog: Arc::new(catalog), lost: false, browse: Arc::new(conn), query: None, tunnel, credentials };
    let result = opened_json(&c);
    for w in &c.info.warnings {
        s.ui_requests.push(UiRequest::Notice(w.clone()));
    }
    s.state.connected = Some(c);
    s.state.screen = Screen::Workspace;
    save_store_or_tell(s);
    if stored == Some(false) {
        secret_store_failed(s, "save the password");
    }
    Ok(result)
}

/// `{password?, sshPassword?, sshPassphrase?, trustHostKey?}` → a job returning `{connection,
/// server, tables, warnings}`: a new tunnel, browse connection and catalog with the secrets kept
/// from the open (the params answer a prompt). On success the old connections are let go (a
/// statement still running on one is cancelled), the query connection is opened again on the
/// next run, and `lost` clears; tabs and their contents stay.
fn reconnect(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    if s.jobs.is_running_label(RECONNECT) {
        return Err(EngineError::Busy("a reconnect is in progress".into()));
    }
    let answers = given(params)?;
    let c = s.state.connected.as_ref().ok_or_else(|| disabled(NOT_CONNECTED))?;
    let mut creds = c.credentials.clone();
    creds.password = answers.password.or(creds.password);
    creds.ssh.password = answers.ssh.password.or(creds.ssh.password);
    creds.ssh.passphrase = answers.ssh.passphrase.or(creds.ssh.passphrase);
    creds.trust = answers.trust.or(creds.trust);
    let config = c.config.clone();
    let old = Arc::clone(&c.browse);
    let (env, store) = (s.ssh.clone(), s.secrets.clone());
    let job = s.spawn_job(RECONNECT, async move {
        let creds = fill_from_store(creds, store, &config).await;
        let result = connect_and_load(&config, &env, &creds).await;
        let apply: Apply = Box::new(move |s: &mut Session| apply_reconnect(s, &old, creds, result));
        apply
    });
    Ok(Outcome::Started(job))
}

fn apply_reconnect(
    s: &mut Session,
    old: &Arc<PgConnection>,
    credentials: Credentials,
    result: Result<(Opened, Catalog), EngineError>,
) -> Result<Json, EngineError> {
    let (opened, catalog) = result?;
    let Some(c) = s.state.connected.as_mut().filter(|c| Arc::ptr_eq(&c.browse, old)) else {
        // Closed or reconnected meanwhile; the new connection closes as it is dropped.
        return Err(stale(RECONNECT));
    };
    let Opened { conn, tunnel } = opened;
    c.info = conn.info().clone();
    c.catalog = Arc::new(catalog);
    let old_browse = std::mem::replace(&mut c.browse, Arc::new(conn));
    let old_query = c.query.take();
    c.tunnel = tunnel;
    c.credentials = credentials;
    c.lost = false;
    let result = opened_json(c);
    if s.busy() {
        cancel_statements(s, std::iter::once(old_browse).chain(old_query));
    }
    Ok(result)
}

/// `{}` → a job returning `{tables}`: reloads the catalog on the browse connection. Tabs keep
/// their tables.
fn refresh(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    let conn = s.browse()?;
    let job = s.spawn_job(REFRESH, async move {
        let result = load_catalog(&conn).await;
        let apply: Apply = Box::new(move |s: &mut Session| {
            if !s.is_current_browse(&conn) {
                return Err(stale(REFRESH));
            }
            let catalog = result.map_err(|e| s.db_failed(&conn, &e))?;
            let tables = catalog.table_count();
            if let Some(c) = s.state.connected.as_mut() {
                c.catalog = Arc::new(catalog);
            }
            Ok(json!({ "tables": tables }))
        });
        apply
    });
    Ok(Outcome::Started(job))
}

/// `{force?}` → `{closed}`: back to the connection screen. Query tabs with text ask first
/// (`Needs::Confirm { what: "queryText" }`) unless `force`. Every tab closes; statements still
/// running are cancelled through their connections' cancellers, and the connections are let go
/// (they close once the jobs using them end; those jobs' results are dropped).
fn close(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let force = p::bool_or(params, "force", false)?;
    let c = s.state.connected.as_ref().ok_or_else(|| disabled(NOT_CONNECTED))?;
    let with_text = s.state.tabs.iter().filter(|t| matches!(t, Tab::Query(q) if !q.text.trim().is_empty())).count();
    if with_text > 0 && !force {
        let tabs = if with_text == 1 { "1 query tab".to_string() } else { format!("{with_text} query tabs") };
        return Err(EngineError::Needs {
            need: Need::Confirm { what: "queryText".into() },
            message: format!("Disconnect from {}? The text of {tabs} will be lost.", c.config.display_name()),
        });
    }
    let Some(c) = s.state.connected.take() else { return Err(disabled(NOT_CONNECTED)) };
    s.state.screen = Screen::Connections;
    s.state.tabs.clear();
    s.state.active = None;
    if s.busy() {
        cancel_statements(s, std::iter::once(Arc::clone(&c.browse)).chain(c.query.clone()));
    }
    Ok(Outcome::Done(json!({ "closed": c.config.id })))
}
