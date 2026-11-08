//! What runs around each statement on a connection dbee made read-only.
//!
//! Before it: the statements dbee does not send ([`Dialect::refusal`]), read as this server reads
//! its version comments ([`gated::as_read`]) and its strings (`NO_BACKSLASH_ESCAPES`, from the last
//! OK packet's status), which says in plain words why; and, read from the raw text, comments
//! included, a statement that names `session_track` or, on MariaDB, `STATEMENT` beside the
//! read-only variable.
//!
//! After it: the server's own report. The session tracks the read-only variable and the
//! transaction's characteristics, so each OK packet says when a statement changed them, however
//! its text was spelled ([`ModeReport`]); when one turned read-only off, dbee rolls back, turns it on
//! again and says so. An error packet carries no report, and MariaDB drops what a statement changed
//! before its error, so after an error the mode is read (the variable, whose packet also brings
//! what MySQL still has to report) before the next statement that is not `SHOW WARNINGS`,
//! `SHOW ERRORS` or `GET DIAGNOSTICS`, which the read would clear. So is a read a cancel stopped.

use std::sync::atomic::Ordering;

use dbee_driver::{DbError, Limits};
use dbee_sql::Dialect;
use dbee_sql::lexer::{TokenKind, tokenize_with};

use crate::connect::{MySqlConnection, Session, names_session_track};
use crate::flavor::Flavor;
use crate::gated::{self, Unread};
use crate::result::ModeReport;

const TURNED_OFF: &str = "the statement turned read-only off, so dbee rolled back the open transaction and turned read-only on again. To write, \
                          turn Read-only off in the connection's settings.";

const TURNED_OFF_BEFORE: &str = "an earlier statement turned read-only off, so dbee rolled back the open transaction, turned read-only on again \
                                 and did not run this statement. Run it again; to write, turn Read-only off in the connection's settings.";

const TURNED_OFF_EARLIER: &str = "an earlier statement turned read-only off, so dbee rolled back the open transaction and turned read-only on \
                                  again. To write, turn Read-only off in the connection's settings.";

const UNREADABLE: &str = "dbee could not read what the server reported of read-only mode, so it rolled back the open transaction and turned \
                          read-only on again. Run the statement again.";

const NOT_RESTORED: &str = "read-only was turned off and dbee could not turn it on again, so it closed the connection. Reconnect to go on.";

const ONE_ROW: Limits = Limits { max_rows: 1, max_bytes: 1024 };

const TOO_MANY_VERSIONS: &str = "this statement has more version comments than dbee can tell how the server reads, so it does not run it here. \
                                 Split the statement, or turn Read-only off in the connection's settings.";

const NESTED: &str = "a version comment this server skips holds another comment, which the server may end elsewhere than dbee reads it, \
                      so dbee does not run it here. Take the inner comment out, or turn Read-only off in the connection's settings.";

const TRACKING: &str = "the server reports read-only mode to dbee through session tracking, which this statement names, so dbee does not \
                        run it here. To run it, turn Read-only off in the connection's settings.";

const BLOCK: &str = "a block of several statements (BEGIN NOT ATOMIC … END, IF … END IF) can turn read-only off and write before the \
                    server reports it, so dbee does not run one here. Run its statements one by one, or turn Read-only off in the \
                    connection's settings.";

const LOOKALIKE: &str = "on a read-only connection dbee runs no SET with a character outside plain ASCII outside its '…' strings, since \
                        the server can read a name spelled with such a letter as one of its own settings. Spell names in plain ASCII, or \
                        turn Read-only off in the connection's settings.";

const SET_STATEMENT: &str = "SET STATEMENT can turn read-only off for the statement it runs, before the server can report it, so dbee does not \
                             run a statement that names STATEMENT and the read-only variable here. Run the statement without them.";

/// What reading the mode back found.
enum Mode {
    ReadOnly,
    Changed,
    /// A cancel stopped the read.
    Stopped,
    /// The server refused the read: the mode is unknown.
    Unread(DbError),
    Failed(DbError),
}

