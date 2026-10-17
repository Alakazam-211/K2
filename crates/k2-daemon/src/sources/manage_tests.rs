//! "Manage Gardens and data sources" (`gardens_manage`, 0181) on the
//! sources routes: an agent without the switch is refused as before; with
//! it, approve / bind / pause / pin / trigger enable need no confirm and
//! are audited; a bind targets only a secret the agent may already use;
//! a multi-source bind is all or nothing; an agent never grants itself
//! the switch. Fail loudly.

use std::collections::HashMap;

use k2_core::sources::store;
use k2_core::sources::trigger_store as ts;
use k2_core::sources::triggers::{TriggerOn, TriggerStatus};
use k2_core::sources::SourceStatus;
use serde_json::{json, Value};

use crate::compute_routes::Who;
use crate::sources::approve_routes::tests::{admin, env, hash, insert_source};
use crate::sources::approve_routes::{self, test_seams};
use crate::sources::SourceErr;

const WS: &str = "p1";

fn agent() -> Who {
    Who::Agent { project_id: WS.into(), session_id: "sess-mgr-1".into() }
}

/// Workspace `p1` exists with the switch (and vault_manage) as given.
fn workspace(manage: bool, vault_manage: bool) {
    let h = k2_core::db::shared();
    let c = h.lock();
    c.execute(
        "INSERT OR REPLACE INTO projects (id, name, path, handle, agents_can_manage_gardens, agents_can_manage_vault) \
         VALUES (?1, 'p1', '/tmp/mgr/p1', 'p1', ?2, ?3)",
        rusqlite::params![WS, i64::from(manage), i64::from(vault_manage)],
    )
    .expect("insert workspace");
}

fn call(path: &str, who: &Who, body: Value) -> Result<Value, SourceErr> {
    approve_routes::handle(path, who, body.to_string().as_bytes(), &HashMap::new())
}

fn row(name: &str) -> k2_core::sources::SourceRow {
    let h = k2_core::db::shared();
    let c = h.lock();
    store::get_by_name(&c, name).expect("get").unwrap_or_else(|| panic!("no source {name}"))
}

/// `(action, actor, detail)` rows of source_audit, oldest first.
fn source_audit() -> Vec<(String, String, Value)> {
    let h = k2_core::db::shared();
    let c = h.lock();
    let mut s = c.prepare("SELECT action, actor, detail_json FROM source_audit ORDER BY id").expect("prepare");
    let rows = s
        .query_map([], |r| {
            let d: String = r.get(2)?;
            Ok((r.get(0)?, r.get(1)?, serde_json::from_str(&d).unwrap_or(Value::Null)))
        })
        .expect("query")
        .map(|r| r.expect("row"))
        .collect();
    rows
}

fn access_audit(event: &str, target: &str) -> Vec<Value> {
    k2_core::auth_audit::tail(1000)
        .expect("audit tail")
        .into_iter()
        .filter(|e| e["event"] == event && e["outcome"].as_str().is_some_and(|o| o.contains(&format!("target={target}"))))
        .collect()
}

#[test]
fn without_the_switch_an_agent_is_refused_as_today() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(false, true);
    insert_source("fleet", "proposed", None, false, "member");
    for path in ["/cli/sources/pending", "/cli/sources/approve", "/cli/sources/batch-approve", "/cli/sources/secret", "/cli/sources/pin"] {
        let e = call(path, &agent(), json!({"name": "fleet", "specHash": hash()})).unwrap_err();
        assert_eq!((e.code, e.status), ("person_only", 403), "{path}");
    }
    assert_eq!(row("fleet").status, SourceStatus::Proposed);
    let r = crate::sources_routes::handle("/cli/sources/pause", agent(), br#"{"name":"fleet"}"#, &HashMap::new());
    assert!(r.body.contains("person_only"), "{}", r.body);
}

