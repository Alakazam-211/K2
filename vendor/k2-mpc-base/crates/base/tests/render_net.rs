//! The net under a file that can't be rendered (docs/SESSIONS.md, "When the file can't be
//! rendered"). A doc whose pack can't render it as its file at its version stays readable:
//!
//! - a whole read gives the doc's content with the file's bytes as last synced (the carrier) and
//!   the failure, typed (`ReadResult::render_failed`);
//! - a pack's structured read verb gives its result, with a warning naming the failure;
//! - the tick reports a write-back that can't render the doc once per version, not every tick;
//! - guards: a version whose whole read was given the synced bytes is no base for a whole-file
//!   write (`MPC-E824` `base_unrendered`), and such a read makes no pairing and leaves the doc
//!   dirty, so the next rendering that works writes the doc, not the synced bytes.
//!
//! The test pack is `net-units` (`tests/net_pack/mod.rs`), whose encode fails while `failing` is
//! set; the guard's table and text rows wrap `grid-lite` and the kit's text pack the same way.
//!
//! Plant (`KeyedMutant::ReadReplaysRenderFailure`, the structured read returns the failure as
//! its error, as before): caught by the structured read's row alone, by name.

#[path = "grid_pack/mod.rs"]
mod grid_pack;
#[path = "net_pack/mod.rs"]
mod net_pack;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{self, KeyedMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    Declined, DiskState, GridWriteOptions, Pack, PackContent, PackError, ReadResult, Session,
    SessionError, TextFilePack, WriteOutcome,
};
use k2_mpc_base::store::DocKey;
use k2_mpc_base::tools::{session_exit, PackVerb};
use net_pack::{argv, fx, need, person_set, Fx, F0, FIELDS, UNITS};

/// `unit`'s `field` as a read gives it.
fn field(r: &ReadResult, unit: &str, f: &str) -> Option<String> {
    r.content.iter().find_map(|(root, n)| match n {
        Neutral::Map(m) if root == FIELDS => m
            .get(&format!("{unit}:{f}"))
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        _ => None,
    })
}

/// A doc whose u1 a person changed to `A2` (in the store, not yet in the file), with the
/// pack's rendering failing from then on.
fn stuck(tag: &str) -> (Fx, Arc<Session>, k2_mpc_base::session::ConnId, u64) {
    let f = fx(tag, true);
    let s = f.open();
    let p = f.person(&s);
    let v = person_set(&s, p, "u1", "title", "A2").expect("the edit is taken");
    f.pack.fail(true);
    (f, s, p, v)
}

// ---------- a whole read ----------

/// The whole read: the doc's content at its version, the file's bytes as last synced, and the
/// pack's failure, typed.
#[test]
fn a_whole_read_gives_the_synced_bytes_and_the_failure() {
    let (f, s, p, v) = stuck("whole");
    let read = s.read(p).expect("the read is Ok");
    assert_eq!(read.version, v);
    assert_eq!(
        field(&read, "u1", "title").as_deref(),
        Some("A2"),
        "the doc's"
    );
    assert_eq!(read.bytes, F0.as_bytes(), "the file as last synced");
    let e = read.render_failed.as_ref().expect("the failure is given");
    assert!(matches!(e, SessionError::Pack(_)), "{e:?}");
    assert_eq!(e.report().id(), "MPC-E517");
    assert_eq!(f.bytes(), F0, "the file is as it was");
}

// ---------- a structured read ----------

/// A pack's structured read verb, once the write-back found the file can't be rendered: its
/// result, with one warning naming the failure.
fn row_a_structured_read_gives_its_result_with_a_warning() -> Result<(), String> {
    const ROW: &str = "row_a_structured_read_gives_its_result_with_a_warning";
    let (f, s, p, _) = stuck("verb");
    let flushed = s.flush(p);
    need(flushed.is_err(), ROW, || {
        format!("control: the write-back can't render: {flushed:?}")
    })?;
    let bot = f.agent(&s);
    let before = f.pack.encodes();
    let out = s
        .run_tool(bot, UNITS, &argv("unit list"), b"")
        .map_err(|e| format!("{ROW}: the structured read failed: {e:?}"))?;
    need(out.stdout == b"u1\nu2\nu3\n" && out.exit == 0, ROW, || {
        format!("the result: {out:?}")
    })?;
    let w = out.warnings.lines();
    need(
        w.len() == 1 && w[0].starts_with("note: ") && w[0].contains("MPC-E517"),
        ROW,
        || format!("the warning: {w:?}"),
    )?;
    need(f.pack.encodes() == before, ROW, || {
        "the read rendered the file".into()
    })
}

