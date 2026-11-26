//! The order a result's rows are shown in when sorted by one column, in memory.
//!
//! A result's cells are the server's printed text and its columns a [`TypeClass`], so this is not
//! the database's `ORDER BY`: it knows no collation, no enum's declaration order and no type's own
//! comparison. It is an order over the rows held, by three rules, each in one place:
//!
//! - **NULL** sorts where the caller says ([`Nulls`], which follows the connection's database):
//!   before every value or after every value ascending, and the other end descending.
//! - **Numbers** ([`TypeClass::Number`], and a [`TypeClass::Other`] or [`TypeClass::Bool`] cell
//!   that reads as one: an engine that names no type for an expression sends `count(*)` so, and
//!   MySQL's boolean is a small integer) compare by value, exactly:
//!   sign, digits and exponent, never through a float, so a 64-bit integer past 2^53 and a long
//!   decimal are right, and `1`, `1.0` and `1e0` are equal. In a Number column `NaN`, `Infinity`
//!   and `-Infinity` (in any case, `inf` too) are numbers, NaN the greatest. A cell that does not
//!   read as a number comes after every number, compared as text.
//!   A number is read once, into its sign, place and digits without the zeros around them, so a
//!   comparison of two numbers is a comparison of slices. A cell longer than [`NUMBER_BYTES`]
//!   is not read as a number: it is text.
//! - **Text** (every other class, dates and times included: the ISO forms servers print sort as
//!   text) compares without regard to case first (each character's Unicode lowercase), then by
//!   bytes, so `Apple, apple, fig, Zucchini` ([`text_cmp`]). Case is disregarded over a text's
//!   first [`FOLDED_CHARS`] characters; texts that are the same there, or the same but for case,
//!   are in byte order.
//!
//! Every cell maps to a key in one ordered set, so the comparison is a total preorder whatever
//! the cells hold (`the_comparison_is_a_total_order`), which the standard sort needs to never
//! panic. The sort is stable and always starts from the order received: rows that compare equal
//! keep the server's order, ascending and descending alike (descending reverses the comparison,
//! not the rows).
//!
//! No cell is copied or moved: keys borrow the cells, and the result is a list of row indices
//! ([`order`]). The one thing a sort makes is a text's key without regard to case, its first
//! [`FOLDED_CHARS`] characters in lowercase, and only for a text met with one that differs from it
//! in case where they first differ: made once, and kept while the sort runs, so no comparison
//! folds a text again ([`Text`]). Nothing indexes: a short row reads as NULL, and a column the
//! result does not have sorts nothing.

use std::cell::OnceCell;
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
/// nothing: the order received, whatever a row holds past the columns. The indices are `u32`:
/// rows past `u32::MAX`, which no result holds, are left out.
pub fn order(result: &ResultSet, column: usize, descending: bool, nulls: Nulls) -> Vec<u32> {
    let count = u32::try_from(result.rows.len()).unwrap_or(u32::MAX);
    let mut order: Vec<u32> = (0..count).collect();
    let Some(class) = result.columns.get(column).map(|c| c.class) else { return order };
    let keys: Vec<Key<'_>> = result.rows.iter().map(|row| Key::of(class, row.get(column).and_then(|cell| cell.as_deref()))).collect();
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

/// How many characters of a text are compared without regard to case ([`text_cmp`]). The bound
/// keeps what a sort folds and holds short whatever the values' length: a text's key is at most
/// this many characters' lowercase, where the whole of 10,000 values of 20 kB would be 200 MB of
/// keys, and past it two long values compare as bytes.
pub const FOLDED_CHARS: usize = 256;

/// Text order: without regard to case first (each character's Unicode lowercase, over the first
/// [`FOLDED_CHARS`] characters), then by bytes, so only the same text compares equal.
pub fn text_cmp(a: &str, b: &str) -> Ordering {
    Text::new(a).cmp(&Text::new(b))
}

/// A text as it sorts: by its key without regard to case, then by its bytes ([`text_cmp`]).
///
/// The key is made by the first comparison that needs it and kept, so a sort folds each text at
/// most once: folding at every comparison took 1.8 s for 10,000 texts of 256 letters that differ
/// only in case, measured. Most comparisons need no key: see [`Text::cmp`].
#[derive(Debug)]
struct Text<'a> {
    text: &'a str,
    folded: OnceCell<Box<str>>,
}

