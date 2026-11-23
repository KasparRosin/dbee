//! The query session's transaction (gaps 14 and 53): whether one is open, as each driver reports
//! it after a run, which tab's run opened it, `query.commit` and `query.rollback`, and the
//! question closing a tab or the connection asks when it would drop it, on every engine. Needs the
//! Docker test servers (`cargo xtask testdb up`); tests that write make their own tables in
//! `scratch` and drop them at the end.

mod common;

use common::*;
use dbee_engine::cmd::query::{FAILED_COMMIT, NO_TRANSACTION, ONLY_ROLLBACK};
use dbee_engine::{Json, Outcome, Session, Tab};
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

// ---- closing asks how to end it ---------------------------------------------------------------

/// The data of the `needsTransactionEnd` that `id` answers `params` with.
fn asks(s: &mut Session, id: &str, params: Json) -> Json {
    match s.execute(id, params) {
        Err(e) if e.code() == "needsTransactionEnd" => e.data(),
        other => panic!("{id}: expected needsTransactionEnd, got {other:?}"),
    }
}

/// Runs a close with `end`, which must start a job, and returns the job's result.
fn ends(s: &mut Session, id: &str, params: Json) -> Json {
    match s.execute(id, params) {
        Ok(Outcome::Started(job)) => s.wait(job, T).unwrap_or_else(|e| panic!("{id}: {e} [{}] {}", e.code(), e.data())),
        other => panic!("{id}: expected a job, got {other:?}"),
    }
}

fn count(t: &TestTarget, table: &str) -> String {
    q(t, &format!("SELECT COUNT(*) FROM {table}"))
}

fn tabs(s: &Session) -> usize {
    s.inspect()["tabs"].as_array().map_or(0, Vec::len)
}

/// A query of about 30 s whose text holds `marker`.
fn slow_with(t: &TestTarget, marker: &str) -> String {
    format!("{}, '{marker}' as tag", t.sleep_sql(30))
}

#[test]
fn closing_the_owner_tab_asks_to_end_the_transaction() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (_drop, table) = one_row(&t, &unique("tx_ask"));
        let mut s = connected_to(&t);
        let a = tab(&mut s, &format!("BEGIN; INSERT INTO {table} VALUES (2, 2)"));
        run(&mut s, "query.run", json!({}));
        // No text, so nothing else would be lost.
        run(&mut s, "query.setText", json!({ "text": "" }));
        let data = asks(&mut s, "tab.close", json!({}));
        assert_eq!(data, json!({ "state": "open", "about": "tab", "name": "Query 1", "lost": null, "running": null }), "{}", t.name());
        assert_eq!((tabs(&s), transaction(&s)), (1, open_in(a)), "{}: nothing closed, nothing ended", t.name());
        assert_eq!(count(&t, &table), "1", "{}", t.name());
        ends(&mut s, "tab.close", json!({ "end": "rollback" }));
    }
}

#[test]
fn closing_another_tab_does_not_ask() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        let b = tab(s, "");
        assert_eq!(run(s, "tab.close", json!({ "tab": b })), json!({ "closed": b, "active": a }), "{}", t.name());
        assert_eq!(transaction(s), open_in(a), "{}", t.name());
        run(s, "query.rollback", json!({}));
    });
}

#[test]
fn the_owner_with_text_asks_once() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        let e = s.execute("tab.close", json!({})).unwrap_err();
        assert_eq!(e.code(), "needsTransactionEnd", "{}: {e}", t.name());
        assert_eq!(e.data()["lost"], "Its query text will be lost.", "{}", t.name());
        assert!(e.to_string().starts_with("Close Query 1? ") && e.to_string().ends_with(" Its query text will be lost."), "{}: {e}", t.name());
        // The answer closes it: the text asks nothing more.
        let r = ends(s, "tab.close", json!({ "tab": a, "end": "rollback" }));
        assert_eq!(r, json!({ "closed": a, "active": null, "ended": "rollback" }), "{}", t.name());
        assert_eq!(transaction(s), Json::Null, "{}", t.name());
    });
}

/// `tab.close` with `end` ends the transaction on the query connection, then closes the tab.
fn end_then_close(end: &str, rows: &str) {
    for t in engine_targets() {
        let (_drop, table) = one_row(&t, &unique("tx_close"));
        let mut s = connected_to(&t);
        let a = tab(&mut s, &format!("BEGIN; INSERT INTO {table} VALUES (2, 2)"));
        run(&mut s, "query.run", json!({}));
        let r = ends(&mut s, "tab.close", json!({ "tab": a, "end": end }));
        assert_eq!(r, json!({ "closed": a, "active": null, "ended": end }), "{}", t.name());
        assert_eq!(tabs(&s), 0, "{}", t.name());
        assert_eq!(s.inspect()["connections"][0]["transaction"], Json::Null, "{}", t.name());
        assert_eq!(count(&t, &table), rows, "{}: another session after the {end}", t.name());
    }
}

