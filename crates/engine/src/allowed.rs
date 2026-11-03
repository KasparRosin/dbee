//! The connection guard of isolated runs (`cargo xtask run-isolated`): `connection.test`,
//! `connection.open` and `connection.reconnect` refuse what the run may not reach before the
//! connection is tried. Which list counts follows the database type's settings: a server is
//! checked by its target ([`crate::SessionOptions::allowed_targets`]), a file by the folders it may
//! lie in ([`crate::SessionOptions::allowed_dirs`]). Without a list there is no limit (a normal
//! run).
//!
//! **Targets.** The database's `host:port`, or with an SSH tunnel the SSH server's (the database
//! host behind it is resolved by the sshd). Hosts are compared lowercased and literally: no name
//! lookup, so `localhost` and `127.0.0.1` are separate entries.
//!
//! **Files.** The file and the folders are resolved first: links followed, `.` and `..` gone. A
//! file that is not there yet (not even as a link) is resolved by its folder. A path that cannot
//! be resolved is refused.

use std::path::{Path, PathBuf};

use dbee_config::{ConnectionConfig, Settings};

use crate::EngineError;

/// `Ok` when this run may open `config`: by its type's settings, a server's target is on
/// `targets`, or its file lies in one of `dirs`.
pub(crate) fn check(targets: Option<&[(String, u16)]>, dirs: Option<&[PathBuf]>, config: &ConnectionConfig) -> Result<(), EngineError> {
    match config.kind.info().settings {
        Settings::Server { .. } => check_target(targets, config),
        Settings::File { .. } => check_file(dirs, config.file.as_deref()),
    }
}

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

/// `Ok` when `file` may be opened: there is no list, or it lies in one of the folders, both
/// resolved. No file at all is the driver's to refuse.
pub(crate) fn check_file(allowed: Option<&[PathBuf]>, file: Option<&Path>) -> Result<(), EngineError> {
    let (Some(allowed), Some(file)) = (allowed, file) else { return Ok(()) };
    if let Some(path) = resolve(file)
        && allowed.iter().filter_map(|dir| std::fs::canonicalize(dir).ok()).any(|dir| path.starts_with(dir))
    {
        return Ok(());
    }
    log::warn!("a file outside the allowed folders of this isolated run was refused");
    Err(EngineError::Config(format!("this isolated run may only open files in its own folders; {} is not in them", file.display())))
}

/// `path` with its links followed and `.` and `..` gone; a path that is not there, by its folder.
fn resolve(path: &Path) -> Option<PathBuf> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => std::fs::canonicalize(path).ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let name = path.file_name()?;
            let folder = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            Some(std::fs::canonicalize(folder).ok()?.join(name))
        }
        Err(_) => None,
    }
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

    /// A folder of its own under `target/`, removed when dropped.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Folder {
            let path = dbee_testkit::repo_root().join("target").join("testkit").join("allowed").join(dbee_testkit::unique(name));
            std::fs::create_dir_all(&path).unwrap();
            Folder(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn files_must_lie_in_the_allowed_folders_once_resolved() {
        let (allowed, outside) = (Folder::new("in"), Folder::new("out"));
        let inside = allowed.0.join("in.sqlite");
        let out = outside.0.join("out.sqlite");
        std::fs::write(&inside, "").unwrap();
        std::fs::write(&out, "").unwrap();
        let dirs = vec![allowed.0.clone()];
        let check = |path: &Path| check_file(Some(&dirs), Some(path));
        assert_eq!(check(&inside), Ok(()));
        // A file not there yet is resolved by its folder.
        assert_eq!(check(&allowed.0.join("new.sqlite")), Ok(()));
        let back_out = allowed.0.join("..").join(outside.0.file_name().unwrap()).join("out.sqlite");
        let mut refused =
            vec![out.clone(), outside.0.join("new.sqlite"), PathBuf::from("/etc/hosts"), back_out, allowed.0.join("gone").join("x.sqlite")];
        #[cfg(unix)]
        {
            // A link out of the folder is refused, whether or not what it names is there; one to
            // a file inside, and a folder given through a link, are fine.
            use std::os::unix::fs::symlink;
            symlink("/etc/hosts", allowed.0.join("hosts.sqlite")).unwrap();
            symlink(outside.0.join("missing.sqlite"), allowed.0.join("dangling.sqlite")).unwrap();
            symlink(&inside, allowed.0.join("alias.sqlite")).unwrap();
            refused.extend([allowed.0.join("hosts.sqlite"), allowed.0.join("dangling.sqlite")]);
            assert_eq!(check(&allowed.0.join("alias.sqlite")), Ok(()));
            symlink(&allowed.0, outside.0.join("to-allowed")).unwrap();
            assert_eq!(check_file(Some(&[outside.0.join("to-allowed")]), Some(&inside)), Ok(()));
        }
        for path in &refused {
            let e = check(path).unwrap_err();
            assert_eq!(e.code(), "config", "{}: {e}", path.display());
            assert_eq!(e.to_string(), format!("this isolated run may only open files in its own folders; {} is not in them", path.display()));
        }
        // No list is no limit, an empty list allows nothing, and no file is the driver's to refuse.
        assert_eq!(check_file(None, Some(Path::new("/etc/hosts"))), Ok(()));
        assert!(check_file(Some(&[]), Some(&inside)).is_err());
        assert_eq!(check_file(Some(&dirs), None), Ok(()));
    }

    #[test]
    fn a_database_type_is_checked_by_its_settings() {
        let folder = Folder::new("kinds");
        let dirs = vec![folder.0.clone()];
        let file = |path: &Path| ConnectionConfig { kind: dbee_config::DbKind::Sqlite, file: Some(path.to_path_buf()), ..at("127.0.0.1", 1) };
        // A file by its folder, whatever host and port it holds; a server by its target alone.
        let targets = list();
        assert!(check(Some(&targets), Some(&dirs), &file(Path::new("/etc/hosts"))).is_err());
        assert_eq!(check(Some(&[]), Some(&dirs), &file(&folder.0.join("x.sqlite"))), Ok(()));
        assert_eq!(check(Some(&targets), Some(&[]), &at("127.0.0.1", 1)), Ok(()));
        assert!(check(Some(&[]), Some(&dirs), &at("127.0.0.1", 1)).is_err());
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

    #[test]
    fn a_reconnect_checks_the_file_again() {
        let seed = dbee_testkit::targets_of(&[dbee_testkit::Kind::Sqlite]).pop().unwrap();
        let file = seed.file().unwrap();
        let folder = file.parent().unwrap().to_path_buf();
        let mut s =
            Session::new(SessionOptions { allowed_dirs: Some(vec![folder]), drivers: dbee_drivers::all(), ..SessionOptions::default() }).unwrap();
        let config = ConnectionConfig { kind: dbee_config::DbKind::Sqlite, file: Some(file.to_path_buf()), ..ConnectionConfig::default() };
        s.run_blocking("connection.open", json!({ "config": config }), Duration::from_secs(30)).unwrap();
        s.allowed_dirs = Some(Vec::new());
        let e = s.execute("connection.reconnect", json!({})).unwrap_err();
        assert_eq!(e.code(), "config", "{e}");
        assert!(!s.busy(), "refused before any job started");
        assert_eq!(s.run_blocking("connection.close", json!({}), Duration::from_secs(5)).map(|_| ()), Ok(()));
    }
}
