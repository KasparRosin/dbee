//! The M2 day end to end in the real window, on three engines: a copy of the SQLite seed opened
//! with `dbee <file>`, MySQL 8.0 (`mysql80`) through the test sshd, and the newest MariaDB
//! (`mariadbnew`) directly. It drives `dbee --control <port>` on a throwaway home with the
//! environment of `cargo xtask run-isolated`, only through the control channel
//! (`docs/control-protocol.md`), checks `document.inspect` after each step and saves a screenshot
//! of each as `target/m2-day/<n>-<engine>-<step>.png`, to look at.
//!
//! The day, in one window:
//! 1. `dbee <file>` opens the SQLite copy, unsaved; Save connection in the status bar saves it.
//! 2. On it: Ctrl+P `ord`, three filters, the JSON panel, the foreign key, an edit, a new row and
//!    a deleted row in one Apply, then a query tab: several result sets, a cancelled statement and
//!    a join copied with its header.
//! 3. Ctrl+Shift+Q, then New connection: the engine cards, PostgreSQL, Change engine, MySQL. The
//!    form through the SSH tunnel; the Database field's list picks `shop` (#1); Save, Connect.
//! 4. The same steps on MySQL, whose query tab's database chip says `shop`, the only database the
//!    connection lists.
//! 5. Ctrl+P `maria` switches to the saved MariaDB connection (#3), which names no database,
//!    through the question about the query text; the same steps there, the chip moving the query tab
//!    to `many`; Ctrl+P switches back to the SQLite file.
//!
//! It opens a window on the desktop, so it runs by hand only, with the test servers up and the
//! window shown (a hidden window runs no frames on Wayland, `docs/testing.md` §4):
//!
//! ```sh
//! cargo xtask testdb up
//! cargo test --workspace --test m2_day -- --ignored --nocapture
//! ```
//!
//! It prints `SKIPPED` without the test servers (with `DBEE_REQUIRE_ORACLES=1` it fails instead)
//! or without a display (`DISPLAY` or `WAYLAND_DISPLAY`).
//!
//! Safety: the app gets an isolated run's environment (`common`): a throwaway home with the test
//! key and the sshd's host key, in-memory secrets and clipboard, no SSH agent, the test servers as
//! its only targets and the home and `target/` as its only folders. The SQLite file is a fresh copy
//! of the seed under `target/testkit/copies`, removed at the end. On the servers the seed is
//! read-only: the day edits customers and an order of its own, which the test inserts into `shop`
//! and deletes when it ends, also when it fails, with the row the day adds. Each server's
//! customers and orders are checked to be as before once they are gone. A run that is killed
//! cannot delete its rows, so every run first removes those of earlier runs by their marker
//! ([`MARK`], the customers' `note`): their orders, then the customers.
//!
//! Running statements are held as the engine's tests hold them: on a server, by a lock a session of
//! the test's own holds (`TestTarget::hold_lock`); a file has no other session, so a count that
//! takes minutes stands in, and the cancel ends it.

mod common;

use std::path::Path;

use common::App;
use dbee_testkit::{Held, Kind, TestTarget, temp_home, unique};
use dbee_ui_egui::value_panel::{LINE_H, PAD};
use serde_json::{Value as Json, json};

/// The `note` of the customers the day inserts and adds: the marker by which a later run finds and
/// removes the rows a killed run left.
const MARK: &str = "the M2 day test row";

/// Where the screenshots go, from the repository root (the app's working directory).
const SHOTS: &str = "target/m2-day";

/// One engine of the day: its target, the prefix of its screenshots, its query tab's database chip
/// (on MySQL and MariaDB), and an unqualified statement with the first cell it gives where it runs.
struct Engine {
    t: TestTarget,
    shots: &'static str,
    chip: Option<Chip>,
    first: (&'static str, &'static str),
}

/// The databases the chip lists, and the one the day chooses with it.
struct Chip {
    lists: &'static [&'static str],
    choose: &'static str,
}

/// `t_001` exists only in `many` (on SQLite, in `main`).
const IN_MANY: (&str, &str) = ("SELECT v FROM t_001", "row of table 1");
/// `customers` exists only in `shop`.
const IN_SHOP: (&str, &str) = ("SELECT name FROM customers WHERE id = 1", "Customer 1");

impl Engine {
    /// The schema the seed's `shop` is on this engine (SQLite's `main`).
    fn shop(&self) -> &'static str {
        self.t.schema("shop")
    }

    fn q(&self, sql: &str) -> String {
        self.t.sql(sql).unwrap_or_else(|e| panic!("{}: {e}", self.t.name())).trim().to_string()
    }
}

/// `n` with a comma between thousands, as the app writes counts.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Removes the day's rows: the orders of the customers marked [`MARK`], then those customers.
fn sweep_sql(e: &Engine) -> String {
    let shop = e.shop();
    format!(
        "DELETE FROM {shop}.orders WHERE customer_id IN (SELECT id FROM {shop}.customers WHERE note = '{MARK}');
         DELETE FROM {shop}.customers WHERE note = '{MARK}'"
    )
}

/// Counts and sums over the customers and orders, to show the day left them as they were.
fn digest(e: &Engine) -> String {
    let shop = e.shop();
    e.q(&format!(
        "SELECT COUNT(*), SUM(id), SUM(LENGTH(name)), SUM(LENGTH(COALESCE(email, ''))), SUM(LENGTH(COALESCE(note, ''))) FROM {shop}.customers;
         SELECT COUNT(*), SUM(id), SUM(customer_id), SUM(dealer_id), SUM(total), COUNT(shipped_at) FROM {shop}.orders"
    ))
}

