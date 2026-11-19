//! Connection commands against the Docker test servers (`cargo xtask testdb up`): save, list,
//! duplicate, delete, URL parsing, the SSH key preview, test, open, secret resolution and the
//! prompts, close, reconnect after a lost connection, and the schema refresh.
//!
//! Sessions come from `common` (temp home, `MemoryStore`, SSH files in the temp home, no agent).
//! The tests that reach a server run on every engine ([`engine_targets`]), but those of SSH and
//! TLS, whose subject is the tunnel or the certificate check. The tests that kill connections run
//! in an exclusive session (see `common`).

mod common;

use std::io::{Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use common::*;
use dbee_config::{ConnectionColor, ConnectionConfig, DbKind, SecretError, SecretKind, SecretStore, Settings, SshAuth, SshConfig, SslMode};
use dbee_driver::{Connection, DbError, DbFuture, Driver, Drivers, Target};
use dbee_engine::{EngineError, JobId, Json, Outcome, Session, UiRequest, Which};
use dbee_testkit::{TestTarget, require_testdb, temp_home, unique};
use serde_json::json;

/// Opening `config` with no password given and none saved.
fn open_err(config: ConnectionConfig) -> EngineError {
    let mut s = session();
    run_err(&mut s, "connection.open", json!({ "config": config }))
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
    assert_eq!(s.stored(&id, SecretKind::DbPassword).as_deref(), Some("pw-1234"));

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
    assert_eq!(s.stored(&dup_id, SecretKind::DbPassword).as_deref(), Some("pw-1234"), "the copy can connect too");

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
    assert_eq!(s.stored(&id, SecretKind::DbPassword).as_deref(), Some("first"));
    // SSH secrets are saved under their own kinds.
    s.run_blocking("connection.save", json!({ "config": again, "sshPassword": "sp", "sshPassphrase": "pp" }), T).unwrap();
    assert_eq!(s.stored(&id, SecretKind::SshPassword).as_deref(), Some("sp"));
    assert_eq!(s.stored(&id, SecretKind::SshPassphrase).as_deref(), Some("pp"));
    // `save_secrets: false` keeps none, and forgets the ones saved before.
    let off = ConnectionConfig { save_secrets: false, ..again };
    s.run_blocking("connection.save", json!({ "config": off, "password": "second" }), T).unwrap();
    for kind in [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase] {
        assert_eq!(s.secrets.get(&id, kind).unwrap(), None, "{kind:?}");
    }
    assert_secret_hidden(&s, "second");
}

/// The saved and recent ids in `s`'s connections file, each sorted.
fn ids_in_file(s: &TestSession) -> (Vec<String>, Vec<String>) {
    let text = std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap();
    let file: Json = serde_json::from_str(&text).unwrap();
    let ids = |list: &Json, at: &str| {
        let mut ids: Vec<String> = list.as_array().unwrap().iter().map(|e| e.pointer(at).and_then(Json::as_str).unwrap().to_owned()).collect();
        ids.sort();
        ids
    };
    (ids(&file["saved"], "/id"), ids(&file["recent"], "/config/id"))
}

#[test]
fn two_sessions_on_one_config_folder_keep_each_others_connections() {
    let mut a = session();
    let mut b = second_session(&a.home);
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    // Each saves a different connection.
    let x = save(&mut a, refused_config("x"), None);
    let y = save(&mut b, refused_config("y"), None);
    assert_eq!(ids_in_file(&a).0, sorted(vec![x.clone(), y.clone()]));
    // One deletes while the other saves.
    run(&mut a, "connection.delete", json!({ "id": x }));
    let z = save(&mut b, refused_config("z"), None);
    assert_eq!(ids_in_file(&a).0, sorted(vec![y.clone(), z.clone()]));
    // An open in one, which writes its recent list, keeps the other's save.
    let w = save(&mut a, refused_config("w"), None);
    let folder = Folder(dbee_testkit::repo_root().join("target").join("testkit").join(unique("two-sessions")));
    std::fs::create_dir_all(&folder.0).unwrap();
    let file = folder.0.join("empty.sqlite");
    std::fs::write(&file, b"").unwrap();
    let opened = open(&mut b, json!({ "config": { "kind": "sqlite", "file": file } }));
    let used = opened["connection"]["id"].as_str().unwrap().to_owned();
    assert_eq!(ids_in_file(&a), (sorted(vec![y, z, w.clone()]), vec![used]));
    // A session's list is the file as its last write left it: b's holds a's w.
    assert!(b.state().store.is_saved(&w));
    // Back on the connection screen, a session reads the file again.
    let v = save(&mut a, refused_config("v"), None);
    assert!(!b.state().store.is_saved(&v));
    run(&mut b, "connection.close", json!({}));
    assert!(b.state().store.is_saved(&v));
}

#[test]
fn a_config_key_this_build_does_not_know_is_refused() {
    let mut s = session();
    let mut typo = serde_json::to_value(refused_config("typo")).unwrap();
    typo["hots"] = json!("db.example");
    let tunnel = SshConfig { host: "b".into(), user: "u".into(), ..SshConfig::default() };
    let mut ssh_typo = serde_json::to_value(ConnectionConfig { ssh: Some(tunnel), ..refused_config("typo") }).unwrap();
    ssh_typo["ssh"]["jumpHost"] = json!("j");
    let mut auth_typo = ssh_typo.clone();
    auth_typo["ssh"].as_object_mut().unwrap().remove("jumpHost");
    auth_typo["ssh"]["auth"] = json!({ "method": "key", "paht": "/k/id" });
    for id in ["connection.save", "connection.test", "connection.open", "connection.listDatabases", "connection.sshKey"] {
        for (config, key) in [(&typo, "hots"), (&ssh_typo, "ssh.jumpHost"), (&auth_typo, "ssh.auth.paht")] {
            let e = s.execute(id, json!({ "config": config })).unwrap_err();
            assert_eq!(e.code(), "badParams", "{id}: {e}");
            assert!(e.to_string().ends_with(&format!(": `{key}` in `config` is not a connection setting")), "{id}: {e}");
        }
    }
    assert!(s.state().store.saved.is_empty() && !s.busy());
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
fn a_host_known_hosts_could_misread_is_bad_params() {
    let mut s = session();
    for host in ["a,b", "a b", "a\nb", "#a", "[a]"] {
        let ssh = json!({ "host": host, "port": 22, "user": "u", "auth": { "method": "password" } });
        for config in
            [json!({ "kind": "postgres", "host": "db", "port": 5432, "ssh": ssh }), json!({ "kind": "postgres", "host": host, "port": 5432 })]
        {
            for id in ["connection.open", "connection.test", "connection.save"] {
                let e = s.execute(id, json!({ "config": config })).unwrap_err();
                assert!(e.code() == "badParams" && e.to_string().contains("letters, digits"), "{id} {host:?}: {e}");
            }
        }
    }
}

/// A socket `.s.PGSQL.5432` in a folder of a throwaway home, bridged to pg14's TCP port: a Postgres
/// on a Unix socket, which no server of the test setup offers.
#[cfg(unix)]
fn socket_folder_for_pg14() -> dbee_testkit::TempHome {
    use std::os::unix::net::UnixListener;
    let home = temp_home(None, None);
    let listener = UnixListener::bind(home.home.join(".s.PGSQL.5432")).unwrap();
    let port = pg14().port;
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let Ok(client) = client else { return };
            let Ok(server) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return };
            let (Ok(mut a), Ok(b)) = (client.try_clone(), server.try_clone()) else { return };
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut a, &mut { b });
            });
            let (mut c, mut d) = (server, client);
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut c, &mut d);
            });
        }
    });
    home
}

#[cfg(unix)]
#[test]
fn a_postgres_socket_folder_is_a_host() {
    let _db = require_testdb!();
    let folder = socket_folder_for_pg14();
    let host = folder.home.display().to_string();
    let c = ConnectionConfig { id: "sock".into(), host: host.clone(), port: 5432, ssl_mode: SslMode::Disable, ..pg14_config() };
    let mut s = session();
    // The test, the save and the open go through the real socket.
    let r = s.run_blocking("connection.test", json!({ "config": c, "password": pw() }), T).unwrap();
    assert_eq!(r["via"], "direct", "{r}");
    assert_eq!(s.run_blocking("connection.save", json!({ "config": c, "password": pw() }), T).unwrap()["id"], "sock");
    // The saved connection, opened by its id.
    open(&mut s, json!({ "id": "sock" }));
    s.run_blocking("connection.close", json!({ "force": true }), T).unwrap();
    s.run_blocking("connection.test", json!({ "id": "sock" }), T).unwrap();
    // A folder is no host of a tunnelled connection.
    let tunnelled = ConnectionConfig { ssh: Some(ssh_config(SshAuth::Password)), ..c };
    let e = s.execute("connection.test", json!({ "config": tunnelled })).unwrap_err();
    assert_eq!(e.code(), "badParams", "{e}");
}

#[test]
fn parse_url_fills_the_form_and_is_not_journaled() {
    let mut s = session();
    let r = s.run_blocking("connection.parseUrl", json!({ "url": "postgresql://ann:s%40cret@db.example.com:6543/shop?sslmode=require" }), T).unwrap();
    assert_eq!(
        r,
        json!({ "kind": "postgres", "host": "db.example.com", "port": 6543, "user": "ann", "passwordGiven": true, "database": "shop", "sslMode": "require" })
    );
    let r = s.run_blocking("connection.parseUrl", json!({ "url": "postgres://localhost" }), T).unwrap();
    assert_eq!(
        r,
        json!({ "kind": "postgres", "host": "localhost", "port": null, "user": null, "passwordGiven": false, "database": null, "sslMode": null })
    );
    // The scheme names the database type.
    for (url, kind) in [("mysql://u@h:3307/shop", "mysql"), ("mariadb://h", "mariadb"), ("postgresql://h", "postgres")] {
        assert_eq!(s.run_blocking("connection.parseUrl", json!({ "url": url }), T).unwrap()["kind"], kind, "{url}");
    }
    assert_eq!(s.execute("connection.parseUrl", json!({ "url": "oracle://x" })).unwrap_err().code(), "badParams");
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
        ssh: Some(SshConfig { host: "127.0.0.1".into(), port: 1, user: "dbee".into(), auth, ..SshConfig::default() }),
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
    let _db = require_testdb!();
    let mut s = session();
    for t in engine_targets() {
        let r = s.run_blocking("connection.test", json!({ "config": config_for(&t), "password": pw() }), T).unwrap();
        let text = r["server"].as_str().unwrap();
        let version = t.server().map_or("", |s| s.version.as_str());
        assert!(text.starts_with(t.kind().info().product) && text.contains(version), "{}: {r}", t.name());
        // How it got there, where the type takes a tunnel.
        assert_eq!(r["via"], if kind_of(&t).takes_ssh() { json!("direct") } else { Json::Null }, "{}", t.name());
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
fn session_without_a_driver_says_so() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let mut s = session_adjusted(|o| o.drivers = Drivers::default());
        let id = save(&mut s, config_for(&t), Some(pw()));
        let want = format!("this build has no {} driver", kind_of(&t).label());
        for command in ["connection.test", "connection.open"] {
            let e = run_err(&mut s, command, json!({ "id": id }));
            assert!(matches!(e, EngineError::Disabled { ref why, .. } if *why == want), "{command}: {e}");
            assert!(!s.busy(), "{command} started a job");
        }
        assert_eq!(s.inspect()["kinds"], json!([]));
    }
}

#[test]
fn a_session_reports_the_kinds_it_can_open() {
    let s = session();
    let ids: Vec<&str> = dbee_drivers::all().kinds().into_iter().map(|k| k.id()).collect();
    for kind in ["postgres", "mysql", "mariadb"] {
        assert!(ids.contains(&kind), "{ids:?}");
    }
    assert_eq!(s.kinds(), dbee_drivers::all().kinds());
    assert_eq!(s.inspect()["kinds"], json!(ids));
}

#[test]
fn summary_names_product_and_kind() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let c = s.state().connection(Which::Active).unwrap();
        let j = c.summary(true);
        let info = t.kind().info();
        assert_eq!(j["kind"], info.id);
        let version = t.server().map_or("", |s| s.version.as_str());
        assert!(j["server"].as_str().unwrap().starts_with(&format!("{} {version}", info.product)), "{j}");
        assert_eq!((c.info.product.as_str(), json!(c.dialect)), (info.product, json!(info.dialect)), "{}", t.name());
    });
}