#[test]
fn a_structured_read_gives_its_result_with_a_warning() {
    row_a_structured_read_gives_its_result_with_a_warning().unwrap();
}

/// The planted fault "read replays render failure": the structured read's row goes red, by
/// name.
#[test]
fn the_planted_read_replays_render_failure_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::ReadReplaysRenderFailure, || {
        let name = "row_a_structured_read_gives_its_result_with_a_warning";
        let e = row_a_structured_read_gives_its_result_with_a_warning()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("the structured read failed"), "{e}");
    });
}

/// A structured read with nothing failing gives no warning.
#[test]
fn a_structured_read_with_the_file_rendering_gives_no_warning() {
    let f = fx("verb-ok", true);
    let s = f.open();
    let bot = f.agent(&s);
    let out = s.run_tool(bot, UNITS, &argv("unit list"), b"").unwrap();
    assert!(out.warnings.is_empty(), "{:?}", out.warnings);
}

// ---------- the tick ----------

/// The tick's errors for this doc.
fn tick_errors(f: &Fx) -> usize {
    f.advance(120_000);
    let rep = f.mgr.tick().expect("the tick runs");
    rep.errors
        .iter()
        .filter(|(k, _)| *k == net_pack::key())
        .count()
}

/// Three ticks while the write-back can't render the doc report it once; a change (a new
/// version) reports it once more. Nothing is lost: once the pack renders, a flush writes the
/// doc.
#[test]
fn the_tick_reports_a_render_failure_once_per_version() {
    let (f, s, p, _) = stuck("tick");
    let first: Vec<usize> = (0..3).map(|_| tick_errors(&f)).collect();
    assert_eq!(first, [1, 0, 0], "one report for the version");
    person_set(&s, p, "u3", "title", "C2").expect("the edit is taken");
    let second: Vec<usize> = (0..3).map(|_| tick_errors(&f)).collect();
    assert_eq!(second, [1, 0, 0], "one more for the new version");
    assert_eq!(f.bytes(), F0, "the file as it was");
    f.pack.fail(false);
    assert!(s.flush(p).unwrap().written);
    assert_eq!(f.bytes(), "u1\ttitle=A2\nu2\ttitle=B\nu3\ttitle=C2\n");
}

// ---------- guard: no base ----------

/// A pack whose `encode` fails while `fail` is set.
struct Failing<P>(P, Arc<AtomicBool>);
impl<P: Pack> Pack for Failing<P> {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        match self.1.load(Ordering::SeqCst) {
            true => Err(PackError("a planted render failure".into())),
            false => self.0.encode(c, codec),
        }
    }
    fn verbs(&self) -> &'static [PackVerb] {
        self.0.verbs()
    }
}

/// `body` as a file of the fixture's folder, opened by `pack` wrapped to fail.
fn open_failing<P: Pack + 'static>(
    f: &Fx,
    name: &str,
    body: &str,
    pack: P,
) -> (Arc<Session>, Arc<AtomicBool>) {
    let path = f.ws.join(name);
    std::fs::write(&path, body).unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let key = DocKey {
        space: "ws".into(),
        path: name.into(),
    };
    let s = f
        .mgr
        .open_file(
            Arc::new(Failing(pack, fail.clone())),
            &key,
            &path,
            Path::new(&f.ws),
        )
        .unwrap();
    (s, fail)
}

fn is_base_unrendered(what: &str, e: &SessionError, base: u64) {
    assert_eq!(
        *e,
        SessionError::BaseUnrendered { base },
        "{what}: refused typed"
    );
    assert_eq!(e.report().id(), "MPC-E824", "{what}");
    assert_eq!(session_exit(e), 2, "{what}");
}

