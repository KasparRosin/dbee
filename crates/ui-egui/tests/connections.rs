//! The connection screen driven through the control channel, one frame at a time in a kittest
//! harness over [`DbeeApp::ui`]: the engine cards, the form for each database type, the saved
//! and recent lists, the dialogs a connect asks for, and inline errors. Everything goes through
//! control requests and automation ids, the way an agent drives the real window; assertions read
//! `ui.inspect` and `document.inspect`.
//!
//! Safety: every session keeps its connections file in a throwaway home, its secrets in a
//! `MemoryStore` (or none), and reads SSH keys and `known_hosts` from `<temp home>/.ssh` with no
//! agent. Tests that need a database or the sshd connect only to the Docker test servers
//! (`require_testdb!`); the others use port 1 or a silent listener of their own on 127.0.0.1.
//! The file picker (rfd) is never opened: a key path comes from a saved connection.

mod common;

use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use dbee_config::{ConnectionConfig, DbKind, SecretKind, SecretStore, Settings, SshAuth, SshConfig, SslMode, StoredSecret};
use dbee_driver::Drivers;
use dbee_testkit::{TempHome, TestDb, temp_home};
use serde_json::{Value as Json, json};

/// A 1200 × 900 window on a throwaway home with a memory secret store, showing the engine cards
/// of a new connection.
fn cards() -> Ui {
    Ui::fresh(egui::vec2(1200.0, 900.0))
}

/// [`cards`] with PostgreSQL chosen: the form of a new PostgreSQL connection.
fn ui() -> Ui {
    let mut ui = cards();
    ui.click("conn-kind-postgres");
    ui
}

/// The secret the store keeps as `kind` of `id`, without the settings kept with it.
fn stored(ui: &Ui, id: &str, kind: SecretKind) -> Option<String> {
    match StoredSecret::parse(&ui.secrets.as_ref().unwrap().get(id, kind).unwrap()?) {
        StoredSecret::For { secret, .. } => Some(secret),
        other => panic!("not in the store's own form: {other:?}"),
    }
}

/// A 1200 × 900 window on `home`, with a memory secret store or none, showing the engine cards.
fn ui_in(home: TempHome, with_secrets: bool) -> Ui {
    Ui::window(Setup { size: egui::vec2(1200.0, 900.0), home: Some(home), secrets: with_secrets, ..Setup::default() })
}

/// `127.0.0.1:1`, a server of `kind` that answers nothing, named `name`.
fn server_of(kind: DbKind, name: &str) -> ConnectionConfig {
    ConnectionConfig { name: name.into(), kind, port: kind.default_port().unwrap_or(1), ..silent_config(1) }
}

/// A SQLite connection to `file` (never opened), named `name`.
fn file_of(file: &str, name: &str) -> ConnectionConfig {
    ConnectionConfig { name: name.into(), kind: DbKind::Sqlite, file: Some(file.into()), host: String::new(), port: 0, ..ConnectionConfig::default() }
}

impl Ui {
    /// Clicks the text field `id`, selects all of it and types `text` over it.
    fn replace(&mut self, id: &str, text: &str) {
        self.click(id);
        self.ok("ui.key", json!({ "key": "A", "cmd": true }));
        self.text(text);
    }