#[test]
fn with_the_switch_approve_binds_its_own_workspace_secret_with_no_confirm_and_audits() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(true, false);
    let id = insert_source("deploys", "proposed", Some("WS_KEY"), false, "admin");
    let p = call("/cli/sources/pending", &agent(), json!({})).expect("pending");
    assert_eq!(p["sources"][0]["name"], "deploys", "{p}");
    // No confirmToken: an agent with the switch needs none.
    let r = call("/cli/sources/approve", &agent(), json!({"name": "deploys", "specHash": hash()})).expect("approve");
    assert_eq!((r["status"].as_str(), r["secretBound"].as_bool()), (Some("active"), Some(true)), "{r}");
    let after = row("deploys");
    assert_eq!(after.status, SourceStatus::Active);
    assert!(after.secret_bound);
    assert_eq!(after.secret_workspace.as_deref(), Some(WS), "bound from the agent's own workspace scope");
    assert_eq!(after.approved_by.as_deref(), Some("agent:p1"));
    let audit = source_audit();
    let approve = audit.iter().find(|(a, _, _)| a == "approve").expect("approve row");
    assert_eq!(approve.1, "agent:p1");
    assert_eq!(approve.2["session"], "sess-mgr-1", "{audit:?}");
    let bind = audit.iter().find(|(a, _, _)| a == "bind").expect("bind row");
    assert_eq!((bind.2["secret"].as_str(), bind.2["scope"].as_str()), (Some("WS_KEY"), Some("workspace")), "{audit:?}");
    let lines = access_audit("sources-bind", "deploys");
    assert_eq!(lines.len(), 1, "one Access Audit bind line");
    let o = lines[0]["outcome"].as_str().expect("outcome");
    assert!(o.contains("secret=WS_KEY") && o.contains("session=sess-mgr-1"), "{o}");
    assert_eq!(access_audit("sources-approve", "deploys").len(), 1);
    let _ = id;
}

#[test]
fn a_server_secret_needs_vault_manage_too_and_an_unknown_one_is_refused() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(true, false);
    insert_source("deploys", "proposed", Some("VERCEL_RO"), false, "admin");
    let e = call("/cli/sources/approve", &agent(), json!({"name": "deploys", "specHash": hash()})).unwrap_err();
    assert_eq!((e.code, e.status), ("secret_not_usable", 403), "{}", e.hint);
    assert_eq!(row("deploys").status, SourceStatus::Proposed, "a refused bind changes nothing");
    // A secret the agent can't see at all (another workspace's, or none).
    let e = call("/cli/sources/approve", &agent(), json!({"name": "deploys", "specHash": hash(), "secret": "OTHER_TEAM_KEY"}))
        .unwrap_err();
    assert_eq!((e.code, e.status), ("secret_not_found", 404));
    assert!(!row("deploys").secret_bound);
    // With vault_manage as well, the server secret binds (server scope).
    workspace(true, true);
    call("/cli/sources/approve", &agent(), json!({"name": "deploys", "specHash": hash()})).expect("approve");
    let after = row("deploys");
    assert!(after.secret_bound);
    assert_eq!(after.secret_workspace, None, "a server secret binds in server scope");
}

#[test]
fn the_secret_route_binds_and_unbinds_for_an_agent_with_the_switch() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(true, false);
    insert_source("deploys", "active", None, false, "admin");
    let e = call("/cli/sources/secret", &agent(), json!({"name": "deploys", "secret": "STRIPE_RK"})).unwrap_err();
    assert_eq!(e.code, "secret_not_usable", "a server secret without vault_manage");
    let r = call("/cli/sources/secret", &agent(), json!({"name": "deploys", "secret": "WS_KEY"})).expect("bind");
    assert_eq!(r["secretBound"], true);
    assert_eq!(row("deploys").secret_workspace.as_deref(), Some(WS));
    call("/cli/sources/secret", &agent(), json!({"name": "deploys", "secret": null})).expect("unbind");
    let after = row("deploys");
    assert!(!after.secret_bound);
    assert_eq!(after.secret_workspace, None);
    assert_eq!(access_audit("sources-unbind", "deploys").len(), 1);
}

