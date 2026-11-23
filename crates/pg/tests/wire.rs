//! What dbee sends and keeps when the other end is not the server it should be: stand-ins on
//! 127.0.0.1 that play a man in the middle or a hostile server, message by message as the
//! PostgreSQL protocol documentation describes them, in plain threads with blocking I/O.
//!
//! The stand-ins that speak TLS use the test servers' certificate and key (`cargo xtask testdb
//! up`), which name `localhost` and `127.0.0.1` and are signed by the test CA.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_driver::{DbError, Purpose, Target, TlsState};
use dbee_pg::PgConnection;
use dbee_testkit::{TestDb, require_testdb};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

const PASSWORD: &str = "Summer2026!";
const SSL_REQUEST: [u8; 4] = 80_877_103_i32.to_be_bytes();

fn listener() -> (TcpListener, u16) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

fn accept(l: &TcpListener) -> TcpStream {
    let (s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.set_nodelay(true).unwrap();
    s
}

fn config(port: u16, mode: SslMode, ca_file: Option<std::path::PathBuf>) -> ConnectionConfig {
    ConnectionConfig {
        kind: DbKind::Postgres,
        host: "127.0.0.1".into(),
        port,
        user: "app".into(),
        database: "shop".into(),
        ssl_mode: mode,
        ca_file,
        ..ConnectionConfig::default()
    }
}

async fn open(c: &ConnectionConfig) -> Result<PgConnection, DbError> {
    let target = Target { config: c, password: Some(PASSWORD), tunnel: None, purpose: Purpose::Browse, create_file: false, allowed_dirs: None };
    dbee_pg::connect(target).await
}

/// The stand-in thread's result, joined off the runtime: a connection task dropped with its
/// client still has to say goodbye on it.
async fn joined<T: Send + 'static>(h: JoinHandle<T>) -> T {
    tokio::task::spawn_blocking(move || h.join().unwrap()).await.unwrap()
}

/// The test servers' certificate and key, for a stand-in that speaks TLS as they do.
fn tls_config(db: &TestDb) -> Arc<rustls::ServerConfig> {
    let dir = db.ca_file.parent().unwrap();
    let certs = CertificateDer::pem_file_iter(dir.join("server.crt")).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    let key = PrivateKeyDer::from_pem_file(dir.join("server.key")).unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    config.alpn_protocols = vec![b"postgresql".to_vec()];
    Arc::new(config)
}

/// A message without a type byte: SSLRequest, CancelRequest, StartupMessage.
fn read_untyped(s: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).ok()?;
    let mut body = vec![0; usize::try_from(i32::from_be_bytes(len)).ok()?.checked_sub(4)?];
    s.read_exact(&mut body).ok()?;
    Some(body)
}

fn read_typed(s: &mut impl Read) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    s.read_exact(&mut head).ok()?;
    let len = usize::try_from(i32::from_be_bytes([head[1], head[2], head[3], head[4]])).ok()?;
    let mut body = vec![0; len.checked_sub(4)?];
    s.read_exact(&mut body).ok()?;
    Some((head[0], body))
}

fn msg(tag: u8, body: &[u8]) -> Vec<u8> {
    [&[tag][..], &i32::try_from(body.len() + 4).unwrap().to_be_bytes(), body].concat()
}

/// An authentication request: 0 is AuthenticationOk, 3 the password in clear text, 5 MD5.
fn auth(code: i32, extra: &[u8]) -> Vec<u8> {
    msg(b'R', &[&code.to_be_bytes()[..], extra].concat())
}

fn ready() -> Vec<u8> {
    msg(b'Z', b"I")
}

fn complete(tag: &str) -> Vec<u8> {
    msg(b'C', &[tag.as_bytes(), &[0]].concat())
}

/// A RowDescription of text columns.
fn row_description(names: &[&str]) -> Vec<u8> {
    let mut b = u16::try_from(names.len()).unwrap().to_be_bytes().to_vec();
    for n in names {
        b.extend_from_slice(n.as_bytes());
        b.push(0);
        b.extend_from_slice(&0i32.to_be_bytes());
        b.extend_from_slice(&0i16.to_be_bytes());
        b.extend_from_slice(&25i32.to_be_bytes());
        b.extend_from_slice(&(-1i16).to_be_bytes());
        b.extend_from_slice(&(-1i32).to_be_bytes());
        b.extend_from_slice(&0i16.to_be_bytes());
    }
    msg(b'T', &b)
}

