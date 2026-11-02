//! The engine seam. Private to the kit: no pack sees an engine type.
//!
//! To add an engine, implement [`EngineDoc`] and add it to [`Engine`], the enum
//! [`crate::doc::Doc`] holds (C1, v0.10.1).
//! Packs, sessions and stored formats above this line don't change (stored state carries
//! an engine/format stamp so it can be re-encoded on upgrade).

mod structured;
#[cfg(test)]
mod structured_tests;
mod yrs;

#[cfg(test)]
pub(crate) use self::yrs::insert_clocks;
#[cfg(any(test, feature = "differential"))]
pub(crate) use self::yrs::TextPlan;
pub(crate) use self::yrs::YrsDoc;

use std::collections::BTreeMap;

use crate::elem::{Edited, ElemState, Facet, Key, Value, WriteId};

/// A relative position in a text root (the conformance suite's anchors only).
#[cfg(feature = "conformance")]
pub(crate) type Sticky = ::yrs::StickyIndex;

use crate::doc::{ClientId, DocError, Edit, UpdateInfo};
use crate::schema::Schema;

/// The doc's own client ID. It never authors content.
pub(crate) const SESSION_CLIENT: ClientId = 1;

/// Engine identity and stored-state encoding, stamped on each doc in the store
/// ([`crate::store::DocMeta`]) so a later release can re-encode on upgrade.
pub(crate) const ENGINE_NAME: &str = "yrs-0.28";
/// Stored state is Yjs v2.
pub(crate) const ENGINE_FORMAT: u32 = 2;
/// The engine's family: a stamp's name up to its first `-` (`yrs` of `yrs-0.28`).
pub(crate) const ENGINE_FAMILY: &str = "yrs";

/// Whether this build's engine reads a doc stamped `engine` / `format` (v0.10.1): the stamp
/// names this engine's family (the name up to its first `-`) and this stored-state format.
/// The rest of the name is the library version that wrote the doc, for the record only: a
/// library update that keeps the format reads its docs as they are (re-encoding on upgrade
/// changes the format). An unstamped doc (an empty name, whatever its format) reads as this
/// engine's: every store the kit wrote stamps its docs, and Yrs is the only engine it ever
/// shipped, so only a host's own `DocStore::create` can leave the name empty.
pub(crate) fn reads_stamp(engine: &str, format: u32) -> bool {
    if engine.is_empty() {
        return true;
    }
    let family = engine.split_once('-').map_or(engine, |(f, _)| f);
    family == ENGINE_FAMILY && format == ENGINE_FORMAT
}

/// An engine's element units (proposal P2, v0.10.1): what one clock step of an element ID
/// counts. Element IDs are `(client, clock)` with a u64 clock in these units, and a run of
/// text `s` occupies `units.of(s)` clocks: the character `k` units into a run whose first
/// ID is `(client, c)` has ID `(client, c + k)`. Yrs counts UTF-16 code units; a native
/// engine counts scalars. The units are the engine's, chosen by the doc's engine stamp
/// ([`ENGINE_NAME`]): the kit never assumes UTF-16 outside the Yrs adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Units {
    /// UTF-16 code units (Yrs).
    Utf16,
}

impl Units {
    /// How many units `s` occupies.
    pub fn of(self, s: &str) -> u64 {
        match self {
            Units::Utf16 => s.encode_utf16().count() as u64,
        }
    }

    /// How many units `c` occupies.
    pub fn of_char(self, c: char) -> u64 {
        match self {
            Units::Utf16 => c.len_utf16() as u64,
        }
    }
}

// There is no build-wide unit (C2): every caller takes the doc's ([`EngineDoc::units`],
// `Doc::units`), so a doc on another engine counts in its own.

/// How an engine numbers what one change inserts (C2): the clock of each text hunk's first
/// inserted element, from the client's next clock before the change. The edit path reports
/// it as it applied it ([`Made::firsts`]); this is the prediction, for the one caller that
/// needs IDs before anything is applied (undo's planner applies each step to scratch copies
/// first, `session::undo::plan`). A test checks it equals what the engine applied, on
/// every engine (`firsts_are_the_engines_ids`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdOrder {
    /// Yrs (the adapter's apply order): edits in order, each text edit's hunks **back to
    /// front**, and only inserted text takes clocks (UTF-16 code units; deletes take none).
    /// Structured roots are written after every text root.
    Yrs,
}