#[test]
fn an_isolated_run_connects_only_to_the_allowed_targets() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let Some(server) = t.server() else { continue };
        let params = json!({ "config": config_for(&t), "password": pw() });
        // One session at a time (see `common`): each is dropped before the next is made.
        {
            // The server is not on this list: refused before any job, so before any network I/O.
            let mut s = session_adjusted(|o| o.allowed_targets = Some(vec![("127.0.0.1".into(), 1)]));
            for id in ["connection.test", "connection.open"] {
                let e = s.execute(id, params.clone()).unwrap_err();
                assert_eq!(e.code(), "config", "{id}: {e}");
                assert!(e.to_string().contains("this isolated run may only connect to the test servers"), "{id}: {e}");
                assert!(!s.busy(), "{id} started a job");
            }
            assert_eq!(s.inspect()["screen"], "connections");
        }
        {
            // On the list: it connects.
            let mut s = session_adjusted(|o| o.allowed_targets = Some(vec![("127.0.0.1".into(), server.port)]));
            s.run_blocking("connection.test", params.clone(), T).unwrap();
            open(&mut s, params.clone());
        }
        // No list: no limit.
        let mut s = session();
        s.run_blocking("connection.test", params, T).unwrap();
    }
}

#[test]
fn open_saved_with_stored_password() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let mut s = session();
        let id = save(&mut s, config_for(&t), Some(pw()));
        s.run_blocking("connection.open", json!({ "id": id }), T).unwrap();
        let i = s.inspect();
        assert_eq!(i["screen"], "workspace");
        assert_eq!(i["connection"]["name"], t.name());
        assert_eq!(s.state().store.recent[0].config.id, id);
    }
}

#[test]
fn open_returns_the_connection_server_and_tables() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let mut s = session();
        let id = save(&mut s, config_for(&t), Some(pw()));
        let r = open(&mut s, json!({ "id": id }));
        assert_eq!(r["connection"]["id"], id);
        assert_eq!(r["connection"]["name"], t.name());
        assert!(r["server"].as_str().unwrap().starts_with(t.kind().info().product), "{r}");
        let tables = r["tables"].as_u64().unwrap();
        assert!(tables > 10, "the seed has the shop tables: {r}");
        let c = s.state().connection(Which::Active).unwrap();
        assert_eq!(c.catalog.table_count() as u64, tables);
        assert!(c.catalog.table(&dbee_core::TableRef { schema: t.schema("shop").into(), name: "orders".into() }).is_some(), "{}", t.name());
        assert!(!c.lost);
        // The recent list was written to the connections file.
        let file = std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap();
        assert!(file.contains("usedAt"), "{file}");
        // One connection per window.
        let e = s.execute("connection.open", json!({ "id": id })).unwrap_err();
        assert_eq!(e.code(), "disabled", "{e}");
    }
}

#[test]
fn missing_password_needs_password() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        assert_eq!(open_err(config_for(&t)).code(), "needsPassword", "{}", t.name());
    }
}

#[test]
fn wrong_password_is_an_auth_error() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        let mut s = session();
        let e = run_err(&mut s, "connection.open", json!({ "config": config_for(&t), "password": "not-the-password" }));
        assert_eq!(e.code(), "auth", "{}: {e}", t.name());
        assert_eq!(s.inspect()["screen"], "connections");
        assert_secret_hidden(&s, "not-the-password");
    }
}

#[test]
fn password_in_params_beats_the_stored_one() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        let mut s = session();
        let id = save(&mut s, config_for(&t), Some("stale".into()));
        let e = run_err(&mut s, "connection.open", json!({ "id": id }));
        assert_eq!(e.code(), "auth", "{}: the stored password is used: {e}", t.name());
        open(&mut s, json!({ "id": id, "password": pw() }));
        assert_eq!(s.stored(&id, SecretKind::DbPassword).as_deref(), Some("stale"), "not saved without savePassword");
    }
}

/// The password typed at an open is kept only for the settings the file holds when it is kept: a
/// second dbee that moved the connection to another host meanwhile wins.
#[test]
fn save_password_on_open_checks_the_settings_the_file_holds_then() {
    let _db = require_testdb!();
    let mut a = session();
    let mut b = second_session(&a.home);
    let id = save(&mut a, ConnectionConfig { save_secrets: false, ..pg14_config() }, None);
    let Outcome::Started(job) = a.execute("connection.open", json!({ "id": id, "password": pw(), "savePassword": true })).unwrap() else {
        panic!("a job")
    };
    // While it opens, the other dbee points the connection at another host name.
    save(&mut b, ConnectionConfig { id: id.clone(), host: "localhost".into(), save_secrets: false, ..pg14_config() }, None);
    let r = a.wait(job, T).unwrap();
    assert_eq!(
        (&r["secretsSaved"], &r["secretsNote"]),
        (&json!(false), &json!("the settings differ from the saved connection; save them to keep the secrets")),
        "{r}"
    );
    assert_eq!(a.secrets.get(&id, SecretKind::DbPassword).unwrap(), None, "no password kept for the other host");
    let file: Json = serde_json::from_str(&std::fs::read_to_string(a.home.home.join("config").join(dbee_config::store::FILE)).unwrap()).unwrap();
    assert_eq!((&file["saved"][0]["host"], &file["saved"][0]["saveSecrets"]), (&json!("localhost"), &json!(false)), "{file}");
}

/// While another dbee holds the connections file, the note is one plain clause: the UI shows it
/// after "The password was not saved:".
#[test]
fn save_password_on_open_while_another_dbee_holds_the_file() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, ConnectionConfig { save_secrets: false, ..pg14_config() }, None);
    let lock = s.home.home.join("config").join(dbee_config::store::LOCK_FILE);
    let other = std::fs::File::options().create(true).truncate(false).write(true).open(lock).unwrap();
    other.lock().unwrap();
    let r = open(&mut s, json!({ "id": id, "password": pw(), "savePassword": true }));
    other.unlock().unwrap();
    assert_eq!((&r["secretsSaved"], &r["secretsNote"]), (&json!(false), &json!("another dbee window was changing the connections file")), "{r}");
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap(), None);
}

#[test]
fn save_password_on_open_of_a_saved_connection_turns_saving_on() {
    let _db = require_testdb!();
    for t in engine_targets() {
        let mut s = session();
        let id = save(&mut s, ConnectionConfig { save_secrets: false, ..config_for(&t) }, Some(pw()));
        assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap(), None, "saveSecrets is off");
        let r = open(&mut s, json!({ "id": id, "password": pw(), "savePassword": true }));
        assert_eq!(r["secretsSaved"], true, "{r}");
        assert_eq!(s.stored(&id, SecretKind::DbPassword), Some(pw()));
        assert!(s.state().store.saved[0].save_secrets, "the saved flag agrees with the store");
        let file = std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap();
        let body: Json = serde_json::from_str(&file).unwrap();
        assert_eq!(body["saved"][0]["saveSecrets"], true, "{file}");
        s.run_blocking("connection.close", json!({}), T).unwrap();
        open(&mut s, json!({ "id": id }));
        assert_secret_hidden(&s, &pw());
    }
}

#[test]
fn save_password_on_open_of_an_ad_hoc_config_stores_nothing() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        let mut s = session();
        let r = open(&mut s, json!({ "config": config_for(&t), "password": pw(), "savePassword": true }));
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
}

/// `connection.open {save: true}` saves the connection once the open has succeeded, and a failed
/// open saves nothing.
#[test]
fn an_open_with_save_saves_only_when_it_succeeds() {
    let _db = require_testdb!();
    let mut s = session();
    let r = open(&mut s, json!({ "config": pg14_config(), "password": pw(), "save": true }));
    assert_eq!(r["connection"]["saved"], true, "{r}");
    let list = run(&mut s, "connection.list", json!({}));
    let saved = list["saved"].as_array().unwrap();
    assert_eq!(saved.len(), 1, "{list}");
    assert_eq!((&saved[0]["id"], &saved[0]["name"]), (&r["connection"]["id"], &json!("pg14")), "{list}");
    assert_eq!(s.inspect()["connection"]["saved"], true);
    assert!(r.get("savedNote").is_none(), "{r}");
    let file: Json = serde_json::from_str(&std::fs::read_to_string(s.home.home.join("config").join(dbee_config::store::FILE)).unwrap()).unwrap();
    assert_eq!(file["saved"][0]["id"], r["connection"]["id"], "{file}");

    let mut s = session();
    let e = run_err(&mut s, "connection.open", json!({ "config": pg14_config(), "password": "not-the-password", "save": true }));
    assert_eq!(e.code(), "auth", "{e}");
    assert_eq!(run(&mut s, "connection.list", json!({}))["saved"], json!([]));
}

/// A cancelled open with `save` saves nothing.
#[test]
fn a_cancelled_open_with_save_saves_nothing() {
    let _db = require_testdb!();
    let host = Host::new(Answer::Nothing);
    let mut s = session();
    let job = started(&mut s, "connection.open", json!({ "config": at(&pg14_target(), &host), "password": pw(), "save": true }));
    host.until("the connect", |seen| seen.taken == 1);
    assert_eq!(cancel(&mut s), json!({ "stopped": true }));
    ends_cancelled(&mut s, job, "the open");
    nothing_open(&s, "the open");
    assert_eq!(run(&mut s, "connection.list", json!({}))["saved"], json!([]));
}

/// While another dbee holds the connections file, the open stands and says the connection was not
/// saved.
#[test]
fn a_save_that_cannot_write_leaves_the_open_standing() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = session();
    let lock = s.home.home.join("config").join(dbee_config::store::LOCK_FILE);
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let other = std::fs::File::options().create(true).truncate(false).write(true).open(lock).unwrap();
    other.lock().unwrap();
    let r = open(&mut s, json!({ "config": config_for(&t), "save": true }));
    other.unlock().unwrap();
    assert_eq!(r["savedNote"], "another dbee window was changing the connections file", "{r}");
    assert_eq!(r["connection"]["saved"], false, "{r}");
    assert_eq!(s.inspect()["connection"]["saved"], false);
    assert!(s.state().store.saved.is_empty());
    let notices = notices(&mut s);
    assert!(notices.iter().any(|n| n == "The connection was not saved: another dbee window was changing the connections file"), "{notices:?}");
}

#[test]
fn open_file_refuses_save() {
    let mut s = session();
    let path = s.home.home.join("shop.sqlite");
    let e = s.execute("connection.openFile", json!({ "path": path, "save": true })).unwrap_err();
    assert_eq!(e.code(), "badParams", "{e}");
    assert!(e.to_string().contains("`save`"), "{e}");
}

/// Closes the connection, then opens `params`, expecting an error.
fn open_or_err_after_close(s: &mut TestSession, params: Json) -> EngineError {
    s.run_blocking("connection.close", json!({}), T).unwrap();
    run_err(s, "connection.open", params)
}

#[test]
fn stored_password_goes_only_to_its_own_server() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        let mut s = session();
        let saved = ConnectionConfig { id: "shop".into(), ..config_for(&t) };
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
}

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
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

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
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

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
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

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
#[test]
fn stored_passphrase_is_used() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(None, Some(&db.sshd.known_hosts_line)));
    let c = via_ssh(SshAuth::Key { path: Some(db.keys.encrypted.clone()) });
    let r = s.run_blocking("connection.save", json!({ "config": c, "password": pw(), "sshPassphrase": db.keys.passphrase }), T).unwrap();
    let id = r["id"].as_str().unwrap().to_owned();
    assert_eq!(s.stored(&id, SecretKind::SshPassphrase), Some(db.keys.passphrase.clone()));
    open(&mut s, json!({ "id": id }));
}

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
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

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
#[test]
fn test_through_ssh_reports_how_it_logged_in() {
    let db = require_testdb!();
    let mut s = session_in(temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line)));
    let r = s.run_blocking("connection.test", json!({ "config": via_ssh(SshAuth::Key { path: None }), "password": pw() }), T).unwrap();
    let key = s.home.ssh_dir().join("id_ed25519");
    assert_eq!(r["via"], format!("ssh key {}", key.display()), "{r}");
    assert_eq!(s.inspect()["screen"], "connections");
}

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
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
    for_each_engine(|_, s| {
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
    });
}

