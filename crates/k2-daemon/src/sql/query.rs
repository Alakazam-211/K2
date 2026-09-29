//! One guest statement for `POST /cli/db/query`.
//!
//! Parsed in-process with the PostgreSQL 16 grammar (`pg_query` 5.1.1)
//! before any user SQL is sent to psql.

use super::ops::OpsError;

/// Fail the HTTP request at this many returned rows. The user text is not
/// given a LIMIT; a wrapper may fetch one past the cap to notice it.
pub const GUEST_ROW_CAP: usize = 5000;

/// Classified user statement. `sql` is the user text with one trailing
/// semicolon removed. Values are never spliced into it.
#[derive(Debug)]
pub struct GuestStmt {
    pub sql: String,
    pub read: bool,
    pub returns_rows: bool,
}

fn strip_one_semi(sql: &str) -> String {
    let t = sql.trim();
    t.strip_suffix(';')
        .map(str::trim_end)
        .unwrap_or(t)
        .to_string()
}

fn forbidden_fn(parsed: &pg_query::ParseResult) -> bool {
    parsed.functions().iter().any(|name| {
        let n = name.to_ascii_lowercase();
        n == "set_config"
            || n.ends_with(".set_config")
            || n == "set_principal"
            || n.ends_with(".set_principal")
    })
}

fn select_has_into(sel: &pg_query::protobuf::SelectStmt) -> bool {
    sel.into_clause.is_some()
        || sel.larg.as_deref().is_some_and(select_has_into)
        || sel.rarg.as_deref().is_some_and(select_has_into)
}

/// One SELECT / INSERT / UPDATE / DELETE, or WITH that ends in one of those.
/// Anything else is an error and must not be executed.
pub fn classify_guest_sql(sql: &str) -> Result<GuestStmt, OpsError> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err(OpsError::Usage("unclassifiable SQL".into()));
    }
    let parsed = match pg_query::parse(trimmed) {
        Ok(p) => p,
        Err(_) => return Err(OpsError::Usage("unclassifiable SQL".into())),
    };
    if parsed.protobuf.stmts.is_empty() {
        return Err(OpsError::Usage("unclassifiable SQL".into()));
    }
    if parsed.protobuf.stmts.len() != 1 {
        return Err(OpsError::Usage("one statement only".into()));
    }
    if forbidden_fn(&parsed) {
        return Err(OpsError::Usage(
            "set_config and k2.set_principal are not allowed".into(),
        ));
    }
    let node = parsed
        .protobuf
        .stmts[0]
        .stmt
        .as_ref()
        .and_then(|n| n.node.as_ref())
        .ok_or_else(|| OpsError::Usage("unclassifiable SQL".into()))?;
    let ty = parsed
        .statement_types()
        .into_iter()
        .next()
        .unwrap_or("");
    let (read, returns_rows) = match node {
        pg_query::NodeEnum::SelectStmt(sel) => {
            if select_has_into(sel) {
                return Err(OpsError::Usage("SELECT INTO is not allowed".into()));
            }
            (true, true)
        }
        pg_query::NodeEnum::InsertStmt(st) => (false, !st.returning_list.is_empty()),
        pg_query::NodeEnum::UpdateStmt(st) => (false, !st.returning_list.is_empty()),
        pg_query::NodeEnum::DeleteStmt(st) => (false, !st.returning_list.is_empty()),
        pg_query::NodeEnum::VariableSetStmt(_) => {
            return Err(OpsError::Usage("SET is not allowed".into()));
        }
        pg_query::NodeEnum::CopyStmt(_) => {
            return Err(OpsError::Usage("COPY is not allowed".into()));
        }
        pg_query::NodeEnum::DoStmt(_) => {
            return Err(OpsError::Usage("DO is not allowed".into()));
        }
        pg_query::NodeEnum::CallStmt(_) => {
            return Err(OpsError::Usage("CALL is not allowed".into()));
        }
        pg_query::NodeEnum::GrantStmt(_) | pg_query::NodeEnum::GrantRoleStmt(_) => {
            return Err(OpsError::Usage("GRANT is not allowed".into()));
        }
        pg_query::NodeEnum::ListenStmt(_) => {
            return Err(OpsError::Usage("LISTEN is not allowed".into()));
        }
        pg_query::NodeEnum::TransactionStmt(_) => {
            return Err(OpsError::Usage("BEGIN is not allowed".into()));
        }
        _ if ty.starts_with("Create") || ty.starts_with("Define") => {
            return Err(OpsError::Usage("CREATE is not allowed".into()));
        }
        _ => return Err(OpsError::Usage("unclassifiable SQL".into())),
    };
    Ok(GuestStmt {
        sql: strip_one_semi(trimmed),
        read,
        returns_rows,
    })
}

