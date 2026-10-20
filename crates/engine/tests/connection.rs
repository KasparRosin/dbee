//! Connection commands against the Docker test servers (`cargo xtask testdb up`): save, list,
//! duplicate, delete, URL parsing, the SSH key preview, test, open, secret resolution and the
//! prompts, close, reconnect after a lost connection, and the schema refresh.
//!
//! Sessions come from `common` (temp home, `MemoryStore`, SSH files in the temp home, no agent).
//! The tests that kill backends run in an exclusive session (see `common`).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use dbee_config::{ConnectionColor, ConnectionConfig, SecretError, SecretKind, SecretStore, SshAuth, SshConfig};
use dbee_engine::{EngineError, Json, Outcome, UiRequest};
use dbee_testkit::{require_testdb, temp_home, unique};
use serde_json::json;

/// Opening `config` with no password given and none saved.
fn open_err(config: ConnectionConfig) -> EngineError {
    let mut s = session();
    run_err(&mut s, "connection.open", json!({ "config": config }))
}

fn config_without_saved_password() -> ConnectionConfig {
    pg14_config()
}

/// A config nothing listens on (port 1 is refused at once), for tests without a server.
fn refused_config(name: &str) -> ConnectionConfig {
    ConnectionConfig { name: name.into(), host: "127.0.0.1".into(), port: 1, user: "u".into(), database: "d".into(), ..ConnectionConfig::default() }
}

/// No secret text anywhere a user, a log or an agent could see it: `inspect()`, the state's
/// `Debug`, the journal and the connections file.
fn assert_secret_hidden(s: &TestSession, secret: &str) {
    assert!(!s.inspect().to_string().contains(secret), "inspect() shows a secret");
    assert!(!format!("{:?}", s.state()).contains(secret), "the state's Debug shows a secret");
    assert!(!format!("{:?}", s.session).contains(secret), "the session's Debug shows a secret");
    assert!(!serde_json::to_string(s.journal()).unwrap().contains(secret), "the journal keeps a secret");
    let file = s.home.home.join("config").join(dbee_config::store::FILE);
    if let Ok(text) = std::fs::read_to_string(&file) {
        assert!(!text.contains(secret), "the connections file holds a secret");
    }
}

#[test]
fn save_list_duplicate_delete() {
    let mut s = session();
    let config = ConnectionConfig { color: ConnectionColor::Red, ..refused_config("Shop live") };
    let id = save(&mut s, config, Some("pw-1234".into()));
    assert!(!id.is_empty(), "an id is assigned");
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap().as_deref(), Some("pw-1234"));

    let list = s.run_blocking("connection.list", json!({}), T).unwrap();
    assert_eq!(list["saved"].as_array().map(Vec::len), Some(1), "{list}");
    assert_eq!(list["saved"][0]["id"], id);
    assert_eq!(list["saved"][0]["name"], "Shop live");
    assert_eq!(list["saved"][0]["color"], "red");
    assert_eq!(list["recent"], json!([]));
    assert!(!list.to_string().contains("pw-1234"), "connection.list shows no secrets");

    let dup = s.run_blocking("connection.duplicate", json!({ "id": id }), T).unwrap();
    let dup_id = dup["id"].as_str().unwrap().to_owned();
    assert_ne!(dup_id, id);
    let saved = &s.state().store.saved;
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[1].name, "Shop live (copy)");
    assert_eq!((saved[1].host.as_str(), saved[1].port, saved[1].color), ("127.0.0.1", 1, ConnectionColor::Red));
    assert_eq!(s.secrets.get(&dup_id, SecretKind::DbPassword).unwrap().as_deref(), Some("pw-1234"), "the copy can connect too");

    s.run_blocking("connection.delete", json!({ "id": id }), T).unwrap();
    assert_eq!(s.state().store.saved.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), [dup_id.as_str()]);
    for kind in [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase] {
        assert_eq!(s.secrets.get(&id, kind).unwrap(), None, "{kind:?} of the deleted connection is gone");
    }
    assert_eq!(s.run_blocking("connection.delete", json!({ "id": id }), T).unwrap_err().code(), "badParams");
    assert_eq!(s.run_blocking("connection.duplicate", json!({ "id": "nope" }), T).unwrap_err().code(), "badParams");

    // The file was written, and a new session reads the saved list back.
    let file = std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap();
    assert!(file.contains("Shop live (copy)"), "{file}");
    assert_secret_hidden(&s, "pw-1234");
}

