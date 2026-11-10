//! Opening a connection: TCP or an SSH tunnel channel, TLS, the login and the session setup; the
//! connection, and the canceller that sends `KILL QUERY` over a second connection.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dbee_config::{ALLOW_PUBLIC_KEY, ConnectionConfig, SslMode};
use dbee_core::ResultSet;
use dbee_driver::{
    CONNECT_TIMEOUT, DbError, Limits, MAX_MESSAGE, MAX_VERSION, Purpose, QUERY_LIMITS, ServerInfo, Target, connect_first, kept_text, tls, within,
};
use dbee_ssh::Tunnel;
use mysql_common::constants::{CapabilityFlags, Command, MariadbCapabilities, StatusFlags};
use mysql_common::packets::CommonOkPacket;
use rustls::pki_types::ServerName;
use rustls::{AlertDescription, ClientConfig};
use tokio::sync::{Mutex, MutexGuard};

use crate::auth::{self, Login, ServerKey};
use crate::cancel::{Canceller, KillRoute, Statements};
use crate::errors;
use crate::flavor::{Flavor, shown_version, version_num, version_warning};
use crate::result::{self, ModeReport, Said};
use crate::wire::{MAX_PAYLOAD, Packets, Stream};

/// The largest payload dbee reads before the login is done.
const LOGIN_MAX_PAYLOAD: usize = 64 * 1024;

/// The browse connection's mode: strict values, and backslashes as plain characters in strings.
/// Works when the server's mode is empty.
const BROWSE_SQL_MODE: &str = "SET SESSION sql_mode = TRIM(BOTH ',' FROM CONCAT(@@SESSION.sql_mode, ',STRICT_ALL_TABLES,NO_BACKSLASH_ESCAPES'))";

/// The browse connection commits each statement outside an apply, whatever the server's default.
const BROWSE_AUTOCOMMIT: &str = "SET SESSION autocommit = 1";

/// Has the server report, in each OK packet, the database a statement made current.
const TRACK_SCHEMA: &str = "SET SESSION session_track_schema = ON";

/// Why a TLS handshake failed that the server ended with a handshake-failure or an
/// insufficient-security alert (ciphers), or with a protocol-version alert (versions).
const NO_SHARED_CIPHER: &str = "the server offers only TLS ciphers dbee does not support; connect through an SSH tunnel instead";
const NO_SHARED_VERSION: &str = "the server offers only TLS versions dbee does not support; connect through an SSH tunnel instead";

/// Why a read-only connection to a server that cannot report a change of read-only mode fails.
const NO_TRACKING: &str = "this server cannot report a change of read-only mode, which dbee relies on to keep a connection read-only. \
                           Connect as a database user without write rights, with Read-only off.";

/// The warning when the user allowed the server's key on a link that should have had TLS.
const UNCHECKED_KEY: &str =
    "The password was protected with a key from the server that dbee could not check, because the connection is not encrypted";

/// Where a connection goes: directly to `host:port` (to `reached` once known), or through the
/// tunnel to it. TLS checks `host`.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tunnel: Option<Arc<Tunnel>>,
    /// The address a direct connection reached, of those `host` gives: a cancel's KILL goes there,
    /// as the name may lead to another server later.
    pub(crate) reached: Option<SocketAddr>,
}

