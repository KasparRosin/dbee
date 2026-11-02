//! The connection guard of isolated runs: with a list of allowed targets
//! ([`crate::SessionOptions::allowed_targets`], set by `cargo xtask run-isolated`),
//! `connection.test`, `connection.open` and `connection.reconnect` refuse every other target
//! before any network I/O. Without a list there is no limit (a normal run).
//!
//! The target is the database's `host:port`, or with an SSH tunnel the SSH server's (the
//! database host behind it is resolved by the sshd). Hosts are compared lowercased and literally:
//! no name lookup, so `localhost` and `127.0.0.1` are separate entries.

use dbee_config::ConnectionConfig;

use crate::EngineError;

/// `Ok` when `config` may be reached: there is no list, or its target is on it.
pub(crate) fn check_target(allowed: Option<&[(String, u16)]>, config: &ConnectionConfig) -> Result<(), EngineError> {
    let Some(allowed) = allowed else { return Ok(()) };
    let (what, host, port) = match &config.ssh {
        Some(ssh) => ("the SSH server", ssh.host.as_str(), ssh.port),
        None => ("the database", config.host.as_str(), config.port),
    };
    let host = host.to_lowercase();
    if allowed.iter().any(|(h, p)| *p == port && h.to_lowercase() == host) {
        return Ok(());
    }
    log::warn!("a connection outside the allowed targets of this isolated run was refused");
    Err(EngineError::Config(format!("this isolated run may only connect to the test servers; {what} {host}:{port} is not one of them")))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use dbee_config::{ConnectionConfig, MemoryStore, SshAuth, SshConfig};
    use serde_json::json;

    use super::*;
    use crate::{Session, SessionOptions};

    fn at(host: &str, port: u16) -> ConnectionConfig {
        ConnectionConfig { host: host.into(), port, user: "u".into(), database: "d".into(), ..ConnectionConfig::default() }
    }

    fn tunnel(ssh_host: &str, ssh_port: u16, db_host: &str, db_port: u16) -> ConnectionConfig {
        let ssh = SshConfig { host: ssh_host.into(), port: ssh_port, user: "dbee".into(), auth: SshAuth::Agent };
        ConnectionConfig { ssh: Some(ssh), ..at(db_host, db_port) }
    }

    fn list() -> Vec<(String, u16)> {
        vec![("127.0.0.1".into(), 1), ("localhost".into(), 32780), ("127.0.0.1".into(), 32779)]
    }

    #[test]
    fn only_the_listed_targets_pass() {
        let list = list();
        let allowed = Some(list.as_slice());
        assert_eq!(check_target(allowed, &at("127.0.0.1", 1)), Ok(()));
        assert_eq!(check_target(allowed, &at("localhost", 32780)), Ok(()));
        // The host is compared lowercased, literally: no name lookup.
        assert_eq!(check_target(allowed, &at("LocalHost", 32780)), Ok(()));
        for refused in [at("localhost", 5432), at("127.0.0.1", 32780), at("db.example", 32780), at("127.0.0.2", 1)] {
            let e = check_target(allowed, &refused).unwrap_err();
            assert_eq!(e.code(), "config", "{e}");
            assert!(e.to_string().contains("this isolated run may only connect to the test servers"), "{e}");
            assert!(e.to_string().contains(&format!("{}:{}", refused.host, refused.port)), "the refused target is named: {e}");
        }
        // An empty list allows nothing.
        assert!(check_target(Some(&[]), &at("127.0.0.1", 1)).is_err());
    }

    #[test]
    fn a_tunnel_is_checked_by_its_ssh_server() {
        let list = list();
        let allowed = Some(list.as_slice());
        // The database behind the tunnel is resolved by the sshd, so only the SSH host and port count.
        assert_eq!(check_target(allowed, &tunnel("127.0.0.1", 32779, "pg14", 5432)), Ok(()));
        let e = check_target(allowed, &tunnel("jump.example", 22, "localhost", 32780)).unwrap_err();
        assert!(e.to_string().contains("jump.example:22"), "{e}");
    }

    #[test]
    fn no_list_is_no_limit() {
        for config in [at("db.example", 5432), at("localhost", 5432), tunnel("jump.example", 22, "db", 5432)] {
            assert_eq!(check_target(None, &config), Ok(()));
        }
    }

    #[test]
    fn reconnect_is_checked_too() {
        let db = dbee_testkit::require_testdb!();
        let server = db.server("pg14").unwrap();
        let mut s = Session::new(SessionOptions {
            secrets: Some(std::sync::Arc::new(MemoryStore::default())),
            allowed_targets: Some(vec![("127.0.0.1".into(), server.port)]),
            drivers: dbee_drivers::all(),
            ..SessionOptions::default()
        })
        .unwrap();
        let config = at("127.0.0.1", server.port);
        let config = ConnectionConfig { user: "postgres".into(), database: "dbee".into(), ..config };
        s.run_blocking("connection.open", json!({ "config": config, "password": db.password }), Duration::from_secs(30)).unwrap();
        s.allowed_targets = Some(vec![("127.0.0.1".into(), 1)]);
        let e = s.execute("connection.reconnect", json!({})).unwrap_err();
        assert_eq!(e.code(), "config", "{e}");
        assert!(!s.busy(), "refused before any job started");
        assert_eq!(s.run_blocking("connection.close", json!({}), Duration::from_secs(5)).map(|_| ()), Ok(()));
    }
}
