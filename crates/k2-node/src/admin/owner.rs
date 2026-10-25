//! Changing a node's owner server (prd-macos-tenant-compute-v1 §6,
//! MT27–MT33): `servers.owner`.
//!
//! Another server already on the node, in a tenant slot N (1–7), becomes
//! the owner (slot 0). It must be one of the owner's own servers (`own`),
//! active, with a confirmed pin, and connected now, so it can take over
//! and first-pin its approval key at once. A `team` server never becomes
//! owner: it would get root admin over the machine.
//!
//! Who: the current owner server, with the owner's approval signature at
//! `full`, or root at the machine (`sudo k2-node servers owner … --fp FP`,
//! an unsigned local op only root may send). Both name the target's full
//! key fingerprint. What anchors that key is root's own record, taken when
//! the target's add was confirmed (`servers.prune`, from the confirmed pin
//! whose first 64 bits the add's code fixed); the node's pin of today and
//! the named `fp` must both equal it. The owner server's `fp` comes from
//! the node's own list, so it guards against a mix-up, not a lying node:
//! the person compares the full key on the approval card with what the new
//! owner server shows (`controllerFp` in `k2 compute nodes --json`).
//!
//! What root does, never during work: a settling drain first (no new
//! starts; running jobs finish, or stop with `now` as
//! `interrupted/node_update`), then, under the servers lock, one step at a
//! time, each recorded in the root-only step file [`STEP_FILE`] before it
//! starts, so a crash resumes it on the executor's next start (never a
//! half-swapped node):
//!
//! 1. **trees**: the two slots' job folders, mirrors and caches (`jobs/`,
//!    `repos/`, `cache/` under the node home and `<home>-t/N`) trade
//!    places, entry by entry, through a root-only staging folder in
//!    `<home>-t` (renames on directory fds, no copies, no link followed;
//!    `caches = wipe` removes them instead);
//! 2. **users**: slot N's old tenant user goes (Linux), files kept;
//! 3. **restart**: root writes the node's instruction [`NODE_FILE`] in its
//!    mark directory (root-owned, the node reads it) and restarts k2-node
//!    the marked way; the node swaps its own files (pins, ledgers,
//!    artifacts, the queue's slot rows) at start, before it opens any
//!    (`node::owner_swap`), while `servers.toml` still names the old owner,
//!    so it opens no tenant in between;
//! 4. **files**: `servers.toml` (the target at slot 0, the old owner at
//!    slot N, kept or revoked and wiped), the old owner's new tenant user
//!    (`tenant.prepare`, a new uid, V64), the moved trees re-owned for
//!    their new slot by the fd walker, and `admin.toml` re-pinned to the
//!    target's key with its approval keys cleared (written atomically and
//!    read back). The level, `auto_update` and `narrow.toml` stay.
//!
//! Control stays `draining` throughout and goes back once it's done.
//! Audited: `owner_change` (and `tenant.unuser`, `tenant.prepare`).

use std::ffi::{CStr, CString};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use k2_node_proto::admin::{code, AdminRefusal};
use k2_node_proto::crypto;
use k2_node_proto::share::{self, codes, Caches, OldOwner, OwnerArgs, ServerEntry, ServersFile, ShareState, Slot, Trust, OWNER_SLOT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::exec::Executor;
use super::servers::{self, Ctx};

/// Root-only, in the executor's state dir: the owner change under way.
pub const STEP_FILE: &str = "owner-change.json";
/// In the update mark's directory (root-owned, node-readable): what the
/// node swaps at its next start.
pub const NODE_FILE: &str = "owner-change.json";
/// In the mark directory (root-owned, readable by all): an owner change is
/// under way. The machine owner's own tools (`k2-node pause|drain|resume`,
/// `k2-node servers …` without sudo) can't see root's step file; they see
/// this and refuse (verification #2, #7).
pub const ACTIVE_FILE: &str = "owner-change-active";
/// Root-only staging in `<home>-t` (root's own 0711 folder).
pub const STAGE_DIR: &str = ".owner-change";
/// A root-only trash on the node home's own volume (renames never cross a
/// filesystem; verification #4).
pub const HOME_TRASH: &str = ".k2-owner-change-trash";
/// Refused while an owner change is under way.
pub const IN_PROGRESS: &str = "owner_change_in_progress";
/// The node acts on an instruction at most this old; root removes older
/// ones when it starts.
pub const NODE_FILE_MAX_AGE_SECS: i64 = 7 * 86_400;
/// The job-facing trees that trade places.
pub const TREES: &[&str] = &["jobs", "cache", "repos"];
/// A resume that failed this many times waits for a person
/// (`sudo k2-node servers owner --resume`).
pub const MAX_ATTEMPTS: u32 = 3;
const STEP_V: u32 = 1;

/// Where an owner change is (each step is recorded before it starts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Trees,
    Users,
    Restart,
    Files,
}

/// [`STEP_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub v: u32,
    /// The signed command's id, or `local-<random>` at the machine.
    pub id: String,
    pub by: String,
    /// The target's old slot (the old owner's new one).
    pub slot: Slot,
    pub from_fp: String,
    pub to_fp: String,
    /// The target's SPKI DER (base64), checked against `to_fp`.
    pub to_key: String,
    pub to_label: String,
    pub old: OldOwner,
    pub caches: Caches,
    pub phase: Phase,
    /// When the change started (the old owner's new line's `added_at`).
    pub at: i64,
    /// Resumes that failed (see [`MAX_ATTEMPTS`]).
    #[serde(default)]
    pub attempts: u32,
}

/// [`NODE_FILE`]: what the node swaps at start (`node::owner_swap`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeChange {
    pub v: u32,
    pub id: String,
    pub slot: Slot,
    pub from_fp: String,
    pub to_fp: String,
    /// The old owner's new line mark in slot N (`<fp_prefix>:<added_at>`),
    /// so the node keeps the state it moved there.
    pub line_mark: String,
    pub at: i64,
}

fn refuse(c: &str, m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(c, m)
}

fn failed(m: impl Into<String>) -> AdminRefusal {
    AdminRefusal::new(code::FAILED, m)
}

fn step_path(e: &Executor) -> PathBuf {
    e.paths.state_dir.join(STEP_FILE)
}

/// Open a directory. Root walks the path one component at a time and
/// never through a link (`wall::open_dir_path`). An executor that isn't
/// root (unit tests, maybe inside a job wall where a parent is
/// traverse-only, so the walk can't open it) opens it in one go, still
/// `O_NOFOLLOW` at the last component.
fn open_dir(p: &Path) -> Result<OwnedFd, String> {
    // SAFETY: plain getter.
    if unsafe { libc::geteuid() } == 0 {
        return crate::wall::open_dir_path(p);
    }
    let c = CString::new(p.as_os_str().as_encoded_bytes()).map_err(|_| format!("{}: a NUL in the path", p.display()))?;
    // SAFETY: a valid C string; the fd is owned right below.
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(format!("open {}: {}", p.display(), std::io::Error::last_os_error()));
    }
    use std::os::fd::FromRawFd;
    // SAFETY: a fresh fd nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn read_step(e: &Executor) -> Result<Option<Step>, String> {
    match std::fs::read(step_path(e)) {
        Ok(b) => serde_json::from_slice(&b).map(Some).map_err(|x| format!("{STEP_FILE}: {x}")),
        Err(x) if x.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(x) => Err(format!("read {STEP_FILE}: {x}")),
    }
}