/// The day's own rows on one engine, removed when dropped.
struct DayRows<'a> {
    e: &'a Engine,
    /// The customer with the order, named with a typo to fix: `Custmer m2day_<hex>`.
    customer: String,
    /// A second customer, without orders, that the day deletes, and its email (on SQLite the
    /// customer the day adds may take its id).
    extra: String,
    extra_email: String,
    order: String,
    typo: String,
    /// The name fixed: `Customer m2day_<hex>`.
    fixed: String,
    /// The name of the customer the day adds.
    added: String,
}

impl<'a> DayRows<'a> {
    /// Two customers, and an order of the first that passes the day's three filters (dealer 42,
    /// completed, a total above 100), with a `billing` JSON holding the customer's name.
    fn insert(e: &'a Engine) -> DayRows<'a> {
        let tag = unique("m2day");
        let shop = e.shop();
        let typo = format!("Custmer {tag}");
        let billing = format!(
            r#"{{"name": "{typo}", "address": {{"street": "1 Day Street", "city": "Testville", "zip": "04242"}}, "items": [{{"sku": "SKU-DAY", "qty": 2}}], "paid": true}}"#
        );
        let (first, second) = (format!("{tag}@example.com"), format!("{tag}.extra@example.com"));
        let id_of = |email: &str| format!("(SELECT id FROM {shop}.customers WHERE email = '{email}')");
        let out = e.q(&format!(
            "INSERT INTO {shop}.customers (name, email, note) VALUES ('{typo}', '{first}', '{MARK}'), ('Extra {tag}', '{second}', '{MARK}');
             INSERT INTO {shop}.orders (customer_id, dealer_id, status, total, billing, shipped_at, raw_json_text)
                 SELECT id, 42, 'completed', 4242.42, '{billing}', '2026-10-26', '{{\"k\":[1,2]}}' FROM {shop}.customers WHERE email = '{first}';
             SELECT (SELECT id FROM {shop}.orders WHERE customer_id = {}), {}, {}",
            id_of(&first),
            id_of(&first),
            id_of(&second)
        ));
        let ids: Vec<&str> = out.split('|').collect();
        let [order, customer, extra] = ids[..] else { panic!("{}: insert: {out}", e.t.name()) };
        DayRows {
            e,
            customer: customer.into(),
            extra: extra.into(),
            extra_email: second,
            order: order.into(),
            typo,
            fixed: format!("Customer {tag}"),
            added: format!("Added {tag}"),
        }
    }
}

impl Drop for DayRows<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.e.t.sql(&sweep_sql(self.e)) {
            eprintln!("m2_day: could not delete its rows on {}: {e}", self.e.t.name());
        }
    }
}

/// Opens the editor of cell `(row, column)`, types `text` over its value and commits it: a
/// several-line one (in the value panel) with SAVE when `save`, else with Ctrl+Enter.
fn edit_cell(app: &mut App, row: usize, column: usize, text: &str, save: bool) {
    app.double_click(&format!("grid-cell-{row}-{column}"));
    let editor = app.value("cell-editor");
    app.ctrl("A");
    app.text(text);
    // A multi-line editor is the value panel's and takes Enter as a newline.
    if editor["multiline"] == true {
        assert!(app.inside("cell-editor", "value-panel"), "several lines are edited in the value panel");
        if save {
            app.click("value-save");
        } else {
            app.ctrl("Enter");
        }
        assert!(app.tag("cell-editor").is_none(), "the editor closed");
    } else {
        app.key("Enter");
    }
    let cell = app.cell(row, column);
    assert_eq!(cell["text"].as_str(), Some(text), "{cell}");
}

/// Shows result `n` (1-based) with its tab in the result bar, or from the ⌄ list, pictured as `shot`.
fn show_result(app: &mut App, root: &Path, n: u64, shot: &str) {
    let button = format!("result-{n}");
    if app.tag(&button).is_some_and(|t| t["visible"] != false) {
        app.click(&button);
    } else {
        let item = format!("result-list-{n}");
        app.click("result-more");
        app.until("the list of results", |app| app.tag(&item).is_some());
        app.shot(root, shot);
        app.click(&item);
    }
    app.until("the result", |app| app.document()["query"]["shown"] == n - 1);
}

/// The query tab's run has ended.
fn query_done(app: &mut App) -> Json {
    app.until("the run to end", |app| app.document()["query"]["running"].is_null());
    app.document()["query"].clone()
}

/// Ctrl+P, `query`, Enter on the saved connection `name` (of type `label`): the question about
/// the query text left in the open connection (`from`), Close anyway, and the window on the
/// workspace connected to it.
fn switch(app: &mut App, root: &Path, shot: &str, query: &str, name: &str, label: &str, from: &str) -> Json {
    app.ctrl("P");
    app.text(query);
    app.until("the connection's hit", |app| app.tag("qs-item-0").is_some_and(|t| t["value"] == name));
    assert_eq!(app.value("qs-item-0-kind"), label);
    app.shot(root, shot);
    app.key("Enter");
    app.until("the question", |app| app.inspect()["dialog"] == "confirm");
    assert_eq!(app.value("dialog-text"), format!("Disconnect from {from}? The text of 1 query tab will be lost."));
    app.click("dialog-ok");
    app.until("the workspace", |app| {
        let d = app.document();
        d["screen"] == "workspace" && d["connection"]["name"] == name && d["jobs"] == json!([])
    });
    let d = app.document();
    assert_eq!((d["tabs"].clone(), app.inspect()["dialog"].clone()), (json!([]), Json::Null), "{d}");
    d["connection"].clone()
}

