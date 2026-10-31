//! Opening a connection: TCP or an SSH tunnel channel, TLS, the login and the session setup; the
//! connection, its text queries, and the canceller that sends `KILL QUERY` over a second
//! connection.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dbee_config::SslMode;
use dbee_driver::{CONNECT_TIMEOUT, DbError, Purpose, QUERY_LIMITS, ServerInfo, Target, tls};
use dbee_ssh::Tunnel;
use mysql_common::constants::{CapabilityFlags, Command};
use rustls::pki_types::ServerName;
use rustls::{AlertDescription, ClientConfig};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard};

use crate::auth::{self, Login};
use crate::cancel::{Canceller, KillRoute, Statements};
use crate::flavor::{Flavor, version_num, version_warning};
use crate::wire::{self, MAX_PAYLOAD, Packets, Stream, TextResult};

/// The largest payload dbee reads before the login is done.
const LOGIN_MAX_PAYLOAD: usize = 64 * 1024;

/// The browse connection's mode: strict values, and backslashes as plain characters in strings.
/// Works when the server's mode is empty.
const BROWSE_SQL_MODE: &str = "SET SESSION sql_mode = TRIM(BOTH ',' FROM CONCAT(@@SESSION.sql_mode, ',STRICT_ALL_TABLES,NO_BACKSLASH_ESCAPES'))";

/// Why a TLS handshake that the server ended with one of these alerts failed.
const NO_SHARED_CIPHER: &str = "the server offers only TLS ciphers dbee does not support; connect through an SSH tunnel instead";

/// Where a connection goes: directly to `host:port`, or through the tunnel to it.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tunnel: Option<Arc<Tunnel>>,
}

impl Route {
    /// `host:port`, with brackets around an IPv6 address.
    fn address(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }
}

/// TLS for one attempt to connect.
#[derive(Clone)]
pub(crate) enum TlsUse {
    Off,
    /// TLS when the server offers it.
    Prefer(Arc<ClientConfig>),
    Require(Arc<ClientConfig>),
}

/// A logged-in protocol session: its packets, and what the login agreed on.
pub(crate) struct Session {
    packets: Packets,
    caps: CapabilityFlags,
    flavor: Flavor,
    /// The TLS settings the session runs with; `None` when it is plain.
    tls: Option<Arc<ClientConfig>>,
}

impl Session {
    /// Open the stream, read the greeting, start TLS as `tls` says, and log in.
    pub(crate) async fn open(route: &Route, who: &Login<'_>, tls: &TlsUse) -> Result<Session, DbError> {
        let mut packets = Packets::new(open_stream(route).await?, LOGIN_MAX_PAYLOAD);
        let greeting = auth::parse_greeting(&packets.read().await?)?;
        let config = match tls {
            TlsUse::Off => None,
            TlsUse::Prefer(config) => greeting.offers_tls().then(|| Arc::clone(config)),
            TlsUse::Require(_) if !greeting.offers_tls() => return Err(DbError::Tls("the server does not offer TLS".into())),
            TlsUse::Require(config) => Some(Arc::clone(config)),
        };
        let caps = auth::capabilities(&greeting, config.is_some(), who.database.is_some())?;
        if let Some(config) = &config {
            packets.write(&auth::ssl_request(caps)).await?;
            packets = start_tls(packets, Arc::clone(config), &route.host).await?;
        }
        auth::login(&mut packets, &greeting, who, caps, config.is_some()).await?;
        packets.set_max(MAX_PAYLOAD);
        Ok(Session { packets, caps, flavor: Flavor::from_version(&greeting.version), tls: config })
    }

    /// Run `sql` as one `COM_QUERY` and read the answer.
    pub(crate) async fn query(&mut self, sql: &str) -> Result<TextResult, DbError> {
        self.packets.reset();
        let mut command = Vec::with_capacity(sql.len().saturating_add(1));
        command.push(Command::COM_QUERY as u8);
        command.extend_from_slice(sql.as_bytes());
        self.packets.write(&command).await?;
        wire::read_result(&mut self.packets, self.caps, self.flavor, QUERY_LIMITS).await
    }

    /// Say goodbye (`COM_QUIT`); the server closes the connection.
    pub(crate) async fn quit(mut self) {
        self.packets.reset();
        let _ = self.packets.write(&[Command::COM_QUIT as u8]).await;
    }
}

async fn open_stream(route: &Route) -> Result<Stream, DbError> {
    if let Some(tunnel) = &route.tunnel {
        return Ok(Box::new(tunnel.connect(&route.host, route.port).await?));
    }
    let tcp = TcpStream::connect((route.host.as_str(), route.port)).await.map_err(|e| DbError::Connect(e.to_string()))?;
    tcp.set_nodelay(true).map_err(|e| DbError::Connect(e.to_string()))?;
    Ok(Box::new(tcp))
}