#[test]
fn blank_query_tabs_close_without_asking() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        s.execute("tab.newQuery", json!({ "text": " \n " })).unwrap();
        s.run_blocking("connection.close", json!({}), T).unwrap();
        assert_eq!(s.inspect()["screen"], "connections");
    });
}

#[test]
fn schema_refresh_reloads_and_keeps_tabs() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        s.execute("tab.newQuery", json!({ "text": "select 2" })).unwrap();
        let table = unique("t_refresh");
        let guard = scratch_table(t, &table, "CREATE TABLE {t} (id int)");
        let r = s.run_blocking("schema.refresh", json!({}), T);
        let scratch = dbee_core::TableRef { schema: t.schema("scratch").into(), name: table.clone() };
        let found = s.state().connection(Which::Active).unwrap().catalog.table(&scratch).is_some();
        drop(guard);
        let r = r.unwrap();
        assert!(found, "{}: the refreshed catalog has the new table", t.name());
        assert_eq!(r["tables"].as_u64().map(|n| n as usize), Some(s.state().connection(Which::Active).unwrap().catalog.table_count()));
        assert_eq!(s.inspect()["query"]["text"], "select 2");
        assert_eq!(s.state().tabs.len(), 1);
    });
}

#[test]
fn reconnect_after_connection_killed() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file cannot lose its connection.
        if t.is_file() {
            continue;
        }
        let mut s = exclusive_session();
        // The password is given only here, so the reconnect must use the one kept in memory.
        open(&mut s, json!({ "config": config_for(&t), "password": pw() }));
        s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
        kill_dbee_connections(&t, &s);

        let e = run_err(&mut s, "schema.refresh", json!({}));
        assert_eq!(e.code(), "connectionLost", "{}: {e}", t.name());
        assert_eq!(e.data()["action"], "connection.reconnect");
        assert_eq!(s.inspect()["connection"]["lost"], true);
        let e = s.execute("schema.refresh", json!({})).unwrap_err();
        assert_eq!(e.code(), "disabled");
        assert_eq!(e.data()["why"], "connection lost — reconnect");
        let refresh = s.commands().into_iter().find(|c| c.id == "schema.refresh").unwrap();
        assert_eq!(refresh.disabled_reason.as_deref(), Some("connection lost — reconnect"));

        let r = s.run_blocking("connection.reconnect", json!({}), T).unwrap();
        assert!(r["server"].as_str().unwrap().starts_with(t.kind().info().product), "{r}");
        assert_eq!(r["querySessionReset"], Json::Null, "no query connection was open: {r}");
        assert_eq!(s.inspect()["connection"]["lost"], false);
        assert_eq!(s.inspect()["screen"], "workspace");
        assert_eq!(s.inspect()["query"]["text"], "select 1");
        s.run_blocking("schema.refresh", json!({}), T).unwrap();
        assert_secret_hidden(&s, &pw());
    }
}

#[test]
fn reconnect_says_when_the_query_session_was_reset() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
        s.run_blocking("query.run", json!({}), T).unwrap();
        let r = s.run_blocking("connection.reconnect", json!({}), T).unwrap();
        assert_eq!(r["querySessionReset"], true, "{r}");
    });
}

#[test]
fn reconnect_through_ssh_after_connection_killed() {
    let db = require_testdb!();
    for t in engine_targets() {
        // A file is not behind an SSH server.
        let Some(server) = t.server() else { continue };
        let mut s = exclusive_session_in(temp_home(None, Some(&db.sshd.known_hosts_line)));
        let c = via_ssh_to(server, &config_for(&t), SshAuth::Key { path: Some(db.keys.encrypted.clone()) });
        open(&mut s, json!({ "config": c, "password": pw(), "sshPassphrase": db.keys.passphrase }));
        kill_dbee_connections(&t, &s);
        assert_eq!(run_err(&mut s, "schema.refresh", json!({})).code(), "connectionLost", "{}", t.name());
        // A new tunnel, with the passphrase kept in memory.
        s.run_blocking("connection.reconnect", json!({}), T).unwrap();
        assert_eq!(s.inspect()["connection"]["lost"], false);
        s.run_blocking("schema.refresh", json!({}), T).unwrap();
        assert_secret_hidden(&s, &db.keys.passphrase);
    }
}

#[test]
fn reconnect_while_connected_replaces_the_connection() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        let before = s.state().connection(Which::Active).unwrap().info.clone();
        s.run_blocking("connection.reconnect", json!({}), T).unwrap();
        assert_eq!(s.state().connection(Which::Active).unwrap().info, before);
        s.run_blocking("schema.refresh", json!({}), T).unwrap();
    });
}

#[test]
fn a_second_open_while_one_runs_is_busy() {
    let _db = require_testdb!();
    let targets = engine_targets();
    let file = targets.iter().find_map(TestTarget::file).expect("a file target");
    for t in &targets {
        let mut s = session();
        let id = save(&mut s, config_for(t), Some(pw()));
        let Outcome::Started(job) = s.execute("connection.open", json!({ "id": id })).unwrap() else { panic!("open is a job") };
        let e = s.execute("connection.open", json!({ "id": id })).unwrap_err();
        assert_eq!(e.code(), "busy", "{e}");
        // Opening a file is one more open: busy too.
        let e = s.execute("connection.openFile", json!({ "path": file })).unwrap_err();
        assert_eq!(e.code(), "busy", "{e}");
        s.wait(job, T).unwrap();
        assert_eq!(s.inspect()["screen"], "workspace");
    }
    // And the other way round.
    let mut s = session();
    let Outcome::Started(job) = s.execute("connection.openFile", json!({ "path": file })).unwrap() else { panic!("openFile is a job") };
    let e = s.execute("connection.open", json!({ "config": config_for(&targets[0]) })).unwrap_err();
    assert_eq!(e.code(), "busy", "{e}");
    s.wait(job, T).unwrap();
    assert_eq!(s.inspect()["screen"], "workspace");
}

#[test]
fn every_command_survives_empty_params_while_connected() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
        for c in s.commands() {
            if let Err(e @ EngineError::Internal { .. }) = s.execute(c.id, json!({})) {
                panic!("{}: {}: {e}", t.name(), c.id);
            }
            assert!(s.wait_idle(Duration::from_secs(30)), "{} left a job running", c.id);
            for (job, r) in s.pump() {
                if let Err(e @ EngineError::Internal { .. }) = r {
                    panic!("{}: {} job {}: {e}", t.name(), c.id, job.0);
                }
            }
        }
    });
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
        "connection.listDatabases",
        "connection.open",
        "connection.openFile",
        "connection.cancel",
    ] {
        assert!(get(id).enabled, "{id} runs on the connection screen");
    }
    for id in ["connection.reconnect", "schema.refresh", "connection.close", "connection.saveOpen"] {
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
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        let mut s = session_adjusted(|o| o.secrets = None);
        assert_eq!(s.inspect()["secretStore"], Json::Null);
        let id = save(&mut s, config_for(&t), Some(pw()));
        let e = run_err(&mut s, "connection.open", json!({ "id": id }));
        assert_eq!(e.code(), "needsPassword", "{e}");
        let r = open(&mut s, json!({ "id": id, "password": pw(), "savePassword": true }));
        assert_eq!(r["secretsSaved"], false, "{r}");
        assert!(r["secretsNote"].as_str().unwrap().contains("secret store"), "{r}");
        assert_eq!(s.take_ui_requests(), [], "no store, so nothing failed");
        assert_secret_hidden(&s, &pw());
    }
}

#[test]
fn a_failing_secret_store_is_not_fatal() {
    let _db = require_testdb!();
    for t in engine_targets() {
        // A file asks for no password.
        if t.is_file() {
            continue;
        }
        let mut s = session_adjusted(|o| o.secrets = Some(Arc::new(BrokenStore)));
        let id = save(&mut s, config_for(&t), Some(pw()));
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
}

#[test]
fn closing_while_a_refresh_runs_drops_its_result() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        let Outcome::Started(job) = s.execute("schema.refresh", json!({})).unwrap() else { panic!("refresh is a job") };
        s.run_blocking("connection.close", json!({}), T).unwrap();
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "a stale result is ignored, not an error");
        assert_eq!(s.inspect()["screen"], "connections");
        assert_eq!(s.inspect()["connection"], Json::Null);
    });
}

#[test]
fn closing_while_reconnecting_drops_the_new_connection() {
    let _db = require_testdb!();
    for_each_engine(|_, s| {
        let Outcome::Started(job) = s.execute("connection.reconnect", json!({})).unwrap() else { panic!("reconnect is a job") };
        assert_eq!(s.execute("connection.reconnect", json!({})).unwrap_err().code(), "busy");
        s.run_blocking("connection.close", json!({}), T).unwrap();
        assert_eq!(s.wait(job, T).unwrap(), json!({ "ignored": true }), "a stale result is ignored, not an error");
        assert_eq!(s.inspect()["connection"], Json::Null);
        let id = s.state().store.recent[0].config.id.clone();
        open(s, json!({ "id": id }));
    });
}

/// SSH logins and host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
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

#[test]
fn saving_new_settings_forgets_the_secrets_that_no_longer_apply() {
    let mut s = session();
    let key = |path: &str| SshAuth::Key { path: Some(path.into()) };
    let base = ConnectionConfig {
        id: "edit".into(),
        ssh: Some(SshConfig { host: "127.0.0.1".into(), port: 1, user: "dbee".into(), auth: key("/k/one"), ..SshConfig::default() }),
        ..refused_config("edit")
    };
    let stored = |s: &TestSession| {
        [SecretKind::DbPassword, SecretKind::SshPassword, SecretKind::SshPassphrase].map(|k| s.secrets.get("edit", k).unwrap().is_some())
    };
    s.run_blocking("connection.save", json!({ "config": base, "password": "db", "sshPassword": "sp", "sshPassphrase": "pp" }), T).unwrap();
    assert_eq!(stored(&s), [true, true, true]);
    // A new name, and another database on the same server and login, keep them all.
    let renamed = ConnectionConfig { name: "renamed".into(), database: "other".into(), ..base };
    save(&mut s, renamed.clone(), None);
    assert_eq!(stored(&s), [true, true, true]);
    // Another key file: the passphrase goes.
    let mut other_key = renamed;
    other_key.ssh.as_mut().unwrap().auth = key("/k/two");
    save(&mut s, other_key.clone(), None);
    assert_eq!(stored(&s), [true, true, false]);
    // Another database host: the password goes, unless it is given again.
    let moved = ConnectionConfig { host: "127.0.0.2".into(), ..other_key };
    save(&mut s, moved.clone(), None);
    assert_eq!(stored(&s), [false, true, false]);
    save(&mut s, moved.clone(), Some("db2".into()));
    assert_eq!(s.stored("edit", SecretKind::DbPassword).as_deref(), Some("db2"));
    // A weaker TLS mode: the password goes.
    save(&mut s, ConnectionConfig { ssl_mode: SslMode::Disable, ..moved.clone() }, None);
    assert_eq!(stored(&s), [false, true, false]);
    // Another SSH host is another tunnel: the database password and the SSH secrets go.
    save(&mut s, moved.clone(), Some("db3-three".into()));
    assert_eq!(stored(&s), [true, true, false]);
    let mut jump = moved;
    jump.ssh.as_mut().unwrap().host = "127.0.0.3".into();
    save(&mut s, jump, None);
    assert_eq!(stored(&s), [false, false, false]);
    assert_secret_hidden(&s, "db3-three");
}

/// No connection is made: the stored password's rules are the engine's, the same for every engine.
#[test]
fn edit_host_save_connect_does_not_send_the_old_password() {
    let _db = require_testdb!();
    let mut s = session();
    let id = save(&mut s, pg14_config(), Some(pw()));
    // The ordinary edit: another server, Save, then Connect.
    save(&mut s, ConnectionConfig { id: id.clone(), port: pgnew().port, ..pg14_config() }, None);
    assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap(), None, "the old server's password is forgotten");
    let e = run_err(&mut s, "connection.open", json!({ "id": id }));
    assert_eq!(e.code(), "needsPassword", "{e}");
}

