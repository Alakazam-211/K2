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

fn owner_refs(owner_env: HashMap<String, String>) -> crate::vault::spawn::SpawnRefs {
    crate::vault::spawn::SpawnRefs { owner_env, ..Default::default() }
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
    let out = crate::vault::spawn::materialize(&mut env, &ws, "s-owner", true, &owner_refs(owner));
    assert_eq!(env.get("PRESET_KEY").map(String::as_str), Some(value.as_str()), "{out:?}");

    // Agent-written (not in owner_env): refused, and the ref is removed.
    let mut env = HashMap::from([("PRESET_KEY".to_string(), r.clone())]);
    let out = crate::vault::spawn::materialize(&mut env, &ws, "s-agent", true, &crate::vault::spawn::SpawnRefs::default());
    assert!(!env.contains_key("PRESET_KEY"), "refused ref is dropped: {env:?}");
    assert!(out.skipped.iter().any(|s| s == "PRESET_KEY: not_granted"), "{out:?}");

    // A different value under an owner-set key is not owner-set.
    let mut env = HashMap::from([("PRESET_KEY".to_string(), r.clone())]);
    let owner = HashMap::from([("PRESET_KEY".to_string(), "k2v://server/SOMETHING_ELSE".to_string())]);
    crate::vault::spawn::materialize(&mut env, &ws, "s-swap", true, &owner_refs(owner));
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
    crate::vault::spawn::materialize(&mut env, &ws, "s-granted", true, &crate::vault::spawn::SpawnRefs::default());
    assert_eq!(env.get("PRESET_KEY").map(String::as_str), Some(value.as_str()));
}

/// Review fix 2 and 10: in a `/v1` cell every non-owner value is data. A
/// caller's prompt that looks like a reference stays as it is (never
/// resolved, never dropped), and what the cell did get is audited under the
/// `/v1` principal, apart from the workspace's own agent.
#[test]
fn a_v1_callers_prompt_is_data_and_cell_deliveries_carry_the_v1_label() {
    let v = crate::vault::ensure_test_vault();
    let ws = uuid::Uuid::new_v4().to_string();
    let name = uniq("R8_V1");
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let sel = put_server(&v, &name, &value);
    // Even a secret the cell's workspace IS granted stays unresolved.
    grant(&v, &sel, Consumer::workspace(ws.clone()), Some("GRANTED_FOR_WS"));
    let r = format!("k2v://server/{name}");
    let mut env = HashMap::from([("K2_REQUEST_PROMPT".to_string(), r.clone())]);
    let refs = crate::vault::spawn::SpawnRefs { others_are_data: true, via: Some("v1:key-abc".into()), ..Default::default() };
    let out = crate::vault::spawn::materialize(&mut env, &ws, "s-v1", true, &refs);
    assert_eq!(env.get("K2_REQUEST_PROMPT"), Some(&r), "the prompt is data, untouched: {out:?}");
    assert!(!out.skipped.iter().any(|s| s.starts_with("K2_REQUEST_PROMPT")), "{out:?}");
    // The cell still gets what its workspace is granted, and that delivery
    // is audited under the /v1 principal.
    assert_eq!(env.get("GRANTED_FOR_WS"), Some(&value));
    let m = v.meta(&sel, &Actor::Owner).expect("meta");
    let rows = v.audit(&k2_vault::AuditQuery { secret_id: Some(m.id.clone()), ..Default::default() }, &Actor::Owner).expect("audit");
    assert!(
        rows.iter().any(|r| r.action == "v1_cell_delivery" && r.actor == "system:v1:key-abc" && r.consumer.as_deref() == Some(&*format!("session:{ws}/s-v1"))),
        "{rows:?}"
    );
}

