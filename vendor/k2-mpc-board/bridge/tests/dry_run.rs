//! Test 4: the run and the dry run. A dry run opens, reads the guard, plans, applies and saves
//! into nowhere: the file's bytes and guard are as they were, the answer says it was a dry run,
//! its echo's version is the guard read, and `would_write` is what a real save then writes. Every
//! refusal a real run gives, the dry run gives too, with the same row and exit: on open, on the
//! guard read (before anything is planned, and the file's, exit 4), and in apply.

mod support;

use k2_mpc_board_bridge::board::json::{Fields, Value as BValue};
use k2_mpc_board_bridge::board::verbs::{Ctx, Me, Verb};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::dry_run::{dry_run, run};
use k2_mpc_board_bridge::kit::errors::Value as KValue;
use k2_mpc_board_bridge::parse::{Call, ReadVerb};
use k2_mpc_board_bridge::{Answer, Refused, Why};
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use support::*;

const PATH: &str = "plans/q4.k5board";

fn target() -> FileTarget {
    FileTarget::new(PATH).expect("check: a path")
}

fn bytes_of(b: &Board) -> Vec<u8> {
    let mut out = Vec::new();
    let meta = WriteMeta::new(Lineage::from_bytes([7; 16]));
    write(&mut out, &meta, b, &mut MemParts::new(), &Limits::V1).expect("check: a file");
    out
}

fn guard_of(bytes: &[u8]) -> String {
    package_guard_of(Cursor::new(bytes), &Limits::V1)
        .expect("check: the file has a guard")
        .to_string()
}

/// A host's context: one actor, a clock, a counter for random bytes.
fn with_cx<T>(f: impl FnOnce(&mut Ctx<'_>) -> T) -> T {
    let mut n = 1u64;
    let mut random = move || {
        n += 1;
        n.to_le_bytes()
    };
    let mut cx = Ctx {
        group_id: "g".to_string(),
        by: Some(Me {
            token: id(ME),
            new: None,
        }),
        at: Some("2026-11-03T12:00:00Z".to_string()),
        random: &mut random,
    };
    f(&mut cx)
}

fn set_text(b: &Board, i: &str, t: &str) -> Call {
    Call::from_verb(Verb::Update {
        id: id(i),
        rev: rev(b, i),
        set: fields(&[("text", text(t))]),
        unset: vec![],
    })
}

fn dry(call: &Call, file: &[u8], on_disk: &[u8]) -> Result<Answer, Box<Refused>> {
    with_cx(|cx| {
        dry_run(
            call,
            target(),
            Cursor::new(file),
            Cursor::new(on_disk),
            cx,
            &Limits::V1,
        )
    })
}

fn real(
    call: &Call,
    file: &[u8],
    on_disk: &[u8],
    out: &mut Vec<u8>,
) -> Result<Answer, Box<Refused>> {
    with_cx(|cx| {
        run(
            call,
            target(),
            Cursor::new(file),
            Cursor::new(on_disk),
            out,
            cx,
            &Limits::V1,
        )
    })
}

fn str_at<'v>(v: &'v KValue, keys: &[&str]) -> Option<&'v str> {
    let mut at = v;
    for k in keys {
        at = at.get(k)?;
    }
    at.as_str()
}

fn row_of(r: &Refused) -> (&'static str, Option<u8>) {
    match &r.why {
        Why::Report(report) => (report.id(), r.why.exit()),
        Why::HostBug(b) => panic!("a host bug, not a row: {b}"),
    }
}

