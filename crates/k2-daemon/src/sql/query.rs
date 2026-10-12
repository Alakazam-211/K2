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

/// K2's own tables (`_k2_migrations`, `_k2_store`) are sealed with row
/// security against guest sessions; refuse direct references up front so
/// the app gets a plain answer instead of an engine error.
fn touches_k2_internal(parsed: &pg_query::ParseResult) -> bool {
    parsed.tables().iter().any(|t| {
        t.rsplit('.')
            .next()
            .unwrap_or(t)
            .to_ascii_lowercase()
            .starts_with("_k2_")
    })
}

/// The per-connection principal and actor locks. Grants already keep a
/// guest from writing them; refuse any reference up front too, so the
/// only way to read who is calling is `k2.skin_uid()` / `k2.actor*()`.
fn touches_session_lock(parsed: &pg_query::ParseResult) -> bool {
    parsed.tables().iter().any(|t| {
        let name = t.rsplit('.').next().unwrap_or(t).to_ascii_lowercase();
        name == "k2_actor_lock" || name == "k2_principal_lock"
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
    if touches_k2_internal(&parsed) {
        return Err(OpsError::Usage(
            "K2's own tables (_k2_*) are not available to apps".into(),
        ));
    }
    if touches_session_lock(&parsed) {
        return Err(OpsError::Usage(
            "the session locks are K2's; read them with k2.skin_uid() and k2.actor_info()".into(),
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

/// Text a RETURNING write raises inside its own statement when it would
/// return more than [`GUEST_ROW_CAP`] rows, so the write rolls back.
pub const ROW_CAP_MARKER: &str = "k2 row cap";

/// INSERT / UPDATE / DELETE … RETURNING. Postgres refuses a data-modifying
/// statement as a FROM subquery, so it is a top-level data-modifying CTE.
/// A LIMIT would not stop the write (the CTE always runs to completion),
/// so the cap is checked inside the statement: over the cap the whole
/// statement errors and nothing is applied.
pub fn returning_write_wrapper(user_sql: &str) -> String {
    format!(
        "WITH k2_guest_src AS (\n{user_sql}\n), \
         k2_guest_n AS (SELECT count(*) AS n FROM k2_guest_src) \
         SELECT to_json(k2_guest_src)::text FROM k2_guest_src, k2_guest_n \
         WHERE CASE WHEN k2_guest_n.n > {cap} \
         THEN ('{ROW_CAP_MARKER} ' || k2_guest_n.n)::int = 0 ELSE true END",
        cap = GUEST_ROW_CAP,
    )
}

/// The SQL actually sent for a classified guest statement (before
/// PREPARE): rows come back as one JSON text line each.
pub fn guest_exec_sql(stmt: &GuestStmt) -> String {
    match (stmt.returns_rows, stmt.read) {
        (false, _) => stmt.sql.clone(),
        (true, true) => rows_wrapper(&stmt.sql),
        (true, false) => returning_write_wrapper(&stmt.sql),
    }
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

/// Map a failed guest psql run. The hint is [`guest_error_message`]; the
/// full text (argv, stderr) goes to the daemon log only.
pub fn map_guest_engine(err: String, internal: &[String]) -> OpsError {
    let lower = err.to_ascii_lowercase();
    if lower.contains("canceling statement due to statement timeout")
        || lower.contains("sqlstate 57014")
    {
        OpsError::StatementTimeout("statement exceeded 10s".into())
    } else if lower.contains(ROW_CAP_MARKER) {
        OpsError::RowCap(format!(
            "statement returned more than {GUEST_ROW_CAP} rows (nothing was written)"
        ))
    } else {
        OpsError::Engine(guest_error_logged(&err, internal))
    }
}

/// Hint shown to a guest when the message would name K2's own setup.
pub const GUEST_GENERIC_ERROR: &str = "database error (details are in the server log)";

/// Log the full failure for the owner, return what the guest may see.
pub fn guest_error_logged(raw: &str, internal: &[String]) -> String {
    eprintln!("[db/guest] statement failed: {raw}");
    guest_error_message(raw, internal)
}

/// What a guest (app session) sees when its psql run fails: only the
/// Postgres `ERROR:` message, which the app needs ("syntax error at or
/// near …", "permission denied for table jobs", "new row violates
/// row-level security policy …"). Never the psql command line, database
/// or role names, or K2's own setup statements; any message that names
/// one of those becomes [`GUEST_GENERIC_ERROR`]. `internal` lists the
/// database and role names for this run.
pub fn guest_error_message(raw: &str, internal: &[String]) -> String {
    // A failed run is `<cmd> <argv…>: exit <code>: <stderr>`. The argv
    // (lock statement, SET ROLE, user SQL) comes before the last marker.
    let stderr = match raw.rfind(": exit ") {
        Some(i) => &raw[i + ": exit ".len()..],
        None => raw,
    };
    let msg = stderr.lines().find_map(|l| {
        l.find("ERROR:")
            .map(|i| l[i + "ERROR:".len()..].trim().to_string())
    });
    let Some(msg) = msg.filter(|m| !m.is_empty()) else {
        return GUEST_GENERIC_ERROR.into();
    };
    let low = msg.to_ascii_lowercase();
    const SETUP: [&str; 7] = [
        "k2_principal_lock",
        "k2_guest_query",
        "pg_temp",
        "_migrator",
        "set role",
        "role \"",
        "statement_timeout",
    ];
    if SETUP.iter().any(|t| low.contains(t)) {
        return GUEST_GENERIC_ERROR.into();
    }
    for name in internal {
        let n = name.trim().to_ascii_lowercase();
        if n.is_empty() {
            continue;
        }
        if low.contains(&format!("\"{n}\"")) || (n.starts_with("ws_") && low.contains(&n)) {
            return GUEST_GENERIC_ERROR.into();
        }
    }
    msg
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
    fn parser_refuses_every_way_a_guest_could_change_the_actor() {
        for sql in [
            // GUC / role changes.
            "SELECT set_config('k2.actor', 'agent:x', false)",
            "SELECT pg_catalog.set_config('role', 'postgres', true)",
            "WITH s AS (SELECT set_config('k2.actor', 'x', true)) SELECT * FROM s",
            "SET k2.actor = 'agent:x'",
            "SET ROLE postgres",
            "RESET ROLE",
            "RESET ALL",
            "SET SESSION AUTHORIZATION postgres",
            // The lock tables themselves.
            "INSERT INTO pg_temp.k2_actor_lock (kind, actor, principal) VALUES ('guest', 'guest:x', gen_random_uuid())",
            "UPDATE pg_temp.k2_actor_lock SET kind = 'guest'",
            "UPDATE k2_actor_lock SET actor = 'guest:x'",
            "DELETE FROM pg_temp.k2_actor_lock",
            "WITH d AS (DELETE FROM pg_temp.K2_ACTOR_LOCK RETURNING 1) SELECT * FROM d",
            "SELECT * FROM pg_temp.k2_actor_lock",
            "SELECT id FROM pg_temp.k2_principal_lock",
            "UPDATE pg_temp.k2_principal_lock SET id = gen_random_uuid()",
            "TRUNCATE pg_temp.k2_actor_lock",
            "DROP TABLE pg_temp.k2_actor_lock",
            "ALTER TABLE pg_temp.k2_actor_lock RENAME TO x",
            "CREATE TEMP TABLE k2_actor_lock (kind text)",
            // Transaction games around the lock.
            "BEGIN",
            "ROLLBACK",
            "DISCARD TEMP",
            "DISCARD ALL",
        ] {
            let err = classify_guest_sql(sql).expect_err(sql);
            assert_eq!(err.code(), "usage", "{sql} {}", err.hint());
        }
        // Reading the actor through the helpers is a plain read.
        let ok = classify_guest_sql("SELECT k2.actor_kind(), k2.actor(), k2.actor_info()")
            .expect("helpers are readable");
        assert!(ok.read && ok.returns_rows);
        let err = classify_guest_sql("SELECT * FROM pg_temp.k2_actor_lock").expect_err("lock");
        assert!(err.hint().contains("k2.actor_info()"), "{}", err.hint());
    }

    #[test]
    fn parser_refuses_k2_internal_tables_and_allows_volatile_function_calls() {
        for sql in [
            "SELECT * FROM _k2_store",
            "SELECT * FROM public._k2_migrations",
            "SELECT * FROM jobs WHERE id IN (SELECT id FROM _K2_STORE)",
            "WITH s AS (SELECT * FROM _k2_store) SELECT * FROM s",
            "UPDATE _k2_store SET doc = '{}'",
            "DELETE FROM _k2_migrations",
            "INSERT INTO _k2_store (collection, id, doc) VALUES ('a', 'b', '{}')",
        ] {
            let err = classify_guest_sql(sql).expect_err(sql);
            assert_eq!(err.code(), "usage", "{sql}");
            assert!(err.hint().contains("_k2_"), "{sql} {}", err.hint());
        }
        // Dannon's RLS walls call SECURITY INVOKER functions through the
        // query door; that stays a read statement that returns rows.
        let call = classify_guest_sql("select job_update($1,$2::jsonb)").expect("function call");
        assert!(call.read);
        assert!(call.returns_rows);
        assert_eq!(call.sql, "select job_update($1,$2::jsonb)");
        let ok = classify_guest_sql("SELECT * FROM k2_jobs").expect("not internal");
        assert!(ok.read);
    }

    #[test]
    fn guest_error_keeps_the_postgres_message_and_drops_the_command_line() {
        let internal = vec![
            "ws_abc".to_string(),
            "ws_abc_migrator".to_string(),
            "ws_abc_agent".to_string(),
            "app_user".to_string(),
        ];
        let argv = "/usr/bin/psql -h 127.0.0.1 -U ws_abc_migrator -d ws_abc -v ON_ERROR_STOP=1 -A \
            -c CREATE TEMP TABLE pg_temp.k2_principal_lock (id uuid) ON COMMIT PRESERVE ROWS; \
            -c SET ROLE \"app_user\" -c SET statement_timeout = '10s' -c selec 1";
        let user_errors = [
            ("ERROR:  syntax error at or near \"selec\"\nLINE 1: selec 1\n        ^", "syntax error at or near \"selec\""),
            ("ERROR:  permission denied for table jobs", "permission denied for table jobs"),
            (
                "ERROR:  new row violates row-level security policy for table \"jobs\"",
                "new row violates row-level security policy for table \"jobs\"",
            ),
            ("psql:<stdin>:1: ERROR:  relation \"nope\" does not exist", "relation \"nope\" does not exist"),
        ];
        for (stderr, want) in user_errors {
            let raw = format!("{argv}: exit Some(1): {stderr}");
            let got = guest_error_message(&raw, &internal);
            assert_eq!(got, want, "{raw}");
            for leak in ["ws_abc", "_migrator", "k2_principal_lock", "SET ROLE", "-U ", "/usr/bin/psql"] {
                assert!(!got.contains(leak), "{leak} leaked: {got}");
            }
        }
        // Messages about K2's own setup (role names, the lock, the db) are
        // replaced, not echoed.
        for stderr in [
            "ERROR:  permission denied to set role \"app_user\"",
            "ERROR:  role \"ws_abc_agent\" does not exist",
            "ERROR:  relation \"pg_temp.k2_principal_lock\" already exists",
            "FATAL:  password authentication failed for user \"ws_abc_migrator\"",
            "ERROR:  permission denied for database ws_abc",
            "",
        ] {
            let raw = format!("{argv}: exit Some(2): {stderr}");
            assert_eq!(guest_error_message(&raw, &internal), GUEST_GENERIC_ERROR, "{raw}");
        }
        // The user's own SQL text in argv never counts as the message.
        let raw = format!("{argv} -c SELECT 'ERROR: ws_abc_migrator': exit Some(1): ERROR:  division by zero");
        assert_eq!(guest_error_message(&raw, &internal), "division by zero");
        let mapped = map_guest_engine(format!("{argv}: exit Some(1): ERROR:  division by zero"), &internal);
        assert_eq!(mapped.code(), "engine");
        assert_eq!(mapped.hint(), "division by zero");
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
    fn returning_writes_use_a_top_level_cte_and_reads_keep_the_subquery() {
        for sql in [
            "INSERT INTO jobs (id) VALUES ($1) RETURNING id",
            "UPDATE jobs SET n = n + 1 WHERE id = $1 RETURNING *",
            "DELETE FROM jobs WHERE id = $1 RETURNING id",
            "WITH c AS (SELECT 1 AS id) INSERT INTO jobs SELECT * FROM c RETURNING id",
        ] {
            let stmt = classify_guest_sql(sql).expect(sql);
            assert!(!stmt.read && stmt.returns_rows, "{sql}");
            let out = guest_exec_sql(&stmt);
            assert!(out.starts_with("WITH k2_guest_src AS (\n"), "{out}");
            assert!(out.contains(&format!("\n{sql}\n)")), "user text kept as-is: {out}");
            assert!(!out.contains("FROM (\n"), "never a DML subquery: {out}");
            assert!(!out.contains("LIMIT"), "a LIMIT would not stop the write: {out}");
            assert!(out.contains("k2_guest_n.n > 5000"), "{out}");
            assert!(out.contains(ROW_CAP_MARKER), "{out}");
        }
        let read = classify_guest_sql("select job_update($1,$2::jsonb)").expect("call");
        let out = guest_exec_sql(&read);
        assert_eq!(out, rows_wrapper("select job_update($1,$2::jsonb)"));
        assert!(out.contains("LIMIT 5001"), "{out}");
        let plain = classify_guest_sql("UPDATE jobs SET n = 1").expect("plain");
        assert_eq!(guest_exec_sql(&plain), "UPDATE jobs SET n = 1", "plain writes are not wrapped");

        let cap = map_guest_engine(
            "psql …: exit Some(1): ERROR:  invalid input syntax for type integer: \"k2 row cap 5001\""
                .into(),
            &[],
        );
        assert_eq!(cap.code(), "row_cap");
        assert_eq!(cap.status(), "409 Conflict");
        assert!(cap.hint().contains("nothing was written"), "{}", cap.hint());
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
            &[],
        );
        assert_eq!(timeout.code(), "statement_timeout");
        assert_eq!(timeout.hint(), "statement exceeded 10s");
        let state = map_guest_engine("ERROR: SQLSTATE 57014".into(), &[]);
        assert_eq!(state.code(), "statement_timeout");
    }
}
