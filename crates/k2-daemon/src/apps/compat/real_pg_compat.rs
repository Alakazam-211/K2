//! Lane A against a real Postgres: `k2 app compat on`, `auth.users`, the
//! claims on every lane, and the policy drill's SQL pass
//! (prd-app-supabase-compat-v1 §12 identity tests, §13.1 lane A done-when).
//! `#[ignore]`: runs only with REALPG_COMPAT=1 and Postgres binaries
//! (`REALPG_BIN`), on a compute node, never the M5.
//!
//! The drill sets a role and claims by hand, as the doors will, and every
//! reference-style policy must answer as on Supabase: as `authenticated`
//! (a web session's claims and a phone's third-party claims under both
//! `sub` settings), as `anon` and as `service_role`. Then the walls: a
//! grantee, a reader, the App's own login and the door login can't read
//! the claims key or `auth.users`, can't become a user by setting
//! `request.jwt.claims` or planting a lock; `k2.set_claims` takes only a
//! token K2 signed, for this database, in time, and refuses while the key
//! is readable by anyone else.
//!
//! Run: `REALPG_COMPAT=1 cargo test -p k2-daemon --lib real_pg_compat -- --ignored --nocapture --test-threads=1`

use std::collections::HashMap;

use k2_core::deploy::store;
use serde_json::{json, Value as J};

use crate::compute_routes::Who;
use crate::sql::secrets::{FileSecretStore, SecretStore as _};
use crate::test_pg::{check, Server};

const PORT: u16 = 5463;
const APP: &str = "crm";

fn call(srv: &'static Server, path: &str, body: J) -> Result<J, String> {
    crate::sql::routes::with_test_ops(srv, || super::routes::handle(path, &Who::Owner, body.to_string().as_bytes(), &HashMap::new()))
        .map_err(|r| format!("{} {}: {}", r.status, r.code, r.hint))
}

/// One door-style transaction as `login`: the role and claims set by hand
/// (transaction-local, as lane C does), `sql`, then ROLLBACK.
fn door(srv: &Server, login: &str, db: &str, role: &str, claims: &J, sql: &str) -> Result<Vec<String>, String> {
    let c = claims.to_string().replace('\'', "''");
    let sub = claims.get("sub").and_then(J::as_str).unwrap_or("").replace('\'', "''");
    srv.script(
        login,
        db,
        &format!(
            "BEGIN;\nSELECT 1 FROM (SELECT set_config('role', '{role}', true), set_config('request.jwt.claims', '{c}', true), set_config('request.jwt.claim.sub', '{sub}', true)) s;\n{sql}\nROLLBACK;\n"
        ),
    )
    .map(|mut o| {
        // the first line is the preamble's 1
        if o.first().map(String::as_str) == Some("1") {
            o.remove(0);
        }
        o
    })
}

fn rows(o: &Result<Vec<String>, String>) -> Vec<String> {
    o.clone().unwrap_or_else(|e| vec![format!("ERR {e}")])
}

