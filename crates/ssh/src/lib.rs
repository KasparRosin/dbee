//! dbee SSH tunnel (layer L2): the host key check against `known_hosts`, key, agent and password
//! login, and a new direct-tcpip channel for every connection made through the tunnel.
//!
//! Everything the tunnel reads from the machine comes in through [`SshEnv`]: the `.ssh`
//! directory and the agent. Tests and agent runs pass a throwaway one, so they never touch the
//! real `~/.ssh` or SSH agent. Logs name at most the auth method and the host, never a password,
//! passphrase or key.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod keys;
pub mod known_hosts;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dbee_config::{SshAuth, SshConfig};
use russh::client::{self, AuthResult, Handle, Msg};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{AgentAuthError, ChannelOpenFailure, ChannelStream, Disconnect, Preferred};

use crate::keys::LoadError;
use crate::known_hosts::Verdict;

/// How long connecting (TCP and the SSH handshake), each login step and opening a channel may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// A keepalive goes out after this long without traffic; three unanswered ones close the tunnel.
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/// How long the agent may take to answer when listing its identities.
const AGENT_TIMEOUT: Duration = Duration::from_secs(5);

/// The Windows OpenSSH agent's named pipe.
pub const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// Where the SSH agent listens.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AgentSource {
    /// No agent.
    #[default]
    None,
    /// A unix socket (`SSH_AUTH_SOCK`). Unix only.
    Socket(PathBuf),
    /// A named pipe, e.g. [`OPENSSH_AGENT_PIPE`]. Windows only.
    Pipe(String),
}

/// What the tunnel may use from the machine: the `.ssh` directory (default keys and
/// `known_hosts`) and the agent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SshEnv {
    pub ssh_dir: Option<PathBuf>,
    pub agent: AgentSource,
}

impl SshEnv {
    /// The real environment: `<home>/.ssh` and the agent the OS points to (`SSH_AUTH_SOCK` on
    /// unix, the OpenSSH agent pipe on Windows when it exists). Only for the app itself.
    pub fn from_os() -> SshEnv {
        SshEnv { ssh_dir: dbee_config::home_dir().map(|h| h.join(".ssh")), agent: os_agent() }
    }

    fn known_hosts(&self) -> Option<PathBuf> {
        self.ssh_dir.as_ref().map(|d| d.join("known_hosts"))
    }
}

#[cfg(unix)]
fn os_agent() -> AgentSource {
    std::env::var_os("SSH_AUTH_SOCK").filter(|v| !v.is_empty()).map_or(AgentSource::None, |v| AgentSource::Socket(PathBuf::from(v)))
}

#[cfg(windows)]
fn os_agent() -> AgentSource {
    if Path::new(OPENSSH_AGENT_PIPE).exists() { AgentSource::Pipe(OPENSSH_AGENT_PIPE.to_owned()) } else { AgentSource::None }
}

#[cfg(not(any(unix, windows)))]
fn os_agent() -> AgentSource {
    AgentSource::None
}

/// What a login will use, for the connection form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPlan {
    /// The agent, which holds this many identities.
    Agent { identities: usize },
    /// This key file.
    File(PathBuf),
    /// The password.
    Password,
    /// Nothing usable: no agent identity and no key file.
    Nothing,
}

/// What [`Tunnel::open`] will log in with for `cfg`: for automatic key choice the agent when it
/// has identities, else [`keys::default_key`]. Asks the agent for its identities; reads no key.
pub async fn plan(cfg: &SshConfig, env: &SshEnv) -> KeyPlan {
    match &cfg.auth {
        SshAuth::Password => KeyPlan::Password,
        SshAuth::Key { path: Some(path) } => KeyPlan::File(path.clone()),
        SshAuth::Agent => match agent_identities(&env.agent).await {
            Some((_, ids)) => KeyPlan::Agent { identities: ids.len() },
            None => KeyPlan::Nothing,
        },
        SshAuth::Key { path: None } => match agent_identities(&env.agent).await {
            Some((_, ids)) if !ids.is_empty() => KeyPlan::Agent { identities: ids.len() },
            _ => keys::default_key(env).map_or(KeyPlan::Nothing, KeyPlan::File),
        },
    }
}