/// A statement of `marker` that runs until it is cancelled, and what holds it: on a server a lock
/// a session of the test's own holds; a file has no other session, so a count of minutes.
fn held_statement(e: &Engine, marker: &str) -> (String, Option<Held>) {
    let shop = e.shop();
    if e.t.server().is_some() {
        let key = unique("dbee_m2day_lock");
        let held = e.t.hold_lock(&key).unwrap();
        (format!("SELECT '{marker}' AS tag FROM {shop}.orders WHERE {} LIMIT 1", e.t.waits_for_lock_sql(&key)), Some(held))
    } else {
        (format!("SELECT '{marker}' AS tag FROM {shop}.orders WHERE {} LIMIT 1", e.t.slow_condition_sql(600)), None)
    }
}

/// Steps 2 to 7 of the day on the open connection: Ctrl+P, the filters, the JSON panel, the
/// foreign key, the edits in one Apply, the query tab and a copy.
fn the_day(app: &mut App, root: &Path, e: &Engine, day: &DayRows) {
    let (p, shop, name) = (e.shots, e.shop(), e.t.name());
    let info = e.t.kind().info();

    // 2. Ctrl+P, `ord`, Enter: the orders tab.
    app.ctrl("P");
    app.text("ord");
    let orders = format!("{shop}.orders");
    app.until("the quick search's hits", |app| app.tag("qs-item-0").is_some_and(|t| t["value"] == orders.as_str()));
    app.shot(root, &format!("{p}-2a-quick-search"));
    app.key("Enter");
    let t = app.settled("the orders page");
    assert_eq!((t["schema"].clone(), t["name"].clone(), t["rows"].clone()), (json!(shop), json!("orders"), json!(100)), "{name}: {t}");
    assert!(app.tag("qs-input").is_none(), "Enter closed the quick search");
    let orders_tab = app.document()["active"].as_u64().unwrap();
    assert_eq!(app.value(&format!("tab-{orders_tab}")), "orders");
    app.shot(root, &format!("{p}-2b-orders"));
    // The table bar's controls stand apart, inside the window.
    app.assert_apart(&["table-view-data", "table-refresh", "row-add", "page-count", "table-import", "table-export"], &format!("{name}: 2b"));
    // The table bar between the filter bar and the rows: DATA chosen, STRUCTURE off, EXPORT off
    // (when it has room), the estimate (none on SQLite: COUNT ROWS); a click counts exactly.
    assert_eq!(app.value("table-view-data"), true, "{name}");
    assert_eq!(app.tag("table-view-structure").map(|e| e["enabled"].clone()), Some(json!(false)), "{name}: STRUCTURE off");
    assert!(app.tag("table-export").is_none_or(|e| e["enabled"] == false), "{name}: EXPORT off");
    let (filter, refresh, header) = (app.rect("filter-0-column"), app.rect("table-refresh"), app.rect("grid-header-0"));
    assert!(
        filter[1] < refresh[1] && refresh[1] < header[1],
        "{name}: the table bar under the filters, over the rows: {filter:?} {refresh:?} {header:?}"
    );
    let count = app.value("page-count");
    assert!(count == "Count rows" || count.as_str().is_some_and(|v| v.starts_with('~')), "{name}: the estimate or COUNT ROWS: {count}");
    app.click("page-count");
    app.until("the count", |app| app.table()["total"]["exact"].is_u64());
    let n = app.table()["total"]["exact"].as_u64().unwrap();
    assert_eq!(n.to_string(), e.q(&format!("SELECT COUNT(*) FROM {shop}.orders")), "{name}: the server's count");
    assert_eq!(app.value("table-rows"), format!("1–100 of {} rows", thousands(n)), "{name}");
    app.shot(root, &format!("{p}-2c-counted"));
    // 2d. STRUCTURE (through the control channel until the window draws it): the tab reads the
    //     structure, and DATA comes back with its rows.
    app.execute("table.view", json!({ "view": "structure" }));
    app.until("the structure read", |app| app.table()["structure"]["shown"].is_object());
    assert_eq!(app.table()["structure"]["shown"]["columns"], 9, "{name}: the orders' columns");
    app.execute("table.view", json!({ "view": "data" }));
    assert_eq!((app.table()["view"].clone(), app.table()["rows"].clone()), (json!("data"), json!(100)));
    // 2e. On a server, a first load held by a table lock shows the shipments' header over rows
    //     of bars; once it loads the tab closes, back to orders. (A file has no other session.)
    if let Some(server) = e.t.server() {
        let lock = match e.t.kind() {
            Kind::Postgres => format!("LOCK TABLE {shop}.shipments IN ACCESS EXCLUSIVE MODE"),
            _ => format!("LOCK TABLES {shop}.shipments WRITE"),
        };
        let held = dbee_testkit::hold(server, &lock).unwrap();
        app.ok("engine.execute", json!({ "command": "table.open", "params": { "schema": shop, "table": "shipments" }, "wait": false }));
        app.until("the skeleton", |app| app.tag("grid-skeleton").is_some());
        assert!(app.tag("status-job").is_none(), "{name}: a page load is no footer job");
        let shipments_tab = app.document()["active"].as_u64().unwrap();
        app.shot(root, &format!("{p}-2e-skeleton"));
        drop(held);
        let t = app.settled("the shipments page");
        assert_eq!(t["name"], "shipments", "{name}: {t}");
        assert!(app.tag("grid-skeleton").is_none(), "{name}: the rows in its place");
        app.execute("tab.close", json!({ "tab": shipments_tab, "force": true }));
        app.execute("tab.activate", json!({ "tab": orders_tab }));
        assert_eq!(app.table()["name"], "orders", "{name}");
    }

    // 3. The three filters through the bar, then Enter in the last value field.
    app.click("filter-add");
    app.click("filter-add");
    for (n, (column, op, value)) in [("dealer_id", "eq", "42"), ("status", "eq", "completed"), ("total", "gt", "100")].iter().enumerate() {
        app.click(&format!("filter-{n}-column"));
        app.click(&format!("filter-{n}-column-{column}"));
        app.click(&format!("filter-{n}-op"));
        app.click(&format!("filter-{n}-op-{op}"));
        app.click(&format!("filter-{n}-value"));
        app.text(value);
    }
    app.key("Enter");
    app.until("the three filters applied", |app| app.table()["applied"]["filters"].as_array().map(Vec::len) == Some(3));
    let t = app.settled("the filtered page");
    assert_eq!(
        t["applied"]["filters"],
        json!([
            { "column": "dealer_id", "op": "eq", "value": "42" },
            { "column": "status", "op": "eq", "value": "completed" },
            { "column": "total", "op": "gt", "value": "100" },
        ]),
        "{name}: {t}"
    );
    assert_eq!(app.value(&format!("tab-{orders_tab}")), "orders [3]");
    let header = |app: &mut App, col: usize| app.value(&format!("grid-header-{col}"))["name"].as_str().unwrap_or_default().to_string();
    let names: Vec<String> = (0..9).map(|c| header(app, c)).collect();
    assert_eq!(names, ["id", "customer_id", "dealer_id", "status", "total", "billing", "meta", "shipped_at", "raw_json_text"], "{name}");
    let rows = app.grid();
    assert_eq!(rows.len() as u64, t["rows"].as_u64().unwrap(), "{name}: every loaded row is listed");
    assert!(rows.len() > 1, "{name}: the seed's rows and the day's own: {}", rows.len());
    for (r, row) in rows.iter().enumerate() {
        let total: f64 = row[4]["text"].as_str().unwrap().parse().unwrap();
        assert!(row[2]["text"] == "42" && row[3]["text"] == "completed" && total > 100.0, "{name}: row {r} passes the filters: {row:?}");
    }
    let r =
        rows.iter().position(|row| row[0]["text"] == day.order.as_str()).unwrap_or_else(|| panic!("{name}: the day's order {} is listed", day.order));
    assert_eq!(rows[r][1]["text"], day.customer.as_str());
    app.shot(root, &format!("{p}-3-filters"));

    // 4. The billing cell's JSON in the value panel; its second line selected and copied.
    app.click(&format!("grid-cell-{r}-5"));
    let d = app.document();
    assert_eq!(d["selection"], json!({ "anchor": [r, 5], "focus": [r, 5], "whole": false }));
    let v = &d["value"];
    assert_eq!(
        (v["column"].clone(), v["typeLabel"].clone(), v["kind"].clone(), v["pretty"].clone(), v["isNull"].clone()),
        (json!("billing"), json!(info.plain_json_label), json!("json"), json!(true), json!(false)),
        "{name}: {v}"
    );
    let panel = app.value("value-panel");
    assert_eq!((panel["column"].clone(), panel["type"].clone()), (json!("billing"), json!(info.plain_json_label)), "{name}: {panel}");
    app.panel_between_the_bars(&format!("{name}, step 4"));
    let text = app.value("value-text")["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("{\n") && text.contains("\"city\": \"Testville\""), "{name}: formatted: {text}");
    let lines: Vec<&str> = text.split('\n').collect();
    let line = lines[1];
    assert!(line.contains(&day.typo), "{name}: the second line holds the name: {text}");
    let start = lines[0].len() + 1;
    app.select_value_line(1, start, line.len(), f64::from(PAD), f64::from(LINE_H));
    app.ctrl("C");
    let d = app.document();
    assert_eq!((d["clipboard"].as_str(), d["clipboardBytes"].as_u64()), (Some(line), Some(line.len() as u64)), "{name}: the line was copied");
    // `value.copy` gives the whole value as stored: the same JSON as the panel's pretty text.
    let copied = app.execute("value.copy", json!({}));
    assert_eq!((copied["bytes"].clone(), copied["row"].clone()), (d["value"]["bytes"].clone(), json!(false)), "{name}: {copied}");
    let parsed: Json = serde_json::from_str(copied["text"].as_str().unwrap()).unwrap();
    assert_eq!(parsed, serde_json::from_str::<Json>(&text).unwrap(), "{name}: the stored text is the shown JSON");
    app.shot(root, &format!("{p}-4-json"));

    // 5. The order's row selected, its customer_id link: a new customers tab filtered to the id.
    app.click(&format!("grid-cell-{r}-1"));
    app.click(&format!("fk-link-{r}-1"));
    app.until("the customers tab", |app| app.table()["name"] == "customers");
    let t = app.settled("the customer's page");
    assert_eq!(t["filters"], json!([{ "column": "id", "op": "eq", "value": day.customer }]), "{name}: {t}");
    assert_eq!((t["applied"]["filters"].clone(), t["rows"].clone()), (t["filters"].clone(), json!(1)), "{name}: {t}");
    let customers_tab = app.document()["active"].as_u64().unwrap();
    assert_ne!(customers_tab, orders_tab, "a new tab");
    assert_eq!(app.value(&format!("tab-{customers_tab}")), "customers [1]");
    assert_eq!(app.cell(0, 1)["text"], day.typo.as_str());
    app.shot(root, &format!("{p}-5-customer"));

    // 6. On the day's customers: a name fixed, one deleted, one added, in one Ctrl+S.
    app.click("filter-0-column");
    app.click("filter-0-column-note");
    app.replace("filter-0-value", MARK);
    app.key("Enter");
    app.until("the day's customers", |app| app.table()["applied"]["filters"][0]["column"] == "note");
    let t = app.settled("the day's customers");
    assert_eq!(t["rows"], 2, "{name}: {t}");
    assert_eq!((app.cell(0, 0)["text"].clone(), app.cell(1, 0)["text"].clone()), (json!(day.customer), json!(day.extra)), "{name}");
    // Several lines are saved with SAVE on Postgres and with Ctrl+Enter on the others, so both
    // ways run each day.
    let save = e.t.kind() == Kind::Postgres;
    edit_cell(app, 0, 1, &day.fixed, save);
    app.right_click("grid-cell-1-1");
    app.click("grid-menu-delete");
    assert_eq!(app.cell(1, 1)["deleted"], true);
    app.click("row-add");
    assert_eq!(app.cell(2, 0)["state"], "new");
    edit_cell(app, 2, 1, &day.added, save);
    edit_cell(app, 2, 4, MARK, save);
    assert_eq!(app.table()["edits"]["count"], 3, "{name}: a changed cell, a row to delete and a new row");
    app.click("grid-cell-0-1");
    // The paging bar counts them; paging and REFRESH wait for them.
    assert_eq!(app.value("edits-count"), "3 changes", "{name}");
    for id in ["page-next", "table-refresh"] {
        assert_eq!(app.tag(id).map(|e| e["enabled"].clone()), Some(json!(false)), "{name}: `{id}` waits for the staged changes");
    }
    app.shot(root, &format!("{p}-6a-staged"));
    app.ctrl("S");
    app.until("the Apply", |app| app.table()["edits"]["count"] == 0);
    let t = app.settled("the Apply");
    assert!(app.tag("edits-count").is_none(), "{name}: nothing staged, no count of changes");
    assert!(t["failed"].is_null() && t["error"].is_null(), "{name}: {t}");
    assert_eq!(e.q(&format!("SELECT name FROM {shop}.customers WHERE id = {}", day.customer)), day.fixed, "{name}: the name is fixed");
    assert_eq!(
        e.q(&format!("SELECT COUNT(*) FROM {shop}.customers WHERE email = '{}'", day.extra_email)),
        "0",
        "{name}: the second customer is gone"
    );
    assert_eq!(
        e.q(&format!("SELECT COUNT(*) FROM {shop}.customers WHERE name = '{}' AND note = '{MARK}' AND email IS NULL", day.added)),
        "1",
        "{name}: the new customer is there, its other columns their defaults"
    );
    app.until("the written rows", |app| {
        let g = app.grid();
        g.len() == 2 && g[0][1]["text"] == day.fixed.as_str() && g[1][1]["text"] == day.added.as_str()
    });
    app.click("grid-cell-1-1");
    app.shot(root, &format!("{p}-6b-applied"));

    // 7. Ctrl+T on the tree's `shop`; the database chip on MySQL and MariaDB only.
    // Other open schemas closed first: `ui.click` scrolls a row 300 rows away in too few frames.
    let shop_row = format!("tree-schema-{shop}");
    for row in app.tags_with("tree-schema-") {
        let id = row["id"].as_str().unwrap_or_default().to_string();
        if row["value"]["open"] == true && id != shop_row {
            app.click(&id);
        }
    }
    app.click(&shop_row);
    app.ctrl("T");
    app.until("the query tab", |app| app.document()["query"].is_object());
    let chip = app.tag("query-database");
    match &e.chip {
        Some(c) => {
            assert_eq!(chip.map(|t| t["value"].clone()), Some(json!("shop")), "{name}: the chip says the tree's selection");
            app.click("query-database");
            let listed: Vec<String> = app.tags_with("query-database-").iter().filter_map(|t| t["value"].as_str().map(str::to_string)).collect();
            assert_eq!(listed, c.lists, "{name}: the chip's list");
            app.shot(root, &format!("{p}-7a-databases"));
            app.click(&format!("query-database-{}", c.choose));
            app.until("the chip's choice", |app| app.tag("query-database").is_some_and(|t| t["value"] == c.choose));
            assert_eq!(app.document()["query"]["database"], c.choose, "{name}");
        }
        None => assert!(chip.is_none(), "{name}: no chip without USE"),
    }
    let (first, cell) = e.first;
    app.click("query-editor");
    app.text(&format!("{first};\nSELECT 2 AS two;\nSELECT 3 AS three"));
    app.ctrl("Enter");
    let q = query_done(app);
    assert!(q["error"].is_null(), "{name}: {q}");
    assert_eq!((q["results"].as_array().map(Vec::len), q["shown"].as_u64()), (Some(3), Some(2)), "{name}: the last result shows: {q}");
    assert!(q["results"].as_array().is_some_and(|r| r.iter().all(|o| o["ms"].is_u64())), "{name}: each statement says how long it took: {q}");
    assert_eq!(app.cell(0, 0)["text"], "3");
    // A tab per result, in view or listed behind the ⌄ list.
    for n in 1..=3 {
        assert!(app.tag(&format!("result-{n}")).is_some(), "{name}: result {n}'s tab");
    }
    show_result(app, root, 1, &format!("{p}-7b-results-list"));
    assert_eq!(app.cell(0, 0)["text"], cell, "{name}: the unqualified statement ran where the chip says");
    app.shot(root, &format!("{p}-7b-results"));
    // The run row and the result bar give way in order: none of their controls over another or
    // past the window.
    let bars = ["query-database", "query-run", "query-run-current", "query-facts", "query-export", "value-toggle"];
    app.assert_apart(&bars, &format!("{name}: the query tab's bars"));
    assert!(app.tag("query-facts").is_some() && app.tag("value-toggle").is_some(), "{name}: the facts and the toggle drawn");
    app.shot(root, &format!("{p}-7e-bars"));

    //    A statement that runs until cancelled; Cancel in the run row stops it.
    let marker = unique("m2day_run");
    let (slow, held) = held_statement(e, &marker);
    app.click("query-editor");
    app.ctrl("A");
    app.text(&slow);
    app.ctrl("Enter");
    if e.t.lists_running_statements() {
        app.until("the statement on the server", |_| e.q(&e.t.running_with_sql(&marker)) == "1");
    } else {
        app.until("the run", |app| !app.document()["query"]["running"].is_null());
    }
    // CANCEL shows once the run has gone 300 ms, and RUN is off while it shows.
    app.until("CANCEL in the run row", |app| app.tag("query-cancel").is_some());
    assert!(app.tag("query-cancel").is_some_and(|t| t.get("enabled") != Some(&json!(false))), "{name}: Cancel is enabled while it runs");
    assert!(app.tag("query-run").is_some_and(|t| t.get("enabled") == Some(&json!(false))), "{name}: Run is off while it runs");
    // The footer's job shows from 300 ms too, with its timer.
    app.until("the footer's timer", |app| app.tag("status-timer").is_some());
    assert!(app.value("status-job").as_str().is_some_and(|j| j.starts_with("Running ")), "{name}: the run is the footer's job");
    let timer = app.value("status-timer");
    let read = timer.as_str().unwrap_or_default();
    let timed = read.len() == 7
        && read.chars().enumerate().all(|(i, c)| [(2, ':'), (5, '.')].iter().find(|(at, _)| *at == i).map_or(c.is_ascii_digit(), |(_, s)| c == *s));
    assert!(timed, "{name}: the timer reads like 00:00.0: {timer}");
    app.shot(root, &format!("{p}-7c-running"));
    app.click("query-cancel");
    let q = query_done(app);
    drop(held);
    app.until("no Cancel once the run ended", |app| app.tag("query-cancel").is_none());
    assert_eq!(q["error"]["code"], "cancelled", "{name}: {q}");
    assert!(app.tag("query-error").is_none(), "{name}: dbee's own cancel shows no band");
    assert!(app.value("query-facts").as_str().is_some_and(|f| f.starts_with("Cancelled")), "{name}: the bar says the run was cancelled");
    app.shot(root, &format!("{p}-7d-cancelled"));

    //    The join, copied with its header: a click on a cell, Ctrl+A, Ctrl+Shift+C.
    let join = format!(
        "select o.id, c.name as customer, o.total, o.shipped_at\nfrom {shop}.orders o\njoin {shop}.customers c on c.id = o.customer_id\nwhere o.dealer_id = 42 and o.status = 'completed' and o.total > 100\norder by o.id"
    );
    app.click("query-editor");
    app.ctrl("A");
    app.text(&join);
    app.ctrl("Enter");
    let q = query_done(app);
    assert!(q["error"].is_null(), "{name}: {q}");
    let n = q["results"][0]["rows"].as_u64().unwrap();
    assert_eq!(n as usize, rows.len(), "{name}: the join finds the filtered orders: {q}");
    assert_eq!(app.value("query-rows"), format!("{n} rows"));
    app.click("grid-cell-0-0");
    app.ctrl("A");
    app.ctrl_shift("C");
    let (head, notice) = ("id\tcustomer\ttotal\tshipped_at\n", format!("Copied {n} rows"));
    // Read first: the notice shows for a few seconds.
    app.until("the copy's notice", |app| app.tag("notice").is_some_and(|t| t["value"] == notice.as_str()));
    let d = app.document();
    assert!(d["clipboard"].as_str().is_some_and(|c| c.starts_with(head)), "{name}: the header row first: {d}");
    let copied = d["clipboard"].as_str().unwrap().to_string();
    assert!(copied.lines().any(|l| l == format!("{}\t{}\t4242.42\t2026-10-26", day.order, day.fixed)), "{name}: the day's order: {copied}");
    assert_eq!((copied.lines().count() as u64, d["clipboardBytes"].as_u64()), (n + 1, Some(copied.len() as u64)), "{name}: {copied}");
    app.shot(root, &format!("{p}-7e-copied"));

    // 7f. The tree's menu on `orders`: New query opens on its schema (the chip names it on MySQL
    //     and MariaDB); Ctrl+W closes the empty tab.
    let tabs = app.document()["tabs"].as_array().map(Vec::len).unwrap_or_default();
    let row = format!("tree-table-{shop}.orders");
    // Step 7's click may have closed it (SQLite's `main` starts open).
    if app.tag(&row).is_none() {
        app.click(&shop_row);
        app.until("the schema open", |app| app.tag(&row).is_some());
    }
    app.right_click(&row);
    app.until("the tree's menu", |app| app.tag("tree-menu-new-query").is_some());
    app.click("tree-menu-new-query");
    app.until("the new query tab", |app| {
        let d = app.document();
        d["tabs"].as_array().map(Vec::len) == Some(tabs + 1) && d["query"].is_object() && d["query"]["text"] == ""
    });
    let chip = app.tag("query-database");
    match &e.chip {
        Some(_) => assert_eq!(chip.map(|t| t["value"].clone()), Some(json!(shop)), "{name}: the chip names the row's schema"),
        None => assert!(chip.is_none(), "{name}: no chip without USE"),
    }
    app.shot(root, &format!("{p}-7f-tree-new-query"));
    app.ctrl("W");
    app.until("the empty tab closed", |app| app.document()["tabs"].as_array().map(Vec::len) == Some(tabs));
}