/// Review fix 10: an owner-set reference's audit row names the consumer
/// session, not just the subsystem.
#[test]
fn an_owner_set_resolve_is_audited_under_the_consumer_session() {
    let v = crate::vault::ensure_test_vault();
    let ws = uuid::Uuid::new_v4().to_string();
    let name = uniq("R8_OWNLBL");
    let sel = put_server(&v, &name, "k2test_ownlbl_value_1234567890");
    let mut env = HashMap::from([("PRESET_KEY".to_string(), format!("k2v://server/{name}"))]);
    let owner = env.clone();
    crate::vault::spawn::materialize(&mut env, &ws, "s-lbl", true, &owner_refs(owner));
    v.flush_rollups();
    let m = v.meta(&sel, &Actor::Owner).expect("meta");
    let rows = v.audit(&k2_vault::AuditQuery { secret_id: Some(m.id), ..Default::default() }, &Actor::Owner).expect("audit");
    assert!(rows.iter().any(|r| r.actor == format!("daemon:preset-env:{ws}/s-lbl") && r.outcome == "ok"), "{rows:?}");
}

/// Review fix 1: an own-workspace secret whose default delivery is fetch
/// never becomes raw env through a reference, from a service's `.env.k2`
/// or an agent-written launch env; one with env default delivery does.
#[test]
fn an_own_workspace_fetch_only_key_never_becomes_env_through_a_reference() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let ws = uuid::Uuid::new_v4().to_string();
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), "FETCH_ONLY", Kind::ApiKey);
    r.hosts = vec!["api.example.com".into()];
    r.default_delivery = Some(Delivery::Fetch);
    r.delivery_ceiling = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put");
    let mut r = PutRequest::new(Namespace::User, Scope::workspace(ws.clone()), "ENV_DEFAULT", Kind::Generic);
    r.default_delivery = Some(Delivery::Env);
    v.put(r, SecretBytes::from_slice(value.as_bytes()), &Actor::Owner).expect("put");
    let fetch_ref = format!("k2v://ws/{ws}/FETCH_ONLY");
    let env_ref = format!("k2v://ws/{ws}/ENV_DEFAULT");
    // Through a session's agent-written env.
    let who = Requester::Session { workspace_id: ws.clone(), session_id: Some("s-f".into()) };
    let fsel = Selector::user(Scope::workspace(ws.clone()), "FETCH_ONLY");
    assert_eq!(v.resolve_ref_env(&fsel, &who).err().expect("refused").code, "not_granted");
    assert!(v.resolve_ref_env(&Selector::user(Scope::workspace(ws.clone()), "ENV_DEFAULT"), &who).is_ok());
    // Through a service's .env.k2 (selector_for_ref needs the workspace
    // registered by id; the vault rule is what's tested, so check it as the
    // service would be checked).
    let svc = Requester::PublishedService { id: uuid::Uuid::new_v4().to_string(), workspace_id: ws.clone() };
    assert!(!v.ref_would_resolve(&fsel, &svc).expect("check"), "{fetch_ref}");
    assert!(v.ref_would_resolve(&Selector::user(Scope::workspace(ws.clone()), "ENV_DEFAULT"), &svc).expect("check"), "{env_ref}");
}

fn insert_service(ws: &str, name: &str, dir: &Path, desired: &str) -> String {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::insert(&conn, ws, name, "node s.js", &dir.to_string_lossy(), 41900 + (rand_port() % 90), "local", desired, "cmd", "")
        .expect("insert service")
        .id
}

fn rand_port() -> u16 {
    (uuid::Uuid::new_v4().as_u128() % 1000) as u16
}

fn set_started(ws: &str, name: &str, pid: Option<i64>, started: i64) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::published_services::set_runtime(&conn, ws, name, pid, None, Some(started)).expect("set runtime");
}

fn old_path_resolve(v: &k2_vault::LocalVault, sel: &Selector) {
    v.resolve(sel, Use::System, &Requester::System("publish".into())).expect("old path resolve");
    v.flush_rollups();
}

