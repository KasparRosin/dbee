//! The schema model: tables, columns, keys, and the catalog.

use serde::{Deserialize, Serialize};

use crate::cell::{Column, TypeClass};

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

/// Whether Postgres computes a column's value, so that it takes no written value (an `UPDATE` or
/// `INSERT` of one fails with SQLSTATE 428C9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Generated {
    /// An ordinary column; an identity `BY DEFAULT` or a default can be written over.
    #[default]
    No,
    /// A generated column, `GENERATED ALWAYS AS (…)` (stored, or virtual on Postgres 18).
    Expression,
    /// An identity column `GENERATED ALWAYS`.
    Identity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// `format_type` output: the type as the server names it, used in key casts (values use
    /// [`write_type_sql`](Self::write_type_sql)).
    pub type_sql: String,
    /// The engine's own base type name (Postgres `pg_type.typname`), read only by that engine's
    /// dialect and driver.
    pub base_type: String,
    /// What the values are, for the grid, editors and copies.
    pub class: TypeClass,
    pub not_null: bool,
    pub has_default: bool,
    /// Postgres computes the value (`attgenerated`, `attidentity = 'a'`).
    #[serde(default)]
    pub generated: Generated,
    /// A column the engine keeps out of sight (a SQLite hidden column); the grid does not list it.
    #[serde(default)]
    pub hidden: bool,
}

impl ColumnInfo {
    /// Clearing a text cell stages `''`; any other class stages NULL.
    pub fn clears_to_null(&self) -> bool {
        !matches!(self.class, TypeClass::Text | TypeClass::LongText)
    }

    /// The type a value to write is cast to. Postgres defines an explicit cast to
    /// `varchar(n)`, `char(n)` and `bit varying(n)` as cutting the value to the length, and to
    /// `bit(n)` as cutting or padding it, without an error; only the assignment to the column (or
    /// to a domain over such a type) refuses a value that does not fit. So these, their arrays
    /// and domains over them are cast to the type without its length: `text`, `bit varying`,
    /// `text[]` or `bit varying[]`. Every other type is cast to [`type_sql`](Self::type_sql)
    /// itself. A key value the server sent fits its column, so key comparisons keep `type_sql`.
    /// Filters use these types too, except for `char(n)`, `bit(n)` and the arrays of
    /// `varchar(n)`, `char(n)` and `bit(n)`, which they compare as `bpchar`, `"bit"`,
    /// `character varying[]`, `bpchar[]` and `"bit"[]` to keep the column's operators and indexes
    /// (`dbee_sql::filter`).
    pub fn write_type_sql(&self) -> &str {
        match self.base_type.as_str() {
            "varchar" | "bpchar" => "text",
            "bit" | "varbit" => "bit varying",
            "_varchar" | "_bpchar" => "text[]",
            "_bit" | "_varbit" => "bit varying[]",
            _ => &self.type_sql,
        }
    }

    /// Why the column's cells cannot be edited, or `None` when they can: Postgres computes the
    /// value of a generated column and of an identity column `GENERATED ALWAYS`.
    pub fn not_editable(&self) -> Option<&'static str> {
        match self.generated {
            Generated::No => None,
            Generated::Expression => Some("generated columns are read-only"),
            Generated::Identity => Some("identity columns GENERATED ALWAYS are read-only"),
        }
    }

    /// Editors for these classes take Enter as a newline.
    pub fn multiline(&self) -> bool {
        matches!(self.class, TypeClass::LongText | TypeClass::Json)
    }

    pub fn column(&self) -> Column {
        Column { name: self.name.clone(), base_type: self.base_type.clone(), type_label: self.type_sql.clone(), class: self.class }
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
    /// Why the grid cannot edit this table, whatever its keys (for example an engine that cannot
    /// undo a write). `None` when it can.
    #[serde(default)]
    pub no_edit_reason: Option<String>,
}

