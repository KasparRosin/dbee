//! Query tabs: run all and run current, one result per statement, errors and the statement they
//! name, the result cap, cancel, a tab's database, and what closing the tab, disconnecting or
//! reconnecting does to a running query, on every engine ([`for_each_engine`]). Needs the Docker
//! test servers (`cargo xtask testdb up`); tests that write make their own tables in `scratch`
//! and drop them at the end.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_engine::cmd::query::QUERY_SESSION_LOST;
use dbee_engine::{EngineError, JobId, Json, Meaning, Outcome, Session, Tab, Which};
use dbee_testkit::{ORDERS_COUNT, TestTarget, require_testdb, unique};
use serde_json::json;

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
fn run_fails(s: &mut Session, params: Json) -> EngineError {
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

/// The first value of the active query tab's shown result.
fn first_value(s: &Session) -> Option<String> {
    let Some(Tab::Query(t)) = s.state().active_tab() else { panic!("a query tab is active") };
    t.shown_result()?.result.rows.first()?.first()?.clone()
}

#[test]
fn run_all_gives_one_result_per_statement() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let text = "select 1 as a; select 2 as b;\nselect 3 as c";
        tab(s, text);
        let r = run(s, "query.run", json!({}));
        let results = r["results"].as_array().unwrap();
        assert_eq!(results.len(), 3, "{r}");
        assert_eq!(r["shown"], 2);
        assert_eq!(names(&results[2]), ["c"]);
        let typed = (&results[2]["columns"][0]["class"], &results[2]["columns"][0]["baseType"]);
        let info = t.kind().info();
        assert_eq!(typed, (&json!(info.int_literal_class), &json!(info.int_literal_type)), "{}: a single statement's columns are typed", t.name());
        let b = text.find("select 2").unwrap();
        assert_eq!((results[1]["start"].as_u64(), results[1]["end"].as_u64()), (Some(b as u64), Some(b as u64 + 13)), "{}", results[1]);
        // The tab keeps them all; the state holds the rows.
        let i = shown(s);
        assert_eq!((i["results"].as_array().map(Vec::len), &i["shown"], &i["error"], &i["running"]), (Some(3), &json!(2), &Json::Null, &Json::Null));
        let Some(Tab::Query(qt)) = s.state().active_tab() else { panic!("a query tab is active") };
        assert_eq!(qt.results.len(), 3);
        assert_eq!(qt.results[0].result.rows, vec![vec![Some("1".to_owned())]]);
        assert_eq!(qt.results[2].range, text.find("select 3").unwrap()..text.len());
        assert_eq!((qt.shown, qt.error.clone()), (2, None));
        // Another result can be shown; one that is not there cannot.
        assert_eq!(run(s, "query.showResult", json!({ "index": 0 }))["shown"], 0);
        assert_eq!(shown(s)["shown"], 0);
        for bad in [json!({ "index": 3 }), json!({ "index": -1 }), json!({})] {
            assert_eq!(s.execute("query.showResult", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
        }
    });
}

#[test]
fn run_current_runs_statement_under_cursor() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        let text = "select 1 as a;\nselect 2 as b;\nselect 3 as c";
        tab(s, text);
        let cursor = text.find("2 as b").unwrap();
        let r = run(s, "query.run", json!({ "current": true, "cursor": cursor }));
        let results = r["results"].as_array().unwrap();
        assert_eq!(results.len(), 1, "{r}");
        assert_eq!(names(&results[0]), ["b"]);
        assert_eq!(r["shown"], 0);
        // At the end is the last statement; `current` needs the cursor, inside the text and on a
        // character boundary.
        let r = run(s, "query.run", json!({ "current": true, "cursor": text.len() }));
        assert_eq!(names(&r["results"][0]), ["c"]);
        assert_eq!(s.execute("query.run", json!({ "current": true })).unwrap_err().code(), "badParams");
        for cursor in [text.len() + 1, 999, usize::MAX] {
            assert_eq!(s.execute("query.run", json!({ "current": true, "cursor": cursor })).unwrap_err().code(), "badParams", "{cursor}");
        }
        run(s, "query.setText", json!({ "text": "select 'é' as e" }));
        let inside = "select 'é".len() - 1;
        let e = s.execute("query.run", json!({ "current": true, "cursor": inside })).unwrap_err();
        assert_eq!(e.code(), "badParams", "{e}");
        assert_eq!(names(&run(s, "query.run", json!({ "current": true, "cursor": inside + 1 }))["results"][0]), ["e"]);
        // Nothing to run: blank text or only comments.
        tab(s, "  -- just a comment\n ; ");
        assert_eq!(s.execute("query.run", json!({})).unwrap_err().code(), "disabled");
    });
}

#[test]
fn an_error_names_its_statement_and_keeps_the_results_before_it() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        tab(s, "select 1;\nselect nope");
        let e = run_fails(s, json!({}));
        assert_eq!(e.code(), "server", "{e}");
        assert_eq!(e.data()["statement"].as_u64(), Some(1), "{}", e.data());
        assert!(e.data()["serverCode"].as_str().is_some_and(|c| !c.is_empty()), "{}: {}", t.name(), e.data());
        let i = shown(s);
        assert_eq!(i["error"]["statement"].as_u64(), Some(1), "{i}");
        assert!(i["error"]["message"].as_str().unwrap().contains("nope"), "{i}");
        assert_eq!(i["error"]["code"], "server");
        assert_eq!(i["error"]["serverCode"], e.data()["serverCode"]);
        // The statement before the error kept its result.
        assert_eq!((i["results"].as_array().map(Vec::len), &i["shown"]), (Some(1), &json!(0)), "{i}");
        // A good run clears the error.
        run(s, "query.setText", json!({ "text": "select 1" }));
        run(s, "query.run", json!({}));
        assert_eq!(shown(s)["error"], Json::Null);
    });
}

/// Postgres says where in the statement the error is (MySQL and MariaDB give no position).
#[test]
fn error_position_maps_into_tab_text() {
    let _db = require_testdb!();
    let mut s = connected();
    let text = "select 1;\nselect nope";
    tab(&mut s, text);
    let e = run_fails(&mut s, json!({}));
    assert_eq!(e.code(), "server", "{e}");
    let at = text.find("nope").unwrap();
    assert_eq!((e.data()["position"].as_u64(), e.data()["statement"].as_u64()), (Some(at as u64), Some(1)), "{}", e.data());
    assert_eq!(e.data()["serverCode"], "42703");
    let i = shown(&s);
    assert_eq!((i["error"]["position"].as_u64(), i["error"]["statement"].as_u64()), (Some(at as u64), Some(1)), "{i}");
}

/// Postgres: a failed statement aborts the transaction (MySQL and MariaDB go on in it).
#[test]
fn a_failed_transaction_is_flagged() {
    let _db = require_testdb!();
    let mut s = connected();
    tab(&mut s, "BEGIN; SELECT 1 / 0");
    run_fails(&mut s, json!({}));
    let error = |s: &Session| match s.state().active_tab() {
        Some(Tab::Query(q)) => q.error.clone().unwrap(),
        _ => panic!("a query tab is active"),
    };
    let e = error(&s);
    assert_eq!((e.server_code.as_deref(), e.meaning), (Some("22012"), None), "{e:?}");
    assert_eq!(shown(&s)["error"]["meaning"], Json::Null);
    // The transaction stays open and failed: every statement is refused until a ROLLBACK.
    run(&mut s, "query.setText", json!({ "text": "SELECT 1" }));
    let failed = run_fails(&mut s, json!({}));
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

/// Postgres's error positions count characters.
#[test]
fn error_position_counts_characters_before_it() {
    let _db = require_testdb!();
    let mut s = connected();
    // Multibyte text before and inside the failing statement, and a comment before it.
    let text = "select '🐝ü' as x;\n-- naïve 🐝\nselect 'é', nope";
    tab(&mut s, text);
    let e = run_fails(&mut s, json!({}));
    assert_eq!(e.data()["position"].as_u64(), Some(text.find("nope").unwrap() as u64), "{}", e.data());
    // An error at the end of the input points at the statement's end.
    tab(&mut s, "select 1; select 'ü' ||");
    let e = run_fails(&mut s, json!({}));
    assert_eq!((e.code(), e.data()["position"].as_u64()), ("server", Some("select 1; select 'ü' ||".len() as u64)), "{e} {}", e.data());
}

#[test]
fn results_capped_with_count() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        tab(s, &format!("select id as n from {}.big_rows where id <= 25000 order by id", t.schema("shop")));
        let r = run(s, "query.run", json!({}));
        let result = &r["results"][0];
        assert_eq!(
            (result["rows"].as_u64(), result["rowsSeen"].as_u64(), result["complete"].as_bool()),
            (Some(10_000), Some(25_000), Some(false)),
            "{}: {r}",
            t.name()
        );
        let Some(Tab::Query(qt)) = s.state().active_tab() else { panic!("a query tab is active") };
        assert_eq!(qt.results[0].result.rows.len(), 10_000);
        assert_eq!(qt.results[0].result.rows.last(), Some(&vec![Some("10000".to_owned())]));
    });
}

