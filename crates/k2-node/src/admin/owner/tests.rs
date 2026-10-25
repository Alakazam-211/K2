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
    // Slot scratch (sccache sockets): never moves, goes to the trash (F8).
    std::fs::create_dir_all(layout.tenant(0).tmp()).unwrap();
    std::fs::write(layout.tenant(0).tmp().join("owner-sock"), "s").unwrap();
    std::fs::create_dir_all(layout.tenant(1).tmp()).unwrap();
    std::fs::write(layout.tenant(1).tmp().join("slot1-sock"), "s").unwrap();
    online(&r, &[1, 2]);
    // Root's key records, as the confirming prune takes them (F5).
    servers::prune(&Ctx::for_executor(&r.exec, "node:prune")).unwrap();
    assert_eq!(servers::recorded_key(&r.exec.paths.state_dir, 1, 100).as_deref(), Some(b.fingerprint().as_str()));
    swap_on_restart(&r);
    T { r, b }
}

/// The node's half at its start, as a real restart would run it.
fn swap_on_restart(r: &Rig) {
    let layout = r.exec.paths.layout();
    *r.plat.on_restart.lock().unwrap() = Some(Box::new(move || {
        crate::node::owner_swap::apply(&layout, crate::admin::tests::NOW).unwrap();
    }));
}