#[test]
#[ignore = "runs only with REALPG_COMPAT=1 and Postgres binaries (a Linux compute job)"]
fn real_pg_compat_identity_and_policy_drill() {
    assert_eq!(std::env::var("REALPG_COMPAT").as_deref(), Ok("1"), "REALPG_COMPAT=1 marks the throwaway job");
    let (srv, _stop) = Server::start(PORT, "rpc");
    let major = srv.major();
    let _home = crate::test_support::TempHome::new();
    let _g = crate::sql::sql_server_test_lock();
    k2_core::db::init_for_tests();
    crate::vault::ensure_test_vault();
    let mut fails = Vec::new();

    // ── K2's records: a running SQL server, a workspace with an App ──
    let ws = format!("ws-rpc-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let other_ws = format!("{ws}-r");
    let dir = std::env::temp_dir().join(format!("k2-rpc-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join(".k2/db/migrations")).unwrap();
    let path = dir.to_string_lossy().into_owned();
    let service_id = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for t in ["sql_grants", "sql_server", "sql_clusters", "sql_databases", "app_compat", "app_identity_issuers"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', ?1, 'localhost', 1)", [major as i64]).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![ws, path]).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![other_ws, format!("{path}-r")]).unwrap();
        let svc = k2_core::published_services::insert(
            &conn,
            &ws,
            APP,
            k2_core::published_services::CMD_APP_SENTINEL,
            "/tmp/k2-rpc-app",
            0,
            k2_core::published_services::EXPOSE_LOCAL,
            k2_core::published_services::DESIRED_STOPPED,
            k2_core::published_services::KIND_CMD,
            "",
        )
        .unwrap();
        store::insert_app(
            &conn,
            &store::NewApp { app_id: APP, service_id: &svc.id, seed_id: None, owner_workspace: &ws, isolation: "container", memory_limit_bytes: None, front_port: 0, created_by: "owner" },
        )
        .unwrap();
        svc.id
    };
    let secrets = FileSecretStore::default();
    let created = crate::sql::ops::create_database(srv, &secrets, &ws, 1, None, None).expect("create db");
    let db = created["name"].as_str().expect("name").to_string();
    eprintln!("db={db} major={major}");

    // CONNECT is walled at create (vs-live §3 item 6): a login with no grant can't connect.
    srv.su("postgres", "CREATE ROLE outsider LOGIN;");
    let out = srv.script("outsider", &db, "SELECT 1;");
    check(concat!(k2_core::cli_name!(), " db create: a login with no grant can't connect"), out.as_ref().err().is_some_and(|e| e.contains("permission denied for database")), &out, &mut fails);

    // The App's users, in the store.
    let alice = k2_core::skin::add_principal_for_app("rpc-alice", Some("alice@example.com"), &service_id, None, "owner").expect("alice");
    let bob = k2_core::skin::add_principal_for_app("rpc-bob", Some("bob@example.com"), &service_id, None, "owner").expect("bob");
    let boss = k2_core::skin::add_principal_for_app("rpc-boss", Some("boss@example.com"), &service_id, None, "owner").expect("boss");
    k2_core::skin::compat::set_metadata(&alice.id, Some(&json!({"tier": "gold"})), Some(&json!({"theme": "dark"}))).unwrap();

    // A service_role someone made before K2, with a member, and itself a
    // member of something: compat on must strip both before BYPASSRLS.
    srv.su("postgres", "CREATE ROLE service_role NOLOGIN; CREATE ROLE planted LOGIN; GRANT service_role TO planted; GRANT pg_read_all_data TO service_role;");

    // ── k2 app compat on ──
    let on = call(srv, super::routes::ON, json!({ "app": APP, "db": db }));
    check(
        "compat on strips a pre-existing service_role's memberships and says so",
        on.as_ref().is_ok_and(|v| v["rolesStripped"] == "planted>service_role, service_role>pg_read_all_data"),
        &on,
        &mut fails,
    );
    let o = srv.script("planted", "postgres", "SET ROLE service_role;");
    check("a planted member of service_role can't SET ROLE to it after compat on", o.as_ref().err().is_some_and(|e| e.contains("permission denied")), &o, &mut fails);
    let o = srv.su("postgres", "SELECT count(*) FROM pg_auth_members WHERE member = 'service_role'::regrole;");
    check("service_role is a member of nothing", o == vec!["0"], &o, &mut fails);
    check(concat!(k2_core::cli_name!(), " app compat on answers"), on.as_ref().is_ok_and(|v| v["ok"] == true), &on, &mut fails);
    let on = on.unwrap_or(J::Null);
    let authn = on["compat"]["authenticator"].as_str().unwrap_or("").to_string();
    check("the door login is <db>_authenticator", authn == format!("{db}_authenticator"), &authn, &mut fails);
    check("auth.users written at turn-on (3 users)", on["users"]["inserted"] == 3, &on["users"], &mut fails);
    let roles = srv.su(
        "postgres",
        "SELECT rolname || ':' || rolcanlogin || ':' || rolbypassrls || ':' || rolsuper FROM pg_roles WHERE rolname IN ('anon','authenticated','service_role') ORDER BY 1;",
    );
    check(
        "the standard roles: NOLOGIN; only service_role bypasses RLS",
        roles == vec!["anon:false:false:false", "authenticated:false:false:false", "service_role:false:true:false"],
        &roles,
        &mut fails,
    );
    let a = srv.su("postgres", &format!("SELECT rolcanlogin || ':' || rolinherit || ':' || rolbypassrls FROM pg_roles WHERE rolname = '{authn}';"));
    check("the door login: LOGIN, NOINHERIT, no bypass", a == vec!["true:false:false"], &a, &mut fails);
    let m = srv.su("postgres", &format!("SELECT string_agg(pg_get_userbyid(roleid), ',' ORDER BY 1) FROM pg_auth_members WHERE member = '{authn}'::regrole;"));
    check("the door login is a member of exactly the three roles", m == vec!["anon,authenticated,service_role"], &m, &mut fails);
    let pub_c = srv.su("postgres", &format!("SELECT has_database_privilege('outsider', '{db}', 'CONNECT');"));
    check("CONNECT stays off PUBLIC", pub_c == vec!["f"], &pub_c, &mut fails);
    let users = srv.su(&db, "SELECT id || '|' || email || '|' || (raw_app_meta_data->>'tier') FROM auth.users WHERE email = 'alice@example.com';");
    check("auth.users keeps the store's ids and metadata", users == vec![format!("{}|alice@example.com|gold", alice.id)], &users, &mut fails);
    let again = call(srv, super::routes::ON, json!({ "app": APP, "db": db }));
    check("turning it on again is a no-op (same login, nothing to write)", again.as_ref().is_ok_and(|v| v["users"]["inserted"] == 0 && v["users"]["updated"] == 0), &again, &mut fails);

    // ── the App's schema: reference-style policies ──
    std::fs::write(
        dir.join(".k2/db/migrations/0001_app.sql"),
        "CREATE TABLE notes (id serial PRIMARY KEY, user_id uuid NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE, body text NOT NULL, tier text);\n\
         ALTER TABLE notes ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY notes_select ON notes FOR SELECT TO authenticated USING (auth.uid() = user_id);\n\
         CREATE POLICY notes_insert ON notes FOR INSERT TO authenticated WITH CHECK (auth.uid() = user_id);\n\
         CREATE POLICY notes_update ON notes FOR UPDATE TO authenticated USING (auth.uid() = user_id) WITH CHECK (auth.uid() = user_id);\n\
         CREATE POLICY notes_delete ON notes FOR DELETE TO authenticated USING ((select auth.uid()) = user_id);\n\
         CREATE POLICY notes_boss ON notes FOR SELECT TO authenticated USING (auth.email() = 'boss@example.com');\n\
         CREATE POLICY notes_gold ON notes FOR SELECT TO authenticated USING ((auth.jwt() -> 'app_metadata' ->> 'tier') = 'gold' AND tier = 'gold');\n\
         CREATE TABLE posts (id serial PRIMARY KEY, body text NOT NULL);\n\
         ALTER TABLE posts ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY posts_read ON posts FOR SELECT TO anon, authenticated USING (true);\n\
         CREATE POLICY posts_write ON posts FOR INSERT TO authenticated WITH CHECK (auth.role() = 'authenticated');\n\
         CREATE TABLE diary (id serial PRIMARY KEY, user_id uuid NOT NULL REFERENCES auth.users(id) ON DELETE CASCADE, body text NOT NULL);\n\
         ALTER TABLE diary ENABLE ROW LEVEL SECURITY;\n\
         CREATE POLICY diary_own ON diary FOR SELECT TO PUBLIC USING (auth.uid() = user_id);\n\
         CREATE FUNCTION public.user_email(uid uuid) RETURNS text LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public AS $$ SELECT email FROM auth.users WHERE id = uid $$;\n\
         GRANT SELECT, INSERT, UPDATE, DELETE ON notes TO authenticated;\n\
         GRANT USAGE, SELECT ON SEQUENCE notes_id_seq, posts_id_seq TO authenticated;\n\
         GRANT SELECT ON posts TO anon;\n\
         GRANT SELECT, INSERT ON posts TO authenticated;\n\
         GRANT ALL ON notes, posts, diary TO service_role;\n\
         GRANT EXECUTE ON FUNCTION public.user_email(uuid) TO authenticated;\n",
    )
    .unwrap();
    let mig = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::migrate(srv, &secrets, &ws, &path, None));
    check(concat!(k2_core::cli_name!(), " db migrate takes the policies (GRANT … TO authenticated works)"), mig.is_ok(), format!("{mig:?}"), &mut fails);
    srv.su(
        &db,
        &format!(
            "INSERT INTO notes (user_id, body, tier) VALUES ('{a}', 'alice note', 'gold'), ('{b}', 'bob note', NULL), ('{b}', 'bob gold', 'gold');\n\
             INSERT INTO posts (body) VALUES ('hello');\n\
             INSERT INTO diary (user_id, body) VALUES ('{a}', 'alice diary'), ('{b}', 'bob diary');",
            a = alice.id,
            b = bob.id
        ),
    );

    // ── the drill's SQL pass: role and claims by hand, every identity ──
    let web_alice = super::claims::for_user(APP, &k2_core::skin::compat::app_user(&service_id, &alice.id).unwrap().unwrap(), Some("sess-a"));
    let web_bob = super::claims::for_user(APP, &k2_core::skin::compat::app_user(&service_id, &bob.id).unwrap().unwrap(), Some("sess-b"));
    let web_boss = super::claims::for_user(APP, &k2_core::skin::compat::app_user(&service_id, &boss.id).unwrap().unwrap(), Some("sess-c"));
    let o = door(srv, &authn, &db, "authenticated", &web_alice, "SELECT auth.uid()::text || '|' || auth.role() || '|' || auth.email() || '|' || (auth.jwt() ->> 'aud') || '|' || (auth.jwt() -> 'app_metadata' ->> 'k2_app') || '|' || current_user || '|' || current_setting('request.jwt.claim.sub');");
    check(
        "web: auth.uid/role/email/jwt answer as documented",
        rows(&o) == vec![format!("{a}|authenticated|alice@example.com|authenticated|{APP}|authenticated|{a}", a = alice.id)],
        &o,
        &mut fails,
    );
    let o = door(srv, &authn, &db, "authenticated", &web_alice, "SELECT body FROM notes ORDER BY id;");
    check("web alice: her rows, and gold-tier rows (her app_metadata says gold)", rows(&o) == vec!["alice note", "bob gold"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_bob, "SELECT body FROM notes ORDER BY id;");
    check("web bob: his rows only (no gold metadata)", rows(&o) == vec!["bob note", "bob gold"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_boss, "SELECT count(*) FROM notes;");
    check("web boss: the email policy shows every note", rows(&o) == vec!["3"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, &format!("INSERT INTO notes (user_id, body) VALUES ('{}', 'mine') RETURNING body;", alice.id));
    check("web alice: insert her own row", rows(&o) == vec!["mine"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, &format!("INSERT INTO notes (user_id, body) VALUES ('{}', 'forged');", bob.id));
    check("web alice: insert as bob is refused by the policy", rows(&o).join(" ").contains("row-level security"), &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, &format!("WITH u AS (UPDATE notes SET body = 'x' WHERE user_id = '{}' RETURNING 1) SELECT count(*) FROM u;", bob.id));
    check("web alice: update bob's rows touches none", rows(&o) == vec!["0"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, "WITH d AS (DELETE FROM notes RETURNING 1) SELECT count(*) FROM d;");
    check("web alice: delete all removes only hers", rows(&o) == vec!["1"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, &format!("SELECT public.user_email('{}');", bob.id));
    check("a SECURITY DEFINER helper reads auth.users", rows(&o) == vec!["bob@example.com"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, "SELECT count(*) FROM auth.users;");
    check("authenticated can't read auth.users directly", rows(&o).join(" ").contains("permission denied"), &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &web_alice, "INSERT INTO posts (body) VALUES ('p') RETURNING body;");
    check("auth.role() = 'authenticated' policy lets the insert through", rows(&o) == vec!["p"], &o, &mut fails);

    // Phone (third-party) claims: setting (a) keeps the issuer's id in sub.
    let phone_a = json!({ "sub": "fb-alice", "role": "authenticated", "email": "alice@example.com", "iss": "https://securetoken.example/p", "aud": "p",
                          "app_metadata": { "k2_app": APP, "tier": "gold" }, "user_metadata": {}, "iat": 1, "exp": 2 });
    let o = door(srv, &authn, &db, "authenticated", &phone_a, "SELECT auth.uid();");
    check("phone (sub token): auth.uid() raises on a non-uuid sub, as Supabase's cast does", rows(&o).join(" ").contains("invalid input syntax for type uuid"), &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &phone_a, "SELECT auth.email() || '|' || auth.role();");
    check("phone (sub token): auth.email() and auth.role() answer", rows(&o) == vec!["alice@example.com|authenticated"], &o, &mut fails);
    let phone_boss = json!({ "sub": "fb-boss", "role": "authenticated", "email": "boss@example.com", "app_metadata": { "k2_app": APP } });
    let o = door(srv, &authn, &db, "authenticated", &phone_boss, "SELECT count(*) FROM posts;");
    check("phone (sub token): a policy that doesn't call auth.uid() answers", rows(&o) == vec!["1"], &o, &mut fails);
    let mut phone_b = phone_a.clone();
    phone_b["sub"] = json!(alice.id);
    phone_b["firebase_uid"] = json!("fb-alice");
    let o = door(srv, &authn, &db, "authenticated", &phone_b, "SELECT body FROM notes ORDER BY id;");
    check("phone (sub user): auth.uid() is the App user's id, her rows", rows(&o) == vec!["alice note", "bob gold"], &o, &mut fails);

    // anon and service.
    let anon = super::claims::anon(APP);
    let o = door(srv, &authn, &db, "anon", &anon, "SELECT coalesce(auth.uid()::text, 'null') || '|' || auth.role();");
    check("anon: no uid, role anon", rows(&o) == vec!["null|anon"], &o, &mut fails);
    let o = door(srv, &authn, &db, "anon", &anon, "SELECT body FROM posts;");
    check("anon: reads what TO anon allows", rows(&o) == vec!["hello"], &o, &mut fails);
    let o = door(srv, &authn, &db, "anon", &anon, "SELECT count(*) FROM notes;");
    check("anon: refused where there is no grant", rows(&o).join(" ").contains("permission denied"), &o, &mut fails);
    let service = super::claims::service(APP);
    let o = door(srv, &authn, &db, "service_role", &service, "SELECT count(*) FROM notes;");
    check("service_role: bypasses row security", rows(&o) == vec!["3"], &o, &mut fails);
    let o = door(srv, &authn, &db, "authenticated", &json!({}), "SELECT count(*) FROM notes;");
    check("authenticated with no claims: no rows", rows(&o) == vec!["0"], &o, &mut fails);

    // ── the walls ──
    let grantee = crate::sql::ops::default_agent_role(&other_ws);
    let g = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::grant_access(srv, &secrets, None, &db, &other_ws, "read", false, &[]));
    check("a read grantee for another workspace", g.is_ok(), format!("{g:?}"), &mut fails);
    let agent = format!("{db}_agent");
    srv.su("postgres", &crate::sql::ops::app_roles_sql(&db, &format!("{db}_app_crm"), &format!("{db}_app_crm__g1"), "pw", &agent));
    let app_login = format!("{db}_app_crm__g1");
    // CONNECT on a new database: every K2 way in holds its own grant (PUBLIC
    // has none), and each one logs in. The brokered lane logs in as the
    // migrator (checked by its own checks below).
    let writer_ws = format!("{ws}-w");
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)", rusqlite::params![writer_ws, format!("{path}-w")]).unwrap();
    }
    let w = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::grant_access(srv, &secrets, None, &db, &writer_ws, "write", false, &[]));
    check("a write grantee (another workspace's agent)", w.is_ok(), format!("{w:?}"), &mut fails);
    let writer = crate::sql::ops::default_agent_role(&writer_ws);
    let acl = srv.su("postgres", &format!("SELECT coalesce(datacl::text, '-') FROM pg_database WHERE datname = '{db}';"));
    check("PUBLIC holds no CONNECT on a new database", acl.len() == 1 && !acl[0].contains("{=c") && !acl[0].contains(",=c") && acl[0] != "-", &acl, &mut fails);
    let migrator = format!("{db}_migrator");
    for (what, role) in [
        ("the migrator (the owner; the brokered lane's login)", migrator.as_str()),
        ("the workspace agent", agent.as_str()),
        ("the App's own login (through its group)", app_login.as_str()),
        (concat!("a read grantee (", k2_core::cli_name!(), " db grant / request-read)"), grantee.as_str()),
        ("a write grantee", writer.as_str()),
        ("the door login", authn.as_str()),
    ] {
        let explicit = srv.su(
            "postgres",
            &format!(
                "SELECT count(*) FROM pg_database d, aclexplode(d.datacl) a WHERE d.datname = '{db}' AND a.privilege_type = 'CONNECT' \
                 AND a.grantee <> 0 AND pg_has_role('{role}', a.grantee, 'USAGE');"
            ),
        );
        let login = srv.script(role, &db, "SELECT 1;");
        check(
            &format!("CONNECT on a new database: {what} holds its own grant and logs in"),
            explicit.first().is_some_and(|n| n != "0") && rows(&login) == vec!["1"],
            (&explicit, &login),
            &mut fails,
        );
    }
    let key = secrets.resolve(&super::door_config::load(APP).unwrap().unwrap().claims_key_ref).unwrap().unwrap();
    for who in [grantee.as_str(), app_login.as_str(), agent.as_str(), authn.as_str()] {
        let o = srv.script(who, &db, "SELECT count(*) FROM k2.claims_key;");
        check(&format!("{who} can't read the claims key"), o.as_ref().err().is_some_and(|e| e.contains("permission denied")), &o, &mut fails);
        let o = srv.script(who, &db, "SELECT count(*) FROM auth.users;");
        check(&format!("{who} can't read auth.users"), o.as_ref().err().is_some_and(|e| e.contains("permission denied")), &o, &mut fails);
        let o = srv.script(who, &db, &format!("SET request.jwt.claims = '{}';\nSELECT coalesce(auth.uid()::text, 'null');", web_alice.to_string().replace('\'', "''")));
        let expect = if who == authn { alice.id.clone() } else { "null".into() };
        check(&format!("{who}: setting request.jwt.claims gives auth.uid() = {expect}"), rows(&o) == vec![expect.clone()], &o, &mut fails);
        let o = srv.script(
            who,
            &db,
            &format!(
                "CREATE TEMP TABLE k2_claims_lock (claims jsonb);\nINSERT INTO k2_claims_lock VALUES ('{{\"sub\":\"{}\"}}');\nGRANT SELECT ON k2_claims_lock TO \"{db}_migrator\";\nSELECT coalesce(auth.uid()::text, 'null');",
                alice.id
            ),
        );
        check(&format!("{who}: a planted claims lock is ignored"), rows(&o) == vec!["null"], &o, &mut fails);
    }
    let o = door(srv, &authn, &db, "service_role", &service, "SELECT count(*) FROM k2.claims_key;");
    check("service_role (BYPASSRLS) still can't read the claims key", rows(&o).join(" ").contains("permission denied"), &o, &mut fails);
    for (what, q) in [
        ("a function's source", format!("SELECT count(*) FROM pg_proc WHERE prosrc LIKE '%{key}%';")),
        ("a setting", format!("SELECT count(*) FROM pg_settings WHERE setting LIKE '%{key}%';")),
        ("a database or role setting", format!("SELECT count(*) FROM pg_db_role_setting WHERE array_to_string(setconfig, ',') LIKE '%{key}%';")),
    ] {
        let o = srv.su(&db, &q);
        check(&format!("the claims key is in no {what}"), o == vec!["0"], &o, &mut fails);
    }
    srv.su(&db, "ANALYZE k2.claims_key;");
    let o = srv.script(&grantee, &db, "SELECT count(*) FROM pg_stats WHERE schemaname = 'k2';");
    check("the grantee sees no statistics of the key table", rows(&o) == vec!["0"], &o, &mut fails);
    let o = srv.script(&grantee, &db, &format!("SELECT count(*) FROM diary;"));
    check("a grantee reads no per-user rows (auth.uid() is null for it)", rows(&o) == vec!["0"], &o, &mut fails);

    // ── k2.set_claims on the App's own connection (S3) ──
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let tok = super::identity_token::mint(&db, &key, &web_alice, now).unwrap();
    let fresh = || super::identity_token::mint(&db, &key, &web_alice, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64).unwrap();
    let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT 1 FROM (SELECT k2.set_claims('{tok}')) s;\nSELECT auth.uid()::text || '|' || auth.email();\nSELECT body FROM diary;\nCOMMIT;\nSELECT coalesce(auth.uid()::text, 'null');"));
    check(
        "set_claims: the App's login is alice for the transaction, then nobody",
        rows(&o) == vec!["1".to_string(), format!("{}|alice@example.com", alice.id), "alice diary".into(), "null".into()],
        &o,
        &mut fails,
    );
    let mut bad = tok.clone();
    bad.replace_range(bad.len() - 2.., if tok.ends_with("00") { "11" } else { "00" });
    for (name, t, want) in [
        ("a changed signature", bad, "bad signature"),
        ("another database's key", super::identity_token::mint(&db, &"cd".repeat(32), &web_alice, now).unwrap(), "bad signature"),
        ("another database", super::identity_token::mint("other_db", &key, &web_alice, now).unwrap(), "another database"),
        ("expired", super::identity_token::mint(&db, &key, &web_alice, now - 3600).unwrap(), "expired"),
        ("not a token", "k2c1.zz.yy".to_string(), concat!("not a ", k2_core::product_name!(), " claims token")),
    ] {
        let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT k2.set_claims('{t}');\nCOMMIT;"));
        check(&format!("set_claims refuses {name}"), o.as_ref().err().is_some_and(|e| e.contains(want)), &o, &mut fails);
    }
    let o = srv.script(&app_login, &db, &format!("CREATE TEMP TABLE k2_claims_lock (claims jsonb);\nBEGIN;\nSELECT k2.set_claims('{tok}');\nCOMMIT;"));
    check(concat!("set_claims refuses next to a lock ", k2_core::product_name!(), " didn't write"), o.as_ref().err().is_some_and(|e| e.contains("did not write")), &o, &mut fails);
    srv.su(&db, &format!("GRANT SELECT ON k2.claims_key TO \"{grantee}\";"));
    let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT k2.set_claims('{tok}');\nCOMMIT;"));
    check("set_claims refuses while another role can read the key", o.as_ref().err().is_some_and(|e| e.contains("readable by another role")), &o, &mut fails);
    let fixed = call(srv, super::routes::ON, json!({ "app": APP, "db": db }));
    let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT 1 FROM (SELECT k2.set_claims('{}')) s;\nCOMMIT;", fresh()));
    check(concat!(k2_core::cli_name!(), " app compat on puts the key back to its owner only"), fixed.is_ok() && rows(&o) == vec!["1"], (&fixed, &o), &mut fails);
    for pre in ["pg_read_all_data", "pg_write_all_data"] {
        srv.su("postgres", &format!("GRANT {pre} TO outsider;"));
        let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT k2.set_claims('{}');\nCOMMIT;", fresh()));
        check(&format!("set_claims refuses while any role is a member of {pre}"), o.as_ref().err().is_some_and(|e| e.contains("readable by another role")), &o, &mut fails);
        srv.su("postgres", &format!("REVOKE {pre} FROM outsider;"));
        let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT 1 FROM (SELECT k2.set_claims('{}')) s;\nCOMMIT;", fresh()));
        check(&format!("and works again once nobody is a member of {pre}"), rows(&o) == vec!["1"], &o, &mut fails);
    }
    srv.su("postgres", &format!("GRANT \"{db}_migrator\" TO outsider;"));
    let o = srv.script(&app_login, &db, &format!("BEGIN;\nSELECT k2.set_claims('{}');\nCOMMIT;", fresh()));
    check("set_claims refuses while another role is a member of the key's owner", o.as_ref().err().is_some_and(|e| e.contains("readable by another role")), &o, &mut fails);
    srv.su("postgres", &format!("REVOKE \"{db}_migrator\" FROM outsider;"));
    let mig2 = crate::sql::routes::with_test_ops(srv, || {
        std::fs::write(dir.join(".k2/db/migrations/0002_more.sql"), "CREATE TABLE more_t (id int);\n").unwrap();
        crate::sql::ops::migrate(srv, &secrets, &ws, &path, None)
    });
    let o = srv.script(&agent, &db, "SELECT count(*) FROM k2.claims_key;");
    check("after a migrate (privilege sync), the agent still can't read the key", mig2.is_ok() && o.as_ref().err().is_some_and(|e| e.contains("permission denied")), (&mig2.map(|_| ()), &o), &mut fails);

    // ── the brokered lane: claims locked, runs as authenticated ──
    let mut policy = k2_core::skin::RoomPolicy::new();
    policy.insert(ws.clone(), vec![k2_core::skin::CAP_STORE_READ.into(), k2_core::skin::CAP_STORE_WRITE.into()]);
    let pass = k2_core::skin::SkinPass {
        id: uuid::Uuid::new_v4().to_string(),
        principal_id: Some(alice.id.clone()),
        username: "rpc-alice".into(),
        caps: vec![k2_core::skin::CAP_STORE_READ.into(), k2_core::skin::CAP_STORE_WRITE.into()],
        rooms: vec![ws.clone()],
        session: true,
        room_policy: policy,
        app: service_id.clone(),
        room_paths: Default::default(),
        expires_at: None,
        chat: false,
    };
    crate::sql::routes::with_test_ops(srv, || {
        let q = |sql: &str| crate::sql::routes::handle_db_query(json!({ "workspace": ws, "sql": sql, "params": [] }).to_string().as_bytes(), Some(pass.clone()));
        let r = q("SELECT body FROM notes ORDER BY id");
        let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
        check("brokered lane: a TO authenticated policy returns only alice's rows", r.status == "200 OK" && v["rows"] == json!([{ "body": "alice note" }, { "body": "bob gold" }]), (&r.status, &r.body), &mut fails);
        let r = q("SELECT auth.uid()::text AS uid, auth.email() AS email, current_setting('request.jwt.claims')::jsonb ->> 'sub' AS sub");
        let v: J = serde_json::from_str(&r.body).unwrap_or(J::Null);
        check(
            "brokered lane: auth.* and request.jwt.claims answer for the pass",
            v["rows"] == json!([{ "uid": alice.id, "email": "alice@example.com", "sub": alice.id }]),
            (&r.status, &r.body),
            &mut fails,
        );
        let r = q("SELECT claims FROM k2_claims_lock");
        check("brokered lane: the claims lock can't be named", r.status != "200 OK", (&r.status, &r.body), &mut fails);
    });

    // ── the App's own triggers on K2's tables run as the writer: never a superuser ──
    srv.su(
        &db,
        "CREATE TABLE public.trig_log (tbl text, who text, sup boolean);\n\
         GRANT ALL ON public.trig_log TO PUBLIC;\n\
         CREATE FUNCTION public.trig_spy() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN\n\
           INSERT INTO public.trig_log SELECT TG_TABLE_NAME, current_user::text, r.rolsuper FROM pg_roles r WHERE r.rolname = current_user;\n\
           RETURN NULL; END $$;\n\
         CREATE TRIGGER spy AFTER INSERT OR UPDATE OR DELETE ON auth.users FOR EACH ROW EXECUTE FUNCTION public.trig_spy();\n\
         CREATE TRIGGER spy AFTER INSERT OR UPDATE OR DELETE ON k2.claims_key FOR EACH ROW EXECUTE FUNCTION public.trig_spy();",
    );

    // ── auth.users follows the store ──
    k2_core::skin::set_principal_email("rpc-alice", Some("alice2@example.com")).unwrap();
    let grant = k2_core::skin::list_grants().unwrap().into_iter().find(|g| g.subject_id == boss.id && g.target_id == service_id).expect("boss's grant");
    k2_core::skin::set_grant_enabled(&grant.id, false).unwrap();
    // The test's own hand edit, as superuser with triggers off (the spy
    // watches K2's writes only).
    srv.su(&db, &format!("SET session_replication_role = replica;\nUPDATE auth.users SET email = 'tampered@example.com' WHERE id = '{}';", bob.id));
    let w = crate::sql::routes::with_test_ops(srv, || super::users_sync::sync_database(&db));
    check("sync repairs: a change, a disable and a hand edit", w.as_ref().is_ok_and(|w| w.updated == 3 && w.inserted == 0 && w.deleted == 0), format!("{w:?}"), &mut fails);
    let u = srv.su(&db, "SELECT email || '|' || coalesce(banned_until::text, '-') FROM auth.users ORDER BY email;");
    check("auth.users equals the store", u == vec!["alice2@example.com|-", "bob@example.com|-", "boss@example.com|infinity"], &u, &mut fails);
    k2_core::skin::remove_principal("rpc-bob").unwrap();
    let w = crate::sql::routes::with_test_ops(srv, || super::users_sync::sync_database(&db));
    let left = srv.su(&db, &format!("SELECT count(*) FROM notes WHERE user_id = '{}';", bob.id));
    check("a removed user's row goes and the App's ON DELETE CASCADE fires", w.as_ref().is_ok_and(|w| w.deleted == 1) && left == vec!["0"], (format!("{w:?}"), &left), &mut fails);
    let w = crate::sql::routes::with_test_ops(srv, || super::users_sync::sync_database(&db));
    check("a second sync changes nothing", w.as_ref().is_ok_and(|w| !w.changed()), format!("{w:?}"), &mut fails);
    let again = call(srv, super::routes::ON, json!({ "app": APP, "db": db }));
    check("compat on again (the claims key written once more)", again.is_ok(), &again, &mut fails);
    let spied = srv.su(&db, "SELECT tbl || '|' || who || '|' || sup FROM public.trig_log GROUP BY 1 ORDER BY 1;");
    check(
        "an App trigger on auth.users or the key fires as the migrator, never a superuser",
        spied == vec![format!("claims_key|{db}_migrator|false"), format!("users|{db}_migrator|false")],
        &spied,
        &mut fails,
    );
    let del = format!("DELETE FROM auth.sessions WHERE user_id = '{}';", alice.id);
    for q in ["SELECT count(*) FROM auth.sessions;", del.as_str(), "DELETE FROM auth.refresh_tokens;"] {
        let o = srv.script(&format!("{db}_migrator"), &db, q);
        check(&format!("auth.sessions / refresh_tokens say they aren't built: {q}"), o.as_ref().err().is_some_and(|e| e.contains(concat!("not built in ", k2_core::product_name!(), " yet"))), &o, &mut fails);
    }

    // ── a database an older K2 made keeps PUBLIC's CONNECT: nothing at start
    // (the boot seal, the stale-rights sweep) changes it ──
    let older = crate::sql::routes::with_test_ops(srv, || crate::sql::ops::create_database(srv, &secrets, &writer_ws, 1, None, None));
    let older = older.ok().and_then(|v| v["name"].as_str().map(str::to_string)).unwrap_or_default();
    srv.su("postgres", &format!("GRANT CONNECT ON DATABASE \"{older}\" TO PUBLIC;"));
    let seal = crate::sql::ops::seal_existing_databases(srv);
    let sweep = crate::sql_stale_sweep::boot_once(srv);
    let still = srv.su("postgres", &format!("SELECT has_database_privilege('outsider', '{older}', 'CONNECT');"));
    check("an older database keeps PUBLIC's CONNECT through the boot seal and sweep", !older.is_empty() && still == vec!["t"], (&older, &seal, &sweep, &still), &mut fails);

    // ── off ──
    let off = call(srv, super::routes::OFF, json!({ "app": APP }));
    let login = srv.su("postgres", &format!("SELECT rolcanlogin FROM pg_roles WHERE rolname = '{authn}';"));
    let kept = srv.su(&db, "SELECT count(*) FROM auth.users;");
    check("compat off: the door login stops, auth.users stays", off.is_ok() && login == vec!["f"] && kept == vec!["2"], (&off, &login, &kept), &mut fails);
    check("compat off: the doors see it off", super::door_config::for_app(APP).is_none(), "for_app", &mut fails);

    let _ = std::fs::remove_dir_all(&dir);
    assert!(fails.is_empty(), "{} check(s) failed:\n{}", fails.len(), fails.join("\n"));
}
