//! The server's answer to a command: OK and ERR packets, and result sets with their rows.

use dbee_core::{DatabaseAfter, ResultSet};
use dbee_driver::{DbError, Limits, ROW_COST, cell_cost, column_cost, kept_text};
use mysql_common::constants::{CapabilityFlags, SessionStateType, StatusFlags};
use mysql_common::io::ParseBuf;
use mysql_common::packets::session_state_change::SessionStateChange;
use mysql_common::packets::{CommonOkPacket, OkPacket, OkPacketDeserializer, OkPacketKind, OldEofPacket, ResultSetTerminator};

use crate::column::parse_column;
use crate::errors;
use crate::flavor::Flavor;
use crate::values::{self, CellText};
use crate::wire::{CHUNK, Packets, lenenc_int, malformed};

/// The most columns dbee reads in one result. A table has at most 4096, but a `SELECT` can return
/// more (70,000 measured on every test server).
const MAX_COLUMNS: u64 = 65_535;

/// The longest database name MySQL and MariaDB have, in bytes: 64 characters of utf8mb4.
const MAX_DATABASE_NAME: usize = 256;

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
            Ok(SessionStateChange::Schema(schema)) => {
                SchemaReport::Said(Some(kept_text(&schema.as_str(), MAX_DATABASE_NAME)).filter(|name| !name.is_empty()))
            }
            _ => return SchemaReport::Unreadable,
        };
    }
    report
}

/// The spellings of the session variable that holds read-only mode, as the servers report it.
const READ_ONLY_VARIABLES: [&str; 2] = ["transaction_read_only", "tx_read_only"];

/// What an OK packet says of read-only mode, on a session that tracks the read-only variable and
/// the transaction's characteristics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ModeReport {
    /// Nothing that turns it off.
    #[default]
    Silent,
    /// The variable with a value that is not on, or characteristics that hold `READ WRITE`.
    Off,
    /// Its session state does not decode: it may have said anything.
    Unreadable,
}

impl ModeReport {
    /// This report, then `later`, of one command: an unreadable one counts most, then one that
    /// turns it off.
    fn then(self, later: ModeReport) -> ModeReport {
        self.max(later)
    }
}

/// What `ok` says of read-only mode. A report that it is on (a `SET` to the same value is
/// reported too) and entries of other kinds say nothing.
pub(crate) fn mode_report(ok: &OkPacket<'_>) -> ModeReport {
    let Ok(entries) = ok.session_state_info() else { return ModeReport::Unreadable };
    let mut report = ModeReport::Silent;
    for entry in entries.iter() {
        let said = match entry.data_type() {
            SessionStateType::SESSION_TRACK_SYSTEM_VARIABLES => match entry.decode() {
                Ok(SessionStateChange::SystemVariables(variables)) => {
                    let off = variables.iter().any(|v| {
                        READ_ONLY_VARIABLES.iter().any(|name| v.name_bytes().eq_ignore_ascii_case(name.as_bytes()))
                            && !(v.value_bytes().eq_ignore_ascii_case(b"ON") || v.value_bytes() == b"1")
                    });
                    if off { ModeReport::Off } else { ModeReport::Silent }
                }
                _ => ModeReport::Unreadable,
            },
            SessionStateType::SESSION_TRACK_TRANSACTION_CHARACTERISTICS => match entry.decode() {
                Ok(SessionStateChange::TransactionCharacteristics(c)) if c.as_str().to_ascii_uppercase().contains("READ WRITE") => ModeReport::Off,
                Ok(SessionStateChange::TransactionCharacteristics(_)) => ModeReport::Silent,
                _ => ModeReport::Unreadable,
            },
            _ => ModeReport::Silent,
        };
        report = report.then(said);
    }
    report
}

/// What an answer's OK packets said of the session, beside its result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Said {
    /// Of read-only mode, over every OK packet of the answer, also one that ended in an error.
    pub(crate) mode: ModeReport,
    /// The last OK packet's `SERVER_STATUS_NO_BACKSLASH_ESCAPES`; `None` when none came.
    pub(crate) no_backslash_escapes: Option<bool>,
}

impl Said {
    fn note(&mut self, mode: ModeReport, status: StatusFlags) {
        self.mode = self.mode.then(mode);
        self.no_backslash_escapes = Some(status.contains(StatusFlags::SERVER_STATUS_NO_BACKSLASH_ESCAPES));
    }
}

