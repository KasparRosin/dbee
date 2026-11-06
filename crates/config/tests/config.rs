use dbee_config::store::{FILE, MAX_RECENT};
use dbee_config::*;

#[test]
fn url_fills_form() {
    let u = parse_url("postgresql://ann:p%40ss@db.example.com:6543/shop?sslmode=require").unwrap();
    assert_eq!(
        (u.host.as_deref(), u.port, u.user.as_deref(), u.password.as_deref(), u.database.as_deref(), u.ssl_mode),
        (Some("db.example.com"), Some(6543), Some("ann"), Some("p@ss"), Some("shop"), Some(SslMode::Require))
    );
}

#[test]
fn url_ipv6_and_bad_scheme() {
    assert_eq!(parse_url("postgres://[::1]/x").unwrap().host.as_deref(), Some("::1"));
    let u = parse_url("postgres://[::1]:5433/x").unwrap();
    assert_eq!((u.host.as_deref(), u.port), (Some("::1"), Some(5433)));
    assert!(parse_url("oracle://x").is_err());
    assert!(parse_url("postgres://h:notaport/x").is_err());
    assert!(parse_url("postgres://h/x?sslmode=bogus").is_err());
    assert!(parse_url("postgres://u%zz@h/x").is_err());
}

#[test]
fn url_minimal_and_hostile() {
    let u = parse_url("postgres://").unwrap();
    assert_eq!((u.host, u.port, u.user, u.password, u.database, u.ssl_mode), (None, None, None, None, None, None));
    let u = parse_url("postgres://u@h").unwrap();
    assert_eq!((u.user.as_deref(), u.password, u.database), (Some("u"), None, None));
    let u = parse_url("postgres://u:a:b@h/d%20x?application_name=z&sslmode=verify-full").unwrap();
    assert_eq!((u.password.as_deref(), u.database.as_deref(), u.ssl_mode), (Some("a:b"), Some("d x"), Some(SslMode::VerifyFull)));
    for s in ["", "postgres", "postgres://[::1", "postgres://%", "postgres://h:99999/x", "\u{0}", "postgres://\u{1f600}@h/\u{1f600}"] {
        let _ = parse_url(s);
    }
}

#[test]
fn defaults() {
    let c = ConnectionConfig::default();
    assert_eq!((c.host.as_str(), c.port, c.ssl_mode, c.save_secrets), ("localhost", 5432, SslMode::Prefer, true));
    assert_eq!(c.color, ConnectionColor::None);
    assert_eq!(SshAuth::default(), SshAuth::Key { path: None });
    assert_eq!(SshConfig::default().port, 22);
}

#[test]
fn config_json_has_no_password() {
    let j = serde_json::to_string(&ConnectionConfig::default()).unwrap();
    assert!(!j.to_lowercase().contains("password"));
}

