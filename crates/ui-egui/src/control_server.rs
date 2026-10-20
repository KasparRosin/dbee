//! The control channel's transport: JSON lines on `127.0.0.1:<port>` (`docs/control-protocol.md`).
//!
//! One thread accepts connections, and each connection gets a thread of its own (at most
//! [`MAX_CLIENTS`] at a time). A connection reads one request per line, at most [`MAX_LINE`]
//! bytes; blank lines are skipped. A line that is not a request (not JSON, too long, no
//! `method`) gets an error reply and the connection goes on. A request goes to the UI thread as a
//! [`ControlRequest`], the window is woken, and the reply line (with the request's `id`) is
//! written once the app answers, or as a `timeout` error when the request's `timeoutMs` (plus a
//! second's grace, so the app's own timeout reply comes first) is up. Requests on one connection
//! are answered in order. Logs name the address and failures, never what a request holds.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use serde_json::{Value as Json, json};

use crate::control::{ControlRequest, Request, error, parse_request, with_id};

/// The longest request line, in bytes.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

/// Most connections served at once; more are told so and closed.
pub const MAX_CLIENTS: usize = 16;

/// How much longer than its `timeoutMs` the server waits for the app's reply.
const GRACE: Duration = Duration::from_secs(1);

/// Wakes the window so it handles a request now.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Listens on `127.0.0.1:port` and returns the requests for the app to handle each frame; each
/// request repaints `ctx`.
pub fn start(port: u16, ctx: egui::Context) -> io::Result<Receiver<ControlRequest>> {
    listen(port, Arc::new(move || ctx.request_repaint())).map(|(_, rx)| rx)
}

/// [`start`] with any wake-up; port 0 picks a free port, returned with the address.
pub fn listen(port: u16, wake: Wake) -> io::Result<(SocketAddr, Receiver<ControlRequest>)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let addr = listener.local_addr()?;
    let (tx, rx) = channel();
    std::thread::Builder::new().name("dbee-control".into()).spawn(move || accept(&listener, &tx, &wake))?;
    log::info!("control channel listening on {addr}");
    Ok((addr, rx))
}

fn accept(listener: &TcpListener, tx: &Sender<ControlRequest>, wake: &Wake) {
    let clients = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(e) => {
                // Out of file descriptors, say: wait a little rather than spin.
                log::warn!("control channel: accepting a connection failed: {e}");
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        if clients.load(Ordering::Relaxed) >= MAX_CLIENTS {
            let reply = with_id(error("busy", format!("at most {MAX_CLIENTS} control connections at a time"), json!({})), &Json::Null);
            let _ = write_reply(&mut stream, &reply);
            continue;
        }
        clients.fetch_add(1, Ordering::Relaxed);
        let (tx, wake, count) = (tx.clone(), Arc::clone(wake), Arc::clone(&clients));
        let spawned = std::thread::Builder::new().name("dbee-control-client".into()).spawn(move || {
            serve(stream, &tx, &wake);
            count.fetch_sub(1, Ordering::Relaxed);
        });
        if let Err(e) = spawned {
            clients.fetch_sub(1, Ordering::Relaxed);
            log::warn!("control channel: no thread for a connection: {e}");
        }
    }
}

/// What [`read_line`] read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// A line, in the buffer, without its `\n` (or `\r\n`).
    Text,
    /// A line longer than the cap; it was skipped to its end.
    TooLong,
    /// The connection closed.
    Eof,
}

/// Reads the next line into `buf`, keeping at most `max` bytes: a longer line is read to its
/// end and dropped. A last line without `\n` counts.
pub(crate) fn read_line(r: &mut impl BufRead, max: usize, buf: &mut Vec<u8>) -> io::Result<Line> {
    buf.clear();
    let mut too_long = false;
    let mut any = false;
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(match (any, too_long) {
                (false, _) => Line::Eof,
                (true, true) => Line::TooLong,
                (true, false) => finish(buf),
            });
        }
        any = true;
        let newline = chunk.iter().position(|&b| b == b'\n');
        let part = chunk.get(..newline.unwrap_or(chunk.len())).unwrap_or_default();
        if !too_long {
            if buf.len().saturating_add(part.len()) > max {
                too_long = true;
                *buf = Vec::new();
            } else {
                buf.extend_from_slice(part);
            }
        }
        let used = part.len().saturating_add(usize::from(newline.is_some()));
        r.consume(used);
        if newline.is_some() {
            return Ok(if too_long { Line::TooLong } else { finish(buf) });
        }
    }
}

fn finish(buf: &mut Vec<u8>) -> Line {
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Line::Text
}

fn write_reply(w: &mut impl Write, reply: &Json) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(reply).map_err(io::Error::other)?;
    bytes.push(b'\n');
    w.write_all(&bytes)?;
    w.flush()
}

fn serve(stream: TcpStream, tx: &Sender<ControlRequest>, wake: &Wake) {
    let _ = stream.set_nodelay(true);
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut writer = stream;
    let mut line = Vec::new();
    loop {
        let reply = match read_line(&mut reader, MAX_LINE, &mut line) {
            Ok(Line::Eof) | Err(_) => return,
            Ok(Line::TooLong) => with_id(error("lineTooLong", format!("a request line holds at most {MAX_LINE} bytes"), json!({})), &Json::Null),
            Ok(Line::Text) if line.iter().all(u8::is_ascii_whitespace) => continue,
            Ok(Line::Text) => match parse_request(&line) {
                Ok(req) => forward(req, tx, wake),
                Err(reply) => reply,
            },
        };
        // A huge line's buffer is not kept for the rest of the connection.
        if line.capacity() > 1 << 20 {
            line = Vec::new();
        }
        if write_reply(&mut writer, &reply).is_err() {
            return;
        }
    }
}

