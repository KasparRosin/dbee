//! dbee's database drivers (layer L4): the one list of every driver dbee ships. Only the apps
//! and test code depend on this crate; the engine and the UI know the drivers through
//! `dbee-driver`'s interface alone.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use dbee_driver::Drivers;

/// Every driver dbee ships.
pub fn all() -> Drivers {
    Drivers::new(vec![Arc::new(dbee_pg::PgDriver), Arc::new(dbee_mysql::MySqlDriver), Arc::new(dbee_sqlite::SqliteDriver)])
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
    fn every_database_type_has_its_driver() {
        for kind in DbKind::ALL {
            assert!(all().for_kind(kind).is_some(), "{kind:?}");
        }
        assert_eq!(all().kinds(), DbKind::ALL);
    }
}
