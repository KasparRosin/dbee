//! Session, registry, job and guard tests, and the test-only commands they drive.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dbee_config::MemoryStore;
use dbee_ssh::SshEnv;
use serde_json::json;

use crate::cmd::{self, CommandSpec, p};
use crate::state::Tab;
use crate::{EngineError, Json, Outcome, Session, SessionOptions, TabId};

/// `test.panic` changes the state, then panics; `test.panicEnabled` panics in its `enabled`;
/// `test.panicAfterJob` starts a job, then panics; `test.sleepJob {ms, tab?, panic?: "job" |
/// "apply" | "applyAfterJob"}` is a job that sleeps.
pub(crate) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "test.panic",
        label: "Panic (test)",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: cmd::always,
        run: run_panic,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: "test.panicEnabled",
        label: "Panic in enabled (test)",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: panic_enabled,
        run: run_panic,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: "test.panicAfterJob",
        label: "Panic after starting a job (test)",
        menu: &[],
        shortcut: None,
        params: "{}",
        enabled: cmd::always,
        run: run_panic_after_job,
        journal: true,
        undoable: false,
    },
    CommandSpec {
        id: "test.sleepJob",
        label: "Sleep job (test)",
        menu: &[],
        shortcut: None,
        params: "{ms, tab?, panic?: \"job\" | \"apply\"}",
        enabled: cmd::always,
        run: run_sleep_job,
        journal: true,
        undoable: false,
    },
];

fn run_panic(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    // Change everything a command could change before it fails, so the restore is visible.
    s.state.tabs.clear();
    s.state.active = None;
    s.state.next_tab = 999;
    s.state.store_warning = Some("changed".into());
    s.ui_requests.push(crate::UiRequest::Notice("from a panicking command".into()));
    panic!("test.panic panics on purpose");
}

/// A job whose `Apply` leaves a mark in the state, so a run that should have been dropped shows.
fn marking_job(s: &mut Session) -> crate::JobId {
    s.spawn_job("test.mark", async {
        let apply: crate::Apply = Box::new(|s: &mut Session| {
            s.state.store_warning = Some("an orphaned job was applied".into());
            Ok(json!({}))
        });
        apply
    })
}

fn run_panic_after_job(s: &mut Session, _: &Json) -> Result<Outcome, EngineError> {
    marking_job(s);
    s.state.tabs.clear();
    panic!("test.panicAfterJob panics on purpose");
}

fn panic_enabled(_: &Session) -> Result<(), String> {
    panic!("test.panicEnabled panics on purpose");
}

fn run_sleep_job(s: &mut Session, params: &Json) -> Result<Outcome, EngineError> {
    let ms = p::usize(params, "ms")?;
    let tab = p::opt_tab_id(params, "tab")?;
    let panic_in = p::opt_str(params, "panic")?.map(str::to_owned);
    if let Some(t) = tab
        && s.state.tab(t).is_none()
    {
        return Err(EngineError::BadParams { id: String::new(), msg: format!("no tab {}", t.0) });
    }
    let millis = u64::try_from(ms).unwrap_or(u64::MAX);
    let job = s.spawn_job("test.sleepJob", async move {
        tokio::time::sleep(Duration::from_millis(millis)).await;
        if panic_in.as_deref() == Some("job") {
            panic!("the sleep job panics on purpose");
        }
        let apply: crate::Apply = Box::new(move |s: &mut Session| {
            if panic_in.as_deref() == Some("apply") {
                s.state.tabs.clear();
                panic!("the sleep job's apply panics on purpose");
            }
            if panic_in.as_deref() == Some("applyAfterJob") {
                marking_job(s);
                panic!("the sleep job's apply panics after starting a job, on purpose");
            }
            // Only touch the tab when it is still open and still runs this job.
            let me = s.applying_job();
            if let Some(Tab::Query(q)) = tab.and_then(|t| s.state.tab_mut(t))
                && q.running == me
            {
                q.running = None;
            }
            Ok(json!({ "slept": ms }))
        });
        apply
    });
    if let Some(Tab::Query(q)) = tab.and_then(|t| s.state.tab_mut(t)) {
        q.running = Some(job);
    }
    Ok(Outcome::Started(job))
}

