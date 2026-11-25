//! The manifest's signature: ed25519 (RFC 8032) over the exact bytes of `SHA256SUMS.txt`, kept
//! beside it as the 64 raw bytes of the signature, checked against the public keys compiled in.
//!
//! One key is pinned in [`PINNED_KEYS`], since 2026-11-25. Its private half is the release
//! workflow's secret (`docs/releasing.md`); no release is signed with it yet.

use ring::signature::{ED25519, UnparsedPublicKey};

use crate::UpdateError;

/// An ed25519 public key.
pub type PublicKey = [u8; 32];

/// The length of an ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// The signature's file in a release, beside the manifest.
pub const SIGNATURE_NAME: &str = "SHA256SUMS.txt.sig";

/// The keys a release may be signed with. More than one lets a key be replaced: a release signed
/// with any of them is accepted.
pub const PINNED_KEYS: &[PublicKey] = &[RELEASE_KEY_2026_11];

/// The release key made on 2026-11-25.
const RELEASE_KEY_2026_11: PublicKey = [
    0x58, 0xb1, 0x5a, 0x06, 0x97, 0x5f, 0x1d, 0x8f, 0xd4, 0x2b, 0x91, 0x6d, 0x63, 0x00, 0x84, 0x9f, 0xd2, 0x99, 0xf2, 0xbb, 0xe9, 0xe4, 0xb5, 0x04,
    0xf8, 0x67, 0x64, 0xc3, 0x6b, 0xf4, 0x88, 0x62,
];

/// Checks that `signature` is a signature of `manifest` by one of `keys`. With no key, nothing
/// verifies.
pub fn verify(manifest: &[u8], signature: &[u8], keys: &[PublicKey]) -> Result<(), UpdateError> {
    if keys.is_empty() {
        return Err(UpdateError::Signature("this copy has no release key to check it against"));
    }
    if signature.len() != SIGNATURE_LEN {
        return Err(UpdateError::Signature("it is not 64 bytes long"));
    }
    if keys.iter().any(|key| UnparsedPublicKey::new(&ED25519, key).verify(manifest, signature).is_ok()) {
        Ok(())
    } else {
        Err(UpdateError::Signature("it does not match the release's file list"))
    }
}

/// What the release workflow learns about the files it is about to publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseCheck {
    /// The signature is by one of the keys.
    Signed,
    /// No key is pinned, so nothing could be checked and no copy will install this release.
    NoKey,
}

