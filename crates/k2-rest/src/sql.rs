//! Quoting. Every name the door puts in SQL is an identifier in double
//! quotes; every value is a string literal (`standard_conforming_strings`
//! is on, Postgres's default since 9.1, so a backslash is just a
//! backslash). A value with a NUL byte can't be a Postgres string at all:
//! [`check_value`] refuses it before any SQL is built.

/// `name` as a quoted identifier.
pub fn qi(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `v` as a string literal.
pub fn lit(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// A value from a request: no NUL.
pub fn check_value(v: &str) -> Result<(), crate::ApiError> {
    if v.contains('\0') {
        return Err(crate::ApiError::new(400, "22021", "invalid byte sequence: a value may not hold a NUL character"));
    }
    Ok(())
}

/// A cast's type name: one word (`[A-Za-z_][A-Za-z0-9_]*`) or one of
/// Postgres's few two-to-four word names, with an optional `[]`. Nothing
/// else (no parentheses, dots or other spaces) can reach the SQL.
pub fn is_type_name(t: &str) -> bool {
    const MULTI: &[&str] = &[
        "double precision",
        "character varying",
        "timestamp with time zone",
        "timestamp without time zone",
        "time with time zone",
        "time without time zone",
    ];
    let base = t.strip_suffix("[]").unwrap_or(t);
    if MULTI.contains(&base.to_ascii_lowercase().as_str()) {
        return true;
    }
    base.len() <= 63
        && base.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_doubles_the_quote() {
        assert_eq!(qi("a\"b"), "\"a\"\"b\"");
        assert_eq!(lit("it's \\ fine"), "'it''s \\ fine'");
        assert!(check_value("x\0y").is_err());
        assert!(is_type_name("text") && is_type_name("double precision") && is_type_name("int4[]"));
        assert!(!is_type_name("text; drop") && !is_type_name("1x") && !is_type_name(""));
        assert!(!is_type_name("int) union select (1") && !is_type_name("int union select 1") && !is_type_name("pg_catalog.text"));
    }
}