#[test]
#[ignore = "opens a window on the desktop; run by hand: cargo test --workspace --test m2_day -- --ignored --nocapture"]
fn m2_day_end_to_end() {
    let db = dbee_testkit::require_testdb!();
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        dbee_testkit::skipped("m2_day: no display (DISPLAY or WAYLAND_DISPLAY) to open the window on");
        return;
    }
    let root = dbee_testkit::repo_root();
    let _ = std::fs::remove_dir_all(root.join(SHOTS));
    let (mysql, maria) = (db.server("mysql80").unwrap(), db.server("mariadbnew").unwrap());
    // Dropped in reverse: the app, the rows, the targets, the home.
    let home = temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    let sqlite = Engine { t: dbee_testkit::targets_of(&[Kind::Sqlite]).pop().unwrap(), shots: "1-sqlite", chip: None, first: IN_MANY };
    // A connection that names `shop` lists only it; one that names none lists every database.
    let my = Engine { t: dbee_testkit::target(mysql), shots: "2-mysql80", chip: Some(Chip { lists: &["shop"], choose: "shop" }), first: IN_SHOP };
    let all = &["many", "odd", "scratch", "shop"];
    let mb = Engine { t: dbee_testkit::target(maria), shots: "3-mariadbnew", chip: Some(Chip { lists: all, choose: "many" }), first: IN_MANY };
    let engines = [&sqlite, &my, &mb];
    for e in engines {
        e.q(&sweep_sql(e));
    }
    let before: Vec<String> = engines.iter().map(|e| digest(e)).collect();
    let (on_sqlite, on_mysql, on_maria) = (DayRows::insert(&sqlite), DayRows::insert(&my), DayRows::insert(&mb));

    // 1. `dbee <file>`: the SQLite copy opens unsaved; Save connection saves it.
    let file = sqlite.t.file().unwrap().to_path_buf();
    let file_name = file.file_name().unwrap().to_string_lossy().into_owned();
    let mut app = App::start_with(&root, &home.home, db, SHOTS, &[file.as_os_str()]);
    app.until("the file's workspace", |app| app.document()["screen"] == "workspace");
    let d = app.document();
    let c = &d["connection"];
    assert_eq!((c["kind"].clone(), c["name"].clone(), c["saved"].clone()), (json!("sqlite"), json!(file_name), json!(false)), "{d}");
    assert!(c["server"].as_str().is_some_and(|s| s.starts_with("SQLite 3.")), "{c}");
    assert_eq!(d["clipboard"], Json::Null);
    assert!(app.tag("status-save").is_some(), "Save connection is offered");
    app.shot(&root, "1-sqlite-1a-opened");
    app.click("status-save");
    app.until("the file saved", |app| app.document()["connection"]["saved"] == true);
    assert!(app.tag("status-save").is_none(), "saved: the button goes");
    let saved = app.execute("connection.list", json!({}))["saved"].clone();
    assert_eq!(saved.as_array().map(Vec::len), Some(1), "{saved}");
    app.shot(&root, "1-sqlite-1b-saved");
    // MariaDB for step 5, saved with its password and no database.
    let config = json!({
        "name": "Shop mariadb", "kind": "mariadb", "color": "green", "host": "127.0.0.1", "port": maria.port,
        "user": "root", "database": "", "sslMode": "prefer",
    });
    app.execute("connection.save", json!({ "config": config, "password": db.password }));
    the_day(&mut app, &root, &sqlite, &on_sqlite);

    // 3. Ctrl+Shift+Q, then New connection: the cards, PostgreSQL, Change engine, MySQL.
    app.ctrl_shift("Q");
    app.until("the question", |app| app.inspect()["dialog"] == "confirm");
    app.click("dialog-ok");
    app.until("the connection screen", |app| app.document()["screen"] == "connections");
    app.click("conn-new");
    for kind in ["postgres", "mysql", "mariadb", "sqlite"] {
        let card = app.value(&format!("conn-kind-{kind}"));
        assert_eq!(card["upcoming"], false, "{kind}: {card}");
    }
    //    The cards are K2: 104 tall, side by side.
    let cells = ["postgres", "mysql", "mariadb", "sqlite"].map(|kind| app.rect(&format!("conn-kind-{kind}")));
    for (n, c) in cells.iter().enumerate() {
        assert!((c[3] - 104.0).abs() <= 0.5 && (c[1] - cells[0][1]).abs() <= 0.5, "card {n}: {cells:?}");
        assert!(n == 0 || c[0] >= cells[n - 1][0] + cells[n - 1][2] - 1.0, "card {n} after the one before: {cells:?}");
    }
    app.shot(&root, "2-mysql80-0a-cards");
    app.click("conn-kind-postgres");
    assert_eq!((app.value("conn-kind"), app.value("conn-port")), (json!("postgres"), json!("5432")));
    //    The engine row under the title row.
    let (title, change) = (app.rect("conn-title"), app.rect("conn-change-engine"));
    assert!(change[1] > title[1] + title[3], "Change engine under the title: {title:?} {change:?}");
    app.click("conn-name");
    app.text("Shop mysql80");
    app.click("conn-change-engine");
    assert!(app.tag("conn-host").is_none() && app.tag("conn-kind-mysql").is_some(), "back on the cards");
    app.click("conn-kind-mysql");
    assert_eq!((app.value("conn-kind"), app.value("conn-port"), app.value("conn-name")), (json!("mysql"), json!("3306"), json!("Shop mysql80")));
    //    The form: mysql80 as the sshd sees it, the tunnel with the key found on its own.
    app.click("conn-color-blue");
    app.replace("conn-host", &mysql.name);
    app.replace("conn-port", &mysql.service_port.to_string());
    app.click("conn-user");
    app.text("root");
    app.click("conn-password");
    app.text(&db.password);
    app.click("conn-ssh-on");
    app.click("conn-ssh-host");
    app.text("127.0.0.1");
    app.replace("conn-ssh-port", &db.sshd.port.to_string());
    app.click("conn-ssh-user");
    app.text(&db.sshd.user);
    app.until("the key line", |app| app.value("conn-ssh-key").as_str().is_some_and(|t| t.starts_with("Uses ") && t.ends_with("id_ed25519")));
    app.shot(&root, "2-mysql80-0b-form");
    //    The title row, the engine row and the form's bar stand apart, inside the window.
    let rows = ["conn-title", "conn-fill-url", "conn-kind", "conn-change-engine", "conn-save", "conn-test", "conn-connect"];
    app.assert_apart(&rows, "2-mysql80-0b-form");
    //    List…: the server's databases through the tunnel, its own last; `shop` picked.
    app.click("conn-database-list");
    app.until("the list", |app| !app.tags_with("conn-database-item-").is_empty());
    let items: Vec<String> = app.tags_with("conn-database-item-").iter().filter_map(|t| t["value"].as_str().map(str::to_string)).collect();
    let system = |n: &str| ["mysql", "information_schema", "performance_schema", "sys"].contains(&n);
    let first_system = items.iter().position(|n| system(n)).unwrap_or(items.len());
    assert!(items[..first_system].iter().any(|n| n == "shop") && items[first_system..].iter().all(|n| system(n)), "{items:?}");
    app.shot(&root, "2-mysql80-0c-databases");
    let shop_item = app.tags_with("conn-database-item-").into_iter().find(|t| t["value"] == "shop").unwrap();
    app.click(shop_item["id"].as_str().unwrap());
    assert_eq!(app.value("conn-database"), "shop");
    assert!(app.tags_with("conn-database-item-").is_empty(), "picking closes the list");
    //    CONNECT saves it once it connects: no SAVE first.
    app.click("conn-connect");
    app.until("the MySQL workspace", |app| app.document()["screen"] == "workspace");
    let saved = app.execute("connection.list", json!({}))["saved"].clone();
    let named = saved.as_array().unwrap().iter().filter(|c| c["name"] == "Shop mysql80").count();
    assert_eq!(named, 1, "CONNECT saved it once: {saved}");
    let d = app.document();
    let c = &d["connection"];
    assert_eq!(
        (c["name"].clone(), c["kind"].clone(), c["color"].clone(), c["ssh"].clone(), c["saved"].clone(), c["database"].clone()),
        (json!("Shop mysql80"), json!("mysql"), json!("blue"), json!(true), json!(true), json!("shop")),
        "{d}"
    );
    assert!(c["server"].as_str().is_some_and(|s| s.starts_with("MySQL 8.0.")), "mysql80 behind the tunnel: {c}");
    assert!(app.tag("tree-schema-shop").is_some(), "the databases are listed");
    app.shot(&root, "2-mysql80-1-workspace");
    the_day(&mut app, &root, &my, &on_mysql);

    // 5. Ctrl+P `maria`: the saved MariaDB connection, directly.
    let c = switch(&mut app, &root, "3-mariadbnew-1-switch", "maria", "Shop mariadb", "MariaDB", "Shop mysql80");
    assert_eq!(
        (c["kind"].clone(), c["ssh"].clone(), c["color"].clone(), c["database"].clone()),
        (json!("mariadb"), json!(false), json!("green"), json!("")),
        "{c}"
    );
    assert!(c["server"].as_str().is_some_and(|s| s.starts_with(&format!("MariaDB {}", maria.version))), "{c}");
    app.shot(&root, "3-mariadbnew-1b-workspace");
    the_day(&mut app, &root, &mb, &on_maria);

    //    Ctrl+P back to the SQLite file, by its name.
    let c = switch(&mut app, &root, "4-sqlite-1-switch", "seed", &file_name, "SQLite", "Shop mariadb");
    assert_eq!((c["kind"].clone(), c["saved"].clone()), (json!("sqlite"), json!(true)), "{c}");
    assert!(app.tag("tree-schema-main").is_some_and(|t| t.get("visible") != Some(&json!(false))), "the tree starts at its top");
    app.shot(&root, "4-sqlite-1b-workspace");

    // Once the day's rows are gone, every engine's customers and orders are as before.
    drop(app);
    drop((on_sqlite, on_mysql, on_maria));
    let after: Vec<String> = engines.iter().map(|e| digest(e)).collect();
    assert_eq!(after, before, "the customers and orders are as they were");
}