/// Checks a release before it is published, as the copies in use will: with a key pinned, the
/// release must carry a signature by it. `signature` is the signature file's bytes, when there
/// is one.
pub fn check_release(manifest: &[u8], signature: Option<&[u8]>, keys: &[PublicKey]) -> Result<ReleaseCheck, UpdateError> {
    if keys.is_empty() {
        return Ok(ReleaseCheck::NoKey);
    }
    let signature = signature.ok_or(UpdateError::Signature("the release has no signature file"))?;
    verify(manifest, signature, keys).map(|()| ReleaseCheck::Signed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    /// A key pair made from a fixed seed, for these tests only.
    fn test_key(seed: u8) -> (Ed25519KeyPair, PublicKey) {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        let public: PublicKey = pair.public_key().as_ref().try_into().unwrap();
        (pair, public)
    }

    const MANIFEST: &[u8] = b"0000000000000000000000000000000000000000000000000000000000000000  dbee-0.5.0-linux-x86_64.deb\n";

    #[test]
    fn a_good_signature_verifies() {
        let (pair, public) = test_key(7);
        let signature = pair.sign(MANIFEST);
        assert_eq!(verify(MANIFEST, signature.as_ref(), &[public]), Ok(()));
    }

    #[test]
    fn a_changed_manifest_or_signature_is_refused() {
        let (pair, public) = test_key(7);
        let signature = pair.sign(MANIFEST).as_ref().to_vec();
        let mut changed = MANIFEST.to_vec();
        changed[0] ^= 1;
        assert!(matches!(verify(&changed, &signature, &[public]), Err(UpdateError::Signature(_))));
        let mut longer = MANIFEST.to_vec();
        longer.push(b'\n');
        assert!(verify(&longer, &signature, &[public]).is_err());
        for i in [0, 31, 32, 63] {
            let mut bad = signature.clone();
            bad[i] ^= 0x40;
            assert!(verify(MANIFEST, &bad, &[public]).is_err(), "byte {i}");
        }
    }

    #[test]
    fn another_keys_signature_is_refused() {
        let (pair, _) = test_key(7);
        let (_, other) = test_key(8);
        let signature = pair.sign(MANIFEST);
        assert!(verify(MANIFEST, signature.as_ref(), &[other]).is_err());
    }

    #[test]
    fn any_pinned_key_may_have_signed() {
        let (pair, public) = test_key(7);
        let (_, other) = test_key(8);
        let signature = pair.sign(MANIFEST);
        assert_eq!(verify(MANIFEST, signature.as_ref(), &[other, public]), Ok(()));
    }

    #[test]
    fn a_truncated_or_padded_signature_is_refused() {
        let (pair, public) = test_key(7);
        let signature = pair.sign(MANIFEST).as_ref().to_vec();
        for len in [0, 1, 32, 63] {
            assert_eq!(verify(MANIFEST, &signature[..len], &[public]), Err(UpdateError::Signature("it is not 64 bytes long")), "{len}");
        }
        let mut padded = signature.clone();
        padded.push(0);
        assert!(verify(MANIFEST, &padded, &[public]).is_err());
    }

    #[test]
    fn a_key_that_is_no_curve_point_verifies_nothing() {
        let (pair, _) = test_key(7);
        let signature = pair.sign(MANIFEST);
        assert!(verify(MANIFEST, signature.as_ref(), &[[0xff; 32]]).is_err());
    }

    #[test]
    fn without_a_pinned_key_nothing_verifies_and_a_test_key_is_not_pinned() {
        let (pair, _) = test_key(7);
        let signature = pair.sign(MANIFEST);
        assert!(verify(MANIFEST, signature.as_ref(), &[]).is_err());
        assert!(verify(MANIFEST, signature.as_ref(), PINNED_KEYS).is_err(), "a test key is not the release key");
    }

    #[test]
    fn a_release_is_checked_before_it_is_published() {
        let (pair, public) = test_key(7);
        let (_, other) = test_key(8);
        let signature = pair.sign(MANIFEST);
        assert_eq!(check_release(MANIFEST, Some(signature.as_ref()), &[public]), Ok(ReleaseCheck::Signed));
        assert_eq!(check_release(MANIFEST, None, &[public]), Err(UpdateError::Signature("the release has no signature file")));
        assert!(check_release(MANIFEST, Some(signature.as_ref()), &[other]).is_err());
        assert_eq!(check_release(MANIFEST, None, &[]), Ok(ReleaseCheck::NoKey));
        assert_eq!(check_release(MANIFEST, Some(signature.as_ref()), &[]), Ok(ReleaseCheck::NoKey));
    }

    /// A signature made the way the release workflow makes one (`openssl pkeyutl -sign -rawin`
    /// with a throwaway ed25519 key, 2026-11-24) over [`MANIFEST`], and that key's public half as
    /// the last 32 bytes of its DER form.
    const OPENSSL_KEY: PublicKey = [
        0x52, 0x5d, 0x51, 0x8b, 0xf4, 0xf1, 0x4e, 0x64, 0x31, 0x1a, 0x99, 0x5e, 0x08, 0xb7, 0x51, 0xb3, 0x37, 0xc8, 0xd4, 0x62, 0xe5, 0x31, 0xef,
        0x63, 0x10, 0xf7, 0x77, 0x1c, 0x00, 0xa0, 0x2d, 0x64,
    ];
    const OPENSSL_SIGNATURE: [u8; SIGNATURE_LEN] = [
        0x7c, 0xeb, 0x10, 0xa9, 0xcd, 0x61, 0xe4, 0x2b, 0x61, 0xb9, 0xb1, 0x2d, 0x50, 0x6e, 0x74, 0x50, 0xda, 0x89, 0x47, 0x95, 0xed, 0xd3, 0xfb,
        0xb4, 0xd3, 0x63, 0xa6, 0xa1, 0xc2, 0x3f, 0x90, 0xc8, 0xe1, 0x2a, 0xdf, 0x6e, 0x00, 0x2a, 0x7c, 0x56, 0xc7, 0xd6, 0xf2, 0xe8, 0x54, 0xe7,
        0x02, 0x95, 0xe3, 0x49, 0x5e, 0xf1, 0x45, 0xe1, 0xf5, 0x28, 0x49, 0xc6, 0xb3, 0xaa, 0x41, 0x12, 0xaa, 0x00,
    ];

    #[test]
    fn a_signature_made_by_the_release_workflow_verifies() {
        assert_eq!(verify(MANIFEST, &OPENSSL_SIGNATURE, &[OPENSSL_KEY]), Ok(()));
        assert_eq!(check_release(MANIFEST, Some(&OPENSSL_SIGNATURE), &[OPENSSL_KEY]), Ok(ReleaseCheck::Signed));
    }

    #[test]
    fn a_release_key_is_pinned_once_and_an_unsigned_release_is_refused() {
        assert!(!PINNED_KEYS.is_empty());
        for (i, key) in PINNED_KEYS.iter().enumerate() {
            assert!(!PINNED_KEYS[..i].contains(key), "key {i} is pinned twice");
            assert_ne!(key, &[0; 32]);
            assert_ne!(key, &OPENSSL_KEY, "the throwaway key of these tests is no release key");
        }
        assert_eq!(check_release(MANIFEST, None, PINNED_KEYS), Err(UpdateError::Signature("the release has no signature file")));
        assert!(check_release(MANIFEST, Some(&OPENSSL_SIGNATURE), PINNED_KEYS).is_err());
    }
}