/// The SSH password and key passphrase for one login. `Debug` hides both.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SshSecrets {
    pub password: Option<String>,
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for SshSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hide = |s: &Option<String>| s.as_ref().map(|_| "<redacted>");
        f.debug_struct("SshSecrets").field("password", &hide(&self.password)).field("passphrase", &hide(&self.passphrase)).finish()
    }
}

/// Why a tunnel could not be opened or used. The text never contains a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshError {
    /// The SSH server could not be reached, or the connection broke.
    Connect(String),
    /// The server's host key is not in `known_hosts`. Ask the user; open again with
    /// `trust = Some(fingerprint)` to accept and learn it. `host` is `host` or `[host]:port`.
    HostKeyUnknown { host: String, key_type: String, fingerprint: String },
    /// `known_hosts` holds another key of this type for the server, at `line` (1-based). Never
    /// overridable.
    HostKeyChanged { host: String, line: usize },
    /// The key file is encrypted; open again with `SshSecrets::passphrase`.
    NeedPassphrase { key: PathBuf },
    /// The passphrase does not open the key file.
    WrongPassphrase { key: PathBuf },
    /// Password login and no password given.
    NeedPassword,
    /// No agent identity and no key file to log in with.
    NoKey,
    /// The server refused every login; `tried` lists them (`agent`, `key <path>`, `password`).
    AuthFailed { tried: Vec<String> },
    /// `known_hosts` could not be read or has a malformed line for this server.
    KnownHosts(String),
    /// A channel through the tunnel could not be opened.
    Channel(String),
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshError::Connect(m) => write!(f, "could not connect to the SSH server: {m}"),
            SshError::HostKeyUnknown { host, key_type, fingerprint } => {
                write!(f, "the SSH server {host} is not known yet; its {key_type} host key has the fingerprint {fingerprint}")
            }
            SshError::HostKeyChanged { host, line } => write!(
                f,
                "the host key of the SSH server {host} has changed since it was saved (known_hosts line {line}); \
                 this can mean someone is intercepting the connection, so dbee will not connect"
            ),
            SshError::NeedPassphrase { key } => write!(f, "the SSH key {} is protected by a passphrase", key.display()),
            SshError::WrongPassphrase { key } => write!(f, "the passphrase does not open the SSH key {}", key.display()),
            SshError::NeedPassword => write!(f, "the SSH login needs a password"),
            SshError::NoKey => {
                write!(f, "there is no SSH key to log in with: the agent has none, and ~/.ssh has no id_ed25519, id_ecdsa or id_rsa")
            }
            SshError::AuthFailed { tried } => write!(f, "the SSH server refused the login (tried: {})", tried.join(", ")),
            SshError::KnownHosts(m) => write!(f, "could not check the SSH server against known_hosts: {m}"),
            SshError::Channel(m) => write!(f, "could not open the tunnel: {m}"),
        }
    }
}

impl std::error::Error for SshError {}

/// The russh handler: checks the host key and leaves its verdict in a slot that [`Tunnel::open`]
/// reads after russh refused the handshake.
struct Client {
    host: String,
    port: u16,
    known_hosts: Option<PathBuf>,
    trust: Option<String>,
    verdict: Arc<Mutex<Option<Verdict>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, server: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let verdict = match server {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                known_hosts::check(&self.host, self.port, key, self.known_hosts.as_deref(), self.trust.as_deref())
            }
            PublicKeyOrCertificate::Certificate(_) => Verdict::Unsupported("the server offered a host certificate, which dbee does not check".into()),
        };
        let accepted = verdict.accepts();
        if let Ok(mut slot) = self.verdict.lock() {
            *slot = Some(verdict);
        }
        Ok(accepted)
    }
}