fn write_step(e: &Executor, s: &Step) -> Result<(), AdminRefusal> {
    crate::util::mkdir_mode(&e.paths.state_dir, 0o700).map_err(failed)?;
    let cfg = &e.paths.config_dir;
    super::update::ensure_mark_dir(cfg).map_err(failed)?;
    super::update::write_owned_in(&super::update::mark_dir(cfg), ACTIVE_FILE, s.id.as_bytes(), 0o644, super::update::mark_owner(cfg)).map_err(failed)?;
    let b = serde_json::to_vec_pretty(s).map_err(|x| failed(x.to_string()))?;
    crate::util::atomic_write(&step_path(e), &b, 0o600).map_err(failed)
}

/// The change is over (done or aborted): the step file goes, then the
/// public marker.
fn end_step(e: &Executor) -> Result<(), AdminRefusal> {
    std::fs::remove_file(step_path(e)).map_err(|x| failed(format!("remove {STEP_FILE}: {x}")))?;
    let cfg = &e.paths.config_dir;
    if let Ok(dir) = open_dir(&super::update::mark_dir(cfg)) {
        use std::os::fd::AsRawFd;
        // SAFETY: unlinkat a fixed name in our directory fd.
        unsafe { libc::unlinkat(dir.as_raw_fd(), cname(ACTIVE_FILE).as_ptr(), 0) };
    }
    Ok(())
}

/// Is an owner change under way? (The machine owner's tools: the public
/// marker, as root wrote it in its own directory.)
pub fn change_in_progress(config_dir: &Path) -> bool {
    super::update::read_owned_in(&super::update::mark_dir(config_dir), ACTIVE_FILE, super::update::mark_owner(config_dir), true).is_some()
}

/// The refusal for anything that would act under a change.
pub fn in_progress_refusal(what: &str) -> AdminRefusal {
    refuse(
        IN_PROGRESS,
        format!("{what} waits: an owner change is under way on this node (control stays draining until it finishes; sudo k2-node servers owner --resume or --abort)"),
    )
}

/// The old owner's new line mark in slot N.
pub fn old_line_mark(from_fp: &str, at: i64) -> String {
    format!("{}:{at}", &from_fp[..16])
}

/// What a valid owner change would do, checked against root's files.
#[derive(Debug, Clone)]
pub struct Plan {
    pub from: ServerEntry,
    pub to: ServerEntry,
    pub to_fp: String,
    pub to_key: Vec<u8>,
}

/// Check every rule (MT28) against `servers.toml`, root's `admin.toml`,
/// the target's pin and the node's own word that it's connected. Changes
/// nothing. `need_fp`: a signed command must name the target's key.
pub fn plan(e: &Executor, a: &OwnerArgs, need_fp: bool, check_online: bool) -> Result<Plan, AdminRefusal> {
    let args = serde_json::to_value(a).map_err(|x| refuse(code::BAD_ARGS, x.to_string()))?;
    share::check_verb_args(share::VERB_OWNER, &args).map_err(|m| refuse(code::BAD_ARGS, m))?;
    let layout = e.paths.layout();
    let now = e.now();
    let f = servers::read_file(&e.paths.config_dir)
        .map_err(|x| refuse(code::NOT_CONFIGURED, x))?
        .ok_or_else(|| refuse(code::BAD_ARGS, "this node serves only its owner server: add the new owner first (servers add), then make it the owner"))?;
    let cfg = super::config::read(&e.paths.admin())
        .map_err(|x| refuse(code::NOT_CONFIGURED, format!("admin.toml: {x}")))?
        .ok_or_else(|| refuse(code::NOT_CONFIGURED, "no admin.toml here (sudo k2-node admin init)"))?;
    let pinned = cfg.controller.as_ref().map(|c| c.fp.clone()).ok_or_else(|| refuse(code::NOT_CONFIGURED, "root has no owner server pinned (sudo k2-node admin pin)"))?;
    if pinned != f.owner {
        return Err(refuse(code::NOT_CONFIGURED, "servers.toml names another owner than the one root pinned in admin.toml; fix that at the machine first"));
    }
    let owner_pin = crate::identity::read_pin_safely(&layout.pin()).map_err(failed)?;
    if owner_pin.as_ref().map(|p| p.controller_fp.as_str()) != Some(f.owner.as_str()) {
        return Err(refuse(code::NOT_CONFIGURED, "the node's own owner pin doesn't match servers.toml; fix that at the machine first"));
    }
    let from = f.entry(OWNER_SLOT).cloned().ok_or_else(|| refuse(code::NOT_CONFIGURED, "servers.toml has no owner line"))?;
    let to = f.find(&a.server).cloned().ok_or_else(|| refuse(code::BAD_ARGS, format!("no server {:?} on this node", a.server)))?;
    if to.slot == OWNER_SLOT {
        return Err(refuse(code::BAD_ARGS, format!("{} is the owner already", to.label)));
    }
    if to.trust != Trust::Own {
        return Err(refuse(
            codes::OWNER_NEEDS_OWN_SERVER,
            format!("{} is another person's server (trust team): only one of your own servers can own this node, since the owner gets root admin here", to.label),
        ));
    }
    let st = to.status(now);
    if st != share::ServerStatus::Active {
        return Err(refuse(codes::OWNER_TARGET_NOT_READY, format!("{} is {}; only an active server can take over (resume or confirm it first)", to.label, st.as_str())));
    }
    let (pin, flags) = crate::identity::read_tenant_pin_safely(&layout.tenant(to.slot).pin())
        .map_err(failed)?
        .ok_or_else(|| refuse(codes::OWNER_TARGET_NOT_READY, format!("this node has no pin for {} yet", to.label)))?;
    if !pin.confirmed || pin.revoked || flags.revoked_by_server || !crate::node::tenant::pin_serves(&to, &pin) {
        return Err(refuse(codes::OWNER_TARGET_NOT_READY, format!("{}'s pin here isn't confirmed (or {} removed this node)", to.label, to.label)));
    }
    let der = pin.controller_spki().map_err(|x| refuse(codes::OWNER_TARGET_NOT_READY, format!("{}'s pin: {x}", to.label)))?;
    let to_fp = crypto::fingerprint_of_spki_der(&der);
    // The anchor (review F5): the full key root recorded in its own state
    // when this server's add was confirmed (servers.prune), never the
    // node's pin of today alone. The pin, the caller's `fp` and the record
    // must all be the same key.
    let recorded = servers::recorded_key(&e.paths.state_dir, to.slot, to.added_at).ok_or_else(|| {
        refuse(
            codes::OWNER_TARGET_NOT_READY,
            format!("root has no record of {}'s key yet (taken when its add is confirmed, on the next prune); try again in a minute", to.label),
        )
    })?;
    if recorded != to_fp {
        return Err(refuse(
            code::BAD_ARGS,
            format!("the node's pin for {} has another key ({}) than root recorded when it was confirmed ({}); nothing changed", to.label, &to_fp[..16], &recorded[..16]),
        ));
    }
    match a.fp.as_deref() {
        Some(want) if want != to_fp => {
            return Err(refuse(
                code::BAD_ARGS,
                format!("{}'s key here ({}) isn't the one named ({}); nothing changed", to.label, &to_fp[..16], &want[..16.min(want.len())]),
            ))
        }
        None if need_fp => {
            return Err(refuse(
                code::BAD_ARGS,
                "name the new owner's full key fingerprint (fp): the new owner server shows it as controllerFp in `k2 compute nodes --json`",
            ))
        }
        _ => {}
    }
    if to_fp == from.fp_prefix {
        return Err(refuse(code::BAD_ARGS, format!("{} has the owner's own key", to.label)));
    }
    let online = crate::status::read(&layout.status()).ok().and_then(|s| s.servers.iter().find(|l| l.slot == to.slot).map(|l| l.link == "online"));
    if check_online && online != Some(true) {
        return Err(refuse(codes::OWNER_TARGET_NOT_READY, format!("{} isn't connected to this node now; it must be online to take over", to.label)));
    }
    let troot = layout.tenants_root();
    let (h, t) = (open_dir(&e.paths.home).map_err(failed)?, open_dir(&troot).map_err(failed)?);
    let dev = |fd: &OwnedFd| crate::wall::device_of(fd).map_err(|x| failed(x.to_string()));
    let (hd, td) = (dev(&h)?, dev(&t)?);
    // Every tree goes by rename (moved, or into its side's trash on a
    // wipe): a tree that's a volume of its own can't (verification #4).
    let slot_dir = open_dir(&layout.tenant(to.slot).root()).ok();
    let sides = [(Some(&h), hd, e.paths.home.clone()), (slot_dir.as_ref(), td, layout.tenant(to.slot).root())];
    for (parent, want, at) in sides {
        let Some(parent) = parent else { continue };
        if dev(parent)? != want {
            return Err(refuse(code::BAD_ARGS, format!("{} is a volume of its own; an owner change moves files only by rename, within one volume", at.display())));
        }
        for name in TREES.iter().chain(&["tmp"]) {
            let Ok(d) = crate::wall::open_child_dir(parent, &cname(name)) else { continue };
            if dev(&d)? != want {
                return Err(refuse(code::BAD_ARGS, format!("{}/{name} is a volume of its own; an owner change moves files only by rename, within one volume", at.display())));
            }
        }
    }
    if a.caches == Caches::Move && hd != td {
        return Err(refuse(code::BAD_ARGS, format!("{} and {} are on different volumes, so the caches can't move by rename; use caches wipe", e.paths.home.display(), troot.display())));
    }
    Ok(Plan { from, to, to_fp, to_key: der })
}