#[test]
fn an_agent_never_sets_a_persons_choices_on_approve() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(true, false);
    insert_source("fleet", "proposed", None, false, "admin");
    for (k, v) in [("viewers", "member"), ("agentAccess", "values"), ("keep", "states")] {
        let mut b = json!({"name": "fleet", "specHash": hash()});
        b[k] = json!(v);
        let e = call("/cli/sources/approve", &agent(), b).unwrap_err();
        assert_eq!((e.code, e.status), ("field_person_only", 403), "{k}");
    }
    assert_eq!(row("fleet").status, SourceStatus::Proposed);
}

/// One secret, three sources: bound together or not at all.
#[test]
fn a_multi_source_bind_is_all_or_nothing() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(true, false);
    for n in ["a-src", "b-src", "c-src"] {
        insert_source(n, "proposed", None, false, "member");
    }
    let items = |secret: &str| json!(["a-src", "b-src", "c-src"].iter().map(|n| json!({"name": n, "specHash": hash(), "secret": secret})).collect::<Vec<_>>());
    // A server secret (no vault_manage) refuses the whole batch up front.
    let e = call("/cli/sources/batch-approve", &agent(), json!({"items": items("VERCEL_RO")})).unwrap_err();
    assert_eq!(e.code, "secret_not_usable");
    // One source fails to bind halfway: the earlier binds are undone and
    // nothing is activated.
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.execute("UPDATE sources SET origins_json = '[\"https://bad.example.com:443\"]' WHERE name = 'b-src'", []).expect("origin");
    }
    test_seams::set_secrets(Some(test_seams::FakeSecrets {
        bind: |_, _, origin| {
            if origin.contains("bad.example.com") {
                Err(SourceErr::new("secret_origin_mismatch", 400, "not one of the secret's sites"))
            } else {
                Ok(())
            }
        },
        unbind: |_| Ok(()),
        root_key: |_| None,
        exists: |ws, s| ws == Some("p1") && s == "WS_KEY",
    }));
    let e = call("/cli/sources/batch-approve", &agent(), json!({"items": items("WS_KEY")})).unwrap_err();
    assert_eq!(e.code, "secret_origin_mismatch");
    for n in ["a-src", "b-src", "c-src"] {
        let r = row(n);
        assert_eq!((r.status, r.secret_bound), (SourceStatus::Proposed, false), "{n} changed after a failed batch");
    }
    // All three bind once every origin fits.
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        c.execute("UPDATE sources SET origins_json = '[\"https://api.example.com:443\"]' WHERE name = 'b-src'", []).expect("origin");
    }
    let r = call("/cli/sources/batch-approve", &agent(), json!({"items": items("WS_KEY")})).expect("batch");
    assert_eq!(r["approved"], json!(["a-src", "b-src", "c-src"]));
    for n in ["a-src", "b-src", "c-src"] {
        let r = row(n);
        assert!(r.secret_bound && r.secret_workspace.as_deref() == Some(WS), "{n}");
        assert_eq!(access_audit("sources-bind", n).len(), 1, "{n}");
    }
}

/// A person binds one secret to many sources with ONE batch confirm.
#[test]
fn a_person_binds_one_secret_to_many_sources_with_one_confirm() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    let ids: Vec<String> = ["a-src", "b-src"].iter().map(|n| insert_source(n, "proposed", None, false, "member")).collect();
    let items = json!([{"name": "a-src", "specHash": hash(), "secret": "VERCEL_RO"}, {"name": "b-src", "specHash": hash(), "secret": "VERCEL_RO"}]);
    let e = call("/cli/sources/batch-approve", &admin(), json!({"items": items})).unwrap_err();
    assert_eq!(e.code, "vault_confirm_required");
    let want = approve_routes::batch_hash(&[(ids[0].clone(), "VERCEL_RO".into()), (ids[1].clone(), "VERCEL_RO".into())]);
    assert_eq!(e.extra["batchHash"], want.as_str());
    let v = crate::compute_admin::test_seams::vault_handle();
    let token = v.confirm.mint("source-batch", Some(&want), "connect:ada", "vault-password");
    call("/cli/sources/batch-approve", &admin(), json!({"items": items, "confirmToken": token})).expect("one confirm");
    for n in ["a-src", "b-src"] {
        assert!(row(n).secret_bound, "{n}");
    }
}

