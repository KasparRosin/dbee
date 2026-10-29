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

use crate::DbError;

/// A CA file larger than this is refused (a big system bundle is about 250 KiB).
const MAX_CA_FILE: u64 = 4 * 1024 * 1024;

/// Why `verify-ca` without a CA file is refused.
const VERIFY_CA_NEEDS_CA_FILE: &str = "verify-ca needs a CA file; use verify-full to check against the system's certificates";

/// The rustls settings for `mode`, offering the `alpn` protocol ids; `None` for `disable`.
/// `ca_file` is read only by the modes that verify (`require`, `verify-ca`, `verify-full`); a
/// missing, unreadable or certificate-less file is [`DbError::Tls`], and so is `verify-ca`
/// without a CA file.
pub fn client_config(mode: SslMode, ca_file: Option<&Path>, alpn: &[&[u8]]) -> Result<Option<Arc<ClientConfig>>, DbError> {
    if mode == SslMode::Disable {
        return Ok(None);
    }
    let verifier = verifier(mode, ca_file)?;
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

/// The certificate check for `mode`.
fn verifier(mode: SslMode, ca_file: Option<&Path>) -> Result<Arc<dyn ServerCertVerifier>, DbError> {
    if mode == SslMode::VerifyCa && ca_file.is_none() {
        return Err(DbError::Tls(VERIFY_CA_NEEDS_CA_FILE.into()));
    }
    let provider = provider();
    let roots = match mode {
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => ca_file.map(load_roots).transpose()?,
        SslMode::Disable | SslMode::Prefer => None,
    };
    Ok(match (mode, roots) {
        // No TLS is started in this mode; should it ever be, nothing is trusted.
        (SslMode::Disable, _) => Arc::new(ChainOnly { roots: Arc::new(RootCertStore::empty()), provider }),
        (SslMode::Prefer, _) | (SslMode::Require, None) => Arc::new(NoVerify { provider }),
        (SslMode::Require | SslMode::VerifyCa, Some(roots)) => Arc::new(ChainOnly { roots: Arc::new(roots), provider }),
        (SslMode::VerifyFull, Some(roots)) => WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|e| DbError::Tls(format!("cannot check certificates: {e}")))?,
        (SslMode::VerifyFull, None) => Arc::new(
            rustls_platform_verifier::Verifier::new(provider)
                .map_err(|e| DbError::Tls(format!("cannot use the system's trusted certificates: {e}")))?,
        ),
        // Refused above.
        (SslMode::VerifyCa, None) => return Err(DbError::Tls(VERIFY_CA_NEEDS_CA_FILE.into())),
    })
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
        let v = verifier(mode, ca).unwrap();
        let name = ServerName::try_from(name.to_owned()).unwrap();
        v.verify_server_cert(&server_cert(db), &[], &name, &[], UnixTime::now()).is_ok()
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
    fn disable_never_accepts() {
        let db = dbee_testkit::require_testdb!();
        assert!(!accepts(db, SslMode::Disable, None, "localhost"));
        assert!(!accepts(db, SslMode::Disable, Some(db.ca_file.as_path()), "localhost"));
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
