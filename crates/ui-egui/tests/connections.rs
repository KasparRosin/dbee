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
use dbee_config::{ConnectionConfig, DbKind, SecretKind, SecretStore, Settings, SshAuth, SshConfig, SslMode};
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
        ssh: Some(SshConfig { host: "127.0.0.1".into(), port: db.sshd.port, user: db.sshd.user.clone(), auth }),
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
    assert!(ui.value("conn-error").as_str().unwrap().contains("file is empty"));
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
    let text = ui.value("dialog-text");
    assert!(text.as_str().unwrap().contains("mysql://"), "{text}");
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
    ui.click("conn-kind-mysql");
    assert!(ui.tag("conn-host").is_none() && ui.tag("conn-kind-mysql").is_some(), "nothing was chosen");
    // Nor through a URL: the dialog names what this build opens.
    ui.click("conn-fill-url");
    let text = ui.value("dialog-text").as_str().unwrap().to_string();
    assert!(text.contains("postgres://") && !text.contains("mysql://"), "{text}");
    ui.text("mysql://u@h/shop");
    ui.click("dialog-ok");
    assert_eq!(ui.inspect()["dialog"], "fillFromUrl");
    let error = ui.value("dialog-error").as_str().unwrap().to_string();
    assert!(error.contains("postgres://"), "{error}");
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
    let store = ui.secrets.clone().unwrap();
    assert_eq!(store.get(&id, SecretKind::SshPassphrase).unwrap().as_deref(), Some(db.keys.passphrase.as_str()));
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
    // Back on the connection screen, the recent list has it.
    ui.execute("connection.close", json!({}));
    let i = ui.inspect();
    let recent = tag_in(&i, "conn-recent-0").expect("the recent card");
    assert_eq!(recent["value"], "pg14");
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
    assert_eq!(ui.secrets.clone().unwrap().get(&id, SecretKind::DbPassword).unwrap().as_deref(), Some("typed-secret-1"));
    let everything = format!("{} {} {}", ui.inspect(), ui.document(), ui.ok("engine.journal", json!({})));
    assert!(!everything.contains("typed-secret-1"), "{everything}");
    let file = std::fs::read_to_string(ui.home.home.join("config").join("connections.json")).unwrap();
    assert!(!file.contains("typed-secret-1"), "{file}");
}
