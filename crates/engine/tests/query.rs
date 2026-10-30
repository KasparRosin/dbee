//! Query tabs: run all and run current, one result per statement, errors with their position in
//! the tab text, the result cap, cancel, and what closing the tab, disconnecting or reconnecting
//! does to a running query. Needs the Docker test servers (`cargo xtask testdb up`); tests that
//! write make their own tables in `scratch` and drop them at the end.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_engine::{EngineError, JobId, Json, Meaning, Outcome, Session, Tab};
use dbee_testkit::{require_testdb, unique};
use serde_json::json;

/// Drops the scratch tables it names when the test ends, also when it fails.
struct Scratch(Vec<String>);

impl Drop for Scratch {
    fn drop(&mut self) {
        for t in &self.0 {
            let _ = dbee_testkit::psql(pg14(), &format!("DROP TABLE IF EXISTS scratch.{t} CASCADE"));
        }
    }
}

fn run(s: &mut Session, id: &str, params: Json) -> Json {
    s.run_blocking(id, params, T).unwrap_or_else(|e| panic!("{id}: {e} [{}] {}", e.code(), e.data()))
}

fn q(sql: &str) -> String {
    dbee_testkit::psql(pg14(), sql).unwrap().trim().to_owned()
}

/// A new active query tab holding `text`.
fn tab(s: &mut Session, text: &str) -> u64 {
    run(s, "tab.newQuery", json!({ "text": text }))["tab"].as_u64().unwrap()
}

/// Starts `query.run` with `params`; the job.
fn start(s: &mut Session, params: Json) -> JobId {
    match s.execute("query.run", params) {
        Ok(Outcome::Started(job)) => job,
        other => panic!("query.run did not start a job: {other:?}"),
    }
}

/// `query.run` that fails.
fn run_err(s: &mut Session, params: Json) -> EngineError {
    match s.run_blocking("query.run", params, T) {
        Ok(v) => panic!("query.run succeeded: {v}"),
        Err(e) => e,
    }
}

/// The active query tab as `inspect()` shows it.
fn shown(s: &Session) -> Json {
    s.inspect()["query"].clone()
}