/// TLS modes: verify-full against the test CA, on pg14 (rustls agrees no cipher with MySQL 5.7).
#[test]
fn a_weaker_tls_mode_does_not_get_the_stored_password() {
    let db = require_testdb!();
    let mut s = session();
    let strict = ConnectionConfig { id: "strict".into(), ssl_mode: SslMode::VerifyFull, ca_file: Some(db.ca_file.clone()), ..pg14_config() };
    save(&mut s, strict.clone(), Some(pw()));
    s.run_blocking("connection.test", json!({ "config": strict }), T).unwrap();
    let plain = ConnectionConfig { ssl_mode: SslMode::Disable, ..strict.clone() };
    let e = run_err(&mut s, "connection.test", json!({ "config": plain }));
    assert_eq!(e.code(), "needsPassword", "a downgrade must not get the stored password: {e}");
    s.run_blocking("connection.test", json!({ "config": strict }), T).unwrap();
}

/// The notices the session's UI requests hold, taken.
fn notices(s: &mut dbee_engine::Session) -> Vec<String> {
    s.take_ui_requests()
        .into_iter()
        .filter_map(|r| match r {
            UiRequest::Notice(n) => Some(n),
            _ => None,
        })
        .collect()
}

/// SSH host keys: the tunnel's own behaviour, the same for every engine, so on pg14 only.
#[cfg(unix)]
#[test]
fn known_hosts_that_cannot_be_read_or_written_say_so() {
    use std::os::unix::fs::PermissionsExt;
    let db = require_testdb!();
    let mut s = session_in(temp_home(Some(&db.keys.plain), None));
    let file = s.home.ssh_dir().join("known_hosts");
    let c = via_ssh(SshAuth::Key { path: None });
    // Readable but not writable: the trusted key connects, and a notice says it was not saved.
    std::fs::write(&file, "").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let e = run_err(&mut s, "connection.open", json!({ "config": c, "password": pw() }));
    assert_eq!(e.code(), "needsHostKey", "{e}");
    let fp = e.data()["fingerprint"].as_str().unwrap().to_owned();
    notices(&mut s);
    open(&mut s, json!({ "config": c, "password": pw(), "trustHostKey": fp }));
    let told = notices(&mut s);
    let host = sshd_entry();
    assert!(
        told.iter().any(|n| n.starts_with(&format!("The host key of {host} is trusted for this connection only: saving it to "))
            && n.contains("known_hosts failed")),
        "{told:?}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "", "nothing was written");
    s.run_blocking("connection.close", json!({}), T).unwrap();
    // Not readable: an error, not an empty file (a changed key would pass as unknown).
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&file).is_ok() {
        dbee_testkit::skipped("known_hosts_that_cannot_be_read: root reads anything");
        return;
    }
    let e = run_err(&mut s, "connection.open", json!({ "config": c, "password": pw(), "trustHostKey": fp }));
    assert_eq!((e.code(), e.data()["reason"].as_str()), ("ssh", Some("knownHosts")), "{e}");
    assert!(e.to_string().contains("known_hosts cannot be read"), "{e}");
}

/// A folder (or a file) of its own under `target/`, removed when dropped.
struct Folder(std::path::PathBuf);

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn isolated_runs_refuse_files_outside_allowed_dirs() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(dbee_testkit::TestTarget::is_file) {
        let file = t.file().unwrap().to_path_buf();
        let folder = file.parent().unwrap().to_path_buf();
        let other = Folder(dbee_testkit::repo_root().join("target").join("testkit").join("allowed").join(unique("other")));
        std::fs::create_dir_all(&other.0).unwrap();
        let params = |path: &std::path::Path| json!({ "config": ConnectionConfig { file: Some(path.to_path_buf()), ..config_for(&t) } });
        let mut refused = vec![
            std::path::PathBuf::from("/etc/hosts"),
            file.clone(),
            other.0.join("..").join("..").join(file.strip_prefix(other.0.parent().unwrap().parent().unwrap()).unwrap()),
        ];
        #[cfg(unix)]
        {
            // A link in an allowed folder to a file outside it is refused.
            std::os::unix::fs::symlink("/etc/hosts", other.0.join("hosts.sqlite")).unwrap();
            std::os::unix::fs::symlink(&file, other.0.join("seed.sqlite")).unwrap();
            refused.extend([other.0.join("hosts.sqlite"), other.0.join("seed.sqlite")]);
        }
        let missing = folder.join(format!("{}.sqlite", unique("never")));
        refused.push(missing.clone());
        {
            // Only `other` is allowed: the seed copy lies outside it. The driver refuses the file
            // inside its open, so the job fails, and nothing was opened or made.
            let mut s = session_adjusted(|o| o.allowed_dirs = Some(vec![other.0.clone()]));
            for path in &refused {
                for id in ["connection.test", "connection.open"] {
                    let e = run_err(&mut s, id, params(path));
                    assert_eq!(e.code(), "config", "{id} {}: {e}", path.display());
                    assert_eq!(e.to_string(), format!("this isolated run may only open files in its own folders; {} is not in them", path.display()));
                }
            }
            // Asked to make a file, it makes none outside them either, also behind a folder that
            // links out.
            let mut asked = vec![missing.clone()];
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&folder, other.0.join("out")).unwrap();
                asked.push(other.0.join("out").join(format!("{}.sqlite", unique("made"))));
            }
            for path in &asked {
                for id in ["connection.test", "connection.open"] {
                    let mut p = params(path);
                    p["createFile"] = json!(true);
                    let e = run_err(&mut s, id, p);
                    assert_eq!(e.code(), "config", "{id} {}: {e}", path.display());
                }
            }
            assert_eq!((&s.inspect()["screen"], &s.inspect()["connection"]), (&json!("connections"), &Json::Null));
            assert!(!missing.exists(), "no file was made");
            for path in &asked {
                let behind = folder.join(path.file_name().unwrap());
                assert!(!behind.exists(), "no file was made behind the link: {}", behind.display());
            }
        }
        {
            // The copy's folder allowed: it opens.
            let mut s = session_adjusted(|o| o.allowed_dirs = Some(vec![folder.clone()]));
            s.run_blocking("connection.test", params(&file), T).unwrap();
            open(&mut s, params(&file));
            assert_eq!(open_table(&mut s, t.schema("shop"), "customers")["rows"], 100);
        }
        #[cfg(unix)]
        {
            // The query connection, which opens on the first run, is checked as well: a link that
            // pointed inside when the browse connection opened, and out by then, is refused.
            let link = folder.join(format!("{}.sqlite", unique("link")));
            let _gone = Folder(link.clone());
            std::os::unix::fs::symlink(&file, &link).unwrap();
            let mut s = session_adjusted(|o| o.allowed_dirs = Some(vec![folder.clone()]));
            open(&mut s, params(&link));
            std::fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink("/etc/hosts", &link).unwrap();
            run(&mut s, "tab.newQuery", json!({ "text": "select 1" }));
            let e = run_err(&mut s, "query.run", json!({}));
            assert_eq!(e.code(), "config", "{e}");
            assert!(e.to_string().starts_with("this isolated run may only open files in its own folders; "), "{e}");
        }
        // No list: no limit.
        let mut s = session();
        open(&mut s, params(&file));
    }
}

#[test]
fn a_file_is_named_by_its_path_not_a_host() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(dbee_testkit::TestTarget::is_file) {
        let mut s = session();
        // No host, port, user or database: the type and the file are the settings.
        let file = json!({ "kind": kind_of(&t), "file": t.file().unwrap() });
        s.run_blocking("connection.test", json!({ "config": file }), T).unwrap();
        open(&mut s, json!({ "config": file }));
        s.run_blocking("connection.close", json!({}), T).unwrap();
        let e = run_err(&mut s, "connection.open", json!({ "config": { "kind": kind_of(&t) } }));
        assert_eq!(
            (e.code(), e.to_string().as_str()),
            ("badParams", "bad params for `connection.open`: `config`: invalid connection settings: no file chosen")
        );
        // A server's settings must still name its host.
        let e = run_err(&mut s, "connection.test", json!({ "config": { "port": 1 } }));
        assert!(e.to_string().contains("`config` must name its `host`"), "{e}");
    }
}

#[test]
fn an_isolated_run_opens_no_tunnel_for_a_file() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(dbee_testkit::TestTarget::is_file) {
        let folder = t.file().unwrap().parent().unwrap().to_path_buf();
        let mut s = session_adjusted(|o| {
            o.allowed_targets = Some(vec![("127.0.0.1".into(), 1)]);
            o.allowed_dirs = Some(vec![folder.clone()]);
        });
        // The file lies in an allowed folder, but the SSH server is no allowed target.
        let ssh = SshConfig { host: "jump.example".into(), port: 22, user: "u".into(), auth: SshAuth::Password, ..SshConfig::default() };
        let config = ConnectionConfig { ssh: Some(ssh), ..config_for(&t) };
        for id in ["connection.test", "connection.open", "connection.save"] {
            let e = s.execute(id, json!({ "config": config, "sshPassword": "pw" })).unwrap_err();
            assert_eq!(e.code(), "badParams", "{id}: {e}");
            assert!(e.to_string().contains("a database file has no SSH tunnel"), "{id}: {e}");
            assert!(!s.busy(), "{id} started a job");
        }
    }
}

#[test]
fn create_file_makes_a_missing_file_only_when_asked() {
    let folder = Folder(dbee_testkit::repo_root().join("target").join("testkit").join(unique("create-file")));
    std::fs::create_dir_all(&folder.0).unwrap();
    let mut s = session();
    for id in ["connection.test", "connection.open"] {
        let file = folder.0.join(format!("{}.sqlite", unique("new")));
        let config = json!({ "kind": "sqlite", "file": file });
        let e = run_err(&mut s, id, json!({ "config": config }));
        assert!(e.code() == "connect" && e.to_string().contains("no file at"), "{id}: {e}");
        assert!(!file.exists(), "{id}: nothing is made unasked");
        let e = s.execute(id, json!({ "config": config, "createFile": "yes" })).unwrap_err();
        assert_eq!(e.code(), "badParams", "{id}: {e}");
        // A read-only connection makes no file: refused before anything runs, in plain words.
        let read_only = json!({ "kind": "sqlite", "file": file, "readOnly": true });
        let e = s.execute(id, json!({ "config": read_only, "createFile": true })).unwrap_err();
        assert_eq!(e.code(), "badParams", "{id}: {e}");
        assert!(e.to_string().ends_with(": A read-only connection can't make a new file. Untick Read-only for the first connect."), "{id}: {e}");
        assert!(!file.exists() && !s.busy(), "{id}");
        s.run_blocking(id, json!({ "config": config, "createFile": true }), T).unwrap();
        assert!(file.is_file(), "{id}: an empty file, which SQLite takes as an empty database");
        if id == "connection.open" {
            assert_eq!(s.inspect()["screen"], "workspace");
            let journal = s.journal().last().cloned().unwrap();
            assert_eq!((journal.0.as_str(), &journal.1["createFile"]), ("connection.open", &json!(true)), "{journal:?}");
            s.run_blocking("connection.close", json!({}), T).unwrap();
        }
        // An existing file opens as it is, asked or not.
        s.run_blocking("connection.test", json!({ "config": config, "createFile": true }), T).unwrap();
        s.run_blocking("connection.test", json!({ "config": config }), T).unwrap();
    }
}

/// The file extensions of `t`'s database type.
fn extensions(t: &TestTarget) -> &'static [&'static str] {
    match kind_of(t).info().settings {
        Settings::File { extensions } => extensions,
        Settings::Server { .. } => panic!("{} is no file", t.name()),
    }
}

/// `connection.openFile` of `path`.
fn open_file(s: &mut Session, path: &Path) -> Json {
    run(s, "connection.openFile", json!({ "path": path }))
}

