//! The query session's transaction (gaps 14 and 53): whether one is open, as each driver reports
//! it after a run, which tab's run opened it, and `query.commit` and `query.rollback`, on every
//! engine. Needs the Docker test servers (`cargo xtask testdb up`); tests that write make their
//! own tables in `scratch` and drop them at the end.

mod common;

use common::*;
use dbee_engine::cmd::query::NO_TRANSACTION;
use dbee_engine::{Json, Session};
use dbee_testkit::{Kind, TestTarget, require_testdb, unique};
use serde_json::json;

/// A new active query tab holding `text`.
fn tab(s: &mut Session, text: &str) -> u64 {
    run(s, "tab.newQuery", json!({ "text": text }))["tab"].as_u64().unwrap()
}

/// Runs `text` in the active query tab.
fn runs(s: &mut Session, text: &str) -> Json {
    run(s, "query.setText", json!({ "text": text }));
    run(s, "query.run", json!({}))
}

/// The active connection's transaction, as `inspect()` shows it.
fn transaction(s: &Session) -> Json {
    s.inspect()["connection"]["transaction"].clone()
}

fn open_in(tab: u64) -> Json {
    json!({ "state": "open", "tab": tab })
}

/// `scratch.<name>` on `t` with one row, `(1, 1)`, made before the session connects (so the
/// catalog has it), dropped with the guard; its qualified name.
fn one_row<'a>(t: &'a TestTarget, name: &str) -> (Scratch<'a>, String) {
    let guard = scratch_table(t, name, "CREATE TABLE {t} (id int PRIMARY KEY, v int); INSERT INTO {t} VALUES (1, 1);");
    (guard, format!("{}.{name}", t.schema("scratch")))
}

fn value(t: &TestTarget, table: &str) -> String {
    q(t, &format!("SELECT v FROM {table} WHERE id = 1"))
}

#[test]
fn begin_opens_a_transaction_owned_by_its_tab() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        assert_eq!(transaction(s), Json::Null, "{}: none before a run", t.name());
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        assert_eq!(transaction(s), open_in(a), "{}", t.name());
        // It belongs to the connection: every tab shows it, a new one too.
        tab(s, "select 1");
        assert_eq!(transaction(s), open_in(a), "{}", t.name());
        assert_eq!(s.inspect()["connections"][0]["transaction"], open_in(a), "{}", t.name());
        // Closing the tab that opened it ends nothing: it has no tab then.
        run(s, "tab.close", json!({ "tab": a, "force": true }));
        assert_eq!(transaction(s), json!({ "state": "open", "tab": null }), "{}", t.name());
        run(s, "query.rollback", json!({}));
        assert_eq!(transaction(s), Json::Null, "{}", t.name());
    });
}

#[test]
fn a_script_that_begins_reads_open() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (_drop, table) = one_row(&t, &unique("tx_script"));
        let mut s = connected_to(&t);
        let a = tab(&mut s, &format!("BEGIN; UPDATE {table} SET v = 2 WHERE id = 1;"));
        run(&mut s, "query.run", json!({}));
        assert_eq!(transaction(&s), open_in(a), "{}", t.name());
        run(&mut s, "query.rollback", json!({}));
        assert_eq!(transaction(&s), Json::Null, "{}", t.name());
        if matches!(t.kind(), Kind::MySql | Kind::MariaDb) {
            // Without autocommit the UPDATE opens it, with no BEGIN in sight.
            runs(&mut s, &format!("SET autocommit = 0; UPDATE {table} SET v = 3 WHERE id = 1"));
            assert_eq!(transaction(&s), open_in(a), "{}: autocommit off", t.name());
            run(&mut s, "query.rollback", json!({}));
            runs(&mut s, "SET autocommit = 1");
            assert_eq!(transaction(&s), Json::Null, "{}", t.name());
        }
        assert_eq!(value(&t, &table), "1", "{}: rolled back", t.name());
    }
}

#[test]
fn another_tabs_run_keeps_the_owner() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        let b = tab(s, "select 1");
        run(s, "query.run", json!({}));
        assert_eq!(transaction(s), open_in(a), "{}", t.name());
        // Another tab's commit ends it, whichever tab opened it.
        assert_eq!(run(s, "query.commit", json!({})), json!({ "tab": b, "sent": true, "transaction": null }), "{}", t.name());
        assert_eq!(transaction(s), Json::Null, "{}", t.name());
        // The next run that opens one makes its own tab the owner.
        runs(s, "BEGIN");
        assert_eq!(transaction(s), open_in(b), "{}", t.name());
        // A tab named in the params, not the active one.
        assert_eq!(run(s, "query.rollback", json!({ "tab": a }))["tab"], a, "{}", t.name());
        assert_eq!(transaction(s), Json::Null, "{}", t.name());
    });
}

