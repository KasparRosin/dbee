//! The control channel's transport: JSON lines on `127.0.0.1:<port>` (`docs/control-protocol.md`).
//!
//! Both ends prove themselves with the two random lines of the run's token file, which only this
//! user can read ([`TokenFile`]). A connection's first line must be the file's first line
//! ([`MAX_TOKEN_LINE`] bytes at most, within [`AUTH_WAIT`]), or it gets one `unauthorized` reply
//! and is closed with nothing run. The reply to the right line carries the file's second line, so
//! the client knows it talks to this start of dbee and not to whatever holds the port now: dbee
//! writes the file only once it holds the port, with new lines each start.
//!
//! One thread accepts connections, and each connection gets a thread of its own (at most
//! [`MAX_CLIENTS`] at a time). A connection reads one request per line, at most [`MAX_LINE`]
//! bytes; blank lines are skipped. A line that is not a request (not JSON, too long, no
//! `method`) gets an error reply and the connection goes on. A line of HTTP ([`looks_like_http`])
//! gets one `httpRefused` reply and the connection is closed without reading on: a web page can
//! make a browser post to `127.0.0.1`, and its body lines must never run. A request goes to the UI
//! thread as a [`ControlRequest`], the window is woken, and the reply line (with the request's
//! `id`) is written once the app answers, or as a `timeout` error when the request's `timeoutMs`
//! (plus a second's grace, so the app's own timeout reply comes first) is up; the request carries
//! that deadline, and the app does not run it once it has passed. Requests on one connection are
//! answered in order. Logs name the address and failures, never what a request holds, and never
//! the token.

use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};

use dbee_config::store::{private_dir, replace_private_file};

use crate::control::{ControlRequest, Request, error, ok, parse_request, with_id};

/// The longest request line, in bytes.
pub const MAX_LINE: usize = 64 * 1024 * 1024;

/// Most connections served at once; more are told so and closed.
pub const MAX_CLIENTS: usize = 16;

/// The longest first line, in bytes: a client that has not proved itself gets no more of the
/// machine than this.
pub const MAX_TOKEN_LINE: usize = 4096;

/// How long a new connection may take to send its whole first line, so one that says nothing does
/// not keep a place among the [`MAX_CLIENTS`].
pub const AUTH_WAIT: Duration = Duration::from_secs(10);

/// How much longer than its `timeoutMs` the server waits for the app's reply.
const GRACE: Duration = Duration::from_secs(1);

/// Wakes the window so it handles a request now.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The two secrets of one start: `token`, which a client sends first, and `proof`, the answer that
/// shows the client this is the dbee that wrote the file.
struct Secrets {
    token: String,
    proof: String,
}

impl Secrets {
    fn new() -> io::Result<Secrets> {
        let mut bytes = [0u8; 64];
        getrandom::fill(&mut bytes).map_err(|e| io::Error::other(format!("no random numbers: {e}")))?;
        let hex = |part: &[u8]| part.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let (token, proof) = bytes.split_at(32);
        Ok(Secrets { token: hex(token), proof: hex(proof) })
    }
}

/// The run's token file, removed when this is dropped, if it still holds this start's lines (a later
/// start that named the same file has replaced them). A file left by a run that was killed does no
/// harm: the next start's lines are new.
#[derive(Debug)]
pub struct TokenFile(PathBuf, String);