/// Read the answer to a command: an OK packet, an ERR packet, or result sets, as many as the
/// server says follow (`SERVER_MORE_RESULTS_EXISTS`). The result holds the last result set's
/// typed columns and rows, values as text ([`CellText`], counted against the caps before it is
/// built), and the affected rows of the last OK packet (matched rows, as dbee logs in with
/// `CLIENT_FOUND_ROWS`), `None` when no OK packet came. An ERR packet ends it with its error.
/// Rows past `limits` are read but not kept; a result whose columns do not fit in them is read
/// to its end and dropped, and the answer ends in [`DbError::TooLarge`] once all of it is read, so
/// the session stays in step. MariaDB's extended metadata is in each column
/// definition when `extended_metadata` was agreed. When the server reports the current
/// database (`tracks_schema`), the result says where the command left it, and untold when a
/// report does not decode. What the OK packets say of the session goes in `said`, also when the
/// answer ends in an error.
pub(crate) async fn read_result(
    p: &mut Packets,
    caps: CapabilityFlags,
    flavor: Flavor,
    extended_metadata: bool,
    tracks_schema: bool,
    limits: Limits,
    said: &mut Said,
) -> Result<ResultSet, DbError> {
    let mut kept: Option<ResultSet> = None;
    let mut too_large = false;
    let mut affected = None;
    let mut moved = SchemaReport::Silent;
    loop {
        let first = p.read().await?;
        let status = match first.first() {
            Some(0x00) => {
                let ok = parse_ok::<CommonOkPacket>(&first, caps)?;
                affected = Some(ok.affected_rows());
                moved = moved.then(schema_change(&ok));
                said.note(mode_report(&ok), ok.status_flags());
                ok.status_flags()
            }
            Some(0xFF) => return Err(errors::from_err_packet(&first, caps, flavor)),
            Some(0xFB) => return Err(DbError::Other("the server asked for a local file, which dbee never sends".into())),
            _ => {
                // The new result set replaces the one kept: no need to hold both.
                drop(kept.take());
                let (set, status, schema, mode) = read_rows(p, &first, caps, flavor, extended_metadata, limits).await?;
                match set {
                    Some(set) => kept = Some(set),
                    None => too_large = true,
                }
                moved = moved.then(schema);
                said.note(mode, status);
                status
            }
        };
        if !status.contains(StatusFlags::SERVER_MORE_RESULTS_EXISTS) {
            break;
        }
    }
    if too_large {
        return Err(DbError::TooLarge { left: limits.max_bytes });
    }
    let mut result = kept.unwrap_or(ResultSet { complete: true, ..ResultSet::default() });
    result.rows_affected = affected;
    result.database_after = match (tracks_schema, moved) {
        (false, _) | (true, SchemaReport::Unreadable) => DatabaseAfter::Untold,
        (true, SchemaReport::Silent) => DatabaseAfter::Unchanged,
        (true, SchemaReport::Said(database)) => DatabaseAfter::Changed(database),
    };
    Ok(result)
}

