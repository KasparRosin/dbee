//! The link to the server as the engine sees it, against Postgres stand-ins on this machine that
//! play the protocol message by message (PostgreSQL's protocol documentation), in plain threads
//! with blocking I/O, and a man in the middle in front of the test servers: dbee's own refusal
//! of a password in clear text, the warnings shown as notices, and a query connection that gets
//! less TLS than its connection.

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use common::*;
use dbee_config::{ConnectionConfig, DbKind, SslMode};
use dbee_engine::UiRequest;
use dbee_testkit::require_testdb;
use serde_json::json;

const SSL_REQUEST: [u8; 4] = 80_877_103_i32.to_be_bytes();

fn listener_on(ip: IpAddr) -> (TcpListener, u16) {
    let l = TcpListener::bind((ip, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    (l, port)
}

fn accept(l: &TcpListener) -> TcpStream {
    let (s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s
}

/// A message without a type byte: SSLRequest, StartupMessage.
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

fn auth(code: i32) -> Vec<u8> {
    msg(b'R', &code.to_be_bytes())
}

/// A config for a stand-in on `ip:port`.
fn standin_config(ip: IpAddr, port: u16, mode: SslMode) -> ConnectionConfig {
    ConnectionConfig {
        kind: DbKind::Postgres,
        host: ip.to_string(),
        port,
        user: "app".into(),
        database: "shop".into(),
        ssl_mode: mode,
        ..ConnectionConfig::default()
    }
}

/// A man in the middle that says `N` to TLS and asks for the password in clear text; whether the
/// client answered.
fn clear_text_asker(l: TcpListener) -> JoinHandle<Option<(u8, Vec<u8>)>> {
    std::thread::spawn(move || {
        let mut s = accept(&l);
        let mut first = read_untyped(&mut s)?;
        if first.get(..4) == Some(&SSL_REQUEST[..]) {
            s.write_all(b"N").ok()?;
            first = read_untyped(&mut s)?;
        }
        drop(first);
        s.write_all(&auth(3)).ok()?;
        read_typed(&mut s)
    })
}

/// dbee's refusal to send the password in clear text is an error of its own, never a prompt for
/// a password it would not send either.
#[test]
fn a_password_in_clear_text_is_refused_without_asking_for_one() {
    let mut s = session();
    let (l, port) = listener_on([127, 0, 0, 1].into());
    let server = clear_text_asker(l);
    let e = run_err(&mut s, "connection.open", json!({ "config": standin_config([127, 0, 0, 1].into(), port, SslMode::Prefer) }));
    assert_eq!(server.join().unwrap(), None, "the password went in clear text");
    assert_eq!(e.code(), "clearPassword", "{e}");
    assert!(e.to_string().contains("dbee stopped the login") && e.to_string().contains("verify-full"), "{e}");
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

fn one_column(rows: &[&str]) -> Vec<u8> {
    let mut out = row_description(&["v"]);
    for row in rows {
        let mut b = 1u16.to_be_bytes().to_vec();
        b.extend_from_slice(&i32::try_from(row.len()).unwrap().to_be_bytes());
        b.extend_from_slice(row.as_bytes());
        out.extend(msg(b'D', &b));
    }
    out.extend(complete("SELECT"));
    out
}

/// A Postgres 16 that logs anyone in without TLS, whose search path puts `public` before
/// `pg_catalog` (which dbee warns of), and whose catalog is empty; for each connection, until the
/// client hangs up.
fn warning_server(l: TcpListener) {
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            std::thread::spawn(move || {
                let Some(first) = read_untyped(&mut s) else { return };
                if first.get(..4) == Some(&SSL_REQUEST[..]) {
                    s.write_all(b"N").unwrap();
                    read_untyped(&mut s).unwrap();
                }
                let mut out = auth(0);
                out.extend(msg(b'K', &[&42i32.to_be_bytes()[..], &7i32.to_be_bytes()].concat()));
                out.extend(ready());
                s.write_all(&out).unwrap();
                while let Some((b'Q', body)) = read_typed(&mut s) {
                    let sql = String::from_utf8_lossy(&body);
                    let out = if sql.starts_with("SHOW default_transaction_read_only") {
                        let values = ["off", "on", "UTF8", "16.0", "160000", "public, pg_catalog"];
                        [values.iter().flat_map(|v| one_column(&[v])).collect(), ready()].concat()
                    } else if sql.contains("current_schemas") {
                        [one_column(&["public"]), ready()].concat()
                    } else if sql.starts_with("SET LOCAL") {
                        [complete("SET"), one_column(&[]), ready()].concat()
                    } else {
                        let sets: Vec<u8> = sql.split(';').flat_map(|_| complete("SET")).collect();
                        [sets, ready()].concat()
                    };
                    if s.write_all(&out).is_err() {
                        return;
                    }
                }
            });
        }
    });
}

/// A connection's warnings reach the window as notices, on open and on a reconnect alike.
#[test]
fn warnings_show_as_notices_on_open_and_on_reconnect() {
    let mut s = session();
    let (l, port) = listener_on([127, 0, 0, 1].into());
    warning_server(l);
    let r = open(&mut s, json!({ "config": standin_config([127, 0, 0, 1].into(), port, SslMode::Prefer) }));
    let warning = r["warnings"][0].as_str().unwrap_or_default().to_owned();
    assert!(warning.contains("search path"), "{r}");
    assert_eq!(s.take_ui_requests(), [UiRequest::Notice(warning.clone())]);
    s.run_blocking("connection.reconnect", json!({}), T).unwrap();
    assert_eq!(s.take_ui_requests(), [UiRequest::Notice(warning)]);
}

/// What a man in the middle does to every connection after the first: says `N` to Postgres's SSL
/// request, or clears `CLIENT_SSL` in MySQL's greeting.
#[derive(Clone, Copy)]
enum Strip {
    PostgresN,
    MysqlGreeting,
}

/// Copies `from` to `to` until either ends, keeping what passed when `seen` is given.
fn pipe(mut from: TcpStream, mut to: TcpStream, seen: Option<Arc<Mutex<Vec<u8>>>>) {
    let mut buf = [0u8; 16 * 1024];
    while let Ok(n @ 1..) = from.read(&mut buf) {
        if let Some(seen) = &seen {
            seen.lock().unwrap().extend_from_slice(&buf[..n]);
        }
        if to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(std::net::Shutdown::Write);
}

/// A proxy on 127.0.0.1 to `upstream`: the first connection (the browse connection) passes as it
/// is, every later one is stripped of TLS as `strip` says; what the client sent on those.
fn tls_stripper(upstream: u16, strip: Strip) -> (u16, Arc<Mutex<Vec<u8>>>) {
    let (l, port) = listener_on([127, 0, 0, 1].into());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    std::thread::spawn(move || {
        for (i, client) in l.incoming().enumerate() {
            let Ok(mut client) = client else { return };
            let seen = Arc::clone(&kept);
            std::thread::spawn(move || {
                let mut server = TcpStream::connect(("127.0.0.1", upstream)).unwrap();
                let mut record = None;
                if i > 0 {
                    match strip {
                        Strip::PostgresN => {
                            let mut first = [0u8; 8];
                            client.read_exact(&mut first).unwrap();
                            if first[4..] == SSL_REQUEST {
                                client.write_all(b"N").unwrap();
                            } else {
                                server.write_all(&first).unwrap();
                            }
                        }
                        Strip::MysqlGreeting => {
                            let mut head = [0u8; 4];
                            server.read_exact(&mut head).unwrap();
                            let mut greeting = vec![0; usize::from(u16::from_le_bytes([head[0], head[1]])) | usize::from(head[2]) << 16];
                            server.read_exact(&mut greeting).unwrap();
                            // After the protocol version, the version text, the thread id, 8 bytes of
                            // challenge and a filler: the lower capability flags; CLIENT_SSL is 0x0800.
                            let at = greeting.iter().skip(1).position(|&b| b == 0).unwrap() + 2 + 4 + 8 + 1;
                            greeting[at + 1] &= !0x08;
                            client.write_all(&[&head[..], &greeting].concat()).unwrap();
                        }
                    }
                    record = Some(seen);
                }
                let (c2, s2) = (client.try_clone().unwrap(), server.try_clone().unwrap());
                let up = std::thread::spawn(move || pipe(c2, s2, record));
                pipe(server, client, None);
                let _ = up.join();
            });
        }
    });
    (port, seen)
}

/// Important 1: the query connection never has less TLS than the connection it belongs to. A man
/// in the middle who lets the browse connection have TLS and strips it from the query connection
/// gets no statement: the run fails before anything is sent, and says so.
#[test]
fn a_query_connection_without_the_tls_of_its_connection_sends_nothing() {
    let _db = require_testdb!();
    let mysql = dbee_testkit::target(db().server("mysql80").expect("the test servers include mysql80"));
    for (upstream, strip, config) in
        [(pg14().port, Strip::PostgresN, pg14_config()), (db().server("mysql80").unwrap().port, Strip::MysqlGreeting, config_for(&mysql))]
    {
        let (port, seen) = tls_stripper(upstream, strip);
        let mut s = session();
        let r = open(&mut s, json!({ "config": ConnectionConfig { port, ..config }, "password": pw() }));
        assert_eq!(r["connection"]["tls"], "unchecked", "{r}");
        s.execute("tab.newQuery", json!({ "text": "select 4242 as marker" })).unwrap();
        let e = run_err(&mut s, "query.run", json!({}));
        assert!(e.to_string().contains("The query connection could not set up TLS") && e.to_string().contains("did not send"), "{e}");
        assert!(!String::from_utf8_lossy(&seen.lock().unwrap()).contains("4242"), "the statement went in plain text");
        assert_eq!(s.inspect()["connection"]["tls"], "unchecked");
    }
}