impl TokenFile {
    /// Writes `secrets` to `path`, `0600` in a `0700` folder where the system has file modes. What
    /// was at `path` before is replaced, not followed.
    fn write(path: &Path, secrets: &Secrets) -> io::Result<TokenFile> {
        let lines = format!("{}\n{}\n", secrets.token, secrets.proof);
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        private_dir(dir).and_then(|()| replace_private_file(path, lines.as_bytes())).map_err(|e| {
            io::Error::other(format!("its token file {}: {e}. Name a file in a folder of your own with DBEE_CONTROL_TOKEN_FILE.", path.display()))
        })?;
        Ok(TokenFile(path.to_path_buf(), lines))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TokenFile {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.0).is_ok_and(|now| now == self.1) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}

/// A channel that listens: its address, the requests for the app to handle each frame, and the
/// token file, to keep until the app ends.
#[derive(Debug)]
pub struct Listening {
    pub addr: SocketAddr,
    pub requests: Receiver<ControlRequest>,
    pub token_file: TokenFile,
}

/// Listens on `127.0.0.1:port` (port 0 picks a free port) and writes the token file at `token_file`.
/// A port in use is an error and leaves that file alone: it is the running dbee's. `wake` wakes the
/// window for each request.
pub fn listen(port: u16, token_file: &Path, wake: Wake) -> io::Result<Listening> {
    listen_within(port, token_file, wake, AUTH_WAIT)
}

fn listen_within(port: u16, token_file: &Path, wake: Wake, auth_wait: Duration) -> io::Result<Listening> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
    let addr = listener.local_addr()?;
    let secrets = Secrets::new()?;
    let token_file = TokenFile::write(token_file, &secrets)?;
    let auth = Arc::new(Auth { secrets, wait: auth_wait });
    let (tx, requests) = channel();
    std::thread::Builder::new().name("dbee-control".into()).spawn(move || accept(&listener, &auth, &tx, &wake))?;
    log::info!("control channel listening on {addr}; its token file is {}", token_file.path().display());
    Ok(Listening { addr, requests, token_file })
}

/// What a connection's first line is checked against, and how long it may take.
struct Auth {
    secrets: Secrets,
    wait: Duration,
}