fn options() -> SessionOptions {
    SessionOptions { config_dir: None, secrets: Some(Arc::new(MemoryStore::default())), ssh: SshEnv::default(), notify: None }
}

fn session() -> Session {
    Session::new(options()).unwrap()
}

fn new_query(s: &mut Session, text: &str) -> TabId {
    let Outcome::Done(r) = s.execute("tab.newQuery", json!({ "text": text })).unwrap() else { panic!("newQuery started a job") };
    TabId(r["tab"].as_u64().unwrap())
}

fn started(o: Outcome) -> crate::JobId {
    match o {
        Outcome::Started(j) => j,
        Outcome::Done(v) => panic!("expected a job, got {v}"),
    }
}

#[test]
fn every_command_survives_empty_params() {
    let mut s = session();
    for c in s.commands() {
        // These return Internal by design (R3).
        if c.id.starts_with("test.panic") {
            continue;
        }
        if let Err(EngineError::Internal { .. }) = s.execute(c.id, json!({})) {
            panic!("{} panicked", c.id)
        }
        let _ = s.wait_idle(Duration::from_secs(5));
    }
}

#[test]
fn panicking_command_is_caught_and_state_restored() {
    let mut s = session();
    s.execute("tab.newQuery", json!({"text": "select 1"})).unwrap();
    let before = s.inspect();
    assert!(matches!(s.execute("test.panic", json!({})), Err(EngineError::Internal { .. })));
    assert_eq!(s.inspect(), before);
    assert_eq!(s.state().next_tab, 2);
    assert!(s.take_ui_requests().is_empty(), "a panicking command's UI requests are dropped");
    // The session keeps working.
    new_query(&mut s, "");
    assert_eq!(s.state().tabs.len(), 2);
}

#[test]
fn internal_error_names_the_command() {
    let mut s = session();
    let e = s.execute("test.panic", json!({})).unwrap_err();
    assert_eq!(e.code(), "internal");
    assert_eq!(e.data()["command"], "test.panic");
    assert!(e.to_string().contains("panics on purpose"), "{e}");
}

#[test]
fn panicking_enabled_is_caught() {
    let mut s = session();
    let info = s.commands().into_iter().find(|c| c.id == "test.panicEnabled").unwrap();
    assert!(!info.enabled);
    assert!(info.disabled_reason.is_some());
    assert_eq!(s.execute("test.panicEnabled", json!({})).unwrap_err().code(), "internal");
}

#[test]
fn journal_leaves_out_secrets() {
    let mut s = session();
    let _ = s.execute("tab.newQuery", json!({"text": "x", "password": "pw", "sshPassphrase": "pp"}));
    let j = serde_json::to_string(s.journal()).unwrap();
    assert!(!j.contains("pw") && !j.contains("pp"));
}

#[test]
fn secret_params_are_recognised() {
    for k in ["password", "passphrase", "sshPassword", "dbPassword", "sshPassphrase"] {
        assert!(cmd::is_secret_param(k), "{k}");
    }
    for k in ["text", "passwordHint", "user", "tab"] {
        assert!(!cmd::is_secret_param(k), "{k}");
    }
    let kept = cmd::journal_params(&json!({
        "text": "keep",
        "password": "pw1",
        "nested": {"sshPassword": "pw2", "list": [{"sshPassphrase": "pp"}, {"user": "u"}]}
    }));
    assert_eq!(kept, json!({"text": "keep", "nested": {"list": [{}, {"user": "u"}]}}));
}