/// The upgrade's per-service evidence (review fix 5/6) and the time box:
/// - the only service naming a secret, wanted running, `.env.k2` unchanged
///   since its start, audited use: granted to it, as env, for 14 days;
/// - a secret two services name: neither (whose use was it?);
/// - a `.env.k2` changed after the last start: refused;
/// - a stopped service not started lately: refused;
/// - no values anywhere; a second run grants nothing new.
#[test]
fn the_upgrade_grants_only_with_per_service_evidence_and_time_boxes_it() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let (used, shared, edited, idle) = (uniq("R8_USED"), uniq("R8_SHARED"), uniq("R8_EDITED"), uniq("R8_IDLE"));
    let sels: Vec<Selector> = [&used, &shared, &edited, &idle].iter().map(|n| put_server(&v, n, &value)).collect();
    let (d_used, d_shared2, d_edited, d_idle) =
        (crate::test_support::Scratch::new(), crate::test_support::Scratch::new(), crate::test_support::Scratch::new(), crate::test_support::Scratch::new());
    env_k2(d_used.path(), &[("USED_TOKEN", format!("k2v://server/{used}")), ("SHARED_TOKEN", format!("k2v://server/{shared}"))]);
    env_k2(d_shared2.path(), &[("SHARED_TOKEN", format!("k2v://server/{shared}"))]);
    env_k2(d_edited.path(), &[("EDITED_TOKEN", format!("k2v://server/{edited}"))]);
    env_k2(d_idle.path(), &[("IDLE_TOKEN", format!("k2v://server/{idle}"))]);
    let ws = "ws-r8-up";
    let s_used = insert_service(ws, "usedsvc", d_used.path(), "running");
    let s_shared2 = insert_service(ws, "shared2svc", d_shared2.path(), "running");
    let s_edited = insert_service(ws, "editedsvc", d_edited.path(), "running");
    let s_idle = insert_service(ws, "idlesvc", d_idle.path(), "stopped");
    let now = chrono::Utc::now();
    let t = now.timestamp();
    set_started(ws, "usedsvc", None, t + 1);
    set_started(ws, "shared2svc", None, t + 1);
    set_started(ws, "editedsvc", None, t - 3600); // .env.k2 written after this start
    set_started(ws, "idlesvc", None, t - 90 * 86_400);
    for s in &sels {
        old_path_resolve(&v, s);
    }

    let report = run(&v, now);
    let granted: Vec<(&str, &str)> = report.granted.iter().map(|l| (l.service_id.as_str(), l.key.as_str())).collect();
    assert_eq!(granted, vec![(s_used.as_str(), "USED_TOKEN")], "{report:#?}");
    let refused = |sid: &str, key: &str| report.refused.iter().find(|l| l.service_id == sid && l.key == key).map(|l| l.reason.clone());
    assert!(refused(&s_used, "SHARED_TOKEN").expect("shared refused for one").contains("2 services name it"), "{report:#?}");
    assert!(refused(&s_shared2, "SHARED_TOKEN").expect("shared refused for two").contains("2 services name it"));
    assert!(refused(&s_edited, "EDITED_TOKEN").expect("edited refused").contains("changed after it last started"));
    assert!(refused(&s_idle, "IDLE_TOKEN").expect("idle refused").contains("not running"));
    assert!(report.unreadable.is_empty(), "{report:#?}");
    let all = format!("{report:?}{}", notice_body(&report));
    assert!(!all.contains(&value), "names only, never values");
    assert!(notice_body(&report).contains(&format!("service {s_used}")), "the notice names the service id");

    // The used service keeps working, for UPGRADE_GRANT_DAYS only.
    let g = super::super::publish_env::granted(&s_used, ws, d_used.path());
    assert_eq!(env_of(&g, "USED_TOKEN").as_deref(), Some(value.as_str()));
    let grants = v.grants(&sels[0], &Actor::Owner).expect("grants");
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!(grants[0].consumer, Consumer::published_service(s_used.clone()));
    assert_eq!(grants[0].delivery, Delivery::Env);
    assert_eq!(grants[0].env_name.as_deref(), Some("USED_TOKEN"));
    assert_eq!(grants[0].granted_by, "system:upgrade-r8");
    let exp = chrono::DateTime::parse_from_rfc3339(grants[0].expires_at.as_deref().expect("time-boxed")).expect("rfc3339");
    assert_eq!((exp.timestamp() - now.timestamp()) / 86_400, UPGRADE_GRANT_DAYS, "expires in {UPGRADE_GRANT_DAYS} days");
    for s in &sels[1..] {
        assert!(v.grants(s, &Actor::Owner).expect("grants").is_empty(), "nothing else granted");
    }
    // Refused references show in the service's status after its start.
    let g = super::super::publish_env::granted(&s_idle, ws, d_idle.path());
    assert_eq!(env_of(&g, "IDLE_TOKEN"), None);
    assert_eq!(super::super::publish_env::refused_refs(&s_idle), vec!["IDLE_TOKEN: not_granted".to_string()]);

    let again = run(&v, now);
    assert!(again.granted.is_empty(), "a second run grants nothing new: {again:#?}");
}

