//! S27 on a real Sew lifecycle (0.46.1): the supervisor over fake ops (the
//! helper fake answers `engine sew token`) with the REAL grant hook
//! (`tool_hook` in a temp `skin.db`), the real delivery worker, and the
//! fake Sew mgmt socket from `tests/support`.
//!
//! - enable → the chat target points at Sew → `k2:owner` and a pending
//!   chat grant deliver;
//! - the worker's token comes from the helper (cached in memory, refreshed
//!   once on a 401), never from a token file: the target names Sew's own
//!   root-only file, and the fake's file is deleted before any delivery;
//! - uninstall with --drop-grants → the target is off, the grant is gone,
//!   its open row is skipped and never retried, even after a new enable.

#![cfg(unix)]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

#[path = "../../tests/support/fake_sew.rs"]
mod fake_sew;

use super::ops::fake::{FakeOps, TOKEN};
use super::secrets::MemSecrets;
use super::supervisor::{self as sup, EnableRequest, Env};
use crate::tool_hook_worker::{self as worker, WorkerConfig};
use k2_core::skin::{self, SkinGrantWrite};
use k2_core::tool_hook::{self as hook, CHAT_KIND};
use k2_root_proto::sew as pin;

const TARBALL: &str = "/home/k2/sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz";
const ROTATED: &str = "sewsvc_ZyXwVuTsRqPoNmLkJiHgFeDcBa9876543210_-ZyXwV";

fn cfg() -> WorkerConfig {
    WorkerConfig {
        tick: Duration::from_millis(50),
        backoff_base: Duration::from_millis(20),
        backoff_cap: Duration::from_millis(200),
        reconcile_every: Duration::from_secs(3600),
        reconcile_debounce: Duration::from_millis(10),
        dep_wait: Duration::from_secs(3600),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew").join(name);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn pext(id: &str) -> String {
    format!("k2:principal:{id}")
}

fn chat(username: &str) {
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: username.into(),
        kind: "chat".into(),
        target_id: String::new(),
        role: None,
        scope: None,
        created_by: Some("owner-token".into()),
    })
    .expect("chat grant");
}

fn rows_for(subject: &str) -> Vec<k2_core::tool_hook::outbox::OutboxRow> {
    hook::log(500, Some(subject)).expect("outbox log")
}

fn token_reads(ops: &FakeOps) -> usize {
    ops.effects().iter().filter(|e| *e == "engine sew token").count()
}

async fn drain(sew: &fake_sew::FakeSew) {
    for _ in 0..40 {
        hook::force_due_for_tests().unwrap();
        worker::pass_once(&cfg()).await.expect("pass");
        if hook::log(500, None).unwrap().iter().all(|r| r.state != "pending") {
            return;
        }
    }
    panic!("outbox never drained:\n{}", sew.log());
}

