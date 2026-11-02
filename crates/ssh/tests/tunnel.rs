//! Tunnel tests against the Docker `sshd` (`cargo xtask testdb up`).
//!
//! Safety: every test builds its own `SshEnv` over a throwaway home from `temp_home`, so the real
//! `~/.ssh` and the user's SSH agent are never touched. The agent test starts its own `ssh-agent`
//! on a socket inside that home and kills it when done.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dbee_config::{SshAuth, SshConfig};
use dbee_ssh::{AgentSource, KeyPlan, SshEnv, SshError, SshSecrets, Tunnel, plan};
use dbee_testkit::{TempHome, TestDb, require_testdb, temp_home};
use russh::keys::{HashAlg, PublicKeyBase64, load_secret_key, parse_public_key_base64};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn cfg(db: &TestDb, auth: SshAuth) -> SshConfig {
    SshConfig { host: "127.0.0.1".into(), port: db.sshd.port, user: db.sshd.user.clone(), auth }
}

fn env(home: &TempHome) -> SshEnv {
    SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::None }
}

fn secrets(password: Option<&str>, passphrase: Option<&str>) -> SshSecrets {
    SshSecrets { password: password.map(str::to_owned), passphrase: passphrase.map(str::to_owned) }
}

/// `[127.0.0.1]:<port>`, as `known_hosts` and the host key errors write it.
fn host_entry(db: &TestDb) -> String {
    format!("[127.0.0.1]:{}", db.sshd.port)
}

/// The server's host key, base64, from the test env.
fn server_key_base64(db: &TestDb) -> String {
    db.sshd.known_hosts_line.split_whitespace().nth(2).unwrap_or_default().to_owned()
}

/// pg14 as the sshd reaches it: `pg14:5432`.
fn pg14(db: &TestDb) -> (&str, u16) {
    let s = db.server("pg14").unwrap();
    (&s.name, s.service_port)
}

fn server_fingerprint(db: &TestDb) -> String {
    parse_public_key_base64(&server_key_base64(db)).unwrap().fingerprint(HashAlg::Sha256).to_string()
}

/// Opens a channel to Postgres through the tunnel and checks that it answers an SSLRequest.
async fn reaches_postgres(t: &Tunnel, db: &TestDb) {
    let mut s = t.connect(pg14(db).0, pg14(db).1).await.unwrap();
    s.write_all(&[0, 0, 0, 8, 4, 210, 22, 47]).await.unwrap(); // SSLRequest
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    assert!(b[0] == b'S' || b[0] == b'N', "{b:?}");
}

#[tokio::test]
async fn default_key_login_reaches_postgres() {
    let db = require_testdb!();
    let home = temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    let t = Tunnel::open(&cfg(db, SshAuth::Key { path: None }), &env(&home), &SshSecrets::default(), None).await.unwrap();
    assert!(t.used().starts_with("key ") && t.used().ends_with("id_ed25519"), "{}", t.used());
    assert_eq!(t.used(), format!("key {}", home.ssh_dir().join("id_ed25519").display()));
    reaches_postgres(&t, db).await;
    // Each call is a new channel; the other Postgres server is allowed too.
    reaches_postgres(&t, db).await;
    let mut s = t.connect("pgnew", 5432).await.unwrap();
    s.write_all(&[0, 0, 0, 8, 4, 210, 22, 47]).await.unwrap();
    let mut b = [0u8; 1];
    s.read_exact(&mut b).await.unwrap();
    // The server forbids every other destination.
    match t.connect("pg14", 22).await {
        Err(SshError::Channel(m)) => assert!(m.contains("pg14:22"), "{m}"),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("the server allowed pg14:22"),
    }
    assert!(!t.is_closed());
    t.close().await;
    assert!(t.is_closed());
    assert!(matches!(t.connect(pg14(db).0, pg14(db).1).await, Err(SshError::Channel(_))));
}

#[tokio::test]
async fn unknown_host_gives_fingerprint_then_learns() {
    let db = require_testdb!();
    let home = temp_home(Some(&db.keys.plain), None);
    let known_hosts = home.ssh_dir().join("known_hosts");
    let c = cfg(db, SshAuth::Key { path: None });
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    let SshError::HostKeyUnknown { host, key_type, fingerprint } = e.clone() else { panic!("{e:?}") };
    assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
    assert_eq!(fingerprint, server_fingerprint(db));
    assert_eq!((host.as_str(), key_type.as_str()), (host_entry(db).as_str(), "ssh-ed25519"));
    assert!(e.to_string().contains(&fingerprint), "{e}");
    assert!(!known_hosts.exists());

    // A fingerprint the user did not accept changes nothing.
    let again = Tunnel::open(&c, &env(&home), &SshSecrets::default(), Some("SHA256:not-this-one")).await.unwrap_err();
    assert_eq!(again, e);
    assert!(!known_hosts.exists());

    let t = Tunnel::open(&c, &env(&home), &SshSecrets::default(), Some(&fingerprint)).await.unwrap();
    reaches_postgres(&t, db).await;
    let text = std::fs::read_to_string(&known_hosts).unwrap();
    assert!(text.contains("[127.0.0.1]:"), "{text}");
    // russh starts a new file with an empty line.
    assert_eq!(text.trim_start_matches('\n'), format!("{} ssh-ed25519 {}\n", host_entry(db), server_key_base64(db)));

    // Learned once: the next open needs no trust.
    Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap();
}

