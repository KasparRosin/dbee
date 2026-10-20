//! Opening a connection: TCP or an SSH tunnel channel, TLS, login, the session setup, and the
//! cancel request.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dbee_config::ConnectionConfig;
use dbee_ssh::Tunnel;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::runtime::Handle;
use tokio::sync::{Mutex, MutexGuard};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{CancelToken, Client, Connection, SimpleQueryMessage};

use crate::PgError;
use crate::error::{self, Phase};
use crate::tls::{self, Tls};

/// How long opening a connection (TCP or the tunnel channel, TLS, login and the session setup)
/// and delivering a cancel request may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The oldest supported server, as `server_version_num`: Postgres 14.
pub const MIN_VERSION_NUM: u32 = 140000;

/// TCP keepalive on direct connections: the first probe after this long without traffic.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
/// Then a probe this often, and the connection is dead after [`KEEPALIVE_RETRIES`] unanswered.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;

/// What the server said about itself when the connection opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    /// `SHOW server_version`, e.g. `14.12` or `18.0 (Debian 18.0-1)`.
    pub version: String,
    /// `SHOW server_version_num`, e.g. `140012`.
    pub version_num: u32,
    /// New transactions are read-only (`default_transaction_read_only` is `on`): set by dbee for a
    /// read-only connection, or by the server's own settings.
    pub read_only: bool,
    /// Things the user should know, e.g. [`version_warning`].
    pub warnings: Vec<String>,
}

/// The warning for a server older than [`MIN_VERSION_NUM`], or `None`.
pub fn version_warning(version_num: u32) -> Option<String> {
    if version_num >= MIN_VERSION_NUM {
        return None;
    }
    // From 10 on the number is major * 10000 + minor; before, major.minor * 100 + patch.
    let version =
        if version_num >= 100_000 { (version_num / 10_000).to_string() } else { format!("{}.{}", version_num / 10_000, version_num / 100 % 100) };
    Some(format!("PostgreSQL {version} is older than {}, the oldest version dbee supports; some things may not work", MIN_VERSION_NUM / 10_000))
}

/// Where and how to connect. `password` is the database password (never stored in `config`);
/// `tunnel` is the open SSH tunnel when `config.ssh` is set.
#[derive(Clone)]
pub struct Target<'a> {
    pub config: &'a ConnectionConfig,
    pub password: Option<&'a str>,
    pub tunnel: Option<Arc<Tunnel>>,
}

impl std::fmt::Debug for Target<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("config", &self.config)
            .field("password", &self.password.map(|_| "<redacted>"))
            .field("tunnel", &self.tunnel)
            .finish()
    }
}

/// An open connection. Every operation on it holds [`PgConnection::lock`] for its whole run, so
/// operations never interleave (an apply's `BEGIN` … `COMMIT` included). Dropping it closes the
/// connection.
pub struct PgConnection {
    client: Client,
    lock: Mutex<()>,
    info: ServerInfo,
    /// Set when the connection task ended, or an error showed the session is gone.
    closed: Arc<AtomicBool>,
    canceller: Canceller,
}

impl std::fmt::Debug for PgConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgConnection").field("info", &self.info).field("closed", &self.is_closed()).finish()
    }
}

impl PgConnection {
    /// What the server said about itself.
    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// The connection has ended: closed by the server, the network or the tunnel. Every further
    /// operation fails with [`PgError::ConnectionLost`].
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.client.is_closed()
    }

    /// Cancels whatever statement runs on this connection at the time of the call; usable while
    /// the statement holds the lock.
    pub fn canceller(&self) -> Canceller {
        self.canceller.clone()
    }

    /// The client, for the crate's runners. Callers hold [`lock`](Self::lock) while they use it.
    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    /// Serializes operations: hold the guard for the whole operation. Refuses with
    /// [`PgError::ConnectionLost`] once the connection is closed, checked after the mutex is
    /// taken so an operation queued behind an abandoned apply cannot run inside its transaction.
    pub(crate) async fn lock(&self) -> Result<MutexGuard<'_, ()>, PgError> {
        let guard = self.lock.lock().await;
        if self.is_closed() {
            return Err(PgError::ConnectionLost);
        }
        Ok(guard)
    }

    /// Mark the session unusable, for an operation that was abandoned half-way (a dropped apply
    /// future leaves the server inside a transaction). Every later operation is refused with
    /// `ConnectionLost`; the socket and the server-side transaction stay open until the
    /// `PgConnection` is dropped, so the engine must drop it and reconnect.
    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Map an error of an operation on this connection; a lost session marks it closed.
    pub(crate) fn error(&self, e: &tokio_postgres::Error) -> PgError {
        let mapped = error::map(e, Phase::Session);
        if mapped == PgError::ConnectionLost {
            self.closed.store(true, Ordering::Release);
        }
        mapped
    }
}

/// The route a cancel request takes when the connection runs through a tunnel.
#[derive(Clone)]
struct TunnelRoute {
    tunnel: Arc<Tunnel>,
    host: String,
    port: u16,
}

