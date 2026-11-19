//! Command-sequence fuzz: random registered commands with random, often hostile, params.
//!
//! `DBEE_FUZZ_SEED` picks the sequence (default: from the clock, printed so a failure can be
//! replayed); `DBEE_FUZZ_STEPS` is its length (default 300). After every step the session must
//! hold its invariants: no command or job result is `Internal` (a caught panic), `inspect()`
//! serializes, the active tab is an open tab (`None` exactly when no tab is open), every tab is
//! on an open connection (on none only while none is open), and no journal entry keeps a secret
//! param at any depth.
//!
//! Every engine task adds its commands' param shapes to [`PARAM_HINTS`]; the test
//! `every_command_has_param_hints` fails when one is missing.
//!
//! `undoing_every_edit_then_redoing_restores_them` runs random edit commands on a table tab of the
//! test server (it needs `cargo xtask testdb up`): undoing until `edit.undo` is disabled must leave
//! no staged edit, and redoing as often must bring back as many as before (and redoing further
//! goes on only where redo was possible before the undos).

mod common;

use std::sync::Arc;
use std::time::Duration;

use dbee_config::MemoryStore;
use dbee_engine::cmd::is_secret_param;
use dbee_engine::{EngineError, Json, Outcome, Session, SessionOptions, Tab, Which};
use dbee_ssh::SshEnv;
use serde_json::json;

/// How to build a plausible value for one param.
#[derive(Clone, Copy)]
enum Hint {
    /// Any text, SQL-looking or not.
    Text,
    /// A whole number, mostly small, sometimes at an edge.
    Count,
    Bool,
    /// The id of an open tab, sometimes one that is not open.
    Tab,
    /// One of these strings.
    OneOf(&'static [&'static str]),
    /// Built by the function, for shapes the variants above do not cover.
    Custom(fn(&mut Rng, &Session) -> Json),
}

