//! TLS for Postgres connections: rustls with ring, and the OS certificate verifier.
//!
//! | mode | TLS | verification |
//! |---|---|---|
//! | `disable` | never | none |
//! | `prefer` | when the server offers it, else plain | none |
//! | `require` | always | none; the chain against `ca_file` when one is set (libpq's rule) |
//! | `verify-ca` | always | the chain; the host name is ignored |
//! | `verify-full` | always | the chain and the host name |
//!
//! The roots are the certificates in `ca_file` when one is given (only them), else the OS's
//! through `rustls-platform-verifier`. The host name checked is the configured `host`, also
//! through an SSH tunnel.

use std::path::Path;
use std::sync::Arc;

use dbee_config::SslMode;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{WebPkiServerVerifier, verify_server_cert_signed_by_trust_anchor};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::ParsedCertificate;
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio_postgres::config::SslMode as PgSslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::PgError;

/// A CA file larger than this is refused (a big system bundle is about 250 KiB).
const MAX_CA_FILE: u64 = 4 * 1024 * 1024;

/// The ALPN protocol id Postgres 17 and newer expect; older servers ignore it.
const ALPN_POSTGRESQL: &[u8] = b"postgresql";

/// What a connection uses for TLS: the tokio-postgres mode and a rustls connector.
#[derive(Clone)]
pub struct Tls {
    mode: SslMode,
    pg_mode: PgSslMode,
    pub(crate) connector: MakeRustlsConnect,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls").field("mode", &self.mode).field("pg_mode", &self.pg_mode).finish()
    }
}

impl Tls {
    /// The configured mode.
    pub fn mode(&self) -> SslMode {
        self.mode
    }

    /// What tokio-postgres is told: `disable`, `prefer` or `require`.
    pub fn pg_mode(&self) -> PgSslMode {
        self.pg_mode
    }

    /// The same settings without TLS: `prefer`'s second try after a failed handshake.
    pub fn plain(&self) -> Tls {
        Tls { pg_mode: PgSslMode::Disable, ..self.clone() }
    }
}

/// The TLS settings for `mode`. `ca_file` is read only by the modes that verify (`require`,
/// `verify-ca`, `verify-full`); a missing, unreadable or certificate-less file is
/// [`PgError::Tls`].
pub fn make(mode: SslMode, ca_file: Option<&Path>) -> Result<Tls, PgError> {
    let provider = provider();
    let verifier = verifier(mode, ca_file)?;
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| PgError::Tls(format!("no usable TLS version: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![ALPN_POSTGRESQL.to_vec()];
    let pg_mode = match mode {
        SslMode::Disable => PgSslMode::Disable,
        SslMode::Prefer => PgSslMode::Prefer,
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => PgSslMode::Require,
    };
    Ok(Tls { mode, pg_mode, connector: MakeRustlsConnect::new(config) })
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The certificate check for `mode`.
fn verifier(mode: SslMode, ca_file: Option<&Path>) -> Result<Arc<dyn ServerCertVerifier>, PgError> {
    let provider = provider();
    let roots = match mode {
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => ca_file.map(load_roots).transpose()?,
        SslMode::Disable | SslMode::Prefer => None,
    };
    Ok(match (mode, roots) {
        // tokio-postgres never starts TLS in this mode; should it ever, nothing is trusted.
        (SslMode::Disable, _) => Arc::new(ChainOnly { roots: Arc::new(RootCertStore::empty()), provider }),
        (SslMode::Prefer, _) | (SslMode::Require, None) => Arc::new(NoVerify { provider }),
        (SslMode::Require | SslMode::VerifyCa, Some(roots)) => Arc::new(ChainOnly { roots: Arc::new(roots), provider }),
        (SslMode::VerifyFull, Some(roots)) => WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
            .map_err(|e| PgError::Tls(format!("cannot check certificates: {e}")))?,
        (SslMode::VerifyCa, None) => Arc::new(OsChainOnly(os_verifier(provider)?)),
        (SslMode::VerifyFull, None) => Arc::new(os_verifier(provider)?),
    })
}

fn os_verifier(provider: Arc<CryptoProvider>) -> Result<rustls_platform_verifier::Verifier, PgError> {
    rustls_platform_verifier::Verifier::new(provider).map_err(|e| PgError::Tls(format!("cannot use the system's trusted certificates: {e}")))
}

/// The certificates in the PEM file at `path`.
fn load_roots(path: &Path) -> Result<RootCertStore, PgError> {
    let shown = path.display();
    let fail = |why: String| PgError::Tls(format!("CA file {shown}: {why}"));
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

/// On these platforms the OS verifier is webpki, which checks the whole chain before it looks at
/// the name, so a name-only failure proves the chain is good. The macOS and Windows verifiers
/// report one error without a documented order, so there `verify-ca` without a CA file checks
/// the name too (stricter, never weaker).
const OS_CHECKS_NAME_LAST: bool = cfg!(all(unix, not(target_vendor = "apple"), not(target_os = "android")));

/// `verify-ca` against the OS roots: the OS verifier, with a wrong name forgiven where that is
/// provably the only fault (see [`OS_CHECKS_NAME_LAST`]).
#[derive(Debug)]
struct OsChainOnly(rustls_platform_verifier::Verifier);

impl ServerCertVerifier for OsChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.0.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
            Err(rustls::Error::InvalidCertificate(CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }))
                if OS_CHECKS_NAME_LAST =>
            {
                Ok(ServerCertVerified::assertion())
            }
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
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
        assert!(!accepts(db, SslMode::VerifyCa, None, "localhost"));
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
    fn make_maps_modes() {
        use tokio_postgres::config::SslMode as Pg;
        let modes = [
            (SslMode::Disable, Pg::Disable),
            (SslMode::Prefer, Pg::Prefer),
            (SslMode::Require, Pg::Require),
            (SslMode::VerifyCa, Pg::Require),
            (SslMode::VerifyFull, Pg::Require),
        ];
        for (mode, pg) in modes {
            let tls = make(mode, None).unwrap();
            assert_eq!(tls.pg_mode(), pg, "{mode:?}");
            assert_eq!(tls.mode(), mode);
        }
        assert_eq!(make(SslMode::Prefer, None).unwrap().plain().pg_mode(), Pg::Disable);
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
                let e = make(mode, Some(file)).err().unwrap();
                assert_eq!(e.code(), "tls", "{mode:?} {}", file.display());
                assert!(e.to_string().contains(&file.display().to_string()), "{e}");
            }
            // Modes that do not verify never read the file.
            make(SslMode::Disable, Some(file)).unwrap();
            make(SslMode::Prefer, Some(file)).unwrap();
        }
    }
}