/// Start an owner change (the signed verb after its checks, or the
/// machine's local op): check, then the settling drain, then [`run_steps`]
/// in the drain's thread. Answers at once (`phase: draining`).
pub fn start(e: &Executor, a: &OwnerArgs, id: &str, by: &str, need_fp: bool) -> Result<Value, AdminRefusal> {
    if let Some(s) = read_step(e).map_err(failed)? {
        return Err(refuse(
            code::BUSY,
            format!("an owner change ({}, to {}) is still finishing; it resumes when the executor starts (sudo k2-node servers owner --resume)", s.id, s.to_label),
        ));
    }
    let p = plan(e, a, need_fp, true)?;
    let args = a.clone();
    let (id_s, by_s) = (id.to_string(), by.to_string());
    let opts = super::update::DrainOpts { give_up_at_limit: true, settle: true, stop_running: a.now };
    let (_, running) = super::update::decide(e, false, true);
    let resp = super::update::run_decided(e, (super::update::Decision::Drain, running), opts, share::VERB_OWNER, id, move |ex| {
        // Nothing runs now; check again: the drain took a while.
        let p = plan(ex, &args, need_fp, false)?;
        let step = Step {
            v: STEP_V,
            id: id_s.clone(),
            by: by_s.clone(),
            slot: p.to.slot,
            from_fp: p.from.fp_prefix.clone(),
            to_fp: p.to_fp.clone(),
            to_key: crypto::b64(&p.to_key),
            to_label: p.to.label.clone(),
            old: args.old,
            caches: args.caches,
            phase: Phase::Trees,
            at: ex.now(),
            attempts: 0,
        };
        // A staging folder with no step file is a finished change's leftover.
        remove_stage(ex);
        write_step(ex, &step)?;
        finish(ex, step)
    })?;
    let mut out = resp;
    out["server"] = p.to.label.clone().into();
    out["slot"] = p.to.slot.into();
    out["fp"] = p.to_fp.clone().into();
    out["from"] = p.from.label.clone().into();
    out["old"] = a.old.as_str().into();
    out["caches"] = a.caches.as_str().into();
    Ok(out)
}

/// Run the remaining steps and record the outcome (audit `owner_change`).
fn finish(e: &Executor, step: Step) -> Result<Value, AdminRefusal> {
    let (id, slot) = (step.id.clone(), step.slot);
    let detail = json!({"from": step.from_fp, "to": step.to_fp, "to_label": step.to_label, "slot": slot, "old": step.old.as_str(), "caches": step.caches.as_str(), "by": step.by});
    let r = run_steps(e, step);
    let mut d = detail;
    match &r {
        Ok(v) => d["result"] = v.clone(),
        Err(x) => d["error"] = x.message.clone().into(),
    }
    let _ = e.audit().append(
        super::store::AuditEntry { event: "owner_change".into(), cmd_id: Some(id), code: r.as_ref().err().map(|x| x.code.clone()), detail: d, ..Default::default() },
        e.now(),
    );
    r
}