#[test]
fn save_keeps_or_drops_secrets_as_told() {
    let mut s = session();
    let id = save(&mut s, refused_config("a"), Some("first".into()));
    // Saving again without a password keeps the stored one.
    let again = ConnectionConfig { id: id.clone(), ..refused_config("a renamed") };
    assert_eq!(save(&mut s, again.clone(), None), id, "an existing id is kept");
    assert_eq!(s.state().store.saved.len(), 1);
    assert_eq!(s.state().store.saved[0].name, "a renamed");
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap().as_deref(), Some("first"));
    // SSH secrets are saved under their own kinds.
    s.run_blocking("connection.save", json!({ "config": again, "sshPassword": "sp", "sshPassphrase": "pp" }), T).unwrap();
    assert_eq!(s.secrets.get(&id, SecretKind::SshPassword).unwrap().as_deref(), Some("sp"));
    assert_eq!(s.secrets.get(&id, SecretKind::SshPassphrase).unwrap().as_deref(), Some("pp"));
    // `save_secrets: false` keeps none, and forgets the ones saved before.
    let off = ConnectionConfig { save_secrets: false, ..again };
    s.run_blocking("connection.save", json!({ "config": off, "password": "second" }), T).unwrap();
    for kind in [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase] {
        assert_eq!(s.secrets.get(&id, kind).unwrap(), None, "{kind:?}");
    }
    assert_secret_hidden(&s, "second");
}

#[test]
fn bad_configs_are_bad_params() {
    let mut s = session();
    for params in [
        json!({}),
        json!({ "config": "postgres://x" }),
        json!({ "config": {} }),
        json!({ "config": { "name": "no host" } }),
        json!({ "config": { "host": "127.0.0.1", "port": 0 } }),
        json!({ "config": { "host": "127.0.0.1", "port": 70000 } }),
        json!({ "config": { "host": " " } }),
        json!({ "config": { "host": "127.0.0.1", "ssh": { "host": "127.0.0.1", "port": 22, "user": "" } } }),
    ] {
        let e = s.execute("connection.save", params.clone()).unwrap_err();
        assert_eq!(e.code(), "badParams", "{params}: {e}");
    }
    assert!(s.state().store.saved.is_empty());
}

#[test]
fn test_and_open_need_an_id_or_a_config() {
    let mut s = session();
    for id in ["connection.test", "connection.open"] {
        for params in [json!({}), json!({ "config": {} }), json!({ "id": "nope" }), json!({ "id": 5 })] {
            let e = s.execute(id, params.clone()).unwrap_err();
            assert_eq!(e.code(), "badParams", "{id} {params}: {e}");
        }
        let both = json!({ "id": "x", "config": refused_config("x") });
        assert_eq!(s.execute(id, both).unwrap_err().code(), "badParams", "{id}: `id` and `config` together");
    }
    assert!(!s.busy(), "nothing was started");
}

#[test]
fn parse_url_fills_the_form_and_is_not_journaled() {
    let mut s = session();
    let r = s.run_blocking("connection.parseUrl", json!({ "url": "postgresql://ann:s%40cret@db.example.com:6543/shop?sslmode=require" }), T).unwrap();
    assert_eq!(r, json!({ "host": "db.example.com", "port": 6543, "user": "ann", "password": "s@cret", "database": "shop", "sslMode": "require" }));
    let r = s.run_blocking("connection.parseUrl", json!({ "url": "postgres://localhost" }), T).unwrap();
    assert_eq!(r, json!({ "host": "localhost", "port": null, "user": null, "password": null, "database": null, "sslMode": null }));
    assert_eq!(s.execute("connection.parseUrl", json!({ "url": "mysql://x" })).unwrap_err().code(), "badParams");
    assert_eq!(s.execute("connection.parseUrl", json!({})).unwrap_err().code(), "badParams");
    assert!(s.journal().iter().all(|(id, _)| id != "connection.parseUrl"), "a URL can carry a password");
}