/// [`IdOrder::plan`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    /// Per edit, per hunk of a text edit, the clock of the first element it inserts (a hunk
    /// that inserts nothing: where its insertion would start); empty for a structured edit.
    pub firsts: Vec<Vec<u64>>,
    /// The client's next clock after the change's text.
    pub next: u64,
}

impl IdOrder {
    /// The clocks a change applying `edits`, by a client whose next clock is `clock`, gives
    /// what each text hunk inserts. `text_of(root)` is a root's text before the change (an
    /// engine whose deletes take clocks counts them from it; Yrs reads none).
    pub fn plan<'t>(
        self,
        clock: u64,
        edits: &[Edit],
        text_of: &dyn Fn(&str) -> Option<&'t str>,
    ) -> Planned {
        match self {
            IdOrder::Yrs => {
                let _ = text_of;
                let firsts = self::yrs::insert_clocks(clock, edits);
                let next = clock
                    + edits
                        .iter()
                        .filter_map(|e| match e {
                            Edit::Text { hunks, .. } => Some(hunks),
                            Edit::List { .. } | Edit::Map { .. } => None,
                        })
                        .flatten()
                        .map(|h| Units::Utf16.of(&h.insert))
                        .sum::<u64>();
                Planned { firsts, next }
            }
        }
    }
}

/// What an edit made (C2): the doc's [`Edited`], and per edit, per text hunk, the clock of
/// the first element the hunk inserted, as the engine applied it ([`IdOrder`]'s order; a
/// hunk that inserts nothing: where its insertion would have started); empty for a
/// structured edit. The client is the edit's. Replaces predicting the clocks after the fact.
#[derive(Debug)]
pub(crate) struct Made {
    pub edited: Edited,
    pub firsts: Vec<Vec<u64>>,
}

/// The stamp of a doc with no engine state: a tier-0 (blob) doc.
pub(crate) const NO_ENGINE: &str = "none";

/// Whether this build may change the stored rows of a doc stamped `engine` / `format`
/// (delete, purge; v0.10.1): a tier-0 doc ([`NO_ENGINE`]: no engine state) or one this
/// build's engine reads ([`reads_stamp`]). A doc another engine wrote may hold rows in tables
/// this build doesn't know, so erasing only the rows it knows would leave the rest behind.
pub(crate) fn owns_stamp(engine: &str, format: u32) -> bool {
    engine == NO_ENGINE || reads_stamp(engine, format)
}

/// One run of a text root's live text inserted by one client with consecutive clocks
/// ([`EngineDoc::blame_runs`]). The runs of a root, in order, cover its text exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlameRun {
    /// The run's length in UTF-8 bytes.
    pub bytes: usize,
    /// The client that inserted it.
    pub client: ClientId,
    /// The clock of its first character, in the engine's units ([`Units`]): the character
    /// `k` units into the run has clock `clock + k`.
    pub clock: u64,
}

/// What the engine's sampled runtime check (the canary, v0.9.3) found: an edit whose fast
/// update differed from the throwaway replica's (the throwaway's was served; the doc uses
/// that path until reload), or a cached text that differed from a fresh read (the fresh one
/// was used; the cache is off until reload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EngineCheck {
    EditPath {
        client: ClientId,
    },
    TextCache {
        root: &'static str,
    },
    /// The engine's position index (v0.9.7): a shadow check found the marker path and the
    /// full walk disagree on text `root` (`None`: a nested text) at `index`, from the marker
    /// at `marker`. The walk's result was used and the text's markers dropped.
    Position {
        root: Option<String>,
        index: u32,
        marker: u32,
    },
}

