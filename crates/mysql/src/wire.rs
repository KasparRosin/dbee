//! The wire: MySQL packets on a byte stream.
//!
//! A packet is a 3-byte little-endian payload length, a sequence id and the payload. A payload of
//! `0xFFFFFF` bytes continues in the next packet (one of exactly that length is followed by an
//! empty packet). The sequence id counts the packets of one exchange from 0, both ways.

use dbee_driver::DbError;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// A byte stream both ways; a trait object can name only one such trait.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// What packets travel on: a TCP stream, an SSH channel, or TLS over either.
pub type Stream = Box<dyn Io>;

/// The largest payload dbee reads after the login: 1 GiB, MySQL's own ceiling for
/// `max_allowed_packet`.
pub const MAX_PAYLOAD: usize = 1 << 30;

/// The largest payload of one packet; a payload this long continues in the next packet.
pub(crate) const CHUNK: usize = 0xFF_FFFF;

/// The largest packet out of turn dbee reads, to see whether it is the server's goodbye.
const MAX_GOODBYE: usize = 64 * 1024;

/// Framed packets on a stream, with the sequence ids of the exchange in progress.
pub struct Packets {
    io: BufReader<Stream>,
    seq: u8,
    max: usize,
}

impl std::fmt::Debug for Packets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Packets").field("seq", &self.seq).field("max", &self.max).finish()
    }
}

impl Packets {
    /// Packets on `stream`; a payload larger than `max` bytes is refused.
    pub fn new(stream: Stream, max: usize) -> Packets {
        Packets::resume(stream, 0, max)
    }

    /// Packets on a new `stream` (TLS over the old one) that go on with sequence id `seq`.
    pub(crate) fn resume(stream: Stream, seq: u8, max: usize) -> Packets {
        Packets { io: BufReader::new(stream), seq, max }
    }

    /// The stream and the next sequence id, for the TLS upgrade. Refused when the server sent
    /// bytes nobody asked for yet: they would bypass TLS.
    pub(crate) fn into_stream(self) -> Result<(Stream, u8), DbError> {
        if !self.io.buffer().is_empty() {
            return Err(DbError::Tls("the server sent data before TLS started".into()));
        }
        Ok((self.io.into_inner(), self.seq))
    }

    pub(crate) fn set_max(&mut self, max: usize) {
        self.max = max;
    }

    /// A new exchange (a command) starts: the next packet sent has sequence id 0.
    pub fn reset(&mut self) {
        self.seq = 0;
    }

    /// The next payload, joined from its packets. A broken or closed stream is
    /// [`DbError::ConnectionLost`], and so is an ERR packet out of turn with sequence id 0: the
    /// goodbye a server sends before it closes the connection (killed, timed out, shutting
    /// down). Any other packet out of sequence or a payload over the cap is [`DbError::Other`],
    /// checked from each header before its payload is read.
    pub async fn read(&mut self) -> Result<Vec<u8>, DbError> {
        self.read_up_to(self.max, "packet").await
    }

    /// [`Packets::read`], capped at `max` bytes as well; a payload over it is refused as a
    /// `what` larger than that.
    pub(crate) async fn read_up_to(&mut self, max: usize, what: &str) -> Result<Vec<u8>, DbError> {
        let max = max.min(self.max);
        let mut payload = Vec::new();
        loop {
            let mut header = [0u8; 4];
            self.io.read_exact(&mut header).await.map_err(|_| DbError::ConnectionLost)?;
            let [l0, l1, l2, seq] = header;
            let len = usize::from(l0) | usize::from(l1) << 8 | usize::from(l2) << 16;
            if seq != self.seq {
                if seq == 0 && payload.is_empty() && len <= MAX_GOODBYE && self.goodbye(len).await {
                    return Err(DbError::ConnectionLost);
                }
                return Err(DbError::Other(format!("the server sent packet {seq} where packet {} was due", self.seq)));
            }
            self.seq = self.seq.wrapping_add(1);
            if payload.len().saturating_add(len) > max {
                return Err(DbError::Other(format!("the server sent a {what} larger than {}", dbee_driver::size_text(max))));
            }
            let want = payload.len().saturating_add(len);
            (&mut self.io).take(len as u64).read_to_end(&mut payload).await.map_err(|_| DbError::ConnectionLost)?;
            if payload.len() < want {
                return Err(DbError::ConnectionLost);
            }
            if len < CHUNK {
                return Ok(payload);
            }
        }
    }

