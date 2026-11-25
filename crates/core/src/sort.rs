//! The order a result's rows are shown in when sorted by one column, in memory.
//!
//! A result's cells are the server's printed text and its columns a [`TypeClass`], so this is not
//! the database's `ORDER BY`: it knows no collation, no enum's declaration order and no type's own
//! comparison. It is an order over the rows held, by three rules, each in one place:
//!
//! - **NULL** sorts where the caller says ([`Nulls`], which follows the connection's database):
//!   before every value or after every value ascending, and the other end descending.
//! - **Numbers** ([`TypeClass::Number`], and a [`TypeClass::Other`] cell that reads as one: an
//!   engine that names no type for an expression sends `count(*)` so) compare by value, exactly:
//!   sign, digits and exponent, never through a float, so a 64-bit integer past 2^53 and a long
//!   decimal are right, and `1`, `1.0` and `1e0` are equal. In a Number column `NaN`, `Infinity`
//!   and `-Infinity` (in any case, `inf` too) are numbers, NaN the greatest. A cell that does not
//!   read as a number comes after every number, compared as text.
//! - **Text** (every other class, dates and times included: the ISO forms servers print sort as
//!   text) compares without regard to case first (each character's Unicode lowercase), then by
//!   bytes, so `Apple, apple, fig, Zucchini` ([`text_cmp`]). Case is disregarded over a text's
//!   first [`FOLDED_CHARS`] characters; texts that are the same there but for case are in byte
//!   order.
//!
//! Every cell maps to a key in one ordered set, so the comparison is a total preorder whatever
//! the cells hold (`the_comparison_is_a_total_order`), which the standard sort needs to never
//! panic. The sort is stable and always starts from the order received: rows that compare equal
//! keep the server's order, ascending and descending alike (descending reverses the comparison,
//! not the rows).
//!
//! Nothing is copied: keys borrow the cells, and the result is a list of row indices
//! ([`order`]). Nothing indexes: a short row and a column the result does not have read as NULL.

use std::cmp::Ordering;

use crate::{ResultSet, TypeClass};

/// Where NULL sorts in ascending order; descending puts it at the other end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nulls {
    /// Before every value ascending, after them descending.
    First,
    /// After every value ascending, before them descending.
    Last,
}

/// The rows of `result` sorted by `column`: shown row `i` is `result.rows[order[i]]`. A stable
/// sort from the order received (see the module docs). A column the result does not have sorts
/// nothing (every cell reads as NULL). The indices are `u32`: rows past `u32::MAX`, which no
/// result holds, are left out.
pub fn order(result: &ResultSet, column: usize, descending: bool, nulls: Nulls) -> Vec<u32> {
    let class = result.columns.get(column).map_or(TypeClass::Other, |c| c.class);
    let keys: Vec<Key<'_>> = result.rows.iter().map(|row| Key::of(class, row.get(column).and_then(|cell| cell.as_deref()))).collect();
    let count = u32::try_from(keys.len()).unwrap_or(u32::MAX);
    let mut order: Vec<u32> = (0..count).collect();
    let key = |row: u32| usize::try_from(row).ok().and_then(|row| keys.get(row));
    order.sort_by(|&a, &b| {
        let (Some(a), Some(b)) = (key(a), key(b)) else { return Ordering::Equal };
        let ascending = a.cmp(b, nulls);
        if descending { ascending.reverse() } else { ascending }
    });
    order
}

/// How cells `a` and `b` of a column of `class` compare ascending (`None` is NULL).
pub fn compare(class: TypeClass, a: Option<&str>, b: Option<&str>, nulls: Nulls) -> Ordering {
    Key::of(class, a).cmp(&Key::of(class, b), nulls)
}

/// How many characters of a text are compared without regard to case ([`text_cmp`]).
pub const FOLDED_CHARS: usize = 256;