#[test]
fn serde_shapes() {
    assert_eq!(serde_json::to_string(&SslMode::VerifyCa).unwrap(), "\"verify-ca\"");
    let c = ConnectionConfig {
        ssh: Some(SshConfig { host: "b".into(), user: "u".into(), auth: SshAuth::Agent, ..SshConfig::default() }),
        ..ConnectionConfig::default()
    };
    let v: serde_json::Value = serde_json::to_value(&c).unwrap();
    assert_eq!(v["sslMode"], "prefer");
    assert_eq!(v["readOnly"], false);
    assert_eq!(v["ssh"]["auth"]["method"], "agent");
    assert_eq!(serde_json::from_value::<ConnectionConfig>(v).unwrap(), c);
    // missing fields fall back to defaults
    let c: ConnectionConfig = serde_json::from_str(r#"{"id":"x"}"#).unwrap();
    assert_eq!((c.id.as_str(), c.port), ("x", 5432));
}

#[test]
fn display_name_and_validate() {
    let mut c = ConnectionConfig { user: "ann".into(), database: "shop".into(), ..ConnectionConfig::default() };
    assert_eq!(c.display_name(), "ann@localhost/shop");
    let no_database = ConnectionConfig { database: String::new(), ..c.clone() };
    assert_eq!(no_database.display_name(), "ann@localhost", "no slash without a database");
    c.name = "Prod".into();
    assert_eq!(c.display_name(), "Prod");
    assert!(c.validate().is_ok());
    c.host.clear();
    assert!(matches!(c.validate(), Err(ConfigError::Invalid(_))));
    c.host = "h".into();
    c.port = 0;
    assert!(c.validate().is_err());
    c.port = 1;
    c.ssh = Some(SshConfig::default());
    assert!(c.validate().is_err());
    c.ssh = Some(SshConfig { host: "b".into(), user: "u".into(), ..SshConfig::default() });
    assert!(c.validate().is_ok());
}

#[test]
fn ids_are_unique_and_home_dir_works() {
    let ids: std::collections::HashSet<String> = (0..1000).map(|_| new_id()).collect();
    assert_eq!(ids.len(), 1000);
    let _ = home_dir();
}

fn cfg(id: &str) -> ConnectionConfig {
    ConnectionConfig { id: id.into(), name: id.into(), ..ConnectionConfig::default() }
}

#[test]
fn store_round_trip_and_recent_cap() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    let (mut s, warn) = ConnectionStore::load(Some(&dir));
    assert!(warn.is_none() && s.saved.is_empty());
    s.upsert(cfg("a"));
    s.upsert(cfg("b"));
    let mut a2 = cfg("a");
    a2.host = "other".into();
    s.upsert(a2);
    assert_eq!(s.saved.len(), 2);
    assert_eq!(s.get("a").unwrap().host, "other");
    for i in 0..25 {
        s.touch_recent(&cfg(&format!("r{i}")), i as u64);
    }
    assert_eq!(s.recent.len(), MAX_RECENT);
    assert_eq!(s.recent[0].config.id, "r24");
    s.touch_recent(&cfg("r10"), 99);
    assert_eq!((s.recent[0].config.id.as_str(), s.recent[0].used_at, s.recent.len()), ("r10", 99, MAX_RECENT));
    assert_eq!(s.get("r24").unwrap().id, "r24");
    s.save().unwrap();
    let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(text.contains("\"version\""));
    let (l, warn) = ConnectionStore::load(Some(&dir));
    assert!(warn.is_none());
    assert_eq!((l.saved, l.recent), (s.saved.clone(), s.recent.clone()));
    let mut l = s.clone();
    assert!(l.remove("a"));
    assert!(!l.remove("a"));
    assert!(l.get("a").is_none());
}

#[test]
fn memory_only_store_saves_nothing() {
    let (s, w) = ConnectionStore::load(None);
    assert!(w.is_none());
    assert!(s.save().is_ok());
}

#[test]
fn corrupt_file_is_moved_aside() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    std::fs::write(dir.join(FILE), "{oops").unwrap();
    let (s, warn) = ConnectionStore::load(Some(&dir));
    assert!(s.saved.is_empty() && s.recent.is_empty());
    assert!(warn.is_some());
    assert!(dir.join("connections.json.bad").exists());
    assert!(!dir.join(FILE).exists());
}

#[test]
fn memory_store_set_get_delete() {
    let s = MemoryStore::default();
    s.set("c1", SecretKind::DbPassword, "pw").unwrap();
    assert_eq!(s.get("c1", SecretKind::DbPassword).unwrap().as_deref(), Some("pw"));
    assert_eq!(s.get("c1", SecretKind::SshPassword).unwrap(), None);
    s.delete("c1", SecretKind::DbPassword).unwrap();
    assert_eq!(s.get("c1", SecretKind::DbPassword).unwrap(), None);
    s.delete("c1", SecretKind::DbPassword).unwrap();
}

#[test]
fn secret_error_never_shows_secret() {
    let e = SecretError("backend said no".into());
    assert!(!format!("{e} {e:?}").contains("pw"));
    assert_eq!(MemoryStore::default().name(), "memory");
}

#[test]
fn url_parts_debug_hides_password() {
    let d = format!("{:?}", parse_url("postgres://u:hunter2@h").unwrap());
    assert!(!d.contains("hunter2"), "{d}");
    assert!(d.contains("redacted"));
}

