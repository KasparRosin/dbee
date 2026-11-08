//! TLS for every server type: rustls with ring, and the OS certificate verifier. The modes are
//! libpq's:
//!
//! | mode | TLS | verification |
//! |---|---|---|
//! | `disable` | never | none |
//! | `prefer` | when the server offers it, else plain | none |
//! | `require` | always | none; the chain against `ca_file` when one is set (libpq's rule) |
//! | `verify-ca` | always | the chain against `ca_file`; the host name is ignored. Refused without a CA file |
//! | `verify-full` | always | the chain and the host name |
//!
//! The roots are the certificates in `ca_file` when one is given (only them), else, for
//! `verify-full`, the OS's through `rustls-platform-verifier`. `verify-ca` without a CA file is
//! refused, as libpq refuses it with the system roots: a chain check alone against public roots
//! would accept any publicly trusted certificate for any host. The host name checked is the
//! configured `host`, also through an SSH tunnel. Falling back to plain in `prefer` mode is the
//! driver's part.

use std::path::Path;
use std::sync::Arc;

use dbee_config::SslMode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{WebPkiServerVerifier, verify_server_cert_signed_by_trust_anchor};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};

use crate::{DbError, TlsState};

/// A CA file larger than this is refused (a big system bundle is about 250 KiB).
const MAX_CA_FILE: u64 = 4 * 1024 * 1024;

/// Why `verify-ca` without a CA file is refused.
const VERIFY_CA_NEEDS_CA_FILE: &str = "verify-ca needs a CA file; use verify-full to check against the system's certificates";

/// The warning on a connection that `prefer` left without TLS on a direct link.
pub const PLAIN_AFTER_PREFER: &str = "This connection is not encrypted: TLS could not be set up, and SSL mode prefer then connects without it. \
                                      Anyone on the network between dbee and the server can read and change what is sent. Set SSL mode to \
                                      require or verify-full to refuse a connection without TLS, or to disable if plain text is fine here.";

/// Whether `mode` with `ca_file` checks the server's certificate: `verify-ca` and `verify-full`,
/// and `require` with a CA file (libpq's rule). `prefer` never does.
pub fn checks(mode: SslMode, ca_file: Option<&Path>) -> bool {
    match mode {
        SslMode::Disable | SslMode::Prefer => false,
        SslMode::Require => ca_file.is_some(),
        SslMode::VerifyCa | SslMode::VerifyFull => true,
    }
}

/// The link's state: TLS started (`on`) or not, and whether `mode` with `ca_file` checked the
/// certificate.
pub fn state(on: bool, mode: SslMode, ca_file: Option<&Path>) -> TlsState {
    match (on, checks(mode, ca_file)) {
        (false, _) => TlsState::Off,
        (true, false) => TlsState::Unchecked,
        (true, true) => TlsState::Checked,
    }
}

/// [`PLAIN_AFTER_PREFER`] for a `prefer` connection without TLS (`on`) on a `direct` link; the
/// SSH tunnel encrypts the link as far as the SSH server.
pub fn prefer_warning(mode: SslMode, on: bool, direct: bool) -> Option<String> {
    (mode == SslMode::Prefer && !on && direct).then(|| PLAIN_AFTER_PREFER.to_owned())
}

