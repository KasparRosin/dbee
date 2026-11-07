//! Copying grid cells to the clipboard, driven through the control channel one frame at a time in
//! a kittest harness over [`DbeeApp::ui`]: Ctrl+C (values only), Ctrl+Shift+C (with the header)
//! and the right-click menu's formats, in table and query grids; the "Copied n rows" notice; a
//! grid click taking the keyboard from a text field; the copy cap; the in-memory clipboard of
//! isolated runs; and how long a large copy takes. Assertions read `ui.inspect`,
//! `document.inspect` and the text the frames put on the clipboard (egui's `CopyText` output,
//! which kittest hands back and never reaches the system clipboard).
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore`, and reads SSH files from `<temp home>/.ssh` with no agent. The tests connect
//! only to the Docker test servers (`require_testdb!`). The seed is only read; a test that needs
//! its own rows makes a table in `scratch` and drops it.

mod common;

use std::time::{Duration, Instant};

use common::*;
use dbee_engine::COPY_MAX_BYTES;
use dbee_testkit::TestDb;
use dbee_ui_egui::DbeeApp;
use dbee_ui_egui::app::Clipboard;
use egui::{Event, Modifiers, PointerButton};
use serde_json::{Value as Json, json};

/// The slowest frame a copy may cost (the bound the 20 MB cell tests use).
const FRAME_BOUND: Duration = Duration::from_millis(250);

/// [`Ui::connected`], with a new query tab that ran `sql` and shows its results. The tab's editor
/// still has the keyboard, as after Ctrl+Enter.
fn ran(db: &TestDb, sql: &str) -> Ui {
    let mut ui = Ui::connected(db);
    ui.ctrl("T");
    ui.frame();
    ui.text(sql);
    ui.ctrl("Enter");
    ui.until("the run to end", |ui| {
        let q = ui.document()["query"].clone();
        q["running"].is_null() && (q["results"].as_array().is_some_and(|r| !r.is_empty()) || !q["error"].is_null())
    });
    ui.frame();
    assert!(ui.document()["query"]["error"].is_null(), "{}", ui.document()["query"]);
    assert_eq!(ui.editor()["focused"], true, "the editor keeps the keyboard after a run");
    ui
}

impl Ui {
    /// The query editor's tag value: `{cursor, focused, lines, length, highlighted}`.
    fn editor(&mut self) -> Json {
        self.value("query-editor")
    }

    /// The notice shown, if any.
    fn notice(&mut self) -> Option<String> {
        self.tag("notice").and_then(|t| t["value"].as_str().map(str::to_string))
    }

    /// A primary-button drag from the centre of `from` to the centre of `to`, in small steps.
    fn drag(&mut self, from: &str, to: &str) {
        let (a, b) = (self.centre(from), self.centre(to));
        let button = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        self.frame_with(vec![Event::PointerMoved(a)]);
        self.frame_with(vec![button(a, true)]);
        for k in 1..=6 {
            self.frame_with(vec![Event::PointerMoved(a + (b - a) * (k as f32 / 6.0))]);
        }
        self.frame_with(vec![button(b, false)]);
        self.frame();
    }
}

const CUSTOMERS: &str = "select id, name from shop.customers order by id limit 5";

#[test]
fn ctrl_c_copies_values_only() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_clip", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text, b text); INSERT INTO scratch.{t} VALUES (1, NULL, ''), (2, 'x', 'y');")
    });
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.ctrl("A");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [1, 2] }), "Ctrl+A selects every cell");
    ui.ctrl("C");
    ui.frame();
    let copied = ui.copied().expect("Ctrl+C put text on the clipboard");
    // NULL and the empty string are both empty in TSV; no header line.
    assert_eq!(copied, "1\t\t\n2\tx\ty");
    assert!(!copied.starts_with("id"), "{copied}");
    assert_eq!(ui.notice().as_deref(), Some("Copied 2 rows"));
}

#[test]
fn ctrl_shift_c_copies_with_header() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ran(db, CUSTOMERS);
    // A click on a result cell takes the keyboard from the editor: Ctrl+A and Ctrl+Shift+C are
    // the grid's from then on.
    ui.click("grid-cell-1-1");
    assert_eq!(ui.editor()["focused"], false, "the grid took the keyboard");
    ui.ctrl("A");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [4, 1] }));
    ui.ctrl_shift("C");
    ui.frame();
    let copied = ui.copied().expect("Ctrl+Shift+C put text on the clipboard");
    assert!(copied.starts_with("id\tname\n1\tCustomer 1\n"), "{copied}");
    assert_eq!(copied.lines().count(), 6, "the header and five rows: {copied}");
    assert_eq!(ui.notice().as_deref(), Some("Copied 5 rows"));
    assert_eq!(ui.query_text(), CUSTOMERS, "the editor's text is unchanged");
}

#[test]
fn menu_copy_as_markdown() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ran(db, CUSTOMERS);
    ui.right_click("grid-cell-2-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [2, 1], "focus": [2, 1] }), "a right-click selects the cell");
    ui.click("grid-menu-copy-markdown");
    ui.frame();
    let copied = ui.copied().expect("Copy as Markdown put text on the clipboard");
    assert!(copied.starts_with("| "), "{copied}");
    assert_eq!(copied, "| name |\n|---|\n| Customer 3 |");
    assert_eq!(ui.notice().as_deref(), Some("Copied 1 row"));
}

#[test]
fn every_menu_format_copies_from_a_table_tab() {
    let db = dbee_testkit::require_testdb!();
    let table = Scratch::new(db, "t_clip", |t| {
        format!("CREATE TABLE scratch.{t} (id int PRIMARY KEY, a text); INSERT INTO scratch.{t} VALUES (1, NULL), (2, 'x, \"y\"');")
    });
    let mut ui = Ui::connected(db);
    ui.open("scratch", &table.0);
    ui.ctrl("A");
    let expected = [
        ("grid-menu-copy", "1\t\n2\tx, \"y\""),
        ("grid-menu-copy-header", "id\ta\n1\t\n2\tx, \"y\""),
        ("grid-menu-copy-csv", "id,a\n1,\n2,\"x, \"\"y\"\"\""),
        ("grid-menu-copy-json", "[\n  {\"id\": 1, \"a\": null},\n  {\"id\": 2, \"a\": \"x, \\\"y\\\"\"}\n]"),
        ("grid-menu-copy-markdown", "| id | a |\n|---|---|\n| 1 | NULL |\n| 2 | x, \"y\" |"),
    ];
    for (id, text) in expected {
        // The selection covers the cell: the right-click keeps it.
        ui.right_click("grid-cell-1-1");
        ui.click(id);
        ui.frame();
        assert_eq!(ui.copied().as_deref(), Some(text), "{id}");
    }
    assert_eq!(ui.notice().as_deref(), Some("Copied 2 rows"));
}

#[test]
fn a_text_field_with_the_keyboard_keeps_ctrl_c_until_the_grid_is_clicked() {
    let db = dbee_testkit::require_testdb!();
    // The query editor has the keyboard after a run: Ctrl+A and Ctrl+C are its own.
    let mut ui = ran(db, CUSTOMERS);
    ui.execute("selection.set", json!({ "anchor": [0, 0], "focus": [1, 1] }));
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied(), None, "the editor has nothing selected, and the grid's copy did not run");
    assert_eq!(ui.notice(), None);
    ui.ctrl("A");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some(CUSTOMERS), "Ctrl+A, Ctrl+C copy the editor's text");
    assert_eq!(ui.notice(), None, "no grid copy ran");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [1, 1] }), "Ctrl+A left the grid's selection alone");
    // A drag over the cells takes the keyboard as a click does.
    ui.drag("grid-cell-1-0", "grid-cell-2-1");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [1, 0], "focus": [2, 1] }));
    assert_eq!(ui.editor()["focused"], false, "the drag took the keyboard from the editor");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("2\tCustomer 2\n3\tCustomer 3"));
    assert_eq!(ui.notice().as_deref(), Some("Copied 2 rows"));

    // A filter input with the keyboard keeps Ctrl+A and Ctrl+C too.
    let mut ui = Ui::connected(db);
    ui.open("shop", "customers");
    ui.execute("selection.set", json!({ "anchor": [0, 0] }));
    ui.click("filter-0-value");
    ui.text("Ann");
    ui.ctrl("A");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Ann"), "the input's own copy");
    assert_eq!(ui.notice(), None, "no grid copy ran");
    assert_eq!(ui.document()["selection"], json!({ "anchor": [0, 0], "focus": [0, 0] }));
    ui.click("grid-cell-2-1");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("Customer 3"), "after a click on a cell, Ctrl+C is the grid's");
    assert_eq!(ui.notice().as_deref(), Some("Copied 1 row"));
}

#[test]
fn result_grids_offer_no_edits_and_read_only_tables_say_why() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ran(db, CUSTOMERS);
    ui.right_click("grid-cell-1-1");
    for id in ["grid-menu-copy", "grid-menu-copy-header", "grid-menu-copy-csv", "grid-menu-copy-json", "grid-menu-copy-markdown"] {
        assert!(ui.tag(id).is_some_and(|t| t["enabled"] != false), "{id} is offered");
    }
    for id in ["grid-menu-null", "grid-menu-delete"] {
        assert!(ui.tag(id).is_none(), "a query's results list no {id}");
    }
    // A view is read-only: the entries are listed, disabled, with the engine's reason.
    let mut ui = Ui::connected(db);
    ui.open("shop", "order_totals");
    ui.right_click("grid-cell-0-0");
    for (id, label) in [("grid-menu-null", "Set as NULL"), ("grid-menu-delete", "Delete row")] {
        let tag = ui.tag(id).unwrap_or_else(|| panic!("a table tab lists {id}"));
        assert_eq!(tag["enabled"], false, "{tag}");
        assert_eq!(tag["value"]["label"], label, "{tag}");
        assert_eq!(tag["value"]["why"], "views are read-only", "{tag}");
    }
    let copy = ui.value("grid-menu-copy");
    assert_eq!((copy["label"].clone(), copy["shortcut"].clone(), copy.get("why")), (json!("Copy"), json!("Ctrl+C"), None), "{copy}");
}

#[test]
fn a_copy_over_the_cap_copies_nothing_and_says_why() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = Ui::connected(db);
    // Its big cells hold 20,000,000 bytes of text each, more than a copy takes.
    ui.open("shop", "big_cell");
    ui.until("the big row", |ui| ui.document()["table"]["rows"] == 1);
    ui.execute("selection.set", json!({ "anchor": [0, 1], "focus": [0, 2] }));
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copy", "params": { "format": "tsv" } }));
    assert_eq!(r["code"], "disabled", "{r}");
    let why = r["data"]["why"].as_str().unwrap_or_default().to_string();
    assert!(why.contains(&cap()) && why.contains("select fewer rows"), "{why}");
    ui.click("grid-cell-0-1");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied(), None, "nothing went to the clipboard");
    let notice = ui.notice().unwrap_or_default();
    let reason = format!("The selection holds 19.1 MiB of cell text, more than the {} a copy can take; select fewer rows.", cap());
    assert_eq!(notice, reason, "the shortcut shows the reason, as a sentence");
    // The row's key fits.
    ui.click("grid-cell-0-0");
    ui.ctrl("C");
    ui.frame();
    assert_eq!(ui.copied().as_deref(), Some("1"));
}

/// The cap as the reasons give it: `8 MiB`.
fn cap() -> String {
    format!("{} MiB", COPY_MAX_BYTES / (1024 * 1024))
}

/// [`Ui::connected`] with the in-memory clipboard of isolated runs.
fn connected_in_memory(db: &TestDb) -> Ui {
    Ui::connected_with(
        db,
        Setup { app: Some(Box::new(|session, rx| DbeeApp::new(session, Some(rx)).with_clipboard(Clipboard::Memory))), ..Setup::default() },
    )
}

#[test]
fn the_memory_clipboard_keeps_the_copy_in_the_app() {
    let db = dbee_testkit::require_testdb!();
    // By default nothing is kept, and inspect has no clipboard.
    let mut ui = Ui::connected(db);
    let d = ui.document();
    assert_eq!((d.get("clipboard"), d.get("clipboardBytes")), (None, None));
    let mut ui = connected_in_memory(db);
    let d = ui.document();
    assert_eq!((d["clipboard"].clone(), d["clipboardBytes"].clone()), (Json::Null, Json::Null), "nothing copied yet");
    ui.ctrl("T");
    ui.execute("query.setText", json!({ "text": CUSTOMERS }));
    ui.execute("query.run", json!({}));
    ui.until("the results", |ui| ui.document()["query"]["results"].as_array().is_some_and(|r| !r.is_empty()));
    ui.frame();
    ui.click("grid-cell-0-0");
    ui.ctrl("A");
    ui.ctrl_shift("C");
    ui.frame();
    let r = ui.execute("clipboard.copy", json!({ "format": "csv" }));
    assert_eq!((r["rows"].clone(), r["columns"].clone()), (json!(5), json!(2)), "{r}");
    let d = ui.document();
    let kept = d["clipboard"].clone();
    assert!(kept.as_str().is_some_and(|t| t.starts_with("id,name\n1,Customer 1\n")), "the last copy: {kept}");
    assert_eq!(r["bytes"].as_u64(), kept.as_str().map(|t| t.len() as u64), "{r}");
    assert_eq!(d["clipboardBytes"], r["bytes"]);
    assert_eq!(ui.notice().as_deref(), Some("Copied 5 rows"));
    assert!(ui.copied.is_empty(), "no copy reached egui: {:?}", ui.copied.iter().map(String::len).collect::<Vec<_>>());
    // A long copy shows its first 4 KiB, cut on a character boundary, and its whole length.
    ui.execute("query.setText", json!({ "text": "select 'a' || repeat('é', 3000) as e" }));
    ui.execute("query.run", json!({}));
    ui.until("the long value", |ui| ui.document()["query"]["results"][0]["columns"][0]["name"] == "e");
    ui.execute("selection.all", json!({}));
    let r = ui.execute("clipboard.copy", json!({ "format": "tsv" }));
    let d = ui.document();
    assert_eq!((r["bytes"].clone(), d["clipboardBytes"].clone()), (json!(6_001), json!(6_001)));
    assert_eq!(d["clipboard"].as_str(), Some(format!("a{}", "é".repeat(2_047)).as_str()), "4,095 bytes: the next 'é' would split");
    assert!(ui.copied.is_empty());
}

#[test]
fn the_memory_clipboard_catches_text_field_copies_and_cuts() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = connected_in_memory(db);
    // The query editor's own copy (Ctrl+A, Ctrl+C) is egui's TextEdit's, not the grid's.
    ui.ctrl("T");
    ui.text(CUSTOMERS);
    ui.ctrl("A");
    ui.ctrl("C");
    ui.frame();
    assert!(ui.copied.is_empty(), "no copy reached egui: {:?}", ui.copied);
    let d = ui.document();
    assert_eq!((d["clipboard"].as_str(), d["clipboardBytes"].as_u64()), (Some(CUSTOMERS), Some(CUSTOMERS.len() as u64)));
    // A filter input's cut.
    ui.open("shop", "customers");
    ui.click("filter-0-value");
    ui.text("Ann");
    ui.ctrl("A");
    ui.ctrl("X");
    ui.frame();
    assert!(ui.copied.is_empty(), "no cut reached egui: {:?}", ui.copied);
    assert_eq!(ui.document()["clipboard"], "Ann");
    assert_eq!(ui.value("filter-0-value"), "", "the cut took the text out");
}

/// Copies `sql`'s whole result in `format` with `clipboard.copy`, in a frame of its own, and
/// checks the outcome: how many bytes it copied (`None`: refused), and how long the frame took.
fn copy_in_a_frame(ui: &mut Ui, format: &str) -> (Option<usize>, Duration) {
    ui.copied.clear();
    ui.time_frames();
    let r = ui.call("engine.execute", json!({ "command": "clipboard.copy", "params": { "format": format } }));
    let frame = ui.slowest;
    let copied = ui.copied().map(|t| t.len());
    match r["ok"].as_bool() {
        Some(true) => assert_eq!(copied, r["result"]["bytes"].as_u64().map(|n| n as usize), "{format}: {r}"),
        _ => {
            assert_eq!((r["code"].as_str(), copied), (Some("disabled"), None), "{format}: {r}");
            let why = r["data"]["why"].as_str().unwrap_or_default();
            assert!(why.contains(&cap()) && why.ends_with("select fewer rows"), "{why}");
        }
    }
    (copied, frame)
}

const FORMATS: [&str; 5] = ["tsv", "tsvHeader", "csv", "json", "markdown"];

/// The middle one of `times`.
fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times.get(times.len() / 2).copied().unwrap_or_default()
}

/// The full 10,000-row result of a join (12 columns, two of them JSON).
const FULL: &str = "select o.*, i.line_no, i.product, i.qty from shop.order_items i join shop.orders o on o.id = i.order_id order by i.order_id, i.line_no limit 10000";

/// One row whose cells fit the cap, but whose JSON (`\u0001`, six bytes each) and Markdown
/// (`<br>`) do not, nor its TSV and CSV by one byte (the line breaks are quoted).
fn escaped() -> String {
    let half = COPY_MAX_BYTES / 2 - 1;
    format!("select repeat(chr(1), {half}) as a, repeat(E'\\n', {half}) as b")
}

/// Copies the full 10,000-row result and the escaped row in each format: the first is copied, the
/// second refused. The frame each took, by what and format.
fn large_copies(db: &TestDb) -> Vec<(String, Duration)> {
    let mut frames = Vec::new();
    for (what, sql) in [("10,000 rows", FULL.to_string()), ("escaped", escaped())] {
        let mut ui = ran(db, &sql);
        ui.click("grid-cell-0-0");
        ui.ctrl("A");
        for format in FORMATS {
            let (copied, frame) = copy_in_a_frame(&mut ui, format);
            assert_eq!(copied.is_some(), what == "10,000 rows", "{what} as {format}: {copied:?}");
            frames.push((format!("{what} as {format}"), frame));
        }
    }
    frames
}

#[test]
fn a_large_copy_is_copied_or_refused() {
    let db = dbee_testkit::require_testdb!();
    large_copies(db);
}

/// A guard against a frozen window, not a measurement (`copy_frames_within_budget` measures): no
/// copy of a large result, copied or refused, takes a frame of a second.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn no_copy_freezes_the_window() {
    let db = dbee_testkit::require_testdb!();
    for (what, frame) in large_copies(db) {
        println!("copying {what}: a {frame:?} frame");
        assert!(frame < Duration::from_secs(1), "copying {what} took a {frame:?} frame");
    }
}

/// How long a copy takes, from the request through to the clipboard (the frame that runs
/// `clipboard.copy` and hands the text to egui), copied or refused, in each format: the full
/// 10,000-row result; 10,000 rows whose TSV is one byte under the cap; one row of control
/// characters (JSON escapes each as six bytes); one row of line breaks (`<br>` in Markdown); and
/// 10,000 rows of 1,664 NULLs. Budget: 250 ms per frame.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn copy_frames_within_budget() {
    let db = dbee_testkit::require_testdb!();
    // TSV of one column is the cells and 9,999 line breaks.
    let cells = COPY_MAX_BYTES - 1 - 9_999;
    let (pad, rest) = (cells / 10_000, cells % 10_000);
    let under = format!("select repeat('x', {pad}) || case when n = 1 then repeat('y', {rest}) else '' end as pad from generate_series(1, 10000) n");
    let control = format!("select repeat(chr(1), {}) as a", COPY_MAX_BYTES - 1);
    let breaks = format!("select repeat(E'\\n', {}) as a", COPY_MAX_BYTES - 1);
    // Every `"` is doubled in CSV (and escaped in JSON): half the cap, so CSV ends just under it.
    let quotes = format!("select repeat('\"', {}) as a", COPY_MAX_BYTES / 2 - 2);
    let nulls: Vec<String> = (1..=1_664).map(|i| format!("null as c{i:04}")).collect();
    let wide = format!("select {} from generate_series(1, 10000)", nulls.join(", "));
    println!("copy frames on {}", machine());
    // Medians, as this machine's load comes and goes. Frames count where a frame without a copy
    // is within the bound: a grid of 1,664 columns is not (its frames vary by hundreds of
    // milliseconds with or without a copy), so there the copy is judged alone.
    let (mut slowest_copy, mut slowest_frame) = (Duration::ZERO, Duration::ZERO);
    for (what, sql) in [
        ("10,000 rows", FULL.to_string()),
        ("TSV 1 byte under the cap", under),
        ("control characters", control),
        ("line breaks", breaks),
        ("double quotes", quotes),
        ("10,000 × 1,664 NULLs", wide),
    ] {
        let mut ui = ran(db, &sql);
        ui.click("grid-cell-0-0");
        ui.ctrl("A");
        let idle = median(
            (0..5)
                .map(|_| {
                    ui.time_frames();
                    ui.frame();
                    ui.slowest
                })
                .collect(),
        );
        println!("  {what} (a frame without a copy: {idle:?}):");
        for format in FORMATS {
            // `clipboard.copy` on its own, outside a frame.
            let alone: Vec<Duration> = (0..5)
                .map(|_| {
                    let start = Instant::now();
                    let _ = ui.harness.state_mut().run("clipboard.copy", json!({ "format": format }));
                    let took = start.elapsed();
                    ui.frame();
                    took
                })
                .collect();
            let worst = alone.iter().max().copied().unwrap_or_default();
            let alone = median(alone);
            let runs: Vec<(Option<usize>, Duration)> = (0..3).map(|_| copy_in_a_frame(&mut ui, format)).collect();
            let outcome = runs.first().and_then(|r| r.0).map_or_else(|| "refused".to_string(), |n| format!("{n} bytes"));
            let frame = median(runs.iter().map(|r| r.1).collect());
            println!("    {format}: {outcome}; clipboard.copy alone {alone:?} (slowest {worst:?}), its frame {frame:?}");
            slowest_copy = slowest_copy.max(alone);
            if idle < FRAME_BOUND {
                slowest_frame = slowest_frame.max(frame);
            }
        }
    }
    println!("slowest median: clipboard.copy alone {slowest_copy:?}; a frame with a copy {slowest_frame:?}");
    assert!(slowest_copy < FRAME_BOUND, "the slowest copy took {slowest_copy:?}");
    assert!(slowest_frame < FRAME_BOUND, "the slowest frame with a copy took {slowest_frame:?}");
}

/// The machine and its load average, which a timing test prints with its numbers
/// (docs/testing.md §6).
fn machine() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split(':').nth(1)).map(|m| m.trim().to_string()))
        .unwrap_or_else(|| "an unknown CPU".into());
    let threads = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "unknown".into());
    format!("{} {}, {cpu}, {threads} threads, load average {load}", std::env::consts::OS, std::env::consts::ARCH)
}

impl Ui {
    /// The active query tab's text.
    fn query_text(&mut self) -> String {
        self.document()["query"]["text"].as_str().unwrap_or_default().to_string()
    }
}
