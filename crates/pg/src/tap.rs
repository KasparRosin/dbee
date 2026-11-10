//! The transaction status Postgres reports in each ReadyForQuery (`I` idle, `T` in a transaction,
//! `E` in a failed one), read off the connection's stream as it passes: tokio-postgres reads the
//! message but keeps the status to itself.
//!
//! A [`Tap`] sits where the stream's bytes are the protocol's own: on the plain stream, where it
//! stops looking once the server agreed to TLS, and on the stream the TLS connector returns
//! ([`TapConnect`]). It only reads: every byte passes unchanged, in order, at once. It follows the
//! framing (a kind byte and a 4-byte length that counts itself) and skips bodies without keeping
//! them, so its memory stays the same whatever the server sends. A length it cannot believe, or a
//! status byte that is none of the three, stops it, and the state reads unknown from then on; the
//! connection itself goes on.
//!
//! A ReadyForQuery ends the answer to each startup, Query, Sync and FunctionCall the client sent,
//! so the tap counts those on the way out too: the status is the session's once every request
//! sent has its ReadyForQuery ([`state`]). tokio-postgres reads some answers after the call that
//! asked has returned (a prepared statement's, and its Close's).

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};
use std::time::Duration;

use dbee_driver::TransactionState;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::watch;
use tokio_postgres::tls::{ChannelBinding, TlsConnect, TlsStream};

/// The longest message the tap believes: Postgres builds each in one buffer of at most 1 GiB, and
/// the length counts its own 4 bytes.
const MAX_LEN: usize = (1 << 30) + 4;

/// The codes of the requests that come before the startup message and get a one-byte answer:
/// SSLRequest and GSSENCRequest.
const SSL_REQUEST: u32 = 80_877_103;
const GSSENC_REQUEST: u32 = 80_877_104;

/// What the taps of one connection have seen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    /// Requests sent that a ReadyForQuery answers: the startup message, each Query, Sync and
    /// FunctionCall.
    asked: u64,
    /// ReadyForQuery messages read.
    answered: u64,
    /// The status byte of the last one.
    status: Option<u8>,
    /// The server sent what the protocol does not allow: the tap stopped.
    lost: bool,
}

impl Seen {
    /// Every request sent has its answer, or the tap stopped.
    fn settled(&self) -> bool {
        self.lost || self.answered >= self.asked
    }

    /// The state the last ReadyForQuery reported, once settled.
    fn state(&self) -> TransactionState {
        match (self.lost, self.status) {
            (false, Some(b'I')) => TransactionState::Idle,
            (false, Some(b'T')) => TransactionState::Open,
            (false, Some(b'E')) => TransactionState::Failed,
            _ => TransactionState::Unknown,
        }
    }
}

/// What a connection's taps share: what they saw, and whether TLS has started (the plain stream's
/// tap then stops looking).
#[derive(Clone)]
pub(crate) struct Shared {
    seen: watch::Sender<Seen>,
    tls: Arc<AtomicBool>,
}

/// A connection's taps' shared state, and the receiver [`state`] reads.
pub(crate) fn channel() -> (Shared, watch::Receiver<Seen>) {
    let (seen, status) = watch::channel(Seen::default());
    (Shared { seen, tls: Arc::default() }, status)
}

/// How long [`state`] waits for the answers still on their way: they follow a statement that has
/// ended, so this is only reached when the server stopped answering.
const SETTLE_LIMIT: Duration = Duration::from_secs(15);

/// The session's transaction state, as the last ReadyForQuery said, once every request sent has its
/// answer. `Unknown` when the tap stopped, the connection ended, or the answers do not come within
/// [`SETTLE_LIMIT`].
pub(crate) async fn state(status: &watch::Receiver<Seen>) -> TransactionState {
    let mut status = status.clone();
    match tokio::time::timeout(SETTLE_LIMIT, status.wait_for(Seen::settled)).await {
        Ok(Ok(seen)) => seen.state(),
        Ok(Err(_)) | Err(_) => TransactionState::Unknown,
    }
}

/// One direction's framing, fed in pieces: a head (kind and length, or for the untyped messages
/// before the startup's end, length and code), then a body to skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// `have` bytes of the head are in.
    Head { have: usize },
    /// `left` bytes of the body still to pass.
    Body { left: usize },
    /// The next byte is a ReadyForQuery's status.
    Status,
    /// The next byte answers an SSLRequest or GSSENCRequest.
    Answer,
    /// Not looking: TLS from here on, or the tap stopped.
    Off,
}