    /// Whether the `len` bytes of payload that follow are an ERR packet.
    async fn goodbye(&mut self, len: usize) -> bool {
        let mut payload = Vec::new();
        let read = (&mut self.io).take(len as u64).read_to_end(&mut payload).await;
        read.is_ok() && payload.first() == Some(&0xFF)
    }

    /// Send `payload`, split into packets as long as it needs.
    pub async fn write(&mut self, payload: &[u8]) -> Result<(), DbError> {
        let mut rest = payload;
        loop {
            let (chunk, tail) = rest.split_at(rest.len().min(CHUNK));
            let len = chunk.len();
            let mut frame = Vec::with_capacity(len.saturating_add(4));
            frame.extend_from_slice(&[(len & 0xFF) as u8, (len >> 8 & 0xFF) as u8, (len >> 16 & 0xFF) as u8, self.seq]);
            frame.extend_from_slice(chunk);
            self.seq = self.seq.wrapping_add(1);
            self.io.get_mut().write_all(&frame).await.map_err(|_| DbError::ConnectionLost)?;
            rest = tail;
            if chunk.len() < CHUNK {
                break;
            }
        }
        self.io.get_mut().flush().await.map_err(|_| DbError::ConnectionLost)
    }
}

/// The error for a packet that is not as MySQL's documentation lays it out.
pub(crate) fn malformed(what: &str) -> DbError {
    DbError::Other(format!("the server sent a malformed {what}"))
}

/// The length-encoded string at the head of `buf`, and the bytes after it.
pub(crate) fn lenenc_str(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    let (len, rest) = lenenc_int(buf)?;
    rest.split_at_checked(usize::try_from(len).ok()?)
}