impl Route {
    /// `host:port`, with brackets around an IPv6 address; the address reached, once known.
    fn address(&self) -> String {
        match self.reached {
            Some(addr) => addr.to_string(),
            None if self.host.contains(':') => format!("[{}]:{}", self.host, self.port),
            None => format!("{}:{}", self.host, self.port),
        }
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
    pub(crate) packets: Packets,
    pub(crate) caps: CapabilityFlags,
    flavor: Flavor,
    /// MariaDB's extended metadata comes in each column definition.
    pub(crate) extended_metadata: bool,
    /// The TLS settings the session runs with; `None` when it is plain.
    tls: Option<Arc<ClientConfig>>,
    /// The password went out encrypted with the server's RSA key.
    used_server_key: bool,
    /// The server reports, in its OK packets, the database each statement makes current. Off for
    /// good once a statement of the user's names `session_track`: it may have turned the reports off.
    tracks_schema: bool,
    /// The address a direct connection reached; `None` through a tunnel.
    reached: Option<SocketAddr>,
    /// What the last command's OK packets said of read-only mode ([`crate::guard`]).
    pub(crate) mode: ModeReport,
    /// A backslash in a string escapes the character after it: the server's `sql_mode` lacks
    /// `NO_BACKSLASH_ESCAPES`, as the last OK packet's status said.
    pub(crate) backslash_escapes: bool,
    /// `"…"` is a quoted name: the session's `sql_mode` held `ANSI_QUOTES` when a read-only
    /// connection's setup read it, and a read-only session cannot change it ([`crate::guard`]).
    pub(crate) ansi_quotes: bool,
    /// The session variable that says whether transactions are read-only, as the server spells it
    /// ([`Flavor::read_only_variables`]); the session setup finds it.
    pub(crate) read_only_variable: &'static str,
    /// A transaction is open, as the last command's last OK or EOF packet said
    /// (`SERVER_STATUS_IN_TRANS`); `None` after a command that ended otherwise, an error among
    /// them, whose packet says nothing of it.
    pub(crate) in_transaction: Option<bool>,
}

impl Session {
    /// Open the stream, read the greeting, start TLS as `tls` says, and log in, with the
    /// server's RSA key as `key` allows; asking for `CLIENT_SESSION_TRACK` when `session_track`
    /// says so (the query connection's and a read-only one's do).
    pub(crate) async fn open(route: &Route, who: &Login<'_>, tls: &TlsUse, key: ServerKey, session_track: bool) -> Result<Session, DbError> {
        Session::open_asking(route, who, tls, key, auth::MARIADB_WANTED, session_track).await
    }

    /// [`Session::open`], asking a MariaDB server for the MariaDB capabilities in `wanted` it
    /// offers.
    pub(crate) async fn open_asking(
        route: &Route,
        who: &Login<'_>,
        tls: &TlsUse,
        key: ServerKey,
        wanted: MariadbCapabilities,
        session_track: bool,
    ) -> Result<Session, DbError> {
        let (stream, reached) = open_stream(route).await?;
        within(CONNECT_TIMEOUT, "the server", Session::start(stream, reached, route, who, tls, key, wanted, session_track)).await
    }

    /// The greeting, TLS and the login on `stream`, which reached `reached`.
    async fn start(
        stream: Stream,
        reached: Option<SocketAddr>,
        route: &Route,
        who: &Login<'_>,
        tls: &TlsUse,
        key: ServerKey,
        wanted: MariadbCapabilities,
        session_track: bool,
    ) -> Result<Session, DbError> {
        let mut packets = Packets::new(stream, LOGIN_MAX_PAYLOAD);
        let greeting = auth::parse_greeting(&packets.read().await?)?;
        let config = match tls {
            TlsUse::Off => None,
            TlsUse::Prefer(config) => greeting.offers_tls().then(|| Arc::clone(config)),
            TlsUse::Require(_) if !greeting.offers_tls() => return Err(DbError::Tls("the server does not offer TLS".into())),
            TlsUse::Require(config) => Some(Arc::clone(config)),
        };
        let caps = auth::capabilities(&greeting, config.is_some(), who.database.is_some(), session_track)?;
        let mariadb = auth::mariadb_capabilities(&greeting, wanted);
        if let Some(config) = &config {
            packets.write(&auth::ssl_request(caps, mariadb)).await?;
            packets = start_tls(packets, Arc::clone(config), &route.host).await?;
        }
        let used_server_key = auth::login(&mut packets, &greeting, who, caps, mariadb, config.is_some(), key).await?;
        packets.set_max(MAX_PAYLOAD);
        let extended_metadata = mariadb.contains(MariadbCapabilities::MARIADB_CLIENT_EXTENDED_METADATA);
        let flavor = Flavor::from_version(&greeting.version);
        let [read_only_variable, _] = flavor.read_only_variables();
        Ok(Session {
            packets,
            caps,
            flavor,
            extended_metadata,
            tls: config,
            used_server_key,
            tracks_schema: false,
            reached,
            mode: ModeReport::Silent,
            backslash_escapes: true,
            ansi_quotes: false,
            read_only_variable,
            in_transaction: None,
        })
    }