fn forward(req: Request, tx: &Sender<ControlRequest>, wake: &Wake) -> Json {
    let Request { id, method, params, timeout } = req;
    let (request, rx) = ControlRequest::new(method, params);
    if tx.send(request).is_err() {
        return with_id(error("closed", "the app is closing", json!({})), &id);
    }
    wake();
    match rx.recv_timeout(timeout.saturating_add(GRACE)) {
        Ok(reply) => with_id(reply, &id),
        Err(RecvTimeoutError::Timeout) => with_id(error("timeout", "the app did not answer in time (is the window hidden?)", json!({})), &id),
        Err(RecvTimeoutError::Disconnected) => with_id(error("closed", "the app dropped the request: it is closing", json!({})), &id),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Cursor, Write};
    use std::net::TcpStream;
    use std::sync::Arc;
    use std::time::Duration;

    use serde_json::{Value as Json, json};

    use super::*;

    fn lines(input: &[u8], max: usize) -> Vec<Line> {
        let mut r = BufReader::with_capacity(4, Cursor::new(input.to_vec()));
        let mut buf = Vec::new();
        let mut out = Vec::new();
        loop {
            match read_line(&mut r, max, &mut buf).unwrap() {
                Line::Eof => return out,
                line => {
                    assert!(buf.len() <= max);
                    out.push(line);
                }
            }
        }
    }

    #[test]
    fn lines_are_capped_and_the_next_one_is_read() {
        let got = lines(b"{}\n0123456789abc\n{\"a\":1}\r\nlast", 10);
        assert_eq!(got, vec![Line::Text, Line::TooLong, Line::Text, Line::Text]);
        let mut r = BufReader::with_capacity(3, Cursor::new(b"0123456789\nok\r\n".to_vec()));
        let mut buf = Vec::new();
        assert_eq!(read_line(&mut r, 10, &mut buf).unwrap(), Line::Text);
        assert_eq!(buf, b"0123456789");
        assert_eq!(read_line(&mut r, 10, &mut buf).unwrap(), Line::Text);
        assert_eq!(buf, b"ok");
        assert_eq!(read_line(&mut r, 10, &mut buf).unwrap(), Line::Eof);
        assert_eq!(MAX_LINE, 64 * 1024 * 1024);
    }

    fn connect(addr: std::net::SocketAddr) -> (BufReader<TcpStream>, TcpStream) {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        (BufReader::new(s.try_clone().unwrap()), s)
    }

    fn roundtrip(r: &mut BufReader<TcpStream>, w: &mut TcpStream, line: &str) -> Json {
        w.write_all(line.as_bytes()).unwrap();
        w.write_all(b"\n").unwrap();
        let mut reply = String::new();
        r.read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn serves_json_lines_on_localhost_and_keeps_serving_after_bad_ones() {
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = Arc::clone(&woken);
        let (addr, rx) = listen(
            0,
            Arc::new(move || {
                w.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }),
        )
        .unwrap();
        assert!(addr.ip().is_loopback());
        // The app: answers every request with its method.
        std::thread::spawn(move || {
            while let Ok(req) = rx.recv() {
                let _ = req.reply.send(json!({ "ok": true, "result": req.method }));
            }
        });
        let (mut r, mut w) = connect(addr);
        let bad = roundtrip(&mut r, &mut w, "{oops");
        assert_eq!((bad["ok"].clone(), bad["code"].clone()), (json!(false), json!("invalidJson")), "{bad}");
        // A blank line gets no reply.
        w.write_all(b"\n\r\n").unwrap();
        let ok = roundtrip(&mut r, &mut w, r#"{"id":7,"method":"engine.commands"}"#);
        assert_eq!(ok, json!({ "id": 7, "ok": true, "result": "engine.commands" }));
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed) >= 1, "the app is woken for each request");
        // Several clients at once.
        let (mut r2, mut w2) = connect(addr);
        assert_eq!(roundtrip(&mut r2, &mut w2, r#"{"id":"b","method":"ui.inspect"}"#)["id"], "b");
        assert_eq!(roundtrip(&mut r, &mut w, r#"{"id":8,"method":"app.quit"}"#)["result"], "app.quit");
    }

    #[test]
    fn a_reply_that_does_not_come_in_time_is_a_timeout() {
        let (addr, rx) = listen(0, Arc::new(|| {})).unwrap();
        // The app takes the requests but never answers.
        let held = std::thread::spawn(move || rx.iter().take(1).collect::<Vec<_>>());
        let (mut r, mut w) = connect(addr);
        let t = roundtrip(&mut r, &mut w, r#"{"id":1,"method":"engine.wait","params":{"timeoutMs":50}}"#);
        assert_eq!((t["id"].clone(), t["ok"].clone(), t["code"].clone()), (json!(1), json!(false), json!("timeout")), "{t}");
        drop(held);
    }

    #[test]
    fn a_gone_app_is_an_error_reply() {
        let (addr, rx) = listen(0, Arc::new(|| {})).unwrap();
        drop(rx);
        let (mut r, mut w) = connect(addr);
        let e = roundtrip(&mut r, &mut w, r#"{"id":1,"method":"ui.inspect"}"#);
        assert_eq!((e["ok"].clone(), e["code"].clone()), (json!(false), json!("closed")), "{e}");
    }
}
