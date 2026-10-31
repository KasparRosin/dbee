//! The wire: MySQL packets on a byte stream, and the server's answer to a command.
//!
//! A packet is a 3-byte little-endian payload length, a sequence id and the payload. A payload of
//! `0xFFFFFF` bytes continues in the next packet (one of exactly that length is followed by an
//! empty packet). The sequence id counts the packets of one exchange from 0, both ways.

use dbee_core::ResultSet;
use dbee_driver::{DbError, Limits};
use mysql_common::constants::{CapabilityFlags, ColumnFlags, ColumnType, StatusFlags};
use mysql_common::io::ParseBuf;
use mysql_common::packets::{CommonOkPacket, OkPacket, OkPacketDeserializer, OkPacketKind, OldEofPacket, ResultSetTerminator};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::errors;
use crate::flavor::Flavor;
use crate::values::{self, Cell, TypeFacts};

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

/// The largest column definition dbee reads: names of up to 64 characters, far under this.
const MAX_COLUMN_DEFINITION: usize = 64 * 1024;

/// The length of a column definition's fixed fields: character set, length, type, flags,
/// decimals and two unused bytes.
const FIXED_FIELDS: u64 = 0x0C;

/// The key of a value in MariaDB's extended metadata that names the format (`json`).
const FORMAT: u8 = 0x01;

/// The largest packet out of turn dbee reads, to see whether it is the server's goodbye.
const MAX_GOODBYE: usize = 64 * 1024;

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
                return Err(DbError::Other(format!("the server sent a {what} larger than {}", size_text(max))));
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

fn size_text(bytes: usize) -> String {
    match bytes {
        b if b >= MIB => format!("{} MiB", b / MIB),
        b if b >= 1024 => format!("{} KiB", b / 1024),
        b => format!("{b} bytes"),
    }
}

fn malformed(what: &str) -> DbError {
    DbError::Other(format!("the server sent a malformed {what}"))
}

/// An OK packet of kind `K` (`0x00` after a command, `0xFE` at the end of a result set).
pub(crate) fn parse_ok<K: OkPacketKind>(payload: &[u8], caps: CapabilityFlags) -> Result<OkPacket<'_>, DbError> {
    ParseBuf(payload).parse::<OkPacketDeserializer<'_, K>>(caps).map(OkPacketDeserializer::into_inner).map_err(|_| malformed("OK packet"))
}

/// Read the answer to a command: an OK packet, an ERR packet, or result sets, as many as the
/// server says follow (`SERVER_MORE_RESULTS_EXISTS`). The result holds the last result set's
/// typed columns and rows, values as text ([`Cell`]), and the affected rows of the last OK packet
/// (matched rows, as dbee logs in with `CLIENT_FOUND_ROWS`), `None` when no OK packet came. An
/// ERR packet ends it with its error. Rows past `limits` are read but not kept.
/// MariaDB's extended metadata is in each column definition when `extended_metadata` was agreed.
pub(crate) async fn read_result(
    p: &mut Packets,
    caps: CapabilityFlags,
    flavor: Flavor,
    extended_metadata: bool,
    limits: Limits,
) -> Result<ResultSet, DbError> {
    let mut kept: Option<ResultSet> = None;
    let mut affected = None;
    loop {
        let first = p.read().await?;
        let status = match first.first() {
            Some(0x00) => {
                let ok = parse_ok::<CommonOkPacket>(&first, caps)?;
                affected = Some(ok.affected_rows());
                ok.status_flags()
            }
            Some(0xFF) => return Err(errors::from_err_packet(&first, caps, flavor)),
            Some(0xFB) => return Err(DbError::Other("the server asked for a local file, which dbee never sends".into())),
            _ => {
                // The new result set replaces the one kept: no need to hold both.
                drop(kept.take());
                let (set, status) = read_rows(p, &first, caps, flavor, extended_metadata, limits).await?;
                kept = Some(set);
                status
            }
        };
        if !status.contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS) {
            break;
        }
    }
    let mut result = kept.unwrap_or(ResultSet { complete: true, ..ResultSet::default() });
    result.rows_affected = affected;
    Ok(result)
}

