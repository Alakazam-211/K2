//! Undo box setup (0.46.3 F8): `POST /cli/app/box/unprep {dryRun?,
//! confirm?}`, `k2 app box unprep [--dry-run] [--yes]`, Settings → Apps →
//! This server → Undo box setup.
//!
//! The owner's only (owner token or an Owner login; never a passport or an
//! Admin), and an explicit confirm (`confirm: true`; the CLI asks at a
//! terminal or takes `--yes`, Settings has a second button). It calls the
//! root helper's `apps unprep` (protocol 6), which undoes what box prep did
//! (the engine unit, the nft table, AppArmor, the process unit, io.cost,
//! the reserved ports, the runtime choice, the budget, linger, delegation)
//! and keeps the k2app user with its sub-ids and data, the distro packages
//! and the engine binary. K2's Podman has its own undo (`k2 app box podman
//! uninstall`).
//!
//! Refused while any App runs, like Podman's removal: the daemon checks its
//! own App table first and the helper checks the k2app processes again. It
//! holds the shared root latch (`root_helper::busy`), and afterwards
//! automatic prep stays off until the owner presses Retry (so it doesn't
//! redo tomorrow what the owner just undid).

use serde_json::{json, Value as J};

use super::routes::{need_credential, no, Refusal, R};
use crate::compute_routes::Who;
use crate::root_helper::busy::Held;
use k2_core::connect_users::Role;
use k2_core::deploy::store;

/// What `apps unprep` undoes, in its order.
pub const UNDOES: &[&str] = &[
    "the App engine's user unit (k2-app-engine stops) and its socket folder",
    "the per-App and box nft table (inet k2app)",
    "the k2-app AppArmor profile",
    "the k2appx process-App unit template",
    "the k2cores slice floor",
    "io.cost on the data disk",
    "the reserved App port range (38600-38999)",
    "k2app's OCI runtime choice",
    "k2app's memory/task budget",
    "linger for k2app (its user manager stops)",
    "cgroup delegation for k2app",
];

/// What it keeps.
pub const KEPT: &[&str] = &[
    "the k2app user and its subordinate ids",
    "/var/lib/k2app (App data)",
    "the distro packages box prep installed",
    "the k2-app-engine binary",
    "K2's Podman in /opt/k2/podman (k2 app box podman uninstall removes it)",
];

const NOTE: &str = "Automatic box prep is paused until you press Retry (Settings → Apps → This server, or k2 app box auto-prepare retry). \
                    K2's Podman stays: k2 app box podman uninstall removes it.";

/// The box as unprep sees it (tests drive a fake).
pub trait UnprepOps {
    fn linux(&self) -> bool;
    /// `Err(why)` when the root helper can't run `apps unprep`.
    fn helper_usable(&self) -> Result<(), String>;
    /// Apps the daemon has running, installing or crash-looping.
    fn running_apps(&self) -> Vec<String>;
    /// The shared root latch (`None` = a fake that doesn't take it).
    fn hold(&self) -> Result<Option<Held>, String>;
    fn unprep(&self) -> Result<J, String>;
    /// Hold automatic prep until Retry.
    fn mark_undone(&self);
    fn audit(&self, actor: &str, outcome: &str);
    fn doctor(&self) -> J;
    fn auto_status(&self) -> J;
}

fn flag(body: &J, k: &str) -> R<bool> {
    match body.get(k) {
        None | Some(J::Null) => Ok(false),
        Some(J::Bool(b)) => Ok(*b),
        Some(_) => Err(no("bad_request", 400, format!("{k} is true or false"))),
    }
}

fn with_plan(mut r: Refusal, running: &[String]) -> Refusal {
    r.extra = Some(json!({ "undoes": UNDOES, "kept": KEPT, "runningApps": running }));
    r
}

pub(crate) fn route(who: &Who, body: &J) -> R<J> {
    route_with(&LiveUnprep, who, body)
}