fn data_row(values: &[Option<&[u8]>]) -> Vec<u8> {
    let mut b = u16::try_from(values.len()).unwrap().to_be_bytes().to_vec();
    for v in values {
        match v {
            None => b.extend_from_slice(&(-1i32).to_be_bytes()),
            Some(v) => {
                b.extend_from_slice(&i32::try_from(v.len()).unwrap().to_be_bytes());
                b.extend_from_slice(v);
            }
        }
    }
    msg(b'D', &b)
}

/// The rest of a login after the password: AuthenticationOk, the cancel key, ready.
fn logged_in(s: &mut impl Write) {
    let mut out = auth(0, &[]);
    out.extend(msg(b'K', &[&42i32.to_be_bytes()[..], &0x005E_C2E7_i32.to_be_bytes()].concat()));
    out.extend(msg(b'S', b"server_version\x0016.0\x00"));
    out.extend(ready());
    s.write_all(&out).unwrap();
}

/// One query's answer: a result of one text column per value of `rows`, then ready.
fn one_column(name: &str, rows: &[&str]) -> Vec<u8> {
    let mut out = row_description(&[name]);
    for row in rows {
        out.extend(data_row(&[Some(row.as_bytes())]));
    }
    out.extend(complete("SELECT"));
    out
}

/// Serves dbee's session setup as Postgres 16 answers it, and any other query through `other`,
/// until the client hangs up or `other` says to stop.
fn serve(s: &mut (impl Read + Write), other: &mut dyn FnMut(&mut dyn Write, &str) -> bool) {
    serve_as(s, "16.0", other);
}

/// [`serve`], with `version` as `server_version`.
fn serve_as(s: &mut (impl Read + Write), version: &str, other: &mut dyn FnMut(&mut dyn Write, &str) -> bool) {
    while let Some((tag, body)) = read_typed(s) {
        if tag != b'Q' {
            return;
        }
        let sql = String::from_utf8_lossy(body.strip_suffix(&[0]).unwrap_or(&body)).into_owned();
        let out = if sql.starts_with("SET ") {
            let mut out: Vec<u8> = sql.split(';').flat_map(|_| complete("SET")).collect();
            out.extend(ready());
            out
        } else if sql.starts_with("SHOW default_transaction_read_only") {
            let mut out = Vec::new();
            for value in ["off", "on", "UTF8", version, "160000", "\"$user\", public"] {
                out.extend(one_column("setting", &[value]));
            }
            out.extend(ready());
            out
        } else if sql.contains("current_schemas") {
            [one_column("unnest", &["public"]), ready()].concat()
        } else {
            if !other(s, &sql) {
                return;
            }
            continue;
        };
        if s.write_all(&out).is_err() {
            return;
        }
    }
}

/// A man in the middle on a plain link: he answers the SSL request with `N` (or is asked for
/// none), then asks for the password with `ask`. Returns the client's answer, if one came.
fn plain_asker(l: TcpListener, ask: Vec<u8>) -> JoinHandle<Option<(u8, Vec<u8>)>> {
    std::thread::spawn(move || {
        let mut s = accept(&l);
        let mut first = read_untyped(&mut s)?;
        if first.get(..4) == Some(&SSL_REQUEST[..]) {
            s.write_all(b"N").ok()?;
            first = read_untyped(&mut s)?;
        }
        drop(first);
        s.write_all(&ask).ok()?;
        let answer = read_typed(&mut s);
        let _ = s.write_all(&msg(b'E', b"SFATAL\0C28P01\0Mpassword authentication failed\0\0"));
        answer
    })
}

/// H1: under `prefer` and `disable`, a request for the password in clear text on a link without
/// TLS gets no answer, and the error says why and what to do.
#[tokio::test]
async fn a_plain_link_never_carries_the_password_in_clear() {
    for mode in [SslMode::Prefer, SslMode::Disable] {
        let (l, port) = listener();
        let server = plain_asker(l, auth(3, &[]));
        let e = open(&config(port, mode, None)).await.err();
        let answer = joined(server).await;
        assert_eq!(answer, None, "{mode:?}: the man in the middle got an answer");
        assert_eq!(e, Some(DbError::ClearPassword), "{mode:?}");
    }
}