#[test]
fn statements_without_rows_report_rows_affected() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let (sc, name) = (t.schema("scratch"), unique("t_query"));
        let _drop = Scratch(t, vec![format!("TABLE IF EXISTS {sc}.{name}")]);
        tab(s, &format!("create table {sc}.{name} (id int);\ninsert into {sc}.{name} values (1), (2), (3);\nselect count(*) from {sc}.{name}"));
        let r = run(s, "query.run", json!({}));
        let results = r["results"].as_array().unwrap();
        assert_eq!(results.len(), 3, "{r}");
        assert_eq!((results[1]["rowsAffected"].as_u64(), results[1]["columns"].as_array().map(Vec::len)), (Some(3), Some(0)), "{r}");
        // Auto-commit: every statement is written at once.
        assert_eq!(q(t, &format!("SELECT count(*) FROM {sc}.{name}")), "3");
    });
}

/// The server no longer runs a statement whose text holds `marker` (a file lists none).
fn stopped_running(t: &TestTarget, marker: &str) {
    if t.lists_running_statements() {
        assert_eq!(q(t, &t.running_with_sql(marker)), "0", "{}", t.name());
    }
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn cancel_stops_a_long_query_within_five_seconds() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let m = marker();
        tab(s, &slow_with(t, &m));
        let job = start(s, json!({}));
        until_running(t, &m);
        let started = Instant::now();
        assert_eq!(run(s, "query.cancel", json!({}))["sent"], true);
        assert_eq!(s.wait(job, T).unwrap_err().code(), "cancelled", "{}", t.name());
        let took = started.elapsed();
        println!("{}: cancelled in {took:?}", t.name());
        assert!(took < Duration::from_secs(5), "{}: {took:?}", t.name());
    });
}

#[test]
fn cancel_stops_a_long_query() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let m = marker();
        tab(s, &slow_with(t, &m));
        let job = start(s, json!({}));
        assert_eq!(shown(s)["running"], json!(job.0));
        until_running(t, &m);
        assert_eq!(run(s, "query.cancel", json!({}))["sent"], true);
        let e = s.wait(job, T).unwrap_err();
        assert_eq!(e.code(), "cancelled", "{}: {e}", t.name());
        let i = shown(s);
        assert_eq!((&i["running"], &i["error"]["code"]), (&Json::Null, &json!("cancelled")), "{i}");
        // Nothing runs now; the connection takes the next run.
        assert_eq!(run(s, "query.cancel", json!({}))["sent"], false);
        run(s, "query.setText", json!({ "text": "select 1 as one" }));
        assert_eq!(names(&run(s, "query.run", json!({}))["results"][0]), ["one"]);
    });
}

/// A statement that cannot end by itself while `held` holds its lock (on a server); on a file, a
/// count of about 30 s, as a file has no other session to hold a lock.
fn held_statement(t: &TestTarget, key: &str) -> (String, Option<dbee_testkit::Held>) {
    let shop = t.schema("shop");
    match t.server() {
        Some(_) => (format!("SELECT 1 AS one FROM {shop}.orders WHERE {} LIMIT 1", t.waits_for_lock_sql(key)), Some(t.hold_lock(key).unwrap())),
        None => (format!("SELECT 1 AS one FROM {shop}.orders WHERE {} LIMIT 1", t.slow_condition_sql(30)), None),
    }
}

/// A cancel sent before the statement reaches the server (or SQLite starts it) stops nothing, so
/// the run sends it again until the statement has ended: stopped right after it starts, the run
/// ends cancelled, while the lock that holds its statement is still held. A second Cancel sends
/// nothing itself and says the run is stopping.
#[test]
fn a_run_stopped_before_its_statement_starts_ends_cancelled() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // The query connection is open: the next run goes straight to its statement.
        tab(s, "select 1");
        run(s, "query.run", json!({}));
        let key = unique("dbee_gate");
        let (sql, held) = held_statement(t, &key);
        run(s, "query.setText", json!({ "text": sql }));
        for _ in 0..5 {
            let job = start(s, json!({}));
            assert_eq!(run(s, "query.cancel", json!({})), json!({ "tab": shown(s)["tab"], "sent": true }));
            assert_eq!(run(s, "query.cancel", json!({})), json!({ "tab": shown(s)["tab"], "sent": false, "stopping": true }));
            let e = s.wait(job, Duration::from_secs(60)).unwrap_err();
            assert_eq!(e.code(), "cancelled", "{}: {e}", t.name());
        }
        drop(held);
        run(s, "query.setText", json!({ "text": "select 2 as two" }));
        assert_eq!(names(&run(s, "query.run", json!({}))["results"][0]), ["two"], "the connection takes the next run");
    });
}

#[test]
fn cancel_stops_the_statements_after_it() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let (sc, name) = (t.schema("scratch"), unique("t_query"));
        let _drop = scratch_table(t, &name, "CREATE TABLE {t} (id int)");
        let m = marker();
        tab(s, &format!("select 1; {}; insert into {sc}.{name} values (1)", slow_with(t, &m)));
        let job = start(s, json!({}));
        until_running(t, &m);
        run(s, "query.cancel", json!({}));
        let e = s.wait(job, T).unwrap_err();
        assert_eq!((e.code(), e.data()["statement"].as_u64()), ("cancelled", Some(1)), "{}: {e} {}", t.name(), e.data());
        assert_eq!(shown(s)["results"].as_array().map(Vec::len), Some(1));
        assert_eq!(q(t, &format!("SELECT count(*) FROM {sc}.{name}")), "0", "the run stopped at the cancelled statement");
    });
}

#[test]
fn second_run_while_running_is_busy() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let first = tab(s, &t.sleep_sql(30));
        let job = start(s, json!({}));
        let e = s.execute("query.run", json!({})).unwrap_err();
        assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: a query is running in Query 1"));
        // Another tab waits too: the query connection runs one statement at a time.
        tab(s, "select 1");
        assert_eq!(s.execute("query.run", json!({})).unwrap_err().code(), "busy");
        assert_eq!(run(s, "query.cancel", json!({}))["sent"], false, "the other tab's run is not this tab's to cancel");
        run(s, "tab.activate", json!({ "tab": first }));
        run(s, "query.cancel", json!({}));
        assert_eq!(s.wait(job, T).unwrap_err().code(), "cancelled", "{}", t.name());
    });
}

#[test]
fn query_connection_is_separate_from_browse() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (sc, name) = (t.schema("scratch"), unique("t_query"));
        let _drop = scratch_table(&t, &name, "CREATE TABLE {t} (id int PRIMARY KEY, name text); INSERT INTO {t} VALUES (1, 'a'), (2, 'b')");
        let mut s = connected_to(&t);
        // A transaction left open in the query tab, with a row written in it.
        let query = tab(&mut s, &format!("BEGIN;\ninsert into {sc}.{name} values (99, 'open')"));
        run(&mut s, "query.run", json!({}));
        // The table tab works on the browse connection; where two sessions can write at once (a
        // SQLite file has one writer), its Apply commits beside the open transaction.
        open_table(&mut s, sc, &name);
        assert_eq!(run(&mut s, "table.page", json!({ "to": "next" }))["rows"], 2);
        let first = if t.kind().info().parallel_writes {
            set_cell(&mut s, 0, "name", "applied");
            assert_eq!(apply(&mut s)["written"], 1);
            "applied"
        } else {
            "a"
        };
        assert_eq!(q(&t, &format!("SELECT CONCAT(id, ':', name) FROM {sc}.{name} ORDER BY id")), format!("1:{first}\n2:b"), "{}", t.name());
        // The query tab's transaction is still open: its row is not there for others.
        run(&mut s, "tab.activate", json!({ "tab": query }));
        run(&mut s, "query.setText", json!({ "text": format!("select count(*) from {sc}.{name}") }));
        assert_eq!(run(&mut s, "query.run", json!({}))["results"][0]["rows"], 1);
        assert_eq!(first_value(&s).as_deref(), Some("3"), "the query connection sees its own row");
        run(&mut s, "query.setText", json!({ "text": "ROLLBACK" }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name} WHERE id = 99")), "0");
    }
}