/// Call `resume` past `busy` answers (control just held at draining).
fn resume_past_busy(r: &Rig) -> Option<Result<serde_json::Value, AdminRefusal>> {
    for _ in 0..3 {
        match resume(&r.exec, false) {
            Some(Err(e)) if e.code == code::BUSY => continue,
            other => return other,
        }
    }
    panic!("resume stayed busy");
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
    assert_eq!(old.limits(), ShareLimits::new_server(Trust::Own, cfg!(target_os = "macos")), "a new server's default share, not the owner's (F9)");
    assert_eq!(servers::recorded_key(&r.exec.paths.state_dir, 1, old.added_at).as_deref(), Some(r.ctl.fingerprint().as_str()), "root records the old owner's key");
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
    assert!(!exists(layout.tenant(0).tmp().join("owner-sock")) && !exists(t1.tmp().join("slot1-sock")), "tmp never moves with a server");
    assert!(!exists(t1.tmp().join("owner-sock")) && !exists(layout.tenant(0).tmp().join("slot1-sock")));

    // One marked restart; the node's instruction is in the mark dir.
    assert_eq!(r.plat.calls.lock().unwrap().iter().filter(|c| *c == "restart-unit k2-node").count(), 1);
    let n = read_node_change(&r.exec.paths.config_dir).expect("the node's instruction");
    assert_eq!((n.slot, n.from_fp.as_str(), n.to_fp.as_str()), (1, r.ctl.fingerprint().as_str(), t.b.fingerprint().as_str()));
    let m = crate::admin::update::read_update_mark(&r.exec.paths.config_dir).unwrap();
    assert_eq!(m.what, share::VERB_OWNER, "the restart is marked: jobs it stops end node_update");

    // Control went back to what it was.
    let c = crate::config::read_control(&layout.control()).unwrap();
    assert_eq!(c.control, k2_node_proto::frames::Control::Active);

    // The node's half ran at its start (F3 waited for it): pins and
    // ledgers traded places.
    assert_eq!(crate::node::owner_swap::apply(&layout, r.exec.now()).unwrap(), crate::node::owner_swap::Outcome::Nothing, "already applied");
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
    assert!(res.message.as_deref().unwrap_or("").contains("isn't the one named"), "{res:?}");
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

    // No key record from root (or one for another key): refused (F5).
    let keys = r.exec.paths.state_dir.join(servers::KEYS_FILE);
    let saved = std::fs::read(&keys).unwrap();
    std::fs::remove_file(&keys).unwrap();
    assert_eq!(owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": fp})).code.as_deref(), Some(codes::OWNER_TARGET_NOT_READY));
    servers::record_key(&r.exec.paths.state_dir, 1, &"9".repeat(64), 100).unwrap();
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": fp}));
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "the pin isn't the key root recorded: {res:?}");
    assert!(res.message.as_deref().unwrap_or("").contains("recorded"), "{res:?}");
    std::fs::write(&keys, saved).unwrap();

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

/// `old = revoke`, `caches = wipe`: admin re-pinned, the old owner's line
/// revoked as a plain revoke (the node tells it its queued jobs ended;
/// wiped 24 h later by the prune, F10), both slots' trees emptied, even a
/// tree too deep to delete (F2: it goes to the trash by rename).
#[test]
fn the_old_owner_can_leave_and_caches_can_be_wiped() {
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let mut deep = layout.cache().join("deep");
    for _ in 0..300 {
        deep = deep.join("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let res = owner_cmd(&t, json!({"server": "slot:1", "fp": t.b.fingerprint(), "old": "revoke", "caches": "wipe"}));
    assert_eq!(res.state, "done", "{res:?}");
    let l = wait_done(r);
    assert!(l.code.is_none(), "{l:?}");
    let f = file(r);
    assert_eq!(f.owner, t.b.fingerprint());
    let old = f.entry(1).expect("the old owner's line stays until its 24 h wipe");
    assert_eq!((old.label.as_str(), old.state), ("rosson.k2.dev", ShareState::Revoked));
    let book = servers::read_book(&r.exec.paths.state_dir).unwrap();
    assert_eq!(book.revoked.get("1").map(|b| b.1), Some(false), "wiped by the prune later, not at once");
    assert!(f.entry(2).is_some());
    assert!(!exists(r.exec.paths.state_dir.join(STEP_FILE)), "a tree too deep to delete never holds the change up");
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
            let work = crate::wall::ensure_child_dir(&stage, &cname(&work_name(&step)), 0o700).unwrap();
            let from = crate::wall::ensure_child_dir(&work, &cname("from"), 0o700).unwrap();
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
        let out = resume_past_busy(r).expect("a change to resume");
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
    // Control isn't draining yet: held now, and this first resume waits a
    // settle without counting an attempt (F1).
    assert_eq!(resume(&r.exec, false).unwrap().unwrap_err().code, code::BUSY);
    assert_eq!(read_step(&r.exec).unwrap().unwrap().attempts, 0);
    assert_eq!(crate::config::read_control(&r.exec.paths.layout().control()).unwrap().control, k2_node_proto::frames::Control::Draining);
    for i in 1..=MAX_ATTEMPTS {
        assert!(resume(&r.exec, false).unwrap().is_err());
        assert_eq!(read_step(&r.exec).unwrap().unwrap().attempts, i);
    }
    let e = resume(&r.exec, false).unwrap().unwrap_err();
    assert!(e.message.contains("--resume"), "{e:?}");
    assert_eq!(read_step(&r.exec).unwrap().unwrap().attempts, MAX_ATTEMPTS, "it stopped trying");
    // A new change waits for it: refused while the step file exists
    // (verification #7), at the machine too.
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": t.b.fingerprint()}));
    assert_eq!(res.code.as_deref(), Some(IN_PROGRESS), "{res:?}");
    let local = crate::admin::exec::LocalRequest::OwnerChange {
        args: Some(OwnerArgs { server: "z3thon.k2.dev".into(), fp: Some(t.b.fingerprint()), old: OldOwner::Keep, caches: Caches::Move, now: false }),
        resume: false,
        abort: false,
        by: "rosson".into(),
    };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&local, None) else { panic!("a result") };
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
    // The full key is required at the machine too (F5).
    let req = |fp: Option<String>| crate::admin::exec::LocalRequest::OwnerChange {
        args: Some(OwnerArgs { server: "z3thon.k2.dev".into(), fp, old: OldOwner::Keep, caches: Caches::Move, now: false }),
        resume: false,
        abort: false,
        by: "rosson".into(),
    };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&req(None), None) else { panic!("a result") };
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "{res:?}");
    assert!(res.message.as_deref().unwrap_or("").contains("controllerFp"), "{res:?}");
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&req(Some(t.b.fingerprint())), None) else { panic!("a result") };
    assert_eq!(res.state, "done", "at the machine no signature and any level: {res:?}");
    assert_eq!(res.result["fp"], t.b.fingerprint());
    let l = wait_done(r);
    assert!(l.code.is_none(), "{l:?}");
    assert_eq!(file(r).owner, t.b.fingerprint());
    let audit = r.exec.audit().verify().unwrap();
    assert!(audit.iter().any(|l| l.event == "local" && l.verb.as_deref() == Some(share::VERB_OWNER) && l.proposed_by.as_deref() == Some("local:rosson")));
    let req = crate::admin::exec::LocalRequest::OwnerChange { args: None, resume: true, abort: false, by: "rosson".into() };
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
    let g = swap_lines(&f, &step, false).unwrap();
    assert_eq!(g.servers.iter().map(|e| e.slot).collect::<Vec<_>>(), vec![0, 1, 2]);
    assert_eq!(g.entry(1).unwrap().added_at, 777);
    assert_eq!(crate::session::line_mark(g.entry(1).unwrap()), old_line_mark(&step.from_fp, 777), "the node's line mark matches the new line");
    let mut wrong = step.clone();
    wrong.to_fp = "f".repeat(64);
    assert!(swap_lines(&f, &wrong, false).is_err(), "slot 1 must still hold the target");
}

