//! The ID-churn judge (K-COD-10), for packs' tests (feature `test-support`): over a corpus, a
//! no-op save and each one-unit edit must keep the native ID of every object they don't
//! touch.
//!
//! The judge runs each file the way a session does: it decodes it into a doc (the import),
//! writes the doc back with [`Pack::encode`], and reads the written bytes back with
//! [`Pack::reconcile`] (what an outside-change check does with a file it didn't write). The
//! pack's test says where the IDs are, with an extractor over a [`Saved`] (the file's bytes
//! and the doc's [`View`]): a codec whose IDs live in the file (docx `w14:paraId`) reads the
//! bytes; a kit-minted key (a grid's `r17`) is read from the view. It gives one
//! `(object, id)` pair per object, where `object` names the object without its ID (its
//! content, its path), so the same object is found again after a save.
//!
//! - **No-op save**: the written bytes are the file's own, byte for byte, and every
//!   object keeps its ID.
//! - **One-unit edit** ([`Unit`]), each from the imported doc: a verb's edits on the doc,
//!   written back and read back ([`Change::Doc`]), or an outside save of new bytes, read back
//!   ([`Change::File`]). Every object the unit doesn't name in [`Unit::touches`] keeps its
//!   ID; objects it adds are free.
//!
//! An extractor lists only the kinds that should survive (the pack's durable and file-local
//! ones, `ids::IdClass`): positional IDs move by definition. It must name each object once
//! (two objects with one name are reported, never guessed between).
//!
//! Not built here: refusing, at write time, a save that would renumber an ID (the row's
//! "a write that would is refused"); this is the test harness.

use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::fmt;

use crate::doc::{Doc, Edit, Neutral};
use crate::session::{Pack, View};

/// The participant the judge imports as (any ID above the reserved ones).
const IMPORT_CLIENT: crate::ClientId = 2;
/// The participant a unit's doc edits are made as.
const EDIT_CLIENT: crate::ClientId = 3;

/// A file as saved: its bytes and the doc they were read into (or written from).
#[derive(Debug, Clone, Copy)]
pub struct Saved<'a> {
    pub bytes: &'a [u8],
    pub view: &'a View,
}

/// How a one-unit edit changes the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A verb's edits on the imported doc (what a session applies), then written back with
    /// [`Pack::encode`] and read back with [`Pack::reconcile`].
    Doc(Vec<Edit>),
    /// An outside save: the file's new bytes, read back with [`Pack::reconcile`] against the
    /// imported doc.
    File(Vec<u8>),
}

/// One one-unit edit and the objects it may change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// For the report (`cell set r2,c1`).
    pub name: String,
    /// The objects (as the extractor names them before the edit) the unit changes: their IDs
    /// may change or go. Every name must be one the extractor gives before the edit.
    pub touches: Vec<String>,
    pub change: Change,
}

impl Unit {
    pub fn doc(name: impl Into<String>, touches: &[&str], edits: Vec<Edit>) -> Self {
        Unit {
            name: name.into(),
            touches: touches.iter().map(|s| (*s).to_owned()).collect(),
            change: Change::Doc(edits),
        }
    }

    pub fn file(name: impl Into<String>, touches: &[&str], bytes: impl Into<Vec<u8>>) -> Self {
        Unit {
            name: name.into(),
            touches: touches.iter().map(|s| (*s).to_owned()).collect(),
            change: Change::File(bytes.into()),
        }
    }
}

/// What a clean run checked, so a test can tell it wasn't empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Judged {
    pub files: usize,
    /// No-op saves and units run.
    pub steps: usize,
    /// (object, ID) pairs compared across all steps.
    pub ids_checked: usize,
}

/// One failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChurnKind {
    /// An untouched object's ID changed.
    Renumbered {
        object: String,
        before: String,
        after: String,
    },
    /// An untouched object has no ID after the step (it went, or changed without the unit
    /// naming it).
    Lost { object: String, id: String },
    /// The no-op save's bytes differ from the file's, from this byte offset.
    NotByteExact { at: usize },
    /// The extractor named two objects alike, so the judge can't tell them apart.
    Ambiguous { object: String },
    /// The unit names an object the extractor doesn't give before it (a test's mistake).
    UnknownTouch { object: String },
    /// The pack refused or failed (decode, encode, reconcile) or the doc refused an edit.
    Failed(String),
}

