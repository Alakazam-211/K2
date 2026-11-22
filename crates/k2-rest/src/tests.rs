//! The grammar end to end. Unit tests use a hand-made schema cache; the
//! `real_pg_*` tests (`#[ignore]`, REALPG_COMPAT=1, feature `testkit`) load
//! `fixture.sql` into a throwaway Postgres, read the cache with
//! [`introspect_sql`], and run each plan as the doors do: one transaction,
//! the role and claims set locally, then [`respond`].

use serde_json::{json, Value};

use super::*;

fn cache() -> SchemaCache {
    let col = |n: &str, t: &str| json!({"name": n, "type": t, "nullable": true, "default": null});
    SchemaCache::from_json(&json!({
        "tables": [
            {"schema": "public", "name": "orgs", "kind": "r", "columns": [col("id", "integer"), col("name", "text")], "pk": ["id"]},
            {"schema": "public", "name": "profiles", "kind": "r", "columns": [col("id", "uuid"), col("org_id", "integer"), col("name", "text")], "pk": ["id"]},
            {"schema": "public", "name": "projects", "kind": "r",
             "columns": [col("id", "integer"), col("org_id", "integer"), col("owner_id", "uuid"), col("reviewer_id", "uuid"), col("title", "text")], "pk": ["id"]},
            {"schema": "public", "name": "tasks", "kind": "r", "columns": [col("id", "integer"), col("project_id", "integer"), col("title", "text")], "pk": ["id"]}
        ],
        "fks": [
            {"name": "profiles_org_id_fkey", "schema": "public", "table": "profiles", "columns": ["org_id"], "ref_schema": "public", "ref_table": "orgs", "ref_columns": ["id"]},
            {"name": "projects_org_id_fkey", "schema": "public", "table": "projects", "columns": ["org_id"], "ref_schema": "public", "ref_table": "orgs", "ref_columns": ["id"]},
            {"name": "projects_owner_id_fkey", "schema": "public", "table": "projects", "columns": ["owner_id"], "ref_schema": "public", "ref_table": "profiles", "ref_columns": ["id"]},
            {"name": "projects_reviewer_id_fkey", "schema": "public", "table": "projects", "columns": ["reviewer_id"], "ref_schema": "public", "ref_table": "profiles", "ref_columns": ["id"]},
            {"name": "tasks_project_id_fkey", "schema": "public", "table": "tasks", "columns": ["project_id"], "ref_schema": "public", "ref_table": "projects", "ref_columns": ["id"]}
        ],
        "uniques": [],
        "functions": [
            {"schema": "public", "name": "add", "args": [{"name": "a", "type": "integer", "mode": "i", "has_default": false}, {"name": "b", "type": "integer", "mode": "i", "has_default": true}],
             "returns_set": false, "ret_type": "integer", "ret_kind": "b", "ret_rel": null, "volatility": "i"}
        ]
    }))
    .unwrap()
}

fn opts() -> Options {
    Options { schemas: vec!["public".into()], max_rows: Some(1000) }
}

fn req(method: &str, path: &str, q: &[(&str, &str)], headers: &[(&str, &str)], body: &str) -> Request {
    Request {
        method: method.into(),
        path: path.into(),
        query: q.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
        headers: headers.iter().map(|(a, b)| (a.to_ascii_lowercase(), b.to_string())).collect(),
        body: body.as_bytes().to_vec(),
    }
}

fn sql_of(p: &Plan) -> String {
    p.stmts[0].sql().to_string()
}

