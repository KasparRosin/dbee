//! Logging in: the server's greeting, the TLS request, the handshake response, and the login
//! methods dbee speaks: `mysql_native_password`, `caching_sha2_password` and `sha256_password`.
//!
//! The password leaves dbee only as a scramble, encrypted with the server's RSA key, or in clear
//! over TLS: [`cleartext`] is the one place that sends it so, and it refuses without TLS. A
//! method dbee does not speak (`mysql_clear_password` among them) ends the login before the
//! password is used. Nothing here logs the password, a scramble or the RSA exchange.

use std::ops::RangeInclusive;

use dbee_driver::DbError;
use mysql_common::auth::plugins::caching_sha2_password::scramble_sha256;
use mysql_common::auth::plugins::mysql_native_password::scramble_native;
use mysql_common::constants::CapabilityFlags;
use mysql_common::crypto::der::{PubKeyFileType, parse_pub_key, pem_to_der};
use mysql_common::crypto::rsa::{GetRandom, Pkcs1OaepPadding, PublicKey};
use mysql_common::io::ParseBuf;
use mysql_common::packets::{AuthMoreData, AuthPlugin, AuthSwitchRequest, CommonOkPacket, HandshakePacket, HandshakeResponse, SslRequest};
use mysql_common::proto::MySerialize;

use crate::connect::Flavor;
use crate::errors;
use crate::wire::{self, MAX_PAYLOAD, Packets};

/// `utf8mb4_general_ci`, the handshake's character set.
const CHARSET: u8 = 45;

/// The capabilities dbee asks for, of those the server offers; `CLIENT_SSL` and
/// `CLIENT_CONNECT_WITH_DB` are added when used.
const WANTED: CapabilityFlags = CapabilityFlags::CLIENT_PROTOCOL_41
    .union(CapabilityFlags::CLIENT_SECURE_CONNECTION)
    .union(CapabilityFlags::CLIENT_PLUGIN_AUTH)
    .union(CapabilityFlags::CLIENT_FOUND_ROWS)
    .union(CapabilityFlags::CLIENT_TRANSACTIONS)
    .union(CapabilityFlags::CLIENT_DEPRECATE_EOF);

/// What the server must offer: the 4.1 protocol with plugin logins (MySQL 5.5 and newer).
const REQUIRED: CapabilityFlags =
    CapabilityFlags::CLIENT_PROTOCOL_41.union(CapabilityFlags::CLIENT_SECURE_CONNECTION).union(CapabilityFlags::CLIENT_PLUGIN_AUTH);

/// A login takes a handful of packets; a server that goes on is refused.
const MAX_LOGIN_PACKETS: usize = 8;

/// The server's answers in a `caching_sha2_password` login (in an AuthMoreData packet).
const FAST_AUTH_OK: u8 = 0x03;
const FULL_AUTH: u8 = 0x04;
/// The client's requests for the server's RSA key: `caching_sha2_password`'s, `sha256_password`'s.
const CACHING_SHA2_KEY_REQUEST: u8 = 0x02;
const SHA256_KEY_REQUEST: u8 = 0x01;

/// The server's RSA key: a PEM text this long at most, a modulus of these bits, an exponent of
/// these bits (65537 has 17).
const MAX_KEY_PEM: usize = 16 * 1024;
const MODULUS_BITS: RangeInclusive<u64> = 1024..=16_384;
const EXPONENT_BITS: RangeInclusive<u64> = 2..=64;
/// What OAEP with SHA-1 takes of each RSA block: twice the hash and two bytes.
const OAEP_OVERHEAD: usize = 2 * 20 + 2;

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

fn too_old() -> DbError {
    DbError::Connect("the server is older than MySQL 5.5's protocol (plugin logins), which dbee needs".into())
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
        return Err(too_old());
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
        nonce: nonce_of(&hs.nonce())?,
        plugin: hs.auth_plugin_name_ref().unwrap_or(b"mysql_native_password").to_vec(),
    })
}