/// Param shapes per command id: (key, hint) pairs. The fuzz builds well-formed params from them
/// most of the time, so commands get past their params checks and change the state.
const PARAM_HINTS: &[(&str, &[(&str, Hint)])] = &[
    ("tab.newQuery", &[("text", Hint::Text), ("schema", Hint::Text), ("connection", CONNECTION)]),
    ("tab.activate", &[("tab", Hint::Tab)]),
    ("tab.list", &[]),
    ("query.setText", &[("text", Hint::Text)]),
    ("query.run", &[("current", Hint::Bool), ("cursor", Hint::Count)]),
    ("query.cancel", &[]),
    ("query.showResult", &[("index", Hint::Count)]),
    ("query.setDatabase", &[("tab", Hint::Tab), ("database", Hint::Text)]),
    ("query.commit", &[("tab", Hint::Tab)]),
    ("query.rollback", &[("tab", Hint::Tab)]),
    ("tab.close", &[("tab", Hint::Tab), ("force", Hint::Bool)]),
    ("connection.list", &[]),
    ("connection.save", &[("config", CONFIG), ("password", Hint::Text), ("sshPassword", Hint::Text), ("sshPassphrase", Hint::Text)]),
    ("connection.delete", &[("id", SAVED_ID)]),
    ("connection.duplicate", &[("id", SAVED_ID)]),
    ("connection.parseUrl", &[("url", Hint::Custom(url))]),
    ("connection.sshKey", &[("config", CONFIG)]),
    ("connection.test", CONNECT),
    ("connection.listDatabases", LIST_DATABASES),
    ("connection.open", OPEN),
    ("connection.openFile", &[("path", FILE_PATH), ("password", Hint::Text)]),
    ("connection.saveOpen", &[("connection", CONNECTION)]),
    ("connection.reconnect", RECONNECT),
    ("connection.cancel", &[]),
    ("schema.refresh", &[("connection", CONNECTION)]),
    ("schema.inspect", &[("connection", CONNECTION), ("schema", Hint::Text), ("table", Hint::Text)]),
    ("quickSearch.find", &[("query", Hint::Text), ("limit", Hint::Count), ("connection", CONNECTION), ("grouped", Hint::Bool)]),
    ("connection.close", &[("connection", CONNECTION), ("force", Hint::Bool)]),
    ("table.open", &[("schema", SCHEMA), ("table", TABLE), ("filters", FILTERS), ("connection", CONNECTION)]),
    ("table.setFilters", &[("filters", FILTERS), ("raw", Hint::Text), ("rawMode", Hint::Bool), ("apply", Hint::Bool)]),
    ("table.page", &[("to", Hint::Custom(page_to))]),
    ("table.sort", &[("column", COLUMN), ("dir", Hint::OneOf(&["asc", "desc", "none", "up", ""]))]),
    ("table.count", &[]),
    ("table.cancel", &[]),
    ("table.refresh", &[]),
    ("table.read", &[("offset", Hint::Count), ("limit", Hint::Count)]),
    ("table.followForeignKey", &[("row", Hint::Count), ("column", COLUMN)]),
    ("table.structure", &[("schema", SCHEMA), ("table", TABLE), ("connection", CONNECTION)]),
    ("table.view", &[("view", Hint::OneOf(&["structure", "data"]))]),
    ("cell.edit", &[("row", GRID_ROW), ("column", EDIT_COLUMN), ("value", VALUE)]),
    ("cell.setNull", &[("row", GRID_ROW), ("column", EDIT_COLUMN)]),
    ("cell.clear", &[("row", GRID_ROW), ("column", EDIT_COLUMN)]),
    ("row.add", &[]),
    ("row.delete", &[("rows", Hint::Custom(grid_rows))]),
    ("edit.apply", &[("force", Hint::Bool)]),
    ("edit.reset", &[]),
    ("edit.undo", &[]),
    ("edit.redo", &[]),
    ("selection.set", &[("anchor", CELL), ("focus", CELL)]),
    ("selection.rows", &[("first", GRID_ROW), ("last", GRID_ROW)]),
    ("selection.all", &[]),
    ("copy.selection", &[("format", Hint::OneOf(&["tsv", "tsvHeader", "csv", "json", "markdown", "xml", ""]))]),
    ("value.show", &[]),
    ("value.copy", &[]),
];

/// Table commands: names from the seed data, odd ones and ones that do not exist.
const SCHEMA: Hint = Hint::OneOf(&["shop", "odd", "scratch", "", "🐙"]);
const TABLE: Hint = Hint::OneOf(&["orders", "customers", "big_rows", "order_totals", "Odd \"Name\" 🐙", "nope"]);
const COLUMN: Hint = Hint::Custom(column);
const FILTERS: Hint = Hint::Custom(filters);

fn column(r: &mut Rng, _: &Session) -> Json {
    if r.chance(30) { json!(r.below(12)) } else { json!(r.pick(&["id", "customer_id", "total", "Total $", "nope", ""])) }
}

fn filters(r: &mut Rng, s: &Session) -> Json {
    let ops = ["eq", "ne", "like", "notLike", "ilike", "notIlike", "lt", "le", "gt", "ge", "in", "isNull", "isNotNull", "between"];
    let rows: Vec<Json> = (0..r.below(4))
        .map(|_| json!({ "column": column(r, s), "op": r.pick(&ops), "value": if r.chance(20) { hostile(r) } else { text(r) } }))
        .collect();
    Json::Array(rows)
}

/// Edit commands: rows of the active table tab's grid (loaded and new), now and then one past it;
/// the columns of the edit test table; values that fit some columns and not others.
const GRID_ROW: Hint = Hint::Custom(grid_row);
const EDIT_COLUMN: Hint = Hint::Custom(edit_column);
const VALUE: Hint = Hint::Custom(value);

fn grid_size(s: &Session) -> usize {
    match s.state().active_tab() {
        Some(Tab::Table(t)) => t.data.as_ref().map_or(0, |d| t.edits.grid_rows(d)),
        _ => 0,
    }
}