/// Review fix 5 + round 2 fix 2 (strong evidence): a running service whose
/// process provably is that start (its start time matches) and whose
/// environment holds the value is granted, even when other services name
/// the same secret. A live pid that isn't that start (a reboot reused it),
/// or a process whose environment lacks the value (environ rewritten for a
/// title), proves nothing either way: it falls back to the audit path.
#[test]
fn a_running_service_is_judged_by_its_own_environment() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let name = uniq("R8_LIVE");
    put_server(&v, &name, &value);
    let (d_has, d_hasnt, d_stale) = (crate::test_support::Scratch::new(), crate::test_support::Scratch::new(), crate::test_support::Scratch::new());
    for d in [&d_has, &d_hasnt, &d_stale] {
        env_k2(d.path(), &[("LIVE_TOKEN", format!("k2v://server/{name}"))]);
    }
    let spawned_at = chrono::Utc::now().timestamp();
    let mut has = std::process::Command::new("sleep").arg("60").env("LIVE_TOKEN", &value).spawn().expect("spawn child with env");
    let mut hasnt = std::process::Command::new("sleep").arg("60").env_remove("LIVE_TOKEN").spawn().expect("spawn child without env");
    let mut stale = std::process::Command::new("sleep").arg("60").env("LIVE_TOKEN", &value).spawn().expect("spawn stale child");
    let ws = "ws-r8-live";
    let s_has = insert_service(ws, "hassvc", d_has.path(), "running");
    let s_hasnt = insert_service(ws, "hasntsvc", d_hasnt.path(), "running");
    let s_stale = insert_service(ws, "stalesvc", d_stale.path(), "running");
    set_started(ws, "hassvc", Some(has.id() as i64), spawned_at);
    set_started(ws, "hasntsvc", Some(hasnt.id() as i64), spawned_at);
    // Recorded an hour before this process started: a different process.
    set_started(ws, "stalesvc", Some(stale.id() as i64), spawned_at - 3600);
    assert!(pid_started_near(has.id() as i64, spawned_at), "the child's start time is read");
    assert!(!pid_started_near(stale.id() as i64, spawned_at - 3600), "an hour off is another start");
    let report = run(&v, chrono::Utc::now());
    for c in [&mut has, &mut hasnt, &mut stale] {
        let _ = c.kill();
        let _ = c.wait();
    }
    assert_eq!(report.granted.len(), 1, "{report:#?}");
    assert_eq!(report.granted[0].service_id, s_has);
    assert!(report.granted[0].reason.contains("running service has it"), "{report:#?}");
    for sid in [&s_hasnt, &s_stale] {
        let r = report.refused.iter().find(|l| &l.service_id == sid).expect("refused");
        assert!(r.reason.contains("3 services name it"), "fell back to the audit path: {report:#?}");
    }
    assert!(!format!("{report:?}").contains(&value));
}

#[test]
fn etime_parses_every_ps_shape() {
    assert_eq!(parse_etime("00:07"), Some(7));
    assert_eq!(parse_etime("12:34"), Some(754));
    assert_eq!(parse_etime("01:02:03"), Some(3723));
    assert_eq!(parse_etime("2-01:02:03"), Some(2 * 86_400 + 3723));
    assert_eq!(parse_etime("junk"), None);
    assert_eq!(parse_etime(""), None);
}

