//! Opening a connection: TCP or an SSH tunnel channel, TLS, the login and the session setup; the
//! connection, and the canceller that sends `KILL QUERY` over a second connection.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dbee_config::{ALLOW_PUBLIC_KEY, ConnectionConfig, SslMode};
use dbee_core::ResultSet;
use dbee_driver::{CONNECT_TIMEOUT, DbError, Limits, Purpose, QUERY_LIMITS, ServerInfo, Target, tls};
use dbee_ssh::Tunnel;
use mysql_common::constants::{CapabilityFlags, Command, MariadbCapabilities};
use rustls::pki_types::ServerName;
use rustls::{AlertDescription, ClientConfig};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, MutexGuard};

use crate::auth::{self, Login, ServerKey};
use crate::cancel::{Canceller, KillRoute, Statements};
use crate::errors;
use crate::flavor::{Flavor, version_num, version_warning};
use crate::wire::{self, MAX_PAYLOAD, Packets, Stream};

/// The largest payload dbee reads before the login is done.
const LOGIN_MAX_PAYLOAD: usize = 64 * 1024;

/// The browse connection's mode: strict values, and backslashes as plain characters in strings.
/// Works when the server's mode is empty.
const BROWSE_SQL_MODE: &str = "SET SESSION sql_mode = TRIM(BOTH ',' FROM CONCAT(@@SESSION.sql_mode, ',STRICT_ALL_TABLES,NO_BACKSLASH_ESCAPES'))";

/// Why a TLS handshake failed that the server ended with a handshake-failure or an
/// insufficient-security alert (ciphers), or with a protocol-version alert (versions).
const NO_SHARED_CIPHER: &str = "the server offers only TLS ciphers dbee does not support; connect through an SSH tunnel instead";
const NO_SHARED_VERSION: &str = "the server offers only TLS versions dbee does not support; connect through an SSH tunnel instead";

/// The warning when the user allowed the server's key on a link that should have had TLS.
const UNCHECKED_KEY: &str =
    "The password was protected with a key from the server that dbee could not check, because the connection is not encrypted";

/// TCP keepalive on direct connections, as for Postgres: the first probe after this long
/// without traffic, then one this often, and the connection is dead after this many unanswered.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;

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
    /// MariaDB's extended metadata comes in each column definition.
    extended_metadata: bool,
    /// The TLS settings the session runs with; `None` when it is plain.
    tls: Option<Arc<ClientConfig>>,
    /// The password went out encrypted with the server's RSA key.
    used_server_key: bool,
}

impl Session {
    /// Open the stream, read the greeting, start TLS as `tls` says, and log in, with the
    /// server's RSA key as `key` allows.
    pub(crate) async fn open(route: &Route, who: &Login<'_>, tls: &TlsUse, key: ServerKey) -> Result<Session, DbError> {
        Session::open_asking(route, who, tls, key, auth::MARIADB_WANTED).await
    }

    /// [`Session::open`], asking a MariaDB server for the MariaDB capabilities in `wanted` it
    /// offers.
    async fn open_asking(route: &Route, who: &Login<'_>, tls: &TlsUse, key: ServerKey, wanted: MariadbCapabilities) -> Result<Session, DbError> {
        let mut packets = Packets::new(open_stream(route).await?, LOGIN_MAX_PAYLOAD);
        let greeting = auth::parse_greeting(&packets.read().await?)?;
        let config = match tls {
            TlsUse::Off => None,
            TlsUse::Prefer(config) => greeting.offers_tls().then(|| Arc::clone(config)),
            TlsUse::Require(_) if !greeting.offers_tls() => return Err(DbError::Tls("the server does not offer TLS".into())),
            TlsUse::Require(config) => Some(Arc::clone(config)),
        };
        let caps = auth::capabilities(&greeting, config.is_some(), who.database.is_some())?;
        let mariadb = auth::mariadb_capabilities(&greeting, wanted);
        if let Some(config) = &config {
            packets.write(&auth::ssl_request(caps, mariadb)).await?;
            packets = start_tls(packets, Arc::clone(config), &route.host).await?;
        }
        let used_server_key = auth::login(&mut packets, &greeting, who, caps, mariadb, config.is_some(), key).await?;
        packets.set_max(MAX_PAYLOAD);
        let extended_metadata = mariadb.contains(MariadbCapabilities::MARIADB_CLIENT_EXTENDED_METADATA);
        Ok(Session { packets, caps, flavor: Flavor::from_version(&greeting.version), extended_metadata, tls: config, used_server_key })
    }

