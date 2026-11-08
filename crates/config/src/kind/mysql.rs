//! MySQL's and MariaDB's fixed facts, and their options.

use crate::connection::{KindInfo, KindOption, Settings, SslMode};

/// MySQL's and MariaDB's: on a link without TLS that the user wanted encrypted, the password
/// may go out encrypted with a key the server sends, which dbee cannot check.
pub const ALLOW_PUBLIC_KEY: KindOption =
    KindOption { id: "allowPublicKey", label: "Allow the server's public key without encryption", weakens: true };

/// MariaDB's row builds on this one: it lists only what differs.
pub(crate) const INFO: KindInfo = KindInfo {
    id: "mysql",
    label: "MySQL",
    default_port: Some(3306),
    settings: Settings::Server { tls: true, ssh: true, socket: false },
    url_schemes: &["mysql"],
    ssl_aliases: &[
        ("DISABLED", SslMode::Disable),
        ("PREFERRED", SslMode::Prefer),
        ("REQUIRED", SslMode::Require),
        ("VERIFY_CA", SslMode::VerifyCa),
        ("VERIFY_IDENTITY", SslMode::VerifyFull),
    ],
    options: &[ALLOW_PUBLIC_KEY],
};

pub(crate) const MARIADB: KindInfo = KindInfo { id: "mariadb", label: "MariaDB", url_schemes: &["mariadb"], ..INFO };