#[test]
fn commit_and_rollback_end_it() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (_drop, table) = one_row(&t, &unique("tx_end"));
        let mut s = connected_to(&t);
        let a = tab(&mut s, &format!("BEGIN; UPDATE {table} SET v = 2 WHERE id = 1"));
        run(&mut s, "query.run", json!({}));
        assert_eq!(transaction(&s), open_in(a), "{}", t.name());
        assert_eq!(run(&mut s, "query.commit", json!({})), json!({ "tab": a, "sent": true, "transaction": null }), "{}", t.name());
        assert_eq!(value(&t, &table), "2", "{}: committed", t.name());
        runs(&mut s, &format!("BEGIN; UPDATE {table} SET v = 3 WHERE id = 1"));
        assert_eq!(transaction(&s), open_in(a), "{}", t.name());
        assert_eq!(run(&mut s, "query.rollback", json!({})), json!({ "tab": a, "sent": true, "transaction": null }), "{}", t.name());
        assert_eq!(value(&t, &table), "2", "{}: rolled back", t.name());
        assert_eq!(transaction(&s), Json::Null, "{}", t.name());
    }
}

#[test]
fn a_failed_postgres_transaction_reads_failed_and_rollback_ends_it() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| t.kind() == Kind::Postgres) {
        let mut s = connected_to(&t);
        let a = tab(&mut s, "BEGIN; SELECT 1/0");
        let e = s.run_blocking("query.run", json!({}), T).unwrap_err();
        assert_eq!(e.data()["serverCode"], "22012", "{}: {e}", t.name());
        let failed = json!({ "state": "failed", "tab": a });
        assert_eq!(transaction(&s), failed, "{}", t.name());
        run(&mut s, "query.setText", json!({ "text": "select 1" }));
        let e = s.run_blocking("query.run", json!({}), T).unwrap_err();
        assert_eq!(e.data()["serverCode"], "25P02", "{}: {e}", t.name());
        assert_eq!(transaction(&s), failed, "{}", t.name());
        assert_eq!(run(&mut s, "query.rollback", json!({})), json!({ "tab": a, "sent": true, "transaction": null }), "{}", t.name());
        assert_eq!(run(&mut s, "query.run", json!({}))["results"][0]["rows"], 1, "{}", t.name());
        assert_eq!(transaction(&s), Json::Null, "{}", t.name());
    }
}

#[test]
fn mysql_ddl_commits_and_the_state_says_so() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| matches!(t.kind(), Kind::MySql | Kind::MariaDb)) {
        let (mut guard, table) = one_row(&t, &unique("tx_ddl"));
        let made = format!("scratch.{}", unique("tx_ddl_made"));
        guard.1.push(format!("TABLE IF EXISTS {made}"));
        let mut s = connected_to(&t);
        let a = tab(&mut s, &format!("BEGIN; INSERT INTO {table} VALUES (2, 2)"));
        run(&mut s, "query.run", json!({}));
        assert_eq!(transaction(&s), open_in(a), "{}", t.name());
        runs(&mut s, &format!("BEGIN; INSERT INTO {table} VALUES (3, 3); CREATE TABLE {made} (id int)"));
        assert_eq!(transaction(&s), Json::Null, "{}: the CREATE committed it", t.name());
        assert_eq!(q(&t, &format!("SELECT COUNT(*) FROM {table}")), "3", "{}: the INSERTs are committed", t.name());
    }
}

#[test]
fn rollback_with_nothing_open_is_harmless() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let a = tab(s, "select 1");
        // Before any run, then after one: the query connection is not open, then idle.
        for ran in [false, true] {
            if ran {
                run(s, "query.run", json!({}));
            }
            for id in ["query.rollback", "query.commit"] {
                let r = run(s, id, json!({}));
                assert_eq!(r, json!({ "tab": a, "sent": false, "transaction": null, "note": NO_TRANSACTION }), "{} {id}", t.name());
            }
        }
        assert_eq!(run(s, "query.run", json!({}))["results"][0]["rows"], 1, "{}", t.name());
    });
}

#[test]
fn a_reconnect_clears_it() {
    let _db = require_testdb!();
    for_each_engine_exclusive(|t, s| {
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        assert_eq!(transaction(s), open_in(a), "{}", t.name());
        // The server drops the sessions; a file has none to drop.
        if t.server().is_some() {
            kill_dbee_connections(t, s);
        }
        run(s, "connection.reconnect", json!({}));
        assert_eq!(transaction(s), Json::Null, "{}", t.name());
        runs(s, "select 1");
        assert_eq!(transaction(s), Json::Null, "{}: a new session", t.name());
    });
}

#[test]
fn the_table_tabs_session_is_not_the_query_sessions() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let name = unique("tx_table");
        let (_drop, table) = one_row(&t, &name);
        let mut s = connected_to(&t);
        let a = tab(&mut s, "BEGIN");
        run(&mut s, "query.run", json!({}));
        // A table read, a count and an Apply run on the table tab's own session.
        open_table(&mut s, t.schema("scratch"), &name);
        run(&mut s, "table.count", json!({}));
        set_cell(&mut s, 0, "v", "5");
        apply(&mut s);
        assert_eq!(value(&t, &table), "5", "{}: Apply committed", t.name());
        assert_eq!(transaction(&s), open_in(a), "{}: the query session's, as it was", t.name());
        run(&mut s, "query.rollback", json!({ "tab": a }));
        // With the query session idle, it stays so.
        set_cell(&mut s, 0, "v", "6");
        apply(&mut s);
        read(&mut s);
        assert_eq!(value(&t, &table), "6", "{}", t.name());
        assert_eq!(transaction(&s), Json::Null, "{}", t.name());
    }
}
