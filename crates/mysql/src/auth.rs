//! Logging in: the server's greeting, the TLS request, the handshake response, and the login
//! methods dbee speaks: `mysql_native_password`, `caching_sha2_password` and `sha256_password`.
//!
//! The password leaves dbee only as a scramble, encrypted with the server's RSA key, or in clear
//! over TLS: [`cleartext`] is the one place that sends it so, and it refuses without TLS. A
//! switch to a method dbee does not speak (`mysql_clear_password` among them) ends the login
//! before the password is used; a greeting that names one is answered with the flavor's default
//! method. Nothing here logs the password, a scramble or the RSA exchange.

use dbee_driver::DbError;
use mysql_common::auth::plugins::caching_sha2_password::scramble_sha256;
use mysql_common::auth::plugins::mysql_native_password::scramble_native;
use mysql_common::constants::{CapabilityFlags, MariadbCapabilities};
use mysql_common::io::ParseBuf;
use mysql_common::packets::{AuthMoreData, AuthPlugin, AuthSwitchRequest, CommonOkPacket, HandshakePacket, HandshakeResponse, SslRequest};
use mysql_common::proto::MySerialize;

use crate::errors;
use crate::flavor::Flavor;
use crate::result;
use crate::rsa::rsa_encrypt;
use crate::wire::{MAX_PAYLOAD, Packets};

/// `utf8mb4_general_ci`, the handshake's character set.
const CHARSET: u8 = 45;

/// The capabilities dbee asks for, of those the server offers; `CLIENT_SSL` and
/// `CLIENT_CONNECT_WITH_DB` are added when used. `CLIENT_MULTI_RESULTS` lets a `CALL` return
/// result sets; several statements in one text stay refused.
const WANTED: CapabilityFlags = CapabilityFlags::CLIENT_PROTOCOL_41
    .union(CapabilityFlags::CLIENT_SECURE_CONNECTION)
    .union(CapabilityFlags::CLIENT_PLUGIN_AUTH)
    .union(CapabilityFlags::CLIENT_FOUND_ROWS)
    .union(CapabilityFlags::CLIENT_TRANSACTIONS)
    .union(CapabilityFlags::CLIENT_MULTI_RESULTS)
    .union(CapabilityFlags::CLIENT_DEPRECATE_EOF);

/// The MariaDB capability dbee asks a MariaDB server for, when it offers it: the extended
/// metadata, which tells a JSON column from a text one.
pub(crate) const MARIADB_WANTED: MariadbCapabilities = MariadbCapabilities::MARIADB_CLIENT_EXTENDED_METADATA;

/// MariaDB's name for the first capability bit: a server that clears it sends MariaDB's own
/// capabilities in the greeting, and reads the client's in the requests.
const CLIENT_MYSQL: CapabilityFlags = CapabilityFlags::CLIENT_LONG_PASSWORD;

/// What the server must offer: the 4.1 protocol with plugin logins (MySQL 5.5 and newer), and
/// matched rather than changed rows in the counts Apply checks.
const REQUIRED: CapabilityFlags = CapabilityFlags::CLIENT_PROTOCOL_41
    .union(CapabilityFlags::CLIENT_SECURE_CONNECTION)
    .union(CapabilityFlags::CLIENT_PLUGIN_AUTH)
    .union(CapabilityFlags::CLIENT_FOUND_ROWS);

/// A login takes a handful of packets; a server that goes on is refused.
const MAX_LOGIN_PACKETS: usize = 8;

/// The server's answers in a `caching_sha2_password` login (in an AuthMoreData packet).
const FAST_AUTH_OK: u8 = 0x03;
const FULL_AUTH: u8 = 0x04;
/// The client's requests for the server's RSA key: `caching_sha2_password`'s, `sha256_password`'s.
const CACHING_SHA2_KEY_REQUEST: u8 = 0x02;
const SHA256_KEY_REQUEST: u8 = 0x01;

/// Whether the password may go out encrypted with the server's RSA key on a link without TLS.
/// dbee cannot check that key: whoever sits on the link can hand over one of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerKey {
    /// The user chose no encryption, or the SSH tunnel encrypts the link.
    Allowed,
    /// The user wanted encryption, and allowed the key in the connection's settings.
    AllowedWithWarning,
    Refused,
}

