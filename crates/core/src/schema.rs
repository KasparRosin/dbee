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

/// Whether the engine computes a column's value, so that it takes no written value (on Postgres an
/// `UPDATE` or `INSERT` of one fails with SQLSTATE 428C9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Generated {
    /// An ordinary column; an identity `BY DEFAULT` or a default can be written over.
    #[default]
    No,
    /// A generated column, `GENERATED ALWAYS AS (…)`, stored or virtual.
    Expression,
    /// A Postgres identity column `GENERATED ALWAYS`.
    Identity,
}

/// Whose operators dbee's own SQL names when it compares a column's values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Operators {
    /// None by a schema: the engine's operators have none (MySQL, SQLite).
    #[default]
    Unnamed,
    /// The engine's built-in ones: on Postgres `pg_catalog`'s, for a base type (after domains) or
    /// an array in `pg_catalog`, an enum, a range or a composite.
    Builtin,
    /// The base type's own: an extension's type (`citext`), or a domain over one. dbee compares
    /// the values as `base`, the base type's SQL name, with the operators of its operator class
    /// (`ops`), each named in its own schema, so none is looked up where someone else made one.
    OfType { base: String, ops: TypeOperators },
}

/// A type's own comparisons, from its default btree operator class (`=` and `<>` from its hash
/// class where it has no btree one; `<>` is the `=`'s negator): each operator as SQL names it, with
/// its schema (`ext.=`), or `None` where the class has none.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeOperators {
    pub eq: Option<String>,
    pub ne: Option<String>,
    pub lt: Option<String>,
    pub le: Option<String>,
    pub gt: Option<String>,
    pub ge: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnInfo {
    pub name: String,
    /// The column's type as the server names it, used in key casts.
    pub type_sql: String,
    /// The engine's own base type name (Postgres `pg_type.typname`), read only by that engine's
    /// dialect and driver.
    pub base_type: String,
    /// What the values are, for the grid, editors and copies.
    pub class: TypeClass,
    pub not_null: bool,
    pub has_default: bool,
    /// The engine computes the value (Postgres's `attgenerated` and `attidentity = 'a'`, a generated
    /// column on MySQL, MariaDB and SQLite).
    #[serde(default)]
    pub generated: Generated,
    /// A column the engine keeps out of sight (a SQLite hidden column): the grid, sort, filters and
    /// copies leave it out, and a page reads it only when it is part of the row key.
    #[serde(default)]
    pub hidden: bool,
    /// The type as the user reads it: on Postgres `type_sql` without a schema that is on the
    /// session's search path, as Postgres itself shows it; empty means `type_sql`.
    #[serde(default)]
    pub type_label: String,
    /// Whose operators compare the column's values, which dbee's own SQL names.
    #[serde(default)]
    pub operators: Operators,
}

impl ColumnInfo {
    /// Clearing a text cell stages `''`; any other class stages NULL.
    pub fn clears_to_null(&self) -> bool {
        !matches!(self.class, TypeClass::Text | TypeClass::LongText)
    }

    /// Why the column's cells cannot be edited, or `None` when they can: the engine computes the
    /// value of a generated column, and Postgres that of an identity column `GENERATED ALWAYS`.
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
        Column { name: self.name.clone(), base_type: self.base_type.clone(), type_label: self.label().to_owned(), class: self.class }
    }

    /// The type as the user reads it: [`ColumnInfo::type_label`], or `type_sql` when that is
    /// empty. Every display of a column's type reads this.
    pub fn label(&self) -> &str {
        if self.type_label.is_empty() { &self.type_sql } else { &self.type_label }
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

/// One part of an index, in index order: a column, or an expression as the engine writes it out
/// (`lower(email)`). SQLite names no expression when it cannot be read from the index's
/// statement: then both are `None`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexPart {
    pub column: Option<String>,
    pub expression: Option<String>,
    pub descending: bool,
}

/// An index of a table, as the structure view lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    pub name: String,
    pub parts: Vec<IndexPart>,
    pub unique: bool,
    /// The index of the primary key.
    pub primary: bool,
    /// The access method or kind as the engine names it (Postgres `btree`, `gin`; MySQL `BTREE`,
    /// `FULLTEXT`); `None` where the engine has one kind (SQLite).
    pub method: Option<String>,
    /// A partial index's condition, as the engine writes it out.
    pub predicate: Option<String>,
}

/// When a trigger fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriggerTiming {
    Before,
    After,
    InsteadOf,
}

/// What fires a trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriggerEvent {
    Insert,
    Update,
    Delete,
    Truncate,
}

