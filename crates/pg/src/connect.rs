//! Opening a connection: TCP or an SSH tunnel channel, TLS, login, the session setup, and the
//! cancel request.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use dbee_config::ConnectionConfig;
use dbee_driver::{CONNECT_TIMEOUT, DbError, ServerInfo, Target, cancel_not_delivered};
use dbee_ssh::Tunnel;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::runtime::Handle;
use tokio::sync::{Mutex, MutexGuard};
use tokio_postgres::tls::MakeTlsConnect;
use tokio_postgres::{CancelToken, Client, Connection, SimpleQueryMessage};

use crate::error::{self, Phase};
use crate::tls::{self, Tls};

/// The oldest supported server, as `server_version_num`: Postgres 14.
pub const MIN_VERSION_NUM: u32 = 140000;

/// [`ServerInfo::product`] for every Postgres server.
const PRODUCT: &str = "PostgreSQL";

/// TCP keepalive on direct connections: the first probe after this long without traffic.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
/// Then a probe this often, and the connection is dead after [`KEEPALIVE_RETRIES`] unanswered.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const KEEPALIVE_RETRIES: u32 = 3;

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
    /// dbee made the session read-only, and reads the mode back after each statement
    /// ([`crate::guard`]).
    pub(crate) read_only: bool,
    /// The mode could not be read after the last statement; it is read before the next one.
    pub(crate) check_due: AtomicBool,
    /// The schemas the session's search path finds, read when it opened: a type in one of them is
    /// shown without its schema.
    visible_schemas: Vec<String>,
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

    /// The schemas the session's search path found when it opened (`current_schemas(false)`).
    pub fn visible_schemas(&self) -> &[String] {
        &self.visible_schemas
    }

    /// The connection has ended: closed by the server, the network or the tunnel. Every further
    /// operation fails with [`DbError::ConnectionLost`].
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
    /// [`DbError::ConnectionLost`] once the connection is closed, checked after the mutex is
    /// taken so an operation queued behind an abandoned apply cannot run inside its transaction.
    /// A cancel request on its way is waited for first ([`Canceller::cancel`]), so it reaches the
    /// statement it was sent for, or none.
    pub(crate) async fn lock(&self) -> Result<MutexGuard<'_, ()>, DbError> {
        let guard = self.lock.lock().await;
        drop(self.canceller.on_its_way.lock().await);
        if self.is_closed() {
            return Err(DbError::ConnectionLost);
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
    pub(crate) fn error(&self, e: &tokio_postgres::Error) -> DbError {
        let mapped = error::map(e, Phase::Session);
        if mapped == DbError::ConnectionLost {
            self.closed.store(true, Ordering::Release);
        }
        mapped
    }
}

/// The server the connection reached, for a cancel request: the way there, and `host`, the name
/// TLS checks.
#[derive(Clone)]
struct Route {
    host: String,
    way: Way,
}

/// How a cancel request reaches the server the connection reached.
#[derive(Clone)]
enum Way {
    /// A new channel through the tunnel, to the host and port the connection asked it for.
    Tunnel { tunnel: Arc<Tunnel>, port: u16 },
    /// The address that answered, of those the host name gave: the name may lead elsewhere later.
    Tcp(SocketAddr),
    /// The server's socket in the folder the host names.
    #[cfg(unix)]
    Socket(std::path::PathBuf),
}

/// Sends a cancel request for the statement running on one connection. Cheap to clone; holds
/// none of the connection's locks.
#[derive(Clone)]
pub struct Canceller {
    token: CancelToken,
    tls: Tls,
    route: Route,
    /// Held while a request is on its way; no operation takes the connection's lock meanwhile
    /// ([`PgConnection::lock`]). An Apply that holds the lock already runs its next statement.
    on_its_way: Arc<Mutex<()>>,
}

impl std::fmt::Debug for Canceller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token holds the backend's secret key: not shown.
        f.debug_struct("Canceller").field("tls", &self.tls).field("tunnel", &matches!(self.route.way, Way::Tunnel { .. })).finish()
    }
}