    /// Run `sql` as one `COM_QUERY` and read the answer, within `limits`.
    pub(crate) async fn query(&mut self, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        if names_session_track(sql) {
            self.tracks_schema = false;
        }
        self.packets.reset();
        let mut command = Vec::with_capacity(sql.len().saturating_add(1));
        command.push(Command::COM_QUERY as u8);
        command.extend_from_slice(sql.as_bytes());
        self.packets.write(&command).await?;
        let mut said = Said::default();
        let answer =
            result::read_result(&mut self.packets, self.caps, self.flavor, self.extended_metadata, self.tracks_schema, limits, &mut said).await;
        self.mode = said.mode;
        if let Some(off) = said.no_backslash_escapes {
            self.backslash_escapes = !off;
        }
        self.in_transaction = if answer.is_ok() { said.in_transaction } else { None };
        answer
    }

    /// Whether a transaction is open, as the server says in its answer to `COM_PING`, an OK packet
    /// with its status. Runs no statement.
    pub(crate) async fn ping(&mut self) -> Result<bool, DbError> {
        self.packets.reset();
        self.packets.write(&[Command::COM_PING as u8]).await?;
        let answer = self.packets.read().await?;
        let in_transaction = match answer.first() {
            Some(0x00) => result::parse_ok::<CommonOkPacket>(&answer, self.caps)?.status_flags().contains(StatusFlags::SERVER_STATUS_IN_TRANS),
            Some(0xFF) => return Err(errors::from_err_packet(&answer, self.caps, self.flavor)),
            _ => return Err(DbError::Other("the server answered a ping with something other than an OK packet".into())),
        };
        self.in_transaction = Some(in_transaction);
        Ok(in_transaction)
    }