    /// The id of the saved connection named `name`.
    fn saved_id(&mut self, name: &str) -> String {
        let list = self.execute("connection.list", json!({}));
        list["saved"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("no saved `{name}` in {list}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Saves `config` (with `password`) through the engine and returns its card's automation id.
    fn save(&mut self, config: &ConnectionConfig, password: Option<&str>) -> String {
        let mut params = json!({ "config": config });
        if let Some(p) = password {
            params["password"] = json!(p);
        }
        let id = self.execute("connection.save", params)["id"].as_str().unwrap().to_string();
        self.frame();
        format!("conn-saved-{id}")
    }
}

fn rect(entry: &Json) -> [f64; 4] {
    let r: Vec<f64> = entry["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    [r[0], r[1], r[2], r[3]]
}

/// pg14 through the test sshd, logging in with `auth`.
fn via_ssh(db: &TestDb, auth: SshAuth) -> ConnectionConfig {
    ConnectionConfig {
        name: "pg14 via ssh".into(),
        host: pg14_server(db).name.clone(),
        port: pg14_server(db).service_port,
        ssh: Some(SshConfig { host: "127.0.0.1".into(), port: db.sshd.port, user: db.sshd.user.clone(), auth, ..SshConfig::default() }),
        ..pg14(db)
    }
}

/// A listener on 127.0.0.1 that accepts connections and never answers, until dropped: a connect
/// to it runs until then.
struct Silent {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Silent {
    fn start() -> Silent {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            while !flag.load(Ordering::Relaxed) {
                if let Ok((sock, _)) = listener.accept() {
                    held.push(sock);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Silent { port, stop, thread: Some(thread) }
    }
}

impl Drop for Silent {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn silent_config(port: u16) -> ConnectionConfig {
    ConnectionConfig { name: "silent".into(), host: "127.0.0.1".into(), port, user: "u".into(), database: "d".into(), ..ConnectionConfig::default() }
}

#[test]
fn new_connection_starts_with_the_engine_cards() {
    let mut ui = cards();
    let i = ui.inspect();
    for kind in DbKind::ALL {
        let card = tag_in(&i, &format!("conn-kind-{}", kind.id())).unwrap_or_else(|| panic!("no card for {kind:?}: {i}"));
        assert_eq!(card["value"], json!({ "label": kind.label(), "upcoming": false }), "{card}");
        assert!(card.get("enabled").is_none(), "{card}");
    }
    assert_eq!(tag_in(&i, "conn-title").unwrap()["value"], "New connection");
    for id in ["conn-host", "conn-file", "conn-name", "conn-save", "conn-change-engine"] {
        assert!(tag_in(&i, id).is_none(), "{id} before an engine is chosen: {i}");
    }
    // The cards and the form share the title row: choosing moves neither the title nor Fill from URL.
    let title = rect(&tag_in(&i, "conn-title").unwrap());
    let url = rect(&tag_in(&i, "conn-fill-url").unwrap());
    ui.click("conn-kind-postgres");
    let i = ui.inspect();
    let now = rect(&tag_in(&i, "conn-title").unwrap());
    assert_eq!(([now[0], now[1], now[3]], rect(&tag_in(&i, "conn-fill-url").unwrap())), ([title[0], title[1], title[3]], url));
    assert_eq!(tag_in(&i, "conn-kind").unwrap()["value"], "postgres");
    assert_eq!(tag_in(&i, "conn-port").unwrap()["value"], "5432");
    // The type and Change engine join the title row, left of Fill from URL and as wide as it.
    let change = rect(&tag_in(&i, "conn-change-engine").unwrap());
    let kind = rect(&tag_in(&i, "conn-kind").unwrap());
    assert_eq!((change[1], change[2], change[3]), (url[1], url[2], url[3]), "{change:?} {url:?}");
    assert!(kind[0] + kind[2] <= change[0] && change[0] + change[2] <= url[0], "{kind:?} {change:?} {url:?}");
}

#[test]
fn every_row_of_a_server_form_is_in_view_at_the_default_window() {
    for kind in DbKind::ALL {
        let Settings::Server { ssh, .. } = kind.info().settings else { continue };
        // The app's window when it starts.
        let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
        ui.click(&format!("conn-kind-{}", kind.id()));
        let i = ui.inspect();
        // A check box in view is as tall as the one near the top of the form.
        let whole = rect(&tag_in(&i, "conn-save-secrets").unwrap())[3];
        let mut rows: Vec<String> = kind.info().options.iter().map(|o| format!("conn-option-{}", o.id)).collect();
        rows.push("conn-readonly".into());
        if ssh {
            rows.push("conn-ssh-on".into());
        }
        for id in rows {
            let tag = tag_in(&i, &id).unwrap_or_else(|| panic!("{kind:?}: no {id}"));
            assert!(tag.get("visible").is_none() && rect(&tag)[3] == whole, "{kind:?}: {id} is not whole in view: {tag}");
        }
    }
}

#[test]
fn the_title_row_stays_in_the_card_at_the_smallest_window() {
    // The app's smallest window.
    let mut ui = Ui::fresh(egui::vec2(640.0, 420.0));
    let check = |ui: &mut Ui, what: &str| {
        let i = ui.inspect();
        let card = rect(&tag_in(&i, "conn-card").unwrap());
        let row: Vec<(&str, [f64; 4])> = ["conn-title", "conn-kind", "conn-change-engine", "conn-fill-url"]
            .into_iter()
            .filter_map(|id| tag_in(&i, id).map(|t| (id, rect(&t))))
            .collect();
        assert!(row.len() >= 2, "{what}: {row:?}");
        for (id, r) in &row {
            let inside = r[2] > 0.0 && r[0] >= card[0] && r[1] >= card[1] && r[0] + r[2] <= card[0] + card[2] && r[1] + r[3] <= card[1] + card[3];
            assert!(inside, "{what}: {id} {r:?} is not inside the card {card:?}");
        }
        for (n, (a, ra)) in row.iter().enumerate() {
            for (b, rb) in &row[n + 1..] {
                let apart = ra[0] + ra[2] <= rb[0] || rb[0] + rb[2] <= ra[0] || ra[1] + ra[3] <= rb[1] || rb[1] + rb[3] <= ra[1];
                assert!(apart, "{what}: {a} {ra:?} overlaps {b} {rb:?}");
            }
        }
        i
    };
    let i = check(&mut ui, "the cards");
    // The engine cards wrap, inside the card.
    let card = rect(&tag_in(&i, "conn-card").unwrap());
    let [postgres, sqlite] = ["conn-kind-postgres", "conn-kind-sqlite"].map(|id| rect(&tag_in(&i, id).unwrap()));
    assert!(sqlite[1] > postgres[1], "{postgres:?} {sqlite:?}");
    assert!(postgres[0] >= card[0] && sqlite[0] + sqlite[2] <= card[0] + card[2], "{card:?} {postgres:?} {sqlite:?}");
    ui.click("conn-kind-postgres");
    check(&mut ui, "PostgreSQL");
    ui.click("conn-change-engine");
    ui.click("conn-kind-sqlite");
    check(&mut ui, "SQLite");
}

#[test]
fn switching_to_mysql_sets_its_port() {
    let mut ui = cards();
    ui.click("conn-new");
    ui.click("conn-kind-mysql");
    assert_eq!(ui.value("conn-kind"), "mysql");
    assert_eq!(ui.value("conn-port"), "3306");
    // Change engine keeps the name and the colour; the default port follows the engine.
    ui.click("conn-name");
    ui.text("Shop");
    ui.click("conn-color-red");
    ui.click("conn-change-engine");
    assert!(ui.tag("conn-host").is_none() && ui.tag("conn-kind-postgres").is_some(), "back on the cards");
    ui.click("conn-kind-postgres");
    let i = ui.inspect();
    let shown = |id: &str| tag_in(&i, id).unwrap()["value"].clone();
    assert_eq!(
        (shown("conn-kind"), shown("conn-port"), shown("conn-name"), shown("conn-color")),
        (json!("postgres"), json!("5432"), json!("Shop"), json!("red"))
    );
}

#[test]
fn a_typed_port_survives_a_type_switch() {
    let mut ui = ui();
    ui.replace("conn-port", "5544");
    ui.click("conn-change-engine");
    ui.click("conn-kind-mariadb");
    assert_eq!(ui.value("conn-kind"), "mariadb");
    assert_eq!(ui.value("conn-port"), "5544");
}

#[test]
fn sqlite_shows_only_file_rows() {
    let mut ui = cards();
    ui.click("conn-kind-sqlite");
    let i = ui.inspect();
    assert_eq!(tag_in(&i, "conn-kind").unwrap()["value"], "sqlite");
    for id in ["conn-name", "conn-color", "conn-file", "conn-file-choose", "conn-file-new", "conn-readonly", "conn-save", "conn-test", "conn-connect"]
    {
        assert!(tag_in(&i, id).is_some(), "{id}: {i}");
    }
    for id in ["conn-host", "conn-port", "conn-user", "conn-password", "conn-save-secrets", "conn-database", "conn-ssl", "conn-ca", "conn-ssh-on"] {
        assert!(tag_in(&i, id).is_none(), "{id}: {i}");
    }
    assert_eq!(tag_in(&i, "conn-file").unwrap()["value"], Json::Null);
    // Without a file there is nothing to save.
    ui.click("conn-save");
    assert!(ui.value("conn-error").as_str().unwrap().contains("no file chosen"));
    assert_eq!(ui.execute("connection.list", json!({}))["saved"], json!([]));
    // A server type has its rows again.
    ui.click("conn-change-engine");
    ui.click("conn-kind-postgres");
    assert!(ui.tag("conn-host").is_some() && ui.tag("conn-ssh-on").is_some() && ui.tag("conn-file").is_none());
}

#[test]
fn mysql_url_fills_type_and_fields() {
    let mut ui = cards();
    ui.click("conn-fill-url");
    ui.text("mysql://u@h:3307/shop");
    ui.click("dialog-ok");
    let i = ui.inspect();
    assert_eq!(i["dialog"], Json::Null, "{i}");
    let shown = |id: &str| tag_in(&i, id).unwrap()["value"].clone();
    assert_eq!(
        (shown("conn-kind"), shown("conn-host"), shown("conn-port"), shown("conn-user"), shown("conn-database")),
        (json!("mysql"), json!("h"), json!("3307"), json!("u"), json!("shop"))
    );
}

#[test]
fn a_url_on_the_cards_skips_them() {
    let mut ui = cards();
    ui.click("conn-fill-url");
    ui.text("mariadb://db.example/shop");
    ui.click("dialog-ok");
    let i = ui.inspect();
    assert!(ids(&i).iter().all(|id| !id.starts_with("conn-kind-")), "the cards are gone: {i}");
    assert_eq!(tag_in(&i, "conn-kind").unwrap()["value"], "mariadb");
    // No port in the URL: the type's default.
    assert_eq!(tag_in(&i, "conn-port").unwrap()["value"], "3306");
    // In the form of a new connection, a URL of another type switches it.
    ui.click("conn-fill-url");
    ui.text("postgresql://ann@pg.example:6543/shop");
    ui.click("dialog-ok");
    assert_eq!((ui.value("conn-kind"), ui.value("conn-port"), ui.value("conn-host")), (json!("postgres"), json!("6543"), json!("pg.example")));
}

#[test]
fn cards_name_the_type() {
    let mut ui = cards();
    let maria = ui.save(&server_of(DbKind::MariaDb, "Maria"), None);
    assert_eq!(ui.value(&maria), "Maria");
    assert_eq!(ui.value(&format!("{maria}-kind")), "MariaDB");
    let file = ui.save(&file_of("/data/shop.sqlite", "Shop file"), None);
    assert_eq!(ui.value(&format!("{file}-kind")), "SQLite");
    let pg = ui.save(&silent_config(1), None);
    assert_eq!(ui.value(&format!("{pg}-kind")), "PostgreSQL");
}

/// A SQLite file on a row follows the status bar's path rule: in the home folder (the process's
/// `HOME`; the file is never opened), its path starts with `~`; short of room it is cut in the
/// middle with `…`, the file's name kept; hovered, the row shows it whole under the name.
#[test]
fn a_sqlite_path_on_a_row_is_cut_in_the_middle_and_whole_on_hover() {
    let Some(home) = dbee_config::home_dir().filter(|h| h.to_str().is_some_and(|h| !h.is_empty())) else {
        eprintln!("SKIPPED: no home folder to write `~` for");
        return;
    };
    let file = home.join("Projects").join("clients").join("northwind-traders").join("archive").join("2026").join("nightly").join("notes.sqlite");
    let path = file.to_str().unwrap_or_default().to_owned();
    let whole = format!("~{}", &path[home.to_str().unwrap_or_default().len()..]);
    let mut ui = cards();
    let card = ui.save(&file_of(&path, "Notes"), None);
    let row = rect_of(&ui.tag(&card).unwrap());
    let drawn: Vec<String> = ui.texts().into_iter().filter(|(r, _)| row.contains_rect(*r)).map(|(_, g)| g.text().to_owned()).collect();
    let [name, line] = drawn.as_slice() else { panic!("the name and the line: {drawn:?}") };
    assert_eq!(name, "Notes");
    let (head, tail) = line.split_once('…').unwrap_or_else(|| panic!("cut in the middle: `{line}` of `{whole}`"));
    assert!(head.starts_with('~') && whole.starts_with(head) && whole.ends_with(tail), "`{line}` of `{whole}`");
    assert!(tail.ends_with("notes.sqlite") && tail.len() > "notes.sqlite".len(), "the file's name kept, with the folder mark: `{line}`");
    assert!(ui.texts().iter().all(|(_, g)| g.text() != whole), "not whole before the hover");
    let at = ui.centre(&card);
    ui.rest(at);
    let hover = format!("Notes\n{whole}");
    assert!(ui.texts().iter().any(|(_, g)| g.text() == hover), "whole on hover: {:?}", ui.texts().iter().map(|(_, g)| g.text()).collect::<Vec<_>>());
}

/// Review 1, M1: a row whose name or line is cut shows both whole on hover, the name over the
/// line; a row with nothing cut shows no hover.
#[test]
fn a_cut_name_on_a_row_reads_whole_on_hover() {
    const LONG: &str = "Reporting replica for the Customer Success team (read-only, EU)";
    let mut ui = cards();
    for (name, cut) in [(LONG, true), ("Shop", false)] {
        let card = ui.save(&server_of(DbKind::Postgres, name), None);
        let row = rect_of(&ui.tag(&card).unwrap());
        let drawn: Vec<(String, bool)> =
            ui.texts().into_iter().filter(|(r, _)| row.contains_rect(*r)).map(|(_, g)| (g.text().to_owned(), g.elided)).collect();
        let [(drawn_name, elided), (line, _)] = drawn.as_slice() else { panic!("the name and the line: {drawn:?}") };
        assert_eq!((drawn_name.as_str(), *elided), (name, cut), "`{name}` cut: {drawn:?}");
        let whole = format!("{name}\n{line}");
        let at = ui.centre(&card);
        ui.rest(at);
        let hover = ui.texts().iter().any(|(_, g)| g.text() == whole);
        assert_eq!(
            hover,
            cut,
            "`{name}`: name and line whole on hover only when cut: {:?}",
            ui.texts().iter().map(|(_, g)| g.text()).collect::<Vec<_>>()
        );
        ui.hover(egui::pos2(1.0, 1.0));
    }
}

#[test]
fn the_filter_finds_what_the_cards_show() {
    let mut ui = cards();
    let maria = ui.save(&server_of(DbKind::MariaDb, "Shop"), None);
    // A file connection saved with a server's leftover host.
    let file = ui.save(&ConnectionConfig { host: "localhost".into(), ..file_of("/data/files/orders.sqlite", "Orders") }, None);
    ui.click("conn-filter");
    for (q, shown) in [
        ("mariadb", [true, false]),
        ("data/files", [false, true]),
        ("sqlite", [false, true]),
        ("127.0.0.1", [true, false]),
        ("localhost", [false, false]),
    ] {
        ui.replace("conn-filter", q);
        let i = ui.inspect();
        assert_eq!([tag_in(&i, &maria).is_some(), tag_in(&i, &file).is_some()], shown, "filter {q:?}");
    }
}

/// The connection list's filter is the sidebar's filter (`components::sidebar`), without a
/// refresh: `conn-filter` keeps its id and its value as typed; the field is 35 tall over a 1 px
/// `line`, its placeholder `Filter connections` as Screens writes it (node 61737); what is typed is
/// drawn as typed. A click gives it focus without the ring, Shift+Tab back to it shows the ring.
#[test]
fn the_connection_filter_is_the_sidebar_filter() {
    let mut ui = cards();
    let e = ui.tag("conn-filter").expect("the filter is drawn");
    assert_eq!((&e["value"], e.get("enabled")), (&json!(""), None), "{e}");
    let f = rect_of(&e);
    assert_eq!(f.height(), 35.0, "the field, 35 above its line: {f:?}");
    assert!(ui.text_in(f, "Filter connections").is_some(), "the placeholder");
    let t = dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    let line = ui.shapes().into_iter().any(|(_, s)| {
        matches!(s, egui::Shape::Rect(r) if r.fill == t.line && r.rect.height() == 1.0 && (r.rect.top() - f.bottom()).abs() < 0.5
            && r.rect.left() <= f.left() && r.rect.right() >= f.right())
    });
    assert!(line, "a 1 px line under the field");
    let ink = t.ink;
    let ring = |ui: &Ui| {
        let at = f.shrink(2.0);
        ui.shapes().iter().any(|(_, s)| {
            matches!(s, egui::Shape::Rect(r) if r.stroke.width == 2.0 && r.stroke.color == ink
                && (r.rect.min - at.min).length() < 0.5 && (r.rect.max - at.max).length() < 0.5)
        })
    };
    ui.click("conn-filter");
    ui.text("Shop Live");
    ui.frame();
    assert_eq!(ui.value("conn-filter"), "Shop Live");
    assert!(ui.text_in(f, "Shop Live").is_some() && ui.text_in(f, "SHOP LIVE").is_none(), "drawn as typed");
    assert!(!ring(&ui), "a click shows no ring");
    ui.key("Tab");
    ui.frame();
    ui.key_with("Tab", json!({ "shift": true }));
    ui.frame();
    assert!(ring(&ui), "Shift+Tab back shows the ring");
    assert_eq!(rect_of(&ui.tag("conn-filter").expect("still drawn")), f, "nothing moved");
}

#[test]
fn a_saved_config_holds_only_what_its_type_uses() {
    let mut ui = cards();
    // A SQLite connection saved with a server's leftovers, then saved again from the form.
    let leftovers =
        ConnectionConfig { host: "localhost".into(), user: "ann".into(), ssl_mode: SslMode::Require, ..file_of("/data/shop.sqlite", "Shop file") };
    let card = ui.save(&leftovers, None);
    ui.click(&card);
    ui.click("conn-save");
    let list = ui.execute("connection.list", json!({}));
    let saved = &list["saved"][0];
    assert_eq!((&saved["host"], &saved["port"], &saved["user"], &saved["sslMode"]), (&json!(""), &json!(0), &json!(""), &json!("prefer")), "{saved}");
    assert_eq!(saved["file"], "/data/shop.sqlite");
    // Its copy made a PostgreSQL connection keeps no file, and no MySQL option it once had.
    ui.right_click(&card);
    ui.click("conn-menu-duplicate");
    ui.click("conn-change-engine");
    ui.click("conn-kind-mysql");
    ui.click("conn-option-allowPublicKey");
    ui.click("conn-change-engine");
    ui.click("conn-kind-postgres");
    ui.replace("conn-host", "db.example");
    ui.replace("conn-port", "5432");
    ui.click("conn-save");
    let id = ui.saved_id("Shop file (copy)");
    let list = ui.execute("connection.list", json!({}));
    let copy = list["saved"].as_array().unwrap().iter().find(|c| c["id"] == id.as_str()).unwrap().clone();
    assert_eq!((&copy["kind"], &copy["host"], &copy["file"]), (&json!("postgres"), &json!("db.example"), &Json::Null), "{copy}");
    assert!(copy.get("options").is_none(), "{copy}");
}

#[test]
fn only_a_connection_opened_from_the_list_has_its_type_fixed() {
    let mut ui = ui();
    ui.click("conn-name");
    ui.text("Mid edit");
    ui.click("conn-save");
    assert!(ui.tag("conn-change-engine").is_some(), "saved during this edit, its type can still change");
    // Saving after Change engine changes the saved connection's type.
    ui.click("conn-change-engine");
    ui.click("conn-kind-mariadb");
    ui.click("conn-save");
    let id = ui.saved_id("Mid edit");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!((&list["saved"][0]["kind"], &list["saved"][0]["port"]), (&json!("mariadb"), &json!(3306)), "{list}");
    // Opened from the list, its type is fixed.
    let card = format!("conn-saved-{id}");
    ui.click(&card);
    assert!(ui.tag("conn-change-engine").is_none());
    // A duplicate is a new connection.
    ui.right_click(&card);
    ui.click("conn-menu-duplicate");
    assert_eq!(ui.value("conn-name"), "Mid edit (copy)");
    assert!(ui.tag("conn-change-engine").is_some());
}

#[test]
fn an_upcoming_engine_cannot_be_chosen() {
    // A build with the Postgres and SQLite drivers only: MySQL and MariaDB are upcoming.
    let drivers = Drivers::new(dbee_drivers::all().iter().filter(|d| !d.kinds().contains(&DbKind::MySql)).cloned().collect());
    let mut ui = Ui::window(Setup { size: egui::vec2(1200.0, 900.0), drivers: Some(drivers), ..Setup::default() });
    let i = ui.inspect();
    for (kind, upcoming) in [(DbKind::Postgres, false), (DbKind::MySql, true), (DbKind::MariaDb, true), (DbKind::Sqlite, false)] {
        let card = tag_in(&i, &format!("conn-kind-{}", kind.id())).unwrap();
        assert_eq!(card["value"]["upcoming"], upcoming, "{card}");
        assert_eq!(card.get("enabled").is_none(), !upcoming, "{card}");
    }
    // The Upcoming chip, 8 px into the card's top right corner: its words 1 + 6 px inside it.
    let [x, y, w, h] = rect(&tag_in(&i, "conn-kind-mysql").unwrap()).map(|v| v as f32);
    let card = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h));
    let (words, cut) = ui.text_in(card, "UPCOMING").expect("the Upcoming chip's words");
    assert!(!cut);
    assert!((words.right() - (card.right() - 15.0)).abs() < 0.5 && (words.center().y - (card.top() + 17.0)).abs() < 0.5, "{words:?} in {card:?}");
    assert!(ui.text_in(card, "Upcoming").is_none(), "only drawn in capitals");
    ui.click("conn-kind-mysql");
    assert!(ui.tag("conn-host").is_none() && ui.tag("conn-kind-mysql").is_some(), "nothing was chosen");
    // Nor through a URL: the error names what this build opens.
    ui.click("conn-fill-url");
    ui.text("mysql://u@h/shop");
    ui.click("dialog-ok");
    assert_eq!(ui.inspect()["dialog"], "fillFromUrl");
    let error = ui.value("dialog-error").as_str().unwrap().to_string();
    assert!(error.contains("postgres://") && !error.contains("mysql://"), "{error}");
    ui.click("dialog-cancel");
    assert!(ui.tag("conn-kind-mysql").is_some(), "still the cards");
}

#[test]
fn an_existing_connection_shows_its_engine_fixed() {
    let mut ui = cards();
    let card = ui.save(&server_of(DbKind::MySql, "Shop mysql"), None);
    ui.click(&card);
    let i = ui.inspect();
    assert_eq!(tag_in(&i, "conn-kind").unwrap()["value"], "mysql");
    assert_eq!(tag_in(&i, "conn-title").unwrap()["value"], "Shop mysql");
    assert!(tag_in(&i, "conn-change-engine").is_none(), "{i}");
    assert!(ids(&i).iter().all(|id| !id.starts_with("conn-kind-")), "no cards: {i}");
    // Fill from URL keeps to its engine.
    ui.click("conn-fill-url");
    ui.text("postgres://h/db");
    ui.click("dialog-ok");
    assert_eq!(ui.inspect()["dialog"], "fillFromUrl");
    let error = ui.value("dialog-error").as_str().unwrap().to_string();
    assert!(error.contains("mysql://") && !error.contains("postgres://"), "{error}");
    ui.replace("dialog-url", "mysql://other.example:3310/db2");
    ui.click("dialog-ok");
    assert_eq!((ui.value("conn-kind"), ui.value("conn-host"), ui.value("conn-port")), (json!("mysql"), json!("other.example"), json!("3310")));
    // A saved file connection: its file, and no URL to fill from.
    let card = ui.save(&file_of("/data/shop.sqlite", ""), None);
    ui.click(&card);
    let i = ui.inspect();
    assert_eq!(tag_in(&i, "conn-kind").unwrap()["value"], "sqlite");
    assert_eq!(tag_in(&i, "conn-file").unwrap()["value"], "/data/shop.sqlite");
    assert_eq!(tag_in(&i, "conn-title").unwrap()["value"], "shop.sqlite");
    assert!(tag_in(&i, "conn-fill-url").is_none() && tag_in(&i, "conn-change-engine").is_none(), "{i}");
}

#[test]
fn a_mysql_option_round_trips_through_save() {
    let mut ui = cards();
    ui.click("conn-kind-postgres");
    assert!(ids(&ui.inspect()).iter().all(|id| !id.starts_with("conn-option-")), "PostgreSQL has no options");
    ui.click("conn-change-engine");
    ui.click("conn-kind-mysql");
    assert_eq!(ui.value("conn-option-allowPublicKey"), false);
    ui.click("conn-name");
    ui.text("keyed");
    ui.click("conn-option-allowPublicKey");
    ui.click("conn-save");
    let id = ui.saved_id("keyed");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"][0]["options"], json!({ "allowPublicKey": "true" }), "{list}");
    // Loaded again, it shows on.
    ui.click("conn-new");
    ui.click(&format!("conn-saved-{id}"));
    assert_eq!(ui.value("conn-option-allowPublicKey"), true);
    // Off, it is gone from the file.
    ui.click("conn-option-allowPublicKey");
    ui.click("conn-save");
    let list = ui.execute("connection.list", json!({}));
    assert!(list["saved"][0].get("options").is_none(), "{list}");
}

#[test]
fn fill_from_url_fills_the_form() {
    let mut ui = ui();
    ui.click("conn-fill-url");
    assert_eq!(ui.inspect()["dialog"], "fillFromUrl");
    // A URL that does not parse keeps the dialog open with the reason.
    ui.text("oracle://nope");
    ui.click("dialog-ok");
    let i = ui.inspect();
    assert_eq!(i["dialog"], "fillFromUrl", "{i}");
    let error = tag_in(&i, "dialog-error").expect("the reason shows in the dialog");
    assert!(error["value"].as_str().unwrap().contains("postgres://"), "{error}");
    ui.replace("dialog-url", "postgresql://ann:url-secret@db.example.com:6543/shop?sslmode=require");
    let i = ui.inspect();
    assert!(!i.to_string().contains("url-secret"), "the URL can hold a password: {i}");
    ui.click("dialog-ok");
    let i = ui.inspect();
    assert_eq!(i["dialog"], Json::Null, "{i}");
    assert_eq!(tag_in(&i, "conn-host").unwrap()["value"], "db.example.com");
    assert_eq!(tag_in(&i, "conn-port").unwrap()["value"], "6543");
    assert_eq!(tag_in(&i, "conn-user").unwrap()["value"], "ann");
    assert_eq!(tag_in(&i, "conn-database").unwrap()["value"], "shop");
    assert_eq!(tag_in(&i, "conn-ssl").unwrap()["value"], "require");
    let password = tag_in(&i, "conn-password").unwrap();
    assert_eq!((password["masked"].clone(), password["set"].clone()), (json!(true), json!(true)), "{password}");
    assert!(!i.to_string().contains("url-secret"), "{i}");
    let journal = ui.ok("engine.journal", json!({}));
    assert!(!journal.to_string().contains("url-secret") && !journal.to_string().contains("parseUrl"), "{journal}");
}

#[test]
fn dialog_widgets_are_in_place_from_their_first_frame() {
    let mut ui = ui();
    let _reply = ui.send("ui.click", json!({ "id": "conn-fill-url" }));
    let start = Instant::now();
    // The frame that first draws the dialog is egui's sizing pass; a click on what `ui.inspect`
    // lists then must still land.
    let first = loop {
        ui.frame();
        let tags = ui.harness.state().tags();
        if let (Some(ok), Some(url)) = (tags.rect("dialog-ok"), tags.rect("dialog-url")) {
            break (ok, url);
        }
        assert!(start.elapsed() < WAIT, "the dialog never showed");
    };
    // egui draws everything disabled in that frame; the tags say what the widgets are.
    let json = ui.harness.state().tags().to_json();
    let entry = |id: &str| json.as_array().unwrap().iter().find(|e| e["id"] == id).cloned().unwrap();
    assert!(entry("dialog-cancel").get("enabled").is_none(), "{json}");
    assert_eq!(entry("dialog-ok")["enabled"], false, "no URL typed yet: {json}");
    ui.frame();
    let tags = ui.harness.state().tags();
    assert_eq!((tags.rect("dialog-ok").unwrap(), tags.rect("dialog-url").unwrap()), first);
}

#[test]
fn clicking_a_field_out_of_view_scrolls_to_it() {
    let mut ui = ui();
    // A short window: with the SSH tunnel on, the form's fields scroll.
    ui.harness.set_size(egui::vec2(1100.0, 560.0));
    ui.frame();
    ui.click("conn-ssh-on");
    let user = ui.tag("conn-ssh-user").unwrap();
    assert_eq!(user["visible"], false, "the window is short enough to hide it: {user}");
    ui.click("conn-ssh-user");
    ui.text("deploy");
    assert_eq!(ui.value("conn-ssh-user"), "deploy");
    assert!(ui.tag("conn-error").is_none(), "the click went nowhere else");
    assert_eq!(ui.document()["jobs"], json!([]));
}

#[test]
fn enter_in_a_field_connects() {
    let mut ui = ui();
    ui.replace("conn-host", "127.0.0.1");
    ui.replace("conn-port", "1");
    ui.ok("ui.key", json!({ "key": "Enter" }));
    ui.until("the inline error", |ui| ui.tag("conn-error").is_some());
    assert!(ui.value("conn-error").as_str().unwrap().contains("could not connect"));
}

#[test]
fn save_puts_the_card_in_the_list() {
    let mut ui = ui();
    assert!(!ids(&ui.inspect()).iter().any(|id| id.starts_with("conn-saved-")));
    ui.click("conn-name");
    ui.text("Shop live");
    ui.click("conn-color-red");
    assert_eq!(ui.value("conn-color"), "red");
    ui.click("conn-save");
    let id = ui.saved_id("Shop live");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"][0]["color"], "red", "{list}");
    assert_eq!(list["saved"][0]["host"], "localhost", "{list}");
    let card = ui.tag(&format!("conn-saved-{id}")).expect("the saved card is listed");
    assert_eq!(card["value"], "Shop live");
    // Saving again updates the same connection instead of adding one.
    ui.replace("conn-name", "Shop (live)");
    ui.click("conn-save");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"].as_array().unwrap().len(), 1, "{list}");
    assert_eq!(list["saved"][0]["id"], id.as_str());
    assert_eq!(ui.value(&format!("conn-saved-{id}")), "Shop (live)");
    // The filter hides cards that do not match.
    ui.click("conn-filter");
    ui.text("nothing like it");
    assert!(ui.tag(&format!("conn-saved-{id}")).is_none());
    ui.replace("conn-filter", "SHOP");
    assert!(ui.tag(&format!("conn-saved-{id}")).is_some());
    // New connection starts over with the engine cards, then an empty form.
    ui.click("conn-new");
    assert!(ui.tag("conn-name").is_none() && ui.tag("conn-kind-postgres").is_some());
    ui.click("conn-kind-postgres");
    assert_eq!(ui.value("conn-name"), "");
    assert_eq!(ui.value("conn-host"), "localhost");
    assert_eq!(ui.value("conn-port"), "5432");
}

#[test]
fn inline_error_on_refused_port() {
    let mut ui = ui();
    let before = ui.inspect();
    assert!(tag_in(&before, "conn-error").is_none());
    ui.replace("conn-host", "127.0.0.1");
    ui.replace("conn-port", "1");
    assert_eq!(ui.value("conn-port"), "1");
    ui.click("conn-connect");
    ui.until("the inline error", |ui| ui.tag("conn-error").is_some());
    let i = ui.inspect();
    let error = tag_in(&i, "conn-error").unwrap();
    assert!(error["value"].as_str().unwrap().contains("could not connect"), "{error}");
    let [_, ey, _, _] = rect(&error);
    let [_, cy, _, ch] = rect(&tag_in(&i, "conn-connect").unwrap());
    assert!(ey >= cy + ch && ey - (cy + ch) < 60.0, "the error shows right under the buttons: {error}");
    assert_eq!(i["screen"], "connections");
    // A port that is no number is caught before anything runs.
    ui.replace("conn-port", "54x");
    ui.click("conn-test");
    let error = ui.tag("conn-error").unwrap();
    assert!(error["value"].as_str().unwrap().contains("port"), "{error}");
    assert_eq!(ui.document()["jobs"], json!([]));
}

#[test]
fn host_key_dialog_then_connect() {
    let db = dbee_testkit::require_testdb!();
    // The test key as ~/.ssh/id_ed25519, and no known_hosts: the sshd is unknown.
    let mut ui = ui_in(temp_home(Some(&db.keys.plain), None), true);
    let card = ui.save(&via_ssh(db, SshAuth::Key { path: None }), Some(&db.password));
    ui.click(&card);
    assert_eq!(ui.value("conn-ssh-on"), true);
    assert_eq!(ui.value("conn-ssh-auth"), "key");
    ui.until("the key line", |ui| ui.value("conn-ssh-key").as_str().is_some_and(|t| t.starts_with("Uses ")));
    assert!(ui.value("conn-ssh-key").as_str().unwrap().ends_with("id_ed25519"));
    ui.click("conn-connect");
    ui.until("the host key dialog", |ui| ui.inspect()["dialog"] == "hostKey");
    let i = ui.inspect();
    let fingerprint = tag_in(&i, "dialog-fingerprint").unwrap()["value"].as_str().unwrap().to_string();
    assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
    assert!(tag_in(&i, "dialog-text").unwrap()["value"].as_str().unwrap().contains(&db.sshd.port.to_string()), "{i}");
    assert!(!ui.home.ssh_dir().join("known_hosts").exists(), "nothing is learned before the user trusts it");
    ui.click("dialog-ok");
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    let d = ui.document();
    assert_eq!(d["connection"]["ssh"], true, "{d}");
    let known = std::fs::read_to_string(ui.home.ssh_dir().join("known_hosts")).unwrap();
    assert!(known.contains(&format!("[127.0.0.1]:{}", db.sshd.port)), "{known}");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
}

#[test]
fn cancelling_the_host_key_dialog_connects_nowhere() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui_in(temp_home(Some(&db.keys.plain), None), true);
    let card = ui.save(&via_ssh(db, SshAuth::Key { path: None }), Some(&db.password));
    ui.double_click(&card);
    ui.until("the host key dialog", |ui| ui.inspect()["dialog"] == "hostKey");
    ui.click("dialog-cancel");
    let i = ui.inspect();
    assert_eq!((i["dialog"].clone(), i["screen"].clone()), (Json::Null, json!("connections")), "{i}");
    assert_eq!(ui.document()["jobs"], json!([]));
    assert!(!ui.home.ssh_dir().join("known_hosts").exists());
}

#[test]
fn passphrase_dialog_saves_in_the_keyring() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui_in(temp_home(Some(&db.keys.encrypted), Some(&db.sshd.known_hosts_line)), true);
    let card = ui.save(&via_ssh(db, SshAuth::Key { path: None }), Some(&db.password));
    let id = card.trim_start_matches("conn-saved-").to_string();
    ui.click(&card);
    ui.click("conn-connect");
    ui.until("the passphrase dialog", |ui| ui.inspect()["dialog"] == "passphrase");
    let save = ui.tag("dialog-save").expect("a saved connection offers to keep the passphrase");
    assert_eq!(save["value"], true, "{save}");
    // The secret field has focus: typing goes there.
    ui.text(&db.keys.passphrase);
    let i = ui.inspect();
    let secret = tag_in(&i, "dialog-secret").unwrap();
    assert_eq!((secret["masked"].clone(), secret["set"].clone()), (json!(true), json!(true)), "{secret}");
    assert!(!i.to_string().contains(&db.keys.passphrase), "{i}");
    ui.click("dialog-ok");
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    assert_eq!(stored(&ui, &id, SecretKind::SshPassphrase).as_deref(), Some(db.keys.passphrase.as_str()));
    let journal = ui.ok("engine.journal", json!({}));
    assert!(!journal.to_string().contains(&db.keys.passphrase), "{journal}");
}

#[test]
fn single_click_loads_and_double_click_connects() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    let config = pg14(db);
    let card = ui.save(&config, Some(&db.password));
    ui.click(&card);
    let i = ui.inspect();
    assert_eq!(tag_in(&i, "conn-name").unwrap()["value"], "pg14");
    assert_eq!(tag_in(&i, "conn-host").unwrap()["value"], "127.0.0.1");
    assert_eq!(tag_in(&i, "conn-port").unwrap()["value"], config.port.to_string());
    // The stored password is not loaded into the form.
    assert_eq!(tag_in(&i, "conn-password").unwrap()["set"], false);
    ui.double_click(&card);
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    // Back on the connection screen, its saved row shows, and it isn't listed twice.
    ui.execute("connection.close", json!({}));
    let i = ui.inspect();
    assert_eq!(tag_in(&i, &card).expect("the saved row")["value"], "pg14");
    assert!(!ids(&i).iter().any(|id| id.starts_with("conn-recent-")), "{}", ids(&i).join(" "));
}

/// The design: a click on the scrim does nothing for a dialog. It stays open, and what is under the
/// scrim gets no click: a saved card there is neither chosen nor, on a double click, opened.
/// Escape still cancels.
#[test]
fn a_click_on_the_scrim_does_nothing() {
    let mut ui = ui();
    let card = ui.save(&silent_config(1), None);
    let at = ui.centre(&card);
    ui.right_click(&card);
    ui.click("conn-menu-delete");
    assert_eq!(ui.inspect()["dialog"], "deleteConnection");
    let before = ui.document();
    for count in [1, 2] {
        ui.ok("ui.click", json!({ "x": at.x, "y": at.y, "count": count }));
        ui.frame();
        let i = ui.inspect();
        assert_eq!(i["dialog"], "deleteConnection", "a click ({count}) on the scrim leaves the dialog open: {i}");
    }
    assert_eq!(ui.document(), before, "nothing under the scrim was clicked");
    assert_eq!(ui.execute("connection.list", json!({}))["saved"].as_array().map(Vec::len), Some(1), "nothing deleted");
    ui.key("Escape");
    assert_eq!(ui.inspect()["dialog"], Json::Null, "Escape cancels");
    assert_eq!(ui.execute("connection.list", json!({}))["saved"].as_array().map(Vec::len), Some(1), "nothing deleted");
}

#[test]
fn delete_asks_first() {
    let mut ui = ui();
    let card = ui.save(&silent_config(1), None);
    ui.right_click(&card);
    ui.click("conn-menu-delete");
    let i = ui.inspect();
    assert_eq!(i["dialog"], "deleteConnection", "{i}");
    assert!(tag_in(&i, "dialog-text").unwrap()["value"].as_str().unwrap().contains("silent"), "{i}");
    ui.click("dialog-cancel");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
    assert_eq!(ui.execute("connection.list", json!({}))["saved"].as_array().unwrap().len(), 1);
    ui.right_click(&card);
    ui.click("conn-menu-delete");
    ui.click("dialog-ok");
    assert_eq!(ui.execute("connection.list", json!({}))["saved"], json!([]));
    assert!(ui.tag(&card).is_none());
    // Duplicate and Edit from the same menu.
    let card = ui.save(&silent_config(1), None);
    ui.right_click(&card);
    ui.click("conn-menu-duplicate");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"].as_array().unwrap().len(), 2, "{list}");
    assert_eq!(ui.value("conn-name"), "silent (copy)");
    ui.right_click(&card);
    ui.click("conn-menu-edit");
    assert_eq!(ui.value("conn-name"), "silent");
}

#[test]
fn test_and_connect_run_once_and_show_it() {
    let silent = Silent::start();
    let mut ui = ui();
    let card = ui.save(&silent_config(silent.port), None);
    ui.click(&card);
    ui.click("conn-test");
    ui.until("the running test", |ui| ui.value("conn-test") == "Testing…");
    let i = ui.inspect();
    assert_eq!(tag_in(&i, "conn-test").unwrap()["enabled"], false, "{i}");
    assert_eq!(tag_in(&i, "conn-connect").unwrap()["enabled"], false, "{i}");
    // Clicking again starts nothing; double-clicking the card starts no connect.
    ui.click("conn-test");
    ui.click("conn-connect");
    ui.double_click(&card);
    let d = ui.document();
    assert_eq!(d["jobs"].as_array().unwrap().len(), 1, "{d}");
    assert_eq!(d["jobs"][0]["label"], "connection.test", "{d}");
    assert!(ui.tag("notice").is_none(), "nothing to complain about");
    drop(silent);
    ui.until("the test to fail", |ui| ui.tag("conn-error").is_some());
    assert_eq!(ui.value("conn-test"), "Test");
    assert_eq!(ui.document()["jobs"], json!([]));
}

/// While Read-only is ticked, the form says what it guards against and what is the lock.
#[test]
fn read_only_says_what_it_guards_against() {
    for kind in ["postgres", "sqlite"] {
        let mut ui = cards();
        ui.click(&format!("conn-kind-{kind}"));
        assert!(ui.tag("conn-readonly-note").is_none(), "{kind}");
        ui.click("conn-readonly");
        let note = ui.tag("conn-readonly-note").unwrap_or_else(|| panic!("{kind}: the note shows while Read-only is ticked"));
        let text = note["value"].as_str().unwrap_or_default();
        assert!(text.contains("mistakes") && text.contains("without write rights"), "{kind}: {note}");
        assert!(note.get("visible").is_none(), "{kind}: the note is in view: {note}");
        ui.click("conn-readonly");
        assert!(ui.tag("conn-readonly-note").is_none(), "{kind}");
    }
}

/// On SSL mode's label row the form says what the chosen mode checks: prefer and require encrypt
/// without checking who answers, verify-ca and verify-full check the certificate.
#[test]
fn the_ssl_mode_says_what_it_checks() {
    let mut ui = cards();
    ui.click("conn-kind-postgres");
    let modes = [
        ("disable", "No encryption"),
        ("prefer", "does not check who answers"),
        ("require", "does not check who answers; with a CA file, checks the certificate against it, but not the server's name"),
        ("verify-ca", "checks the certificate against the CA file"),
        ("verify-full", "checks the server's certificate and its name"),
    ];
    for (mode, says) in modes {
        ui.click(&format!("conn-ssl-{mode}"));
        let note = ui.tag("conn-ssl-note").unwrap_or_else(|| panic!("{mode}: the note shows beside the label"));
        assert!(note["value"].as_str().unwrap_or_default().contains(says), "{mode}: {note}");
        assert!(note.get("visible").is_none(), "{mode}: the note is in view: {note}");
    }
    // A file has no SSL mode.
    let mut ui = cards();
    ui.click("conn-kind-sqlite");
    assert!(ui.tag("conn-ssl-note").is_none());
}

#[test]
fn no_secret_store_turns_saving_off() {
    let mut without = ui_in(temp_home(None, None), false);
    without.click("conn-kind-postgres");
    let i = without.inspect();
    let save = tag_in(&i, "conn-save-secrets").unwrap();
    assert_eq!((save["value"].clone(), save["enabled"].clone()), (json!(false), json!(false)), "{save}");
    let note = tag_in(&i, "conn-no-keyring").expect("the note says passwords are asked each time");
    assert!(note["value"].as_str().unwrap().contains("each time"), "{note}");
    // With a store, saving is on for a new connection.
    let mut ui = ui();
    let save = ui.tag("conn-save-secrets").unwrap();
    assert_eq!(save["value"], true, "{save}");
    assert!(save.get("enabled").is_none(), "{save}");
    assert!(ui.tag("conn-no-keyring").is_none());
}

#[test]
fn store_warning_shows_while_set() {
    let home = temp_home(None, None);
    std::fs::create_dir_all(home.home.join("config")).unwrap();
    std::fs::write(home.home.join("config").join("connections.json"), "{ not json").unwrap();
    let mut ui = ui_in(home, true);
    assert!(ui.document()["storeWarning"].is_string());
    let warning = ui.tag("conn-store-warning").expect("the warning line");
    assert!(warning["value"].as_str().unwrap().contains("damaged"), "{warning}");
    ui.click("conn-new");
    assert!(ui.tag("conn-store-warning").is_some(), "it stays while the warning is set");
    // Its own area under the filter, across the sidebar, its words 12 in.
    let filter = rect_of(&ui.tag("conn-filter").unwrap());
    let w = rect_of(&ui.tag("conn-store-warning").unwrap());
    assert!(w.top() >= filter.bottom() + 8.0 - 0.5 && (w.left() - filter.left() - 12.0).abs() <= 0.5, "under the filter, 12 in: {w:?} {filter:?}");
}

/// The connection screen takes the workspace's frame: the menu bar on top, a 260 sidebar whose
/// first row is `+ NEW CONNECTION` (a bar button in a 36 row from the sidebar's left edge), the
/// filter under it, and the version bar along the window's bottom (gap 62).
#[test]
fn the_connection_screen_has_the_workspace_frame() {
    let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
    // The window as the app gets it: inside the harness's own 8 round it.
    let window = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 720.0)).shrink(KITTEST_MARGIN);
    let i = ui.inspect();
    for id in ["menu-File", "menu-View"] {
        let r = rect_of(&tag_in(&i, id).unwrap_or_else(|| panic!("no {id}: {}", ids(&i).join(" "))));
        assert!(r.top() < window.top() + 30.0 && r.bottom() <= window.top() + 30.5, "{id} in the menu bar: {r:?}");
    }
    let version = tag_in(&i, "conn-version").expect("the version bar");
    assert!(version["value"].as_str().unwrap().starts_with("dbee "), "{version}");
    let v = rect_of(&version);
    let along = (v.bottom() - window.bottom()).abs() <= 0.5 && (v.height() - 30.0).abs() <= 0.5 && (v.left() - window.left()).abs() <= 0.5;
    assert!(along, "the version bar along the bottom: {v:?}");
    let new = rect_of(&tag_in(&i, "conn-new").unwrap());
    let row = (new.left() - window.left()).abs() <= 0.5 && (new.height() - 36.0).abs() <= 0.5 && new.width() <= 260.0;
    assert!(row, "`+ NEW CONNECTION`: {new:?}");
    assert!((new.top() - window.top() - 30.0).abs() <= 0.5, "right under the menu bar: {new:?}");
    let filter = rect_of(&tag_in(&i, "conn-filter").unwrap());
    assert!(filter.top() >= new.bottom() - 0.5 && filter.right() <= window.left() + 260.5, "the filter under it, in the sidebar: {filter:?}");
    // The words in caps, the version as written in the tag.
    assert!(ui.text_in(v, &format!("DBEE {}", env!("CARGO_PKG_VERSION"))).is_some(), "`DBEE x.y.z` drawn in the bar");
    // No boxed button: nothing stroked around `+ NEW CONNECTION`.
    assert!(edges_on(&ui, new).is_empty(), "a bar button, not a boxed one");
}

/// Gap 62: View > Theme works before connecting.
#[test]
fn view_theme_works_before_connecting() {
    let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
    ui.click("menu-View");
    ui.frame();
    ui.click("menu-View-Theme");
    ui.frame();
    ui.frame();
    ui.click("menu-ui.themeDark");
    ui.frame();
    assert_eq!(ui.inspect()["theme"]["choice"], "dark");
    assert_eq!(ui.document()["screen"], "connections");
}

/// The room egui_kittest leaves round what the app draws.
const KITTEST_MARGIN: f32 = 8.0;

/// Writes a connections file to `home` with `saved` and `recent` (newest first).
fn write_store(home: &TempHome, saved: &[&ConnectionConfig], recent: &[&ConnectionConfig]) {
    let dir = home.home.join("config");
    std::fs::create_dir_all(&dir).unwrap();
    let recent: Vec<Json> = recent.iter().enumerate().map(|(n, c)| json!({ "config": c, "usedAt": 1_700_000_000 - n as u64 * 60 })).collect();
    let file = json!({ "version": 1, "saved": saved, "recent": recent });
    std::fs::write(dir.join("connections.json"), serde_json::to_vec_pretty(&file).unwrap()).unwrap();
}

/// Review Focus 3: the saved rows, then each recent entry that isn't saved, newest first, in one
/// list with no headings and no time words; a saved entry used lately isn't listed twice.
#[test]
fn recent_connections_stay_in_the_one_list() {
    let with = |id: &str, name: &str| ConnectionConfig { id: id.into(), ..server_of(DbKind::Postgres, name) };
    let (a, b, c, d) = (with("A", "Alpha"), with("B", "Beta"), with("C", "Gamma"), with("D", "Delta"));
    let home = temp_home(None, None);
    write_store(&home, &[&a, &b], &[&c, &a, &d]);
    let mut ui = ui_in(home, true);
    let i = ui.inspect();
    let mut rows: Vec<(f32, String)> = ids(&i)
        .into_iter()
        .filter(|id| (id.starts_with("conn-saved-") || id.starts_with("conn-recent-")) && !id.ends_with("-kind"))
        .map(|id| (rect_of(&tag_in(&i, &id).unwrap()).top(), id))
        .collect();
    rows.sort_by(|x, y| x.0.total_cmp(&y.0));
    let order: Vec<&str> = rows.iter().map(|(_, id)| id.as_str()).collect();
    assert_eq!(order, ["conn-saved-A", "conn-saved-B", "conn-recent-0", "conn-recent-2"], "{rows:?}");
    assert_eq!(tag_in(&i, "conn-recent-0").unwrap()["value"], "Gamma");
    assert_eq!(tag_in(&i, "conn-recent-2").unwrap()["value"], "Delta");
    let words: Vec<String> = ui.texts().into_iter().map(|(_, g)| g.text().to_owned()).collect();
    for gone in ["SAVED", "RECENT", "2"] {
        assert!(!words.iter().any(|w| w == gone), "`{gone}` painted: {words:?}");
    }
    assert!(!words.iter().any(|w| w.contains(" ago") || w.contains("just now")), "no time words: {words:?}");
}

/// E2: on first start the list is empty, with no phrase, beside the cards.
#[test]
fn first_start_shows_an_empty_list_and_the_cards() {
    let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
    let i = ui.inspect();
    assert!(!ids(&i).iter().any(|id| id.starts_with("conn-saved-") || id.starts_with("conn-recent-")), "{}", ids(&i).join(" "));
    assert!(tag_in(&i, "conn-kind-postgres").is_some());
    let words: Vec<String> = ui.texts().into_iter().map(|(_, g)| g.text().to_owned()).collect();
    for gone in ["No saved connections yet.", "Connections you open show here.", "No connection matches."] {
        assert!(!words.iter().any(|w| w == gone), "`{gone}` painted: {words:?}");
    }
}

/// A filter that hides every row says so once, in `body` `text_muted`.
#[test]
fn a_filter_that_matches_nothing_says_so() {
    let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
    ui.save(&server_of(DbKind::Postgres, "Shop"), None);
    ui.click("conn-filter");
    ui.text("zzz");
    ui.frame();
    let side = egui::Rect::from_min_max(
        egui::pos2(KITTEST_MARGIN, KITTEST_MARGIN + 30.0),
        egui::pos2(KITTEST_MARGIN + 260.0, 720.0 - KITTEST_MARGIN - 30.0),
    );
    let t = dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    let (colour, font) = drawn_as(&ui, side, "No connection matches.").expect("the words in the sidebar");
    assert_eq!((colour, font), (t.text_muted, dbee_ui_egui::theme::Text::Body.font()));
    assert!(drawn_as(&ui, side, "No saved connection matches.").is_none() && drawn_as(&ui, side, "No recent connection matches.").is_none());
}

/// The row's menu: Connect, Edit, Duplicate, a line, Delete; Connect off while a test runs, its
/// hover saying why.
#[test]
fn the_row_menu_says_why_connect_is_off() {
    let silent = Silent::start();
    let mut ui = Ui::fresh(egui::vec2(1100.0, 720.0));
    let card = ui.save(&silent_config(silent.port), None);
    ui.click(&card);
    ui.click("conn-test");
    ui.frame();
    ui.right_click(&card);
    ui.frame();
    ui.frame();
    let i = ui.inspect();
    let connect = tag_in(&i, "conn-menu-connect").expect("the menu");
    assert_eq!(connect["enabled"], false, "{connect}");
    let duplicate = rect_of(&tag_in(&i, "conn-menu-duplicate").unwrap());
    let delete = rect_of(&tag_in(&i, "conn-menu-delete").unwrap());
    let t = dbee_ui_egui::theme::tokens(&ui.harness.ctx);
    let line = ui.shapes().into_iter().any(|(_, s)| match s {
        egui::Shape::LineSegment { points, stroke } => {
            stroke.color == t.raised_line && points[0].y > duplicate.bottom() - 0.5 && points[0].y < delete.top() + 0.5
        }
        _ => false,
    });
    assert!(line, "a separator between Duplicate and Delete");
    let at = ui.centre("conn-menu-connect");
    ui.rest(at);
    let words: Vec<String> = ui.texts().into_iter().map(|(_, g)| g.text().to_owned()).collect();
    assert!(words.iter().any(|w| w == "Connect: not now, a test or connect is running"), "the reason on hover: {words:?}");
    ui.key("Escape");
    ui.execute("connection.cancel", json!({}));
}

#[test]
fn typing_and_errors_do_not_move_the_layout() {
    let mut ui = ui();
    ui.click("conn-ssh-on");
    // Every widget of the form and the side bar, but the message under the buttons, the cards and
    // the key file buttons, which only the Key login shows (in its fixed slot).
    let fixed = |i: &Json| -> Vec<(String, [f64; 4])> {
        let varies = ["conn-error", "conn-result", "conn-ssh-change", "conn-ssh-auto"];
        ids(i)
            .into_iter()
            .filter(|id| {
                id.starts_with("conn-") && !varies.contains(&id.as_str()) && !id.starts_with("conn-saved-") && !id.starts_with("conn-recent-")
            })
            .map(|id| {
                let r = rect(&tag_in(i, &id).unwrap());
                (id, r)
            })
            .collect()
    };
    let before = fixed(&ui.inspect());
    assert!(before.iter().any(|(id, _)| id == "conn-ssh-host") && before.iter().any(|(id, _)| id == "conn-ssh-key"), "{before:?}");
    ui.click("conn-name");
    ui.text("A very long connection name that goes on and on and on to see whether the title grows");
    ui.click("conn-ssh-host");
    ui.text("bastion.example.com");
    ui.click("conn-ssh-user");
    ui.text("deploy");
    ui.replace("conn-host", "127.0.0.1");
    ui.replace("conn-port", "1");
    for auth in ["agent", "password", "key"] {
        ui.click(&format!("conn-ssh-auth-{auth}"));
        assert_eq!(ui.value("conn-ssh-auth"), auth);
        assert_eq!(fixed(&ui.inspect()), before, "switching the login to {auth} moved the form");
    }
    // An inline error takes its reserved place: nothing above or beside it moves.
    ui.click("conn-ssh-on");
    let before = fixed(&ui.inspect());
    ui.click("conn-connect");
    ui.until("the inline error", |ui| ui.tag("conn-error").is_some());
    assert_eq!(fixed(&ui.inspect()), before, "the error moved the form");
}

#[test]
fn confirm_dialog_closes_anyway() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    ui.execute("connection.open", json!({ "config": pg14(db), "password": db.password }));
    ui.execute("tab.newQuery", json!({ "text": "select 1" }));
    ui.ok("ui.key", json!({ "key": "Q", "cmd": true, "shift": true }));
    let i = ui.inspect();
    assert_eq!(i["dialog"], "confirm", "{i}");
    assert_eq!(tag_in(&i, "dialog-ok").unwrap()["value"], "Close anyway");
    ui.click("dialog-cancel");
    assert_eq!(ui.document()["screen"], "workspace");
    ui.ok("ui.key", json!({ "key": "Q", "cmd": true, "shift": true }));
    ui.click("dialog-ok");
    let d = ui.document();
    assert_eq!((d["screen"].clone(), d["tabs"].clone()), (json!("connections"), json!([])), "{d}");
}

