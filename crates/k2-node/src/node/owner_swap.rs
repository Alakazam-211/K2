//! The node's half of `servers.owner` (prd-macos-tenant-compute-v1 §6.2
//! step 4): at start, before it opens a pin or a ledger, the node swaps
//! its own files between slot 0 and slot N when root's instruction says so
//! (`admin::owner::NodeChange`, in the root-only mark directory).
//!
//! - the pins: `state/controller.json` (the owner's `Pin`) and
//!   `state/servers/<N>.json` (a tenant pin with the node's flags);
//! - the ledgers (`ledger.sqlite` with its `-wal` and `-shm`), so queued
//!   jobs stay with their server, each with its own `ledger_id`;
//! - the kept artifacts (`artifacts/` and `state/tenants/<N>/artifacts`);
//! - slot N's line mark (the old owner's new line);
//! - the queue store's slot rows (once per change id).
//!
//! Nothing is open yet, so plain renames move the WAL with its database.
//! Crash safe: phase A moves both sides into a staging folder in `state/`
//! (`owner-change/<id>/from|to`), a marker says it's done, phase B moves
//! each into the other's place; every rename has a clear before and after.
//! It acts only while the files are as the instruction says (the owner's
//! pin is `from` and slot N's is `to`), finishes a staged swap, and
//! otherwise leaves everything alone: a re-run never swaps back. An
//! instruction older than 7 days is ignored.

use std::path::{Path, PathBuf};

use crate::admin::owner::{NodeChange, NODE_FILE_MAX_AGE_SECS};
use crate::paths::Layout;

const STAGE: &str = "owner-change";
const APPLIED: &str = "owner-change-applied";

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No instruction, or one that's too old or done.
    Nothing,
    /// The files were swapped now (or a staged swap finished).
    Swapped,
    /// The files match neither side: left alone.
    Mismatch,
}

