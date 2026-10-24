//! `servers.owner` without root (prd-macos-tenant-compute-v1 §10: O1, O2,
//! O4, O5, O7 at the file level; the users and re-own walk run in the
//! Linux root suite). A real executor (temp dirs, a recording platform,
//! real signatures), a node that isn't running: its half
//! (`node::owner_swap`) is called where the restart would run it.

use std::path::Path;
use std::time::{Duration, Instant};

use k2_node_proto::admin::code;
use k2_node_proto::crypto::SigningKey;
use k2_node_proto::share::{self, codes, Network, ServerEntry, ServersFile, ShareLimits, ShareState, Trust};
use serde_json::json;

use super::*;
use crate::admin::tests::Rig;

struct T {
    r: Rig,
    /// The target server (slot 1).
    b: SigningKey,
}

fn pin_of(k: &SigningKey, url: &str, at: i64) -> crate::identity::Pin {
    crate::identity::Pin {
        controller_fp: k.fingerprint(),
        controller_public_key_pem: k.spki_pem(),
        routes: vec![url.into()],
        node_id: "n0".into(),
        name: "mini-1".into(),
        enrolled_at: at,
        sas: "000 000".into(),
        revoked: false,
        revoked_reason: None,
        labels: Default::default(),
        confirmed: true,
    }
}

fn line(slot: u8, label: &str, prefix: &str, trust: Trust, base: &ServerEntry) -> ServerEntry {
    let mut e = base.clone();
    e.slot = slot;
    e.label = label.into();
    e.fp_prefix = prefix.into();
    e.trust = trust;
    e.state = ShareState::Active;
    e.set_limits(&ShareLimits::new_server(trust, false));
    e.added_at = 100;
    e.added_by = "local:test".into();
    e
}

/// A shared node: the owner (rosson, the rig's controller), the target
/// (z3thon, slot 1, own, online) and another server (baden, slot 2), each
/// with a file in its trees.
fn setup(level: &str) -> T {
    let r = Rig::new(level);
    let layout = r.exec.paths.layout();
    std::fs::write(layout.policy(), "max_parallel = 2\n").unwrap();
    crate::identity::write_pin(&layout.pin(), &pin_of(&r.ctl, "https://rosson.k2.dev", 1)).unwrap();
    let b = SigningKey::generate().unwrap();
    let c = SigningKey::generate().unwrap();
    let mut f = ServersFile::new(&r.ctl.fingerprint(), "rosson.k2.dev", ShareLimits::owner(2, 8, 16, 100, Network::On), 3, 50, "local:test");
    let base = f.servers[0].clone();
    f.servers.push(line(1, "z3thon.k2.dev", &b.fingerprint()[..16], Trust::Own, &base));
    f.servers.push(line(2, "baden.k2.dev", &c.fingerprint()[..16], Trust::Own, &base));
    servers::write_file(&layout.config, &f).unwrap();
    for (slot, k, url) in [(1, &b, "https://z3thon.k2.dev"), (2, &c, "https://baden.k2.dev")] {
        let t = layout.tenant(slot);
        t.ensure_state().unwrap();
        crate::identity::write_tenant_pin(&t.pin(), &pin_of(k, url, 200), &Default::default()).unwrap();
        std::fs::write(t.ledger(), format!("LEDGER-{slot}")).unwrap();
        std::fs::write(t.state().join(crate::session::LINE_MARK_FILE), format!("{}:100", &k.fingerprint()[..16])).unwrap();
        for d in TREES {
            std::fs::create_dir_all(t.root().join(d)).unwrap();
        }
        std::fs::write(t.root().join("jobs").join(format!("slot{slot}-job")), "x").unwrap();
        std::fs::write(t.root().join("cache").join(format!("slot{slot}-cache")), "warm").unwrap();
    }
    std::fs::write(layout.ledger(), "LEDGER-0").unwrap();
    std::fs::write(layout.jobs().join("owner-job"), "x").unwrap();
    std::fs::write(layout.cache().join("owner-cache"), "warm").unwrap();
    online(&r, &[1, 2]);
    T { r, b }
}

fn online(r: &Rig, slots: &[u8]) {
    let servers = slots.iter().map(|s| crate::status::ServerLine { slot: *s, link: "online".into(), ..Default::default() }).collect();
    crate::status::write(&r.exec.paths.layout().status(), &crate::status::Status { wall: "seatbelt".into(), servers, ..Default::default() }).unwrap();
}