#[test]
fn password_dialog_for_an_unsaved_connection_keeps_nothing() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    let pg = pg14(db);
    ui.replace("conn-host", &pg.host);
    ui.replace("conn-port", &pg.port.to_string());
    ui.replace("conn-user", &pg.user);
    ui.replace("conn-database", &pg.database);
    ui.click("conn-connect");
    ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
    // Not saved: there is nothing to keep it for.
    let save = ui.tag("dialog-save").unwrap();
    assert_eq!((save["value"].clone(), save["enabled"].clone()), (json!(false), json!(false)), "{save}");
    assert_eq!(ui.tag("dialog-ok").unwrap()["enabled"], false, "no password typed yet");
    ui.text(&db.password);
    ui.click("dialog-ok");
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"], json!([]), "{list}");
    let id = list["recent"][0]["config"]["id"].as_str().unwrap().to_string();
    let store = ui.secrets.clone().unwrap();
    assert_eq!(store.get(&id, SecretKind::DbPassword).unwrap(), None);
    let everything = format!("{} {} {}", ui.inspect(), ui.document(), ui.ok("engine.journal", json!({})));
    assert!(!everything.contains(&db.password), "{everything}");
}

#[test]
fn save_in_keyring_follows_the_form() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    // Saved without a password, with "Save passwords" on.
    let card = ui.save(&pg14(db), None);
    ui.click(&card);
    // Turned off in the form (not saved): the dialog's "Save in keyring" starts off.
    ui.click("conn-save-secrets");
    assert_eq!(ui.value("conn-save-secrets"), false);
    ui.click("conn-connect");
    ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
    let save = ui.tag("dialog-save").unwrap();
    assert_eq!(save["value"], false, "{save}");
    assert!(save.get("enabled").is_none(), "the connection is saved, so it can be kept: {save}");
    ui.click("dialog-cancel");
    // On again: it starts on.
    ui.click("conn-save-secrets");
    ui.click("conn-connect");
    ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
    assert_eq!(ui.value("dialog-save"), true);
}