#[test]
fn unreadable_file_is_never_overwritten() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    // a directory where the file should be: reading it fails with a non-NotFound error
    std::fs::create_dir_all(dir.join(FILE).join("keep")).unwrap();
    let (mut s, warn) = ConnectionStore::load(Some(&dir));
    assert!(warn.unwrap().contains("untouched"));
    assert!(s.dir.is_none());
    s.upsert(cfg("a"));
    s.save().unwrap();
    assert!(dir.join(FILE).join("keep").is_dir());
}

#[test]
fn failed_bad_rename_leaves_file_untouched() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    std::fs::write(dir.join(FILE), "{oops").unwrap();
    // every .bad name taken means no free place to move it
    std::fs::create_dir_all(dir.join("connections.json.bad").join("x")).unwrap();
    for n in 1..100 {
        std::fs::create_dir_all(dir.join(format!("connections.json.bad.{n}")).join("x")).unwrap();
    }
    let (mut s, warn) = ConnectionStore::load(Some(&dir));
    assert!(warn.unwrap().contains("untouched"));
    assert!(s.dir.is_none());
    s.upsert(cfg("a"));
    s.save().unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(FILE)).unwrap(), "{oops");
}

#[test]
fn save_leaves_no_temp_files() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    let (mut s, _) = ConnectionStore::load(Some(&dir));
    s.upsert(cfg("a"));
    s.save().unwrap();
    s.save().unwrap();
    let tmp = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
    assert_eq!(tmp, 0);
}

