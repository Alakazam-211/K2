//! R8 end to end in the daemon: a published service's `.env.k2` server
//! reference is refused without a grant and works with one; owner-set
//! session env still resolves (K2's own path), agent-writable env doesn't;
//! the once-only upgrade grants only references that were really in use.

use std::collections::HashMap;
use std::path::Path;

use k2_vault::{Actor, Consumer, Delivery, GrantRequest, Kind, Namespace, PutRequest, Requester, Scope, SecretBytes, SecretStore, Selector, Use};

use super::*;

fn put_server(v: &k2_vault::LocalVault, name: &str, value: &str) -> Selector {
    let mut r = PutRequest::new(Namespace::User, Scope::server(), name, Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put server secret");
    Selector::user(Scope::server(), name)
}

fn grant(v: &k2_vault::LocalVault, sel: &Selector, consumer: Consumer, env: Option<&str>) {
    v.grant(
        GrantRequest {
            selector: sel.clone(),
            consumer,
            delivery: Delivery::Env,
            env_name: env.map(str::to_string),
            file_name: None,
            hosts_narrowed: None,
            expires_at: None,
            version: None,
        },
        &Actor::Owner,
    )
    .expect("grant");
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple().to_string()[..10].to_ascii_uppercase())
}

fn env_k2(dir: &Path, lines: &[(&str, String)]) {
    let body: String = lines.iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    std::fs::write(dir.join(".env.k2"), body).expect("write .env.k2");
}

fn env_of(g: &super::super::publish_env::Granted, key: &str) -> Option<String> {
    g.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

/// The hole (vs-live C22): a service read any server secret by naming it.
#[test]
fn a_service_without_a_grant_gets_no_server_secret_and_one_with_a_grant_does() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let dir = crate::test_support::Scratch::new();
    let name = uniq("R8_SVC");
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let sel = put_server(&v, &name, &value);
    env_k2(dir.path(), &[("API_TOKEN", format!("k2v://server/{name}")), ("PLAIN_URL", "https://example.com".into())]);
    let (svc_a, svc_b) = (uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string());

    let g = super::super::publish_env::granted(&svc_a, "ws-r8-a", dir.path());
    assert_eq!(env_of(&g, "API_TOKEN"), None, "no grant: refused");
    assert!(!g.env.iter().any(|(_, x)| x == &value), "the value never reaches the env");
    assert_eq!(env_of(&g, "PLAIN_URL").as_deref(), Some("https://example.com"), "literals still pass");

    // Granted to service A only: A gets it, B (same workspace) doesn't.
    grant(&v, &sel, Consumer::published_service(svc_a.clone()), Some("API_TOKEN"));
    let g = super::super::publish_env::granted(&svc_a, "ws-r8-a", dir.path());
    assert_eq!(env_of(&g, "API_TOKEN").as_deref(), Some(value.as_str()), "granted service works");
    let g = super::super::publish_env::granted(&svc_b, "ws-r8-a", dir.path());
    assert_eq!(env_of(&g, "API_TOKEN"), None, "another service is still refused");

    // A workspace grant reaches that workspace's services through `.env.k2`,
    // never a service in another workspace (vs-live A3).
    let name2 = uniq("R8_WS");
    let sel2 = put_server(&v, &name2, &value);
    env_k2(dir.path(), &[("WS_TOKEN", format!("k2v://server/{name2}"))]);
    grant(&v, &sel2, Consumer::workspace("ws-r8-a"), None);
    let g = super::super::publish_env::granted(&svc_b, "ws-r8-a", dir.path());
    assert_eq!(env_of(&g, "WS_TOKEN").as_deref(), Some(value.as_str()), "service in A via ws:A");
    let g = super::super::publish_env::granted(&svc_b, "ws-r8-b", dir.path());
    assert_eq!(env_of(&g, "WS_TOKEN"), None, "service in B, secret granted only to A");
}

