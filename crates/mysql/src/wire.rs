//! The wire: MySQL packets on a byte stream, and the server's answer to a command.
//!
//! A packet is a 3-byte little-endian payload length, a sequence id and the payload. A payload of
//! `0xFFFFFF` bytes continues in the next packet (one of exactly that length is followed by an
//! empty packet). The sequence id counts the packets of one exchange from 0, both ways.

use dbee_driver::{DbError, Limits};
use mysql_common::constants::{CapabilityFlags, StatusFlags};
use mysql_common::io::ParseBuf;
use mysql_common::packets::{Column, CommonOkPacket, OkPacket, OkPacketDeserializer, OkPacketKind, OldEofPacket, ResultSetTerminator};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::errors;
use crate::flavor::Flavor;

/// A byte stream both ways; a trait object can name only one such trait.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// What packets travel on: a TCP stream, an SSH channel, or TLS over either.
pub type Stream = Box<dyn Io>;

/// The largest payload dbee reads after the login: 1 GiB, MySQL's own ceiling for
/// `max_allowed_packet`.
pub const MAX_PAYLOAD: usize = 1 << 30;

/// The largest payload of one packet; a payload this long continues in the next packet.
const CHUNK: usize = 0xFF_FFFF;

/// More columns than any table can have (MySQL's limit is 4096).
const MAX_COLUMNS: u64 = 65_535;

const MIB: usize = 1024 * 1024;

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
    /// [`DbError::ConnectionLost`]; a packet out of sequence or a payload over the cap is
    /// [`DbError::Other`], checked from each header before its payload is read.
    pub async fn read(&mut self) -> Result<Vec<u8>, DbError> {
        let mut payload = Vec::new();
        loop {
            let mut header = [0u8; 4];
            self.io.read_exact(&mut header).await.map_err(|_| DbError::ConnectionLost)?;
            let [l0, l1, l2, seq] = header;
            let len = usize::from(l0) | usize::from(l1) << 8 | usize::from(l2) << 16;
            if seq != self.seq {
                return Err(DbError::Other(format!("the server sent packet {seq} where packet {} was due", self.seq)));
            }
            self.seq = self.seq.wrapping_add(1);
            if payload.len().saturating_add(len) > self.max {
                return Err(DbError::Other(format!("the server sent a packet larger than {}", size_text(self.max))));
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

fn size_text(bytes: usize) -> String {
    if bytes >= MIB { format!("{} MiB", bytes / MIB) } else { format!("{} KiB", bytes / 1024) }
}

fn malformed(what: &str) -> DbError {
    DbError::Other(format!("the server sent a malformed {what}"))
}

/// An OK packet of kind `K` (`0x00` after a command, `0xFE` at the end of a result set).
pub(crate) fn parse_ok<K: OkPacketKind>(payload: &[u8], caps: CapabilityFlags) -> Result<OkPacket<'_>, DbError> {
    ParseBuf(payload).parse::<OkPacketDeserializer<'_, K>>(caps).map(OkPacketDeserializer::into_inner).map_err(|_| malformed("OK packet"))
}

/// The answer to one command as text: the last result set's columns and rows (lossy UTF-8), and
/// the last statement's affected rows. Rows past the caps are read but not kept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// Every row the server sent, kept or not.
    pub rows_seen: u64,
    /// No row was left out for the caps.
    pub complete: bool,
    /// The affected rows of the last OK packet: matched rows, as dbee logs in with
    /// `CLIENT_FOUND_ROWS`.
    pub affected_rows: u64,
}

/// Read the answer to a command: an OK packet, an ERR packet, or result sets, as many as the
/// server says follow. An ERR packet ends it with its error.
pub(crate) async fn read_result(p: &mut Packets, caps: CapabilityFlags, flavor: Flavor, limits: Limits) -> Result<TextResult, DbError> {
    let mut kept: Option<TextResult> = None;
    let mut affected = 0;
    loop {
        let first = p.read().await?;
        let status = match first.first() {
            Some(0x00) => {
                let ok = parse_ok::<CommonOkPacket>(&first, caps)?;
                affected = ok.affected_rows();
                ok.status_flags()
            }
            Some(0xFF) => return Err(errors::from_err_packet(&first, caps, flavor)),
            Some(0xFB) => return Err(DbError::Other("the server asked for a local file, which dbee never sends".into())),
            _ => {
                let (set, status) = read_rows(p, &first, caps, flavor, limits).await?;
                kept = Some(set);
                status
            }
        };
        if !status.contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS) {
            break;
        }
    }
    let mut result = kept.unwrap_or(TextResult { complete: true, ..TextResult::default() });
    result.affected_rows = affected;
    Ok(result)
}