/// Quote one JSON param for an EXECUTE argument list. The user SQL keeps `$1`.
pub fn quote_guest_param(v: &serde_json::Value) -> Result<String, OpsError> {
    match v {
        serde_json::Value::Null => Ok("NULL".into()),
        serde_json::Value::Bool(true) => Ok("'true'".into()),
        serde_json::Value::Bool(false) => Ok("'false'".into()),
        serde_json::Value::Number(n) => {
            let s = n.to_string();
            if s.is_empty()
                || !s.chars().all(|c| {
                    c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'
                })
            {
                return Err(OpsError::Usage(
                    "param number is not a SQL literal".into(),
                ));
            }
            Ok(s)
        }
        serde_json::Value::String(s) => Ok(format!("'{}'", s.replace('\'', "''"))),
        _ => Err(OpsError::Usage(
            "params must be string, number, boolean, or null".into(),
        )),
    }
}

pub fn quote_guest_params(params: &[serde_json::Value]) -> Result<String, OpsError> {
    let mut out = Vec::with_capacity(params.len());
    for p in params {
        out.push(quote_guest_param(p)?);
    }
    Ok(out.join(", "))
}

/// Fetch one past the cap. LIMIT is outside the user text.
pub fn rows_wrapper(user_sql: &str) -> String {
    format!(
        "SELECT to_json(k2_guest_src)::text FROM (\n{user_sql}\n) AS k2_guest_src LIMIT {}",
        GUEST_ROW_CAP + 1
    )
}

pub fn guest_rows_json(stdout: &str) -> Result<serde_json::Value, OpsError> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let t = line.trim();
        if t.is_empty() || !t.starts_with('{') {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(t)
            .map_err(|e| OpsError::Engine(format!("guest row is not json: {e}")))?;
        rows.push(v);
        if rows.len() > GUEST_ROW_CAP {
            return Err(OpsError::RowCap(
                "statement returned more than 5000 rows".into(),
            ));
        }
    }
    let columns: Vec<String> = rows
        .first()
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let n = rows.len();
    Ok(serde_json::json!({
        "columns": columns,
        "rows": rows,
        "rowCount": n,
    }))
}

fn parse_command_tag(tag: &str) -> Result<u64, OpsError> {
    let mut parts = tag.split_whitespace();
    let kind = parts.next().unwrap_or("").to_ascii_uppercase();
    match kind.as_str() {
        "INSERT" => {
            let mut last = 0u64;
            let mut saw = false;
            for p in parts {
                if let Ok(n) = p.parse::<u64>() {
                    last = n;
                    saw = true;
                }
            }
            if saw {
                Ok(last)
            } else {
                Err(OpsError::Engine(format!("bad command tag: {tag}")))
            }
        }
        "UPDATE" | "DELETE" => {
            let n = parts.next().unwrap_or("");
            n.parse::<u64>()
                .map_err(|_| OpsError::Engine(format!("bad command tag: {tag}")))
        }
        _ => Err(OpsError::Engine(format!("bad command tag: {tag}"))),
    }
}

/// Last non-empty line is the user command tag. Earlier `-c` tags
/// (the lock INSERT, SET, PREPARE) are not the row count.
pub fn command_row_count(stdout: &str) -> Result<u64, OpsError> {
    let Some(tag) = stdout.lines().rev().map(str::trim).find(|l| !l.is_empty()) else {
        return Ok(0);
    };
    parse_command_tag(tag)
}

pub fn guest_command_json(stdout: &str) -> Result<serde_json::Value, OpsError> {
    let n = command_row_count(stdout)?;
    Ok(serde_json::json!({
        "rows": [],
        "rowCount": n,
    }))
}