/// Run the TLS handshake on the session's stream; the server name checked is the configured
/// host, also through a tunnel.
async fn start_tls(packets: Packets, config: Arc<ClientConfig>, host: &str) -> Result<Packets, DbError> {
    let (stream, seq) = packets.into_stream()?;
    let name = ServerName::try_from(host.to_owned()).map_err(|_| DbError::Tls(format!("{host} is not a host name TLS can check")))?;
    let stream = tokio_rustls::TlsConnector::from(config).connect(name, stream).await.map_err(|e| tls_error(&e))?;
    Ok(Packets::resume(Box::new(stream), seq, LOGIN_MAX_PAYLOAD))
}

fn tls_error(e: &std::io::Error) -> DbError {
    match e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
        Some(rustls::Error::AlertReceived(
            AlertDescription::HandshakeFailure | AlertDescription::InsufficientSecurity | AlertDescription::ProtocolVersion,
        )) => DbError::Tls(NO_SHARED_CIPHER.into()),
        Some(inner) => DbError::Tls(inner.to_string()),
        None => DbError::Tls(e.to_string()),
    }
}

/// While connecting, a closed stream means the server would not talk.
pub(crate) fn while_connecting(e: DbError) -> DbError {
    match e {
        DbError::ConnectionLost => DbError::Connect("the server closed the connection".into()),
        other => other,
    }
}

/// Open a connection on the caller's tokio runtime.
///
/// After the login (as `utf8mb4_general_ci`, with `CLIENT_FOUND_ROWS`), the session runs `SET
/// NAMES utf8mb4`; a browse connection also makes its `sql_mode` strict and turns backslash
/// escapes off; a read-only connection runs `SET SESSION TRANSACTION READ ONLY`, which the
/// flavor's variable must then confirm. In `prefer` mode a failed TLS handshake is tried again
/// without TLS, as libpq does.
pub async fn connect(t: Target<'_>) -> Result<MySqlConnection, DbError> {
    // The timeouts need a runtime; without one they would panic.
    tokio::runtime::Handle::try_current().map_err(|_| DbError::Other("connecting needs a tokio runtime".into()))?;
    let cfg = t.config;
    cfg.validate().map_err(|e| DbError::Connect(e.to_string()))?;
    if cfg.ssh.is_some() && t.tunnel.is_none() {
        return Err(DbError::Connect("this connection goes through an SSH tunnel, and none is open".into()));
    }
    let tls = match tls::client_config(cfg.ssl_mode, cfg.ca_file.as_deref(), &[])? {
        None => TlsUse::Off,
        Some(config) if cfg.ssl_mode == SslMode::Prefer => TlsUse::Prefer(config),
        Some(config) => TlsUse::Require(config),
    };
    let route = Route { host: cfg.host.clone(), port: cfg.port, tunnel: t.tunnel.clone() };
    let password = t.password.unwrap_or_default();
    let who = Login { user: &cfg.user, password: password.as_bytes(), database: Some(cfg.database.as_str()).filter(|d| !d.is_empty()) };
    let (session, info, connection_id) = match attempt(&route, &who, &tls, cfg.read_only, t.purpose).await {
        Err(DbError::Tls(_)) if matches!(tls, TlsUse::Prefer(_)) => {
            log::info!("mysql: the TLS handshake failed in prefer mode; connecting without TLS");
            attempt(&route, &who, &TlsUse::Off, cfg.read_only, t.purpose).await
        }
        other => other,
    }?;
    let statements = Arc::new(Statements::default());
    let tls_used = session.tls.clone().map_or(TlsUse::Off, TlsUse::Require);
    let canceller = Canceller(Arc::new(KillRoute {
        route,
        user: cfg.user.clone(),
        password: password.to_owned(),
        tls: tls_used,
        connection_id,
        statements: Arc::clone(&statements),
    }));
    Ok(MySqlConnection { session: Mutex::new(session), info, closed: AtomicBool::new(false), statements, canceller })
}

async fn attempt(route: &Route, who: &Login<'_>, tls: &TlsUse, read_only: bool, purpose: Purpose) -> Result<(Session, ServerInfo, u64), DbError> {
    let addr = route.address();
    log::info!("mysql: connecting to {addr}{}", if route.tunnel.is_some() { " through SSH" } else { "" });
    let opening = async {
        let mut session = Session::open(route, who, tls).await?;
        let (info, id) = setup(&mut session, read_only, purpose).await?;
        Ok((session, info, id))
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, opening).await {
        Ok(Ok((session, info, id))) => {
            log::info!(
                "mysql: connected to {addr} ({} {}, TLS {})",
                info.product,
                info.version_num,
                if session.tls.is_some() { "on" } else { "off" }
            );
            Ok((session, info, id))
        }
        Ok(Err(e)) => {
            let e = while_connecting(e);
            log::info!("mysql: connecting to {addr} failed ({})", e.code());
            Err(match e {
                DbError::Connect(m) => DbError::Connect(format!("{addr}: {m}")),
                other => other,
            })
        }
        Err(_) => Err(DbError::Connect(format!("{addr} did not answer within {} s", CONNECT_TIMEOUT.as_secs()))),
    }
}

