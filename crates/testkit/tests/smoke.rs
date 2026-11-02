//! The Docker test servers are up and seeded (`cargo xtask testdb up`).

use dbee_testkit::Kind;

#[test]
fn seed_is_present_on_every_server() {
    let db = dbee_testkit::require_testdb!();
    assert!(db.servers_of(&[Kind::Postgres]).any(|s| s.major() == 14));
    for s in db.servers_of(&[Kind::Postgres]) {
        assert_eq!(dbee_testkit::psql(s, "SELECT count(*) FROM shop.big_rows").unwrap().trim(), "1000000");
        assert_eq!(dbee_testkit::psql(s, "SELECT count(*) FROM pg_tables WHERE schemaname = 'many'").unwrap().trim(), "300");
    }
}

/// Facts later tasks rely on: sizes, the filter example, the huge cell, TLS and the key files.
#[test]
fn seed_facts_and_secrets() {
    let db = dbee_testkit::require_testdb!();
    let newest = db.server("pgnew").unwrap();
    assert!(newest.major() >= 18, "{newest:?}");
    for s in db.servers_of(&[Kind::Postgres]) {
        let q = |sql: &str| dbee_testkit::psql(s, sql).unwrap().trim().to_owned();
        assert_eq!(q("SHOW server_version_num").parse::<u32>().unwrap() / 10_000, s.major());
        assert_eq!(q("SHOW ssl"), "on");
        assert_eq!(q("SELECT count(*) FROM shop.customers"), "500");
        assert_eq!(q("SELECT count(*) FROM shop.orders"), "5000");
        assert_ne!(q("SELECT count(*) FROM shop.orders WHERE dealer_id = 42 AND status = 'completed' AND total > 100"), "0");
        assert_eq!(q("SELECT meta::text, raw_json_text FROM shop.orders WHERE id = 1"), r#""{\"a\":1}"|{"k":[1,2]}"#);
        assert_eq!(q("SELECT count(*) FROM shop.all_types"), "3");
        assert_eq!(q("SELECT length(t) FROM shop.big_cell"), "20000000");
        assert!(q("SELECT length(j::text) FROM shop.big_cell").parse::<u64>().unwrap() >= 20_000_000);
        assert!(q("SELECT reltuples FROM pg_class WHERE oid = 'shop.big_rows'::regclass").parse::<f64>().unwrap() >= 900_000.0);
        assert_eq!(q(r#"SELECT count(*) FROM odd."Odd ""Name"" 🐝" WHERE "Total $" > 1"#), "2");
        assert_eq!(q("SELECT count(*) FROM pg_tables WHERE schemaname = 'scratch'"), "0");
        assert!(dbee_testkit::psql(s, "SELECT nope").unwrap_err().contains("nope"));
    }
    for key in [&db.keys.plain, &db.keys.encrypted, &db.keys.rsa, &db.ca_file] {
        assert!(key.is_file(), "{key:?}");
    }
    assert!(std::fs::read_to_string(&db.keys.encrypted).unwrap().contains("OPENSSH PRIVATE KEY"));
    assert!(db.sshd.known_hosts_line.starts_with(&format!("[127.0.0.1]:{} ssh-ed25519 AAAA", db.sshd.port)));
}