/// The steps from wherever `step` stands (each recorded before it runs).
/// Control stays `draining` while the step file exists (review F1).
pub fn run_steps(e: &Executor, mut step: Step) -> Result<Value, AdminRefusal> {
    let ctx = Ctx::for_executor(e, &format!("owner-change:{}", step.id));
    let _g = servers::servers_lock(&ctx)?;
    if step.phase == Phase::Trees {
        // Nothing of either server's jobs may still run while their files
        // move (a daemonized leftover of an ended job): slot N's user and
        // the owner's job user on Linux, orphans of the node user's jobs on
        // macOS (review F6, verification #8).
        if ctx.root {
            e.stop_job_leftovers(step.slot)?;
        }
        move_trees(e, &ctx, &step)?;
        step.phase = Phase::Users;
        write_step(e, &step)?;
        remove_stage(e);
    }
    if step.phase == Phase::Users {
        if ctx.root && cfg!(target_os = "linux") {
            e.tenant_unuser(step.slot)?;
        }
        step.phase = Phase::Restart;
        write_step(e, &step)?;
    }
    if step.phase == Phase::Restart {
        write_node_change(e, &step)?;
        // The node swaps its files at start, while servers.toml still names
        // the old owner (it opens no tenant in between).
        super::update::restart_marked(e, share::VERB_OWNER, &step.id, |e| e.platform.restart_unit("k2-node")).map_err(failed)?;
        // Nothing else moves until the node shows it swapped (review F3).
        wait_for_swap(e, &step)?;
        step.phase = Phase::Files;
        write_step(e, &step)?;
    }
    rewrite_servers(&ctx, &step)?;
    // Root's admin goes to the new owner before anything of the old
    // owner's slot is touched, so nothing after this can leave it with the
    // old one (review F2).
    repin_admin(e, &step)?;
    let old_uid = match step.old {
        OldOwner::Keep => prepare_old_owner(&ctx, &step)?,
        OldOwner::Revoke => {
            revoke_old_owner(&ctx, &step)?;
            None
        }
    };
    let rehomed = rehome(e, &ctx, &step)?;
    end_step(e)?;
    Ok(json!({"owner": step.to_label, "slot": step.slot, "old": step.old.as_str(), "caches": step.caches.as_str(), "old_uid": old_uid, "rehomed": rehomed}))
}