/// F1 + F3: the node never shows it swapped (its owner pin stays the old
/// owner's): the change stops at its restart step with nothing else
/// changed, control stays `draining` and its drain record stays, a resume
/// waits while a job runs; once the node does swap, the resume finishes
/// and control goes back.
#[test]
fn control_stays_draining_until_a_stopped_change_finishes() {
    let mut t = setup("full");
    t.r.exec.node_swap_wait = Duration::from_millis(200);
    *t.r.plat.on_restart.lock().unwrap() = None;
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let res = owner_cmd(&t, json!({"server": "z3thon.k2.dev", "fp": t.b.fingerprint()}));
    assert_eq!(res.state, "done", "{res:?}");
    let l = wait_done(r);
    assert_eq!(l.code.as_deref(), Some(code::FAILED), "{l:?}");
    assert!(l.detail["error"].as_str().unwrap_or("").contains("didn't swap"), "{l:?}");
    assert_eq!(read_step(&r.exec).unwrap().unwrap().phase, Phase::Restart, "stopped at the restart");
    assert_eq!(file(r).owner, r.ctl.fingerprint(), "servers.toml untouched");
    assert_eq!(crate::admin::config::read(&r.exec.paths.admin()).unwrap().unwrap().controller.unwrap().fp, r.ctl.fingerprint(), "admin.toml untouched");
    let draining = || crate::config::read_control(&layout.control()).unwrap().control == k2_node_proto::frames::Control::Draining;
    assert!(draining(), "no job may start while the change is unfinished");
    assert!(r.exec.paths.state_dir.join(crate::admin::update::DRAIN_FILE).exists(), "the drain record stays to put control back later");
    // An executor start: the drain record isn't recovered while it's held.
    crate::admin::update::recover_drain(&r.exec);
    assert!(draining());
    // A job running: the resume waits, and counts no attempt.
    let st = crate::status::Status { running: vec!["j1-g1".into()], ..crate::status::read(&layout.status()).unwrap() };
    crate::status::write(&layout.status(), &st).unwrap();
    assert_eq!(resume(&r.exec, false).unwrap().unwrap_err().code, code::BUSY);
    assert_eq!(read_step(&r.exec).unwrap().unwrap().attempts, 0);
    online(r, &[1, 2]);
    // The node swaps on the next restart: the resume finishes.
    swap_on_restart(r);
    let out = resume_past_busy(r).expect("a change to resume");
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(file(r).owner, t.b.fingerprint());
    crate::admin::update::recover_drain(&r.exec);
    assert_eq!(crate::config::read_control(&layout.control()).unwrap().control, k2_node_proto::frames::Control::Active, "control is back once it's done");
}