/// One result set after its column count `first`: the column definitions, then the rows until
/// the terminator, whose status says whether more results follow, and which may report a change of
/// database ([`schema_change`]) and of read-only mode ([`mode_report`]). The set is `None` when
/// its columns did not fit in `limits`: nothing of it is kept, and it is read to its end all the
/// same.
async fn read_rows(
    p: &mut Packets,
    first: &[u8],
    caps: CapabilityFlags,
    flavor: Flavor,
    extended_metadata: bool,
    limits: Limits,
) -> Result<(Option<ResultSet>, StatusFlags, SchemaReport, ModeReport), DbError> {
    let count = match lenenc_int(first) {
        Some((n, [])) if (1..=MAX_COLUMNS).contains(&n) => usize::try_from(n).map_err(|_| malformed("column count"))?,
        _ => return Err(malformed("column count")),
    };
    let mut columns = Vec::new();
    // The columns count against the byte cap, with the rows.
    let mut bytes: usize = 0;
    let mut fits = true;
    for _ in 0..count {
        let def = p.read_up_to(MAX_COLUMN_DEFINITION, "column definition").await?;
        let column = parse_column(&def, extended_metadata)?;
        let column = values::column(column.name, &column.facts, column.type_name.as_deref());
        bytes = bytes.saturating_add(column_cost(&column));
        fits = fits && bytes <= limits.max_bytes;
        if fits {
            columns.push(column);
        } else {
            columns = Vec::new();
        }
    }
    let old_eof = !caps.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF);
    if old_eof {
        let eof = p.read().await?;
        if eof.first() != Some(&0xFE) {
            return Err(malformed("end of the column definitions"));
        }
        parse_ok::<OldEofPacket>(&eof, caps)?;
    }
    let mut set = ResultSet { columns, complete: fits, ..ResultSet::default() };
    loop {
        let packet = p.read().await?;
        match packet.first() {
            Some(0xFF) => return Err(errors::from_err_packet(&packet, caps, flavor)),
            // A row starting 0xFE has a first value of 16 MiB or more, so it is that long too.
            Some(0xFE) if packet.len() < CHUNK => {
                let end = if old_eof { parse_ok::<OldEofPacket>(&packet, caps)? } else { parse_ok::<ResultSetTerminator>(&packet, caps)? };
                return Ok((fits.then_some(set), end.status_flags(), schema_change(&end), mode_report(&end)));
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
        let row_bytes = cells.iter().fold(ROW_COST, |sum, cell| sum.saturating_add(cell_cost(cell.as_ref().map(CellText::len))));
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
            read_result(&mut p, TRACKING, Flavor::MySql, false, tracks, dbee_driver::QUERY_LIMITS, &mut Said::default()).await.unwrap().database_after
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
        read_result(
            &mut p,
            CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF,
            Flavor::MySql,
            false,
            false,
            limits,
            &mut Said::default(),
        )
        .await
    }

    #[tokio::test]
    async fn invalid_text_counts_as_its_replacements_at_the_cap() {
        // 20 bytes of FF are shown as 20 U+FFFD: 60 bytes.
        let value: &[u8] = &[0xFF; 20];
        let whole = read(result_set_of("c", VAR_STRING, &[value]), dbee_driver::QUERY_LIMITS).await.unwrap();
        assert_eq!(whole.rows, [[Some("\u{FFFD}".repeat(20))]]);
        let held = dbee_driver::column_cost(&whole.columns[0]) + dbee_driver::ROW_COST + dbee_driver::cell_cost(Some(60));
        assert_eq!(dbee_driver::result_cost(&whole), held);
        let fits = read(result_set_of("c", VAR_STRING, &[value]), Limits { max_rows: 10, max_bytes: held }).await.unwrap();
        assert_eq!((fits.rows.len(), fits.complete), (1, true));
        let cut = read(result_set_of("c", VAR_STRING, &[value, b"x"]), Limits { max_rows: 10, max_bytes: held - 1 }).await.unwrap();
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
        let read = |input: Vec<u8>, limits: Limits| async move {
            let (mut p, _) = packets(input);
            read_result(&mut p, caps, Flavor::MySql, false, false, limits, &mut Said::default()).await
        };
        // The column counts with the rows: with it, one row fits and the second does not.
        let column = read(result_set("abcdef", &[]), dbee_driver::QUERY_LIMITS).await.unwrap().columns.remove(0);
        let one_row = dbee_driver::ROW_COST + dbee_driver::cell_cost(Some(3));
        let small = Limits { max_rows: 100, max_bytes: dbee_driver::column_cost(&column) + one_row };
        let r = read(result_set("abcdef", &["xyz", "xyz"]), small).await.unwrap();
        let names: Vec<&str> = r.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!((names, r.rows, r.rows_seen, r.complete), (vec!["abcdef"], vec![vec![Some("xyz".to_owned())]], 2, false));
        let tight = Limits { max_bytes: dbee_driver::column_cost(&column) - 1, ..small };
        assert_eq!(read(result_set("abcdef", &[]), tight).await.unwrap_err(), DbError::TooLarge { left: tight.max_bytes });
        let e = read(result_set(&"n".repeat(70_000), &[]), dbee_driver::QUERY_LIMITS).await.unwrap_err();
        assert_eq!(e, DbError::Other("the server sent a column definition larger than 64 KiB".into()));
    }

    /// What `n` NULL cells hold.
    fn null_cells(n: usize) -> usize {
        n * dbee_driver::cell_cost(None)
    }

    /// M3: a NULL cell holds its slot, so the cap counts it: a result of wide NULL rows stops at
    /// the cap instead of holding every row.
    #[tokio::test]
    async fn null_cells_count_against_the_cap() {
        let (columns, rows) = (1000usize, 100usize);
        let mut payloads = vec![vec![0xFC, 0xE8, 0x03]];
        payloads.extend((0..columns).map(|_| column_def_of("c", VAR_STRING)));
        payloads.extend((0..rows).map(|_| vec![0xFB; columns]));
        payloads.push(vec![0xFE, 0, 0, 2, 0, 0, 0]);
        let input = payloads.iter().enumerate().map(|(i, p)| frame(p, i as u8)).collect::<Vec<_>>().concat();
        let limits = Limits { max_rows: 1000, max_bytes: 1 << 20 };
        let r = read(input, limits).await.unwrap();
        assert_eq!((r.rows_seen, r.complete), (100, false), "{} rows kept", r.rows.len());
        assert!(dbee_driver::result_cost(&r) <= limits.max_bytes);
        // Each row holds its 1000 slots: what the columns leave of 1 MiB holds that many rows.
        let columns = r.columns.iter().map(dbee_driver::column_cost).sum::<usize>();
        assert_eq!(r.rows.len(), (limits.max_bytes - columns) / (dbee_driver::ROW_COST + null_cells(1000)));
    }

    /// L4: MariaDB's type name, from its extended metadata, is kept to 64 bytes.
    #[tokio::test]
    async fn a_type_name_is_kept_cut() {
        let long = vec![b'u'; 60 * 1024];
        let meta = [&[0x00, 0xFC][..], &u16::try_from(long.len()).unwrap().to_le_bytes(), &long].concat();
        let mut def = Vec::new();
        for part in [&b"def"[..], b"", b"", b"", b"c", b"c"] {
            def.push(u8::try_from(part.len()).unwrap());
            def.extend_from_slice(part);
        }
        def.extend([&[0xFD][..], &u32::try_from(meta.len()).unwrap().to_le_bytes()[..3], &meta].concat());
        def.extend_from_slice(&[0x0C, 45, 0, 0, 4, 0, 0, VAR_STRING, 0, 0, 0, 0, 0]);
        let input = [frame(&[1], 0), frame(&def, 1), frame(&[0xFE, 0, 0, 2, 0, 0, 0], 2)].concat();
        let (mut p, _) = packets(input);
        let caps = CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF;
        let r = read_result(&mut p, caps, Flavor::MariaDb, true, false, dbee_driver::QUERY_LIMITS, &mut Said::default()).await.unwrap();
        let c = &r.columns[0];
        assert!(c.base_type.len() <= 64 + 3 && c.type_label.len() <= 64 + 3, "{} and {} bytes", c.base_type.len(), c.type_label.len());
        assert!(c.base_type.starts_with("uuu") && c.base_type.ends_with('\u{2026}'), "{}", c.base_type);
    }

    /// Important 3: a column's name is kept to the longest the servers send.
    #[tokio::test]
    async fn a_column_name_is_kept_cut() {
        let long = "n".repeat(1_000);
        let r = read(result_set(&long, &[]), dbee_driver::QUERY_LIMITS).await.unwrap();
        let kept = &r.columns[0].name;
        assert!(kept.len() <= 256 + '\u{2026}'.len_utf8() && kept.starts_with("nnn"), "{} bytes", kept.len());
    }

    /// Minor 7: a result whose columns do not fit is read to the end of the answer, later results
    /// included, before the statement fails, so the next command reads its own answer.
    #[tokio::test]
    async fn columns_that_do_not_fit_are_read_to_the_end_of_the_answer() {
        let more = [0xFE, 0, 0, 0x0A, 0, 0, 0];
        let payloads = [
            vec![1],
            column_def_of("c", VAR_STRING),
            vec![1, b'x'],
            more.to_vec(),
            vec![1],
            column_def_of("d", VAR_STRING),
            vec![0xFE, 0, 0, 2, 0, 0, 0],
            // The next command's answer.
            vec![0x00, 0, 0, 2, 0, 0, 0],
        ];
        let input = payloads.iter().enumerate().map(|(i, p)| frame(p, u8::try_from(i).unwrap())).collect::<Vec<_>>().concat();
        let (mut p, _) = packets(input);
        let caps = CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_DEPRECATE_EOF;
        let spent = Limits { max_rows: 10, max_bytes: 0 };
        let e = read_result(&mut p, caps, Flavor::MySql, false, false, spent, &mut Said::default()).await.unwrap_err();
        assert_eq!(e, DbError::TooLarge { left: 0 });
        let next = read_result(&mut p, caps, Flavor::MySql, false, false, spent, &mut Said::default()).await.unwrap();
        assert!(next.columns.is_empty());
    }

    /// Minor 6: the database a session state names is kept to what a server can name.
    #[tokio::test]
    async fn a_session_tracked_database_name_is_kept_cut() {
        let lenenc = |b: &[u8]| [&[0xFC][..], &u16::try_from(b.len()).unwrap().to_le_bytes(), b].concat();
        let entry = [&[0x01][..], &lenenc(&lenenc("d".repeat(1_000).as_bytes()))].concat();
        let changed = (StatusFlags::SERVER_STATUS_AUTOCOMMIT | StatusFlags::SERVER_SESSION_STATE_CHANGED).bits().to_le_bytes();
        let ok = [&[0x00, 0, 0][..], &changed, &[0, 0, 0], &lenenc(&entry)].concat();
        let (mut p, _) = packets(frame(&ok, 0));
        let r = read_result(&mut p, TRACKING, Flavor::MySql, false, true, dbee_driver::QUERY_LIMITS, &mut Said::default()).await.unwrap();
        let DatabaseAfter::Changed(Some(name)) = r.database_after else { panic!("{:?}", r.database_after) };
        assert!(name.len() <= 256 + '\u{2026}'.len_utf8() && name.starts_with("ddd"), "{} bytes", name.len());
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