#[test]
fn pin_pause_and_trigger_enable_need_no_confirm_with_the_switch() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(true, false);
    insert_source("fleet", "active", None, false, "member");
    let r = call("/cli/sources/pin", &agent(), json!({"name": "fleet"})).expect("pin");
    assert_eq!(r["pinned"], true);
    assert_eq!(access_audit("sources-pin", "fleet").len(), 1);
    call("/cli/sources/unpin", &agent(), json!({"name": "fleet"})).expect("unpin");
    let r = crate::sources_routes::handle("/cli/sources/pause", agent(), br#"{"name":"fleet"}"#, &HashMap::new());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(row("fleet").status, SourceStatus::Paused);
    assert_eq!(access_audit("sources-pause", "fleet").len(), 1);
    let pause_row = source_audit().into_iter().find(|(a, _, _)| a == "pause").expect("pause row");
    assert_eq!(pause_row.2["session"], "sess-mgr-1");
    let r = crate::sources_routes::handle("/cli/sources/resume", agent(), br#"{"name":"fleet"}"#, &HashMap::new());
    assert_eq!(r.status, "200 OK", "{}", r.body);
    // A trigger another agent proposed: approve, pause, resume.
    let t = {
        let h = k2_core::db::shared();
        let c = h.lock();
        let src = store::get_by_name(&c, "fleet").expect("get").expect("fleet");
        ts::insert(
            &c,
            &ts::NewTrigger {
                source_id: &src.id,
                on: &TriggerOn::WentCrit,
                target_project_id: "p2",
                note: None,
                cooldown_secs: 1800,
                daily_cap: 12,
                status: TriggerStatus::Proposed,
                made_by: "agent:p2",
                made_by_project: Some("p2"),
                approved_by: None,
                now: 0,
            },
        )
        .expect("trigger")
    };
    let tr = |p: &str| crate::sources::trigger_routes::handle(p, &agent(), json!({"id": t.id}).to_string().as_bytes(), &HashMap::new());
    assert_eq!(tr("/cli/sources/triggers/approve").expect("approve")["status"], "active");
    assert_eq!(tr("/cli/sources/triggers/pause").expect("pause")["status"], "paused");
    // Cooldown and cap stay a person's, switch or not.
    let e = crate::sources::trigger_routes::handle(
        "/cli/sources/triggers/edit",
        &agent(),
        json!({"id": t.id, "set": {"cooldownSecs": 60}}).to_string().as_bytes(),
        &HashMap::new(),
    )
    .unwrap_err();
    assert_eq!(e.code, "person_only");
    assert_eq!(access_audit("sources-trigger-approve", &format!("fleet:{}", t.id)).len(), 1);
}

/// The switch is written only by the Owner/Admin route; an agent (with
/// the switch or not) can never turn it on, its own or another's.
#[test]
fn an_agent_never_grants_itself_the_switch() {
    let _h = crate::test_support::TempHome::new();
    let _e = env();
    workspace(false, false);
    let body = json!({"workspace": WS, "toggle": "gardens_manage", "value": true}).to_string();
    let me = crate::sidecar_routes::Caller::Agent(crate::sidecar_routes::AgentCaller {
        project_id: WS.into(),
        map_key: None,
        args: vec![],
        sandboxed: false,
    });
    for c in [me, crate::sidecar_routes::Caller::Shell { project_id: WS.into() }] {
        let r = crate::sidecar_routes::handle("/cli/agent-access/set", c.clone(), body.as_bytes(), &HashMap::new(), "test");
        assert_eq!(r.status, "403 Forbidden", "{c:?}: {}", r.body);
    }
    assert!(!k2_core::workspace::settings::agents_can_manage_gardens(WS), "a refused flip changes nothing");
    assert!(!crate::session_token::is_agent_verb("/cli/agent-access/set"));
    let r = crate::sidecar_routes::handle("/cli/agent-access/set", crate::sidecar_routes::Caller::Owner, body.as_bytes(), &HashMap::new(), "test");
    assert_eq!(r.status, "200 OK", "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    assert_eq!(v["agentsCanManageGardens"], true, "{v}");
    assert!(k2_core::workspace::settings::agents_can_manage_gardens(WS));
}
