//! Real-Postgres smoke for the guest db doors (rows, query, store, the
//! `_k2_*` seal). `#[ignore]`: it needs a throwaway Postgres 16 cluster
//! that this test owns, never the box's own server:
//!
//!   REALPG_PORT  TCP port on 127.0.0.1 (scram for roles K2 creates)
//!   REALPG_SOCK  unix socket dir (trust, superuser `postgres`)
//!   REALPG_BIN   dir with that cluster's `psql`
//!
//! Run: `cargo test -p k2-daemon --lib real_pg -- --ignored --nocapture`
//! on Linux, with every K2_*/K2SO_* variable cleared. The route handlers
//! run unchanged; only `SystemOps` points at the throwaway cluster (the
//! helper's superuser psql over the socket, the daemon's psql over TCP).
//! Every check runs and prints PASS/FAIL; the test fails if any failed.

use super::*;
use crate::sql::sysops::{RealSystemOps, SystemOps};

struct ClusterOps {
    port: String,
    sock: String,
    psql: String,
}

fn need_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("{key} must be set for the real-Postgres smoke"))
}

impl SystemOps for ClusterOps {
    fn write_file(&self, path: &str, contents: &[u8], mode: u32) -> Result<(), String> {
        RealSystemOps.write_file(path, contents, mode)
    }
    fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        RealSystemOps.read_file(path)
    }
    fn create_dir_all(&self, path: &str) -> Result<(), String> {
        RealSystemOps.create_dir_all(path)
    }
    fn remove_path(&self, path: &str) -> Result<(), String> {
        RealSystemOps.remove_path(path)
    }
    fn path_exists(&self, path: &str) -> bool {
        RealSystemOps.path_exists(path)
    }
    fn systemctl(&self, args: &[&str]) -> Result<String, String> {
        Err(format!("systemctl {} is not part of the smoke", args.join(" ")))
    }
    fn systemctl_query(&self, _args: &[&str]) -> String {
        String::new()
    }
    /// The root helper's `psql -d <db> …`: superuser, SQL on stdin.
    fn run_helper(&self, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
        if args.first().copied() != Some("psql") {
            return Err(format!("helper {} is not part of the smoke", args.join(" ")));
        }
        let mut full: Vec<&str> = vec!["-X", "-h", &self.sock, "-p", &self.port, "-U", "postgres"];
        full.extend_from_slice(&args[1..]);
        let out = RealSystemOps.run_cmd(&self.psql, &full, &[], stdin)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }
    /// The daemon's own psql runs: same argv, the cluster's port.
    fn run_cmd(
        &self,
        cmd: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        let mut env: Vec<(&str, &str)> = env.to_vec();
        env.push(("PGPORT", &self.port));
        env.push(("PSQLRC", "/dev/null"));
        RealSystemOps.run_cmd(cmd, args, &env, stdin)
    }
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

fn body_json(r: &crate::cli_response::CliResponse) -> serde_json::Value {
    serde_json::from_str(&r.body).unwrap_or(serde_json::Value::Null)
}

fn no_leaks(body: &str, db: &str) -> Result<(), String> {
    for leak in [db, "_migrator", "k2_principal_lock", "SET ROLE", "psql", "statement_timeout"] {
        if body.contains(leak) {
            return Err(format!("{leak} leaked: {body}"));
        }
    }
    Ok(())
}

fn want(cond: bool, msg: String) -> Result<(), String> {
    if cond {
        Ok(())
    } else {
        Err(msg)
    }
}

