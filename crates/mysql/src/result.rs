//! The server's answer to a command: OK and ERR packets, and result sets with their rows.

use dbee_core::{DatabaseAfter, ResultSet};
use dbee_driver::{DbError, Limits};
use mysql_common::constants::{CapabilityFlags, SessionStateType, StatusFlags};
use mysql_common::io::ParseBuf;
use mysql_common::packets::session_state_change::SessionStateChange;
use mysql_common::packets::{CommonOkPacket, OkPacket, OkPacketDeserializer, OkPacketKind, OldEofPacket, ResultSetTerminator};

use crate::column::parse_column;
use crate::errors;
use crate::flavor::Flavor;
use crate::values::{self, CellText};
use crate::wire::{CHUNK, Packets, lenenc_int, malformed, size_text};

/// More columns than any table can have (MySQL's limit is 4096).
const MAX_COLUMNS: u64 = 65_535;

/// The largest column definition dbee reads: names of up to 64 characters, far under this.
const MAX_COLUMN_DEFINITION: usize = 64 * 1024;

/// An OK packet of kind `K` (`0x00` after a command, `0xFE` at the end of a result set).
pub(crate) fn parse_ok<K: OkPacketKind>(payload: &[u8], caps: CapabilityFlags) -> Result<OkPacket<'_>, DbError> {
    ParseBuf(payload).parse::<OkPacketDeserializer<'_, K>>(caps).map(OkPacketDeserializer::into_inner).map_err(|_| malformed("OK packet"))
}

/// What an OK packet says of the database the statement made current (`SESSION_TRACK_SCHEMA`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SchemaReport {
    /// Nothing.
    Silent,
    /// This database, or none.
    Said(Option<String>),
    /// Its session state, or a schema entry in it, does not decode: it may have said anything.
    Unreadable,
}

impl SchemaReport {
    /// This report, then `later`, of one command: the last thing said counts, and an unreadable
    /// report spoils them all.
    fn then(self, later: SchemaReport) -> SchemaReport {
        match (self, later) {
            (SchemaReport::Unreadable, _) | (_, SchemaReport::Unreadable) => SchemaReport::Unreadable,
            (earlier, SchemaReport::Silent) => earlier,
            (_, said) => said,
        }
    }
}

/// What `ok` says of the database the statement made current. Entries of other kinds are passed
/// over.
pub(crate) fn schema_change(ok: &OkPacket<'_>) -> SchemaReport {
    let Ok(entries) = ok.session_state_info() else { return SchemaReport::Unreadable };
    let mut report = SchemaReport::Silent;
    for entry in entries.iter().filter(|entry| matches!(entry.data_type(), SessionStateType::SESSION_TRACK_SCHEMA)) {
        report = match entry.decode() {
            Ok(SessionStateChange::Schema(schema)) => SchemaReport::Said(Some(schema.as_str().into_owned()).filter(|name| !name.is_empty())),
            _ => return SchemaReport::Unreadable,
        };
    }
    report
}

/// Read the answer to a command: an OK packet, an ERR packet, or result sets, as many as the
/// server says follow (`SERVER_MORE_RESULTS_EXISTS`). The result holds the last result set's
/// typed columns and rows, values as text ([`CellText`], counted against the caps before it is
/// built), and the affected rows of the last OK packet (matched rows, as dbee logs in with
/// `CLIENT_FOUND_ROWS`), `None` when no OK packet came. An ERR packet ends it with its error.
/// Rows past `limits` are read but not kept. MariaDB's extended metadata is in each column
/// definition when `extended_metadata` was agreed. When the server reports the current
/// database (`tracks_schema`), the result says where the command left it, and untold when a
/// report does not decode. With it, the warning count the answer's last packet gives.
pub(crate) async fn read_result(
    p: &mut Packets,
    caps: CapabilityFlags,
    flavor: Flavor,
    extended_metadata: bool,
    tracks_schema: bool,
    limits: Limits,
) -> Result<(ResultSet, u16), DbError> {
    let mut kept: Option<ResultSet> = None;
    let mut affected = None;
    let mut moved = SchemaReport::Silent;
    let mut warnings;
    loop {
        let first = p.read().await?;
        let status = match first.first() {
            Some(0x00) => {
                let ok = parse_ok::<CommonOkPacket>(&first, caps)?;
                affected = Some(ok.affected_rows());
                moved = moved.then(schema_change(&ok));
                warnings = ok.warnings();
                ok.status_flags()
            }
            Some(0xFF) => return Err(errors::from_err_packet(&first, caps, flavor)),
            Some(0xFB) => return Err(DbError::Other("the server asked for a local file, which dbee never sends".into())),
            _ => {
                // The new result set replaces the one kept: no need to hold both.
                drop(kept.take());
                let (set, status, schema, set_warnings) = read_rows(p, &first, caps, flavor, extended_metadata, limits).await?;
                kept = Some(set);
                moved = moved.then(schema);
                warnings = set_warnings;
                status
            }
        };
        if !status.contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS) {
            break;
        }
    }
    let mut result = kept.unwrap_or(ResultSet { complete: true, ..ResultSet::default() });
    result.rows_affected = affected;
    result.database_after = match (tracks_schema, moved) {
        (false, _) | (true, SchemaReport::Unreadable) => DatabaseAfter::Untold,
        (true, SchemaReport::Silent) => DatabaseAfter::Unchanged,
        (true, SchemaReport::Said(database)) => DatabaseAfter::Changed(database),
    };
    Ok((result, warnings))
}

