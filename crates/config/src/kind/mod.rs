//! Each database type's fixed facts, in its engine's own file. [`DbKind::info`](crate::DbKind::info)
//! is the one place a type is matched on.

pub(crate) mod mysql;
pub(crate) mod postgres;
pub(crate) mod sqlite;
