//! The server's RSA key, which `caching_sha2_password` and `sha256_password` encrypt the
//! password with when there is no TLS. Its DER is checked before mysql_common reads it.

use std::ops::RangeInclusive;

use dbee_driver::DbError;
use mysql_common::crypto::der::{PubKeyFileType, parse_pub_key, pem_to_der};
use mysql_common::crypto::rsa::{GetRandom, Pkcs1OaepPadding, PublicKey};

/// The server's RSA key: a PEM text this long at most, a modulus of these bits, an exponent of
/// these bits (65537 has 17).
const MAX_KEY_PEM: usize = 16 * 1024;
const MODULUS_BITS: RangeInclusive<u64> = 1024..=16_384;
const EXPONENT_BITS: RangeInclusive<u64> = 2..=64;
/// What OAEP with SHA-1 takes of each RSA block: twice the hash and two bytes.
const OAEP_OVERHEAD: usize = 2 * 20 + 2;

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
pub(crate) fn rsa_encrypt(password: &[u8], nonce: &[u8; 20], pem: &[u8]) -> Result<Vec<u8>, DbError> {
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

#[cfg(test)]
pub(crate) mod tests {
    use proptest::prelude::*;

    use super::*;

    const NONCE: [u8; 20] = *b"abcdefghijklmnopqrst";

    /// A 2048-bit RSA public key, as MySQL sends its own.
    pub(crate) const KEY_2048: &str = "-----BEGIN PUBLIC KEY-----
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
    }
}