/// A trigger on a table, as the structure view lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerInfo {
    pub name: String,
    /// `None` when the engine's text could not be read for it.
    pub timing: Option<TriggerTiming>,
    /// In the order insert, update, delete, truncate.
    pub events: Vec<TriggerEvent>,
    /// Once for each row (`true`) or once for each statement; `None` when unknown.
    pub for_each_row: Option<bool>,
    /// The trigger as the engine gives it: Postgres's `CREATE TRIGGER` statement, SQLite's, and
    /// MySQL's and MariaDB's action (the body after `FOR EACH ROW`).
    pub definition: String,
}

/// What a foreign key does when the row it references is deleted or its key updated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FkAction {
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

/// A column's default as the structure view writes it: the server's text (`'pending'::text`,
/// `now()`), or a dbee word where dbee names it (`identity`, `auto_increment`, `generated`, or
/// `generated always as (…)`). `None` when the column has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnDefault {
    pub column: String,
    pub default: Option<String>,
}

/// A foreign key's actions, named by its columns (in key order) and the table it references, as
/// the catalog's [`ForeignKey`] is. `None` when the engine's word was not one dbee knows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForeignKeyActions {
    pub columns: Vec<String>,
    pub target: TableRef,
    pub on_delete: Option<FkAction>,
    pub on_update: Option<FkAction>,
}

/// What a table's structure adds to the catalog: its indexes, triggers, column defaults and
/// foreign key actions, read when asked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableStructure {
    /// The primary key's index first, then by name.
    pub indexes: Vec<IndexInfo>,
    /// By name.
    pub triggers: Vec<TriggerInfo>,
    /// In column order; a column without a default may have no entry.
    #[serde(default)]
    pub defaults: Vec<ColumnDefault>,
    /// One per foreign key the engine reports.
    #[serde(default)]
    pub actions: Vec<ForeignKeyActions>,
}

impl TableStructure {
    /// The actions of the foreign key on `columns` that points at `schema.name`: the key whose
    /// target is spelled the same, else one spelled the same ignoring ASCII case (SQLite keeps a
    /// key's REFERENCES as written, the catalog the table's own name).
    pub fn actions_of(&self, columns: &[&str], schema: &str, name: &str) -> Option<&ForeignKeyActions> {
        let same = |a: &&ForeignKeyActions| a.columns.iter().map(String::as_str).eq(columns.iter().copied()) && a.target.schema == schema;
        self.actions
            .iter()
            .filter(same)
            .find(|a| a.target.name == name)
            .or_else(|| self.actions.iter().filter(same).find(|a| a.target.name.eq_ignore_ascii_case(name)))
    }
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

    /// The columns the grid shows, and that sort, filters, copies and the value view know: every
    /// column but the hidden ones. The one place that decides what is visible.
    pub fn visible_columns(&self) -> impl Iterator<Item = &ColumnInfo> {
        self.columns.iter().filter(|c| !c.hidden)
    }

    /// The visible column named `name`.
    pub fn visible_column(&self, name: &str) -> Option<&ColumnInfo> {
        self.visible_columns().find(|c| c.name == name)
    }

