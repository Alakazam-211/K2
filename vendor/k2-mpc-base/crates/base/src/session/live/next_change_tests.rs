//! What a change that didn't land leaves for the next one: nothing. The flags a call sets for its
//! change (`verb_hint`, `no_stack`, `next_row_marks`, `next_col_remap`, `next_note_rows`) must not
//! reach anyone else's change when its commit fails or changes nothing.
//!
//! No public call reaches these exits, so the tests use two test-only hooks:
//! [`crate::differential::with_apply_fault`] (commit's apply) and `DocStore::damage_change`
//! (`overridden`'s read, which a transient read error would also fail). Each test first asserts
//! that its first call failed as planted. `tests/grid_store.rs`, `refused_live_write`, has what
//! public calls reach.

use std::sync::Mutex;

use super::*;
use crate::differential::{with_apply_fault, ApplyFault};
use crate::elem::Key;
use crate::session::{
    ClaimSpan, EditOptions, EditWithError, GridWrite, GridWriteOptions, NoteKind, SessionManager,
};
use crate::store::StoreError;
use crate::table::{self, Field, Shape, Table, TableError};

#[derive(Default)]
struct H {
    events: Mutex<Vec<AuditEvent>>,
}
impl Host for H {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        1_000
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
}

struct Lite;
const LITE: Schema = Schema {
    name: "csv-lite",
    roots: table::ROOTS,
};
impl Pack for Lite {
    fn schema(&self) -> Schema {
        LITE
    }
    fn decode(&self, b: &[u8]) -> std::result::Result<super::super::PackContent, Declined> {
        let recs: Vec<Vec<Field>> = std::str::from_utf8(b)
            .unwrap()
            .lines()
            .map(|l| l.split(',').map(Field::new).collect())
            .collect();
        let (_, edits) = Table::import(&recs, false, Shape::detect(&recs)).unwrap();
        let mut d = Doc::new(LITE).unwrap();
        d.edit_as(2, &edits).unwrap();
        Ok(super::super::PackContent {
            roots: d.export_neutral().unwrap(),
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> std::result::Result<Vec<u8>, PackError> {
        let t = View::of_content(c)
            .table()
            .map_err(|e| PackError(e.to_string()))?;
        let mut out = String::new();
        for rec in t.render() {
            let f: Vec<&str> = rec.iter().map(|f| f.value.as_str()).collect();
            out.push_str(&f.join(","));
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

struct Fx {
    dir: std::path::PathBuf,
    host: Arc<H>,
    store: Arc<DocStore>,
    mgr: SessionManager,
    key: DocKey,
}
impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fx(tag: &str, body: &str) -> (Fx, Arc<Session>) {
    let dir = std::env::temp_dir().join(format!("k2mb-nextchange-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let c = Config {
        edit_burst: 10_000,
        edit_rate_per_s: 10_000,
        ..Config::default()
    };
    let host = Arc::new(H::default());
    let mgr = SessionManager::new(store.clone(), host.clone(), c);
    let key = DocKey {
        space: "ws".into(),
        path: format!("{tag}.csv"),
    };
    let s = mgr
        .open(Arc::new(Lite), &key, Some(body.as_bytes()))
        .unwrap();
    (
        Fx {
            dir,
            host,
            store,
            mgr,
            key,
        },
        s,
    )
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

fn join(s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
    s.join(
        Participant::new(pid, pid.to_uppercase(), "#111111", kind, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn op(
    s: &Session,
    c: ConnId,
    opts: &EditOptions,
    f: impl FnOnce(&mut Table) -> std::result::Result<Vec<Edit>, TableError>,
) -> Result<u64> {
    match s.edit_ops_with(c, opts, |v| {
        let mut t = v.table().map_err(|e| e.to_string())?;
        f(&mut t).map_err(|e| e.to_string())
    }) {
        Ok(r) => Ok(r.map_or(0, |r| r.version)),
        Err(EditWithError::Session(e)) => Err(e),
        Err(EditWithError::Compute(e)) => unreachable!("table op: {e}"),
    }
}

fn set(s: &Session, c: ConnId, r: &str, col: &str, v: &str) -> u64 {
    op(s, c, &EditOptions::new(), |t| t.set_cell(&k(r), &k(col), v)).unwrap()
}

fn text(s: &Session, c: ConnId) -> String {
    String::from_utf8(s.read(c).unwrap().bytes).unwrap()
}

fn last_edit_verb(s: &Session, c: ConnId) -> String {
    s.activity(c, 50)
        .unwrap()
        .into_iter()
        .rfind(|a| matches!(a.kind, ActivityKind::Edit))
        .expect("an edit row")
        .verb
}

fn is_rewritten(r: &Result<u64>) -> bool {
    matches!(r, Err(SessionError::FacetConflict(c)) if c.iter().any(|c| c.rewritten_row))
}

/// The error a planted fault makes, as text, to check a first call failed as planted.
fn planted(fault: ApplyFault) -> String {
    match fault {
        ApplyFault::Refused => SessionError::BadEdit("a planted refusal".into()),
        _ => SessionError::Doc(crate::doc::DocError::Pending),
    }
    .to_string()
}

// ---------- no_stack and verb_hint: a failed `rows drop-deleted` ----------

/// A deleted row someone else wrote, so `overridden` reads that change's record. Damaged, the
/// read fails, after the point where a drop set its flags.
fn drop_failing_at_overridden(tag: &str) -> (Fx, Arc<Session>, ConnId) {
    let (f, s) = fx(tag, "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    let p2 = join(&s, "p2", ParticipantKind::Person);
    let seq = set(&s, p2, "r1", "c1", "OTHER");
    op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
    f.store.damage_change(s.doc_id, seq);
    let v = s.read(p).unwrap().version;
    let e = s.drop_deleted(p).expect_err("the drop fails");
    assert!(
        matches!(e, SessionError::Store(StoreError::Corrupt(_))),
        "failed at the damaged record: {e:?}"
    );
    assert_eq!(s.read(p).unwrap().version, v, "nothing applied");
    (f, s, p)
}

fn drop_failing_at_apply(tag: &str, fault: ApplyFault) -> (Fx, Arc<Session>, ConnId) {
    let (f, s) = fx(tag, "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
    let e = with_apply_fault(fault, || s.drop_deleted(p)).expect_err("the drop fails");
    assert_eq!(e.to_string(), planted(fault), "failed as planted");
    (f, s, p)
}

fn next_edit_can_be_undone(s: &Session, p: ConnId) {
    set(s, p, "r2", "c1", "NEXT");
    assert!(text(s, p).contains("NEXT"), "the next edit landed");
    let u = s.undo(p, 1);
    assert!(
        !text(s, p).contains("NEXT"),
        "the edit after a failed drop is off the undo stack (undo: {u:?})"
    );
}

#[test]
fn with_no_failed_drop_the_next_edit_can_be_undone() {
    let (_f, s) = fx("ctl-stack", "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    next_edit_can_be_undone(&s, p);
}

#[test]
fn a_drop_failing_at_its_read_leaves_the_next_edit_on_the_undo_stack() {
    let (_f, s, p) = drop_failing_at_overridden("ns-read");
    next_edit_can_be_undone(&s, p);
}

#[test]
fn a_drop_refused_at_apply_leaves_the_next_edit_on_the_undo_stack() {
    let (_f, s, p) = drop_failing_at_apply("ns-refused", ApplyFault::Refused);
    next_edit_can_be_undone(&s, p);
}

/// `recover` reloads the doc and clears the stacks; the flag can't outlive it either.
#[test]
fn a_drop_poisoned_at_apply_leaves_the_next_edit_on_the_undo_stack() {
    let (_f, s, p) = drop_failing_at_apply("ns-poisoned", ApplyFault::Poisoned);
    next_edit_can_be_undone(&s, p);
}

#[test]
fn a_drop_failing_at_its_read_does_not_label_the_next_change() {
    let (_f, s, p) = drop_failing_at_overridden("vh-read");
    set(&s, p, "r2", "c1", "NEXT");
    assert_eq!(
        last_edit_verb(&s, p),
        "edit",
        "labelled with the failed drop"
    );
}

// ---------- next_row_marks ----------

const MARKED_DOC: &str = "a,1\nc,2\nz,9\n";
const MARKING_BODY: &[u8] = b"c,X\na,Y\nz,9\n";

#[test]
fn the_body_marks_the_row_when_it_lands() {
    let (_f, s) = fx("ctl-mark", MARKED_DOC);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let old = s.read(bot).unwrap().version;
    let r = s
        .write_base_grid(bot, old, MARKING_BODY, &GridWriteOptions::default())
        .unwrap();
    assert!(matches!(r.outcome, GridWrite::Applied(_)), "{r:?}");
    let w = op(&s, bot, &EditOptions::new().base(old), |t| {
        t.set_cell(&k("r1"), &k("c1"), "BOT")
    });
    assert!(is_rewritten(&w), "{w:?}");
}

/// The write through `fault`, then an unrelated edit, then a keyed write from before them all,
/// which only a mark on `r1` would refuse. `p`'s edit is on another row.
fn row_marks_after(tag: &str, fault: ApplyFault) {
    let (_f, s) = fx(tag, MARKED_DOC);
    let p = join(&s, "p", ParticipantKind::Person);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let old = s.read(bot).unwrap().version;
    let r = with_apply_fault(fault, || {
        s.write_base_grid(bot, old, MARKING_BODY, &GridWriteOptions::default())
    });
    match fault {
        ApplyFault::NoChange => assert!(
            matches!(&r, Ok(x) if matches!(&x.outcome, GridWrite::Applied(a) if a.version == old)),
            "changed nothing: {r:?}"
        ),
        _ => assert_eq!(
            r.expect_err("the write fails").to_string(),
            planted(fault),
            "failed as planted"
        ),
    }
    assert_eq!(s.read(bot).unwrap().version, old, "nothing applied");
    set(&s, p, "r3", "c2", "UNRELATED");
    let w = op(&s, bot, &EditOptions::new().base(old), |t| {
        t.set_cell(&k("r1"), &k("c1"), "BOT")
    });
    assert!(
        w.is_ok(),
        "the unrelated change marked r1 for a write that never landed: {w:?}"
    );
}

#[test]
fn a_refused_write_does_not_mark_rows_on_the_next_change() {
    row_marks_after("rm-refused", ApplyFault::Refused);
}

#[test]
fn a_poisoned_write_does_not_mark_rows_on_the_next_change() {
    row_marks_after("rm-poisoned", ApplyFault::Poisoned);
}

#[test]
fn a_commit_that_changes_nothing_does_not_mark_rows_on_the_next_change() {
    row_marks_after("rm-nochange", ApplyFault::NoChange);
}

// ---------- next_col_remap ----------

fn col_remap_after(tag: &str, fault: ApplyFault) {
    let (_f, s) = fx(tag, "c,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let old = s.read(bot).unwrap().version;
    let r = with_apply_fault(fault, || {
        s.write_base_grid(bot, old, b"d,c,N\n", &GridWriteOptions::default())
    });
    match fault {
        ApplyFault::NoChange => assert!(
            matches!(&r, Ok(x) if matches!(&x.outcome, GridWrite::Applied(a) if a.version == old)),
            "changed nothing: {r:?}"
        ),
        _ => assert_eq!(
            r.expect_err("the write fails").to_string(),
            planted(fault),
            "failed as planted"
        ),
    }
    set(&s, p, "r1", "c2", "UNRELATED");
    let w = op(&s, bot, &EditOptions::new().base(old), |t| {
        t.set_cell(&k("r1"), &k("c1"), "BOT")
    });
    assert!(
        w.is_ok(),
        "the unrelated change marked a remap for a write that never landed: {w:?}"
    );
}

#[test]
fn a_refused_write_does_not_mark_a_remap_on_the_next_change() {
    col_remap_after("cr-refused", ApplyFault::Refused);
}

#[test]
fn a_commit_that_changes_nothing_does_not_mark_a_remap_on_the_next_change() {
    col_remap_after("cr-nochange", ApplyFault::NoChange);
}

// ---------- next_note_rows: being told a write overwrote your value ----------

fn notes_without_change(f: &Fx) -> usize {
    f.host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AuditEvent::NotesWithoutChange { .. }))
        .count()
}

/// How many overwritten notes `who` finds after a restart.
fn notes_after_restart(f: &Fx, who: &str) -> usize {
    f.mgr.close(&f.key, "test").unwrap();
    let s = f.mgr.open(Arc::new(Lite), &f.key, None).unwrap();
    let c = join(&s, who, ParticipantKind::Person);
    s.take_notes(c)
        .unwrap()
        .notes
        .iter()
        .filter(|x| x.kind == NoteKind::Overwritten)
        .count()
}

/// `p1` holds a live value; `bot` writes over it from an older version (`write_live_stale`,
/// through `fault`); `p2` then makes an unrelated edit. Returns the fixture for the restart.
fn stale_write_over_p1(tag: &str, fault: Option<ApplyFault>) -> Fx {
    let (f, s) = fx(tag, "a,b\nc,d\n");
    let p1 = join(&s, "p1", ParticipantKind::Person);
    let p2 = join(&s, "p2", ParticipantKind::Person);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let old = s.read(bot).unwrap().version;
    set(&s, p1, "r2", "c2", "LIVE");
    let v = s.read(bot).unwrap().version;
    let write = || s.write_live_grid(bot, old, b"a,b\nc,FILE\n", &GridWriteOptions::default());
    match fault {
        None => assert!(matches!(write().unwrap().outcome, GridWrite::Applied(_))),
        Some(ApplyFault::NoChange) => {
            let r = with_apply_fault(ApplyFault::NoChange, write);
            assert!(
                matches!(&r, Ok(x) if matches!(&x.outcome, GridWrite::Applied(a) if a.version == v)),
                "changed nothing: {r:?}"
            );
        }
        Some(fault) => {
            let e = with_apply_fault(fault, write).expect_err("the write fails");
            assert_eq!(e.to_string(), planted(fault), "failed as planted");
        }
    }
    if fault.is_some() {
        assert_eq!(s.read(bot).unwrap().version, v, "nothing applied");
        assert!(text(&s, p1).contains("LIVE"), "p1's value still stands");
    }
    set(&s, p2, "r1", "c1", "UNRELATED");
    drop(s);
    f
}

/// When the write lands, the note is there, so the 0 below means something.
#[test]
fn a_stale_write_that_lands_leaves_its_note() {
    let f = stale_write_over_p1("nr-landed", None);
    assert_eq!(notes_without_change(&f), 0, "the change landed");
    assert_eq!(notes_after_restart(&f, "p1"), 1);
}

#[test]
fn a_stale_write_that_never_landed_tells_nobody_their_value_was_overwritten() {
    let f = stale_write_over_p1("nr-refused", Some(ApplyFault::Refused));
    assert_eq!(
        notes_after_restart(&f, "p1"),
        0,
        "p1 is told a write overwrote their value, and the write never landed"
    );
}

#[test]
fn a_poisoned_stale_write_tells_nobody_their_value_was_overwritten() {
    let f = stale_write_over_p1("nr-poisoned", Some(ApplyFault::Poisoned));
    assert_eq!(
        notes_after_restart(&f, "p1"),
        0,
        "p1 is told a write overwrote their value, and the write never landed"
    );
}

/// `finish_overrides` after a commit that changed nothing: it stores nothing, and the state is
/// reported (`AuditEvent::NotesWithoutChange`).
#[test]
fn a_stale_write_that_changed_nothing_stores_no_note_and_is_reported() {
    let f = stale_write_over_p1("nr-nochange", Some(ApplyFault::NoChange));
    assert_eq!(notes_without_change(&f), 1, "the state is reported");
    assert_eq!(
        notes_after_restart(&f, "p1"),
        0,
        "p1 is told a write overwrote their value, and the write never landed"
    );
}

/// `rows drop-deleted`'s own notes (`dropped_row`): a row `p2` wrote, dropped by `p`.
fn drop_over_p2(tag: &str, fault: Option<ApplyFault>) -> Fx {
    let (f, s) = fx(tag, "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    let p2 = join(&s, "p2", ParticipantKind::Person);
    set(&s, p2, "r1", "c1", "OTHER");
    op(&s, p, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
    match fault {
        None => {
            s.drop_deleted(p).unwrap();
        }
        Some(fault) => {
            let e = with_apply_fault(fault, || s.drop_deleted(p)).expect_err("the drop fails");
            assert_eq!(e.to_string(), planted(fault), "failed as planted");
        }
    }
    set(&s, p, "r2", "c1", "UNRELATED");
    drop(s);
    f
}

#[test]
fn a_drop_that_lands_leaves_its_note() {
    let f = drop_over_p2("nd-landed", None);
    assert_eq!(notes_after_restart(&f, "p2"), 1);
}

#[test]
fn a_drop_that_never_landed_tells_nobody_their_value_was_overwritten() {
    let f = drop_over_p2("nd-refused", Some(ApplyFault::Refused));
    assert_eq!(
        notes_after_restart(&f, "p2"),
        0,
        "p2 is told a drop took their value, and the drop never landed"
    );
}

// ---------- a failure after the doc took the change ----------

/// What the doc holds for a cell, read from the doc itself: a hidden cell doesn't render.
fn cell_in_doc(s: &Session, key: &str) -> Option<crate::elem::Value> {
    s.lock().unwrap().doc.map_get(table::CELLS, key).unwrap()
}

/// A write into a row someone else deleted reads who deleted it, after the doc took the write
/// (`hidden_writes`). If that read fails, the write never reaches the store, so the doc can't keep
/// it, and a claim of the writer's mustn't be left marked.
#[test]
fn a_hidden_write_that_fails_after_the_apply_leaves_the_doc_as_the_store_has_it() {
    let (f, s) = fx("hidden", "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    let p2 = join(&s, "p2", ParticipantKind::Person);
    let seq = op(&s, p2, &EditOptions::new(), |t| t.delete_row(&k("r1"))).unwrap();
    f.store.damage_change(s.doc_id, seq);
    let cell = table::cell_key(&k("r1"), &k("c1"));
    let before = cell_in_doc(&s, &cell);
    let edits = vec![Edit::Map {
        root: table::CELLS.into(),
        ops: vec![crate::elem::MapOp::Set {
            key: cell.clone(),
            value: crate::elem::Value::str("HIDDEN"),
        }],
    }];
    let e = s
        .edit_ops(p, edits, &EditOptions::new())
        .expect_err("the write fails");
    assert!(
        matches!(e, SessionError::Store(StoreError::Corrupt(_))),
        "failed at the damaged record: {e:?}"
    );
    assert_eq!(
        cell_in_doc(&s, &cell),
        before,
        "the doc kept a write the caller was told failed, and the store never got"
    );
}

/// A restore reads the row marks after the doc took it (`marked_again`). If that read fails, the
/// restore never reaches the store: the doc can't keep it, and the claim it touched can't stay
/// marked for the next change to end.
#[test]
fn a_restore_that_fails_after_the_apply_resets_and_leaves_the_doc_as_the_store_has_it() {
    let (f, s) = fx("remark", "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    let other = join(&s, "other", ParticipantKind::Person);
    let v1 = s.name_version(p, "before", None).unwrap();
    set(&s, p, "r1", "c1", "LATER");
    let before = text(&s, p);
    let id = s
        .claim_span(
            p,
            ClaimSpan::Rows {
                first: k("r1"),
                last: k("r1"),
            },
            None,
            None,
            false,
        )
        .unwrap()
        .id;
    f.store.lose_row_marks();
    let e = s.restore(p, v1).expect_err("the restore fails");
    assert!(
        matches!(e, SessionError::Store(StoreError::MarksLost(_))),
        "failed reading the marks: {e:?}"
    );
    assert_eq!(
        text(&s, p),
        before,
        "the doc kept a restore the caller was told failed, and the store never got"
    );
    set(&s, other, "r2", "c1", "UNRELATED");
    let ended: Vec<_> = f
        .host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AuditEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
            _ => None,
        })
        .collect();
    assert_eq!(
        ended,
        [awareness::ClaimEnd::Reset],
        "the claim ended by the reload, not by someone else's change"
    );
}

fn reloads(f: &Fx) -> usize {
    f.host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, AuditEvent::Recovered { .. }))
        .count()
}

fn claim_ends(f: &Fx, id: awareness::ClaimId) -> Vec<awareness::ClaimEnd> {
    f.host
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            AuditEvent::ClaimEnded { claim, how, .. } if *claim == id => Some(*how),
            _ => None,
        })
        .collect()
}

fn claim_r1(s: &Session, c: ConnId) -> awareness::ClaimId {
    let span = ClaimSpan::Rows {
        first: k("r1"),
        last: k("r1"),
    };
    s.claim_span(c, span, None, None, false).unwrap().id
}

/// The change's bookkeeping fails after the doc took it: one reload, and the claim it touched ends
/// `Reset`.
#[test]
fn bookkeeping_that_fails_after_the_apply_reloads_once() {
    let (f, s) = fx("book-one", "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    let id = claim_r1(&s, p);
    let before = text(&s, p);
    let e = with_apply_fault(ApplyFault::BookkeepingFailsAfter(0), || {
        op(&s, p, &EditOptions::new(), |t| {
            t.set_cell(&k("r1"), &k("c1"), "X")
        })
    })
    .expect_err("the change fails");
    assert!(matches!(e, SessionError::Store(StoreError::Db(_))), "{e:?}");
    assert_eq!(text(&s, p), before, "the doc is as the store has it");
    assert_eq!(reloads(&f), 1);
    assert_eq!(claim_ends(&f, id), [awareness::ClaimEnd::Reset]);
}

/// The same at a later step of a commit, after an earlier step applied. It reloads **twice**, and
/// each `Recovered` event is true: `apply_structured` reloads as its change can't reach the store,
/// and `commit` reloads as an earlier step's can't. The claim still ends once.
#[test]
fn bookkeeping_that_fails_at_a_later_step_reloads_twice() {
    let (f, s) = fx("book-later", "a,b\nc,d\n");
    let p = join(&s, "p", ParticipantKind::Person);
    set(&s, p, "r1", "c1", "A");
    set(&s, p, "r1", "c2", "B");
    let id = claim_r1(&s, p);
    let before = text(&s, p);
    let e = with_apply_fault(ApplyFault::BookkeepingFailsAfter(1), || s.undo(p, 2))
        .expect_err("the second step fails");
    assert!(matches!(e, SessionError::Store(StoreError::Db(_))), "{e:?}");
    assert_eq!(text(&s, p), before, "the doc is as the store has it");
    assert_eq!(reloads(&f), 2, "one from each of the two levels");
    assert_eq!(claim_ends(&f, id), [awareness::ClaimEnd::Reset]);
}