/// MD5 over a plain link is answered, as libpq answers it (docs/decisions.md, 2026-11-08): the
/// answer is a hash, never the password.
#[tokio::test]
async fn md5_over_a_plain_link_is_answered_with_a_hash() {
    let (l, port) = listener();
    let server = plain_asker(l, auth(5, &[1, 2, 3, 4]));
    let _ = open(&config(port, SslMode::Prefer, None)).await;
    let (tag, body) = joined(server).await.unwrap();
    assert_eq!(tag, b'p');
    assert!(body.starts_with(b"md5") && !body.windows(PASSWORD.len()).any(|w| w == PASSWORD.as_bytes()), "{body:?}");
}

/// The password in clear text goes over TLS whose certificate dbee checked: such a login works.
#[tokio::test]
async fn the_password_in_clear_text_goes_over_checked_tls() {
    let db = require_testdb!();
    let tls = tls_config(db);
    let (l, port) = listener();
    let server = std::thread::spawn(move || {
        let mut s = accept(&l);
        assert_eq!(read_untyped(&mut s).unwrap().get(..4), Some(&SSL_REQUEST[..]));
        s.write_all(b"S").unwrap();
        let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(tls).unwrap(), s);
        read_untyped(&mut tls).unwrap();
        tls.write_all(&auth(3, &[])).unwrap();
        let answer = read_typed(&mut tls);
        logged_in(&mut tls);
        serve(&mut tls, &mut |_, _| false);
        answer
    });
    let conn = open(&config(port, SslMode::VerifyFull, Some(db.ca_file.clone()))).await.unwrap();
    drop(conn);
    let answer = joined(server).await;
    assert_eq!(answer, Some((b'p', [PASSWORD.as_bytes(), &[0]].concat())));
}

/// L1: the cancel request of a session over TLS goes over TLS or not at all, under `prefer` as
/// under `verify-full`. A man in the middle who answers its SSL request with `N` gets no cancel
/// key, and the cancel says what happened and what to do.
#[tokio::test]
async fn the_cancel_of_a_tls_session_never_goes_in_plain() {
    let db = require_testdb!();
    for (mode, ca_file) in [(SslMode::Prefer, None), (SslMode::VerifyFull, Some(db.ca_file.clone()))] {
        let tls = tls_config(db);
        let (l, port) = listener();
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut s = accept(&l);
            read_untyped(&mut s).unwrap();
            s.write_all(b"S").unwrap();
            let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(tls).unwrap(), s);
            read_untyped(&mut tls).unwrap();
            logged_in(&mut tls);
            // The cancel's own connection: no TLS this time.
            let cancel = std::thread::spawn(move || {
                let mut c = accept(&l);
                let asked = read_untyped(&mut c);
                c.write_all(b"N").unwrap();
                tx.send((asked, read_untyped(&mut c))).unwrap();
            });
            serve(&mut tls, &mut |_, _| true);
            cancel.join().unwrap();
        });
        let conn = open(&config(port, mode, ca_file)).await.unwrap();
        let cancelled = conn.canceller().cancel().await;
        let (asked, sent) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(asked.as_ref().and_then(|a| a.get(..4)), Some(&SSL_REQUEST[..]), "{mode:?}: the cancel asked for TLS");
        assert_eq!(sent, None, "{mode:?}: the cancel key went in plain");
        let e = cancelled.unwrap_err();
        let shown = e.to_string();
        assert!(shown.contains("did not send it") && shown.contains("Try again") && !shown.contains("could not set up TLS"), "{mode:?}: {shown}");
        drop(conn);
        joined(server).await;
    }
}

/// A server that takes no TLS (`N`, or no SSL request under `disable`) and logs anyone in.
fn plain_server(l: TcpListener) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut s = accept(&l);
        let Some(first) = read_untyped(&mut s) else { return };
        if first.get(..4) == Some(&SSL_REQUEST[..]) {
            s.write_all(b"N").unwrap();
            read_untyped(&mut s).unwrap();
        }
        logged_in(&mut s);
        serve(&mut s, &mut |_, _| false);
    })
}