impl Canceller {
    /// Ask the server to cancel the running statement, over a new connection: a new channel
    /// through the tunnel, or a direct connection to the address the connection reached. `Ok`
    /// means the request was handled: the server closed that connection, as it does once it has
    /// (the protocol sends no answer); the statement then fails with [`DbError::Cancelled`], unless
    /// it finished first. Until then no operation takes the connection's lock
    /// ([`PgConnection::lock`]); an Apply already holding it runs its next statement.
    pub async fn cancel(&self) -> Result<(), DbError> {
        // The timeout below needs a runtime; without one it would panic.
        tokio::runtime::Handle::try_current().map_err(|_| DbError::Other("cancelling needs a tokio runtime".into()))?;
        let sending = async {
            let _on_its_way = self.on_its_way.lock().await;
            let host = &self.route.host;
            match &self.route.way {
                Way::Tunnel { tunnel, port } => self.request(tunnel.connect(host, *port).await?).await,
                Way::Tcp(addr) => {
                    let stream = tokio::net::TcpStream::connect(addr).await.map_err(|e| DbError::Connect(format!("{addr}: {e}")))?;
                    self.request(stream).await
                }
                #[cfg(unix)]
                Way::Socket(socket) => {
                    let stream = tokio::net::UnixStream::connect(socket).await.map_err(|e| DbError::Connect(format!("{host}: {e}")))?;
                    self.request(stream).await
                }
            }
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, sending).await {
            Ok(result) => {
                log::info!("pg: cancel request {}", if result.is_ok() { "sent" } else { "failed" });
                result
            }
            Err(_) => Err(cancel_not_delivered()),
        }
    }
}

impl Canceller {
    /// Sends the cancel request over `stream`, with TLS as the connection has it, and waits until
    /// the server has closed it.
    async fn request<S>(&self, stream: S) -> Result<(), DbError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let stream = UntilClosed { inner: stream, shut: false };
        let tls = tls_connect(&self.tls, &self.route.host, &stream);
        self.token.cancel_query_raw(stream, tls).await.map_err(|e| error::map(&e, Phase::Connect))
    }
}

/// A stream whose shutdown also waits until the other end has closed the connection: a cancel
/// request has been handled once the server closed its connection.
struct UntilClosed<S> {
    inner: S,
    shut: bool,
}

impl<S: AsyncRead + Unpin> AsyncRead for UntilClosed<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> AsyncWrite for UntilClosed<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if !self.shut {
            ready!(Pin::new(&mut self.inner).poll_shutdown(cx))?;
            self.shut = true;
        }
        let mut bytes = [0u8; 256];
        loop {
            let mut read = ReadBuf::new(&mut bytes);
            match ready!(Pin::new(&mut self.inner).poll_read(cx, &mut read)) {
                Ok(()) if read.filled().is_empty() => return Poll::Ready(Ok(())),
                // Whatever the server sends before it closes (a TLS alert) is not read.
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return Poll::Ready(Ok(())),
                Err(e) => return Poll::Ready(Err(e)),
            }
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
pub async fn connect(t: Target<'_>) -> Result<PgConnection, DbError> {
    let cfg = t.config;
    // The connection task needs a runtime; `tokio::spawn` would panic without one.
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| DbError::Other("connecting needs a tokio runtime".into()))?;
    cfg.validate().map_err(|e| DbError::Connect(e.to_string()))?;
    if cfg.ssh.is_some() && t.tunnel.is_none() {
        return Err(DbError::Connect("this connection goes through an SSH tunnel, and none is open".into()));
    }
    let tls = tls::make(cfg.ssl_mode, cfg.ca_file.as_deref())?;
    match attempt(&t, &tls, &runtime).await {
        Err(DbError::Tls(_)) if tls.pg_mode() == tokio_postgres::config::SslMode::Prefer => {
            log::info!("pg: the TLS handshake failed in prefer mode; connecting without TLS");
            attempt(&t, &tls.plain(), &runtime).await
        }
        other => other,
    }
}

async fn attempt(t: &Target<'_>, tls: &Tls, runtime: &Handle) -> Result<PgConnection, DbError> {
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
                DbError::Connect(m) => DbError::Connect(format!("{addr}: {m}")),
                other => other,
            })
        }
        Err(_) => Err(DbError::Connect(format!("{addr} did not answer within {} s", CONNECT_TIMEOUT.as_secs()))),
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

