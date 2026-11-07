//! A result column's definition (Protocol::ColumnDefinition41), read by dbee itself on MySQL and
//! MariaDB alike, with MariaDB's extended metadata when it was agreed.

use dbee_driver::DbError;
use mysql_common::constants::{ColumnFlags, ColumnType};

use crate::values::{TypeFacts, type_of_name};
use crate::wire::{lenenc_int, lenenc_str, malformed};

/// The length of a column definition's fixed fields: character set, length, type, flags,
/// decimals and two unused bytes.
const FIXED_FIELDS: u64 = 0x0C;

/// The keys of the values in MariaDB's extended metadata that name the type (`uuid`, `point`)
/// and the format (`json`).
const TYPE_NAME: u8 = 0x00;
const FORMAT: u8 = 0x01;

/// A result column as its definition (Protocol::ColumnDefinition41) gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnDefinition {
    /// The name the result gives the column (its alias), as UTF-8 with invalid bytes replaced.
    pub(crate) name: String,
    pub(crate) facts: TypeFacts,
    /// MariaDB's name for the column's type, when its extended metadata gives one.
    pub(crate) type_name: Option<String>,
}

/// A column definition as MySQL's and MariaDB's documentation lay it out: six length-encoded
/// strings (catalog, always `def`; schema; table; original table; name; original name), then
/// MariaDB's extended metadata when `extended_metadata` was agreed (a length-encoded string of
/// pairs of a 1-byte key and a length-encoded value, of which dbee reads the type's name and
/// the format `json`), then the 12 bytes of fixed fields. Every length is checked against the
/// packet, and nothing may follow. A type byte dbee does not know (a newer server's type) is
/// `MYSQL_TYPE_UNKNOWN`; a type name, the last one given, decides the type as the catalog's
/// `DATA_TYPE` does, so MariaDB's `uuid` is a type dbee does not know either.
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
    let mut type_name = None;
    if extended_metadata {
        let (mut pairs, after) = lenenc_str(rest).ok_or_else(bad)?;
        rest = after;
        while let Some((&key, tail)) = pairs.split_first() {
            let (value, tail) = lenenc_str(tail).ok_or_else(bad)?;
            match key {
                TYPE_NAME => type_name = Some(value),
                FORMAT => json |= value == b"json",
                _ => {}
            }
            pairs = tail;
        }
    }
    let Some((FIXED_FIELDS, fixed)) = lenenc_int(rest) else { return Err(bad()) };
    let [c0, c1, l0, l1, l2, l3, column_type, f0, f1, _decimals, _, _] = <[u8; 12]>::try_from(fixed).map_err(|_| bad())?;
    let column_type = match type_name {
        Some(name) => type_of_name(name),
        None => ColumnType::try_from(column_type).unwrap_or(ColumnType::MYSQL_TYPE_UNKNOWN),
    };
    let facts = TypeFacts {
        column_type,
        flags: ColumnFlags::from_bits_truncate(u16::from_le_bytes([f0, f1])),
        charset: u16::from_le_bytes([c0, c1]),
        length: u32::from_le_bytes([l0, l1, l2, l3]),
        json,
    };
    let type_name = type_name.map(|n| String::from_utf8_lossy(n).into_owned());
    Ok(ColumnDefinition { name: String::from_utf8_lossy(name).into_owned(), facts, type_name })
}

#[cfg(test)]
mod tests {
    use mysql_common::constants::{CapabilityFlags, Command, MariadbCapabilities};
    use mysql_common::io::ParseBuf;
    use proptest::prelude::*;

