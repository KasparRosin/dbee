use dbee_core::{ColumnInfo, RelKind, TableInfo, TableRef};
use dbee_sql::SqlError;
use dbee_sql::edit::{NewValue, delete_sql, insert_sql, update_sql};
use dbee_sql::filter::{Condition, Filter, Op, Sort, SortDir, where_sql};
use dbee_sql::select::{PAGE_SIZE, count_sql, page_sql};

fn ci(name: &str, type_sql: &str, category: char) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        type_sql: type_sql.into(),
        type_oid: 0,
        base_type: type_sql.into(),
        category,
        not_null: false,
        has_default: false,
    }
}

fn table_of(schema: &str, name: &str, cols: Vec<ColumnInfo>, key: &[&str]) -> TableInfo {
    TableInfo {
        table: TableRef { schema: schema.into(), name: name.into() },
        kind: RelKind::Table,
        columns: cols,
        primary_key: key.iter().map(|s| s.to_string()).collect(),
        unique_keys: vec![],
        foreign_keys: vec![],
        estimated_rows: None,
    }
}

fn orders() -> TableInfo {
    table_of(
        "shop",
        "orders",
        vec![
            ci("id", "bigint", 'N'),
            ci("customer_id", "integer", 'N'),
            ci("dealer_id", "integer", 'N'),
            ci("status", "shop.order_status", 'E'),
            ci("total", "numeric(12,2)", 'N'),
            ci("billing", "jsonb", 'U'),
            ci("shipped_at", "date", 'D'),
            ci("note", "text", 'S'),
        ],
        &["id"],
    )
}

fn f(column: &str, op: Op, value: &str) -> Filter {
    Filter { column: column.into(), op, value: value.into() }
}

fn w(filter: Filter) -> String {
    where_sql(&Condition::Filters(vec![filter]), &orders()).unwrap().unwrap()
}

fn col(name: &str) -> &'static ColumnInfo {
    use std::sync::OnceLock;
    static T: OnceLock<TableInfo> = OnceLock::new();
    T.get_or_init(orders).column(name).unwrap()
}