#[tokio::test]
async fn changed_host_key_refuses() {
    let db = require_testdb!();
    // Another ed25519 key recorded for the server's host and port (here: the client key's public half).
    let other = load_secret_key(&db.keys.plain, None).unwrap().public_key().public_key_base64();
    assert_ne!(other, server_key_base64(db));
    let line = format!("# saved earlier\n{} ssh-ed25519 {other}\n", host_entry(db));
    let home = temp_home(Some(&db.keys.plain), Some(&line));
    let c = cfg(db, SshAuth::Key { path: None });
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    assert_eq!(e, SshError::HostKeyChanged { host: host_entry(db), line: 2 });
    // Never overridable, not even with the new key's fingerprint.
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), Some(&server_fingerprint(db))).await.unwrap_err();
    assert_eq!(e, SshError::HostKeyChanged { host: host_entry(db), line: 2 });
    assert_eq!(std::fs::read_to_string(home.ssh_dir().join("known_hosts")).unwrap(), line);
}

#[tokio::test]
async fn encrypted_key_needs_passphrase() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let key = db.keys.encrypted.clone();
    let c = cfg(db, SshAuth::Key { path: Some(key.clone()) });
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    assert_eq!(e, SshError::NeedPassphrase { key: key.clone() });
    let e = Tunnel::open(&c, &env(&home), &secrets(None, Some("wrong")), None).await.unwrap_err();
    assert_eq!(e, SshError::WrongPassphrase { key: key.clone() });
    // A password is not a passphrase.
    let e = Tunnel::open(&c, &env(&home), &secrets(Some(&db.keys.passphrase), None), None).await.unwrap_err();
    assert_eq!(e, SshError::NeedPassphrase { key: key.clone() });
    let t = Tunnel::open(&c, &env(&home), &secrets(None, Some(&db.keys.passphrase)), None).await.unwrap();
    assert_eq!(t.used(), format!("key {}", key.display()));
    reaches_postgres(&t, db).await;

    // The same key as the default key asks for its passphrase under its default name.
    let home = temp_home(Some(&db.keys.encrypted), Some(&db.sshd.known_hosts_line));
    let c = cfg(db, SshAuth::Key { path: None });
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    assert_eq!(e, SshError::NeedPassphrase { key: home.ssh_dir().join("id_ed25519") });
    Tunnel::open(&c, &env(&home), &secrets(None, Some(&db.keys.passphrase)), None).await.unwrap();
}

#[tokio::test]
async fn password_login_and_missing_password() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let c = SshConfig { user: db.sshd.password_user.clone(), ..cfg(db, SshAuth::Password) };
    let t = Tunnel::open(&c, &env(&home), &secrets(Some(&db.password), None), None).await.unwrap();
    assert_eq!(t.used(), "password");
    reaches_postgres(&t, db).await;
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    assert_eq!(e, SshError::NeedPassword);
    let e = Tunnel::open(&c, &env(&home), &secrets(Some("not the password"), None), None).await.unwrap_err();
    assert_eq!(e, SshError::AuthFailed { tried: vec!["password".into()] });
}

#[tokio::test]
async fn rsa_key_login() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let c = cfg(db, SshAuth::Key { path: Some(db.keys.rsa.clone()) });
    let t = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap();
    assert_eq!(t.used(), format!("key {}", db.keys.rsa.display()));
    reaches_postgres(&t, db).await;
}

/// A throwaway `ssh-agent` on a socket of its own; killed on drop, also when the test fails.
struct OwnAgent {
    child: Child,
    socket: PathBuf,
}

impl Drop for OwnAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// `ssh-agent -D -a <socket>` (foreground, so the child is the agent), then `ssh-add <key>` with
/// `SSH_AUTH_SOCK` set on that one child process only.
fn start_agent(socket: &Path, key: &Path) -> OwnAgent {
    let child = Command::new("ssh-agent")
        .arg("-D")
        .arg("-a")
        .arg(socket)
        .env_remove("SSH_AUTH_SOCK")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let agent = OwnAgent { child, socket: socket.to_path_buf() };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "ssh-agent did not create {}", socket.display());
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = Command::new("ssh-add")
        .arg(key)
        .env("SSH_AUTH_SOCK", socket)
        .env_remove("SSH_ASKPASS")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "ssh-add: {}", String::from_utf8_lossy(&out.stderr));
    agent
}