/// Review fix 4 and 6: the done flag lives in the vault's audit chain, so
/// deleting the report file doesn't re-run the pass; a pass that couldn't
/// read a service's `.env.k2` doesn't set it and runs again.
#[test]
fn the_done_flag_is_in_the_vault_and_a_partial_pass_runs_again() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    // Its own vault: the done flag is per vault.
    let dir = k2_vault::fsutil::TestDir::new("r8-done");
    let ks: std::sync::Arc<dyn k2_vault::KeyStore> = std::sync::Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
    let v = k2_vault::LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
        .expect("open vault");
    // A service whose env file is unreadable (a directory where the file
    // should be): the pass is partial, no done flag.
    let broken = crate::test_support::Scratch::new();
    std::fs::create_dir_all(broken.path().join(".env.k2")).expect("dir in place of the file");
    insert_service("ws-r8-done", "brokensvc", broken.path(), "running");
    NOTICES.with(|n| n.set(0));
    upgrade_once(&v);
    assert!(!done(&v), "a partial pass doesn't mark itself done");
    assert_eq!(NOTICES.with(|n| n.get()), 1, "the first pass tells the owner what it couldn't read");
    // Still broken next boot: a rerun, no new decision, no new notice.
    upgrade_once(&v);
    assert!(!done(&v));
    assert_eq!(NOTICES.with(|n| n.get()), 1, "a rerun that decided nothing doesn't nag");
    let report: Report = serde_json::from_slice(&std::fs::read(report_path()).expect("report")).expect("report json");
    assert_eq!(report.unreadable.len(), 1, "{report:#?}");
    // Fixed: the next boot completes and marks it done in the audit chain.
    std::fs::remove_dir_all(broken.path().join(".env.k2")).expect("fix");
    upgrade_once(&v);
    assert!(done(&v), "a complete pass is marked done");
    // Deleting the report file doesn't bring the pass back: a reference
    // added now is never granted by the upgrade.
    std::fs::remove_file(report_path()).expect("delete report");
    let name = uniq("R8_LATE");
    let sel = put_server(&v, &name, "k2test_late_value_123456789012");
    let late = crate::test_support::Scratch::new();
    env_k2(late.path(), &[("LATE_TOKEN", format!("k2v://server/{name}"))]);
    insert_service("ws-r8-done", "latesvc", late.path(), "running");
    set_started("ws-r8-done", "latesvc", None, chrono::Utc::now().timestamp() + 1);
    old_path_resolve(&v, &sel);
    upgrade_once(&v);
    assert!(v.grants(&sel, &Actor::Owner).expect("grants").is_empty(), "the pass ran once only");
    assert!(!report_path().exists(), "and didn't run again");
}