#[test]
fn ssh_key_preview() {
    let home = temp_home(None, None);
    std::fs::write(home.ssh_dir().join("id_ed25519"), "not read by the preview").unwrap();
    let key = home.ssh_dir().join("id_ed25519");
    let mut s = session_in(home);
    let with = |auth: SshAuth| ConnectionConfig {
        ssh: Some(SshConfig { host: "127.0.0.1".into(), port: 1, user: "dbee".into(), auth }),
        ..refused_config("ssh")
    };
    let plan = |s: &mut TestSession, c: ConnectionConfig| s.run_blocking("connection.sshKey", json!({ "config": c }), T).unwrap();
    assert_eq!(plan(&mut s, with(SshAuth::Key { path: None })), json!({ "method": "file", "path": key.display().to_string() }));
    assert_eq!(plan(&mut s, with(SshAuth::Key { path: Some("/k/other".into()) })), json!({ "method": "file", "path": "/k/other" }));
    assert_eq!(plan(&mut s, with(SshAuth::Password)), json!({ "method": "password" }));
    assert_eq!(plan(&mut s, with(SshAuth::Agent)), json!({ "method": "none" }), "no agent in tests");
    std::fs::remove_file(&key).unwrap();
    assert_eq!(plan(&mut s, with(SshAuth::Key { path: None })), json!({ "method": "none" }));
    let e = s.run_blocking("connection.sshKey", json!({ "config": refused_config("no ssh") }), T).unwrap_err();
    assert_eq!(e.code(), "badParams");
}

#[test]
fn test_command_reports_server() {
    let db = require_testdb!();
    let mut s = session();
    for server in &db.servers {
        let r = s.run_blocking("connection.test", json!({ "config": server_config(server), "password": pw() }), T).unwrap();
        let text = r["server"].as_str().unwrap();
        assert!(text.contains("PostgreSQL") && text.contains(&server.major.to_string()), "{r}");
        assert_eq!(r["via"], "direct");
        let i = s.inspect();
        assert_eq!(i["screen"], "connections");
        assert_eq!(i["connection"], Json::Null);
    }
    assert!(s.state().store.recent.is_empty(), "a test is not a use");
    assert!(!s.busy());
    let e = run_err(&mut s, "connection.test", json!({ "config": refused_config("refused") }));
    assert_eq!(e.code(), "connect", "{e}");
}

#[test]
fn open_saved_with_stored_password() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, pg14_config(), Some(pw()));
    s.run_blocking("connection.open", json!({ "id": id }), T).unwrap();
    let i = s.inspect();
    assert_eq!(i["screen"], "workspace");
    assert_eq!(i["connection"]["name"], "pg14");
    assert_eq!(s.state().store.recent[0].config.id, id);
}

#[test]
fn open_returns_the_connection_server_and_tables() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, pg14_config(), Some(pw()));
    let r = open(&mut s, json!({ "id": id }));
    assert_eq!(r["connection"]["id"], id);
    assert_eq!(r["connection"]["name"], "pg14");
    assert!(r["server"].as_str().unwrap().starts_with("PostgreSQL 14"), "{r}");
    let tables = r["tables"].as_u64().unwrap();
    assert!(tables > 10, "the seed has the shop tables: {r}");
    let c = s.state().connected.as_ref().unwrap();
    assert_eq!(c.catalog.table_count() as u64, tables);
    assert!(c.catalog.table(&dbee_core::TableRef { schema: "shop".into(), name: "orders".into() }).is_some());
    assert!(!c.lost);
    // The recent list was written to the connections file.
    let file = std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap();
    assert!(file.contains("usedAt"), "{file}");
    // One connection per window.
    let e = s.execute("connection.open", json!({ "id": id })).unwrap_err();
    assert_eq!(e.code(), "disabled", "{e}");
}

#[test]
fn missing_password_needs_password() {
    let _db = require_testdb!();
    assert_eq!(open_err(config_without_saved_password()).code(), "needsPassword");
}

#[test]
fn wrong_password_is_an_auth_error() {
    let _db = require_testdb!();
    let mut s = session();
    let e = run_err(&mut s, "connection.open", json!({ "config": pg14_config(), "password": "not-the-password" }));
    assert_eq!(e.code(), "auth", "{e}");
    assert_eq!(s.inspect()["screen"], "connections");
    assert_secret_hidden(&s, "not-the-password");
}