/// `sql` reads the warnings or errors of the statement before it, and leaves them in place.
fn diagnostic(sql: &str, backslash_escapes: bool) -> bool {
    let words: Vec<String> = tokenize_with(Dialect::MySql, sql, backslash_escapes)
        .into_iter()
        .filter(|t| matches!(t.kind, TokenKind::Keyword | TokenKind::Ident))
        .take(3)
        .filter_map(|t| sql.get(t.range).map(str::to_ascii_lowercase))
        .collect();
    matches!(
        words.iter().map(String::as_str).collect::<Vec<_>>().as_slice(),
        ["show", "warnings" | "errors" | "count", ..] | ["get", "diagnostics", ..] | ["get", "current" | "stacked", "diagnostics"]
    )
}

/// `sql` holds `word` as a whole word, in any case.
fn holds_word(sql: &str, word: &str) -> bool {
    let b = sql.as_bytes();
    let part = |at: usize| b.get(at).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80);
    (0..b.len().saturating_sub(word.len()).saturating_add(1)).any(|at| {
        b.get(at..at + word.len()).is_some_and(|w| w.eq_ignore_ascii_case(word.as_bytes())) && !part(at + word.len()) && (at == 0 || !part(at - 1))
    })
}

/// `sql` holds the word `SET` and a character outside plain ASCII outside its `'…'` strings, read
/// with backslash escapes and without: every system variable's name is ASCII, while the server
/// folds a name spelled with a letter like `ë` or `ſ` to one of them. A `"…"` counts as outside
/// (`ANSI_QUOTES` makes it a name), and the two readings cover a character set such as `sjis`,
/// whose characters can end in the byte `\`.
fn sets_a_lookalike(sql: &str) -> bool {
    holds_word(sql, "set")
        && [true, false].into_iter().any(|escapes| {
            tokenize_with(Dialect::MySql, sql, escapes).iter().any(|t| {
                let text = sql.get(t.range.clone()).unwrap_or_default();
                let quoted = t.kind == TokenKind::String && text.trim_start_matches(|c: char| c.is_ascii_alphabetic()).starts_with('\'');
                !text.is_ascii() && !quoted
            })
        })
}

/// `sql` still holds a `;` outside strings and comments, read with backslash escapes and without:
/// after dbee's split only a `DELIMITER` leaves one in, as a block of several statements needs.
fn holds_a_semicolon(sql: &str) -> bool {
    [true, false].into_iter().any(|escapes| tokenize_with(Dialect::MySql, sql, escapes).iter().any(|t| t.kind == TokenKind::Semicolon))
}

/// Why dbee does not send `sql` on a read-only connection, from its raw text, comments included:
/// what no reading of it can show.
fn raw_refusal(sql: &str, flavor: Flavor) -> Option<&'static str> {
    let holds = |part: &str| sql.as_bytes().windows(part.len()).any(|w| w.eq_ignore_ascii_case(part.as_bytes()));
    if holds_a_semicolon(sql) {
        Some(BLOCK)
    } else if sets_a_lookalike(sql) {
        Some(LOOKALIKE)
    } else if names_session_track(sql) {
        Some(TRACKING)
    } else if flavor == Flavor::MariaDb && holds("statement") && flavor.read_only_variables().iter().any(|v| holds(v)) {
        Some(SET_STATEMENT)
    } else {
        None
    }
}

impl MySqlConnection {
    /// `sql` as this server reads it, in each way it may ([`gated::as_read`]).
    fn as_read(&self, session: &Session, sql: &str) -> Result<Vec<String>, DbError> {
        gated::as_read(sql, self.flavor(), self.info().version_num, session.backslash_escapes).map_err(|why| DbError::ReadOnly {
            message: match why {
                Unread::TooMany => TOO_MANY_VERSIONS,
                Unread::Nested => NESTED,
            }
            .into(),
        })
    }