pub(crate) fn route_with(ops: &dyn UnprepOps, who: &Who, body: &J) -> R<J> {
    need_credential(who)?;
    if !matches!(who, Who::Owner | Who::Login { role: Role::Owner, .. }) {
        return Err(no(
            "owner_only",
            403,
            "undoing the box setup is the owner's: Settings → Apps → This server → Undo box setup, or k2 app box unprep from a terminal outside K2",
        ));
    }
    let dry = flag(body, "dryRun")?;
    let confirm = flag(body, "confirm")?;
    let running = ops.running_apps();
    if dry {
        return Ok(json!({
            "ok": true, "dryRun": true, "changed": false,
            "undoes": UNDOES, "kept": KEPT, "runningApps": running, "note": NOTE,
        }));
    }
    if !ops.linux() {
        return Err(no("linux_only", 409, "Apps run on Linux boxes only"));
    }
    ops.helper_usable().map_err(|e| no("helper_needed", 409, e))?;
    if !confirm {
        return Err(with_plan(
            no("confirm_needed", 409, "undoing the box setup needs your confirm: send confirm: true (k2 app box unprep --yes); --dry-run lists what it undoes"),
            &running,
        ));
    }
    if !running.is_empty() {
        return Err(with_plan(
            no(
                "apps_running",
                409,
                format!("{} App(s) still run: {}. Stop every App first (k2 app stop <app>), then undo the box setup", running.len(), running.join(", ")),
            ),
            &running,
        ));
    }
    let _held = ops.hold().map_err(|e| no("root_busy", 409, e))?;
    match ops.unprep() {
        Ok(v) => {
            ops.mark_undone();
            ops.audit(&super::routes::actor(who), &format!("ok steps={}", v["steps"].as_array().map_or(0, Vec::len)));
            Ok(json!({ "ok": true, "result": v, "kept": KEPT, "note": NOTE, "box": ops.doctor(), "autoPrepare": ops.auto_status() }))
        }
        Err(e) => {
            ops.audit(&super::routes::actor(who), &format!("failed {e}"));
            // The helper's own check (k2app processes the daemon didn't list).
            if e.contains("refused:") && e.contains("running") {
                Err(with_plan(no("apps_running", 409, e), &running))
            } else {
                Err(no("unprep_failed", 409, e))
            }
        }
    }
}

struct LiveUnprep;

impl UnprepOps for LiveUnprep {
    fn linux(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn helper_usable(&self) -> Result<(), String> {
        let min = k2_root_proto::version::APPS_MIN_PROTOCOL;
        let st = crate::root_helper::state();
        if st.usable(min) {
            Ok(())
        } else {
            Err(crate::root_helper::helper_needed("undoing the App box setup", min, &st))
        }
    }
    fn running_apps(&self) -> Vec<String> {
        let h = k2_core::db::shared();
        let conn = h.lock();
        store::list_apps(&conn, None)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| [store::STATE_RUNNING, store::STATE_INSTALLING, store::STATE_CRASHLOOPING].contains(&a.state.as_str()))
            .map(|a| a.app_id)
            .collect()
    }
    fn hold(&self) -> Result<Option<Held>, String> {
        crate::root_helper::busy::try_hold("an App box undo").map(Some)
    }
    fn unprep(&self) -> Result<J, String> {
        let out = crate::root_helper::call_text(&["apps".into(), "unprep".into()], None);
        crate::root_helper::invalidate();
        out.map(|t| serde_json::from_str(&t).unwrap_or(J::String(t)))
    }
    fn mark_undone(&self) {
        if let Err(e) = super::auto_prepare::Store::live().mark_undone(k2_core::deploy::now()) {
            k2_core::log_debug!("[apps] box unprep: auto-prepare hold not saved: {e}");
        }
    }
    fn audit(&self, actor: &str, outcome: &str) {
        super::grace::audit("app-box-unprep", actor, outcome);
    }
    fn doctor(&self) -> J {
        super::box_prep::doctor(&super::box_prep::gather())
    }
    fn auto_status(&self) -> J {
        super::auto_prepare::status_json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        linux: bool,
        helper: Result<(), String>,
        running: Vec<String>,
        /// Take the real shared latch (F9) instead of none.
        real_latch: bool,
        unprep: Result<J, String>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Self {
            Fake {
                linux: true,
                helper: Ok(()),
                running: vec![],
                real_latch: false,
                unprep: Ok(json!({"unprepared": true, "steps": [{"step": "nft remove", "changed": true}], "kept": ["the k2app user"]})),
                calls: RefCell::default(),
            }
        }
    }

    impl UnprepOps for Fake {
        fn linux(&self) -> bool {
            self.linux
        }
        fn helper_usable(&self) -> Result<(), String> {
            self.helper.clone()
        }
        fn running_apps(&self) -> Vec<String> {
            self.running.clone()
        }
        fn hold(&self) -> Result<Option<Held>, String> {
            if self.real_latch {
                crate::root_helper::busy::try_hold("an App box undo").map(Some)
            } else {
                Ok(None)
            }
        }
        fn unprep(&self) -> Result<J, String> {
            self.calls.borrow_mut().push("unprep".into());
            self.unprep.clone()
        }
        fn mark_undone(&self) {
            self.calls.borrow_mut().push("mark_undone".into());
        }
        fn audit(&self, _actor: &str, outcome: &str) {
            self.calls.borrow_mut().push(format!("audit {outcome}"));
        }
        fn doctor(&self) -> J {
            json!({"ready": false})
        }
        fn auto_status(&self) -> J {
            json!({"undoneAt": 1})
        }
    }

    fn agent() -> Who {
        Who::Agent { project_id: "p".into(), session_id: "s".into() }
    }