/// `host:port`, with brackets around an IPv6 address.
fn address(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

/// An open SSH connection that tunnels TCP connections. Cheap to share: every
/// [`connect`](Tunnel::connect) opens its own channel.
pub struct Tunnel {
    handle: Handle<Client>,
    used: String,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tunnel").field("used", &self.used).field("closed", &self.is_closed()).finish()
    }
}

impl Tunnel {
    /// Connect to the SSH server, check its host key and log in. `trust` is the SHA256
    /// fingerprint the user accepted after [`SshError::HostKeyUnknown`].
    pub async fn open(cfg: &SshConfig, env: &SshEnv, secrets: &SshSecrets, trust: Option<&str>) -> Result<Tunnel, SshError> {
        let entry = known_hosts::host_port(&cfg.host, cfg.port);
        let addr = address(&cfg.host, cfg.port);
        let known_hosts = env.known_hosts();
        let verdict = Arc::new(Mutex::new(None));
        let handler = Client {
            host: cfg.host.clone(),
            port: cfg.port,
            known_hosts: known_hosts.clone(),
            trust: trust.map(str::to_owned),
            verdict: Arc::clone(&verdict),
        };
        let key_order = known_hosts::preferred_key_algorithms(&cfg.host, cfg.port, known_hosts.as_deref());
        let config = client::Config {
            keepalive_interval: Some(KEEPALIVE),
            keepalive_max: 3,
            nodelay: true,
            preferred: Preferred { key: Cow::Owned(key_order), ..Preferred::DEFAULT },
            ..client::Config::default()
        };
        log::info!("ssh: connecting to {addr}");
        let connecting = client::connect(Arc::new(config), (cfg.host.as_str(), cfg.port), handler);
        let mut handle = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
            Err(_) => return Err(SshError::Connect(format!("{addr} did not answer within {} s", CONNECT_TIMEOUT.as_secs()))),
            Ok(Ok(handle)) => handle,
            Ok(Err(e)) => {
                let verdict = verdict.lock().ok().and_then(|mut slot| slot.take());
                return Err(match verdict {
                    Some(Verdict::Unknown { key_type, fingerprint }) => SshError::HostKeyUnknown { host: entry, key_type, fingerprint },
                    Some(Verdict::Changed { line }) => SshError::HostKeyChanged { host: entry, line },
                    Some(Verdict::Failed(m)) => SshError::KnownHosts(m),
                    Some(Verdict::Unsupported(m)) => SshError::Connect(format!("{addr}: {m}")),
                    _ => SshError::Connect(format!("{addr}: {e}")),
                });
            }
        };
        match login(&mut handle, cfg, env, secrets, &addr).await {
            Ok(used) => Ok(Tunnel { handle, used }),
            Err(e) => {
                let _ = handle.disconnect(Disconnect::ByApplication, "", "en").await;
                Err(e)
            }
        }
    }

    /// A new direct-tcpip channel to `host:port`, as the SSH server resolves it.
    pub async fn connect(&self, host: &str, port: u16) -> Result<ChannelStream<Msg>, SshError> {
        let target = address(host, port);
        if self.is_closed() {
            return Err(SshError::Channel(format!("the SSH connection is closed, so {target} cannot be reached")));
        }
        let opening = self.handle.channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0);
        match tokio::time::timeout(CONNECT_TIMEOUT, opening).await {
            Err(_) => Err(SshError::Channel(format!("the SSH server did not open a channel to {target} within {} s", CONNECT_TIMEOUT.as_secs()))),
            Ok(Err(e)) => Err(SshError::Channel(channel_error(&target, e))),
            Ok(Ok(channel)) => Ok(channel.into_stream()),
        }
    }

    /// What logged in: `agent`, `key <path>` or `password`.
    pub fn used(&self) -> &str {
        &self.used
    }

    /// The SSH connection has ended (closed, dropped by the server, or keepalives unanswered).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Disconnect, and wait (briefly) for the connection to end.
    pub async fn close(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "", "en").await;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !self.handle.is_closed() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