/// One result set after its column count `first`: the column definitions, then the rows until
/// the terminator, whose status says whether more results follow.
async fn read_rows(
    p: &mut Packets,
    first: &[u8],
    caps: CapabilityFlags,
    flavor: Flavor,
    limits: Limits,
) -> Result<(TextResult, StatusFlags), DbError> {
    let count = match lenenc_int(first) {
        Some((n, [])) if (1..=MAX_COLUMNS).contains(&n) => usize::try_from(n).map_err(|_| malformed("column count"))?,
        _ => return Err(malformed("column count")),
    };
    let mut columns = Vec::new();
    for _ in 0..count {
        let def = p.read().await?;
        let column = ParseBuf(&def).parse::<Column>(()).map_err(|_| malformed("column definition"))?;
        columns.push(column.name_str().into_owned());
    }
    let old_eof = !caps.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF);
    if old_eof {
        let eof = p.read().await?;
        if eof.first() != Some(&0xFE) {
            return Err(malformed("end of the column definitions"));
        }
        parse_ok::<OldEofPacket>(&eof, caps)?;
    }
    let mut set = TextResult { columns, complete: true, ..TextResult::default() };
    let mut bytes: usize = 0;
    loop {
        let packet = p.read().await?;
        match packet.first() {
            Some(0xFF) => return Err(errors::from_err_packet(&packet, caps, flavor)),
            // A row starting 0xFE has a first value of 16 MiB or more, so it is that long too.
            Some(0xFE) if packet.len() < CHUNK => {
                let end = if old_eof { parse_ok::<OldEofPacket>(&packet, caps)? } else { parse_ok::<ResultSetTerminator>(&packet, caps)? };
                return Ok((set, end.status_flags()));
            }
            _ => {}
        }
        set.rows_seen = set.rows_seen.saturating_add(1);
        if !set.complete {
            continue;
        }
        let row = parse_row(&packet, count)?;
        let row_bytes = row.iter().fold(0usize, |sum, cell| sum.saturating_add(cell.map_or(0, <[u8]>::len)));
        if set.rows.len() >= limits.max_rows || bytes.saturating_add(row_bytes) > limits.max_bytes {
            set.complete = false;
            continue;
        }
        bytes = bytes.saturating_add(row_bytes);
        set.rows.push(row.into_iter().map(|cell| cell.map(|v| String::from_utf8_lossy(v).into_owned())).collect());
    }
}