fn items(layout: &Layout, slot: u8) -> [(&'static str, PathBuf, PathBuf); 5] {
    let t = layout.tenant(slot);
    let owner_ledger = layout.ledger();
    let tenant_ledger = t.ledger();
    let wal = |p: &Path, sfx: &str| PathBuf::from(format!("{}{sfx}", p.display()));
    [
        ("pin", layout.pin(), t.pin()),
        ("ledger.sqlite", owner_ledger.clone(), tenant_ledger.clone()),
        ("ledger.sqlite-wal", wal(&owner_ledger, "-wal"), wal(&tenant_ledger, "-wal")),
        ("ledger.sqlite-shm", wal(&owner_ledger, "-shm"), wal(&tenant_ledger, "-shm")),
        ("artifacts", layout.tenant(0).artifacts(), t.artifacts()),
    ]
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

fn mv(from: &Path, to: &Path) -> Result<(), String> {
    if !exists(from) || exists(to) {
        return Ok(());
    }
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::rename(from, to).map_err(|e| format!("move {} → {}: {e}", from.display(), to.display()))
}

fn fsync_dir(p: &Path) {
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.sync_all();
    }
}

fn applied(layout: &Layout) -> Option<String> {
    std::fs::read_to_string(layout.state().join(APPLIED)).ok().map(|s| s.trim().to_string())
}

/// Swap if root's instruction says so (see the module doc). `Err` stops
/// the node's start: launchd/systemd start it again and it finishes.
pub fn apply(layout: &Layout, now: i64) -> Result<Outcome, String> {
    let Some(n) = crate::admin::owner::read_node_change(&layout.config) else { return Ok(Outcome::Nothing) };
    apply_change(layout, &n, now)
}

pub fn apply_change(layout: &Layout, n: &NodeChange, now: i64) -> Result<Outcome, String> {
    if now - n.at > NODE_FILE_MAX_AGE_SECS || applied(layout).as_deref() == Some(n.id.as_str()) {
        return Ok(Outcome::Nothing);
    }
    let stage = layout.state().join(STAGE).join(&n.id);
    if !exists(&stage) {
        let owner_fp = crate::identity::read_pin(&layout.pin()).ok().flatten().map(|p| p.controller_fp);
        let tenant_fp = crate::identity::read_tenant_pin(&layout.tenant(n.slot).pin()).ok().flatten().map(|(p, _)| p.controller_fp);
        match (owner_fp.as_deref(), tenant_fp.as_deref()) {
            (Some(o), Some(t)) if o == n.from_fp && t == n.to_fp => {}
            // Swapped already (the staging folder went last): the queue rows
            // may still be due.
            (Some(o), _) if o == n.to_fp => return finish(layout, n, &stage).map(|_| Outcome::Swapped),
            _ => {
                crate::nlog!("owner change {}: this node's pins match neither side; nothing swapped", n.id);
                return Ok(Outcome::Mismatch);
            }
        }
    }
    crate::util::mkdir_mode(&layout.state().join(STAGE), 0o700)?;
    crate::util::mkdir_mode(&stage, 0o700)?;
    let (from, to) = (stage.join("from"), stage.join("to"));
    crate::util::mkdir_mode(&from, 0o700)?;
    crate::util::mkdir_mode(&to, 0o700)?;
    let marker = stage.join("b");
    if !exists(&marker) {
        for (name, owner, tenant) in items(layout, n.slot) {
            mv(&owner, &from.join(name))?;
            mv(&tenant, &to.join(name))?;
        }
        fsync_dir(&from);
        fsync_dir(&to);
        std::fs::create_dir(&marker).or_else(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) }).map_err(|e| format!("create {}: {e}", marker.display()))?;
        fsync_dir(&stage);
    }
    // Only the parents now: a fresh `artifacts/` would block the move.
    for d in [layout.state().join("servers"), layout.state().join("tenants"), layout.tenant(n.slot).state()] {
        crate::util::mkdir_mode(&d, 0o700)?;
    }
    for (name, owner, tenant) in items(layout, n.slot) {
        match name {
            // The pins change format: the owner's is a plain `Pin`, a
            // tenant's carries the node's flags (fresh for the old owner).
            "pin" => {
                if let Some((p, _)) = crate::identity::read_tenant_pin(&to.join(name))? {
                    crate::identity::write_pin(&owner, &p)?;
                }
                if let Some(p) = crate::identity::read_pin(&from.join(name))? {
                    crate::identity::write_tenant_pin(&tenant, &p, &Default::default())?;
                }
            }
            _ => {
                mv(&to.join(name), &owner)?;
                mv(&from.join(name), &tenant)?;
            }
        }
    }
    layout.tenant(n.slot).ensure_state()?;
    // The old owner's state in slot N belongs to its new line.
    crate::util::atomic_write(&layout.tenant(n.slot).state().join(crate::session::LINE_MARK_FILE), n.line_mark.as_bytes(), 0o600)?;
    fsync_dir(&layout.state());
    finish(layout, n, &stage)?;
    crate::nlog!("owner change {}: swapped the owner's and slot {}'s files", n.id, n.slot);
    Ok(Outcome::Swapped)
}