fn enable(env: &Env<'static>) {
    let req = EnableRequest { tarball_path: Some(TARBALL.into()), recovery_key_file: None, ..Default::default() };
    let latch = sup::enable_begin(env, &req).expect("begin").expect("a machine");
    sup::enable_machine(env, &req).expect("enable");
    drop(latch);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_points_s27_at_sew_and_uninstall_drops_it_with_the_helper_token() {
    let home = k2_core::test_env::TempHome::short();
    let _db = k2_core::db::scoped_for_test();
    hook::allow_target_off_linux_for_tests(true);
    let mut sew = fake_sew::FakeSew::start(&home.path().join("s")).await;
    sew.set_token(TOKEN);

    let mut ops = FakeOps::new();
    ops.real_hook = true;
    *ops.mgmt_socket.lock().unwrap() = sew.socket.display().to_string();
    ops.files.lock().unwrap().insert(TARBALL.into(), fixture("sew-0.0.2-x86_64-unknown-linux-gnu.tar.gz"));
    let ops: &'static FakeOps = Box::leak(Box::new(ops));
    let secrets: &'static MemSecrets = Box::leak(Box::new(MemSecrets::default()));
    let latch: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
    let token: &'static Mutex<Option<String>> = Box::leak(Box::new(Mutex::new(None)));
    let env: Env<'static> = Env { ops, secrets, latch, token };
    crate::sew_mgmt::set_token_provider(Some(Arc::new(move |refresh| sup::mgmt_token(&env, refresh))));

    // ── enable → target on → owner + a pending grant deliver ────────────
    let ann = skin::add_principal("ann").unwrap();
    enable(&env);
    let t = hook::get_target(CHAT_KIND).unwrap().expect("the chat target");
    assert!(t.enabled);
    assert_eq!(t.socket, sew.socket.display().to_string());
    assert_eq!(t.token_file, pin::SEW_MGMT_TOKEN, "Sew's own file: the worker asks the helper");
    chat("ann");
    // No file the worker could read: the fake's is gone, Sew's is root's.
    std::fs::remove_file(&sew.token_file).unwrap();
    assert!(!std::path::Path::new(pin::SEW_MGMT_TOKEN).exists() || std::fs::read(pin::SEW_MGMT_TOKEN).is_err());
    *token.lock().unwrap() = None;
    ops.clear();
    drain(&sew).await;
    assert_eq!(sew.owner().as_deref(), Some("k2:owner"), "{}", sew.log());
    let m = sew.member(&pext(&ann.id)).expect("ann is a Sew member");
    assert!(!m.deactivated);
    assert_eq!(token_reads(ops), 1, "one helper read, then the in-memory cache");
    assert!(rows_for(&pext(&ann.id)).iter().all(|r| r.state == "done"));

    // ── Sew rotated its token: one 401, one helper refresh, delivered ───
    sew.set_token(ROTATED);
    ops.queue("engine sew token", Ok(json!({"token": ROTATED})));
    let bo = skin::add_principal("bo").unwrap();
    chat("bo");
    drain(&sew).await;
    assert!(sew.member(&pext(&bo.id)).is_some(), "{}", sew.log());
    assert_eq!(token_reads(ops), 2, "refreshed once after the 401");

    // ── uninstall --drop-grants → off, grant gone, row never retried ────
    sew.down();
    let cy = skin::add_principal("cy").unwrap();
    chat("cy");
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.expect("pass");
    let open: Vec<_> = rows_for(&pext(&cy.id)).into_iter().filter(|r| r.state == "pending").collect();
    assert_eq!(open.len(), 1, "Sew down: cy's provision waits");
    assert!(open[0].attempts >= 1);
    sew.up();
    let r = sup::uninstall(&env, true).expect("uninstall");
    assert_eq!(r["grants"], "dropped");
    assert_eq!(r["grantHook"]["target"], "off", "{r}");
    assert_eq!(r["grantHook"]["dropped"]["grantsDeleted"], 3, "{r}");
    assert!(!hook::get_target(CHAT_KIND).unwrap().unwrap().enabled);
    assert!(skin::list_grants().unwrap().iter().all(|g| g.kind != "chat"));
    assert!(rows_for(&pext(&cy.id)).iter().all(|r| r.state == "skipped"), "{:?}", rows_for(&pext(&cy.id)));
    let posts_before = sew.posts().len();
    hook::force_due_for_tests().unwrap();
    worker::pass_once(&cfg()).await.expect("pass");
    assert_eq!(sew.posts().len(), posts_before, "nothing is sent for a dropped target");
    // A new enable turns the hook back on; cy's skipped row stays skipped.
    // (The fake ops' own mgmt answers only the first token: rotate back.)
    sew.set_token(TOKEN);
    *token.lock().unwrap() = None;
    enable(&env);
    assert!(hook::get_target(CHAT_KIND).unwrap().unwrap().enabled);
    drain(&sew).await;
    assert!(sew.member(&pext(&cy.id)).is_none(), "{}", sew.log());
    assert!(sew.posts().iter().all(|c| c.body["external_id"] != pext(&cy.id)));
    sew.assert_clean();
    crate::sew_mgmt::set_token_provider(None);
}

fn app_grant(principal_id: &str, app: &str) -> skin::SkinGrant {
    skin::create_grant(&SkinGrantWrite {
        subject_kind: "principal".into(),
        subject_id: principal_id.into(),
        kind: "app".into(),
        target_id: app.into(),
        role: None,
        scope: Some("[]".into()),
        created_by: Some("owner-token".into()),
    })
    .expect("app grant")
}

/// K31 through the REAL worker against the fake Sew's SW13 routes: App
/// users (App grant + chat member) join the App's Space after their own
/// provision; a banned one is held (HC2); Connect Owners/Admins with their
/// own Sew accounts and `k2:owner` are never added; losing the App grant
/// removes the person.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn k31_app_users_join_the_apps_space_and_connect_admins_never_do() {
    const SPACE: &str = "spc_hearth01";
    let home = k2_core::test_env::TempHome::short();
    let _db = k2_core::db::scoped_for_test();
    hook::allow_target_off_linux_for_tests(true);
    let sew = fake_sew::FakeSew::start(&home.path().join("s")).await;
    hook::set_target(
        CHAT_KIND,
        true,
        Some(&sew.socket.display().to_string()),
        Some(&sew.token_file.display().to_string()),
        "owner-token",
    )
    .expect("target");
    sew.add_app_space(SPACE, "hearth");

    // Connect Owner + Admin, each with their own Sew account (a `connect`
    // chat grant): server control, never App Space members by role.
    let mut connect_exts = Vec::new();
    for (login, role) in [("boss", k2_core::connect_users::Role::Owner), ("adm", k2_core::connect_users::Role::Admin)] {
        k2_core::connect_users::add_user(login, "pw-long-enough").unwrap();
        k2_core::connect_users::set_role(login, role).unwrap();
        let id = k2_core::connect_users::user_uuid(login).unwrap().unwrap();
        skin::create_grant(&SkinGrantWrite {
            subject_kind: "connect".into(),
            subject_id: id.clone(),
            kind: "chat".into(),
            target_id: String::new(),
            role: None,
            scope: Some("member".into()),
            created_by: Some("owner-token".into()),
        })
        .expect("connect chat grant");
        connect_exts.push(format!("k2:connect:{id}"));
    }
    let ann = skin::add_principal("ann").unwrap();
    let bea = skin::add_principal("bea").unwrap();
    let ann_app = app_grant(&ann.id, "hearth");
    app_grant(&bea.id, "hearth");
    chat("ann");
    chat("bea");
    sew.ban_in_space(SPACE, &pext(&bea.id));

    let (created, plan) = hook::app_space_bind("hearth", SPACE, "owner-token").expect("bind");
    assert!(created);
    assert_eq!(plan.adds, 2, "{plan:?}");
    drain(&sew).await;
    let sp = sew.space(SPACE).expect("space");
    assert_eq!(sp.members.iter().cloned().collect::<Vec<_>>(), vec![pext(&ann.id)], "{}", sew.log());
    for ext in &connect_exts {
        assert!(sew.member(ext).is_some(), "{ext} has its own Sew account");
        assert!(!sp.members.contains(ext), "{ext} is never an App Space member by role");
    }
    assert!(!sp.members.contains("k2:owner"));
    let bea_rows = rows_for(&format!("space:{SPACE}:{}", pext(&bea.id)));
    assert_eq!(bea_rows.len(), 1);
    assert_eq!(bea_rows[0].state, "held", "{:?}", bea_rows[0]);
    let views = hook::app_spaces().unwrap();
    assert_eq!((views[0].members.added, views[0].members.held), (1, 1));
    // No App Space call ever named a Connect login or the owner.
    for c in sew.posts().iter().filter(|c| c.path.starts_with("/mgmt/v1/spaces/")) {
        let names = c.body["external_ids"].as_array().cloned().unwrap_or_else(|| vec![c.body["external_id"].clone()]);
        for n in names {
            assert!(n.as_str().unwrap().starts_with("k2:principal:"), "{c:?}");
        }
    }

    // A second sweep re-adds nobody (bea stays held).
    worker::reconcile_once().await.expect("sweep");
    drain(&sew).await;
    assert_eq!(rows_for(&format!("space:{SPACE}:{}", pext(&bea.id))).len(), 1, "held is not retried");

    // Ann loses the App grant → she leaves the Space.
    skin::delete_grant_by_id(&ann_app.id).unwrap();
    worker::reconcile_once().await.expect("sweep");
    drain(&sew).await;
    assert!(sew.space(SPACE).unwrap().members.is_empty(), "{}", sew.log());
    assert!(sew.member(&pext(&ann.id)).is_some_and(|m| !m.deactivated), "her Sew account stays (her chat grant does)");
    sew.assert_clean();
}
