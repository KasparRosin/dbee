//! A stream that watches the start of a Postgres connection, below TLS: whether the server took
//! the SSL request up, and on a plain link the login's requests, so that a request for the
//! password in clear text fails before tokio-postgres can answer it.
//!
//! tokio-postgres answers `AuthenticationCleartextPassword` with the password on any link, and
//! `prefer` goes on without TLS when the server (or whoever answers in its place) says `N`.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};

use dbee_driver::DbError;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The SSLRequest message: its length, 8, and its code, 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xD2, 0x16, 0x2F];

/// Why the login stops when the server asks for the password in clear text on a plain link.
pub(crate) const CLEAR_PASSWORD: &str = "the server asks for the password in clear text, which dbee sends only over TLS, and this connection has none. Set SSL mode to require \
     or verify-full";

/// Why a cancel request is not sent when the server will not take it over TLS.
pub(crate) const PLAIN_CANCEL: &str = "the server would not take the cancel request over TLS, which the connection uses, so the request was not sent";

/// What the watch refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The server asked for the password in clear text on a link without TLS.
    ClearPassword,
    /// The cancel request of a session over TLS would have gone without it.
    PlainCancel,
    /// A message of the login is shorter than its kind can be.
    Malformed,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::ClearPassword => CLEAR_PASSWORD,
            Refusal::PlainCancel => PLAIN_CANCEL,
            Refusal::Malformed => "the server sent a malformed message during the login",
        })
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    fn io(self) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, self)
    }

    /// The refusal behind a tokio-postgres error, if the watch made it.
    pub(crate) fn of(e: &tokio_postgres::Error) -> Option<Refusal> {
        let io = std::error::Error::source(e)?.downcast_ref::<std::io::Error>()?;
        io.get_ref()?.downcast_ref::<Refusal>().copied()
    }

    pub(crate) fn error(self) -> DbError {
        match self {
            Refusal::ClearPassword => DbError::Auth { message: CLEAR_PASSWORD.into() },
            Refusal::PlainCancel => DbError::Tls(PLAIN_CANCEL.into()),
            Refusal::Malformed => DbError::Connect(self.to_string()),
        }
    }
}

/// What a watched stream lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule {
    /// A login: on a link without TLS, a request for the password in clear text is refused.
    Login,
    /// A cancel request of a session over TLS: it goes over TLS, or not at all.
    TlsOnly,
    /// Anything.
    Any,
}

/// How far the connection has got.
#[derive(Debug)]
enum State {
    /// Nothing sent yet.
    Start,
    /// The client asked for TLS; the server's one-byte answer comes next.
    Asked,
    /// A login without TLS: the server's messages are read up to its AuthenticationOk.
    Plain(Messages),
    /// TLS, or the login is over: bytes pass as they are.
    Through,
}

/// A stream to a Postgres server, watched as [`Rule`] says. `tls` is set once the server agreed
/// to TLS.
pub(crate) struct Watched<S> {
    inner: S,
    rule: Rule,
    state: State,
    tls: Arc<AtomicBool>,
}

impl<S> Watched<S> {
    pub(crate) fn new(inner: S, rule: Rule) -> Watched<S> {
        let state = if rule == Rule::Any { State::Through } else { State::Start };
        Watched { inner, rule, state, tls: Arc::default() }
    }