/// A server that takes TLS with the test servers' certificate and logs anyone in.
fn tls_server(l: TcpListener, tls: Arc<rustls::ServerConfig>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut s = accept(&l);
        read_untyped(&mut s).unwrap();
        s.write_all(b"S").unwrap();
        let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(tls).unwrap(), s);
        if read_untyped(&mut tls).is_none() {
            return;
        }
        logged_in(&mut tls);
        serve(&mut tls, &mut |_, _| false);
    })
}

/// M1: the connection says what protects its link, and a `prefer` connection that ended
/// without TLS on a direct link warns; one that asked for no TLS does not.
#[tokio::test]
async fn the_link_says_whether_it_is_encrypted_and_checked() {
    let db = require_testdb!();
    // On 127.0.0.1 nothing leaves the machine: no warning, also under prefer (the decision for an
    // address on the network is `dbee_driver::tls`'s own test).
    for (mode, warned) in [(SslMode::Prefer, false), (SslMode::Disable, false)] {
        let (l, port) = listener();
        let server = plain_server(l);
        let conn = open(&config(port, mode, None)).await.unwrap();
        let info = conn.info().clone();
        drop(conn);
        joined(server).await;
        assert_eq!(info.tls, Some(TlsState::Off), "{mode:?}");
        assert!(info.local, "{mode:?}: 127.0.0.1 is this machine");
        assert_eq!(info.warnings.iter().any(|w| w.contains("not encrypted") && w.contains("verify-full")), warned, "{mode:?}: {info:?}");
        assert_eq!(info.warnings.len(), usize::from(warned), "{mode:?}: {info:?}");
    }
    for (mode, ca, state) in [
        (SslMode::Prefer, None, TlsState::Unchecked),
        (SslMode::Require, None, TlsState::Unchecked),
        (SslMode::Require, Some(db.ca_file.clone()), TlsState::CertificateChecked),
        (SslMode::VerifyCa, Some(db.ca_file.clone()), TlsState::CertificateChecked),
        (SslMode::VerifyFull, Some(db.ca_file.clone()), TlsState::Checked),
    ] {
        let (l, port) = listener();
        let server = tls_server(l, tls_config(db));
        let conn = open(&config(port, mode, ca)).await.unwrap();
        let info = conn.info().clone();
        drop(conn);
        joined(server).await;
        assert_eq!((info.tls, info.warnings.as_slice()), (Some(state), &[][..]), "{mode:?}");
    }
}

/// A server that answers any other query with `columns` text columns and `rows` rows of NULL.
fn wide_server(l: TcpListener, columns: usize, rows: usize) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut s = accept(&l);
        read_untyped(&mut s).unwrap();
        logged_in(&mut s);
        let names: Vec<String> = (0..columns).map(|i| format!("c{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let row = data_row(&vec![None; columns]);
        serve(&mut s, &mut |w, _| {
            let mut out = row_description(&names);
            for _ in 0..rows {
                out.extend_from_slice(&row);
            }
            out.extend([complete("SELECT"), ready()].concat());
            w.write_all(&out).is_ok()
        });
    })
}

/// M3: a NULL cell holds its slot, so the cap counts it: a result of wide NULL rows stops at the
/// cap instead of holding every row.
#[tokio::test]
async fn null_cells_count_against_the_cap() {
    let (l, port) = listener();
    let server = wide_server(l, 1000, 100);
    let conn = open(&config(port, SslMode::Disable, None)).await.unwrap();
    let limits = dbee_driver::Limits { max_rows: 1000, max_bytes: 1 << 20 };
    let r = dbee_pg::query::run_untyped(&conn, "SELECT * FROM wide", limits).await.unwrap();
    drop(conn);
    joined(server).await;
    assert_eq!((r.rows_seen, r.complete), (100, false), "{} rows kept", r.rows.len());
    assert!(dbee_driver::result_cost(&r) <= limits.max_bytes);
}