impl TableInfo {
    /// The primary key, else the first unique key whose columns are all NOT NULL.
    /// `None` for views and other relations that cannot be edited, for a table with a
    /// [`no_edit_reason`](Self::no_edit_reason), or when there is no key.
    pub fn row_key(&self) -> Option<Vec<&ColumnInfo>> {
        if self.no_edit_reason.is_some() || !matches!(self.kind, RelKind::Table | RelKind::PartitionedTable) {
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
        ColumnInfo {
            name: name.into(),
            type_sql: "text".into(),
            base_type: "text".into(),
            class: TypeClass::Text,
            not_null,
            has_default: false,
            generated: Generated::No,
            hidden: false,
        }
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
            no_edit_reason: None,
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
    fn classes_drive_clearing_and_lines() {
        let info = |class| ColumnInfo { class, ..col("x", false) };
        assert!(!info(TypeClass::Text).clears_to_null());
        assert!(!info(TypeClass::LongText).clears_to_null());
        for class in [TypeClass::Number, TypeClass::Bool, TypeClass::Json, TypeClass::DateTime, TypeClass::Binary, TypeClass::Other] {
            assert!(info(class).clears_to_null(), "{class:?}");
        }
        assert!(info(TypeClass::Json).multiline());
        assert!(info(TypeClass::LongText).multiline());
        for class in [TypeClass::Text, TypeClass::Number, TypeClass::Bool, TypeClass::DateTime, TypeClass::Binary, TypeClass::Other] {
            assert!(!info(class).multiline(), "{class:?}");
        }
        let mut c = col("n", false);
        c.type_sql = "character varying(200)".into();
        c.base_type = "varchar".into();
        let col = c.column();
        assert_eq!(
            (col.name.as_str(), col.base_type.as_str(), col.type_label.as_str(), col.class),
            ("n", "varchar", "character varying(200)", TypeClass::Text)
        );
    }

    #[test]
    fn no_edit_reason_blocks_row_key() {
        let mut t = table("shop", "users");
        t.primary_key = vec!["id".into()];
        assert!(t.row_key().is_some());
        t.no_edit_reason = Some("The MyISAM table engine can't undo a write".into());
        assert!(t.row_key().is_none());
        // Catalogs saved before the field existed have none.
        let mut j = serde_json::to_value(table("shop", "users")).unwrap();
        j.as_object_mut().unwrap().remove("noEditReason");
        assert_eq!(serde_json::from_value::<TableInfo>(j).unwrap().no_edit_reason, None);
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
    fn computed_columns_say_why_they_cannot_be_edited() {
        let mut c = col("x", false);
        assert_eq!(c.not_editable(), None);
        c.generated = Generated::Expression;
        assert_eq!(c.not_editable(), Some("generated columns are read-only"));
        c.generated = Generated::Identity;
        assert_eq!(c.not_editable(), Some("identity columns GENERATED ALWAYS are read-only"));
        // Catalogs saved before the field existed read as ordinary columns.
        let mut j = serde_json::to_value(col("x", false)).unwrap();
        j.as_object_mut().unwrap().remove("generated");
        assert_eq!(serde_json::from_value::<ColumnInfo>(j).unwrap().generated, Generated::No);
        assert_eq!(serde_json::to_value(Generated::Identity).unwrap(), "identity");
    }

    #[test]
    fn write_type_drops_the_lengths_an_explicit_cast_cuts_to() {
        let typed = |base: &str, type_sql: &str| ColumnInfo { type_sql: type_sql.into(), base_type: base.into(), ..col("x", false) };
        for (base, type_sql, write) in [
            ("varchar", "character varying(10)", "text"),
            ("varchar", "character varying", "text"),
            ("bpchar", "character(2)", "text"),
            ("bit", "bit(2)", "bit varying"),
            ("varbit", "bit varying(2)", "bit varying"),
            ("_varchar", "character varying(3)[]", "text[]"),
            ("_bpchar", "character(2)[]", "text[]"),
            ("_bit", "bit(2)[]", "bit varying[]"),
            ("_varbit", "bit varying(2)[]", "bit varying[]"),
            // A domain over varchar(5): the base type decides, the assignment to the column checks the domain.
            ("varchar", "scratch.d5", "text"),
            // Everything else is written as its own type.
            ("text", "text", "text"),
            ("int4", "integer", "integer"),
            ("numeric", "numeric(12,2)", "numeric(12,2)"),
            ("timestamp", "timestamp(3) without time zone", "timestamp(3) without time zone"),
            ("char", "\"char\"", "\"char\""),
            ("_text", "text[]", "text[]"),
            ("order_status", "shop.order_status", "shop.order_status"),
        ] {
            let c = typed(base, type_sql);
            assert_eq!(c.write_type_sql(), write, "{base} {type_sql}");
            assert_eq!(c.type_sql, type_sql, "the type itself is unchanged");
        }
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