#[test]
fn open_file_is_unsaved_until_saved() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let file = t.file().unwrap();
        let mut s = session();
        let r = open_file(&mut s, file);
        assert_eq!(s.inspect()["screen"], "workspace");
        let c = &r["connection"];
        assert_eq!((&c["kind"], &c["saved"]), (&json!(kind_of(&t)), &json!(false)), "{r}");
        assert_eq!(c["name"], file.file_name().unwrap().to_str().unwrap(), "no name: the file's shows");
        assert_eq!(s.inspect()["connection"]["saved"], false);
        // Not saved; in Recent, as every open is.
        let list = run(&mut s, "connection.list", json!({}));
        assert_eq!(list["saved"], json!([]));
        let recent = &list["recent"][0]["config"];
        assert_eq!((&recent["file"], &recent["name"], &recent["id"]), (&json!(file), &json!(""), &c["id"]), "{list}");
        let saved = run(&mut s, "connection.saveOpen", json!({}));
        assert_eq!(saved["id"], c["id"]);
        assert_eq!(s.inspect()["connection"]["saved"], true);
        let list = run(&mut s, "connection.list", json!({}));
        assert_eq!(list["saved"].as_array().map(Vec::len), Some(1), "{list}");
        assert_eq!((&list["saved"][0]["id"], &list["saved"][0]["file"], &list["saved"][0]["kind"]), (&c["id"], &json!(file), &c["kind"]));
        // In the connections file too.
        let path = s.home.home.join("config").join(dbee_config::store::FILE);
        let written: Json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["saved"][0]["file"], json!(file));
        let journal: Vec<&str> = s.journal().iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(journal, ["connection.openFile", "connection.saveOpen"]);
        // Saved now: nothing left to save.
        let e = s.execute("connection.saveOpen", json!({})).unwrap_err();
        assert!(matches!(e, EngineError::Disabled { ref why, .. } if why == "the open connection is saved already"), "{e}");
    }
}

#[test]
fn open_file_opens_the_saved_connection_of_that_file() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let file = t.file().unwrap();
        let mut s = session();
        let config = ConnectionConfig { name: "Shop file".into(), color: ConnectionColor::Red, read_only: true, ..config_for(&t) };
        let id = save(&mut s, config, None);
        // Named another way: through `..`, and through a link.
        let folder = file.parent().unwrap();
        let roundabout = folder.join("..").join(folder.file_name().unwrap()).join(file.file_name().unwrap());
        let mut paths = vec![file.to_path_buf(), roundabout];
        #[cfg(unix)]
        let link = Folder(file.with_file_name(format!("{}.{}", unique("link"), extensions(&t)[0])));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(file, &link.0).unwrap();
            paths.push(link.0.clone());
        }
        for path in paths {
            let r = open_file(&mut s, &path);
            let c = &r["connection"];
            let shown = (&c["id"], &c["name"], &c["color"], &c["readOnly"], &c["saved"]);
            assert_eq!(shown, (&json!(id), &json!("Shop file"), &json!("red"), &json!(true), &json!(true)), "{}: {r}", path.display());
            // Read-only, as saved: a write is refused.
            run(&mut s, "tab.newQuery", json!({ "text": "CREATE TABLE never_made (a int)" }));
            assert_eq!(run_err(&mut s, "query.run", json!({})).code(), "readOnly", "{}", path.display());
            run(&mut s, "connection.close", json!({ "force": true }));
        }
        assert_eq!(run(&mut s, "connection.list", json!({}))["saved"].as_array().map(Vec::len), Some(1), "no copy was saved");
        // Another file: unsaved, writable, as before.
        let other = Folder(file.with_file_name(format!("{}.{}", unique("other"), extensions(&t)[0])));
        std::fs::copy(file, &other.0).unwrap();
        let c = open_file(&mut s, &other.0)["connection"].clone();
        assert_eq!(
            (&c["saved"], &c["readOnly"], &c["name"]),
            (&json!(false), &json!(false), &json!(other.0.file_name().unwrap().to_str().unwrap())),
            "{c}"
        );
        assert_ne!(c["id"], json!(id));
    }
}

#[test]
fn open_file_takes_no_ssh_secrets_and_keeps_no_password() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let mut s = session();
        for name in ["savePassword", "sshPassword", "sshPassphrase", "trustHostKey"] {
            let mut params = json!({ "path": t.file().unwrap() });
            params[name] = if name == "savePassword" { json!(true) } else { json!("x") };
            let e = s.execute("connection.openFile", params).unwrap_err();
            assert_eq!(e.code(), "badParams", "{name}: {e}");
            assert!(e.to_string().contains(&format!("`{name}` is not a param of connection.openFile")), "{e}");
        }
        assert!(!s.busy() && s.inspect()["screen"] == "connections");
    }
}

#[test]
fn open_file_finds_the_type_by_extension_in_any_case() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let file = t.file().unwrap();
        let mut s = session();
        for ext in extensions(&t) {
            let upper = Folder(file.with_extension(ext.to_uppercase()));
            std::fs::hard_link(file, &upper.0).unwrap();
            let r = open_file(&mut s, &upper.0);
            assert_eq!(r["connection"]["kind"], json!(kind_of(&t)), "{}", upper.0.display());
            run(&mut s, "connection.close", json!({}));
        }
        // Another extension, or none: refused at once, with the list, and nothing runs.
        for name in ["notes.txt", "notes"] {
            let other = Folder(file.with_file_name(format!("{}-{name}", unique("other"))));
            std::fs::hard_link(file, &other.0).unwrap();
            let e = s.execute("connection.openFile", json!({ "path": other.0 })).unwrap_err();
            assert_eq!(e.code(), "badParams", "{e}");
            let EngineError::BadParams { msg, .. } = &e else { panic!("{e}") };
            let shown = other.0.file_name().unwrap().to_str().unwrap();
            assert!(msg.starts_with(&format!("Can't open {shown}: dbee opens ")), "{msg}");
            assert!(extensions(&t).iter().all(|ext| msg.contains(&format!(".{ext}"))), "{msg}");
            let hint = format!("To open a file with another name, use New connection and choose {}", kind_of(&t).label());
            assert!(msg.contains(&hint), "{msg}");
            assert!(!s.busy() && s.inspect()["screen"] == "connections");
        }
        let e = s.execute("connection.openFile", json!({ "path": "" })).unwrap_err();
        assert_eq!(e.code(), "badParams", "{e}");
    }
}

#[test]
fn open_file_makes_no_file() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let ext = extensions(&t)[0];
        let folder = Folder(dbee_testkit::repo_root().join("target").join("testkit").join(unique("open-file")));
        std::fs::create_dir_all(&folder.0).unwrap();
        let mut s = session();
        let missing = folder.0.join(format!("gone.{ext}"));
        let e = run_err(&mut s, "connection.openFile", json!({ "path": missing }));
        assert_eq!(e.code(), "connect", "{e}");
        assert!(e.to_string().ends_with(&format!(": no file at {}", missing.display())), "{e}");
        assert!(!missing.exists(), "nothing is made");
        // A relative path is the current folder's, as a shell gives it; the error names it whole.
        let name = format!("{}.{ext}", unique("relative"));
        let e = run_err(&mut s, "connection.openFile", json!({ "path": name }));
        assert!(e.to_string().ends_with(&format!(": no file at {}", std::env::current_dir().unwrap().join(&name).display())), "{e}");
        assert!(!Path::new(&name).exists(), "nothing is made");
        // A folder with a database file's name is no file, with a trailing slash or without.
        let dir = folder.0.join(format!("a.{ext}"));
        std::fs::create_dir(&dir).unwrap();
        let slashed = |p: &Path| format!("{}/", p.display());
        for path in [dir.display().to_string(), slashed(&dir)] {
            let e = run_err(&mut s, "connection.openFile", json!({ "path": path }));
            assert_eq!(e.code(), "connect", "{e}");
            assert!(e.to_string().ends_with(&format!(": {path} is not a file")), "{e}");
        }
        // A trailing slash names a folder, also after a file's name or a missing one's: nothing opens
        // or is made.
        let file = folder.0.join(format!("b.{ext}"));
        std::fs::hard_link(t.file().unwrap(), &file).unwrap();
        for path in [slashed(&file), slashed(&missing)] {
            let e = run_err(&mut s, "connection.openFile", json!({ "path": path }));
            assert_eq!(e.code(), "connect", "{path}: {e}");
            assert!(e.to_string().ends_with(&format!(": {path} is not a file")), "{e}");
        }
        assert!(!missing.exists(), "nothing is made");
        assert_eq!(s.inspect()["screen"], "connections");
    }
}

#[test]
fn open_file_in_an_isolated_run_stays_in_its_folders() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let file = t.file().unwrap().to_path_buf();
        let folder = file.parent().unwrap().to_path_buf();
        let other = Folder(dbee_testkit::repo_root().join("target").join("testkit").join("allowed").join(unique("other")));
        std::fs::create_dir_all(&other.0).unwrap();
        let missing = folder.join(format!("{}.{}", unique("never"), extensions(&t)[0]));
        {
            // Only `other` is allowed: the driver refuses the file inside its open, and makes none.
            let mut s = session_adjusted(|o| o.allowed_dirs = Some(vec![other.0.clone()]));
            for path in [&file, &missing] {
                let e = run_err(&mut s, "connection.openFile", json!({ "path": path }));
                assert_eq!(e.code(), "config", "{}: {e}", path.display());
                assert_eq!(e.to_string(), format!("this isolated run may only open files in its own folders; {} is not in them", path.display()));
            }
            assert!(!missing.exists(), "no file was made");
            assert_eq!((&s.inspect()["screen"], &s.inspect()["connection"]), (&json!("connections"), &Json::Null));
        }
        // Its own folder: it opens.
        let mut s = session_adjusted(|o| o.allowed_dirs = Some(vec![folder.clone()]));
        open_file(&mut s, &file);
        assert_eq!(s.inspect()["screen"], "workspace");
    }
}

#[test]
fn save_open_keeps_the_password_as_the_forms_save_does() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(|t| !t.is_file()) {
        // Saving passwords on (the default): the password the connection was opened with is kept, and
        // the next open asks for none.
        let mut s = session();
        open(&mut s, json!({ "config": config_for(&t), "password": pw() }));
        let id = run(&mut s, "connection.saveOpen", json!({}))["id"].as_str().unwrap().to_owned();
        assert_eq!(s.stored(&id, SecretKind::DbPassword), Some(pw()), "{}", t.name());
        assert_secret_hidden(&s, &pw());
        run(&mut s, "connection.close", json!({}));
        open(&mut s, json!({ "id": id }));
        // Off: no password is kept, so the next open asks.
        // One session at a time: two on one thread can deadlock on the tests' lock.
        drop(s);
        let mut s = session();
        open(&mut s, json!({ "config": ConnectionConfig { save_secrets: false, ..config_for(&t) }, "password": pw() }));
        let id = run(&mut s, "connection.saveOpen", json!({}))["id"].as_str().unwrap().to_owned();
        assert_eq!(s.secrets.get(&id, SecretKind::DbPassword).unwrap(), None, "{}", t.name());
        run(&mut s, "connection.close", json!({}));
        assert_eq!(run_err(&mut s, "connection.open", json!({ "id": id })).code(), "needsPassword", "{}", t.name());
    }
}

#[test]
fn save_open_needs_an_unsaved_connection() {
    let _db = require_testdb!();
    let mut s = session();
    let e = s.execute("connection.saveOpen", json!({})).unwrap_err();
    assert!(matches!(e, EngineError::Disabled { ref why, .. } if why == "not connected"), "{e}");
    drop(s);
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        // Opened from the saved list: saved already.
        let mut s = connected_to(&t);
        assert_eq!(s.inspect()["connection"]["saved"], true);
        let before = run(&mut s, "connection.list", json!({}));
        let e = s.execute("connection.saveOpen", json!({})).unwrap_err();
        assert!(matches!(e, EngineError::Disabled { ref why, .. } if why == "the open connection is saved already"), "{e}");
        assert_eq!(run(&mut s, "connection.list", json!({})), before);
    }
}

#[test]
fn open_file_while_connected_is_refused_as_open_is() {
    let _db = require_testdb!();
    for t in engine_targets().into_iter().filter(TestTarget::is_file) {
        let file = t.file().unwrap();
        let mut s = connected_to(&t);
        open_table(&mut s, t.schema("shop"), "customers");
        set_cell(&mut s, 0, "name", "staged");
        let before = s.inspect();
        let by_open = s.execute("connection.open", json!({ "config": config_for(&t) })).unwrap_err();
        let by_file = s.execute("connection.openFile", json!({ "path": file })).unwrap_err();
        for e in [&by_open, &by_file] {
            assert!(matches!(e, EngineError::Disabled { why, .. } if why == "already connected; disconnect first"), "{e}");
        }
        assert_eq!(s.inspect(), before, "the connection and its staged edit stay");
        assert_eq!(summary(&s)["edits"]["count"], 1, "{}", summary(&s));
    }
}