/// A step for the setup's target (slot 1, z3thon), `caches = move`.
fn step_for(t: &T, id: &str, phase: Phase) -> Step {
    let r = &t.r;
    let p = plan(&r.exec, &OwnerArgs { server: "z3thon.k2.dev".into(), fp: None, old: OldOwner::Keep, caches: Caches::Move, now: false }, false, true).unwrap();
    Step {
        v: 1,
        id: id.into(),
        by: "test".into(),
        slot: 1,
        from_fp: r.ctl.fingerprint(),
        to_fp: p.to_fp.clone(),
        to_key: crypto::b64(&p.to_key),
        to_label: p.to.label.clone(),
        old: OldOwner::Keep,
        caches: Caches::Move,
        phase,
        at: r.exec.now(),
        attempts: 0,
    }
}

/// Half of the trees step's phase A: the owner's jobs are in staging.
fn half_move(t: &T, step: &Step) {
    let r = &t.r;
    let ctx = Ctx::for_executor(&r.exec, "test");
    let troot = open_troot(&ctx).unwrap();
    let stage = crate::wall::ensure_child_dir(&troot, &cname(STAGE_DIR), 0o700).unwrap();
    let work = crate::wall::ensure_child_dir(&stage, &cname(&work_name(step)), 0o700).unwrap();
    let from = crate::wall::ensure_child_dir(&work, &cname("from"), 0o700).unwrap();
    let dst = crate::wall::ensure_child_dir(&from, &cname("jobs"), 0o700).unwrap();
    move_entries(&open_dir(&r.exec.paths.layout().jobs()).unwrap(), &dst, "jobs").unwrap();
}

/// The trees as before any change: each server's in its own place.
fn trees_unchanged(r: &Rig, what: &str) {
    let layout = r.exec.paths.layout();
    let t1 = layout.tenant(1).root();
    assert!(exists(layout.jobs().join("owner-job")) && exists(layout.cache().join("owner-cache")), "{what}: the owner's trees are home");
    assert!(exists(t1.join("jobs/slot1-job")) && exists(t1.join("cache/slot1-cache")), "{what}: slot 1's are in slot 1");
    assert!(!exists(layout.jobs().join("slot1-job")) && !exists(t1.join("jobs/owner-job")), "{what}: nothing crossed over");
}

/// Verification #1: two resumers at once (a `--resume` racing the
/// background loop): the step is read under the busy lock, so the second
/// sees the change done and never replays the trees step; they trade
/// places exactly once.
#[test]
fn one_resumer_at_a_time() {
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let step = step_for(&t, "race1", Phase::Trees);
    half_move(&t, &step);
    write_step(&r.exec, &step).unwrap();
    // Control held already, so neither resumer stops for a settle.
    assert!(crate::admin::update::hold_draining(&r.exec, share::VERB_OWNER, "race1").unwrap());
    let (a, b) = (r.exec.bg_handle(), r.exec.bg_handle());
    let barrier = std::sync::Barrier::new(2);
    let go = &barrier;
    let (ra, rb) = std::thread::scope(|s| {
        let ha = s.spawn(move || {
            go.wait();
            resume(&a, false)
        });
        let hb = s.spawn(move || {
            go.wait();
            resume(&b, false)
        });
        (ha.join().unwrap(), hb.join().unwrap())
    });
    let outcomes = [&ra, &rb];
    assert_eq!(outcomes.iter().filter(|o| matches!(o, Some(Ok(_)))).count(), 1, "exactly one resumer ran the change: {ra:?} / {rb:?}");
    assert_eq!(outcomes.iter().filter(|o| o.is_none()).count(), 1, "the other found nothing left: {ra:?} / {rb:?}");
    assert!(exists(layout.jobs().join("slot1-job")) && exists(layout.tenant(1).root().join("jobs/owner-job")), "the trees swapped exactly once");
    assert!(!exists(layout.jobs().join("owner-job")) && !exists(layout.tenant(1).root().join("jobs/slot1-job")));
    assert_eq!(file(r).owner, t.b.fingerprint());
    assert_eq!(r.plat.calls.lock().unwrap().iter().filter(|c| *c == "restart-unit k2-node").count(), 1, "one restart");
}

