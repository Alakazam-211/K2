//! A pack's check of every live change (`Pack::check_edit`, docs/SESSIONS.md "A pack's check of
//! every live change"): a pack whose write-back can't write a change refuses it up front, so the
//! doc never holds what its file can't. The rows, on the test pack `net-units`
//! (`tests/net_pack/mod.rs`), which refuses any change to the unit it names:
//!
//! - a person's direct edit is refused at the call, typed (`MPC-E823` `pack_refused`, the pack's
//!   code in its details); the version and the file don't move; a whole read and a structured
//!   read are both Ok after it;
//! - the same for an agent's pack verb and an agent's computed edit (`edit_ops_with`);
//! - an undo that would put such a change back is refused the same way;
//! - a change the pack accepts commits and writes back as before;
//! - a text doc's pack that opts in is asked of a text change too;
//! - a purge's removal is never asked (erasure always goes through).
//!
//! Plant (`KeyedMutant::EditCheckDropped`, the commit doesn't ask the pack): caught by the
//! person's row alone, by name.

#[path = "net_pack/mod.rs"]
mod net_pack;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{self, KeyedMutant};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    Declined, EditOptions, EditWithError, Pack, PackContent, PackError, PackRefusal,
    ParticipantKind, PurgeRequest, PurgeScope, ReadResult, Session, SessionError, TextFilePack,
    View,
};
use k2_mpc_base::tools::{session_exit, ToolRunError};
use net_pack::{argv, fx, need, person_set, set, Fx, F0, FIELDS, REFUSED, UNITS};

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

/// `e` is the pack's refusal: `PackRefused` with the pack's code, `MPC-E823`, exit 2.
fn is_refusal(row: &str, what: &str, e: &SessionError) -> Result<(), String> {
    let SessionError::PackRefused(PackRefusal { code, .. }) = e else {
        return Err(format!("{row}: {what}: not the pack's refusal: {e:?}"));
    };
    need(*code == REFUSED, row, || format!("{what}: the code {code}"))?;
    let rep = e.report();
    need(rep.id() == "MPC-E823", row, || {
        format!("{what}: the id {}", rep.id())
    })?;
    need(session_exit(e) == 2, row, || {
        format!("{what}: the exit {}", session_exit(e))
    })
}

/// Nothing moved: the version is `v`, the file `F0` (after a flush), and a whole read and a
/// structured read are both Ok, the whole read with the file's bytes.
fn nothing_moved(row: &str, f: &Fx, s: &Session, v: u64) -> Result<(), String> {
    let now = s
        .status()
        .map_err(|e| format!("{row}: status: {e}"))?
        .version;
    need(now == v, row, || format!("the version moved: {v} -> {now}"))?;
    let p = f.person(s);
    let flushed = s.flush(p);
    need(flushed.is_ok(), row, || format!("the flush: {flushed:?}"))?;
    need(f.bytes() == F0, row, || {
        format!("the file: {:?}", f.bytes())
    })?;
    let read = s
        .read(p)
        .map_err(|e| format!("{row}: the whole read failed: {e}"))?;
    need(read.bytes == F0.as_bytes(), row, || {
        format!("the whole read: {read:?}")
    })?;
    let bot = f.agent(s);
    let list = s
        .run_tool(bot, UNITS, &argv("unit list"), b"")
        .map_err(|e| format!("{row}: the structured read failed: {e:?}"))?;
    need(list.stdout == b"u1\nu2\nu3\n", row, || {
        format!("the structured read: {:?}", list.stdout)
    })
}

// ---------- a person's edit ----------

/// A person's direct edit of the unit the pack refuses: refused at the call, typed; nothing
/// moved.
fn row_a_persons_refused_edit_is_refused_at_the_call() -> Result<(), String> {
    const ROW: &str = "row_a_persons_refused_edit_is_refused_at_the_call";
    let f = fx("person", true);
    let s = f.open();
    let p = f.person(&s);
    let v = s.status().unwrap().version;
    f.pack.refuse(Some("u2"));
    let out = person_set(&s, p, "u2", "title", "B2");
    let Err(e) = out else {
        return Err(format!("{ROW}: not refused at the call: {out:?}"));
    };
    is_refusal(ROW, "the person's edit", &e)?;
    need(f.pack.checked() > 0, ROW, || {
        "the pack was never asked".into()
    })?;
    nothing_moved(ROW, &f, &s, v)
}

#[test]
fn a_persons_refused_edit_is_refused_at_the_call() {
    row_a_persons_refused_edit_is_refused_at_the_call().unwrap();
}

/// The planted fault "edit check dropped": the person's row goes red, by name.
#[test]
fn the_planted_edit_check_dropped_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::EditCheckDropped, || {
        let name = "row_a_persons_refused_edit_is_refused_at_the_call";
        let e = row_a_persons_refused_edit_is_refused_at_the_call()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("not refused at the call"), "{e}");
    });
}

// ---------- an agent's edits ----------

/// An agent's pack verb and its computed edit, on the refused unit: each refused at the call,
/// typed; nothing moved.
fn row_an_agents_edits_are_refused_at_the_call() -> Result<(), String> {
    const ROW: &str = "row_an_agents_edits_are_refused_at_the_call";
    let f = fx("agent", true);
    let s = f.open();
    let bot = f.agent(&s);
    let v = s.status().unwrap().version;
    f.pack.refuse(Some("u2"));
    let verb = s.run_tool(
        bot,
        UNITS,
        &argv(&format!("unit set u2 title B2 --base {v}")),
        b"",
    );
    match verb {
        Err(ToolRunError::Session(e)) => is_refusal(ROW, "the verb", &e)?,
        other => {
            return Err(format!(
                "{ROW}: the verb not refused at the call: {other:?}"
            ))
        }
    }
    let mut opts = EditOptions::new();
    opts.base = Some(v);
    let computed = s.edit_ops_with(bot, &opts, |_: &View| {
        Ok::<_, String>(set("u2", "title", "B3"))
    });
    match computed {
        Err(EditWithError::Session(e)) => is_refusal(ROW, "the computed edit", &e)?,
        other => {
            return Err(format!(
                "{ROW}: the computed edit not refused at the call: {other:?}"
            ))
        }
    }
    nothing_moved(ROW, &f, &s, v)
}