fn grid_row(r: &mut Rng, s: &Session) -> Json {
    json!(r.below(grid_size(s) + 2))
}

/// Selection: `[row, column]` in or just outside the active table grid.
const CELL: Hint = Hint::Custom(cell);

fn cell(r: &mut Rng, s: &Session) -> Json {
    if r.chance(5) { json!([r.below(3), r.below(3)]) } else { json!([r.below(grid_size(s) + 2), r.below(8)]) }
}

fn grid_rows(r: &mut Rng, s: &Session) -> Json {
    Json::Array((0..r.below(4)).map(|_| grid_row(r, s)).collect())
}

fn edit_column(r: &mut Rng, _: &Session) -> Json {
    if r.chance(30) { json!(r.below(8)) } else { json!(r.pick(&["id", "name", "total", "shipped_at", "flag", "doc", "nope"])) }
}

fn value(r: &mut Rng, _: &Session) -> Json {
    if r.chance(15) {
        return Json::Null;
    }
    json!(r.pick(&["", "1", "42", "-7.5", "x", "2025-01-01", "not a date", "true", "{}", "[1, 2]", "it's \"q\" 🐙", "Alice"]))
}

fn page_to(r: &mut Rng, _: &Session) -> Json {
    if r.chance(40) { json!(r.pick(&[0u64, 1, 2, 10_000, u64::MAX])) } else { json!(r.pick(&["first", "prev", "next", "last", "middle"])) }
}

/// Connection configs. Every host is 127.0.0.1 and every port 1, which refuses at once, for the
/// database and for SSH: the fuzz never reaches a server, and never names a host that would need
/// a DNS lookup.
const CONFIG: Hint = Hint::Custom(config);
const SAVED_ID: Hint = Hint::Custom(saved_id);
/// For commands that take `id` or `config`: mostly absent, so `config` alone gets through too.
const MAYBE_ID: Hint = Hint::Custom(maybe_id);
const RECONNECT: &[(&str, Hint)] = &[
    ("connection", CONNECTION),
    ("password", Hint::Text),
    ("sshPassword", Hint::Text),
    ("sshPassphrase", Hint::Text),
    ("trustHostKey", FINGERPRINT),
];
const CONNECT: &[(&str, Hint)] = &[
    ("id", MAYBE_ID),
    ("config", CONFIG),
    ("password", Hint::Text),
    ("sshPassword", Hint::Text),
    ("sshPassphrase", Hint::Text),
    ("trustHostKey", FINGERPRINT),
];
const LIST_DATABASES: &[(&str, Hint)] =
    &[("config", CONFIG), ("password", Hint::Text), ("sshPassword", Hint::Text), ("sshPassphrase", Hint::Text), ("trustHostKey", FINGERPRINT)];
const OPEN: &[(&str, Hint)] = &[
    ("id", MAYBE_ID),
    ("config", CONFIG),
    ("password", Hint::Text),
    ("sshPassword", Hint::Text),
    ("sshPassphrase", Hint::Text),
    ("trustHostKey", FINGERPRINT),
    ("savePassword", Hint::Bool),
];
/// Database file paths that are never there (or a name without a type dbee opens), so the fuzz
/// opens no file.
const FILE_PATH: Hint = Hint::OneOf(&[
    "/nonexistent/dbee-fuzz/a.sqlite",
    "/nonexistent/dbee-fuzz/B.DB",
    "/nonexistent/dbee-fuzz/notes.txt",
    "/nonexistent/dbee-fuzz/noext",
    "",
]);
const FINGERPRINT: Hint = Hint::OneOf(&["SHA256:AAAA", "SHA256:", "", "md5:00:11"]);

