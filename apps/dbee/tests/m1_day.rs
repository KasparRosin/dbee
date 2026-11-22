//! The M1 day end to end in the real window: `dbee --control <port>` on a throwaway home with the
//! environment of `cargo xtask run-isolated`, driven only through the control channel
//! (`docs/control-protocol.md`), with `document.inspect` checked after every step and a screenshot
//! of each saved as `target/m1-day/<n>-<step>.png`, to look at.
//!
//! It opens a window on the desktop, so it runs by hand only, with the test servers up and the
//! window shown (a hidden window runs no frames on Wayland, `docs/testing.md` §4):
//!
//! ```sh
//! cargo xtask testdb up
//! cargo test --workspace --test m1_day -- --ignored --nocapture
//! ```
//!
//! It prints `SKIPPED` without the test servers (with `DBEE_REQUIRE_ORACLES=1` it fails instead)
//! or without a display (`DISPLAY` or `WAYLAND_DISPLAY`).
//!
//! Safety: the app gets an isolated run's environment: a throwaway home whose `.ssh` holds the
//! test key as `id_ed25519` and the test sshd's host key in `known_hosts`, in-memory secrets and
//! clipboard (no copy reaches the system clipboard), no SSH agent, and the test servers as its
//! only targets. The seed is read-only: the day edits a customer and an order of its own, which
//! the test inserts with values the day's filters find and deletes when it ends, also when it
//! fails; the seed's customers and orders are checked to be unchanged, and so are their counts
//! once the day's rows are gone. A run that is killed (Ctrl+C, a tool's timeout, SIGKILL) cannot
//! delete its rows, so every run first removes the rows of earlier runs by their marker ([`MARK`],
//! the customers' `note`): their orders, then the customers.
//!
//! Step 6 is two Applies by design: the seed keeps `name` on `shop.customers` and `shipped_at` on
//! `shop.orders`, and Apply writes one tab's staged changes. So the name is fixed and written in
//! the customers tab, and the date cleared and written in the orders tab, each all or nothing.
//! Two changes in one Apply are covered on one table by `day_scenario_two_changes_one_apply`
//! (engine) and `ctrl_s_applies_all_or_nothing` (`crates/ui-egui/tests/editing.rs`).

mod common;

use common::App;
use dbee_testkit::{Server, TestDb, psql, temp_home, unique};
use dbee_ui_egui::value_panel::{LINE_H, PAD};
use serde_json::{Value as Json, json};

/// The `note` of the customers the day inserts: the marker by which a later run finds and
/// removes the rows a killed run left.
const MARK: &str = "the M1 day test row";

/// Where the screenshots go, from the repository root (the app's working directory).
const SHOTS: &str = "target/m1-day";

/// The join pasted into the query tab in step 7, as from an AI chat.
const JOIN: &str = "select o.id, c.name as customer, o.total, o.shipped_at
from shop.orders o
join shop.customers c on c.id = o.customer_id
where o.dealer_id = 42 and o.status = 'completed' and o.total > 100
order by o.id";

fn pg14(db: &TestDb) -> &Server {
    db.server("pg14").unwrap()
}

/// The day's own customer and order, deleted when the test ends, also when it fails.
struct DayRows {
    server: Server,
    customer: String,
    order: String,
    /// The customer's name as inserted, with a typo to fix: `Custmer m1day_<hex>`.
    typo: String,
    /// The name fixed: `Customer m1day_<hex>`.
    fixed: String,
}

/// Removes the rows earlier runs left (a killed run cannot delete its own): the orders of the
/// customers marked [`MARK`], then those customers.
fn sweep(server: &Server) {
    let deleted = |sql: String| {
        let out = psql(server, &format!("WITH d AS ({sql} RETURNING 1) SELECT count(*) FROM d")).unwrap();
        out.trim().parse::<u64>().unwrap_or_else(|_| panic!("sweep: {out}"))
    };
    let orders = deleted(format!("DELETE FROM shop.orders WHERE customer_id IN (SELECT id FROM shop.customers WHERE note = '{MARK}')"));
    let customers = deleted(format!("DELETE FROM shop.customers WHERE note = '{MARK}'"));
    if customers > 0 || orders > 0 {
        println!("m1_day: removed {customers} customer(s) and {orders} order(s) an earlier run left");
    }
}

/// `<customers> <orders>`: the row counts of the tables the day writes to.
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

fn counts(server: &Server) -> String {
    psql(server, "SELECT (SELECT count(*) FROM shop.customers) || ' ' || (SELECT count(*) FROM shop.orders)").unwrap().trim().to_string()
}

impl DayRows {
    /// A customer, and an order of theirs that passes the day's three filters (dealer 42,
    /// completed, a total above 100), with a `billing` jsonb and a `shipped_at` date to clear.
    fn insert(db: &TestDb) -> DayRows {
        let tag = unique("m1day");
        let typo = format!("Custmer {tag}");
        let sql = format!(
            "WITH c AS (
                INSERT INTO shop.customers (name, email, note) VALUES ('{typo}', '{tag}@example.com', '{MARK}') RETURNING id
             ), o AS (
                INSERT INTO shop.orders (customer_id, dealer_id, status, total, billing, meta, shipped_at, raw_json_text)
                SELECT c.id, 42, 'completed', 4242.42,
                       jsonb_build_object(
                           'name', '{typo}',
                           'address', jsonb_build_object('street', '1 Day Street', 'city', 'Testville', 'zip', '04242'),
                           'items', jsonb_build_array(jsonb_build_object('sku', 'SKU-DAY', 'qty', 2)),
                           'paid', true),
                       to_json('{{\"a\":1}}'::text), date '2026-10-26', '{{\"k\":[1,2]}}'
                FROM c RETURNING id, customer_id
             )
             SELECT id || ' ' || customer_id FROM o"
        );
        let server = pg14(db).clone();
        let out = psql(&server, &sql).unwrap();
        let (order, customer) = out.trim().split_once(' ').unwrap_or_else(|| panic!("insert: {out}"));
        DayRows { server, customer: customer.into(), order: order.into(), fixed: format!("Customer {tag}"), typo }
    }

    /// One line of `psql` output, trimmed.
    fn q(&self, sql: &str) -> String {
        psql(&self.server, sql).unwrap().trim().to_string()
    }

    /// A digest of every seed customer and order (all but the day's own rows), to show the day
    /// changed none of them.
    fn seed_digest(&self) -> String {
        self.q(&format!(
            "SELECT (SELECT md5(string_agg(o::text, '|' ORDER BY o.id)) FROM shop.orders o WHERE o.id <> {})
                 || ' ' || (SELECT md5(string_agg(c::text, '|' ORDER BY c.id)) FROM shop.customers c WHERE c.id <> {})",
            self.order, self.customer
        ))
    }
}