/// A table's whole read given while its file can't be rendered (no carrier: no bytes): neither
/// `write --base` nor `--seen` takes that version, and nothing applies.
#[test]
fn a_flagged_reads_version_is_no_base_for_a_tables_write() {
    let f = fx("grid", false);
    let (s, fail) = open_failing(
        &f,
        "t.csv",
        "id,city\nk1,Oslo\nk2,Rome\n",
        grid_pack::GridLite,
    );
    let p = f.person(&s);
    fail.store(true, Ordering::SeqCst);
    let read = s.read(p).unwrap();
    assert!(read.render_failed.is_some(), "{read:?}");
    assert!(read.bytes.is_empty(), "no carrier: no bytes");
    let v = read.version;
    let body = b"id,city\nk1,Lima\n";
    let e = s
        .write_base_grid(p, v, body, &GridWriteOptions::default())
        .expect_err("write --base refused");
    is_base_unrendered("write --base", &e, v);
    let e = s
        .write_live_grid(p, v, body, &GridWriteOptions::default())
        .expect_err("--seen refused");
    is_base_unrendered("write --seen", &e, v);
    assert_eq!(s.status().unwrap().version, v, "nothing applied");
}

/// A text doc: `write --base` from a flagged read's version is refused; from a version whose
/// read was the doc's file, it applies as before.
#[test]
fn a_flagged_reads_version_is_no_base_for_a_text_write() {
    let f = fx("text", false);
    let (s, fail) = open_failing(
        &f,
        "n.md",
        "one\ntwo\n",
        TextFilePack::new("net-text", 1 << 20),
    );
    let p = f.person(&s);
    fail.store(true, Ordering::SeqCst);
    let read = s.read(p).unwrap();
    assert!(read.render_failed.is_some(), "{read:?}");
    let v = read.version;
    let e = s
        .write_base(p, TextFilePack::ROOT, v, "one\nTWO\n")
        .expect_err("refused");
    is_base_unrendered("write --base", &e, v);
    assert_eq!(s.status().unwrap().version, v, "nothing applied");
    // control: the file renders again, a new version, read whole: a base as before
    fail.store(false, Ordering::SeqCst);
    s.replace_text(p, TextFilePack::ROOT, "one\ntwo\nthree\n")
        .unwrap();
    let read = s.read(p).unwrap();
    assert!(read.render_failed.is_none());
    let out = s
        .write_base(p, TextFilePack::ROOT, read.version, "one\nTWO\nthree\n")
        .unwrap();
    assert!(matches!(out, WriteOutcome::Applied(_)), "{out:?}");
}

// ---------- guard: no pairing, the doc stays dirty ----------

/// Whole reads given the synced bytes make no pairing and leave the doc dirty; once the pack
/// renders again, the write-back writes the doc, not the synced bytes.
#[test]
fn a_flagged_read_makes_no_pairing_and_leaves_the_doc_dirty() {
    let f = fx("pairing", true);
    f.pack.pairs.store(true, Ordering::SeqCst);
    let s = f.open();
    let p = f.person(&s);
    person_set(&s, p, "u1", "title", "A2").expect("the edit is taken");
    f.pack.fail(true);
    differential::take_pairings_made();
    for _ in 0..3 {
        let read = s.read(p).unwrap();
        assert!(read.render_failed.is_some());
        assert_eq!(read.bytes, F0.as_bytes());
    }
    assert_eq!(differential::take_pairings_made(), 0, "no pairing made");
    assert!(
        matches!(s.status().unwrap().disk, Some(DiskState::Dirty { .. })),
        "the doc is still dirty: {:?}",
        s.status().unwrap().disk
    );
    f.pack.fail(false);
    assert!(s.flush(p).unwrap().written);
    assert_eq!(f.bytes(), "u1\ttitle=A2\nu2\ttitle=B\nu3\ttitle=C\n");
    let read = s.read(p).unwrap();
    assert!(read.render_failed.is_none());
    assert_eq!(read.bytes, f.bytes().as_bytes());
}