/// After the marked restart: the node's swap must be complete (its applied
/// record names this change, written last, after the pins, ledgers and
/// queue rows) and its owner pin must name the new owner, before the
/// Files step. Never arriving fails the step; it's retried on resume
/// (review F3, verification #5).
fn wait_for_swap(e: &Executor, step: &Step) -> Result<(), AdminRefusal> {
    let layout = e.paths.layout();
    let (pin, applied) = (layout.pin(), layout.state().join(crate::node::owner_swap::APPLIED));
    let deadline = std::time::Instant::now() + e.node_swap_wait;
    loop {
        let done = crate::util::read_regular(&applied).is_ok_and(|s| s.trim() == step.id);
        if done && crate::identity::read_pin_safely(&pin).ok().flatten().is_some_and(|p| p.controller_fp == step.to_fp) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(failed(format!(
                "k2-node didn't swap its files within {} s of the restart (its owner pin isn't {}'s); nothing else changed, the change resumes",
                e.node_swap_wait.as_secs(),
                step.to_label
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(if cfg!(test) { 10 } else { 1000 }));
    }
}

/// Finish an owner change the last executor didn't, and drop a node
/// instruction past its age. Never with a job running: control is held at
/// `draining` first, and a job still running (or a control that only just
/// went to `draining`) answers `busy` without counting an attempt (review
/// F1). [`spawn_resume`] retries those.
pub fn resume(e: &Executor, force: bool) -> Option<Result<Value, AdminRefusal>> {
    // One resumer at a time, and the step read under the same lock that
    // every step runs under: a second resumer (a `--resume` racing the
    // background loop, or the drain's own act) waits, then sees where the
    // first left it, never an old phase to replay (verification #1).
    let _g = e.busy.lock().unwrap_or_else(|p| p.into_inner());
    retire_node_change(e);
    let mut step = match read_step(e) {
        Ok(Some(s)) => s,
        Ok(None) => return None,
        Err(x) => {
            crate::nlog!("admin: owner change: {x} (needs a person at the machine)");
            return Some(Err(failed(x)));
        }
    };
    if step.attempts >= MAX_ATTEMPTS && !force {
        crate::nlog!("admin: owner change {} failed {} times; it waits for sudo k2-node servers owner --resume", step.id, step.attempts);
        return Some(Err(failed(format!("owner change {} failed {} times; run sudo k2-node servers owner --resume", step.id, step.attempts))));
    }
    match super::update::hold_draining(e, share::VERB_OWNER, &step.id) {
        Err(x) => return Some(Err(failed(format!("control can't be held at draining: {x}")))),
        Ok(true) => {
            return Some(Err(refuse(code::BUSY, "control was set to draining just now; the owner change resumes after one settle")));
        }
        Ok(false) => {}
    }
    let running = super::update::running_jobs(e);
    if running > 0 {
        return Some(Err(refuse(code::BUSY, format!("{running} job(s) still running; the owner change resumes once they end (control stays draining)"))));
    }
    crate::nlog!("admin: resuming owner change {} (to {}) at {:?}", step.id, step.to_label, step.phase);
    step.attempts += 1;
    if let Err(x) = write_step(e, &step) {
        return Some(Err(x));
    }
    Some(finish(e, step))
}

/// Has the node swapped (or begun swapping) its files for this change: its
/// applied record, its owner pin already the new owner's, or its own
/// staging folder for this id?
fn node_swapped(e: &Executor, step: &Step) -> bool {
    let layout = e.paths.layout();
    crate::util::read_regular(&layout.state().join(crate::node::owner_swap::APPLIED)).is_ok_and(|s| s.trim() == step.id)
        || crate::identity::read_pin_safely(&layout.pin()).ok().flatten().is_some_and(|p| p.controller_fp == step.to_fp)
        || std::fs::symlink_metadata(layout.state().join(crate::node::owner_swap::STAGE).join(&step.id)).is_ok()
}

/// `sudo k2-node servers owner --abort` (the machine's owner, root only;
/// verification #7): end a wedged change safely instead of deleting root's
/// step file by hand. Before the node swapped its files it rolls back:
///
/// - the node's instruction goes first, so a node start never swaps later
///   (in the `restart` phase, the node is given a settle to show a swap it
///   began before that, and then the abort is refused);
/// - moved trees go back: a half move is finished first (only while its
///   staging is there; after the trees step they're whole), then traded
///   back under the abort's own staging, so a re-run of an interrupted
///   abort carries on rather than swapping twice;
/// - the target gets a tenant user again (Linux; a new uid, recorded in its
///   line) and its trees are re-owned for that user's group;
/// - then the step file goes and control is put back. Audited
///   `owner_change_aborted`.
///
/// Wiped caches can't come back. Once the node swapped (the `files` phase,
/// or a swap seen in `restart`), only `--resume` (forward) is safe, and the
/// instruction is put back for it.
pub fn abort(e: &Executor) -> Result<Value, AdminRefusal> {
    // Never beside a running step (a resume waiting for the node's swap
    // holds this for minutes).
    let _g = e.lock_briefly().ok_or_else(|| refuse(code::BUSY, "an owner change step is running right now; try again in a few minutes"))?;
    let step = read_step(e).map_err(failed)?.ok_or_else(|| refuse(code::BAD_ARGS, "no owner change is under way"))?;
    let forward_only = || refuse(IN_PROGRESS, format!("owner change {} is past the node's swap; only finishing it is safe: sudo k2-node servers owner --resume", step.id));
    if step.phase == Phase::Files {
        return Err(forward_only());
    }
    let running = super::update::running_jobs(e);
    if running > 0 {
        return Err(refuse(code::BUSY, format!("{running} job(s) still running; abort again once they end (control stays draining)")));
    }
    let ctx = Ctx::for_executor(e, &format!("owner-change-abort:{}", step.id));
    let _s = servers::servers_lock(&ctx)?;
    retire_node_change_now(e);
    if step.phase == Phase::Restart {
        // A node start that read the instruction just before it went
        // shows its swap (staging, pin, record) within this settle.
        std::thread::sleep(e.drain_settle.max(std::time::Duration::from_millis(if cfg!(test) { 10 } else { 2000 })));
        if node_swapped(e, &step) {
            write_node_change(e, &step)?;
            return Err(forward_only());
        }
    }
    let mut undone = Vec::new();
    if step.caches == Caches::Move {
        let half = step.phase == Phase::Trees && stage_started(e, &step);
        if half || step.phase > Phase::Trees {
            if ctx.root {
                e.stop_job_leftovers(step.slot)?;
            }
            if half {
                move_trees(e, &ctx, &step)?;
            }
            let back = Step { id: format!("abort-{}", step.id), ..step.clone() };
            move_trees(e, &ctx, &back)?;
            undone.push("trees");
        }
    }
    // The users step removed the target's user on Linux only. Slot N's
    // line still names the target (servers.toml changes only in the files
    // step), so this gives the target a new user, recorded in its line.
    if step.phase > Phase::Trees && ctx.root && cfg!(target_os = "linux") {
        prepare_old_owner(&ctx, &step)?;
        if step.caches == Caches::Move {
            let group = super::jobs::tenant_group_name(step.slot);
            let tgid = super::jobs::group_id(&group).ok_or_else(|| failed(format!("group {group} is missing")))?;
            let (nuid, _) = super::server::user_ids(&ctx.node_user).ok_or_else(|| failed("the node user is missing"))?;
            let slot = open_dir(&ctx.layout().tenant(step.slot).root()).map_err(failed)?;
            for t in TREES {
                let Ok(d) = crate::wall::open_child_dir(&slot, &cname(t)) else { continue };
                for name in crate::wall::entry_names(&d).map_err(io(t))? {
                    crate::wall::rehome_at(&d, &name, nuid, tgid, false).map_err(failed)?;
                }
            }
        }
        undone.push("user");
    }
    end_step(e)?;
    remove_stage(e);
    super::update::recover_drain(e);
    let detail = json!({"id": step.id, "phase": step.phase, "to": step.to_label, "undone": undone, "caches": step.caches.as_str()});
    let _ = e.audit().append(super::store::AuditEntry { event: "owner_change_aborted".into(), cmd_id: Some(step.id.clone()), detail: detail.clone(), ..Default::default() }, e.now());
    crate::nlog!("admin: owner change {} aborted at {:?}", step.id, step.phase);
    Ok(detail)
}

/// Did the trees step stage anything for this change?
fn stage_started(e: &Executor, step: &Step) -> bool {
    open_dir(&e.paths.layout().tenants_root().join(STAGE_DIR).join(work_name(step))).is_ok()
}

/// Remove the node's instruction now (`unlinkat` on the directory fd,
/// never following a link).
fn retire_node_change_now(e: &Executor) {
    if let Ok(dir) = open_dir(&super::update::mark_dir(&e.paths.config_dir)) {
        use std::os::fd::AsRawFd;
        // SAFETY: unlinkat a fixed name in our directory fd.
        unsafe { libc::unlinkat(dir.as_raw_fd(), cname(NODE_FILE).as_ptr(), 0) };
    }
}

/// On executor start: resume an unfinished owner change in the background,
/// again after `busy` (jobs still running, control just held), until it's
/// done or waits for a person; then put control back.
pub fn spawn_resume(e: &Executor) {
    if !matches!(read_step(e), Ok(Some(_))) {
        retire_node_change(e);
        return;
    }
    let ex = e.bg_handle();
    std::thread::spawn(move || loop {
        match resume(&ex, false) {
            None => return,
            Some(Ok(_)) => {
                super::update::recover_drain(&ex);
                return;
            }
            Some(Err(r)) if r.code == code::BUSY => std::thread::sleep(ex.drain_settle.max(std::time::Duration::from_millis(if cfg!(test) { 20 } else { 2000 }))),
            Some(Err(r)) => {
                crate::nlog!("admin: owner change not finished: {} ({})", r.message, r.code);
                if read_step(&ex).ok().flatten().is_none_or(|s| s.attempts >= MAX_ATTEMPTS) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(if cfg!(test) { 20 } else { 60_000 }));
            }
        }
    });
}

// ── 1. trees ─────────────────────────────────────────────────────────

fn cname(s: &str) -> CString {
    CString::new(s).expect("no NUL in a fixed name")
}

/// `<home>-t`: root's own (0711) when root; a test or `--dev` node's own.
fn open_troot(ctx: &Ctx) -> Result<OwnedFd, AdminRefusal> {
    let p = ctx.layout().tenants_root();
    if ctx.root {
        crate::wall::open_root_owned_dir(&p).map_err(failed)
    } else {
        std::fs::create_dir_all(&p).map_err(|x| failed(format!("create {}: {x}", p.display())))?;
        open_dir(&p).map_err(failed)
    }
}

fn io(what: &str) -> impl Fn(std::io::Error) -> AdminRefusal + '_ {
    move |x| failed(format!("{what}: {x}"))
}

/// A tree folder in `parent`, made (and given to `owner`) when missing.
fn tree_dir(parent: &OwnedFd, name: &str, owner: Option<(u32, u32)>, mode: u32) -> Result<OwnedFd, AdminRefusal> {
    let n = cname(name);
    let made = !crate::wall::exists_at(parent, &n);
    let fd = crate::wall::ensure_child_dir(parent, &n, mode).map_err(io(name))?;
    if made {
        if let Some((u, g)) = owner {
            use std::os::fd::AsRawFd;
            // SAFETY: our own fresh directory fd.
            if unsafe { libc::fchown(fd.as_raw_fd(), u, g) } != 0 {
                return Err(failed(format!("chown {name}: {}", std::io::Error::last_os_error())));
            }
        }
    }
    Ok(fd)
}

/// Move every entry of `src` into `dst` (a name already in `dst` stays
/// behind; the staging folder's removal takes it).
fn move_entries(src: &OwnedFd, dst: &OwnedFd, what: &str) -> Result<u64, AdminRefusal> {
    let mut n = 0;
    for name in crate::wall::entry_names(src).map_err(io(what))? {
        if crate::wall::exists_at(dst, &name) {
            crate::nlog!("admin: owner change: {what}/{} is in both places; the old copy goes", name.to_string_lossy());
            continue;
        }
        crate::wall::rename_between(src, &name, dst, &name).map_err(io(what))?;
        n += 1;
    }
    crate::wall::sync_dir(src);
    crate::wall::sync_dir(dst);
    Ok(n)
}