#[test]
fn save_passwords_off_keeps_no_password() {
    let mut ui = ui();
    ui.click("conn-name");
    ui.text("no secrets");
    ui.click("conn-password");
    ui.text("typed-secret-2");
    ui.click("conn-save-secrets");
    assert_eq!(ui.value("conn-save-secrets"), false);
    ui.click("conn-save");
    let id = ui.saved_id("no secrets");
    let list = ui.execute("connection.list", json!({}));
    assert_eq!(list["saved"][0]["saveSecrets"], false, "{list}");
    assert_eq!(ui.secrets.clone().unwrap().get(&id, SecretKind::DbPassword).unwrap(), None);
    assert_eq!(ui.value("conn-result"), "Saved.");
}

#[test]
fn typed_password_stays_out_of_inspect_and_journal() {
    let mut ui = ui();
    ui.click("conn-password");
    ui.text("typed-secret-1");
    let i = ui.inspect();
    let password = tag_in(&i, "conn-password").unwrap();
    assert_eq!((password["masked"].clone(), password["set"].clone()), (json!(true), json!(true)), "{password}");
    assert!(password.get("value").is_none(), "{password}");
    // Showing the password on screen still keeps it out of inspect.
    ui.click("conn-password-show");
    assert!(!ui.inspect().to_string().contains("typed-secret-1"));
    ui.click("conn-name");
    ui.text("pw test");
    ui.click("conn-save");
    let id = ui.saved_id("pw test");
    assert_eq!(stored(&ui, &id, SecretKind::DbPassword).as_deref(), Some("typed-secret-1"));
    let everything = format!("{} {} {}", ui.inspect(), ui.document(), ui.ok("engine.journal", json!({})));
    assert!(!everything.contains("typed-secret-1"), "{everything}");
    let file = std::fs::read_to_string(ui.home.home.join("config").join("connections.json")).unwrap();
    assert!(!file.contains("typed-secret-1"), "{file}");
}