/// The keep card: "Keep all" by the owner makes the grants permanent (as
/// the owner); a row keeps one; a Member login keeps nothing; "Let them
/// lapse" changes nothing.
#[test]
fn the_keep_card_keeps_upgrade_grants_only_for_the_owner() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let svc = uuid::Uuid::new_v4().to_string();
    let mut rows = vec![];
    for key in ["KEEP_A", "KEEP_B"] {
        let name = uniq(key);
        let sel = put_server(&v, &name, "k2test_keep_value_1234567890ab");
        let g = v
            .grant(
                GrantRequest {
                    selector: sel.clone(),
                    consumer: Consumer::published_service(svc.clone()),
                    delivery: Delivery::Env,
                    env_name: Some(key.into()),
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: Some((chrono::Utc::now() + chrono::Duration::days(UPGRADE_GRANT_DAYS)).to_rfc3339()),
                    version: None,
                },
                &Actor::System("upgrade-r8".into()),
            )
            .expect("upgrade grant");
        rows.push(CardRow { grant_id: g.id, secret_id: g.secret_id, service_id: svc.clone(), key: key.into(), secret: name });
    }
    let params = serde_json::json!({ "workspaceId": "ws-keep", "rows": rows, "expiresAt": 0 });
    let (_, options) = card_for(&rows);
    assert_eq!(options.first().map(String::as_str), Some(KEEP_ALL));
    assert_eq!(options.last().map(String::as_str), Some(LET_LAPSE));
    assert!(options.contains(&row_label(&rows[0])) && options.contains(&row_label(&rows[1])));
    let expiry = |key: &str| {
        v.grants_to(&Consumer::published_service(svc.clone()))
            .expect("grants")
            .into_iter()
            .find(|(g, _)| g.env_name.as_deref() == Some(key))
            .map(|(g, _)| g.expires_at)
            .expect("grant present")
    };
    // A login that isn't the owner (here one K2 doesn't know): refused.
    let e = keep_action(&params, KEEP_ALL, "no-such-login-r8").expect_err("not the owner");
    assert!(!e.is_empty());
    assert!(expiry("KEEP_A").is_some() && expiry("KEEP_B").is_some());
    // Let them lapse: nothing kept.
    let r = keep_action(&params, LET_LAPSE, "owner").expect("lapse");
    assert_eq!(r["kept"], 0);
    assert!(expiry("KEEP_A").is_some());
    // Keep one row: that one is permanent, the other still time-boxed.
    let r = keep_action(&params, &row_label(&rows[0]), "owner").expect("keep one");
    assert_eq!(r["kept"], 1, "{r}");
    assert_eq!(expiry("KEEP_A"), None, "kept for good");
    assert!(expiry("KEEP_B").is_some(), "the other still lapses");
    // Keep all: the rest too (the kept one is "no longer there" by id: it
    // was re-made; the message says so, nothing is widened).
    let r = keep_action(&params, KEEP_ALL, "owner").expect("keep all");
    assert_eq!(expiry("KEEP_B"), None, "{r}");
    assert_eq!(r["kept"], 1, "{r}");
    assert!(r["message"].as_str().expect("message").contains("no longer there"), "{r}");
    // Kept grants are the owner's, never wider than the upgrade's.
    for (g, _) in v.grants_to(&Consumer::published_service(svc.clone())).expect("grants") {
        assert_eq!(g.granted_by, "owner");
        assert_eq!(g.delivery, Delivery::Env);
    }
}

fn own_vault(tag: &str) -> (k2_vault::fsutil::TestDir, k2_vault::LocalVault) {
    let dir = k2_vault::fsutil::TestDir::new(tag);
    let ks: std::sync::Arc<dyn k2_vault::KeyStore> = std::sync::Arc::new(k2_vault::keystore::fake::FakeKeyStore::new());
    let v = k2_vault::LocalVault::open(dir.path().join("vault"), ks, k2_vault::OpenOptions { kdf: k2_vault::confirm::KdfCost::Floor })
        .expect("open vault");
    (dir, v)
}

fn grant_of(v: &k2_vault::LocalVault, sel: &Selector) -> Option<k2_vault::Grant> {
    v.grants(sel, &Actor::Owner).expect("grants").into_iter().next()
}

/// Round 2 fix 1: an agent keeping one of its services' `.env.k2`
/// unreadable can't make every boot re-scan and renew grants. A rerun reads
/// only the services that were unreadable, never renews a grant the first
/// pass made, accepts only live-env evidence (no audit window, no mtime),
/// and tells the owner only about new decisions.
#[test]
fn a_rerun_reads_only_unreadable_services_and_never_renews() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let (_d, v) = own_vault("r8-rerun");
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let (used, later) = (uniq("R8_RR_USED"), uniq("R8_RR_LATER"));
    let sel_used = put_server(&v, &used, &value);
    let sel_later = put_server(&v, &later, &value);
    let (d_used, d_broken) = (crate::test_support::Scratch::new(), crate::test_support::Scratch::new());
    env_k2(d_used.path(), &[("USED_TOKEN", format!("k2v://server/{used}"))]);
    std::fs::create_dir_all(d_broken.path().join(".env.k2")).expect("dir in place of the file");
    let ws = "ws-r8-rr";
    let s_used = insert_service(ws, "usedsvc", d_used.path(), "running");
    insert_service(ws, "brokensvc", d_broken.path(), "running");
    let t = chrono::Utc::now().timestamp();
    set_started(ws, "usedsvc", None, t + 1);
    set_started(ws, "brokensvc", None, t + 1);
    old_path_resolve(&v, &sel_used);
    old_path_resolve(&v, &sel_later);
    NOTICES.with(|n| n.set(0));

    upgrade_once(&v);
    let first = grant_of(&v, &sel_used).expect("first pass grants the used one");
    assert_eq!(first.consumer, Consumer::published_service(s_used.clone()));
    assert!(!done(&v));
    assert_eq!(NOTICES.with(|n| n.get()), 1);

    // Boots while it stays unreadable: nothing renewed, no notice.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    upgrade_once(&v);
    upgrade_once(&v);
    let again = grant_of(&v, &sel_used).expect("still there");
    assert_eq!((again.id.clone(), again.expires_at.clone()), (first.id.clone(), first.expires_at.clone()), "never renewed");
    assert_eq!(NOTICES.with(|n| n.get()), 1, "no notice for a rerun that decided nothing");

    // Readable now, naming a secret with audit-only evidence: a rerun
    // refuses it (it accepts only a live environment), and says so once.
    std::fs::remove_dir_all(d_broken.path().join(".env.k2")).expect("fix");
    env_k2(d_broken.path(), &[("LATER_TOKEN", format!("k2v://server/{later}"))]);
    upgrade_once(&v);
    assert!(grant_of(&v, &sel_later).is_none(), "a rerun never grants on audit evidence");
    assert!(done(&v), "every service read: done");
    assert_eq!(NOTICES.with(|n| n.get()), 2, "the new refusal is told once");
    let again = grant_of(&v, &sel_used).expect("still there");
    assert_eq!(again.expires_at, first.expires_at, "the first grant's time box is unchanged");
}