fn channel_error(target: &str, e: russh::Error) -> String {
    match e {
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::AdministrativelyProhibited) => {
            format!("the SSH server does not allow connections to {target}")
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ConnectFailed) => format!("the SSH server could not reach {target}"),
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::UnknownChannelType) => "the SSH server does not offer tunnels".to_owned(),
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::ResourceShortage) => {
            format!("the SSH server has no resources left for a connection to {target}")
        }
        russh::Error::ChannelOpenFailure(ChannelOpenFailure::Other { reason, .. }) => {
            format!("the SSH server refused a connection to {target}: {reason}")
        }
        other => format!("{target}: {other}"),
    }
}

/// The agent connection, whatever its transport.
type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

async fn connect_agent(source: &AgentSource) -> Option<Agent> {
    match source {
        AgentSource::None => None,
        #[cfg(unix)]
        AgentSource::Socket(path) => AgentClient::connect_uds(path).await.ok().map(|a| a.dynamic()),
        #[cfg(windows)]
        AgentSource::Pipe(name) => AgentClient::connect_named_pipe(name).await.ok().map(|a| a.dynamic()),
        _ => None,
    }
}

/// The agent and its identities, or `None` when there is no agent or it does not answer.
async fn agent_identities(source: &AgentSource) -> Option<(Agent, Vec<AgentIdentity>)> {
    let asking = async {
        let mut agent = connect_agent(source).await?;
        let ids = agent.request_identities().await.ok()?;
        Some((agent, ids))
    };
    tokio::time::timeout(AGENT_TIMEOUT, asking).await.ok().flatten()
}

/// The login steps of one [`Tunnel::open`], and what they tried.
struct Login<'a> {
    handle: &'a mut Handle<Client>,
    user: &'a str,
    addr: &'a str,
    tried: Vec<String>,
}

