//! dbee's database drivers (layer L4): the one list of every driver dbee ships. Only the apps
//! and test code depend on this crate; the engine and the UI know the drivers through
//! `dbee-driver`'s interface alone.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use dbee_driver::Drivers;

/// Every driver dbee ships.
pub fn all() -> Drivers {
    Drivers::new(vec![Arc::new(dbee_pg::PgDriver), Arc::new(dbee_mysql::MySqlDriver)])
}

#[cfg(test)]
mod tests {
    use dbee_config::DbKind;

    use super::all;

    #[test]
    fn no_two_drivers_claim_one_kind() {
        let mut claimed: Vec<DbKind> = Vec::new();
        for driver in all().iter() {
            for kind in driver.kinds() {
                assert!(!claimed.contains(kind), "{kind:?} is claimed twice");
                claimed.push(*kind);
            }
        }
        assert!(!claimed.is_empty());
    }

    #[test]
    fn postgres_mysql_and_mariadb_have_their_drivers() {
        for kind in [DbKind::Postgres, DbKind::MySql, DbKind::MariaDb] {
            assert!(all().for_kind(kind).is_some(), "{kind:?}");
            assert!(all().kinds().contains(&kind), "{kind:?}");
        }
        assert!(all().for_kind(DbKind::Sqlite).is_none(), "SQLite's driver comes in M2.14");
    }
}