/// Text order: without regard to case first (each character's Unicode lowercase, over the first
/// [`FOLDED_CHARS`] characters), then by bytes, so only the same text compares equal. The bound
/// keeps a comparison of two long values at a byte comparison: folding 10,000 values of 20 kB
/// that differ only in case took 23 s, measured, and a sort runs in a frame.
pub fn text_cmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (mut x, mut y) = (a.chars().take(FOLDED_CHARS), b.chars().take(FOLDED_CHARS));
    loop {
        match (x.next(), y.next()) {
            // The same character folds the same: only from the first difference on is anything
            // folded.
            (Some(p), Some(q)) if p == q => {}
            (p, q) => {
                let folded = p.into_iter().chain(x).flat_map(char::to_lowercase).cmp(q.into_iter().chain(y).flat_map(char::to_lowercase));
                return folded.then_with(|| a.cmp(b));
            }
        }
    }
}

/// What a cell sorts by. Variants in their order, but for NULL, which [`Nulls`] places.
#[derive(Clone, Copy, Debug)]
enum Key<'a> {
    Null,
    Number(Number<'a>),
    Text(&'a str),
}

impl<'a> Key<'a> {
    fn of(class: TypeClass, cell: Option<&'a str>) -> Key<'a> {
        let Some(text) = cell else { return Key::Null };
        let number = match class {
            TypeClass::Number => Number::read(text, true),
            // No type named: digits are a number, a word is a word.
            TypeClass::Other => Number::read(text, false),
            TypeClass::Bool | TypeClass::Text | TypeClass::LongText | TypeClass::Json | TypeClass::DateTime | TypeClass::Binary => None,
        };
        number.map_or(Key::Text(text), Key::Number)
    }

    fn cmp(&self, other: &Key<'_>, nulls: Nulls) -> Ordering {
        let null = match nulls {
            Nulls::First => Ordering::Less,
            Nulls::Last => Ordering::Greater,
        };
        match (self, other) {
            (Key::Null, Key::Null) => Ordering::Equal,
            (Key::Null, _) => null,
            (_, Key::Null) => null.reverse(),
            (Key::Number(a), Key::Number(b)) => a.cmp(b),
            (Key::Number(_), Key::Text(_)) => Ordering::Less,
            (Key::Text(_), Key::Number(_)) => Ordering::Greater,
            (Key::Text(a), Key::Text(b)) => text_cmp(a, b),
        }
    }
}

/// A number as printed. Variants in their order.
#[derive(Clone, Copy, Debug)]
enum Number<'a> {
    NegInfinity,
    Finite(Decimal<'a>),
    Infinity,
    NaN,
}

impl<'a> Number<'a> {
    /// `text` as a number: digits with an optional sign, point and exponent; with `words`, also
    /// `NaN`, `Infinity` and `inf` in any case, the last two with an optional sign.
    fn read(text: &'a str, words: bool) -> Option<Number<'a>> {
        if let Some(decimal) = Decimal::read(text) {
            return Some(Number::Finite(decimal));
        }
        if !words {
            return None;
        }
        let (negative, word) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        if word.eq_ignore_ascii_case("infinity") || word.eq_ignore_ascii_case("inf") {
            Some(if negative { Number::NegInfinity } else { Number::Infinity })
        } else if text.eq_ignore_ascii_case("nan") {
            Some(Number::NaN)
        } else {
            None
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Number::NegInfinity => 0,
            Number::Finite(_) => 1,
            Number::Infinity => 2,
            Number::NaN => 3,
        }
    }

    fn cmp(&self, other: &Number<'_>) -> Ordering {
        match (self, other) {
            (Number::Finite(a), Number::Finite(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

/// A finite number as its sign, the place of its first digit that is not zero, and its digits
/// where they stand in the cell.
#[derive(Clone, Copy, Debug)]
struct Decimal<'a> {
    /// -1, 0 (every digit is zero, whatever the sign) or 1.
    sign: i8,
    /// The power of ten above the first digit that is not zero: 1 for `5`, 3 for `123.4`, 0 for
    /// `0.5`, -1 for `0.05`. Saturating, so exponents no number has compare by their digits.
    top: i64,
    int: &'a str,
    frac: &'a str,
}

impl<'a> Decimal<'a> {
    fn read(text: &'a str) -> Option<Decimal<'a>> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
            Some((mantissa, exponent)) => (mantissa, read_exponent(exponent)?),
            None => (rest, 0),
        };
        let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        let digits = |part: &str| part.bytes().all(|b| b.is_ascii_digit());
        if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
            return None;
        }
        let zeros = int.bytes().chain(frac.bytes()).take_while(|b| *b == b'0').count();
        if zeros == int.len().saturating_add(frac.len()) {
            return Some(Decimal { sign: 0, top: 0, int, frac });
        }
        let wide = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        let top = wide(int.len()).saturating_sub(wide(zeros)).saturating_add(exponent);
        Some(Decimal { sign: if negative { -1 } else { 1 }, top, int, frac })
    }

    /// The digits from the first that is not zero.
    fn digits(&self) -> impl Iterator<Item = u8> + '_ {
        self.int.bytes().chain(self.frac.bytes()).skip_while(|b| *b == b'0')
    }

    /// Which is further from zero, both being non-zero.
    fn magnitude_cmp(&self, other: &Decimal<'_>) -> Ordering {
        self.top.cmp(&other.top).then_with(|| {
            let (mut a, mut b) = (self.digits(), other.digits());
            loop {
                // Past its last digit a number goes on in zeros.
                let (x, y) = match (a.next(), b.next()) {
                    (None, None) => return Ordering::Equal,
                    (x, y) => (x.unwrap_or(b'0'), y.unwrap_or(b'0')),
                };
                if x != y {
                    return x.cmp(&y);
                }
            }
        })
    }

    fn cmp(&self, other: &Decimal<'_>) -> Ordering {
        self.sign.cmp(&other.sign).then_with(|| match self.sign {
            1 => self.magnitude_cmp(other),
            -1 => other.magnitude_cmp(self),
            _ => Ordering::Equal,
        })
    }
}

/// An exponent's digits with an optional sign, saturating.
fn read_exponent(text: &str) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for b in digits.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.saturating_mul(10).saturating_add(i64::from(b.saturating_sub(b'0')));
    }
    Some(if negative { value.saturating_neg() } else { value })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use proptest::prelude::*;

    use super::*;
    use crate::Column;

    const CLASSES: [TypeClass; 8] = [
        TypeClass::Number,
        TypeClass::Bool,
        TypeClass::Text,
        TypeClass::LongText,
        TypeClass::Json,
        TypeClass::DateTime,
        TypeClass::Binary,
        TypeClass::Other,
    ];

    /// One column of `class` holding `cells` (`None` is NULL).
    fn column(class: TypeClass, cells: &[Option<&str>]) -> ResultSet {
        ResultSet {
            columns: vec![Column { class, ..Column::untyped("c") }],
            rows: cells.iter().map(|c| vec![c.map(str::to_owned)]).collect(),
            rows_seen: cells.len() as u64,
            complete: true,
            ..ResultSet::default()
        }
    }

    /// `cells` as [`order`] sorts them, NULL written `NULL`.
    fn sorted<'a>(class: TypeClass, cells: &[Option<&'a str>], descending: bool, nulls: Nulls) -> Vec<&'a str> {
        let rs = column(class, cells);
        order(&rs, 0, descending, nulls).iter().map(|&i| cells[i as usize].unwrap_or("NULL")).collect()
    }

    fn values<'a>(cells: &[&'a str]) -> Vec<Option<&'a str>> {
        cells.iter().map(|c| Some(*c)).collect()
    }

    #[test]
    fn numbers_sort_by_value_not_text() {
        let asc = |cells: &[&'static str]| sorted(TypeClass::Number, &values(cells), false, Nulls::Last);
        assert_eq!(asc(&["2", "10", "9", "100"]), ["2", "9", "10", "100"]);
        assert_eq!(asc(&["-2", "3", "-10", "0", "-0.5", "+4"]), ["-10", "-2", "-0.5", "0", "3", "+4"]);
        assert_eq!(asc(&["1.5", "1.25", "1.125", "0.05", "0.5", ".7", "12."]), ["0.05", "0.5", ".7", "1.125", "1.25", "1.5", "12."]);
        // A 64-bit integer past 2^53, where a float no longer tells neighbours apart.
        assert_eq!(
            asc(&["9007199254740993", "9007199254740992", "9223372036854775807"]),
            ["9007199254740992", "9007199254740993", "9223372036854775807"]
        );
        // A 40-digit numeric, and one that differs from it in its last digit.
        let (big, bigger) = ("1234567890123456789012345678901234567890", "1234567890123456789012345678901234567891");
        assert_eq!(asc(&[bigger, "99999999999999999999", big]), ["99999999999999999999", big, bigger]);
        // Exponent forms compare by value with the plain ones, exactly.
        assert_eq!(
            asc(&["1e3", "999", "1001", "2.5e-1", "0.3", "9007199254740993e0", "9007199254740992"]),
            ["2.5e-1", "0.3", "999", "1e3", "1001", "9007199254740992", "9007199254740993e0"]
        );
        assert_eq!(asc(&["Infinity", "5", "NaN", "-Infinity", "-1e400", "1e400"]), ["-Infinity", "-1e400", "5", "1e400", "Infinity", "NaN"]);
        // What is no number comes after every number, as text.
        assert_eq!(asc(&["$5.00", "7", "NaN", "$10.00", "abc"]), ["7", "NaN", "$10.00", "$5.00", "abc"]);
        let desc = sorted(TypeClass::Number, &values(&["2", "10", "9", "100"]), true, Nulls::Last);
        assert_eq!(desc, ["100", "10", "9", "2"]);
    }

    #[test]
    fn equal_numbers_in_other_spellings_are_equal() {
        let eq = |a: &str, b: &str| compare(TypeClass::Number, Some(a), Some(b), Nulls::Last);
        for (a, b) in
            [("1", "1.0"), ("1", "1e0"), ("100", "1e2"), ("0.01", "1E-2"), ("0", "-0"), ("0", "0.000"), ("007", "7"), ("-0e5", "0"), ("+3", "3")]
        {
            assert_eq!(eq(a, b), Ordering::Equal, "{a} {b}");
        }
        // Exponents past what a number can have saturate instead of overflowing.
        assert_eq!(eq("1e99999999999999999999999", "1e9"), Ordering::Greater);
        assert_eq!(eq("1e-99999999999999999999999", "1e-9"), Ordering::Less);
        assert_eq!(eq("-1e99999999999999999999999", "-1e9"), Ordering::Less);
        // No numbers: they are text, after the numbers.
        for text in ["", "-", "+", ".", "e5", "1e", "1e+", "1.2.3", "1 ", " 1", "0x10", "1,000", "١", "--1", "1e5e5"] {
            assert_eq!(eq("999999", text), Ordering::Less, "{text:?}");
        }
    }

    #[test]
    fn null_placement_follows_the_rule() {
        let cells = [Some("b"), None, Some("a"), None];
        assert_eq!(sorted(TypeClass::Text, &cells, false, Nulls::First), ["NULL", "NULL", "a", "b"]);
        assert_eq!(sorted(TypeClass::Text, &cells, true, Nulls::First), ["b", "a", "NULL", "NULL"]);
        assert_eq!(sorted(TypeClass::Text, &cells, false, Nulls::Last), ["a", "b", "NULL", "NULL"]);
        assert_eq!(sorted(TypeClass::Text, &cells, true, Nulls::Last), ["NULL", "NULL", "b", "a"]);
        let cells = [Some("2"), None, Some("10"), Some("x")];
        assert_eq!(sorted(TypeClass::Number, &cells, false, Nulls::First), ["NULL", "2", "10", "x"]);
        assert_eq!(sorted(TypeClass::Number, &cells, false, Nulls::Last), ["2", "10", "x", "NULL"]);
    }

    #[test]
    fn text_sorts_as_decided() {
        let asc = |cells: &[&'static str]| sorted(TypeClass::Text, &values(cells), false, Nulls::Last);
        // Without regard to case first, then by bytes: the capital before its small letter.
        assert_eq!(asc(&["pear", "Zucchini", "apple", "fig", "Apple"]), ["Apple", "apple", "fig", "pear", "Zucchini"]);
        assert_eq!(asc(&["Öl", "öl", "zebra", "ÉCOLE", "école", "Zebra"]), ["Zebra", "zebra", "ÉCOLE", "école", "Öl", "öl"]);
        // Digits in text are text.
        assert_eq!(asc(&["10", "9", "2"]), ["10", "2", "9"]);
        assert_eq!(asc(&["b", "", "a"]), ["", "a", "b"]);
        // The ISO forms of dates and times sort as text; so do t and f.
        let dates = values(&["2026-11-25 10:00:00", "2026-02-03 09:00:00", "1999-12-31 23:59:59"]);
        assert_eq!(sorted(TypeClass::DateTime, &dates, false, Nulls::Last), ["1999-12-31 23:59:59", "2026-02-03 09:00:00", "2026-11-25 10:00:00"]);
        assert_eq!(sorted(TypeClass::Bool, &values(&["t", "f", "t"]), false, Nulls::Last), ["f", "t", "t"]);
        // Only the same text is equal.
        assert_eq!(text_cmp("aé", "aè"), "é".cmp("è"));
        assert_eq!(text_cmp("straße", "STRASSE"), Ordering::Greater);
        assert_eq!(text_cmp("same", "same"), Ordering::Equal);
        assert_eq!(text_cmp("Same", "same"), Ordering::Less);
        assert_eq!(text_cmp("ab", "abc"), Ordering::Less);
        // Case is disregarded over the first 256 characters; past them the bytes decide.
        let start = "é".repeat(FOLDED_CHARS - 1);
        assert_eq!(text_cmp(&format!("{start}B"), &format!("{start}a")), Ordering::Greater, "the 256th character folds");
        assert_eq!(text_cmp(&format!("{start}xB"), &format!("{start}xa")), Ordering::Less, "the 257th does not");
        assert_eq!(text_cmp(&format!("{start}XB"), &format!("{start}xa")), Ordering::Less);
        assert_eq!(text_cmp(&format!("{start}Xa"), &format!("{start}xB")), Ordering::Less, "the same but for case there: bytes");
    }

    #[test]
    fn untyped_cells_that_are_numbers_sort_as_numbers() {
        let asc = |cells: &[&'static str]| sorted(TypeClass::Other, &values(cells), false, Nulls::Last);
        assert_eq!(asc(&["10", "9", "100", "2"]), ["2", "9", "10", "100"]);
        assert_eq!(asc(&["pear", "10", "Apple", "9", "-1.5"]), ["-1.5", "9", "10", "Apple", "pear"]);
        // Without a type a word is a word: a name is not a number.
        assert_eq!(asc(&["Nan", "Ian", "Inf", "Zoe", "3"]), ["3", "Ian", "Inf", "Nan", "Zoe"]);
    }

    #[test]
    fn ties_keep_the_received_order_both_ways() {
        let rs = ResultSet {
            columns: vec![Column { class: TypeClass::Number, ..Column::untyped("n") }, Column::untyped("tag")],
            rows: [("1", "a"), ("2", "b"), ("1.0", "c"), ("2", "d"), ("1", "e")]
                .iter()
                .map(|(n, t)| vec![Some((*n).to_owned()), Some((*t).to_owned())])
                .collect(),
            ..ResultSet::default()
        };
        assert_eq!(order(&rs, 0, false, Nulls::Last), [0, 2, 4, 1, 3]);
        assert_eq!(order(&rs, 0, true, Nulls::Last), [1, 3, 0, 2, 4], "descending keeps the server's order among equals too");
        // All NULL, or all the same: the order received.
        let nulls = column(TypeClass::Text, &[None, None, None]);
        assert_eq!(order(&nulls, 0, true, Nulls::First), [0, 1, 2]);
    }

    #[test]
    fn a_short_row_and_a_missing_column_read_as_null() {
        let rs = ResultSet {
            columns: vec![Column::untyped("a"), Column::untyped("b")],
            rows: vec![vec![Some("x".into()), Some("2".into())], vec![Some("y".into())], vec![], vec![Some("z".into()), Some("1".into())]],
            ..ResultSet::default()
        };
        assert_eq!(order(&rs, 1, false, Nulls::Last), [3, 0, 1, 2]);
        assert_eq!(order(&rs, 1, false, Nulls::First), [1, 2, 3, 0]);
        // A column the result does not have: nothing to sort by, so the order received.
        for column in [2, 99, usize::MAX] {
            assert_eq!(order(&rs, column, false, Nulls::Last), [0, 1, 2, 3]);
            assert_eq!(order(&rs, column, true, Nulls::First), [0, 1, 2, 3]);
        }
        assert_eq!(order(&ResultSet::default(), 0, false, Nulls::Last), Vec::<u32>::new());
    }

    fn arb_class() -> impl Strategy<Value = TypeClass> {
        (0..CLASSES.len()).prop_map(|i| CLASSES[i])
    }

    /// Cells that meet every branch: NULL, numbers in every spelling, the words, texts that differ
    /// in case only, and any text.
    fn arb_cell() -> impl Strategy<Value = Option<String>> {
        prop_oneof![
            1 => Just(None),
            4 => "[+-]?[0-9]{0,4}(\\.[0-9]{0,4})?([eE][+-]?[0-9]{1,2})?".prop_map(Some),
            1 => "[+-]?0{0,3}[1-9]?0{0,3}(\\.0{0,3}[1-9]?0{0,3})?([eE][+-]?[0-9]{1,25})?".prop_map(Some),
            1 => prop_oneof![Just("NaN"), Just("nan"), Just("Infinity"), Just("-Infinity"), Just("inf"), Just("-inf"), Just("+Inf")].prop_map(|w| Some(w.to_owned())),
            2 => "[aAbBßẞéÉİi ]{0,4}".prop_map(Some),
            2 => ".{0,12}".prop_map(Some),
        ]
    }

    fn arb_nulls() -> impl Strategy<Value = Nulls> {
        prop_oneof![Just(Nulls::First), Just(Nulls::Last)]
    }

    proptest! {
        #[test]
        fn the_order_is_a_permutation_and_never_panics(
            classes in proptest::collection::vec(arb_class(), 0..4),
            rows in proptest::collection::vec(proptest::collection::vec(arb_cell(), 0..5), 0..40),
            column in prop_oneof![0..6usize, Just(usize::MAX)],
            descending: bool,
            nulls in arb_nulls(),
        ) {
            let rs = ResultSet {
                columns: classes.iter().map(|class| Column { class: *class, ..Column::untyped("c") }).collect(),
                rows,
                ..ResultSet::default()
            };
            let got = order(&rs, column, descending, nulls);
            let mut seen = got.clone();
            seen.sort_unstable();
            prop_assert_eq!(seen, (0..rs.rows.len() as u32).collect::<Vec<_>>());
            // In order, and equal neighbours in the order received.
            let class = rs.columns.get(column).map_or(TypeClass::Other, |c| c.class);
            let cell = |row: u32| rs.rows[row as usize].get(column).and_then(|c| c.as_deref());
            for pair in got.windows(2) {
                let ascending = compare(class, cell(pair[0]), cell(pair[1]), nulls);
                let shown = if descending { ascending.reverse() } else { ascending };
                prop_assert_ne!(shown, Ordering::Greater);
                prop_assert!(shown == Ordering::Less || pair[0] < pair[1], "a tie out of the order received");
            }
        }

        #[test]
        fn the_comparison_is_a_total_order(class in arb_class(), a in arb_cell(), b in arb_cell(), c in arb_cell(), nulls in arb_nulls()) {
            let cmp = |x: &Option<String>, y: &Option<String>| compare(class, x.as_deref(), y.as_deref(), nulls);
            let cells = [&a, &b, &c];
            for x in cells {
                prop_assert_eq!(cmp(x, x), Ordering::Equal);
                for y in cells {
                    prop_assert_eq!(cmp(x, y), cmp(y, x).reverse(), "{:?} {:?}", x, y);
                    for z in cells {
                        if cmp(x, y) != Ordering::Greater && cmp(y, z) != Ordering::Greater {
                            prop_assert_ne!(cmp(x, z), Ordering::Greater, "{:?} {:?} {:?}", x, y, z);
                        }
                        if cmp(x, y) == Ordering::Equal && cmp(y, z) == Ordering::Equal {
                            prop_assert_eq!(cmp(x, z), Ordering::Equal, "{:?} {:?} {:?}", x, y, z);
                        }
                    }
                }
            }
        }

        #[test]
        fn text_order_is_the_folded_text_then_the_bytes(a in "[aAbBßẞéÉİi ]{0,6}|.{0,12}", b in "[aAbBßẞéÉİi ]{0,6}|.{0,12}") {
            // For texts within the folded length, `text_cmp` is the lowercase texts compared,
            // then the bytes. `str::to_lowercase` differs from the per-character lowercase only
            // for a final sigma.
            if !a.contains('Σ') && !b.contains('Σ') {
                let whole = a.to_lowercase().chars().cmp(b.to_lowercase().chars()).then_with(|| a.cmp(&b));
                prop_assert_eq!(text_cmp(&a, &b), whole);
            }
            prop_assert_eq!(text_cmp(&a, &b) == Ordering::Equal, a == b);
        }

        #[test]
        fn numbers_agree_with_floats_where_floats_are_exact(a in -100_000i64..100_000, b in -100_000i64..100_000, da in 0u32..4, db in 0u32..4) {
            // a / 10^da against b / 10^db, written with a point, compared as scaled integers.
            let write = |n: i64, d: u32| {
                let digits = format!("{:0>width$}", n.unsigned_abs(), width = d as usize + 1);
                let (int, frac) = digits.split_at(digits.len() - d as usize);
                format!("{}{int}{}{frac}", if n < 0 { "-" } else { "" }, if d > 0 { "." } else { "" })
            };
            let want = (a * 10i64.pow(db)).cmp(&(b * 10i64.pow(da)));
            prop_assert_eq!(compare(TypeClass::Number, Some(&write(a, da)), Some(&write(b, db)), Nulls::Last), want, "{} {}", write(a, da), write(b, db));
            // The same numbers in exponent form.
            let exp = |n: i64, d: u32| format!("{n}e-{d}");
            prop_assert_eq!(compare(TypeClass::Other, Some(&exp(a, da)), Some(&write(b, db)), Nulls::Last), want);
        }
    }

    /// The cost of a sort at the query cap (10,000 rows), which runs in a frame: 16 ms for what
    /// results usually hold, and half a second for the worst the text order can meet.
    #[test]
    #[ignore = "timing; run with --ignored --nocapture"]
    fn a_sort_of_ten_thousand_rows_within_a_frame() {
        let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        println!("load average: {}", load.trim());
        let numbers: Vec<String> = (0..10_000u64).map(|i| (i.wrapping_mul(2_654_435_761) % 1_000_003).to_string()).collect();
        let texts: Vec<String> = (0..10_000u64).map(|i| format!("Customer {} of {}", i.wrapping_mul(2_654_435_761) % 1_000_003, i % 7)).collect();
        // 10,000 values of 20 kB that differ only at their end, and only in case before that.
        let long: Vec<String> = (0..10_000u64)
            .map(|i| format!("{}{}", if i % 2 == 0 { "x" } else { "X" }.repeat(20_000), i.wrapping_mul(2_654_435_761) % 10_007))
            .collect();
        let same: Vec<String> = (0..10_000u64).map(|_| "y".repeat(20_000)).collect();
        for (what, class, cells, budget) in [
            ("numbers", TypeClass::Number, &numbers, 16.0),
            ("numbers without a type", TypeClass::Other, &numbers, 16.0),
            ("short texts", TypeClass::Text, &texts, 16.0),
            ("the same 20 kB text", TypeClass::LongText, &same, 16.0),
            ("20 kB texts that differ in case along their whole length", TypeClass::LongText, &long, 500.0),
        ] {
            let rs = ResultSet {
                columns: vec![Column { class, ..Column::untyped("c") }],
                rows: cells.iter().map(|c| vec![Some(c.clone())]).collect(),
                ..ResultSet::default()
            };
            let mut times: Vec<f64> = (0..5)
                .map(|i| {
                    let start = Instant::now();
                    let got = order(&rs, 0, i % 2 == 1, Nulls::Last);
                    std::hint::black_box(got);
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            times.sort_by(|a, b| a.total_cmp(b));
            let median = times[times.len() / 2];
            println!("order() over 10,000 rows of {what}: median {median:.2} ms");
            assert!(median < budget, "{what}: median {median} ms");
        }
    }
}