/// Verification #2, #3, #7: while the step file exists nothing may change
/// control, restart or update k2-node, or touch servers.toml, by any path
/// (signed verbs, the local servers verbs, a drain from inside); `servers
/// list` still answers. Once the change is over, they work again.
#[test]
fn nothing_acts_beside_an_unfinished_change() {
    let t = setup("full");
    let r = &t.r;
    let cfg = r.exec.paths.config_dir.clone();
    assert!(!change_in_progress(&cfg));
    let step = step_for(&t, "held1", Phase::Restart);
    write_step(&r.exec, &step).unwrap();
    assert!(change_in_progress(&cfg), "the public marker says so");
    assert_eq!(std::fs::read_to_string(crate::admin::update::mark_dir(&cfg).join(ACTIVE_FILE)).unwrap(), "held1");
    let refused = |verb: &str, args: serde_json::Value| {
        let res = r.run(&r.sign(&r.cmd(verb, args)));
        assert_eq!(res.code.as_deref(), Some(IN_PROGRESS), "{verb}: {res:?}");
    };
    refused("control.resume", json!({}));
    refused("control.pause", json!({}));
    refused("node.rollback", json!({}));
    refused("servers.pause", json!({"server": "baden.k2.dev"}));
    let res = r.run(&r.approve(r.cmd("unit.restart", json!({"unit": "k2-node"}))));
    assert_eq!(res.code.as_deref(), Some(IN_PROGRESS), "unit.restart: {res:?}");
    // Any drain from inside the executor (an update, a rollback, a restart).
    let e = crate::admin::update::drain_then(&r.exec, false, "unit.restart", "c1", |_| Ok(json!({}))).unwrap_err();
    assert_eq!(e.code, IN_PROGRESS, "{e:?}");
    // The local servers verbs; `list` still answers.
    let ctx = Ctx::for_executor(&r.exec, "local:test");
    let list = servers::run(&ctx, share::VERB_LIST, &json!({})).unwrap();
    assert_eq!(list["servers"].as_array().map(|s| s.len()), Some(3), "servers list still answers: {list}");
    let e = servers::run(&ctx, share::VERB_PAUSE, &json!({"server": "baden.k2.dev"})).unwrap_err();
    assert_eq!(e.code, IN_PROGRESS, "{e:?}");
    let e = servers::run(&ctx, share::VERB_REVOKE, &json!({"server": "z3thon.k2.dev", "now": false, "wipe": false})).unwrap_err();
    assert_eq!(e.code, IN_PROGRESS, "{e:?}");
    assert!(r.plat.calls.lock().unwrap().is_empty(), "nothing restarted");
    assert_eq!(file(r).entry(2).unwrap().state, ShareState::Active, "baden untouched");
    // Over (here: ended by hand): everything works again.
    end_step(&r.exec).unwrap();
    assert!(!change_in_progress(&cfg));
    let res = r.run(&r.sign(&r.cmd("control.resume", json!({}))));
    assert_eq!(res.state, "done", "{res:?}");
    servers::run(&ctx, share::VERB_PAUSE, &json!({"server": "baden.k2.dev"})).unwrap();
}

/// Verification #5: the files step waits for the node's applied record
/// naming this change, not only its owner pin.
#[test]
fn the_files_step_waits_for_the_applied_record() {
    let mut t = setup("full");
    t.r.exec.node_swap_wait = Duration::from_millis(100);
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let step = step_for(&t, "applied1", Phase::Restart);
    // The pin says swapped, but the swap isn't finished (no record).
    crate::identity::write_pin(&layout.pin(), &pin_of(&t.b, "https://z3thon.k2.dev", 1)).unwrap();
    let e = wait_for_swap(&r.exec, &step).unwrap_err();
    assert!(e.message.contains("didn't swap"), "{e:?}");
    // A record for another change isn't this one.
    std::fs::write(layout.state().join(crate::node::owner_swap::APPLIED), "other").unwrap();
    assert!(wait_for_swap(&r.exec, &step).is_err());
    std::fs::write(layout.state().join(crate::node::owner_swap::APPLIED), "applied1").unwrap();
    wait_for_swap(&r.exec, &step).unwrap();
}