#[test]
fn read_only_refuses_insert() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (sc, name) = (t.schema("scratch"), unique("t_query_ro"));
        let _drop = scratch_table(&t, &name, "CREATE TABLE {t} (id int)");
        let mut s = session();
        let config = dbee_config::ConnectionConfig { read_only: true, ..config_for(&t) };
        open(&mut s, json!({ "config": config, "password": pw() }));
        tab(&mut s, &format!("insert into {sc}.{name} values (1)"));
        let e = run_fails(&mut s, json!({}));
        assert_eq!(e.code(), "readOnly", "{}: {e}", t.name());
        assert_eq!(shown(&s)["error"]["code"], "readOnly");
        let (sql, on) = t.kind().info().read_only_sql;
        run(&mut s, "query.setText", json!({ "text": sql }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s).as_deref(), Some(on), "{}", t.name());
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name}")), "0");
    }
}

/// MySQL and MariaDB run what an executable comment (`/*! … */`) holds, so the editor shows it as
/// code and the statement sent keeps it, also at its end: `WHERE id = 1 /*! OR 1 = 1 */` deletes
/// every row, as the editor shows (measured on all five servers).
#[test]
fn mysql_executable_comments_run_as_the_editor_shows_them() {
    use dbee_sql::Dialect;
    use dbee_sql::lexer::{TokenKind, tokenize};
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| matches!(t.kind().id(), "mysql" | "mariadb")) {
        let (sc, name) = (t.schema("scratch"), unique("t_exec_comment"));
        let _drop = scratch_table(&t, &name, "CREATE TABLE {t} (id int)");
        q(&t, &format!("INSERT INTO {sc}.{name} VALUES (1), (2), (3)"));
        let text = format!("DELETE FROM {sc}.{name} WHERE id = 1 /*! OR 1 = 1 */");
        let clause = text.find("OR 1 = 1").unwrap();
        let comments = tokenize(Dialect::MySql, &text).into_iter().filter(|k| k.kind == TokenKind::Comment).collect::<Vec<_>>();
        assert!(comments.iter().all(|k| !k.range.contains(&clause)), "{}: the editor shows the clause as a comment", t.name());
        let mut s = connected_to(&t);
        tab(&mut s, &text);
        run(&mut s, "query.run", json!({}));
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name}")), "0", "{}: the server ran the clause the editor shows", t.name());
    }
}

/// A query tab of a new read-only connection to `t`.
fn read_only_session(t: &TestTarget) -> TestSession {
    let mut s = session();
    let config = dbee_config::ConnectionConfig { read_only: true, ..config_for(t) };
    open(&mut s, json!({ "config": config, "password": pw() }));
    tab(&mut s, "");
    s
}

/// `text` run in the active query tab.
fn run_text(s: &mut Session, text: &str) -> Result<Json, EngineError> {
    run(s, "query.setText", json!({ "text": text }));
    s.run_blocking("query.run", json!({}), T)
}

/// On a read-only Postgres connection the statements that can end read-only, or write in spite of
/// it, are refused before they are sent, a `pg_dumpall` script's first line among them; one that
/// turned it off all the same (a `set_config` whose setting dbee cannot read) is caught after it
/// ran: rolled back, read-only turned on again, and said. Nothing is written.
#[test]
fn read_only_postgres_refuses_what_turns_it_off_and_reads_it_back() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| t.kind().id() == "postgres") {
        let (sc, name) = (t.schema("scratch"), unique("t_ro_pg"));
        let _drop = scratch_table(&t, &name, "CREATE TABLE {t} (id int)");
        let objects = q(&t, "SELECT count(*) FROM pg_largeobject_metadata");
        let mut s = read_only_session(&t);
        let insert = format!("INSERT INTO {sc}.{name} VALUES (1)");
        let refused = [
            format!("DO $$ BEGIN COMMIT; SET TRANSACTION READ WRITE; {insert}; END $$"),
            "CALL anything()".to_owned(),
            "SET default_transaction_read_only = off".to_owned(),
            "/* pasted */ set \"transaction_read_only\" TO off".to_owned(),
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE".to_owned(),
            "BEGIN READ WRITE".to_owned(),
            "START TRANSACTION ISOLATION LEVEL SERIALIZABLE, READ WRITE".to_owned(),
            "RESET ALL".to_owned(),
            "COMMIT AND CHAIN".to_owned(),
            "SELECT set_config('default_transaction_read_only', 'off', false)".to_owned(),
            "SELECT lo_from_bytea(0, 'dbee')".to_owned(),
            r#"SELECT U&"lo_from_byte\0061"(0, 'dbee')"#.to_owned(),
            r#"SET U&"default_transaction_read_onl!0079" UESCAPE '!' = off"#.to_owned(),
            "ALTER SYSTEM RESET work_mem".to_owned(),
            "COPY (SELECT 1) TO PROGRAM 'cat > /dev/null'".to_owned(),
            r#"COPY (SELECT "to" stdout FROM (VALUES (1)) AS v("to")) TO PROGRAM 'cat > /dev/null'"#.to_owned(),
            r#"COPY (WITH "to" AS (SELECT 1 AS x) SELECT * FROM "to" stdout) TO PROGRAM 'cat > /dev/null'"#.to_owned(),
        ];
        for sql in &refused {
            let e = run_text(&mut s, sql).err().unwrap_or_else(|| panic!("{}: ran {sql}", t.name()));
            assert_eq!(e.code(), "readOnly", "{}: {sql}: {e}", t.name());
        }
        let e = run_text(&mut s, "SELECT set_config('default_transaction_re' || 'ad_only', 'off', false)").expect_err("caught after it ran");
        assert!(e.code() == "readOnly" && e.to_string().contains("turned read-only on again"), "{}: {e}", t.name());
        assert_eq!(run_text(&mut s, &insert).err().map(|e| e.code()), Some("readOnly"), "{}", t.name());
        run_text(&mut s, "SELECT current_setting('default_transaction_read_only') || '/' || current_setting('transaction_read_only')").unwrap();
        assert_eq!(first_value(&s).as_deref(), Some("on/on"), "{}", t.name());
        // Reading the mode back takes no snapshot: a transaction can still set its isolation level.
        for sql in ["BEGIN", "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ", "SELECT 1", "COMMIT"] {
            run_text(&mut s, sql).unwrap_or_else(|e| panic!("{}: {sql}: {e}", t.name()));
        }
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name}")), "0", "{}", t.name());
        assert_eq!(q(&t, "SELECT count(*) FROM pg_largeobject_metadata"), objects, "{}: no large object", t.name());
    }
}

