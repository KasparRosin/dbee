//! The schema model: tables, columns, keys, and the catalog.

use serde::{Deserialize, Serialize};

use crate::cell::Column;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRef {
    pub schema: String,
    pub name: String,
}

impl TableRef {
    /// `schema.name`, unquoted.
    pub fn display(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RelKind {
    Table,
    PartitionedTable,
    View,
    MaterializedView,
    ForeignTable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// `format_type` output, used in casts.
    pub type_sql: String,
    pub type_oid: u32,
    /// `pg_type.typname` of the base type.
    pub base_type: String,
    /// `pg_type.typcategory`.
    pub category: char,
    pub not_null: bool,
    pub has_default: bool,
}

impl ColumnInfo {
    /// Clearing a string-category cell stages `''`; any other type stages NULL.
    pub fn clears_to_null(&self) -> bool {
        self.category != 'S'
    }

    /// Editors for these types take Enter as a newline.
    pub fn multiline(&self) -> bool {
        matches!(self.base_type.as_str(), "text" | "json" | "jsonb" | "xml")
    }

    pub fn column(&self) -> Column {
        Column { name: self.name.clone(), base_type: self.base_type.clone(), type_label: self.type_sql.clone() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub target: TableRef,
    pub target_columns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableInfo {
    pub table: TableRef,
    pub kind: RelKind,
    pub columns: Vec<ColumnInfo>,
    pub primary_key: Vec<String>,
    pub unique_keys: Vec<Vec<String>>,
    pub foreign_keys: Vec<ForeignKey>,
    pub estimated_rows: Option<u64>,
}

impl TableInfo {
    /// The primary key, else the first unique key whose columns are all NOT NULL.
    /// `None` for views and other relations that cannot be edited, or when there is no key.
    pub fn row_key(&self) -> Option<Vec<&ColumnInfo>> {
        if !matches!(self.kind, RelKind::Table | RelKind::PartitionedTable) {
            return None;
        }
        let lookup = |names: &[String]| -> Option<Vec<&ColumnInfo>> {
            if names.is_empty() {
                return None;
            }
            names.iter().map(|n| self.column(n)).collect()
        };
        if !self.primary_key.is_empty() {
            return lookup(&self.primary_key);
        }
        self.unique_keys.iter().find_map(|key| lookup(key).filter(|cols| cols.iter().all(|c| c.not_null)))
    }

    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The foreign key that includes `column`.
    pub fn fk_for(&self, column: &str) -> Option<&ForeignKey> {
        self.foreign_keys.iter().find(|fk| fk.columns.iter().any(|c| c == column))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    pub system: bool,
    pub tables: Vec<TableInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schemas: Vec<SchemaInfo>,
}

impl Catalog {
    pub fn table(&self, t: &TableRef) -> Option<&TableInfo> {
        self.schemas.iter().find(|s| s.name == t.schema)?.tables.iter().find(|x| x.table.name == t.name)
    }

    /// Case-insensitive substring match on `name` or `schema.name`, the needle trimmed. User
    /// schemas first (catalog order), then system schemas. An empty (or blank) needle gives every
    /// table.
    pub fn filter_tables(&self, needle: &str) -> Vec<&TableInfo> {
        let needle = needle.trim().to_lowercase();
        let mut out = Vec::new();
        for system in [false, true] {
            for schema in self.schemas.iter().filter(|s| s.system == system) {
                for t in &schema.tables {
                    if needle.is_empty() || matches_needle(&t.table, &needle) {
                        out.push(t);
                    }
                }
            }
        }
        out
    }

    pub fn table_count(&self) -> usize {
        self.schemas.iter().map(|s| s.tables.len()).sum()
    }
}

fn matches_needle(t: &TableRef, needle_lower: &str) -> bool {
    t.name.to_lowercase().contains(needle_lower) || format!("{}.{}", t.schema, t.name).to_lowercase().contains(needle_lower)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn col(name: &str, not_null: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), type_sql: "text".into(), type_oid: 25, base_type: "text".into(), category: 'S', not_null, has_default: false }
    }
    fn table(schema: &str, name: &str) -> TableInfo {
        TableInfo {
            table: TableRef { schema: schema.into(), name: name.into() },
            kind: RelKind::Table,
            columns: vec![col("id", true), col("email", true), col("nick", false)],
            primary_key: vec![],
            unique_keys: vec![],
            foreign_keys: vec![],
            estimated_rows: None,
        }
    }
    fn names(v: Vec<&ColumnInfo>) -> Vec<&str> {
        v.into_iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn row_key_prefers_primary_then_not_null_unique() {
        let mut t = table("shop", "users");
        assert!(t.row_key().is_none(), "no key at all");
        t.unique_keys = vec![vec!["email".into()]];
        assert_eq!(names(t.row_key().unwrap()), ["email"]);
        t.primary_key = vec!["id".into()];
        assert_eq!(names(t.row_key().unwrap()), ["id"], "primary key wins");
        t.primary_key.clear();
        t.unique_keys = vec![vec!["nick".into()]];
        assert!(t.row_key().is_none(), "unique on a nullable column");
        t.unique_keys = vec![vec!["nick".into()], vec!["id".into(), "email".into()]];
        assert_eq!(names(t.row_key().unwrap()), ["id", "email"], "first fully not-null unique key");
        t.unique_keys = vec![vec!["missing".into()]];
        assert!(t.row_key().is_none());
        t.primary_key = vec!["id".into()];
        for kind in [RelKind::View, RelKind::MaterializedView, RelKind::ForeignTable] {
            t.kind = kind;
            assert!(t.row_key().is_none(), "{kind:?}");
        }
        t.kind = RelKind::PartitionedTable;
        assert!(t.row_key().is_some());
    }

    #[test]
    fn clearing_rules() {
        let mut c = col("x", false);
        c.category = 'S';
        assert!(!c.clears_to_null());
        for cat in ['N', 'D', 'B', 'U', 'E'] {
            c.category = cat;
            assert!(c.clears_to_null(), "{cat}");
        }
        for (ty, multi) in
            [("text", true), ("json", true), ("jsonb", true), ("xml", true), ("varchar", false), ("int4", false), ("bytea", false), ("_text", false)]
        {
            c.base_type = ty.into();
            assert_eq!(c.multiline(), multi, "{ty}");
        }
        c.name = "n".into();
        c.type_sql = "character varying(200)".into();
        c.base_type = "varchar".into();
        let col = c.column();
        assert_eq!((col.name.as_str(), col.base_type.as_str(), col.type_label.as_str()), ("n", "varchar", "character varying(200)"));
    }

    fn catalog() -> Catalog {
        Catalog {
            schemas: vec![
                SchemaInfo { name: "pg_catalog".into(), system: true, tables: vec![table("pg_catalog", "pg_class")] },
                SchemaInfo { name: "shop".into(), system: false, tables: vec![table("shop", "orders"), table("shop", "items")] },
            ],
        }
    }

    #[test]
    fn filter_tables_case_insensitive_user_schemas_first() {
        let c = catalog();
        let r = |n: &str| c.filter_tables(n).iter().map(|t| t.table.display()).collect::<Vec<_>>();
        assert_eq!(r("ORD"), ["shop.orders"]);
        assert_eq!(r("SHOP.it"), ["shop.items"]);
        assert_eq!(r(""), ["shop.orders", "shop.items", "pg_catalog.pg_class"]);
        assert_eq!(r("zzz"), Vec::<String>::new());
        // The tree's rule: the filter is trimmed, so blanks around it match as nothing.
        assert_eq!(r("  ord\t"), ["shop.orders"]);
        assert_eq!(r("   "), ["shop.orders", "shop.items", "pg_catalog.pg_class"]);
        assert_eq!(c.table_count(), 3);
        assert!(c.table(&TableRef { schema: "shop".into(), name: "items".into() }).is_some());
        assert!(c.table(&TableRef { schema: "shop".into(), name: "nope".into() }).is_none());
    }

    #[test]
    fn fk_lookup_and_serde() {
        let mut t = table("shop", "orders");
        t.foreign_keys = vec![ForeignKey {
            name: "fk".into(),
            columns: vec!["id".into()],
            target: TableRef { schema: "shop".into(), name: "users".into() },
            target_columns: vec!["id".into()],
        }];
        assert!(t.fk_for("id").is_some() && t.fk_for("email").is_none());
        let j = serde_json::to_value(&t).unwrap();
        assert!(j.get("primaryKey").is_some() && j["kind"] == "table");
        assert_eq!(serde_json::from_value::<TableInfo>(j).unwrap(), t);
    }

    #[test]
    #[ignore]
    fn filter_1000_tables_within_a_frame() {
        let tables: Vec<TableInfo> = (0..1000).map(|i| table("public", &format!("Table_{i}_Name"))).collect();
        let c = Catalog { schemas: vec![SchemaInfo { name: "public".into(), system: false, tables }] };
        let mut times: Vec<f64> = (0..100)
            .map(|_| {
                let start = Instant::now();
                let n = c.filter_tables("e_5").len();
                std::hint::black_box(n);
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        times.sort_by(|a, b| a.total_cmp(b));
        let median = times[times.len() / 2];
        println!("filter_tables median over 1000 tables: {median:.3} ms");
        assert!(median < 2.0, "median {median} ms");
    }
}