// ---- the database list -----------------------------------------------------------------------

const LIST: &str = "connection.listDatabases";

/// The database the seed is in on `t`'s server: the kind's test database or, with none (MySQL's
/// servers, whose databases are the seed's schemas), `shop`.
fn seeded_database(t: &TestTarget) -> &'static str {
    let info = t.kind().info();
    if info.database.is_empty() { t.schema("shop") } else { info.database }
}

/// The names of a `connection.listDatabases` result.
fn names(r: &Json) -> Vec<&str> {
    r["databases"].as_array().unwrap_or_else(|| panic!("no list: {r}")).iter().map(|d| d.as_str().unwrap()).collect()
}

#[test]
fn list_databases_for_an_unsaved_form() {
    let _db = require_testdb!();
    let mut s = session();
    let r = s.run_blocking(LIST, json!({ "config": pg14_config(), "password": pw() }), T).unwrap();
    assert!(r["databases"].as_array().unwrap().iter().any(|d| d == "dbee"), "{r}");
    // Every server lists the database its seed is in, whatever the field holds.
    for t in engine_targets().into_iter().filter(|t| !t.is_file()) {
        let config = ConnectionConfig { database: String::new(), ..config_for(&t) };
        let r = s.run_blocking(LIST, json!({ "config": config, "password": pw() }), T).unwrap();
        assert!(names(&r).contains(&seeded_database(&t)), "{}: {r}", t.name());
        // Without a password, the server's refusal asks for one, as Test's does.
        assert_eq!(run_err(&mut s, LIST, json!({ "config": config })).code(), "needsPassword", "{}", t.name());
    }
    // It leaves the session as it was: nothing opened, nothing used, nothing journaled (its params
    // can carry a password).
    assert_eq!(s.inspect()["screen"], "connections");
    assert_eq!(s.inspect()["connection"], Json::Null);
    assert!(s.state().store.recent.is_empty() && s.state().store.saved.is_empty());
    assert!(s.journal().iter().all(|(id, _)| id != LIST), "{:?}", s.journal());
    assert!(!s.busy());
    assert_secret_hidden(&s, &pw());
}

#[test]
fn list_databases_respects_allowed_targets() {
    let _db = require_testdb!();
    let params = json!({ "config": pg14_config(), "password": pw() });
    {
        // Not on the list: refused before any job, as connection.test is.
        let mut s = session_adjusted(|o| o.allowed_targets = Some(vec![("127.0.0.1".into(), 1)]));
        let e = s.execute(LIST, params.clone()).unwrap_err();
        assert_eq!(e.code(), "config", "{e}");
        assert!(e.to_string().contains("this isolated run may only connect to the test servers"), "{e}");
        assert!(!s.busy());
    }
    let mut s = session_adjusted(|o| o.allowed_targets = Some(vec![("127.0.0.1".into(), pg14().port)]));
    assert!(names(&s.run_blocking(LIST, params, T).unwrap()).contains(&"dbee"));
    drop(s);
    // Through a tunnel, the SSH server's target counts, not the database host behind it (`pg14:5432`,
    // which the sshd resolves).
    let db = db();
    let config = via_ssh(SshAuth::Key { path: None });
    let tunnelled = json!({ "config": config, "password": pw() });
    let home = || temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    {
        let behind = (config.host.clone(), config.port);
        let mut s = session_in_adjusted(home(), |o| o.allowed_targets = Some(vec![behind]));
        let e = s.execute(LIST, tunnelled.clone()).unwrap_err();
        assert!(e.code() == "config" && e.to_string().contains("the SSH server"), "{e}");
        assert!(!s.busy());
    }
    let mut s = session_in_adjusted(home(), |o| o.allowed_targets = Some(vec![("127.0.0.1".into(), db.sshd.port)]));
    assert!(names(&s.run_blocking(LIST, tunnelled, T).unwrap()).contains(&"dbee"), "only the SSH server is allowed, and it lists");
}

/// The Database field may hold anything when the list is asked for: Postgres still lists, through
/// the server's default database.
#[test]
fn a_half_typed_database_still_lists() {
    let _db = require_testdb!();
    let mut s = session();
    for server in [pg14(), pgnew()] {
        for database in ["sh", "no_such_database"] {
            let config = ConnectionConfig { database: database.into(), ..server_config(server) };
            let r = s.run_blocking(LIST, json!({ "config": config, "password": pw() }), T).unwrap();
            assert!(names(&r).contains(&"dbee") && names(&r).contains(&"postgres"), "{} {database:?}: {r}", server.name);
        }
    }
}

#[test]
fn a_file_type_has_no_database_list() {
    let mut s = session();
    for kind in dbee_config::DbKind::ALL.into_iter().filter(|k| matches!(k.info().settings, Settings::File { .. })) {
        let with_file = ConnectionConfig { kind, file: Some("/nowhere/shop.db".into()), host: String::new(), port: 0, ..ConnectionConfig::default() };
        let without = ConnectionConfig { file: None, ..with_file.clone() };
        for config in [with_file, without] {
            let e = s.execute(LIST, json!({ "config": config })).unwrap_err();
            assert_eq!(e.code(), "badParams", "{e}");
            let want = format!("{} opens one database file, so there are no databases to list.", kind.label());
            assert!(matches!(&e, EngineError::BadParams { msg, .. } if *msg == want), "{e}");
        }
    }
    assert!(!s.busy());
    for params in [json!({}), json!({ "config": {} }), json!({ "config": refused_config("x"), "password": 5 })] {
        assert_eq!(s.execute(LIST, params.clone()).unwrap_err().code(), "badParams", "{params}");
    }
    assert_eq!(run_err(&mut s, LIST, json!({ "config": refused_config("refused") })).code(), "connect");
}

/// Through the test sshd to a MySQL server, which it reaches as `mysql80:3306`.
#[test]
fn list_databases_through_ssh() {
    let db = require_testdb!();
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let server = mysql.server().unwrap();
    let mut s = session_in(temp_home(Some(&db.keys.plain), None));
    let config = via_ssh_to(server, &config_for(&mysql), SshAuth::Key { path: None });
    // An unknown host key asks first, and the trusted one is learned.
    let e = run_err(&mut s, LIST, json!({ "config": config, "password": pw() }));
    assert_eq!(e.code(), "needsHostKey", "{e}");
    let trust = e.data()["fingerprint"].as_str().unwrap().to_owned();
    let r = s.run_blocking(LIST, json!({ "config": config, "password": pw(), "trustHostKey": trust }), T).unwrap();
    assert!(names(&r).contains(&"shop") && names(&r).last() == Some(&"sys"), "{r}");
    assert!(std::fs::read_to_string(s.home.ssh_dir().join("known_hosts")).unwrap().contains(&sshd_entry()));
}

/// The list opens a connection of its own: it works while connected to another server, and the
/// open connection, its tabs and the lists stay as they were. A saved connection's stored password
/// is used as Test uses it.
#[test]
fn listing_leaves_the_session_as_it_is() {
    let db = require_testdb!();
    let mut s = connected();
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    let before = (s.inspect()["connection"].clone(), s.inspect()["tabs"].clone(), s.state().store.clone());
    let catalog = Arc::clone(&s.state().connection(Which::Active).unwrap().catalog);
    let mysql = dbee_testkit::target(db.server("mysql80").unwrap());
    let r = s.run_blocking(LIST, json!({ "config": config_for(&mysql), "password": pw() }), T).unwrap();
    assert!(names(&r).contains(&"shop"), "{r}");
    assert_eq!((s.inspect()["connection"].clone(), s.inspect()["tabs"].clone(), s.state().store.clone()), before);
    let c = s.state().connection(Which::Active).unwrap();
    assert!(Arc::ptr_eq(&catalog, &c.catalog) && !c.lost, "the open connection is as it was");
    assert_eq!(run(&mut s, "query.run", json!({}))["results"].as_array().map(Vec::len), Some(1), "the query tab still runs");
    assert_eq!(s.inspect()["screen"], "workspace");
    // The saved pg14's id: its stored password.
    let saved = s.state().store.saved[0].clone();
    assert!(names(&s.run_blocking(LIST, json!({ "config": saved }), T).unwrap()).contains(&"dbee"));
    // Another host under that id gets no password, so it asks for one.
    let elsewhere = ConnectionConfig { port: pgnew().port, ..saved };
    assert_eq!(run_err(&mut s, LIST, json!({ "config": elsewhere })).code(), "needsPassword");
}

// ---- open connections by id ------------------------------------------------------------------

/// The open connection's id, as `inspect()` shows it (`connection.connection`).
fn open_id(s: &Session) -> Json {
    s.inspect()["connection"]["connection"].clone()
}

/// The SQLite seed copy: what these tests check is the same on every engine.
fn file_target() -> TestTarget {
    engine_targets().into_iter().find(TestTarget::is_file).expect("a file target")
}

/// The `connection` of every open tab, as `inspect()` and `tab.list` show them.
fn tab_connections(s: &mut Session) -> (Vec<Json>, Vec<Json>) {
    let of = |tabs: &Json| tabs.as_array().map_or_else(Vec::new, |tabs| tabs.iter().map(|t| t["connection"].clone()).collect());
    let inspected = of(&s.inspect()["tabs"]);
    (inspected, of(&run(s, "tab.list", json!({}))["tabs"]))
}

#[test]
fn the_open_connection_has_an_id_that_its_tabs_name() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = connected_to(&t);
    let i = s.inspect();
    let id = open_id(&s);
    assert!(id.is_u64(), "an id: {i}");
    assert_eq!(i["connections"], json!([i["connection"].clone()]), "one open connection: {i}");
    assert!(i["connection"]["id"].is_string(), "`id` stays the saved connection's: {i}");
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    open_table(&mut s, t.schema("shop"), "customers");
    s.execute("tab.newQuery", json!({ "connection": id })).unwrap();
    run(&mut s, "table.open", json!({ "schema": t.schema("shop"), "table": "orders", "connection": id }));
    assert_eq!(tab_connections(&mut s), (vec![id.clone(); 4], vec![id.clone(); 4]));
    // Named or left out, a command means the same connection.
    for (command, params) in [("schema.inspect", json!({ "schema": t.schema("shop") })), ("quickSearch.find", json!({ "query": "cust" }))] {
        let mut named = params.clone();
        named["connection"] = id.clone();
        assert_eq!(run(&mut s, command, named), run(&mut s, command, params), "{command}");
    }
    // What concerns the connection names it.
    assert_eq!(run(&mut s, "schema.refresh", json!({}))["connection"], id);
    assert_eq!(run(&mut s, "schema.refresh", json!({ "connection": id }))["connection"], id);
}

#[test]
fn connection_close_takes_the_connection_by_its_id() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = connected_to(&t);
    let id = open_id(&s);
    assert!(id.is_u64(), "{}", s.inspect());
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    open_table(&mut s, t.schema("shop"), "customers");
    let r = run(&mut s, "connection.close", json!({ "connection": id, "force": true }));
    assert_eq!(r["connection"], id, "{r}");
    let i = s.inspect();
    let after = (i["connection"].clone(), i["connections"].clone(), i["tabs"].clone(), i["screen"].clone());
    assert_eq!(after, (Json::Null, json!([]), json!([]), json!("connections")), "its tabs closed with it: {i}");
}

#[test]
fn a_connection_that_is_not_open_is_refused() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = session();
    // Not saved, so `connection.saveOpen` can run.
    open(&mut s, json!({ "config": config_for(&t) }));
    let id = open_id(&s);
    assert!(id.is_u64(), "{}", s.inspect());
    let before = s.inspect();
    let commands = [
        ("connection.close", json!({ "force": true })),
        ("connection.reconnect", json!({})),
        ("connection.saveOpen", json!({})),
        ("schema.refresh", json!({})),
        ("schema.inspect", json!({})),
        ("quickSearch.find", json!({ "query": "cust" })),
        ("table.open", json!({ "schema": t.schema("shop"), "table": "customers" })),
        ("tab.newQuery", json!({})),
    ];
    for (command, params) in &commands {
        for (given, why) in [
            (json!(999), "no connection 999 is open"),
            (json!(0), "no connection 0 is open"),
            (json!("1"), "`connection` must be a whole number, 0 or more"),
            (json!(-1), "`connection` must be a whole number, 0 or more"),
        ] {
            let mut p = params.clone();
            p["connection"] = given.clone();
            let e = s.execute(command, p).unwrap_err();
            assert!(matches!(&e, EngineError::BadParams { msg, .. } if msg == why), "{command} {given}: {e}");
        }
    }
    assert_eq!(s.inspect(), before, "nothing changed");
    let r = run(&mut s, "connection.saveOpen", json!({ "connection": id }));
    assert_eq!(r["connection"], id, "{r}");
}