impl fmt::Display for ChurnKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChurnKind::Renumbered {
                object,
                before,
                after,
            } => write!(f, "{object:?} renumbered {before:?} -> {after:?}"),
            ChurnKind::Lost { object, id } => write!(f, "{object:?} lost its ID {id:?}"),
            ChurnKind::NotByteExact { at } => {
                write!(f, "the save's bytes differ from the file's at byte {at}")
            }
            ChurnKind::Ambiguous { object } => {
                write!(
                    f,
                    "the extractor gave {object:?} twice (name each object once)"
                )
            }
            ChurnKind::UnknownTouch { object } => {
                write!(
                    f,
                    "the unit touches {object:?}, which the file doesn't have"
                )
            }
            ChurnKind::Failed(why) => f.write_str(why),
        }
    }
}

/// One failure, where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Churn {
    /// The corpus file's name.
    pub file: String,
    /// `no-op save`, or the unit's name.
    pub step: String,
    pub kind: ChurnKind,
}

/// Every failure of a run (the judge never stops at the first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChurnReport {
    pub failures: Vec<Churn>,
    /// What was checked before and around them.
    pub judged: Judged,
}

impl fmt::Display for ChurnReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ID churn: {} failure(s)", self.failures.len())?;
        for c in &self.failures {
            writeln!(f, "  {} / {}: {}", c.file, c.step, c.kind)?;
        }
        Ok(())
    }
}

impl std::error::Error for ChurnReport {}

/// The doc a session imports `bytes` into, and the codec settings.
struct Live {
    doc: Doc,
    codec: Vec<u8>,
}

impl Live {
    fn import(pack: &dyn Pack, bytes: &[u8]) -> Result<Live, String> {
        let schema = pack.schema();
        let content = pack
            .decode(bytes)
            .map_err(|d| format!("decode declined: {d}"))?;
        let mut ordered: Vec<(String, Neutral)> = Vec::with_capacity(schema.roots.len());
        for (name, _) in schema.roots {
            let mut found = content.roots.iter().filter(|(r, _)| r == name);
            match (found.next(), found.next()) {
                (Some(x), None) => ordered.push(x.clone()),
                _ => return Err(format!("decode gave root {name:?} other than once")),
            }
        }
        if content.roots.len() != ordered.len() {
            return Err("decode gave a root the schema doesn't have".into());
        }
        let mut doc = Doc::new(schema).map_err(|e| format!("doc: {e}"))?;
        doc.edit_as(
            IMPORT_CLIENT,
            &crate::session::test_support::import_edits(&ordered),
        )
        .map_err(|e| format!("import: {e}"))?;
        Ok(Live {
            doc,
            codec: content.codec,
        })
    }

    fn view(&self) -> Result<View, String> {
        let state = self
            .doc
            .keyed_state()
            .map_err(|e| format!("doc state: {e}"))?;
        Ok(View::new(state).with_codec(&self.codec))
    }

    fn edit(&mut self, client: crate::ClientId, edits: &[Edit]) -> Result<(), String> {
        if edits.is_empty() {
            return Ok(());
        }
        self.doc
            .edit_as(client, edits)
            .map(|_| ())
            .map_err(|e| format!("the doc refused the edits: {e}"))
    }

    /// The doc written back, as a session's write-back does.
    fn encode(&self, pack: &dyn Pack) -> Result<Vec<u8>, String> {
        pack.encode(&self.view()?.content(), &self.codec)
            .map_err(|e| format!("encode: {e}"))
    }

    /// `bytes` read back into the doc, by key (an outside save, or the doc's own write).
    fn read_back(&mut self, pack: &dyn Pack, bytes: &[u8]) -> Result<(), String> {
        let view = self.view()?;
        let r = pack
            .reconcile(&view, bytes, &self.codec)
            .map_err(|d| format!("reconcile declined: {d}"))?;
        self.edit(EDIT_CLIENT, &r.edits)?;
        self.codec = r.codec;
        Ok(())
    }
}

type Ids = BTreeMap<String, String>;

fn ids_of(pairs: Vec<(String, String)>, bad: &mut Vec<ChurnKind>) -> Ids {
    let mut m = Ids::new();
    for (object, id) in pairs {
        match m.entry(object) {
            Entry::Occupied(e) => bad.push(ChurnKind::Ambiguous {
                object: e.key().clone(),
            }),
            Entry::Vacant(e) => {
                e.insert(id);
            }
        }
    }
    m
}