/// What dbee's side of the session uses: [`WANTED`] as far as the server offers it, plus TLS
/// and a database at login when asked for.
pub(crate) fn capabilities(g: &Greeting, tls: bool, database: bool) -> Result<CapabilityFlags, DbError> {
    if !g.capabilities.contains(REQUIRED) {
        return Err(too_old());
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

/// The request to start TLS (Protocol::SSLRequest), sent before the handshake response.
pub(crate) fn ssl_request(caps: CapabilityFlags) -> Vec<u8> {
    let mut buf = Vec::new();
    SslRequest::new(caps, MAX_PAYLOAD as u32, CHARSET).serialize(&mut buf);
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

    fn name(self) -> &'static str {
        match self {
            Method::Native => "mysql_native_password",
            Method::CachingSha2 => "caching_sha2_password",
            Method::Sha256 => "sha256_password",
        }
    }

    /// The first answer to the challenge `nonce`, and where that leaves the exchange. An empty
    /// password is an empty answer.
    fn first(self, password: &[u8], nonce: &[u8; 20], tls: bool) -> Result<(Vec<u8>, Step), DbError> {
        let scrambled = |s: Option<Vec<u8>>| s.ok_or_else(|| DbError::Other("could not compute the login scramble".into()));
        Ok(match self {
            Method::Native | Method::CachingSha2 if password.is_empty() => (Vec::new(), Step::Answered),
            Method::Native => (scrambled(scramble_native(nonce, password).map(Vec::from))?, Step::Answered),
            Method::CachingSha2 => (scrambled(scramble_sha256(nonce, password).map(Vec::from))?, Step::Answered),
            Method::Sha256 if tls || password.is_empty() => (cleartext(password, tls)?, Step::PasswordSent),
            Method::Sha256 => (vec![SHA256_KEY_REQUEST], Step::WaitingForKey),
        })
    }

    /// The answer to the server's AuthMoreData `data` in `step`, if one is due, and the next step.
    fn more(self, step: Step, data: &[u8], password: &[u8], nonce: &[u8; 20], tls: bool) -> Result<(Option<Vec<u8>>, Step), DbError> {
        match (self, step, data) {
            (Method::CachingSha2, Step::Answered, [FAST_AUTH_OK]) => Ok((None, Step::FastOk)),
            (Method::CachingSha2, Step::Answered, [FULL_AUTH]) if tls => Ok((Some(cleartext(password, tls)?), Step::PasswordSent)),
            (Method::CachingSha2, Step::Answered, [FULL_AUTH]) => Ok((Some(vec![CACHING_SHA2_KEY_REQUEST]), Step::WaitingForKey)),
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

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const BIT_STRING: u8 = 0x03;
/// The algorithm of a PKCS #8 RSA key: the OID `rsaEncryption` and no parameters.
const RSA_ENCRYPTION: &[u8] = &[0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00];

/// One DER element at the head of `buf`: its tag, its content, and the bytes after it. Lengths
/// take at most four bytes; the indefinite form is not DER.
fn der_element(buf: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = buf.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = match first {
        0..=0x7F => (usize::from(first), rest),
        0x81..=0x84 => {
            let (bytes, rest) = rest.split_at_checked(usize::from(first & 0x7F))?;
            (bytes.iter().fold(0usize, |n, &b| n << 8 | usize::from(b)), rest)
        }
        _ => return None,
    };
    let (content, rest) = rest.split_at_checked(len)?;
    Some((tag, content, rest))
}

/// An RSA public key's DER (PKCS #1's RSAPublicKey, or PKCS #8's SubjectPublicKeyInfo around
/// it) in the standard shape, checked before mysql_common reads it: its parser panics on
/// hostile lengths.
fn check_rsa_der(der: &[u8], kind: PubKeyFileType) -> Option<()> {
    let key = match kind {
        PubKeyFileType::Pkcs1 => der,
        PubKeyFileType::Pkcs8 => {
            let (SEQUENCE, info, []) = der_element(der)? else { return None };
            let (SEQUENCE, algorithm, rest) = der_element(info)? else { return None };
            let (BIT_STRING, bits, []) = der_element(rest)? else { return None };
            if algorithm != RSA_ENCRYPTION {
                return None;
            }
            bits.strip_prefix(&[0])?
        }
    };
    let (SEQUENCE, numbers, []) = der_element(key)? else { return None };
    let (INTEGER, modulus, rest) = der_element(numbers)? else { return None };
    let (INTEGER, exponent, []) = der_element(rest)? else { return None };
    (!modulus.is_empty() && !exponent.is_empty()).then_some(())
}

/// A key dbee encrypts with: a modulus and an exponent of sizes real keys have.
fn usable_key(modulus_bits: u64, exponent_bits: u64) -> bool {
    MODULUS_BITS.contains(&modulus_bits) && EXPONENT_BITS.contains(&exponent_bits)
}

/// The NUL-terminated password, XORed with the challenge, encrypted with the server's RSA key
/// `pem` (OAEP padding), as `caching_sha2_password` and `sha256_password` take it without TLS.
fn rsa_encrypt(password: &[u8], nonce: &[u8; 20], pem: &[u8]) -> Result<Vec<u8>, DbError> {
    let bad = |why: &str| DbError::Connect(format!("the server's RSA key {why}"));
    if pem.len() > MAX_KEY_PEM {
        return Err(bad("is too long"));
    }
    let (der, kind) = pem_to_der(pem).map_err(|_| bad("is not a PEM public key"))?;
    check_rsa_der(&der, kind).ok_or_else(|| bad("is not an RSA public key"))?;
    let (modulus, exponent) = parse_pub_key(&der, kind).map_err(|_| bad("is not an RSA public key"))?;
    if !usable_key(modulus.bits(), exponent.bits()) {
        return Err(bad("has a size dbee does not use"));
    }
    let key = PublicKey::new(modulus, exponent);
    let message: Vec<u8> = password.iter().chain(std::iter::once(&0)).zip(nonce.iter().cycle()).map(|(p, n)| p ^ n).collect();
    // encrypt_block panics on a block too long for the key.
    if message.len().saturating_add(OAEP_OVERHEAD) > key.num_octets() {
        return Err(DbError::Auth { message: "the password is too long for the server's RSA key".into() });
    }
    key.encrypt_block(&message, Pkcs1OaepPadding::new(GetRandom)).map_err(|_| DbError::Other("could not encrypt the password".into()))
}

/// The handshake response (Protocol::HandshakeResponse41), answering with `method`.
fn handshake_response(g: &Greeting, who: &Login<'_>, caps: CapabilityFlags, method: Method, answer: Vec<u8>) -> Vec<u8> {
    let response = HandshakeResponse::new(
        Some(answer),
        g.version_parts,
        Some(who.user.as_bytes()),
        who.database.map(str::as_bytes),
        Some(AuthPlugin::from_bytes(method.name().as_bytes())),
        caps,
        None,
        MAX_PAYLOAD as u32,
    );
    let mut buf = Vec::new();
    response.serialize(&mut buf);
    // mysql_common picks the character set by the server's version; dbee always asks for utf8mb4.
    if let Some(charset) = buf.get_mut(8) {
        *charset = CHARSET;
    }
    buf
}

/// Log in after greeting `g`, on `p` (over TLS when `tls`), until the server's OK packet.
pub(crate) async fn login(p: &mut Packets, g: &Greeting, who: &Login<'_>, caps: CapabilityFlags, tls: bool) -> Result<(), DbError> {
    let flavor = Flavor::from_version(&g.version);
    let mut method = Method::named(&g.plugin)?;
    let mut nonce = g.nonce;
    let (answer, mut step) = method.first(who.password, &nonce, tls)?;
    log::debug!("mysql: logging in with {}", method.name());
    p.write(&handshake_response(g, who, caps, method, answer)).await?;
    for _ in 0..MAX_LOGIN_PACKETS {
        let reply = p.read().await?;
        match reply.first() {
            Some(0x00) => {
                wire::parse_ok::<CommonOkPacket>(&reply, caps)?;
                return Ok(());
            }
            Some(0xFF) => return Err(errors::from_err_packet(&reply, caps, flavor)),
            Some(0xFE) => {
                let switch = ParseBuf(&reply).parse::<AuthSwitchRequest>(()).map_err(|_| out_of_turn())?;
                method = Method::named(switch.auth_plugin().as_bytes())?;
                nonce = nonce_of(switch.plugin_data())?;
                log::debug!("mysql: the server switched the login to {}", method.name());
                let (answer, next) = method.first(who.password, &nonce, tls)?;
                step = next;
                p.write(&answer).await?;
            }
            Some(0x01) => {
                let more = ParseBuf(&reply).parse::<AuthMoreData>(()).map_err(|_| out_of_turn())?;
                let (answer, next) = method.more(step, more.data(), who.password, &nonce, tls)?;
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

    const NONCE: [u8; 20] = *b"abcdefghijklmnopqrst";

    /// A 2048-bit RSA public key, as MySQL sends its own.
    const KEY_2048: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAlZT/mLblmmuu4gQOB1ha
7ixbmi0XguCiWT9sXg6N/C9/wEfE/t96ZzdQIg9YduYVHEgWG3QgYUj4Oh3r7nFo
k276KvixSutLYjAz8Pp58iDPRxDQz5Po6uEABLna312psovJLOTH87OfJKlKVQG2
oLdSJuoVVp+YzNE9xlR8k03mTVCpRgBKR8bKb8oZe0OKhacqokceaisVzsk837rj
HVS3m+rvy/Ea3+mtZpqmawtVPvxzCElTfdGtOVwlp63JlJ3CCQ5FB6swEfTW5BWa
iUYSUy3d4JX3yQ/kS5EeezphehJ0WUg1WKgxwaSRAriJwl7p4NVCi+ScixSPyC9D
iwIDAQAB
-----END PUBLIC KEY-----
";

    /// A 512-bit key: too small to use.
    const KEY_512: &str = "-----BEGIN PUBLIC KEY-----
MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAOGqvspe9u1GOXJEOWMRIkYjeHhp3lJJ
IHQrJkJjC7DbB+uQzWgUsyp/GKY4r1lXoIqw3RrSLH8eZTdsuNCGn+kCAwEAAQ==
-----END PUBLIC KEY-----
";

    fn greeting(caps: CapabilityFlags, version: (u16, u16, u16)) -> Greeting {
        Greeting { version: "8.0.99".into(), version_parts: version, capabilities: caps, nonce: NONCE, plugin: b"caching_sha2_password".to_vec() }
    }

    #[test]
    fn handshake_response_is_as_documented() {
        let all = CapabilityFlags::all();
        for version in [(5, 5, 5), (8, 0, 99), (0, 0, 0)] {
            let g = greeting(all, version);
            let caps = capabilities(&g, false, true).unwrap();
            let who = Login { user: "root", password: b"pw", database: Some("shop") };
            let r = handshake_response(&g, &who, caps, Method::CachingSha2, vec![1; 32]);
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
        let ssl = ssl_request(caps);
        assert_eq!((&ssl[..4], &ssl[4..8], ssl[8], ssl.len()), (&caps.bits().to_le_bytes()[..], &(1u32 << 30).to_le_bytes()[..], CHARSET, 32));
    }

    #[test]
    fn capabilities_follow_the_server() {
        let old = greeting(CapabilityFlags::CLIENT_PROTOCOL_41 | CapabilityFlags::CLIENT_SECURE_CONNECTION, (5, 1, 0));
        assert_eq!(capabilities(&old, false, false).unwrap_err(), too_old());
        let no_eof = greeting(REQUIRED | CapabilityFlags::CLIENT_FOUND_ROWS, (5, 6, 0));
        assert_eq!(capabilities(&no_eof, false, false).unwrap(), REQUIRED | CapabilityFlags::CLIENT_FOUND_ROWS);
        assert_eq!(capabilities(&no_eof, false, true).unwrap_err().code(), "connect");
    }

    /// Protocol::HandshakeV10 as MySQL's documentation lays it out, with a challenge of
    /// `challenge_len` bytes announced and 13 sent in its second part.
    fn greeting_bytes(caps: CapabilityFlags, challenge_len: u8) -> Vec<u8> {
        let caps = caps.bits().to_le_bytes();
        let fixed = [&[0; 4][..], &NONCE[..8], &[0], &caps[..2], &[45], &[2, 0], &caps[2..], &[challenge_len], &[0; 10]].concat();
        [&b"\x0a8.0.99\0"[..], &fixed, &NONCE[8..], &[0], b"caching_sha2_password\0"].concat()
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
        assert_eq!(parse_greeting(&greeting_bytes(CapabilityFlags::CLIENT_PROTOCOL_41, 21)).unwrap_err(), too_old());
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
        assert_eq!(Method::Native.first(b"", &NONCE, false).unwrap(), (vec![], Step::Answered));
        assert_eq!(Method::CachingSha2.first(b"", &NONCE, false).unwrap(), (vec![], Step::Answered));
        assert_eq!(Method::Sha256.first(b"", &NONCE, false).unwrap(), (vec![0], Step::PasswordSent));
        assert_eq!(Method::Native.first(pw, &NONCE, false).unwrap().0.len(), 20);
        assert_eq!(Method::CachingSha2.first(pw, &NONCE, false).unwrap().0.len(), 32);
        // sha256_password: the password over TLS, else a request for the key.
        assert_eq!(Method::Sha256.first(pw, &NONCE, true).unwrap(), (b"secret\0".to_vec(), Step::PasswordSent));
        assert_eq!(Method::Sha256.first(pw, &NONCE, false).unwrap(), (vec![SHA256_KEY_REQUEST], Step::WaitingForKey));
        // caching_sha2_password: the fast path, or the full one.
        let more = |step, data: &[u8], tls| Method::CachingSha2.more(step, data, pw, &NONCE, tls);
        assert_eq!(more(Step::Answered, &[FAST_AUTH_OK], false).unwrap(), (None, Step::FastOk));
        assert_eq!(more(Step::Answered, &[FULL_AUTH], true).unwrap(), (Some(b"secret\0".to_vec()), Step::PasswordSent));
        assert_eq!(more(Step::Answered, &[FULL_AUTH], false).unwrap(), (Some(vec![CACHING_SHA2_KEY_REQUEST]), Step::WaitingForKey));
        let (sent, step) = more(Step::WaitingForKey, KEY_2048.as_bytes(), false).unwrap();
        assert_eq!((sent.map(|s| s.len()), step), (Some(256), Step::PasswordSent));
        // Anything else is out of turn.
        for (step, data) in [(Step::FastOk, &[FULL_AUTH][..]), (Step::PasswordSent, &[FAST_AUTH_OK]), (Step::Answered, &[9])] {
            assert_eq!(more(step, data, false).unwrap_err().code(), "connect", "{step:?} {data:?}");
        }
        assert_eq!(Method::Native.more(Step::Answered, &[FULL_AUTH], pw, &NONCE, false).unwrap_err().code(), "connect");
        assert_eq!(Method::Sha256.more(Step::Answered, KEY_2048.as_bytes(), pw, &NONCE, false).unwrap_err().code(), "connect");
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

    #[test]
    fn rsa_keys() {
        assert_eq!(rsa_encrypt(b"pw", &NONCE, KEY_2048.as_bytes()).unwrap().len(), 256);
        let e = rsa_encrypt(b"pw", &NONCE, KEY_512.as_bytes()).unwrap_err();
        assert!(matches!(&e, DbError::Connect(m) if m.contains("size")), "{e:?}");
        assert_eq!(rsa_encrypt(&[b'x'; 214], &NONCE, KEY_2048.as_bytes()).unwrap_err().code(), "auth");
        assert_eq!(rsa_encrypt(&[b'x'; 213], &NONCE, KEY_2048.as_bytes()).unwrap().len(), 256);
        assert_eq!(rsa_encrypt(b"pw", &NONCE, b"no key").unwrap_err().code(), "connect");
        assert_eq!(rsa_encrypt(b"pw", &NONCE, &vec![b'A'; MAX_KEY_PEM + 1]).unwrap_err().code(), "connect");
        assert!(usable_key(2048, 17) && usable_key(1024, 2) && usable_key(16_384, 64));
        // Found by hostile_rsa_keys_never_panic: a length byte changed, which mysql_common's DER
        // parser panics on.
        let mut pem = KEY_2048.as_bytes().to_vec();
        pem[29] = b'A';
        assert_eq!(rsa_encrypt(b"pw", &NONCE, &pem).unwrap_err().code(), "connect");
        // PKCS #1's form is read too.
        let (der, _) = pem_to_der(KEY_2048.as_bytes()).unwrap();
        let pkcs1 = &der[24..];
        assert_eq!(rsa_encrypt(b"pw", &NONCE, pem_text("RSA PUBLIC KEY", pkcs1).as_bytes()).unwrap().len(), 256);
        assert!(!usable_key(1023, 17) && !usable_key(16_385, 17) && !usable_key(2048, 1) && !usable_key(2048, 65));
    }

    const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";

    /// `der` as a PEM text labelled `label`.
    fn pem_text(label: &str, der: &[u8]) -> String {
        let mut b64 = String::new();
        for chunk in der.chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
            for i in 0..4 {
                b64.push(if i <= chunk.len() { char::from(BASE64[(n >> (18 - 6 * i) & 63) as usize]) } else { '=' });
            }
        }
        let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|l| std::str::from_utf8(l).unwrap()).collect();
        format!("-----BEGIN {label}-----\n{}\n-----END {label}-----\n", lines.join("\n"))
    }

    /// A DER element of `tag` around `content`.
    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let len = content.len();
        let header = if len < 0x80 { vec![tag, len as u8] } else { vec![tag, 0x82, (len >> 8) as u8, len as u8] };
        [header, content.to_vec()].concat()
    }

    /// Well-formed DER around random numbers, with random tags and lengths swapped in.
    fn hostile_der() -> impl Strategy<Value = (bool, Vec<u8>)> {
        let numbers = (prop::collection::vec(any::<u8>(), 0..300), prop::collection::vec(any::<u8>(), 0..10));
        let edits = prop::collection::vec((0usize..400, any::<u8>()), 0..3);
        (any::<bool>(), numbers, edits).prop_map(|(pkcs8, (n, e), edits)| {
            let key = der(SEQUENCE, &[der(INTEGER, &n), der(INTEGER, &e)].concat());
            let spki = der(SEQUENCE, &[RSA_ENCRYPTION, &der(BIT_STRING, &[&[0][..], &key].concat())].concat());
            let mut out = if pkcs8 { spki } else { key };
            for (at, byte) in edits {
                if let Some(b) = out.get_mut(at) {
                    *b = byte;
                }
            }
            (pkcs8, out)
        })
    }

    proptest! {
        #[test]
        fn hostile_rsa_keys_never_panic(
            random in prop::collection::vec(any::<u8>(), 0..600),
            flips in prop::collection::vec((27usize..420, 0usize..65), 0..6),
            cut in 0usize..460,
        ) {
            let _ = rsa_encrypt(b"pw", &NONCE, &random);
            let mut pem = KEY_2048.as_bytes().to_vec();
            for (at, char) in flips {
                pem[at] = BASE64[char];
            }
            let _ = rsa_encrypt(b"pw", &NONCE, &pem);
            pem.truncate(cut);
            let _ = rsa_encrypt(b"pw", &NONCE, &pem);
        }

        #[test]
        fn hostile_der_keys_never_panic((pkcs8, der) in hostile_der()) {
            let pem = pem_text(if pkcs8 { "PUBLIC KEY" } else { "RSA PUBLIC KEY" }, &der);
            let _ = rsa_encrypt(b"pw", &NONCE, pem.as_bytes());
        }

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