/// One result set after its column count `first`: the column definitions, then the rows until
/// the terminator, whose status says whether more results follow, which may report a change of
/// database ([`schema_change`]), and which gives a warning count.
async fn read_rows(
    p: &mut Packets,
    first: &[u8],
    caps: CapabilityFlags,
    flavor: Flavor,
    extended_metadata: bool,
    limits: Limits,
) -> Result<(ResultSet, StatusFlags, SchemaReport, u16), DbError> {
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
        columns.push(values::column(column.name, &column.facts, column.type_name.as_deref()));
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
                return Ok((set, end.status_flags(), schema_change(&end), end.warnings()));
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
        let cells: Vec<Option<CellText<'_>>> =
            row.into_iter().zip(&set.columns).map(|(value, column)| value.map(|v| CellText::of(column.class, v))).collect();
        let row_bytes = cells.iter().fold(0usize, |sum, cell| sum.saturating_add(cell.as_ref().map_or(0, CellText::len)));
        if bytes.saturating_add(row_bytes) > limits.max_bytes {
            set.complete = false;
            continue;
        }
        bytes = bytes.saturating_add(row_bytes);
        set.rows.push(cells.into_iter().map(|cell| cell.map(CellText::into_string)).collect());
    }
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
pub(crate) mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::wire::tests::{frame, packets};

    /// Every parser of mysql_common's that reads server bytes, on `payload`, with `caps`.
    fn parse_everything(payload: &[u8], caps: CapabilityFlags) {
        let _ = parse_ok::<CommonOkPacket>(payload, caps).map(|ok| schema_change(&ok));
        let _ = parse_ok::<ResultSetTerminator>(payload, caps).map(|ok| schema_change(&ok));
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
            ok_with_state(&schema_entry("many")),
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
            which in 0usize..8,
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
        assert_eq!(schema_change(&parse_ok::<CommonOkPacket>(&v[1], TRACKING).unwrap()), SchemaReport::Said(Some("many".to_owned())));
        assert!(parse_ok::<ResultSetTerminator>(&v[2], caps).unwrap().status_flags().contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT));
        assert!(parse_ok::<OldEofPacket>(&v[3], caps).unwrap().status_flags().contains(StatusFlags::SERVER_STATUS_AUTOCOMMIT));
        assert_eq!(errors::from_err_packet(&v[4], caps, Flavor::MySql).to_string(), "Table 'x' doesn't exist (MySQL error 1146)");
        let switch = ParseBuf(&v[5]).parse::<mysql_common::packets::AuthSwitchRequest>(()).unwrap();
        assert_eq!((switch.auth_plugin().as_bytes(), &switch.plugin_data()[..20]), (&b"caching_sha2_password"[..], &b"0123456789abcdefghij"[..]));
        assert_eq!(ParseBuf(&v[6]).parse::<mysql_common::packets::AuthMoreData>(()).unwrap().data(), b"-----BEGIN PUBLIC KEY-----");
        assert_eq!(parse_column(&v[7], false).unwrap().name, "name");
        // The parsers check the header byte.
        assert!(parse_ok::<CommonOkPacket>(&v[2], caps).is_err() && parse_ok::<ResultSetTerminator>(&v[0], caps).is_err());
    }

    /// The capabilities of a session whose OK packets carry session state.
    pub(crate) const TRACKING: CapabilityFlags = CapabilityFlags::CLIENT_PROTOCOL_41
        .union(CapabilityFlags::CLIENT_DEPRECATE_EOF)
        .union(CapabilityFlags::CLIENT_TRANSACTIONS)
        .union(CapabilityFlags::CLIENT_SESSION_TRACK);

    /// An OK packet (no rows, autocommit, no warnings, no info) whose session state is `state`.
    pub(crate) fn ok_with_state(state: &[u8]) -> Vec<u8> {
        let changed = (StatusFlags::SERVER_STATUS_AUTOCOMMIT | StatusFlags::SERVER_SESSION_STATE_CHANGED).bits().to_le_bytes();
        [&[0x00, 0, 0][..], &changed, &[0, 0, 0], &[state.len() as u8], state].concat()
    }

    /// A session state entry of type `SESSION_TRACK_SCHEMA` naming `name`.
    pub(crate) fn schema_entry(name: &str) -> Vec<u8> {
        let data = [&[name.len() as u8][..], name.as_bytes()].concat();
        [&[0x01, data.len() as u8][..], &data].concat()
    }

    #[test]
    fn an_ok_packet_says_which_database_a_statement_made_current() {
        let change = |state: &[u8]| schema_change(&parse_ok::<CommonOkPacket>(&ok_with_state(state), TRACKING).unwrap());
        let said = |name: &str| SchemaReport::Said(Some(name.to_owned()));
        assert_eq!(change(&schema_entry("many")), said("many"));
        assert_eq!(change(&schema_entry("")), SchemaReport::Said(None), "an empty name: no database");
        // A system variable entry first (type 0x00: `autocommit`, `ON`), then the schema.
        let variable = [&[0x00, 14, 10][..], b"autocommit", &[2], b"ON"].concat();
        assert_eq!(change(&[variable.clone(), schema_entry("odd")].concat()), said("odd"));
        assert_eq!(change(&variable), SchemaReport::Silent, "no schema entry: nothing said of it");
        // The last schema entry counts.
        assert_eq!(change(&[schema_entry("odd"), schema_entry("many")].concat()), said("many"));
        // An entry of another kind that does not decode is passed over.
        assert_eq!(change(&[&[0x00, 2, 9, b'a'][..], &schema_entry("many")].concat()), said("many"));
        // Without session tracking the same flags carry no state: nothing is said.
        let plain = parse_ok::<CommonOkPacket>(&[0x00, 0, 0, 2, 0, 0, 0], TRACKING).unwrap();
        assert_eq!(schema_change(&plain), SchemaReport::Silent);
    }

    /// A session state entry of kind 6, past the 0 to 5 that MySQL's documentation lists.
    const UNKNOWN_KIND: [u8; 3] = [0x06, 1, 0];

    #[test]
    fn a_session_state_that_does_not_decode_is_unreadable() {
        let ok = |state: &[u8]| parse_ok::<CommonOkPacket>(&ok_with_state(state), TRACKING).map(|ok| schema_change(&ok));
        // An entry longer than the block, a schema name longer than its entry, an entry cut off
        // after its type, and an unknown type: the packet parses, and what it says is unreadable.
        for state in [&[0x01, 9, 4][..], &[0x01, 2, 9, b'a'], &[0x01], &[0x7F, 1, 0], &UNKNOWN_KIND] {
            assert_eq!(ok(state), Ok(SchemaReport::Unreadable), "{state:?}");
        }
        // A good schema entry beside one that does not decode, or beside an unknown kind, before
        // it or after it, is no proof of where the statement left the connection.
        let good = schema_entry("many");
        for bad in [&[0x01, 2, 9, b'a'][..], &UNKNOWN_KIND] {
            assert_eq!(ok(&[bad, &good].concat()), Ok(SchemaReport::Unreadable), "{bad:?} first");
            assert_eq!(ok(&[&good, bad].concat()), Ok(SchemaReport::Unreadable), "{bad:?} after");
        }
    }

    #[test]
    fn unreadable_spoils_every_report_of_a_command() {
        let (said, silent, unreadable) = (|| SchemaReport::Said(Some("many".into())), || SchemaReport::Silent, || SchemaReport::Unreadable);
        assert_eq!(said().then(silent()), said());
        assert_eq!(silent().then(said()), said());
        assert_eq!(SchemaReport::Said(None).then(said()), said(), "the last thing said counts");
        assert_eq!(said().then(unreadable()), unreadable());
        assert_eq!(unreadable().then(said()), unreadable());
        assert_eq!(unreadable().then(silent()), unreadable());
    }

    #[tokio::test]
    async fn a_result_says_where_the_statement_left_the_connection() {
        let read = |payload: Vec<u8>, tracks: bool| async move {
            let (mut p, _) = packets(frame(&payload, 0));
            read_result(&mut p, TRACKING, Flavor::MySql, false, tracks, dbee_driver::QUERY_LIMITS).await.unwrap().0.database_after
        };
        assert_eq!(read(ok_with_state(&schema_entry("many")), true).await, DatabaseAfter::Changed(Some("many".into())));
        assert_eq!(read(vec![0x00, 0, 0, 2, 0, 0, 0], true).await, DatabaseAfter::Unchanged);
        assert_eq!(read(ok_with_state(&schema_entry("many")), false).await, DatabaseAfter::Untold, "a session that does not track tells nothing");
        // A schema change beside an entry of a kind above 5: the server's report is unreadable.
        let beside = ok_with_state(&[&schema_entry("many")[..], &UNKNOWN_KIND].concat());
        assert_eq!(read(beside, true).await, DatabaseAfter::Untold);
        assert_eq!(read(ok_with_state(&[&UNKNOWN_KIND[..], &schema_entry("many")].concat()), true).await, DatabaseAfter::Untold);
    }

    /// `VAR_STRING`, the type of a `VARCHAR` result.
    const VAR_STRING: u8 = 0xFD;

    /// Protocol::ColumnDefinition41 of a column `name` of type `column_type`, in utf8mb4.
    fn column_def_of(name: &str, column_type: u8) -> Vec<u8> {
        let lenenc =
            |s: &[u8]| [if s.len() < 251 { vec![s.len() as u8] } else { [&[0xFD][..], &s.len().to_le_bytes()[..3]].concat() }, s.to_vec()].concat();
        let mut p = Vec::new();
        for part in [&b"def"[..], b"", b"", b"", name.as_bytes(), name.as_bytes()] {
            p.extend(lenenc(part));
        }
        p.extend_from_slice(&[0x0C, 45, 0, 0, 4, 0, 0, column_type, 0, 0, 0, 0, 0]);
        p
    }

    /// A one-column result set (column `name`, a row per value), framed from sequence id 0.
    fn result_set(name: &str, values: &[&str]) -> Vec<u8> {
        let values: Vec<&[u8]> = values.iter().map(|v| v.as_bytes()).collect();
        result_set_of(name, VAR_STRING, &values)
    }

    /// [`result_set`] of a column of type `column_type`, with values of any bytes.
    fn result_set_of(name: &str, column_type: u8, values: &[&[u8]]) -> Vec<u8> {
        let mut payloads = vec![vec![1], column_def_of(name, column_type)];
        payloads.extend(values.iter().map(|v| [&[v.len() as u8][..], v].concat()));
        payloads.push(vec![0xFE, 0, 0, 2, 0, 0, 0]);
        payloads.iter().enumerate().map(|(i, p)| frame(p, i as u8)).collect::<Vec<_>>().concat()
    }

    async fn read(input: Vec<u8>, limits: Limits) -> Result<ResultSet, DbError> {
        let (mut p, _) = packets(input);
        read_result(&mut p, CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF, Flavor::MySql, false, false, limits)
            .await
            .map(|(r, _)| r)
    }

    #[tokio::test]
    async fn invalid_text_counts_as_its_replacements_at_the_cap() {
        // E9 61 FF is shown as U+FFFD a U+FFFD: 7 bytes, and 8 with the name.
        let value: &[u8] = &[0xE9, b'a', 0xFF];
        let fits = read(result_set_of("c", VAR_STRING, &[value]), Limits { max_rows: 10, max_bytes: 8 }).await.unwrap();
        assert_eq!((fits.rows, fits.complete), (vec![vec![Some("\u{FFFD}a\u{FFFD}".to_owned())]], true));
        let cut = read(result_set_of("c", VAR_STRING, &[value, b"x"]), Limits { max_rows: 10, max_bytes: 7 }).await.unwrap();
        assert_eq!((cut.rows.len(), cut.rows_seen, cut.complete), (0, 2, false));
    }

    #[tokio::test]
    async fn a_type_dbee_does_not_know_comes_as_text() {
        // 0x15 is no type MySQL's documentation lists: a newer server's, say.
        let r = read(result_set_of("v", 0x15, &[b"\x01new", b"plain"]), dbee_driver::QUERY_LIMITS).await.unwrap();
        let c = &r.columns[0];
        assert_eq!((c.name.as_str(), c.base_type.as_str(), c.class), ("v", "", dbee_core::TypeClass::Other));
        assert_eq!(r.rows, [[Some("\u{1}new".to_owned())], [Some("plain".to_owned())]]);
    }

    #[tokio::test]
    async fn column_definitions_and_names_are_capped() {
        let caps = CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF;
        let small = Limits { max_rows: 100, max_bytes: 10 };
        let read = |input: Vec<u8>, limits: Limits| async move {
            let (mut p, _) = packets(input);
            read_result(&mut p, caps, Flavor::MySql, false, false, limits).await.map(|(r, _)| r)
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
}