fn node_ids(ctx: &Ctx) -> Option<(u32, u32)> {
    if ctx.root {
        super::server::user_ids(&ctx.node_user)
    } else {
        None
    }
}

/// Move every entry of `src` into the root-only trash `dst` (renames only:
/// nothing a job planted, however deep, can make this fail; a name already
/// in the trash gets a random suffix).
fn trash_entries(src: &OwnedFd, dst: &OwnedFd, what: &str) -> Result<(), AdminRefusal> {
    for name in crate::wall::entry_names(src).map_err(io(what))? {
        let mut to = name.clone();
        if crate::wall::exists_at(dst, &to) {
            let r = crypto::random_id().map_err(failed)?;
            to = CString::new(format!("{}.{}", name.to_string_lossy(), &r[..12.min(r.len())])).map_err(|x| failed(x.to_string()))?;
        }
        crate::wall::rename_between(src, &name, dst, &to).map_err(io(what))?;
    }
    crate::wall::sync_dir(src);
    crate::wall::sync_dir(dst);
    Ok(())
}

/// Empty `trees` of each `(tag, parent)` into the trash folder.
fn trash_trees(parents: &[(&str, &OwnedFd)], trees: &[&str], trash: &OwnedFd) -> Result<(), AdminRefusal> {
    for (tag, parent) in parents {
        for t in trees {
            let Ok(src) = crate::wall::open_child_dir(parent, &cname(t)) else { continue };
            let dst = crate::wall::ensure_child_dir(trash, &cname(&format!("{tag}-{t}")), 0o700).map_err(io(t))?;
            trash_entries(&src, &dst, t)?;
        }
    }
    Ok(())
}

/// The change's own staging folder name (its id when that's a plain name).
fn work_name(step: &Step) -> String {
    let ok = (1..=80).contains(&step.id.len()) && step.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        format!("change-{}", step.id)
    } else {
        "change".into()
    }
}

/// Step 1 (see the module doc). Both slots' `tmp/` (sccache sockets,
/// scratch) never move: they go to the root-only trash. With `caches =
/// wipe`, so do the trees, by rename: a wipe can never fail half-way
/// because of what a job left behind (the trash is removed afterwards, best
/// effort; review F2). With `move`, idempotent per entry: phase A empties
/// both slots' trees into this change's staging folder (`from/` = the old
/// owner's, `to/` = the target's), the marker `b` says it's done, phase B
/// fills each slot from the other's staging.
fn move_trees(e: &Executor, ctx: &Ctx, step: &Step) -> Result<(), AdminRefusal> {
    let home = open_dir(&e.paths.home).map_err(failed)?;
    let troot = open_troot(ctx)?;
    let nu = node_ids(ctx);
    let slot = tree_dir(&troot, &step.slot.to_string(), nu, 0o700)?;
    let stage_root = crate::wall::ensure_child_dir(&troot, &cname(STAGE_DIR), 0o700).map_err(io(STAGE_DIR))?;
    // One trash per volume: the slot's in `<home>-t`, the owner's in the
    // node home, so a rename never crosses a filesystem (verification #4).
    let slot_trash = crate::wall::ensure_child_dir(&stage_root, &cname("trash"), 0o700).map_err(io("trash"))?;
    let home_trash = home_trash_dir(&home, ctx.root)?;
    trash_trees(&[("owner", &home)], &["tmp"], &home_trash)?;
    trash_trees(&[("slot", &slot)], &["tmp"], &slot_trash)?;
    if step.caches == Caches::Wipe {
        trash_trees(&[("owner", &home)], TREES, &home_trash)?;
        trash_trees(&[("slot", &slot)], TREES, &slot_trash)?;
        return Ok(());
    }
    if crate::wall::device_of(&home).map_err(io("home"))? != crate::wall::device_of(&troot).map_err(io("tenants root"))? {
        return Err(failed("the node home and the tenants root are on different volumes; nothing moved (use caches wipe)"));
    }
    let stage = crate::wall::ensure_child_dir(&stage_root, &cname(&work_name(step)), 0o700).map_err(io(STAGE_DIR))?;
    let from = crate::wall::ensure_child_dir(&stage, &cname("from"), 0o700).map_err(io("from"))?;
    let to = crate::wall::ensure_child_dir(&stage, &cname("to"), 0o700).map_err(io("to"))?;
    let marker = cname("b");
    if !crate::wall::exists_at(&stage, &marker) {
        for t in TREES {
            if let Ok(src) = crate::wall::open_child_dir(&home, &cname(t)) {
                let dst = crate::wall::ensure_child_dir(&from, &cname(t), 0o700).map_err(io(t))?;
                move_entries(&src, &dst, t)?;
            }
            if let Ok(src) = crate::wall::open_child_dir(&slot, &cname(t)) {
                let dst = crate::wall::ensure_child_dir(&to, &cname(t), 0o700).map_err(io(t))?;
                move_entries(&src, &dst, t)?;
            }
        }
        crate::wall::ensure_child_dir(&stage, &marker, 0o700).map_err(io("marker"))?;
        crate::wall::sync_dir(&stage);
    }
    for t in TREES {
        if let Ok(src) = crate::wall::open_child_dir(&to, &cname(t)) {
            let dst = tree_dir(&home, t, nu, 0o770)?;
            move_entries(&src, &dst, t)?;
        }
        if let Ok(src) = crate::wall::open_child_dir(&from, &cname(t)) {
            let dst = tree_dir(&slot, t, nu, 0o700)?;
            move_entries(&src, &dst, t)?;
        }
    }
    Ok(())
}

/// The owner side's trash in the node home: root's own (0700). One the
/// node user made or changed (it owns the home) is moved aside, never
/// used: root never moves trees into a folder another user controls.
fn home_trash_dir(home: &OwnedFd, root: bool) -> Result<OwnedFd, AdminRefusal> {
    let n = cname(HOME_TRASH);
    if root && crate::wall::exists_at(home, &n) {
        let ok = crate::wall::open_child_dir(home, &n).ok().and_then(|fd| crate::wall::owner_and_mode(&fd).ok()).is_some_and(|(uid, mode)| uid == 0 && mode & 0o077 == 0);
        if !ok {
            let r = crypto::random_id().map_err(failed)?;
            let aside = CString::new(format!("{HOME_TRASH}.not-root.{}", &r[..12.min(r.len())])).map_err(|x| failed(x.to_string()))?;
            crate::wall::rename_between(home, &n, home, &aside).map_err(io(HOME_TRASH))?;
            crate::nlog!("admin: owner change: {HOME_TRASH} wasn't root's own; moved aside");
        }
    }
    crate::wall::ensure_child_dir(home, &n, 0o700).map_err(io(HOME_TRASH))
}