/// On a read-only MySQL or MariaDB connection the statements that can end read-only, or hide what
/// they run, are refused before they are sent, also inside an executable comment; one that turned
/// it off all the same (a function that sets the variable) is caught after it ran. A statement's
/// warnings stay for `SHOW WARNINGS`: the check waits for the statement after it. Nothing is
/// written.
#[test]
fn read_only_mysql_refuses_what_turns_it_off_and_reads_it_back() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| matches!(t.kind().id(), "mysql" | "mariadb")) {
        let variable = if t.kind().id() == "mariadb" { "tx_read_only" } else { "transaction_read_only" };
        let (sc, name, off) = (t.schema("scratch"), unique("t_ro_my"), unique("f_ro_off"));
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {sc}.{name}"), format!("FUNCTION IF EXISTS {sc}.{off}")]);
        q(&t, &format!("CREATE TABLE {sc}.{name} (id int)"));
        let mut w = connected_to(&t);
        tab(&mut w, &format!("CREATE FUNCTION {sc}.{off}() RETURNS INT DETERMINISTIC BEGIN SET SESSION {variable} = 0; RETURN 1; END"));
        run(&mut w, "query.run", json!({}));
        drop(w);
        let mut s = read_only_session(&t);
        let insert = format!("INSERT INTO {sc}.{name} VALUES (1)");
        let refused = [
            "START TRANSACTION READ WRITE".to_owned(),
            "/*!40101 START TRANSACTION READ WRITE */".to_owned(),
            "SET SESSION TRANSACTION READ WRITE".to_owned(),
            "SET TRANSACTION ISOLATION LEVEL READ COMMITTED, READ WRITE".to_owned(),
            format!("SET @@SESSION.`{variable}` = 0"),
            format!("SET STATEMENT {variable}=0 FOR {insert}"),
            "CALL anything()".to_owned(),
            "PREPARE p FROM 'START TRANSACTION READ WRITE'".to_owned(),
            "EXECUTE p".to_owned(),
            "COMMIT AND CHAIN".to_owned(),
            "SET GLOBAL max_connections = @@GLOBAL.max_connections".to_owned(),
            "SELECT 1 INTO OUTFILE '/var/lib/mysql-files/dbee-never-written'".to_owned(),
            format!("SET @@{variable} = 0"),
            format!("SET @@{variable} = 1 - 1"),
            format!("SET @@{variable} = TRUE - 1"),
            format!("SET SESSION session_track_system_variables = '{variable}'"),
        ];
        for sql in &refused {
            let e = run_text(&mut s, sql).err().unwrap_or_else(|| panic!("{}: ran {sql}", t.name()));
            assert_eq!(e.code(), "readOnly", "{}: {sql}: {e}", t.name());
        }
        // Behind a version comment a server skips (each of these is skipped by some of the five), the
        // same forms are refused, or are a syntax error where the server runs the comment; the
        // insert after each is refused all the same.
        for gate in [
            "/*M!999999 SELECT */",
            "/*M! SELECT */",
            "/*!99999 SELECT */",
            "/*!99999 /* */ SELECT */",
            "/*!999999 /* */ SELECT */",
            "/*M!999999 /* */ SELECT */",
            "/*!99999 /*!50000 x */ SELECT */",
        ] {
            for form in [
                "START TRANSACTION READ WRITE".to_owned(),
                format!("SET @@{variable} = 0"),
                format!("SET STATEMENT {variable}=0 FOR {insert}"),
                "CALL anything()".to_owned(),
                "PREPARE p FROM 'START TRANSACTION READ WRITE'".to_owned(),
                "COMMIT AND CHAIN".to_owned(),
            ] {
                let sql = format!("{gate} {form}");
                let e = run_text(&mut s, &sql).err().unwrap_or_else(|| panic!("{}: ran {sql}", t.name()));
                assert!(matches!(e.code(), "readOnly" | "server"), "{}: {sql}: {e}", t.name());
                assert_eq!(run_text(&mut s, &insert).err().map(|e| e.code()), Some("readOnly"), "{}: after {sql}", t.name());
            }
        }
        // With backslash escapes off, as the server then reads strings: `'\'` is a whole string.
        run_text(&mut s, "SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')").unwrap();
        let hidden = format!(r"SET @a = '\', @@{variable} = 0, @b = '\'");
        let e = run_text(&mut s, &hidden).expect_err("refused");
        assert!(e.code() == "readOnly" && e.to_string().contains("changes read-only mode"), "{}: {hidden}: {e}", t.name());
        assert_eq!(run_text(&mut s, &insert).err().map(|e| e.code()), Some("readOnly"), "{}: after {hidden}", t.name());
        run_text(&mut s, "SET SESSION sql_mode = DEFAULT").unwrap();
        let e = run_text(&mut s, &format!("SELECT {sc}.{off}()")).expect_err("caught after it ran");
        assert!(e.code() == "readOnly" && e.to_string().contains("turned read-only on again"), "{}: {e}", t.name());
        assert_eq!(run_text(&mut s, &insert).err().map(|e| e.code()), Some("readOnly"), "{}", t.name());
        let (sql, on) = t.kind().info().read_only_sql;
        run_text(&mut s, sql).unwrap();
        assert_eq!(first_value(&s).as_deref(), Some(on), "{}", t.name());
        run_text(&mut s, "SELECT CAST('1x' AS SIGNED)").unwrap();
        run_text(&mut s, "SHOW WARNINGS").unwrap();
        let Some(Tab::Query(qt)) = s.state().active_tab() else { panic!("a query tab is active") };
        assert_eq!(qt.shown_result().map(|r| r.result.rows.len()), Some(1), "{}: the cast's warning", t.name());
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name}")), "0", "{}", t.name());
    }
}

/// After a statement that turned read-only off and then failed (an error carries no report), the
/// mode is read before the next statement that is not a diagnostic one; a version comment the
/// server skips cannot make an `INSERT` look like `SHOW WARNINGS` to dbee: it reads the statement
/// as the server will.
#[test]
fn read_only_mysql_reads_a_skipped_version_comment_as_the_server_does() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| matches!(t.kind().id(), "mysql" | "mariadb")) {
        let variable = if t.kind().id() == "mariadb" { "tx_read_only" } else { "transaction_read_only" };
        let (sc, name, off) = (t.schema("scratch"), unique("t_ro_gate"), unique("f_ro_gate"));
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {sc}.{name}"), format!("FUNCTION IF EXISTS {sc}.{off}")]);
        q(&t, &format!("CREATE TABLE {sc}.{name} (id int)"));
        let mut w = connected_to(&t);
        tab(&mut w, &format!("CREATE FUNCTION {sc}.{off}() RETURNS INT DETERMINISTIC BEGIN SET SESSION {variable} = 0; RETURN 1; END"));
        run(&mut w, "query.run", json!({}));
        drop(w);
        let mut s = read_only_session(&t);
        run_text(&mut s, &format!("SELECT {sc}.{off}(), (SELECT 1 UNION SELECT 2)")).expect_err("more than one row");
        let e = run_text(&mut s, &format!("/*M!999999 SHOW WARNINGS */ INSERT INTO {sc}.{name} VALUES (1)")).expect_err("not run");
        assert!(e.code() == "readOnly" && e.to_string().contains("did not run this statement"), "{}: {e}", t.name());
        run_text(&mut s, "SELECT 1").unwrap();
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{name}")), "0", "{}", t.name());
    }
}

/// Every engine's read-only refuses, or catches after it ran, each statement its test kind lists as
/// turning read-only off ([`KindInfo::read_only_breakers`](dbee_testkit::KindInfo)), and catches a
/// function that turns it off unseen; nothing is written and the mode still reads read-only. It
/// runs on every engine, a new one too, whose kind must list both.
#[test]
fn read_only_holds_on_every_engine() {
    use dbee_testkit::Unseen;
    let _db = require_testdb!();
    for t in engine_targets() {
        let info = t.kind().info();
        let sc = t.schema("scratch");
        let (table, function) = (format!("{sc}.{}", unique("t_ro_all")), format!("{sc}.{}", unique("f_ro_all")));
        let mut drops = vec![format!("TABLE IF EXISTS {table}")];
        if let Unseen::Function { drop, .. } = info.read_only_unseen {
            drops.push(drop.replace("{f}", &function));
        }
        let _drop = Scratch(&t, drops);
        q(&t, &format!("CREATE TABLE {table} (id int)"));
        if let Unseen::Function { make, .. } = info.read_only_unseen {
            let mut w = connected_to(&t);
            tab(&mut w, &make.replace("{f}", &function));
            run(&mut w, "query.run", json!({}));
        }
        let mut s = read_only_session(&t);
        let insert = format!("INSERT INTO {table} VALUES (1)");
        for breaker in info.read_only_breakers {
            let sql = breaker.replace("{t}", &table);
            let e = run_text(&mut s, &sql).err().unwrap_or_else(|| panic!("{}: ran {sql}", t.name()));
            assert!(matches!(e.code(), "readOnly" | "server"), "{}: {sql}: {e}", t.name());
            assert!(run_text(&mut s, &insert).is_err(), "{}: written after {sql}", t.name());
        }
        match info.read_only_unseen {
            Unseen::Function { call, .. } => {
                let e = run_text(&mut s, &call.replace("{f}", &function)).expect_err("caught after it ran");
                assert!(e.code() == "readOnly" && e.to_string().contains("turned read-only on again"), "{}: {e}", t.name());
            }
            Unseen::None(why) => println!("{}: no unseen way: {why}", t.name()),
        }
        assert!(run_text(&mut s, &insert).is_err(), "{}", t.name());
        let (sql, on) = info.read_only_sql;
        run_text(&mut s, sql).unwrap();
        assert_eq!(first_value(&s).as_deref(), Some(on), "{}", t.name());
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {table}")), "0", "{}", t.name());
    }
}

/// A read-only SQLite file's connection attaches no file, so `VACUUM INTO` makes none, and keeps
/// `query_only` on: SQLite's authorizer refuses both, whatever the text looks like.
#[test]
fn read_only_sqlite_attaches_nothing_and_keeps_query_only() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let dir = dbee_testkit::repo_root().join("target").join("testkit");
        std::fs::create_dir_all(&dir).unwrap();
        let (copy, other) = (dir.join(unique("ro_vacuum_into")), dir.join(unique("ro_attach")));
        let _gone = Removed(vec![copy.clone(), other.clone()]);
        std::fs::copy(t.file().unwrap(), &other).unwrap();
        let literal = |path: &std::path::Path| dbee_sql::Dialect::Sqlite.literal(&path.display().to_string()).unwrap();
        let mut s = read_only_session(&t);
        for (sql, about) in [
            (format!("VACUUM INTO {}", literal(&copy)), "attach"),
            ("PRAGMA query_only = 0".to_owned(), "query_only"),
            ("PRAGMA main.query_only = OFF".to_owned(), "query_only"),
            (format!("ATTACH {} AS other", literal(&other)), "attach"),
        ] {
            let e = run_text(&mut s, &sql).err().unwrap_or_else(|| panic!("{}: ran {sql}", t.name()));
            // The hint names the case that applies, and only it.
            let hint = e.data()["hint"].as_str().unwrap_or_default().to_owned();
            assert!(hint.contains("read-only") && hint.contains(about) && !hint.contains("isolated"), "{}: {sql}: {e} {hint}", t.name());
        }
        assert!(!copy.exists(), "{}: VACUUM INTO made no file", t.name());
        run_text(&mut s, "PRAGMA query_only").unwrap();
        assert_eq!(first_value(&s).as_deref(), Some("1"), "{}", t.name());
    }
}