/// Round 2 fix 1: a pair that had an upgrade grant (since revoked or
/// lapsed) is never granted again by a later pass.
#[test]
fn a_pair_that_had_an_upgrade_grant_is_never_granted_again() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let (_d, v) = own_vault("r8-had");
    let value = format!("k2test_{}", uuid::Uuid::new_v4().simple());
    let name = uniq("R8_HAD");
    let sel = put_server(&v, &name, &value);
    let dir = crate::test_support::Scratch::new();
    env_k2(dir.path(), &[("HAD_TOKEN", format!("k2v://server/{name}"))]);
    let ws = "ws-r8-had";
    let sid = insert_service(ws, "hadsvc", dir.path(), "running");
    set_started(ws, "hadsvc", None, chrono::Utc::now().timestamp() + 1);
    old_path_resolve(&v, &sel);
    let r = run(&v, chrono::Utc::now());
    assert_eq!(r.granted.len(), 1, "{r:#?}");
    let g = grant_of(&v, &sel).expect("granted");
    v.revoke(&g.id, &Actor::Owner).expect("owner revokes it");
    let only: std::collections::BTreeSet<String> = [sid.clone()].into_iter().collect();
    let r = run_scoped(&v, chrono::Utc::now(), &PassScope { only: Some(only), decided: Default::default() });
    assert!(r.granted.is_empty() && grant_of(&v, &sel).is_none(), "never again: {r:#?}");
    let r = run(&v, chrono::Utc::now());
    assert!(r.granted.is_empty() && grant_of(&v, &sel).is_none(), "not by a full pass either: {r:#?}");
}

/// Round 2 fix 5: when the list of services can't be read the pass isn't
/// done, and the owner is told.
#[test]
fn an_unreadable_service_list_is_not_a_complete_pass() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let (_d, v) = own_vault("r8-sqlerr");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute_batch("DROP TABLE apps;").expect("drop apps");
    }
    NOTICES.with(|n| n.set(0));
    upgrade_once(&v);
    assert!(!done(&v), "no done flag after a SQL error");
    assert_eq!(NOTICES.with(|n| n.get()), 1, "and a notice");
    let report: Report = serde_json::from_slice(&std::fs::read(report_path()).expect("report")).expect("json");
    assert_eq!(report.unreadable_ids, vec!["*".to_string()], "{report:#?}");
}