#[test]
fn a_drop_while_a_dialog_shows_or_a_connect_runs_is_let_go() {
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let file = sqlite.file().unwrap();
    let mut ui = cards();
    let jobs = |ui: &mut Ui| ui.document()["jobs"].as_array().map_or(0, Vec::len);
    // A dialog asks something: the file opens neither behind it nor once it closes.
    ui.click("conn-fill-url");
    assert_eq!(ui.inspect()["dialog"], "fillFromUrl");
    drop_files(&mut ui, &[file]);
    ui.frame();
    assert_eq!((jobs(&mut ui), ui.inspect()["dialog"].clone()), (0, json!("fillFromUrl")));
    ui.click("dialog-cancel");
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!((ui.document()["screen"].clone(), jobs(&mut ui)), (json!("connections"), 0));
    // A connect runs: no second open, and no busy notice either.
    let silent = Silent::start();
    let params = json!({ "config": silent_config(silent.port) });
    ui.ok("engine.execute", json!({ "command": "connection.open", "params": params, "wait": false }));
    ui.frame();
    assert_eq!(ui.document()["jobs"][0]["label"], "connection.open");
    drop_files(&mut ui, &[file]);
    ui.frame();
    assert!(ui.tag("notice").is_none(), "{:?}", ui.tag("notice"));
    assert_eq!(jobs(&mut ui), 1);
    drop(silent);
    ui.until("the silent connect to end", |ui| ui.document()["jobs"] == json!([]));
    for _ in 0..3 {
        ui.frame();
    }
    assert_eq!(ui.document()["screen"], "connections", "the file was not kept for later");
}

// ---- the database list -----------------------------------------------------------------------

/// A new form of `server`'s type at the app's first window size, its host, port, user and
/// password typed in, the Database field left empty.
fn form_for(db: &TestDb, server: &dbee_testkit::Server) -> Ui {
    let info = server.kind.info();
    let mut ui = Ui::fresh(APP_SIZE);
    ui.click(&format!("conn-kind-{}", info.id));
    ui.replace("conn-host", "127.0.0.1");
    ui.replace("conn-port", &server.port.to_string());
    ui.replace("conn-user", info.user);
    ui.click("conn-password");
    ui.text(&db.password);
    ui
}

/// The database list's items, in order: (automation id, name).
fn listed(i: &Json) -> Vec<(String, String)> {
    ids(i)
        .into_iter()
        .filter(|id| id.starts_with("conn-database-item-"))
        .map(|id| {
            let name = tag_in(i, &id).unwrap()["value"].as_str().unwrap().to_string();
            (id, name)
        })
        .collect()
}

/// The rects of every widget of the form and the side bar but the list's items and the message
/// under the buttons.
fn form_rects(i: &Json) -> Vec<(String, [f64; 4])> {
    let varies = ["conn-error", "conn-result", "conn-note"];
    ids(i)
        .into_iter()
        .filter(|id| id.starts_with("conn-") && !varies.contains(&id.as_str()) && !id.starts_with("conn-database-item-"))
        .map(|id| {
            let r = rect(&tag_in(i, &id).unwrap());
            (id, r)
        })
        .collect()
}