fn config(r: &mut Rng, s: &Session) -> Json {
    let mut c = json!({
        "host": "127.0.0.1",
        "port": if r.chance(5) { 0 } else { 1 },
        "name": text(r),
        "user": r.pick(&["", "postgres", "it's", "🐙"]),
        "database": r.pick(&["", "dbee", "a\"b", "🐙"]),
        "sslMode": if r.chance(5) { "bogus" } else { *r.pick(&["disable", "prefer", "require", "verify-ca", "verify-full"]) },
        "color": if r.chance(5) { "plaid" } else { *r.pick(&["none", "red", "blue"]) },
        "readOnly": r.chance(50),
        "saveSecrets": r.chance(70),
    });
    if r.chance(10) {
        c["caFile"] = json!("/nonexistent/dbee-fuzz/ca.crt");
    }
    if r.chance(15) {
        c["id"] = saved_id(r, s);
    }
    if r.chance(40) {
        let auth = if r.chance(5) {
            json!({ "method": "telepathy" })
        } else {
            r.pick(&[
                json!({ "method": "key", "path": null }),
                json!({ "method": "key", "path": "/nonexistent/dbee-fuzz/id_ed25519" }),
                json!({ "method": "agent" }),
                json!({ "method": "password" }),
            ])
            .clone()
        };
        c["ssh"] = json!({ "host": "127.0.0.1", "port": 1, "user": if r.chance(10) { "" } else { "dbee" }, "auth": auth });
    }
    c
}

/// A saved connection's id most of the time, else one that is not saved.
fn saved_id(r: &mut Rng, s: &Session) -> Json {
    let saved = &s.state().store.saved;
    if !saved.is_empty() && r.chance(80) { json!(r.pick(saved).id) } else { json!(r.pick(&["", "nope", "🐙", "../x"])) }
}

/// An open connection's id most of the time, else one that is not open.
const CONNECTION: Hint = Hint::Custom(connection_id);

fn connection_id(r: &mut Rng, s: &Session) -> Json {
    let open = &s.state().connections;
    if !open.is_empty() && r.chance(85) { json!(r.pick(open).id.0) } else { json!(*r.pick(&[0u64, s.state().next_connection, u64::MAX])) }
}

fn maybe_id(r: &mut Rng, s: &Session) -> Json {
    if r.chance(60) { Json::Null } else { saved_id(r, s) }
}

fn url(r: &mut Rng, _: &Session) -> Json {
    json!(r.pick(&[
        "postgres://u:p%40ss@127.0.0.1:1/db?sslmode=require",
        "postgresql://127.0.0.1",
        "postgres://",
        "postgres://u@/db",
        "postgres://127.0.0.1:99999/x",
        "postgres://%zz@127.0.0.1/x",
        "postgres://[127.0.0.1/x",
        "postgres://127.0.0.1/x?sslmode=bogus",
        "mysql://127.0.0.1/x",
        "",
    ]))
}

/// xorshift64.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed })
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// 0..n (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// True with probability `percent`/100.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const TEXTS: &[&str] = &[
    "",
    "   \n\t",
    "select 1",
    "select 'a;b'; select \"x\" from t; -- done",
    "it's \"quoted\"; \\ backslash",
    "nul \u{0} inside",
    "🐙 emoji, ünïcödé and a combining e\u{301}",
    "select * from shop.orders where id = '1''; drop table shop.orders; --'",
];

fn nested(depth: usize) -> Json {
    let mut v = json!({"leaf": "🐙"});
    for i in 0..depth {
        v = if i % 2 == 0 { json!({ "n": v }) } else { json!([v]) };
    }
    v
}

fn text(r: &mut Rng) -> Json {
    if r.chance(5) { Json::String("x'\";🐙\u{0}".repeat(1_000)) } else { json!(r.pick(TEXTS)) }
}

