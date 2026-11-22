//! The doors' pool against a real Postgres: roles and claims are set per
//! request, end with it, and RLS decides. `#[ignore]`: runs only with
//! REALPG_COMPAT=1 and a Postgres server's binaries (REALPG_BIN), on a
//! compute node:
//! `REALPG_COMPAT=1 cargo test -p k2-daemon --lib apps::compat -- --ignored`.

use super::*;
use k2_rest::testkit;
use serde_json::json;

const ALICE: &str = "11111111-1111-4111-8111-111111111111";
const BOB: &str = "22222222-2222-4222-8222-222222222222";

fn guard() -> &'static testkit::Server {
    assert_eq!(std::env::var("REALPG_COMPAT").as_deref(), Ok("1"), "REALPG_COMPAT=1 marks the throwaway job");
    testkit::shared()
}

fn target(s: &testkit::Server, db: &str, size: u32) -> Target {
    Target {
        host: "127.0.0.1".into(),
        port: s.port,
        db: db.into(),
        user: format!("{db}_authenticator"),
        password: testkit::AUTHENTICATOR_PASSWORD.into(),
        size,
    }
}

fn user(sub: &str, email: &str) -> Caller {
    Caller {
        role: DbRole::Authenticated,
        claims: json!({"sub": sub, "role": "authenticated", "email": email, "aud": "authenticated",
                       "app_metadata": {"k2_app": "svc-pool"}, "user_metadata": {}}),
    }
}

fn anon() -> Caller {
    Caller { role: DbRole::Anon, claims: json!({"role": "anon"}) }
}

fn service() -> Caller {
    Caller { role: DbRole::Service, claims: json!({"role": "service_role"}) }
}

fn meta() -> RequestMeta {
    RequestMeta { method: "GET".into(), path: "/rest/v1/notes".into(), headers: Default::default(), schema: "public".into() }
}

fn notes_fixture(s: &testkit::Server, db: &str) {
    s.su(
        db,
        &[
            "CREATE TABLE public.notes (id serial PRIMARY KEY, owner uuid NOT NULL, body text NOT NULL)",
            "ALTER TABLE public.notes ENABLE ROW LEVEL SECURITY",
            "CREATE POLICY own_read ON public.notes FOR SELECT TO authenticated USING (auth.uid() = owner)",
            "CREATE POLICY own_write ON public.notes FOR INSERT TO authenticated WITH CHECK (auth.uid() = owner)",
            "GRANT SELECT, INSERT ON public.notes TO authenticated",
            "GRANT ALL ON public.notes TO service_role",
            "GRANT USAGE ON SEQUENCE public.notes_id_seq TO authenticated, service_role",
            &format!("INSERT INTO public.notes (owner, body) VALUES ('{ALICE}', 'a1'), ('{ALICE}', 'a2'), ('{BOB}', 'b1')"),
            "CREATE FUNCTION public.leak() RETURNS text LANGUAGE sql AS $$ SELECT set_config('app.leak', 'leaked', false) $$",
            "GRANT EXECUTE ON FUNCTION public.leak() TO authenticated",
        ],
    );
}

async fn one<E: std::fmt::Debug>(pool: &Pool, c: &Caller, timeout: u32, sql: &str) -> Result<Vec<Row>, RunError<E>> {
    run(pool, c, &meta(), timeout, &[sql.to_string()], |_| Ok::<(), E>(())).await.map(|mut v| v.remove(0))
}