#[test]
fn a_reconnect_keeps_the_id_and_the_tabs_on_it() {
    let _db = require_testdb!();
    for_each_engine(|t, s| {
        let id = open_id(s);
        assert!(id.is_u64(), "{}: {}", t.name(), s.inspect());
        let query = run(s, "tab.newQuery", json!({ "text": "select 1" }))["tab"].clone();
        open_table(s, t.schema("shop"), "customers");
        let r = run(s, "connection.reconnect", json!({ "connection": id }));
        assert_eq!(r["connection"]["connection"], id, "{}: the id stays: {r}", t.name());
        assert_eq!(s.inspect()["connections"].as_array().map(Vec::len), Some(1));
        assert_eq!(tab_connections(s), (vec![id.clone(); 2], vec![id.clone(); 2]), "{}", t.name());
        // Both tabs run on the new connection.
        run(s, "table.refresh", json!({}));
        run(s, "tab.activate", json!({ "tab": query }));
        run(s, "query.run", json!({}));
    });
}

#[test]
fn every_open_gets_a_new_id_and_a_second_is_still_refused() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = session();
    open(&mut s, json!({ "config": config_for(&t) }));
    let first = open_id(&s);
    assert!(first.is_u64(), "{}", s.inspect());
    let before = s.inspect();
    let e = s.execute("connection.open", json!({ "config": config_for(&t) })).unwrap_err();
    assert!(matches!(&e, EngineError::Disabled { why, .. } if why == "already connected; disconnect first"), "{e}");
    assert_eq!(s.inspect(), before, "the open connection stays, with its id");
    run(&mut s, "connection.close", json!({}));
    open(&mut s, json!({ "config": config_for(&t) }));
    let second = open_id(&s);
    assert!(second.is_u64() && second != first, "{first}, then {second}");
}

#[test]
fn a_query_tab_opened_before_connecting_joins_the_connection() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = session();
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    assert_eq!(tab_connections(&mut s), (vec![Json::Null], vec![Json::Null]), "no connection yet");
    open(&mut s, json!({ "config": config_for(&t) }));
    let id = open_id(&s);
    assert!(id.is_u64(), "{}", s.inspect());
    assert_eq!(tab_connections(&mut s), (vec![id.clone()], vec![id]));
    run(&mut s, "query.run", json!({}));
}

#[test]
fn the_journal_keeps_a_connection_param_only_where_it_was_given() {
    let _db = require_testdb!();
    let t = file_target();
    let mut s = session();
    open(&mut s, json!({ "config": config_for(&t) }));
    let id = open_id(&s);
    assert!(id.is_u64(), "{}", s.inspect());
    run(&mut s, "schema.refresh", json!({}));
    run(&mut s, "schema.refresh", json!({ "connection": id }));
    let journal = s.journal().to_vec();
    let tail: Vec<(String, Json)> = journal.iter().rev().take(2).rev().cloned().collect();
    assert_eq!(tail, [("schema.refresh".to_owned(), json!({})), ("schema.refresh".to_owned(), json!({ "connection": id }))]);
    // A replay in a new session: ids count from the session's start, so the one given names the
    // replay's connection too, and the entry without one takes the active tab's.
    drop(s);
    let mut replay = session();
    for (command, params) in journal {
        run(&mut replay, &command, params);
    }
    assert_eq!(open_id(&replay), id);
}

// ---- cancelling a connection attempt ---------------------------------------------------------

/// What a [`Host`] does with each connection it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// Nothing: it reads what comes and never answers, as a host that hangs.
    Nothing,
    /// An SSH server's first line, then nothing: the client's key exchange never ends.
    SshFirstLine,
    /// Everything, both ways, to and from this port on 127.0.0.1.
    Forward(u16),
    /// Postgres that logs the client in and answers the first `answered` queries of dbee's setup
    /// ([`setup_answers`]), then reads what comes and never answers: a server that hangs after the
    /// login, or after the setup.
    Pg { answered: usize },
}

/// What a [`Host`] saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Seen {
    /// The connections it took.
    taken: usize,
    /// Those whose other end closed them.
    closed: usize,
    /// Those that sent more than one line: an SSH client's key exchange, after its first line.
    past_first_line: usize,
    /// The requests a Postgres client sent after its login (its goodbye left out).
    requests: usize,
}

/// What a test saw, and a way to wait until it saw something: no wait is on a timer, and the 30 s
/// bound is only for a test that fails.
struct Watch<S>(Mutex<S>, Condvar);

impl<S: Default + std::fmt::Debug> Watch<S> {
    fn new() -> Arc<Watch<S>> {
        Arc::new(Watch(Mutex::new(S::default()), Condvar::new()))
    }

    fn note(&self, f: impl FnOnce(&mut S)) {
        f(&mut self.0.lock().unwrap());
        self.1.notify_all();
    }

    fn until(&self, what: &str, done: impl Fn(&S) -> bool) {
        let deadline = Instant::now() + T;
        let mut seen = self.0.lock().unwrap();
        while !done(&seen) {
            let left = deadline.checked_duration_since(Instant::now()).unwrap_or_else(|| panic!("gave up waiting for {what}: {seen:?}"));
            seen = self.1.wait_timeout(seen, left).unwrap().0;
        }
    }
}

/// A host on 127.0.0.1 that the test opens itself: it answers each connection as [`Answer`] said
/// when it took it, and tells what it saw.
struct Host {
    port: u16,
    answer: Arc<Mutex<Answer>>,
    seen: Arc<Watch<Seen>>,
}

impl Host {
    fn new(answer: Answer) -> Host {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = Host { port: listener.local_addr().unwrap().port(), answer: Arc::new(Mutex::new(answer)), seen: Watch::new() };
        let (answer, seen) = (Arc::clone(&host.answer), Arc::clone(&host.seen));
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let answer = *answer.lock().unwrap();
                seen.note(|s| s.taken += 1);
                let seen = Arc::clone(&seen);
                std::thread::spawn(move || {
                    serve(conn, answer, &seen);
                    seen.note(|s| s.closed += 1);
                });
            }
        });
        host
    }

    fn set(&self, answer: Answer) {
        *self.answer.lock().unwrap() = answer;
    }

    fn until(&self, what: &str, done: impl Fn(&Seen) -> bool) {
        self.seen.until(what, done);
    }

    fn seen(&self) -> Seen {
        *self.seen.0.lock().unwrap()
    }
}

/// Serves one connection as `answer` says, until its other end closes it.
fn serve(mut conn: TcpStream, answer: Answer, seen: &Watch<Seen>) {
    let mut upstream = match answer {
        Answer::Forward(port) => {
            let up = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let (mut from, mut to) = (up.try_clone().unwrap(), conn.try_clone().unwrap());
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut from, &mut to);
                let _ = to.shutdown(Shutdown::Write);
            });
            Some(up)
        }
        Answer::SshFirstLine => {
            conn.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").unwrap();
            None
        }
        Answer::Pg { answered } => return serve_pg(conn, answered, seen),
        Answer::Nothing => None,
    };
    let (mut buf, mut first_line_ended, mut past) = ([0u8; 16 * 1024], false, false);
    loop {
        let n = match conn.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if let Some(up) = upstream.as_mut() {
            // A write that fails (the server has closed) is not this end closing.
            let _ = up.write_all(&buf[..n]);
        }
        if !past {
            let newline = buf[..n].iter().position(|b| *b == b'\n');
            past = first_line_ended || newline.is_some_and(|at| at + 1 < n);
            first_line_ended |= newline.is_some();
            if past {
                seen.note(|s| s.past_first_line += 1);
            }
        }
    }
    if let Some(up) = upstream {
        let _ = up.shutdown(Shutdown::Write);
    }
}

/// One Postgres message: its kind and its body, with the length that counts itself.
fn message(kind: u8, body: &[u8]) -> Vec<u8> {
    let len = i32::try_from(body.len() + 4).unwrap();
    [&[kind][..], &len.to_be_bytes(), body].concat()
}

/// A query's answer of one text column named `x`: a row with each of `values`, each ended by
/// `tag`.
fn rows(values: &[&str], tag: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        let field = [
            &b"x\0"[..],
            &0i32.to_be_bytes(),
            &0i16.to_be_bytes(),
            &25i32.to_be_bytes(),
            &(-1i16).to_be_bytes(),
            &(-1i32).to_be_bytes(),
            &0i16.to_be_bytes(),
        ]
        .concat();
        out.extend(message(b'T', &[&1i16.to_be_bytes()[..], &field].concat()));
        let len = i32::try_from(value.len()).unwrap();
        out.extend(message(b'D', &[&1i16.to_be_bytes()[..], &len.to_be_bytes(), value.as_bytes()].concat()));
        out.extend(message(b'C', format!("{tag}\0").as_bytes()));
    }
    out
}

/// The answers to dbee's setup on a connection that is not read-only, in order: its SET, its six
/// SHOWs, its search path's schemas; each ends ready for the next.
fn setup_answers() -> Vec<Vec<u8>> {
    let ready = message(b'Z', b"I");
    vec![
        [message(b'C', b"SET\0"), ready.clone()].concat(),
        [rows(&["off", "on", "UTF8", "14.0", "140000", "public"], "SHOW"), ready.clone()].concat(),
        [rows(&["public"], "SELECT 1"), ready].concat(),
    ]
}

/// Serves one Postgres client: no TLS, a login with no password, then the first `answered` of
/// [`setup_answers`]; after that it reads and never answers, until the client closes.
fn serve_pg(mut conn: TcpStream, answered: usize, seen: &Watch<Seen>) {
    const SSL_REQUEST: u32 = 80_877_103;
    let (mut buf, mut have, mut started) = ([0u8; 16 * 1024], Vec::new(), false);
    let mut answers = setup_answers().into_iter().take(answered);
    loop {
        let n = match conn.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        have.extend_from_slice(&buf[..n]);
        loop {
            // Before the login a message is its length and its body; after it, a kind first.
            let head = usize::from(started);
            let Some(len) = have.get(head..head + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize) else { break };
            if have.len() < head + len {
                break;
            }
            let msg: Vec<u8> = have.drain(..head + len).collect();
            if !started {
                if msg.get(4..8) == Some(&SSL_REQUEST.to_be_bytes()[..]) {
                    conn.write_all(b"N").unwrap();
                } else {
                    started = true;
                    let login = [message(b'R', &0i32.to_be_bytes()), message(b'K', &[1u8, 0, 0, 0, 2, 0, 0, 0]), message(b'Z', b"I")].concat();
                    conn.write_all(&login).unwrap();
                }
            } else if msg.first() != Some(&b'X') {
                seen.note(|s| s.requests += 1);
                if let Some(answer) = answers.next() {
                    conn.write_all(&answer).unwrap();
                }
            }
        }
    }
}

/// What [`Mute`] saw: connects waiting through the tunnel, and those dropped while they waited.
#[derive(Debug, Default)]
struct MuteSeen {
    waiting: usize,
    dropped: usize,
}

/// Postgres, with a database behind the tunnel that does not answer until told to: until then a
/// connect opens its channel through the tunnel to the database and waits there, as one to a
/// database that never answers would. (The test sshd reaches only the test servers, which all
/// answer.)
struct Mute {
    real: Arc<dyn Driver>,
    answers: AtomicBool,
    seen: Arc<Watch<MuteSeen>>,
}

/// Notes a mute connect's wait, and its end when it is dropped.
struct Waiting(Arc<Watch<MuteSeen>>);

impl Drop for Waiting {
    fn drop(&mut self) {
        self.0.note(|s| s.dropped += 1);
    }
}

impl Mute {
    fn new() -> Arc<Mute> {
        let real = dbee_drivers::all().for_kind(DbKind::Postgres).cloned().expect("a Postgres driver");
        Arc::new(Mute { real, answers: AtomicBool::new(false), seen: Watch::new() })
    }