impl<'a> Text<'a> {
    fn new(text: &'a str) -> Text<'a> {
        Text { text, folded: OnceCell::new() }
    }

    /// The key without regard to case: the first [`FOLDED_CHARS`] characters, each as its
    /// Unicode lowercase.
    fn folded(&self) -> &str {
        self.folded.get_or_init(|| self.text.chars().take(FOLDED_CHARS).flat_map(char::to_lowercase).collect::<String>().into_boxed_str())
    }

    /// By the keys ([`Self::folded`]), then by the bytes. Up to the first characters that differ
    /// both keys are the same, so where those two fold to another first character that settles
    /// the keys' order with nothing folded but the two; the keys themselves are compared, and so
    /// made, only where the two are the same but for case (or their lowercases begin alike).
    fn cmp(&self, other: &Text<'_>) -> Ordering {
        let (a, b) = (self.text, other.text);
        if a == b {
            return Ordering::Equal;
        }
        let (mut x, mut y) = (a.chars().take(FOLDED_CHARS), b.chars().take(FOLDED_CHARS));
        loop {
            match (x.next(), y.next()) {
                (Some(p), Some(q)) if p == q => {}
                // The same over the folded length: the same key, so the bytes decide.
                (None, None) => return a.cmp(b),
                // One ends where the other goes on: its key is the start of the other's.
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(p), Some(q)) => {
                    return match p.to_lowercase().next().cmp(&q.to_lowercase().next()) {
                        Ordering::Equal => self.folded().cmp(other.folded()).then_with(|| a.cmp(b)),
                        decided => decided,
                    };
                }
            }
        }
    }
}