/// The rustls settings for `mode`, offering the `alpn` protocol ids; `None` for `disable`.
/// `ca_file` is read only by the modes that verify (`require`, `verify-ca`, `verify-full`); a
/// missing, unreadable or certificate-less file is [`DbError::Tls`], and so is `verify-ca`
/// without a CA file.
pub fn client_config(mode: SslMode, ca_file: Option<&Path>, alpn: &[&[u8]]) -> Result<Option<Arc<ClientConfig>>, DbError> {
    let Some(verifier) = verifier(mode, ca_file)? else { return Ok(None) };
    let mut config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| DbError::Tls(format!("no usable TLS version: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(Some(Arc::new(config)))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The certificate check for `mode`; `None` for `disable`.
fn verifier(mode: SslMode, ca_file: Option<&Path>) -> Result<Option<Arc<dyn ServerCertVerifier>>, DbError> {
    let provider = provider();
    Ok(Some(match (mode, ca_file) {
        (SslMode::Disable, _) => return Ok(None),
        (SslMode::Prefer, _) | (SslMode::Require, None) => Arc::new(NoVerify { provider }),
        (SslMode::VerifyCa, None) => return Err(DbError::Tls(VERIFY_CA_NEEDS_CA_FILE.into())),
        (SslMode::Require | SslMode::VerifyCa, Some(path)) => Arc::new(ChainOnly { roots: Arc::new(load_roots(path)?), provider }),
        (SslMode::VerifyFull, Some(path)) => WebPkiServerVerifier::builder_with_provider(Arc::new(load_roots(path)?), provider)
            .build()
            .map_err(|e| DbError::Tls(format!("cannot check certificates: {e}")))?,
        (SslMode::VerifyFull, None) => Arc::new(
            rustls_platform_verifier::Verifier::new(provider)
                .map_err(|e| DbError::Tls(format!("cannot use the system's trusted certificates: {e}")))?,
        ),
    }))
}

/// The certificates in the PEM file at `path`.
fn load_roots(path: &Path) -> Result<RootCertStore, DbError> {
    let shown = path.display();
    let fail = |why: String| DbError::Tls(format!("CA file {shown}: {why}"));
    let meta = std::fs::metadata(path).map_err(|e| fail(e.to_string()))?;
    if meta.len() > MAX_CA_FILE {
        return Err(fail(format!("larger than {} MiB", MAX_CA_FILE / (1024 * 1024))));
    }
    let pem = std::fs::read(path).map_err(|e| fail(e.to_string()))?;
    let certs = CertificateDer::pem_slice_iter(&pem).collect::<Result<Vec<_>, _>>().map_err(|e| fail(format!("not a PEM file: {e}")))?;
    let mut roots = RootCertStore::empty();
    let (added, ignored) = roots.add_parsable_certificates(certs);
    if added == 0 {
        let why = if ignored == 0 { "holds no certificate".to_owned() } else { format!("none of its {ignored} certificates is usable") };
        return Err(fail(why));
    }
    Ok(roots)
}

fn signature_schemes(provider: &CryptoProvider) -> Vec<SignatureScheme> {
    provider.signature_verification_algorithms.supported_schemes()
}

/// Accepts any certificate, but still checks that the server holds its key.
#[derive(Debug)]
struct NoVerify {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        signature_schemes(&self.provider)
    }
}

/// The chain must end in one of `roots`; the host name is not checked.
#[derive(Debug)]
struct ChainOnly {
    roots: Arc<RootCertStore>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let cert = ParsedCertificate::try_from(end_entity)?;
        verify_server_cert_signed_by_trust_anchor(&cert, &self.roots, intermediates, now, self.provider.signature_verification_algorithms.all)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        signature_schemes(&self.provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The servers' certificate (SAN localhost, 127.0.0.1, pg14, pgnew), signed by the test CA.
    fn server_cert(db: &dbee_testkit::TestDb) -> CertificateDer<'static> {
        CertificateDer::from_pem_file(db.ca_file.with_file_name("server.crt")).unwrap()
    }

    /// Does the verifier for `mode` (with the test CA as `ca_file`, or the OS roots) accept the
    /// servers' certificate for `name`?
    fn accepts(db: &dbee_testkit::TestDb, mode: SslMode, ca: Option<&Path>, name: &str) -> bool {
        let v = verifier(mode, ca).unwrap().unwrap();
        let name = ServerName::try_from(name.to_owned()).unwrap();
        v.verify_server_cert(&server_cert(db), &[], &name, &[], UnixTime::now()).is_ok()
    }

    /// [`checks`] says what the verifier does: a certificate from another CA (the server's own,
    /// standing in for one) is refused exactly where it says the certificate is checked.
    #[test]
    fn checks_is_what_the_verifier_does() {
        let db = dbee_testkit::require_testdb!();
        let other = db.ca_file.with_file_name("server.crt");
        let wrong = Some(other.as_path());
        for (mode, ca) in [
            (SslMode::Prefer, None),
            (SslMode::Prefer, wrong),
            (SslMode::Require, None),
            (SslMode::Require, wrong),
            (SslMode::VerifyCa, wrong),
            (SslMode::VerifyFull, wrong),
            (SslMode::VerifyFull, None),
        ] {
            assert_eq!(accepts(db, mode, ca, "localhost"), !checks(mode, ca), "{mode:?} {ca:?}");
        }
        assert!(!checks(SslMode::Disable, wrong));
    }

    #[test]
    fn the_state_and_the_warning_follow_tls_the_mode_and_the_link() {
        let ca = Some(Path::new("/ca.crt"));
        assert_eq!(state(false, SslMode::VerifyFull, ca), TlsState::Off);
        assert_eq!(state(true, SslMode::Prefer, ca), TlsState::Unchecked);
        assert_eq!(state(true, SslMode::Require, None), TlsState::Unchecked);
        assert_eq!(state(true, SslMode::Require, ca), TlsState::Checked);
        assert_eq!(state(true, SslMode::VerifyFull, None), TlsState::Checked);
        assert_eq!(prefer_warning(SslMode::Prefer, false, true).as_deref(), Some(PLAIN_AFTER_PREFER));
        // TLS on, the tunnel's link, or plain text chosen: no warning.
        for (mode, on, direct) in [(SslMode::Prefer, true, true), (SslMode::Prefer, false, false), (SslMode::Disable, false, true)] {
            assert_eq!(prefer_warning(mode, on, direct), None, "{mode:?} {on} {direct}");
        }
        assert!(PLAIN_AFTER_PREFER.contains("not encrypted") && PLAIN_AFTER_PREFER.contains("verify-full"));
    }

    #[test]
    fn verify_full_checks_the_host_name() {
        let db = dbee_testkit::require_testdb!();
        let ca = Some(db.ca_file.as_path());
        for name in ["localhost", "127.0.0.1", "pg14", "pgnew"] {
            assert!(accepts(db, SslMode::VerifyFull, ca, name), "{name}");
        }
        assert!(!accepts(db, SslMode::VerifyFull, ca, "wrong.example"));
        assert!(!accepts(db, SslMode::VerifyFull, ca, "10.0.0.1"));
        // The OS does not trust the test CA.
        assert!(!accepts(db, SslMode::VerifyFull, None, "localhost"));
    }

    #[test]
    fn verify_ca_and_require_check_the_chain_only() {
        let db = dbee_testkit::require_testdb!();
        let ca = Some(db.ca_file.as_path());
        let wrong = db.ca_file.with_file_name("server.crt");
        for mode in [SslMode::VerifyCa, SslMode::Require] {
            assert!(accepts(db, mode, ca, "wrong.example"), "{mode:?}");
            assert!(accepts(db, mode, ca, "localhost"), "{mode:?}");
            assert!(!accepts(db, mode, Some(&wrong), "localhost"), "{mode:?}");
        }
        assert!(client_config(SslMode::VerifyCa, ca, &[]).unwrap().is_some());
    }

    #[test]
    fn verify_ca_without_a_ca_file_is_refused() {
        // On every platform, before any certificate is looked at: the OS roots alone would accept
        // any publicly trusted certificate for any host.
        let e = verifier(SslMode::VerifyCa, None).err().unwrap();
        assert_eq!(e.code(), "tls");
        assert_eq!(e, DbError::Tls(VERIFY_CA_NEEDS_CA_FILE.into()));
        let e = client_config(SslMode::VerifyCa, None, &[]).err().unwrap();
        assert!(e.to_string().contains("verify-ca needs a CA file") && e.to_string().contains("verify-full"), "{e}");
        // verify-full may use the OS roots.
        client_config(SslMode::VerifyFull, None, &[]).unwrap();
    }

    #[test]
    fn prefer_and_require_without_ca_do_not_verify() {
        let db = dbee_testkit::require_testdb!();
        for mode in [SslMode::Prefer, SslMode::Require] {
            assert!(accepts(db, mode, None, "wrong.example"), "{mode:?}");
        }
        // Prefer ignores a CA file.
        assert!(accepts(db, SslMode::Prefer, Some(&db.ca_file.with_file_name("server.crt")), "wrong.example"));
    }

    #[test]
    fn disable_is_no_tls_and_the_rest_carry_the_alpn() {
        assert!(client_config(SslMode::Disable, None, &[b"x"]).unwrap().is_none());
        for mode in [SslMode::Prefer, SslMode::Require, SslMode::VerifyFull] {
            let config = client_config(mode, None, &[b"postgresql"]).unwrap().unwrap();
            assert_eq!(config.alpn_protocols, [b"postgresql".to_vec()], "{mode:?}");
            assert!(client_config(mode, None, &[]).unwrap().unwrap().alpn_protocols.is_empty(), "{mode:?}");
        }
    }

    #[test]
    fn bad_ca_files_are_tls_errors() {
        let home = dbee_testkit::temp_home(None, None);
        let missing = home.home.join("missing.crt");
        let empty = home.home.join("empty.crt");
        std::fs::write(&empty, "no certificate here\n").unwrap();
        let broken = home.home.join("broken.crt");
        std::fs::write(&broken, "-----BEGIN CERTIFICATE-----\nnot base64 !!\n-----END CERTIFICATE-----\n").unwrap();
        for file in [&missing, &empty, &broken] {
            for mode in [SslMode::Require, SslMode::VerifyCa, SslMode::VerifyFull] {
                let e = client_config(mode, Some(file), &[]).err().unwrap();
                assert_eq!(e.code(), "tls", "{mode:?} {}", file.display());
                assert!(e.to_string().contains(&file.display().to_string()), "{e}");
            }
            // Modes that do not verify never read the file.
            client_config(SslMode::Disable, Some(file), &[]).unwrap();
            client_config(SslMode::Prefer, Some(file), &[]).unwrap();
        }
    }
}