fn accept(listener: &TcpListener, auth: &Arc<Auth>, tx: &Sender<ControlRequest>, wake: &Wake) {
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
        let (auth, tx, wake, count) = (Arc::clone(auth), tx.clone(), Arc::clone(wake), Arc::clone(&clients));
        let spawned = std::thread::Builder::new().name("dbee-control-client".into()).spawn(move || {
            serve(stream, &auth, &tx, &wake);
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

/// A line of an HTTP request: a request line `<METHOD> <target> HTTP/<digit>…` (METHOD in
/// upper-case letters), or a header line `Name: value` (Name of letters, digits and `-`). No
/// request line of the channel is either: a request starts with `{`.
pub(crate) fn looks_like_http(line: &[u8]) -> bool {
    let mut parts = line.splitn(3, |&b| b == b' ');
    if let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
        && !method.is_empty()
        && method.iter().all(u8::is_ascii_uppercase)
        && !target.is_empty()
        && version.strip_prefix(b"HTTP/").and_then(<[u8]>::first).is_some_and(u8::is_ascii_digit)
    {
        return true;
    }
    match line.iter().position(|&b| b == b':') {
        None | Some(0) => false,
        Some(colon) => line.get(..colon).is_some_and(|name| name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')),
    }
}

fn write_reply(w: &mut impl Write, reply: &Json) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(reply).map_err(io::Error::other)?;
    bytes.push(b'\n');
    w.write_all(&bytes)?;
    w.flush()
}

/// Reads a connection's first line into `buf`, within `wait` for the whole line and at most
/// [`MAX_TOKEN_LINE`] bytes: past those it stops reading. What follows the line stays in `r`.
fn first_line(r: &mut BufReader<TcpStream>, wait: Duration, buf: &mut Vec<u8>) -> io::Result<Line> {
    let start = Instant::now();
    loop {
        let left = wait.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        r.get_ref().set_read_timeout(Some(left))?;
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(if buf.is_empty() { Line::Eof } else { finish(buf) });
        }
        let newline = chunk.iter().position(|&b| b == b'\n');
        let part = chunk.get(..newline.unwrap_or(chunk.len())).unwrap_or_default();
        if buf.len().saturating_add(part.len()) > MAX_TOKEN_LINE {
            return Ok(Line::TooLong);
        }
        buf.extend_from_slice(part);
        let used = part.len().saturating_add(usize::from(newline.is_some()));
        r.consume(used);
        if newline.is_some() {
            return Ok(finish(buf));
        }
    }
}

/// The two are the same, in a time that does not show how far they agree.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |differ, (x, y)| differ | (x ^ y)) == 0
}

/// Whether the connection may go on: its first line is the token, answered with the proof. Any other
/// first line gets one reply, and nothing the connection sent is run.
fn authenticate(reader: &mut BufReader<TcpStream>, writer: &mut TcpStream, auth: &Auth) -> bool {
    let mut line = Vec::new();
    let reply = match first_line(reader, auth.wait, &mut line) {
        Ok(Line::Text) if same(&line, auth.secrets.token.as_bytes()) => {
            // Past this point a client may take its time.
            let served = reader.get_ref().set_read_timeout(None).is_ok();
            return served && write_reply(writer, &with_id(ok(json!({ "proof": auth.secrets.proof })), &Json::Null)).is_ok();
        }
        Ok(Line::Text) if looks_like_http(&line) => {
            log::warn!("control channel: refused an HTTP request and closed its connection");
            http_refused()
        }
        Ok(Line::Eof) => return false,
        Ok(Line::Text | Line::TooLong) | Err(_) => {
            log::warn!("control channel: refused a connection without the token");
            error(
                "unauthorized",
                "the first line must be the token, the first line of this run's control token file (docs/control-protocol.md). Nothing was run, and this connection is closed.",
                json!({}),
            )
        }
    };
    let _ = write_reply(writer, &with_id(reply, &Json::Null));
    false
}

fn http_refused() -> Json {
    error("httpRefused", "the control channel speaks JSON lines, not HTTP: HTTP requests are refused, so web pages cannot use it", json!({}))
}

fn serve(stream: TcpStream, auth: &Auth, tx: &Sender<ControlRequest>, wake: &Wake) {
    let _ = stream.set_nodelay(true);
    let Ok(read) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read);
    let mut writer = stream;
    if !authenticate(&mut reader, &mut writer, auth) {
        let _ = writer.shutdown(Shutdown::Both);
        return;
    }
    let mut line = Vec::new();
    loop {
        let reply = match read_line(&mut reader, MAX_LINE, &mut line) {
            Ok(Line::Eof) | Err(_) => return,
            Ok(Line::TooLong) => with_id(error("lineTooLong", format!("a request line holds at most {MAX_LINE} bytes"), json!({})), &Json::Null),
            Ok(Line::Text) if looks_like_http(&line) => {
                // A web page can make the browser post to 127.0.0.1; its body lines must never run.
                log::warn!("control channel: refused an HTTP request and closed its connection");
                let _ = write_reply(&mut writer, &with_id(http_refused(), &Json::Null));
                let _ = writer.shutdown(Shutdown::Both);
                return;
            }
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
    let wait = timeout.saturating_add(GRACE);
    let (mut request, rx) = ControlRequest::new(method, params);
    // Past it this side answers `timeout`, so the app must not run the request any more.
    let deadline = Instant::now().checked_add(wait);
    request.deadline = deadline;
    if tx.send(request).is_err() {
        return with_id(error("closed", "the app is closing", json!({})), &id);
    }
    wake();
    match rx.recv_timeout(deadline.map_or(wait, |d| d.saturating_duration_since(Instant::now()))) {
        Ok(reply) => with_id(reply, &id),
        Err(RecvTimeoutError::Timeout) => with_id(error("timeout", "the app did not answer in time (is the window hidden?)", json!({})), &id),
        Err(RecvTimeoutError::Disconnected) => with_id(error("closed", "the app dropped the request: it is closing", json!({})), &id),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Cursor, Write};
    use std::net::TcpStream;
    use std::path::PathBuf;
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

    /// A folder of its own under `target/`, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Folder {
            let path = dbee_testkit::repo_root().join("target").join("testkit").join("control").join(dbee_testkit::unique("token"));
            std::fs::create_dir_all(&path).unwrap();
            Folder(path)
        }

        fn file(&self) -> PathBuf {
            self.0.join("sub").join("control-token")
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A channel on a free port, with the folder its token file lies in.
    fn channel(wake: Wake) -> (Listening, Folder) {
        let folder = Folder::new();
        (listen(0, &folder.file(), wake).unwrap(), folder)
    }

    /// The token file's two lines.
    fn secrets(file: &TokenFile) -> (String, String) {
        let text = std::fs::read_to_string(file.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "the token, then the proof");
        (lines[0].to_string(), lines[1].to_string())
    }

    /// A connection that has said nothing yet.
    fn connect_only(addr: std::net::SocketAddr) -> (BufReader<TcpStream>, TcpStream) {
        let s = TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        (BufReader::new(s.try_clone().unwrap()), s)
    }

    /// A connection that has sent the token and checked the server's proof.
    fn connect(addr: std::net::SocketAddr, file: &TokenFile) -> (BufReader<TcpStream>, TcpStream) {
        let (mut r, mut w) = connect_only(addr);
        let (token, proof) = secrets(file);
        let reply = roundtrip(&mut r, &mut w, &token);
        assert_eq!(reply, json!({ "id": null, "ok": true, "result": { "proof": proof } }));
        (r, w)
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
        let (Listening { addr, requests, token_file }, _folder) = channel(Arc::new(move || {
            w.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
        assert!(addr.ip().is_loopback());
        // The app: answers every request with its method.
        std::thread::spawn(move || {
            while let Ok(req) = requests.recv() {
                let _ = req.reply.send(json!({ "ok": true, "result": req.method }));
            }
        });
        let (mut r, mut w) = connect(addr, &token_file);
        let bad = roundtrip(&mut r, &mut w, "{oops");
        assert_eq!((bad["ok"].clone(), bad["code"].clone()), (json!(false), json!("invalidJson")), "{bad}");
        // A blank line gets no reply.
        w.write_all(b"\n\r\n").unwrap();
        let ok = roundtrip(&mut r, &mut w, r#"{"id":7,"method":"engine.commands"}"#);
        assert_eq!(ok, json!({ "id": 7, "ok": true, "result": "engine.commands" }));
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed) >= 1, "the app is woken for each request");
        // Several clients at once.
        let (mut r2, mut w2) = connect(addr, &token_file);
        assert_eq!(roundtrip(&mut r2, &mut w2, r#"{"id":"b","method":"ui.inspect"}"#)["id"], "b");
        assert_eq!(roundtrip(&mut r, &mut w, r#"{"id":8,"method":"app.quit"}"#)["result"], "app.quit");
    }

    #[test]
    fn a_reply_that_does_not_come_in_time_is_a_timeout() {
        let (Listening { addr, requests: rx, token_file }, _folder) = channel(Arc::new(|| {}));
        let (mut r, mut w) = connect(addr, &token_file);
        // The app takes the requests but never answers.
        let held = std::thread::spawn(move || rx.iter().take(1).collect::<Vec<_>>());
        let sent = std::time::Instant::now();
        let t = roundtrip(&mut r, &mut w, r#"{"id":1,"method":"engine.wait","params":{"timeoutMs":50}}"#);
        assert_eq!((t["id"].clone(), t["ok"].clone(), t["code"].clone()), (json!(1), json!(false), json!("timeout")), "{t}");
        // The request carries the instant the server gave up at, so the app will not run it late.
        let held = held.join().unwrap();
        let deadline = held.first().and_then(|req| req.deadline).expect("a forwarded request has a deadline");
        assert!(deadline > sent && deadline <= std::time::Instant::now(), "{deadline:?}");
    }

    #[test]
    fn http_is_recognised_by_its_request_or_header_lines() {
        for http in [
            &b"GET / HTTP/1.1"[..],
            b"POST /x?y=1 HTTP/1.0",
            b"OPTIONS * HTTP/2",
            b"Host: 127.0.0.1:7979",
            b"Origin: http://evil",
            b"Content-Type: text/plain",
            b"X-Requested-With:",
            b"sec-fetch-mode: no-cors",
        ] {
            assert!(looks_like_http(http), "{}", String::from_utf8_lossy(http));
        }
        for not in [
            &br#"{"method":"app.quit"}"#[..],
            b"{oops",
            b"",
            b"get / HTTP/1.1",
            b"GET / HTTPS/1.1",
            b"GET / HTTP/x",
            b"GET /",
            b": value",
            b"Bad Name: value",
            b"[1, 2]",
            b"\xff\xfe",
        ] {
            assert!(!looks_like_http(not), "{}", String::from_utf8_lossy(not));
        }
    }

    /// Reads to the end of the connection: the lines the server sent before closing it.
    fn read_until_closed(r: &mut BufReader<TcpStream>) -> Vec<Json> {
        let mut out = Vec::new();
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) => return out,
                Ok(_) => out.push(serde_json::from_str(&line).unwrap()),
                Err(e) => panic!("the server should close the connection, not leave it open: {e}"),
            }
        }
    }

    #[test]
    fn http_requests_are_refused_and_the_connection_closed() {
        let (Listening { addr, requests: rx, token_file }, _folder) = channel(Arc::new(|| {}));
        let (mut r, mut w) = connect_only(addr);
        // What a web page's fetch() sends: the JSON body would follow the blank line.
        w.write_all(b"POST / HTTP/1.1\r\nHost: x\r\n\r\n{\"method\":\"app.quit\"}\n").unwrap();
        let replies = read_until_closed(&mut r);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!((replies[0]["ok"].clone(), replies[0]["code"].clone()), (json!(false), json!("httpRefused")), "{replies:?}");
        assert!(replies[0]["error"].as_str().unwrap().contains("HTTP"), "{replies:?}");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing reaches the app");
        // A header line alone closes the connection too; other connections are served.
        let (mut r, mut w) = connect_only(addr);
        w.write_all(b"Origin: http://evil\n{\"method\":\"app.quit\"}\n").unwrap();
        assert_eq!(read_until_closed(&mut r).iter().map(|v| v["code"].clone()).collect::<Vec<_>>(), vec![json!("httpRefused")]);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing reaches the app");
        // After the token, HTTP is refused just the same.
        let (mut r, mut w) = connect(addr, &token_file);
        w.write_all(b"POST / HTTP/1.1\r\n{\"method\":\"app.quit\"}\n").unwrap();
        assert_eq!(read_until_closed(&mut r).iter().map(|v| v["code"].clone()).collect::<Vec<_>>(), vec![json!("httpRefused")]);
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing reaches the app");
        let (mut r, mut w) = connect(addr, &token_file);
        let app = std::thread::spawn(move || {
            let req = rx.recv().unwrap();
            let _ = req.reply.send(json!({ "ok": true, "result": req.method }));
        });
        assert_eq!(roundtrip(&mut r, &mut w, r#"{"id":1,"method":"ui.inspect"}"#)["result"], "ui.inspect");
        app.join().unwrap();
    }

    #[test]
    fn a_gone_app_is_an_error_reply() {
        let (Listening { addr, requests, token_file }, _folder) = channel(Arc::new(|| {}));
        drop(requests);
        let (mut r, mut w) = connect(addr, &token_file);
        let e = roundtrip(&mut r, &mut w, r#"{"id":1,"method":"ui.inspect"}"#);
        assert_eq!((e["ok"].clone(), e["code"].clone()), (json!(false), json!("closed")), "{e}");
    }

    /// One `unauthorized` reply, then the connection is closed.
    fn assert_refused(r: &mut BufReader<TcpStream>, what: &str, proof: &str) {
        let mut reply = String::new();
        r.read_line(&mut reply).unwrap();
        let mut replies: Vec<Json> = vec![serde_json::from_str(&reply).unwrap()];
        // What the client still sent can turn the close into a reset.
        let mut rest = String::new();
        match r.read_line(&mut rest) {
            Ok(0) => {}
            Ok(_) => replies.extend(serde_json::from_str(&rest).ok()),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset, "{what}: the connection stays open"),
        }
        assert_eq!(replies.len(), 1, "{what}: {replies:?}");
        assert_eq!((replies[0]["ok"].clone(), replies[0]["code"].clone()), (json!(false), json!("unauthorized")), "{what}: {replies:?}");
        assert!(!replies[0].to_string().contains(proof), "{what}: the proof is for those who know the token");
    }

    #[test]
    fn a_connection_without_the_token_is_refused_and_nothing_reaches_the_app() {
        let (ch, _folder) = channel(Arc::new(|| {}));
        let (token, proof) = secrets(&ch.token_file);
        let quit = r#"{"id":1,"method":"app.quit"}"#;
        let almost = format!("{}0", token.get(..63).unwrap());
        let longer = format!("{token}0");
        let upper = token.to_uppercase();
        for first in [quit, "", " ", "wrong", almost.as_str(), longer.as_str(), upper.as_str(), proof.as_str()] {
            let (mut r, mut w) = connect_only(ch.addr);
            // A request right after it must not run either.
            w.write_all(format!("{first}\n{quit}\n").as_bytes()).unwrap();
            assert_refused(&mut r, first, &proof);
        }
        assert!(ch.requests.recv_timeout(Duration::from_millis(200)).is_err(), "nothing reaches the app");
    }

    #[test]
    fn the_token_opens_the_channel_and_the_reply_proves_the_server() {
        let (ch, _folder) = channel(Arc::new(|| {}));
        let (token, proof) = secrets(&ch.token_file);
        assert_ne!(token, proof);
        for line in [&token, &proof] {
            assert_eq!(line.len(), 64, "256 bits");
            assert!(line.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        // The token and a request may come in one write, and `\r\n` ends a line too.
        let (mut r, mut w) = connect_only(ch.addr);
        w.write_all(format!("{token}\r\n{{\"id\":2,\"method\":\"ui.inspect\"}}\n").as_bytes()).unwrap();
        let mut reply = String::new();
        r.read_line(&mut reply).unwrap();
        assert_eq!(serde_json::from_str::<Json>(&reply).unwrap(), json!({ "id": null, "ok": true, "result": { "proof": proof } }));
        let req = ch.requests.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(req.method, "ui.inspect");
        let _ = req.reply.send(json!({ "ok": true, "result": 1 }));
        reply.clear();
        r.read_line(&mut reply).unwrap();
        assert_eq!(serde_json::from_str::<Json>(&reply).unwrap(), json!({ "id": 2, "ok": true, "result": 1 }));
        // The token is good for every connection of this start.
        connect(ch.addr, &ch.token_file);
    }

    #[test]
    fn a_client_that_proved_itself_may_take_its_time() {
        let folder = Folder::new();
        let wait = Duration::from_secs(1);
        let ch = listen_within(0, &folder.file(), Arc::new(|| {}), wait).unwrap();
        let (_r, mut w) = connect(ch.addr, &ch.token_file);
        std::thread::sleep(wait + Duration::from_millis(200));
        w.write_all(b"{\"id\":3,\"method\":\"engine.wait\"}\n").unwrap();
        assert_eq!(ch.requests.recv_timeout(Duration::from_secs(20)).unwrap().method, "engine.wait");
    }

    #[test]
    fn a_connection_that_stays_silent_or_trickles_is_closed() {
        let folder = Folder::new();
        let ch = listen_within(0, &folder.file(), Arc::new(|| {}), Duration::from_millis(100)).unwrap();
        let (token, proof) = secrets(&ch.token_file);
        let (mut r, _w) = connect_only(ch.addr);
        assert_refused(&mut r, "silent", &proof);
        // The time is for the whole line: most of the token, a byte at a time, does not reset it.
        let (mut r, mut w) = connect_only(ch.addr);
        let writer = std::thread::spawn(move || {
            for byte in token.bytes().take(60) {
                if w.write_all(&[byte]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        assert_refused(&mut r, "trickling", &proof);
        writer.join().unwrap();
    }

    #[test]
    fn a_first_line_past_the_cap_is_refused_without_reading_it_all() {
        let (ch, _folder) = channel(Arc::new(|| {}));
        let (_, proof) = secrets(&ch.token_file);
        let (mut r, mut w) = connect_only(ch.addr);
        w.write_all(&vec![b'x'; MAX_TOKEN_LINE + 1]).unwrap();
        // No newline has come, and the connection is closed all the same.
        assert_refused(&mut r, "long", &proof);
    }

    #[test]
    fn the_token_file_is_private_new_each_start_and_gone_at_the_end() {
        let folder = Folder::new();
        let path = folder.file();
        let first = listen(0, &path, Arc::new(|| {})).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
            // A folder that was looser is tightened.
            std::fs::set_permissions(path.parent().unwrap(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old = secrets(&first.token_file);
        let second = listen(0, &path, Arc::new(|| {})).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let new = secrets(&second.token_file);
        assert!(new.0 != old.0 && new.1 != old.1, "a file left by a run before tells nothing about this one");
        // The token of a start before does not open this one.
        let (mut r, mut w) = connect_only(second.addr);
        w.write_all(format!("{}\n", old.0).as_bytes()).unwrap();
        assert_refused(&mut r, "the old token", &new.1);
        drop(second);
        assert!(!path.exists(), "the file goes when the run ends");
    }

    #[test]
    fn a_start_that_ends_leaves_the_file_of_a_later_start_that_named_it() {
        let folder = Folder::new();
        let path = folder.file();
        let first = listen(0, &path, Arc::new(|| {})).unwrap();
        let second = listen(0, &path, Arc::new(|| {})).unwrap();
        let wanted = secrets(&second.token_file);
        drop(first);
        assert!(path.exists(), "the second start still runs");
        assert_eq!(secrets(&second.token_file), wanted);
        drop(second);
        assert!(!path.exists());
    }

    #[test]
    fn a_token_file_that_cannot_be_made_says_what_to_do() {
        let folder = Folder::new();
        let blocker = folder.0.join("a-file");
        std::fs::write(&blocker, "x").unwrap();
        let e = listen(0, &blocker.join("control-token"), Arc::new(|| {})).unwrap_err().to_string();
        assert!(e.contains("Name a file in a folder of your own with DBEE_CONTROL_TOKEN_FILE."), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_at_the_token_file_is_replaced_and_what_it_names_is_untouched() {
        let folder = Folder::new();
        let path = folder.file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let victim = folder.0.join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        let ch = listen(0, &path, Arc::new(|| {})).unwrap();
        assert!(std::fs::symlink_metadata(&path).unwrap().is_file(), "no longer a link");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert_eq!(secrets(&ch.token_file).0.len(), 64);
    }

    #[test]
    fn a_port_in_use_stops_the_start_and_leaves_the_running_dbees_file_alone() {
        let (ch, folder) = channel(Arc::new(|| {}));
        let before = secrets(&ch.token_file);
        assert!(listen(ch.addr.port(), &folder.file(), Arc::new(|| {})).is_err());
        assert_eq!(secrets(&ch.token_file), before, "the running dbee's file is as it was");
        let other = folder.0.join("other-token");
        assert!(listen(ch.addr.port(), &other, Arc::new(|| {})).is_err());
        assert!(!other.exists(), "a start that holds no port writes no file");
        connect(ch.addr, &ch.token_file);
    }

    #[test]
    fn a_token_file_that_cannot_be_written_stops_the_start() {
        let folder = Folder::new();
        std::fs::write(folder.0.join("a file"), "").unwrap();
        let e = listen(0, &folder.0.join("a file").join("control-token"), Arc::new(|| {})).unwrap_err();
        assert!(e.to_string().contains("its token file"), "{e}");
    }
}