#[test]
fn commit_then_close() {
    let _db = require_testdb!();
    end_then_close("commit", "2");
}

#[test]
fn rollback_then_close() {
    let _db = require_testdb!();
    end_then_close("rollback", "1");
}

#[test]
fn a_failed_commit_closes_nothing() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| t.kind() == Kind::Postgres) {
        let schema = t.schema("scratch");
        let (parent, child) = (format!("{schema}.{}", unique("tx_parent")), format!("{schema}.{}", unique("tx_child")));
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {child}"), format!("TABLE IF EXISTS {parent}")]);
        q(
            &t,
            &format!(
                "CREATE TABLE {parent} (id int PRIMARY KEY); \
                 CREATE TABLE {child} (id int PRIMARY KEY, parent int REFERENCES {parent} (id) DEFERRABLE INITIALLY DEFERRED);"
            ),
        );
        let mut s = connected_to(&t);
        // The key is checked at the COMMIT, which the server then refuses.
        let a = tab(&mut s, &format!("BEGIN; INSERT INTO {child} VALUES (1, 99)"));
        run(&mut s, "query.run", json!({}));
        assert_eq!(transaction(&s), open_in(a), "{}", t.name());
        let job = match s.execute("tab.close", json!({ "end": "commit" })) {
            Ok(Outcome::Started(job)) => job,
            other => panic!("{}: expected a job, got {other:?}", t.name()),
        };
        let e = s.wait(job, T).unwrap_err();
        assert_eq!((e.code(), &e.data()["serverCode"]), ("server", &json!("23503")), "{}: {e}", t.name());
        assert_eq!(tabs(&s), 1, "{}: the tab stays", t.name());
        assert_eq!(transaction(&s), Json::Null, "{}: the server ended it", t.name());
        assert_eq!(count(&t, &child), "0", "{}", t.name());
    }
}

#[test]
fn connection_close_asks_with_any_state() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let name = unique("tx_conn");
        let (_drop, _) = one_row(&t, &name);
        let mut s = connected_to(&t);
        let a = tab(&mut s, "BEGIN");
        run(&mut s, "query.run", json!({}));
        run(&mut s, "query.setText", json!({ "text": "" }));
        let open = json!({ "state": "open", "about": "connection", "name": t.name(), "lost": null, "running": null });
        assert_eq!(asks(&mut s, "connection.close", json!({})), open, "{}", t.name());
        // Text in another tab and a staged change: listed as `connection.close` words them.
        tab(&mut s, "select 1");
        open_table(&mut s, t.schema("scratch"), &name);
        set_cell(&mut s, 0, "v", "5");
        let lost = "The text of 1 query tab and the staged changes of 1 table tab will be lost.";
        assert_eq!(asks(&mut s, "connection.close", json!({}))["lost"], lost, "{}", t.name());
        // Without a transaction it is the sentence of the question about text and edits.
        run(&mut s, "query.rollback", json!({ "tab": a }));
        let e = s.execute("connection.close", json!({})).unwrap_err();
        assert_eq!((e.code(), e.to_string()), ("needsConfirm", format!("Disconnect from {}? {lost}", t.name())), "{}", t.name());
        // The answer ends it and closes the connection, its text and edits with it.
        run(&mut s, "tab.activate", json!({ "tab": a }));
        runs(&mut s, "BEGIN");
        let (id, connection) = (s.inspect()["connection"]["id"].clone(), s.inspect()["connection"]["connection"].clone());
        let r = ends(&mut s, "connection.close", json!({ "end": "rollback" }));
        assert_eq!(r, json!({ "closed": id, "connection": connection, "ended": "rollback" }), "{}", t.name());
        assert_eq!(s.inspect()["screen"], "connections", "{}", t.name());
    }
    // A failed Postgres transaction: its commit is the server's rollback, which the note says.
    for t in engine_targets().into_iter().filter(|t| t.kind() == Kind::Postgres) {
        let mut s = connected_to(&t);
        tab(&mut s, "BEGIN; SELECT 1/0");
        s.run_blocking("query.run", json!({}), T).unwrap_err();
        run(&mut s, "query.setText", json!({ "text": "" }));
        assert_eq!(asks(&mut s, "connection.close", json!({}))["state"], "failed", "{}", t.name());
        let on_tab = asks(&mut s, "tab.close", json!({}));
        assert_eq!((&on_tab["state"], &on_tab["about"]), (&json!("failed"), &json!("tab")), "{}", t.name());
        let (id, connection) = (s.inspect()["connection"]["id"].clone(), s.inspect()["connection"]["connection"].clone());
        let r = ends(&mut s, "connection.close", json!({ "end": "commit" }));
        assert_eq!(r, json!({ "closed": id, "connection": connection, "ended": "commit", "note": FAILED_COMMIT }), "{}", t.name());
    }
}

