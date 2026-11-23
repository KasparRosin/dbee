//! The manifest's signature: ed25519 (RFC 8032) over the exact bytes of `SHA256SUMS.txt`, kept
//! beside it as the 64 raw bytes of the signature, checked against the public keys compiled in.
//!
//! No production key is pinned yet, so [`PINNED_KEYS`] is empty and nothing verifies against it:
//! a copy installs nothing until a key is here.

use ring::signature::{ED25519, UnparsedPublicKey};

use crate::UpdateError;

/// An ed25519 public key.
pub type PublicKey = [u8; 32];

/// The length of an ed25519 signature.
pub const SIGNATURE_LEN: usize = 64;

/// The keys a release may be signed with. More than one lets a key be replaced: a release signed
/// with any of them is accepted.
pub const PINNED_KEYS: &[PublicKey] = &[];

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
    fn without_a_pinned_key_nothing_verifies() {
        let (pair, _) = test_key(7);
        let signature = pair.sign(MANIFEST);
        assert!(verify(MANIFEST, signature.as_ref(), &[]).is_err());
        assert!(verify(MANIFEST, signature.as_ref(), PINNED_KEYS).is_err(), "no production key is pinned yet");
    }
}