#[test]
#[ignore = "needs a throwaway Postgres 16 cluster (REALPG_PORT/REALPG_SOCK/REALPG_BIN)"]
fn real_pg_guest_doors_smoke() {
    let cluster: &'static ClusterOps = Box::leak(Box::new(ClusterOps {
        port: need_env("REALPG_PORT"),
        sock: need_env("REALPG_SOCK"),
        psql: format!("{}/psql", need_env("REALPG_BIN")),
    }));
    let _home = crate::test_support::TempHome::new();
    let _g = sql_server_test_lock();
    k2_core::db::init_for_tests();
    seed_running_sidecar();

    let dir = std::env::temp_dir().join(format!("k2-realpg-{}", uuid::Uuid::new_v4()));
    let mig = dir.join(".k2/db/migrations");
    std::fs::create_dir_all(&mig).unwrap();
    std::fs::write(
        mig.join("0001_jobs.sql"),
        "CREATE TABLE jobs (\n\
           id uuid PRIMARY KEY DEFAULT gen_random_uuid(),\n\
           owner uuid NOT NULL,\n\
           title text,\n\
           state jsonb NOT NULL DEFAULT '{}'::jsonb\n\
         );\n\
         ALTER TABLE jobs ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY jobs_own ON jobs FOR ALL TO PUBLIC\n\
           USING (owner = k2.skin_uid()) WITH CHECK (owner = k2.skin_uid());\n\
         CREATE FUNCTION job_update(p_id uuid, patch jsonb) RETURNS uuid\n\
           LANGUAGE sql VOLATILE SECURITY INVOKER\n\
           AS $$ UPDATE jobs SET state = state || patch WHERE id = p_id RETURNING owner $$;\n\
         CREATE FUNCTION k2_leak() RETURNS bigint LANGUAGE sql STABLE SECURITY INVOKER\n\
           AS $$ SELECT count(*) FROM public._k2_migrations $$;\n\
         CREATE TABLE audit_log (\n\
           id bigserial PRIMARY KEY,\n\
           actor_kind text DEFAULT k2.actor_kind(),\n\
           actor text DEFAULT k2.actor(),\n\
           principal uuid DEFAULT k2.skin_uid(),\n\
           action text NOT NULL\n\
         );\n\
         ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY audit_append ON audit_log FOR INSERT TO PUBLIC\n\
           WITH CHECK (actor_kind IS NOT DISTINCT FROM k2.actor_kind()\n\
             AND actor IS NOT DISTINCT FROM k2.actor()\n\
             AND principal IS NOT DISTINCT FROM k2.skin_uid());\n\
         CREATE POLICY audit_read_own ON audit_log FOR SELECT TO PUBLIC\n\
           USING (principal = k2.skin_uid());\n\
         CREATE TABLE quotes (\n\
           id uuid PRIMARY KEY DEFAULT gen_random_uuid(),\n\
           owner uuid NOT NULL,\n\
           confirmed boolean NOT NULL DEFAULT false\n\
         );\n\
         ALTER TABLE quotes ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY quotes_read ON quotes FOR SELECT TO PUBLIC USING (owner = k2.skin_uid());\n\
         CREATE POLICY quotes_add ON quotes FOR INSERT TO PUBLIC WITH CHECK (owner = k2.skin_uid());\n\
         CREATE POLICY quotes_confirm_person ON quotes FOR UPDATE TO PUBLIC\n\
           USING (owner = k2.skin_uid() AND k2.actor_kind() = 'guest')\n\
           WITH CHECK (owner = k2.skin_uid() AND k2.actor_kind() = 'guest');\n",
    )
    .unwrap();
    let path = dir.to_string_lossy().into_owned();
    let handle = format!("rpg{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let pid = insert_project_handle("real-pg", &path, &handle);
    let secrets = crate::sql::secrets::FileSecretStore::default();
    let created = ops::create_database(cluster, &secrets, &pid, 1, None, None).expect("create db");
    let db = created["name"].as_str().expect("db name").to_string();
    ops::migrate(cluster, &secrets, &pid, &path, None).expect("migrate 0001");

    let mut policy = k2_core::skin::RoomPolicy::new();
    policy.insert(
        pid.clone(),
        vec![
            k2_core::skin::CAP_STORE_READ.into(),
            k2_core::skin::CAP_STORE_WRITE.into(),
        ],
    );
    let a = skin_session("alice", policy.clone());
    let policy_for_bot = policy.clone();
    let b = skin_session("bob", policy);
    let a_id = a.principal_id.clone().expect("a");
    let b_id = b.principal_id.clone().expect("b");
    // A guest login the owner marked as this workspace's agent.
    let bot_principal = k2_core::skin::add_principal("salesbot").expect("bot principal");
    k2_core::skin::set_principal_agent("salesbot", Some(&handle)).expect("owner mark");
    let mut bot = skin_session("salesbot", policy_for_bot.clone());
    bot.principal_id = Some(bot_principal.id.clone());
    let bot_id = bot_principal.id.clone();
    println!("[realpg] db={db} alice={a_id} bob={b_id}");

    let mut results: Vec<(&str, Result<(), String>)> = Vec::new();
    routes::with_test_ops(cluster, || {
        let q = |pass: &k2_core::skin::SkinPass, sql: &str, params: serde_json::Value| {
            routes::handle_db_query(&query_json(&handle, sql, Some(params)), Some(pass.clone()))
        };

        // Seed bob's row with a plain (unwrapped) write: works on both.
        let r = q(&b, "INSERT INTO jobs (owner, title) VALUES ($1::uuid, 'bob job')", serde_json::json!([b_id]));
        results.push(("seed: query-door plain INSERT as bob", want(r.status == "200 OK", r.body.clone())));

        let r = routes::handle_dump_insert_gated(
            serde_json::json!({"workspace": handle, "table": "jobs", "row": {"owner": a_id, "title": "alice job"}})
                .to_string()
                .as_bytes(),
            Some(a.clone()),
        );
        let v = body_json(&r);
        results.push((
            "rows insert (POST /cli/db/rows) as alice",
            want(r.status == "200 OK" && v["row"]["owner"] == a_id, r.body.clone()),
        ));

        let r = routes::handle_dump_rows_gated(&store_ws_params(&handle, &[("table", "jobs")]), Some(a.clone()));
        let v = body_json(&r);
        let rows = v["rows"].as_array().cloned().unwrap_or_default();
        results.push((
            "rows read (GET /cli/db/rows) as alice: only her rows, real objects",
            want(
                r.status == "200 OK" && !rows.is_empty() && rows.iter().all(|x| x["owner"] == a_id),
                r.body.clone(),
            ),
        ));

        let ids = q(&a, "SELECT id FROM jobs WHERE title = 'alice job'", serde_json::json!([]));
        let a_job = body_json(&ids)["rows"][0]["id"].as_str().unwrap_or("").to_string();
        let ids = q(&b, "SELECT id FROM jobs", serde_json::json!([]));
        let b_job = body_json(&ids)["rows"][0]["id"].as_str().unwrap_or("").to_string();
        println!("[realpg] alice job={a_job} bob job={b_job}");

        let r = routes::handle_dump_update_gated(
            serde_json::json!({"workspace": handle, "table": "jobs", "id": a_job, "row": {"title": "alice edited"}})
                .to_string()
                .as_bytes(),
            Some(a.clone()),
        );
        let v = body_json(&r);
        results.push((
            "rows update (POST /cli/db/rows/update) as alice",
            want(r.status == "200 OK" && v["row"]["title"] == "alice edited", r.body.clone()),
        ));

        let r = routes::handle_dump_insert_gated(
            serde_json::json!({"workspace": handle, "table": "jobs", "row": {"owner": b_id, "title": "forged"}})
                .to_string()
                .as_bytes(),
            Some(a.clone()),
        );
        let v = body_json(&r);
        results.push((
            "RLS refusal on rows door: PG message only",
            want(
                r.status == "400 Bad Request"
                    && v["error"]["hint"] == "new row violates row-level security policy for table \"jobs\"",
                r.body.clone(),
            )
            .and_then(|_| no_leaks(&r.body, &db)),
        ));
        let r = q(&a, "INSERT INTO jobs (owner, title) VALUES ($1::uuid, 'forged')", serde_json::json!([b_id]));
        let v = body_json(&r);
        results.push((
            "RLS refusal on query door: PG message only",
            want(
                v["error"]["hint"] == "new row violates row-level security policy for table \"jobs\"",
                r.body.clone(),
            )
            .and_then(|_| no_leaks(&r.body, &db)),
        ));
        let r = q(&a, "SELEC 1", serde_json::json!([]));
        results.push(("syntax error refused before psql", want(r.status == "400 Bad Request", r.body.clone())));
        let r = q(&a, "SELECT nope FROM jobs", serde_json::json!([]));
        let v = body_json(&r);
        results.push((
            "engine error on query door: PG message only",
            want(v["error"]["hint"] == "column \"nope\" does not exist", r.body.clone())
                .and_then(|_| no_leaks(&r.body, &db)),
        ));

        for (name, sql) in [
            ("guest SELECT _k2_store refused", "SELECT * FROM _k2_store"),
            ("guest SELECT _k2_migrations refused", "SELECT * FROM _k2_migrations"),
        ] {
            let r = q(&a, sql, serde_json::json!([]));
            results.push((name, want(r.status != "200 OK", r.body.clone())));
        }
        let r = q(&a, "SELECT k2_leak() AS n", serde_json::json!([]));
        let v = body_json(&r);
        results.push((
            "guest reaching _k2_migrations via a function refused (row security)",
            want(
                r.status != "200 OK" && v["error"]["hint"] == "permission denied for K2 internal table",
                r.body.clone(),
            ),
        ));

        let r = q(&a, "select job_update($1,$2::jsonb)", serde_json::json!([a_job, "{\"done\":true}"]));
        let v = body_json(&r);
        let own = r.status == "200 OK" && v["rows"][0]["job_update"] == a_id;
        let r2 = q(&a, "select job_update($1,$2::jsonb)", serde_json::json!([b_job, "{\"done\":true}"]));
        let v2 = body_json(&r2);
        let other = r2.status == "200 OK" && v2["rows"][0]["job_update"].is_null();
        let r3 = q(&a, "select k2.skin_uid() AS me", serde_json::json!([]));
        let me = body_json(&r3)["rows"][0]["me"] == a_id;
        results.push((
            "volatile SECURITY INVOKER fn via query door runs as locked principal",
            want(own && other && me, format!("{} | {} | {}", r.body, r2.body, r3.body)),
        ));

        let r = q(
            &a,
            "INSERT INTO jobs (owner, title) VALUES ($1::uuid, 'ret') RETURNING id, owner",
            serde_json::json!([a_id]),
        );
        let v = body_json(&r);
        results.push((
            "INSERT … RETURNING via query door",
            want(r.status == "200 OK" && v["rows"][0]["owner"] == a_id && v["rowCount"] == 1, r.body.clone()),
        ));
        let r = q(
            &a,
            "UPDATE jobs SET title = 'ret2' WHERE title = 'ret' RETURNING title",
            serde_json::json!([]),
        );
        let r2 = q(&a, "DELETE FROM jobs WHERE title = 'ret2' RETURNING id", serde_json::json!([]));
        results.push((
            "UPDATE/DELETE … RETURNING via query door",
            want(
                body_json(&r)["rows"][0]["title"] == "ret2" && body_json(&r2)["rowCount"] == 1,
                format!("{} | {}", r.body, r2.body),
            ),
        ));
        let r = q(
            &a,
            "INSERT INTO jobs (owner, title) SELECT $1::uuid, 'bulk' FROM generate_series(1, 5001) RETURNING id",
            serde_json::json!([a_id]),
        );
        let c = q(&a, "SELECT count(*) AS n FROM jobs WHERE title = 'bulk'", serde_json::json!([]));
        results.push((
            "RETURNING write over 5000 rows: row_cap and nothing written",
            want(
                r.status == "409 Conflict" && body_json(&c)["rows"][0]["n"] == 0,
                format!("{} | {}", r.body, c.body),
            ),
        ));

        // Actor stamp: derived from the pass, locked before SET ROLE.
        let r = q(
            &a,
            "SELECT k2.actor_kind() AS kind, k2.actor() AS actor, k2.actor_info() AS info",
            serde_json::json!([]),
        );
        let v = body_json(&r);
        results.push((
            "actor helpers on the query door: guest, guest:<principal>, pass id",
            want(
                v["rows"][0]["kind"] == "guest"
                    && v["rows"][0]["actor"] == format!("guest:{a_id}")
                    && v["rows"][0]["info"]["principal"] == a_id
                    && v["rows"][0]["info"]["pass"] == a.id
                    && v["rows"][0]["info"]["kind"] == "guest",
                r.body.clone(),
            )
            .and_then(|_| no_leaks(&r.body, &db)),
        ));
        let r = q(
            &a,
            "INSERT INTO audit_log (action) VALUES ('viewed quote') RETURNING actor_kind, actor, principal",
            serde_json::json!([]),
        );
        let v = body_json(&r);
        results.push((
            "audit row defaults come from the locked actor",
            want(
                r.status == "200 OK"
                    && v["rows"][0]["actor_kind"] == "guest"
                    && v["rows"][0]["actor"] == format!("guest:{a_id}")
                    && v["rows"][0]["principal"] == a_id,
                r.body.clone(),
            ),
        ));
        let r = routes::handle_dump_insert_gated(
            serde_json::json!({"workspace": handle, "table": "audit_log",
                "row": {"action": "forged", "actor_kind": "guest", "actor": "agent:sales"}})
            .to_string()
            .as_bytes(),
            Some(a.clone()),
        );
        results.push((
            "audit row with a forged actor refused (rows door, WITH CHECK)",
            want(r.status == "400 Bad Request", r.body.clone()),
        ));
        let r = q(&a, "UPDATE audit_log SET action = 'rewritten'", serde_json::json!([]));
        let r2 = q(&a, "DELETE FROM audit_log", serde_json::json!([]));
        results.push((
            "audit log is append-only for guests (no UPDATE/DELETE policy)",
            want(
                body_json(&r)["rowCount"] == 0 && body_json(&r2)["rowCount"] == 0,
                format!("{} | {}", r.body, r2.body),
            ),
        ));
        let r = q(
            &a,
            "INSERT INTO quotes (owner) VALUES ($1::uuid) RETURNING id",
            serde_json::json!([a_id]),
        );
        let quote = body_json(&r)["rows"][0]["id"].as_str().unwrap_or("").to_string();
        let r = q(
            &a,
            "UPDATE quotes SET confirmed = true WHERE id = $1::uuid",
            serde_json::json!([quote]),
        );
        results.push((
            "person-only confirm: the guest's own login session confirms",
            want(!quote.is_empty() && body_json(&r)["rowCount"] == 1, r.body.clone()),
        ));
        // An owner-marked agent login: kind agent, person-only confirm refuses.
        let r = q(
            &bot,
            "SELECT k2.actor_kind() AS kind, k2.actor() AS actor, k2.actor_info() AS info, k2.skin_uid() AS me",
            serde_json::json!([]),
        );
        let v = body_json(&r);
        results.push((
            "marked agent login: kind agent, agent:<handle>, workspace id, on_behalf_of null",
            want(
                v["rows"][0]["kind"] == "agent"
                    && v["rows"][0]["actor"] == format!("agent:{handle}")
                    && v["rows"][0]["info"]["workspace"] == pid
                    && v["rows"][0]["info"]["on_behalf_of"].is_null()
                    && v["rows"][0]["info"]["principal"] == bot_id
                    && v["rows"][0]["me"] == bot_id,
                r.body.clone(),
            ),
        ));
        let r = q(
            &bot,
            "INSERT INTO quotes (owner) VALUES ($1::uuid) RETURNING id",
            serde_json::json!([bot_id]),
        );
        let bot_quote = body_json(&r)["rows"][0]["id"].as_str().unwrap_or("").to_string();
        let r = q(
            &bot,
            "UPDATE quotes SET confirmed = true WHERE id = $1::uuid",
            serde_json::json!([bot_quote]),
        );
        results.push((
            "person-only confirm refuses the marked agent (0 rows)",
            want(!bot_quote.is_empty() && body_json(&r)["rowCount"] == 0, r.body.clone()),
        ));
        let r = q(
            &bot,
            "INSERT INTO audit_log (action) VALUES ('agent viewed') RETURNING actor_kind, actor",
            serde_json::json!([]),
        );
        results.push((
            "audit row from the marked agent says agent",
            want(
                body_json(&r)["rows"][0]["actor_kind"] == "agent"
                    && body_json(&r)["rows"][0]["actor"] == format!("agent:{handle}"),
                r.body.clone(),
            ),
        ));

        let r = q(
            &a,
            "SELECT set_config('k2.actor', 'agent:x', true)",
            serde_json::json!([]),
        );
        results.push((
            "guest cannot set an actor GUC (parser)",
            want(r.status == "400 Bad Request", r.body.clone()),
        ));
        let r = q(&a, "SELECT * FROM pg_temp.k2_actor_lock", serde_json::json!([]));
        results.push((
            "guest cannot touch the actor lock (parser)",
            want(r.status == "400 Bad Request", r.body.clone()),
        ));

        // Owner/agent store door, then the same under a bind role.
        let put = |id: &str| {
            routes::handle_store_put(
                serde_json::json!({"project": path, "name": "items", "id": id, "json": {"v": id}})
                    .to_string()
                    .as_bytes(),
            )
        };
        let get = |id: &str| {
            let mut p = std::collections::HashMap::new();
            p.insert("project".to_string(), path.clone());
            p.insert("name".to_string(), "items".to_string());
            p.insert("id".to_string(), id.to_string());
            routes::handle_store_get(&p)
        };
        let p1 = put("one");
        let g1 = get("one");
        results.push((
            "owner store put/get",
            want(
                p1.status == "200 OK" && body_json(&g1)["doc"]["v"] == "one",
                format!("{} | {}", p1.body, g1.body),
            ),
        ));
    });

    // Actor stamp outside the brokered lane. The superuser socket stands in
    // for an agent DSN by SET ROLE to the agent login (objects it creates
    // are owned by that role, as on a real agent connection).
    let agent = format!("{db}_agent");
    let migrator = format!("{db}_migrator");
    let su = |sql: &str| cluster.run_helper(&["psql", "-d", db.as_str(), "-q", "-tA", "-v", "ON_ERROR_STOP=1"], Some(sql.as_bytes()));
    let owner = su(
        "SELECT pg_get_userbyid(p.proowner) FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
         WHERE n.nspname = 'k2' AND p.proname = 'actor_info'",
    );
    results.push((
        "k2.actor_info() is owned by the migrator (SECURITY DEFINER, not superuser)",
        want(owner.as_deref() == Ok(migrator.as_str()), format!("{owner:?}")),
    ));
    let r = su(&format!(
        "BEGIN;\nSET LOCAL ROLE \"{agent}\";\nSELECT k2.set_principal('{a_id}');\n\
         SELECT k2.skin_uid()::text || '|' || coalesce(k2.actor_kind(), 'NULL') || '|' || coalesce(k2.actor_info()::text, 'NULL');\nCOMMIT;"
    ));
    results.push((
        "BFF set_principal lane: principal set, actor NULL",
        want(r.as_deref() == Ok(format!("{a_id}|NULL|NULL").as_str()), format!("{r:?}")),
    ));
    let r = su(&format!(
        "SET ROLE \"{agent}\";\n\
         SELECT coalesce(k2.actor_kind(), 'NULL');\n\
         SELECT count(*) FROM quotes;"
    ));
    results.push((
        "agent DSN lane: actor NULL",
        want(r.as_ref().map(|s| s.starts_with("NULL\n")).unwrap_or(false), format!("{r:?}")),
    ));
    // A DSN holder plants its own lock table: ignored (not migrator-owned),
    // so the person-only confirm still refuses. If the agent login has no
    // TEMP privilege the plant itself fails, which is also a refusal.
    let planted = su(&format!(
        "BEGIN;\nSET LOCAL ROLE \"{agent}\";\n\
         CREATE TEMP TABLE pg_temp.k2_actor_lock (kind text, actor text, principal uuid, pass text);\n\
         INSERT INTO pg_temp.k2_actor_lock VALUES ('guest', 'guest:{a_id}', '{a_id}', NULL);\n\
         SELECT k2.set_principal('{a_id}');\n\
         SELECT coalesce(k2.actor_kind(), 'NULL');\n\
         UPDATE quotes SET confirmed = false RETURNING id;\n\
         COMMIT;"
    ));
    results.push((
        "planted pg_temp actor lock from a DSN is ignored; person-only confirm refuses",
        match &planted {
            Ok(out) => want(out.trim() == "NULL", format!("{planted:?}")),
            Err(e) => want(
                e.contains("permission denied to create temporary tables"),
                format!("{planted:?}"),
            ),
        },
    ));
    // Two quotes exist: alice's (she confirmed it) and the marked agent's
    // (its confirm was refused). Neither the planted lock nor the agent
    // changed either one.
    let confirmed = su(&format!(
        "SELECT string_agg(CASE owner WHEN '{a_id}' THEN 'alice' WHEN '{bot_id}' THEN 'bot' ELSE 'other' END \
         || '=' || confirmed::text, ',' ORDER BY confirmed DESC) FROM quotes"
    ));
    results.push((
        "quotes: alice's stays confirmed, the marked agent's never was",
        want(confirmed.as_deref() == Ok("alice=true,bot=false"), format!("{confirmed:?}")),
    ));

    ops::bind_role(cluster, Some(&db), Some(&pid), "app_user").expect("bind app_user");
    std::fs::write(
        mig.join("0002_app_user.sql"),
        "GRANT SELECT, INSERT, UPDATE, DELETE ON jobs TO app_user;\n",
    )
    .unwrap();
    ops::migrate(cluster, &secrets, &pid, &path, None).expect("migrate 0002");
    routes::with_test_ops(cluster, || {
        let p2 = routes::handle_store_put(
            serde_json::json!({"project": path, "name": "items", "id": "two", "json": {"v": "two"}})
                .to_string()
                .as_bytes(),
        );
        let mut p = std::collections::HashMap::new();
        p.insert("project".to_string(), path.clone());
        p.insert("name".to_string(), "items".to_string());
        p.insert("id".to_string(), "two".to_string());
        let g2 = routes::handle_store_get(&p);
        results.push((
            "owner store put/get under a bind role",
            want(
                p2.status == "200 OK" && body_json(&g2)["doc"]["v"] == "two",
                format!("{} | {}", p2.body, g2.body),
            ),
        ));
        let r = routes::handle_dump_rows_gated(&store_ws_params(&handle, &[("table", "jobs")]), Some(a.clone()));
        let rows = body_json(&r)["rows"].as_array().cloned().unwrap_or_default();
        results.push((
            "rows read under bind role: RLS still applies",
            want(
                r.status == "200 OK" && !rows.is_empty() && rows.iter().all(|x| x["owner"] == a_id),
                r.body.clone(),
            ),
        ));
        let r = routes::handle_db_query(
            &query_json(&handle, "SELECT current_user AS u", Some(serde_json::json!([]))),
            Some(a.clone()),
        );
        results.push((
            "query door under bind role runs as the bind role",
            want(body_json(&r)["rows"][0]["u"] == "app_user", r.body.clone()),
        ));
    });

    let mut failed = 0;
    for (name, res) in &results {
        match res {
            Ok(()) => println!("[realpg] PASS {name}"),
            Err(e) => {
                failed += 1;
                println!("[realpg] FAIL {name}: {e}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(failed, 0, "{failed} real-Postgres checks failed (see [realpg] lines)");
}