#[test]
fn journal_records_successful_journaled_commands_only() {
    let mut s = session();
    let t = new_query(&mut s, "select 1");
    s.execute("tab.list", json!({})).unwrap();
    let _ = s.execute("tab.activate", json!({"tab": 999}));
    s.execute("tab.activate", json!({"tab": t.0})).unwrap();
    let ids: Vec<&str> = s.journal().iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["tab.newQuery", "tab.activate"]);
    assert_eq!(s.journal()[1].1, json!({"tab": t.0}));
}

#[test]
fn typing_collapses_into_one_journal_entry() {
    let mut s = session();
    new_query(&mut s, "");
    for text in ["s", "se", "sel", "select 1"] {
        s.execute("query.setText", json!({ "text": text })).unwrap();
    }
    assert_eq!(s.journal().len(), 2);
    assert_eq!(s.journal()[1], ("query.setText".to_string(), json!({"text": "select 1"})));
}

#[test]
fn journal_keeps_the_newest_entries() {
    let mut s = session();
    let t = new_query(&mut s, "");
    for _ in 0..10_001 {
        s.execute("tab.activate", json!({"tab": t.0})).unwrap();
    }
    // 10,002 entries pushed; each time the journal passes 10,000 the oldest 5,000 go.
    let mut expected = 0;
    for _ in 0..1 + 10_001 {
        expected += 1;
        if expected > 10_000 {
            expected -= 5_000;
        }
    }
    assert_eq!(expected, 5_002);
    assert_eq!(s.journal().len(), expected);
    assert_eq!(s.journal()[0].0, "tab.activate", "the oldest entries went first");
}

#[test]
fn unknown_and_disabled() {
    let mut s = session();
    assert_eq!(s.execute("nope", json!({})).unwrap_err().code(), "unknownCommand");
    assert_eq!(s.execute("query.setText", json!({"text": "a"})).unwrap_err().code(), "disabled");
}

#[test]
fn errors_carry_codes_and_data() {
    let mut s = session();
    let e = s.execute("nope", json!({})).unwrap_err();
    assert_eq!(e.data(), json!({"command": "nope"}));
    let e = s.execute("query.setText", json!({"text": "a"})).unwrap_err();
    assert_eq!(e.data()["command"], "query.setText");
    assert!(e.data()["why"].as_str().is_some_and(|w| !w.is_empty()));
    let e = s.execute("tab.newQuery", json!({"text": 5})).unwrap_err();
    assert_eq!(e.code(), "badParams");
    assert_eq!(e.data()["command"], "tab.newQuery", "the helpers' BadParams gets the command id");
    assert!(e.to_string().contains("text"), "{e}");
}

#[test]
fn needs_codes_and_data() {
    use crate::Need;
    let cases = [
        (Need::HostKey { host: "h".into(), key_type: "ssh-ed25519".into(), fingerprint: "SHA256:x".into() }, "needsHostKey"),
        (Need::Passphrase { key: "/k".into() }, "needsPassphrase"),
        (Need::Password, "needsPassword"),
        (Need::SshPassword, "needsSshPassword"),
        (Need::Confirm { what: "queryText".into() }, "needsConfirm"),
    ];
    for (need, code) in cases {
        assert_eq!(EngineError::Needs { need, message: "m".into() }.code(), code);
    }
    let e = EngineError::Needs {
        need: Need::HostKey { host: "h".into(), key_type: "ssh-ed25519".into(), fingerprint: "SHA256:x".into() },
        message: "m".into(),
    };
    assert_eq!(e.data(), json!({"host": "h", "keyType": "ssh-ed25519", "fingerprint": "SHA256:x"}));
    let db = EngineError::Db { code: "connectionLost", message: "gone".into(), data: json!({"x": 1}) };
    assert_eq!((db.code(), db.data()), ("connectionLost", json!({"x": 1})));
    assert_eq!(EngineError::Busy("apply in progress".into()).code(), "busy");
    assert_eq!(EngineError::Timeout.code(), "timeout");
    assert_eq!(EngineError::Config("c".into()).code(), "config");
    assert_eq!(EngineError::Disabled { id: "a".into(), why: "b".into() }.code(), "disabled");
}