    /// The columns a page reads: the visible ones in order, then the row key's columns that are
    /// not visible (a SQLite rowid), which the page keeps for its edits.
    pub fn page_columns(&self) -> Vec<&ColumnInfo> {
        let mut page: Vec<&ColumnInfo> = self.visible_columns().collect();
        for key in self.row_key().unwrap_or_default() {
            if !page.iter().any(|c| c.name == key.name) {
                page.push(key);
            }
        }
        page
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
    /// The schema to show first, as the driver names it (Postgres `public`), when it exists.
    #[serde(default)]
    pub default_schema: Option<String>,
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

    /// The foreign keys of every table that reference `t`, with the table each belongs to: tables
    /// in catalog order, each table's keys in its order. A table that references itself is listed.
    pub fn referenced_by(&self, t: &TableRef) -> Vec<(&TableRef, &ForeignKey)> {
        let tables = self.schemas.iter().flat_map(|s| s.tables.iter());
        tables.flat_map(|info| info.foreign_keys.iter().filter(|fk| fk.target == *t).map(move |fk| (&info.table, fk))).collect()
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
            type_label: String::new(),
            operators: Operators::Unnamed,
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
    fn hidden_columns_are_left_out_and_a_page_reads_the_key_too() {
        let mut t = table("main", "notes");
        t.columns.insert(0, ColumnInfo { hidden: true, ..col("rowid", true) });
        t.columns.push(ColumnInfo { hidden: true, ..col("rank", false) });
        assert_eq!(t.visible_columns().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["id", "email", "nick"]);
        assert!(t.visible_column("rowid").is_none() && t.visible_column("rank").is_none());
        assert_eq!(t.visible_column("email").map(|c| c.name.as_str()), Some("email"));
        assert!(t.column("rowid").is_some(), "the catalog keeps every column");
        // A page reads what shows, then the key's hidden columns; never another hidden one.
        assert_eq!(names(t.page_columns()), ["id", "email", "nick"]);
        t.primary_key = vec!["rowid".into()];
        assert_eq!(names(t.page_columns()), ["id", "email", "nick", "rowid"]);
        t.primary_key = vec!["email".into(), "rowid".into()];
        assert_eq!(names(t.page_columns()), ["id", "email", "nick", "rowid"]);
        t.primary_key = vec!["id".into()];
        assert_eq!(names(t.page_columns()), ["id", "email", "nick"]);
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
            default_schema: None,
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
    fn referenced_by_lists_every_key_that_points_at_the_table() {
        let fk = |name: &str, target: &str| ForeignKey {
            name: name.into(),
            columns: vec!["id".into()],
            target: TableRef { schema: "shop".into(), name: target.into() },
            target_columns: vec!["id".into()],
        };
        let mut orders = table("shop", "orders");
        orders.foreign_keys = vec![fk("orders_user", "users"), fk("orders_item", "items")];
        let mut users = table("shop", "users");
        users.foreign_keys = vec![fk("users_parent", "users")];
        let mut elsewhere = table("other", "users");
        elsewhere.foreign_keys = vec![fk("other_user", "users")];
        let cat = Catalog {
            schemas: vec![
                SchemaInfo { name: "other".into(), system: false, tables: vec![elsewhere] },
                SchemaInfo { name: "shop".into(), system: false, tables: vec![orders, users] },
            ],
            default_schema: None,
        };
        let users = TableRef { schema: "shop".into(), name: "users".into() };
        let found: Vec<(String, &str)> = cat.referenced_by(&users).into_iter().map(|(t, fk)| (t.display(), fk.name.as_str())).collect();
        assert_eq!(found, [("other.users".to_string(), "other_user"), ("shop.orders".into(), "orders_user"), ("shop.users".into(), "users_parent")]);
        assert!(cat.referenced_by(&TableRef { schema: "shop".into(), name: "nobody".into() }).is_empty());
    }

    #[test]
    fn structure_serializes_in_camel_case() {
        let s = TableStructure {
            indexes: vec![IndexInfo {
                name: "i".into(),
                parts: vec![IndexPart { column: None, expression: Some("lower(a)".into()), descending: true }],
                unique: true,
                primary: false,
                method: Some("btree".into()),
                predicate: Some("a IS NOT NULL".into()),
            }],
            triggers: vec![TriggerInfo {
                name: "t".into(),
                timing: Some(TriggerTiming::InsteadOf),
                events: vec![TriggerEvent::Insert, TriggerEvent::Truncate],
                for_each_row: Some(false),
                definition: "CREATE TRIGGER t".into(),
            }],
            defaults: vec![ColumnDefault { column: "a".into(), default: Some("now()".into()) }],
            actions: vec![ForeignKeyActions {
                columns: vec!["a".into()],
                target: TableRef { schema: "s".into(), name: "p".into() },
                on_delete: Some(FkAction::SetNull),
                on_update: None,
            }],
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["triggers"][0]["timing"], "insteadOf");
        assert_eq!(j["defaults"][0], serde_json::json!({ "column": "a", "default": "now()" }));
        assert_eq!(j["actions"][0]["onDelete"], "setNull");
        assert_eq!(j["actions"][0]["onUpdate"], serde_json::Value::Null);
        assert_eq!(serde_json::json!(FkAction::NoAction), "noAction");
        let old: TableStructure = serde_json::from_value(serde_json::json!({ "indexes": [], "triggers": [] })).unwrap();
        assert!(old.defaults.is_empty() && old.actions.is_empty());
        assert_eq!(j["triggers"][0]["events"], serde_json::json!(["insert", "truncate"]));
        assert_eq!(j["triggers"][0]["forEachRow"], false);
        assert_eq!(j["indexes"][0]["parts"][0]["expression"], "lower(a)");
        assert_eq!(serde_json::from_value::<TableStructure>(j).unwrap(), s);
    }

    #[test]
    #[ignore = "timing; run with --ignored --nocapture"]
    fn filter_1000_tables_within_a_frame() {
        let tables: Vec<TableInfo> = (0..1000).map(|i| table("public", &format!("Table_{i}_Name"))).collect();
        let c = Catalog { schemas: vec![SchemaInfo { name: "public".into(), system: false, tables }], default_schema: None };
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