/// A saved connection with a stored password, another database in its form, not saved: List… and
/// Connect both use the stored password and ask for nothing.
#[test]
fn another_database_on_a_saved_connection_asks_for_no_password() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    let saved = ConnectionConfig { database: "dbee".into(), ..pg14(db) };
    let card = ui.save(&saved, Some(&db.password));
    ui.click(&card);
    ui.replace("conn-database", "postgres");
    ui.click("conn-database-list");
    ui.until("the list", |ui| ui.inspect()["dialog"] != Json::Null || !listed(&ui.inspect()).is_empty());
    assert_eq!(ui.inspect()["dialog"], Json::Null, "List… asked for a secret");
    let (item, _) = listed(&ui.inspect()).into_iter().find(|(_, n)| n == "postgres").unwrap();
    ui.click(&item);
    ui.click("conn-connect");
    ui.until("the workspace", |ui| ui.inspect()["dialog"] != Json::Null || ui.document()["screen"] == "workspace");
    assert_eq!(ui.inspect()["dialog"], Json::Null, "Connect asked for a secret");
    assert_eq!(ui.document()["connection"]["database"], "postgres");
}

#[test]
fn database_list_fills_the_field() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = form_for(db, pg14_server(db));
    assert_eq!(ui.value("conn-database"), "");
    assert_eq!(ui.value("conn-database-list"), "List…");
    ui.click("conn-database-list");
    ui.until("the list", |ui| !listed(&ui.inspect()).is_empty());
    ui.frame();
    let i = ui.inspect();
    let names: Vec<String> = listed(&i).into_iter().map(|(_, n)| n).collect();
    assert!(names.contains(&"dbee".to_string()) && names.contains(&"postgres".to_string()), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("template")), "{names:?}");
    assert_eq!(ui.value("conn-database-list"), "List…", "it no longer lists");
    let (item, _) = listed(&i).into_iter().find(|(_, n)| n == "dbee").unwrap();
    ui.click(&item);
    assert_eq!(ui.value("conn-database"), "dbee");
    assert!(listed(&ui.inspect()).is_empty(), "the list closed");
    // Typing works as before.
    ui.click("conn-database");
    ui.text("_x");
    assert_eq!(ui.value("conn-database"), "dbee_x");
    // The list ran as the form's job and left nothing behind: no journal entry, no connection.
    let journal = ui.ok("engine.journal", json!({}));
    assert!(journal.as_array().unwrap().iter().all(|e| e["command"] != "connection.listDatabases"), "{journal}");
    assert_eq!((ui.document()["screen"].clone(), ui.document()["jobs"].clone()), (json!("connections"), json!([])));
}

#[test]
fn sqlite_form_has_no_database_list() {
    let mut ui = Ui::fresh(APP_SIZE);
    ui.click("conn-kind-sqlite");
    assert!(ui.tag("conn-database-list").is_none() && ui.tag("conn-database").is_none());
    // Every server type has one, beside its Database field.
    for kind in DbKind::ALL.into_iter().filter(|k| matches!(k.info().settings, Settings::Server { .. })) {
        ui.click("conn-change-engine");
        ui.click(&format!("conn-kind-{}", kind.id()));
        let i = ui.inspect();
        let (field, button) = (rect(&tag_in(&i, "conn-database").unwrap()), rect(&tag_in(&i, "conn-database-list").unwrap()));
        assert!(field[0] + field[2] <= button[0] && (field[1] - button[1]).abs() < 0.5 && field[3] == button[3], "{kind:?}: {field:?} {button:?}");
    }
}

/// A MySQL list at the app's first window: under the field, the server's own databases last, and
/// the form stays where it was. Escape or a click elsewhere closes it.
#[test]
fn a_mysql_list_opens_under_the_field_without_moving_the_form() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = form_for(db, db.server("mysql80").unwrap());
    let before = form_rects(&ui.inspect());
    ui.click("conn-database-list");
    ui.until("the list", |ui| !listed(&ui.inspect()).is_empty());
    ui.frame();
    let i = ui.inspect();
    let names: Vec<String> = listed(&i).into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["many", "odd", "scratch", "shop", "information_schema", "mysql", "performance_schema", "sys"]);
    let field = rect(&tag_in(&i, "conn-database").unwrap());
    let first = rect(&tag_in(&i, "conn-database-item-0").unwrap());
    assert!(first[1] >= field[1] + field[3] && first[1] - (field[1] + field[3]) < 12.0, "under the field: {field:?} {first:?}");
    assert!(first[0] >= field[0] - 1.0 && first[0] < field[0] + 12.0, "from its left edge: {field:?} {first:?}");
    assert_eq!(form_rects(&i), before, "the list moved the form");
    // The button closes an open list rather than listing again.
    ui.click("conn-database-list");
    assert!(listed(&ui.inspect()).is_empty() && ui.document()["jobs"] == json!([]), "the button closes it");
    ui.click("conn-database-list");
    ui.until("the list again", |ui| !listed(&ui.inspect()).is_empty());
    ui.key("Escape");
    ui.frame();
    assert!(listed(&ui.inspect()).is_empty(), "Escape closes it");
    ui.click("conn-database-list");
    ui.until("the list again", |ui| !listed(&ui.inspect()).is_empty());
    ui.click("conn-name");
    ui.frame();
    assert!(listed(&ui.inspect()).is_empty(), "a click elsewhere closes it");
    assert_eq!(ui.value("conn-database"), "", "nothing was chosen");
}

/// While it lists, the button says so and waits; so it does while a test runs. Its error goes
/// under the buttons, as Test's does.
#[test]
fn the_database_list_button_shows_that_it_lists() {
    let silent = Silent::start();
    let mut ui = Ui::fresh(APP_SIZE);
    ui.click("conn-kind-postgres");
    ui.replace("conn-host", "127.0.0.1");
    ui.replace("conn-port", &silent.port.to_string());
    ui.click("conn-database-list");
    ui.until("the listing", |ui| ui.value("conn-database-list") == "Listing…");
    assert_eq!(ui.tag("conn-database-list").unwrap()["enabled"], false);
    // A second click starts nothing; Test still can.
    ui.click("conn-database-list");
    assert!(ui.tag("conn-test").unwrap().get("enabled").is_none());
    let d = ui.document();
    assert_eq!(d["jobs"].as_array().unwrap().len(), 1, "{d}");
    assert_eq!(d["jobs"][0]["label"], "connection.listDatabases", "{d}");
    drop(silent);
    ui.until("the listing to fail", |ui| ui.tag("conn-error").is_some());
    assert_eq!(ui.value("conn-database-list"), "List…");
    assert!(ui.tag("conn-database-list").unwrap().get("enabled").is_none());
    // A test that runs disables it.
    let silent = Silent::start();
    ui.replace("conn-port", &silent.port.to_string());
    ui.click("conn-test");
    ui.until("the running test", |ui| ui.value("conn-test") == "Testing…");
    assert_eq!(ui.tag("conn-database-list").unwrap()["enabled"], false);
    drop(silent);
    ui.until("the test to fail", |ui| ui.value("conn-test") == "Test");
    assert!(ui.tag("conn-database-list").unwrap().get("enabled").is_none());
}

/// The open list takes Down, Up, Enter and Escape before the form's fields, as the quick search
/// does: Enter picks the highlighted name rather than connecting.
#[test]
fn the_database_list_takes_the_keyboard() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = form_for(db, db.server("mysql80").unwrap());
    ui.click("conn-database-list");
    ui.until("the list", |ui| !listed(&ui.inspect()).is_empty());
    for key in ["ArrowDown", "ArrowDown", "ArrowUp"] {
        ui.key(key);
    }
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.value("conn-database"), "odd");
    assert!(listed(&ui.inspect()).is_empty(), "the list closed");
    assert_eq!((ui.document()["screen"].clone(), ui.document()["jobs"].clone()), (json!("connections"), json!([])), "Enter connected nowhere");
    // The highlight starts on the name the field holds.
    ui.replace("conn-database", "shop");
    ui.click("conn-database-list");
    ui.until("the list again", |ui| !listed(&ui.inspect()).is_empty());
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.value("conn-database"), "shop");
}

/// The keyboard leaving the list closes it, so Enter does what it does where the keyboard went.
/// The list opens with the keyboard in its field; Tab goes to its button, then past the list.
#[test]
fn tab_takes_the_keyboard_out_of_the_list() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = form_for(db, db.server("mysql80").unwrap());
    let before = ui.value("conn-ssl");
    ui.click("conn-database-list");
    ui.until("the list", |ui| !listed(&ui.inspect()).is_empty());
    ui.key("Tab");
    ui.frame();
    assert!(!listed(&ui.inspect()).is_empty(), "on its button the list stays");
    ui.key("Tab");
    ui.key("Enter");
    ui.frame();
    assert!(listed(&ui.inspect()).is_empty(), "the list closed");
    assert_eq!(ui.value("conn-database"), "", "nothing was picked");
    assert_ne!(ui.value("conn-ssl"), before, "Enter pressed the first SSL choice, where Tab went");
    assert_eq!((ui.document()["screen"].clone(), ui.document()["jobs"].clone()), (json!("connections"), json!([])));
}

/// Typing in the field closes the list, and Enter connects with the text typed.
#[test]
fn typing_in_the_field_closes_the_list_and_enter_connects_with_the_text() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = form_for(db, db.server("mysql80").unwrap());
    ui.click("conn-database-list");
    ui.until("the list", |ui| !listed(&ui.inspect()).is_empty());
    ui.text("shop");
    ui.frame();
    assert!(listed(&ui.inspect()).is_empty(), "typing closed it");
    assert_eq!(ui.value("conn-database"), "shop");
    ui.key("Enter");
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    let journal = ui.ok("engine.journal", json!({}));
    let open = journal.as_array().unwrap().iter().find(|e| e["command"] == "connection.open").cloned().unwrap();
    assert_eq!(open["params"]["config"]["database"], "shop", "{open}");
}

/// The form's and the URL dialog's buttons, moved to `components::button`, say in `ui.inspect`
/// what they said before: ids, values, enabled flags.
#[test]
fn moved_buttons_keep_their_ids_values_and_flags() {
    let mut ui = ui();
    ui.frame();
    let form: [Moved<'_>; 6] = [
        ("conn-password-show", Some(json!(false)), true, "SHOW"),
        ("conn-ca-choose", None, true, "CHOOSE…"),
        ("conn-ca-clear", None, false, "CLEAR"),
        ("conn-save", Some(json!("Save")), true, "SAVE"),
        ("conn-test", Some(json!("Test")), true, "TEST"),
        ("conn-connect", Some(json!("Connect")), true, "CONNECT"),
    ];
    assert_moved(&mut ui, &form, "the form");
    ui.click("conn-password-show");
    assert_moved(&mut ui, &[("conn-password-show", Some(json!(true)), true, "HIDE")], "the password shown");
    ui.click("conn-fill-url");
    ui.frame();
    let dialog: [Moved<'_>; 2] = [("dialog-ok", Some(json!("Fill")), false, "FILL"), ("dialog-cancel", Some(json!("Cancel")), true, "CANCEL")];
    assert_moved(&mut ui, &dialog, "the URL dialog");
    ui.text("postgres://ann@db.example/shop");
    ui.frame();
    assert_moved(&mut ui, &[("dialog-ok", Some(json!("Fill")), true, "FILL")], "a URL typed");
    ui.click("dialog-cancel");
    assert!(ui.tag("dialog-ok").is_none(), "Cancel closed the dialog");
}

/// The rect of automation entry `e`.
fn rect_of(e: &Json) -> egui::Rect {
    let [x, y, w, h] = rect(e).map(|v| v as f32);
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
}

/// The edges the last frame drew on `at`: each rect's stroke, drawn inside it, as (width, colour).
fn edges_on(ui: &Ui, at: egui::Rect) -> Vec<(f32, egui::Color32)> {
    let same = |a: egui::Rect| (a.min - at.min).length() < 0.5 && (a.max - at.max).length() < 0.5;
    ui.shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if r.stroke.width > 0.0 && r.stroke_kind == egui::StrokeKind::Inside && same(r.rect) => {
                Some((r.stroke.width, r.stroke.color))
            }
            _ => None,
        })
        .collect()
}