#[test]
fn a_read_is_one_quoted_statement() {
    let p = plan(&req("GET", "/projects", &[("select", "id,t:title,owner:profiles!owner_id(name),tasks(*)"), ("title", "eq.it's"), ("order", "id.desc.nullslast"), ("limit", "5")], &[], ""), &cache(), &opts()).unwrap();
    let s = sql_of(&p);
    assert!(p.read_only);
    assert!(s.contains(r#""public"."projects"."title" = 'it''s'"#), "{s}");
    assert!(s.contains(r#""public"."projects"."title" AS "t""#), "{s}");
    assert!(s.contains(r#"ORDER BY "public"."projects"."id" DESC NULLS LAST"#), "{s}");
    assert!(s.contains("LIMIT 5"), "{s}");
    assert!(s.contains(r#"row_to_json"#) && s.contains(r#"AS "owner""#), "{s}");
    assert!(s.contains(r#"json_agg"#) && s.contains(r#"AS "tasks""#), "{s}");
    // max_rows caps a bigger limit, and no limit at all.
    let p = plan(&req("GET", "/projects", &[("limit", "5000")], &[], ""), &cache(), &opts()).unwrap();
    assert!(sql_of(&p).contains("LIMIT 1000"));
    let p = plan(&req("GET", "/projects", &[], &[], ""), &cache(), &opts()).unwrap();
    assert!(sql_of(&p).contains("LIMIT 1000"));
}

#[test]
fn refusals_name_the_part() {
    let c = cache();
    let e = |r: Request| plan(&r, &c, &opts()).unwrap_err();
    assert_eq!(e(req("GET", "/nope", &[], &[], "")).code, "42P01");
    assert_eq!(e(req("GET", "/nope", &[], &[], "")).status, 404);
    assert_eq!(e(req("GET", "/", &[], &[], "")).code, "K2REST001");
    // Two keys from projects to profiles: ambiguous without a hint.
    let a = e(req("GET", "/projects", &[("select", "*,profiles(*)")], &[], ""));
    assert_eq!((a.status, a.code.as_str()), (300, "PGRST201"));
    assert!(a.details.as_ref().unwrap().as_array().unwrap().len() == 2);
    assert_eq!(e(req("GET", "/projects", &[("select", "*,nothing(*)")], &[], "")).code, "PGRST200");
    assert_eq!(e(req("GET", "/projects", &[], &[("accept-profile", "private")], "")).code, "PGRST106");
    assert_eq!(e(req("GET", "/projects", &[], &[("accept", "text/csv")], "")).code, "K2REST001");
    assert_eq!(e(req("DELETE", "/projects", &[], &[], "")).code, "21000", "no unfiltered delete");
    assert_eq!(e(req("PATCH", "/projects", &[], &[], r#"{"title":"x"}"#)).code, "21000");
    assert_eq!(e(req("PATCH", "/projects", &[("id", "eq.1")], &[], r#"{"nope":"x"}"#)).code, "PGRST204");
    assert_eq!(e(req("POST", "/projects", &[], &[], r#"[{"title":"a"},{"org_id":1}]"#)).code, "PGRST102");
    assert_eq!(e(req("POST", "/projects", &[], &[], r#"{"title":"#)).code, "PGRST102");
    assert_eq!(e(req("PUT", "/projects", &[("id", "eq.1")], &[], "{}")).code, "K2REST001");
    assert_eq!(e(req("POST", "/rpc/add", &[], &[], r#"{"z":1}"#)).code, "PGRST202");
    assert_eq!(e(req("POST", "/rpc/add", &[], &[], r#"{"b":1}"#)).code, "PGRST202", "a has no default");
    assert_eq!(e(req("GET", "/projects", &[("title", "eq.a\0b")], &[], "")).code, "22021");
}

#[test]
fn rpc_args_are_typed_by_the_catalog() {
    let p = plan(&req("POST", "/rpc/add", &[], &[], r#"{"a": 2}"#), &cache(), &opts()).unwrap();
    let s = sql_of(&p);
    assert!(s.contains(r#"json_to_record('{"a":2}'::json) AS _("a" integer)"#), "{s}");
    assert!(s.contains(r#""public"."add"("a" := pgrst_args."a")"#), "{s}");
    assert_eq!(p.kind, Kind::RpcScalar);
    let p = plan(&req("GET", "/rpc/add", &[("a", "2"), ("b", "3")], &[], ""), &cache(), &opts()).unwrap();
    assert!(p.read_only);
    assert!(sql_of(&p).contains(r#"{"a":"2","b":"3"}"#), "GET arguments arrive as text and the catalog types them");
}

// ── real Postgres ────────────────────────────────────────────────────

#[cfg(feature = "testkit")]
pub(crate) mod real {
    use super::*;
    use crate::testkit;
    use tokio_postgres::types::{FromSql, Type};

    /// Any text-like column as a string (json's wire form is its text;
    /// jsonb's has a version byte first).
    struct AnyText(String);
    impl<'a> FromSql<'a> for AnyText {
        fn from_sql(ty: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
            let raw = if *ty == Type::JSONB { &raw[1..] } else { raw };
            Ok(AnyText(String::from_utf8(raw.to_vec())?))
        }
        fn accepts(ty: &Type) -> bool {
            matches!(*ty, Type::TEXT | Type::VARCHAR | Type::NAME | Type::JSON | Type::JSONB | Type::UNKNOWN)
        }
    }

    pub struct Db {
        pub client: tokio_postgres::Client,
        pub cache: SchemaCache,
        pub name: String,
    }

    pub async fn db(tag: &str) -> Db {
        assert_eq!(std::env::var("REALPG_COMPAT").as_deref(), Ok("1"), "REALPG_COMPAT=1 marks the throwaway job");
        let s = testkit::shared();
        let name = s.fresh_db(tag);
        s.su(&name, &[include_str!("fixture.sql")]);
        let (client, conn) = tokio_postgres::connect(&s.conninfo(&name, &format!("{name}_authenticator"), testkit::AUTHENTICATOR_PASSWORD), tokio_postgres::NoTls)
            .await
            .expect("connect as the authenticator");
        tokio::spawn(conn);
        let row = client.query_one(introspect_sql(&["public".to_string()]).as_str(), &[]).await.expect("introspect");
        let v: Value = serde_json::from_str(&row.get::<_, AnyText>(0).0).unwrap();
        let cache = SchemaCache::from_json(&v).expect("cache");
        Db { client, cache, name }
    }

    pub struct Answer {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Value,
        pub code: Option<String>,
    }

    impl Answer {
        pub fn header(&self, n: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
        }
    }

    /// Run `r` as `role` with `claims`, as the door does.
    pub async fn exec(db: &mut Db, role: &str, claims: Value, r: Request) -> Answer {
        let err = |e: ApiError| Answer { status: e.status, headers: vec![], body: e.body(), code: Some(e.code.clone()) };
        let p = match plan(&r, &db.cache, &opts()) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
        let tx = db.client.build_transaction().read_only(p.read_only).start().await.unwrap();
        tx.execute(
            "SELECT set_config('role', $1, true), set_config('request.jwt.claims', $2, true), set_config('search_path', '\"public\"', true)",
            &[&role, &claims.to_string()],
        )
        .await
        .unwrap();
        let mut outs = Vec::new();
        for s in &p.stmts {
            let got = match s {
                Stmt::Rows(sql) => tx.query(sql.as_str(), &[]).await.map(|rows| {
                    Out::Rows(rows.iter().map(|row| (0..row.len()).map(|i| row.get::<_, Option<AnyText>>(i).map(|t| t.0)).collect()).collect())
                }),
                Stmt::Exec(sql) => tx.execute(sql.as_str(), &[]).await.map(Out::Affected),
            };
            match got {
                Ok(o) => outs.push(o),
                Err(e) => {
                    let d = e.as_db_error().unwrap_or_else(|| panic!("not a Postgres error: {e}\n{}", s.sql()));
                    let code = d.code().code().to_string();
                    let status = pg_status(&code, role != "anon");
                    return Answer { status, headers: vec![], body: json!({"code": code, "message": d.message(), "details": d.detail(), "hint": d.hint()}), code: Some(code) };
                }
            }
        }
        match respond(&p, &outs) {
            Ok(resp) => {
                tx.commit().await.unwrap();
                let body = if resp.body.is_empty() { Value::Null } else { serde_json::from_slice(&resp.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&resp.body))) };
                Answer { status: resp.status, headers: resp.headers, body, code: None }
            }
            Err(e) => {
                tx.rollback().await.unwrap();
                err(e)
            }
        }
    }

    pub fn get(path: &str, q: &[(&str, &str)]) -> Request {
        req("GET", path, q, &[], "")
    }

    pub fn svc() -> Value {
        json!({"role": "service_role"})
    }

    fn ids(a: &Answer) -> Vec<i64> {
        a.body.as_array().unwrap_or_else(|| panic!("not an array: {}", a.body)).iter().map(|r| r["id"].as_i64().unwrap()).collect()
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real Postgres: REALPG_COMPAT=1"]
    async fn real_pg_filters() {
        let mut d = db("rest_filters").await;
        let cases: Vec<(Vec<(&str, &str)>, Vec<i64>)> = vec![
            (vec![("id", "eq.3")], vec![3]),
            (vec![("id", "neq.3")], vec![1, 2, 4, 5]),
            (vec![("id", "gt.3")], vec![4, 5]),
            (vec![("id", "gte.3")], vec![3, 4, 5]),
            (vec![("id", "lt.3")], vec![1, 2]),
            (vec![("id", "lte.2")], vec![1, 2]),
            (vec![("id", "in.(1,4,9)")], vec![1, 4]),
            (vec![("id", "in.()")], vec![]),
            (vec![("id", "not.in.(1,4)")], vec![2, 3, 5]),
            (vec![("title", "in.(\"Beta, the second\",Gamma)")], vec![2, 3]),
            (vec![("due", "is.null")], vec![2, 4]),
            (vec![("due", "not.is.null")], vec![1, 3, 5]),
            (vec![("title", "like.*a")], vec![1, 3]),
            (vec![("title", "ilike.*A*")], vec![1, 2, 3, 4]),
            (vec![("title", "ilike.epsilon*")], vec![5]),
            (vec![("title", "match.^[A-G]")], vec![1, 2, 3, 5]),
            (vec![("tags", "cs.{red}")], vec![1, 5]),
            (vec![("tags", "cd.{red,blue}")], vec![1, 2, 3, 5]),
            (vec![("data", "cs.{\"kind\":\"web\"}")], vec![1, 4]),
            (vec![("data->>kind", "eq.web")], vec![1, 4]),
            (vec![("data->score", "gt.4")], vec![1, 5]),
            (vec![("or", "(status.eq.closed,id.eq.5)")], vec![2, 5]),
            (vec![("not.or", "(status.eq.open,status.eq.closed)")], vec![5]),
            (vec![("status", "eq.open"), ("org_id", "eq.1")], vec![1, 4]),
            (vec![("budget", "gte.99.99")], vec![1, 5]),
        ];
        let mut fails = Vec::new();
        for (q, want) in cases {
            let mut qq = q.clone();
            qq.push(("order", "id"));
            let a = exec(&mut d, "service_role", svc(), get("/projects", &qq)).await;
            if a.status != 200 || ids(&a) != want {
                fails.push(format!("{q:?}: {} {} want {want:?}", a.status, a.body));
            }
        }
        assert!(fails.is_empty(), "{}", fails.join("\n"));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real Postgres: REALPG_COMPAT=1"]
    async fn real_pg_select_embeds_order_and_ranges() {
        let mut d = db("rest_embeds").await;
        // Columns, aliases, casts, JSON paths.
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id,name:title,idt:id::text,kind:data->>kind,data->score"), ("id", "eq.1")])).await;
        assert_eq!(a.status, 200, "{}", a.body);
        assert_eq!(a.body, json!([{"id": 1, "name": "Alpha", "idt": "1", "kind": "web", "score": 5}]));
        // A to-one embed through a hinted key, a to-many embed with its own
        // filter, order and limit, and a one-to-one.
        let a = exec(
            &mut d,
            "service_role",
            svc(),
            get(
                "/projects",
                &[
                    ("select", "id,owner:profiles!owner_id(name,org:orgs(name)),tasks(title,done),project_settings(theme)"),
                    ("tasks.done", "eq.false"),
                    ("tasks.order", "pos.asc.nullslast"),
                    ("tasks.limit", "5"),
                    ("id", "in.(1,4)"),
                    ("order", "id"),
                ],
            ),
        )
        .await;
        assert_eq!(a.status, 200, "{}", a.body);
        assert_eq!(
            a.body,
            json!([
                {"id": 1, "owner": {"name": "Ann", "org": {"name": "Acme"}}, "tasks": [{"title": "a2", "done": false}, {"title": "a3", "done": false}], "project_settings": {"theme": "dark"}},
                {"id": 4, "owner": null, "tasks": [], "project_settings": null}
            ])
        );
        // !inner keeps only parents with a matching child; a filter-only
        // embed shows nothing.
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id,tasks!inner()"), ("tasks.done", "eq.true"), ("order", "id")])).await;
        assert_eq!(a.body, json!([{"id": 1}, {"id": 2}]), "{}", a.body);
        // An embed that is null / not null.
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id,project_settings()"), ("project_settings", "is.null"), ("order", "id")])).await;
        assert_eq!(ids(&a), vec![2, 4, 5]);
        // The column name as the embed (a many-to-one by its key column).
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id,reviewer_id(name)"), ("id", "eq.1")])).await;
        assert_eq!(a.body, json!([{"id": 1, "reviewer_id": {"name": "Bo"}}]), "{}", a.body);
        // One-to-many from the parent side.
        let a = exec(&mut d, "service_role", svc(), get("/orgs", &[("select", "name,projects(id)"), ("projects.order", "id.desc"), ("order", "id")])).await;
        assert_eq!(a.body[0], json!({"name": "Acme", "projects": [{"id": 4}, {"id": 2}, {"id": 1}]}));
        assert_eq!(a.body[2], json!({"name": "Empty Co", "projects": []}));
        // Order with nulls, limit, offset and the Range header.
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id"), ("order", "due.desc.nullslast,id")])).await;
        assert_eq!(ids(&a), vec![3, 1, 5, 2, 4]);
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id"), ("order", "due.asc.nullsfirst,id")])).await;
        assert_eq!(ids(&a), vec![2, 4, 5, 1, 3]);
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("select", "id"), ("order", "id"), ("limit", "2"), ("offset", "1")])).await;
        assert_eq!((a.status, ids(&a), a.header("Content-Range")), (200, vec![2, 3], Some("1-2/*")));
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("select", "id"), ("order", "id")], &[("Range", "3-")], "")).await;
        assert_eq!(ids(&a), vec![4, 5]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real Postgres: REALPG_COMPAT=1"]
    async fn real_pg_counts_singular_and_head() {
        let mut d = db("rest_counts").await;
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("select", "id"), ("order", "id"), ("limit", "2")], &[("Prefer", "count=exact")], "")).await;
        assert_eq!((a.status, a.header("Content-Range")), (206, Some("0-1/5")));
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("select", "id"), ("status", "eq.open")], &[("Prefer", "count=exact")], "")).await;
        assert_eq!((a.status, a.header("Content-Range")), (200, Some("0-2/3")));
        let a = exec(&mut d, "service_role", svc(), req("HEAD", "/projects", &[("status", "eq.nothing")], &[("Prefer", "count=exact")], "")).await;
        assert_eq!((a.status, a.header("Content-Range"), a.body.clone()), (200, Some("*/0"), Value::Null));
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("select", "id")], &[("Prefer", "count=planned")], "")).await;
        assert!(a.header("Content-Range").is_some_and(|r| !r.ends_with("/*")), "{:?}", a.headers);
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("select", "id")], &[("Prefer", "count=estimated")], "")).await;
        assert_eq!(a.header("Content-Range"), Some("0-4/5"), "small tables count exactly");
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("offset", "9")], &[("Prefer", "count=exact")], "")).await;
        assert_eq!((a.status, a.code.as_deref()), (416, Some("PGRST103")));
        // Singular.
        let obj = [("Accept", "application/vnd.pgrst.object+json")];
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("select", "id,title"), ("id", "eq.2")], &obj, "")).await;
        assert_eq!((a.status, a.body.clone()), (200, json!({"id": 2, "title": "Beta, the second"})));
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("status", "eq.open")], &obj, "")).await;
        assert_eq!((a.status, a.code.as_deref()), (406, Some("PGRST116")));
        assert_eq!(a.body["details"], "The result contains 3 rows");
        let a = exec(&mut d, "service_role", svc(), req("GET", "/projects", &[("id", "eq.99")], &obj, "")).await;
        assert_eq!(a.body["details"], "The result contains 0 rows");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real Postgres: REALPG_COMPAT=1"]
    async fn real_pg_writes() {
        let mut d = db("rest_writes").await;
        let rep = [("Prefer", "return=representation")];
        // Insert one, return it shaped.
        let a = exec(&mut d, "authenticated", json!({"role": "authenticated"}), req("POST", "/projects", &[("select", "id,title,status,org:orgs(name)")], &rep, r#"{"org_id": 2, "title": "Zeta"}"#)).await;
        assert_eq!((a.status, a.body.clone()), (201, json!([{"id": 6, "title": "Zeta", "status": "open", "org": {"name": "Globex"}}])), "{}", a.body);
        // Bulk insert with `columns` (missing keys are NULL), minimal.
        let a = exec(&mut d, "service_role", svc(), req("POST", "/tasks", &[("columns", "\"project_id\",\"title\",\"pos\"")], &[], r#"[{"project_id": 6, "title": "z1", "pos": 1}, {"project_id": 6, "title": "z2"}]"#)).await;
        assert_eq!((a.status, a.body.clone(), a.header("Content-Range")), (201, Value::Null, Some("*/*")));
        let a = exec(&mut d, "service_role", svc(), get("/tasks", &[("project_id", "eq.6"), ("select", "title,pos,done"), ("order", "id")])).await;
        assert_eq!(a.body, json!([{"title": "z1", "pos": 1, "done": false}, {"title": "z2", "pos": null, "done": false}]));
        // missing=default fills a key an object left out with its default.
        let a = exec(
            &mut d,
            "service_role",
            svc(),
            req("POST", "/projects", &[("columns", "\"id\",\"org_id\",\"title\",\"status\""), ("select", "id,status")], &[("Prefer", "return=representation,missing=default")], r#"[{"id": 20, "org_id": 1, "title": "m1"}, {"id": 21, "org_id": 1, "title": "m2", "status": "closed"}]"#),
        )
        .await;
        assert_eq!(a.body, json!([{"id": 20, "status": "open"}, {"id": 21, "status": "closed"}]), "{}", a.body);
        // A unique violation is 409 23505 (the phone's offline queue).
        let a = exec(&mut d, "service_role", svc(), req("POST", "/orgs", &[], &[], r#"{"id": 1, "name": "dup"}"#)).await;
        assert_eq!((a.status, a.code.as_deref()), (409, Some("23505")));
        // Upsert: merge, then ignore.
        let a = exec(&mut d, "service_role", svc(), req("POST", "/orgs", &[], &[("Prefer", "return=representation,resolution=merge-duplicates")], r#"[{"id": 1, "name": "Acme 2"}, {"id": 9, "name": "New"}]"#)).await;
        assert_eq!((a.status, a.body.clone()), (201, json!([{"id": 1, "name": "Acme 2"}, {"id": 9, "name": "New"}])), "{}", a.body);
        let a = exec(&mut d, "service_role", svc(), req("POST", "/orgs", &[], &[("Prefer", "return=representation,resolution=ignore-duplicates")], r#"[{"id": 1, "name": "ignored"}, {"id": 10, "name": "Ten"}]"#)).await;
        assert_eq!(a.body, json!([{"id": 10, "name": "Ten"}]));
        let a = exec(&mut d, "service_role", svc(), req("POST", "/devices", &[("on_conflict", "owner_id,label")], &[("Prefer", "return=representation,resolution=merge-duplicates")], r#"{"id": "aaaaaaaa-0000-4000-8000-000000000009", "owner_id": "11111111-1111-4111-8111-111111111111", "label": "phone"}"#)).await;
        assert_eq!(a.status, 200, "every row merged into an existing one: {}", a.body);
        assert_eq!(a.body[0]["id"], "aaaaaaaa-0000-4000-8000-000000000009");
        // Update with filters, representation; count.
        let a = exec(&mut d, "service_role", svc(), req("PATCH", "/projects", &[("status", "eq.open"), ("org_id", "eq.1"), ("select", "id,status")], &[("Prefer", "return=representation,count=exact")], r#"{"status": "review"}"#)).await;
        assert_eq!((a.status, a.header("Content-Range")), (200, Some("0-2/3")), "{}", a.body);
        // Singular write that matches several rows: 406 and nothing changed.
        let a = exec(&mut d, "service_role", svc(), req("PATCH", "/projects", &[("status", "eq.review")], &[("Prefer", "return=representation"), ("Accept", "application/vnd.pgrst.object+json")], r#"{"status": "gone"}"#)).await;
        assert_eq!((a.status, a.code.as_deref()), (406, Some("PGRST116")));
        let a = exec(&mut d, "service_role", svc(), get("/projects", &[("status", "eq.gone")])).await;
        assert_eq!(a.body, json!([]), "rolled back");
        // Delete, minimal and with representation.
        let a = exec(&mut d, "service_role", svc(), req("DELETE", "/tasks", &[("project_id", "eq.6")], &[], "")).await;
        assert_eq!(a.status, 204);
        let a = exec(&mut d, "service_role", svc(), req("DELETE", "/orgs", &[("id", "in.(9,10)"), ("select", "id")], &rep, "")).await;
        assert_eq!((a.status, a.body.clone()), (200, json!([{"id": 9}, {"id": 10}])));
        // A read-only GET can't write through a function.
        let a = exec(&mut d, "service_role", svc(), get("/rpc/close_project", &[("p_id", "1")])).await;
        assert_eq!((a.status, a.code.as_deref()), (405, Some("25006")));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "real Postgres: REALPG_COMPAT=1"]
    async fn real_pg_rpc() {
        let mut d = db("rest_rpc").await;
        let a = exec(&mut d, "anon", json!({"role": "anon"}), req("POST", "/rpc/add", &[], &[], r#"{"a": 2, "b": 3}"#)).await;
        assert_eq!((a.status, a.body.clone()), (200, json!(5)));
        let a = exec(&mut d, "anon", json!({"role": "anon"}), get("/rpc/add", &[("a", "2")])).await;
        assert_eq!(a.body, json!(12), "b's default");
        let a = exec(&mut d, "anon", json!({"role": "anon"}), get("/rpc/org_projects", &[("p_org", "1"), ("select", "id,title"), ("status", "eq.open"), ("order", "id.desc")])).await;
        assert_eq!(a.body, json!([{"id": 4, "title": "delta lowercase"}, {"id": 1, "title": "Alpha"}]), "{}", a.body);
        let a = exec(&mut d, "anon", json!({"role": "anon"}), req("POST", "/rpc/org_projects", &[("select", "id,owner:profiles!owner_id(name)"), ("limit", "1")], &[], r#"{"p_org": 2}"#)).await;
        assert_eq!(a.body, json!([{"id": 3, "owner": {"name": "Cy"}}]), "{}", a.body);
        let a = exec(&mut d, "anon", json!({"role": "anon"}), req("POST", "/rpc/first_project", &[("select", "id")], &[], "")).await;
        assert_eq!(a.body, json!({"id": 1}), "one row is an object");
        let a = exec(&mut d, "anon", json!({"role": "anon"}), req("POST", "/rpc/project_titles", &[("pid", "lt.3")], &[], "{}")).await;
        assert_eq!(a.body, json!([{"pid": 1, "title": "Alpha"}, {"pid": 2, "title": "Beta, the second"}]), "{}", a.body);
        let a = exec(&mut d, "anon", json!({"role": "anon"}), get("/rpc/tag_list", &[])).await;
        assert_eq!(a.body, json!(["blue", "green", "red"]));
        let a = exec(&mut d, "anon", json!({"role": "anon"}), req("POST", "/rpc/echo_json", &[], &[], r#"{"x": [1, 2]}"#)).await;
        assert_eq!(a.body, json!({"x": [1, 2]}));
        let a = exec(&mut d, "authenticated", json!({"role": "authenticated"}), req("POST", "/rpc/close_project", &[], &[], r#"{"p_id": 1}"#)).await;
        assert_eq!(a.status, 204);
        let a = exec(&mut d, "anon", json!({"role": "anon"}), req("POST", "/rpc/fail_teapot", &[], &[], "{}")).await;
        assert_eq!((a.status, a.code.as_deref(), a.body["details"].clone()), (418, Some("PT418"), json!("empty pot")));
    }
}