async fn open(t: &Target<'_>, tls: &Tls, runtime: &Handle) -> Result<PgConnection, DbError> {
    let cfg = t.config;
    let pg = pg_config(cfg, t.password, tls);
    let connect_error = |e: tokio_postgres::Error| error::map(&e, Phase::Connect);
    let (client, closed, way) = match &t.tunnel {
        Some(tunnel) => {
            let stream = tunnel.connect(&cfg.host, cfg.port).await?;
            let connect = tls_connect(tls, &cfg.host, &stream);
            let (client, connection) = pg.connect_raw(stream, connect).await.map_err(connect_error)?;
            (client, drive(runtime, connection), Way::Tunnel { tunnel: Arc::clone(tunnel), port: cfg.port })
        }
        #[cfg(unix)]
        None if cfg.host.starts_with('/') => {
            let (client, connection) = pg.connect(tls.connector.clone()).await.map_err(connect_error)?;
            let socket = std::path::Path::new(&cfg.host).join(format!(".s.PGSQL.{}", cfg.port));
            (client, drive(runtime, connection), Way::Socket(socket))
        }
        None => {
            let (client, closed, addr) = connect_tcp(&pg, tls, &cfg.host, cfg.port, runtime).await?;
            (client, closed, Way::Tcp(addr))
        }
    };
    let (info, visible_schemas) = setup(&client, cfg.read_only).await?;
    let route = Route { host: cfg.host.clone(), way };
    let canceller = Canceller { token: client.cancel_token(), tls: tls.clone(), route, on_its_way: Arc::default() };
    let check_due = AtomicBool::new(false);
    Ok(PgConnection { client, lock: Mutex::new(()), info, closed, canceller, read_only: cfg.read_only, check_due, visible_schemas })
}

/// Connects to the addresses `host` gives, in turn, as tokio-postgres does (TLS still checks
/// `host`), and says which one answered.
async fn connect_tcp(
    pg: &tokio_postgres::Config,
    tls: &Tls,
    host: &str,
    port: u16,
    runtime: &Handle,
) -> Result<(Client, Arc<AtomicBool>, SocketAddr), DbError> {
    let addrs = tokio::net::lookup_host((host, port)).await.map_err(|e| DbError::Connect(e.to_string()))?;
    let mut last = DbError::Connect("the host name gave no address".into());
    for addr in addrs {
        let mut one = pg.clone();
        one.hostaddr(addr.ip());
        match one.connect(tls.connector.clone()).await {
            Ok((client, connection)) => return Ok((client, drive(runtime, connection), addr)),
            Err(e) => last = error::map(&e, Phase::Connect),
        }
    }
    Err(last)
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
                             SHOW server_version; SHOW server_version_num; SHOW search_path";

/// The schemas of a `search_path` value, in order, as the server reads them: a quoted name
/// unquoted, an unquoted one in lowercase, whitespace outside quotes skipped.
fn search_path_schemas(path: &str) -> Vec<String> {
    let mut schemas = Vec::new();
    let (mut name, mut quoted, mut chars) = (String::new(), false, path.chars().peekable());
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                name.push('"');
            }
            '"' => quoted = !quoted,
            c if quoted => name.push(c),
            ',' => schemas.push(std::mem::take(&mut name)),
            c if c.is_whitespace() => {}
            c => name.push(c.to_ascii_lowercase()),
        }
    }
    schemas.push(name);
    schemas.retain(|s| !s.is_empty());
    schemas
}

/// The warning for a search path that names another schema before `pg_catalog` (the session's
/// own temporary schema aside), or `None`. dbee names `pg_catalog`'s functions and operators
/// itself, whatever the path, but its casts name built-in types such as `jsonb` and `uuid` as the
/// server prints them, and such a path finds that schema's first.
fn search_path_warning(path: &str) -> Option<String> {
    let schemas = search_path_schemas(path);
    let at = schemas.iter().position(|s| s == "pg_catalog")?;
    schemas.get(..at)?.iter().any(|s| s != "pg_temp").then(|| {
        format!(
            "The search path ({path}) puts other schemas before pg_catalog, so a type someone made in one of them could take the place \
             of a built-in one in the values dbee filters by and saves. Ask the database owner to put pg_catalog first in the search \
             path, or to leave it out."
        )
    })
}

/// The schemas the session's search path finds, in order.
const VISIBLE_SCHEMAS: &str = "SELECT pg_catalog.unnest(pg_catalog.current_schemas(false))";