/// The length-encoded integer at the head of `buf`, and the bytes after it.
fn lenenc_int(buf: &[u8]) -> Option<(u64, &[u8])> {
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

/// The values of a text row of `count` columns; `None` is SQL NULL. The row must hold exactly
/// that many values.
fn parse_row(payload: &[u8], count: usize) -> Result<Vec<Option<&[u8]>>, DbError> {
    let mut rest = payload;
    let mut cells = Vec::with_capacity(count);
    for _ in 0..count {
        if let Some(tail) = rest.strip_prefix(&[0xFB]) {
            cells.push(None);
            rest = tail;
            continue;
        }
        let (len, tail) = lenenc_int(rest).ok_or_else(|| malformed("row"))?;
        let len = usize::try_from(len).map_err(|_| malformed("row"))?;
        let (value, tail) = tail.split_at_checked(len).ok_or_else(|| malformed("row"))?;
        cells.push(Some(value));
        rest = tail;
    }
    if !rest.is_empty() {
        return Err(malformed("row"));
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// A stream over `input` that collects what is written into `output`.
    struct Duplex {
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

    fn packets(input: Vec<u8>) -> (Packets, std::sync::Arc<std::sync::Mutex<Vec<u8>>>) {
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stream = Duplex { input: std::io::Cursor::new(input), output: std::sync::Arc::clone(&output) };
        (Packets::new(Box::new(stream), MAX_PAYLOAD), output)
    }

    fn frame(payload: &[u8], seq: u8) -> Vec<u8> {
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

    /// Every parser of mysql_common's that reads server bytes, on `payload`, with `caps`.
    fn parse_everything(payload: &[u8], caps: CapabilityFlags) {
        let _ = parse_ok::<CommonOkPacket>(payload, caps);
        let _ = parse_ok::<ResultSetTerminator>(payload, caps);
        let _ = parse_ok::<OldEofPacket>(payload, caps);
        let _ = errors::from_err_packet(payload, caps, Flavor::MySql);
        let _ = ParseBuf(payload).parse::<Column>(());
        let _ = ParseBuf(payload).parse::<mysql_common::packets::AuthSwitchRequest>(());
        let _ = ParseBuf(payload).parse::<mysql_common::packets::AuthMoreData>(());
        let _ = read_rows_of(payload);
    }

    /// `payload` as a one-column result set's column count, definition and row.
    fn read_rows_of(payload: &[u8]) -> Option<()> {
        lenenc_int(payload)?;
        parse_row(payload, 1).ok()?;
        parse_row(payload, 3).ok().map(drop)
    }

    /// Valid packets of each kind, to change bytes of.
    fn valid_packets() -> Vec<Vec<u8>> {
        let mut column = Vec::new();
        for part in ["def", "shop", "t", "t", "name", "name"] {
            column.extend([&[part.len() as u8][..], part.as_bytes()].concat());
        }
        column.extend_from_slice(&[0x0C, 45, 0, 0, 4, 0, 0, 0xFD, 0, 0, 0, 0, 0]);
        vec![
            vec![0x00, 1, 0, 2, 0, 0, 0],
            vec![0xFE, 0, 0, 2, 0, 0, 0],
            vec![0xFE, 0, 0, 2, 0],
            [&[0xFF, 0x7A, 0x04][..], b"#42S02Table 'x' doesn't exist"].concat(),
            [&[0xFE][..], b"caching_sha2_password\0", b"0123456789abcdefghij\0"].concat(),
            [&[0x01][..], b"-----BEGIN PUBLIC KEY-----"].concat(),
            column,
        ]
    }

    fn caps_dbee_uses() -> impl Strategy<Value = CapabilityFlags> {
        let base = CapabilityFlags::CLIENT_PROTOCOL_41
            | CapabilityFlags::CLIENT_SECURE_CONNECTION
            | CapabilityFlags::CLIENT_PLUGIN_AUTH
            | CapabilityFlags::CLIENT_FOUND_ROWS
            | CapabilityFlags::CLIENT_TRANSACTIONS;
        prop_oneof![
            Just(base),
            Just(base | CapabilityFlags::CLIENT_DEPRECATE_EOF),
            Just(CapabilityFlags::empty()),
            any::<u32>().prop_map(CapabilityFlags::from_bits_truncate)
        ]
    }

    proptest! {
        #[test]
        fn hostile_packets_never_panic(
            caps in caps_dbee_uses(),
            head in prop_oneof![Just(0x00u8), Just(0x01), Just(0x03), Just(0xFB), Just(0xFC), Just(0xFD), Just(0xFE), Just(0xFF), any::<u8>()],
            random in prop::collection::vec(any::<u8>(), 0..80),
            which in 0usize..7,
            edits in prop::collection::vec((0usize..64, any::<u8>()), 0..5),
            cut in 0usize..64,
        ) {
            parse_everything(&random, caps);
            parse_everything(&[&[head][..], &random].concat(), caps);
            let mut packet = valid_packets().swap_remove(which);
            for (at, byte) in edits {
                if let Some(b) = packet.get_mut(at) {
                    *b = byte;
                }
            }
            parse_everything(&packet, caps);
            packet.truncate(cut);
            parse_everything(&packet, caps);
        }
    }

    #[test]
    fn the_valid_packets_parse() {
        let caps = CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF | CapabilityFlags::CLIENT_TRANSACTIONS;
        let v = valid_packets();
        assert_eq!(parse_ok::<CommonOkPacket>(&v[0], caps).unwrap().affected_rows(), 1);
        assert!(parse_ok::<ResultSetTerminator>(&v[1], caps).unwrap().status_flags().contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT));
        assert!(parse_ok::<OldEofPacket>(&v[2], caps).unwrap().status_flags().contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT));
        assert_eq!(errors::from_err_packet(&v[3], caps, Flavor::MySql).to_string(), "Table 'x' doesn't exist (MySQL error 1146)");
        let switch = ParseBuf(&v[4]).parse::<mysql_common::packets::AuthSwitchRequest>(()).unwrap();
        assert_eq!((switch.auth_plugin().as_bytes(), &switch.plugin_data()[..20]), (&b"caching_sha2_password"[..], &b"0123456789abcdefghij"[..]));
        assert_eq!(ParseBuf(&v[5]).parse::<mysql_common::packets::AuthMoreData>(()).unwrap().data(), b"-----BEGIN PUBLIC KEY-----");
        assert_eq!(ParseBuf(&v[6]).parse::<Column>(()).unwrap().name_str(), "name");
        // The parsers check the header byte.
        assert!(parse_ok::<CommonOkPacket>(&v[1], caps).is_err() && parse_ok::<ResultSetTerminator>(&v[0], caps).is_err());
    }

    #[test]
    fn rows_hold_exactly_their_values() {
        let row = [&[1, b'a', 0xFB, 0][..], &[0xFC, 3, 0], b"xyz"].concat();
        assert_eq!(parse_row(&row, 4).unwrap(), [Some(&b"a"[..]), None, Some(&b""[..]), Some(&b"xyz"[..])]);
        for (bad, count) in [(&row[..], 3), (&row[..], 5), (&[5, b'a'][..], 1), (&[0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF][..], 1)] {
            assert_eq!(parse_row(bad, count).unwrap_err().code(), "other", "{bad:?} {count}");
        }
    }
}
