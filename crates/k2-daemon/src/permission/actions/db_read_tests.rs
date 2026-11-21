//! `db.read` through `k2 permission request` itself (the gate passed): the
//! values that don't hold are refused at the ask, the words never come from
//! the agent, and `apply` refuses to run from an agent's session. The whole
//! flow (`k2 db request-read`, Allow, Deny, `--assign`) is
//! `db_read_routes_tests`; read-only on a real Postgres is `db_read_real_pg`.

use serde_json::json;

use super::super::held::tests::{apply_as, ask, ask_ok, ask_refused, refused_code, seed, Ws};
use super::*;
use crate::sql::sysops::FakeSystemOps;

fn running_with(owner: &Ws, name: &str) -> &'static FakeSystemOps {
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for t in ["sql_grants", "sql_server", "sql_clusters", "sql_databases"] {
            conn.execute(&format!("DELETE FROM {t}"), []).unwrap();
        }
        conn.execute("INSERT INTO sql_server (id, status, installed_major, listen, updated_at) VALUES (1, 'running', 16, 'localhost', 1)", [])
            .unwrap();
    }
    let ops: &'static FakeSystemOps = Box::leak(Box::new(FakeSystemOps::baked()));
    let secrets = crate::sql::secrets::FileSecretStore::default();
    crate::sql::routes::with_fake_ops(ops, || crate::sql::ops::create_database(ops, &secrets, &owner.id, 10, None, Some(name))).expect("create");
    ops
}

#[test]
fn values_that_dont_hold_are_refused_at_the_ask_and_the_words_are_k2s() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    let owner = seed("dbrqown");
    let reader = seed("dbrqread");
    let _ops = running_with(&owner, "dbrq_items");

    let hint = ask_refused(&reader, ACTION, json!({ "db": "dbrq_missing" }));
    assert!(hint.contains("this server has no database dbrq_missing"), "{hint}");
    let hint = ask_refused(&owner, ACTION, json!({ "db": "dbrq_items" }));
    assert!(hint.contains("owns dbrq_items") && hint.contains("no card needed"), "{hint}");
    let (s, v) = ask(&reader, ACTION, json!({}));
    assert!(s.starts_with("400") && v["error"]["hint"].as_str().unwrap_or("").contains("db="), "{s}: {v}");
    // The reader is never a value.
    let (s, v) = ask(&reader, ACTION, json!({ "db": "dbrq_items", "workspace": owner.handle }));
    assert!(s.starts_with("400") && v["error"]["hint"].as_str().unwrap_or("").contains("passport"), "{s}: {v}");

    let v = ask_ok(&reader, ACTION, json!({ "db": "dbrq_items" }));
    assert_eq!(v["text"], format!("Let {} read the dbrq_items database (owned by {}).", reader.handle, owner.handle), "{v}");
    assert_eq!(v["approvers"], "owner or admin", "{v}");

    // Missing database: computed words say so, from the stored value only.
    let ctx = Ctx { workspace_id: reader.id.clone(), workspace_handle: reader.handle.clone(), params: json!({ "db": "gone_db" }).as_object().cloned().unwrap() };
    let w = with_computed(ACTION, &ctx).expect("its own words");
    assert_eq!((w.params["name"].as_str(), w.params["owner"].as_str()), (Some("gone_db"), Some("a workspace this server can't find")));
    assert!(with_computed("vault.use", &ctx).is_none());
}

#[test]
fn apply_never_runs_from_an_agents_session() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    k2_core::db::init_for_tests();
    let owner = seed("dbrpown");
    let reader = seed("dbrpread");
    let ops = running_with(&owner, "dbrp_items");
    let v = ask_ok(&reader, ACTION, json!({ "db": "dbrp_items" }));
    let id = v["id"].as_str().expect("id").to_string();
    let p = crate::session_token::HookPrincipal { workspace_uuid: reader.id.clone(), agent_address: reader.handle.clone() };
    let got = crate::caller_workspace::with_request_principal(Some(p), || {
        crate::sql::routes::with_fake_ops(ops, || apply_as(&id, &Answerer::OwnerToken, None))
    });
    assert_eq!(refused_code(got), "person_only");
    let granted: i64 = {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.query_row("SELECT COUNT(*) FROM sql_grants WHERE project_id = ?1", [&reader.id], |r| r.get(0)).unwrap()
    };
    assert_eq!(granted, 0, "nothing granted");
}