/// Any value at all, with the inputs that break code: edges of numbers, quotes, NUL, emoji,
/// 10 kB strings, nulls, arrays and deep nesting.
fn hostile(r: &mut Rng) -> Json {
    match r.below(16) {
        0 => json!(r.next() as i64 % 1_000),
        1 => json!(-1),
        2 => json!(u64::MAX),
        3 => json!(i64::MIN),
        4 => json!(1.5),
        5 => json!(1e308),
        6 => json!("it's"),
        7 => json!("say \"hi\"; drop table x; --"),
        8 => json!("nul\u{0}byte"),
        9 => json!("🐙🐙🐙"),
        10 => Json::String("x".repeat(10_000)),
        11 => Json::Null,
        12 => json!([1, "two", null, [3], {"four": 4, "inner": {"sshPassword": "pw"}}]),
        13 => nested(64),
        14 => json!(r.chance(50)),
        _ => r.pick(&[json!(""), json!({}), json!([]), json!(0)]).clone(),
    }
}

fn well_formed(r: &mut Rng, hint: Hint, s: &Session) -> Json {
    match hint {
        Hint::Text => text(r),
        Hint::Count => match r.below(4) {
            0 => json!(*r.pick(&[0u64, 1, 99, 100, 101, 10_000])),
            _ => json!(r.below(200)),
        },
        Hint::Bool => json!(r.chance(50)),
        Hint::Tab => {
            let tabs = &s.state().tabs;
            if !tabs.is_empty() && r.chance(85) { json!(r.pick(tabs).id().0) } else { json!(*r.pick(&[0u64, s.state().next_tab, u64::MAX])) }
        }
        Hint::OneOf(options) => json!(r.pick(options)),
        Hint::Custom(f) => f(r, s),
    }
}

const EXTRA_KEYS: &[&str] = &["password", "sshPassphrase", "tab", "text", "force", "", "🐙", "ms"];

fn params(r: &mut Rng, id: &str, s: &Session) -> Json {
    if r.chance(4) {
        // Not an object at all.
        return r.pick(&[Json::Null, json!([]), json!("params"), json!(7), json!(true)]).clone();
    }
    let mut map = serde_json::Map::new();
    let hints = PARAM_HINTS.iter().find(|(c, _)| *c == id).map_or(&[][..], |(_, h)| *h);
    for (key, hint) in hints {
        if r.chance(15) {
            continue;
        }
        let v = if r.chance(70) { well_formed(r, *hint, s) } else { hostile(r) };
        map.insert((*key).to_string(), v);
    }
    if r.chance(20) {
        for _ in 0..=r.below(2) {
            map.insert((*r.pick(EXTRA_KEYS)).to_string(), hostile(r));
        }
    }
    Json::Object(map)
}

/// More open tabs than this, and the step closes the active one: the count stays realistic and
/// long runs stay fast.
const MAX_TABS: usize = 40;

fn session() -> Session {
    let options = SessionOptions {
        config_dir: None,
        secrets: Some(Arc::new(MemoryStore::default())),
        ssh: SshEnv::default(),
        notify: None,
        allowed_targets: None,
        allowed_dirs: None,
        drivers: dbee_drivers::all(),
    };
    Session::new(options).unwrap()
}

fn short(v: &Json) -> String {
    let t = v.to_string();
    if t.len() > 300 { format!("{}… ({} bytes)", t.chars().take(300).collect::<String>(), t.len()) } else { t }
}

fn check(s: &Session, at: &str) {
    if let Err(e) = serde_json::to_string(&s.inspect()) {
        panic!("{at}: inspect() does not serialize: {e}");
    }
    let st = s.state();
    match st.active {
        Some(a) => assert!(st.tabs.iter().any(|t| t.id() == a), "{at}: active tab {a:?} is not open"),
        None => assert!(st.tabs.is_empty(), "{at}: tabs are open but none is active"),
    }
    let mut ids: Vec<u64> = st.tabs.iter().map(|t| t.id().0).collect();
    assert!(ids.iter().all(|&id| id < st.next_tab), "{at}: a tab id is not below next_tab {}", st.next_tab);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), st.tabs.len(), "{at}: duplicate tab ids");
    for t in &st.tabs {
        match t.connection() {
            Some(c) => assert!(st.connection(Which::Id(c)).is_some(), "{at}: tab {} is on connection {}, which is not open", t.id().0, c.0),
            None => assert!(st.connections.is_empty(), "{at}: tab {} is on no connection while one is open", t.id().0),
        }
    }
}

