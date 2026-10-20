//! `cargo xtask run-isolated [--release] [--port N] [-- args]`: the app on a throwaway home, for
//! agents and tests (`docs/agents.md` §4).
//!
//! 1. Builds `dbee` (`cargo build -p dbee`, `--release` when asked).
//! 2. Recreates `target/isolated/<port>/home` with `.config/` and `.ssh/` (mode 0700). When the
//!    Docker test servers' `target/testdb/env.json` exists, `.ssh/id_ed25519` is their plain test
//!    key (mode 0600) and `.ssh/known_hosts` holds their sshd's host key line.
//! 3. Runs the binary from the repository root with `--control <port>` (default
//!    [`DEFAULT_PORT`]) and the arguments after `--`, and waits for it to exit (an agent starts
//!    it in the background and ends it with `app.quit`).
//!
//! Only the child's environment changes ([`command`]): `HOME` and `USERPROFILE` are the
//! throwaway home, the XDG directories and `APPDATA`/`LOCALAPPDATA` lie inside it,
//! `SSH_AUTH_SOCK` and `DBEE_CONTROL_PORT` are removed and `DBEE_SECRET_STORE=memory`. So the run
//! never reads or writes the real home, `~/.ssh` or the keyring, and on Linux and macOS never
//! reaches the SSH agent. On Windows the OpenSSH agent pipe is still reachable: dbee finds it
//! without `SSH_AUTH_SOCK`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::testdb;

/// The control port without `--port`.
pub const DEFAULT_PORT: u16 = 7979;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub release: bool,
    pub port: u16,
    /// The app's own arguments (after `--`).
    pub rest: Vec<String>,
}

pub fn parse_args(args: &[&str]) -> Result<Options, String> {
    let mut o = Options { release: false, port: DEFAULT_PORT, rest: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--release" => o.release = true,
            "--port" => {
                let v = it.next().ok_or("--port needs a number")?;
                o.port = v.parse().ok().filter(|p| *p > 0).ok_or_else(|| format!("--port: `{v}` is not a port (1-65535)"))?;
            }
            "--" => {
                o.rest = it.map(|s| (*s).to_string()).collect();
                break;
            }
            other => return Err(format!("run-isolated: unknown argument `{other}` (usage: run-isolated [--release] [--port N] [-- args])")),
        }
    }
    Ok(o)
}

/// `<repo>/target/isolated/<port>/home`.
pub fn home_dir(root: &Path, port: u16) -> PathBuf {
    root.join("target").join("isolated").join(port.to_string()).join("home")
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), String> {
    Ok(())
}

/// Recreates the throwaway home for `port`; `testdb` is the test key file and the sshd's
/// `known_hosts` line. Returns the home.
pub fn prepare_home(root: &Path, port: u16, testdb: Option<(&Path, &str)>) -> Result<PathBuf, String> {
    let home = home_dir(root, port);
    match std::fs::remove_dir_all(&home) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("{}: {e}", home.display())),
        _ => {}
    }
    let ssh = home.join(".ssh");
    for dir in [home.join(".config"), ssh.clone()] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    set_mode(&ssh, 0o700)?;
    if let Some((key, known_hosts)) = testdb {
        let dest = ssh.join("id_ed25519");
        std::fs::copy(key, &dest).map_err(|e| format!("copy {}: {e}", key.display()))?;
        set_mode(&dest, 0o600)?;
        let hosts = ssh.join("known_hosts");
        std::fs::write(&hosts, format!("{known_hosts}\n")).map_err(|e| format!("{}: {e}", hosts.display()))?;
    }
    Ok(home)
}

/// Builds `dbee` and returns the binary cargo reports.
pub fn build(root: &Path, release: bool) -> Result<PathBuf, String> {
    let mut c = crate::cargo();
    c.current_dir(root).args(["build", "-p", "dbee", "--message-format=json-render-diagnostics"]);
    if release {
        c.arg("--release");
    }
    let mut child = c.stdout(Stdio::piped()).spawn().map_err(|e| format!("cargo build: {e}"))?;
    let mut exe = None;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if msg["reason"] == "compiler-artifact"
                && msg["target"]["name"] == "dbee"
                && let Some(path) = msg["executable"].as_str()
            {
                exe = Some(PathBuf::from(path));
            }
        }
    }
    let status = child.wait().map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build -p dbee: exited with {status}"));
    }
    exe.ok_or_else(|| "cargo build -p dbee reported no dbee binary".to_string())
}

