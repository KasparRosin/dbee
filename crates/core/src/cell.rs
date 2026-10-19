//! Cells, columns and result sets.

use serde::{Deserialize, Serialize};

/// One value as Postgres printed it. `None` is SQL NULL.
pub type Cell = Option<String>;

/// A result column.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    /// `pg_type.typname` of the base type (`int4`, `jsonb`, `varchar`, `_text`), or `""` when unknown.
    pub base_type: String,
    /// What the value panel names, for example `character varying(200)`.
    pub type_label: String,
}

impl Column {
    /// A column whose type is not known.
    pub fn untyped(name: &str) -> Column {
        Column { name: name.to_string(), base_type: String::new(), type_label: String::new() }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untyped_and_serde_shape() {
        let c = Column::untyped("a");
        assert_eq!(c.base_type, "");
        let rs = ResultSet { columns: vec![c], rows: vec![vec![None, Some("x".into())]], rows_seen: 1, complete: true, rows_affected: None };
        let j = serde_json::to_value(&rs).unwrap();
        assert_eq!(j["rowsSeen"], 1);
        assert_eq!(j["columns"][0]["baseType"], "");
        assert_eq!(serde_json::from_value::<ResultSet>(j).unwrap(), rs);
    }
}