#[test]
fn an_end_with_nothing_open_still_closes() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        // Another tab commits it while the question would be open.
        let b = tab(s, "select 1");
        run(s, "query.commit", json!({}));
        let r = ends(s, "tab.close", json!({ "tab": a, "end": "commit" }));
        assert_eq!(r, json!({ "closed": a, "active": b, "ended": "commit", "note": NO_TRANSACTION }), "{}", t.name());
        assert_eq!(transaction(s), Json::Null, "{}", t.name());
    });
}

/// A statement still running in the transaction: the question says so and is asked from the
/// state the last finished run left. Only a rollback ends it then, which stops the run first.
#[test]
fn a_run_inside_the_transaction_allows_only_rollback() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (_drop, table) = one_row(&t, &unique("tx_running"));
        let mut s = connected_to(&t);
        let a = tab(&mut s, &format!("BEGIN; INSERT INTO {table} VALUES (2, 2)"));
        run(&mut s, "query.run", json!({}));
        let marker = unique("dbee_marker");
        run(&mut s, "query.setText", json!({ "text": slow_with(&t, &marker) }));
        let Ok(Outcome::Started(slow)) = s.execute("query.run", json!({})) else { panic!("{}: the slow run did not start", t.name()) };
        if t.lists_running_statements() {
            until_running(&t, &marker);
        }
        let data = asks(&mut s, "tab.close", json!({}));
        let want = json!({
            "state": "open", "about": "tab", "name": "Query 1",
            "lost": "Its query text will be lost.", "running": { "title": "Query 1", "count": 1 },
        });
        assert_eq!(data, want, "{}", t.name());
        let e = s.execute("tab.close", json!({ "end": "commit" })).unwrap_err();
        assert!(e.code() == "badParams" && e.to_string().contains(ONLY_ROLLBACK), "{}: {e}", t.name());
        assert!(s.inspect()["jobs"].as_array().unwrap().iter().any(|j| j["job"] == json!(slow.0)), "{}: still running", t.name());
        let r = ends(&mut s, "tab.close", json!({ "end": "rollback" }));
        assert_eq!(r, json!({ "closed": a, "active": null, "ended": "rollback" }), "{}", t.name());
        // The stopped run's result came in first, so the rollback's state is the last word.
        let stopped = s.pump();
        assert!(matches!(stopped.as_slice(), [(job, Err(e))] if *job == slow && e.code() == "cancelled"), "{}: {stopped:?}", t.name());
        assert_eq!(transaction(&s), Json::Null, "{}", t.name());
        assert_eq!(count(&t, &table), "1", "{}: rolled back", t.name());
    }
}

#[test]
fn force_still_closes_without_asking() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let a = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        assert_eq!(run(s, "tab.close", json!({ "tab": a, "force": true })), json!({ "closed": a, "active": null }), "{}", t.name());
        assert_eq!(s.inspect()["connections"][0]["transaction"], json!({ "state": "open", "tab": null }), "{}", t.name());
        // It has no tab now, so no tab's close asks; the connection's does.
        let b = tab(s, "");
        assert_eq!(run(s, "tab.close", json!({ "tab": b })), json!({ "closed": b, "active": null }), "{}", t.name());
        run(s, "connection.close", json!({ "force": true }));
        assert_eq!(s.inspect()["screen"], "connections", "{}", t.name());
    });
}

/// The query connection was lost while the question was open: the commit cannot happen, so it
/// fails and nothing closes; a rollback has nothing left to do and closes.
#[test]
fn a_lost_query_connection_at_commit_closes_nothing() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // A file cannot lose its connection.
        if t.is_file() {
            return;
        }
        let a = tab(s, t.kind().info().connection_id_sql);
        run(s, "query.run", json!({}));
        let Some(Tab::Query(q)) = s.state().active_tab() else { panic!("a query tab is active") };
        let id = q.shown_result().and_then(|o| o.result.rows.first()?.first()?.clone()).unwrap();
        runs(s, "BEGIN");
        t.kill_connection(&id).unwrap();
        let job = match s.execute("tab.close", json!({ "tab": a, "end": "commit" })) {
            Ok(Outcome::Started(job)) => job,
            other => panic!("{}: expected a job, got {other:?}", t.name()),
        };
        let e = s.wait(job, T).unwrap_err();
        assert_eq!(e.code(), "connectionLost", "{}: {e}", t.name());
        assert_eq!(tabs(s), 1, "{}: the tab stays", t.name());
        let r = ends(s, "tab.close", json!({ "tab": a, "end": "rollback" }));
        assert_eq!((&r["closed"], &r["ended"]), (&json!(a), &json!("rollback")), "{}: {r}", t.name());
    });
}