/// M3: a result of more columns than a Postgres query can return is refused before its rows, and
/// the connection goes on.
#[tokio::test]
async fn more_columns_than_postgres_returns_are_refused() {
    let (l, port) = listener();
    let server = wide_server(l, 1665, 10);
    let conn = open(&config(port, SslMode::Disable, None)).await.unwrap();
    let e = dbee_pg::query::run_untyped(&conn, "SELECT * FROM wide", dbee_driver::QUERY_LIMITS).await.unwrap_err();
    assert!(e.to_string().contains("1665 columns") && e.to_string().contains("1664"), "{e}");
    assert!(!conn.is_closed());
    drop(conn);
    joined(server).await;
}

/// L4: server text is kept to a size where dbee keeps it: an error's message, detail and hint,
/// and the version.
#[tokio::test]
async fn long_server_text_is_cut_where_it_is_kept() {
    let long = "\u{e9}".repeat(500_000);
    let (l, port) = listener();
    let server = std::thread::spawn(move || {
        let mut s = accept(&l);
        read_untyped(&mut s).unwrap();
        logged_in(&mut s);
        // Within what the setup's read holds, past what it keeps of a version.
        let version = "\u{e9}".repeat(10_000);
        serve_as(&mut s, &version, &mut |w, _| {
            let fields = format!("SERROR\0VERROR\0C42P01\0M{long}\0D{long}\0H{long}\0\0");
            w.write_all(&[msg(b'E', fields.as_bytes()), ready()].concat()).is_ok()
        });
    });
    let conn = open(&config(port, SslMode::Disable, None)).await.unwrap();
    let kept = conn.info().version.clone();
    let e = dbee_pg::query::run_untyped(&conn, "SELECT 1", dbee_driver::QUERY_LIMITS).await.unwrap_err();
    drop(conn);
    joined(server).await;
    assert!(kept.len() <= dbee_driver::MAX_VERSION + 3 && kept.ends_with('\u{2026}'), "{} bytes", kept.len());
    let DbError::Server(e) = e else { panic!("{e:?}") };
    for text in [Some(&e.message), e.detail.as_ref(), e.hint.as_ref()] {
        let text = text.unwrap();
        assert!(text.len() <= dbee_driver::MAX_MESSAGE + 3 && text.starts_with("\u{e9}\u{e9}"), "{} bytes", text.len());
    }
}

/// A TLS handshake that fails on the server's side: its fatal alert, after the ClientHello.
fn break_handshake(s: &mut TcpStream) {
    let mut head = [0u8; 5];
    if s.read_exact(&mut head).is_ok() {
        let mut body = vec![0; usize::from(u16::from_be_bytes([head[3], head[4]]))];
        let _ = s.read_exact(&mut body);
    }
    let _ = s.write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28]);
    let _ = s.shutdown(std::net::Shutdown::Write);
    let _ = s.read_to_end(&mut Vec::new());
}

/// `prefer` connects again without TLS after a broken handshake, as libpq does; that second
/// connection is watched too, so its request for the password in clear text gets no answer.
#[tokio::test]
async fn prefer_after_a_broken_handshake_still_refuses_the_password_in_clear() {
    let (l, port) = listener();
    let server = std::thread::spawn(move || {
        let mut s = accept(&l);
        assert_eq!(read_untyped(&mut s).unwrap().get(..4), Some(&SSL_REQUEST[..]));
        s.write_all(b"S").unwrap();
        break_handshake(&mut s);
        let mut s = accept(&l);
        let first = read_untyped(&mut s).unwrap();
        let asked = first.get(..4) == Some(&SSL_REQUEST[..]);
        s.write_all(&auth(3, &[])).unwrap();
        (asked, read_typed(&mut s))
    });
    let e = open(&config(port, SslMode::Prefer, None)).await.err();
    let (asked, answer) = joined(server).await;
    assert!(!asked, "the second connection is plain, as libpq's");
    assert_eq!(answer, None, "the password went in clear text");
    assert_eq!(e, Some(DbError::ClearPassword));
}