#[test]
fn an_agents_edits_are_refused_at_the_call() {
    row_an_agents_edits_are_refused_at_the_call().unwrap();
}

// ---------- an undo ----------

/// A person's change the pack took, then refuses once the unit is driven: undoing it would
/// change the unit, so the undo is refused at the call, typed, and nothing moves.
#[test]
fn an_undo_that_would_change_a_refused_unit_is_refused() {
    let f = fx("undo", true);
    let s = f.open();
    let p = f.person(&s);
    let v = person_set(&s, p, "u2", "title", "B2").expect("taken while the pack accepts it");
    f.pack.refuse(Some("u2"));
    let e = s.undo(p, 1).expect_err("the undo is refused");
    is_refusal("undo", "the undo", &e).unwrap();
    assert_eq!(s.status().unwrap().version, v, "the version didn't move");
    let read = s.read(p).unwrap();
    assert_eq!(field(&read, "u2", "title").as_deref(), Some("B2"));
}

// ---------- an accepted change ----------

/// A change the pack accepts (another unit) commits and writes back as before.
#[test]
fn an_accepted_change_commits_and_writes_back() {
    let f = fx("accepted", true);
    let s = f.open();
    let p = f.person(&s);
    let v0 = s.status().unwrap().version;
    f.pack.refuse(Some("u2"));
    let before = f.pack.checked();
    let v = person_set(&s, p, "u1", "title", "A2").expect("the pack accepts it");
    assert!(v > v0, "the version moved");
    assert_eq!(f.pack.checked(), before + 1, "asked once, for the one step");
    assert!(s.flush(p).unwrap().written);
    assert_eq!(f.bytes(), "u1\ttitle=A2\nu2\ttitle=B\nu3\ttitle=C\n");
}

/// A pack that doesn't opt in is never asked.
#[test]
fn a_pack_that_doesnt_opt_in_is_never_asked() {
    let f = fx("off", true);
    f.pack.checks.store(false, Ordering::SeqCst);
    let s = f.open();
    let p = f.person(&s);
    f.pack.refuse(Some("u2"));
    person_set(&s, p, "u2", "title", "B2").expect("no check: taken");
    assert_eq!(f.pack.checked(), 0);
}

// ---------- a text doc ----------

/// A text pack that refuses any text holding `FORBIDDEN`.
struct CheckedText(TextFilePack, Arc<AtomicU64>);

impl Pack for CheckedText {
    fn schema(&self) -> Schema {
        self.0.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.0.encode(c, codec)
    }
    fn checks_edits(&self) -> bool {
        true
    }
    fn check_edit(&self, _: &View, edits: &[Edit]) -> Result<(), PackRefusal> {
        self.1.fetch_add(1, Ordering::SeqCst);
        let bad = edits.iter().any(|e| match e {
            Edit::Text { hunks, .. } => hunks.iter().any(|h| h.insert.contains("FORBIDDEN")),
            _ => false,
        });
        match bad {
            true => Err(PackRefusal {
                code: REFUSED,
                message: "this file can't hold that word".into(),
            }),
            false => Ok(()),
        }
    }
}

/// A text doc's pack that opts in is asked of a text change: a refused one is refused at the
/// call, an accepted one commits.
#[test]
fn a_text_change_is_checked_too() {
    let f = fx("text", false);
    std::fs::write(f.path(), "hello\n").unwrap();
    let asked = Arc::new(AtomicU64::new(0));
    let s = f
        .mgr
        .open_file(
            Arc::new(CheckedText(
                TextFilePack::new("checked-text", 1 << 20),
                asked.clone(),
            )),
            &net_pack::key(),
            &f.path(),
            &f.ws,
        )
        .unwrap();
    let p = f.join(&s, "ann", ParticipantKind::Person);
    let v = s.status().unwrap().version;
    let e = s
        .replace_text(p, TextFilePack::ROOT, "hello FORBIDDEN\n")
        .expect_err("refused");
    is_refusal("text", "the text change", &e).unwrap();
    assert_eq!(s.status().unwrap().version, v, "the version didn't move");
    let ok = s
        .replace_text(p, TextFilePack::ROOT, "hello world\n")
        .unwrap();
    assert!(ok.version > v);
    assert!(asked.load(Ordering::SeqCst) >= 2, "asked of both");
}

// ---------- a purge ----------

const SECRET: &str = "Qz7Kx9Wm3pV8rT2n";

/// A purge's removal of a value the pack would refuse to change goes through: erasure is never
/// the pack's to refuse.
#[test]
fn a_purges_removal_is_never_refused() {
    let f = fx("purge", true);
    let s = f.open();
    let p = f.person(&s);
    person_set(&s, p, "u2", "title", SECRET).expect("taken while the pack accepts it");
    f.pack.refuse(Some("u2"));
    let out = s.purge(
        p,
        PurgeRequest::new(PurgeScope::Text(SECRET.into())).and_remove(),
    );
    assert!(out.is_ok(), "the purge: {out:?}");
    let read = s.read(p).unwrap();
    assert_eq!(field(&read, "u2", "title").as_deref(), Some(""), "{read:?}");
}