/// One result set after its column count `first`: the column definitions, then the rows until
/// the terminator, whose status says whether more results follow.
async fn read_rows(
    p: &mut Packets,
    first: &[u8],
    caps: CapabilityFlags,
    flavor: Flavor,
    extended_metadata: bool,
    limits: Limits,
) -> Result<(ResultSet, StatusFlags), DbError> {
    let count = match lenenc_int(first) {
        Some((n, [])) if (1..=MAX_COLUMNS).contains(&n) => usize::try_from(n).map_err(|_| malformed("column count"))?,
        _ => return Err(malformed("column count")),
    };
    let mut columns = Vec::new();
    // The column names count against the byte cap, with the cells.
    let mut bytes: usize = 0;
    for _ in 0..count {
        let def = p.read_up_to(MAX_COLUMN_DEFINITION, "column definition").await?;
        let column = parse_column(&def, extended_metadata)?;
        bytes = bytes.saturating_add(column.name.len());
        if bytes > limits.max_bytes {
            return Err(DbError::Other(format!("the server sent column names larger than {}", size_text(limits.max_bytes))));
        }
        columns.push(values::column(column.name, &column.facts));
    }
    let old_eof = !caps.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF);
    if old_eof {
        let eof = p.read().await?;
        if eof.first() != Some(&0xFE) {
            return Err(malformed("end of the column definitions"));
        }
        parse_ok::<OldEofPacket>(&eof, caps)?;
    }
    let mut set = ResultSet { columns, complete: true, ..ResultSet::default() };
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
        if set.rows.len() >= limits.max_rows {
            set.complete = false;
            continue;
        }
        let row = parse_row(&packet, count)?;
        let cells: Vec<Option<Cell<'_>>> =
            row.into_iter().zip(&set.columns).map(|(value, column)| value.map(|v| Cell::of(column.class, v))).collect();
        let row_bytes = cells.iter().fold(0usize, |sum, cell| sum.saturating_add(cell.as_ref().map_or(0, Cell::len)));
        if bytes.saturating_add(row_bytes) > limits.max_bytes {
            set.complete = false;
            continue;
        }
        bytes = bytes.saturating_add(row_bytes);
        set.rows.push(cells.into_iter().map(|cell| cell.map(Cell::into_string)).collect());
    }
}

/// A result column as its definition (Protocol::ColumnDefinition41) gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnDefinition {
    /// The name the result gives the column (its alias), as UTF-8 with invalid bytes replaced.
    pub(crate) name: String,
    pub(crate) facts: TypeFacts,
    /// The digits after the point; nothing reads it yet but the tests that check the parser.
    #[allow(dead_code)]
    pub(crate) decimals: u8,
}

/// A column definition as MySQL's and MariaDB's documentation lay it out: six length-encoded
/// strings (catalog, always `def`; schema; table; original table; name; original name), then
/// MariaDB's extended metadata when `extended_metadata` was agreed (a length-encoded string of
/// pairs of a 1-byte key and a length-encoded value, of which dbee reads the format `json`),
/// then the 12 bytes of fixed fields. Every length is checked against the packet, and nothing
/// may follow.
pub(crate) fn parse_column(def: &[u8], extended_metadata: bool) -> Result<ColumnDefinition, DbError> {
    let bad = || malformed("column definition");
    let mut rest = def;
    let mut strings: [&[u8]; 6] = [&[]; 6];
    for string in &mut strings {
        (*string, rest) = lenenc_str(rest).ok_or_else(bad)?;
    }
    let [catalog, _schema, _table, _original_table, name, _original_name] = strings;
    if catalog != b"def" {
        return Err(bad());
    }
    let mut json = false;
    if extended_metadata {
        let (mut pairs, after) = lenenc_str(rest).ok_or_else(bad)?;
        rest = after;
        while let Some((&key, tail)) = pairs.split_first() {
            let (value, tail) = lenenc_str(tail).ok_or_else(bad)?;
            json |= key == FORMAT && value == b"json";
            pairs = tail;
        }
    }
    let Some((FIXED_FIELDS, fixed)) = lenenc_int(rest) else { return Err(bad()) };
    let [c0, c1, l0, l1, l2, l3, column_type, f0, f1, decimals, _, _] = <[u8; 12]>::try_from(fixed).map_err(|_| bad())?;
    let facts = TypeFacts {
        column_type: ColumnType::try_from(column_type).map_err(|_| bad())?,
        flags: ColumnFlags::from_bits_truncate(u16::from_le_bytes([f0, f1])),
        charset: u16::from_le_bytes([c0, c1]),
        length: u32::from_le_bytes([l0, l1, l2, l3]),
        json,
    };
    Ok(ColumnDefinition { name: String::from_utf8_lossy(name).into_owned(), facts, decimals })
}