    /// Set once the server agreed to TLS; it stays readable after the stream moved on.
    pub(crate) fn tls(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.tls)
    }

    /// The state after the client's first bytes, `first`, or a refusal.
    fn after_first_write(&self, first: &[u8]) -> Result<State, Refusal> {
        Ok(match (first.starts_with(&SSL_REQUEST), self.rule) {
            (true, _) => State::Asked,
            (false, Rule::TlsOnly) => return Err(Refusal::PlainCancel),
            (false, Rule::Login) => State::Plain(Messages::default()),
            (false, Rule::Any) => State::Through,
        })
    }

    /// Reads the server's `bytes` in the current state; a refusal fails the read.
    fn after_read(&mut self, bytes: &[u8]) -> Result<(), Refusal> {
        let mut rest = bytes;
        if let State::Asked = self.state {
            let Some((&answer, after)) = rest.split_first() else { return Ok(()) };
            rest = after;
            self.state = match (answer, self.rule) {
                (b'S', _) => {
                    self.tls.store(true, Ordering::Release);
                    State::Through
                }
                (_, Rule::TlsOnly) => return Err(Refusal::PlainCancel),
                (_, Rule::Login) => State::Plain(Messages::default()),
                (_, Rule::Any) => State::Through,
            };
        }
        if let State::Plain(messages) = &mut self.state {
            match messages.read(rest) {
                Seen::Nothing => {}
                Seen::LoggedIn => self.state = State::Through,
                Seen::ClearPassword => return Err(Refusal::ClearPassword),
                Seen::Malformed => return Err(Refusal::Malformed),
            }
        }
        Ok(())
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Watched<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        if !matches!(self.state, State::Through) {
            let new = buf.filled().get(before..).unwrap_or_default();
            if let Err(refusal) = self.after_read(new) {
                // What the server sent stays unread: the caller sees the refusal instead.
                buf.set_filled(before);
                return Poll::Ready(Err(refusal.io()));
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Watched<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        if let State::Start = self.state
            && !buf.is_empty()
        {
            match self.after_first_write(buf) {
                Ok(next) => self.state = next,
                Err(refusal) => return Poll::Ready(Err(refusal.io())),
            }
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// What the server's login messages said so far.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    Nothing,
    /// AuthenticationOk: the login is over.
    LoggedIn,
    /// AuthenticationCleartextPassword.
    ClearPassword,
    Malformed,
}

/// The server's messages, as they arrive in pieces: each is a kind byte, a 4-byte length that
/// counts itself, and a body; an authentication request (`R`) starts its body with a 4-byte code.
#[derive(Debug, Default)]
struct Messages {
    /// The current message's kind, length and, for an `R`, its code, as far as they came.
    head: [u8; 9],
    have: usize,
    /// Bytes of the current message's body still to pass.
    skip: usize,
}

impl Messages {
    fn read(&mut self, mut bytes: &[u8]) -> Seen {
        loop {
            if self.skip > 0 {
                let passed = self.skip.min(bytes.len());
                self.skip -= passed;
                bytes = bytes.get(passed..).unwrap_or_default();
            }
            let Some((&byte, rest)) = bytes.split_first() else { return Seen::Nothing };
            bytes = rest;
            let Some(slot) = self.head.get_mut(self.have) else { return Seen::Malformed };
            *slot = byte;
            self.have += 1;
            let [kind, l0, l1, l2, l3, c0, c1, c2, c3] = self.head;
            let len = usize::try_from(u32::from_be_bytes([l0, l1, l2, l3])).unwrap_or(usize::MAX);
            match (kind, self.have) {
                (b'R', 5) if len < 8 => return Seen::Malformed,
                (b'R', 9) => match i32::from_be_bytes([c0, c1, c2, c3]) {
                    0 => return Seen::LoggedIn,
                    3 => return Seen::ClearPassword,
                    _ => self.next(len - 8),
                },
                (b'R', _) => {}
                (_, 5) => match len.checked_sub(4) {
                    Some(body) => self.next(body),
                    None => return Seen::Malformed,
                },
                _ => {}
            }
        }
    }

    /// The current message's head is read; `body` bytes of it follow.
    fn next(&mut self, body: usize) {
        self.skip = body;
        self.have = 0;
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn msg(kind: u8, body: &[u8]) -> Vec<u8> {
        [&[kind][..], &u32::try_from(body.len() + 4).unwrap().to_be_bytes(), body].concat()
    }

    fn auth(code: i32) -> Vec<u8> {
        msg(b'R', &code.to_be_bytes())
    }

    /// The server's bytes, read through a watch after the client sent `first`, `chunk` bytes at a
    /// time; what came through, and the refusal that ended the reading.
    async fn through(rule: Rule, first: &[u8], server: &[u8], chunk: usize) -> (Vec<u8>, Option<Refusal>, bool) {
        let (client, mut far) = tokio::io::duplex(64);
        let mut watched = Watched::new(client, rule);
        let tls = watched.tls();
        let sent = watched.write_all(first).await;
        if let Err(e) = sent {
            return (Vec::new(), e.get_ref().and_then(|r| r.downcast_ref::<Refusal>()).copied(), tls.load(Ordering::Acquire));
        }
        let mut seen = vec![0; first.len()];
        far.read_exact(&mut seen).await.unwrap();
        let mut got = Vec::new();
        for piece in server.chunks(chunk) {
            far.write_all(piece).await.unwrap();
            let mut buf = vec![0; piece.len()];
            let mut filled = 0;
            while filled < piece.len() {
                match watched.read(&mut buf[filled..]).await {
                    Ok(n) => filled += n,
                    Err(e) => return (got, e.get_ref().and_then(|r| r.downcast_ref::<Refusal>()).copied(), tls.load(Ordering::Acquire)),
                }
            }
            got.extend_from_slice(&buf);
        }
        (got, None, tls.load(Ordering::Acquire))
    }

    const STARTUP: &[u8] = &[0, 0, 0, 9, 0, 3, 0, 0, 0];

    #[tokio::test]
    async fn a_plain_login_stops_at_a_request_for_the_password_in_clear() {
        let notice = msg(b'N', b"SNOTICE\0Mhello\0\0");
        let server = [notice.clone(), auth(3)].concat();
        for chunk in [1, 2, 5, 9, 64] {
            let (got, refused, _) = through(Rule::Login, STARTUP, &server, chunk).await;
            assert_eq!(refused, Some(Refusal::ClearPassword), "{chunk}");
            // Never the request's last byte: the reader cannot act on it.
            assert!(got.len() < server.len(), "{chunk}: {got:?}");
        }
        // After N to the SSL request too.
        let (got, refused, tls) = through(Rule::Login, &SSL_REQUEST, &[b"N".as_slice(), &auth(3)].concat(), 1).await;
        assert_eq!((refused, tls), (Some(Refusal::ClearPassword), false), "{got:?}");
    }

    #[tokio::test]
    async fn md5_scram_and_everything_after_the_login_pass() {
        let mut server = [auth(5), msg(b'R', &[&10i32.to_be_bytes()[..], b"SCRAM-SHA-256\0\0"].concat()), msg(b'E', b"x")].concat();
        let after = auth(0);
        server.extend(&after);
        // After AuthenticationOk nothing is read: not even a later request.
        server.extend(auth(3));
        for chunk in [1, 3, 64] {
            let (got, refused, tls) = through(Rule::Login, STARTUP, &server, chunk).await;
            assert_eq!((refused, tls), (None, false), "{chunk}");
            assert_eq!(got, server, "{chunk}");
        }
    }

    #[tokio::test]
    async fn after_s_the_bytes_are_tls_and_pass() {
        let server = [b"S".as_slice(), &auth(3)].concat();
        let (got, refused, tls) = through(Rule::Login, &SSL_REQUEST, &server, 1).await;
        assert_eq!((got, refused, tls), (server, None, true));
        let (_, refused, tls) = through(Rule::TlsOnly, &SSL_REQUEST, b"S", 1).await;
        assert_eq!((refused, tls), (None, true));
    }

    #[tokio::test]
    async fn a_cancel_over_tls_goes_over_tls_or_not_at_all() {
        let cancel = [0, 0, 0, 16, 0x04, 0xD2, 0x16, 0x2E, 0, 0, 0, 42, 0, 0, 0, 7];
        assert_eq!(through(Rule::TlsOnly, &cancel, b"", 1).await.1, Some(Refusal::PlainCancel));
        assert_eq!(through(Rule::TlsOnly, &SSL_REQUEST, b"N", 1).await.1, Some(Refusal::PlainCancel));
        // Without the rule, a plain cancel goes.
        assert_eq!(through(Rule::Any, &cancel, b"", 1).await.1, None);
        assert_eq!(through(Rule::Any, &SSL_REQUEST, b"N", 1).await.1, None);
    }

    #[tokio::test]
    async fn short_lengths_are_malformed() {
        for server in [msg(b'R', &[0, 0]), [b"E".as_slice(), &[0, 0, 0, 3]].concat()] {
            assert_eq!(through(Rule::Login, STARTUP, &server, 1).await.1, Some(Refusal::Malformed), "{server:?}");
        }
    }
}