/// No journal entry from `from` on holds a secret param, at any depth.
fn no_secrets_in_journal(s: &Session, from: usize, at: &str) {
    for (id, params) in s.journal().iter().skip(from) {
        let mut stack = vec![params];
        while let Some(v) = stack.pop() {
            match v {
                Json::Object(map) => {
                    if let Some(k) = map.keys().find(|k| is_secret_param(k)) {
                        panic!("{at}: the journal entry for `{id}` keeps secret param `{k}`");
                    }
                    stack.extend(map.values());
                }
                Json::Array(items) => stack.extend(items),
                _ => {}
            }
        }
    }
}

fn no_internal(r: &Result<Json, EngineError>, at: &str) {
    if let Err(e @ EngineError::Internal { .. }) = r {
        panic!("{at}: {e}");
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

#[test]
fn random_command_sequences_keep_the_invariants() {
    let seed = env_u64("DBEE_FUZZ_SEED")
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64));
    let steps = env_u64("DBEE_FUZZ_STEPS").unwrap_or(300);
    eprintln!("command_sequence_fuzz: DBEE_FUZZ_SEED={seed} DBEE_FUZZ_STEPS={steps}");
    let mut r = Rng::new(seed);
    let mut s = session();
    let ids: Vec<&'static str> = s.commands().iter().map(|c| c.id).collect();
    assert!(!ids.is_empty());
    for step in 0..steps {
        let (id, p) = if s.state().tabs.len() > MAX_TABS {
            ("tab.close", json!({ "force": true }))
        } else {
            let id = *r.pick(&ids);
            (id, params(&mut r, id, &s))
        };
        let at = format!("seed {seed} step {step} `{id}` {}", short(&p));
        let journal_before = s.journal().len();
        let result = s.execute(id, p).map(|o| match o {
            Outcome::Done(v) => v,
            Outcome::Started(j) => json!({ "started": j.0 }),
        });
        no_internal(&result, &at);
        check(&s, &at);
        // A step adds an entry or replaces the last one; when the cap dropped the oldest, check all.
        let from = if s.journal().len() >= journal_before { journal_before.saturating_sub(1) } else { 0 };
        no_secrets_in_journal(&s, from, &at);
        for (job, r) in s.pump() {
            no_internal(&r, &format!("{at}, job {}", job.0));
        }
        check(&s, &format!("{at}, after pump"));
    }
    assert!(s.wait_idle(Duration::from_secs(30)), "seed {seed}: jobs still running at the end");
    for (job, r) in s.pump() {
        no_internal(&r, &format!("seed {seed}, job {} at the end", job.0));
    }
    check(&s, &format!("seed {seed}, end"));
    no_secrets_in_journal(&s, 0, &format!("seed {seed}, end"));
}

#[test]
fn every_command_has_param_hints() {
    let s = session();
    for c in s.commands() {
        assert!(PARAM_HINTS.iter().any(|(id, _)| *id == c.id), "add `{}` to PARAM_HINTS", c.id);
    }
    for (id, _) in PARAM_HINTS {
        assert!(s.commands().iter().any(|c| c.id == *id), "PARAM_HINTS names `{id}`, which is not a command");
    }
}

/// The staged edits of the active table tab.
fn staged(s: &Session) -> usize {
    match s.state().active_tab() {
        Some(Tab::Table(t)) => t.edits.count(),
        _ => 0,
    }
}

fn can_redo(s: &Session) -> bool {
    match s.state().active_tab() {
        Some(Tab::Table(t)) => t.edits.can_redo(),
        _ => false,
    }
}