/// The queue rows (once per id), the done record, the staging folder.
fn finish(layout: &Layout, n: &NodeChange, stage: &Path) -> Result<(), String> {
    let mut q = crate::queue_store::QueueStore::open(&layout.state().join("queue.sqlite"))?;
    q.swap_slots(0, n.slot, &n.id)?;
    drop(q);
    crate::util::atomic_write(&layout.state().join(APPLIED), n.id.as_bytes(), 0o600)?;
    if exists(stage) {
        std::fs::remove_dir_all(stage).map_err(|e| format!("remove {}: {e}", stage.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Pin, TenantFlags};

    fn pin(fp: &str) -> Pin {
        let mut p = Pin::for_test("n0");
        p.controller_fp = fp.to_string();
        p.routes = vec![format!("https://{}.k2.dev", &fp[..4])];
        p
    }

    fn setup() -> (Layout, NodeChange) {
        let d = crate::util::temp_dir("owner-swap");
        let layout = Layout::new(d.join("home"), d.join("cfg"));
        layout.ensure().unwrap();
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        crate::identity::write_pin(&layout.pin(), &pin(&a)).unwrap();
        let t = layout.tenant(2);
        t.ensure_state().unwrap();
        crate::identity::write_tenant_pin(&t.pin(), &pin(&b), &TenantFlags { last_seen: Some(5), ..Default::default() }).unwrap();
        std::fs::write(layout.ledger(), b"OWNER-LEDGER").unwrap();
        std::fs::write(format!("{}-wal", layout.ledger().display()), b"OWNER-WAL").unwrap();
        std::fs::write(t.ledger(), b"B-LEDGER").unwrap();
        std::fs::create_dir_all(layout.tenant(0).artifacts()).unwrap();
        std::fs::write(layout.tenant(0).artifacts().join("a.txt"), b"owner artifact").unwrap();
        std::fs::write(t.state().join(crate::session::LINE_MARK_FILE), b"bbbbbbbbbbbbbbbb:10").unwrap();
        let n = NodeChange { v: 1, id: "c1".into(), slot: 2, from_fp: a.clone(), to_fp: b, line_mark: format!("{}:500", &a[..16]), at: 500 };
        (layout, n)
    }

    fn read(p: impl AsRef<Path>) -> String {
        std::fs::read_to_string(p).unwrap()
    }

    /// MT30: pins, ledgers (with the WAL), artifacts and the line mark
    /// trade places; a second start changes nothing.
    #[test]
    fn swaps_the_owner_and_slot_files_once() {
        let (layout, n) = setup();
        assert_eq!(apply_change(&layout, &n, 600).unwrap(), Outcome::Swapped);
        let t = layout.tenant(2);
        assert_eq!(crate::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, n.to_fp, "the target owns the node now");
        let (old, flags) = crate::identity::read_tenant_pin(&t.pin()).unwrap().unwrap();
        assert_eq!((old.controller_fp.as_str(), flags), (n.from_fp.as_str(), TenantFlags::default()), "the old owner is slot 2, fresh flags");
        assert_eq!((read(layout.ledger()), read(t.ledger())), ("B-LEDGER".to_string(), "OWNER-LEDGER".to_string()), "each ledger follows its server");
        assert_eq!(read(format!("{}-wal", t.ledger().display())), "OWNER-WAL", "the WAL moves with its database");
        assert!(!exists(&PathBuf::from(format!("{}-wal", layout.ledger().display()))));
        assert_eq!(read(t.artifacts().join("a.txt")), "owner artifact");
        assert_eq!(read(t.state().join(crate::session::LINE_MARK_FILE)), n.line_mark);
        assert!(!exists(&layout.state().join(STAGE).join("c1")), "the staging folder is gone");
        assert_eq!(apply_change(&layout, &n, 601).unwrap(), Outcome::Nothing, "done: never swapped back");
        assert_eq!(crate::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, n.to_fp);
    }

    /// MT33: a start that died in phase A or B finishes the swap.
    #[test]
    fn a_crash_mid_swap_finishes_on_the_next_start() {
        for crash_after_b in [false, true] {
            let (layout, n) = setup();
            // Phase A half done: the owner's ledger is in staging already.
            let stage = layout.state().join(STAGE).join(&n.id);
            std::fs::create_dir_all(stage.join("from")).unwrap();
            std::fs::create_dir_all(stage.join("to")).unwrap();
            std::fs::rename(layout.ledger(), stage.join("from").join("ledger.sqlite")).unwrap();
            if crash_after_b {
                // Phase A done, phase B started: the target's ledger is in place.
                for (name, owner, tenant) in items(&layout, 2) {
                    mv(&owner, &stage.join("from").join(name)).unwrap();
                    mv(&tenant, &stage.join("to").join(name)).unwrap();
                }
                std::fs::create_dir(stage.join("b")).unwrap();
                mv(&stage.join("to").join("ledger.sqlite"), &layout.ledger()).unwrap();
            }
            assert_eq!(apply_change(&layout, &n, 600).unwrap(), Outcome::Swapped, "crash_after_b={crash_after_b}");
            assert_eq!(read(layout.ledger()), "B-LEDGER");
            assert_eq!(read(layout.tenant(2).ledger()), "OWNER-LEDGER");
            assert_eq!(crate::identity::read_pin(&layout.pin()).unwrap().unwrap().controller_fp, n.to_fp);
        }
    }

    /// Files that match neither side (a re-enrol since), a stale or an
    /// applied instruction: nothing moves.
    #[test]
    fn leaves_other_files_alone() {
        let (layout, mut n) = setup();
        n.from_fp = "c".repeat(64);
        assert_eq!(apply_change(&layout, &n, 600).unwrap(), Outcome::Mismatch);
        assert_eq!(read(layout.ledger()), "OWNER-LEDGER");
        let (layout, n) = setup();
        assert_eq!(apply_change(&layout, &n, n.at + NODE_FILE_MAX_AGE_SECS + 1).unwrap(), Outcome::Nothing, "too old");
        assert_eq!(read(layout.ledger()), "OWNER-LEDGER");
    }
}