impl Drop for DayRows {
    fn drop(&mut self) {
        let sql = format!("DELETE FROM shop.orders WHERE id = {}; DELETE FROM shop.customers WHERE id = {};", self.order, self.customer);
        if let Err(e) = psql(&self.server, &sql) {
            eprintln!("m1_day: could not delete its rows (order {}, customer {}): {e}", self.order, self.customer);
        }
    }
}

#[test]
#[ignore = "opens a window on the desktop; run by hand: cargo test --workspace --test m1_day -- --ignored --nocapture"]
fn m1_day_end_to_end() {
    let db = dbee_testkit::require_testdb!();
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        dbee_testkit::skipped("m1_day: no display (DISPLAY or WAYLAND_DISPLAY) to open the window on");
        return;
    }
    let root = dbee_testkit::repo_root();
    let _ = std::fs::remove_dir_all(root.join(SHOTS));
    // Dropped in reverse order: the app goes first, then the rows, then the home.
    let home = temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    sweep(pg14(db));
    let before = counts(pg14(db));
    let day = DayRows::insert(db);
    let seed = day.seed_digest();
    // A server that never answers: the day's own listener, which accepts no connection.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_port = silent.local_addr().unwrap().port();
    let mut app = App::start_allowing(&root, &home.home, db, SHOTS, &[silent_port]);
    let d = app.document();
    assert_eq!((d["screen"].clone(), d["clipboard"].clone()), (json!("connections"), Json::Null), "{d}");

    // The saved connection "Shop live", red: through the test sshd with the key found
    // automatically (no path), to pg14 behind it, its password kept in the session's memory.
    let config = json!({
        "name": "Shop live",
        "color": "red",
        "host": pg14(db).name,
        "port": pg14(db).service_port,
        "user": "postgres",
        "database": "dbee",
        "sslMode": "prefer",
        "ssh": { "host": "127.0.0.1", "port": db.sshd.port, "user": db.sshd.user, "auth": { "method": "key", "path": null } },
    });
    let id = app.execute("connection.save", json!({ "config": config, "password": db.password }))["id"].as_str().unwrap().to_string();
    let card = format!("conn-saved-{id}");
    app.until("the saved card", |app| app.tag(&card).is_some());
    // The workspace's frame: the menu bar, and the version along the bottom; one list, with the
    // saved connection listed once.
    assert!(app.tag("menu-View").is_some(), "the menu bar on the connection screen");
    assert!(app.value("conn-version").as_str().is_some_and(|v| v.starts_with("dbee ")), "the version bar");
    assert!(app.tags_with("conn-recent-").is_empty(), "the saved connection isn't listed twice");
    app.shot(&root, "0-connections");

    // 0a. A connection to a server that never answers: a double-click connects; from 300 ms the
    //     form's bar shows CONNECTING, its timer and CANCEL, the form dimmed. CANCEL ends it and
    //     shows nothing. The connection is deleted again, so the list is as it was.
    let silent_config = json!({ "name": "Silent", "host": "127.0.0.1", "port": silent_port, "user": "postgres", "database": "dbee" });
    let silent_id = app.execute("connection.save", json!({ "config": silent_config }))["id"].as_str().unwrap().to_string();
    let silent_row = format!("conn-saved-{silent_id}");
    app.until("the Silent row", |app| app.tag(&silent_row).is_some());
    app.double_click(&silent_row);
    let last_click = std::time::Instant::now();
    app.until("the connecting state", |app| app.tag("conn-running").is_some());
    assert_eq!(app.value("conn-running")["words"], "Connecting");
    assert!(app.value("conn-running")["timer"].as_str().is_some_and(|t| t.starts_with("00:")), "the timer");
    assert_eq!(app.tag("conn-connect").unwrap()["enabled"], false, "CONNECT off, CANCEL beside it");
    assert!(app.tag("conn-cancel").is_some());
    app.shot(&root, "0a-connecting");
    // Pressed from the keyboard, as the card below: no pointer click until step 1's double-click.
    app.press("conn-cancel");
    app.until("the cancelled connect's end", |app| app.tag("conn-running").is_none());
    let d = app.document();
    assert_eq!((d["screen"].clone(), d["jobs"].clone()), (json!("connections"), json!([])), "{d}");
    assert!(app.tag("conn-error").is_none() && app.tag("conn-cancel").is_none(), "a cancel shows nothing");
    app.execute("connection.delete", json!({ "id": silent_id }));
    app.until("the Silent row gone", |app| app.tag(&silent_row).is_none());
    drop(silent);

    // 0b. A press on the card shows its form, in two columns: its engine fixed, the tunnel on, the
    //     key it will use under KEY FILE, SHOW ending with its column as PORT does. It is pressed
    //     from the keyboard (`ui.focus`, Space): egui counts clicks less than 0.6 s apart as one
    //     double or triple click, so a click here would make step 1's double-click a triple one.
    app.press(&card);
    app.until("the form", |app| app.tag("conn-host").is_some());
    assert_eq!(app.value("conn-kind"), "postgres");
    assert!(app.tag("conn-change-engine").is_none(), "a saved connection's engine is fixed");
    assert_eq!(app.value("conn-ssh-on"), true);
    app.until("the key line", |app| app.value("conn-ssh-key").as_str().is_some_and(|t| t.starts_with("Uses ")));
    let (show, port) = (app.rect("conn-password-show"), app.rect("conn-port"));
    assert!(((show[0] + show[2]) - (port[0] + port[2])).abs() <= 0.5, "SHOW {show:?} ends where PORT {port:?} does");
    app.shot(&root, "0b-edit-form");
    // 0a's connecting state alone takes 300 ms, and its cancel, the form and the picture more:
    // about 1.4 s under Xvfb. Under 0.6 s egui would read the double-click below as a triple one.
    let since = last_click.elapsed();
    assert!(since > std::time::Duration::from_millis(600), "the last pointer click only {since:?} before step 1's double-click");

    // 1. A double-click on the card reaches the workspace: no key to pick, no host key to trust,
    //    no password to type.
    app.double_click(&card);
    app.until("the workspace", |app| app.document()["screen"] == "workspace");
    let d = app.document();
    let c = &d["connection"];
    assert_eq!(
        (c["name"].clone(), c["color"].clone(), c["ssh"].clone(), c["lost"].clone()),
        (json!("Shop live"), json!("red"), json!(true), json!(false)),
        "{d}"
    );
    assert!(c["server"].as_str().is_some_and(|v| v.starts_with("PostgreSQL 14.")) && c["kind"] == "postgres", "pg14 behind the tunnel: {c}");
    assert_eq!(app.inspect()["dialog"], Json::Null, "nothing was asked");
    assert!(app.value("status-connection").as_str().is_some_and(|s| s.contains("Shop live")));
    app.shot(&root, "1-workspace");

    // 2. Ctrl+P, `ord`, Enter: the orders tab.
    app.ctrl("P");
    app.until("the quick search", |app| app.tag("qs-input").is_some());
    // Nothing typed: the field alone.
    assert!(app.tag("qs-item-0").is_none() && app.tag("qs-group-0").is_none(), "nothing listed before typing");
    app.shot(&root, "2a0-quick-search-empty");
    app.text("zzz");
    app.until("No match", |app| app.tag("qs-none").is_some_and(|t| t["value"] == "No match"));
    app.ctrl("A");
    app.text("ord");
    app.until("the quick search's hits", |app| app.tag("qs-item-0").is_some_and(|t| t["value"] == "shop.orders"));
    // Grouped under the open connection's heading; the field 40 tall, rows 32, 616 wide.
    assert_eq!(app.value("qs-group-0"), "Shop live");
    let (heading, first, field) = (app.rect("qs-group-0"), app.rect("qs-item-0"), app.rect("qs-input"));
    assert!(heading[1] + heading[3] <= first[1] + 0.5, "the heading above the first row: {heading:?} {first:?}");
    assert!((field[2] - 616.0).abs() <= 0.5 && (field[3] - 40.0).abs() <= 0.5, "the field: {field:?}");
    assert!((first[3] - 32.0).abs() <= 0.5, "a row: {first:?}");
    app.shot(&root, "2a-quick-search");
    app.key("Enter");
    let t = app.settled("the orders page");
    assert_eq!((t["schema"].clone(), t["name"].clone()), (json!("shop"), json!("orders")), "{t}");
    assert!(app.tag("qs-input").is_none(), "Enter closed the quick search");
    let orders_tab = app.document()["active"].as_u64().unwrap();
    assert_eq!(app.value(&format!("tab-{orders_tab}")), "orders");
    assert_eq!(t["rows"], 100, "the first page");
    app.shot(&root, "2b-orders");
    // The table bar's controls stand apart, inside the window.
    app.assert_apart(&["table-view-data", "table-refresh", "row-add", "page-count", "table-import", "table-export"], "2b");
    // The table bar between the filter bar and the rows: DATA chosen, STRUCTURE on, EXPORT off
    // (when it has room), the planner's estimate; COUNT counts the rows exactly.
    assert_eq!(app.value("table-view-data"), true);
    assert!(app.tag("table-view-structure").is_some_and(|e| e["enabled"] != false), "STRUCTURE on");
    assert!(app.tag("table-export").is_none_or(|e| e["enabled"] == false), "EXPORT off");
    let (filter, refresh, header) = (app.rect("filter-0-column"), app.rect("table-refresh"), app.rect("grid-header-0"));
    assert!(filter[1] < refresh[1] && refresh[1] < header[1], "the table bar under the filters, over the rows: {filter:?} {refresh:?} {header:?}");
    assert!(app.value("page-count").as_str().is_some_and(|v| v.starts_with('~')), "the estimate");
    app.click("page-count");
    app.until("the count", |app| app.table()["total"]["exact"].is_u64());
    let n = app.table()["total"]["exact"].as_u64().unwrap();
    assert_eq!(n.to_string(), psql(pg14(db), "SELECT count(*) FROM shop.orders").unwrap().trim(), "psql's count");
    assert_eq!(app.value("table-rows"), format!("1–100 of {} rows", thousands(n)));
    app.shot(&root, "2c-counted");
    // 2d. STRUCTURE: the page reads the orders' structure (COLUMNS as the server has them, the
    //     facts with the count from 2c), and DATA comes back with its rows and filters.
    let switch = app.rect("table-view-data");
    app.click("table-view-structure");
    app.until("the structure", |app| app.tag("structure-columns-0-name").is_some());
    assert_eq!(app.rect("table-view-data"), switch, "the view switch stands where it stood in DATA, in the bottom bar");
    assert_eq!(app.value("structure-columns-0-name"), "id");
    assert_eq!(app.value("structure-columns-0-keys"), "primary key");
    assert_eq!(app.value("structure-columns-1-keys"), "shop.customers.id");
    assert_eq!(app.value("structure-facts"), format!("9 columns  {} rows", thousands(n)));
    assert!(app.tag("filter-0-column").is_none(), "no filter bar in STRUCTURE");
    app.shot(&root, "2d-structure");
    // The bar's parts and COLUMNS' first row (those drawn) stand apart, inside the window.
    let row: Vec<String> = ["position", "name", "type", "nullable", "default", "keys"].iter().map(|k| format!("structure-columns-0-{k}")).collect();
    let mut apart = vec!["table-view-data", "table-view-structure", "table-refresh", "structure-facts"];
    apart.extend(row.iter().map(String::as_str));
    app.assert_apart(&apart, "2d");
    // A double-click on a column's edge fits the column to its content: Name, 150 as drawn.
    let drawn = app.value("structure-columns-edge-name").as_f64().unwrap();
    app.double_click("structure-columns-edge-name");
    app.until("the fitted column", |app| app.value("structure-columns-edge-name").as_f64().is_some_and(|w| w < drawn - 1.0));
    app.shot(&root, "2d-fitted");
    // The other sections, REFERENCED BY below the page's bottom (listed, out of view); a foreign
    // key's link opens its table, and closing that comes back to orders' STRUCTURE.
    assert_eq!(app.value("structure-indexes-0-keys"), "primary key");
    assert_eq!(app.value("structure-referenced-by-0-from"), "shop.order_items.order_id");
    assert!(app.tag("structure-triggers-0-name").is_none(), "orders has no triggers");
    let orders_tab = app.document()["active"].clone();
    app.click("structure-columns-1-keys");
    app.until("customers", |app| app.table()["name"] == "customers");
    assert_eq!(app.table()["view"], "data", "a link opens DATA");
    app.until("the customers' rows", |app| app.tag("grid-header-0").is_some());
    app.shot(&root, "2d-link");
    app.ctrl("W");
    app.until("back on orders", |app| app.document()["active"] == orders_tab);
    assert_eq!(app.table()["view"], "structure", "orders still in STRUCTURE");
    app.until("the structure", |app| app.tag("structure-columns-0-name").is_some());
    app.click("table-view-data");
    app.until("the rows", |app| app.tag("grid-header-0").is_some());
    assert_eq!((app.table()["view"].clone(), app.table()["rows"].clone()), (json!("data"), json!(100)));
    assert!(app.tag("filter-0-column").is_some(), "the filter bar back");
    // 2e. A first load held by a lock shows the shipments' header over rows of bars, LOADING and
    //     CANCEL in the table bar and no job in the footer; then a reload held the same way shows
    //     the old rows at half until CANCEL ends it. Back to orders.
    let lock = "LOCK TABLE shop.shipments IN ACCESS EXCLUSIVE MODE";
    let held = dbee_testkit::hold(pg14(db), lock).unwrap();
    app.ok("engine.execute", json!({ "command": "table.open", "params": { "schema": "shop", "table": "shipments" }, "wait": false }));
    app.until("the skeleton", |app| app.tag("grid-skeleton").is_some());
    assert!(app.tag("status-job").is_none(), "a page load is no footer job");
    assert!(app.tag("table-loading").is_some() && app.tag("table-cancel").is_some(), "LOADING and CANCEL");
    let shipments_tab = app.document()["active"].as_u64().unwrap();
    app.shot(&root, "2e-skeleton");
    drop(held);
    let t = app.settled("the shipments page");
    assert_eq!(t["name"], "shipments", "{t}");
    assert!(app.tag("grid-skeleton").is_none(), "the rows in its place");
    let held = dbee_testkit::hold(pg14(db), lock).unwrap();
    app.click("table-refresh");
    app.until("the old rows dimmed", |app| app.tag("grid-dimmed").is_some());
    app.shot(&root, "2e-reload");
    app.click("table-cancel");
    app.until("the cancelled reload", |app| app.table()["loading"].is_null() && app.tag("grid-dimmed").is_none());
    drop(held);
    app.execute("tab.close", json!({ "tab": shipments_tab, "force": true }));
    app.execute("tab.activate", json!({ "tab": orders_tab }));
    assert_eq!(app.table()["name"], "orders");
    // 2f. A tree refresh waiting its turn behind a held first load reads LOADING over the tree's
    //     rows at half; CANCEL on the held load lets it run, and the count's words come back.
    let held = dbee_testkit::hold(pg14(db), lock).unwrap();
    app.ok("engine.execute", json!({ "command": "table.open", "params": { "schema": "shop", "table": "shipments" }, "wait": false }));
    app.until("the held load", |app| !app.table()["loading"].is_null());
    let shipments_tab = app.document()["active"].as_u64().unwrap();
    app.click("tree-refresh");
    app.until("the tree's LOADING", |app| app.tag("tree-count").is_some_and(|t| t["value"] == "Loading"));
    assert_eq!(app.tag("tree-dimmed").map(|t| t["value"].clone()), Some(json!(true)), "the rows at half");
    app.shot(&root, "2f-tree-loading");
    app.click("table-cancel");
    app.until("the refresh's end", |app| app.tag("tree-count").is_some_and(|t| t["value"].as_str().is_some_and(|w| w.ends_with("tables"))));
    assert!(app.tag("tree-dimmed").is_none(), "the rows at full");
    drop(held);
    app.execute("tab.close", json!({ "tab": shipments_tab, "force": true }));
    app.execute("tab.activate", json!({ "tab": orders_tab }));
    assert_eq!(app.table()["name"], "orders");

    // 3. The three filters through the bar (dealer_id = 42, + status = completed, + total > 100),
    //    then Enter in the last value field.
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
        "{t}"
    );
    assert_eq!(app.value(&format!("tab-{orders_tab}")), "orders [3]");
    let header = |app: &mut App, col: usize| app.value(&format!("grid-header-{col}"))["name"].as_str().unwrap_or_default().to_string();
    let names: Vec<String> = (0..9).map(|c| header(&mut app, c)).collect();
    assert_eq!(names, ["id", "customer_id", "dealer_id", "status", "total", "billing", "meta", "shipped_at", "raw_json_text"]);
    let rows = app.grid();
    assert_eq!(rows.len() as u64, t["rows"].as_u64().unwrap(), "every loaded row is listed");
    assert!(rows.len() > 1, "the seed's rows and the day's own: {}", rows.len());
    // One whole page with nothing after it: it tells the count, without a count.
    assert_eq!(t["total"], json!({ "exact": rows.len() }), "{t}");
    assert_eq!(app.value("page-number"), "Page 1 of 1", "the paging bar knows the last page");
    for (r, row) in rows.iter().enumerate() {
        let total: f64 = row[4]["text"].as_str().unwrap().parse().unwrap();
        assert!(row[2]["text"] == "42" && row[3]["text"] == "completed" && total > 100.0, "row {r} passes the filters: {row:?}");
    }
    let r = rows.iter().position(|row| row[0]["text"] == day.order.as_str()).unwrap_or_else(|| panic!("the day's order {} is listed", day.order));
    assert_eq!(rows[r][1]["text"], day.customer.as_str());
    app.shot(&root, "3-filters");

    // 3b. The raw toggle: the field reads as SQL, a grey WHERE before the condition; back, the
    //     three filters are as they were and nothing reloaded.
    let filtered = app.table();
    app.click("filter-raw-toggle");
    app.until("the raw field", |app| app.tag("filter-raw-where").is_some());
    assert_eq!(app.value("filter-raw-toggle"), true);
    assert_eq!(app.value("filter-raw-where"), "WHERE");
    app.shot(&root, "3b-raw");
    app.click("filter-raw-toggle");
    app.until("the filter rows", |app| app.tag("filter-0-column").is_some());
    assert_eq!(app.value("filter-raw-toggle"), false);
    assert!(app.tag("filter-raw-where").is_none(), "the rows again");
    let back = app.table();
    assert_eq!(back["filters"], filtered["filters"], "the three filters are still there");
    assert_eq!(back["applied"], filtered["applied"], "nothing was applied");
    assert!(back["loading"].is_null(), "nothing reloaded: {back}");

    // 4. A click on the order's billing cell: the value panel shows the jsonb formatted. A line
    //    of it selected (a click, then a shift-click) and Ctrl+C copy that line.
    app.click(&format!("grid-cell-{r}-5"));
    let d = app.document();
    assert_eq!(d["selection"], json!({ "anchor": [r, 5], "focus": [r, 5], "whole": false }));
    let v = &d["value"];
    assert_eq!(
        (v["column"].clone(), v["typeLabel"].clone(), v["pretty"].clone(), v["isNull"].clone()),
        (json!("billing"), json!("jsonb"), json!(true), json!(false)),
        "{v}"
    );
    assert_eq!((v["kind"].clone(), v["kindLabel"].clone()), (json!("json"), json!("jsonb")), "{v}");
    let panel = app.value("value-panel");
    assert_eq!(
        (panel["column"].clone(), panel["type"].clone(), panel["label"].clone()),
        (json!("billing"), json!("jsonb"), json!("jsonb")),
        "{panel}"
    );
    // The toggle at the table bar's right end, the panel under the table bar.
    let (toggle, head, filter) = (app.rect("value-toggle"), app.rect("grid-header-0"), app.rect("filter-0-column"));
    let toggle_y = toggle[1] + toggle[3] / 2.0;
    assert!(toggle_y < head[1] && toggle_y > filter[1] + filter[3], "the toggle in the table bar: {toggle:?}, {filter:?}, {head:?}");
    app.panel_between_the_bars("step 4");
    // WRAP off and on again: the text comes back as it was.
    let value_rows = app.value("value-text")["rows"].clone();
    assert_eq!(app.value("value-panel")["wrap"], true, "WRAP starts on");
    app.click("value-wrap");
    assert_eq!((app.value("value-panel")["wrap"].clone(), app.value("value-wrap")), (json!(false), json!(false)), "WRAP off");
    app.click("value-wrap");
    assert_eq!(app.value("value-panel")["wrap"], true, "WRAP on again");
    assert_eq!(app.value("value-text")["rows"], value_rows, "the rows as they were");
    let shown = app.value("value-text");
    let text = shown["text"].as_str().unwrap().to_string();
    assert!(text.starts_with("{\n") && text.contains("\"city\": \"Testville\""), "formatted: {text}");
    let lines: Vec<&str> = text.split('\n').collect();
    let line = lines[1];
    assert!(line.contains(&day.typo), "the second line holds the name: {text}");
    let start = lines[0].len() + 1;
    app.select_value_line(1, start, line.len(), f64::from(PAD), f64::from(LINE_H));
    app.ctrl("C");
    let d = app.document();
    assert_eq!((d["clipboard"].as_str(), d["clipboardBytes"].as_u64()), (Some(line), Some(line.len() as u64)), "the selected line was copied");
    app.shot(&root, "4-billing");

    // 4b. COPY puts the whole jsonb on the clipboard as the database stores it, not as the panel
    //     shows it; the facts say its lines and size. A text column holding JSON says the panel
    //     formatted it.
    let facts = app.value("value-panel")["facts"].as_str().unwrap_or_default().to_string();
    let words: Vec<&str> = facts.split(' ').collect();
    let number = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
    assert!(matches!(words.as_slice(), [n, "lines", b, "B"] if number(n) && number(b)), "the facts: {facts:?}");
    app.click("value-copy");
    let stored = day.q(&format!("SELECT billing::text FROM shop.orders WHERE id = {}", day.order));
    assert!(!stored.contains('\n'), "the database keeps it on one line: {stored}");
    app.until("the copy", |app| app.document()["clipboard"].as_str() == Some(stored.as_str()));
    let raw = names.iter().position(|n| n == "raw_json_text").unwrap_or_else(|| panic!("raw_json_text in {names:?}"));
    app.click(&format!("grid-cell-{r}-{raw}"));
    let facts = app.value("value-panel")["facts"].as_str().unwrap_or_default().to_string();
    assert!(facts.starts_with("Formatted as JSON "), "the facts: {facts:?}");
    app.shot(&root, "4b-formatted");

    // 5. The order's row selected, its customer_id link: a new customers tab filtered to the id.
    app.click(&format!("grid-cell-{r}-1"));
    assert_eq!(app.document()["selection"], json!({ "anchor": [r, 1], "focus": [r, 1], "whole": false }));
    app.click(&format!("fk-link-{r}-1"));
    app.until("the customers tab", |app| app.table()["name"] == "customers");
    let t = app.settled("the customer's page");
    assert_eq!(t["filters"], json!([{ "column": "id", "op": "eq", "value": day.customer }]), "{t}");
    assert_eq!(t["applied"]["filters"], t["filters"], "{t}");
    assert_eq!(t["rows"], 1, "{t}");
    let d = app.document();
    let customers_tab = d["active"].as_u64().unwrap();
    assert_ne!(customers_tab, orders_tab, "a new tab");
    assert_eq!(d["tabs"].as_array().map(Vec::len), Some(2), "{d}");
    assert_eq!(app.value(&format!("tab-{customers_tab}")), "customers [1]");
    assert_eq!(app.cell(0, 1)["text"], day.typo.as_str());
    app.shot(&root, "5-customer");

    // 6. The typo in the name fixed (in the customers tab) and the wrong shipped_at cleared (in
    //    the orders tab), each written with Ctrl+S; psql shows both. Two tables, so two Applies
    //    (see the module docs).
    app.double_click("grid-cell-0-1");
    let editor = app.value("cell-editor");
    assert_eq!((editor["text"].as_str(), editor["multiline"].as_bool()), (Some(day.typo.as_str()), Some(true)), "{editor}");
    // Several lines: the value panel is the editor, with CANCEL and SAVE in its bar.
    assert!(app.inside("cell-editor", "value-panel"), "the name is edited in the value panel");
    assert!(app.tag("value-save").is_some(), "SAVE in the panel's bar");
    app.ctrl("A");
    app.text(&day.fixed);
    app.shot(&root, "6a0-name-editing");
    app.click("value-save");
    assert!(app.tag("cell-editor").is_none(), "SAVE closed the editor");
    let cell = app.cell(0, 1);
    assert_eq!((cell["text"].as_str(), cell["state"].as_str()), (Some(day.fixed.as_str()), Some("staged")), "{cell}");
    assert_eq!(app.table()["edits"]["count"], 1);
    app.shot(&root, "6a-name-staged");
    app.ctrl("S");
    app.until("the name's Apply", |app| app.table()["edits"]["count"] == 0);
    let t = app.settled("the name's Apply");
    assert!(t["failed"].is_null() && t["error"].is_null(), "{t}");
    assert_eq!(day.q(&format!("SELECT name FROM shop.customers WHERE id = {}", day.customer)), day.fixed, "psql shows the new name");
    assert_eq!(app.cell(0, 1)["text"], day.fixed.as_str());
    app.click(&format!("tab-{orders_tab}"));
    assert_eq!(app.document()["active"], orders_tab);
    assert_eq!(app.cell(r, 0)["text"], day.order.as_str(), "the orders tab kept its page");
    assert_eq!(app.cell(r, 7)["text"], "2026-10-26");
    app.double_click(&format!("grid-cell-{r}-7"));
    let editor = app.value("cell-editor");
    assert_eq!((editor["text"].as_str(), editor["multiline"].as_bool()), (Some("2026-10-26"), Some(false)), "{editor}");
    app.ctrl("A");
    app.key("Delete");
    app.key("Enter");
    let cell = app.cell(r, 7);
    assert_eq!(
        (cell["text"].clone(), cell["null"].clone(), cell["state"].clone()),
        (json!("NULL"), json!(true), json!("staged")),
        "a date clears to NULL: {cell}"
    );
    assert_eq!(app.table()["edits"]["count"], 1, "the date is the orders tab's one change");
    app.shot(&root, "6b-date-staged");
    app.ctrl("S");
    app.until("the date's Apply", |app| app.table()["edits"]["count"] == 0);
    let t = app.settled("the date's Apply");
    assert!(t["failed"].is_null() && t["error"].is_null(), "{t}");
    assert_eq!(
        day.q(&format!("SELECT (shipped_at IS NULL)::text || ' ' || total FROM shop.orders WHERE id = {}", day.order)),
        "true 4242.42",
        "psql shows shipped_at NULL, the rest of the row as it was"
    );
    assert_eq!(day.q(&format!("SELECT name FROM shop.customers WHERE id = {}", day.customer)), day.fixed, "and the name");
    // The cell written shows the saved mark for a moment, then nothing.
    app.until("the saved mark to fade", |app| app.cell(r, 7).get("state").is_none());
    let cell = app.cell(r, 7);
    assert_eq!((cell["null"].clone(), cell.get("state")), (json!(true), None), "written: {cell}");
    app.shot(&root, "6c-applied");

    // 6d. Back in the customers tab, NULL into the NOT NULL name: Ctrl+S fails, and the band under
    //     the table bar says why with the server's detail; RESET drops it, and the name stays.
    app.click(&format!("tab-{customers_tab}"));
    assert_eq!(app.document()["active"], customers_tab);
    app.right_click("grid-cell-0-1");
    app.click("grid-menu-null");
    assert_eq!(app.table()["edits"]["count"], 1, "NULL staged in the name");
    app.ctrl("S");
    app.until("the failed Apply", |app| app.table()["failed"].is_object());
    let failed = app.table()["failed"].clone();
    let band = app.value("table-error");
    assert!(band.as_str().is_some_and(|m| m.contains("not-null")), "the server's message: {band}");
    assert!(failed["detail"].as_str().is_some_and(|d| d.starts_with("Failing row contains")), "the server's detail: {failed}");
    app.shot(&root, "6d-apply-failed");
    app.click("edit-reset");
    app.until("the band to go", |app| app.tag("table-error").is_none());
    assert_eq!(day.q(&format!("SELECT name FROM shop.customers WHERE id = {}", day.customer)), day.fixed, "psql keeps the name");

    // 7. Ctrl+T, the join pasted, Ctrl+Enter; a click on a result cell gives the grid the
    //    keyboard (the editor keeps it after a run), Ctrl+A, Ctrl+Shift+C: the copy starts with
    //    the header row.
    app.ctrl("T");
    app.until("the query tab", |app| app.document()["query"].is_object());
    assert_eq!(app.value("query-editor")["focused"], true, "the new tab's editor has the keyboard");
    app.text(JOIN);
    assert_eq!(app.document()["query"]["text"], JOIN);
    app.ctrl("Enter");
    app.until("the join's result", |app| {
        let q = app.document()["query"].clone();
        q["running"].is_null() && (q["results"].as_array().is_some_and(|r| !r.is_empty()) || !q["error"].is_null())
    });
    let q = app.document()["query"].clone();
    assert!(q["error"].is_null(), "{q}");
    assert_eq!(q["text"], JOIN, "Ctrl+Enter put no newline in");
    let n = q["results"][0]["rows"].as_u64().unwrap();
    assert_eq!(n as usize, rows.len(), "the join finds the filtered orders: {q}");
    assert_eq!(app.value("query-rows"), format!("{n} rows"));
    // The result bar: the result's tab, the facts with the time, EXPORT off.
    assert!(app.tag("result-1").is_some_and(|t| t["visible"] != false), "the result's tab");
    let facts = app.value("query-facts");
    let ms = facts.as_str().and_then(|f| f.strip_prefix(&format!("{n} rows "))).and_then(|f| f.strip_suffix(" ms"));
    assert!(ms.is_some_and(|ms| ms.parse::<u64>().is_ok()), "the facts say the rows and the time: {facts}");
    assert!(app.tag("query-export").is_some_and(|t| t["enabled"] == false), "EXPORT off");
    // The value toggle in the result bar's row, between the run row and the grid.
    let (toggle, run, head) = (app.rect("value-toggle"), app.rect("query-run"), app.rect("grid-header-0"));
    let toggle_y = toggle[1] + toggle[3] / 2.0;
    assert!(toggle_y > run[1] + run[3] && toggle_y < head[1], "the toggle in the result bar: {toggle:?}, {run:?}, {head:?}");
    app.shot(&root, "7a-query");
    app.click("grid-cell-0-0");
    assert_eq!(app.value("query-editor")["focused"], false, "the grid took the keyboard");
    app.ctrl("A");
    assert_eq!(app.document()["selection"], json!({ "anchor": [0, 0], "focus": [n - 1, 3], "whole": false }));
    app.ctrl_shift("C");
    // The clipboard still holds step 4's line: wait for the new copy.
    let header = "id\tcustomer\ttotal\tshipped_at\n";
    app.until("the copy", |app| app.document()["clipboard"].as_str().is_some_and(|c| c.starts_with(header)));
    let d = app.document();
    let copied = d["clipboard"].as_str().unwrap().to_string();
    assert!(copied.starts_with(header), "the header row first: {copied}");
    assert!(copied.lines().any(|l| l == format!("{}\t{}\t4242.42\t", day.order, day.fixed)), "the day's order, its NULL date empty: {copied}");
    assert_eq!(copied.lines().count() as u64, n + 1, "the header and every row: {copied}");
    assert_eq!(d["clipboardBytes"].as_u64(), Some(copied.len() as u64), "all of it under 4 KiB");
    let rendered = app.execute("copy.selection", json!({ "format": "tsvHeader" }));
    assert_eq!(rendered["text"].as_str(), Some(copied.as_str()), "the engine's rendering of the selection");
    assert_eq!(app.value("notice"), format!("Copied {n} rows"));
    app.shot(&root, "7b-copied");

    // 7c. A failed query: the band under the result bar, FAILED in the bar, the error's line
    //     number red and its token underlined.
    let run = |app: &mut App, sql: &str, says: &str| {
        app.click("query-editor");
        app.ctrl("A");
        app.text(sql);
        assert_eq!(app.document()["query"]["text"], sql);
        app.ctrl("Enter");
        app.until("the run's error", |app| {
            let q = app.document()["query"].clone();
            q["running"].is_null() && q["error"]["message"].as_str().is_some_and(|m| m.contains(says))
        });
        app.until("the band", |app| app.tag("query-error").is_some_and(|t| t["value"]["message"].as_str().is_some_and(|m| m.contains(says))));
    };
    run(&mut app, "SELECT o.totl FROM shop.orders o", "totl");
    let card = app.value("query-error");
    assert_eq!((card["heading"].clone(), card["line"].clone()), (json!("Query failed"), json!(1)), "{card}");
    let ed = app.value("query-editor");
    assert!(ed["errorLine"] == 1 && !ed["underline"].is_null(), "the red number and the underline: {ed}");
    assert!(app.value("query-facts").as_str().is_some_and(|f| f.starts_with("Failed")), "the bar says it failed");
    app.shot(&root, "7c-failed");
    //     A failed transaction: ROLLBACK in the band ends it.
    run(&mut app, "BEGIN; SELECT nope;", "nope");
    app.until("ROLLBACK", |app| app.tag("query-rollback").is_some());
    assert_eq!(app.document()["connection"]["transaction"]["state"], "failed");
    app.shot(&root, "7d-rollback");
    app.click("query-rollback");
    app.until("the rollback", |app| app.document()["connection"]["transaction"].is_null());
    app.until("its toast", |app| app.tag("notice").is_some_and(|t| t["value"].as_str().is_some_and(|v| v.starts_with("Rolled back on"))));
    app.until("ROLLBACK gone", |app| app.tag("query-rollback").is_none());

    // 8. The tree's right-click menu: filtered to order_items, Copy name, then Open. The count
    //    says how many of the tables match.
    let all = app.value("tree-count");
    let n = all.as_str().and_then(|w| w.strip_suffix(" tables")).unwrap_or_else(|| panic!("the unfiltered count: {all}")).to_owned();
    app.replace("tree-filter", "order_it");
    app.until("the filtered row", |app| app.tag("tree-table-shop.order_items").is_some_and(|t| t["visible"] != false));
    let some = app.value("tree-count");
    let m = some.as_str().and_then(|w| w.strip_suffix(&format!(" of {n} tables"))).and_then(|m| m.parse::<u64>().ok());
    assert!(m.is_some_and(|m| m >= 1), "`<m> of {n} tables`: {some}");
    app.right_click("tree-table-shop.order_items");
    app.until("the tree's menu", |app| app.tag("tree-menu-open").is_some());
    let items: Vec<Json> = app.tags_with("tree-menu-").into_iter().map(|t| t["value"].clone()).collect();
    assert_eq!(
        items,
        [
            json!({ "label": "Open", "shortcut": "Enter" }),
            json!({ "label": "New query", "shortcut": "Ctrl+T" }),
            json!({ "label": "Copy name", "shortcut": "Ctrl+C" }),
        ],
        "the menu's items in order"
    );
    app.shot(&root, "8a-tree-menu");
    app.click("tree-menu-copy-name");
    app.until("the name copied", |app| app.document()["clipboard"] == "order_items");
    app.right_click("tree-table-shop.order_items");
    app.until("the tree's menu", |app| app.tag("tree-menu-open").is_some());
    app.click("tree-menu-open");
    let t = app.settled("the order_items page");
    assert_eq!((t["schema"].clone(), t["name"].clone()), (json!("shop"), json!("order_items")), "{t}");
    app.shot(&root, "8b-tree-opened");
    //     The keys the menu names: a click gives the row the keyboard, Ctrl+C copies its name (a
    //     grid copy first, so the name is new on the clipboard), and Enter opens it.
    app.click("grid-cell-0-0");
    app.ctrl("C");
    app.until("the grid's copy", |app| app.document()["clipboard"] != "order_items");
    app.click("tree-table-shop.order_items");
    app.ctrl("C");
    app.until("the name copied by Ctrl+C", |app| app.document()["clipboard"] == "order_items");
    let tabs = app.document()["tabs"].as_array().map_or(0, Vec::len);
    app.ctrl("W");
    app.until("the tab closed", |app| app.document()["tabs"].as_array().map_or(0, Vec::len) + 1 == tabs);
    app.click("tree-table-shop.order_items");
    app.key("Enter");
    let t = app.settled("the order_items page from Enter");
    assert_eq!((t["schema"].clone(), t["name"].clone()), (json!("shop"), json!("order_items")), "{t}");
    assert_eq!(app.document()["tabs"].as_array().map_or(0, Vec::len), tabs, "Enter opened one tab");
    app.shot(&root, "8c-tree-enter");
    //     A filter with no match reads `0 TABLES` over an empty tree; cleared, the whole count.
    app.replace("tree-filter", "zzz");
    app.until("no match", |app| app.tag("tree-count").is_some_and(|t| t["value"] == "0 tables"));
    assert!(app.tags_with("tree-table-").is_empty(), "an empty tree: {:?}", app.tags_with("tree-table-"));
    assert!(app.tag("tree-empty").is_none(), "no sentence under it");
    app.shot(&root, "8d-tree-no-match");
    app.click("tree-filter");
    app.ctrl("A");
    app.key("Backspace");
    app.until("the whole count again", |app| app.tag("tree-count").is_some_and(|t| t["value"] == format!("{n} tables")));

    // No seed row changed, and once the app and the day's rows are gone the counts are as before.
    assert_eq!(day.seed_digest(), seed, "the seed's customers and orders are unchanged");
    drop(app);
    drop(day);
    assert_eq!(counts(pg14(db)), before, "the day's customer and order are gone");
}