/// What one piece of bytes changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct Change {
    asked: u64,
    /// The status bytes of the ReadyForQuery messages read, the last one kept.
    answered: u64,
    status: Option<u8>,
    lost: bool,
    tls: bool,
}

/// The client's messages: untyped ones (length, code) until the startup message, then typed ones.
#[derive(Debug)]
struct Frontend {
    step: Step,
    head: [u8; 8],
    typed: bool,
}

impl Default for Frontend {
    fn default() -> Frontend {
        Frontend { step: Step::Head { have: 0 }, head: [0; 8], typed: false }
    }
}

impl Frontend {
    /// Reads `bytes`, the client's; whether an answer byte is due from the server next.
    fn write(&mut self, mut bytes: &[u8], change: &mut Change) -> bool {
        let mut answer_due = false;
        loop {
            match self.step {
                Step::Off | Step::Status | Step::Answer => return answer_due,
                Step::Body { left } => {
                    let passed = left.min(bytes.len());
                    bytes = bytes.get(passed..).unwrap_or_default();
                    if passed < left {
                        self.step = Step::Body { left: left - passed };
                        return answer_due;
                    }
                    self.step = Step::Head { have: 0 };
                }
                Step::Head { have } => {
                    let Some((&byte, rest)) = bytes.split_first() else { return answer_due };
                    bytes = rest;
                    let Some(slot) = self.head.get_mut(have) else {
                        self.step = Step::Off;
                        return answer_due;
                    };
                    *slot = byte;
                    let have = have + 1;
                    let need = if self.typed { 5 } else { 8 };
                    if have < need {
                        self.step = Step::Head { have };
                        continue;
                    }
                    // Typed: kind, then the length. Untyped: the length, then the code.
                    let [b0, b1, b2, b3, b4, b5, b6, b7] = self.head;
                    let (len, head) = if self.typed { (u32::from_be_bytes([b1, b2, b3, b4]), 4) } else { (u32::from_be_bytes([b0, b1, b2, b3]), 8) };
                    let len = usize::try_from(len).unwrap_or(usize::MAX);
                    let Some(body) = len.checked_sub(head).filter(|_| len <= MAX_LEN) else {
                        // The client's own bytes, which tokio-postgres frames: this does not
                        // happen, and if it did the count would be wrong, so the tap stops.
                        change.lost = true;
                        self.step = Step::Off;
                        return answer_due;
                    };
                    if self.typed {
                        if matches!(b0, b'Q' | b'S' | b'F') {
                            change.asked += 1;
                        }
                    } else {
                        match u32::from_be_bytes([b4, b5, b6, b7]) {
                            SSL_REQUEST | GSSENC_REQUEST => answer_due = true,
                            _ => {
                                change.asked += 1;
                                self.typed = true;
                            }
                        }
                    }
                    self.step = if body == 0 { Step::Head { have: 0 } } else { Step::Body { left: body } };
                }
            }
        }
    }
}

/// The server's messages: typed, after the one-byte answer to an SSLRequest when one is due.
#[derive(Debug)]
struct Backend {
    step: Step,
    head: [u8; 5],
}

impl Default for Backend {
    fn default() -> Backend {
        Backend { step: Step::Head { have: 0 }, head: [0; 5] }
    }
}

impl Backend {
    /// The next byte answers the client's SSLRequest or GSSENCRequest.
    fn answer_due(&mut self) {
        if self.step == (Step::Head { have: 0 }) {
            self.step = Step::Answer;
        }
    }