#[tokio::test]
async fn agent_login_when_ssh_agent_exists() {
    let db = require_testdb!();
    if !on_path("ssh-agent") || !on_path("ssh-add") {
        dbee_testkit::skipped("agent_login_when_ssh_agent_exists: ssh-agent or ssh-add is not on PATH");
        return;
    }
    // No key files at all: only the agent can log in.
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    let socket = home.home.join("agent.sock");
    let _agent = start_agent(&socket, &db.keys.plain);
    let e = SshEnv { ssh_dir: Some(home.ssh_dir()), agent: AgentSource::Socket(socket.clone()) };

    let t = Tunnel::open(&cfg(db, SshAuth::Agent), &e, &SshSecrets::default(), None).await.unwrap();
    assert_eq!(t.used(), "agent");
    reaches_postgres(&t, db).await;
    // The automatic choice prefers an agent with identities.
    let t = Tunnel::open(&cfg(db, SshAuth::Key { path: None }), &e, &SshSecrets::default(), None).await.unwrap();
    assert_eq!(t.used(), "agent");
    assert_eq!(plan(&cfg(db, SshAuth::Key { path: None }), &e).await, KeyPlan::Agent { identities: 1 });
    assert_eq!(plan(&cfg(db, SshAuth::Agent), &e).await, KeyPlan::Agent { identities: 1 });

    // The agent's key is refused for the password-only user; that is reported as tried.
    let c = SshConfig { user: db.sshd.password_user.clone(), ..cfg(db, SshAuth::Agent) };
    let err = Tunnel::open(&c, &e, &SshSecrets::default(), None).await.unwrap_err();
    assert_eq!(err, SshError::AuthFailed { tried: vec!["agent".into()] });
}

#[tokio::test]
async fn wrong_user_fails_with_what_was_tried() {
    let db = require_testdb!();
    let home = temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    let c = SshConfig { user: "nobody".into(), ..cfg(db, SshAuth::Key { path: None }) };
    let e = Tunnel::open(&c, &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    let SshError::AuthFailed { tried } = &e else { panic!("{e:?}") };
    assert_eq!(tried, &vec![format!("key {}", home.ssh_dir().join("id_ed25519").display())]);
    assert!(e.to_string().contains("id_ed25519"), "{e}");
}

#[tokio::test]
async fn no_key_and_no_agent_is_no_key() {
    let db = require_testdb!();
    let home = temp_home(None, Some(&db.sshd.known_hosts_line));
    for auth in [SshAuth::Key { path: None }, SshAuth::Agent] {
        let e = Tunnel::open(&cfg(db, auth), &env(&home), &SshSecrets::default(), None).await.unwrap_err();
        assert_eq!(e, SshError::NoKey);
    }
    // A chosen key file that does not exist is tried and reported, not silently skipped.
    let missing = home.home.join("missing_key");
    let e = Tunnel::open(&cfg(db, SshAuth::Key { path: Some(missing.clone()) }), &env(&home), &SshSecrets::default(), None).await.unwrap_err();
    let SshError::AuthFailed { tried } = &e else { panic!("{e:?}") };
    assert!(tried.len() == 1 && tried[0].starts_with(&format!("key {}", missing.display())), "{tried:?}");
}

#[tokio::test]
async fn nothing_listening_is_a_connect_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let home = temp_home(None, None);
    let c = SshConfig { host: "127.0.0.1".into(), port, user: "x".into(), auth: SshAuth::Password };
    let e = Tunnel::open(&c, &env(&home), &secrets(Some("pw"), None), None).await.unwrap_err();
    assert!(matches!(e, SshError::Connect(_)), "{e:?}");
    assert!(e.to_string().contains(&port.to_string()), "{e}");
}

#[tokio::test]
async fn plan_shows_what_will_be_used() {
    let home = temp_home(None, None);
    let ssh = home.ssh_dir();
    let e = env(&home);
    let base = SshConfig { host: "h".into(), port: 22, user: "u".into(), auth: SshAuth::Password };
    let with = |auth| SshConfig { auth, ..base.clone() };
    assert_eq!(plan(&with(SshAuth::Password), &e).await, KeyPlan::Password);
    assert_eq!(plan(&with(SshAuth::Key { path: None }), &e).await, KeyPlan::Nothing);
    assert_eq!(plan(&with(SshAuth::Agent), &e).await, KeyPlan::Nothing);
    std::fs::write(ssh.join("id_rsa"), "x").unwrap();
    assert_eq!(plan(&with(SshAuth::Key { path: None }), &e).await, KeyPlan::File(ssh.join("id_rsa")));
    let chosen = PathBuf::from("/somewhere/key");
    assert_eq!(plan(&with(SshAuth::Key { path: Some(chosen.clone()) }), &e).await, KeyPlan::File(chosen));
    // An agent socket that does not answer counts as no agent.
    let gone = SshEnv { agent: AgentSource::Socket(home.home.join("no-agent.sock")), ..e.clone() };
    assert_eq!(plan(&with(SshAuth::Key { path: None }), &gone).await, KeyPlan::File(ssh.join("id_rsa")));
    assert_eq!(plan(&with(SshAuth::Agent), &gone).await, KeyPlan::Nothing);
}