#[test]
fn password_in_params_beats_the_stored_one() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, pg14_config(), Some("stale".into()));
    let e = run_err(&mut s, "connection.open", json!({ "id": id }));
    assert_eq!(e.code(), "auth", "the stored password is used: {e}");
    open(&mut s, json!({ "id": id, "password": pw() }));
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap().as_deref(), Some("stale"), "not saved without savePassword");
}

#[test]
fn save_password_on_open_of_a_saved_connection_turns_saving_on() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, ConnectionConfig { save_secrets: false, ..pg14_config() }, Some(pw()));
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap(), None, "saveSecrets is off");
    let r = open(&mut s, json!({ "id": id, "password": pw(), "savePassword": true }));
    assert_eq!(r["secretsSaved"], true, "{r}");
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap(), Some(pw()));
    assert!(s.state().store.saved[0].save_secrets, "the saved flag agrees with the store");
    let file = std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap();
    let body: Json = serde_json::from_str(&file).unwrap();
    assert_eq!(body["saved"][0]["saveSecrets"], true, "{file}");
    s.run_blocking("connection.close", json!({}), T).unwrap();
    open(&mut s, json!({ "id": id }));
    assert_secret_hidden(&s, &pw());
}

#[test]
fn save_password_on_open_of_an_ad_hoc_config_stores_nothing() {
    let _db = require_testdb!();
    let mut s = session();
    let r = open(&mut s, json!({ "config": pg14_config(), "password": pw(), "savePassword": true }));
    assert_eq!(r["secretsSaved"], false, "{r}");
    assert!(r["secretsNote"].as_str().unwrap().contains("not saved"), "{r}");
    let id = s.state().store.recent[0].config.id.clone();
    assert!(!id.is_empty(), "an unsaved config gets an id when it is used");
    for kind in [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase] {
        assert_eq!(s.secrets.get(&id, kind).unwrap(), None, "{kind:?}");
    }
    assert!(s.state().store.saved.is_empty());
    let plain = open_or_err_after_close(&mut s, json!({ "id": id }));
    assert_eq!(plain.code(), "needsPassword", "the recent entry has no stored password: {plain}");
}

/// Closes the connection, then opens `params`, expecting an error.
fn open_or_err_after_close(s: &mut TestSession, params: Json) -> EngineError {
    s.run_blocking("connection.close", json!({}), T).unwrap();
    run_err(s, "connection.open", params)
}

#[test]
fn stored_password_goes_only_to_its_own_server() {
    let _db = require_testdb!();
    let mut s = session();
    let saved = ConnectionConfig { id: "shop".into(), ..pg14_config() };
    save(&mut s, saved.clone(), Some(pw()));
    // The saved settings themselves use the stored password.
    s.run_blocking("connection.test", json!({ "config": saved }), T).unwrap();
    // The same id with another user (or host, or port) does not get it.
    let other_user = ConnectionConfig { user: "someone_else".into(), ..saved.clone() };
    let e = run_err(&mut s, "connection.test", json!({ "config": other_user }));
    assert_eq!(e.code(), "needsPassword", "the stored password must not be sent for another user: {e}");
    let e = run_err(&mut s, "connection.open", json!({ "config": ConnectionConfig { user: "someone_else".into(), ..saved } }));
    assert_eq!(e.code(), "needsPassword", "{e}");
}

#[test]
fn stored_passphrase_needs_the_same_key_file_and_ssh_login() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(None, Some(&db.sshd.known_hosts_line)));
    let saved = ConnectionConfig { id: "tunnel".into(), ..via_ssh(SshAuth::Key { path: Some(db.keys.encrypted.clone()) }) };
    s.run_blocking("connection.save", json!({ "config": saved, "password": pw(), "sshPassphrase": db.keys.passphrase }), T).unwrap();
    // The same key at another path: the stored passphrase is not tried on it.
    let copy = s.home.home.join("same_key_elsewhere");
    std::fs::copy(&db.keys.encrypted, &copy).unwrap();
    let mut moved = saved.clone();
    moved.ssh.as_mut().unwrap().auth = SshAuth::Key { path: Some(copy) };
    let e = run_err(&mut s, "connection.open", json!({ "config": moved }));
    assert_eq!(e.code(), "needsPassphrase", "{e}");
    // Another SSH user: not reused either.
    let mut other = saved.clone();
    other.ssh.as_mut().unwrap().user = "someone_else".into();
    let e = run_err(&mut s, "connection.open", json!({ "config": other }));
    assert_eq!(e.code(), "needsPassphrase", "{e}");
    // The saved settings work with the stored secrets.
    open(&mut s, json!({ "config": saved }));
}

