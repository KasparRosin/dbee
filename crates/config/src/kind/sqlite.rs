//! SQLite's fixed facts.

use crate::connection::{KindInfo, Settings};

pub(crate) const INFO: KindInfo = KindInfo {
    id: "sqlite",
    label: "SQLite",
    default_port: None,
    settings: Settings::File { extensions: &["sqlite", "sqlite3", "db"] },
    url_schemes: &[],
    ssl_aliases: &[],
    options: &[],
};