/// Why a login stopped before asking for the server's key: the server offers no TLS, or its TLS
/// could not be used (a handshake that failed).
const KEY_REFUSED_NO_TLS: &str = "the server wants the password protected with its own key, and this connection is not encrypted because the \
                                  server offers no TLS; turn on TLS on the server, connect through an SSH tunnel, or allow the server's \
                                  public key in this connection's settings";
const KEY_REFUSED_TLS_UNUSABLE: &str = "the server wants the password protected with its own key, and this connection is not encrypted \
                                        because the server's TLS could not be used; connect through an SSH tunnel, or allow the server's \
                                        public key in this connection's settings";

/// The link a login runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link {
    /// The stream is TLS: the password may go in clear.
    tls: bool,
    /// The server's greeting offers TLS; without `tls`, it could not be used.
    tls_offered: bool,
    /// Without TLS, whether the server's RSA key may protect the password.
    key: ServerKey,
}

impl Link {
    /// The request for the server's key, `request`, when the link allows it.
    fn key_request(self, request: u8) -> Result<Vec<u8>, DbError> {
        let refused = |text: &str| Err(DbError::Auth { message: text.into() });
        match self.key {
            ServerKey::Allowed | ServerKey::AllowedWithWarning => Ok(vec![request]),
            ServerKey::Refused if self.tls_offered => refused(KEY_REFUSED_TLS_UNUSABLE),
            ServerKey::Refused => refused(KEY_REFUSED_NO_TLS),
        }
    }

    /// A first answer by `method` would ask for a key the link refuses (`sha256_password`'s
    /// does, for a password, without TLS).
    fn refuses_first_answer_of(self, method: Method, password: &[u8]) -> bool {
        method == Method::Sha256 && !self.tls && !password.is_empty() && self.key == ServerKey::Refused
    }
}

/// Who logs in.
pub(crate) struct Login<'a> {
    pub(crate) user: &'a str,
    pub(crate) password: &'a [u8],
    pub(crate) database: Option<&'a str>,
}

/// The server's greeting (Protocol::HandshakeV10).
#[derive(Debug, Clone)]
pub(crate) struct Greeting {
    /// The version as the greeting says it; MariaDB's starts `5.5.5-`.
    pub(crate) version: String,
    version_parts: (u16, u16, u16),
    pub(crate) capabilities: CapabilityFlags,
    /// MariaDB's own capabilities; none when the server sets [`CLIENT_MYSQL`].
    mariadb: MariadbCapabilities,
    nonce: [u8; 20],
    plugin: Vec<u8>,
}

impl Greeting {
    pub(crate) fn offers_tls(&self) -> bool {
        self.capabilities.contains(CapabilityFlags::CLIENT_SSL)
    }
}

fn not_mysql() -> DbError {
    DbError::Connect("the server did not answer like a MySQL or MariaDB server".into())
}

/// A server without some of [`REQUIRED`].
fn lacks(missing: CapabilityFlags) -> DbError {
    let names: Vec<&str> = missing.iter_names().map(|(name, _)| name).collect();
    DbError::Connect(format!("the server does not offer {}, which dbee needs", names.join(", ")))
}

fn out_of_turn() -> DbError {
    DbError::Connect("the server sent a login packet out of turn".into())
}

/// The first 20 bytes of a login challenge (the server may add a NUL).
fn nonce_of(data: &[u8]) -> Result<[u8; 20], DbError> {
    data.get(..20)
        .and_then(|n| <[u8; 20]>::try_from(n).ok())
        .ok_or_else(|| DbError::Connect("the server sent a login challenge shorter than 20 bytes".into()))
}

/// The greeting's layout as MySQL's documentation gives it (Protocol::HandshakeV10), checked
/// before mysql_common reads it: its parser panics on a challenge length of 128 to 135. dbee
/// needs the 4.1 protocol with plugin logins, where the challenge's second part is
/// `max(13, challenge length - 8)` bytes; servers send a length of 21.
fn check_greeting(payload: &[u8]) -> Result<(), DbError> {
    // The protocol version, then the server version up to its NUL.
    let version_len = payload.get(1..).and_then(|p| p.iter().position(|&b| b == 0)).ok_or_else(not_mysql)?;
    let at = version_len.saturating_add(2);
    // Thread id 4, challenge 8, filler 1, capabilities 2, character set 1, status 2,
    // capabilities 2, challenge length 1, reserved 10.
    let fixed: &[u8; 31] = payload.get(at..at.saturating_add(31)).and_then(|f| f.try_into().ok()).ok_or_else(not_mysql)?;
    let caps = CapabilityFlags::from_bits_truncate(u32::from_le_bytes([fixed[13], fixed[14], fixed[18], fixed[19]]));
    if !caps.contains(REQUIRED) {
        return Err(lacks(REQUIRED.difference(caps)));
    }
    let challenge_len = usize::from(fixed[20]);
    if !(8..=127).contains(&challenge_len) {
        return Err(not_mysql());
    }
    let second_part = challenge_len.saturating_sub(8).max(13);
    if payload.len() < at.saturating_add(31).saturating_add(second_part) {
        return Err(not_mysql());
    }
    Ok(())
}