#[test]
fn a_dry_run_writes_nothing_and_says_so() {
    let b = board();
    let bytes = bytes_of(&b);
    let before = bytes.clone();
    let g = guard_of(&bytes);
    let a = dry(&set_text(&b, "s-1", "changed"), &bytes, &bytes).expect("check: it answers");
    assert!(a.dry_run, "the answer says it was a dry run");
    assert_eq!(bytes, before, "the file's bytes are as they were");
    assert_eq!(guard_of(&bytes), g, "and so is its guard");
    assert_eq!(a.on.kind, "k5board", "on.kind");
    assert_eq!(a.on.path, PATH, "on.path");
    assert_eq!(a.on.mode, Mode::File, "on.mode");
    assert_eq!(
        a.on.version.as_deref(),
        Some(g.as_str()),
        "the echo's version is the guard read: the file didn't change"
    );
    let would = str_at(&a.value, &["would_write", "guard"]);
    assert!(
        would.is_some_and(|w| w.starts_with("sha256-") && w != g),
        "would_write names the guard a save would write: {would:?}"
    );
    assert_eq!(a.value.get("saved"), None, "nothing was saved");
    assert_eq!(
        a.value.get("report").and_then(|r| r.get("changed")),
        Some(&KValue::list(["s-1"])),
        "the change was applied, in memory"
    );
    assert_eq!(
        a.value.get("verb"),
        Some(&KValue::from("update")),
        "the verb run"
    );
}

/// The same plan, run for real, writes the file the dry run said it would.
#[test]
fn a_real_run_writes_what_the_dry_run_said() {
    let b = board();
    let bytes = bytes_of(&b);
    let call = set_text(&b, "s-1", "changed");
    let d = dry(&call, &bytes, &bytes).expect("check: the dry run answers");
    let mut out = Vec::new();
    let a = real(&call, &bytes, &bytes, &mut out).expect("check: the run answers");
    assert!(!a.dry_run, "a real run says so");
    assert!(!out.is_empty(), "a real run writes the new file");
    let written = guard_of(&out);
    assert_ne!(written, guard_of(&bytes), "the new file is another file");
    assert_eq!(
        str_at(&d.value, &["would_write", "guard"]),
        Some(written.as_str()),
        "the dry run named the guard the run wrote"
    );
    assert_eq!(
        str_at(&a.value, &["saved", "guard"]),
        Some(written.as_str()),
        "the run names it too"
    );
    assert_eq!(
        a.on.version.as_deref(),
        Some(written.as_str()),
        "a real run's echo is the guard written"
    );
    assert_eq!(
        d.value.get("would_write").and_then(|w| w.get("revs")),
        a.value.get("saved").and_then(|s| s.get("revs")),
        "the same new revisions"
    );
    let opened = BoardFile::open(target(), Cursor::new(&out[..]), &Limits::V1)
        .unwrap_or_else(|e| panic!("the new file opens: {:?}", e.value));
    assert_eq!(
        obj(opened.board(), "s-1").fields.get("text"),
        Some(&text("changed")),
        "the new file holds the change"
    );
}

/// The guard is read before anything is planned: a file that moved on is MPC-E832 even for a
/// verb that would itself refuse (voting is off here, MPC-E836), in a dry run as in a run.
#[test]
fn the_guard_is_read_before_the_plan() {
    let b = board();
    let bytes = bytes_of(&b);
    let mut moved = b.clone();
    moved.settings.insert("title".into(), text("moved on"));
    let on_disk = bytes_of(&moved);
    let vote = Call::from_verb(Verb::Vote {
        id: id("s-1"),
        delta: 1,
    });
    let d = dry(&vote, &bytes, &on_disk).expect_err("the dry run refuses");
    let mut out = Vec::new();
    let r = real(&vote, &bytes, &on_disk, &mut out).expect_err("the run refuses");
    assert_eq!(row_of(&d), ("MPC-E832", Some(1)), "the dry run: busy");
    assert_eq!(row_of(&r), ("MPC-E832", Some(1)), "the run: busy");
    assert!(d.on.version.is_some(), "the file was read");
    assert!(out.is_empty(), "nothing was written");
    // with the file as read, the verb's own refusal shows
    let own = dry(&vote, &bytes, &bytes).expect_err("voting is off");
    assert_eq!(row_of(&own), ("MPC-E836", Some(5)), "voting_off");
}

