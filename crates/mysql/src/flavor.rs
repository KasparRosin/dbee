//! The members of the MySQL family, told apart by the server's version, and the version
//! rules: MySQL from 5.7, MariaDB from 10.11.

/// Which server a connection reached, told by its version: the MySQL family's members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    MySql,
    MariaDb,
}

impl Flavor {
    /// MariaDB says so in its version (`10.11.19-MariaDB-ubu2204`); any other server is MySQL.
    pub fn from_version(version: &str) -> Flavor {
        if version.contains("MariaDB") { Flavor::MariaDb } else { Flavor::MySql }
    }

    /// [`ServerInfo::product`].
    pub fn product(self) -> &'static str {
        match self {
            Flavor::MySql => "MySQL",
            Flavor::MariaDb => "MariaDB",
        }
    }

    /// What an error number is called in the error text: `… (MySQL error 1146)`.
    pub(crate) fn code_label(self) -> &'static str {
        match self {
            Flavor::MySql => "MySQL error",
            Flavor::MariaDb => "MariaDB error",
        }
    }

    /// The oldest supported server, as [`version_num`]: MySQL 5.7, MariaDB 10.11.
    pub fn min_version_num(self) -> u32 {
        match self {
            Flavor::MySql => 50700,
            Flavor::MariaDb => 101100,
        }
    }

    /// The session variable that says whether transactions are read-only, then the other
    /// spelling: MySQL has `transaction_read_only` (from 5.7.20), MariaDB 10.11 only
    /// `tx_read_only`.
    pub(crate) fn read_only_variables(self) -> [&'static str; 2] {
        match self {
            Flavor::MySql => ["transaction_read_only", "tx_read_only"],
            Flavor::MariaDb => ["tx_read_only", "transaction_read_only"],
        }
    }

    /// The text `information_schema.COLUMNS.COLUMN_DEFAULT` holds for a column whose default is
    /// NULL: MariaDB writes `NULL` and quotes string defaults; MySQL gives SQL NULL, so its
    /// `NULL` is a string default.
    pub(crate) fn null_default(self) -> Option<&'static str> {
        match self {
            Flavor::MySql => None,
            Flavor::MariaDb => Some("NULL"),
        }
    }

    /// A string column with its own `json_valid` check, which
    /// `information_schema.CHECK_CONSTRAINTS` lists, holds JSON: MariaDB's, whose JSON type is
    /// such a `LONGTEXT`. MySQL has a JSON type, and no such table before 8.0.16.
    pub(crate) fn json_is_checked_text(self) -> bool {
        self == Flavor::MariaDb
    }
}

/// `@@version`'s leading `major.minor.patch` as one number that grows with it:
/// `major * 10000 + minor * 100 + patch` (`8.0.46` is 80046, `10.11.19` is 101119).
pub fn version_num(version: &str) -> Option<u32> {
    let mut rest = version;
    let mut parts = [0u32; 3];
    for (i, part) in parts.iter_mut().enumerate() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        *part = rest.get(..digits)?.parse().ok()?;
        rest = rest.get(digits..)?;
        if i < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    let [major, minor, patch] = parts;
    major.checked_mul(10_000)?.checked_add(minor.min(99) * 100)?.checked_add(patch.min(99))
}

/// The warning for a server older than its flavor's [`Flavor::min_version_num`], or `None`.
pub fn version_warning(flavor: Flavor, version_num: u32) -> Option<String> {
    let min = flavor.min_version_num();
    if version_num >= min {
        return None;
    }
    let short = |n: u32| format!("{}.{}", n / 10_000, n / 100 % 100);
    Some(format!(
        "{} {} is older than {}, the oldest version dbee supports; some things may not work",
        flavor.product(),
        short(version_num),
        short(min)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_variables_by_flavor() {
        assert_eq!(Flavor::MySql.read_only_variables()[0], "transaction_read_only");
        assert_eq!(Flavor::MariaDb.read_only_variables()[0], "tx_read_only");
        assert_eq!((Flavor::MySql.code_label(), Flavor::MariaDb.code_label()), ("MySQL error", "MariaDB error"));
    }

    #[test]
    fn catalog_spellings_by_flavor() {
        assert_eq!((Flavor::MySql.null_default(), Flavor::MariaDb.null_default()), (None, Some("NULL")));
        assert!(!Flavor::MySql.json_is_checked_text() && Flavor::MariaDb.json_is_checked_text());
    }
}
