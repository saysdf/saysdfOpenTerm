//! Dolt helpers (design D7): branches, history, `AS OF '<ref>'` with a validated ref.
//! The backend itself is [`crate::mysql_backend::MysqlBackend`] with `Kind::Dolt`.

use crate::{Error, Result};

pub const BRANCHES_SQL: &str =
    "SELECT name, hash, latest_commit_message FROM dolt_branches ORDER BY name";
pub const LOG_SQL: &str = "SELECT commit_hash, committer, CAST(date AS CHAR), message FROM dolt_log ORDER BY date DESC LIMIT ?";

/// [`LOG_SQL`] for the current branch, or `dolt_log AS OF '<ref>'` for another one.
pub fn log_sql(at: Option<&str>) -> Result<String> {
    Ok(match at {
        None => LOG_SQL.to_string(),
        Some(r) => LOG_SQL.replace(
            "FROM dolt_log",
            &format!("FROM dolt_log{}", as_of_clause(r)?),
        ),
    })
}

/// A branch, tag or commit ref: `[A-Za-z0-9_./~^-]`, 1..=200 chars, no `..` tricks needed
/// beyond that because the value is also quoted.
pub fn validate_ref(r: &str) -> Result<&str> {
    let ok = !r.is_empty()
        && r.len() <= 200
        && !r.starts_with('-')
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./~^-".contains(c));
    if ok {
        Ok(r)
    } else {
        Err(Error::InvalidArgument(format!(
            "invalid Dolt ref (allowed: letters, digits, _ . / ~ ^ -): {:?}",
            r.chars().take(40).collect::<String>()
        )))
    }
}

/// ` AS OF '<ref>'` (leading space).
pub fn as_of_clause(r: &str) -> Result<String> {
    let r = validate_ref(r)?;
    Ok(format!(" AS OF '{}'", r.replace('\'', "''")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{page_sql, Kind, PageRequest, TableRef};

    #[test]
    fn refs() {
        assert!(validate_ref("main").is_ok());
        assert!(validate_ref("feature/x-1").is_ok());
        assert!(validate_ref("HEAD~2").is_ok());
        assert!(validate_ref("abc123def").is_ok());
        for bad in ["", "a'b", "x; drop", "a b", "-x", "a`b", "a\\b"] {
            assert!(validate_ref(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn log_sql_as_of() {
        assert_eq!(log_sql(None).unwrap(), LOG_SQL);
        assert!(log_sql(Some("dev"))
            .unwrap()
            .contains("FROM dolt_log AS OF 'dev' ORDER BY"));
        assert!(log_sql(Some("x' --")).is_err());
    }

    #[test]
    fn as_of_sql() {
        let req = PageRequest {
            table: TableRef {
                schema: None,
                name: "t".into(),
            },
            order_by: None,
            filter: None,
            offset: 0,
            limit: 5,
            as_of: Some("feature/x".into()),
        };
        let (sql, _) = page_sql(Kind::Dolt, &req, "?").unwrap();
        assert_eq!(sql, "SELECT * FROM `t` AS OF 'feature/x' LIMIT 5 OFFSET 0");
        let mut bad = req.clone();
        bad.as_of = Some("x' OR '1".into());
        assert!(page_sql(Kind::Dolt, &bad, "?").is_err());
    }
}