#[allow(dead_code)] // the trait documents the seam; one implementation today
pub(crate) trait EngineDoc: Sized + Send {
    /// A new, empty doc on `schema`. Fallible (P14, v0.10.1): an engine may refuse a schema
    /// it can't hold; Yrs refuses none.
    fn new(schema: &Schema) -> Result<Self, DocError>;
    fn load(schema: &Schema, state: &[u8]) -> Result<Self, DocError>;
    /// Text root `root`'s current text. Fallible (P14): a root the doc doesn't hold is an
    /// error ([`DocError::Malformed`]).
    fn text(&self, root: &str) -> Result<String, DocError>;
    fn state_vector(&self) -> Vec<u8>;
    /// Fails (never panics) on a poisoned doc: see [`crate::doc::DocError::Pending`].
    fn encode_state(&self) -> Result<Vec<u8>, DocError>;
    fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError>;
    fn check_update(&self, update: &[u8], allowed: ClientId) -> Result<UpdateInfo, DocError>;
    fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError>;
    /// Apply edits (roots, kinds and text hunks already checked) as `client`, as one change on
    /// this doc (v0.9.3: on the doc itself, as the client, with a sampled check against a
    /// throwaway replica). Structured ops are checked here, in order; a refused op
    /// refuses the change and nothing is applied. Returns the update (`None` when nothing is
    /// written: no text hunks, and every structured op a no-op) and the applied stream, in
    /// apply order, at element identity, with each text hunk's first inserted clock
    /// ([`Made`]).
    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Made, DocError>;
    /// A MovableList root's live elements, in order.
    fn list(&self, root: &str) -> Result<Vec<(Key, Value)>, DocError>;
    fn elem(&self, root: &str, key: &str) -> Result<ElemState, DocError>;
    /// A known element's value, live or deleted.
    fn elem_value(&self, root: &str, key: &str) -> Result<Option<Value>, DocError>;
    /// A list root's deleted elements and their values (for the keyed diff).
    fn hidden(&self, root: &str) -> Result<BTreeMap<Key, Value>, DocError>;
    /// Every element of a list root, live and deleted, in place order.
    fn places(&self, root: &str) -> Result<Vec<Key>, DocError>;
    fn map(&self, root: &str) -> Result<BTreeMap<String, Value>, DocError>;
    fn map_get(&self, root: &str, key: &str) -> Result<Option<Value>, DocError>;
    /// The write that holds a register now (E4's `winner`), from the state alone: O(1).
    fn winner(&self, root: &str, key: &str, facet: Facet) -> Result<Option<WriteId>, DocError>;
    /// How many clock units `client` has inserted into this doc (0 if none): a change by
    /// `client` occupies the clocks between this before it and this after it. In the
    /// engine's units ([`EngineDoc::units`]).
    fn clock_of(&self, client: ClientId) -> u64;
    /// The engine's element units ([`Units`]).
    fn units(&self) -> Units;
    /// How the engine numbers what a change inserts ([`IdOrder`]).
    fn id_order(&self) -> IdOrder;
    /// The largest clock this engine makes (Yrs: `u32::MAX`). A clock the store holds for
    /// this doc above it can only be damage: the store's clock reads refuse it
    /// ([`crate::store::StoreError::Corrupt`]) instead of handing it on.
    fn max_clock(&self) -> u64;
    /// Blame (K-HIST-11): text root `root`'s live text as runs, each with the client and
    /// clock that inserted it. **O(doc) per call** (the engine copies the doc to a throwaway
    /// replica and walks it); never on the edit path. Leaves this doc as it was.
    fn blame_runs(&self, root: &str) -> Result<Vec<BlameRun>, DocError>;
}

/// The engine a doc runs on (C1, v0.10.1): one variant per engine this build carries, read
/// from the doc's engine stamp ([`reads_stamp`]: a stamp no variant reads is refused at load,
/// `MPC-E520`). Yrs is the only variant today. [`crate::doc::Doc`] holds this, not an engine
/// type, so a second engine is a second variant: every `match` below must then say what it
/// does there, and the compiler finds each one.
pub(crate) enum Engine {
    Yrs(YrsDoc),
}

/// `$body` on the doc's engine, bound to `$d`, whichever variant it is.
macro_rules! on_engine {
    ($e:expr, $d:ident => $body:expr) => {
        match $e {
            Engine::Yrs($d) => $body,
        }
    };
}

impl EngineDoc for Engine {
    /// A new doc runs on this build's default engine (Yrs).
    fn new(schema: &Schema) -> Result<Self, DocError> {
        Ok(Engine::Yrs(YrsDoc::new(schema)?))
    }

    /// Stored state of this build's engine: the session checks the doc's stamp before
    /// ([`reads_stamp`]), so the state is Yrs's.
    fn load(schema: &Schema, state: &[u8]) -> Result<Self, DocError> {
        Ok(Engine::Yrs(YrsDoc::load(schema, state)?))
    }

    fn text(&self, root: &str) -> Result<String, DocError> {
        on_engine!(self, d => d.text(root))
    }

    fn state_vector(&self) -> Vec<u8> {
        on_engine!(self, d => d.state_vector())
    }

    fn encode_state(&self) -> Result<Vec<u8>, DocError> {
        on_engine!(self, d => d.encode_state())
    }