/// Remove `name` in `parent` best effort. A tree deeper than the remover
/// goes (`MAX_DEPTH`) is flattened first: whatever sits 128 levels down
/// is renamed up to the tree's top under a fresh name, again on each pass, so a
/// planted tree goes in the end (verification #6). Returns whether it's gone.
fn remove_deep(parent: &OwnedFd, name: &CStr) -> bool {
    for _ in 0..64 {
        if !crate::wall::exists_at(parent, name) {
            return true;
        }
        match crate::wall::remove_tree_at(parent, name) {
            Ok(()) => return true,
            Err(x) => {
                if !flatten_at(parent, name, 128) {
                    crate::nlog!("admin: owner change: removing {}: {x} (retried on the next prune)", name.to_string_lossy());
                    return false;
                }
            }
        }
    }
    false
}

/// Move every entry `depth` levels below `name` up into `name` itself
/// (so it stays inside the tree being removed). False when nothing that
/// deep was found (the failure is something else).
fn flatten_at(parent: &OwnedFd, name: &CStr, depth: u32) -> bool {
    let Ok(top) = crate::wall::open_child_dir(parent, name) else { return false };
    let Ok(first) = top.try_clone() else { return false };
    let mut level = vec![first];
    for _ in 1..depth {
        let mut next = Vec::new();
        for d in &level {
            for n in crate::wall::entry_names(d).unwrap_or_default() {
                if let Ok(c) = crate::wall::open_child_dir(d, &n) {
                    next.push(c);
                }
            }
        }
        next.truncate(256);
        if next.is_empty() {
            return false;
        }
        level = next;
    }
    let mut moved = false;
    for d in &level {
        for n in crate::wall::entry_names(d).unwrap_or_default() {
            let Ok(r) = crypto::random_id() else { return moved };
            let Ok(to) = CString::new(format!(".flat-{}", &r[..16.min(r.len())])) else { continue };
            if crate::wall::rename_between(d, &n, &top, &to).is_ok() {
                moved = true;
            }
        }
    }
    moved
}

/// Remove the staging folder and the home trash (after the trees step, a
/// leftover, and on every prune while no change runs). Best effort.
pub(crate) fn remove_stage(e: &Executor) -> bool {
    let mut clean = true;
    if let Ok(fd) = open_dir(&e.paths.layout().tenants_root()) {
        clean &= remove_deep(&fd, &cname(STAGE_DIR));
    }
    if let Ok(fd) = open_dir(&e.paths.home) {
        let ok = crate::wall::open_child_dir(&fd, &cname(HOME_TRASH)).ok().and_then(|t| crate::wall::owner_and_mode(&t).ok()).is_some_and(|(uid, _)| uid == unsafe { libc::geteuid() });
        if ok {
            clean &= remove_deep(&fd, &cname(HOME_TRASH));
        }
    }
    clean
}

/// The prune's sweep: leftover staging and trash go while no change runs.
pub(crate) fn sweep_leftovers(e: &Executor) {
    if !matches!(read_step(e), Ok(None)) {
        return;
    }
    let stage = e.paths.layout().tenants_root().join(STAGE_DIR);
    let trash = e.paths.home.join(HOME_TRASH);
    let there = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if !there(&stage) && !there(&trash) {
        return;
    }
    if !remove_stage(e) {
        crate::nlog!("admin: owner change leftovers (staging, trash) not all removed; retried on the next prune");
    } else if !there(&stage) && !there(&trash) {
        crate::nlog!("admin: owner change leftovers (staging, trash) removed");
    }
}

// ── 3. the node's instruction ────────────────────────────────────────

fn write_node_change(e: &Executor, step: &Step) -> Result<(), AdminRefusal> {
    let n = NodeChange {
        v: STEP_V,
        id: step.id.clone(),
        slot: step.slot,
        from_fp: step.from_fp.clone(),
        to_fp: step.to_fp.clone(),
        line_mark: old_line_mark(&step.from_fp, step.at),
        // Written now, so a late resume is never ignored as too old (the
        // line mark keeps the change's own time; review F3).
        at: e.now(),
    };
    let cfg = &e.paths.config_dir;
    super::update::ensure_mark_dir(cfg).map_err(failed)?;
    let b = serde_json::to_vec(&n).map_err(|x| failed(x.to_string()))?;
    super::update::write_owned_in(&super::update::mark_dir(cfg), NODE_FILE, &b, 0o644, super::update::mark_owner(cfg)).map_err(failed)
}

/// The node's instruction, as the node reads it: only when root (the
/// mark's owner) wrote it, in a directory only root writes.
pub fn read_node_change(config_dir: &std::path::Path) -> Option<NodeChange> {
    let b = super::update::read_owned_in(&super::update::mark_dir(config_dir), NODE_FILE, super::update::mark_owner(config_dir), true)?;
    let n: NodeChange = serde_json::from_slice(&b).ok()?;
    let ok = share::valid_slot(n.slot)
        && n.slot != OWNER_SLOT
        && share::valid_fp(&n.from_fp)
        && share::valid_fp(&n.to_fp)
        && n.from_fp != n.to_fp
        && (1..=80).contains(&n.id.len())
        && n.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then_some(n)
}

/// Remove an instruction past its age (root, at start; `unlinkat` on the
/// directory fd, never following a link).
fn retire_node_change(e: &Executor) {
    let cfg = &e.paths.config_dir;
    let Some(n) = read_node_change(cfg) else { return };
    if e.now() - n.at > NODE_FILE_MAX_AGE_SECS {
        retire_node_change_now(e);
    }
}

// ── 4. files ─────────────────────────────────────────────────────────

/// `servers.toml` (MT31): the target at slot 0 (its limits, no end date),
/// the old owner at slot N (`own`, active or revoked), every other line
/// untouched. A file that names the target as owner already is done.
fn rewrite_servers(ctx: &Ctx, step: &Step) -> Result<(), AdminRefusal> {
    // Root's key record for the old owner's new line: its key is the one
    // root pinned as controller (admin.toml), known for certain.
    servers::record_key(&ctx.state_dir, step.slot, &step.from_fp, step.at).map_err(failed)?;
    let mut f = servers::read_file(&ctx.config_dir).map_err(failed)?.ok_or_else(|| failed("servers.toml is gone"))?;
    if f.owner == step.to_fp {
        return Ok(());
    }
    if f.owner != step.from_fp {
        return Err(failed("servers.toml names neither the old nor the new owner; fix it at the machine"));
    }
    let f2 = swap_lines(&f, step, ctx.macos)?;
    f = f2;
    servers::write_file(&ctx.config_dir, &f).map_err(failed)
}