/// A file on disk whose guard can't be read is the file's refusal (exit 4), not the write's.
#[test]
fn a_guard_that_cant_be_read_is_the_files_refusal() {
    let b = board();
    let bytes = bytes_of(&b);
    let call = set_text(&b, "s-1", "changed");
    let d = dry(&call, &bytes, b"not a package").expect_err("the dry run refuses");
    let mut out = Vec::new();
    let r = real(&call, &bytes, b"not a package", &mut out).expect_err("the run refuses");
    for (who, x) in [("dry run", &d), ("run", &r)] {
        let (row, exit) = row_of(x);
        assert_eq!(exit, Some(4), "{who}: exit 4, the file's");
        assert_eq!(row, "MPC-E842", "{who}: board_zip");
    }
    assert!(out.is_empty(), "nothing was written");
}

/// The board with a picture: one zip entry more than the board without.
fn with_picture(b: &Board) -> Vec<u8> {
    let mut parts = MemParts::new();
    let hash = parts.add_blob("image/png", b"picture".to_vec());
    let mut blob = Fields::new();
    blob.insert("blob".into(), text(&hash.to_string()));
    let mut fields = Fields::new();
    fields.insert("src".into(), BValue::Obj(blob));
    let mut b = b.clone();
    b.objects.push(Object {
        id: id("i-1"),
        kind: ObjectType::Image,
        fields,
    });
    let mut out = Vec::new();
    let meta = WriteMeta::new(Lineage::from_bytes([7; 16]));
    write(&mut out, &meta, &b, &mut parts, &Limits::V1).expect("check: a file with a picture");
    out
}

/// The fewest zip entries `bytes` opens with.
fn fewest_entries(bytes: &[u8]) -> u64 {
    (1..16)
        .find(|n| {
            let l = Limits {
                entries: *n,
                ..Limits::V1
            };
            BoardFile::open(target(), Cursor::new(bytes), &l).is_ok()
        })
        .expect("check: the file opens with some count of entries")
}

/// A limit the file on disk passes when the guard is read is the file's refusal (exit 4,
/// MPC-E846), never the write's (MPC-E848, exit 2): the file as read opens within the limits, the
/// one on disk now has one entry too many.
#[test]
fn a_guard_over_a_limit_is_the_files_refusal() {
    let b = board();
    let bytes = bytes_of(&b);
    let on_disk = with_picture(&b);
    let n = fewest_entries(&bytes);
    assert!(
        fewest_entries(&on_disk) > n,
        "the file on disk has more entries"
    );
    let l = Limits {
        entries: n,
        ..Limits::V1
    };
    let call = set_text(&b, "s-1", "changed");
    let d = with_cx(|cx| {
        dry_run(
            &call,
            target(),
            Cursor::new(&bytes[..]),
            Cursor::new(&on_disk[..]),
            cx,
            &l,
        )
    })
    .expect_err("the dry run refuses");
    let mut out = Vec::new();
    let r = with_cx(|cx| {
        run(
            &call,
            target(),
            Cursor::new(&bytes[..]),
            Cursor::new(&on_disk[..]),
            &mut out,
            cx,
            &l,
        )
    })
    .expect_err("the run refuses");
    assert_eq!(
        row_of(&d),
        ("MPC-E846", Some(4)),
        "the dry run: board_too_large"
    );
    assert_eq!(
        row_of(&r),
        ("MPC-E846", Some(4)),
        "the run: board_too_large"
    );
    assert!(d.on.version.is_some(), "the file as read was read");
    assert!(out.is_empty(), "nothing was written");
}

/// A file over a limit on open is the file's refusal: MPC-E846, exit 4, with no version.
#[test]
fn a_file_over_a_limit_on_open_is_the_files_refusal() {
    let b = board();
    let bytes = bytes_of(&b);
    let l = Limits {
        entries: fewest_entries(&bytes) - 1,
        ..Limits::V1
    };
    let call = set_text(&b, "s-1", "changed");
    let d = with_cx(|cx| {
        dry_run(
            &call,
            target(),
            Cursor::new(&bytes[..]),
            Cursor::new(&bytes[..]),
            cx,
            &l,
        )
    })
    .expect_err("the dry run refuses");
    assert_eq!(row_of(&d), ("MPC-E846", Some(4)), "board_too_large");
    assert_eq!(d.on.version, None, "no guard was read");
}