/// The names of a result's columns.
fn names(result: &Json) -> Vec<String> {
    result["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn run_all_gives_one_result_per_statement() {
    let _db = require_testdb!();
    let mut s = connected();
    let text = "select 1 as a; select 2 as b;\nselect 3 as c";
    tab(&mut s, text);
    let r = run(&mut s, "query.run", json!({}));
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "{r}");
    assert_eq!(r["shown"], 2);
    assert_eq!(names(&results[2]), ["c"]);
    assert_eq!(results[2]["columns"][0]["baseType"], "int4", "a single statement's columns are typed");
    let b = text.find("select 2").unwrap();
    assert_eq!((results[1]["start"].as_u64(), results[1]["end"].as_u64()), (Some(b as u64), Some(b as u64 + 13)), "{}", results[1]);
    // The tab keeps them all; the state holds the rows.
    let i = shown(&s);
    assert_eq!((i["results"].as_array().map(Vec::len), &i["shown"], &i["error"], &i["running"]), (Some(3), &json!(2), &Json::Null, &Json::Null));
    let Some(Tab::Query(t)) = s.state().active_tab() else { panic!("a query tab is active") };
    assert_eq!(t.results.len(), 3);
    assert_eq!(t.results[0].result.rows, vec![vec![Some("1".to_owned())]]);
    assert_eq!(t.results[2].range, text.find("select 3").unwrap()..text.len());
    assert_eq!((t.shown, t.error.clone()), (2, None));
    // Another result can be shown; one that is not there cannot.
    assert_eq!(run(&mut s, "query.showResult", json!({ "index": 0 }))["shown"], 0);
    assert_eq!(shown(&s)["shown"], 0);
    for bad in [json!({ "index": 3 }), json!({ "index": -1 }), json!({})] {
        assert_eq!(s.execute("query.showResult", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
    }
}

#[test]
fn run_current_runs_statement_under_cursor() {
    let _db = require_testdb!();
    let mut s = connected();
    let text = "select 1 as a;\nselect 2 as b;\nselect 3 as c";
    tab(&mut s, text);
    let cursor = text.find("2 as b").unwrap();
    let r = run(&mut s, "query.run", json!({ "current": true, "cursor": cursor }));
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{r}");
    assert_eq!(names(&results[0]), ["b"]);
    assert_eq!(r["shown"], 0);
    // At the end is the last statement; `current` needs the cursor, inside the text and on a
    // character boundary.
    let r = run(&mut s, "query.run", json!({ "current": true, "cursor": text.len() }));
    assert_eq!(names(&r["results"][0]), ["c"]);
    assert_eq!(s.execute("query.run", json!({ "current": true })).unwrap_err().code(), "badParams");
    for cursor in [text.len() + 1, 999, usize::MAX] {
        assert_eq!(s.execute("query.run", json!({ "current": true, "cursor": cursor })).unwrap_err().code(), "badParams", "{cursor}");
    }
    run(&mut s, "query.setText", json!({ "text": "select 'é' as e" }));
    let inside = "select 'é".len() - 1;
    let e = s.execute("query.run", json!({ "current": true, "cursor": inside })).unwrap_err();
    assert_eq!(e.code(), "badParams", "{e}");
    assert_eq!(names(&run(&mut s, "query.run", json!({ "current": true, "cursor": inside + 1 }))["results"][0]), ["e"]);
    // Nothing to run: blank text or only comments.
    tab(&mut s, "  -- just a comment\n ; ");
    assert_eq!(s.execute("query.run", json!({})).unwrap_err().code(), "disabled");
}

#[test]
fn error_position_maps_into_tab_text() {
    let _db = require_testdb!();
    let mut s = connected();
    let text = "select 1;\nselect nope";
    tab(&mut s, text);
    let e = run_err(&mut s, json!({}));
    assert_eq!(e.code(), "server", "{e}");
    let at = text.find("nope").unwrap();
    assert_eq!((e.data()["position"].as_u64(), e.data()["statement"].as_u64()), (Some(at as u64), Some(1)), "{}", e.data());
    assert_eq!(e.data()["serverCode"], "42703");
    let i = shown(&s);
    assert_eq!((i["error"]["position"].as_u64(), i["error"]["statement"].as_u64()), (Some(at as u64), Some(1)), "{i}");
    assert!(i["error"]["message"].as_str().unwrap().contains("nope"), "{i}");
    assert_eq!(i["error"]["code"], "server");
    // The statement before the error kept its result.
    assert_eq!((i["results"].as_array().map(Vec::len), &i["shown"]), (Some(1), &json!(0)), "{i}");
    // A good run clears the error.
    run(&mut s, "query.setText", json!({ "text": "select 1" }));
    run(&mut s, "query.run", json!({}));
    assert_eq!(shown(&s)["error"], Json::Null);
}

#[test]
fn a_failed_transaction_is_flagged() {
    let _db = require_testdb!();
    let mut s = connected();
    tab(&mut s, "BEGIN; SELECT 1 / 0");
    run_err(&mut s, json!({}));
    let error = |s: &Session| match s.state().active_tab() {
        Some(Tab::Query(q)) => q.error.clone().unwrap(),
        _ => panic!("a query tab is active"),
    };
    let e = error(&s);
    assert_eq!((e.server_code.as_deref(), e.meaning), (Some("22012"), None), "{e:?}");
    assert_eq!(shown(&s)["error"]["meaning"], Json::Null);
    // The transaction stays open and failed: every statement is refused until a ROLLBACK.
    run(&mut s, "query.setText", json!({ "text": "SELECT 1" }));
    let failed = run_err(&mut s, json!({}));
    assert_eq!((failed.code(), &failed.data()["serverCode"]), ("server", &json!("25P02")), "{failed}");
    let e = error(&s);
    assert_eq!((e.server_code.as_deref(), e.meaning), (Some("25P02"), Some(Meaning::TransactionAborted)), "{e:?}");
    assert!(e.message.ends_with("(SQLSTATE 25P02)"), "{}", e.message);
    let i = shown(&s);
    assert_eq!((&i["error"]["serverCode"], &i["error"]["meaning"]), (&json!("25P02"), &json!("transactionAborted")), "{i}");
    run(&mut s, "query.setText", json!({ "text": "ROLLBACK; SELECT 1" }));
    run(&mut s, "query.run", json!({}));
    assert_eq!(shown(&s)["error"], Json::Null);
}

#[test]
fn error_position_counts_characters_before_it() {
    let _db = require_testdb!();
    let mut s = connected();
    // Multibyte text before and inside the failing statement, and a comment before it.
    let text = "select '🐝ü' as x;\n-- naïve 🐝\nselect 'é', nope";
    tab(&mut s, text);
    let e = run_err(&mut s, json!({}));
    assert_eq!(e.data()["position"].as_u64(), Some(text.find("nope").unwrap() as u64), "{}", e.data());
    // An error at the end of the input points at the statement's end.
    tab(&mut s, "select 1; select 'ü' ||");
    let e = run_err(&mut s, json!({}));
    assert_eq!((e.code(), e.data()["position"].as_u64()), ("server", Some("select 1; select 'ü' ||".len() as u64)), "{e} {}", e.data());
}

#[test]
fn results_capped_with_count() {
    let _db = require_testdb!();
    let mut s = connected();
    tab(&mut s, "select generate_series(1, 25000) as n");
    let r = run(&mut s, "query.run", json!({}));
    let result = &r["results"][0];
    assert_eq!(
        (result["rows"].as_u64(), result["rowsSeen"].as_u64(), result["complete"].as_bool()),
        (Some(10_000), Some(25_000), Some(false)),
        "{r}"
    );
    let Some(Tab::Query(t)) = s.state().active_tab() else { panic!("a query tab is active") };
    assert_eq!(t.results[0].result.rows.len(), 10_000);
    assert_eq!(t.results[0].result.rows.last(), Some(&vec![Some("10000".to_owned())]));
}

#[test]
fn statements_without_rows_report_rows_affected() {
    let _db = require_testdb!();
    let t = unique("t_query");
    let _drop = Scratch(vec![t.clone()]);
    let mut s = connected();
    tab(
        &mut s,
        &format!("create table scratch.{t} (id int);\ninsert into scratch.{t} select generate_series(1, 3);\nselect count(*) from scratch.{t}"),
    );
    let r = run(&mut s, "query.run", json!({}));
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "{r}");
    assert_eq!((results[1]["rowsAffected"].as_u64(), results[1]["columns"].as_array().map(Vec::len)), (Some(3), Some(0)), "{r}");
    // Auto-commit: every statement is written at once.
    assert_eq!(q(&format!("SELECT count(*) FROM scratch.{t}")), "3");
}

#[test]
fn cancel_stops_a_long_query() {
    let _db = require_testdb!();
    let mut s = connected();
    tab(&mut s, "select pg_sleep(30)");
    let job = start(&mut s, json!({}));
    assert_eq!(shown(&s)["running"], json!(job.0));
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    assert_eq!(run(&mut s, "query.cancel", json!({}))["sent"], true);
    let e = s.wait(job, Duration::from_secs(5)).unwrap_err();
    assert_eq!(e.code(), "cancelled", "{e}");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    let i = shown(&s);
    assert_eq!((&i["running"], &i["error"]["code"]), (&Json::Null, &json!("cancelled")), "{i}");
    // Nothing runs now; the connection takes the next run.
    assert_eq!(run(&mut s, "query.cancel", json!({}))["sent"], false);
    run(&mut s, "query.setText", json!({ "text": "select 1 as one" }));
    assert_eq!(names(&run(&mut s, "query.run", json!({}))["results"][0]), ["one"]);
}

#[test]
fn cancel_stops_the_statements_after_it() {
    let _db = require_testdb!();
    let t = unique("t_query");
    let _drop = Scratch(vec![t.clone()]);
    q(&format!("CREATE TABLE scratch.{t} (id int)"));
    let mut s = connected();
    tab(&mut s, &format!("select 1; select pg_sleep(30); insert into scratch.{t} values (1)"));
    let job = start(&mut s, json!({}));
    std::thread::sleep(Duration::from_millis(300));
    run(&mut s, "query.cancel", json!({}));
    let e = s.wait(job, Duration::from_secs(5)).unwrap_err();
    assert_eq!((e.code(), e.data()["statement"].as_u64()), ("cancelled", Some(1)), "{e} {}", e.data());
    assert_eq!(shown(&s)["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(q(&format!("SELECT count(*) FROM scratch.{t}")), "0", "the run stopped at the cancelled statement");
}

#[test]
fn second_run_while_running_is_busy() {
    let _db = require_testdb!();
    let mut s = connected();
    let first = tab(&mut s, "select pg_sleep(30)");
    let job = start(&mut s, json!({}));
    let e = s.execute("query.run", json!({})).unwrap_err();
    assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: a query is running in Query 1"));
    // Another tab waits too: the query connection runs one statement at a time.
    tab(&mut s, "select 1");
    assert_eq!(s.execute("query.run", json!({})).unwrap_err().code(), "busy");
    assert_eq!(run(&mut s, "query.cancel", json!({}))["sent"], false, "the other tab's run is not this tab's to cancel");
    run(&mut s, "tab.activate", json!({ "tab": first }));
    run(&mut s, "query.cancel", json!({}));
    assert_eq!(s.wait(job, Duration::from_secs(5)).unwrap_err().code(), "cancelled");
}

#[test]
fn query_connection_is_separate_from_browse() {
    let _db = require_testdb!();
    let t = unique("t_query");
    let _drop = Scratch(vec![t.clone()]);
    q(&format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, name text); INSERT INTO scratch.{t} VALUES (1, 'a'), (2, 'b')"));
    let mut s = connected();
    // A transaction left open in the query tab, with a row written in it.
    let query = tab(&mut s, &format!("BEGIN;\ninsert into scratch.{t} values (99, 'open')"));
    run(&mut s, "query.run", json!({}));
    // The table tab works on the browse connection, and its Apply commits at once.
    run(&mut s, "table.open", json!({ "schema": "scratch", "table": t }));
    assert_eq!(run(&mut s, "table.page", json!({ "to": "next" }))["rows"], 2);
    run(&mut s, "cell.edit", json!({ "row": 0, "column": "name", "value": "applied" }));
    assert_eq!(run(&mut s, "edit.apply", json!({}))["written"], 1);
    assert_eq!(q(&format!("SELECT string_agg(id || ':' || name, ',' ORDER BY id) FROM scratch.{t}")), "1:applied,2:b");
    // The query tab's transaction is still open: its row is not there for others.
    run(&mut s, "tab.activate", json!({ "tab": query }));
    run(&mut s, "query.setText", json!({ "text": format!("select count(*) from scratch.{t}") }));
    assert_eq!(run(&mut s, "query.run", json!({}))["results"][0]["rows"], 1);
    let Some(Tab::Query(qt)) = s.state().active_tab() else { panic!("a query tab is active") };
    assert_eq!(qt.results[0].result.rows, vec![vec![Some("3".to_owned())]], "the query connection sees its own row");
    run(&mut s, "query.setText", json!({ "text": "ROLLBACK" }));
    run(&mut s, "query.run", json!({}));
    assert_eq!(q(&format!("SELECT count(*) FROM scratch.{t} WHERE id = 99")), "0");
}

#[test]
fn read_only_refuses_insert() {
    let _db = require_testdb!();
    let t = unique("t_query_ro");
    let _drop = Scratch(vec![t.clone()]);
    q(&format!("CREATE TABLE scratch.{t} (id int)"));
    let mut s = session();
    let config = dbee_config::ConnectionConfig { read_only: true, ..pg14_config() };
    open(&mut s, json!({ "config": config, "password": pw() }));
    tab(&mut s, &format!("insert into scratch.{t} values (1)"));
    let e = run_err(&mut s, json!({}));
    assert_eq!(e.code(), "readOnly", "{e}");
    assert_eq!(shown(&s)["error"]["code"], "readOnly");
    run(&mut s, "query.setText", json!({ "text": "show default_transaction_read_only" }));
    run(&mut s, "query.run", json!({}));
    let Some(Tab::Query(qt)) = s.state().active_tab() else { panic!("a query tab is active") };
    assert_eq!(qt.results[0].result.rows, vec![vec![Some("on".to_owned())]]);
    assert_eq!(q(&format!("SELECT count(*) FROM scratch.{t}")), "0");
}

/// A string only this test's query holds, so other tests' queries do not count.
fn marker() -> String {
    unique("dbee_marker")
}

/// How many dbee backends on pg14 are running a query that holds `marker`.
fn active_with(marker: &str) -> String {
    q(&format!("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'dbee' AND state = 'active' AND query LIKE '%{marker}%'"))
}

#[test]
fn disconnect_during_query_cancels_it() {
    let _db = require_testdb!();
    let mut s = connected();
    let m = marker();
    tab(&mut s, &format!("select pg_sleep(30), '{m}' as tag"));
    let job = start(&mut s, json!({}));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(active_with(&m), "1", "the query runs");
    run(&mut s, "connection.close", json!({ "force": true }));
    assert!(s.wait_idle(Duration::from_secs(5)), "the query was not cancelled");
    assert_eq!(active_with(&m), "0");
    let results = s.pump();
    assert_eq!(results, vec![(job, Ok(json!({ "ignored": true })))]);
    assert_eq!(s.inspect()["screen"], "connections");
}

#[test]
fn closing_running_tab_ignores_result() {
    let _db = require_testdb!();
    let mut s = connected();
    let m = marker();
    tab(&mut s, &format!("select pg_sleep(30), '{m}' as tag"));
    let job = start(&mut s, json!({}));
    std::thread::sleep(Duration::from_millis(300));
    run(&mut s, "tab.close", json!({ "force": true }));
    assert!(s.wait_idle(Duration::from_secs(5)), "closing the tab did not cancel its query");
    assert_eq!(s.pump(), vec![(job, Ok(json!({ "ignored": true })))]);
    assert!(s.state().tabs.is_empty());
    assert_eq!(active_with(&m), "0");
    // The query connection goes on with the next run.
    tab(&mut s, "select 1 as one");
    assert_eq!(names(&run(&mut s, "query.run", json!({}))["results"][0]), ["one"]);
}

#[test]
fn reconnect_during_query_stops_it() {
    let _db = require_testdb!();
    let mut s = connected();
    let m = marker();
    tab(&mut s, &format!("select pg_sleep(30), '{m}' as tag"));
    let job = start(&mut s, json!({}));
    std::thread::sleep(Duration::from_millis(300));
    run(&mut s, "connection.reconnect", json!({}));
    assert_eq!(shown(&s)["running"], Json::Null, "the run died with its connection");
    assert_eq!(s.wait(job, Duration::from_secs(5)).unwrap(), json!({ "ignored": true }));
    assert_eq!(active_with(&m), "0");
    run(&mut s, "query.setText", json!({ "text": "select 2 as two" }));
    assert_eq!(names(&run(&mut s, "query.run", json!({}))["results"][0]), ["two"]);
}

/// `pg_backend_pid()` of the active query tab's connection, from a run of its own.
fn query_backend(s: &mut Session) -> String {
    run(s, "query.setText", json!({ "text": "select pg_backend_pid()" }));
    run(s, "query.run", json!({}));
    let Some(Tab::Query(t)) = s.state().active_tab() else { panic!("a query tab is active") };
    t.results[0].result.rows[0][0].clone().unwrap()
}

/// Terminates one backend on pg14 and waits until it has exited.
fn terminate(pid: &str) {
    assert_eq!(q(&format!("SELECT pg_terminate_backend({pid}, 5000)")), "t");
}

#[test]
fn lost_query_connection_is_opened_again() {
    let _db = require_testdb!();
    let mut s = connected();
    tab(&mut s, "BEGIN");
    run(&mut s, "query.run", json!({}));
    let pid = query_backend(&mut s);
    // Only the query connection dies: its session, with the open transaction, is gone.
    terminate(&pid);
    run(&mut s, "query.setText", json!({ "text": "COMMIT" }));
    let e = run_err(&mut s, json!({}));
    let lost = "the query connection was lost, and its session with it: the statement that was running may or may not have taken effect (its outcome is unknown), any other uncommitted work in the transaction is gone, and SET values and temporary tables are gone. Run again to open a new one";
    assert_eq!((e.code(), e.to_string().as_str()), ("connectionLost", lost));
    assert_eq!(e.data()["action"], Json::Null, "running again is enough: {}", e.data());
    let i = shown(&s);
    assert_eq!((&i["error"]["message"], &i["error"]["code"], &i["running"]), (&json!(lost), &json!("connectionLost"), &Json::Null), "{i}");
    // The session is not lost: the browse connection works on.
    assert_eq!(s.inspect()["connection"]["lost"], false);
    run(&mut s, "table.open", json!({ "schema": "shop", "table": "customers" }));
    // The next run opens a new query connection.
    let query = s.state().tabs[0].id().0;
    run(&mut s, "tab.activate", json!({ "tab": query }));
    let fresh = query_backend(&mut s);
    assert_ne!(fresh, pid);
    assert_eq!(shown(&s)["error"], Json::Null);
    // Lost in the middle of a run: it stops at that statement, with the same message.
    run(&mut s, "query.setText", json!({ "text": format!("select 1; select pg_terminate_backend({fresh}); select 3") }));
    let e = run_err(&mut s, json!({}));
    assert_eq!((e.code(), e.to_string().as_str(), e.data()["statement"].as_u64()), ("connectionLost", lost, Some(1)), "{}", e.data());
    assert_eq!(e.data()["action"], Json::Null);
    assert_eq!(shown(&s)["results"].as_array().map(Vec::len), Some(1));
    assert_eq!(s.inspect()["connection"]["lost"], false);
    assert_ne!(query_backend(&mut s), fresh);
}

#[test]
fn queries_and_their_cancel_go_through_the_tunnel() {
    let db = require_testdb!();
    let mut s = session_in(dbee_testkit::temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line)));
    open(&mut s, json!({ "config": via_ssh(dbee_config::SshAuth::Key { path: None }), "password": pw() }));
    tab(&mut s, "select current_setting('application_name') as app");
    assert_eq!(names(&run(&mut s, "query.run", json!({}))["results"][0]), ["app"]);
    run(&mut s, "query.setText", json!({ "text": "select pg_sleep(30)" }));
    let job = start(&mut s, json!({}));
    std::thread::sleep(Duration::from_millis(300));
    run(&mut s, "query.cancel", json!({}));
    assert_eq!(s.wait(job, Duration::from_secs(5)).unwrap_err().code(), "cancelled");
}

#[test]
fn query_commands_need_a_query_tab_and_a_connection() {
    let _db = require_testdb!();
    {
        let mut s = session();
        tab(&mut s, "select 1");
        assert_eq!(s.execute("query.run", json!({})).unwrap_err().data()["why"], "not connected");
    }
    // One session at a time (see `common`).
    let mut s = connected();
    run(&mut s, "table.open", json!({ "schema": "shop", "table": "customers" }));
    for id in ["query.run", "query.cancel", "query.showResult"] {
        assert_eq!(s.execute(id, json!({ "index": 0 })).unwrap_err().data()["why"], "no query tab is active", "{id}");
    }
    tab(&mut s, "select 1");
    assert_eq!(s.execute("query.showResult", json!({ "index": 0 })).unwrap_err().code(), "badParams", "no results yet");
    for bad in [json!({ "current": "yes" }), json!({ "current": true, "cursor": -1 }), json!({ "cursor": "x" })] {
        assert_eq!(s.execute("query.run", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
    }
}