    fn encode_diff(&self, remote_sv: &[u8]) -> Result<Vec<u8>, DocError> {
        on_engine!(self, d => d.encode_diff(remote_sv))
    }

    fn check_update(&self, update: &[u8], allowed: ClientId) -> Result<UpdateInfo, DocError> {
        on_engine!(self, d => d.check_update(update, allowed))
    }

    fn apply_trusted(&mut self, update: &[u8]) -> Result<(), DocError> {
        on_engine!(self, d => d.apply_trusted(update))
    }

    fn edit_as(&mut self, client: ClientId, edits: &[Edit]) -> Result<Made, DocError> {
        on_engine!(self, d => d.edit_as(client, edits))
    }

    fn list(&self, root: &str) -> Result<Vec<(Key, Value)>, DocError> {
        on_engine!(self, d => d.list(root))
    }

    fn elem(&self, root: &str, key: &str) -> Result<ElemState, DocError> {
        on_engine!(self, d => d.elem(root, key))
    }

    fn elem_value(&self, root: &str, key: &str) -> Result<Option<Value>, DocError> {
        on_engine!(self, d => d.elem_value(root, key))
    }

    fn hidden(&self, root: &str) -> Result<BTreeMap<Key, Value>, DocError> {
        on_engine!(self, d => d.hidden(root))
    }

    fn places(&self, root: &str) -> Result<Vec<Key>, DocError> {
        on_engine!(self, d => d.places(root))
    }

    fn map(&self, root: &str) -> Result<BTreeMap<String, Value>, DocError> {
        on_engine!(self, d => d.map(root))
    }

    fn map_get(&self, root: &str, key: &str) -> Result<Option<Value>, DocError> {
        on_engine!(self, d => d.map_get(root, key))
    }

    fn winner(&self, root: &str, key: &str, facet: Facet) -> Result<Option<WriteId>, DocError> {
        on_engine!(self, d => d.winner(root, key, facet))
    }

    fn clock_of(&self, client: ClientId) -> u64 {
        on_engine!(self, d => d.clock_of(client))
    }

    fn units(&self) -> Units {
        on_engine!(self, d => d.units())
    }

    fn id_order(&self) -> IdOrder {
        on_engine!(self, d => d.id_order())
    }

    fn max_clock(&self) -> u64 {
        on_engine!(self, d => d.max_clock())
    }

    fn blame_runs(&self, root: &str) -> Result<Vec<BlameRun>, DocError> {
        on_engine!(self, d => d.blame_runs(root))
    }
}

/// What a doc's engine does beyond the seam: the checks and the comparisons the kit runs on
/// it (Yrs's today). A second engine says what each does there, or that it has none.
impl Engine {
    /// The same inserts as `other` (state vectors). Docs on two engines never compare equal.
    pub(crate) fn same_state_vector(&self, other: &Engine) -> bool {
        match (self, other) {
            (Engine::Yrs(a), Engine::Yrs(b)) => a.same_state_vector(b),
        }
    }

    /// The same content as `other` ([`crate::doc::Doc::same_content`]). Docs on two engines
    /// never compare equal.
    pub(crate) fn same_content(&self, other: &Engine) -> bool {
        match (self, other) {
            (Engine::Yrs(a), Engine::Yrs(b)) => a.same_content(b),
        }
    }

    /// Updates the doc couldn't integrate, or a poisoned doc (K-AUD-1).
    pub(crate) fn unresolved(&self) -> bool {
        on_engine!(self, d => d.unresolved())
    }

    /// What the engine's runtime checks found since the last call.
    pub(crate) fn take_checks(&mut self) -> Vec<EngineCheck> {
        on_engine!(self, d => d.take_checks())
    }

    /// Whether the edit path fell back to the throwaway replica (the canary, v0.9.3).
    pub(crate) fn on_fallback(&self) -> bool {
        on_engine!(self, d => d.on_fallback())
    }

    /// The canary's sampling: one edit in `every`.
    pub(crate) fn set_check_every(&mut self, every: u32) {
        on_engine!(self, d => d.set_check_every(every))
    }

    /// The canary's sampling scale, in history units.
    pub(crate) fn set_check_scale(&mut self, units: u64) {
        on_engine!(self, d => d.set_check_scale(units))
    }

    /// The position index's shadow check: one lookup in `every`.
    pub(crate) fn set_position_every(&mut self, every: u32) {
        on_engine!(self, d => d.set_position_every(every))
    }