/// On MySQL and MariaDB a `*/` closes an executable comment only while one is open, and a `;`
/// inside an open one does not end the statement: dbee sends each of these texts whole, and the
/// server reads it as the one statement it is (measured on all five servers).
#[test]
fn mysql_texts_split_as_the_server_reads_executable_comments() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| matches!(t.kind().id(), "mysql" | "mariadb")) {
        let mut s = connected_to(&t);
        for (text, value) in [("SELECT 6*/*x;SELECT 7;*/2", "12"), ("SELECT 1 /*! + 1 ; */", "2")] {
            tab(&mut s, text);
            let r = s.run_blocking("query.run", json!({}), T).unwrap_or_else(|e| panic!("{}: {text}: {e}", t.name()));
            assert_eq!(r["results"].as_array().map(Vec::len), Some(1), "{}: {text}: {r}", t.name());
            assert_eq!(first_value(&s).as_deref(), Some(value), "{}: {text}", t.name());
        }
    }
}

/// A query tab cannot turn `standard_conforming_strings` off, which dbee's editor and splitter
/// read as on; after a function turned it off, a statement whose `'…'` strings hold a backslash
/// runs only once dbee has turned it on again. Text the editor shows as one string never runs as
/// two statements.
#[test]
fn postgres_query_tabs_keep_standard_conforming_strings_on() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| t.kind().id() == "postgres") {
        let (made, off) = (unique("t_hidden"), unique("f_scs_off"));
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS scratch.{made}"), format!("FUNCTION IF EXISTS scratch.{off}()")]);
        let function = format!(
            "CREATE FUNCTION scratch.{off}() RETURNS text LANGUAGE sql AS $$ SELECT pg_catalog.set_config('standard_conforming_strings', 'off', false) $$"
        );
        q(&t, &function);
        let mut s = connected_to(&t);
        tab(&mut s, "");
        for sql in [
            "SET standard_conforming_strings = off",
            "RESET standard_conforming_strings",
            "SELECT set_config('standard_conforming_strings', 'off', false)",
        ] {
            let e = run_text(&mut s, sql).err().unwrap_or_else(|| panic!("{}: ran {sql}", t.name()));
            assert!(e.to_string().contains("standard_conforming_strings"), "{}: {sql}: {e}", t.name());
        }
        run_text(&mut s, "SET standard_conforming_strings = on").unwrap();
        run_text(&mut s, &format!("SELECT scratch.{off}()")).unwrap();
        let hiding = format!("SELECT 'a\\' || '; CREATE TABLE scratch.{made} (x int); --'");
        let e = run_text(&mut s, &hiding).expect_err("not run while it was off");
        assert!(e.to_string().contains("turned it on again"), "{}: {e}", t.name());
        run_text(&mut s, &hiding).unwrap();
        assert_eq!(first_value(&s), Some(format!("a\\; CREATE TABLE scratch.{made} (x int); --")), "{}", t.name());
        assert_eq!(q(&t, &format!("SELECT to_regclass('scratch.{made}') IS NULL")), "t", "{}: nothing hidden ran", t.name());
    }
}

/// A read-only connection's query tab refuses `CREATE TABLE`, `ALTER TABLE` and `DROP TABLE` on
/// every engine: on MySQL and MariaDB the session's `SET SESSION TRANSACTION READ ONLY` holds them
/// too (measured on 5.7, 8.0, 26.10, 10.11 and 13.0).
#[test]
fn read_only_refuses_ddl() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let (sc, old, new) = (t.schema("scratch"), unique("t_ro_ddl"), unique("t_ro_new"));
        let _drop = Scratch(&t, vec![format!("TABLE IF EXISTS {sc}.{old}"), format!("TABLE IF EXISTS {sc}.{new}")]);
        q(&t, &format!("CREATE TABLE {sc}.{old} (id int)"));
        let mut s = session();
        let config = dbee_config::ConnectionConfig { read_only: true, ..config_for(&t) };
        open(&mut s, json!({ "config": config, "password": pw() }));
        tab(&mut s, "");
        for sql in
            [format!("CREATE TABLE {sc}.{new} (id int)"), format!("ALTER TABLE {sc}.{old} ADD COLUMN extra int"), format!("DROP TABLE {sc}.{old}")]
        {
            run(&mut s, "query.setText", json!({ "text": sql }));
            let e = run_fails(&mut s, json!({}));
            assert_eq!(e.code(), "readOnly", "{}: {sql}: {e}", t.name());
        }
        // The old table is there as it was, and no new one.
        assert_eq!(q(&t, &format!("SELECT count(*) FROM {sc}.{old}")), "0", "{}", t.name());
        assert!(t.sql(&format!("SELECT extra FROM {sc}.{old}")).is_err(), "{}: no column was added", t.name());
        assert!(t.sql(&format!("SELECT count(*) FROM {sc}.{new}")).is_err(), "{}: no table was made", t.name());
    }
}

/// A string only this test's query holds, so other tests' queries do not count.
fn marker() -> String {
    unique("dbee_marker")
}

/// A query of about 30 s whose text holds `marker`.
fn slow_with(t: &TestTarget, marker: &str) -> String {
    format!("{}, '{marker}' as tag", t.sleep_sql(30))
}

#[test]
fn disconnect_during_query_cancels_it() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let m = marker();
        tab(s, &slow_with(t, &m));
        let job = start(s, json!({}));
        until_running(t, &m);
        run(s, "connection.close", json!({ "force": true }));
        assert!(s.wait_idle(T), "the query was not cancelled");
        stopped_running(t, &m);
        let results = s.pump();
        assert_eq!(results, vec![(job, Ok(json!({ "ignored": true })))]);
        assert_eq!(s.inspect()["screen"], "connections");
    });
}

#[test]
fn closing_running_tab_ignores_result() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let m = marker();
        tab(s, &slow_with(t, &m));
        let job = start(s, json!({}));
        until_running(t, &m);
        run(s, "tab.close", json!({ "force": true }));
        assert!(s.wait_idle(T), "closing the tab did not cancel its query");
        assert_eq!(s.pump(), vec![(job, Ok(json!({ "ignored": true })))]);
        assert!(s.state().tabs.is_empty());
        stopped_running(t, &m);
        // The query connection goes on with the next run.
        tab(s, "select 1 as one");
        assert_eq!(names(&run(s, "query.run", json!({}))["results"][0]), ["one"]);
    });
}

#[test]
fn reconnect_during_query_stops_it() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let m = marker();
        tab(s, &slow_with(t, &m));
        let job = start(s, json!({}));
        until_running(t, &m);
        run(s, "connection.reconnect", json!({}));
        assert_eq!(shown(s)["running"], Json::Null, "the run died with its connection");
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }));
        stopped_running(t, &m);
        run(s, "query.setText", json!({ "text": "select 2 as two" }));
        assert_eq!(names(&run(s, "query.run", json!({}))["results"][0]), ["two"]);
    });
}

/// The id of the active query tab's connection, from a run of its own.
fn query_connection_id(t: &TestTarget, s: &mut Session) -> String {
    run(s, "query.setText", json!({ "text": t.kind().info().connection_id_sql }));
    run(s, "query.run", json!({}));
    first_value(s).unwrap()
}