fn pg<E: std::fmt::Debug>(r: Result<Vec<Row>, RunError<E>>) -> PgFail {
    match r {
        Err(RunError::Pg(p)) => p,
        other => panic!("expected a Postgres refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1 on a compute node"]
async fn real_pg_pool_sets_role_and_claims_per_request() {
    let s = guard();
    let db = s.fresh_db("pool_claims");
    notes_fixture(s, &db);
    let pool = pool_for(&target(s, &db, 2)).expect("pool");

    let rows = one::<()>(&pool, &user(ALICE, "alice@example.com"), 8000, "SELECT current_user::text, auth.uid()::text, auth.role(), auth.email(), current_setting('request.method'), current_setting('request.path'), current_setting('statement_timeout'), current_setting('search_path')")
        .await
        .expect("as alice");
    let r = &rows[0];
    assert_eq!(r.get::<_, String>(0), "authenticated");
    assert_eq!(r.get::<_, String>(1), ALICE);
    assert_eq!(r.get::<_, String>(2), "authenticated");
    assert_eq!(r.get::<_, String>(3), "alice@example.com");
    assert_eq!(r.get::<_, String>(4), "GET");
    assert_eq!(r.get::<_, String>(5), "/rest/v1/notes");
    assert_eq!(r.get::<_, String>(6), "8s");
    assert_eq!(r.get::<_, String>(7), "\"public\"");

    // RLS decides: alice sees her two rows, bob his one, the service all.
    let n = |rows: Vec<Row>| rows[0].get::<_, i64>(0);
    assert_eq!(n(one::<()>(&pool, &user(ALICE, "a@x"), 8000, "SELECT count(*) FROM public.notes").await.unwrap()), 2);
    assert_eq!(n(one::<()>(&pool, &user(BOB, "b@x"), 8000, "SELECT count(*) FROM public.notes").await.unwrap()), 1);
    assert_eq!(n(one::<()>(&pool, &service(), 0, "SELECT count(*) FROM public.notes").await.unwrap()), 3);
    // anon has no grant: Postgres's own 42501.
    let p = pg(one::<()>(&pool, &anon(), 3000, "SELECT count(*) FROM public.notes").await);
    assert_eq!(p.code, "42501", "{p:?}");
    // A write the policy refuses.
    let p = pg(one::<()>(&pool, &user(ALICE, "a@x"), 8000, &format!("INSERT INTO public.notes (owner, body) VALUES ('{BOB}', 'forged')")).await);
    assert_eq!(p.code, "42501", "{p:?}");
    assert!(p.message.contains("row-level security"), "{p:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1 on a compute node"]
async fn real_pg_nothing_outlives_its_request_on_a_pooled_connection() {
    let s = guard();
    let db = s.fresh_db("pool_leak");
    notes_fixture(s, &db);
    // One connection: every request below reuses it.
    let pool = pool_for(&target(s, &db, 1)).expect("pool");
    let pid = |rows: &[Row]| rows[0].get::<_, i32>(0);
    let first = one::<()>(&pool, &user(ALICE, "a@x"), 8000, "SELECT pg_backend_pid(), public.leak()").await.expect("alice");
    // The bare connection after the request: the authenticator, no claims,
    // no role, nothing the function set for the session.
    let raw = pool.get().await.expect("the same connection");
    let after = raw
        .query(
            "SELECT pg_backend_pid(), current_user::text, coalesce(current_setting('request.jwt.claims', true), ''), \
             coalesce(current_setting('request.jwt.claim.sub', true), ''), coalesce(current_setting('app.leak', true), '')",
            &[],
        )
        .await
        .expect("bare query");
    assert_eq!(pid(&first), pid(&after), "the pool of one reused its connection");
    assert_eq!(after[0].get::<_, String>(1), format!("{db}_authenticator"));
    assert_eq!(after[0].get::<_, String>(2), "", "claims ended with the request");
    assert_eq!(after[0].get::<_, String>(3), "", "sub ended with the request");
    // Our own raw checkout above held the connection; drop it and let the
    // pool's Clean recycle run, then a session setting is gone too.
    drop(raw);
    let again = pool.get().await.expect("recycled");
    let v = again.query_one("SELECT coalesce(current_setting('app.leak', true), '')", &[]).await.expect("after recycle");
    assert_eq!(v.get::<_, String>(0), "", "the recycle reset what a function set for the session");
    drop(again);
    // The authenticator itself holds no rights (NOINHERIT): without the
    // role switch it can't read the table.
    let bare = pool.get().await.expect("bare");
    let e = bare.query("SELECT count(*) FROM public.notes", &[]).await.expect_err("authenticator alone is refused");
    assert_eq!(e.as_db_error().map(|d| d.code().code().to_string()).as_deref(), Some("42501"), "{e}");
    drop(bare);
    // The next request as bob on the same connection sees only bob.
    let rows = one::<()>(&pool, &user(BOB, "b@x"), 8000, "SELECT auth.uid()::text, count(*) OVER () FROM public.notes").await.expect("bob");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<_, String>(0), BOB);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1 on a compute node"]
async fn real_pg_a_failed_check_or_statement_rolls_back_and_timeouts_hold() {
    let s = guard();
    let db = s.fresh_db("pool_tx");
    notes_fixture(s, &db);
    let pool = pool_for(&target(s, &db, 2)).expect("pool");
    let insert = format!("INSERT INTO public.notes (owner, body) VALUES ('{ALICE}', 'rolled') RETURNING id");
    // The door's check refuses: nothing is kept.
    let r = run(&pool, &user(ALICE, "a@x"), &meta(), 8000, &[insert.clone()], |rows| {
        assert_eq!(rows[0].len(), 1, "the insert ran");
        Err("singular check failed")
    })
    .await;
    assert!(matches!(r, Err(RunError::Check("singular check failed"))), "{r:?}");
    // A later statement fails: the earlier insert goes too.
    let r = run(&pool, &user(ALICE, "a@x"), &meta(), 8000, &[insert.clone(), "SELECT 1/0".into()], |_| Ok::<(), ()>(())).await;
    assert_eq!(pg(r.map(|mut v| v.remove(0))).code, "22012");
    let count = one::<()>(&pool, &service(), 0, "SELECT count(*) FROM public.notes WHERE body = 'rolled'").await.unwrap();
    assert_eq!(count[0].get::<_, i64>(0), 0, "both rolled back");
    // A committed one stays.
    run(&pool, &user(ALICE, "a@x"), &meta(), 8000, &[insert], |_| Ok::<(), ()>(())).await.expect("commit");
    let count = one::<()>(&pool, &service(), 0, "SELECT count(*) FROM public.notes WHERE body = 'rolled'").await.unwrap();
    assert_eq!(count[0].get::<_, i64>(0), 1);
    // The role's statement timeout holds (57014, query_canceled).
    s.su(&db, &["GRANT EXECUTE ON FUNCTION pg_sleep(double precision) TO anon"]);
    let p = pg(one::<()>(&pool, &anon(), 200, "SELECT pg_sleep(2)").await);
    assert_eq!(p.code, "57014", "{p:?}");
    // And the connection is fine afterwards.
    let ok = one::<()>(&pool, &service(), 0, "SELECT 1").await.expect("after a timeout");
    assert_eq!(ok[0].get::<_, i32>(0), 1);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1 on a compute node"]
async fn real_pg_a_new_password_gets_a_new_pool_and_a_wrong_one_is_unavailable() {
    let s = guard();
    let db = s.fresh_db("pool_pw");
    let t = target(s, &db, 1);
    let a = pool_for(&t).expect("pool");
    let b = pool_for(&t).expect("pool again");
    let ok = one::<()>(&b, &service(), 0, "SELECT 1").await.expect("works");
    assert_eq!(ok[0].get::<_, i32>(0), 1);
    drop(a);
    let mut wrong = t.clone();
    wrong.password = "not-it".into();
    let w = pool_for(&wrong).expect("a pool is built lazily");
    match one::<()>(&w, &service(), 0, "SELECT 1").await {
        Err(RunError::Unavailable(why)) => assert!(why.contains("password") || why.contains("authentication"), "{why}"),
        other => panic!("a wrong password must be unavailable, got {other:?}"),
    }
    forget_db(&db);
}

/// The role comes only from [`DbRole`]: claims that name another role
/// (`postgres`, `service_role`, the migrator) change nothing about who the
/// statement runs as, and RLS still scopes the rows.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1 on a compute node"]
async fn real_pg_claims_never_choose_the_role() {
    let s = guard();
    let db = s.fresh_db("pool_role");
    notes_fixture(s, &db);
    let pool = pool_for(&target(s, &db, 2)).expect("pool");
    let migrator = format!("{db}_migrator");
    for forged in ["postgres", "service_role", migrator.as_str()] {
        let c = Caller { role: DbRole::Authenticated, claims: json!({"sub": ALICE, "role": forged, "email": "a@x"}) };
        let rows = one::<()>(&pool, &c, 8000, "SELECT current_user::text, (SELECT count(*) FROM public.notes), auth.role()").await.expect("runs");
        assert_eq!(rows[0].get::<_, String>(0), "authenticated", "claims role {forged}");
        assert_eq!(rows[0].get::<_, i64>(1), 2, "RLS still scopes to alice under claims role {forged}");
        assert_eq!(rows[0].get::<_, String>(2), forged, "auth.role() reports the claim, as on Supabase");
    }
    // Anon is anon whatever the claims say.
    let c = Caller { role: DbRole::Anon, claims: json!({"role": "service_role"}) };
    let rows = one::<()>(&pool, &c, 3000, "SELECT current_user::text").await.expect("runs");
    assert_eq!(rows[0].get::<_, String>(0), "anon");
    // Each of the three lands as itself.
    for (r, want) in [(DbRole::Anon, "anon"), (DbRole::Authenticated, "authenticated"), (DbRole::Service, "service_role")] {
        let rows = one::<()>(&pool, &Caller { role: r, claims: json!({}) }, 0, "SELECT current_user::text").await.expect("runs");
        assert_eq!(rows[0].get::<_, String>(0), want);
    }
}