/// The length-encoded string at the head of `buf`, and the bytes after it.
fn lenenc_str(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    let (len, rest) = lenenc_int(buf)?;
    rest.split_at_checked(usize::try_from(len).ok()?)
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

    /// Every parser of mysql_common's that reads server bytes, on `payload`, with `caps`.
    fn parse_everything(payload: &[u8], caps: CapabilityFlags) {
        let _ = parse_ok::<CommonOkPacket>(payload, caps);
        let _ = parse_ok::<ResultSetTerminator>(payload, caps);
        let _ = parse_ok::<OldEofPacket>(payload, caps);
        let _ = errors::from_err_packet(payload, caps, Flavor::MySql);
        let _ = parse_column(payload, false);
        let _ = parse_column(payload, true);
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
        assert_eq!(parse_column(&v[6], false).unwrap().name, "name");
        // The parsers check the header byte.
        assert!(parse_ok::<CommonOkPacket>(&v[1], caps).is_err() && parse_ok::<ResultSetTerminator>(&v[0], caps).is_err());
    }

    /// Protocol::ColumnDefinition41 of a column `name`.
    fn column_def(name: &str) -> Vec<u8> {
        let lenenc =
            |s: &[u8]| [if s.len() < 251 { vec![s.len() as u8] } else { [&[0xFD][..], &s.len().to_le_bytes()[..3]].concat() }, s.to_vec()].concat();
        let mut p = Vec::new();
        for part in [&b"def"[..], b"", b"", b"", name.as_bytes(), name.as_bytes()] {
            p.extend(lenenc(part));
        }
        p.extend_from_slice(&[0x0C, 45, 0, 0, 4, 0, 0, 0xFD, 0, 0, 0, 0, 0]);
        p
    }

    /// A one-column result set (column `name`, a row per value), framed from sequence id 0.
    fn result_set(name: &str, values: &[&str]) -> Vec<u8> {
        let mut payloads = vec![vec![1], column_def(name)];
        payloads.extend(values.iter().map(|v| [&[v.len() as u8][..], v.as_bytes()].concat()));
        payloads.push(vec![0xFE, 0, 0, 2, 0, 0, 0]);
        payloads.iter().enumerate().map(|(i, p)| frame(p, i as u8)).collect::<Vec<_>>().concat()
    }

    #[tokio::test]
    async fn column_definitions_and_names_are_capped() {
        let caps = CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF;
        let small = Limits { max_rows: 100, max_bytes: 10 };
        let read = |input: Vec<u8>, limits: Limits| async move {
            let (mut p, _) = packets(input);
            read_result(&mut p, caps, Flavor::MySql, false, limits).await
        };
        // The name's 6 bytes count with the cells': the second row does not fit in 10.
        let r = read(result_set("abcdef", &["xyz", "xyz"]), small).await.unwrap();
        let names: Vec<&str> = r.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!((names, r.rows, r.rows_seen, r.complete), (vec!["abcdef"], vec![vec![Some("xyz".to_owned())]], 2, false));
        let e = read(result_set("abcdefghijk", &[]), small).await.unwrap_err();
        assert!(matches!(&e, DbError::Other(m) if m.contains("column names larger than")), "{e:?}");
        let e = read(result_set(&"n".repeat(70_000), &[]), dbee_driver::QUERY_LIMITS).await.unwrap_err();
        assert_eq!(e, DbError::Other("the server sent a column definition larger than 64 KiB".into()));
    }

    #[test]
    fn rows_hold_exactly_their_values() {
        let row = [&[1, b'a', 0xFB, 0][..], &[0xFC, 3, 0], b"xyz"].concat();
        assert_eq!(parse_row(&row, 4).unwrap(), [Some(&b"a"[..]), None, Some(&b""[..]), Some(&b"xyz"[..])]);
        for (bad, count) in [(&row[..], 3), (&row[..], 5), (&[5, b'a'][..], 1), (&[0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF][..], 1)] {
            assert_eq!(parse_row(bad, count).unwrap_err().code(), "other", "{bad:?} {count}");
        }
    }

    fn lenenc_of(n: usize) -> Vec<u8> {
        match n {
            0..=250 => vec![n as u8],
            251..=0xFFFF => [&[0xFC][..], &n.to_le_bytes()[..2]].concat(),
            0x1_0000..=0xFF_FFFF => [&[0xFD][..], &n.to_le_bytes()[..3]].concat(),
            _ => [&[0xFE][..], &n.to_le_bytes()].concat(),
        }
    }

    fn lenenc_string(s: &[u8]) -> Vec<u8> {
        [lenenc_of(s.len()), s.to_vec()].concat()
    }

    /// A column definition of `strings` (catalog … original name), MariaDB's extended metadata
    /// `pairs` when given, and the 12 bytes `fixed`.
    fn definition(strings: &[Vec<u8>], pairs: Option<&[(u8, Vec<u8>)]>, fixed: [u8; 12]) -> Vec<u8> {
        let mut def: Vec<u8> = strings.iter().flat_map(|s| lenenc_string(s)).collect();
        if let Some(pairs) = pairs {
            let meta: Vec<u8> = pairs.iter().flat_map(|(key, value)| [vec![*key], lenenc_string(value)].concat()).collect();
            def.extend(lenenc_string(&meta));
        }
        def.push(0x0C);
        def.extend_from_slice(&fixed);
        def
    }

    /// Character set 45, length 4294967295, `BLOB`, flags `BLOB|BINARY`, no decimals.
    const LONGTEXT: [u8; 12] = [45, 0, 0xFF, 0xFF, 0xFF, 0xFF, 252, 0x90, 0, 0, 0, 0];

    fn names(name: &str) -> Vec<Vec<u8>> {
        ["def", "shop", "all_types", "all_types", name, name].iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    /// Our parse against mysql_common's of a definition without the extended metadata.
    fn same_as_mysql_common(def: &[u8], ours: &ColumnDefinition) -> Result<(), TestCaseError> {
        let theirs = ParseBuf(def).parse::<mysql_common::packets::Column>(()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let f = &ours.facts;
        prop_assert_eq!(
            (ours.name.as_str(), f.column_type, f.flags, f.charset, f.length, ours.decimals),
            (&*theirs.name_str(), theirs.column_type(), theirs.flags(), theirs.character_set(), theirs.column_length(), theirs.decimals())
        );
        Ok(())
    }

    #[test]
    fn column_definitions_as_documented() {
        let def = definition(&names("doc"), None, LONGTEXT);
        let c = parse_column(&def, false).unwrap();
        assert_eq!(
            (c.name.as_str(), c.facts.column_type, c.facts.charset, c.facts.length, c.decimals),
            ("doc", ColumnType::MYSQL_TYPE_BLOB, 45, u32::MAX, 0)
        );
        assert_eq!(c.facts.flags, ColumnFlags::BLOB_FLAG | ColumnFlags::BINARY_FLAG);
        assert!(!c.facts.json);
        same_as_mysql_common(&def, &c).unwrap();
        // MariaDB's extended metadata: only the format json makes JSON.
        for (pairs, json) in [
            (vec![(1, b"json".to_vec())], true),
            (vec![(0, b"point".to_vec()), (1, b"json".to_vec())], true),
            (vec![], false),
            (vec![(0, b"json".to_vec())], false),
            (vec![(1, b"jsonb".to_vec()), (7, vec![1; 300])], false),
        ] {
            let ext = parse_column(&definition(&names("doc"), Some(&pairs), LONGTEXT), true).unwrap();
            assert_eq!(ext, ColumnDefinition { facts: TypeFacts { json, ..c.facts }, ..c.clone() }, "{pairs:?}");
        }
        // The definition must be what was agreed: with the extended metadata or without it.
        let ext = definition(&names("doc"), Some(&[(1, b"json".to_vec())]), LONGTEXT);
        assert_eq!(parse_column(&ext, false).unwrap_err().code(), "other");
        assert_eq!(parse_column(&def, true).unwrap_err().code(), "other");
        let bad = |def: Vec<u8>| parse_column(&def, false).unwrap_err().code();
        assert_eq!(bad([def.clone(), vec![0]].concat()), "other", "a byte after the fixed fields");
        assert_eq!(bad(def[..def.len() - 1].to_vec()), "other", "one byte short");
        let thirteen = [&def[..def.len() - 13], &[0x0D], &LONGTEXT, &[0]].concat();
        assert_eq!(bad(thirteen), "other", "fixed fields that are not 12 bytes");
        let unknown_type = [&def[..def.len() - 6], &[21], &def[def.len() - 5..]].concat();
        assert_eq!(bad(unknown_type), "other");
        assert_eq!(bad([&b"\x03abc"[..], &def[4..]].concat()), "other", "the catalog is always def");
        // A string longer than the packet, in each of its lenenc widths.
        for long in [
            vec![0xFA],
            vec![0xFC, 0xFF, 0xFF],
            vec![0xFD, 0xFF, 0xFF, 0xFF],
            vec![0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
            vec![0xFB],
            vec![0xFF],
        ] {
            let def = [&long[..], &def[4..]].concat();
            assert_eq!(bad(def), "other", "{long:?}");
        }
        let overlong_value = [&def[..def.len() - 13], &[3, 1, 0xFC, 0xFF], &def[def.len() - 13..]].concat();
        assert_eq!(parse_column(&overlong_value, true).unwrap_err().code(), "other");
    }

    /// The fields of a column definition, as a server could send them: the catalog `def`
    /// (mostly), the other strings any bytes, short or long.
    fn fields() -> impl Strategy<Value = (Vec<Vec<u8>>, [u8; 12])> {
        let string = prop_oneof![prop::collection::vec(any::<u8>(), 0..20), prop::collection::vec(any::<u8>(), 240..300)];
        let catalog = prop_oneof![9 => Just(b"def".to_vec()), 1 => prop::collection::vec(any::<u8>(), 0..5)];
        (catalog, prop::collection::vec(string, 5), any::<[u8; 12]>()).prop_map(|(c, rest, fixed)| ([vec![c], rest].concat(), fixed))
    }

    fn pairs() -> impl Strategy<Value = Vec<(u8, Vec<u8>)>> {
        let value = prop_oneof![Just(b"json".to_vec()), Just(b"point".to_vec()), prop::collection::vec(any::<u8>(), 0..300)];
        prop::collection::vec((prop_oneof![Just(0u8), Just(1u8), any::<u8>()], value), 0..4)
    }

    proptest! {
        #[test]
        fn column_definitions_read_as_mysql_common_reads_them((strings, fixed) in fields(), pairs in pairs()) {
            let def = definition(&strings, None, fixed);
            let ours = parse_column(&def, false);
            prop_assert_eq!(ours.is_ok(), strings[0] == b"def" && ColumnType::try_from(fixed[6]).is_ok());
            prop_assert_eq!(ours.is_ok(), ParseBuf(&def).parse::<mysql_common::packets::Column>(()).is_ok());
            if let Ok(ours) = ours {
                same_as_mysql_common(&def, &ours)?;
                let ext = parse_column(&definition(&strings, Some(&pairs), fixed), true).unwrap();
                let json = pairs.iter().any(|(key, value)| *key == 1 && value == b"json");
                prop_assert_eq!(ext, ColumnDefinition { facts: TypeFacts { json, ..ours.facts }, ..ours });
            }
        }

        #[test]
        fn hostile_column_definitions_never_panic(
            random in prop::collection::vec(any::<u8>(), 0..120),
            (strings, fixed) in fields(),
            pairs in pairs(),
            extended in any::<bool>(),
            edits in prop::collection::vec((0usize..400, any::<u8>()), 0..5),
            cut in 0usize..400,
            long in prop_oneof![Just(vec![0xFCu8, 0xFF, 0xFF]), Just(vec![0xFD, 0xFF, 0xFF, 0xFF]), Just(vec![0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F])],
            at in 0usize..7,
        ) {
            let _ = parse_column(&random, extended);
            let mut def = definition(&strings, extended.then_some(&pairs[..]), fixed);
            for (i, byte) in edits {
                if let Some(b) = def.get_mut(i) {
                    *b = byte;
                }
            }
            let _ = parse_column(&def, extended);
            def.truncate(cut);
            let _ = parse_column(&def, extended);
            // One string, or the extended metadata, announced longer than the packet.
            let mut parts: Vec<Vec<u8>> = strings.iter().map(|s| lenenc_string(s)).collect();
            if extended {
                parts.push(lenenc_string(&[1, 4, b'j', b's', b'o', b'n']));
            }
            if let Some(part) = parts.get_mut(at) {
                *part = [&long[..], &part[1..]].concat();
                let def = [parts.concat(), vec![0x0C], fixed.to_vec()].concat();
                prop_assert!(parse_column(&def, extended).is_err());
            }
        }
    }
}