/// Round 2 fix 6: a row written straight into `vault.db` (no audit key)
/// fails the chain check: a forged done flag isn't believed, and the pass
/// makes no grants on a chain it can't verify (fail closed).
#[test]
fn a_forged_audit_row_is_not_believed() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let (d, v) = own_vault("r8-forge");
    let name = uniq("R8_FORGE");
    let sel = put_server(&v, &name, "k2test_forge_value_12345678901234");
    let dir = crate::test_support::Scratch::new();
    env_k2(dir.path(), &[("FORGE_TOKEN", format!("k2v://server/{name}"))]);
    insert_service("ws-r8-forge", "forgesvc", dir.path(), "running");
    set_started("ws-r8-forge", "forgesvc", None, chrono::Utc::now().timestamp() + 1);
    old_path_resolve(&v, &sel);
    {
        let raw = rusqlite::Connection::open(d.path().join("vault").join("vault.db")).expect("open vault.db directly");
        let seq: i64 = raw.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM audit", [], |r| r.get(0)).expect("seq");
        raw.execute(
            "INSERT INTO audit (seq, ts, action, actor, outcome, count, prev_mac, row_mac) VALUES (?1, '2026-01-01T00:00:00Z', ?2, 'system:upgrade-r8', 'ok', 1, x'00', x'00')",
            rusqlite::params![seq, DONE_ACTION],
        )
        .expect("forge a done row");
    }
    assert!(v.verify_audit_chain().is_err(), "the chain check catches it");
    assert!(!done(&v), "a forged done flag isn't believed");
    upgrade_once(&v);
    assert!(grant_of(&v, &sel).is_none(), "no grants on a chain that doesn't verify");
}

/// Round 2 fix 4: the card only makes a time-boxed upgrade grant
/// permanent, never some other grant, and keeps a version pin.
#[test]
fn the_keep_card_only_keeps_upgrade_grants_and_keeps_the_pin() {
    let _home = crate::test_support::TempHome::new();
    let _db = k2_core::db::scoped_for_test();
    let v = crate::vault::ensure_test_vault();
    let svc = uuid::Uuid::new_v4().to_string();
    let mk = |key: &str, by: &Actor, expires: bool, version: Option<u32>| {
        let name = uniq(key);
        let sel = put_server(&v, &name, "k2test_pin_value_1234567890abcd");
        if version.is_some() {
            v.rotate(&sel, SecretBytes::from_slice(b"k2test_pin_value_second_000000"), &Actor::Owner).expect("rotate");
        }
        let g = v
            .grant(
                GrantRequest {
                    selector: sel.clone(),
                    consumer: Consumer::published_service(svc.clone()),
                    delivery: Delivery::Env,
                    env_name: Some(key.into()),
                    file_name: None,
                    hosts_narrowed: None,
                    expires_at: expires.then(|| (chrono::Utc::now() + chrono::Duration::days(UPGRADE_GRANT_DAYS)).to_rfc3339()),
                    version,
                },
                by,
            )
            .expect("grant");
        CardRow { grant_id: g.id, secret_id: g.secret_id, service_id: svc.clone(), key: key.into(), secret: name }
    };
    let up = Actor::System("upgrade-r8".into());
    let owners = mk("OWNERS_OWN", &Actor::Owner, true, None);
    let pinned = mk("PINNED_UP", &up, true, Some(1));
    let params = serde_json::json!({ "workspaceId": "ws-pin", "rows": [owners.clone(), pinned.clone()], "expiresAt": 0 });
    let r = keep_action(&params, KEEP_ALL, "owner").expect("keep all");
    assert_eq!(r["kept"], 1, "{r}");
    assert!(r["message"].as_str().expect("message").contains("isn't a time-boxed upgrade grant"), "{r}");
    let grants = v.grants_to(&Consumer::published_service(svc.clone())).expect("grants");
    let own = grants.iter().find(|(g, _)| g.env_name.as_deref() == Some("OWNERS_OWN")).expect("owner's grant").0.clone();
    assert!(own.expires_at.is_some(), "an owner's own grant is never touched by the card");
    let kept = grants.iter().find(|(g, _)| g.env_name.as_deref() == Some("PINNED_UP")).expect("kept").0.clone();
    assert_eq!(kept.expires_at, None);
    assert_eq!(kept.version_selector, "1", "the pin is kept");
    assert_eq!(kept.granted_by, "owner");
}