    /// Before a statement runs, with the session locked.
    pub(crate) async fn before(&self, session: &mut Session, sql: &str) -> Result<(), DbError> {
        if !self.read_only {
            return Ok(());
        }
        if let Some(why) = raw_refusal(sql, self.flavor()) {
            return Err(DbError::ReadOnly { message: why.into() });
        }
        let readings = self.as_read(session, sql)?;
        let escapes = session.backslash_escapes;
        if let Some(refused) = readings.iter().find_map(|r| Dialect::MySql.refusal_with(r, true, escapes)) {
            return Err(refused.into());
        }
        if self.check_due.load(Ordering::Acquire) && !readings.iter().all(|r| diagnostic(r, escapes)) {
            match self.mode(session).await {
                Mode::ReadOnly => self.check_due.store(false, Ordering::Release),
                Mode::Changed => return Err(self.restore(session, TURNED_OFF_BEFORE).await),
                Mode::Stopped => return Err(DbError::Cancelled),
                Mode::Unread(e) | Mode::Failed(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// After a statement ran, with the session still locked: what the server reported of it.
    pub(crate) async fn after<T>(&self, session: &mut Session, ran: Result<T, DbError>) -> Result<T, DbError> {
        if !self.read_only || ran.as_ref().err() == Some(&DbError::ConnectionLost) {
            return ran;
        }
        let said = match session.mode {
            ModeReport::Silent => None,
            ModeReport::Off if self.check_due.load(Ordering::Acquire) => Some(TURNED_OFF_EARLIER),
            ModeReport::Off => Some(TURNED_OFF),
            ModeReport::Unreadable => Some(UNREADABLE),
        };
        if let Some(said) = said {
            return Err(self.restore(session, said).await);
        }
        if ran.is_err() {
            self.check_due.store(true, Ordering::Release);
        }
        ran
    }

    /// The mode as the variable and the report of reading it say.
    async fn mode(&self, session: &mut Session) -> Mode {
        let sql = format!("SELECT @@SESSION.{}", session.read_only_variable);
        match self.statement(session, &sql, ONE_ROW).await {
            Ok(r) if session.mode == ModeReport::Silent && r.rows.first().and_then(|row| row.first()).is_some_and(|v| v.as_deref() == Some("1")) => {
                Mode::ReadOnly
            }
            Ok(_) => Mode::Changed,
            Err(DbError::Cancelled) => Mode::Stopped,
            Err(e @ DbError::Server(_)) => Mode::Unread(e),
            Err(e) => Mode::Failed(e),
        }
    }

    /// Rolls back what is open and turns read-only on again; the error to report, `said` when it
    /// worked. When it did not, the connection is closed.
    async fn restore(&self, session: &mut Session, said: &str) -> DbError {
        let mut set = self.statement(session, "ROLLBACK", ONE_ROW).await.map(drop);
        if set.is_ok() {
            set = self.statement(session, "SET SESSION TRANSACTION READ ONLY", ONE_ROW).await.map(drop);
        }
        if set.is_ok() && matches!(self.mode(session).await, Mode::ReadOnly) {
            self.check_due.store(false, Ordering::Release);
            log::info!("mysql: a statement turned read-only off; turned it on again");
            return DbError::ReadOnly { message: said.into() };
        }
        self.mark_closed();
        DbError::Other(NOT_RESTORED.into())
    }
}

#[cfg(test)]
mod tests {
    use dbee_config::{ConnectionConfig, DbKind};
    use dbee_driver::{Purpose, QUERY_LIMITS, Target};
    use dbee_testkit::{Kind, require_testdb, unique};

    use super::*;
    use crate::connect::connect;

    /// What the reviews found turns read-only off, sent with dbee's check of the text skipped:
    /// the server's own report catches each, right after it ran, or, after an error (which carries
    /// no report), before the next statement. Nothing is written, and the session reads read-only.
    #[tokio::test]
    async fn the_servers_report_catches_what_the_text_check_misses() {
        let db = require_testdb!();
        for server in db.servers_of(&[Kind::MySql, Kind::MariaDb]) {
            let kind = DbKind::from_id(server.kind.id()).unwrap();
            let config = ConnectionConfig { kind, host: "127.0.0.1".into(), port: server.port, user: "root".into(), ..ConnectionConfig::default() };
            let target = |config| Target {
                config,
                password: Some(&db.password),
                tunnel: None,
                purpose: Purpose::Query,
                create_file: false,
                allowed_dirs: None,
            };
            let w = connect(target(&config)).await.unwrap();
            let read_only = ConnectionConfig { read_only: true, ..config.clone() };
            let conn = connect(target(&read_only)).await.unwrap();
            let v = conn.lock().await.unwrap().read_only_variable;
            let (t, off, off_once, proc_off) =
                (unique("scratch.t_track"), unique("scratch.f_track"), unique("scratch.f_track_once"), unique("scratch.p_track"));
            w.run(&format!("CREATE TABLE {t} (id int)"), QUERY_LIMITS).await.unwrap();
            w.run(&format!("CREATE FUNCTION {off}() RETURNS INT DETERMINISTIC BEGIN SET SESSION {v} = 0; RETURN 1; END"), QUERY_LIMITS)
                .await
                .unwrap();
            w.run(&format!("CREATE FUNCTION {off_once}() RETURNS INT DETERMINISTIC BEGIN SET @@{v} = 0; RETURN 1; END"), QUERY_LIMITS).await.unwrap();
            w.run(&format!("CREATE PROCEDURE {proc_off}() BEGIN SET SESSION {v} = 0; END"), QUERY_LIMITS).await.unwrap();
            let mut forms = vec![
                format!("SET @@{v} = 0"),
                format!("SET @@SESSION.{v} = 0"),
                format!("SET SESSION {v} = 1 - 1"),
                format!("SET @@{v} = TRUE - 1"),
                "START TRANSACTION READ WRITE".to_owned(),
                "/*!99999 /* */ SELECT */ START TRANSACTION READ WRITE".to_owned(),
                "/*M!999999 SELECT */ START TRANSACTION READ WRITE".to_owned(),
                "SET TRANSACTION READ WRITE".to_owned(),
                "SET SESSION TRANSACTION READ WRITE".to_owned(),
                "SET SESSION sql_mode = CONCAT(@@SESSION.sql_mode, ',NO_BACKSLASH_ESCAPES')".to_owned(),
                format!(r"SET @a = '\', @@{v} = 0, @b = '\'"),
                "SET SESSION sql_mode = DEFAULT".to_owned(),
                format!("SELECT {off}()"),
                format!("SELECT {off}(), CAST('1x' AS SIGNED)"),
                format!("SELECT {off}(), (SELECT 1 UNION SELECT 2)"),
                format!("SELECT {off_once}(), (SELECT 1 UNION SELECT 2)"),
                format!("CALL {proc_off}()"),
                format!("PREPARE s FROM 'SET @@{v} = 0'"),
                "EXECUTE s".to_owned(),
            ];
            if kind == DbKind::MariaDb {
                forms.push(format!("EXECUTE IMMEDIATE 'SET @@{v} = 0'"));
            }
            let mut written_after = Vec::new();
            for (n, form) in forms.iter().enumerate() {
                let mut session = conn.lock().await.unwrap();
                let ran = conn.statement(&mut session, form, QUERY_LIMITS).await;
                let after = conn.after(&mut session, ran).await;
                drop(session);
                let insert = conn.run(&format!("INSERT INTO {t} VALUES ({n})"), QUERY_LIMITS).await;
                println!(
                    "{}: {form} -> {:?}; the insert after it -> {:?}",
                    server.name,
                    after.err().map(|e| e.to_string()),
                    insert.as_ref().err().map(DbError::code)
                );
                if insert.is_ok() {
                    written_after.push(form.clone());
                }
            }
            let mode = conn.run(&format!("SELECT @@SESSION.{v}"), QUERY_LIMITS).await.map(|r| r.rows);
            let written = w.run(&format!("SELECT COUNT(*) FROM {t}"), QUERY_LIMITS).await.unwrap();
            for what in [format!("TABLE {t}"), format!("FUNCTION {off}"), format!("FUNCTION {off_once}"), format!("PROCEDURE {proc_off}")] {
                w.run(&format!("DROP {what}"), QUERY_LIMITS).await.unwrap();
            }
            assert_eq!(written_after, Vec::<String>::new(), "{}: written after these", server.name);
            assert_eq!(written.rows, [[Some("0".to_owned())]], "{}: nothing written", server.name);
            assert_eq!(mode, Ok(vec![vec![Some("1".to_owned())]]), "{}", server.name);
        }
    }

    /// A `SET` with a character outside ASCII outside its `'…'` strings, by either reading of
    /// backslashes, is refused; one inside a string is not, nor a statement without the word `SET`.
    #[test]
    fn a_set_with_a_name_outside_plain_ascii_is_refused() {
        for sql in [
            "SET SESSION s\u{eb}ssion_track_system_variables = ''",
            "set session \u{17f}ession_track_system_variables = ''",
            "SET @@transact\u{131}on_read_only = 0",
            "SET SESSION `transaction_r\u{eb}ad_only` = 0",
            "SET SESSION \"transaction_r\u{eb}ad_only\" = 0",
            "SET @a = '\u{c1}\\', s\u{eb}ssion_track_system_variables = '', @b = '\u{c1}\\'",
            "SET @gr\u{f6}\u{df}e = 1",
            "UPDATE t SET n = 1 -- \u{e9}",
        ] {
            assert_eq!(raw_refusal(sql, Flavor::MySql), Some(LOOKALIKE), "{sql}");
        }
        for sql in [
            "SET @city = 'Z\u{fc}rich'",
            "SET NAMES utf8mb4",
            "SELECT 'gr\u{f6}\u{df}e', `gr\u{f6}\u{df}e` FROM t",
            "SELECT charset('\u{e9}'), \u{e9}t\u{e9} FROM asset",
            "SET @a = _utf8mb4'\u{e9}', @b = N'\u{e9}'",
        ] {
            assert_eq!(raw_refusal(sql, Flavor::MySql), None, "{sql}");
        }
    }

    /// A statement that still holds a `;` is a block of several: refused, also when a character
    /// set could end a string at a `\` dbee reads as an escape. One in a string or a comment is not.
    #[test]
    fn a_block_of_several_statements_is_refused() {
        for sql in [
            "BEGIN NOT ATOMIC SET SESSION tx_read_only = 0; INSERT INTO t VALUES (1); END",
            "IF 1 THEN SET @@tx_read_only = 0; END IF",
            "SELECT 1 /*! ; */",
            "SET @a = '\u{c1}\\'; INSERT INTO t VALUES (1)",
        ] {
            assert_eq!(raw_refusal(sql, Flavor::MariaDb), Some(BLOCK), "{sql}");
        }
        for sql in ["SELECT ';'", "SELECT 1 /* ; */", "SELECT 1 -- ;", "SELECT `a;b` FROM t"] {
            assert_eq!(raw_refusal(sql, Flavor::MariaDb), None, "{sql}");
        }
    }

    #[test]
    fn diagnostic_statements_are_told_apart() {
        for sql in [
            "SHOW WARNINGS",
            "show errors limit 5",
            "SHOW COUNT(*) WARNINGS",
            "GET DIAGNOSTICS @n = NUMBER",
            "get current diagnostics condition 1 @m = MESSAGE_TEXT",
            "/* x */ SHOW WARNINGS",
        ] {
            assert!(diagnostic(sql, true), "{sql}");
        }
        for sql in ["SELECT 1", "SHOW TABLES", "SHOW VARIABLES LIKE 'warnings'", "SELECT 'show warnings'", ""] {
            assert!(!diagnostic(sql, true), "{sql}");
        }
    }
}