#[test]
fn tunnel_and_open_are_send() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>(_: &T) {}
    send_sync::<Tunnel>();
    send_sync::<SshEnv>();
    let home = temp_home(None, None);
    let (c, e, s) = (SshConfig::default(), env(&home), SshSecrets::default());
    let open = Tunnel::open(&c, &e, &s, None);
    send(&open);
    drop(open);
    let p = plan(&c, &e);
    send(&p);
    // What `connect` returns can carry a database connection on a spawned task.
    fn stream<T: Send + Unpin + tokio::io::AsyncRead + tokio::io::AsyncWrite>() {}
    stream::<dbee_ssh::TunnelStream>();
}

/// A stream is dropped wherever its last owner goes: in the app, on the UI thread, which runs no
/// runtime, and after the runtime it was opened on shut down. Neither panics, and the tunnel goes on.
#[test]
fn a_stream_can_be_dropped_off_the_runtime() {
    let db = require_testdb!();
    let home = temp_home(Some(&db.keys.plain), Some(&db.sshd.known_hosts_line));
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let (tunnel, on_a_plain_thread, after_shutdown) = runtime.block_on(async {
        let t = Tunnel::open(&cfg(db, SshAuth::Key { path: None }), &env(&home), &SshSecrets::default(), None).await.unwrap();
        let a = t.connect(pg14(db).0, pg14(db).1).await.unwrap();
        let b = t.connect(pg14(db).0, pg14(db).1).await.unwrap();
        (t, a, b)
    });
    std::thread::spawn(move || drop(on_a_plain_thread)).join().expect("a stream dropped on a thread without a runtime");
    assert!(!tunnel.is_closed());
    runtime.block_on(reaches_postgres(&tunnel, db));
    drop(runtime);
    std::thread::spawn(move || drop(after_shutdown)).join().expect("a stream dropped after its runtime shut down");
    drop(tunnel);
}

#[test]
fn secrets_never_show_in_debug() {
    let s = secrets(Some("hunter2"), Some("open sesame"));
    let d = format!("{s:?}");
    assert!(!d.contains("hunter2") && !d.contains("open sesame"), "{d}");
    assert!(d.contains("redacted"), "{d}");
    assert!(!format!("{:?}", SshSecrets::default()).contains("redacted"));
}

#[test]
fn errors_read_plainly() {
    let all = [
        SshError::Connect("refused".into()),
        SshError::HostKeyUnknown { host: "[h]:2222".into(), key_type: "ssh-ed25519".into(), fingerprint: "SHA256:abc".into() },
        SshError::HostKeyChanged { host: "h".into(), line: 7 },
        SshError::NeedPassphrase { key: "/k/id".into() },
        SshError::WrongPassphrase { key: "/k/id".into() },
        SshError::NeedPassword,
        SshError::NoKey,
        SshError::AuthFailed { tried: vec!["agent".into(), "key /k/id".into()] },
        SshError::KnownHosts("bad line".into()),
        SshError::Channel("prohibited".into()),
    ];
    for e in &all {
        let text = e.to_string();
        assert!(!text.is_empty() && !text.contains("Error") && !text.contains('{'), "{text}");
    }
    assert!(all[1].to_string().contains("SHA256:abc") && all[1].to_string().contains("[h]:2222"));
    assert!(all[2].to_string().contains("line 7"));
    assert!(all[3].to_string().contains("/k/id"));
    assert!(all[7].to_string().contains("agent, key /k/id"));
    let _: &dyn std::error::Error = &all[0];
}

#[test]
fn env_from_os_points_at_home_ssh() {
    // Reads the environment only; never connects anywhere.
    let e = SshEnv::from_os();
    if let Some(home) = dbee_config::home_dir() {
        assert_eq!(e.ssh_dir, Some(home.join(".ssh")));
    }
    #[cfg(unix)]
    match std::env::var_os("SSH_AUTH_SOCK").filter(|v| !v.is_empty()) {
        Some(sock) => assert_eq!(e.agent, AgentSource::Socket(PathBuf::from(sock))),
        None => assert_eq!(e.agent, AgentSource::None),
    }
}