    /// Reads `bytes`, the server's.
    fn read(&mut self, mut bytes: &[u8], change: &mut Change) {
        loop {
            if let Step::Body { left } = self.step {
                let passed = left.min(bytes.len());
                bytes = bytes.get(passed..).unwrap_or_default();
                if passed < left {
                    self.step = Step::Body { left: left - passed };
                    return;
                }
                self.step = Step::Head { have: 0 };
            }
            let Some((&byte, rest)) = bytes.split_first() else { return };
            bytes = rest;
            self.step = match self.step {
                Step::Off | Step::Body { .. } => return,
                // `S` and `G` start TLS or GSS encryption: none of the protocol's bytes from here.
                Step::Answer => match byte {
                    b'N' => Step::Head { have: 0 },
                    b'S' | b'G' => {
                        change.tls = true;
                        Step::Off
                    }
                    _ => stop(change),
                },
                Step::Status => match byte {
                    b'I' | b'T' | b'E' => {
                        change.answered += 1;
                        change.status = Some(byte);
                        Step::Head { have: 0 }
                    }
                    _ => stop(change),
                },
                Step::Head { have } => {
                    let Some(slot) = self.head.get_mut(have) else { return };
                    *slot = byte;
                    let have = have + 1;
                    if have < self.head.len() {
                        Step::Head { have }
                    } else {
                        let [kind, l0, l1, l2, l3] = self.head;
                        let len = usize::try_from(u32::from_be_bytes([l0, l1, l2, l3])).unwrap_or(usize::MAX);
                        match (kind, len) {
                            (_, len) if !(4..=MAX_LEN).contains(&len) => stop(change),
                            // ReadyForQuery: its body is the status byte alone.
                            (b'Z', 5) => Step::Status,
                            (b'Z', _) => stop(change),
                            (_, 4) => Step::Head { have: 0 },
                            (_, len) => Step::Body { left: len - 4 },
                        }
                    }
                }
            };
        }
    }
}

/// The server sent what the protocol does not allow: the tap stops.
fn stop(change: &mut Change) -> Step {
    change.lost = true;
    Step::Off
}

/// Where a tap sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// On the plain stream: it stops looking once TLS starts.
    Plain,
    /// On the stream TLS returns, from the startup message on.
    AboveTls,
}

/// A stream whose bytes the tap reads as they pass ([module docs](self)).
pub(crate) struct Tap<S> {
    inner: S,
    place: Place,
    frontend: Frontend,
    backend: Backend,
    shared: Shared,
}

impl<S> Tap<S> {
    /// The tap on the plain stream.
    pub(crate) fn plain(inner: S, shared: Shared) -> Tap<S> {
        Tap { inner, place: Place::Plain, frontend: Frontend::default(), backend: Backend::default(), shared }
    }

    fn above_tls(inner: S, shared: Shared) -> Tap<S> {
        Tap { inner, place: Place::AboveTls, frontend: Frontend::default(), backend: Backend::default(), shared }
    }

    fn looking(&self) -> bool {
        self.place == Place::AboveTls || !self.shared.tls.load(Ordering::Acquire)
    }