#[test]
fn lost_query_connection_is_opened_again() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // A file cannot lose its connection.
        if t.is_file() {
            return;
        }
        tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        let id = query_connection_id(t, s);
        // Only the query connection dies: its session, with the open transaction, is gone.
        t.kill_connection(&id).unwrap();
        run(s, "query.setText", json!({ "text": "COMMIT" }));
        let e = run_fails(s, json!({}));
        let lost = "the query connection was lost, and its session with it: the statement that was running may or may not have taken effect (its outcome is unknown), any other uncommitted work in the transaction is gone, and SET values and temporary tables are gone. Run again to open a new one";
        assert_eq!((e.code(), e.to_string().as_str()), ("connectionLost", lost), "{}", t.name());
        assert_eq!(e.data()["action"], Json::Null, "running again is enough: {}", e.data());
        let i = shown(s);
        assert_eq!((&i["error"]["message"], &i["error"]["code"], &i["running"]), (&json!(lost), &json!("connectionLost"), &Json::Null), "{i}");
        // The session is not lost: the browse connection works on.
        assert_eq!(s.inspect()["connection"]["lost"], false);
        open_table(s, t.schema("shop"), "customers");
        // The next run opens a new query connection.
        let query = s.state().tabs[0].id().0;
        run(s, "tab.activate", json!({ "tab": query }));
        let fresh = query_connection_id(t, s);
        assert_ne!(fresh, id);
        assert_eq!(shown(s)["error"], Json::Null);
        // Lost in the middle of a run: it stops at that statement, with the same message.
        let marker = marker();
        run(s, "query.setText", json!({ "text": format!("select 1; {}; select 3", slow_with(t, &marker)) }));
        let job = start(s, json!({}));
        let deadline = Instant::now() + Duration::from_secs(10);
        while q(t, &t.running_with_sql(&marker)) != "1" {
            assert!(Instant::now() < deadline, "{}: the run's second statement did not start", t.name());
            std::thread::sleep(Duration::from_millis(50));
        }
        t.kill_connection(&fresh).unwrap();
        let e = s.wait(job, T).unwrap_err();
        assert_eq!((e.code(), e.to_string().as_str(), e.data()["statement"].as_u64()), ("connectionLost", lost, Some(1)), "{}", e.data());
        assert_eq!(e.data()["action"], Json::Null);
        assert_eq!(shown(s)["results"].as_array().map(Vec::len), Some(1));
        assert_eq!(s.inspect()["connection"]["lost"], false);
        assert_ne!(query_connection_id(t, s), fresh);
    });
}

/// The query connection is lost under a run whose tab then closes: the run is ignored, and the
/// next run says the session was lost, rather than run on a new one as if nothing happened (a
/// `COMMIT` there would succeed with nothing to commit).
#[test]
fn a_closed_tabs_lost_query_connection_is_told_to_the_next_run() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        // A file cannot lose its connection.
        if t.is_file() {
            return;
        }
        let first = tab(s, "BEGIN");
        run(s, "query.run", json!({}));
        let id = query_connection_id(t, s);
        let (key, m) = (unique("dbee_lock"), marker());
        let held = t.hold_lock(&key).unwrap();
        tab(s, &format!("SELECT '{m}' AS tag FROM {}.orders WHERE {} LIMIT 1", t.schema("shop"), t.waits_for_lock_sql(&key)));
        let job = start(s, json!({}));
        until_running(t, &m);
        t.kill_connection(&id).unwrap();
        run(s, "tab.close", json!({ "force": true }));
        drop(held);
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "{}", t.name());
        run(s, "tab.activate", json!({ "tab": first }));
        run(s, "query.setText", json!({ "text": "COMMIT" }));
        let e = run_fails(s, json!({}));
        assert_eq!((e.code(), e.to_string().as_str()), ("connectionLost", QUERY_SESSION_LOST), "{}", t.name());
        assert_eq!(s.inspect()["connection"]["lost"], false, "{}", t.name());
    });
}

#[test]
fn queries_and_their_cancel_go_through_the_tunnel() {
    let db = require_testdb!();
    for t in engine_targets() {
        // A file is not behind an SSH server.
        let Some(server) = t.server() else { continue };
        let mut s = session_in(dbee_testkit::temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line)));
        let config = via_ssh_to(server, &config_for(&t), dbee_config::SshAuth::Key { path: None });
        open(&mut s, json!({ "config": config, "password": pw() }));
        assert_eq!(s.inspect()["connection"]["ssh"], true);
        tab(&mut s, "select 'through' as app");
        assert_eq!(names(&run(&mut s, "query.run", json!({}))["results"][0]), ["app"]);
        let m = marker();
        run(&mut s, "query.setText", json!({ "text": slow_with(&t, &m) }));
        let job = start(&mut s, json!({}));
        until_running(&t, &m);
        run(&mut s, "query.cancel", json!({}));
        assert_eq!(s.wait(job, T).unwrap_err().code(), "cancelled", "{}", t.name());
    }
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
    for_each_engine(|t, s| {
        open_table(s, t.schema("shop"), "customers");
        for id in ["query.run", "query.cancel", "query.showResult"] {
            assert_eq!(s.execute(id, json!({ "index": 0 })).unwrap_err().data()["why"], "no query tab is active", "{id}");
        }
        tab(s, "select 1");
        assert_eq!(s.execute("query.showResult", json!({ "index": 0 })).unwrap_err().code(), "badParams", "no results yet");
        for bad in [json!({ "current": "yes" }), json!({ "current": true, "cursor": -1 }), json!({ "cursor": "x" })] {
            assert_eq!(s.execute("query.run", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
        }
    });
}

#[test]
fn mysql_query_tab_uses_the_tree_database() {
    let _db = require_testdb!();
    let mut seen = 0;
    for t in engine_targets() {
        let mut s = connected_to(&t);
        let dialect = s.state().dialect(Which::Active);
        // The tree's schema becomes the tab's database only where the engine has `USE`.
        if !dialect.has_use_database() {
            tab(&mut s, "select 1");
            run(&mut s, "tab.newQuery", json!({ "schema": "shop" }));
            assert_eq!(shown(&s)["database"], Json::Null, "{}", t.name());
            let e = s.execute("query.setDatabase", json!({ "database": "shop" })).unwrap_err();
            assert_eq!(e.code(), "disabled", "{}: {e}", t.name());
            let why = e.data()["why"].as_str().unwrap_or_default().to_owned();
            for engine in ["Postgres", "MySQL", "MariaDB", "SQLite"] {
                assert!(!why.contains(engine), "{why}");
            }
            continue;
        }
        seen += 1;
        // Neither the settings nor the tree give a database: none, and the server says so.
        assert_eq!(config_for(&t).database, "", "{}", t.name());
        let first = tab(&mut s, "SELECT COUNT(*) FROM orders");
        assert_eq!(shown(&s)["database"], Json::Null);
        let e = run_fails(&mut s, json!({}));
        assert_eq!(e.code(), "server", "{}: no database is selected: {e}", t.name());
        // query.setDatabase gives it one.
        let r = run(&mut s, "query.setDatabase", json!({ "database": "odd" }));
        assert_eq!((r["tab"].as_u64(), r["database"].as_str()), (Some(first), Some("odd")));
        run(&mut s, "query.setText", json!({ "text": "SELECT COUNT(*) FROM `Other Ödd`" }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s).as_deref(), Some("3"));
        // A tab opened with the tree's table selected takes that table's schema.
        let shop = run(&mut s, "tab.newQuery", json!({ "text": "SELECT COUNT(*) FROM orders", "schema": "shop" }))["tab"].clone();
        assert_eq!(shown(&s)["database"], "shop", "{}", t.name());
        let Some(Tab::Query(qt)) = s.state().active_tab() else { panic!("a query tab is active") };
        assert_eq!(qt.database.as_deref(), Some("shop"));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s), Some(ORDERS_COUNT.to_string()), "{}", t.name());
        // Each tab runs in its own database on the one query connection.
        run(&mut s, "tab.activate", json!({ "tab": first }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s).as_deref(), Some("3"));
        // A `USE` in the text sticks to the tab: its next run starts there too.
        run(&mut s, "tab.activate", json!({ "tab": shop }));
        run(&mut s, "query.setText", json!({ "text": "USE many; SELECT v FROM t_002" }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s).as_deref(), Some("row of table 2"));
        assert_eq!(shown(&s)["database"], "many", "{}", t.name());
        run(&mut s, "query.setText", json!({ "text": "SELECT v FROM t_001" }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s).as_deref(), Some("row of table 1"));
        run(&mut s, "query.setDatabase", json!({ "database": "shop" }));
        run(&mut s, "query.setText", json!({ "text": "SELECT COUNT(*) FROM orders" }));
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s), Some(ORDERS_COUNT.to_string()));
        // A database that is gone: the run fails, saying which.
        run(&mut s, "query.setDatabase", json!({ "tab": shop, "database": "dbee_gone" }));
        let e = run_fails(&mut s, json!({}));
        assert_eq!(e.code(), "server", "{}: {e}", t.name());
        assert!(e.to_string().starts_with("the tab's database `dbee_gone`: ") && e.to_string().contains("dbee_gone'"), "{e}");
        assert!(shown(&s)["error"]["message"].as_str().unwrap_or_default().starts_with("the tab's database `dbee_gone`: "), "{}", shown(&s));
        // None again, and refusals.
        let r = run(&mut s, "query.setDatabase", json!({ "tab": shop, "database": null }));
        assert_eq!(r["database"], Json::Null);
        assert_eq!(shown(&s)["database"], Json::Null);
        for bad in [
            json!({}),
            json!({ "database": "" }),
            json!({ "database": 5 }),
            json!({ "database": "a\u{0}b" }),
            json!({ "tab": 999, "database": "shop" }),
        ] {
            assert_eq!(s.execute("query.setDatabase", bad.clone()).unwrap_err().code(), "badParams", "{bad}");
        }
        open_table(&mut s, "shop", "customers");
        let table_tab = s.inspect()["active"].clone();
        assert_eq!(s.execute("query.setDatabase", json!({ "database": "shop" })).unwrap_err().data()["why"], "no query tab is active");
        assert_eq!(s.execute("query.setDatabase", json!({ "tab": table_tab, "database": "shop" })).unwrap_err().code(), "badParams");
        // The settings' database is the default when the tree gives none.
        drop(s);
        let mut s = session();
        open(&mut s, json!({ "config": dbee_config::ConnectionConfig { database: "many".into(), ..config_for(&t) }, "password": pw() }));
        tab(&mut s, "SELECT v FROM t_001");
        assert_eq!(shown(&s)["database"], "many", "{}", t.name());
        run(&mut s, "query.run", json!({}));
        assert_eq!(first_value(&s).as_deref(), Some("row of table 1"));
        run(&mut s, "tab.newQuery", json!({ "schema": "odd" }));
        assert_eq!(shown(&s)["database"], "odd", "the tree's schema comes first");
    }
    assert!(seen > 0, "the MySQL and MariaDB servers have USE");
}