#[test]
fn old_saved_connections_are_postgres() {
    let c: ConnectionConfig = serde_json::from_str(r#"{"id":"a","name":"x","host":"h","port":5432}"#).unwrap();
    assert_eq!(c.kind, DbKind::Postgres);
}

#[test]
fn kinds_round_trip_by_id() {
    for k in DbKind::ALL {
        assert_eq!(DbKind::from_id(k.id()), Some(k));
    }
    assert_eq!(DbKind::from_id("oracle"), None);
}

#[test]
fn mysql_url() {
    let p = parse_url("mysql://u:p@db.example:3307/shop?ssl-mode=VERIFY_IDENTITY").unwrap();
    assert_eq!((p.kind, p.port, p.database.as_deref(), p.ssl_mode), (DbKind::MySql, Some(3307), Some("shop"), Some(SslMode::VerifyFull)));
}

#[test]
fn url_schemes_name_their_kind() {
    for (url, kind) in [
        ("postgres://h", DbKind::Postgres),
        ("postgresql://h", DbKind::Postgres),
        ("mysql://h", DbKind::MySql),
        ("mariadb://h", DbKind::MariaDb),
        ("MariaDB://h", DbKind::MariaDb),
    ] {
        assert_eq!(parse_url(url).unwrap().kind, kind, "{url}");
    }
    assert!(parse_url("sqlite://h").is_err());
}

#[test]
fn url_takes_both_sslmode_spellings() {
    assert_eq!(parse_url("mariadb://h/d?sslmode=verify-ca").unwrap().ssl_mode, Some(SslMode::VerifyCa));
    assert_eq!(parse_url("mysql://h/d?ssl-mode=REQUIRED").unwrap().ssl_mode, Some(SslMode::Require));
    assert_eq!(parse_url("mysql://h/d?ssl-mode=DISABLED").unwrap().ssl_mode, Some(SslMode::Disable));
    assert_eq!(parse_url("mysql://h/d?ssl-mode=PREFERRED").unwrap().ssl_mode, Some(SslMode::Prefer));
    assert_eq!(parse_url("mysql://h/d?ssl-mode=VERIFY_CA").unwrap().ssl_mode, Some(SslMode::VerifyCa));
}

#[test]
fn sqlite_needs_a_file() {
    let c = ConnectionConfig { kind: DbKind::Sqlite, file: None, ..Default::default() };
    assert!(c.validate().is_err());
    let c = ConnectionConfig { kind: DbKind::Sqlite, file: Some("/x/shop.sqlite".into()), host: String::new(), port: 0, ..Default::default() };
    assert!(c.validate().is_ok());
}

#[test]
fn a_database_file_has_no_ssh_tunnel() {
    let ssh = SshConfig { host: "jump.example".into(), port: 22, user: "u".into(), auth: SshAuth::Password };
    let c = ConnectionConfig { kind: DbKind::Sqlite, file: Some("/x/shop.sqlite".into()), ssh: Some(ssh.clone()), ..Default::default() };
    assert_eq!(c.validate(), Err(ConfigError::Invalid("a database file has no SSH tunnel".into())));
    assert!(ConnectionConfig { kind: DbKind::Postgres, ssh: Some(ssh), ..Default::default() }.validate().is_ok());
}

#[test]
fn sqlite_display_name_is_the_file() {
    let c = ConnectionConfig { kind: DbKind::Sqlite, file: Some("/x/shop.sqlite".into()), ..Default::default() };
    assert_eq!(c.display_name(), "shop.sqlite");
    let c = ConnectionConfig { name: "Mine".into(), ..c };
    assert_eq!(c.display_name(), "Mine");
}

#[test]
fn server_kinds_need_host_and_port() {
    for kind in [DbKind::MySql, DbKind::MariaDb] {
        let c = ConnectionConfig { kind, host: String::new(), ..Default::default() };
        assert!(c.validate().is_err());
        let c = ConnectionConfig { kind, port: 0, ..Default::default() };
        assert!(c.validate().is_err());
    }
}

#[test]
fn default_port_follows_the_default_kind() {
    assert_eq!(ConnectionConfig::default().port, DbKind::default().default_port().unwrap());
}

#[test]
fn every_kind_has_one_row() {
    for k in DbKind::ALL {
        assert_eq!(DbKind::from_id(k.info().id), Some(k));
        assert!(!k.label().is_empty());
    }
    let ids: std::collections::HashSet<&str> = DbKind::ALL.iter().map(|k| k.id()).collect();
    assert_eq!(ids.len(), DbKind::ALL.len());
}

#[test]
fn file_kinds_have_extensions_and_no_port() {
    for k in DbKind::ALL {
        match k.info().settings {
            Settings::File { extensions } => {
                assert!(k.uses_file());
                assert!(!extensions.is_empty());
                assert_eq!(k.default_port(), None);
            }
            Settings::Server { .. } => {
                assert!(!k.uses_file());
                assert!(k.default_port().is_some());
            }
        }
    }
}

#[test]
fn ssl_aliases_stay_with_their_kind() {
    assert!(parse_url("postgres://h/db?sslmode=VERIFY_IDENTITY").is_err());
    assert_eq!(parse_url("mysql://h/db?sslmode=VERIFY_IDENTITY").unwrap().ssl_mode, Some(SslMode::VerifyFull));
}

#[test]
fn saved_kind_json_is_the_id() {
    for k in DbKind::ALL {
        assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.id()));
        assert_eq!(serde_json::from_str::<DbKind>(&serde_json::to_string(&k).unwrap()).unwrap(), k);
    }
    assert_eq!(serde_json::to_string(&DbKind::MariaDb).unwrap(), "\"mariadb\"");
    assert!(serde_json::from_str::<DbKind>("\"oracle\"").is_err());
    let j = serde_json::to_value(ConnectionConfig::default()).unwrap();
    assert_eq!(j["kind"], "postgres");
}

#[test]
fn corrupt_file_never_overwrites_an_earlier_bad_file() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    std::fs::write(dir.join("connections.json.bad"), "first").unwrap();
    std::fs::write(dir.join(FILE), "{oops").unwrap();
    let (_, warn) = ConnectionStore::load(Some(&dir));
    assert!(warn.is_some());
    assert_eq!(std::fs::read_to_string(dir.join("connections.json.bad")).unwrap(), "first");
    assert_eq!(std::fs::read_to_string(dir.join("connections.json.bad.1")).unwrap(), "{oops");
}

