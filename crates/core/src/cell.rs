//! Cells, columns and result sets.

use serde::{Deserialize, Serialize};

/// One value as the server printed it. `None` is SQL NULL.
pub type Cell = Option<String>;

/// What a column's values are, decided by the driver from the engine's own type. The grid,
/// editors, copies and the value view read this instead of an engine's type names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TypeClass {
    Number,
    Bool,
    /// Short text: a cleared cell stages `''`.
    Text,
    /// Text that can run long: edited on several lines.
    LongText,
    Json,
    DateTime,
    Binary,
    #[default]
    Other,
}

/// A result column.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// The engine's own base type name (Postgres `pg_type.typname`: `int4`, `jsonb`, `_text`), or
    /// `""` when unknown. Only that engine's dialect and driver read it.
    pub base_type: String,
    /// What the value panel names, for example `character varying(200)`.
    pub type_label: String,
    /// What the values are, for the grid, editors and copies.
    #[serde(default)]
    pub class: TypeClass,
}

impl Column {
    /// A column whose type is not known.
    pub fn untyped(name: &str) -> Column {
        Column { name: name.to_string(), base_type: String::new(), type_label: String::new(), class: TypeClass::Other }
    }
}

/// The result of one statement.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultSet {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
    pub rows_seen: u64,
    pub complete: bool,
    pub rows_affected: Option<u64>,
    /// The database the connection is in after the statement, as far as the driver can tell.
    #[serde(skip)]
    pub database_after: DatabaseAfter,
}

/// The database a connection is in after a statement, as its driver tells it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DatabaseAfter {
    /// The driver cannot tell: neither it nor the server keeps track of it.
    #[default]
    Untold,
    /// The server keeps track, and the statement left it where it was.
    Unchanged,
    /// The statement made this database current, or left none (`None`).
    Changed(Option<String>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untyped_and_serde_shape() {
        let c = Column::untyped("a");
        assert_eq!((c.base_type.as_str(), c.class), ("", TypeClass::Other));
        let rs = ResultSet { columns: vec![c], rows: vec![vec![None, Some("x".into())]], rows_seen: 1, complete: true, ..ResultSet::default() };
        let j = serde_json::to_value(&rs).unwrap();
        assert_eq!(j["rowsSeen"], 1);
        assert_eq!(j["columns"][0]["baseType"], "");
        assert_eq!(j["columns"][0]["class"], "other");
        assert_eq!(serde_json::from_value::<ResultSet>(j).unwrap(), rs);
    }
}