    /// Check, then apply, a client's update: the body of `Doc::client_applied`
    /// (`untrusted-wire`, and the conformance suite's hook).
    #[cfg(any(feature = "untrusted-wire", feature = "conformance"))]
    pub(crate) fn apply_client(
        &mut self,
        update: &[u8],
        allowed: ClientId,
    ) -> Result<UpdateInfo, DocError> {
        on_engine!(self, d => d.apply_client(update, allowed))
    }

    /// The Yrs doc, for the Yrs-only hooks of test, `differential` and `conformance` builds
    /// (the canary's and the position index's test hooks, the conformance suite's sync and
    /// anchors); `None` on another engine (C2), where each hook says what it does instead
    /// ([`crate::doc::Doc`]'s hooks).
    #[allow(dead_code)] // test and feature builds only
    pub(crate) fn yrs(&self) -> Option<&YrsDoc> {
        match self {
            Engine::Yrs(d) => Some(d),
        }
    }

    #[allow(dead_code)] // test and feature builds only
    pub(crate) fn yrs_mut(&mut self) -> Option<&mut YrsDoc> {
        match self {
            Engine::Yrs(d) => Some(d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine enum (C1, v0.10.1): a new or loaded doc runs on Yrs, the only variant, with
    /// Yrs's units (UTF-16 code units) and clock bound (u32), and the seam's calls reach it.
    #[test]
    fn a_doc_runs_on_the_yrs_variant_with_its_units() {
        const S: Schema = Schema {
            name: "t",
            roots: &[("source", crate::schema::Kind::Text)],
        };
        let mut e = Engine::new(&S).unwrap();
        assert!(matches!(e, Engine::Yrs(_)));
        assert_eq!(e.units(), Units::Utf16);
        assert_eq!(e.max_clock(), u64::from(u32::MAX));
        let edit = Edit::Text {
            root: "source".into(),
            hunks: vec![crate::text::diff::Hunk {
                start: 0,
                end: 0,
                insert: "a🎉é".into(),
            }],
        };
        e.edit_as(7, &[edit]).unwrap();
        assert_eq!(e.text("source").unwrap(), "a🎉é");
        assert_eq!(e.clock_of(7), Units::Utf16.of("a🎉é"));
        let l = Engine::load(&S, &e.encode_state().unwrap()).unwrap();
        assert!(l.same_content(&e) && l.same_state_vector(&e));
        assert!(matches!(e.text("nope"), Err(DocError::Malformed(_))));
    }

    /// `edit_made` on `doc` as `client`: [`Made::firsts`] are the engine's own IDs (each
    /// hunk's inserted characters, read back from blame, are `(client, first + k units)`),
    /// and the engine's [`IdOrder`] predicts them from the texts before, with the client's
    /// next clock after. On every engine the doc can run on (C2).
    pub(crate) fn check_firsts(doc: &mut crate::doc::Doc, client: ClientId, edits: &[Edit]) {
        let roots: Vec<String> = edits.iter().map(|e| e.root().to_owned()).collect();
        let before: Vec<(String, String)> = roots
            .iter()
            .map(|r| (r.clone(), doc.text(r).unwrap()))
            .collect();
        let clock = doc.clock_of(client);
        let made = doc.edit_made(client, edits).unwrap();
        let text_of = |r: &str| before.iter().find(|(x, _)| x == r).map(|(_, t)| t.as_str());
        let planned = doc.id_order().plan(clock, edits, &text_of);
        assert_eq!(made.firsts, planned.firsts, "the order's prediction");
        assert_eq!(
            doc.clock_of(client),
            planned.next,
            "the client's next clock"
        );
        let u = doc.units();
        for (e, firsts) in edits.iter().zip(&made.firsts) {
            let Edit::Text { root, hunks } = e else {
                assert!(firsts.is_empty(), "a structured edit has no text firsts");
                continue;
            };
            assert_eq!(firsts.len(), hunks.len());
            let after = doc.text(root).unwrap();
            // each character of `after`, its element ID, from blame
            let mut ids = Vec::new();
            let mut at = 0;
            for r in doc.blame_runs(root).unwrap() {
                let mut k = r.clock;
                for ch in after[at..at + r.bytes].chars() {
                    ids.push((r.client, k));
                    k += u.of_char(ch);
                }
                at += r.bytes;
            }
            assert_eq!(at, after.len(), "blame covers the text");
            let mut shift: isize = 0;
            for (h, first) in hunks.iter().zip(firsts) {
                let start = (h.start as isize + shift) as usize;
                shift += h.insert.len() as isize - (h.end - h.start) as isize;
                let skip = after[..start].chars().count();
                let mut k = *first;
                for (i, ch) in h.insert.chars().enumerate() {
                    assert_eq!(ids[skip + i], (client, k), "hunk {h:?}'s character {i}");
                    k += u.of_char(ch);
                }
            }
        }
    }

    /// [`Made::firsts`] and [`IdOrder::plan`] name the IDs the engine gives: several hunks
    /// with deletes, multi-byte and astral text, two roots in one change, a client that has
    /// written before, and a structured edit's empty list.
    #[test]
    fn firsts_are_the_engines_ids() {
        use crate::text::diff::Hunk;
        const S: Schema = Schema {
            name: "t2",
            roots: &[
                ("a", crate::schema::Kind::Text),
                ("b", crate::schema::Kind::Text),
            ],
        };
        let h = |start, end, insert: &str| Hunk {
            start,
            end,
            insert: insert.into(),
        };
        let mut d = crate::doc::Doc::new(S).unwrap();
        d.replace_text_as(500, "a", "one two three\n").unwrap();
        d.replace_text_as(500, "b", "αβγ🎉\nz").unwrap();
        let edits = vec![
            Edit::Text {
                root: "a".into(),
                hunks: vec![h(0, 3, "ONE🎉"), h(4, 4, "é"), h(8, 13, "")],
            },
            Edit::Text {
                root: "b".into(),
                hunks: vec![h(2, 4, "x"), h(10, 10, "αω")],
            },
        ];
        check_firsts(&mut d, 600, &edits);
        // the same client again: its clocks continue
        let edits = vec![Edit::Text {
            root: "a".into(),
            hunks: vec![h(0, 0, "🎉🎉"), h(7, 8, "Z")],
        }];
        check_firsts(&mut d, 600, &edits);
    }

    /// Yrs's units are UTF-16 code units: an astral character is two.
    #[test]
    fn utf16_units() {
        assert_eq!(Units::Utf16.of("a🎉é"), 4);
        assert_eq!(Units::Utf16.of(""), 0);
        assert_eq!(Units::Utf16.of_char('🎉'), 2);
        assert_eq!(Units::Utf16.of_char('é'), 1);
    }

    /// Every doc this build stamps, it reads; another engine's, or another format's, it
    /// doesn't; an unstamped one it does.
    #[test]
    fn this_engine_reads_its_own_stamp_and_no_other() {
        assert!(ENGINE_NAME.starts_with(ENGINE_FAMILY));
        assert!(reads_stamp(ENGINE_NAME, ENGINE_FORMAT));
        // another library version of the same engine and format
        assert!(reads_stamp("yrs-0.27", ENGINE_FORMAT));
        assert!(reads_stamp("yrs", ENGINE_FORMAT));
        // unstamped
        assert!(reads_stamp("", 0));
        assert!(reads_stamp("", ENGINE_FORMAT));
        // another engine, another format, a blob doc's stamp, a lookalike family
        assert!(!reads_stamp("loro-1.0", ENGINE_FORMAT));
        assert!(!reads_stamp("mpce-0.1", 1));
        assert!(!reads_stamp(ENGINE_NAME, 1));
        assert!(!reads_stamp(ENGINE_NAME, ENGINE_FORMAT + 1));
        assert!(!reads_stamp("none", 0));
        assert!(!reads_stamp("yrsx-0.28", ENGINE_FORMAT));
        assert!(!reads_stamp("YRS-0.28", ENGINE_FORMAT));
    }

    /// Delete and purge (v0.10.1): a tier-0 doc and this engine's are this build's to change;
    /// another engine's, or this engine at another format, are not.
    #[test]
    fn this_build_owns_tier0_docs_and_its_engines_only() {
        assert!(owns_stamp(NO_ENGINE, 0));
        assert!(owns_stamp(ENGINE_NAME, ENGINE_FORMAT));
        assert!(owns_stamp("", 0));
        assert!(!owns_stamp("mpce-0.1", 1));
        assert!(!owns_stamp("loro-1.0", ENGINE_FORMAT));
        assert!(!owns_stamp(ENGINE_NAME, ENGINE_FORMAT + 1));
    }
}
