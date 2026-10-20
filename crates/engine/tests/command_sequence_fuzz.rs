//! Command-sequence fuzz: random registered commands with random, often hostile, params.
//!
//! `DBEE_FUZZ_SEED` picks the sequence (default: from the clock, printed so a failure can be
//! replayed); `DBEE_FUZZ_STEPS` is its length (default 300). After every step the session must
//! hold its invariants: no command or job result is `Internal` (a caught panic), `inspect()`
//! serializes, the active tab is an open tab (`None` exactly when no tab is open), and no
//! journal entry keeps a secret param at any depth.
//!
//! Every engine task adds its commands' param shapes to [`PARAM_HINTS`]; the test
//! `every_command_has_param_hints` fails when one is missing.

use std::sync::Arc;
use std::time::Duration;

use dbee_config::MemoryStore;
use dbee_engine::cmd::is_secret_param;
use dbee_engine::{EngineError, Json, Outcome, Session, SessionOptions};
use dbee_ssh::SshEnv;
use serde_json::json;

/// How to build a plausible value for one param.
#[derive(Clone, Copy)]
#[expect(dead_code, reason = "some variants are there for later tasks' hints")]
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
    ("tab.newQuery", &[("text", Hint::Text)]),
    ("tab.activate", &[("tab", Hint::Tab)]),
    ("tab.list", &[]),
    ("query.setText", &[("text", Hint::Text)]),
    ("tab.close", &[("tab", Hint::Tab), ("force", Hint::Bool)]),
    ("connection.list", &[]),
    ("connection.save", &[("config", CONFIG), ("password", Hint::Text), ("sshPassword", Hint::Text), ("sshPassphrase", Hint::Text)]),
    ("connection.delete", &[("id", SAVED_ID)]),
    ("connection.duplicate", &[("id", SAVED_ID)]),
    ("connection.parseUrl", &[("url", Hint::Custom(url))]),
    ("connection.sshKey", &[("config", CONFIG)]),
    ("connection.test", CONNECT),
    ("connection.open", OPEN),
    ("connection.reconnect", SECRETS),
    ("schema.refresh", &[]),
    ("connection.close", &[("force", Hint::Bool)]),
];

/// Connection configs. Every host is 127.0.0.1 and every port 1, which refuses at once, for the
/// database and for SSH: the fuzz never reaches a server, and never names a host that would need
/// a DNS lookup.
const CONFIG: Hint = Hint::Custom(config);
const SAVED_ID: Hint = Hint::Custom(saved_id);
/// For commands that take `id` or `config`: mostly absent, so `config` alone gets through too.
const MAYBE_ID: Hint = Hint::Custom(maybe_id);
const SECRETS: &[(&str, Hint)] =
    &[("password", Hint::Text), ("sshPassword", Hint::Text), ("sshPassphrase", Hint::Text), ("trustHostKey", FINGERPRINT)];
const CONNECT: &[(&str, Hint)] = &[
    ("id", MAYBE_ID),
    ("config", CONFIG),
    ("password", Hint::Text),
    ("sshPassword", Hint::Text),
    ("sshPassphrase", Hint::Text),
    ("trustHostKey", FINGERPRINT),
];
const OPEN: &[(&str, Hint)] = &[
    ("id", MAYBE_ID),
    ("config", CONFIG),
    ("password", Hint::Text),
    ("sshPassword", Hint::Text),
    ("sshPassphrase", Hint::Text),
    ("trustHostKey", FINGERPRINT),
    ("savePassword", Hint::Bool),
];
const FINGERPRINT: Hint = Hint::OneOf(&["SHA256:AAAA", "SHA256:", "", "md5:00:11"]);

fn config(r: &mut Rng, s: &Session) -> Json {
    let mut c = json!({
        "host": "127.0.0.1",
        "port": if r.chance(5) { 0 } else { 1 },
        "name": text(r),
        "user": r.pick(&["", "postgres", "it's", "🐝"]),
        "database": r.pick(&["", "dbee", "a\"b", "🐝"]),
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
    if !saved.is_empty() && r.chance(80) { json!(r.pick(saved).id) } else { json!(r.pick(&["", "nope", "🐝", "../x"])) }
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
    "🐝 emoji, ünïcödé and a combining e\u{301}",
    "select * from shop.orders where id = '1''; drop table shop.orders; --'",
];

fn nested(depth: usize) -> Json {
    let mut v = json!({"leaf": "🐝"});
    for i in 0..depth {
        v = if i % 2 == 0 { json!({ "n": v }) } else { json!([v]) };
    }
    v
}

fn text(r: &mut Rng) -> Json {
    if r.chance(5) { Json::String("x'\";🐝\u{0}".repeat(1_000)) } else { json!(r.pick(TEXTS)) }
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
        9 => json!("🐝🐝🐝"),
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

const EXTRA_KEYS: &[&str] = &["password", "sshPassphrase", "tab", "text", "force", "", "🐝", "ms"];

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
    let options = SessionOptions { config_dir: None, secrets: Some(Arc::new(MemoryStore::default())), ssh: SshEnv::default(), notify: None };
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
}

/// No journal entry from `from` on holds a secret param, at any depth (R2).
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