/// Every target whose dialect has `USE`, connected; at least one.
fn with_use(mut each: impl FnMut(&TestTarget, &mut Session)) {
    let mut seen = 0;
    for t in engine_targets() {
        let mut s = connected_to(&t);
        if s.state().dialect(Which::Active).has_use_database() {
            seen += 1;
            each(&t, &mut s);
        }
    }
    assert!(seen > 0, "the MySQL and MariaDB servers have USE");
}

/// Where the active tab's next run starts: `(database, known)`.
fn starts_in(s: &Session) -> (Json, Json) {
    let at = &shown(s)["startsIn"];
    (at["database"].clone(), at["known"].clone())
}

/// A tab with no database runs where the query connection is: another tab's `USE` moved it,
/// so the tab says so, and its run goes there.
#[test]
fn a_tab_without_a_database_starts_where_the_connection_is() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let first = tab(s, "SELECT DATABASE()");
        assert_eq!(shown(s)["database"], Json::Null);
        assert_eq!(starts_in(s), (Json::Null, json!(true)), "{}: the settings name no database", t.name());
        tab(s, "USE shop");
        run(s, "query.run", json!({}));
        assert_eq!(shown(s)["database"], "shop", "{}: a typed USE sticks to its tab", t.name());
        run(s, "tab.activate", json!({ "tab": first }));
        assert_eq!(shown(s)["database"], Json::Null);
        assert_eq!(starts_in(s), (json!("shop"), json!(true)), "{}", t.name());
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("shop"), "{}", t.name());
        assert_eq!(shown(s)["database"], "shop", "{}: after its run the tab names where it ran", t.name());
    });
}

/// A `USE` typed in a tab makes that tab's database the one it names; another tab's next run
/// still starts in its own.
#[test]
fn a_typed_use_sticks_to_its_tab_and_no_other() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let moved = run(s, "tab.newQuery", json!({ "text": "USE many; SELECT DATABASE()", "schema": "shop" }))["tab"].clone();
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("many"));
        assert_eq!(shown(s)["database"], "many", "{}", t.name());
        run(s, "tab.newQuery", json!({ "text": "SELECT DATABASE()", "schema": "odd" }));
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("odd"), "{}: the other tab starts in its own", t.name());
        run(s, "tab.activate", json!({ "tab": moved }));
        run(s, "query.setText", json!({ "text": "SELECT DATABASE()" }));
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("many"), "{}", t.name());
        // A tab whose `USE` fails keeps its own database: it never ran its text.
        run(s, "query.setDatabase", json!({ "database": "dbee_gone" }));
        assert_eq!(run_fails(s, json!({})).code(), "server");
        assert_eq!(shown(s)["database"], "dbee_gone", "{}", t.name());
    });
}

/// A cancelled statement may have moved the connection before its kill landed: after it, the
/// database the connection is in is unknown, though the server reported the `USE many` before it.
/// A tab with a database then sends its `USE` first.
#[test]
fn a_cancelled_statement_leaves_the_database_not_known() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let (key, m) = (unique("dbee_lock"), marker());
        let held = t.hold_lock(&key).unwrap();
        tab(s, &format!("USE many; SELECT '{m}' AS tag FROM DUAL WHERE {}", t.waits_for_lock_sql(&key)));
        let job = start(s, json!({}));
        until_running(t, &m);
        run(s, "query.cancel", json!({}));
        assert_eq!(s.wait(job, T).unwrap_err().code(), "cancelled", "{}", t.name());
        drop(held);
        assert_eq!(shown(s)["database"], Json::Null, "{}: the tab keeps its own", t.name());
        assert_eq!(starts_in(s), (json!("many"), json!(false)), "{}", t.name());
        run(s, "query.setDatabase", json!({ "database": "odd" }));
        run(s, "query.setText", json!({ "text": "SELECT DATABASE()" }));
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("odd"), "{}", t.name());
        tab(s, "");
        assert_eq!(starts_in(s), (json!("odd"), json!(true)), "{}: the tab's USE made it known again", t.name());
    });
}

/// Closing a tab stops its run, whose result is then ignored; but the `USE many` it ran moved the
/// query connection, and the engine still takes note: another tab's run does not skip its `USE`.
#[test]
fn a_run_whose_tab_closed_still_says_where_it_left_the_connection() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let shop = run(s, "tab.newQuery", json!({ "text": "SELECT DATABASE()", "schema": "shop" }))["tab"].clone();
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("shop"));
        let (key, m) = (unique("dbee_lock"), marker());
        let held = t.hold_lock(&key).unwrap();
        tab(s, &format!("USE many; SELECT '{m}' AS tag FROM DUAL WHERE {}", t.waits_for_lock_sql(&key)));
        let job = start(s, json!({}));
        until_running(t, &m);
        run(s, "tab.close", json!({ "force": true }));
        drop(held);
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "{}", t.name());
        // Its statement was cancelled, or ended first: either way the connection is not in shop.
        tab(s, "");
        assert_eq!(starts_in(s).0, json!("many"), "{}", t.name());
        run(s, "tab.activate", json!({ "tab": shop }));
        assert_eq!(starts_in(s), (json!("shop"), json!(true)), "{}", t.name());
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("shop"), "{}: the run went where its tab said", t.name());
    });
}

/// A tab's database waits for the tab's run, which sets it when it ends: `query.setDatabase` is
/// busy meanwhile. Another tab's can be set.
#[test]
fn a_running_tab_keeps_its_database_until_the_run_ends() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let other = run(s, "tab.newQuery", json!({ "text": "SELECT DATABASE()", "schema": "shop" }))["tab"].clone();
        let (key, m) = (unique("dbee_lock"), marker());
        let held = t.hold_lock(&key).unwrap();
        let text = format!("SELECT '{m}' AS tag FROM DUAL WHERE {}", t.waits_for_lock_sql(&key));
        run(s, "tab.newQuery", json!({ "text": text, "schema": "shop" }));
        let job = start(s, json!({}));
        until_running(t, &m);
        let e = s.execute("query.setDatabase", json!({ "database": "odd" })).unwrap_err();
        assert_eq!((e.code(), e.to_string().as_str()), ("busy", "busy: a query is running in Query 2"), "{}", t.name());
        assert_eq!(run(s, "query.setDatabase", json!({ "tab": other, "database": "odd" }))["database"], "odd");
        drop(held);
        s.wait(job, T).unwrap();
        assert_eq!(shown(s)["database"], "shop", "{}", t.name());
        assert_eq!(run(s, "query.setDatabase", json!({ "database": "odd" }))["database"], "odd", "{}: the run has ended", t.name());
    });
}

/// Every MySQL and MariaDB server reports a drop of the current database as a move to none:
/// the tab and the connection are then in no database, and say so.
#[test]
fn dropping_the_current_database_leaves_none() {
    let _db = require_testdb!();
    with_use(|t, s| {
        for statement in ["DROP DATABASE", "drop schema"] {
            let name = unique("dbee_dropped");
            let _drop = Scratch(t, vec![format!("DATABASE IF EXISTS {name}")]);
            q(t, &format!("CREATE DATABASE {name}"));
            run(s, "tab.newQuery", json!({ "text": format!("{statement} {name}"), "schema": name }));
            run(s, "query.run", json!({}));
            assert_eq!(shown(s)["database"], Json::Null, "{}: {statement}", t.name());
            assert_eq!(starts_in(s), (Json::Null, json!(true)), "{}: {statement}: the chip names no database", t.name());
            run(s, "query.setText", json!({ "text": "SELECT DATABASE()" }));
            run(s, "query.run", json!({}));
            assert_eq!(first_value(s), None, "{}: {statement}", t.name());
        }
    });
}