/// Read the server's first packet: its greeting, or the error it refuses the connection with.
pub(crate) fn parse_greeting(payload: &[u8]) -> Result<Greeting, DbError> {
    if payload.first() == Some(&0xFF) {
        return Err(errors::from_err_packet(payload, CapabilityFlags::empty(), Flavor::MySql));
    }
    check_greeting(payload)?;
    let hs = ParseBuf(payload).parse::<HandshakePacket>(()).map_err(|_| not_mysql())?;
    if hs.protocol_version() != 10 {
        return Err(DbError::Connect(format!("the server speaks protocol version {}; dbee speaks 10", hs.protocol_version())));
    }
    Ok(Greeting {
        version: hs.server_version_str().into_owned(),
        version_parts: hs.server_version_parsed().unwrap_or_default(),
        capabilities: hs.capabilities(),
        mariadb: if hs.capabilities().contains(CLIENT_MYSQL) { MariadbCapabilities::empty() } else { hs.mariadb_ext_capabilities() },
        nonce: nonce_of(&hs.nonce())?,
        plugin: hs.auth_plugin_name_ref().unwrap_or(b"mysql_native_password").to_vec(),
    })
}

/// What dbee's side of the session uses: [`WANTED`] as far as the server offers it, plus TLS
/// and a database at login when asked for.
pub(crate) fn capabilities(g: &Greeting, tls: bool, database: bool) -> Result<CapabilityFlags, DbError> {
    if !g.capabilities.contains(REQUIRED) {
        return Err(lacks(REQUIRED.difference(g.capabilities)));
    }
    let mut caps = WANTED & g.capabilities;
    if tls {
        caps |= CapabilityFlags::CLIENT_SSL;
    }
    if database {
        if !g.capabilities.contains(CapabilityFlags::CLIENT_CONNECT_WITH_DB) {
            return Err(DbError::Connect("the server cannot select a database at login".into()));
        }
        caps |= CapabilityFlags::CLIENT_CONNECT_WITH_DB;
    }
    Ok(caps)
}

/// The MariaDB capabilities dbee asks for: `wanted` as far as a MariaDB server offers it, and
/// none from MySQL.
pub(crate) fn mariadb_capabilities(g: &Greeting, wanted: MariadbCapabilities) -> MariadbCapabilities {
    if Flavor::from_version(&g.version) == Flavor::MariaDb { wanted & g.mariadb } else { MariadbCapabilities::empty() }
}

/// The request to start TLS (Protocol::SSLRequest), sent before the handshake response.
pub(crate) fn ssl_request(caps: CapabilityFlags, mariadb: MariadbCapabilities) -> Vec<u8> {
    let mut buf = Vec::new();
    SslRequest::new(caps, MAX_PAYLOAD as u32, CHARSET).with_mariadb_capabilities(mariadb).serialize(&mut buf);
    buf
}

/// A login method dbee speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Native,
    CachingSha2,
    Sha256,
}

/// Where a method's exchange stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The answer to the challenge is sent.
    Answered,
    /// `caching_sha2_password` took the scramble; the OK packet follows.
    FastOk,
    /// The client asked for the server's RSA key.
    WaitingForKey,
    /// The password went to the server, encrypted or over TLS.
    PasswordSent,
}

impl Method {
    fn named(name: &[u8]) -> Result<Method, DbError> {
        match name {
            b"mysql_native_password" => Ok(Method::Native),
            b"caching_sha2_password" => Ok(Method::CachingSha2),
            b"sha256_password" => Ok(Method::Sha256),
            other => {
                let shown: String = other.iter().take(64).map(|&b| if b.is_ascii_graphic() { char::from(b) } else { '?' }).collect();
                Err(DbError::Auth { message: format!("the server asks for the login method {shown}, which dbee does not support") })
            }
        }
    }

