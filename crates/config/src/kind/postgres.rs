//! PostgreSQL's fixed facts.

use crate::connection::{KindInfo, Settings};

pub(crate) const INFO: KindInfo = KindInfo {
    id: "postgres",
    label: "PostgreSQL",
    default_port: Some(5432),
    settings: Settings::Server { tls: true, ssh: true },
    url_schemes: &["postgres", "postgresql"],
    ssl_aliases: &[],
    options: &[],
};