/// A server that logs anyone in and answers dbee's own reads as given: its settings, the schemas
/// its search path finds, and the read-only guard's read of the mode; any other query with a row.
fn own_reads_server(l: TcpListener, settings: Vec<u8>, schemas: Vec<u8>, mode: Vec<u8>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut s = accept(&l);
        read_untyped(&mut s).unwrap();
        logged_in(&mut s);
        while let Some((b'Q', body)) = read_typed(&mut s) {
            let sql = String::from_utf8_lossy(&body);
            let out = if sql.starts_with("SET ") {
                let sets: Vec<u8> = sql.split(';').flat_map(|_| complete("SET")).collect();
                [sets, ready()].concat()
            } else if sql.contains("standard_conforming_strings") {
                [settings.clone(), ready()].concat()
            } else if sql.contains("current_schemas") {
                [schemas.clone(), ready()].concat()
            } else if sql.contains("transaction_read_only") {
                [mode.clone(), ready()].concat()
            } else {
                [one_column("v", &["1"]), ready()].concat()
            };
            if s.write_all(&out).is_err() {
                return;
            }
        }
    })
}

fn settings(read_only: &str) -> Vec<u8> {
    [read_only, "on", "UTF8", "16.0", "160000", "\"$user\", public"].iter().flat_map(|v| one_column("setting", &[v])).collect()
}