    /// Say goodbye (`COM_QUIT`); the server closes the connection.
    pub(crate) async fn quit(mut self) {
        self.packets.reset();
        let _ = self.packets.write(&[Command::COM_QUIT as u8]).await;
    }
}

/// `sql` names `session_track` in any case: it may change what the server reports, so the
/// session stops trusting its reports of the database, and a read-only connection does not send
/// it ([`crate::guard`]).
pub(crate) fn names_session_track(sql: &str) -> bool {
    const NAME: &[u8] = b"session_track";
    sql.as_bytes().windows(NAME.len()).any(|w| w.eq_ignore_ascii_case(NAME))
}

/// The stream to the server, and the address it reached when it is direct: the address already
/// reached, or each the host name gives in turn, each within [`CONNECT_TIMEOUT`] as libpq's
/// `connect_timeout`.
async fn open_stream(route: &Route) -> Result<(Stream, Option<SocketAddr>), DbError> {
    if let Some(tunnel) = &route.tunnel {
        let channel = within(CONNECT_TIMEOUT, "the SSH tunnel's channel", async { Ok(tunnel.connect(&route.host, route.port).await?) }).await?;
        return Ok((Box::new(channel), None));
    }
    let addrs: Vec<SocketAddr> = match route.reached {
        Some(addr) => vec![addr],
        None => tokio::net::lookup_host((route.host.as_str(), route.port)).await.map_err(|e| DbError::Connect(e.to_string()))?.collect(),
    };
    // Without the address it reached, a cancel's KILL would look the name up again.
    let (tcp, reached) = connect_first(&addrs, CONNECT_TIMEOUT).await?;
    Ok((Box::new(tcp), Some(reached)))
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
        Some(rustls::Error::AlertReceived(AlertDescription::HandshakeFailure | AlertDescription::InsufficientSecurity)) => {
            DbError::Tls(NO_SHARED_CIPHER.into())
        }
        Some(rustls::Error::AlertReceived(AlertDescription::ProtocolVersion)) => DbError::Tls(NO_SHARED_VERSION.into()),
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

/// Whether the login may use the server's RSA key on a link without TLS: yes when the user chose
/// no encryption or the tunnel gives it; when TLS was wanted (`prefer` ending up plain on a
/// direct connection, which an attacker on the link can force) only with the option on.
fn server_key(cfg: &ConnectionConfig, tunnel: bool) -> ServerKey {
    if tunnel || cfg.ssl_mode == SslMode::Disable {
        ServerKey::Allowed
    } else if cfg.option_on(ALLOW_PUBLIC_KEY.id) {
        ServerKey::AllowedWithWarning
    } else {
        ServerKey::Refused
    }
}

/// Open a connection on the caller's tokio runtime.
///
/// After the login (as `utf8mb4_general_ci`, with `CLIENT_FOUND_ROWS`, and on a query connection
/// `CLIENT_SESSION_TRACK` when offered), the session runs `SET NAMES utf8mb4`; a browse
/// connection also makes its `sql_mode` strict, turns backslash escapes off and autocommit on; a
/// query connection turns `session_track_schema` on where the server can report it (it then
/// says in each OK packet which database a statement made current; a server without it leaves
/// that untold, as does the session after a statement of the user's that names `session_track`);
/// a read-only connection runs `SET SESSION TRANSACTION READ ONLY`, which the flavor's variable
/// must then confirm, has the server report each change of that variable and of the
/// transaction's characteristics (`CLIENT_SESSION_TRACK`; a server that cannot is refused), and
/// reads whether the session's `sql_mode` holds `ANSI_QUOTES`. In
/// `prefer` mode a failed TLS handshake is tried again without TLS, as libpq does.
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
    let route = Route { host: cfg.host.clone(), port: cfg.port, tunnel: t.tunnel.clone(), reached: None };
    let password = t.password.unwrap_or_default();
    let who = Login { user: &cfg.user, password: password.as_bytes(), database: Some(cfg.database.as_str()).filter(|d| !d.is_empty()) };
    let key = server_key(cfg, t.tunnel.is_some());
    let (session, mut info, connection_id) = match attempt(&route, &who, &tls, key, cfg.read_only, t.purpose).await {
        Err(DbError::Tls(_)) if matches!(tls, TlsUse::Prefer(_)) => {
            log::info!("mysql: the TLS handshake failed in prefer mode; connecting without TLS");
            attempt(&route, &who, &TlsUse::Off, key, cfg.read_only, t.purpose).await
        }
        other => other,
    }?;
    let on = session.tls.is_some();
    info.tls = Some(tls::state(on, cfg.ssl_mode, cfg.ca_file.as_deref()));
    info.local = session.reached.is_some_and(tls::is_local);
    info.warnings.extend(tls::prefer_warning(cfg.ssl_mode, on, session.reached));
    if session.used_server_key && key == ServerKey::AllowedWithWarning {
        info.warnings.push(UNCHECKED_KEY.into());
    }
    let statements = Arc::new(Statements::default());
    let tls_used = session.tls.clone().map_or(TlsUse::Off, TlsUse::Require);
    let canceller = Canceller(Arc::new(KillRoute {
        route: Route { reached: session.reached, ..route },
        user: cfg.user.clone(),
        password: password.to_owned(),
        tls: tls_used,
        server_key: key,
        connection_id,
        statements: Arc::clone(&statements),
    }));
    let database = who.database.map(str::to_owned);
    Ok(MySqlConnection {
        flavor: session.flavor,
        session: Mutex::new(session),
        info,
        database,
        closed: AtomicBool::new(false),
        statements,
        canceller,
        read_only: cfg.read_only,
        check_due: AtomicBool::new(false),
    })
}

async fn attempt(
    route: &Route,
    who: &Login<'_>,
    tls: &TlsUse,
    key: ServerKey,
    read_only: bool,
    purpose: Purpose,
) -> Result<(Session, ServerInfo, u64), DbError> {
    let addr = route.address();
    log::info!("mysql: connecting to {addr}{}", if route.tunnel.is_some() { " through SSH" } else { "" });
    let opening = async {
        let mut session = Session::open(route, who, tls, key, purpose == Purpose::Query || read_only).await?;
        let (info, id) = within(CONNECT_TIMEOUT, "the server", setup(&mut session, read_only, purpose)).await?;
        Ok((session, info, id))
    };
    match opening.await {
        Ok((session, info, id)) => {
            log::info!(
                "mysql: connected to {addr} ({} {}, TLS {})",
                info.product,
                info.version_num,
                if session.tls.is_some() { "on" } else { "off" }
            );
            Ok((session, info, id))
        }
        Err(e) => {
            let e = while_connecting(e);
            log::info!("mysql: connecting to {addr} failed ({})", e.code());
            Err(match e {
                DbError::Connect(m) => DbError::Connect(format!("{addr}: {m}")),
                other => other,
            })
        }
    }
}

/// The first row's cells of a setup query, `n` of them, each kept to [`MAX_MESSAGE`].
fn setup_row<const N: usize>(r: ResultSet, what: &str) -> Result<[Option<String>; N], DbError> {
    r.rows
        .into_iter()
        .next()
        .and_then(|row| <[Option<String>; N]>::try_from(row).ok())
        .map(|cells| cells.map(|cell| cell.map(|text| kept_text(&text, MAX_MESSAGE))))
        .ok_or_else(|| DbError::Other(format!("the server sent no {what}")))
}

/// The session setup; returns what the server says about itself, and the connection id.
async fn setup(session: &mut Session, read_only: bool, purpose: Purpose) -> Result<(ServerInfo, u64), DbError> {
    session.query("SET NAMES utf8mb4", QUERY_LIMITS).await?;
    if purpose == Purpose::Browse {
        session.query(BROWSE_SQL_MODE, QUERY_LIMITS).await?;
        session.query(BROWSE_AUTOCOMMIT, QUERY_LIMITS).await?;
    }
    if read_only {
        session.query("SET SESSION TRANSACTION READ ONLY", QUERY_LIMITS).await?;
    }
    let [id, version] = setup_row(session.query("SELECT CONNECTION_ID(), @@version", QUERY_LIMITS).await?, "version")?;
    let version = kept_text(&version.unwrap_or_default(), MAX_VERSION);
    let flavor = Flavor::from_version(&version);
    session.flavor = flavor;
    let version_num =
        version_num(shown_version(&version)).ok_or_else(|| DbError::Other(format!("the server sent an unreadable version ({version:?})")))?;
    let connection_id =
        id.and_then(|id| id.parse::<u64>().ok()).ok_or_else(|| DbError::Other("the server sent an unreadable connection id".into()))?;
    let (variable, value) = read_only_state(session, flavor).await?;
    session.read_only_variable = variable;
    let server_read_only = value == "1";
    if read_only && !server_read_only {
        return Err(DbError::Other(format!("read-only mode did not stick (@@session.{variable} is {value:?})")));
    }
    if read_only {
        track_read_only(session, variable).await?;
        let [mode] = setup_row(session.query("SELECT @@SESSION.sql_mode", QUERY_LIMITS).await?, "sql_mode")?;
        session.ansi_quotes = mode.unwrap_or_default().split(',').any(|m| m.eq_ignore_ascii_case("ANSI_QUOTES"));
    }
    // Last, as a statement that names `session_track`, the read-only setup's too, ends the trust in the reports.
    if purpose == Purpose::Query && session.caps.contains(CapabilityFlags::CLIENT_SESSION_TRACK) {
        session.tracks_schema = match session.query(TRACK_SCHEMA, QUERY_LIMITS).await {
            Ok(_) => true,
            Err(DbError::Server(_)) => {
                log::info!("mysql: the server cannot report the current database; dbee leaves it untold");
                false
            }
            Err(e) => return Err(e),
        };
    }
    let warnings = version_warning(flavor, version_num).into_iter().collect();
    let version = shown_version(&version).to_owned();
    Ok((
        ServerInfo { product: flavor.product().into(), version, version_num, read_only: server_read_only, warnings, tls: None, local: false },
        connection_id,
    ))
}

/// The statements that have the server report, in each OK packet, a change of the read-only
/// `variable` and of the transaction's characteristics, which a `SET` of the variable without
/// `SESSION` and `START TRANSACTION READ WRITE` change.
fn tracking(variable: &str) -> [String; 2] {
    [format!("SET SESSION session_track_system_variables = '{variable}'"), "SET SESSION session_track_transaction_info = 'CHARACTERISTICS'".into()]
}

/// Has the server report a change of read-only mode ([`tracking`]).
async fn track_read_only(session: &mut Session, variable: &str) -> Result<(), DbError> {
    if !session.caps.contains(CapabilityFlags::CLIENT_SESSION_TRACK | CapabilityFlags::CLIENT_DEPRECATE_EOF) {
        return Err(DbError::Other(NO_TRACKING.into()));
    }
    for sql in tracking(variable) {
        session.query(&sql, QUERY_LIMITS).await?;
    }
    Ok(())
}

/// The flavor's read-only variable and its value; the other spelling when the server has no
/// such variable.
async fn read_only_state(session: &mut Session, flavor: Flavor) -> Result<(&'static str, String), DbError> {
    let [first, second] = flavor.read_only_variables();
    for variable in [first, second] {
        match session.query(&format!("SELECT @@session.{variable}"), QUERY_LIMITS).await {
            Ok(r) => {
                let [value] = setup_row(r, variable)?;
                return Ok((variable, value.unwrap_or_default()));
            }
            Err(e) if errors::is_errno(&e, errors::ER_UNKNOWN_SYSTEM_VARIABLE) => {}
            Err(e) => return Err(e),
        }
    }
    Err(DbError::Other(format!(
        "the server has neither @@session.{first} nor @@session.{second}, so dbee cannot tell whether the session is read-only"
    )))
}

/// An open connection. Every operation holds the session lock for its whole run, so
/// operations never interleave. Dropping it closes the connection.
pub struct MySqlConnection {
    session: Mutex<Session>,
    info: ServerInfo,
    flavor: Flavor,
    /// The database the settings name, if any: the catalog lists only it.
    pub(crate) database: Option<String>,
    /// Set when the session broke, or an operation on it was abandoned half-way.
    closed: AtomicBool,
    pub(crate) statements: Arc<Statements>,
    canceller: Canceller,
    /// dbee made the session read-only, and checks each statement and the server's report of it
    /// ([`crate::guard`]).
    pub(crate) read_only: bool,
    /// The mode is read before the next statement: the last one ended in an error, which carries
    /// no report, or a cancel stopped the read.
    pub(crate) check_due: AtomicBool,
}

impl std::fmt::Debug for MySqlConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MySqlConnection").field("info", &self.info).field("closed", &self.is_closed()).finish()
    }
}