pub fn map_guest_engine(err: String) -> OpsError {
    let lower = err.to_ascii_lowercase();
    if lower.contains("canceling statement due to statement timeout")
        || lower.contains("sqlstate 57014")
    {
        OpsError::StatementTimeout("statement exceeded 10s".into())
    } else {
        OpsError::Engine(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reject(sql: &str) {
        let err = classify_guest_sql(sql).expect_err(sql);
        assert_eq!(err.code(), "usage", "{sql} {}", err.hint());
    }

    #[test]
    fn parser_rejects_second_statement_set_copy_create_grant_listen_set_config() {
        reject("SELECT 1; SELECT 2");
        reject("SET statement_timeout = '1s'");
        reject("COPY t TO STDOUT");
        reject("CREATE TABLE t (id int)");
        reject("GRANT SELECT ON t TO u");
        reject("LISTEN chan");
        reject("SELECT set_config('k2.skin_principal', 'x', true)");
        reject("SELECT k2.set_principal('00000000-0000-0000-0000-000000000001')");
        reject("DO $$ BEGIN NULL; END $$;");
        reject("BEGIN");
        reject("SELECT 1 INTO new_t");
        reject("CALL foo()");
        let err = classify_guest_sql("VACUUM").expect_err("vacuum");
        assert!(err.hint().contains("unclassifiable"), "{}", err.hint());
    }

    #[test]
    fn with_ending_select_is_read_and_insert_is_write() {
        let read = classify_guest_sql("WITH c AS (SELECT 1) SELECT * FROM c").expect("read");
        assert!(read.read);
        assert!(read.returns_rows);
        let write = classify_guest_sql("WITH c AS (SELECT 1 AS id) INSERT INTO t SELECT * FROM c")
            .expect("write");
        assert!(!write.read);
        assert!(!write.returns_rows);
        let ret = classify_guest_sql("INSERT INTO t (id) VALUES (1) RETURNING id").expect("ret");
        assert!(!ret.read);
        assert!(ret.returns_rows);
        let plain = classify_guest_sql("UPDATE t SET n = 1").expect("upd");
        assert!(!plain.returns_rows);
        assert!(!plain.sql.ends_with(';'));
    }

    #[test]
    fn quote_keeps_user_placeholders_out_of_the_literal() {
        assert_eq!(
            quote_guest_param(&serde_json::json!("O'Brien")).unwrap(),
            "'O''Brien'"
        );
        assert_eq!(quote_guest_param(&serde_json::json!(1.5)).unwrap(), "1.5");
        assert_eq!(quote_guest_param(&serde_json::json!(true)).unwrap(), "'true'");
        assert_eq!(quote_guest_param(&serde_json::json!(null)).unwrap(), "NULL");
        assert!(quote_guest_param(&serde_json::json!({"a": 1})).is_err());
        let stmt = classify_guest_sql("SELECT $1::text").unwrap();
        assert!(stmt.sql.contains("$1"));
        let wrapped = rows_wrapper(&stmt.sql);
        assert!(wrapped.contains("SELECT $1::text"));
        assert!(!wrapped.contains("SELECT $1::text LIMIT"));
        assert!(wrapped.contains("LIMIT 5001"));
    }

    #[test]
    fn row_cap_and_command_tag_and_timeout() {
        let mut lines = String::new();
        for _ in 0..GUEST_ROW_CAP {
            lines.push_str("{\"a\":1}\n");
        }
        let ok = guest_rows_json(&lines).expect("5000");
        assert_eq!(ok["rowCount"], GUEST_ROW_CAP);
        assert_eq!(ok["columns"][0], "a");
        lines.push_str("INSERT 0 1\n");
        lines.push_str("{\"a\":1}\n");
        let err = guest_rows_json(&lines).expect_err("5001");
        assert_eq!(err.code(), "row_cap");
        assert_eq!(err.status(), "409 Conflict");
        assert_eq!(err.hint(), "statement returned more than 5000 rows");

        let tag = "CREATE TABLE\nINSERT 0 1\nSET\nINSERT 0 4\n";
        assert_eq!(command_row_count(tag).unwrap(), 4);
        assert_eq!(command_row_count("").unwrap(), 0);
        assert_eq!(command_row_count("UPDATE 3").unwrap(), 3);
        assert_eq!(command_row_count("DELETE 0").unwrap(), 0);
        let body = guest_command_json("INSERT 0 2").unwrap();
        assert_eq!(body["rows"].as_array().unwrap().len(), 0);
        assert_eq!(body["rowCount"], 2);

        let timeout = map_guest_engine(
            "psql: ERROR:  canceling statement due to statement timeout".into(),
        );
        assert_eq!(timeout.code(), "statement_timeout");
        assert_eq!(timeout.hint(), "statement exceeded 10s");
        let state = map_guest_engine("ERROR: SQLSTATE 57014".into());
        assert_eq!(state.code(), "statement_timeout");
    }
}