#[test]
fn params_must_be_an_object() {
    let mut s = session();
    assert_eq!(s.execute("tab.newQuery", json!([1, 2])).unwrap_err().code(), "badParams");
    assert_eq!(s.execute("tab.newQuery", json!("text")).unwrap_err().code(), "badParams");
    // `null` counts as no params.
    assert!(s.execute("tab.newQuery", Json::Null).is_ok());
}

#[test]
fn param_helpers_name_the_key() {
    let p = json!({"s": "x", "n": 3, "neg": -1, "f": 1.5, "b": true, "nul": null, "t": 7});
    assert_eq!(p::str(&p, "s").unwrap(), "x");
    assert_eq!(p::opt_str(&p, "missing").unwrap(), None);
    assert_eq!(p::opt_str(&p, "nul").unwrap(), None);
    assert_eq!(p::usize(&p, "n").unwrap(), 3);
    assert_eq!(p::opt_usize(&p, "missing").unwrap(), None);
    assert!(p::bool_or(&p, "missing", true).unwrap());
    assert!(p::bool_or(&p, "b", false).unwrap());
    assert_eq!(p::tab_id(&p, "t").unwrap(), TabId(7));
    assert_eq!(p::opt_tab_id(&p, "missing").unwrap(), None);
    let fails = [
        p::str(&p, "n").map(|_| ()),
        p::str(&p, "missing").map(|_| ()),
        p::opt_str(&p, "n").map(|_| ()),
        p::usize(&p, "neg").map(|_| ()),
        p::usize(&p, "f").map(|_| ()),
        p::usize(&p, "s").map(|_| ()),
        p::opt_usize(&p, "s").map(|_| ()),
        p::bool_or(&p, "s", true).map(|_| ()),
        p::tab_id(&p, "missing").map(|_| ()),
        p::opt_tab_id(&p, "neg").map(|_| ()),
    ];
    for (i, r) in fails.into_iter().enumerate() {
        match r {
            Err(EngineError::BadParams { msg, .. }) => assert!(msg.contains('`'), "case {i}: {msg}"),
            other => panic!("case {i}: {other:?}"),
        }
    }
}

#[test]
fn registry_ids_are_unique_and_shortcuts_set() {
    let s = session();
    let infos = s.commands();
    let mut ids: Vec<&str> = infos.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), before, "duplicate command ids");
    let get = |id: &str| infos.iter().find(|c| c.id == id).unwrap();
    assert_eq!(get("tab.newQuery").shortcut, Some("Ctrl+T"));
    assert_eq!(get("tab.close").shortcut, Some("Ctrl+W"));
    assert_eq!(get("tab.newQuery").menu, ["File"]);
    let json = serde_json::to_value(get("tab.close")).unwrap();
    assert_eq!(json["disabledReason"], "no tab is open");
    assert_eq!(json["enabled"], false);
}

#[test]
fn new_query_tabs_are_numbered_and_active() {
    let mut s = session();
    let a = new_query(&mut s, "");
    let b = new_query(&mut s, "select 2");
    assert_ne!(a, b);
    assert_eq!(s.state().active, Some(b));
    let titles: Vec<String> = s.state().tabs.iter().map(Tab::title).collect();
    assert_eq!(titles, ["Query 1", "Query 2"]);
    s.execute("tab.close", json!({"tab": a.0})).unwrap();
    new_query(&mut s, "");
    let titles: Vec<String> = s.state().tabs.iter().map(Tab::title).collect();
    assert_eq!(titles, ["Query 2", "Query 1"], "the lowest free number is reused");
    let list = s.execute("tab.list", json!({})).unwrap();
    let Outcome::Done(list) = list else { panic!() };
    assert_eq!(list["tabs"].as_array().unwrap().len(), 2);
    assert_eq!(list["tabs"][0]["kind"], "query");
}

