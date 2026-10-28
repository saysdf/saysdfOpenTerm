//! Statement checks before a user query reaches the database (design D4).
//!
//! This is an additional layer, not the main protection: the database itself is in read-only
//! mode. The lexer understands quotes (`'..'`, `".."`, `` `..` ``), comments (`--`, `/* */`,
//! MySQL `#`) and Postgres dollar quoting.

use crate::{Error, Kind, Result};

/// Result of lexing one query.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Lexed {
    /// Upper-cased bare words and single punctuation characters of the first statement,
    /// outside strings and comments.
    pub words: Vec<String>,
    /// Non-empty statements separated by top-level `;`.
    pub statements: usize,
    /// MySQL executable comment `/*! ... */` seen.
    pub executable_comment: bool,
}

pub fn lex(sql: &str, kind: Kind) -> Lexed {
    let b = sql.as_bytes();
    let mysql = matches!(kind, Kind::Mysql | Kind::Dolt);
    let mut out = Lexed::default();
    let mut i = 0;
    let mut cur_has_content = false;
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut Lexed| {
        if !word.is_empty() {
            if out.statements == 0 {
                out.words.push(word.to_ascii_uppercase());
            }
            word.clear();
        }
    };
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        // comments
        if c == b'-' && next == Some(b'-') || mysql && c == b'#' {
            flush(&mut word, &mut out);
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && next == Some(b'*') {
            flush(&mut word, &mut out);
            if mysql && b.get(i + 2) == Some(&b'!') {
                out.executable_comment = true;
            }
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // quoted strings / identifiers
        if c == b'\'' || c == b'"' || c == b'`' {
            flush(&mut word, &mut out);
            cur_has_content = true;
            i += 1;
            while i < b.len() {
                if mysql && b[i] == b'\\' && c != b'`' {
                    i += 2;
                    continue;
                }
                if b[i] == c {
                    if b.get(i + 1) == Some(&c) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        // Postgres $tag$ ... $tag$
        if kind == Kind::Postgres && c == b'$' && word.is_empty() {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < b.len() && b[j] == b'$' {
                let tag = &sql[i..=j];
                cur_has_content = true;
                match sql[j + 1..].find(tag) {
                    Some(p) => i = j + 1 + p + tag.len(),
                    None => i = b.len(),
                }
                continue;
            }
        }
        if c == b';' {
            flush(&mut word, &mut out);
            if cur_has_content {
                out.statements += 1;
                cur_has_content = false;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 {
            word.push(c as char);
            cur_has_content = true;
        } else {
            flush(&mut word, &mut out);
            if !c.is_ascii_whitespace() {
                cur_has_content = true;
                if out.statements == 0 {
                    out.words.push((c as char).to_string());
                }
            }
        }
        i += 1;
    }
    flush(&mut word, &mut out);
    if cur_has_content {
        out.statements += 1;
    }
    out
}

/// Statements that change the session/transaction mode or touch files: refused in read-only.
const DENY_FIRST: &[&str] = &[
    "SET",
    "RESET",
    "BEGIN",
    "START",
    "COMMIT",
    "END",
    "ROLLBACK",
    "ABORT",
    "SAVEPOINT",
    "RELEASE",
    "ATTACH",
    "DETACH",
    "VACUUM",
    "COPY",
    "LOAD",
    "PREPARE",
    "EXECUTE",
    "DEALLOCATE",
    "DISCARD",
    "LOCK",
    "UNLOCK",
    "USE",
    "CHECKPOINT",
    "LISTEN",
    "NOTIFY",
    "HANDLER",
    "INSTALL",
    "UNINSTALL",
    "SHUTDOWN",
    "KILL",
    "FLUSH",
    "PURGE",
    "CHANGE",
];

/// Read-only pragmas that take an argument.
const PRAGMA_ARG_ALLOW: &[&str] = &[
    "TABLE_INFO",
    "TABLE_XINFO",
    "TABLE_LIST",
    "INDEX_LIST",
    "INDEX_INFO",
    "INDEX_XINFO",
    "FOREIGN_KEY_LIST",
    "FOREIGN_KEY_CHECK",
    "INTEGRITY_CHECK",
    "QUICK_CHECK",
];

/// MySQL/Dolt read-only: only these leading words (whether `READ ONLY` blocks DDL there is
/// still to be verified manually, so DDL is refused here regardless).
const MYSQL_RO_ALLOW: &[&str] = &[
    "SELECT", "WITH", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "TABLE", "VALUES",
];

/// Checks one user query. Always: exactly one statement. Read-only: deny-list.
pub fn check(sql: &str, kind: Kind, read_only: bool) -> Result<Lexed> {
    let lx = lex(sql, kind);
    if lx.statements == 0 {
        return Err(Error::InvalidArgument("empty query".into()));
    }
    if lx.statements > 1 {
        return Err(Error::MultipleStatements);
    }
    if !read_only {
        return Ok(lx);
    }
    let first = lx.words.first().map(String::as_str).unwrap_or("");
    if DENY_FIRST.contains(&first) {
        return Err(Error::Denied(first.to_string()));
    }
    if first == "PRAGMA" {
        let w = |i: usize| lx.words.get(i).map(String::as_str).unwrap_or("");
        // `PRAGMA [schema.]name [= v | (v)]`
        let (name, rest) = if w(2) == "." { (w(3), 4) } else { (w(1), 2) };
        let has_arg = matches!(w(rest), "=" | "(");
        if name == "QUERY_ONLY" || (has_arg && !PRAGMA_ARG_ALLOW.contains(&name)) {
            return Err(Error::Denied(format!("PRAGMA {name}")));
        }
    }
    if matches!(kind, Kind::Mysql | Kind::Dolt) {
        if lx.executable_comment {
            return Err(Error::Denied("MySQL executable comment /*! */".into()));
        }
        if !MYSQL_RO_ALLOW.contains(&first) {
            return Err(Error::Denied(first.to_string()));
        }
        if lx
            .words
            .windows(2)
            .any(|w| w[0] == "INTO" && (w[1] == "OUTFILE" || w[1] == "DUMPFILE"))
        {
            return Err(Error::Denied("SELECT ... INTO OUTFILE".into()));
        }
    }
    Ok(lx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn denied(sql: &str, kind: Kind) -> bool {
        check(sql, kind, true).is_err()
    }

    #[test]
    fn single_statement() {
        assert_eq!(lex("select 1;", Kind::Sqlite).statements, 1);
        assert_eq!(lex("select 1; -- c\n ;", Kind::Sqlite).statements, 1);
        assert_eq!(lex("select ';'; ", Kind::Sqlite).statements, 1);
        assert_eq!(lex("select $$a;b$$", Kind::Postgres).statements, 1);
        assert_eq!(lex("select 'a\\';' ; select 2", Kind::Mysql).statements, 2);
        assert!(matches!(
            check("select 1; drop table t", Kind::Sqlite, false),
            Err(Error::MultipleStatements)
        ));
        assert!(check("  ;  ", Kind::Sqlite, false).is_err());
    }

    #[test]
    fn deny_list() {
        for s in [
            "SET TRANSACTION READ WRITE",
            "set session characteristics as transaction read write",
            "SET transaction_read_only = 0",
            "commit",
            "BEGIN",
            "start transaction",
            "PRAGMA query_only=OFF",
            "pragma query_only(0)",
            "PRAGMA main.query_only = 0",
            "pragma journal_mode=WAL",
            "PRAGMA main.journal_mode = DELETE",
            "ATTACH 'x.db' AS x",
            "VACUUM INTO 'copy.db'",
            "COPY t TO '/tmp/x'",
            "/* c */ RESET ALL",
        ] {
            assert!(denied(s, Kind::Sqlite) || denied(s, Kind::Postgres), "{s}");
        }
        for s in [
            "SELECT * FROM t",
            "pragma table_info('t')",
            "PRAGMA foreign_key_list(t)",
            "PRAGMA user_version",
            "with x as (select 1) select * from x",
            "select 'commit; set'",
        ] {
            assert!(!denied(s, Kind::Sqlite), "{s}");
        }
        assert!(!check("SET TRANSACTION READ WRITE", Kind::Postgres, false).is_err());
    }

    #[test]
    fn mysql_read_only_allowlist() {
        assert!(!denied("SELECT 1", Kind::Mysql));
        assert!(!denied("show tables", Kind::Dolt));
        assert!(denied("CREATE TABLE t (a int)", Kind::Mysql));
        assert!(denied("insert into t values (1)", Kind::Dolt));
        assert!(denied(
            "/*!50000 SET SESSION transaction_read_only=0 */ SELECT 1",
            Kind::Mysql
        ));
        assert!(denied("select * from t into outfile '/tmp/x'", Kind::Mysql));
        assert!(denied("CALL dolt_checkout('b')", Kind::Dolt));
    }
}