impl Login<'_> {
    /// One auth request, bounded by [`CONNECT_TIMEOUT`]: did the server accept it?
    async fn ask<E: std::fmt::Display>(addr: &str, request: impl Future<Output = Result<AuthResult, E>>) -> Result<bool, SshError> {
        match tokio::time::timeout(CONNECT_TIMEOUT, request).await {
            Err(_) => Err(SshError::Connect(format!("{addr} did not answer the login within {} s", CONNECT_TIMEOUT.as_secs()))),
            Ok(Err(e)) => Err(SshError::Connect(format!("{addr}: the connection broke during login: {e}"))),
            Ok(Ok(result)) => Ok(result.success()),
        }
    }

    /// The hash for an RSA signature that the server accepts; `None` for other keys.
    async fn rsa_hash(&self, is_rsa: bool) -> Option<HashAlg> {
        if !is_rsa {
            return None;
        }
        tokio::time::timeout(CONNECT_TIMEOUT, self.handle.best_supported_rsa_hash()).await.ok().and_then(Result::ok).flatten().flatten()
    }

    async fn password(&mut self, password: &str) -> Result<bool, SshError> {
        self.tried.push("password".to_owned());
        log::debug!("ssh: password login on {}", self.addr);
        Self::ask(self.addr, self.handle.authenticate_password(self.user, password)).await
    }

    /// Each distinct agent key in turn until one is accepted.
    async fn agent(&mut self, mut agent: Agent, ids: Vec<AgentIdentity>) -> Result<bool, SshError> {
        self.tried.push("agent".to_owned());
        let mut seen: Vec<PublicKey> = Vec::new();
        for id in &ids {
            let key = id.public_key().into_owned();
            if seen.contains(&key) {
                continue;
            }
            seen.push(key.clone());
            if self.handle.is_closed() {
                return Ok(false);
            }
            let hash = self.rsa_hash(key.algorithm().is_rsa()).await;
            log::debug!("ssh: agent login on {}", self.addr);
            let request = self.handle.authenticate_publickey_with(self.user, key, hash, &mut agent);
            match tokio::time::timeout(CONNECT_TIMEOUT, request).await {
                Ok(Ok(result)) if result.success() => return Ok(true),
                // Refused, or the agent would not sign with this key: try the next one.
                Ok(Ok(_)) | Ok(Err(AgentAuthError::Key(_))) => {}
                Ok(Err(e)) => return Err(SshError::Connect(format!("{}: the connection broke during login: {e}", self.addr))),
                Err(_) => return Err(SshError::Connect(format!("{} did not answer the login within {} s", self.addr, CONNECT_TIMEOUT.as_secs()))),
            }
        }
        Ok(false)
    }

    /// The key file at `path`. A file that cannot be read counts as tried, with the reason.
    async fn key_file(&mut self, path: &Path, passphrase: Option<&str>) -> Result<bool, SshError> {
        let key = match keys::load_async(path, passphrase).await {
            Ok(key) => key,
            Err(LoadError::NeedPassphrase) => return Err(SshError::NeedPassphrase { key: path.to_path_buf() }),
            Err(LoadError::WrongPassphrase) => return Err(SshError::WrongPassphrase { key: path.to_path_buf() }),
            Err(LoadError::Unreadable(why)) => {
                self.tried.push(format!("key {} (could not be read: {why})", path.display()));
                return Ok(false);
            }
        };
        self.tried.push(format!("key {}", path.display()));
        if self.handle.is_closed() {
            return Ok(false);
        }
        let hash = self.rsa_hash(key.algorithm().is_rsa()).await;
        log::debug!("ssh: key login on {}", self.addr);
        Self::ask(self.addr, self.handle.authenticate_publickey(self.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))).await
    }
}

/// Log in as `cfg.auth` says; on success, what was used (see [`Tunnel::used`]).
async fn login(handle: &mut Handle<Client>, cfg: &SshConfig, env: &SshEnv, secrets: &SshSecrets, addr: &str) -> Result<String, SshError> {
    let mut l = Login { handle, user: &cfg.user, addr, tried: Vec::new() };
    let passphrase = secrets.passphrase.as_deref();
    let used = match &cfg.auth {
        SshAuth::Password => {
            let password = secrets.password.as_deref().ok_or(SshError::NeedPassword)?;
            l.password(password).await?.then(|| "password".to_owned())
        }
        SshAuth::Agent => {
            let (agent, ids) = agent_identities(&env.agent).await.filter(|(_, ids)| !ids.is_empty()).ok_or(SshError::NoKey)?;
            l.agent(agent, ids).await?.then(|| "agent".to_owned())
        }
        SshAuth::Key { path: Some(path) } => l.key_file(path, passphrase).await?.then(|| format!("key {}", path.display())),
        SshAuth::Key { path: None } => {
            let mut used = None;
            if let Some((agent, ids)) = agent_identities(&env.agent).await.filter(|(_, ids)| !ids.is_empty())
                && l.agent(agent, ids).await?
            {
                used = Some("agent".to_owned());
            }
            if used.is_none() {
                match keys::default_key(env) {
                    Some(path) => used = l.key_file(&path, passphrase).await?.then(|| format!("key {}", path.display())),
                    None if l.tried.is_empty() => return Err(SshError::NoKey),
                    None => {}
                }
            }
            used
        }
    };
    match used {
        Some(used) => {
            log::info!("ssh: logged in to {addr} with {}", if used.starts_with("key ") { "a key file" } else { used.as_str() });
            Ok(used)
        }
        None => Err(SshError::AuthFailed { tried: l.tried }),
    }
}