    /// Hands what one piece changed to the shared state. A ReadyForQuery that answers no request
    /// stops the tap: the server answered what was not asked.
    fn tell(&mut self, change: Change) {
        if change.tls {
            self.shared.tls.store(true, Ordering::Release);
        }
        if change.asked == 0 && change.answered == 0 && !change.lost {
            return;
        }
        self.shared.seen.send_modify(|seen| {
            seen.asked = seen.asked.saturating_add(change.asked);
            seen.answered = seen.answered.saturating_add(change.answered);
            if change.status.is_some() {
                seen.status = change.status;
            }
            seen.lost |= change.lost || seen.answered > seen.asked;
        });
        if self.shared.seen.borrow().lost {
            self.frontend.step = Step::Off;
            self.backend.step = Step::Off;
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Tap<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        ready!(Pin::new(&mut self.inner).poll_read(cx, buf))?;
        if self.looking() {
            let mut change = Change::default();
            self.backend.read(buf.filled().get(before..).unwrap_or_default(), &mut change);
            self.tell(change);
        }
        Poll::Ready(Ok(()))
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tap<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let n = ready!(Pin::new(&mut self.inner).poll_write(cx, buf))?;
        if self.looking() {
            let mut change = Change::default();
            if self.frontend.write(buf.get(..n).unwrap_or_default(), &mut change) {
                self.backend.answer_due();
            }
            self.tell(change);
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<S: TlsStream + Unpin> TlsStream for Tap<S> {
    fn channel_binding(&self) -> ChannelBinding {
        self.inner.channel_binding()
    }
}

/// A TLS connector whose stream is tapped above TLS. Starting TLS stops the plain stream's tap.
pub(crate) struct TapConnect<T> {
    inner: T,
    shared: Shared,
}

impl<T> TapConnect<T> {
    pub(crate) fn new(inner: T, shared: Shared) -> TapConnect<T> {
        TapConnect { inner, shared }
    }
}

impl<S, T> TlsConnect<S> for TapConnect<T>
where
    T: TlsConnect<S>,
    T::Future: Send + 'static,
    T::Stream: Unpin,
{
    type Stream = Tap<T::Stream>;
    type Error = T::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Tap<T::Stream>, T::Error>> + Send>>;

    fn connect(self, stream: S) -> Self::Future {
        let TapConnect { inner, shared } = self;
        shared.tls.store(true, Ordering::Release);
        let handshake = inner.connect(stream);
        Box::pin(async move { handshake.await.map(|tls| Tap::above_tls(tls, shared)) })
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn msg(kind: u8, body: &[u8]) -> Vec<u8> {
        [&[kind][..], &u32::try_from(body.len() + 4).unwrap().to_be_bytes(), body].concat()
    }

    fn ready(status: u8) -> Vec<u8> {
        msg(b'Z', &[status])
    }

    const STARTUP: &[u8] = &[0, 0, 0, 13, 0, 3, 0, 0, b'u', b's', b'e', b'r', 0];
    const SSL: &[u8] = &[0, 0, 0, 8, 0x04, 0xD2, 0x16, 0x2F];

    /// The client's `sent`, then the server's `got`, fed through fresh parsers `chunk` bytes at a
    /// time, as `Tap` feeds them; what they saw.
    fn feed(sent: &[u8], got: &[u8], chunk: usize) -> Seen {
        let (mut front, mut back) = (Frontend::default(), Backend::default());
        let mut seen = Seen::default();
        let apply = |change: Change, seen: &mut Seen| {
            seen.asked += change.asked;
            seen.answered += change.answered;
            seen.status = change.status.or(seen.status);
            seen.lost |= change.lost || seen.answered > seen.asked;
        };
        for piece in sent.chunks(chunk) {
            let mut change = Change::default();
            if front.write(piece, &mut change) {
                back.answer_due();
            }
            apply(change, &mut seen);
        }
        for piece in got.chunks(chunk) {
            let mut change = Change::default();
            back.read(piece, &mut change);
            apply(change, &mut seen);
        }
        seen
    }

    /// A login and a few runs: the startup and two queries, a prepare with its Sync, as the
    /// client sends them; the server's login, a notice, rows and ReadyForQuery messages.
    fn session(statuses: [u8; 4]) -> (Vec<u8>, Vec<u8>) {
        let sent = [
            STARTUP.to_vec(),
            msg(b'p', b"secret\0"),
            msg(b'Q', b"BEGIN\0"),
            msg(b'Q', b"SELECT 1\0"),
            msg(b'P', b"s0\0SELECT 1\0\0\0"),
            msg(b'D', b"Ss0\0"),
            msg(b'S', b""),
        ]
        .concat();
        let got = [
            msg(b'R', &3i32.to_be_bytes()),
            msg(b'R', &0i32.to_be_bytes()),
            msg(b'S', b"client_encoding\0UTF8\0"),
            msg(b'K', &[0, 0, 0, 1, 0, 0, 0, 2]),
            ready(statuses[0]),
            msg(b'C', b"BEGIN\0"),
            ready(statuses[1]),
            msg(b'N', b"SNOTICE\0Mhello\0\0"),
            msg(b'T', &[0, 1, b'a', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 23, 0, 4, 255, 255, 255, 255, 0, 0]),
            msg(b'D', &[0, 1, 0, 0, 0, 1, b'1']),
            msg(b'C', b"SELECT 1\0"),
            ready(statuses[2]),
            msg(b'1', b""),
            msg(b't', &[0, 0]),
            msg(b'T', &[0, 0]),
            ready(statuses[3]),
        ]
        .concat();
        (sent, got)
    }

    #[test]
    fn every_chunk_size_reads_the_same_status() {
        let (sent, got) = session(*b"ITTT");
        let want = Seen { asked: 4, answered: 4, status: Some(b'T'), lost: false };
        for chunk in 1..=sent.len().max(got.len()) {
            assert_eq!(feed(&sent, &got, chunk), want, "{chunk}");
        }
        assert_eq!(want.state(), TransactionState::Open);
        // After an SSLRequest the server's `N` comes first; the status reads the same.
        let plain = [SSL, &sent].concat();
        let got_n = [b"N".as_slice(), &got].concat();
        for chunk in 1..=plain.len() {
            assert_eq!(feed(&plain, &got_n, chunk), want, "{chunk}");
        }
        let (sent, got) = session(*b"ITEI");
        assert_eq!(feed(&sent, &got, 3).state(), TransactionState::Idle);
        let (sent, got) = session(*b"ITTE");
        assert_eq!(feed(&sent, &got, 7).state(), TransactionState::Failed);
    }

    #[test]
    fn an_answer_still_on_its_way_is_waited_for() {
        let (sent, got) = session(*b"ITTT");
        let last = ready(b'T').len();
        let short = feed(&sent, &got[..got.len() - last], 1);
        assert!(!short.settled() && !short.lost, "{short:?}");
        assert!(feed(&sent, &got, 1).settled());
    }

    #[test]
    fn a_length_it_cannot_believe_stops_it_for_good() {
        let (sent, got) = session(*b"ITTT");
        let bad = [
            // Shorter than the length itself.
            [b"D".as_slice(), &[0, 0, 0, 3]].concat(),
            // Past what Postgres can build.
            [b"D".as_slice(), &[0x7F, 0xFF, 0xFF, 0xFF]].concat(),
            // A ReadyForQuery of another length, or with another status.
            msg(b'Z', b"TT"),
            msg(b'Z', b"X"),
        ];
        for lie in bad {
            // The lie comes before the last ReadyForQuery: what follows is never read as one.
            let at = got.len() - ready(b'T').len();
            let lied = [&got[..at], &lie, &ready(b'I')].concat();
            for chunk in [1, 2, 5, 64] {
                let seen = feed(&sent, &lied, chunk);
                assert!(seen.lost && seen.settled(), "{lie:?} {chunk}: {seen:?}");
                assert_eq!(seen.state(), TransactionState::Unknown, "{lie:?}");
            }
        }
        // A ReadyForQuery that answers nothing asked.
        let extra = [&got[..], &ready(b'I')].concat();
        assert_eq!(feed(&sent, &extra, 4).state(), TransactionState::Unknown);
        // An answer to the SSLRequest other than S, N or G.
        assert!(feed(&[SSL, &sent].concat(), &[b"E".as_slice(), &got].concat(), 1).lost);
    }

    #[test]
    fn after_s_the_plain_stream_is_not_read() {
        let (mut back, mut front) = (Backend::default(), Frontend::default());
        let mut change = Change::default();
        assert!(front.write(SSL, &mut change));
        back.answer_due();
        // `S`, then TLS's bytes, which may look like anything.
        back.read(&[b"S".as_slice(), &ready(b'I')].concat(), &mut change);
        assert_eq!(change, Change { tls: true, ..Change::default() });
    }

    /// Through the stream: every byte passes as it came, both ways, and the status is shared.
    #[tokio::test]
    async fn the_tap_passes_every_byte_as_it_came() {
        let (sent, got) = session(*b"ITTE");
        for chunk in [1, 3, 64] {
            let (client, mut server) = tokio::io::duplex(16);
            let (shared, status) = channel();
            let mut tap = Tap::plain(client, shared);
            let writing = async {
                for piece in sent.chunks(chunk) {
                    tap.write_all(piece).await.unwrap();
                }
                let mut back = vec![0; got.len()];
                tap.read_exact(&mut back).await.unwrap();
                back
            };
            let serving = async {
                let mut asked = vec![0; sent.len()];
                server.read_exact(&mut asked).await.unwrap();
                for piece in got.chunks(chunk) {
                    server.write_all(piece).await.unwrap();
                }
                asked
            };
            let (back, asked) = tokio::join!(writing, serving);
            assert_eq!((asked, back), (sent.clone(), got.clone()), "{chunk}");
            assert_eq!(state(&status).await, TransactionState::Failed, "{chunk}");
        }
        // With the taps gone, nothing is waited for.
        let (shared, status) = channel();
        shared.seen.send_modify(|s| s.asked = 1);
        drop(shared);
        assert_eq!(state(&status).await, TransactionState::Unknown);
    }
}