/// The session setup; returns what the server says about itself, and the schemas its search path
/// finds.
async fn setup(client: &Client, read_only: bool) -> Result<(ServerInfo, Vec<String>), DbError> {
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
    let [ro, conforming, encoding, version, num, path] = answers.as_slice() else {
        return Err(DbError::Other(format!("the server answered {} of 6 settings", answers.len())));
    };
    // dbee quotes values into SQL text; that is safe only with both of these.
    if conforming != "on" || encoding != "UTF8" {
        return Err(DbError::Other(format!(
            "the session runs with standard_conforming_strings = {conforming} and client_encoding = {encoding}; dbee needs on and UTF8"
        )));
    }
    let version_num = num.trim().parse::<u32>().map_err(|_| DbError::Other(format!("the server sent an unreadable server_version_num ({num:?})")))?;
    let server_read_only = ro == "on";
    if read_only && !server_read_only {
        return Err(DbError::Other(format!(
            "read-only mode did not stick (default_transaction_read_only is {ro:?}); a transaction-mode connection pooler is not supported"
        )));
    }
    let warnings = version_warning(version_num).into_iter().chain(search_path_warning(path)).collect();
    let mut visible = Vec::new();
    for message in client.simple_query(VISIBLE_SCHEMAS).await.map_err(fail)? {
        if let SimpleQueryMessage::Row(row) = message {
            visible.extend(row.try_get(0).ok().flatten().map(str::to_owned));
        }
    }
    Ok((ServerInfo { product: PRODUCT.into(), version: version.clone(), version_num, read_only: server_read_only, warnings }, visible))
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::*;

    #[test]
    fn a_search_path_is_read_as_the_server_names_its_schemas() {
        assert_eq!(search_path_schemas("\"$user\", public"), ["$user", "public"]);
        assert_eq!(search_path_schemas(" A ,\"b, \"\"C\" , PG_Catalog"), ["a", "b, \"C", "pg_catalog"]);
        assert_eq!(search_path_schemas(""), Vec::<String>::new());
        assert_eq!(search_path_schemas("\"\""), Vec::<String>::new());
    }

    #[test]
    fn only_another_schema_before_pg_catalog_is_warned_of() {
        for warned in
            ["public, pg_catalog", "\"$user\", pg_catalog", "pg_temp, app, pg_catalog", "\"a,pg_catalog\", pg_catalog", "public, PG_CATALOG"]
        {
            let w = search_path_warning(warned);
            assert!(w.as_deref().is_some_and(|w| w.contains(warned) && w.contains("put pg_catalog first")), "{warned}: {w:?}");
        }
        for fine in
            ["\"$user\", public", "pg_catalog, public", "pg_temp, pg_catalog, public", "", "public, \"pg_catalog \"", "public, \"PG_CATALOG\""]
        {
            assert_eq!(search_path_warning(fine), None, "{fine}");
        }
    }

    #[tokio::test]
    async fn a_shutdown_waits_until_the_other_end_closes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let at = listener.local_addr().unwrap();
        let (got_all, all_got) = tokio::sync::oneshot::channel();
        let (close, closing) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            socket.read_to_end(&mut seen).await.unwrap();
            got_all.send(seen).unwrap();
            let _ = closing.await;
        });
        let client = tokio::spawn(async move {
            let mut stream = UntilClosed { inner: TcpStream::connect(at).await.unwrap(), shut: false };
            stream.write_all(b"cancel").await.unwrap();
            stream.shutdown().await
        });
        // The server has the request and the client's end of it, and has not closed: the
        // client's shutdown is still waiting.
        assert_eq!(all_got.await.unwrap(), b"cancel");
        assert!(!client.is_finished());
        close.send(()).unwrap();
        server.await.unwrap();
        assert!(client.await.unwrap().is_ok(), "done once the server closed");
    }

    #[test]
    fn no_statement_starts_while_a_cancel_request_is_on_its_way() {
        let db = dbee_testkit::require_testdb!();
        let server = db.server("pg14").unwrap();
        let config = ConnectionConfig {
            host: "127.0.0.1".into(),
            port: server.port,
            user: "postgres".into(),
            database: "dbee".into(),
            ..ConnectionConfig::default()
        };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let target = Target {
                config: &config,
                password: Some(&db.password),
                tunnel: None,
                purpose: dbee_driver::Purpose::Browse,
                create_file: false,
                allowed_dirs: None,
            };
            let conn = Arc::new(connect(target).await.unwrap());
            let on_its_way = conn.canceller.on_its_way.lock().await;
            let starting = tokio::spawn({
                let conn = Arc::clone(&conn);
                async move { conn.lock().await.map(drop) }
            });
            // One thread: the task has run as far as it can, to the request on its way.
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            assert!(!starting.is_finished(), "started while a cancel request was on its way");
            drop(on_its_way);
            assert_eq!(starting.await.unwrap(), Ok(()));
        });
    }
}