/// The first row's cells of a setup query, `n` of them.
fn setup_row<const N: usize>(r: TextResult, what: &str) -> Result<[Option<String>; N], DbError> {
    r.rows
        .into_iter()
        .next()
        .and_then(|row| <[Option<String>; N]>::try_from(row).ok())
        .ok_or_else(|| DbError::Other(format!("the server sent no {what}")))
}

/// The session setup; returns what the server says about itself, and the connection id.
async fn setup(session: &mut Session, read_only: bool, purpose: Purpose) -> Result<(ServerInfo, u64), DbError> {
    session.query("SET NAMES utf8mb4").await?;
    if purpose == Purpose::Browse {
        session.query(BROWSE_SQL_MODE).await?;
    }
    if read_only {
        session.query("SET SESSION TRANSACTION READ ONLY").await?;
    }
    let [id, version] = setup_row(session.query("SELECT CONNECTION_ID(), @@version").await?, "version")?;
    let version = version.unwrap_or_default();
    let flavor = Flavor::from_version(&version);
    session.flavor = flavor;
    let version_num = version_num(&version).ok_or_else(|| DbError::Other(format!("the server sent an unreadable version ({version:?})")))?;
    let connection_id =
        id.and_then(|id| id.parse::<u64>().ok()).ok_or_else(|| DbError::Other("the server sent an unreadable connection id".into()))?;
    let (variable, value) = read_only_state(session, flavor).await?;
    let server_read_only = value == "1";
    if read_only && !server_read_only {
        return Err(DbError::Other(format!("read-only mode did not stick (@@session.{variable} is {value:?})")));
    }
    let warnings = version_warning(flavor, version_num).into_iter().collect();
    Ok((ServerInfo { product: flavor.product().into(), version, version_num, read_only: server_read_only, warnings }, connection_id))
}

/// The flavor's read-only variable and its value; the other spelling when the server has no
/// such variable.
async fn read_only_state(session: &mut Session, flavor: Flavor) -> Result<(&'static str, String), DbError> {
    let [first, second] = flavor.read_only_variables();
    for variable in [first, second] {
        match session.query(&format!("SELECT @@session.{variable}")).await {
            Ok(r) => {
                let [value] = setup_row(r, variable)?;
                return Ok((variable, value.unwrap_or_default()));
            }
            Err(DbError::Server(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Err(DbError::Other(format!(
        "the server has neither @@session.{first} nor @@session.{second}, so dbee cannot tell whether the session is read-only"
    )))
}

/// An error the server reported leaves the session in step; any other leaves it unusable.
fn session_survives(e: &DbError) -> bool {
    matches!(e, DbError::Server(_) | DbError::Auth { .. } | DbError::Cancelled | DbError::ReadOnly { .. })
}

/// An open connection. Every operation holds the session lock for its whole run, so
/// operations never interleave. Dropping it closes the connection.
pub struct MySqlConnection {
    session: Mutex<Session>,
    info: ServerInfo,
    /// Set when the session broke, or an operation on it was abandoned half-way.
    closed: AtomicBool,
    statements: Arc<Statements>,
    canceller: Canceller,
}

impl std::fmt::Debug for MySqlConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySqlConnection").field("info", &self.info).field("closed", &self.is_closed()).finish()
    }
}

/// Marks the connection closed when dropped before [`Closes::disarm`]: an operation dropped
/// half-way leaves unread answers on the wire.
struct Closes<'a>(Option<&'a AtomicBool>);

impl Closes<'_> {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for Closes<'_> {
    fn drop(&mut self) {
        if let Some(closed) = self.0 {
            closed.store(true, Ordering::Release);
        }
    }
}

impl MySqlConnection {
    /// What the server said about itself.
    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// The connection has ended; every further operation fails with
    /// [`DbError::ConnectionLost`].
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Cancels whatever statement runs on this connection at the time of the call.
    pub fn canceller(&self) -> Canceller {
        self.canceller.clone()
    }

    /// The session, for one operation. Refuses with [`DbError::ConnectionLost`] once the
    /// connection is closed, checked after the lock is taken.
    async fn lock(&self) -> Result<MutexGuard<'_, Session>, DbError> {
        let guard = self.session.lock().await;
        if self.is_closed() {
            return Err(DbError::ConnectionLost);
        }
        Ok(guard)
    }

    /// Run `sql` as one statement (`COM_QUERY`), within [`QUERY_LIMITS`]. A statement that a
    /// cancel's `KILL QUERY` targeted and that still returned rows (`SELECT SLEEP(…)` returns 1
    /// when killed) is [`DbError::Cancelled`].
    pub async fn query_text(&self, sql: &str) -> Result<TextResult, DbError> {
        let mut session = self.lock().await?;
        let n = self.statements.begin().await;
        let closes = Closes(Some(&self.closed));
        let result = session.query(sql).await;
        let killed = self.statements.end(n);
        closes.disarm();
        match result {
            Ok(r) if killed && !r.columns.is_empty() => Err(DbError::Cancelled),
            Ok(r) => Ok(r),
            Err(e) => {
                if !session_survives(&e) {
                    self.closed.store(true, Ordering::Release);
                }
                Err(e)
            }
        }
    }
}
