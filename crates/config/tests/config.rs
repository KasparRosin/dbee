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
    assert!(parse_url("mysql://x").is_err());
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
    // a non-empty directory at the .bad path makes the rename fail
    std::fs::create_dir_all(dir.join("connections.json.bad").join("x")).unwrap();
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
