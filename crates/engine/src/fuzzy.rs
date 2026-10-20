//! Fuzzy scoring for the quick search (Ctrl+P) and [`find`], which ranks a catalog's tables.
//!
//! A query matches when its characters occur in the candidate in order (case-insensitively).
//! The score rewards a match at the start or after `.` or `_`, consecutive characters and an
//! exact match, and penalises skipped characters. Work is linear in the candidate's length
//! (a candidate is cut at [`MAX_CANDIDATE`] characters), so 1,000 tables rank in well under a
//! millisecond.

use dbee_core::{Catalog, RelKind, TableRef};

/// Characters of a candidate looked at; a table name is at most 63 bytes, so this only bounds
/// hostile input.
const MAX_CANDIDATE: usize = 256;

const MATCH: i64 = 16;
const BOUNDARY: i64 = 24;
const CONSECUTIVE: i64 = 26;
/// Extra for a match at the very start.
const START: i64 = 10;
const GAP: i64 = 1;
const EXACT: i64 = 500;

fn lower(c: char) -> impl Iterator<Item = char> {
    c.to_lowercase()
}

/// The score of `candidate` for `query`, higher is better; `None` when the query is not a
/// subsequence. An empty query matches everything with score 0.
pub fn score(query: &str, candidate: &str) -> Option<u32> {
    let q: Vec<char> = query.chars().flat_map(lower).collect();
    if q.is_empty() {
        return Some(0);
    }
    let mut next = 0;
    let mut total: i64 = 0;
    let mut prev: Option<char> = None;
    let mut last_match: Option<usize> = None;
    let mut len = 0;
    for (i, ch) in candidate.chars().take(MAX_CANDIDATE).enumerate() {
        len = i + 1;
        let mut lowered = lower(ch);
        // Characters that lowercase to several are compared by their first.
        let lc = lowered.next().unwrap_or(ch);
        if next < q.len() && lc == q[next] {
            total += MATCH;
            if prev.is_none() {
                total += BOUNDARY + START;
            } else if matches!(prev, Some('.' | '_')) {
                total += BOUNDARY;
            }
            if last_match == Some(i.wrapping_sub(1)) && i > 0 {
                total += CONSECUTIVE;
            }
            last_match = Some(i);
            next += 1;
        } else if next > 0 && next < q.len() {
            total -= GAP;
        }
        prev = Some(ch);
    }
    if next < q.len() {
        return None;
    }
    // Skipped characters before the first match cost a little, so shorter names win ties.
    total -= i64::try_from(len.saturating_sub(q.len())).unwrap_or(0) / 4;
    if len == q.len() && candidate.chars().take(MAX_CANDIDATE).flat_map(lower).eq(q.iter().copied()) {
        total += EXACT;
    }
    Some(u32::try_from(total.max(1)).unwrap_or(u32::MAX))
}

/// One ranked table.
#[derive(Clone, Copy, Debug)]
pub struct Hit<'a> {
    pub table: &'a TableRef,
    pub kind: RelKind,
    pub score: u32,
    system: bool,
}

/// The best `limit` tables for `query`: by score, then shorter name, then user schemas before
/// system schemas, then catalog order. The query is scored against the table name and against
/// `schema.name`, whichever is higher.
pub fn find<'a>(catalog: &'a Catalog, query: &str, limit: usize) -> Vec<Hit<'a>> {
    if limit == 0 {
        return Vec::new();
    }
    let query = query.trim();
    let mut hits = Vec::new();
    let mut qualified = String::new();
    for schema in &catalog.schemas {
        for t in &schema.tables {
            let by_name = score(query, &t.table.name);
            let by_full = if query.contains('.') {
                qualified.clear();
                qualified.push_str(&t.table.schema);
                qualified.push('.');
                qualified.push_str(&t.table.name);
                score(query, &qualified)
            } else {
                None
            };
            if let Some(score) = by_name.max(by_full) {
                hits.push(Hit { table: &t.table, kind: t.kind, score, system: schema.system });
            }
        }
    }
    // Stable, so catalog order breaks the last tie.
    hits.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| a.table.name.chars().count().cmp(&b.table.name.chars().count())).then_with(|| a.system.cmp(&b.system))
    });
    hits.truncate(limit);
    hits
}