/// The rewritten file (pure; see [`rewrite_servers`]). The old owner's new
/// line gets a new server's default share, not the owner's limits: a raise
/// goes through the usual approval (review F9).
pub fn swap_lines(f: &ServersFile, step: &Step, macos: bool) -> Result<ServersFile, AdminRefusal> {
    let to = f.entry(step.slot).filter(|e| e.matches_fp(&step.to_fp)).cloned().ok_or_else(|| failed(format!("slot {} no longer holds {}", step.slot, step.to_label)))?;
    let from = f.entry(OWNER_SLOT).cloned().ok_or_else(|| failed("no owner line"))?;
    let by = format!("owner-change:{}", step.id);
    let mut owner = to;
    owner.slot = OWNER_SLOT;
    owner.fp_prefix = step.to_fp.clone();
    owner.trust = Trust::Own;
    owner.state = ShareState::Active;
    owner.uid = None;
    owner.expires_at = None;
    owner.pending_until = 0;
    owner.added_by = by.clone();
    let mut old = from;
    old.slot = step.slot;
    old.fp_prefix = step.from_fp[..16].to_string();
    old.trust = Trust::Own;
    old.state = if step.old == OldOwner::Revoke { ShareState::Revoked } else { ShareState::Active };
    old.set_limits(&share::ShareLimits::new_server(Trust::Own, macos));
    old.uid = None;
    old.expires_at = None;
    old.pending_until = 0;
    old.added_at = step.at;
    old.added_by = by;
    let mut out = f.clone();
    out.owner = step.to_fp.clone();
    out.servers = f.servers.iter().filter(|e| e.slot != OWNER_SLOT && e.slot != step.slot).cloned().collect();
    out.servers.push(owner);
    out.servers.push(old);
    out.servers.sort_by_key(|e| e.slot);
    out.validate().map_err(failed)?;
    Ok(out)
}

/// `old = keep`: the old owner's new user in slot N (`tenant.prepare`, a
/// new uid), recorded in its line.
fn prepare_old_owner(ctx: &Ctx, step: &Step) -> Result<Option<u32>, AdminRefusal> {
    let mut f = servers::read_file(&ctx.config_dir).map_err(failed)?.ok_or_else(|| failed("servers.toml is gone"))?;
    let line = f.entry(step.slot).cloned().ok_or_else(|| failed(format!("slot {} has no line", step.slot)))?;
    let uid = servers::prepare_tenant(ctx, &line)?;
    if uid.is_some() && line.uid != uid {
        if let Some(x) = f.servers.iter_mut().find(|x| x.slot == step.slot) {
            x.uid = uid;
        }
        servers::write_file(&ctx.config_dir, &f).map_err(failed)?;
    }
    Ok(uid)
}

/// `old = revoke`: slot N (now the old owner's) is written revoked, as a
/// plain revoke: the node opens it just long enough to tell the old owner
/// its queued jobs ended `share_revoked` (review F10), and the slot is
/// wiped 24 h later by the prune, which retries a wipe that fails. Nothing
/// here can fail the change (review F2).
fn revoke_old_owner(ctx: &Ctx, step: &Step) -> Result<(), AdminRefusal> {
    let f = servers::read_file(&ctx.config_dir).map_err(failed)?.ok_or_else(|| failed("servers.toml is gone"))?;
    if f.entry(step.slot).is_none() {
        return Ok(());
    }
    let mut b = servers::read_book(&ctx.state_dir).map_err(failed)?;
    b.revoked.entry(step.slot.to_string()).or_insert((ctx.now, false));
    servers::write_book(&ctx.state_dir, &b).map_err(failed)
}

/// The moved trees, re-owned for their new slot (Linux, root): the new
/// owner's to the node user and its group, ACLs stripped (the old slot's
/// group entries go); the old owner's to the node user and slot N's new
/// group. macOS: every tree is the node user's already (one uid behind
/// Seatbelt in 0.46.4), so nothing to do.
fn rehome(e: &Executor, ctx: &Ctx, step: &Step) -> Result<Value, AdminRefusal> {
    if !ctx.root || !cfg!(target_os = "linux") || step.caches == Caches::Wipe {
        return Ok(Value::Null);
    }
    let (nuid, ngid) = super::server::user_ids(&ctx.node_user).ok_or_else(|| failed("the node user is missing"))?;
    let home = open_dir(&e.paths.home).map_err(failed)?;
    let mut sides = vec![(home, nuid, ngid, true, "owner")];
    if step.old == OldOwner::Keep {
        let tgid = super::jobs::group_id(&super::jobs::tenant_group_name(step.slot)).ok_or_else(|| failed(format!("group {} is missing", super::jobs::tenant_group_name(step.slot))))?;
        let slot = open_dir(&ctx.layout().tenant(step.slot).root()).map_err(failed)?;
        sides.push((slot, nuid, tgid, false, "slot"));
    }
    let mut out = serde_json::Map::new();
    for (dir, uid, gid, strip, what) in sides {
        let mut st = crate::wall::WalkStats::default();
        for t in TREES {
            let Ok(d) = crate::wall::open_child_dir(&dir, &cname(t)) else { continue };
            for name in crate::wall::entry_names(&d).map_err(io(t))? {
                let s = crate::wall::rehome_at(&d, &name, uid, gid, strip).map_err(failed)?;
                st.changed += s.changed;
                st.skipped += s.skipped;
            }
        }
        out.insert(what.into(), json!({"changed": st.changed, "skipped": st.skipped}));
    }
    Ok(Value::Object(out))
}

/// `admin.toml` (MT32): the target's key, approval keys cleared (the rule
/// of `admin pin`); the level, `auto_update`, alerts and node user stay.
/// Written atomically (temp file, rename) and read back: anything but the
/// file just written fails the step (it's retried on resume).
fn repin_admin(e: &Executor, step: &Step) -> Result<(), AdminRefusal> {
    let path = e.paths.admin();
    let cfg = super::config::read(&path).map_err(failed)?.ok_or_else(|| failed("admin.toml is gone"))?;
    let cur = cfg.controller.as_ref().map(|c| c.fp.clone());
    if cur.as_deref() == Some(step.to_fp.as_str()) {
        return Ok(());
    }
    if cur.as_deref() != Some(step.from_fp.as_str()) {
        return Err(failed("admin.toml pins neither the old nor the new owner; fix it at the machine"));
    }
    let der = crypto::unb64(&step.to_key).map_err(|x| failed(format!("the new owner's key: {x}")))?;
    if crypto::fingerprint_of_spki_der(&der) != step.to_fp {
        return Err(failed("the recorded key doesn't match the new owner's fingerprint"));
    }
    let mut f = cfg.file.clone();
    f.controller_fp = Some(step.to_fp.clone());
    f.controller_key = Some(step.to_key.clone());
    f.approval_keys.clear();
    super::config::write(&path, &f).map_err(failed)?;
    let back = super::config::read(&path).map_err(failed)?.ok_or_else(|| failed("admin.toml vanished after the write"))?;
    let same = back.controller.as_ref().is_some_and(|c| c.fp == step.to_fp && c.spki_der == der)
        && back.approval_keys.is_empty()
        && back.level == cfg.level
        && back.auto_update == cfg.auto_update
        && back.node_user == cfg.node_user;
    if !same {
        return Err(failed("admin.toml didn't read back as written"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