/// Runs `id` until it is disabled; returns how often it ran.
fn until_disabled(s: &mut Session, id: &str, at: &str) -> usize {
    for n in 0..100_000 {
        match s.execute(id, json!({})) {
            Ok(_) => {}
            Err(EngineError::Disabled { .. }) => return n,
            Err(e) => panic!("{at}: {id}: {e}"),
        }
    }
    panic!("{at}: {id} never became disabled");
}

/// Drops the scratch table when the test ends, also when it fails.
struct DropTable(String);

impl Drop for DropTable {
    fn drop(&mut self) {
        let _ = dbee_testkit::psql(common::pg14(), &format!("DROP TABLE IF EXISTS scratch.{}", self.0));
    }
}

#[test]
fn undoing_every_edit_then_redoing_restores_them() {
    let _db = dbee_testkit::require_testdb!();
    let seed = env_u64("DBEE_FUZZ_SEED")
        .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64));
    eprintln!("undoing_every_edit_then_redoing_restores_them: DBEE_FUZZ_SEED={seed}");
    let name = dbee_testkit::unique("t_fuzz_edit");
    let _drop = DropTable(name.clone());
    dbee_testkit::psql(
        common::pg14(),
        &format!(
            "CREATE TABLE scratch.{name} (id int PRIMARY KEY, name text, total numeric(12,2), shipped_at date, flag bool, doc jsonb);
             INSERT INTO scratch.{name} VALUES (1, 'Alice', 10.50, '2025-01-02', true, '{{}}'), (2, 'Bob', 20, NULL, false, NULL), (3, NULL, NULL, NULL, NULL, NULL);"
        ),
    )
    .unwrap();
    let mut s = common::connected();
    s.run_blocking("table.open", json!({ "schema": "scratch", "table": name }), common::T).unwrap();
    // Mostly cell edits; now and then an Apply, which writes to the scratch table.
    let commands = [
        "cell.edit",
        "cell.edit",
        "cell.edit",
        "cell.setNull",
        "cell.clear",
        "row.add",
        "row.delete",
        "edit.undo",
        "edit.undo",
        "edit.redo",
        "edit.reset",
        "edit.apply",
    ];
    let mut r = Rng::new(seed);
    let (mut ran, mut undos) = (0, 0);
    // Six rounds, and more (up to 20) while too few edits ran: an Apply or a Reset near a round's
    // end leaves little to undo, so some seeds need more rounds to test anything.
    for round in 0..20 {
        if round >= 6 && ran >= 30 && undos >= 6 {
            break;
        }
        for step in 0..30 {
            let id = *r.pick(&commands);
            let p = params(&mut r, id, &s);
            let at = format!("seed {seed} round {round} step {step} `{id}` {}", short(&p));
            match s.execute(id, p) {
                Ok(Outcome::Started(job)) => no_internal(&s.wait(job, common::T), &at),
                Ok(Outcome::Done(_)) => ran += 1,
                Err(e) => no_internal(&Err(e), &at),
            }
            check(&s, &at);
        }
        let at = format!("seed {seed} round {round}");
        let (before, could_redo) = (staged(&s), can_redo(&s));
        let undone = until_disabled(&mut s, "edit.undo", &at);
        undos += undone;
        assert_eq!(staged(&s), 0, "{at}: {undone} undos left staged edits");
        // Redoing every undo brings the edits back; past them is only what could be redone before.
        for i in 0..undone {
            if let Err(e) = s.execute("edit.redo", json!({})) {
                panic!("{at}: redo {i} of {undone}: {e}");
            }
        }
        assert_eq!((staged(&s), can_redo(&s)), (before, could_redo), "{at}: after redoing every undo");
        until_disabled(&mut s, "edit.redo", &at);
    }
    eprintln!("seed {seed}: {ran} edit commands ran, {undos} undos");
    assert!(ran >= 30 && undos >= 6, "seed {seed}: too few edits ran ({ran}, {undos} undos) to test anything");
}