#[test]
fn an_unreadable_entry_is_kept_and_the_others_load() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    let oracle = serde_json::json!({"id": "o", "name": "Newer", "kind": "oracle", "host": "h", "port": 1521, "future": [1, 2]});
    let recent = serde_json::json!({"config": {"id": "r", "kind": "oracle"}, "usedAt": 5});
    let good = serde_json::to_value(cfg("g")).unwrap();
    let file = serde_json::json!({"version": 1, "saved": [oracle, good], "recent": [recent]});
    std::fs::write(dir.join(FILE), serde_json::to_vec(&file).unwrap()).unwrap();
    let (mut s, warn) = ConnectionStore::load(Some(&dir));
    assert_eq!(s.saved.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["g"]);
    let warn = warn.unwrap();
    // One saved entry and one recent one, counted together.
    assert_eq!(warn, "2 saved or recent connection entries could not be read and are kept unchanged in the file");
    assert!(!dir.join("connections.json.bad").exists());
    s.upsert(cfg("h"));
    s.save().unwrap();
    let back: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(FILE)).unwrap()).unwrap();
    assert!(back["saved"].as_array().unwrap().contains(&oracle));
    assert!(back["recent"].as_array().unwrap().contains(&recent));
    assert_eq!(back["saved"].as_array().unwrap().len(), 3);
    let (again, _) = ConnectionStore::load(Some(&dir));
    assert_eq!(again.saved.len(), 2);
}

#[test]
fn a_sqlite_connection_round_trips_through_the_store() {
    let th = dbee_testkit::temp_home(None, None);
    let dir = th.home.clone();
    let (mut s, _) = ConnectionStore::load(Some(&dir));
    let c = ConnectionConfig { kind: DbKind::Sqlite, file: Some("/x/shop.sqlite".into()), ..cfg("s") };
    s.upsert(c.clone());
    s.save().unwrap();
    let (l, warn) = ConnectionStore::load(Some(&dir));
    assert_eq!((l.saved, warn), (vec![c], None));
}

#[test]
fn options_round_trip_and_stay_out_of_the_file_when_empty() {
    let v = serde_json::to_value(ConnectionConfig::default()).unwrap();
    assert!(v.get("options").is_none(), "{v}");
    let mut c = ConnectionConfig { kind: DbKind::MySql, ..cfg("m") };
    c.options.insert(ALLOW_PUBLIC_KEY.id.into(), "true".into());
    c.options.insert("aLaterOption".into(), "42".into());
    let v = serde_json::to_value(&c).unwrap();
    assert_eq!((&v["options"]["allowPublicKey"], &v["options"]["aLaterOption"]), (&serde_json::json!("true"), &serde_json::json!("42")));
    assert_eq!(serde_json::from_value::<ConnectionConfig>(v).unwrap(), c);
    assert!(c.option_on("allowPublicKey"));
    assert!(!c.option_on("aLaterOption") && !c.option_on("missing"));
    c.options.insert(ALLOW_PUBLIC_KEY.id.into(), "yes".into());
    assert!(!c.option_on(ALLOW_PUBLIC_KEY.id), "only \"true\" is on");

    // A key this build does not know survives a load and a save.
    let th = dbee_testkit::temp_home(None, None);
    let (mut s, _) = ConnectionStore::load(Some(&th.home));
    s.upsert(c.clone());
    s.save().unwrap();
    let (l, warn) = ConnectionStore::load(Some(&th.home));
    assert!(warn.is_none());
    l.save().unwrap();
    let (l, _) = ConnectionStore::load(Some(&th.home));
    assert_eq!(l.get("m").unwrap().options.get("aLaterOption").map(String::as_str), Some("42"));
    assert_eq!(l.get("m"), Some(&c));
}

#[test]
fn each_kind_lists_its_options() {
    assert_eq!(ALLOW_PUBLIC_KEY, KindOption { id: "allowPublicKey", label: "Allow the server's public key without encryption", weakens: true });
    assert_eq!(DbKind::MySql.info().options, [ALLOW_PUBLIC_KEY]);
    assert_eq!(DbKind::MariaDb.info().options, [ALLOW_PUBLIC_KEY]);
    assert!(DbKind::Postgres.info().options.is_empty() && DbKind::Sqlite.info().options.is_empty());
}