#[test]
fn ssh_default_key_then_unknown_host_flow() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(Some(&db.keys.plain), None));
    let c = via_ssh(SshAuth::Key { path: None });
    let e = run_err(&mut s, "connection.open", json!({ "config": c, "password": pw() }));
    assert_eq!(e.code(), "needsHostKey", "{e}");
    let fp = e.data()["fingerprint"].as_str().unwrap().to_owned();
    assert!(fp.starts_with("SHA256:"), "{fp}");
    assert_eq!(e.data()["host"], sshd_entry());
    assert_eq!(e.data()["keyType"], "ssh-ed25519");
    assert!(!s.home.ssh_dir().join("known_hosts").exists(), "nothing is learned without trust");

    let r = open(&mut s, json!({ "config": c, "password": pw(), "trustHostKey": fp }));
    assert!(r["server"].as_str().unwrap().starts_with("PostgreSQL 14"), "{r}");
    assert_eq!(s.inspect()["connection"]["ssh"], true);
    let known = std::fs::read_to_string(s.home.ssh_dir().join("known_hosts")).unwrap();
    assert!(known.contains(&sshd_entry()), "{known}");

    s.run_blocking("connection.close", json!({}), T).unwrap();
    open(&mut s, json!({ "config": c, "password": pw() }));
}

#[test]
fn encrypted_key_needs_passphrase() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(None, Some(&db.sshd.known_hosts_line)));
    let c = via_ssh(SshAuth::Key { path: Some(db.keys.encrypted.clone()) });
    let e = run_err(&mut s, "connection.open", json!({ "config": c, "password": pw() }));
    assert_eq!(e.code(), "needsPassphrase", "{e}");
    assert_eq!(e.data()["key"], db.keys.encrypted.display().to_string());
    let e = run_err(&mut s, "connection.open", json!({ "config": c, "password": pw(), "sshPassphrase": "wrong" }));
    assert_eq!(e.code(), "ssh", "a wrong passphrase is an SSH error: {e}");
    open(&mut s, json!({ "config": c, "password": pw(), "sshPassphrase": db.keys.passphrase }));
    assert_secret_hidden(&s, &db.keys.passphrase);
}

#[test]
fn stored_passphrase_is_used() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(None, Some(&db.sshd.known_hosts_line)));
    let c = via_ssh(SshAuth::Key { path: Some(db.keys.encrypted.clone()) });
    let r = s.run_blocking("connection.save", json!({ "config": c, "password": pw(), "sshPassphrase": db.keys.passphrase }), T).unwrap();
    let id = r["id"].as_str().unwrap().to_owned();
    assert_eq!(s.secrets.get(&id, SecretKind::SshPassphrase).unwrap(), Some(db.keys.passphrase.clone()));
    open(&mut s, json!({ "id": id }));
}

#[test]
fn ssh_password_login_asks_for_the_password() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(None, Some(&db.sshd.known_hosts_line)));
    let mut c = via_ssh(SshAuth::Password);
    if let Some(ssh) = c.ssh.as_mut() {
        ssh.user = db.sshd.password_user.clone();
    }
    let e = run_err(&mut s, "connection.test", json!({ "config": c, "password": pw() }));
    assert_eq!(e.code(), "needsSshPassword", "{e}");
    let r = s.run_blocking("connection.test", json!({ "config": c, "password": pw(), "sshPassword": db.password }), T).unwrap();
    assert_eq!(r["via"], "ssh password", "{r}");
    let e = run_err(&mut s, "connection.test", json!({ "config": c, "password": pw(), "sshPassword": "wrong" }));
    assert_eq!(e.code(), "ssh", "{e}");
    assert_secret_hidden(&s, "wrong");
}

#[test]
fn test_through_ssh_reports_how_it_logged_in() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line)));
    let r = s.run_blocking("connection.test", json!({ "config": via_ssh(SshAuth::Key { path: None }), "password": pw() }), T).unwrap();
    let key = s.home.ssh_dir().join("id_ed25519");
    assert_eq!(r["via"], format!("ssh key {}", key.display()), "{r}");
    assert_eq!(s.inspect()["screen"], "connections");
}