    /// Owner only, dry run and apply alike.
    #[test]
    fn only_the_owner_undoes_the_box_setup() {
        for w in [Who::Login { username: "a".into(), role: Role::Admin }, agent(), Who::Shell { project_id: "p".into(), session_id: "s".into() }] {
            for body in [json!({"dryRun": true}), json!({"confirm": true})] {
                let f = Fake::new();
                assert_eq!(route_with(&f, &w, &body).unwrap_err().status, 403, "{w:?} {body}");
                assert!(f.calls.borrow().is_empty());
            }
        }
        assert!(!crate::session_token::is_agent_verb("/cli/app/box/unprep"), "no passport reaches it");
        assert!(!crate::session_token::is_agent_verb(super::super::auto_prepare::RETRY_ROUTE));
    }

    /// F8: refused while any App runs (no root call, the list named); the
    /// dry run says so too; without the confirm nothing happens.
    #[test]
    fn refused_while_apps_run_and_without_the_confirm() {
        let mut f = Fake::new();
        f.running = vec!["shop".into(), "blog".into()];
        let e = route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap_err();
        assert_eq!((e.code, e.status), ("apps_running", 409));
        assert!(e.hint.contains("shop, blog") && e.hint.contains("Stop every App first"), "{}", e.hint);
        assert_eq!(e.extra.unwrap()["runningApps"], json!(["shop", "blog"]));
        assert!(f.calls.borrow().is_empty(), "no root call while Apps run");

        let d = route_with(&f, &Who::Owner, &json!({"dryRun": true})).unwrap();
        assert_eq!((d["dryRun"].clone(), d["changed"].clone()), (json!(true), json!(false)));
        assert_eq!(d["runningApps"], json!(["shop", "blog"]));
        assert_eq!(d["undoes"].as_array().unwrap().len(), UNDOES.len());
        assert!(d["kept"].as_array().unwrap().iter().any(|k| k.as_str().unwrap().contains("podman uninstall")));
        assert!(f.calls.borrow().is_empty(), "a dry run makes no root call");

        // The helper's own check (k2app processes the daemon didn't list).
        let mut f = Fake::new();
        f.unprep = Err("refused: 2 k2app processes are running (stop every App first)".into());
        let e = route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap_err();
        assert_eq!(e.code, "apps_running");
        assert!(!f.calls.borrow().contains(&"mark_undone".to_string()), "a refused undo holds nothing");

        let f = Fake::new();
        let e = route_with(&f, &Who::Owner, &json!({})).unwrap_err();
        assert_eq!((e.code, e.status), ("confirm_needed", 409));
        assert!(e.extra.unwrap()["undoes"].is_array());
        assert!(f.calls.borrow().is_empty());
        assert_eq!(route_with(&f, &Who::Owner, &json!({"confirm": "yes"})).unwrap_err().code, "bad_request");
    }

    /// F8: with Apps stopped and the confirm, the helper's unprep runs once,
    /// automatic prep is held, the audit says so; off Linux or without the
    /// helper nothing runs.
    #[test]
    fn it_undoes_the_box_and_holds_automatic_prep() {
        let f = Fake::new();
        let v = route_with(&f, &Who::Login { username: "o".into(), role: Role::Owner }, &json!({"confirm": true})).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["unprepared"], true);
        assert!(v["note"].as_str().unwrap().contains("Retry"), "{v}");
        assert_eq!(*f.calls.borrow(), vec!["unprep", "mark_undone", "audit ok steps=1"]);

        let mut f = Fake::new();
        f.linux = false;
        assert_eq!(route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap_err().code, "linux_only");
        let mut f = Fake::new();
        f.helper = Err("needs the root helper".into());
        assert_eq!(route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap_err().code, "helper_needed");
        assert!(f.calls.borrow().is_empty());

        let mut f = Fake::new();
        f.unprep = Err("nft exit 1: busy".into());
        let e = route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap_err();
        assert_eq!(e.code, "unprep_failed");
        assert_eq!(*f.calls.borrow(), vec!["unprep", "audit failed nft exit 1: busy"]);
    }

    /// F9: undo waits its turn behind a hosted-mail upgrade (the shared latch).
    #[test]
    fn undo_is_refused_inside_another_root_sequence() {
        let _g = crate::mail::mail_server_test_lock();
        let mut f = Fake::new();
        f.real_latch = true;
        assert!(crate::mail::supervisor::try_begin_enable(), "a mail upgrade holds the latch");
        let e = route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap_err();
        assert_eq!((e.code, e.status), ("root_busy", 409));
        assert!(e.hint.contains("hosted-mail"), "{}", e.hint);
        assert!(f.calls.borrow().is_empty(), "no root call");
        crate::mail::supervisor::end_enable();
        route_with(&f, &Who::Owner, &json!({"confirm": true})).unwrap();
        assert_eq!(crate::root_helper::busy::holder(), None, "released after");
    }
}