/// Verification #7: `--abort` before the node's swap rolls the change back
/// from each phase: the trees go back (a half move finished first), the
/// node's instruction goes, the step file and marker go, control is put
/// back, and it's audited. A re-run of an interrupted abort carries on
/// rather than swapping twice.
#[test]
fn abort_rolls_back_before_the_node_swaps() {
    for (phase, interrupted) in [(Phase::Trees, false), (Phase::Users, false), (Phase::Users, true), (Phase::Restart, false)] {
        let t = setup("full");
        let r = &t.r;
        let what = format!("{phase:?}{}", if interrupted { " (interrupted)" } else { "" });
        let step = step_for(&t, "abort1", phase);
        let ctx = Ctx::for_executor(&r.exec, "test");
        if phase == Phase::Trees {
            half_move(&t, &step);
        } else {
            move_trees(&r.exec, &ctx, &step).unwrap();
            remove_stage(&r.exec);
        }
        if phase == Phase::Restart {
            // The restart ran, the node didn't swap (yet).
            write_node_change(&r.exec, &step).unwrap();
        }
        if interrupted {
            // An earlier abort traded the trees back, then died.
            move_trees(&r.exec, &ctx, &Step { id: format!("abort-{}", step.id), ..step.clone() }).unwrap();
        }
        write_step(&r.exec, &step).unwrap();
        assert!(crate::admin::update::hold_draining(&r.exec, share::VERB_OWNER, "abort1").unwrap());
        let v = abort(&r.exec).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_eq!(v["id"], "abort1", "{what}");
        trees_unchanged(r, &what);
        let layout = r.exec.paths.layout();
        assert!(!exists(r.exec.paths.state_dir.join(STEP_FILE)), "{what}: the step file goes");
        assert!(!change_in_progress(&r.exec.paths.config_dir), "{what}: the marker goes");
        assert!(read_node_change(&r.exec.paths.config_dir).is_none(), "{what}: the node never swaps later");
        assert!(!exists(layout.tenants_root().join(STAGE_DIR)), "{what}: no staging left");
        assert_eq!(file(r).owner, r.ctl.fingerprint(), "{what}: servers.toml untouched");
        assert_eq!(crate::admin::config::read(&r.exec.paths.admin()).unwrap().unwrap().controller.unwrap().fp, r.ctl.fingerprint(), "{what}");
        assert_eq!(crate::config::read_control(&layout.control()).unwrap().control, k2_node_proto::frames::Control::Active, "{what}: control is back");
        let audit = r.exec.audit().verify().unwrap();
        assert!(audit.iter().any(|l| l.event == "owner_change_aborted" && l.cmd_id.as_deref() == Some("abort1")), "{what}: audited");
        // The node at its next start: nothing to swap.
        assert_eq!(crate::node::owner_swap::apply(&layout, r.exec.now()).unwrap(), crate::node::owner_swap::Outcome::Nothing, "{what}");
        assert!(resume(&r.exec, false).is_none(), "{what}: nothing left to resume");
    }
}