/// The second value of the first row of the active query tab's shown result.
fn second_value(s: &Session) -> Option<String> {
    let Some(Tab::Query(t)) = s.state().active_tab() else { panic!("a query tab is active") };
    t.shown_result()?.result.rows.first()?.get(1)?.clone()
}

/// The statements the query connection's session has sent the server so far (`Questions`).
fn questions(s: &mut Session) -> u64 {
    run(s, "query.setText", json!({ "text": "SHOW SESSION STATUS LIKE 'Questions'" }));
    run(s, "query.run", json!({}));
    second_value(s).and_then(|v| v.parse().ok()).expect("a count of questions")
}

/// The engine follows the database through what the server reports: a typed `USE` moves its
/// tab with no statement of the engine's own, and a tab already where its database is sends no
/// `USE` before its run.
#[test]
fn a_typed_use_moves_its_tab_with_no_extra_statement() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let moved = tab(s, "");
        let before = questions(s);
        run(s, "query.setText", json!({ "text": "USE many" }));
        run(s, "query.run", json!({}));
        assert_eq!(shown(s)["database"], "many", "{}", t.name());
        tab(s, "");
        let after = questions(s);
        assert_eq!(after - before, 2, "{}: the typed USE and this count, nothing else", t.name());
        assert_eq!(shown(s)["database"], "many", "{}: a tab with none starts where the connection is", t.name());
        run(s, "tab.activate", json!({ "tab": moved }));
        let again = questions(s);
        assert_eq!(again - after, 1, "{}: the connection is in many already: no USE first", t.name());
    });
}

/// A run that names `session_track` may have turned the server's reports off: dbee no longer
/// trusts them on that connection, so a tab's `USE` goes before every run again, and a `USE`
/// typed in another tab the server no longer reports cannot leave a tab in the wrong database.
/// A new connection is trusted again.
#[test]
fn a_run_that_may_turn_tracking_off_ends_the_trust_in_it() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let first = run(s, "tab.newQuery", json!({ "text": "SET SESSION session_track_schema = OFF", "schema": "shop" }))["tab"].clone();
        run(s, "query.run", json!({}));
        run(s, "tab.newQuery", json!({ "text": "USE many", "schema": "shop" }));
        run(s, "query.run", json!({}));
        run(s, "tab.activate", json!({ "tab": first }));
        run(s, "query.setText", json!({ "text": "SELECT DATABASE()" }));
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("shop"), "{}: the run starts in its tab's database", t.name());
        tab(s, "");
        assert_eq!(starts_in(s).1, json!(false), "{}: the database the connection is in is unknown", t.name());
        // A new query connection reports again.
        run(s, "connection.reconnect", json!({}));
        run(s, "tab.activate", json!({ "tab": first }));
        run(s, "query.run", json!({}));
        tab(s, "");
        assert_eq!(starts_in(s), (json!("shop"), json!(true)), "{}", t.name());
    });
}

/// A warning outlives its run: nothing of the engine's runs between it and a `SHOW WARNINGS` of
/// its own, in a tab with a database or without one.
#[test]
fn show_warnings_as_its_own_run_sees_the_last_runs_warning() {
    let _db = require_testdb!();
    with_use(|t, s| {
        let name = unique("t_warn");
        let _drop = scratch_table(t, &name, "CREATE TABLE {t} (v TINYINT)");
        for schema in [Some("scratch"), None] {
            let insert = match schema {
                Some(_) => format!("INSERT IGNORE INTO {name} VALUES (300)"),
                None => format!("INSERT IGNORE INTO scratch.{name} VALUES (300)"),
            };
            let mut params = json!({ "text": insert });
            if let Some(schema) = schema {
                params["schema"] = json!(schema);
            }
            run(s, "tab.newQuery", params);
            run(s, "query.run", json!({}));
            run(s, "query.setText", json!({ "text": "SHOW WARNINGS" }));
            run(s, "query.run", json!({}));
            let warning = shown(s)["results"][0]["rows"].as_u64();
            assert_eq!(warning, Some(1), "{} ({schema:?}): the INSERT's warning is still there", t.name());
        }
    });
}

#[test]
fn a_nul_in_a_statement_runs_where_the_engine_takes_it() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let refused = |s: &mut TestSession, text: &str| {
            run(s, "query.setText", json!({ "text": text }));
            let e = run_fails(s, json!({}));
            assert_eq!(e.code(), "other", "{}: {text:?}: {e}", t.name());
            assert!(e.to_string().contains("which this database can't take"), "{e}");
            assert!(!e.to_string().contains("Postgres"), "{e}");
            assert_eq!(
                (e.data()["statement"].as_u64(), e.data()["position"].as_u64()),
                (Some(1), Some(text.rfind('\u{0}').unwrap() as u64)),
                "{text:?}"
            );
        };
        let (sc, name) = (t.schema("scratch"), unique("t_nul"));
        let _drop = scratch_table(t, &name, "CREATE TABLE {t} (id int PRIMARY KEY); INSERT INTO {t} VALUES (1), (2)");
        tab(s, "select 1");
        // Outside a string literal no engine takes it: MySQL runs `DELETE … <NUL>  ` as if the NUL
        // ended it, and turns down a NUL followed by more text. Nothing is sent. (A comment after
        // the statement is not sent at all.)
        for text in [
            "select 1;\nselect 2 -- a comment\u{0}\n + 1".to_owned(),
            "select 1;\nselect 2\u{0} + 1".to_owned(),
            format!("select 1;\ndelete from {sc}.{name} \u{0}   "),
        ] {
            refused(s, &text);
        }
        assert_eq!(q(t, &format!("SELECT count(*) FROM {sc}.{name}")), "2", "{}", t.name());
        // Inside a string literal: a character where the engine takes it.
        let text = "select 1;\nselect 'a\u{0}b' as v";
        if !s.state().dialect(Which::Active).statement_may_hold_nul() {
            refused(s, text);
            return;
        }
        run(s, "query.setText", json!({ "text": text }));
        run(s, "query.run", json!({}));
        assert_eq!(first_value(s).as_deref(), Some("a\u{0}b"), "{}", t.name());
    });
}

#[test]
fn an_isolated_run_lets_sql_open_no_other_file() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let folder = t.file().unwrap().parent().unwrap().to_path_buf();
        let (other, copy) = (folder.join(format!("{}.sqlite", unique("attached"))), folder.join(format!("{}.sqlite", unique("vacuumed"))));
        let _gone = Removed(vec![other.clone(), copy.clone()]);
        let literal = |path: &std::path::Path| dbee_sql::Dialect::Sqlite.literal(&path.display().to_string()).unwrap();
        let (attach, vacuum) = (format!("ATTACH {} AS other", literal(&other)), format!("VACUUM INTO {}", literal(&copy)));
        {
            let mut s = session_adjusted(|o| o.allowed_dirs = Some(vec![folder.clone()]));
            open(&mut s, json!({ "config": config_for(&t) }));
            tab(&mut s, "select 1");
            for sql in [&attach, &vacuum] {
                run(&mut s, "query.setText", json!({ "text": sql }));
                let e = run_fails(&mut s, json!({}));
                assert_eq!((e.code(), e.data()["serverCode"].as_str()), ("server", Some("23")), "{sql}: {e}");
                assert!(e.data()["hint"].as_str().is_some_and(|h| h.contains("ATTACH")), "{sql}: {}", e.data());
            }
            assert!(!other.exists() && !copy.exists(), "no file was made");
            // The file's own SQL runs.
            run(&mut s, "query.setText", json!({ "text": format!("select count(*) from {}.orders", t.schema("shop")) }));
            run(&mut s, "query.run", json!({}));
            assert_eq!(first_value(&s), Some(ORDERS_COUNT.to_string()));
        }
        // A normal run runs both (ATTACH opens a file that is there; an empty one is a database).
        std::fs::write(&other, "").unwrap();
        let mut s = connected_to(&t);
        tab(&mut s, &format!("{attach}; CREATE TABLE other.o (a INTEGER); DETACH other; {vacuum}"));
        run(&mut s, "query.run", json!({}));
        assert!(other.is_file() && copy.is_file(), "{}", t.name());
    }
}

/// Removes the files it names when the test ends, also when it fails.
struct Removed(Vec<std::path::PathBuf>);

impl Drop for Removed {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}