/// Sends a cancel request for the statement running on one connection. Cheap to clone; holds
/// no lock.
#[derive(Clone)]
pub struct Canceller {
    token: CancelToken,
    tls: Tls,
    route: Option<TunnelRoute>,
}

impl std::fmt::Debug for Canceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token holds the backend's secret key: not shown.
        f.debug_struct("Canceller").field("tls", &self.tls).field("tunnel", &self.route.is_some()).finish()
    }
}

impl Canceller {
    /// Ask the server to cancel the running statement, over a new connection: a new channel
    /// through the tunnel, or a direct TCP connection to the address the connection uses. `Ok`
    /// means the request was delivered; the statement then fails with [`PgError::Cancelled`],
    /// unless it finished first.
    pub async fn cancel(&self) -> Result<(), PgError> {
        // The timeout below needs a runtime; without one it would panic.
        tokio::runtime::Handle::try_current().map_err(|_| PgError::Other("cancelling needs a tokio runtime".into()))?;
        let sending = async {
            match &self.route {
                Some(r) => {
                    let stream = r.tunnel.connect(&r.host, r.port).await?;
                    let tls = tls_connect(&self.tls, &r.host, &stream);
                    self.token.cancel_query_raw(stream, tls).await
                }
                None => self.token.cancel_query(self.tls.connector.clone()).await,
            }
            .map_err(|e| error::map(&e, Phase::Connect))
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, sending).await {
            Ok(result) => {
                log::info!("pg: cancel request {}", if result.is_ok() { "sent" } else { "failed" });
                result
            }
            Err(_) => Err(PgError::Connect(format!("the cancel request was not delivered within {} s", CONNECT_TIMEOUT.as_secs()))),
        }
    }
}

/// `host:port`, with brackets around an IPv6 address.
fn address(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

/// Open a connection. Runs on the caller's tokio runtime, which also drives the connection
/// afterwards.
///
/// The startup sets `application_name=dbee` (and tokio-postgres `client_encoding=UTF8`). Then
/// the session runs `SET standard_conforming_strings = on` and, for a read-only connection, `SET
/// default_transaction_read_only = on`, which a separate `SHOW` must confirm (a transaction-mode
/// pooler that moved the session elsewhere shows there); the same `SHOW` confirms
/// `standard_conforming_strings` and `client_encoding`. In `prefer` mode a failed TLS handshake
/// is tried again without TLS, as libpq does.
pub async fn connect(t: Target<'_>) -> Result<PgConnection, PgError> {
    let cfg = t.config;
    // The connection task needs a runtime; `tokio::spawn` would panic without one.
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| PgError::Other("connecting needs a tokio runtime".into()))?;
    cfg.validate().map_err(|e| PgError::Connect(e.to_string()))?;
    if cfg.ssh.is_some() && t.tunnel.is_none() {
        return Err(PgError::Connect("this connection goes through an SSH tunnel, and none is open".into()));
    }
    let tls = tls::make(cfg.ssl_mode, cfg.ca_file.as_deref())?;
    match attempt(&t, &tls, &runtime).await {
        Err(PgError::Tls(_)) if tls.pg_mode() == tokio_postgres::config::SslMode::Prefer => {
            log::info!("pg: the TLS handshake failed in prefer mode; connecting without TLS");
            attempt(&t, &tls.plain(), &runtime).await
        }
        other => other,
    }
}

async fn attempt(t: &Target<'_>, tls: &Tls, runtime: &Handle) -> Result<PgConnection, PgError> {
    let addr = address(&t.config.host, t.config.port);
    log::info!("pg: connecting to {addr}{}", if t.tunnel.is_some() { " through SSH" } else { "" });
    match tokio::time::timeout(CONNECT_TIMEOUT, open(t, tls, runtime)).await {
        Ok(Ok(conn)) => {
            log::info!("pg: connected to {addr} (server {}, TLS mode {:?})", conn.info.version_num, tls.mode());
            Ok(conn)
        }
        Ok(Err(e)) => {
            log::info!("pg: connecting to {addr} failed ({})", e.code());
            Err(match e {
                PgError::Connect(m) => PgError::Connect(format!("{addr}: {m}")),
                other => other,
            })
        }
        Err(_) => Err(PgError::Connect(format!("{addr} did not answer within {} s", CONNECT_TIMEOUT.as_secs()))),
    }
}

/// The tokio-postgres settings. An empty user or database is left out, so the defaults apply
/// as in libpq (the OS user name; a database named like the user).
fn pg_config(cfg: &ConnectionConfig, password: Option<&str>, tls: &Tls) -> tokio_postgres::Config {
    let mut pg = tokio_postgres::Config::new();
    pg.host(&cfg.host)
        .port(cfg.port)
        .application_name("dbee")
        .ssl_mode(tls.pg_mode())
        .connect_timeout(CONNECT_TIMEOUT)
        .keepalives(true)
        .keepalives_idle(KEEPALIVE_IDLE)
        .keepalives_interval(KEEPALIVE_INTERVAL)
        .keepalives_retries(KEEPALIVE_RETRIES);
    if !cfg.user.is_empty() {
        pg.user(&cfg.user);
    }
    if !cfg.database.is_empty() {
        pg.dbname(&cfg.database);
    }
    if let Some(password) = password {
        pg.password(password);
    }
    pg
}