impl MySqlConnection {
    /// What the server said about itself.
    pub fn info(&self) -> &ServerInfo {
        &self.info
    }

    /// Which member of the MySQL family the connection reached.
    pub(crate) fn flavor(&self) -> Flavor {
        self.flavor
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
    /// connection is closed, checked after the lock is taken, so an operation queued behind an
    /// abandoned apply cannot run inside its transaction.
    pub(crate) async fn lock(&self) -> Result<MutexGuard<'_, Session>, DbError> {
        let guard = self.session.lock().await;
        if self.is_closed() {
            return Err(DbError::ConnectionLost);
        }
        Ok(guard)
    }

    /// Mark the session unusable: it broke, or an operation on it was abandoned half-way. Every
    /// later operation is refused with [`DbError::ConnectionLost`]; the socket stays open until
    /// the connection is dropped.
    pub(crate) fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    /// Sets the session's tracking up again as the setup left it, after a statement that turned
    /// read-only off, and may have turned the tracking off with it: read-only mode's, then the
    /// current database's where the session told it.
    pub(crate) async fn track_again(&self, session: &mut Session) -> Result<(), DbError> {
        let schema = session.tracks_schema;
        for sql in tracking(session.read_only_variable) {
            self.statement(session, &sql, QUERY_LIMITS).await?;
        }
        if schema {
            self.statement(session, TRACK_SCHEMA, QUERY_LIMITS).await?;
            session.tracks_schema = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use dbee_core::DatabaseAfter;

    use super::*;
    use crate::result::tests::{TRACKING, ok_with_state, schema_entry};
    use crate::wire::tests::{frame, packets};

    /// A session that tracks the database, whose server answers each query with an OK packet
    /// saying it made `many` current.
    fn tracking_session(queries: usize) -> Session {
        let input = (0..queries).map(|_| frame(&ok_with_state(&schema_entry("many")), 1)).collect::<Vec<_>>().concat();
        let (packets, _) = packets(input);
        Session {
            packets,
            caps: TRACKING,
            flavor: Flavor::MySql,
            extended_metadata: false,
            tls: None,
            used_server_key: false,
            tracks_schema: true,
            reached: None,
            mode: ModeReport::Silent,
            backslash_escapes: true,
            ansi_quotes: false,
            read_only_variable: "transaction_read_only",
            in_transaction: None,
        }
    }

    #[tokio::test]
    async fn a_run_that_names_session_track_ends_the_trust_for_the_connections_life() {
        let mut s = tracking_session(4);
        let moved = DatabaseAfter::Changed(Some("many".into()));
        assert_eq!(s.query("USE many", QUERY_LIMITS).await.unwrap().database_after, moved);
        // It may have turned the tracker off: the server's word no longer counts, for this run and
        // every later one, in any case.
        assert_eq!(s.query("SET SESSION_TRACK_SCHEMA = OFF", QUERY_LIMITS).await.unwrap().database_after, DatabaseAfter::Untold);
        assert_eq!(s.query("USE many", QUERY_LIMITS).await.unwrap().database_after, DatabaseAfter::Untold);
        assert_eq!(s.query("SELECT 1", QUERY_LIMITS).await.unwrap().database_after, DatabaseAfter::Untold);
        // A new connection is trusted again.
        let mut fresh = tracking_session(1);
        assert_eq!(fresh.query("USE many", QUERY_LIMITS).await.unwrap().database_after, moved);
    }

    /// The query connection asks for session tracking to learn the current database, and both
    /// connections of a read-only one ask to learn of a change of read-only mode.
    #[tokio::test]
    async fn the_query_connection_and_a_read_only_one_ask_for_session_tracking() {
        let db = dbee_testkit::require_testdb!();
        for s in db.servers_of(&[dbee_testkit::Kind::MySql, dbee_testkit::Kind::MariaDb]) {
            let kind = dbee_config::DbKind::from_id(s.kind.id()).unwrap();
            let config = ConnectionConfig { kind, host: "127.0.0.1".into(), port: s.port, user: "root".into(), ..ConnectionConfig::default() };
            let read_only = ConnectionConfig { read_only: true, ..config.clone() };
            let mut asked = Vec::new();
            for (config, purpose) in
                [(&config, Purpose::Browse), (&config, Purpose::Query), (&read_only, Purpose::Browse), (&read_only, Purpose::Query)]
            {
                let target = Target { config, password: Some(&db.password), tunnel: None, purpose, create_file: false, allowed_dirs: None };
                let conn = connect(target).await.unwrap();
                let session = conn.session.lock().await;
                asked.push((session.caps.contains(CapabilityFlags::CLIENT_SESSION_TRACK), session.tracks_schema));
            }
            let want = [(false, false), (true, true), (true, false), (true, true)];
            assert_eq!(asked, want, "{}: (asked for, the database read) on the browse and the query connection, then read-only", s.name);
        }
    }

    #[test]
    fn session_track_is_found_in_any_case_and_nowhere_else() {
        for sql in ["SET SESSION session_track_schema = OFF", "set @@SESSION_TRACK_SCHEMA = 0", "select 1 -- Session_Track"] {
            assert!(names_session_track(sql), "{sql}");
        }
        for sql in ["", "SELECT 1", "session track", "session_trac", "sessiön_track"] {
            assert!(!names_session_track(sql), "{sql}");
        }
    }

    #[test]
    fn the_servers_key_follows_the_tls_mode_the_tunnel_and_the_option() {
        let with = |ssl_mode, allow: bool| {
            let mut c = ConnectionConfig { kind: dbee_config::DbKind::MySql, ssl_mode, ..ConnectionConfig::default() };
            if allow {
                c.options.insert(ALLOW_PUBLIC_KEY.id.into(), "true".into());
            }
            c
        };
        assert_eq!(server_key(&with(SslMode::Disable, false), false), ServerKey::Allowed);
        assert_eq!(server_key(&with(SslMode::Prefer, false), true), ServerKey::Allowed);
        assert_eq!(server_key(&with(SslMode::Prefer, false), false), ServerKey::Refused);
        assert_eq!(server_key(&with(SslMode::Prefer, true), false), ServerKey::AllowedWithWarning);
        // require and verify-* always have TLS; should one ever be plain, the key is refused.
        assert_eq!(server_key(&with(SslMode::Require, false), false), ServerKey::Refused);
    }

    #[test]
    fn tls_alerts_say_what_the_server_lacks() {
        let alert = |a| tls_error(&std::io::Error::new(std::io::ErrorKind::InvalidData, rustls::Error::AlertReceived(a)));
        assert_eq!(alert(AlertDescription::HandshakeFailure), DbError::Tls(NO_SHARED_CIPHER.into()));
        assert_eq!(alert(AlertDescription::InsufficientSecurity), DbError::Tls(NO_SHARED_CIPHER.into()));
        let versions = alert(AlertDescription::ProtocolVersion);
        assert_eq!(versions, DbError::Tls(NO_SHARED_VERSION.into()));
        assert!(versions.to_string().contains("TLS versions") && versions.to_string().contains("SSH tunnel"), "{versions}");
        assert_eq!(alert(AlertDescription::BadCertificate).code(), "tls");
    }

    /// A route that knows the address reached connects there, without looking its name up.
    #[tokio::test]
    async fn a_route_with_an_address_reached_connects_there() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let route = Route { host: "nohost.invalid".into(), port: addr.port(), tunnel: None, reached: Some(addr) };
        let ((_stream, reached), accepted) = tokio::join!(async { open_stream(&route).await.unwrap() }, listener.accept());
        assert_eq!(reached, Some(addr));
        assert!(accepted.is_ok(), "the listener took it");
        // Without it, the name is looked up: this one has no address.
        assert_eq!(open_stream(&Route { reached: None, ..route }).await.err().map(|e| e.code()), Some("connect"));
    }

    /// The KILL goes to the address the connection reached, not to whichever its host name gives
    /// first when it is sent: here `localhost` names `::1` and `127.0.0.1`, and the servers listen
    /// on `127.0.0.1` only.
    #[tokio::test]
    async fn the_kill_goes_to_the_address_the_connection_reached() {
        let db = dbee_testkit::require_testdb!();
        for s in db.servers_of(&[dbee_testkit::Kind::MySql, dbee_testkit::Kind::MariaDb]) {
            let kind = dbee_config::DbKind::from_id(s.kind.id()).unwrap();
            let cfg = ConnectionConfig { kind, host: "localhost".into(), port: s.port, user: "root".into(), ..ConnectionConfig::default() };
            let target =
                Target { config: &cfg, password: Some(&db.password), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
            let conn = connect(target).await.unwrap_or_else(|e| panic!("{}: {e}", s.name));
            assert_eq!(conn.canceller.0.route.address(), format!("127.0.0.1:{}", s.port), "{}", s.name);
        }
    }
}