    /// The answer to a greeting that names a method dbee does not speak: the flavor's own
    /// default, which the server's auth switch then corrects.
    fn default_for(flavor: Flavor) -> Method {
        match flavor {
            Flavor::MySql => Method::CachingSha2,
            Flavor::MariaDb => Method::Native,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Method::Native => "mysql_native_password",
            Method::CachingSha2 => "caching_sha2_password",
            Method::Sha256 => "sha256_password",
        }
    }

    /// The first answer to the challenge `nonce`, and where that leaves the exchange. An empty
    /// password is an empty answer.
    fn first(self, password: &[u8], nonce: &[u8; 20], link: Link) -> Result<(Vec<u8>, Step), DbError> {
        let scrambled = |s: Option<Vec<u8>>| s.ok_or_else(|| DbError::Other("could not compute the login scramble".into()));
        Ok(match self {
            Method::Native | Method::CachingSha2 if password.is_empty() => (Vec::new(), Step::Answered),
            Method::Native => (scrambled(scramble_native(nonce, password).map(Vec::from))?, Step::Answered),
            Method::CachingSha2 => (scrambled(scramble_sha256(nonce, password).map(Vec::from))?, Step::Answered),
            Method::Sha256 if link.tls || password.is_empty() => (cleartext(password, link.tls)?, Step::PasswordSent),
            Method::Sha256 => (link.key_request(SHA256_KEY_REQUEST)?, Step::WaitingForKey),
        })
    }

    /// The answer to the server's AuthMoreData `data` in `step`, if one is due, and the next step.
    fn more(self, step: Step, data: &[u8], password: &[u8], nonce: &[u8; 20], link: Link) -> Result<(Option<Vec<u8>>, Step), DbError> {
        match (self, step, data) {
            (Method::CachingSha2, Step::Answered, [FAST_AUTH_OK]) => Ok((None, Step::FastOk)),
            (Method::CachingSha2, Step::Answered, [FULL_AUTH]) if link.tls => Ok((Some(cleartext(password, link.tls)?), Step::PasswordSent)),
            (Method::CachingSha2, Step::Answered, [FULL_AUTH]) => Ok((Some(link.key_request(CACHING_SHA2_KEY_REQUEST)?), Step::WaitingForKey)),
            (Method::CachingSha2 | Method::Sha256, Step::WaitingForKey, key) => Ok((Some(rsa_encrypt(password, nonce, key)?), Step::PasswordSent)),
            _ => Err(out_of_turn()),
        }
    }
}

/// The password as the server reads it in clear (NUL-terminated): only over TLS, or when it is
/// empty.
fn cleartext(password: &[u8], tls: bool) -> Result<Vec<u8>, DbError> {
    if !tls && !password.is_empty() {
        return Err(DbError::Auth { message: "the server asks for the password in clear text, which dbee sends only over TLS".into() });
    }
    Ok([password, &[0]].concat())
}

/// The handshake response (Protocol::HandshakeResponse41), answering with `method`.
fn handshake_response(
    g: &Greeting,
    who: &Login<'_>,
    caps: CapabilityFlags,
    mariadb: MariadbCapabilities,
    method: Method,
    answer: Vec<u8>,
) -> Vec<u8> {
    let response = HandshakeResponse::new(
        Some(answer),
        g.version_parts,
        Some(who.user.as_bytes()),
        who.database.map(str::as_bytes),
        Some(AuthPlugin::from_bytes(method.name().as_bytes())),
        caps,
        None,
        MAX_PAYLOAD as u32,
    )
    .with_mariadb_ext_capabilities(mariadb);
    let mut buf = Vec::new();
    response.serialize(&mut buf);
    // mysql_common picks the character set by the server's version; dbee always asks for utf8mb4.
    if let Some(charset) = buf.get_mut(8) {
        *charset = CHARSET;
    }
    buf
}