/// A stale rev refuses in apply, the same in both.
#[test]
fn a_dry_run_refuses_in_apply_as_the_run_does() {
    let b = board();
    let bytes = bytes_of(&b);
    let call = Call::from_verb(Verb::Update {
        id: id("s-1"),
        rev: stale(),
        set: fields(&[("text", text("changed"))]),
        unset: vec![],
    });
    let d = dry(&call, &bytes, &bytes).expect_err("the dry run refuses");
    let mut out = Vec::new();
    let r = real(&call, &bytes, &bytes, &mut out).expect_err("the run refuses");
    assert_eq!(row_of(&d), ("MPC-E830", Some(1)), "the dry run: conflict");
    assert_eq!(row_of(&r), row_of(&d), "the run gives the same row");
    assert_eq!(
        d.on.version.as_deref(),
        Some(guard_of(&bytes).as_str()),
        "the guard read"
    );
    assert!(out.is_empty(), "nothing was written");
}

/// A file that isn't a package refuses on open, with no version: no guard was read.
#[test]
fn a_file_that_doesnt_open_refuses_with_no_version() {
    let b = board();
    let call = set_text(&b, "s-1", "changed");
    let d = dry(&call, b"not a package", b"not a package").expect_err("the dry run refuses");
    assert_eq!(row_of(&d), ("MPC-E842", Some(4)), "board_zip");
    assert_eq!(d.on.version, None, "no guard was read");
    assert_eq!(d.on.path, PATH, "the path, as the host gave it");
}

/// A change that changes nothing would write nothing.
#[test]
fn a_dry_run_that_changes_nothing_would_write_nothing() {
    let b = board();
    let bytes = bytes_of(&b);
    let a = dry(&set_text(&b, "s-1", "one"), &bytes, &bytes).expect("check: it answers");
    assert!(a.dry_run, "a dry run");
    assert_eq!(
        a.value.get("would_write"),
        Some(&KValue::Null),
        "nothing to write"
    );
}

/// A read verb's dry run is its run, said to be dry.
#[test]
fn a_dry_run_of_a_read_says_so() {
    let b = board();
    let bytes = bytes_of(&b);
    let call = Call::from_read(ReadVerb::Read { id: id("s-1") });
    let d = dry(&call, &bytes, &bytes).expect("check: the dry run answers");
    let mut out = Vec::new();
    let a = real(&call, &bytes, &bytes, &mut out).expect("check: the run answers");
    assert!(d.dry_run && !a.dry_run, "each says what it was");
    assert_eq!(d.value, a.value, "the same answer");
    assert_eq!(
        d.on.version.as_deref(),
        Some(guard_of(&bytes).as_str()),
        "the guard read"
    );
    assert!(out.is_empty(), "a read writes nothing");
}

/// A file on disk that counts its seeks, and refuses the one numbered `fail_at` (from 1).
struct Seeks {
    inner: Cursor<Vec<u8>>,
    seeks: usize,
    fail_at: Option<usize>,
}

impl Seeks {
    fn new(bytes: &[u8], fail_at: Option<usize>) -> Seeks {
        Seeks {
            inner: Cursor::new(bytes.to_vec()),
            seeks: 0,
            fail_at,
        }
    }
}

impl Read for Seeks {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for Seeks {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.seeks += 1;
        if Some(self.seeks) == self.fail_at {
            return Err(io::Error::other("seek refused"));
        }
        self.inner.seek(to)
    }
}

/// Where the new file goes, refusing every write.
struct Full;