    use super::*;
    use crate::auth::{Login, ServerKey};
    use crate::connect::{Route, Session, TlsUse};

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
            (ours.name.as_str(), f.column_type, f.flags, f.charset, f.length),
            (&*theirs.name_str(), theirs.column_type(), theirs.flags(), theirs.character_set(), theirs.column_length())
        );
        Ok(())
    }

    #[test]
    fn column_definitions_as_documented() {
        let def = definition(&names("doc"), None, LONGTEXT);
        let c = parse_column(&def, false).unwrap();
        assert_eq!((c.name.as_str(), c.facts.column_type, c.facts.charset, c.facts.length), ("doc", ColumnType::MYSQL_TYPE_BLOB, 45, u32::MAX));
        assert_eq!(c.facts.flags, ColumnFlags::BLOB_FLAG | ColumnFlags::BINARY_FLAG);
        assert!(!c.facts.json);
        same_as_mysql_common(&def, &c).unwrap();
        // MariaDB's extended metadata: the format json makes JSON; a type name, the last one
        // given, decides the type as DATA_TYPE does, in any case.
        use ColumnType::*;
        for (pairs, json, column_type, type_name) in [
            (vec![(1, b"json".to_vec())], true, MYSQL_TYPE_BLOB, None),
            (vec![(0, b"point".to_vec()), (1, b"json".to_vec())], true, MYSQL_TYPE_GEOMETRY, Some("point")),
            (vec![], false, MYSQL_TYPE_BLOB, None),
            (vec![(0, b"json".to_vec())], false, MYSQL_TYPE_JSON, Some("json")),
            (vec![(1, b"jsonb".to_vec()), (7, vec![1; 300])], false, MYSQL_TYPE_BLOB, None),
            (vec![(0, b"uuid".to_vec())], false, MYSQL_TYPE_UNKNOWN, Some("uuid")),
            (vec![(0, b"uuid".to_vec()), (0, b"POINT".to_vec())], false, MYSQL_TYPE_GEOMETRY, Some("POINT")),
            (vec![(0, vec![0xFF, b'x'])], false, MYSQL_TYPE_UNKNOWN, Some("\u{FFFD}x")),
        ] {
            let ext = parse_column(&definition(&names("doc"), Some(&pairs), LONGTEXT), true).unwrap();
            let facts = TypeFacts { json, column_type, ..c.facts };
            assert_eq!(ext, ColumnDefinition { facts, type_name: type_name.map(str::to_owned), ..c.clone() }, "{pairs:?}");
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
        // A type a newer server added is one dbee does not know, not a broken definition.
        let unknown_type = [&def[..def.len() - 6], &[21], &def[def.len() - 5..]].concat();
        assert_eq!(parse_column(&unknown_type, false).unwrap().facts.column_type, ColumnType::MYSQL_TYPE_UNKNOWN);
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
        for key in [0, 1] {
            let overlong_value = [&def[..def.len() - 13], &[3, key, 0xFC, 0xFF], &def[def.len() - 13..]].concat();
            assert_eq!(parse_column(&overlong_value, true).unwrap_err().code(), "other", "{key}");
        }
    }

    /// The fields of a column definition, as a server could send them: the catalog `def`
    /// (mostly), the other strings any bytes, short or long.
    fn fields() -> impl Strategy<Value = (Vec<Vec<u8>>, [u8; 12])> {
        let string = prop_oneof![prop::collection::vec(any::<u8>(), 0..20), prop::collection::vec(any::<u8>(), 240..300)];
        let catalog = prop_oneof![9 => Just(b"def".to_vec()), 1 => prop::collection::vec(any::<u8>(), 0..5)];
        (catalog, prop::collection::vec(string, 5), any::<[u8; 12]>()).prop_map(|(c, rest, fixed)| ([vec![c], rest].concat(), fixed))
    }

    fn pairs() -> impl Strategy<Value = Vec<(u8, Vec<u8>)>> {
        let value = prop_oneof![Just(b"json".to_vec()), Just(b"point".to_vec()), Just(b"uuid".to_vec()), prop::collection::vec(any::<u8>(), 0..300)];
        prop::collection::vec((prop_oneof![Just(0u8), Just(1u8), any::<u8>()], value), 0..4)
    }

    proptest! {
        #[test]
        fn generated_definitions_read_as_mysql_common_reads_them((strings, fixed) in fields(), pairs in pairs()) {
            let def = definition(&strings, None, fixed);
            let ours = parse_column(&def, false);
            let known = ColumnType::try_from(fixed[6]).is_ok();
            prop_assert_eq!(ours.is_ok(), strings[0] == b"def");
            prop_assert_eq!(ours.is_ok() && known, ParseBuf(&def).parse::<mysql_common::packets::Column>(()).is_ok());
            if let Ok(ours) = ours {
                if known {
                    same_as_mysql_common(&def, &ours)?;
                } else {
                    prop_assert_eq!(ours.facts.column_type, ColumnType::MYSQL_TYPE_UNKNOWN);
                }
                let ext = parse_column(&definition(&strings, Some(&pairs), fixed), true).unwrap();
                let json = pairs.iter().any(|(key, value)| *key == 1 && value == b"json");
                let type_name = pairs.iter().rev().find(|(key, _)| *key == 0).map(|(_, value)| value);
                let column_type = type_name.map_or(ours.facts.column_type, |name| type_of_name(name));
                let type_name = type_name.map(|name| String::from_utf8_lossy(name).into_owned());
                prop_assert_eq!(ext, ColumnDefinition { facts: TypeFacts { json, column_type, ..ours.facts }, type_name, ..ours });
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

    /// The column definitions of `sql`'s result set, as `session` receives them; the rest of
    /// the answer is read and dropped.
    async fn definitions(session: &mut Session, sql: &str) -> Vec<Vec<u8>> {
        session.packets.reset();
        session.packets.write(&[&[Command::COM_QUERY as u8][..], sql.as_bytes()].concat()).await.unwrap();
        let count = session.packets.read().await.unwrap();
        assert!(count.len() == 1 && count[0] < 251, "{sql}: {count:?}");
        let mut defs = Vec::new();
        for _ in 0..count[0] {
            defs.push(session.packets.read().await.unwrap());
        }
        if !session.caps.contains(CapabilityFlags::CLIENT_DEPRECATE_EOF) {
            session.packets.read().await.unwrap();
        }
        while !matches!(session.packets.read().await.unwrap().as_slice(), [0xFE, ..]) {}
        defs
    }

    /// The fields dbee reads of a column definition, as mysql_common reads them.
    fn mysql_common_reads(def: &[u8]) -> (String, ColumnType, ColumnFlags, u16, u32) {
        let c = ParseBuf(def).parse::<mysql_common::packets::Column>(()).unwrap();
        (c.name_str().into_owned(), c.column_type(), c.flags(), c.character_set(), c.column_length())
    }

    fn dbee_reads(def: &[u8], extended: bool) -> ((String, ColumnType, ColumnFlags, u16, u32), bool) {
        let c = parse_column(def, extended).unwrap();
        ((c.name, c.facts.column_type, c.facts.flags, c.facts.charset, c.facts.length), c.facts.json)
    }

    #[tokio::test]
    async fn the_servers_definitions_read_as_mysql_common_reads_them() {
        let db = dbee_testkit::require_testdb!();
        let queries = [
            "SELECT * FROM shop.all_types",
            "SELECT CAST(1.50 AS DECIMAL(10,2)) n, NOW(3) t, NULL z, '' e, CAST(X'00FF' AS BINARY) b, 1.5e0 f, JSON_OBJECT('a', 1) j, \
             JSON_EXTRACT('[1]', '$[0]') x",
            "SELECT * FROM odd.`Odd \"Name\" é`",
        ];
        for s in db.servers_of(&[dbee_testkit::Kind::MySql, dbee_testkit::Kind::MariaDb]) {
            let route = Route { host: "127.0.0.1".into(), port: s.port, tunnel: None, reached: None };
            let who = Login { user: "root", password: db.password.as_bytes(), database: None };
            let mut plain = Session::open_asking(&route, &who, &TlsUse::Off, ServerKey::Allowed, MariadbCapabilities::empty(), false).await.unwrap();
            let mut extended = Session::open(&route, &who, &TlsUse::Off, ServerKey::Allowed, false).await.unwrap();
            let mariadb = s.kind == dbee_testkit::Kind::MariaDb;
            assert_eq!((plain.extended_metadata, extended.extended_metadata), (false, mariadb), "{}", s.name);
            for sql in queries {
                let (before, now) = (definitions(&mut plain, sql).await, definitions(&mut extended, sql).await);
                assert_eq!(before.len(), now.len(), "{}: {sql}", s.name);
                let mut json = Vec::new();
                for (b, n) in before.iter().zip(&now) {
                    let theirs = mysql_common_reads(b);
                    assert_eq!(dbee_reads(b, false), (theirs.clone(), false), "{}: {sql}", s.name);
                    let (ours, is_json) = dbee_reads(n, extended.extended_metadata);
                    assert_eq!(ours, theirs, "{}: {sql}", s.name);
                    if is_json {
                        json.push(ours.0);
                    }
                }
                // MariaDB names its JSON in the extended metadata; MySQL has a JSON type.
                let expected: &[&str] = match sql {
                    _ if !mariadb => &[],
                    "SELECT * FROM shop.all_types" => &["doc"],
                    q if q.contains("JSON_OBJECT") => &["j", "x"],
                    _ => &[],
                };
                assert_eq!(json, expected, "{}: {sql}", s.name);
            }
        }
    }
}