/// The form's fields and the URL dialog's, moved to `components::input`, say in `ui.inspect` what
/// they said before (ids, values, secrets, enabled flags), are the page's 28 tall, keep what is
/// typed as typed, and have their label drawn in capitals over them.
#[test]
fn moved_inputs_keep_their_ids_values_and_secrets() {
    const SECRET: &str = "typed-Secret-3";
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = ui();
    ui.click("conn-ssh-on");
    let fields = [
        ("conn-name", "Shop (Live)", "NAME"),
        ("conn-host", "Db.Example.COM", "HOST"),
        ("conn-port", "6543", "PORT"),
        ("conn-user", "Ann", "USER"),
        ("conn-database", "Shop", "DATABASE"),
        ("conn-ssh-host", "Bastion.Example.com", "SSH HOST"),
        ("conn-ssh-port", "2222", "SSH PORT"),
        ("conn-ssh-user", "Deploy", "SSH USER"),
    ];
    for (id, typed, _) in fields {
        ui.replace(id, typed);
    }
    ui.click("conn-password");
    ui.text(SECRET);
    let i = ui.inspect();
    for (id, typed, label) in fields {
        let e = tag_in(&i, id).unwrap_or_else(|| panic!("`{id}` is drawn"));
        assert_eq!(e["value"], typed, "`{id}`'s value");
        assert!(e.get("enabled").is_none() && e.get("masked").is_none(), "`{id}`: {e}");
        let at = rect_of(&e);
        assert_eq!(at.height(), 28.0, "`{id}` is the page's 28 tall: {at:?}");
        assert!(ui.text_in(at, typed).is_some(), "`{id}` draws `{typed}` as typed");
        assert!(typed == typed.to_uppercase() || ui.text_in(at, &typed.to_uppercase()).is_none(), "`{id}`: never in capitals");
        let above = egui::Rect::from_min_max(egui::pos2(at.left(), at.top() - 18.0), egui::pos2(at.right(), at.top() - 4.0));
        assert!(ui.text_in(above, label).is_some(), "`{id}`: its label `{label}` 4 above it, in {above:?}");
        assert_eq!(edges_on(&ui, at), [(1.0, t.line_strong)], "`{id}`: the input's edge");
    }
    let password = tag_in(&i, "conn-password").unwrap();
    assert_eq!((password["masked"].clone(), password["set"].clone()), (json!(true), json!(true)), "conn-password stays a secret");
    assert!(password.get("value").is_none(), "conn-password has no value in ui.inspect");
    assert!(!i.to_string().contains(SECRET), "the typed password is not in ui.inspect");
    let at = rect_of(&password);
    assert_eq!(at.height(), 28.0, "conn-password is the page's 28 tall: {at:?}");
    assert!(ui.text_in(at, &"•".repeat(SECRET.len())).is_some(), "conn-password draws a dot for each letter");
    let above = egui::Rect::from_min_max(egui::pos2(at.left(), at.top() - 18.0), egui::pos2(at.right(), at.top() - 4.0));
    assert!(ui.text_in(above, "PASSWORD").is_some(), "conn-password: its label 4 above it");
    ui.click("conn-password-show");
    assert!(ui.text_in(at, SECRET).is_some(), "Show draws the password");
    assert!(!ui.inspect().to_string().contains(SECRET), "shown, it is still not in ui.inspect");

    // A click keeps a field's edge; Shift+Tab's focus shows, and nothing moves.
    ui.click("conn-port");
    let (host, port) = (rect_of(&ui.tag("conn-host").unwrap()), rect_of(&ui.tag("conn-port").unwrap()));
    assert_eq!(edges_on(&ui, port), [(1.0, t.line_strong)], "clicked, conn-port keeps its edge");
    ui.key_with("Tab", json!({ "shift": true }));
    assert_eq!(edges_on(&ui, host), [(2.0, t.ink)], "Shift+Tab: conn-host shows the keyboard's focus");
    assert_eq!(edges_on(&ui, port), [(1.0, t.line_strong)], "and conn-port its edge");
    assert_eq!((rect_of(&ui.tag("conn-host").unwrap()), rect_of(&ui.tag("conn-port").unwrap())), (host, port), "nothing moved");

    // The URL dialog: its field a secret, focused as it opens; the reason a URL didn't parse under
    // it, with a red edge.
    ui.click("conn-fill-url");
    ui.frame();
    let url = ui.tag("dialog-url").unwrap();
    assert_eq!((url["masked"].clone(), url["set"].clone()), (json!(true), json!(false)), "{url}");
    assert!(url.get("enabled").is_none(), "{url}");
    let at = rect_of(&url);
    assert_eq!(at.height(), 28.0, "dialog-url is the page's 28 tall: {at:?}");
    assert_eq!(edges_on(&ui, at), [(2.0, t.ink)], "the dialog gives it focus, and it shows");
    ui.text("oracle://Nope.Example");
    ui.click("dialog-ok");
    let i = ui.inspect();
    let url = tag_in(&i, "dialog-url").unwrap();
    assert_eq!((url["masked"].clone(), url["set"].clone()), (json!(true), json!(true)), "{url}");
    assert!(!i.to_string().contains("Nope.Example"), "the URL is not in ui.inspect");
    let error = tag_in(&i, "dialog-error").expect("the reason shows");
    let reason = error["value"].as_str().unwrap().to_string();
    assert!(reason.contains("postgres://"), "{error}");
    assert_eq!(rect_of(&url), at, "the field didn't move");
    assert_eq!(edges_on(&ui, at), [(1.0, t.danger)], "a red edge");
    let line = rect_of(&error);
    assert!((line.top() - at.bottom() - 4.0).abs() < 0.5 && (line.left() - at.left()).abs() < 0.5, "the reason 4 under the field: {line:?} {at:?}");
    assert!(ui.text_in(line, &reason).is_some(), "the reason is drawn as it reads");
    ui.click("dialog-cancel");
    assert!(ui.tag("dialog-url").is_none(), "Cancel closed the dialog");
}

/// The secret dialog's field, moved to `components::input`: a secret in `ui.inspect`, focused as
/// the dialog opens, its text drawn as dots.
#[test]
fn the_moved_secret_field_stays_a_secret() {
    const SECRET: &str = "not-the-Password-4";
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = ui();
    let pg = pg14(db);
    ui.replace("conn-host", &pg.host);
    ui.replace("conn-port", &pg.port.to_string());
    ui.replace("conn-user", &pg.user);
    ui.replace("conn-database", &pg.database);
    ui.click("conn-connect");
    ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
    ui.frame();
    let secret = ui.tag("dialog-secret").unwrap();
    assert_eq!((secret["masked"].clone(), secret["set"].clone()), (json!(true), json!(false)), "{secret}");
    assert!(secret.get("enabled").is_none(), "{secret}");
    let at = rect_of(&secret);
    assert_eq!(at.height(), 28.0, "dialog-secret is the page's 28 tall: {at:?}");
    assert_eq!(edges_on(&ui, at), [(2.0, t.ink)], "the dialog gives it focus, and it shows");
    ui.text(SECRET);
    let i = ui.inspect();
    let secret = tag_in(&i, "dialog-secret").unwrap();
    assert_eq!((secret["masked"].clone(), secret["set"].clone()), (json!(true), json!(true)), "dialog-secret is set and masked");
    assert!(!i.to_string().contains(SECRET), "the typed secret is not in ui.inspect");
    assert!(!ui.document().to_string().contains(SECRET), "nor in document.inspect");
    assert!(ui.text_in(at, &"•".repeat(SECRET.len())).is_some(), "a dot for each letter");
    assert!(ui.text_in(at, SECRET).is_none(), "the secret itself is not drawn");
    ui.click("dialog-cancel");
    assert_eq!(ui.inspect()["dialog"], Json::Null);
}

/// The fills the last frame drew on `at`: each rect's ground.
fn fills_on(ui: &Ui, at: egui::Rect) -> Vec<egui::Color32> {
    let same = |a: egui::Rect| (a.min - at.min).length() < 0.5 && (a.max - at.max).length() < 0.5;
    ui.shapes()
        .into_iter()
        .filter_map(|(_, s)| match s {
            egui::Shape::Rect(r) if r.fill.a() > 0 && same(r.rect) => Some(r.fill),
            _ => None,
        })
        .collect()
}

/// The checkbox `id` (moved to `components::checkbox`) says `value` and `enabled` in `i` as before,
/// is the page's 18 tall, with its 14 box at its left, `ink` when ticked (`text_faint` when off),
/// and its label drawn as written 8 after the box.
fn assert_checkbox(ui: &Ui, i: &Json, id: &str, value: bool, enabled: bool, label: &str) {
    let t = dbee_ui_egui::theme::LIGHT;
    let e = tag_in(i, id).unwrap_or_else(|| panic!("`{id}` is drawn"));
    assert_eq!(e["value"], value, "`{id}`'s value: {e}");
    assert_eq!(e.get("enabled") != Some(&json!(false)), enabled, "`{id}` enabled: {e}");
    let at = rect_of(&e);
    assert_eq!(at.height(), 18.0, "`{id}` is the page's 18 tall: {at:?}");
    let boxed = egui::Rect::from_min_size(at.min + egui::vec2(0.0, 2.0), egui::Vec2::splat(14.0));
    let ground = match (value, enabled) {
        (true, true) => t.ink,
        (true, false) => t.text_faint,
        (false, _) => t.card,
    };
    assert_eq!(fills_on(ui, boxed), [ground], "`{id}`: its box");
    let (words, _) = ui.text_in(at, label).unwrap_or_else(|| panic!("`{id}` draws `{label}` as written in {at:?}"));
    assert!((words.left() - boxed.right() - 8.0).abs() < 0.5, "`{id}`: its label 8 after its box: {words:?} in {at:?}");
}

/// The form's checkboxes, SSL mode, SSH login and colours, moved to `components`, say in
/// `ui.inspect` what they said before (ids, values, enabled flags) and change as before; they are
/// drawn as the page draws them.
#[test]
fn moved_choices_keep_their_ids_and_values() {
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = ui();
    ui.click("conn-ssh-on");
    let i = ui.inspect();
    assert_checkbox(&ui, &i, "conn-save-secrets", true, true, "Save passwords in the keyring");
    assert_checkbox(&ui, &i, "conn-readonly", false, true, "Read-only");
    assert_checkbox(&ui, &i, "conn-ssh-on", true, true, "SSH tunnel");

    // SSL mode: the box, then a segment for each mode, its words as written.
    let ssl = tag_in(&i, "conn-ssl").expect("SSL mode is drawn");
    assert_eq!(ssl["value"], "prefer", "{ssl}");
    let row = rect_of(&ssl);
    assert_eq!(row.height(), 28.0, "the SSL box is the page's 28 tall: {row:?}");
    assert_eq!((edges_on(&ui, row), fills_on(&ui, row)), (vec![(1.0, t.line_strong)], vec![t.card]), "the SSL box");
    let modes = ["disable", "prefer", "require", "verify-ca", "verify-full"];
    for mode in modes {
        let e = tag_in(&i, &format!("conn-ssl-{mode}")).unwrap_or_else(|| panic!("`conn-ssl-{mode}` is drawn"));
        assert_eq!(e["value"], mode == "prefer", "{e}");
        assert!(e.get("enabled").is_none(), "{e}");
        let at = rect_of(&e);
        assert_eq!(at.height(), 26.0, "`conn-ssl-{mode}` is 26 tall inside the box: {at:?}");
        assert!(row.shrink(1.0).expand(0.01).contains_rect(at), "`conn-ssl-{mode}` inside the box's edge: {at:?} {row:?}");
        assert!(ui.text_in(at, mode).is_some(), "`conn-ssl-{mode}` draws `{mode}` as written");
    }
    assert_eq!(fills_on(&ui, rect_of(&tag_in(&i, "conn-ssl-prefer").unwrap())), [t.ink], "the chosen mode on ink");

    // The colours: squares, none chosen.
    let colours = tag_in(&i, "conn-color").expect("the colours are drawn");
    assert_eq!(colours["value"], "none", "{colours}");
    for name in ["none", "red", "orange", "yellow", "green", "blue", "purple", "gray"] {
        let e = tag_in(&i, &format!("conn-color-{name}")).unwrap_or_else(|| panic!("`conn-color-{name}` is drawn"));
        assert_eq!(e["value"], name == "none", "{e}");
        assert_eq!(rect_of(&e).size(), egui::Vec2::splat(20.0), "`conn-color-{name}` is a 20 square");
    }
    let swatches = rect_of(&colours);
    // The no-colour mark's round line ends are discs too, but small ones.
    let dots = ui.shapes().into_iter().filter(|(_, s)| matches!(s, egui::Shape::Circle(c) if swatches.contains(c.center) && c.radius >= 4.0)).count();
    assert_eq!(dots, 0, "a colour is a square, never a circle");
    assert_eq!(edges_on(&ui, rect_of(&tag_in(&i, "conn-color-none").unwrap())), [(2.0, t.ink)], "the chosen one's 2 px ink edge");

    // The SSH login, as SSL mode.
    let login = tag_in(&i, "conn-ssh-auth").expect("the SSH login is drawn");
    assert_eq!(login["value"], "key", "{login}");
    let row = rect_of(&login);
    assert_eq!((row.height(), edges_on(&ui, row)), (28.0, vec![(1.0, t.line_strong)]), "the login's box");
    for (name, words) in [("key", "Key"), ("agent", "Agent"), ("password", "Password")] {
        let e = tag_in(&i, &format!("conn-ssh-auth-{name}")).unwrap_or_else(|| panic!("`conn-ssh-auth-{name}` is drawn"));
        assert_eq!(e["value"], name == "key", "{e}");
        assert!(e.get("enabled").is_none(), "{e}");
        let at = rect_of(&e);
        assert_eq!(at.height(), 26.0, "`conn-ssh-auth-{name}` is 26 tall inside the box: {at:?}");
        assert!(ui.text_in(at, words).is_some(), "`conn-ssh-auth-{name}` draws `{words}` as written");
    }

    // They change as before.
    ui.click("conn-ssl-require");
    ui.click("conn-ssh-auth-agent");
    assert_eq!(
        (ui.value("conn-ssh-auth"), ui.value("conn-ssh-auth-agent"), ui.value("conn-ssh-auth-key")),
        (json!("agent"), json!(true), json!(false))
    );
    ui.click("conn-color-green");
    ui.click("conn-readonly");
    ui.click("conn-save-secrets");
    ui.click("conn-ssh-on");
    let i = ui.inspect();
    let shown = |id: &str| tag_in(&i, id).unwrap_or_else(|| panic!("`{id}` is drawn"))["value"].clone();
    assert_eq!(
        [shown("conn-ssl"), shown("conn-ssl-require"), shown("conn-ssl-prefer"), shown("conn-color"), shown("conn-color-green")],
        [json!("require"), json!(true), json!(false), json!("green"), json!(true)]
    );
    assert_eq!([shown("conn-readonly"), shown("conn-save-secrets"), shown("conn-ssh-on")], [json!(true), json!(false), json!(false)]);
    assert!(tag_in(&i, "conn-ssh-host").is_none(), "the SSH tunnel is off");

    // MySQL's own option.
    ui.click("conn-change-engine");
    ui.click("conn-kind-mysql");
    let i = ui.inspect();
    assert_checkbox(&ui, &i, "conn-option-allowPublicKey", false, true, "Allow the server's public key without encryption");
    ui.click("conn-option-allowPublicKey");
    assert_eq!(ui.value("conn-option-allowPublicKey"), true);

    // No keyring: Save passwords is off and says why.
    let mut without = ui_in(temp_home(None, None), false);
    without.click("conn-kind-postgres");
    let i = without.inspect();
    assert_checkbox(&without, &i, "conn-save-secrets", false, false, "Save passwords");
    assert!(tag_in(&i, "conn-no-keyring").is_some(), "the note says why");
}