/// Verification #7: once the node swapped (the files phase, or a swap seen
/// in the restart phase) only finishing is safe: refused, the step and the
/// node's instruction stay, and a resume finishes. No change, a running
/// job and mixed flags are refused too.
#[test]
fn abort_refuses_after_the_node_swapped() {
    let t = setup("full");
    let r = &t.r;
    let e = abort(&r.exec).unwrap_err();
    assert_eq!(e.code, code::BAD_ARGS, "no change: {e:?}");
    let step = step_for(&t, "late1", Phase::Restart);
    let ctx = Ctx::for_executor(&r.exec, "test");
    move_trees(&r.exec, &ctx, &step).unwrap();
    remove_stage(&r.exec);
    write_node_change(&r.exec, &step).unwrap();
    crate::node::owner_swap::apply(&r.exec.paths.layout(), r.exec.now()).unwrap();
    write_step(&r.exec, &step).unwrap();
    let e = abort(&r.exec).unwrap_err();
    assert_eq!(e.code, IN_PROGRESS, "{e:?}");
    assert!(e.message.contains("--resume"), "{e:?}");
    assert!(exists(r.exec.paths.state_dir.join(STEP_FILE)), "the step stays");
    assert_eq!(read_node_change(&r.exec.paths.config_dir).map(|n| n.id), Some("late1".to_string()), "the instruction is back for the resume");
    // The files phase: always forward.
    let mut s = read_step(&r.exec).unwrap().unwrap();
    s.phase = Phase::Files;
    write_step(&r.exec, &s).unwrap();
    assert_eq!(abort(&r.exec).unwrap_err().code, IN_PROGRESS);
    // At the machine: mixed flags are refused; then the resume finishes.
    let req = crate::admin::exec::LocalRequest::OwnerChange { args: None, resume: true, abort: true, by: "rosson".into() };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&req, None) else { panic!("a result") };
    assert_eq!(res.code.as_deref(), Some(code::BAD_ARGS), "{res:?}");
    let out = resume_past_busy(r).expect("a change to resume");
    assert!(out.is_ok(), "{out:?}");
    assert_eq!(file(r).owner, t.b.fingerprint());

    // A job still running: the abort waits (nothing moves under it).
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let step = step_for(&t, "busy1", Phase::Users);
    move_trees(&r.exec, &Ctx::for_executor(&r.exec, "test"), &step).unwrap();
    write_step(&r.exec, &step).unwrap();
    let st = crate::status::Status { running: vec!["j1-g1".into()], ..crate::status::read(&layout.status()).unwrap() };
    crate::status::write(&layout.status(), &st).unwrap();
    assert_eq!(abort(&r.exec).unwrap_err().code, code::BUSY);
    assert!(exists(r.exec.paths.state_dir.join(STEP_FILE)));
    // Through the socket's local op (root only in production).
    let st = crate::status::Status { running: vec![], ..crate::status::read(&layout.status()).unwrap() };
    crate::status::write(&layout.status(), &st).unwrap();
    let req = crate::admin::exec::LocalRequest::OwnerChange { args: None, resume: false, abort: true, by: "rosson".into() };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&req, None) else { panic!("a result") };
    assert_eq!(res.state, "done", "{res:?}");
    assert_eq!(res.result["aborted"], true, "{res:?}");
    assert!(!exists(r.exec.paths.state_dir.join(STEP_FILE)));
    trees_unchanged(r, "aborted at the machine");
}

/// Verification #6: staging or trash a change left (a tree deeper than
/// the remover goes) is removed by the next prune's sweep while no change
/// runs; never while one does.
#[test]
fn the_prune_sweeps_leftover_staging_and_trash() {
    let t = setup("full");
    let r = &t.r;
    let layout = r.exec.paths.layout();
    let mut deep = layout.tenants_root().join(STAGE_DIR).join("trash").join("slot-cache");
    for _ in 0..300 {
        deep = deep.join("d");
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("f"), "x").unwrap();
    let home_trash = r.exec.paths.home.join(HOME_TRASH).join("owner-jobs").join("j");
    std::fs::create_dir_all(&home_trash).unwrap();
    // Not while a change runs.
    let step = step_for(&t, "sweep1", Phase::Restart);
    write_step(&r.exec, &step).unwrap();
    sweep_leftovers(&r.exec);
    assert!(exists(&deep), "a running change's staging stays");
    end_step(&r.exec).unwrap();
    let op = crate::admin::exec::LocalRequest::Servers { servers: crate::admin::exec::ServersOp::Prune };
    let crate::admin::exec::LocalResponse::Result(res) = r.exec.handle(&op, None) else { panic!("a result") };
    assert_eq!(res.state, "done", "{res:?}");
    assert!(!exists(layout.tenants_root().join(STAGE_DIR)), "the staging and its deep trash are gone");
    assert!(!exists(r.exec.paths.home.join(HOME_TRASH)), "the home trash is gone");
    let names: Vec<String> = std::fs::read_dir(layout.tenants_root()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.starts_with(".flat-")), "nothing flattened out of the tree: {names:?}");
}
