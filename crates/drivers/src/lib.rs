//! dbee's database drivers (layer L4): the one list of every driver dbee ships. Only the apps
//! and test code depend on this crate; the engine and the UI know the drivers through
//! `dbee-driver`'s interface alone.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use dbee_driver::Drivers;

/// Every driver dbee ships.
pub fn all() -> Drivers {
    Drivers::new(vec![Arc::new(dbee_pg::PgDriver)])
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
    fn postgres_has_its_driver() {
        assert!(all().for_kind(DbKind::Postgres).is_some());
        assert!(all().kinds().contains(&DbKind::Postgres));
    }
}