/// Session env: an Owner/Admin-set reference (an agent preset's env) still
/// resolves; the same reference from env the agent can write (an AGENT.md
/// launch block, a `/v1` caller's prompt) is refused and removed.
#[test]
fn owner_set_session_refs_resolve_and_agent_written_ones_need_a_grant() {
    let v = crate::vault::ensure_test_vault();
    let ws = uuid::Uuid::new_v4().to_string();
    let name = uniq("R8_PRESET");
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let sel = put_server(&v, &name, &value);
    let r = format!("k2v://server/{name}");

    // Owner-set (preset env): resolves as before (K2's own path).
    let mut env = HashMap::from([("PRESET_KEY".to_string(), r.clone())]);
    let owner = env.clone();
    let out = crate::vault::spawn::materialize(&mut env, &ws, "s-owner", true, &owner);
    assert_eq!(env.get("PRESET_KEY").map(String::as_str), Some(value.as_str()), "{out:?}");

    // Agent-written (not in owner_env): refused, and the ref is removed.
    let mut env = HashMap::from([("PRESET_KEY".to_string(), r.clone())]);
    let out = crate::vault::spawn::materialize(&mut env, &ws, "s-agent", true, &HashMap::new());
    assert!(!env.contains_key("PRESET_KEY"), "refused ref is dropped: {env:?}");
    assert!(out.skipped.iter().any(|s| s == "PRESET_KEY: not_granted"), "{out:?}");

    // A different value under an owner-set key is not owner-set.
    let mut env = HashMap::from([("PRESET_KEY".to_string(), r.clone())]);
    let owner = HashMap::from([("PRESET_KEY".to_string(), "k2v://server/SOMETHING_ELSE".to_string())]);
    crate::vault::spawn::materialize(&mut env, &ws, "s-swap", true, &owner);
    assert!(!env.contains_key("PRESET_KEY"), "a swapped ref isn't the owner's: {env:?}");

    // Another workspace's secret through agent-written env: refused (it
    // used to resolve with no scope check at all).
    let other = uuid::Uuid::new_v4().to_string();
    let mut pr = PutRequest::new(Namespace::User, Scope::workspace(other.clone()), "OTHER_WS_KEY", Kind::Generic);
    pr.default_delivery = Some(Delivery::Fetch);
    v.put(pr, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put other ws");
    let other_sel = Selector::user(Scope::workspace(other.clone()), "OTHER_WS_KEY");
    let who = Requester::Session { workspace_id: ws.clone(), session_id: Some("s-x".into()) };
    assert_eq!(v.resolve_ref_env(&other_sel, &who).err().expect("refused").code, "not_granted");

    // Granted to the workspace: the agent-written ref resolves.
    grant(&v, &sel, Consumer::workspace(ws.clone()), Some("PRESET_KEY_GRANTED"));
    let mut env = HashMap::from([("PRESET_KEY".to_string(), r.clone())]);
    crate::vault::spawn::materialize(&mut env, &ws, "s-granted", true, &HashMap::new());
    assert_eq!(env.get("PRESET_KEY").map(String::as_str), Some(value.as_str()));
}

/// The upgrade: a reference the old rule resolved for a service that ran
/// lately is granted to that service (audited), so it keeps working; a
/// reference never used lately is refused and listed; a run twice changes
/// nothing more.
#[test]
fn the_upgrade_keeps_references_in_use_and_refuses_the_rest() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let dir_used = crate::test_support::Scratch::new();
    let dir_idle = crate::test_support::Scratch::new();
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let used = uniq("R8_USED");
    let idle = uniq("R8_IDLE");
    let sel_used = put_server(&v, &used, &value);
    let sel_idle = put_server(&v, &idle, &value);
    env_k2(dir_used.path(), &[("USED_TOKEN", format!("k2v://server/{used}"))]);
    env_k2(dir_idle.path(), &[("IDLE_TOKEN", format!("k2v://server/{idle}"))]);
    let now = chrono::Utc::now();
    let (svc_used, svc_idle) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let a = k2_core::published_services::insert(&conn, "ws-r8-up", "usedsvc", "node s.js", &dir_used.path().to_string_lossy(), 41901, "local", "running", "cmd", "")
            .expect("insert used");
        let b = k2_core::published_services::insert(&conn, "ws-r8-up", "idlesvc", "node s.js", &dir_idle.path().to_string_lossy(), 41902, "local", "stopped", "cmd", "")
            .expect("insert idle");
        k2_core::published_services::set_runtime(&conn, "ws-r8-up", "usedsvc", Some(4242), None, Some(now.timestamp() - 3600)).expect("started");
        k2_core::published_services::set_runtime(&conn, "ws-r8-up", "idlesvc", None, None, Some(now.timestamp() - 90 * 86_400)).expect("old start");
        (a.id, b.id)
    };
    // What the pre-R8 code did at the used service's last start.
    v.resolve(&sel_used, Use::System, &Requester::System("publish".into())).expect("old path resolve");
    v.flush_rollups();

    // Before the upgrade: the new rule refuses both.
    assert_eq!(env_of(&super::super::publish_env::granted(&svc_used, "ws-r8-up", dir_used.path()), "USED_TOKEN"), None);

    let report = run(&v, now);
    assert_eq!(report.granted.len(), 1, "{report:?}");
    assert_eq!(report.granted[0].consumer, "usedsvc");
    assert_eq!(report.granted[0].key, "USED_TOKEN");
    assert_eq!(report.granted[0].secret, used);
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(report.refused[0].consumer, "idlesvc");
    assert_eq!(report.refused[0].secret, idle);
    assert!(!format!("{report:?}").contains(&value), "names only, never values");
    assert!(!notice_body(&report).contains(&value), "the notice never holds a value");

    // The used service keeps working; the idle one stays refused.
    let g = super::super::publish_env::granted(&svc_used, "ws-r8-up", dir_used.path());
    assert_eq!(env_of(&g, "USED_TOKEN").as_deref(), Some(value.as_str()));
    let g = super::super::publish_env::granted(&svc_idle, "ws-r8-up", dir_idle.path());
    assert_eq!(env_of(&g, "IDLE_TOKEN"), None);

    // Never widened: the grant is to that one service, as env, under the
    // `.env.k2` name, made by the upgrade (audited).
    let grants = v.grants(&sel_used, &Actor::Owner).expect("grants");
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!(grants[0].consumer, Consumer::published_service(svc_used.clone()));
    assert_eq!(grants[0].delivery, Delivery::Env);
    assert_eq!(grants[0].env_name.as_deref(), Some("USED_TOKEN"));
    assert_eq!(grants[0].granted_by, "system:upgrade-r8");
    assert!(v.grants(&sel_idle, &Actor::Owner).expect("grants").is_empty(), "nothing granted for the idle one");
    let rows = v.audit(&k2_vault::AuditQuery { secret_id: Some(grants[0].secret_id.clone()), ..Default::default() }, &Actor::Owner).expect("audit");
    assert!(rows.iter().any(|r| r.action == "grant" && r.actor == "system:upgrade-r8" && r.outcome == "ok"), "{rows:?}");

    // Running again finds nothing new to grant.
    let again = run(&v, now);
    assert!(again.granted.is_empty(), "{again:?}");
    assert_eq!(again.refused.len(), 1, "the idle one is still listed");
}
