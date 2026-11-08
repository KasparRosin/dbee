//! The link to the server as the engine sees it, against Postgres stand-ins on this machine that
//! play the protocol message by message (PostgreSQL's protocol documentation), in plain threads
//! with blocking I/O: dbee's own refusal of a password in clear text.

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::Duration;

use common::*;
use dbee_config::{ConnectionConfig, DbKind, SslMode};
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
