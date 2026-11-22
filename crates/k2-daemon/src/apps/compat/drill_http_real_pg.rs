//! The drill's HTTP pass against real Postgres: every table as every
//! identity, RLS deciding, and a difference found when the two sides hold
//! different rows. `#[ignore]`: REALPG_COMPAT=1 on a compute node.

use serde_json::json;

use super::super::door::tests::cfg_for;
use super::super::pool::{set_test_target, Caller, DbRole, Target};
use super::*;
use k2_rest::testkit;

const ANN: &str = "11111111-1111-4111-8111-111111111111";
const POLICIES: &str = "
ALTER TABLE public.projects ENABLE ROW LEVEL SECURITY;
CREATE POLICY anon_open ON public.projects FOR SELECT TO anon USING (status = 'open');
CREATE POLICY mine ON public.projects FOR ALL TO authenticated USING (owner_id = auth.uid() OR reviewer_id = auth.uid());
";

fn side(tag: &str) -> DoorConfig {
    let s = testkit::shared();
    let db = s.fresh_db(tag);
    s.su(&db, &[include_str!("../../../../k2-rest/src/fixture.sql"), POLICIES]);
    let app = format!("drill-{db}");
    set_test_target(&app, Target { host: "127.0.0.1".into(), port: s.port, db: db.clone(), user: format!("{db}_authenticator"), password: testkit::AUTHENTICATOR_PASSWORD.into(), size: 2 });
    cfg_for(&app, &db)
}

fn identities() -> Vec<Identity> {
    vec![
        Identity { label: "service".into(), caller: Caller { role: DbRole::Service, claims: json!({"role": "service_role"}) } },
        Identity { label: "anon".into(), caller: Caller { role: DbRole::Anon, claims: json!({"role": "anon"}) } },
        Identity { label: "user:ann".into(), caller: Caller { role: DbRole::Authenticated, claims: json!({"role": "authenticated", "sub": ANN}) } },
    ]
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1"]
async fn real_pg_the_http_pass_reads_every_table_as_every_identity() {
    assert_eq!(std::env::var("REALPG_COMPAT").as_deref(), Ok("1"));
    let a = side("drill_a");
    let b = side("drill_b");
    let ours = k2_side(&a, &identities()).await.expect("our side");
    let total = |t: &str, who: &str| ours.iter().find(|p| p.table == t && p.identity == who).unwrap_or_else(|| panic!("{t} {who}")).total;
    assert_eq!(total("projects", "service"), Some(5));
    assert_eq!(total("projects", "anon"), Some(3), "RLS: anon sees open ones");
    assert_eq!(total("projects", "user:ann"), Some(3), "RLS: ann's own and reviewed");
    assert!(ours.iter().any(|p| p.table == "open_projects_v"), "views are read too");
    // Two copies of the same data: no difference.
    let theirs = k2_side(&b, &identities()).await.expect("their side");
    assert_eq!(differences(&ours, &theirs), Vec::<String>::new());
    // One row changed on one side: exactly the probes that see it differ.
    testkit::shared().su(&b.database, &["UPDATE public.projects SET title = 'changed' WHERE id = 3"]);
    let theirs = k2_side(&b, &identities()).await.expect("their side");
    let d = differences(&ours, &theirs);
    assert!(d.iter().any(|l| l.starts_with("projects as service:")), "{d:?}");
    assert!(d.iter().any(|l| l.starts_with("projects as user:ann:")), "{d:?}");
    assert!(d.iter().any(|l| l.starts_with("open_projects_v as")), "{d:?}");
    assert!(!d.iter().any(|l| l.starts_with("tasks as")), "{d:?}");
}