#[test]
fn the_m1_day_filters() {
    let c = Condition::Filters(vec![f("dealer_id", Op::Eq, "42"), f("status", Op::Eq, "completed"), f("total", Op::Gt, "100")]);
    assert_eq!(
        where_sql(&c, &orders()).unwrap().unwrap(),
        "\"dealer_id\" = CAST('42' AS integer) AND \"status\" = CAST('completed' AS shop.order_status) AND \"total\" > CAST('100' AS numeric(12,2))"
    );
}
#[test]
fn like_casts_column_to_text() {
    assert_eq!(w(f("note", Op::ILike, "%ann%")), "CAST(\"note\" AS text) ILIKE '%ann%'");
    assert_eq!(w(f("note", Op::NotLike, "a'b")), "CAST(\"note\" AS text) NOT LIKE 'a''b'");
}
#[test]
fn in_splits_commas() {
    assert_eq!(w(f("id", Op::In, "1, 2,")), "\"id\" IN (CAST('1' AS bigint), CAST('2' AS bigint))");
    assert_eq!(where_sql(&Condition::Filters(vec![f("id", Op::In, " , ")]), &orders()), Err(SqlError::EmptyIn));
}
#[test]
fn hostile_value_stays_a_value() {
    assert_eq!(w(f("note", Op::Eq, "x'); DROP TABLE t; --")), "\"note\" = CAST('x''); DROP TABLE t; --' AS text)");
    assert_eq!(where_sql(&Condition::Filters(vec![f("note", Op::Eq, "a\0b")]), &orders()), Err(SqlError::Nul));
    assert_eq!(where_sql(&Condition::Filters(vec![f("nope", Op::Eq, "a")]), &orders()), Err(SqlError::UnknownColumn("nope".into())));
}
#[test]
fn blank_value_rows_are_skipped() {
    assert_eq!(where_sql(&Condition::Filters(vec![f("id", Op::Eq, "")]), &orders()).unwrap(), None);
    assert_eq!(where_sql(&Condition::None, &orders()).unwrap(), None);
    assert_eq!(w(f("shipped_at", Op::IsNull, "")), "\"shipped_at\" IS NULL");
    assert_eq!(w(f("shipped_at", Op::IsNotNull, "ignored")), "\"shipped_at\" IS NOT NULL");
}
#[test]
fn raw_condition() {
    assert_eq!(where_sql(&Condition::Raw("total > 5; drop table x".into()), &orders()), Err(SqlError::Semicolon));
    assert_eq!(where_sql(&Condition::Raw("status = 'a;b'".into()), &orders()).unwrap().unwrap(), "(status = 'a;b')");
    assert_eq!(where_sql(&Condition::Raw("  \n ".into()), &orders()).unwrap(), None);
}
#[test]
fn raw_line_comment_cannot_swallow_the_rest() {
    let c = Condition::Raw("total > 5 -- big ones".into());
    assert_eq!(where_sql(&c, &orders()).unwrap().unwrap(), "(total > 5 -- big ones\n)");
    let sql = page_sql(&orders(), &c, None, 0).unwrap();
    assert!(sql.contains("-- big ones\n) ORDER BY \"id\" LIMIT 101"), "{sql}");
    // a block comment, or `--` inside a string, needs no newline
    assert_eq!(where_sql(&Condition::Raw("a /* -- */ = 1".into()), &orders()).unwrap().unwrap(), "(a /* -- */ = 1)");
    assert_eq!(where_sql(&Condition::Raw("note = '--'".into()), &orders()).unwrap().unwrap(), "(note = '--')");
}
#[test]
fn page_and_count() {
    assert_eq!(
        page_sql(&orders(), &Condition::None, None, 2).unwrap(),
        "SELECT \"id\", \"customer_id\", \"dealer_id\", \"status\", \"total\", \"billing\", \"shipped_at\", \"note\" FROM \"shop\".\"orders\" ORDER BY \"id\" LIMIT 101 OFFSET 200"
    );
    assert!(
        page_sql(&orders(), &Condition::None, Some(&Sort { column: "total".into(), dir: SortDir::Desc }), 0)
            .unwrap()
            .ends_with("ORDER BY \"total\" DESC, \"id\" LIMIT 101 OFFSET 0")
    );
    assert_eq!(count_sql(&orders(), &Condition::None).unwrap(), "SELECT count(*) FROM \"shop\".\"orders\"");
    assert_eq!(PAGE_SIZE, 100);
}
#[test]
fn page_details() {
    let c = Condition::Filters(vec![f("id", Op::Gt, "5")]);
    let sql = page_sql(&orders(), &c, Some(&Sort { column: "id".into(), dir: SortDir::Asc }), 1).unwrap();
    assert!(sql.ends_with("WHERE \"id\" > CAST('5' AS bigint) ORDER BY \"id\" ASC LIMIT 101 OFFSET 100"), "{sql}");
    assert!(page_sql(&orders(), &Condition::None, Some(&Sort { column: "x".into(), dir: SortDir::Asc }), 0).is_err());
    assert!(page_sql(&orders(), &Condition::None, None, u64::MAX).unwrap().ends_with("OFFSET 18446744073709551615"));
    let mut keyless = orders();
    keyless.primary_key.clear();
    assert!(!page_sql(&keyless, &Condition::None, None, 0).unwrap().contains("ORDER BY"));
    assert_eq!(count_sql(&orders(), &c).unwrap(), "SELECT count(*) FROM \"shop\".\"orders\" WHERE \"id\" > CAST('5' AS bigint)");
}
#[test]
fn edit_statements() {
    assert_eq!(
        update_sql(&orders(), &[(col("id"), "7")], &[(col("note"), &NewValue::Text("Ann".into())), (col("shipped_at"), &NewValue::Null)]).unwrap(),
        "UPDATE \"shop\".\"orders\" SET \"note\" = CAST('Ann' AS text), \"shipped_at\" = NULL WHERE \"id\" = CAST('7' AS bigint)"
    );
    assert_eq!(insert_sql(&orders(), &[]).unwrap(), "INSERT INTO \"shop\".\"orders\" DEFAULT VALUES");
    assert_eq!(delete_sql(&orders(), &[(col("id"), "7")]).unwrap(), "DELETE FROM \"shop\".\"orders\" WHERE \"id\" = CAST('7' AS bigint)");
}
#[test]
fn edit_errors_and_insert_values() {
    assert_eq!(update_sql(&orders(), &[(col("id"), "7")], &[]), Err(SqlError::NoChanges));
    assert_eq!(update_sql(&orders(), &[], &[(col("note"), &NewValue::Null)]), Err(SqlError::NoKey));
    assert_eq!(delete_sql(&orders(), &[]), Err(SqlError::NoKey));
    assert_eq!(
        insert_sql(&orders(), &[(col("note"), &NewValue::Text("it's".into())), (col("id"), &NewValue::Null)]).unwrap(),
        "INSERT INTO \"shop\".\"orders\" (\"note\", \"id\") VALUES (CAST('it''s' AS text), NULL)"
    );
    assert_eq!(update_sql(&orders(), &[(col("id"), "7")], &[(col("note"), &NewValue::Text("a\0".into()))]), Err(SqlError::Nul));
}
#[test]
fn odd_identifiers_are_quoted() {
    let t = table_of("odd", "Odd \"Name\" 🐝", vec![ci("Total $", "integer", 'N'), ci("k", "integer", 'N')], &["k"]);
    let sort = Sort { column: "Total $".into(), dir: SortDir::Asc };
    let c = Condition::Filters(vec![f("Total $", Op::Eq, "1")]);
    let sql = page_sql(&t, &c, Some(&sort), 0).unwrap();
    assert!(sql.contains("\"odd\".\"Odd \"\"Name\"\" 🐝\""), "{sql}");
    assert!(sql.contains("\"Total $\" = CAST('1' AS integer)"), "{sql}");
    assert!(sql.contains("ORDER BY \"Total $\" ASC, \"k\""), "{sql}");
    assert_eq!(count_sql(&t, &Condition::None).unwrap(), "SELECT count(*) FROM \"odd\".\"Odd \"\"Name\"\" 🐝\"");
}
#[test]
fn filter_serde_round_trip() {
    let flt = f("note", Op::NotILike, "x");
    let json = serde_json::to_string(&flt).unwrap();
    assert_eq!(json, r#"{"column":"note","op":"notIlike","value":"x"}"#);
    assert_eq!(serde_json::from_str::<Filter>(&json).unwrap(), flt);
    for op in Op::ALL {
        assert_eq!(Op::from_id(op.id()), Some(op));
        let s = serde_json::to_string(&op).unwrap();
        assert_eq!(s, format!("\"{}\"", op.id()));
        assert_eq!(serde_json::from_str::<Op>(&s).unwrap(), op);
    }
    assert!(serde_json::from_str::<Op>("\"bogus\"").is_err());
    assert_eq!(Op::from_id("bogus"), None);
    assert_eq!(Op::ALL.map(|o| o.id()), ["eq", "ne", "like", "notLike", "ilike", "notIlike", "lt", "le", "gt", "ge", "in", "isNull", "isNotNull"]);
    assert_eq!(Op::Ne.symbol(), "<>");
    assert_eq!(Op::Le.label(), "less than or equal");
    assert!(Op::Eq.takes_value() && !Op::IsNull.takes_value());
    let nv = serde_json::to_string(&NewValue::Text("a".into())).unwrap();
    assert_eq!(serde_json::from_str::<NewValue>(&nv).unwrap(), NewValue::Text("a".into()));
    assert_eq!(serde_json::from_str::<NewValue>(&serde_json::to_string(&NewValue::Null).unwrap()).unwrap(), NewValue::Null);
}
