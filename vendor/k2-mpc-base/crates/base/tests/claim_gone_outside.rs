//! A claim on elements whose last live element an **outside save** removed ends with its own
//! reason, `ClaimEnd::GoneOutsideSave`, and its end events carry the claim's element addresses as
//! they were, so a view can say what went (a timeline track renamed in another app gets a new
//! key, and the old one goes). A live delete still ends `ClaimEnd::Deleted`. On a keyed doc that
//! isn't a table, opened from a file (`keyed_pack::KeyedFile`: an outside save applies by key;
//! an item's address is `@item:<key>`). Public API only.
//!
//! Planted faults (`k2_mpc_base::differential::ClaimMutant`) are caught by name: each
//! `plant_*_is_caught` row needs its check to pass as built and to fail under the fault where it
//! says.

mod keyed_pack;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::{with_claim_mutant, ClaimMutant};
use k2_mpc_base::doc::Edit;
use k2_mpc_base::elem::ListOp;
use k2_mpc_base::session::{
    AuditEvent, ClaimEnd, ClaimId, Config, ConnId, DiskCheck, EditOptions, Host, Participant,
    ParticipantKind, Rights, RosterEvent, Session, SessionManager, Stay,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::Key;
use keyed_pack::{KeyedFile, ITEMS};

struct H {
    now: AtomicU64,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

struct Fx {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
    host: Arc<H>,
    mgr: SessionManager,
    s: Arc<Session>,
    ada: ConnId,
    bot: ConnId,
    roster: Receiver<RosterEvent>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A doc of items `i1` to `i4`, from its file, with a person (Ada, subscribed to the roster) and
/// an agent (Bot).
fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-gone-outside-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let path = ws.join("doc.keyed");
    std::fs::write(&path, "i1\ni2\ni3\ni4\n").unwrap();
    let host = Arc::new(H {
        now: AtomicU64::new(1_700_000_000_000),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), c);
    let key = DocKey {
        space: "ws".into(),
        path: "doc.keyed".into(),
    };
    let s = mgr
        .open_file(Arc::new(KeyedFile), &key, &path, &ws)
        .unwrap();
    let join = |pid: &str, kind| {
        s.join(
            Participant::new(pid, pid, "#2f9e8f", kind, "tab"),
            Stay::Held,
        )
        .unwrap()
        .conn
    };
    let ada = join("ada", ParticipantKind::Person);
    let bot = join("bot", ParticipantKind::Agent);
    let roster = s.subscribe_roster(ada).unwrap();
    Fx {
        dir,
        path,
        host,
        mgr,
        s,
        ada,
        bot,
        roster,
    }
}

impl Fx {
    /// Bot claims `keys` of the items, held.
    fn claim(&self, keys: &[&str]) -> ClaimId {
        let keys: Vec<Key> = keys.iter().map(|k| Key::new(*k).unwrap()).collect();
        self.s
            .claim_elems(self.bot, ITEMS, &keys, None, None, true)
            .unwrap()
            .id
    }
    /// An app saves the file holding `items`, and the kit takes it in.
    fn outside_save(&self, items: &[&str]) -> Result<DiskCheck, String> {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
        let text: String = items.iter().map(|i| format!("{i}\n")).collect();
        std::fs::write(&self.path, text).unwrap();
        self.s.file_changed().map_err(|e| format!("{e}"))
    }
    /// Ada deletes `key` live.
    fn delete_live(&self, key: &str) -> Result<(), String> {
        self.s
            .edit_ops(
                self.ada,
                vec![Edit::List {
                    root: ITEMS.into(),
                    ops: vec![ListOp::Delete {
                        key: Key::new(key).unwrap(),
                    }],
                }],
                &EditOptions::new(),
            )
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }
    /// The claim's audit end, as `(how, addresses)`, if it ended.
    fn audit_end(&self, id: ClaimId) -> Option<(ClaimEnd, Vec<String>)> {
        self.host
            .events
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match e {
                AuditEvent::ClaimEnded {
                    claim,
                    how,
                    addresses,
                    ..
                } if *claim == id => Some((*how, addresses.clone())),
                _ => None,
            })
    }
    /// The claim's roster end, as `(how, addresses)`, if it ended (the roster flushed first, the
    /// host's clock past its window).
    fn roster_end(&self, id: ClaimId) -> Option<(ClaimEnd, Vec<String>)> {
        self.host.now.fetch_add(1_000, Ordering::SeqCst);
        self.mgr.flush_rosters();
        self.roster.try_iter().find_map(|e| match e {
            RosterEvent::ClaimEnded {
                claim,
                how,
                addresses,
                ..
            } if claim == id => Some((how, addresses)),
            _ => None,
        })
    }
}

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

/// An outside save that removes the last live element of a claim ends it `GoneOutsideSave`, and
/// both end events (the host's audit event and the roster's) carry its element addresses as they
/// were, in key order. Control first: a save that removes one of two claimed elements leaves the
/// claim live.
fn row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save() -> Result<(), String>
{
    const ROW: &str = "row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save";
    let f = fx("outside");
    let id = f.claim(&["i3", "i2"]);
    let out = f.outside_save(&["i1", "i3", "i4"])?;
    step(
        matches!(out, DiskCheck::Ingested { .. }) && f.audit_end(id).is_none(),
        ROW,
        "control",
        || {
            format!(
                "a save removing one of two claimed elements: {out:?}, the claim ended {:?}",
                f.audit_end(id)
            )
        },
    )?;
    let out = f.outside_save(&["i1", "i4"])?;
    step(
        matches!(out, DiskCheck::Ingested { .. }),
        ROW,
        "save",
        || format!("the save isn't taken in: {out:?}"),
    )?;
    let want = vec!["@item:i2".to_owned(), "@item:i3".to_owned()];
    let audit = f.audit_end(id);
    step(
        audit.as_ref().map(|(h, _)| *h) == Some(ClaimEnd::GoneOutsideSave),
        ROW,
        "ended",
        || format!("the claim ended {audit:?}, want gone_outside_save"),
    )?;
    step(
        audit.as_ref().map(|(_, a)| a) == Some(&want),
        ROW,
        "addresses",
        || format!("the audit end's addresses {audit:?}, want {want:?}"),
    )?;
    let roster = f.roster_end(id);
    step(
        roster == Some((ClaimEnd::GoneOutsideSave, want.clone())),
        ROW,
        "roster",
        || format!("the roster's end {roster:?}, want gone_outside_save with {want:?}"),
    )
}

#[test]
fn an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save() {
    row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save().unwrap();
}

/// A live delete of the last live element of a claim still ends it `Deleted`, with its element
/// addresses too.
fn row_a_live_delete_still_ends_deleted() -> Result<(), String> {
    const ROW: &str = "row_a_live_delete_still_ends_deleted";
    let f = fx("live");
    let id = f.claim(&["i3"]);
    f.delete_live("i3")?;
    let want = vec!["@item:i3".to_owned()];
    let audit = f.audit_end(id);
    step(
        audit == Some((ClaimEnd::Deleted, want.clone())),
        ROW,
        "ended",
        || format!("the claim ended {audit:?}, want deleted with {want:?}"),
    )?;
    let roster = f.roster_end(id);
    step(
        roster == Some((ClaimEnd::Deleted, want.clone())),
        ROW,
        "roster",
        || format!("the roster's end {roster:?}, want deleted with {want:?}"),
    )
}

#[test]
fn a_live_delete_still_ends_deleted() {
    row_a_live_delete_still_ends_deleted().unwrap();
}

/// `row` passes as built and fails under `mutant` with an error starting `starts`.
fn caught(row: fn() -> Result<(), String>, mutant: ClaimMutant, starts: &str) {
    row().expect("the row passes as built");
    let e = with_claim_mutant(mutant, row).expect_err("the row passes with the fault planted");
    println!("PLANT {mutant:?}: caught: {e}");
    assert!(e.starts_with(starts), "{mutant:?}: {e}");
}

#[test]
fn plant_outside_end_reported_deleted_is_caught() {
    caught(
        row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save,
        ClaimMutant::OutsideEndReportedDeleted,
        "row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save: ended: the \
         claim ended Some((Deleted,",
    );
}

#[test]
fn plant_end_addresses_dropped_is_caught() {
    caught(
        row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save,
        ClaimMutant::EndAddressesDropped,
        "row_an_outside_save_removing_a_claimed_element_ends_it_gone_outside_save: addresses: \
         the audit end's addresses Some((GoneOutsideSave, []))",
    );
}