/// A result of `columns` columns and `rows` rows of NULL.
fn wide(columns: usize, rows: usize) -> Vec<u8> {
    let names: Vec<String> = (0..columns).map(|i| format!("c{i}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let row = data_row(&vec![None; columns]);
    let mut out = row_description(&names);
    for _ in 0..rows {
        out.extend_from_slice(&row);
    }
    out.extend(complete("SELECT"));
    out
}

/// Important 2: the session setup holds no more than it asked for, whatever the server sends: a
/// hostile server's answer ends the connect in a plain error, before it can grow.
#[tokio::test]
async fn the_setup_holds_no_more_than_it_asked_for() {
    let many_rows = one_column("unnest", &["s"; 5_000]);
    let cases: [(Vec<u8>, Vec<u8>, &str); 4] = [
        (settings("off"), wide(65_535, 3), "65535 columns"),
        (settings("off"), many_rows, "more than"),
        (wide(2, 1), one_column("unnest", &["public"]), "2 columns"),
        ([settings("off"), one_column("setting", &["extra"])].concat(), one_column("unnest", &["public"]), "more than"),
    ];
    for (settings, schemas, says) in cases {
        let (l, port) = listener();
        let server = own_reads_server(l, settings, schemas, Vec::new());
        let e = open(&config(port, SslMode::Disable, None)).await.err();
        let shown = e.as_ref().map(ToString::to_string).unwrap_or_default();
        assert!(shown.contains("a query of dbee's own") && shown.contains(says), "{says}: {shown:?}");
        joined(server).await;
    }
}

/// Each schema the search path finds is kept to the longest name Postgres has.
#[tokio::test]
async fn a_schema_name_is_cut() {
    let long = "n".repeat(1_000);
    let (l, port) = listener();
    let server = own_reads_server(l, settings("off"), one_column("unnest", &[&long]), Vec::new());
    let conn = open(&config(port, SslMode::Disable, None)).await.unwrap();
    let kept = conn.visible_schemas().to_vec();
    drop(conn);
    joined(server).await;
    assert!(kept.len() == 1 && kept[0].len() <= 63 + '\u{2026}'.len_utf8() && kept[0].starts_with("nnn"), "{} bytes", kept[0].len());
}

/// Important 2: so does the read-only guard's read of the mode after a statement.
#[tokio::test]
async fn the_read_only_guard_holds_no_more_than_it_asked_for() {
    let (l, port) = listener();
    let server = own_reads_server(l, settings("on"), one_column("unnest", &["public"]), one_column("setting", &["on"; 5_000]));
    let c = ConnectionConfig { read_only: true, ..config(port, SslMode::Disable, None) };
    let conn = open(&c).await.unwrap();
    let e = dbee_pg::query::run_untyped(&conn, "SELECT 1", dbee_driver::QUERY_LIMITS).await.unwrap_err();
    assert!(e.to_string().contains("a query of dbee's own") && e.to_string().contains("more than"), "{e}");
    drop(conn);
    joined(server).await;
}

/// Important 3: a result's column names are cut to the longest name Postgres has, and its columns
/// count against what is left of the cap. When they don't fit, the statement ends in a plain error
/// and the next one runs.
#[tokio::test]
async fn column_names_are_cut_and_columns_that_do_not_fit_fail_the_statement() {
    let (l, port) = listener();
    let server = std::thread::spawn(move || {
        let mut s = accept(&l);
        read_untyped(&mut s).unwrap();
        logged_in(&mut s);
        let names: Vec<String> = (0..100).map(|i| format!("{i}{}", "n".repeat(100_000))).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        serve(&mut s, &mut |w, _| {
            let mut out = row_description(&names);
            out.extend(data_row(&vec![None; 100]));
            out.extend([complete("SELECT 1"), ready()].concat());
            w.write_all(&out).is_ok()
        });
    });
    let conn = open(&config(port, SslMode::Disable, None)).await.unwrap();
    let limits = dbee_driver::Limits { max_rows: 1000, max_bytes: 1 << 20 };
    let r = dbee_pg::query::run_untyped(&conn, "SELECT * FROM wide", limits).await.unwrap();
    assert_eq!((r.columns.len(), r.rows.len(), r.complete), (100, 1, true));
    assert!(r.columns.iter().all(|c| c.name.len() <= 63 + '\u{2026}'.len_utf8()), "{} bytes", r.columns[0].name.len());
    assert!(dbee_driver::result_cost(&r) <= limits.max_bytes);
    // The run's budget is spent: the columns do not fit, and the connection goes on.
    let spent = dbee_pg::query::run_untyped(&conn, "SELECT * FROM wide", dbee_driver::Limits { max_rows: 0, max_bytes: 0 }).await;
    assert_eq!(spent.err(), Some(DbError::TooLarge { left: 0 }));
    assert!(!conn.is_closed());
    let next = dbee_pg::query::run_untyped(&conn, "SELECT * FROM wide", limits).await.unwrap();
    assert_eq!(next.rows.len(), 1);
    drop(conn);
    joined(server).await;
}

/// Every text taken from tokio-postgres is cut: here the error a SCRAM exchange ends in,
/// the server's own words.
#[tokio::test]
async fn a_scram_error_is_cut() {
    let (l, port) = listener();
    let server = std::thread::spawn(move || {
        let mut s = accept(&l);
        read_untyped(&mut s)?;
        s.write_all(&auth(10, b"SCRAM-SHA-256\0\0")).ok()?;
        let (_, first) = read_typed(&mut s)?;
        let first = String::from_utf8_lossy(&first).into_owned();
        let nonce = first.rsplit("r=").next()?.to_owned();
        s.write_all(&auth(11, format!("r={nonce}server,s=c2FsdA==,i=4096").as_bytes())).ok()?;
        read_typed(&mut s)?;
        s.write_all(&auth(12, format!("e={}", "x".repeat(1_000_000)).as_bytes())).ok()?;
        read_typed(&mut s)
    });
    let e = open(&config(port, SslMode::Disable, None)).await.err().unwrap();
    joined(server).await;
    let DbError::Auth { message } = &e else { panic!("{e:?}") };
    assert!(message.contains("xxx") && message.len() <= dbee_driver::MAX_MESSAGE + '\u{2026}'.len_utf8(), "{} bytes", message.len());
}

/// `prefer` connects once more without TLS after a broken handshake, as libpq does, and the
/// connection says so; on 127.0.0.1 without a warning.
#[tokio::test]
async fn prefer_after_a_broken_handshake_ends_plain() {
    let (l, port) = listener();
    let server = std::thread::spawn(move || {
        let mut s = accept(&l);
        assert_eq!(read_untyped(&mut s).unwrap().get(..4), Some(&SSL_REQUEST[..]));
        s.write_all(b"S").unwrap();
        break_handshake(&mut s);
        let mut s = accept(&l);
        let asked = read_untyped(&mut s).unwrap().get(..4) == Some(&SSL_REQUEST[..]);
        logged_in(&mut s);
        serve(&mut s, &mut |_, _| false);
        asked
    });
    let conn = open(&config(port, SslMode::Prefer, None)).await.unwrap();
    let info = conn.info().clone();
    drop(conn);
    assert!(!joined(server).await, "the second connection is plain, as libpq's");
    assert_eq!((info.tls, info.warnings.as_slice()), (Some(TlsState::Off), &[][..]), "{info:?}");
}