/// Log in after greeting `g`, on `p` (over TLS when `tls`), asking for `caps` and MariaDB's
/// `mariadb`, until the server's OK packet; true when the password went out encrypted with the
/// server's key, as `key` allows.
pub(crate) async fn login(
    p: &mut Packets,
    g: &Greeting,
    who: &Login<'_>,
    caps: CapabilityFlags,
    mariadb: MariadbCapabilities,
    tls: bool,
    key: ServerKey,
) -> Result<bool, DbError> {
    let flavor = Flavor::from_version(&g.version);
    let link = Link { tls, tls_offered: g.offers_tls(), key };
    // A method dbee does not speak, or one whose first answer the link refuses, is answered with
    // the flavor's default: the server's switch then names the account's own method.
    let mut method = match Method::named(&g.plugin) {
        Ok(named) if !link.refuses_first_answer_of(named, who.password) => named,
        _ => Method::default_for(flavor),
    };
    let mut nonce = g.nonce;
    let mut used_key = false;
    let (answer, mut step) = method.first(who.password, &nonce, link)?;
    log::debug!("mysql: logging in with {}", method.name());
    p.write(&handshake_response(g, who, caps, mariadb, method, answer)).await?;
    for _ in 0..MAX_LOGIN_PACKETS {
        let reply = p.read().await?;
        match reply.first() {
            Some(0x00) => {
                result::parse_ok::<CommonOkPacket>(&reply, caps)?;
                return Ok(used_key);
            }
            Some(0xFF) => return Err(errors::from_err_packet(&reply, caps, flavor)),
            Some(0xFE) => {
                let switch = ParseBuf(&reply).parse::<AuthSwitchRequest>(()).map_err(|_| out_of_turn())?;
                method = Method::named(switch.auth_plugin().as_bytes())?;
                nonce = nonce_of(switch.plugin_data())?;
                log::debug!("mysql: the server switched the login to {}", method.name());
                let (answer, next) = method.first(who.password, &nonce, link)?;
                step = next;
                p.write(&answer).await?;
            }
            Some(0x01) => {
                let more = ParseBuf(&reply).parse::<AuthMoreData>(()).map_err(|_| out_of_turn())?;
                let (answer, next) = method.more(step, more.data(), who.password, &nonce, link)?;
                used_key |= step == Step::WaitingForKey && next == Step::PasswordSent;
                step = next;
                if let Some(answer) = answer {
                    p.write(&answer).await?;
                }
            }
            _ => return Err(out_of_turn()),
        }
    }
    Err(DbError::Connect(format!("the login did not end within {MAX_LOGIN_PACKETS} packets")))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::rsa::tests::KEY_2048;

    const NONCE: [u8; 20] = *b"abcdefghijklmnopqrst";
    const PLAIN: Link = Link { tls: false, tls_offered: false, key: ServerKey::Allowed };
    const TLS: Link = Link { tls: true, tls_offered: true, key: ServerKey::Allowed };

    fn greeting(caps: CapabilityFlags, version: (u16, u16, u16)) -> Greeting {
        Greeting {
            version: "8.0.99".into(),
            version_parts: version,
            capabilities: caps,
            mariadb: MariadbCapabilities::empty(),
            nonce: NONCE,
            plugin: b"caching_sha2_password".to_vec(),
        }
    }

    #[test]
    fn handshake_response_is_as_documented() {
        let all = CapabilityFlags::all();
        for version in [(5, 5, 5), (8, 0, 99), (0, 0, 0)] {
            let g = greeting(all, version);
            let caps = capabilities(&g, false, true).unwrap();
            let who = Login { user: "root", password: b"pw", database: Some("shop") };
            let r = handshake_response(&g, &who, caps, MariadbCapabilities::empty(), Method::CachingSha2, vec![1; 32]);
            // capability flags, max packet size, character set, 23 bytes of filler.
            assert_eq!(r[..4], caps.bits().to_le_bytes(), "{version:?}");
            assert_eq!(r[4..8], (1u32 << 30).to_le_bytes());
            assert_eq!(r[8], CHARSET, "{version:?}");
            assert_eq!(r[9..32], [0; 23]);
            let rest = &r[32..];
            let expected = [&b"root\0"[..], &[32], &[1; 32], b"shop\0", b"caching_sha2_password\0"].concat();
            assert_eq!(rest, &expected[..], "{version:?}");
        }
        let caps = capabilities(&greeting(all, (8, 0, 0)), true, false).unwrap();
        assert!(caps.contains(WANTED | CapabilityFlags::CLIENT_SSL) && !caps.contains(CapabilityFlags::CLIENT_CONNECT_WITH_DB));
        assert!(!caps.intersects(CapabilityFlags::CLIENT_LOCAL_FILES | CapabilityFlags::CLIENT_MULTI_STATEMENTS | CapabilityFlags::CLIENT_COMPRESS));
        assert!(caps.contains(CapabilityFlags::CLIENT_MULTI_RESULTS) && !caps.contains(CapabilityFlags::CLIENT_PS_MULTI_RESULTS));
        let ssl = ssl_request(caps, MariadbCapabilities::empty());
        assert_eq!((&ssl[..4], &ssl[4..8], ssl[8], ssl.len()), (&caps.bits().to_le_bytes()[..], &(1u32 << 30).to_le_bytes()[..], CHARSET, 32));
    }

    #[test]
    fn capabilities_follow_the_server() {
        let old = greeting(CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_SECURE_CONNECTION, (5, 1, 0));
        assert!(matches!(capabilities(&old, false, false).unwrap_err(), DbError::Connect(m) if m.contains("CLIENT_PLUGIN_AUTH")));
        let no_eof = greeting(REQUIRED | CapabilityFlags::CLIENT_FOUND_ROWS, (5, 6, 0));
        assert_eq!(capabilities(&no_eof, false, false).unwrap(), REQUIRED | CapabilityFlags::CLIENT_FOUND_ROWS);
        assert_eq!(capabilities(&no_eof, false, true).unwrap_err().code(), "connect");
    }

    /// Protocol::HandshakeV10 as MySQL's documentation lays it out, with a challenge of
    /// `challenge_len` bytes announced and 13 sent in its second part.
    fn greeting_bytes(caps: CapabilityFlags, challenge_len: u8) -> Vec<u8> {
        greeting_of("8.0.99", caps, challenge_len, 0)
    }

    /// [`greeting_bytes`] of server `version`, with MariaDB's capabilities `mariadb` in the last
    /// 4 of the 10 reserved bytes, where MariaDB's documentation puts them.
    fn greeting_of(version: &str, caps: CapabilityFlags, challenge_len: u8, mariadb: u32) -> Vec<u8> {
        let caps = caps.bits().to_le_bytes();
        let reserved = [&[0; 6][..], &mariadb.to_le_bytes()].concat();
        let fixed = [&[0; 4][..], &NONCE[..8], &[0], &caps[..2], &[45], &[2, 0], &caps[2..], &[challenge_len], &reserved].concat();
        [&[10][..], version.as_bytes(), &[0], &fixed, &NONCE[8..], &[0], b"caching_sha2_password\0"].concat()
    }

    #[test]
    fn mariadb_capabilities_only_from_mariadb() {
        let extended = MariadbCapabilities::MARIADB_CLIENT_EXTENDED_METADATA;
        let offered = extended.bits() | MariadbCapabilities::MARIADB_CLIENT_PROGRESS.bits();
        let mariadb = CapabilityFlags::all().difference(CLIENT_MYSQL);
        let g = parse_greeting(&greeting_of("5.5.5-11.4.2-MariaDB", mariadb, 21, offered)).unwrap();
        assert_eq!(mariadb_capabilities(&g, MARIADB_WANTED), extended, "only what dbee wants");
        assert_eq!(mariadb_capabilities(&g, MariadbCapabilities::empty()), MariadbCapabilities::empty());
        // A server that sets CLIENT_MYSQL has no room for them; MySQL is never asked.
        let g = parse_greeting(&greeting_of("5.5.5-11.4.2-MariaDB", CapabilityFlags::all(), 21, offered)).unwrap();
        assert_eq!(mariadb_capabilities(&g, MARIADB_WANTED), MariadbCapabilities::empty());
        let g = parse_greeting(&greeting_of("8.0.99", mariadb, 21, offered)).unwrap();
        assert_eq!(mariadb_capabilities(&g, MARIADB_WANTED), MariadbCapabilities::empty());
        // MariaDB's documentation puts them in the last 4 of the 23 filler bytes, both requests.
        let g = parse_greeting(&greeting_of("5.5.5-11.4.2-MariaDB", mariadb, 21, offered)).unwrap();
        let caps = capabilities(&g, true, false).unwrap();
        let asked = mariadb_capabilities(&g, MARIADB_WANTED);
        assert!(!caps.contains(CLIENT_MYSQL));
        let who = Login { user: "root", password: b"pw", database: None };
        let r = handshake_response(&g, &who, caps, asked, Method::Native, vec![1; 20]);
        let ssl = ssl_request(caps, asked);
        for request in [&r[..32], &ssl[..]] {
            assert_eq!((&request[9..28], &request[28..32]), (&[0u8; 19][..], &extended.bits().to_le_bytes()[..]));
        }
    }

    #[test]
    fn greetings() {
        let g = parse_greeting(&greeting_bytes(CapabilityFlags::all(), 21)).unwrap();
        assert_eq!((g.version.as_str(), g.version_parts, g.nonce, g.plugin.as_slice()), ("8.0.99", (8, 0, 99), NONCE, &b"caching_sha2_password"[..]));
        assert!(g.offers_tls());
        // A challenge length under 8 or past the bytes sent; a server without plugin logins.
        // Found by hostile_greetings_never_panic: mysql_common's parser panics on 128 to 135.
        for len in [0, 7, 60, 127, 128, 135, 255] {
            assert_eq!(parse_greeting(&greeting_bytes(CapabilityFlags::all(), len)).unwrap_err().code(), "connect", "{len}");
        }
        let e = parse_greeting(&greeting_bytes(CapabilityFlags::CLIENT_PROTOCOL_41, 21)).unwrap_err();
        assert_eq!(
            e,
            DbError::Connect("the server does not offer CLIENT_FOUND_ROWS, CLIENT_SECURE_CONNECTION, CLIENT_PLUGIN_AUTH, which dbee needs".into())
        );
        let no_found_rows = CapabilityFlags::all().difference(CapabilityFlags::CLIENT_FOUND_ROWS);
        let e = parse_greeting(&greeting_bytes(no_found_rows, 21)).unwrap_err();
        assert_eq!(e, DbError::Connect("the server does not offer CLIENT_FOUND_ROWS, which dbee needs".into()));
        let mut panicked = vec![10];
        panicked.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 128, 0, 168, 0, 0, 0, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(parse_greeting(&panicked).unwrap_err().code(), "connect");
    }

    #[test]
    fn greetings_that_are_not_mysql() {
        assert_eq!(parse_greeting(&[]).unwrap_err().code(), "connect");
        assert_eq!(parse_greeting(b"\x09hello\0").unwrap_err().code(), "connect");
        let refused = [&[0xFF][..], &1130u16.to_le_bytes(), b"Host is not allowed to connect"].concat();
        assert!(matches!(parse_greeting(&refused).unwrap_err(), DbError::Server(s) if s.code == "1130"));
    }

    #[test]
    fn the_methods_exchanges() {
        let pw = b"secret";
        // Empty passwords answer empty, except sha256's, which is the bare terminator.
        assert_eq!(Method::Native.first(b"", &NONCE, PLAIN).unwrap(), (vec![], Step::Answered));
        assert_eq!(Method::CachingSha2.first(b"", &NONCE, PLAIN).unwrap(), (vec![], Step::Answered));
        assert_eq!(Method::Sha256.first(b"", &NONCE, PLAIN).unwrap(), (vec![0], Step::PasswordSent));
        assert_eq!(Method::Native.first(pw, &NONCE, PLAIN).unwrap().0.len(), 20);
        assert_eq!(Method::CachingSha2.first(pw, &NONCE, PLAIN).unwrap().0.len(), 32);
        // sha256_password: the password over TLS, else a request for the key.
        assert_eq!(Method::Sha256.first(pw, &NONCE, TLS).unwrap(), (b"secret\0".to_vec(), Step::PasswordSent));
        assert_eq!(Method::Sha256.first(pw, &NONCE, PLAIN).unwrap(), (vec![SHA256_KEY_REQUEST], Step::WaitingForKey));
        // caching_sha2_password: the fast path, or the full one.
        let more = |step, data: &[u8], tls| Method::CachingSha2.more(step, data, pw, &NONCE, Link { tls, ..PLAIN });
        assert_eq!(more(Step::Answered, &[FAST_AUTH_OK], false).unwrap(), (None, Step::FastOk));
        assert_eq!(more(Step::Answered, &[FULL_AUTH], true).unwrap(), (Some(b"secret\0".to_vec()), Step::PasswordSent));
        assert_eq!(more(Step::Answered, &[FULL_AUTH], false).unwrap(), (Some(vec![CACHING_SHA2_KEY_REQUEST]), Step::WaitingForKey));
        let (sent, step) = more(Step::WaitingForKey, KEY_2048.as_bytes(), false).unwrap();
        assert_eq!((sent.map(|s| s.len()), step), (Some(256), Step::PasswordSent));
        // Anything else is out of turn.
        for (step, data) in [(Step::FastOk, &[FULL_AUTH][..]), (Step::PasswordSent, &[FAST_AUTH_OK]), (Step::Answered, &[9])] {
            assert_eq!(more(step, data, false).unwrap_err().code(), "connect", "{step:?} {data:?}");
        }
        assert_eq!(Method::Native.more(Step::Answered, &[FULL_AUTH], pw, &NONCE, PLAIN).unwrap_err().code(), "connect");
        assert_eq!(Method::Sha256.more(Step::Answered, KEY_2048.as_bytes(), pw, &NONCE, PLAIN).unwrap_err().code(), "connect");
    }

    #[test]
    fn the_servers_key_is_asked_for_only_when_allowed() {
        let pw = b"secret";
        for key in [ServerKey::Allowed, ServerKey::AllowedWithWarning] {
            let link = Link { key, ..PLAIN };
            assert_eq!(Method::Sha256.first(pw, &NONCE, link).unwrap(), (vec![SHA256_KEY_REQUEST], Step::WaitingForKey));
            let full = Method::CachingSha2.more(Step::Answered, &[FULL_AUTH], pw, &NONCE, link).unwrap();
            assert_eq!(full, (Some(vec![CACHING_SHA2_KEY_REQUEST]), Step::WaitingForKey));
        }
        // The text says whether the server offers no TLS, or offers TLS that could not be used.
        let no_tls = Link { key: ServerKey::Refused, ..PLAIN };
        let unusable = Link { tls_offered: true, ..no_tls };
        for (link, text) in [(no_tls, KEY_REFUSED_NO_TLS), (unusable, KEY_REFUSED_TLS_UNUSABLE)] {
            let refused = DbError::Auth { message: text.into() };
            assert_eq!(Method::Sha256.first(pw, &NONCE, link).unwrap_err(), refused);
            assert_eq!(Method::CachingSha2.more(Step::Answered, &[FULL_AUTH], pw, &NONCE, link).unwrap_err(), refused);
        }
        assert!(KEY_REFUSED_NO_TLS.contains("turn on TLS on the server") && !KEY_REFUSED_TLS_UNUSABLE.contains("turn on TLS"));
        assert!(KEY_REFUSED_TLS_UNUSABLE.contains("could not be used") && KEY_REFUSED_TLS_UNUSABLE.contains("SSH tunnel"));
        // Over TLS, or with nothing to protect, no key is asked for.
        assert_eq!(Method::Sha256.first(pw, &NONCE, Link { tls: true, ..no_tls }).unwrap().1, Step::PasswordSent);
        assert_eq!(Method::Sha256.first(b"", &NONCE, no_tls).unwrap(), (vec![0], Step::PasswordSent));
        assert_eq!(Method::CachingSha2.more(Step::Answered, &[FULL_AUTH], pw, &NONCE, Link { tls: true, ..no_tls }).unwrap().1, Step::PasswordSent);
        // Only sha256_password's first answer asks for the key, and only for a password.
        assert!(no_tls.refuses_first_answer_of(Method::Sha256, pw));
        assert!(!no_tls.refuses_first_answer_of(Method::Sha256, b"") && !no_tls.refuses_first_answer_of(Method::CachingSha2, pw));
        assert!(!PLAIN.refuses_first_answer_of(Method::Sha256, pw) && !Link { tls: true, ..no_tls }.refuses_first_answer_of(Method::Sha256, pw));
    }

    #[test]
    fn the_password_goes_in_clear_only_over_tls() {
        assert_eq!(cleartext(b"pw", false).unwrap_err().code(), "auth");
        assert_eq!(cleartext(b"pw", true).unwrap(), b"pw\0");
        assert_eq!(cleartext(b"", false).unwrap(), b"\0");
    }

    #[test]
    fn unknown_methods_are_auth_errors() {
        for name in [&b"mysql_clear_password"[..], b"client_ed25519", b"", &[0xFF; 300]] {
            let e = Method::named(name).unwrap_err();
            assert_eq!(e.code(), "auth");
            assert!(e.to_string().len() < 200, "{e}");
        }
    }

    proptest! {
        #[test]
        fn hostile_greetings_never_panic(
            random in prop::collection::vec(any::<u8>(), 0..300),
            edits in prop::collection::vec((0usize..80, any::<u8>()), 0..6),
            cut in 0usize..80,
            len in any::<u8>(),
        ) {
            let _ = parse_greeting(&random);
            let _ = parse_greeting(&[&[10][..], &random].concat());
            let mut g = greeting_bytes(CapabilityFlags::from_bits_truncate(u32::from_le_bytes([random.first().copied().unwrap_or(0xFF), 0xFF, 0xFF, 0xFF])), len);
            for (at, byte) in edits {
                if let Some(b) = g.get_mut(at) {
                    *b = byte;
                }
            }
            let _ = parse_greeting(&g);
            g.truncate(cut);
            let _ = parse_greeting(&g);
        }
    }
}