#[test]
fn changed_host_key_refuses_with_the_line() {
    let db = require_testdb!();
    // Another ed25519 key recorded for the sshd: the client key's public half.
    let other = std::fs::read_to_string(db.keys.plain.with_extension("pub")).unwrap();
    let other = other.split_whitespace().nth(1).unwrap().to_owned();
    let line = format!("# saved earlier\n{} ssh-ed25519 {other}\n", sshd_entry());
    let mut s = session_in(temp_home(Some(&db.keys.plain), Some(&line)));
    let c = via_ssh(SshAuth::Key { path: None });
    let e = run_err(&mut s, "connection.open", json!({ "config": c, "password": pw() }));
    assert_eq!(e.code(), "ssh", "{e}");
    assert_eq!(e.data()["line"], 2, "{}", e.data());
    assert!(e.to_string().contains("line 2"), "{e}");
    assert_eq!(s.inspect()["screen"], "connections");
}

#[test]
fn close_with_query_text_needs_confirm() {
    let _db = require_testdb!();
    let mut s = connected();
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    let e = s.execute("connection.close", json!({})).unwrap_err();
    assert_eq!(e.code(), "needsConfirm", "{e}");
    assert_eq!(e.data()["what"], "queryText");
    assert_eq!(s.inspect()["screen"], "workspace");
    s.run_blocking("connection.close", json!({ "force": true }), T).unwrap();
    let i = s.inspect();
    assert_eq!(i["screen"], "connections");
    assert_eq!(i["connection"], Json::Null);
    assert_eq!(i["tabs"], json!([]));
    assert_eq!(s.execute("connection.close", json!({})).unwrap_err().code(), "disabled");
    assert_eq!(s.execute("schema.refresh", json!({})).unwrap_err().code(), "disabled");
    assert_eq!(s.execute("connection.reconnect", json!({})).unwrap_err().code(), "disabled");
}

#[test]
fn blank_query_tabs_close_without_asking() {
    let _db = require_testdb!();
    let mut s = connected();
    s.execute("tab.newQuery", json!({ "text": " \n " })).unwrap();
    s.run_blocking("connection.close", json!({}), T).unwrap();
    assert_eq!(s.inspect()["screen"], "connections");
}

#[test]
fn schema_refresh_reloads_and_keeps_tabs() {
    let _db = require_testdb!();
    let mut s = connected();
    s.execute("tab.newQuery", json!({ "text": "select 2" })).unwrap();
    let table = unique("t_refresh");
    dbee_testkit::psql(pg14(), &format!("CREATE TABLE scratch.{table} (id int)")).unwrap();
    let r = s.run_blocking("schema.refresh", json!({}), T);
    let found = s.state().connected.as_ref().unwrap().catalog.table(&dbee_core::TableRef { schema: "scratch".into(), name: table.clone() }).is_some();
    dbee_testkit::psql(pg14(), &format!("DROP TABLE scratch.{table}")).unwrap();
    let r = r.unwrap();
    assert!(found, "the refreshed catalog has the new table");
    assert_eq!(r["tables"].as_u64().map(|n| n as usize), Some(s.state().connected.as_ref().unwrap().catalog.table_count()));
    assert_eq!(s.inspect()["query"]["text"], "select 2");
    assert_eq!(s.state().tabs.len(), 1);
}

#[test]
fn reconnect_after_backend_killed() {
    let _db = require_testdb!();
    let mut s = exclusive_session();
    // The password is given only here, so the reconnect must use the one kept in memory.
    open(&mut s, json!({ "config": pg14_config(), "password": pw() }));
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    kill_dbee_backends(&s, pg14());

    let e = run_err(&mut s, "schema.refresh", json!({}));
    assert_eq!(e.code(), "connectionLost", "{e}");
    assert_eq!(e.data()["action"], "connection.reconnect");
    assert_eq!(s.inspect()["connection"]["lost"], true);
    let e = s.execute("schema.refresh", json!({})).unwrap_err();
    assert_eq!(e.code(), "disabled");
    assert_eq!(e.data()["why"], "connection lost — reconnect");
    let refresh = s.commands().into_iter().find(|c| c.id == "schema.refresh").unwrap();
    assert_eq!(refresh.disabled_reason.as_deref(), Some("connection lost — reconnect"));

    let r = s.run_blocking("connection.reconnect", json!({}), T).unwrap();
    assert!(r["server"].as_str().unwrap().starts_with("PostgreSQL 14"), "{r}");
    assert_eq!(s.inspect()["connection"]["lost"], false);
    assert_eq!(s.inspect()["screen"], "workspace");
    assert_eq!(s.inspect()["query"]["text"], "select 1");
    s.run_blocking("schema.refresh", json!({}), T).unwrap();
    assert_secret_hidden(&s, &pw());
}

