//! [`Drivers`]: the registered drivers, found by the database type they open.

use std::sync::Arc;

use dbee_config::DbKind;

use crate::Driver;

/// The registered drivers. A database type belongs to the first driver that claims it; later
/// claims are ignored.
#[derive(Clone, Default)]
pub struct Drivers(Vec<Arc<dyn Driver>>);

impl Drivers {
    pub fn new(drivers: Vec<Arc<dyn Driver>>) -> Drivers {
        Drivers(drivers)
    }

    /// The driver that opens `kind`.
    pub fn for_kind(&self, kind: DbKind) -> Option<&Arc<dyn Driver>> {
        self.0.iter().find(|d| d.kinds().contains(&kind))
    }

    /// The types some registered driver opens, in [`DbKind::ALL`] order.
    pub fn kinds(&self) -> Vec<DbKind> {
        DbKind::ALL.into_iter().filter(|k| self.for_kind(*k).is_some()).collect()
    }

    /// The drivers, in the order they were registered.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Driver>> {
        self.0.iter()
    }
}

impl std::fmt::Debug for Drivers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter().map(|d| d.kinds())).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Connection, DbError, DbFuture, Target};

    /// A driver that opens nothing, for the kinds it is given.
    struct Fake(&'static [DbKind]);

    impl Driver for Fake {
        fn kinds(&self) -> &'static [DbKind] {
            self.0
        }

        fn connect<'a>(&'a self, _target: Target<'a>) -> DbFuture<'a, Arc<dyn Connection>> {
            Box::pin(async { Err(DbError::Other("fake".into())) })
        }
    }

    #[test]
    fn registry_finds_by_kind() {
        let d = Drivers::new(vec![Arc::new(Fake(&[DbKind::Postgres]))]);
        assert!(d.for_kind(DbKind::Postgres).is_some());
        assert!(d.for_kind(DbKind::Sqlite).is_none());
        assert!(Drivers::default().for_kind(DbKind::Postgres).is_none());
    }

    #[test]
    fn the_first_claim_wins() {
        let first: Arc<dyn Driver> = Arc::new(Fake(&[DbKind::MySql, DbKind::MariaDb]));
        let second: Arc<dyn Driver> = Arc::new(Fake(&[DbKind::MariaDb, DbKind::Sqlite]));
        let d = Drivers::new(vec![Arc::clone(&first), Arc::clone(&second)]);
        assert!(d.for_kind(DbKind::MariaDb).is_some_and(|got| Arc::ptr_eq(got, &first)));
        assert!(d.for_kind(DbKind::Sqlite).is_some_and(|got| Arc::ptr_eq(got, &second)));
        assert_eq!(d.iter().count(), 2);
    }

    #[test]
    fn kinds_follow_db_kind_order() {
        let d = Drivers::new(vec![Arc::new(Fake(&[DbKind::Sqlite])), Arc::new(Fake(&[DbKind::MariaDb, DbKind::Postgres]))]);
        assert_eq!(d.kinds(), [DbKind::Postgres, DbKind::MariaDb, DbKind::Sqlite]);
        assert!(Drivers::default().kinds().is_empty());
        assert!(format!("{d:?}").contains("Sqlite"), "{d:?}");
    }
}