    /// This driver for Postgres, then every driver dbee has.
    fn drivers(self: &Arc<Mute>) -> Drivers {
        let mute: Arc<dyn Driver> = Arc::clone(self) as Arc<dyn Driver>;
        Drivers::new(std::iter::once(mute).chain(dbee_drivers::all().iter().cloned()).collect())
    }
}

impl Driver for Mute {
    fn kinds(&self) -> &'static [DbKind] {
        self.real.kinds()
    }

    fn connect<'a>(&'a self, target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
        if self.answers.load(Ordering::SeqCst) {
            return self.real.connect(target);
        }
        Box::pin(async move {
            let tunnel = target.tunnel.clone().ok_or_else(|| DbError::Other("the mute database is behind a tunnel".into()))?;
            let _channel = tunnel.connect(&target.config.host, target.config.port).await.map_err(DbError::Ssh)?;
            self.seen.note(|s| s.waiting += 1);
            let _waiting = Waiting(Arc::clone(&self.seen));
            std::future::pending().await
        })
    }
}

/// `command` started as a job.
fn started(s: &mut Session, command: &str, params: Json) -> JobId {
    match s.execute(command, params) {
        Ok(Outcome::Started(job)) => job,
        other => panic!("{command} did not start a job: {other:?}"),
    }
}

/// `connection.cancel`'s answer.
fn cancel(s: &mut Session) -> Json {
    run(s, "connection.cancel", json!({}))
}

/// Job `job`'s result, which must be the cancelled attempt's.
fn ends_cancelled(s: &mut Session, job: JobId, what: &str) {
    let e = match s.wait(job, T) {
        Ok(v) => panic!("{what}: the cancelled attempt ended {v}"),
        Err(e) => e,
    };
    assert_eq!((e.code(), e.to_string().as_str(), e.data()), ("cancelled", "the connection attempt was cancelled", json!({})), "{what}");
}

/// Nothing open, nothing running, nothing remembered: the connection screen as before the attempt.
fn nothing_open(s: &TestSession, what: &str) {
    let doc = s.inspect();
    assert_eq!(
        (&doc["screen"], &doc["connection"], &doc["connections"], &doc["jobs"]),
        (&json!("connections"), &Json::Null, &json!([]), &json!([])),
        "{what}"
    );
    assert!(s.state().store.recent.is_empty(), "{what}: the attempt went to Recent");
}

/// `t`'s settings, but at `host`.
fn at(t: &TestTarget, host: &Host) -> ConnectionConfig {
    ConnectionConfig { port: host.port, ..config_for(t) }
}

/// The targets on a server.
fn server_targets() -> Vec<TestTarget> {
    engine_targets().into_iter().filter(|t| !t.is_file()).collect()
}

/// On each server engine, a connect to a host that takes the connection and never answers, which
/// would wait for the connect limit (15 s), is cancelled and ends `cancelled`. How long each took
/// from the cancel to its end.
fn silent_hosts_cancelled() -> Vec<(String, Duration)> {
    let mut took = Vec::new();
    for t in server_targets() {
        let host = Host::new(Answer::Nothing);
        let mut s = session();
        let job = started(&mut s, "connection.open", json!({ "config": at(&t, &host), "password": pw() }));
        host.until("the connect", |seen| seen.taken == 1);
        let asked = Instant::now();
        assert_eq!(cancel(&mut s), json!({ "stopped": true }), "{}", t.name());
        ends_cancelled(&mut s, job, &t.name());
        let ended = asked.elapsed();
        println!("{}: the attempt ended {ended:?} after the cancel", t.name());
        took.push((t.name(), ended));
    }
    took
}

#[test]
fn a_connect_to_a_silent_host_can_be_cancelled() {
    let _db = require_testdb!();
    silent_hosts_cancelled();
}

/// The cancel ends the attempt within a second.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn a_connect_to_a_silent_host_is_cancelled_within_a_second() {
    let _db = require_testdb!();
    for (name, took) in silent_hosts_cancelled() {
        assert!(took < Duration::from_secs(1), "{name}: {took:?}");
    }
}

/// Review Focus 4: a connect through the SSH tunnel stops wherever it is, while the SSH server has
/// sent its first line and nothing more, and while the tunnel is up and the database behind it does
/// not answer. Each time the socket to the SSH server closes, and with it the tunnel; then the next
/// connect to the same place works at once.
#[test]
fn a_connect_through_the_tunnel_can_be_cancelled() {
    let db = require_testdb!();
    let host = Host::new(Answer::SshFirstLine);
    let key: Vec<&str> = db.sshd.known_hosts_line.split_whitespace().skip(1).collect();
    let known = format!("[127.0.0.1]:{} {}", host.port, key.join(" "));
    let mute = Mute::new();
    let mut s = session_in_adjusted(temp_home(Some(&db.keys.plain), Some(&known)), |o| o.drivers = mute.drivers());
    let mut config = via_ssh(SshAuth::Key { path: None });
    config.ssh.as_mut().unwrap().port = host.port;
    let params = json!({ "config": config, "password": pw() });

    let job = started(&mut s, "connection.open", params.clone());
    host.until("the key exchange", |seen| seen.past_first_line == 1);
    assert_eq!(cancel(&mut s), json!({ "stopped": true }));
    ends_cancelled(&mut s, job, "in the key exchange");
    host.until("the socket to the SSH server to close", |seen| seen.closed == 1);
    nothing_open(&s, "in the key exchange");

    host.set(Answer::Forward(db.sshd.port));
    let job = started(&mut s, "connection.open", params.clone());
    mute.seen.until("the channel to the database", |m| m.waiting == 1);
    assert_eq!(cancel(&mut s), json!({ "stopped": true }));
    ends_cancelled(&mut s, job, "with the tunnel up");
    host.until("the tunnel to close", |seen| seen.closed == 2);
    mute.seen.until("the wait to be dropped", |m| m.dropped == 1);
    nothing_open(&s, "with the tunnel up");

    mute.answers.store(true, Ordering::SeqCst);
    open(&mut s, params);
    assert_eq!(s.inspect()["connection"]["ssh"], true);
    s.run_blocking("schema.refresh", json!({}), T).unwrap();
}

/// Review 2, Minor 1: a direct Postgres connect cancelled after the login leaves nothing open.
/// The server logs the client in, then stops answering: during the session's setup, or, once the
/// setup is answered, during the catalog's load. Cancelled there, the attempt ends `cancelled` and
/// the socket to the server closes. How long each took from the cancel to the close.
fn pg_cancelled_after_login() -> Vec<(&'static str, Duration)> {
    let t = pg14_target();
    let mut took = Vec::new();
    for (when, answered) in [("in the setup", 0), ("in the catalog's load", setup_answers().len())] {
        let host = Host::new(Answer::Pg { answered });
        let mut s = session();
        let job = started(&mut s, "connection.open", json!({ "config": at(&t, &host), "password": pw() }));
        host.until(&format!("the request {when} that gets no answer"), |seen| seen.requests > answered);
        let asked = Instant::now();
        assert_eq!(cancel(&mut s), json!({ "stopped": true }), "{when}");
        ends_cancelled(&mut s, job, when);
        host.until(&format!("the socket to close {when}"), |seen| seen.closed == 1);
        let closed = asked.elapsed();
        println!("{when}: the socket closed {closed:?} after the cancel");
        nothing_open(&s, when);
        took.push((when, closed));
    }
    took
}

#[test]
fn a_postgres_connect_cancelled_after_the_login_leaves_no_socket() {
    let _db = require_testdb!();
    pg_cancelled_after_login();
}

/// The socket closes within a second of the cancel.
#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn a_postgres_connect_cancelled_after_the_login_closes_its_socket_within_a_second() {
    let _db = require_testdb!();
    for (when, took) in pg_cancelled_after_login() {
        assert!(took < Duration::from_secs(1), "{when}: {took:?}");
    }
}

/// A test stops as a connect does, and leaves the screen and the lists as they were.
#[test]
fn a_test_can_be_cancelled() {
    let _db = require_testdb!();
    for t in server_targets() {
        let host = Host::new(Answer::Nothing);
        let mut s = session();
        let job = started(&mut s, "connection.test", json!({ "config": at(&t, &host), "password": pw() }));
        host.until("the connect", |seen| seen.taken == 1);
        assert_eq!(cancel(&mut s), json!({ "stopped": true }), "{}", t.name());
        ends_cancelled(&mut s, job, &t.name());
        host.until("the socket to close", |seen| seen.closed == 1);
        nothing_open(&s, &t.name());
    }
}

/// A reconnect stops too: the connection stays as it was, with its tabs, and goes on working.
#[test]
fn a_reconnect_can_be_cancelled() {
    let _db = require_testdb!();
    let t = pg14_target();
    let host = Host::new(Answer::Forward(pg14().port));
    let mut s = session();
    open(&mut s, json!({ "config": at(&t, &host), "password": pw() }));
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    let before = s.inspect();
    let opened = host.seen();
    host.set(Answer::Nothing);
    let job = started(&mut s, "connection.reconnect", json!({}));
    host.until("the reconnect", |seen| seen.taken > opened.taken);
    assert_eq!(cancel(&mut s), json!({ "stopped": true }));
    ends_cancelled(&mut s, job, "the reconnect");
    host.until("its socket to close", |seen| seen.closed > opened.closed);
    let after = s.inspect();
    assert_eq!((&after["connection"], &after["tabs"]), (&before["connection"], &before["tabs"]));
    s.run_blocking("schema.refresh", json!({}), T).unwrap();
    assert_eq!(host.seen().closed, opened.closed + 1, "the connection it had stays open");
}

/// With no attempt running, a cancel does nothing and says so, without an error: on the
/// connection screen, while connected (the connection goes on), and after an attempt has ended.
#[test]
fn cancel_with_nothing_running_is_harmless() {
    let _db = require_testdb!();
    let none = json!({ "stopped": false, "note": "no connection attempt is running" });
    let mut s = session();
    assert_eq!(cancel(&mut s), none);
    assert!(s.journal().is_empty(), "a cancel is not journaled");
    drop(s);
    let mut s = connected();
    assert_eq!(cancel(&mut s), none);
    s.run_blocking("schema.refresh", json!({}), T).unwrap();
    s.execute("tab.newQuery", json!({ "text": "select 1" })).unwrap();
    s.run_blocking("query.run", json!({}), T).unwrap();
    drop(s);
    let host = Host::new(Answer::Nothing);
    let mut s = session();
    let job = started(&mut s, "connection.test", json!({ "config": at(&pg14_target(), &host), "password": pw() }));
    host.until("the connect", |seen| seen.taken == 1);
    assert_eq!(cancel(&mut s), json!({ "stopped": true }));
    assert_eq!(cancel(&mut s), json!({ "stopped": false, "stopping": true }), "asked again before it ended");
    ends_cancelled(&mut s, job, "the test");
    assert_eq!(cancel(&mut s), none);
}

/// A cancelled attempt leaves no connection: the document shows none, the host sees its socket
/// closed, and Recent is as before. That holds also when the attempt had connected before the
/// cancel came, while its result waited to be applied: what it opened is closed.
#[test]
fn a_cancelled_attempt_leaves_no_connection() {
    let _db = require_testdb!();
    for t in server_targets() {
        let host = Host::new(Answer::Nothing);
        let mut s = session();
        let job = started(&mut s, "connection.open", json!({ "config": at(&t, &host), "password": pw() }));
        host.until("the connect", |seen| seen.taken == 1);
        assert_eq!(cancel(&mut s), json!({ "stopped": true }), "{}", t.name());
        ends_cancelled(&mut s, job, &t.name());
        nothing_open(&s, &t.name());
        host.until("the socket to close", |seen| seen.closed == 1);
        drop(s);

        let host = Host::new(Answer::Forward(t.server().unwrap().port));
        let (finished, ended) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = finished.send(());
        });
        let mut s = session_adjusted(|o| o.notify = Some(notify));
        let job = started(&mut s, "connection.open", json!({ "config": at(&t, &host), "password": pw() }));
        ended.recv_timeout(T).expect("the open's work to end");
        assert_eq!(cancel(&mut s), json!({ "stopped": true }), "{}", t.name());
        ends_cancelled(&mut s, job, &t.name());
        nothing_open(&s, &t.name());
        host.until("the connection it made to close", |seen| seen.taken > 0 && seen.closed == seen.taken);
    }
}