#[test]
fn set_text_changes_the_active_query_tab() {
    let mut s = session();
    let a = new_query(&mut s, "a");
    let b = new_query(&mut s, "b");
    s.execute("tab.activate", json!({"tab": a.0})).unwrap();
    s.execute("query.setText", json!({"text": "select 'a;b' -- 🐝"})).unwrap();
    let text = |s: &Session, t: TabId| match s.state().tab(t) {
        Some(Tab::Query(q)) => q.text.clone(),
        _ => panic!("no query tab {t:?}"),
    };
    assert_eq!(text(&s, a), "select 'a;b' -- 🐝");
    assert_eq!(text(&s, b), "b");
    assert_eq!(s.inspect()["query"]["text"], "select 'a;b' -- 🐝");
    assert_eq!(s.execute("tab.activate", json!({"tab": 42})).unwrap_err().code(), "badParams");
}

#[test]
fn closing_query_tab_with_text_needs_confirmation() {
    let mut s = session();
    new_query(&mut s, "select 1");
    let e = s.execute("tab.close", json!({})).unwrap_err();
    assert_eq!(e.code(), "needsConfirm");
    assert_eq!(s.state().tabs.len(), 1);
    s.execute("tab.close", json!({"force": true})).unwrap();
    assert!(s.state().tabs.is_empty());
    assert_eq!(s.state().active, None);
}

#[test]
fn blank_query_tab_closes_without_asking() {
    let mut s = session();
    new_query(&mut s, "  \n\t ");
    s.execute("tab.close", json!({})).unwrap();
    assert!(s.state().tabs.is_empty());
}

#[test]
fn closing_the_active_tab_activates_a_neighbour() {
    let mut s = session();
    let a = new_query(&mut s, "");
    let b = new_query(&mut s, "");
    let c = new_query(&mut s, "");
    s.execute("tab.activate", json!({"tab": b.0})).unwrap();
    s.execute("tab.close", json!({})).unwrap();
    assert_eq!(s.state().active, Some(c), "the tab to the right");
    s.execute("tab.close", json!({})).unwrap();
    assert_eq!(s.state().active, Some(a), "the last tab: the one to the left");
    let d = new_query(&mut s, "");
    s.execute("tab.close", json!({"tab": a.0})).unwrap();
    assert_eq!(s.state().active, Some(d), "closing another tab keeps the active one");
}

#[test]
fn jobs_finish_through_pump_and_wait() {
    let mut s = session();
    let Outcome::Started(j) = s.execute("test.sleepJob", json!({"ms": 50})).unwrap() else { panic!() };
    assert!(s.busy());
    assert_eq!(s.wait(j, Duration::from_secs(5)).unwrap(), json!({"slept": 50}));
    assert!(!s.busy());
}