/// The length-encoded integer at the head of `buf`, and the bytes after it.
pub(crate) fn lenenc_int(buf: &[u8]) -> Option<(u64, &[u8])> {
    let (&first, rest) = buf.split_first()?;
    let width = match first {
        0..=0xFA => return Some((u64::from(first), rest)),
        0xFC => 2,
        0xFD => 3,
        0xFE => 8,
        _ => return None,
    };
    let (bytes, rest) = rest.split_at_checked(width)?;
    let mut le = [0u8; 8];
    le.get_mut(..width)?.copy_from_slice(bytes);
    Some((u64::from_le_bytes(le), rest))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A stream over `input` that collects what is written into `output`.
    pub(crate) struct Duplex {
        input: std::io::Cursor<Vec<u8>>,
        output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl AsyncRead for Duplex {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.input).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for Duplex {
        fn poll_write(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> {
            self.output.lock().unwrap().extend_from_slice(buf);
            std::task::Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    pub(crate) fn packets(input: Vec<u8>) -> (Packets, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stream = Duplex { input: std::io::Cursor::new(input), output: std::sync::Arc::clone(&output) };
        (Packets::new(Box::new(stream), MAX_PAYLOAD), output)
    }

    pub(crate) fn frame(payload: &[u8], seq: u8) -> Vec<u8> {
        [&payload.len().to_le_bytes()[..3], &[seq], payload].concat()
    }

    #[tokio::test]
    async fn long_payloads_split_and_join() {
        let long = vec![7u8; CHUNK + 10];
        let (mut p, out) = packets(Vec::new());
        p.write(&long).await.unwrap();
        p.write(&[1, 2]).await.unwrap();
        let sent = out.lock().unwrap().clone();
        let expected = [frame(&long[..CHUNK], 0), frame(&long[CHUNK..], 1), frame(&[1, 2], 2)].concat();
        assert!(sent == expected, "{} bytes sent", sent.len());

        // Exactly 0xFFFFFF bytes: an empty packet follows, both ways.
        let exact = vec![9u8; CHUNK];
        let (mut p, out) = packets([frame(&exact, 0), frame(&[], 1), frame(b"next", 2)].concat());
        assert!(p.read().await.unwrap() == exact);
        assert_eq!(p.read().await.unwrap(), b"next");
        p.write(&exact).await.unwrap();
        assert!(*out.lock().unwrap() == [frame(&exact, 3), frame(&[], 4)].concat());
    }

    #[tokio::test]
    async fn sequence_ids_are_checked_and_reset() {
        let (mut p, out) = packets([frame(b"a", 0), frame(b"b", 5)].concat());
        assert_eq!(p.read().await.unwrap(), b"a");
        let e = p.read().await.unwrap_err();
        assert!(matches!(&e, DbError::Other(m) if m.contains("packet 5 where packet 1")), "{e:?}");
        p.reset();
        p.write(b"q").await.unwrap();
        assert_eq!(*out.lock().unwrap(), frame(b"q", 0));
    }

    #[tokio::test]
    async fn an_error_out_of_turn_is_the_servers_goodbye() {
        // MySQL 8 and newer send ERR 4031 as packet 0 before they close an idle connection.
        let goodbye = [&[0xFF, 0xBF, 0x0F][..], b"#HY000The client was disconnected by the server because of inactivity."].concat();
        let answer = |packet: Vec<u8>| async move {
            let (mut p, _) = packets(packet);
            p.write(b"\x03SELECT 1").await.unwrap();
            p.read().await
        };
        assert_eq!(answer(frame(&goodbye, 0)).await.unwrap_err(), DbError::ConnectionLost);
        // Anything else out of turn is the server out of step.
        for other in [frame(&goodbye, 5), frame(b"\x01\x02", 0), frame(&[0xFF; MAX_GOODBYE + 1], 0)] {
            assert_eq!(answer(other).await.unwrap_err().code(), "other");
        }
        assert_eq!(answer(frame(&goodbye, 1)).await.unwrap(), goodbye, "in turn it is the answer");
    }

    #[tokio::test]
    async fn a_short_stream_is_a_lost_connection() {
        for input in [Vec::new(), vec![5, 0], [&[5, 0, 0, 0][..], b"abc"].concat()] {
            let (mut p, _) = packets(input);
            assert_eq!(p.read().await.unwrap_err(), DbError::ConnectionLost);
        }
    }

    #[tokio::test]
    async fn bytes_before_tls_are_refused() {
        let (mut p, _) = packets([frame(b"greeting", 0), b"extra".to_vec()].concat());
        p.read().await.unwrap();
        let e = p.into_stream().err().unwrap();
        assert_eq!(e.code(), "tls");
        let (mut p, _) = packets(frame(b"greeting", 0));
        p.read().await.unwrap();
        assert_eq!(p.into_stream().map(|(_, seq)| seq).ok(), Some(1));
    }

    #[test]
    fn lenenc_ints() {
        assert_eq!(lenenc_int(&[0xFA, 1]), Some((250, &[1u8][..])));
        assert_eq!(lenenc_int(&[0xFC, 0x34, 0x12]), Some((0x1234, &[][..])));
        assert_eq!(lenenc_int(&[0xFD, 1, 2, 3]), Some((0x030201, &[][..])));
        assert_eq!(lenenc_int(&[0xFE, 1, 0, 0, 0, 0, 0, 0, 0x80]), Some((0x8000_0000_0000_0001, &[][..])));
        for bad in [&[][..], &[0xFB], &[0xFF], &[0xFC, 1], &[0xFE, 1, 2, 3]] {
            assert_eq!(lenenc_int(bad), None, "{bad:?}");
        }
    }
}