    /// Run `sql` as one `COM_QUERY` and read the answer, within `limits`.
    pub(crate) async fn query(&mut self, sql: &str, limits: Limits) -> Result<ResultSet, DbError> {
        self.packets.reset();
        let mut command = Vec::with_capacity(sql.len().saturating_add(1));
        command.push(Command::COM_QUERY as u8);
        command.extend_from_slice(sql.as_bytes());
        self.packets.write(&command).await?;
        wire::read_result(&mut self.packets, self.caps, self.flavor, self.extended_metadata, limits).await
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
    tune(&tcp).map_err(|e| DbError::Connect(e.to_string()))?;
    Ok(Box::new(tcp))
}

/// A direct connection's socket options: no delay for small packets, and keepalive probes.
fn tune(tcp: &TcpStream) -> std::io::Result<()> {
    tcp.set_nodelay(true)?;
    let keepalive = TcpKeepalive::new().with_time(KEEPALIVE_IDLE).with_interval(KEEPALIVE_INTERVAL).with_retries(KEEPALIVE_RETRIES);
    SockRef::from(tcp).set_tcp_keepalive(&keepalive)
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
    let key = server_key(cfg, t.tunnel.is_some());
    let (session, mut info, connection_id) = match attempt(&route, &who, &tls, key, cfg.read_only, t.purpose).await {
        Err(DbError::Tls(_)) if matches!(tls, TlsUse::Prefer(_)) => {
            log::info!("mysql: the TLS handshake failed in prefer mode; connecting without TLS");
            attempt(&route, &who, &TlsUse::Off, key, cfg.read_only, t.purpose).await
        }
        other => other,
    }?;
    if session.used_server_key && key == ServerKey::AllowedWithWarning {
        info.warnings.push(UNCHECKED_KEY.into());
    }
    let statements = Arc::new(Statements::default());
    let tls_used = session.tls.clone().map_or(TlsUse::Off, TlsUse::Require);
    let canceller = Canceller(Arc::new(KillRoute {
        route,
        user: cfg.user.clone(),
        password: password.to_owned(),
        tls: tls_used,
        server_key: key,
        connection_id,
        statements: Arc::clone(&statements),
    }));
    Ok(MySqlConnection { session: Mutex::new(session), info, closed: AtomicBool::new(false), statements, canceller })
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
        let mut session = Session::open(route, who, tls, key).await?;
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
fn setup_row<const N: usize>(r: ResultSet, what: &str) -> Result<[Option<String>; N], DbError> {
    r.rows
        .into_iter()
        .next()
        .and_then(|row| <[Option<String>; N]>::try_from(row).ok())
        .ok_or_else(|| DbError::Other(format!("the server sent no {what}")))
}

/// The session setup; returns what the server says about itself, and the connection id.
async fn setup(session: &mut Session, read_only: bool, purpose: Purpose) -> Result<(ServerInfo, u64), DbError> {
    session.query("SET NAMES utf8mb4", QUERY_LIMITS).await?;
    if purpose == Purpose::Browse {
        session.query(BROWSE_SQL_MODE, QUERY_LIMITS).await?;
    }
    if read_only {
        session.query("SET SESSION TRANSACTION READ ONLY", QUERY_LIMITS).await?;
    }
    let [id, version] = setup_row(session.query("SELECT CONNECTION_ID(), @@version", QUERY_LIMITS).await?, "version")?;
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
    /// Set when the session broke, or an operation on it was abandoned half-way.
    closed: AtomicBool,
    pub(crate) statements: Arc<Statements>,
    canceller: Canceller,
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
}

#[cfg(test)]
mod tests {
    use mysql_common::constants::{ColumnFlags, ColumnType};
    use mysql_common::io::ParseBuf;

    use super::*;

    /// The column definitions of `sql`'s result set, as `session` receives them; the rest of
    /// the answer is read and dropped.
    async fn definitions(session: &mut Session, sql: &str) -> Vec<Vec<u8>> {
        session.packets.reset();
        session.packets.write(&[&[Command::COM_QUERY as u8][..], sql.as_bytes()].concat()).await.unwrap();
        let count = session.packets.read().await.unwrap();
        assert!(count.len() == 1 && count[0] < 251, "{sql}: {count:?}");
        let mut defs = Vec::new();
        for _ in 0..count[0] {
            defs.push(session.packets.read().await.unwrap());
        }
        if !session.caps.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF) {
            session.packets.read().await.unwrap();
        }
        while !matches!(session.packets.read().await.unwrap().as_slice(), [0xFE, ..]) {}
        defs
    }

    /// The fields dbee reads of a column definition, as mysql_common reads them.
    fn mysql_common_reads(def: &[u8]) -> (String, ColumnType, ColumnFlags, u16, u32, u8) {
        let c = ParseBuf(def).parse::<mysql_common::packets::Column>(()).unwrap();
        (c.name_str().into_owned(), c.column_type(), c.flags(), c.character_set(), c.column_length(), c.decimals())
    }

    fn dbee_reads(def: &[u8], extended: bool) -> ((String, ColumnType, ColumnFlags, u16, u32, u8), bool) {
        let c = wire::parse_column(def, extended).unwrap();
        ((c.name, c.facts.column_type, c.facts.flags, c.facts.charset, c.facts.length, c.decimals), c.facts.json)
    }

    #[tokio::test]
    async fn column_definitions_read_as_mysql_common_reads_them() {
        let db = dbee_testkit::require_testdb!();
        let queries = [
            "SELECT * FROM shop.all_types",
            "SELECT CAST(1.50 AS DECIMAL(10,2)) n, NOW(3) t, NULL z, '' e, CAST(X'00FF' AS BINARY) b, 1.5e0 f, JSON_OBJECT('a', 1) j, \
             JSON_EXTRACT('[1]', '$[0]') x",
            "SELECT * FROM odd.`Odd \"Name\" é`",
        ];
        for s in &db.mysql {
            let route = Route { host: "127.0.0.1".into(), port: s.port, tunnel: None };
            let who = Login { user: "root", password: db.password.as_bytes(), database: None };
            let mut plain = Session::open_asking(&route, &who, &TlsUse::Off, ServerKey::Allowed, MariadbCapabilities::empty()).await.unwrap();
            let mut extended = Session::open(&route, &who, &TlsUse::Off, ServerKey::Allowed).await.unwrap();
            let mariadb = s.flavor == dbee_testkit::MyFlavor::MariaDb;
            assert_eq!((plain.extended_metadata, extended.extended_metadata), (false, mariadb), "{}", s.name);
            for sql in queries {
                let (before, now) = (definitions(&mut plain, sql).await, definitions(&mut extended, sql).await);
                assert_eq!(before.len(), now.len(), "{}: {sql}", s.name);
                let mut json = Vec::new();
                for (b, n) in before.iter().zip(&now) {
                    let theirs = mysql_common_reads(b);
                    assert_eq!(dbee_reads(b, false), (theirs.clone(), false), "{}: {sql}", s.name);
                    let (ours, is_json) = dbee_reads(n, extended.extended_metadata);
                    assert_eq!(ours, theirs, "{}: {sql}", s.name);
                    if is_json {
                        json.push(ours.0);
                    }
                }
                // MariaDB names its JSON in the extended metadata; MySQL has a JSON type.
                let expected: &[&str] = match sql {
                    _ if !mariadb => &[],
                    "SELECT * FROM shop.all_types" => &["doc"],
                    q if q.contains("JSON_OBJECT") => &["j", "x"],
                    _ => &[],
                };
                assert_eq!(json, expected, "{}: {sql}", s.name);
            }
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

    // socket2 reads the keepalive times back on Unix only.
    #[cfg(not(windows))]
    #[tokio::test]
    async fn direct_connections_have_keepalive() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        tune(&tcp).unwrap();
        let sock = socket2::SockRef::from(&tcp);
        assert!(tcp.nodelay().unwrap() && sock.keepalive().unwrap());
        let set = (sock.tcp_keepalive_time().unwrap(), sock.tcp_keepalive_interval().unwrap(), sock.tcp_keepalive_retries().unwrap());
        assert_eq!(set, (KEEPALIVE_IDLE, KEEPALIVE_INTERVAL, KEEPALIVE_RETRIES));
    }
}
