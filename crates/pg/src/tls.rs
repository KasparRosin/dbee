//! TLS for Postgres connections: `dbee_driver::tls`'s rustls settings, wrapped for
//! tokio-postgres, with the ALPN id Postgres 17 and newer expect.

use std::path::Path;
use std::sync::Arc;

use dbee_config::SslMode;
use dbee_driver::{DbError, tls};
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres::config::SslMode as PgSslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

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
    pub(crate) fn plain(&self) -> Tls {
        Tls { pg_mode: PgSslMode::Disable, ..self.clone() }
    }
}

/// The TLS settings for `mode` ([`tls::client_config`]'s errors).
pub fn make(mode: SslMode, ca_file: Option<&Path>) -> Result<Tls, DbError> {
    let config = match tls::client_config(mode, ca_file, &[ALPN_POSTGRESQL])? {
        Some(config) => Arc::unwrap_or_clone(config),
        None => trusts_nothing()?,
    };
    let pg_mode = match mode {
        SslMode::Disable => PgSslMode::Disable,
        SslMode::Prefer => PgSslMode::Prefer,
        SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => PgSslMode::Require,
    };
    Ok(Tls { mode, pg_mode, connector: MakeRustlsConnect::new(config) })
}

/// The connector tokio-postgres needs even without TLS: it never starts TLS then; should it
/// ever, no certificate is trusted.
fn trusts_nothing() -> Result<ClientConfig, DbError> {
    let builder = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| DbError::Tls(format!("no usable TLS version: {e}")))?;
    Ok(builder.with_root_certificates(RootCertStore::empty()).with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_maps_modes() {
        use tokio_postgres::config::SslMode as Pg;
        let modes =
            [(SslMode::Disable, Pg::Disable), (SslMode::Prefer, Pg::Prefer), (SslMode::Require, Pg::Require), (SslMode::VerifyFull, Pg::Require)];
        // verify-ca needs a CA file: its mapping is checked in `verify_ca_maps_to_require`.
        for (mode, pg) in modes {
            let tls = make(mode, None).unwrap();
            assert_eq!(tls.pg_mode(), pg, "{mode:?}");
            assert_eq!(tls.mode(), mode);
        }
        assert_eq!(make(SslMode::Prefer, None).unwrap().plain().pg_mode(), Pg::Disable);
    }

    #[test]
    fn verify_ca_maps_to_require() {
        let db = dbee_testkit::require_testdb!();
        let tls = make(SslMode::VerifyCa, Some(db.ca_file.as_path())).unwrap();
        assert_eq!((tls.mode(), tls.pg_mode()), (SslMode::VerifyCa, tokio_postgres::config::SslMode::Require));
        let e = make(SslMode::VerifyCa, None).err().unwrap();
        assert!(e.to_string().contains("verify-ca needs a CA file"), "{e}");
    }

    #[tokio::test]
    async fn the_disable_connector_trusts_no_certificate() {
        let db = dbee_testkit::require_testdb!();
        let s = db.server("pg14").unwrap();
        // TLS forced on: the disable connector must refuse the server, the require one accepts it.
        let attempt = |tls: Tls| {
            let mut pg = tokio_postgres::Config::new();
            pg.host("127.0.0.1").port(s.port).user("postgres").dbname("dbee").password(&db.password).ssl_mode(PgSslMode::Require);
            async move { pg.connect(tls.connector).await.map(|_| ()).map_err(|e| crate::error::map(&e, crate::error::Phase::Connect)) }
        };
        let refused = attempt(make(SslMode::Disable, None).unwrap()).await;
        assert_eq!(refused.as_ref().map_err(DbError::code), Err("tls"), "{refused:?}");
        assert_eq!(attempt(make(SslMode::Require, None).unwrap()).await, Ok(()));
    }
}