/// What a cell sorts by. Variants in their order, but for NULL, which [`Nulls`] places.
#[derive(Debug)]
enum Key<'a> {
    Null,
    Number(Number<'a>),
    Text(Text<'a>),
}

impl<'a> Key<'a> {
    fn of(class: TypeClass, cell: Option<&'a str>) -> Key<'a> {
        let Some(text) = cell else { return Key::Null };
        let number = match class {
            TypeClass::Number => Number::read(text, true),
            // No type named: digits are a number, a word is a word. A boolean is read the same
            // way: where the engine keeps one as a small integer (MySQL's `TINYINT(1)`) the
            // column can hold any, and `f`, `false` and `0` come before `t`, `true` and `1`
            // either way.
            TypeClass::Other | TypeClass::Bool => Number::read(text, false),
            TypeClass::Text | TypeClass::LongText | TypeClass::Json | TypeClass::DateTime | TypeClass::Binary => None,
        };
        number.map_or_else(|| Key::Text(Text::new(text)), Key::Number)
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
            (Key::Text(a), Key::Text(b)) => a.cmp(b),
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

/// The longest cell that is read as a number; a longer one is text, after the numbers. Few
/// numbers print longer (PostgreSQL's `numeric` can: `factorial(1000)` has 2,568 digits), and the
/// bound keeps reading a cell, and comparing two, short: 10,000 numbers of 25,000 digits that
/// differ at their end took 0.5 s to sort, measured, the reading and the comparing of their
/// shared digits both counted.
pub const NUMBER_BYTES: usize = 1024;

/// A finite number as its sign, the place of its first digit that is not zero, and its digits
/// from that one to the last that is not zero, where they stand in the cell. The same value in
/// any spelling (`1`, `1.0`, `0.1e1`, `001`) reads as the same three, so two numbers compare by
/// a few slice comparisons, whatever their length.
///
/// **Near the exponent's bound** the order is by a rule, not by value. The written exponent
/// saturates at `i64::MAX` and at its negative before the digits' place is counted in, and `top`
/// saturates at the ends of an `i64`, so of two numbers whose exponent is at such an end or
/// within the cell's length of it, the digits decide:
/// `9e9223372036854775806` sorts above `1e9223372036854775807`, though it is the smaller, and
/// `1e9223372036854775807` equals `10e9223372036854775807`. No database prints such a number.
/// The order stays total there (`numbers_past_the_exponent_bound_keep_a_total_order`).
#[derive(Clone, Copy, Debug)]
struct Decimal<'a> {
    /// -1, 0 (every digit is zero, whatever the sign) or 1.
    sign: i8,
    /// The power of ten above the first digit that is not zero: 1 for `5`, 3 for `123.4`, 0 for
    /// `0.5`, -1 for `0.05`. Saturating.
    top: i64,
    /// The digits in two parts that follow each other (the integer part's and the fraction's,
    /// or one of them and nothing): the first starts with a digit that is not zero and the last
    /// ends with one. Both empty for zero.
    digits: [&'a [u8]; 2],
}

impl<'a> Decimal<'a> {
    fn read(text: &'a str) -> Option<Decimal<'a>> {
        if text.len() > NUMBER_BYTES {
            return None;
        }
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
        let wide = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        let (top, digits) = if int.is_empty() {
            // 0.00ddd: the place falls by each zero after the point.
            let first = frac.trim_start_matches('0');
            let zeros = frac.len().saturating_sub(first.len());
            (exponent.saturating_sub(wide(zeros)), [first.as_bytes(), &[][..]])
        } else if frac.is_empty() {
            // ddd000: the zeros at the end are not digits of the number.
            (wide(int.len()).saturating_add(exponent), [int.trim_end_matches('0').as_bytes(), &[][..]])
        } else {
            (wide(int.len()).saturating_add(exponent), [int.as_bytes(), frac.as_bytes()])
        };
        if digits.iter().all(|part| part.is_empty()) {
            return Some(Decimal { sign: 0, top: 0, digits });
        }
        Some(Decimal { sign: if negative { -1 } else { 1 }, top, digits })
    }

    /// Which is further from zero, both being non-zero: the higher place, then the digits. With
    /// no zeros at either end, digits that go on where the other's stop are the larger.
    fn magnitude_cmp(&self, other: &Decimal<'_>) -> Ordering {
        self.top.cmp(&other.top).then_with(|| {
            let ([mut a, mut a_next], [mut b, mut b_next]) = (self.digits, other.digits);
            loop {
                if a.is_empty() {
                    (a, a_next) = (a_next, &[]);
                }
                if b.is_empty() {
                    (b, b_next) = (b_next, &[]);
                }
                let shared = a.len().min(b.len());
                let (Some((a_head, a_rest)), Some((b_head, b_rest))) = (a.split_at_checked(shared), b.split_at_checked(shared)) else {
                    return a.len().cmp(&b.len());
                };
                if shared == 0 {
                    return a.len().cmp(&b.len());
                }
                match a_head.cmp(b_head) {
                    Ordering::Equal => (a, b) = (a_rest, b_rest),
                    decided => return decided,
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
        // The same digits split otherwise between the integer part, the fraction and the
        // exponent: the same number, and its neighbours on their own sides of it.
        for (a, b) in
            [(".99", "9.9e-1"), (".99e1", "9.9"), ("0.05e3", "50"), ("1234e-2", "12.34"), ("1.25e1", "12.5"), ("120", "1.2e2"), ("1200.0", "12e2")]
        {
            assert_eq!((eq(a, b), eq(b, a)), (Ordering::Equal, Ordering::Equal), "{a} {b}");
        }
        for (less, more) in
            [(".99", "9.99e-1"), ("1234e-2", "12.35"), ("1.25e1", "12.6"), ("100.25", "100.5"), ("100.5", "1000"), ("9.99", "10"), ("0.05e3", "50.5")]
        {
            assert_eq!((eq(less, more), eq(more, less)), (Ordering::Less, Ordering::Greater), "{less} {more}");
            let (neg_less, neg_more) = (format!("-{less}"), format!("-{more}"));
            assert_eq!(eq(&neg_less, &neg_more), Ordering::Greater, "{neg_less} {neg_more}");
        }
        // Exponents past what a number can have saturate instead of overflowing.
        assert_eq!(eq("1e99999999999999999999999", "1e9"), Ordering::Greater);
        assert_eq!(eq("1e-99999999999999999999999", "1e-9"), Ordering::Less);
        assert_eq!(eq("-1e99999999999999999999999", "-1e9"), Ordering::Less);
        // A cell past the length read as a number is text, after the numbers.
        let long = "1".repeat(NUMBER_BYTES + 1);
        assert_eq!(eq(&long, "2"), Ordering::Greater);
        assert_eq!(eq(&"1".repeat(NUMBER_BYTES), "2"), Ordering::Greater, "the larger number");
        // `!` is text, and by bytes comes before a digit: a number is before it all the same.
        assert_eq!(eq(&"1".repeat(NUMBER_BYTES), "!"), Ordering::Less, "a number, before any text");
        assert_eq!(eq(&long, "!"), Ordering::Greater, "text, by its bytes");
        // The bound itself: zeros are zero up to it, and text one byte past it.
        assert_eq!(eq(&"0".repeat(NUMBER_BYTES), "0"), Ordering::Equal, "still a number");
        assert_eq!(eq(&"0".repeat(NUMBER_BYTES + 1), "0"), Ordering::Greater, "text, after the numbers");
        assert_eq!(eq(&format!("0.{}", "0".repeat(NUMBER_BYTES - 3)), "0"), Ordering::Equal);
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
        // Only the same text is equal.
        assert_eq!(text_cmp("aé", "aè"), "é".cmp("è"));
        assert_eq!(text_cmp("straße", "STRASSE"), Ordering::Greater);
        assert_eq!(text_cmp("same", "same"), Ordering::Equal);
        assert_eq!(text_cmp("Same", "same"), Ordering::Less);
        assert_eq!(text_cmp("ab", "abc"), Ordering::Less);
        // A text longer than the folded length is folded at its start like any other.
        let tail = "x".repeat(300);
        assert_eq!(text_cmp(&format!("B{tail}"), &format!("a{tail}")), Ordering::Greater);
        assert_eq!(text_cmp(&format!("a{tail}"), &format!("B{tail}")), Ordering::Less);
        assert_eq!(text_cmp(&format!("b{tail}"), "C"), Ordering::Less);
        // Case is disregarded over the first 256 characters; past them the bytes decide.
        let start = "é".repeat(FOLDED_CHARS - 1);
        assert_eq!(text_cmp(&format!("{start}B"), &format!("{start}a")), Ordering::Greater, "the 256th character folds");
        assert_eq!(text_cmp(&format!("{start}xB"), &format!("{start}xa")), Ordering::Less, "the 257th does not");
        assert_eq!(text_cmp(&format!("{start}XB"), &format!("{start}xa")), Ordering::Less);
        assert_eq!(text_cmp(&format!("{start}Xa"), &format!("{start}xB")), Ordering::Less, "the same but for case there: bytes");
    }

    /// A text's key without regard to case is made by the comparison that needs it, once: where
    /// two texts first differ by more than case nothing is kept. The key is each of the first
    /// 256 characters as its own lowercase, however long that makes it.
    #[test]
    fn a_texts_key_is_made_where_a_first_difference_is_one_of_case() {
        let (apple, pear) = (Text::new("apple pie"), Text::new("apple tart"));
        assert_eq!(apple.cmp(&pear), Ordering::Less);
        assert_eq!(apple.cmp(&Text::new("apple")), Ordering::Greater, "the longer, after its own start");
        assert_eq!(apple.cmp(&Text::new("apple pie")), Ordering::Equal);
        assert!(apple.folded.get().is_none() && pear.folded.get().is_none(), "no key for texts that differ by a letter");
        let capital = Text::new("Apple Pie");
        assert_eq!(capital.cmp(&apple), Ordering::Less);
        assert_eq!((capital.folded.get().map(|k| &**k), apple.folded.get().map(|k| &**k)), (Some("apple pie"), Some("apple pie")));
        // A lowercase of two characters begins as another's of one: the keys decide.
        let (dotted, plain) = (Text::new("İx"), Text::new("iy"));
        assert_eq!(dotted.cmp(&plain), "i\u{307}x".cmp("iy"));
        assert_eq!(dotted.folded.get().map(|k| &**k), Some("i\u{307}x"));
        // The key of a long text is its first 256 characters', whatever follows them.
        let long = Text::new(&"É".repeat(5_000)).folded().to_owned();
        assert_eq!(long, "é".repeat(FOLDED_CHARS));
        // The 256 are counted before they are folded: a lowercase of two characters takes none
        // of the others' room.
        let dotted = Text::new(&"İ".repeat(300)).folded().to_owned();
        assert_eq!(dotted, "i\u{307}".repeat(FOLDED_CHARS));
        let run = "İ".repeat(FOLDED_CHARS - 2);
        assert_eq!(text_cmp(&format!("X{run}b"), &format!("x{run}a")), Ordering::Greater, "the 256th character is in the key");
        // Each character's own lowercase, not the text's: a sigma at a word's end is a sigma.
        assert_eq!(Text::new("AΣ").folded(), "aσ");
        assert_eq!(text_cmp("Aσ", "aΣ"), Ordering::Less, "the same key, so the bytes");
    }

    #[test]
    fn untyped_cells_that_are_numbers_sort_as_numbers() {
        let asc = |cells: &[&'static str]| sorted(TypeClass::Other, &values(cells), false, Nulls::Last);
        assert_eq!(asc(&["10", "9", "100", "2"]), ["2", "9", "10", "100"]);
        assert_eq!(asc(&["pear", "10", "Apple", "9", "-1.5"]), ["-1.5", "9", "10", "Apple", "pear"]);
        // Without a type a word is a word: a name is not a number.
        assert_eq!(asc(&["Nan", "Ian", "Inf", "Zoe", "3"]), ["3", "Ian", "Inf", "Nan", "Zoe"]);
    }

    /// False before true in every spelling the engines print, and MySQL's `TINYINT(1)`, which is
    /// classed a boolean and holds any small integer, by value.
    #[test]
    fn booleans_sort_false_first_and_small_integers_by_value() {
        let asc = |cells: &[&'static str]| sorted(TypeClass::Bool, &values(cells), false, Nulls::Last);
        assert_eq!(asc(&["t", "f", "t"]), ["f", "t", "t"]);
        assert_eq!(asc(&["true", "false", "true"]), ["false", "true", "true"]);
        assert_eq!(asc(&["1", "0", "1", "0"]), ["0", "0", "1", "1"]);
        assert_eq!(asc(&["2", "10", "-5", "-1", "0"]), ["-5", "-1", "0", "2", "10"]);
        let cells = [Some("1"), None, Some("0")];
        assert_eq!(sorted(TypeClass::Bool, &cells, true, Nulls::First), ["1", "0", "NULL"]);
    }

    /// Past the bound of the exponent the order is by a rule (the digits), not by value; it is
    /// still one order: every pair agrees both ways round, and the whole list sorts the same
    /// from any start.
    #[test]
    fn numbers_past_the_exponent_bound_keep_a_total_order() {
        let cmp = |a: &str, b: &str| compare(TypeClass::Number, Some(a), Some(b), Nulls::Last);
        // By the rule: at the bound the digits decide.
        assert_eq!(cmp("9e9223372036854775806", "1e9223372036854775807"), Ordering::Greater, "the smaller value, the larger digits");
        assert_eq!(cmp("1e9223372036854775807", "10e9223372036854775807"), Ordering::Equal);
        assert_eq!(cmp("1e9223372036854775807", "1e99999999999999999999999999"), Ordering::Equal);
        assert_eq!(cmp("0.01e-9223372036854775808", "0.001e-9223372036854775808"), Ordering::Equal);
        assert_eq!(cmp("1e-9223372036854775808", "0.1e-9223372036854775808"), Ordering::Greater, "the digits' place still tells these two apart");
        // Below the bound it is by value.
        assert_eq!(cmp("9e9223372036854775805", "1e9223372036854775806"), Ordering::Less);
        assert_eq!(cmp("1e9223372036854775805", "10e9223372036854775804"), Ordering::Equal);
        let cells = [
            "9e9223372036854775806",
            "1e9223372036854775807",
            "10e9223372036854775807",
            "2e9223372036854775807",
            "0.5e9223372036854775807",
            "-1e9223372036854775807",
            "-9e9223372036854775806",
            "1e-9223372036854775808",
            "5e-9223372036854775809",
            "0.01e-9223372036854775808",
            "0.002e-9223372036854775808",
            "0e9223372036854775807",
            "1e99999999999999999999999999",
            "7",
            "Infinity",
        ];
        for a in cells {
            for b in cells {
                assert_eq!(cmp(a, b), cmp(b, a).reverse(), "{a} {b}");
                for c in cells {
                    if cmp(a, b) != Ordering::Greater && cmp(b, c) != Ordering::Greater {
                        assert_ne!(cmp(a, c), Ordering::Greater, "{a} {b} {c}");
                    }
                }
            }
        }
        let forward = sorted(TypeClass::Number, &values(&cells), false, Nulls::Last);
        let mut turned = cells;
        turned.reverse();
        let backward = sorted(TypeClass::Number, &values(&turned), false, Nulls::Last);
        let classes = |list: &[&str]| list.iter().map(|c| list.iter().filter(|d| cmp(c, d) == Ordering::Greater).count()).collect::<Vec<_>>();
        assert_eq!(classes(&forward), classes(&backward));
        assert_eq!(forward.first(), Some(&"-9e9223372036854775806"), "by the rule again: the larger digits, further from zero");
        assert_eq!(forward.last(), Some(&"Infinity"));
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
    fn a_short_row_reads_as_null_and_a_missing_column_sorts_nothing() {
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
        // Also where rows hold more cells than the result has columns.
        let wide = ResultSet {
            columns: vec![Column::untyped("a")],
            rows: vec![vec![Some("x".into()), Some("2".into())], vec![Some("y".into()), Some("1".into())]],
            ..ResultSet::default()
        };
        assert_eq!(order(&wide, 1, false, Nulls::Last), [0, 1]);
        assert_eq!(order(&wide, 0, true, Nulls::Last), [1, 0]);
        assert_eq!(order(&ResultSet::default(), 0, false, Nulls::Last), Vec::<u32>::new());
    }

    fn arb_class() -> impl Strategy<Value = TypeClass> {
        (0..CLASSES.len()).prop_map(|i| CLASSES[i])
    }

    /// Cells that meet every branch: NULL, numbers in every spelling, the words, texts that differ
    /// in case only, any text, and texts and digits about as long as the two bounds.
    fn arb_cell() -> impl Strategy<Value = Option<String>> {
        prop_oneof![
            1 => Just(None),
            4 => "[+-]?[0-9]{0,4}(\\.[0-9]{0,4})?([eE][+-]?[0-9]{1,2})?".prop_map(Some),
            1 => "[+-]?0{0,3}[1-9]?0{0,3}(\\.0{0,3}[1-9]?0{0,3})?([eE][+-]?[0-9]{1,25})?".prop_map(Some),
            1 => prop_oneof![Just("NaN"), Just("nan"), Just("Infinity"), Just("-Infinity"), Just("inf"), Just("-inf"), Just("+Inf")].prop_map(|w| Some(w.to_owned())),
            2 => "[aAbBßẞéÉİi ]{0,4}".prop_map(Some),
            2 => ".{0,12}".prop_map(Some),
            2 => arb_long_text().prop_map(Some),
            1 => arb_long_digits().prop_map(Some),
        ]
    }

    /// Texts as long as the folded length or a few characters to either side of it, alike but for
    /// case and for their ends: one letter repeated, some of it in the other case, then a few
    /// letters more.
    fn arb_long_text() -> impl Strategy<Value = String> {
        let letter = prop_oneof![Just(('x', 'X')), Just(('é', 'É')), Just(('ж', 'Ж')), Just(('i', 'İ')), Just(('σ', 'Σ')), Just(('İ', 'i'))];
        (letter, FOLDED_CHARS - 6..FOLDED_CHARS + 6, proptest::collection::vec(0..FOLDED_CHARS + 6, 0..4), "[aAbBxX]{0,3}").prop_map(
            |((lower, upper), length, capitals, end)| {
                let mut text: String = (0..length).map(|at| if capitals.contains(&at) { upper } else { lower }).collect();
                text.push_str(&end);
                text
            },
        )
    }

    /// Digits as long as the longest number read or a few bytes to either side of it, with a
    /// sign, a point or an exponent at their end.
    fn arb_long_digits() -> impl Strategy<Value = String> {
        (
            prop_oneof![Just(""), Just("-")],
            prop_oneof![Just("7"), Just("0")],
            NUMBER_BYTES - 6..NUMBER_BYTES + 6,
            "[0-9]{0,2}(\\.[0-9]{0,2})?(e[0-9])?",
        )
            .prop_map(|(sign, digit, length, end)| format!("{sign}{}{end}", digit.repeat(length)))
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
            // A column the result does not have sorts nothing, whatever the rows hold there.
            let Some(class) = rs.columns.get(column).map(|c| c.class) else {
                prop_assert_eq!(got, (0..rs.rows.len() as u32).collect::<Vec<_>>());
                return Ok(());
            };
            // In order, and equal neighbours in the order received.
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

        /// At the folded length too, the order of two texts is that of their keys (the first 256
        /// characters, each as its lowercase), then of their bytes.
        #[test]
        fn long_texts_order_by_their_folded_start_then_their_bytes(a in arb_long_text(), b in arb_long_text()) {
            let key = |text: &str| text.chars().take(FOLDED_CHARS).flat_map(char::to_lowercase).collect::<String>();
            let want = key(&a).chars().cmp(key(&b).chars()).then_with(|| a.cmp(&b));
            prop_assert_eq!(text_cmp(&a, &b), want);
            prop_assert_eq!(text_cmp(&b, &a), want.reverse());
            prop_assert_eq!(compare(TypeClass::LongText, Some(&a), Some(&b), Nulls::Last), want);
        }

        #[test]
        fn numbers_agree_with_scaled_integers(a in -100_000i64..100_000, b in -100_000i64..100_000, da in 0u32..4, db in 0u32..4) {
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

    /// The cost of [`order`] at the query cap (10,000 rows), which runs in the frame of the click:
    /// 16 ms for what results usually hold, and half a second for the worst found for it: values
    /// of 20 kB and more (most of a run's 256 MiB, and all of it) that are the same nearly to
    /// their end, and texts that are the same over the whole folded length but for case.
    #[test]
    #[ignore = "timing; run with --ignored --nocapture"]
    fn a_sort_of_ten_thousand_rows_within_a_frame() {
        let cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split(':').nth(1)).map(|m| m.trim().to_string()))
            .unwrap_or_else(|| "an unknown CPU".into());
        let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        println!("{cpu}, load average: {}", load.trim());
        let numbers: Vec<String> = (0..10_000u64).map(|i| (i.wrapping_mul(2_654_435_761) % 1_000_003).to_string()).collect();
        let texts: Vec<String> = (0..10_000u64).map(|i| format!("Customer {} of {}", i.wrapping_mul(2_654_435_761) % 1_000_003, i % 7)).collect();
        // 10,000 values of 20 kB that differ only at their end, and only in case before that.
        let long: Vec<String> = (0..10_000u64)
            .map(|i| format!("{}{}", if i % 2 == 0 { "x" } else { "X" }.repeat(20_000), i.wrapping_mul(2_654_435_761) % 10_007))
            .collect();
        let same: Vec<String> = (0..10_000u64).map(|_| "y".repeat(20_000)).collect();
        // Texts of 20 kB that differ only at their end: every comparison walks what they share.
        let late: Vec<String> = (0..10_000u64).map(|i| format!("{}{}", "z".repeat(20_000), i.wrapping_mul(2_654_435_761) % 10_007)).collect();
        // Numbers of 1,000 digits that differ only at their end. Digit strings of 25,000 (past
        // what is read as a number, so text) that differ at their end; zeros before a number, and
        // after its point.
        let numbers_1000: Vec<String> = (0..10_000u64).map(|i| format!("{}{}", "7".repeat(1_000), i.wrapping_mul(2_654_435_761) % 10_007)).collect();
        let digits: Vec<String> = (0..10_000u64).map(|i| format!("{}{}", "7".repeat(25_000), i.wrapping_mul(2_654_435_761) % 10_007)).collect();
        let zeros: Vec<String> = (0..10_000u64).map(|i| format!("{}{}", "0".repeat(25_000), i.wrapping_mul(2_654_435_761) % 10_007)).collect();
        let small: Vec<String> = (0..10_000u64).map(|i| format!("0.{}{}", "0".repeat(25_000), i.wrapping_mul(2_654_435_761) % 10_007)).collect();
        // Texts that are the same over the whole folded length but for case, each letter in a
        // case a generator's coin gives it: every pair differs in case only, at many places.
        let mut coin = 0x2545_F491_4F6C_DD1D_u64;
        let mut cased = |upper: char, lower: char| -> Vec<String> {
            (0..10_000u64)
                .map(|i| {
                    let letters: String = (0..FOLDED_CHARS)
                        .map(|_| {
                            coin = coin.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                            if (coin >> 33).is_multiple_of(2) { upper } else { lower }
                        })
                        .collect();
                    format!("{letters}{i}")
                })
                .collect()
        };
        let (cased_ascii, cased_cyrillic) = (cased('X', 'x'), cased('Ж', 'ж'));
        // Texts of 256 letters that differ in case at their first letter and by more right after:
        // each gets a key, where its first letters would have settled every comparison.
        let first_cased: Vec<String> = (0..10_000u64)
            .map(|i| format!("{}{:05}{}", if i % 2 == 0 { "ж" } else { "Ж" }, i.wrapping_mul(2_654_435_761) % 10_007, "ж".repeat(250)))
            .collect();
        // Values that fill a run's 256 MiB and differ at their end, their first 256 characters of
        // four bytes each: the most one comparison reads.
        let full: Vec<String> = (0..10_000u64)
            .map(|i| format!("{}{}{:05}", "\u{10428}".repeat(256), "z".repeat(25_811), i.wrapping_mul(2_654_435_761) % 10_007))
            .collect();
        for (what, class, cells, budget) in [
            ("numbers", TypeClass::Number, &numbers, 16.0),
            ("numbers without a type", TypeClass::Other, &numbers, 16.0),
            ("short texts", TypeClass::Text, &texts, 16.0),
            ("the same 20 kB text", TypeClass::LongText, &same, 16.0),
            ("20 kB texts that differ in case along their whole length", TypeClass::LongText, &long, 500.0),
            ("20 kB texts that differ at their end", TypeClass::LongText, &late, 500.0),
            ("numbers of 1,000 digits that differ at their end", TypeClass::Number, &numbers_1000, 100.0),
            ("digit strings of 25,000 that differ at their end", TypeClass::Number, &digits, 500.0),
            ("the same without a type", TypeClass::Other, &digits, 500.0),
            ("25,000 zeros before a number", TypeClass::Number, &zeros, 500.0),
            ("25,000 zeros after the point", TypeClass::Other, &small, 500.0),
            ("256 letters, each in a case of its own", TypeClass::Text, &cased_ascii, 500.0),
            ("256 Cyrillic letters, each in a case of its own", TypeClass::Text, &cased_cyrillic, 500.0),
            ("the same without a type", TypeClass::Other, &cased_cyrillic, 500.0),
            ("256 Cyrillic letters that differ in case at the first, then by more", TypeClass::Text, &first_cased, 100.0),
            ("26.8 kB texts that fill 256 MiB and differ at their end", TypeClass::LongText, &full, 500.0),
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