/// The app's command: the binary with `--control <port>` and `rest`, run from `root` with the
/// throwaway home's environment (see the module docs).
pub fn command(root: &Path, port: u16, exe: &Path, rest: &[String]) -> Command {
    let home = home_dir(root, port);
    let mut c = Command::new(exe);
    c.current_dir(root)
        .arg("--control")
        .arg(port.to_string())
        .args(rest)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("XDG_STATE_HOME", home.join(".local").join("state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("APPDATA", home.join("AppData").join("Roaming"))
        .env("LOCALAPPDATA", home.join("AppData").join("Local"))
        .env("DBEE_SECRET_STORE", "memory")
        .env_remove("SSH_AUTH_SOCK")
        .env_remove("DBEE_CONTROL_PORT");
    c
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let o = parse_args(args)?;
    let exe = build(root, o.release)?;
    let env = testdb::read_env(&testdb::state_dir(root).join("env.json"));
    let testdb = env.as_ref().map(|e| (e.keys.plain.as_path(), e.sshd.known_hosts_line.as_str()));
    let home = prepare_home(root, o.port, testdb)?;
    eprintln!(
        "dbee on the throwaway home {} ({}), control channel on 127.0.0.1:{}",
        home.display(),
        if testdb.is_some() { "with the test servers' SSH key and host" } else { "no test servers" },
        o.port
    );
    let status = command(root, o.port, &exe, &o.rest).status().map_err(|e| format!("{}: {e}", exe.display()))?;
    if status.success() { Ok(()) } else { Err(format!("dbee exited with {status}")) }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    use super::*;

    fn env_of<'a>(c: &'a Command, key: &str) -> Option<Option<&'a OsStr>> {
        c.get_envs().find(|(k, _)| *k == OsStr::new(key)).map(|(_, v)| v)
    }

    #[test]
    fn isolated_env_hides_real_home_and_agent() {
        let root = Path::new("/repo");
        let c = command(root, 7979, Path::new("/repo/target/debug/dbee"), &["--x".to_string()]);
        let home = root.join("target").join("isolated").join("7979").join("home");
        assert_eq!(home_dir(root, 7979), home);
        assert_eq!(env_of(&c, "HOME"), Some(Some(home.as_os_str())));
        assert_eq!(env_of(&c, "USERPROFILE"), Some(Some(home.as_os_str())));
        assert_eq!(env_of(&c, "XDG_CONFIG_HOME"), Some(Some(home.join(".config").as_os_str())));
        assert_eq!(env_of(&c, "SSH_AUTH_SOCK"), Some(None), "the agent socket is removed");
        assert_eq!(env_of(&c, "DBEE_SECRET_STORE"), Some(Some(OsStr::new("memory"))));
        assert_eq!(env_of(&c, "DBEE_CONTROL_PORT"), Some(None));
        let args: Vec<&OsStr> = c.get_args().collect();
        assert_eq!(args, vec![OsStr::new("--control"), OsStr::new("7979"), OsStr::new("--x")]);
        assert_eq!(c.get_current_dir(), Some(root));
    }

    #[test]
    fn args_take_release_port_and_the_rest() {
        let o = |a: &[&str]| parse_args(a);
        assert_eq!(o(&[]), Ok(Options { release: false, port: DEFAULT_PORT, rest: vec![] }));
        assert_eq!(o(&["--release", "--port", "8000"]), Ok(Options { release: true, port: 8000, rest: vec![] }));
        assert_eq!(
            o(&["--port", "8000", "--", "--version", "x"]),
            Ok(Options { release: false, port: 8000, rest: vec!["--version".into(), "x".into()] })
        );
        for bad in [&["--port"][..], &["--port", "x"], &["--port", "0"], &["--port", "99999"], &["--fast"]] {
            assert!(o(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_home_is_recreated_with_the_test_key_and_host() {
        let root = std::env::temp_dir().join(format!("dbee-isolated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let key = root.join("key");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&key, "PRIVATE KEY").unwrap();
        let stale = home_dir(&root, 7).join("stale");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, "old run").unwrap();
        let home = prepare_home(&root, 7, Some((&key, "[127.0.0.1]:2222 ssh-ed25519 AAAA"))).unwrap();
        assert!(!stale.exists(), "the home starts empty");
        assert_eq!(std::fs::read_to_string(home.join(".ssh/id_ed25519")).unwrap(), "PRIVATE KEY");
        assert_eq!(std::fs::read_to_string(home.join(".ssh/known_hosts")).unwrap(), "[127.0.0.1]:2222 ssh-ed25519 AAAA\n");
        assert!(home.join(".config").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.join(".ssh")), 0o700);
            assert_eq!(mode(&home.join(".ssh/id_ed25519")), 0o600);
        }
        // Without the test servers: an empty .ssh.
        let home = prepare_home(&root, 7, None).unwrap();
        assert!(home.join(".ssh").is_dir() && !home.join(".ssh/id_ed25519").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A port nothing listens on right now.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    /// `ui.inspect`'s `frame`, or `None` while the app does not answer yet.
    fn inspect_frame(port: u16) -> Option<u64> {
        let s = TcpStream::connect(("127.0.0.1", port)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let mut w = s.try_clone().ok()?;
        w.write_all(b"{\"id\":1,\"method\":\"ui.inspect\"}\n").ok()?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).ok()?;
        serde_json::from_str::<serde_json::Value>(&line).ok()?["result"]["frame"].as_u64()
    }

    #[test]
    #[ignore = "opens a window; run by hand: cargo test -p xtask -- --ignored"]
    fn connection_screen_within_one_second() {
        let root = crate::root();
        let exe = build(&root, true).unwrap();
        let port = free_port();
        prepare_home(&root, port, None).unwrap();
        let mut cmd = command(&root, port, &exe, &[]);
        let started = Instant::now();
        let mut child = cmd.spawn().unwrap();
        let took = loop {
            if inspect_frame(port).is_some_and(|f| f >= 1) {
                break started.elapsed();
            }
            if started.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                panic!("no frame within 30 s");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        println!("connection screen drawn {took:?} after spawn (release build)");
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let _ = s.write_all(b"{\"method\":\"app.quit\"}\n");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(root.join("target").join("isolated").join(port.to_string()));
        assert!(took < Duration::from_secs(1), "{took:?}");
    }
}