fn file(r: &Rig) -> ServersFile {
    servers::read_file(&r.exec.paths.config_dir).unwrap().unwrap()
}

/// Wait for the drain thread to run the change (its audit line).
fn wait_done(r: &Rig) -> crate::admin::store::AuditLine {
    let t0 = Instant::now();
    loop {
        if let Some(l) = r.exec.audit().verify().unwrap().into_iter().rev().find(|l| l.event == "owner_change") {
            // The drain record goes after the act returns.
            while r.exec.draining.load(std::sync::atomic::Ordering::SeqCst) {
                assert!(t0.elapsed() < Duration::from_secs(20), "the drain never ended");
                std::thread::sleep(Duration::from_millis(10));
            }
            return l;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "the owner change never ran");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn owner_cmd(t: &T, args: serde_json::Value) -> k2_node_proto::admin::AdminResult {
    t.r.run(&t.r.approve(t.r.cmd(share::VERB_OWNER, args)))
}

fn exists(p: impl AsRef<Path>) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// O1, O2, O7: the signed, owner-approved verb drains, swaps the two
/// slots' trees, restarts the node (marked), rewrites servers.toml and
/// re-pins admin.toml; the other server's line and files are untouched;
/// the node's half swaps pins and ledgers; the old controller's commands
/// no longer verify.
#[test]
fn the_owner_moves_to_another_own_server() {
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let before = file(r);
    let baden_before = before.entry(2).cloned().unwrap();
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": t.b.fingerprint()}));
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(res.result["phase"], "draining", "always a settling drain first: {}", res.result);
    assert_eq!((res.result["server"].as_str(), res.result["slot"].as_u64()), (Some("z3thon.k2.dev"), Some(1)));
    let line = wait_done(r);
    assert!(line.code.is_none(), "{line:?}");
    assert!(!exists(r.exec.paths.state_dir.join(STEP_FILE)), "the step file goes once it's done");

    // servers.toml (MT31).
    let f = file(r);
    assert_eq!(f.owner, t.b.fingerprint());
    let o = f.entry(0).unwrap();
    assert_eq!((o.label.as_str(), o.fp_prefix.as_str(), o.expires_at), ("z3thon.k2.dev", t.b.fingerprint().as_str(), None));
    assert_eq!(o.limits(), before.entry(1).unwrap().limits(), "the target keeps its limits");
    let old = f.entry(1).unwrap();
    assert_eq!((old.label.as_str(), old.fp_prefix.as_str(), old.trust, old.state), ("rosson.k2.dev", &r.ctl.fingerprint()[..16], Trust::Own, ShareState::Active));
    assert!(old.added_by.starts_with("owner-change:"), "{}", old.added_by);
    assert_eq!(f.entry(2), Some(&baden_before), "another server's line is untouched (O7)");

    // admin.toml (MT32).
    let cfg = crate::admin::config::read(&r.exec.paths.admin()).unwrap().unwrap();
    assert_eq!(cfg.controller.as_ref().unwrap().fp, t.b.fingerprint());
    assert!(cfg.approval_keys.is_empty(), "the old owner's approval keys are gone; the new owner first-pins its own");
    assert_eq!(cfg.level, k2_node_proto::admin::Level::Full, "the level stays");

    // The trees traded places (caches = move); baden's are untouched.
    let t1 = layout.tenant(1);
    assert!(exists(layout.jobs().join("slot1-job")) && exists(layout.cache().join("slot1-cache")), "the new owner's trees are the home's now");
    assert!(exists(t1.root().join("jobs/owner-job")) && exists(t1.root().join("cache/owner-cache")), "the old owner's moved to slot 1");
    assert!(!exists(layout.jobs().join("owner-job")) && !exists(t1.root().join("jobs/slot1-job")));
    assert!(exists(layout.tenant(2).root().join("jobs/slot2-job")) && exists(layout.tenant(2).root().join("cache/slot2-cache")));
    assert!(!exists(layout.tenants_root().join(STAGE_DIR)), "no staging left");

    // One marked restart; the node's instruction is in the mark dir.
    assert_eq!(r.plat.calls.lock().unwrap().iter().filter(|c| *c == "restart-unit k2-node").count(), 1);
    let n = read_node_change(&r.exec.paths.config_dir).expect("the node's instruction");
    assert_eq!((n.slot, n.from_fp.as_str(), n.to_fp.as_str()), (1, r.ctl.fingerprint().as_str(), t.b.fingerprint().as_str()));
    let m = crate::admin::update::read_update_mark(&r.exec.paths.config_dir).unwrap();
    assert_eq!(m.what, share::VERB_OWNER, "the restart is marked: jobs it stops end node_update");

    // Control went back to what it was.
    let c = crate::config::read_control(&layout.control()).unwrap();
    assert_eq!(c.control, k2_node_proto::frames::Control::Active);

    // The node's half at its start: pins and ledgers trade places.
    assert_eq!(crate::node::owner_swap::apply(&layout, r.exec.now()).unwrap(), crate::node::owner_swap::Outcome::Swapped);
    assert_eq!(crate::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, t.b.fingerprint());
    assert_eq!(std::fs::read_to_string(layout.ledger()).unwrap(), "LEDGER-1", "queued jobs stay with their server (MT29)");
    assert_eq!(std::fs::read_to_string(t1.ledger()).unwrap(), "LEDGER-0");
    assert_eq!(std::fs::read_to_string(t1.state().join(crate::session::LINE_MARK_FILE)).unwrap(), crate::session::line_mark(old), "slot 1's state belongs to its new line");
    assert_eq!(std::fs::read_to_string(layout.tenant(2).ledger()).unwrap(), "LEDGER-2");

    // O2: the old controller's commands no longer verify here.
    let res = r.run(&r.sign(&r.cmd("control.pause", json!({}))));
    assert_eq!(res.code.as_deref(), Some(code::BAD_CONTROLLER_SIGNATURE), "{res:?}");
}

/// O4: a `team`, paused, unconfirmed or offline target, a wrong or
/// missing key, `policy` level and no approval are refused before
/// anything changes.
#[test]
fn bad_targets_are_refused_before_any_change() {
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let fp = t.b.fingerprint();
    let before_file = std::fs::read(servers::servers_path(&layout.config)).unwrap();
    let before_admin = std::fs::read(r.exec.paths.admin()).unwrap();

    let res = r.run(&r.sign(&r.cmd(share::VERB_OWNER, json!({"server": "z3thon.k2.dev", "fp": fp}))));
    assert_eq!(res.code.as_deref(), Some(code::APPROVAL_REQUIRED), "always the owner's approval: {res:?}");
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev"}));
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "a signed command names the key: {res:?}");
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": "c".repeat(64)}));
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "the node's pin has another key: {res:?}");
    assert!(res.message.as_deref().unwrap_or("").contains("another key"), "{res:?}");
    let res = owner_cmd(&t, json!({"server": "slot:0", "fp": fp}));
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS));
    let res = owner_cmd(&t, json!({"server": "nope.k2.dev", "fp": fp}));
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS));

    // Offline.
    online(r, &[2]);
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": fp}));
    assert_eq!(res.code.as_deref(), Some(codes::OWNER_TARGET_NOT_READY), "{res:?}");
    online(r, &[1, 2]);

    // Paused, then a team server.
    let mut f = file(r);
    f.servers.iter_mut().find(|e| e.slot == 1).unwrap().state = ShareState::Paused;
    servers::write_file(&layout.config, &f).unwrap();
    assert_eq!(owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": fp})).code.as_deref(), Some(codes::OWNER_TARGET_NOT_READY));
    let mut f = file(r);
    let e = f.servers.iter_mut().find(|e| e.slot == 1).unwrap();
    e.state = ShareState::Active;
    e.trust = Trust::Team;
    servers::write_file(&layout.config, &f).unwrap();
    assert_eq!(owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": fp})).code.as_deref(), Some(codes::OWNER_NEEDS_OWN_SERVER));
    std::fs::write(servers::servers_path(&layout.config), &before_file).unwrap();

    // An unconfirmed pin.
    let mut p = pin_of(&t.b, "https://z3thon.k2.dev", 200);
    p.confirmed = false;
    crate::identity::write_tenant_pin(&layout.tenant(1).pin(), &p, &Default::default()).unwrap();
    assert_eq!(owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": fp})).code.as_deref(), Some(codes::OWNER_TARGET_NOT_READY));
    crate::identity::write_tenant_pin(&layout.tenant(1).pin(), &pin_of(&t.b, "https://z3thon.k2.dev", 200), &Default::default()).unwrap();

    assert_eq!(std::fs::read(servers::servers_path(&layout.config)).unwrap(), before_file, "servers.toml unchanged");
    assert_eq!(std::fs::read(r.exec.paths.admin()).unwrap(), before_admin, "admin.toml unchanged");
    assert!(r.plat.calls.lock().unwrap().is_empty(), "no restart");
    assert!(!r.exec.draining.load(std::sync::atomic::Ordering::SeqCst), "no drain started");

    // `policy`: refused with the machine hint.
    let p = setup("policy");
    let res = owner_cmd(&p, json!({"server": "z3thon.k2.dev", "fp": p.b.fingerprint()}));
    assert_eq!(res.code.as_deref(), Some(code::REMOTE_ADMIN_OFF), "{res:?}");
    assert!(res.message.as_deref().unwrap_or("").contains("sudo k2-node servers owner"), "{res:?}");
}

/// `old = revoke`, `caches = wipe`: the old owner's line goes (wiped at
/// once), both slots' trees are emptied, admin re-pinned.
#[test]
fn the_old_owner_can_leave_and_caches_can_be_wiped() {
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let res = owner_cmd(&t, json!({"server": "slot:1", "fp": t.b.fingerprint(), "old": "revoke", "caches": "wipe"}));
    assert_eq!(res.state, "done", "{res:?}");
    let l = wait_done(r);
    assert!(l.code.is_none(), "{l:?}");
    let f = file(r);
    assert_eq!(f.owner, t.b.fingerprint());
    assert!(f.entry(1).is_none(), "the old owner's slot is revoked and wiped at once: {f:?}");
    assert!(f.entry(2).is_some());
    assert!(std::fs::read_dir(layout.jobs()).unwrap().next().is_none(), "the owner's job folders are wiped");
    assert!(std::fs::read_dir(layout.cache()).unwrap().next().is_none(), "and its caches");
    assert!(exists(layout.tenant(2).root().join("cache/slot2-cache")), "another slot's caches stay");
    assert_eq!(crate::admin::config::read(&r.exec.paths.admin()).unwrap().unwrap().controller.unwrap().fp, t.b.fingerprint());
}

/// O5: an executor that died at a step finishes it on its next start;
/// the files end as one change makes them. A step that keeps failing
/// waits for a person after [`MAX_ATTEMPTS`].
#[test]
fn a_crash_at_any_step_resumes_on_the_next_start() {
    for phase in [Phase::Trees, Phase::Users, Phase::Restart, Phase::Files] {
        let t = setup("full");
        let r = &t.r;
        let layout = r.exec.paths.layout();
        let p = plan(&r.exec, &OwnerArgs { server: "z3thon.k2.dev".into(), fp: None, old: OldOwner::Keep, caches: Caches::Move, now: false }, false, true).unwrap();
        let mut step = Step {
            v: 1,
            id: "crash1".into(),
            by: "test".into(),
            slot: 1,
            from_fp: r.ctl.fingerprint(),
            to_fp: p.to_fp.clone(),
            to_key: crypto::b64(&p.to_key),
            to_label: p.to.label.clone(),
            old: OldOwner::Keep,
            caches: Caches::Move,
            phase: Phase::Trees,
            at: r.exec.now(),
            attempts: 0,
        };
        // Reach the crash point the way the steps would, then "die".
        let ctx = Ctx::for_executor(&r.exec, "owner-change:crash1");
        if phase == Phase::Trees {
            // Half of phase A: the owner's jobs went to staging already.
            let troot = open_troot(&ctx).unwrap();
            let stage = crate::wall::ensure_child_dir(&troot, &cname(STAGE_DIR), 0o700).unwrap();
            let from = crate::wall::ensure_child_dir(&stage, &cname("from"), 0o700).unwrap();
            let dst = crate::wall::ensure_child_dir(&from, &cname("jobs"), 0o700).unwrap();
            let src = open_dir(&layout.jobs()).unwrap();
            move_entries(&src, &dst, "jobs").unwrap();
        } else {
            move_trees(&r.exec, &ctx, &step).unwrap();
            remove_stage(&r.exec);
        }
        if phase == Phase::Files {
            write_node_change(&r.exec, &step).unwrap();
            // servers.toml written, admin.toml not yet.
            rewrite_servers(&ctx, &step).unwrap();
        }
        step.phase = phase;
        write_step(&r.exec, &step).unwrap();
        let out = resume(&r.exec, false).expect("a change to resume");
        assert!(out.is_ok(), "{phase:?}: {out:?}");
        let f = file(r);
        assert_eq!(f.owner, t.b.fingerprint(), "{phase:?}");
        assert_eq!(f.entry(1).unwrap().label, "rosson.k2.dev", "{phase:?}");
        assert_eq!(crate::admin::config::read(&r.exec.paths.admin()).unwrap().unwrap().controller.unwrap().fp, t.b.fingerprint(), "{phase:?}");
        assert!(exists(layout.jobs().join("slot1-job")) && exists(layout.tenant(1).root().join("jobs/owner-job")), "{phase:?}: the trees swapped exactly once");
        assert!(!exists(layout.jobs().join("owner-job")), "{phase:?}");
        assert!(!exists(r.exec.paths.state_dir.join(STEP_FILE)), "{phase:?}");
        assert!(resume(&r.exec, false).is_none(), "{phase:?}: nothing left");
    }

    // A step that can't finish counts its attempts and then waits.
    let t = setup("full");
    let r = &t.r;
    let step = Step {
        v: 1,
        id: "stuck".into(),
        by: "test".into(),
        slot: 1,
        from_fp: "d".repeat(64),
        to_fp: "e".repeat(64),
        to_key: String::new(),
        to_label: "z3thon.k2.dev".into(),
        old: OldOwner::Keep,
        caches: Caches::Wipe,
        phase: Phase::Files,
        at: r.exec.now(),
        attempts: 0,
    };
    write_step(&r.exec, &step).unwrap();
    for i in 1..=MAX_ATTEMPTS {
        assert!(resume(&r.exec, false).unwrap().is_err());
        assert_eq!(read_step(&r.exec).unwrap().unwrap().attempts, i);
    }
    let e = resume(&r.exec, false).unwrap().unwrap_err();
    assert!(e.message.contains("--resume"), "{e:?}");
    assert_eq!(read_step(&r.exec).unwrap().unwrap().attempts, MAX_ATTEMPTS, "it stopped trying");
    // A new change waits for it.
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": t.b.fingerprint()}));
    assert_eq!(res.code.as_deref(), Some(code::BUSY), "{res:?}");
}

/// The machine's own path: root at the machine, through the socket's
/// local op (the peer check is root only; unit tests run as one user).
#[test]
fn the_machine_changes_the_owner_without_signatures() {
    assert!(crate::admin::server::owner_change_peer_ok(0), "root may");
    assert!(!crate::admin::server::owner_change_peer_ok(54_321), "nobody else (the node user included)");
    let t = setup("policy");
    let r = &t.r;
    let req = crate::admin::exec::LocalRequest::OwnerChange {
        args: Some(OwnerArgs { server: "z3thon.k2.dev".into(), fp: None, old: OldOwner::Keep, caches: Caches::Move, now: false }),
        resume: false,
        by: "rosson".into(),
    };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&req, None) else { panic!("a result") };
    assert_eq!(res.state, "done", "at the machine no signature and any level: {res:?}");
    assert_eq!(res.result["fp"], t.b.fingerprint(), "root shows the key it read from the pin");
    let l = wait_done(r);
    assert!(l.code.is_none(), "{l:?}");
    assert_eq!(file(r).owner, t.b.fingerprint());
    let audit = r.exec.audit().verify().unwrap();
    assert!(audit.iter().any(|l| l.event == "local" && l.verb.as_deref() == Some(share::VERB_OWNER) && l.proposed_by.as_deref() == Some("local:rosson")));
    let req = crate::admin::exec::LocalRequest::OwnerChange { args: None, resume: true, by: "rosson".into() };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&req, None) else { panic!("a result") };
    assert_eq!(res.result["resumed"], false, "{res:?}");
}

/// `swap_lines` is pure: the target at 0 with its own key, the old owner
/// at N with a fresh line, everyone else as they were.
#[test]
fn swap_lines_keeps_everyone_else() {
    let t = setup("full");
    let f = file(&t.r);
    let step = Step {
        v: 1,
        id: "c9".into(),
        by: "t".into(),
        slot: 1,
        from_fp: t.r.ctl.fingerprint(),
        to_fp: t.b.fingerprint(),
        to_key: String::new(),
        to_label: "z3thon.k2.dev".into(),
        old: OldOwner::Keep,
        caches: Caches::Move,
        phase: Phase::Files,
        at: 777,
        attempts: 0,
    };
    let g = swap_lines(&f, &step).unwrap();
    assert_eq!(g.servers.iter().map(|e| e.slot).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(g.entry(1).unwrap().added_at, 777);
    assert_eq!(crate::session::line_mark(g.entry(1).unwrap()), old_line_mark(&step.from_fp, 777), "the node's line mark matches the new line");
    let mut wrong = step.clone();
    wrong.to_fp = "f".repeat(64);
    assert!(swap_lines(&f, &wrong).is_err(), "slot 1 must still hold the target");

}