#[test]
fn reconnect_through_ssh_after_backend_killed() {
    let db = require_testdb!();
    let mut s = exclusive_session();
    std::fs::write(s.home.ssh_dir().join("known_hosts"), format!("{}\n", db.sshd.known_hosts_line)).unwrap();
    let c = via_ssh(SshAuth::Key { path: Some(db.keys.encrypted.clone()) });
    open(&mut s, json!({ "config": c, "password": pw(), "sshPassphrase": db.keys.passphrase }));
    kill_dbee_backends(&s, pg14());
    assert_eq!(run_err(&mut s, "schema.refresh", json!({})).code(), "connectionLost");
    // A new tunnel, with the passphrase kept in memory.
    s.run_blocking("connection.reconnect", json!({}), T).unwrap();
    assert_eq!(s.inspect()["connection"]["lost"], false);
    s.run_blocking("schema.refresh", json!({}), T).unwrap();
    assert_secret_hidden(&s, &db.keys.passphrase);
}

#[test]
fn reconnect_while_connected_replaces_the_connection() {
    let _db = require_testdb!();
    let mut s = connected();
    let before = s.state().connected.as_ref().unwrap().info.clone();
    s.run_blocking("connection.reconnect", json!({}), T).unwrap();
    assert_eq!(s.state().connected.as_ref().unwrap().info, before);
    s.run_blocking("schema.refresh", json!({}), T).unwrap();
}

#[test]
fn a_second_open_while_one_runs_is_busy() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, pg14_config(), Some(pw()));
    let Outcome::Started(job) = s.execute("connection.open", json!({ "id": id })).unwrap() else { panic!("open is a job") };
    let e = s.execute("connection.open", json!({ "id": id })).unwrap_err();
    assert_eq!(e.code(), "busy", "{e}");
    s.wait(job, T).unwrap();
    assert_eq!(s.inspect()["screen"], "workspace");
}

#[test]
fn every_command_survives_empty_params_while_connected() {
    let _db = require_testdb!();
    let mut s = connected();
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    for c in s.commands() {
        if let Err(e @ EngineError::Internal { .. }) = s.execute(c.id, json!({})) {
            panic!("{}: {e}", c.id);
        }
        assert!(s.wait_idle(Duration::from_secs(30)), "{} left a job running", c.id);
        for (job, r) in s.pump() {
            if let Err(e @ EngineError::Internal { .. }) = r {
                panic!("{} job {}: {e}", c.id, job.0);
            }
        }
    }
}

#[test]
fn connection_commands_are_registered() {
    let s = session();
    let infos = s.commands();
    let get = |id: &str| infos.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("no command {id}"));
    assert_eq!(get("connection.close").shortcut, Some("Ctrl+Shift+Q"));
    assert_eq!(get("connection.close").menu, ["File"]);
    assert_eq!(get("connection.open").menu, ["File"]);
    assert_eq!(get("connection.close").disabled_reason.as_deref(), Some("not connected"));
    for id in [
        "connection.list",
        "connection.save",
        "connection.delete",
        "connection.duplicate",
        "connection.parseUrl",
        "connection.sshKey",
        "connection.test",
        "connection.open",
    ] {
        assert!(get(id).enabled, "{id} runs on the connection screen");
    }
    for id in ["connection.reconnect", "schema.refresh", "connection.close"] {
        assert!(!get(id).enabled, "{id} needs a connection");
    }
}

/// A secret store that refuses everything, like a locked keyring.
struct BrokenStore;