/// Every untouched object of `before` keeps its ID in `after`; the pairs compared.
fn compare(before: &Ids, after: &Ids, touches: &[String], bad: &mut Vec<ChurnKind>) -> usize {
    for t in touches {
        if !before.contains_key(t) {
            bad.push(ChurnKind::UnknownTouch { object: t.clone() });
        }
    }
    let mut n = 0;
    for (object, id) in before {
        if touches.contains(object) {
            continue;
        }
        n += 1;
        match after.get(object) {
            None => bad.push(ChurnKind::Lost {
                object: object.clone(),
                id: id.clone(),
            }),
            Some(a) if a != id => bad.push(ChurnKind::Renumbered {
                object: object.clone(),
                before: id.clone(),
                after: a.clone(),
            }),
            Some(_) => {}
        }
    }
    n
}

/// Run the judge over `corpus` (name, file bytes): for each file, a no-op save, then each of
/// `units(name, imported)`'s one-unit edits, each from the imported doc. `extract` gives the
/// `(object, id)` pairs of a saved file. `Ok` only when no step failed anywhere; the report
/// lists every failure.
pub fn judge<E, U>(
    pack: &dyn Pack,
    corpus: &[(&str, &[u8])],
    extract: E,
    units: U,
) -> Result<Judged, ChurnReport>
where
    E: Fn(&Saved<'_>) -> Vec<(String, String)>,
    U: Fn(&str, &Saved<'_>) -> Vec<Unit>,
{
    let mut judged = Judged::default();
    let mut failures = Vec::new();
    for &(name, bytes) in corpus {
        judged.files += 1;
        let mut fail = |step: &str, kinds: Vec<ChurnKind>| {
            for kind in kinds {
                failures.push(Churn {
                    file: name.to_owned(),
                    step: step.to_owned(),
                    kind,
                });
            }
        };
        // the import, and the IDs every step starts from
        let start = Live::import(pack, bytes).and_then(|l| l.view().map(|v| (l, v)));
        let (live, view0) = match start {
            Ok(x) => x,
            Err(e) => {
                fail("import", vec![ChurnKind::Failed(e)]);
                continue;
            }
        };
        let saved0 = Saved {
            bytes,
            view: &view0,
        };
        let mut bad = Vec::new();
        let ids0 = ids_of(extract(&saved0), &mut bad);
        fail("import", std::mem::take(&mut bad));

        // the no-op save: written back, byte for byte, and read back
        judged.steps += 1;
        match no_op_save(pack, live) {
            Ok((out, view)) => {
                if out != bytes {
                    let at = out
                        .iter()
                        .zip(bytes.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(out.len().min(bytes.len()));
                    bad.push(ChurnKind::NotByteExact { at });
                }
                let after = ids_of(
                    extract(&Saved {
                        bytes: &out,
                        view: &view,
                    }),
                    &mut bad,
                );
                judged.ids_checked += compare(&ids0, &after, &[], &mut bad);
            }
            Err(e) => bad.push(ChurnKind::Failed(e)),
        }
        fail("no-op save", std::mem::take(&mut bad));

        // each one-unit edit, from the import
        for u in units(name, &saved0) {
            judged.steps += 1;
            match one_unit(pack, bytes, &u.change) {
                Ok((out, view)) => {
                    let after = ids_of(
                        extract(&Saved {
                            bytes: &out,
                            view: &view,
                        }),
                        &mut bad,
                    );
                    judged.ids_checked += compare(&ids0, &after, &u.touches, &mut bad);
                }
                Err(e) => bad.push(ChurnKind::Failed(e)),
            }
            fail(&u.name, std::mem::take(&mut bad));
        }
    }
    if failures.is_empty() {
        Ok(judged)
    } else {
        Err(ChurnReport { failures, judged })
    }
}

/// The imported doc written back and read back: the bytes written, and the doc after.
fn no_op_save(pack: &dyn Pack, mut live: Live) -> Result<(Vec<u8>, View), String> {
    let out = live.encode(pack)?;
    live.read_back(pack, &out)?;
    Ok((out, live.view()?))
}

/// `change` from a fresh import of `bytes`: the file's bytes after it, and the doc after.
fn one_unit(pack: &dyn Pack, bytes: &[u8], change: &Change) -> Result<(Vec<u8>, View), String> {
    let mut live = Live::import(pack, bytes)?;
    let out = match change {
        Change::Doc(edits) => {
            live.edit(EDIT_CLIENT, edits)?;
            let out = live.encode(pack)?;
            live.read_back(pack, &out)?;
            out
        }
        Change::File(new) => {
            live.read_back(pack, new)?;
            new.clone()
        }
    };
    Ok((out, live.view()?))
}
