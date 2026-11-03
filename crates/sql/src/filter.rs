//! Filters and the WHERE clause. Values are always quoted literals; column names must exist in the table.

use crate::lexer::{TokenKind, has_top_level_semicolon, tokenize};
use crate::{Dialect, SqlError};
use dbee_core::TableInfo;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The 13 filter operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Eq,
    Ne,
    Like,
    NotLike,
    ILike,
    NotILike,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    IsNull,
    IsNotNull,
}

impl Op {
    pub const ALL: [Op; 13] =
        [Op::Eq, Op::Ne, Op::Like, Op::NotLike, Op::ILike, Op::NotILike, Op::Lt, Op::Le, Op::Gt, Op::Ge, Op::In, Op::IsNull, Op::IsNotNull];

    pub fn id(self) -> &'static str {
        match self {
            Op::Eq => "eq",
            Op::Ne => "ne",
            Op::Like => "like",
            Op::NotLike => "notLike",
            Op::ILike => "ilike",
            Op::NotILike => "notIlike",
            Op::Lt => "lt",
            Op::Le => "le",
            Op::Gt => "gt",
            Op::Ge => "ge",
            Op::In => "in",
            Op::IsNull => "isNull",
            Op::IsNotNull => "isNotNull",
        }
    }

    pub fn from_id(id: &str) -> Option<Op> {
        Op::ALL.into_iter().find(|op| op.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Op::Eq => "equals",
            Op::Ne => "does not equal",
            Op::Like => "like",
            Op::NotLike => "not like",
            Op::ILike => "ilike",
            Op::NotILike => "not ilike",
            Op::Lt => "less than",
            Op::Le => "less than or equal",
            Op::Gt => "greater than",
            Op::Ge => "greater than or equal",
            Op::In => "in",
            Op::IsNull => "is null",
            Op::IsNotNull => "is not null",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "<>",
            Op::Like => "LIKE",
            Op::NotLike => "NOT LIKE",
            Op::ILike => "ILIKE",
            Op::NotILike => "NOT ILIKE",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::In => "IN",
            Op::IsNull => "IS NULL",
            Op::IsNotNull => "IS NOT NULL",
        }
    }

    pub fn takes_value(self) -> bool {
        !matches!(self, Op::IsNull | Op::IsNotNull)
    }

    fn is_pattern(self) -> bool {
        matches!(self, Op::Like | Op::NotLike | Op::ILike | Op::NotILike)
    }
}

impl Serialize for Op {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.id())
    }
}

impl<'de> Deserialize<'de> for Op {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Op, D::Error> {
        let id = String::deserialize(d)?;
        Op::from_id(&id).ok_or_else(|| serde::de::Error::custom(format!("unknown filter operator \"{id}\"")))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Filter {
    pub column: String,
    pub op: Op,
    pub value: String,
}

impl Filter {
    /// Whether the filter is part of the condition: an operator that takes a value needs one.
    /// A row with an empty value is left out (so `= ''` cannot be expressed with a filter row).
    pub fn restricts(&self) -> bool {
        !self.op.takes_value() || !self.value.is_empty()
    }
}

/// What restricts a table's rows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Condition {
    #[default]
    None,
    Filters(Vec<Filter>),
    Raw(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sort {
    pub column: String,
    pub dir: SortDir,
}

/// The condition without the `WHERE` keyword, or `None` when nothing restricts the rows. A filter
/// row names a visible column.
pub fn where_sql(d: Dialect, cond: &Condition, table: &TableInfo) -> Result<Option<String>, SqlError> {
    match cond {
        Condition::None => Ok(None),
        Condition::Raw(raw) => raw_sql(d, raw),
        Condition::Filters(filters) => {
            let mut parts = Vec::new();
            for filter in filters {
                if let Some(part) = filter_sql(d, filter, table)? {
                    parts.push(part);
                }
            }
            Ok(if parts.is_empty() { None } else { Some(parts.join(" AND ")) })
        }
    }
}

fn raw_sql(d: Dialect, raw: &str) -> Result<Option<String>, SqlError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    if has_top_level_semicolon(d, raw) {
        return Err(SqlError::Semicolon);
    }
    // A `--` (or MySQL `#`) comment runs to the end of the line, so the closing paren goes on its own line.
    let line_comment = tokenize(d, raw).iter().any(|t| t.kind == TokenKind::Comment && raw.get(t.range.clone()).is_some_and(|s| !s.starts_with('/')));
    Ok(Some(if line_comment { format!("({raw}\n)") } else { format!("({raw})") }))
}

fn filter_sql(d: Dialect, filter: &Filter, table: &TableInfo) -> Result<Option<String>, SqlError> {
    let column = table.visible_column(&filter.column).ok_or_else(|| SqlError::UnknownColumn(filter.column.clone()))?;
    let name = d.ident(&column.name)?;
    if !filter.op.takes_value() {
        return Ok(Some(format!("{name} {}", filter.op.symbol())));
    }
    if !filter.restricts() {
        return Ok(None);
    }
    if filter.op.is_pattern() {
        return Ok(Some(d.like(filter.op, &name, &d.literal(&filter.value)?)));
    }
    if filter.op == Op::In {
        let items =
            filter.value.split(',').map(str::trim).filter(|p| !p.is_empty()).map(|p| d.filter_value(column, p)).collect::<Result<Vec<_>, _>>()?;
        if items.is_empty() {
            return Err(SqlError::EmptyIn);
        }
        return Ok(Some(format!("{name} IN ({})", items.join(", "))));
    }
    Ok(Some(format!("{name} {} {}", filter.op.symbol(), d.filter_value(column, &filter.value)?)))
}