impl SecretStore for BrokenStore {
    fn get(&self, _: &str, _: SecretKind) -> Result<Option<String>, SecretError> {
        Err(SecretError("locked".into()))
    }
    fn set(&self, _: &str, _: SecretKind, _: &str) -> Result<(), SecretError> {
        Err(SecretError("locked".into()))
    }
    fn delete(&self, _: &str, _: SecretKind) -> Result<(), SecretError> {
        Err(SecretError("locked".into()))
    }
    fn name(&self) -> &'static str {
        "broken"
    }
}

#[test]
fn without_a_secret_store_dbee_asks_each_time() {
    let _db = require_testdb!();
    let mut s = session_adjusted(|o| o.secrets = None);
    assert_eq!(s.inspect()["secretStore"], Json::Null);
    let id = save(&mut s, pg14_config(), Some(pw()));
    let e = run_err(&mut s, "connection.open", json!({ "id": id }));
    assert_eq!(e.code(), "needsPassword", "{e}");
    let r = open(&mut s, json!({ "id": id, "password": pw(), "savePassword": true }));
    assert_eq!(r["secretsSaved"], false, "{r}");
    assert!(r["secretsNote"].as_str().unwrap().contains("secret store"), "{r}");
    assert_eq!(s.take_ui_requests(), [], "no store, so nothing failed");
    assert_secret_hidden(&s, &pw());
}

#[test]
fn a_failing_secret_store_is_not_fatal() {
    let _db = require_testdb!();
    let mut s = session_adjusted(|o| o.secrets = Some(Arc::new(BrokenStore)));
    let id = save(&mut s, pg14_config(), Some(pw()));
    assert_eq!(s.state().store.saved.len(), 1, "the connection is saved without its password");
    let notices = s.take_ui_requests();
    assert!(matches!(notices.as_slice(), [UiRequest::Notice(n)] if n.contains("secret store")), "{notices:?}");
    let e = run_err(&mut s, "connection.open", json!({ "id": id }));
    assert_eq!(e.code(), "needsPassword", "an unreadable store counts as no password: {e}");
    let r = open(&mut s, json!({ "id": id, "password": pw(), "savePassword": true }));
    assert_eq!(r["secretsSaved"], false, "{r}");
    let notices = s.take_ui_requests();
    assert!(notices.iter().any(|r| matches!(r, UiRequest::Notice(n) if n.contains("save the password"))), "{notices:?}");
    s.run_blocking("connection.close", json!({}), T).unwrap();
    s.run_blocking("connection.delete", json!({ "id": id }), T).unwrap();
    assert!(s.state().store.saved.is_empty());
}

#[test]
fn closing_while_a_refresh_runs_drops_its_result() {
    let _db = require_testdb!();
    let mut s = connected();
    let Outcome::Started(job) = s.execute("schema.refresh", json!({})).unwrap() else { panic!("refresh is a job") };
    s.run_blocking("connection.close", json!({}), T).unwrap();
    assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "a stale result is ignored, not an error");
    assert_eq!(s.inspect()["screen"], "connections");
    assert_eq!(s.inspect()["connection"], Json::Null);
}

#[test]
fn closing_while_reconnecting_drops_the_new_connection() {
    let _db = require_testdb!();
    let mut s = connected();
    let Outcome::Started(job) = s.execute("connection.reconnect", json!({})).unwrap() else { panic!("reconnect is a job") };
    assert_eq!(s.execute("connection.reconnect", json!({})).unwrap_err().code(), "busy");
    s.run_blocking("connection.close", json!({}), T).unwrap();
    assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "a stale result is ignored, not an error");
    assert_eq!(s.inspect()["connection"], Json::Null);
    let id = s.state().store.recent[0].config.id.clone();
    open(&mut s, json!({ "id": id }));
}

#[test]
fn a_failed_reconnect_after_close_is_ignored() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line)));
    open(&mut s, json!({ "config": via_ssh(SshAuth::Key { path: None }), "password": pw() }));
    // Without its key file the tunnel cannot log in again, so a reconnect fails.
    std::fs::remove_file(s.home.ssh_dir().join("id_ed25519")).unwrap();
    let e = run_err(&mut s, "connection.reconnect", json!({}));
    assert_eq!(e.code(), "ssh", "{e}");
    let Outcome::Started(job) = s.execute("connection.reconnect", json!({})).unwrap() else { panic!("reconnect is a job") };
    s.run_blocking("connection.close", json!({}), T).unwrap();
    assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "checked for staleness before its error");
}