/// The colour is chosen with the keyboard as with a click: Tab to a swatch shows its focus box,
/// Space chooses it. A click shows no focus.
#[test]
fn a_swatch_is_chosen_with_the_keyboard() {
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = ui();
    ui.click("conn-name");
    ui.key("Tab");
    ui.frame();
    let none = rect_of(&ui.tag("conn-color-none").unwrap());
    assert_eq!(edges_on(&ui, none.expand(4.0)), [(2.0, t.ink)], "Tab from the name: the first swatch shows the keyboard's focus");
    ui.key("Tab");
    ui.frame();
    ui.key("Space");
    ui.frame();
    assert_eq!(ui.value("conn-color"), "red", "Space chose red, as a click does");
    assert_eq!(ui.value("conn-color-red"), true);
    let red = rect_of(&ui.tag("conn-color-red").unwrap());
    assert_eq!(edges_on(&ui, red), [(2.0, t.ink)], "red is chosen");
    assert_eq!(edges_on(&ui, red.expand(4.0)), [(2.0, t.ink)], "and keeps the keyboard's focus");
    assert_eq!(edges_on(&ui, none.expand(4.0)), [], "the focus left none");
    ui.click("conn-color-blue");
    assert_eq!(ui.value("conn-color"), "blue", "a click chose blue");
    let blue = rect_of(&ui.tag("conn-color-blue").unwrap());
    assert_eq!(edges_on(&ui, blue.expand(4.0)), [], "a click shows no focus");
    assert_eq!(edges_on(&ui, red), [], "red is no longer chosen");
}

/// The password dialog's "Save in keyring", moved to `components::checkbox`: its id, value and
/// enabled flag as before, drawn as the page's checkbox.
#[test]
fn the_moved_keyring_checkbox_keeps_its_id_and_value() {
    let db = dbee_testkit::require_testdb!();
    let mut ui = ui();
    let card = ui.save(&pg14(db), None);
    ui.click(&card);
    ui.click("conn-connect");
    ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
    ui.frame();
    let i = ui.inspect();
    assert_checkbox(&ui, &i, "dialog-save", true, true, "Save in keyring");
    ui.click("dialog-save");
    assert_eq!(ui.value("dialog-save"), false, "a click unticks it");
    ui.click("dialog-cancel");
    // A connection that isn't saved: nothing to keep the password for.
    let mut ui = self::ui();
    let pg = pg14(db);
    ui.replace("conn-host", &pg.host);
    ui.replace("conn-port", &pg.port.to_string());
    ui.replace("conn-user", &pg.user);
    ui.replace("conn-database", &pg.database);
    ui.click("conn-connect");
    ui.until("the password dialog", |ui| ui.inspect()["dialog"] == "password");
    ui.frame();
    let i = ui.inspect();
    assert_checkbox(&ui, &i, "dialog-save", false, false, "Save in keyring");
    ui.click("dialog-cancel");
}

/// The open database list moves its own highlight with Down and Up (its names take no keyboard
/// focus), drawn as the menu item's hover: `raised_hover` over the page's 28 tall item.
#[test]
fn the_database_list_still_moves_with_the_arrows() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mut ui = form_for(db, db.server("mysql80").unwrap());
    ui.click("conn-database-list");
    ui.until("the list", |ui| !listed(&ui.inspect()).is_empty());
    ui.frame();
    for (keys, on) in [(&[][..], 0), (&["ArrowDown", "ArrowDown"][..], 2), (&["ArrowUp"][..], 1)] {
        for key in keys {
            ui.key(key);
        }
        ui.frame();
        let i = ui.inspect();
        for (n, (id, name)) in listed(&i).into_iter().enumerate().take(4) {
            let r = rect(&tag_in(&i, &id).unwrap());
            let at = egui::Rect::from_min_size(egui::pos2(r[0] as f32, r[1] as f32), egui::vec2(r[2] as f32, r[3] as f32));
            assert_eq!(at.height(), 28.0, "{id}: the page's 28 tall item");
            let grounds: Vec<egui::Color32> = ui
                .shapes()
                .into_iter()
                .filter_map(|(_, s)| match s {
                    egui::Shape::Rect(r) if r.rect == at && r.fill.a() > 0 => Some(r.fill),
                    _ => None,
                })
                .collect();
            let want = if n == on { vec![t.raised_hover] } else { vec![] };
            assert_eq!(grounds, want, "after {keys:?}: {name} highlighted: {}", n == on);
        }
    }
    assert_eq!(ui.value("conn-database"), "", "nothing chosen yet");
    ui.key("Enter");
    ui.frame();
    assert_eq!(ui.value("conn-database"), "odd", "Enter picks the highlighted name");
}

/// IN TRANSACTION comes from the engine's report of the query session: a `BEGIN` lights it, a
/// job running in the transaction shows in its place (with its CANCEL) and gives it back, and
/// `COMMIT` clears it. On Postgres and MySQL.
#[test]
fn in_transaction_shows_from_the_engine() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    for (server, sleep) in [("pg14", "select pg_sleep(30)"), ("mysql80", "select sleep(30)")] {
        let target = dbee_testkit::target(db.server(server).unwrap());
        let mut ui = connected_to_target(db, &target, None, APP_SIZE);
        assert!(ui.tag("status-transaction").is_none(), "{server}: no transaction yet");
        ui.execute("tab.newQuery", json!({ "text": "BEGIN" }));
        ui.execute("query.run", json!({}));
        ui.frame();
        assert_eq!(ui.document()["connection"]["transaction"]["state"], "open", "{server}");
        assert_eq!(ui.value("status-transaction"), "In transaction", "{server}");
        let tag = ui.tag("status-transaction").unwrap();
        let f = |i: usize| tag["rect"][i].as_f64().unwrap() as f32;
        let at = egui::Rect::from_min_size(egui::pos2(f(0), f(1)), egui::vec2(f(2), f(3)));
        assert!(ui.text_in(at, "IN TRANSACTION").is_some(), "{server}: drawn in capitals");
        let mark = egui::Rect::from_center_size(egui::pos2(at.left() - 8.0 - 4.0, at.center().y), egui::Vec2::splat(8.0));
        let marked = ui.shapes().iter().any(|(_, s)| matches!(s, egui::Shape::Rect(r) if r.rect == mark && r.fill == t.staged_strong));
        assert!(marked, "{server}: the staged_strong square before it at {mark:?}");
        // A job in the transaction shows in its place, with its CANCEL, and gives it back.
        ui.execute("query.setText", json!({ "text": sleep }));
        ui.ok("engine.execute", json!({ "command": "query.run", "wait": false }));
        ui.until("the run", |ui| ui.tag("status-job").is_some());
        assert!(ui.tag("status-transaction").is_none(), "{server}: one activity at a time");
        assert!(ui.tag("status-cancel").is_some(), "{server}: the job's CANCEL");
        ui.click("status-cancel");
        ui.until("the cancel", |ui| ui.document()["query"]["running"].is_null());
        ui.frame();
        assert_eq!(ui.value("status-transaction"), "In transaction", "{server}: back once the job ended");
        ui.execute("query.setText", json!({ "text": "COMMIT" }));
        ui.execute("query.run", json!({}));
        ui.frame();
        assert_eq!(ui.document()["connection"]["transaction"], Json::Null, "{server}");
        assert!(ui.tag("status-transaction").is_none(), "{server}: COMMIT clears it");
    }
}

/// The text `words` the last frame drew within `within`: its colour and font.
fn drawn_as(ui: &Ui, within: egui::Rect, words: &str) -> Option<(egui::Color32, egui::FontId)> {
    ui.shapes().into_iter().find_map(|(_, s)| match s {
        egui::Shape::Text(t) if t.galley.text() == words && within.expand(1.0).contains_rect(t.galley.rect.translate(t.pos.to_vec2())) => {
            let section = t.galley.job.sections.first()?;
            let colour = if section.format.color == egui::Color32::PLACEHOLDER { t.fallback_color } else { section.format.color };
            Some((t.override_text_color.unwrap_or(colour), section.format.font_id.clone()))
        }
        _ => None,
    })
}

/// The status bar's address (as the design draws it): after the name block, in `data`
/// grey, the connection as typed in its form. Postgres and MySQL, through the SSH tunnel (the
/// database's address as typed, then ` via ` and the SSH host, never the tunnel's local port),
/// and a SQLite file with the home folder as `~`. `status-address` holds it.
#[test]
fn the_address_is_the_connection_as_typed() {
    let db = dbee_testkit::require_testdb!();
    let t = dbee_ui_egui::theme::LIGHT;
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let sqlite = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().expect("the SQLite seed");
    let file = sqlite.file().unwrap().display().to_string();
    let home = dbee_config::home_dir().map(|h| h.display().to_string());
    let in_home = home.as_deref().and_then(|h| file.strip_prefix(h)).filter(|rest| rest.starts_with('/'));
    let file_address = in_home.map_or_else(|| file.clone(), |rest| format!("~{rest}"));
    let plain = || temp_home(None, None);
    let tunnel = || temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    let cases: Vec<(&str, ConnectionConfig, TempHome, String)> = vec![
        ("pg14", pg14(db), plain(), format!("postgres@127.0.0.1:{}/dbee", pg14_server(db).port)),
        ("mysql80, no database", config_of(&mysql), plain(), format!("root@127.0.0.1:{}", mysql.server().unwrap().port)),
        ("through the tunnel", via_ssh(db, SshAuth::Key { path: None }), tunnel(), "postgres@pg14:5432/dbee via 127.0.0.1".to_string()),
        ("sqlite", config_of(&sqlite), plain(), file_address),
    ];
    for (what, config, home, want) in cases {
        let mut ui = ui_in(home, true);
        ui.execute("connection.open", json!({ "config": config, "password": db.password }));
        ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
        assert_eq!(ui.value("status-address"), json!(want), "{what}");
        let at = rect_of(&ui.tag("status-address").unwrap());
        let name = rect_of(&ui.tag("status-connection").unwrap());
        let (colour, font) = drawn_as(&ui, at, &want).unwrap_or_else(|| panic!("{what}: `{want}` drawn as written in {at:?}"));
        assert_eq!((colour, font), (t.text_muted, dbee_ui_egui::theme::Text::Data.font()), "{what}: in `data` grey");
        assert!(at.left() > name.right() && (at.center().y - name.center().y).abs() < 1.0, "{what}: after the name {name:?}: {at:?}");
    }
}

/// The address never holds the password: not a connection's whose URL held one (Fill from URL),
/// named like a URL, nor one opened with a password and a name dbee made up. The password is in
/// no drawn text, no hover and no tag.
#[test]
fn the_address_never_holds_the_password() {
    let db = dbee_testkit::require_testdb!();
    let port = pg14_server(db).port;
    let mut ui = cards();
    ui.click("conn-fill-url");
    ui.text(&format!("postgres://postgres:{}@127.0.0.1:{port}/dbee", db.password));
    ui.click("dialog-ok");
    ui.replace("conn-name", &format!("postgres://postgres@127.0.0.1:{port}/dbee"));
    ui.click("conn-connect");
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    let want = format!("postgres@127.0.0.1:{port}/dbee");
    assert_eq!(ui.value("status-address"), json!(want), "the address shows");
    let at = ui.centre("status-address");
    ui.rest(at);
    let nowhere = |ui: &mut Ui, what: &str| {
        let texts: Vec<String> = ui.texts().iter().map(|(_, g)| g.text().to_string()).collect();
        assert!(texts.iter().all(|t| !t.contains(&db.password)), "{what}: drawn: {texts:?}");
        let inspect = ui.inspect().to_string();
        assert!(!inspect.contains(&db.password), "{what}: tags: {inspect}");
    };
    nowhere(&mut ui, "a URL-shaped name");
    // A name dbee made up: no address, and still no password.
    let mut ui = Ui::fresh(APP_SIZE);
    ui.execute("connection.open", json!({ "config": ConnectionConfig { name: String::new(), ..pg14(db) }, "password": db.password }));
    ui.until("the workspace", |ui| ui.document()["screen"] == "workspace");
    assert!(ui.tag("status-address").is_none(), "the name block already is the address");
    nowhere(&mut ui, "a made-up name");
}