impl Write for Full {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// MPC-E857 (`board_io`, exit 1), with the I/O error's own text as its reason.
fn assert_io(r: &Refused, says: &str, who: &str) {
    assert_eq!(row_of(r), ("MPC-E857", Some(1)), "{who}: board_io");
    let Why::Report(report) = &r.why else {
        panic!("{who}: a report");
    };
    let reason = report.detail("reason").and_then(KValue::as_str);
    assert!(
        reason.is_some_and(|t| t.contains(says)),
        "{who}: the reason is the I/O error's: {reason:?}"
    );
}

fn run_on_disk<D: Read + Seek, W: Write>(
    call: &Call,
    file: &[u8],
    on_disk: D,
    out: &mut W,
) -> Result<Answer, Box<Refused>> {
    with_cx(|cx| {
        run(
            call,
            target(),
            Cursor::new(file),
            on_disk,
            out,
            cx,
            &Limits::V1,
        )
    })
}

/// The file on disk can't be read where the guard is: MPC-E857, in the run and the dry run.
#[test]
fn a_file_on_disk_that_cant_be_read_is_board_io() {
    let b = board();
    let bytes = bytes_of(&b);
    let call = set_text(&b, "s-1", "changed");
    let mut out = Vec::new();
    let r = run_on_disk(&call, &bytes, Seeks::new(&bytes, Some(1)), &mut out)
        .expect_err("the run refuses");
    assert_io(&r, "seek refused", "the run");
    assert!(out.is_empty(), "nothing was written");
    let d = with_cx(|cx| {
        dry_run(
            &call,
            target(),
            Cursor::new(&bytes[..]),
            Seeks::new(&bytes, Some(1)),
            cx,
            &Limits::V1,
        )
    })
    .expect_err("the dry run refuses");
    assert_io(&d, "seek refused", "the dry run");
}

/// The file on disk reads for the guard, then can't be rewound for the save: MPC-E857.
#[test]
fn a_file_on_disk_that_cant_be_read_for_the_save_is_board_io() {
    let b = board();
    let bytes = bytes_of(&b);
    // the seeks the guard read takes; the run's first seek is its rewind, then the guard read's
    let mut probe = Seeks::new(&bytes, None);
    package_guard_of(&mut probe, &Limits::V1).expect("check: the guard reads");
    let at = probe.seeks + 2;
    let call = set_text(&b, "s-1", "changed");
    let mut out = Vec::new();
    let r = run_on_disk(&call, &bytes, Seeks::new(&bytes, Some(at)), &mut out)
        .expect_err("the run refuses");
    assert_io(&r, "seek refused", "the run");
    assert!(out.is_empty(), "nothing was written");
}

/// Writing the new file fails: MPC-E857 with the write's text. A dry run writes nowhere, so it
/// answers.
#[test]
fn a_write_that_fails_is_board_io() {
    let b = board();
    let bytes = bytes_of(&b);
    let call = set_text(&b, "s-1", "changed");
    let r = run_on_disk(&call, &bytes, Cursor::new(&bytes[..]), &mut Full)
        .expect_err("the run refuses");
    assert_io(&r, "disk full", "the run");
    let d = dry(&call, &bytes, &bytes).expect("check: a dry run writes nowhere");
    assert!(d.dry_run, "a dry run");
}

/// A raw `apply` whose group the board refuses as it applies it (here a locked object) reports the
/// row the ops' phase gives: MPC-E835, exit 5, in the run and the dry run.
#[test]
fn a_raw_apply_refused_in_applying_takes_the_ops_row() {
    let b = board();
    let bytes = bytes_of(&b);
    let ops = format!(
        "{{\"ops\":[{{\"op\":\"set\",\"id\":\"l-1\",\"rev\":\"{}\",\"f\":{{\"text\":\"x\"}}}}]}}",
        rev(&b, "l-1")
    );
    let call = Call::apply(ops, &Limits::V1);
    let d = dry(&call, &bytes, &bytes).expect_err("the dry run refuses");
    let mut out = Vec::new();
    let r = real(&call, &bytes, &bytes, &mut out).expect_err("the run refuses");
    assert_eq!(row_of(&d), ("MPC-E835", Some(5)), "the dry run: locked");
    assert_eq!(row_of(&r), ("MPC-E835", Some(5)), "the run: locked");
    assert!(out.is_empty(), "nothing was written");
}