async fn open(t: &Target<'_>, tls: &Tls, runtime: &Handle) -> Result<PgConnection, PgError> {
    let cfg = t.config;
    let pg = pg_config(cfg, t.password, tls);
    let connect_error = |e: tokio_postgres::Error| error::map(&e, Phase::Connect);
    let (client, closed) = match &t.tunnel {
        Some(tunnel) => {
            let stream = tunnel.connect(&cfg.host, cfg.port).await?;
            let connect = tls_connect(tls, &cfg.host, &stream);
            let (client, connection) = pg.connect_raw(stream, connect).await.map_err(connect_error)?;
            (client, drive(runtime, connection))
        }
        None => {
            let (client, connection) = pg.connect(tls.connector.clone()).await.map_err(connect_error)?;
            (client, drive(runtime, connection))
        }
    };
    let info = setup(&client, cfg.read_only).await?;
    let route = t.tunnel.as_ref().map(|tunnel| TunnelRoute { tunnel: Arc::clone(tunnel), host: cfg.host.clone(), port: cfg.port });
    let canceller = Canceller { token: client.cancel_token(), tls: tls.clone(), route };
    Ok(PgConnection { client, lock: Mutex::new(()), info, closed, canceller })
}

/// The TLS connector for one stream to `host` (the stream only fixes the type).
fn tls_connect<S>(tls: &Tls, host: &str, _stream: &S) -> <tokio_postgres_rustls::MakeRustlsConnect as MakeTlsConnect<S>>::TlsConnect
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut make = tls.connector.clone();
    match MakeTlsConnect::<S>::make_tls_connect(&mut make, host) {
        Ok(connect) => connect,
        Err(never) => match never {},
    }
}

/// Drive the connection on the runtime until it ends; the flag tells when it has. Server notices
/// and notifications are dropped unlogged, as they can quote data.
fn drive<S, T>(runtime: &Handle, mut connection: Connection<S, T>) -> Arc<AtomicBool>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let closed = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&closed);
    runtime.spawn(async move {
        loop {
            match std::future::poll_fn(|cx| connection.poll_message(cx)).await {
                Some(Ok(_)) => {}
                // The text of a tokio-postgres error is its kind only, never server text.
                Some(Err(e)) => {
                    log::info!("pg: connection ended: {e}");
                    break;
                }
                None => {
                    log::debug!("pg: connection closed");
                    break;
                }
            }
        }
        flag.store(true, Ordering::Release);
    });
    closed
}

/// The settings the setup reads back, in order.
const SHOW_SETTINGS: &str = "SHOW default_transaction_read_only; SHOW standard_conforming_strings; SHOW client_encoding; \
                             SHOW server_version; SHOW server_version_num";

/// The session setup; returns what the server says about itself.
async fn setup(client: &Client, read_only: bool) -> Result<ServerInfo, PgError> {
    let fail = |e: tokio_postgres::Error| error::map(&e, Phase::Connect);
    let set = if read_only {
        "SET standard_conforming_strings = on; SET default_transaction_read_only = on"
    } else {
        "SET standard_conforming_strings = on"
    };
    client.batch_execute(set).await.map_err(fail)?;
    let mut answers = Vec::new();
    for message in client.simple_query(SHOW_SETTINGS).await.map_err(fail)? {
        if let SimpleQueryMessage::Row(row) = message {
            answers.push(row.try_get(0).ok().flatten().unwrap_or_default().to_owned());
        }
    }
    let [ro, conforming, encoding, version, num] = answers.as_slice() else {
        return Err(PgError::Other(format!("the server answered {} of 5 settings", answers.len())));
    };
    // dbee quotes values into SQL text; that is safe only with both of these.
    if conforming != "on" || encoding != "UTF8" {
        return Err(PgError::Other(format!(
            "the session runs with standard_conforming_strings = {conforming} and client_encoding = {encoding}; dbee needs on and UTF8"
        )));
    }
    let version_num = num.trim().parse::<u32>().map_err(|_| PgError::Other(format!("the server sent an unreadable server_version_num ({num:?})")))?;
    let server_read_only = ro == "on";
    if read_only && !server_read_only {
        return Err(PgError::Other(format!(
            "read-only mode did not stick (default_transaction_read_only is {ro:?}); a transaction-mode connection pooler is not supported"
        )));
    }
    let warnings = version_warning(version_num).into_iter().collect();
    Ok(ServerInfo { version: version.clone(), version_num, read_only: server_read_only, warnings })
}