#[test]
fn pump_returns_finished_jobs() {
    let mut s = session();
    let j = started(s.execute("test.sleepJob", json!({"ms": 10})).unwrap());
    let mut got = Vec::new();
    for _ in 0..500 {
        got.extend(s.pump());
        if !got.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(got, vec![(j, Ok(json!({"slept": 10})))]);
    assert!(!s.busy());
    assert!(s.pump().is_empty());
}

#[test]
fn results_applied_while_waiting_come_from_the_next_pump() {
    let mut s = session();
    let first = started(s.execute("test.sleepJob", json!({"ms": 10})).unwrap());
    let second = started(s.execute("test.sleepJob", json!({"ms": 200})).unwrap());
    assert_eq!(s.wait(second, Duration::from_secs(5)).unwrap(), json!({"slept": 200}));
    assert_eq!(s.pump(), vec![(first, Ok(json!({"slept": 10})))]);
    // A finished job not yet pumped can still be waited for.
    let third = started(s.execute("test.sleepJob", json!({"ms": 10})).unwrap());
    assert!(s.wait_idle(Duration::from_secs(5)));
    assert_eq!(s.wait(third, Duration::from_secs(1)).unwrap(), json!({"slept": 10}));
    assert!(s.pump().is_empty());
    assert_eq!(s.wait(third, Duration::from_secs(1)).unwrap_err().code(), "badParams", "already delivered");
}

#[test]
fn run_blocking_returns_done_and_job_results() {
    let mut s = session();
    assert_eq!(s.run_blocking("tab.newQuery", json!({}), Duration::from_secs(1)).unwrap(), json!({"tab": 1}));
    assert_eq!(s.run_blocking("test.sleepJob", json!({"ms": 5}), Duration::from_secs(5)).unwrap(), json!({"slept": 5}));
    assert_eq!(s.run_blocking("nope", json!({}), Duration::from_secs(1)).unwrap_err().code(), "unknownCommand");
}

#[test]
fn wait_times_out() {
    let mut s = session();
    let j = started(s.execute("test.sleepJob", json!({"ms": 2000})).unwrap());
    assert_eq!(s.wait(j, Duration::from_millis(50)).unwrap_err().code(), "timeout");
    assert!(s.busy());
    assert!(s.wait_idle(Duration::from_secs(5)));
    assert!(!s.busy());
}

#[test]
fn wait_idle_times_out_while_a_job_runs() {
    let mut s = session();
    started(s.execute("test.sleepJob", json!({"ms": 1000})).unwrap());
    assert!(!s.wait_idle(Duration::from_millis(20)));
    assert!(s.wait_idle(Duration::from_secs(5)));
    assert!(s.wait_idle(Duration::ZERO), "nothing runs");
    // A huge timeout must not overflow the deadline.
    assert!(s.wait_idle(Duration::MAX));
}

#[test]
fn panicking_job_becomes_internal() {
    let mut s = session();
    new_query(&mut s, "keep");
    let j = started(s.execute("test.sleepJob", json!({"ms": 1, "panic": "job"})).unwrap());
    let e = s.wait(j, Duration::from_secs(5)).unwrap_err();
    assert_eq!(e.code(), "internal");
    assert_eq!(e.data()["command"], "test.sleepJob");
    assert!(!s.busy());
    assert_eq!(s.state().tabs.len(), 1);
}

#[test]
fn panicking_apply_restores_the_state() {
    let mut s = session();
    new_query(&mut s, "keep");
    let before = s.inspect();
    let j = started(s.execute("test.sleepJob", json!({"ms": 1, "panic": "apply"})).unwrap());
    let e = s.wait(j, Duration::from_secs(5)).unwrap_err();
    assert_eq!(e.code(), "internal");
    assert_eq!(s.inspect(), before);
    assert!(!s.busy());
}

#[test]
fn notify_is_called_after_each_result() {
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let mut s = Session::new(SessionOptions {
        notify: Some(Arc::new(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })),
        ..options()
    })
    .unwrap();
    for ms in [1, 5] {
        let j = started(s.execute("test.sleepJob", json!({ "ms": ms })).unwrap());
        s.wait(j, Duration::from_secs(5)).unwrap();
    }
    // The worker sends the result first and calls `notify` after it, so `wait` can return
    // before the last call: poll.
    let deadline = Instant::now() + Duration::from_secs(5);
    while count.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn result_for_closed_tab_is_ignored() {
    let mut s = session();
    let t = new_query(&mut s, "select pg_sleep(1)");
    let j = started(s.execute("test.sleepJob", json!({"ms": 100, "tab": t.0})).unwrap());
    match s.state().tab(t) {
        Some(Tab::Query(q)) => assert_eq!(q.running, Some(j)),
        other => panic!("{other:?}"),
    }
    s.execute("tab.close", json!({"tab": t.0, "force": true})).unwrap();
    assert!(s.wait(j, Duration::from_secs(5)).is_ok());
    assert!(s.state().tabs.is_empty());
    assert_eq!(s.state().active, None);
}

#[test]
fn finished_job_clears_its_tab() {
    let mut s = session();
    let t = new_query(&mut s, "");
    let j = started(s.execute("test.sleepJob", json!({"ms": 1, "tab": t.0})).unwrap());
    assert_eq!(s.inspect()["query"]["running"], json!(j.0));
    s.wait(j, Duration::from_secs(5)).unwrap();
    assert_eq!(s.inspect()["query"]["running"], Json::Null);
}

#[test]
fn inspect_summarises_the_session() {
    let mut s = session();
    let i = s.inspect();
    assert_eq!(i["screen"], "connections");
    assert_eq!(i["connection"], Json::Null);
    assert_eq!(i["active"], Json::Null);
    assert_eq!(i["secretStore"], "memory");
    let t = new_query(&mut s, "select 1");
    let j = started(s.execute("test.sleepJob", json!({"ms": 1000})).unwrap());
    let i = s.inspect();
    assert_eq!(i["active"], json!(t.0));
    assert_eq!(i["tabs"], json!([{"id": t.0, "kind": "query", "title": "Query 1"}]));
    assert_eq!(i["query"]["text"], "select 1");
    assert_eq!(i["table"], Json::Null);
    assert_eq!(i["jobs"], json!([{"job": j.0, "label": "test.sleepJob"}]));
    assert!(s.wait_idle(Duration::from_secs(5)));
}

#[test]
fn corrupt_connections_file_gives_a_store_warning() {
    let home = dbee_testkit::temp_home(None, None);
    std::fs::write(home.home.join(dbee_config::store::FILE), "{ not json").unwrap();
    let s = Session::new(SessionOptions { config_dir: Some(home.home.clone()), ..options() }).unwrap();
    let w = s.state().store_warning.clone().unwrap();
    assert!(w.contains("damaged"), "{w}");
    assert_eq!(s.inspect()["storeWarning"], json!(w));
    let clean = Session::new(options()).unwrap();
    assert_eq!(clean.state().store_warning, None);
}

#[test]
fn jobs_started_by_a_panicking_command_are_orphaned() {
    let mut s = session();
    new_query(&mut s, "keep");
    let before = s.inspect();
    assert_eq!(s.execute("test.panicAfterJob", json!({})).unwrap_err().code(), "internal");
    // The job is not aborted: it runs to its end, and then its result is dropped.
    assert!(s.wait_idle(Duration::from_secs(5)));
    let results = s.pump();
    assert_eq!(results.len(), 1, "{results:?}");
    let e = results[0].1.clone().unwrap_err();
    assert_eq!((e.code(), e.data()["command"].as_str()), ("internal", Some("test.mark")));
    assert_eq!(s.state().store_warning, None, "the orphan's Apply must not run");
    assert_eq!(s.inspect(), before);
}

#[test]
fn jobs_started_by_a_panicking_apply_are_orphaned() {
    let mut s = session();
    let before = s.inspect();
    let j = started(s.execute("test.sleepJob", json!({"ms": 1, "panic": "applyAfterJob"})).unwrap());
    assert_eq!(s.wait(j, Duration::from_secs(5)).unwrap_err().code(), "internal");
    assert!(s.wait_idle(Duration::from_secs(5)));
    let results = s.pump();
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0].1.clone().unwrap_err().code(), "internal");
    assert_eq!(s.state().store_warning, None, "the orphan's Apply must not run");
    assert_eq!(s.inspect(), before);
}

#[test]
fn jobs_of_a_successful_command_are_not_orphaned_by_a_later_panic() {
    let mut s = session();
    let j = started(s.execute("test.sleepJob", json!({"ms": 50})).unwrap());
    assert_eq!(s.execute("test.panic", json!({})).unwrap_err().code(), "internal");
    assert_eq!(s.wait(j, Duration::from_secs(5)).unwrap(), json!({"slept": 50}));
}

#[test]
fn session_can_be_dropped_inside_async_code() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut s = session();
    started(s.execute("test.sleepJob", json!({"ms": 10_000})).unwrap());
    let t = Instant::now();
    rt.block_on(async move { drop(s) });
    assert!(t.elapsed() < Duration::from_secs(5), "dropping waited for the job");
}

#[test]
fn session_is_send() {
    fn send<T: Send>() {}
    send::<Session>();
}
