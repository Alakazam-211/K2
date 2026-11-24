//! The keyed watch: what a person watching a live keyed doc (a deck, a board) needs, as numbered
//! frames (`client/WIRE.md`, "The keyed watch"). A child of `live`, so it works on the session's
//! own state.
//!
//! - **A snapshot first,** at a version `upto`: a head (`keyed_snapshot`: the schema, the view
//!   index's snapshot when the pack declares a view schema, and how many pages follow), then the
//!   doc's keyed state in pages (`keyed_page`), each one root's run of entries, then each
//!   container's units of the view index ([`crate::view_index::ViewIndex::units_page`]), under a
//!   byte cap the host sets ([`WatchOptions::page_bytes`]; [`WATCH_PAGE_BYTES`] by default, well
//!   under the wire's 4 MiB frame).
//! - **Addresses on the wire:** every element is named by its address
//!   ([`Pack::address_of`]; a composite map key is its element's address, `\x1f`, the field),
//!   as the view index and the presence setters by address name it.
//! - **Then one change frame per change** (`ops_change`): the change's ops by root, key and field,
//!   with their values, and who made it (the actor's id, name, kind and colour), its client,
//!   `turn`, `verb` and the actor's own `claim` on what it touched, as the kit has them. A pack
//!   adds its own `derived` values and `render_hashes` through [`Pack::watch_extras`] (none by
//!   default), reading the view index before and after the change (built through the kept
//!   index, [`Session::view_index`]: the carrier's derived items too); when the pack gives no
//!   render hashes, the kit's are the containers whose render hash changed between the two.
//! - **View deltas:** for a pack with a view schema, each change frame also says what the change
//!   did to the view (`units`, `containers_changed` and the render hint `containers`), worked
//!   out by the kit from the change's ops and the schema ([`crate::view_delta`]), so a watcher
//!   keeps the snapshot's view current without running pack code.
//! - **Numbered per watch:** every frame takes the next `seq` (the snapshot's head, each page,
//!   each change), so a client sees a lost frame as a gap and asks for a fresh snapshot.
//!   Presence, roster and claim frames are never numbered and never counted.
//! - **Refused verbs** (`verb_refused`, [`WatchRefusal`]): a pack's structured edit verb that a
//!   refusal stopped (another's claim, a stale version, a limit, the verb's own "no") is one
//!   unnumbered frame: who, the verb, the unit it addressed, the refusal's id and, when a claim
//!   or lease held by someone else caused it, the holder. It changes nothing, so a snapshot
//!   never holds it and a lost one is never asked for again. It goes only to the watches whose
//!   connection has the right to read the doc when it is refused (the host is asked then), as
//!   change frames do; an App viewer sees the others by handle.
//! - **Outside saves** (`outside_save`, [`WatchOutsideSave`]): when the kit handles an outside
//!   save of the doc's file, one unnumbered frame says what came of it: `ingested`, `kept_both`
//!   (with the conflict copy and the version holding what was live before), `declined` or
//!   `missing` (once, when the file goes or stops decoding). The save's own change still comes
//!   as its change frame, before it. The same audience as the refused verbs.
//! - **The snapshot is the fallback** for everything a watch can't carry: a client's gap, a
//!   subscriber cut off for falling behind, a change whose frame would pass the frame cap. A
//!   catch-up from a version (the changes since, under the cap) is not built: it would sit in
//!   [`Session::watch_snapshot`], before the snapshot it replaces, served from the log.
//! - **Gated:** a host that doesn't list [`KEYED_WATCH_FEATURE`] in a connection's
//!   `welcome.features` gets no watch on it ([`Session::watch_keyed`] gives `None`), so none of
//!   these frames ever reach it.
//!
//! The snapshot reads the doc through one function, [`Session::watch_state`]: the live view's
//! keyed state and the view index built from it ([`crate::view_index::ViewIndex::live`], as
//! [`Session::view_index`] builds it), at one version. The keyed state stays in the snapshot:
//! the change frames are ops on it, and a watcher replays them there.

use super::*;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::time::Duration;

use crate::elem::{
    Applied as ElemApplied, Effect, Key, KeyedRoot, KeyedState, ListOpKind, MapOpKind,
    Value as ElemValue,
};
use crate::errors::Value as Json;
use crate::session::pack::elem_of_map_key;
use crate::view_delta::{view_deltas_run, ViewDeltas};
use crate::view_index::{
    UnitsPage, ViewChange, ViewIndex, ViewSchema, ViewSnapshot, VIEW_PAGE_MIN,
};

/// The feature name a host lists in `welcome.features` when it serves the keyed watch: the
/// client's `watch` and `report` frames, and the `keyed_snapshot`, `keyed_page` and `ops_change`
/// frames.
pub const KEYED_WATCH_FEATURE: &str = "keyed_watch";

/// The wire's frame cap (`client/WIRE.md`, "Frame cap"): 4 MiB of JSON.
pub const WATCH_FRAME_MAX: usize = 4 * 1024 * 1024;

/// The default byte cap of one snapshot page: 1 MiB of JSON.
pub const WATCH_PAGE_BYTES: usize = 1024 * 1024;

/// The smallest page cap a host may set: smaller ones are raised to it.
pub const WATCH_PAGE_BYTES_MIN: usize = 1024;

/// How a host opens a keyed watch on a connection ([`Session::watch_keyed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchOptions {
    listed: bool,
    page_bytes: usize,
    frame_max: usize,
}

impl WatchOptions {
    /// From the feature names the host lists in this connection's `welcome.features`: the watch
    /// opens only when they hold [`KEYED_WATCH_FEATURE`].
    pub fn new<S: AsRef<str>>(features: &[S]) -> Self {
        Self {
            listed: features.iter().any(|f| f.as_ref() == KEYED_WATCH_FEATURE),
            page_bytes: WATCH_PAGE_BYTES,
            frame_max: WATCH_FRAME_MAX,
        }
    }

    /// The byte cap of one snapshot page (its JSON), at least [`WATCH_PAGE_BYTES_MIN`] and at
    /// most the frame cap.
    pub fn page_bytes(mut self, n: usize) -> Self {
        self.page_bytes = n;
        self
    }

    /// The host's frame cap ([`WATCH_FRAME_MAX`] by default). A change whose frame is over it
    /// isn't sent: the watch asks for a snapshot instead. A snapshot page over it (one entry
    /// that big) refuses the snapshot, `MPC-E401`.
    pub fn frame_max(mut self, n: usize) -> Self {
        self.frame_max = n;
        self
    }

    /// Whether the host listed the feature.
    pub fn listed(&self) -> bool {
        self.listed
    }

    fn caps(&self) -> (usize, usize) {
        let frame = self.frame_max.max(WATCH_PAGE_BYTES_MIN);
        (self.page_bytes.clamp(WATCH_PAGE_BYTES_MIN, frame), frame)
    }
}

/// Who made a change, on its frame, so a view words it without a roster lookup (even after
/// they left). The file's own saves are [`crate::session::EXTERNAL_PID`], kind `file` on the
/// wire, with no colour (the host's to choose).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchActor {
    /// The participant's pid (an App viewer gets a handle).
    pub id: String,
    pub name: String,
    pub kind: ParticipantKind,
    /// As the host stamped it on the participant; `None` for the file.
    pub colour: Option<String>,
}

/// What a pack adds to a change frame ([`Pack::watch_extras`]): its derived values (a slide's
/// title, an inherited box) and the render hashes of the containers whose hash changed. Opaque
/// to the kit, which only carries them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchExtras {
    /// JSON the pack builds (`derived` on the frame); `None`: `null`.
    pub derived: Option<Json>,
    /// `(container, hash)` pairs (`render_hashes` on the frame); none: `null`.
    pub render_hashes: Vec<(String, String)>,
}

impl WatchExtras {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn derived(mut self, v: Json) -> Self {
        self.derived = Some(v);
        self
    }

    /// A grid pack's derived values, typed ([`crate::grid::FrameDerived`]): `derived` becomes
    /// its JSON (`units.<row>.a1`, `containers.<sheet>.merges` and `.styles`, `written`,
    /// `bands`), or `null` when it has nothing.
    pub fn grid(mut self, d: &crate::grid::FrameDerived) -> Self {
        self.derived = d.to_value();
        self
    }

    pub fn render_hash(mut self, container: impl Into<String>, hash: impl Into<String>) -> Self {
        self.render_hashes.push((container.into(), hash.into()));
        self
    }
}

/// One op of a change, as a watcher replays it. Ops with no effect are left out (they change
/// nothing); a `drop` is carried (it takes a deleted element out of the state, though a view
/// shows nothing for it).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchOp {
    /// A text root's hunks, in the coordinates of its text before them (UTF-8 bytes).
    Text { root: String, hunks: Vec<Hunk> },
    /// A MovableList op on the element at address `key` ([`Pack::address_of`]): positions in
    /// the live order just before and just after it (`None`: not shown), the element's value
    /// before and after (`None`: none, before an insert), and whether it was hidden (written to
    /// a deleted element: nothing shows).
    List {
        root: String,
        op: ListOpKind,
        key: String,
        index_before: Option<usize>,
        index_after: Option<usize>,
        before: Option<ElemValue>,
        after: Option<ElemValue>,
        hidden: bool,
    },
    /// A Map op. On a root the pack declares composite, `key` is the element's address and
    /// `field` the rest of the map key (`<key>\x1f<field>`); otherwise `key` is the whole map
    /// key and `field` is `None`. `None` values: the key absent.
    Map {
        root: String,
        op: MapOpKind,
        key: String,
        field: Option<String>,
        before: Option<ElemValue>,
        after: Option<ElemValue>,
    },
}

/// One committed change, as a watcher gets it (`ops_change`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchChange {
    pub seq: u64,
    /// The doc's version after it (`ChangeEvent::version`).
    pub rev: Seq,
    pub at_ms: u64,
    pub kind: ChangeKind,
    pub actor: WatchActor,
    pub client: ClientId,
    pub via: String,
    pub on_behalf_of: Option<String>,
    pub turn: Option<Turn>,
    pub verb: Option<String>,
    /// The actor's own claim on what it touched (an element, or that element's container),
    /// `claim-N`; `None` when it held none.
    pub claim: Option<String>,
    /// Every root's ops, in apply order.
    pub ops: Vec<WatchOp>,
    /// What the change did to the view (`units` and `containers_changed` on the wire, and the
    /// render hint `containers`, [`ViewDeltas::hint`]), worked out by the kit from the ops and
    /// the pack's view schema ([`crate::view_delta`]); `None` when the pack has none (the
    /// frame then has none of the three fields).
    pub deltas: Option<ViewDeltas>,
    pub extras: WatchExtras,
    /// For a 3D slot: the doc's space after the change, when it changed (`view_space`).
    pub space_after: Option<crate::placement::Space>,
}

/// A verb refused on the watched doc (`verb_refused`): a pack's structured edit verb run through
/// [`Session::run_tool`] that applied nothing because it was refused, whatever refused it (the
/// session: another's claim, a stale `--if-version`, a size limit, rights; or the verb's own
/// "no", with its id). Unnumbered: it changes nothing, so no snapshot holds it, and a watch
/// that misses one (its queue full) doesn't ask for it again. Sent only to the watches whose
/// connection may read the doc at the time (the host's rights, asked when it is refused).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchRefusal {
    pub at_ms: u64,
    /// Who was refused (an App viewer gets a handle).
    pub actor: WatchActor,
    /// The refused connection's client.
    pub client: ClientId,
    /// The call's turn; `None` for an App viewer, as on a change frame.
    pub turn: Option<Turn>,
    /// The verb's name (`shape move`).
    pub verb: String,
    /// The unit the verb addressed, by address ([`Pack::address_of`]): the one the verb names
    /// ([`crate::tools::StructOutput::addressing`]), else, for a claim, the first element it
    /// touched under the claim, else the first element its edits touched. `None` when it was
    /// refused before it named one (its arguments, rights, a stale version).
    pub unit: Option<String>,
    /// The refusal's id (`MPC-E301`): [`crate::errors::ErrorReport::id`] of what the call
    /// returned.
    pub code: &'static str,
    /// Who holds the claim (or lease) that refused it, when that refused it; `None` otherwise.
    pub holder: Option<WatchActor>,
    /// The holder's claim, `claim-N`, when a claim refused it.
    pub claim: Option<String>,
}

/// What the kit made of an outside save of the watched doc's file (`outside_save`): unnumbered,
/// one per save it took in, and one when the file goes or stops decoding. Sent to the watches
/// whose connection may read the doc then, as [`WatchRefusal`]. The save's change, when it made
/// one, came first as its own change frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchOutsideSave {
    /// When the kit handled it (host clock).
    pub at_ms: u64,
    /// The doc's address: its [`DocKey::path`].
    pub doc: String,
    pub outcome: OutsideSaveOutcome,
}

/// How an outside save went ([`WatchOutsideSave::outcome`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutsideSaveOutcome {
    /// Taken in by key: the doc is at `rev` (`DiskCheck::Ingested`).
    Ingested { rev: Seq },
    /// Kept both (`DiskCheck::Conflict`): the save is the doc at `rev`, and what was live before
    /// is the stored version `kept` and the conflict copy (`None` when it couldn't be written).
    KeptBoth {
        rev: Seq,
        kept: crate::store::VersionId,
        copy: Option<OutsideSaveCopy>,
    },
    /// The file can't be a live doc any more (`DiskCheck::Declined`): the pack's `code` and its
    /// `message`. Once, when it starts.
    Declined { code: &'static str, message: String },
    /// The file is gone (`DiskCheck::Missing`). Once, when it goes.
    Missing,
}

impl OutsideSaveOutcome {
    /// `ingested`, `kept_both`, `declined` or `missing`, as the wire carries it.
    pub fn as_str(&self) -> &'static str {
        match self {
            OutsideSaveOutcome::Ingested { .. } => "ingested",
            OutsideSaveOutcome::KeptBoth { .. } => "kept_both",
            OutsideSaveOutcome::Declined { .. } => "declined",
            OutsideSaveOutcome::Missing => "missing",
        }
    }
}

/// A kept-both save's conflict copy ([`OutsideSaveOutcome::KeptBoth`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OutsideSaveCopy {
    /// Its file name.
    pub name: String,
    /// Its path relative to the doc's file's folder, as the kit records it.
    pub path: String,
    /// Its doc address ([`DocKey::path`]) when the copy is itself a kit document: the host's
    /// [`Host::document_at`] for its file. `None` otherwise.
    pub doc: Option<String>,
}

/// The snapshot's head (`keyed_snapshot`): the schema, the view index's snapshot, and how many
/// pages follow.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchSnapshot {
    pub seq: u64,
    /// The doc's version the snapshot holds.
    pub upto: Seq,
    pub schema: Schema,
    /// The pack's composite maps ([`Pack::composite`]), as `(map root, list root)`: where a
    /// list's elements keep their fields (`composite` in the head's `schema`).
    pub composite: Vec<(String, String)>,
    /// The pack's view schema and the view index's snapshot at `upto` (everything but the
    /// units, which follow in pages); `None` when the pack has no view schema.
    pub view: Option<(ViewSchema, ViewSnapshot)>,
    pub pages: u32,
}

/// One page's run of one root's content.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchPart {
    /// Live elements by address, in the live order, after those of the root's earlier pages.
    Live(Vec<(String, ElemValue)>),
    /// Deleted elements by address (their values).
    Hidden(Vec<(String, ElemValue)>),
    /// Map entries: key, field (as [`WatchOp::Map`] splits them), value.
    Entries(Vec<(String, Option<String>, ElemValue)>),
    /// A run of a text root, appended to the earlier pages' runs.
    Text(String),
    /// One page of a container's units of the view index
    /// ([`crate::view_index::ViewIndex::units_page`]); the page's root is the units root.
    Units(UnitsPage),
}

/// One page of a snapshot (`keyed_page`), numbered from 1.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WatchPage {
    pub seq: u64,
    pub upto: Seq,
    pub page: u32,
    pub pages: u32,
    pub root: String,
    pub part: WatchPart,
    /// On a page of the units' fields: the pack's derived values of the fields in this page
    /// ([`Pack::page_derived`]), `<name> -> <unit address> -> <field> -> value`; empty: none.
    pub derived: PageExtra,
}

/// A frame of the keyed watch, as the host sends it ([`WatchFrame::to_json`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchFrame {
    Snapshot(Box<WatchSnapshot>),
    Page(WatchPage),
    Change(Box<WatchChange>),
    /// A refused verb: unnumbered ([`WatchRefusal`]).
    Refused(Box<WatchRefusal>),
    /// What came of an outside save: unnumbered ([`WatchOutsideSave`]).
    OutsideSave(Box<WatchOutsideSave>),
}

impl WatchFrame {
    /// Its number in the watch; 0 for an unnumbered frame ([`WatchFrame::Refused`]).
    pub fn seq(&self) -> u64 {
        match self {
            WatchFrame::Snapshot(s) => s.seq,
            WatchFrame::Page(p) => p.seq,
            WatchFrame::Change(c) => c.seq,
            WatchFrame::Refused(_) | WatchFrame::OutsideSave(_) => 0,
        }
    }

    /// Whether it takes a number in the watch (every frame but a refusal and an outside save's).
    pub fn numbered(&self) -> bool {
        !matches!(self, WatchFrame::Refused(_) | WatchFrame::OutsideSave(_))
    }

    /// The frame as the wire carries it (`client/WIRE.md`, "The keyed watch").
    pub fn to_value(&self) -> Json {
        match self {
            WatchFrame::Snapshot(s) => Json::obj([
                ("type", Json::from("keyed_snapshot")),
                ("seq", Json::from(s.seq)),
                ("upto", Json::from(s.upto)),
                ("schema", schema_json(&s.schema, &s.composite)),
                (
                    "view",
                    s.view.as_ref().map_or(Json::Null, |(vs, v)| v.to_value(vs)),
                ),
                ("pages", Json::from(s.pages)),
            ]),
            WatchFrame::Page(p) => {
                let mut v = page_head(p.seq, p.upto, p.page, p.pages, &p.root);
                let (name, body) = match &p.part {
                    WatchPart::Live(l) => ("live", Json::List(l.iter().map(str_pair).collect())),
                    WatchPart::Hidden(h) => {
                        ("hidden", Json::List(h.iter().map(str_pair).collect()))
                    }
                    WatchPart::Entries(e) => (
                        "entries",
                        Json::List(
                            e.iter()
                                .map(|(k, f, v)| entry_triple(k, f.as_deref(), v))
                                .collect(),
                        ),
                    ),
                    WatchPart::Text(t) => ("text", Json::from(t.as_str())),
                    WatchPart::Units(u) => ("units", u.to_value()),
                };
                if let Json::Obj(m) = &mut v {
                    m.push((name.to_owned(), body));
                    if !p.derived.is_empty() {
                        m.push(("derived".to_owned(), page_derived_json(&p.derived)));
                    }
                }
                v
            }
            WatchFrame::Change(c) => change_json(c),
            WatchFrame::Refused(r) => refusal_json(r),
            WatchFrame::OutsideSave(o) => outside_save_json(o),
        }
    }

    /// [`WatchFrame::to_value`] as compact JSON.
    pub fn to_json(&self) -> String {
        self.to_value().to_json()
    }
}

/// A doc's keyed state as canonical JSON: every root in schema order, a list's live elements in
/// order and its deleted ones by key, a map's entries by key (UTF-8 byte order), a text whole;
/// no places. Two states are the same when this is the same, byte for byte: what a watcher's
/// state is compared by (the client's `keyedStateJson`).
pub fn keyed_state_json(state: &KeyedState) -> String {
    let roots = state
        .roots
        .iter()
        .map(|(name, r)| {
            let mut v = vec![("name".to_owned(), Json::from(name.as_str()))];
            match r {
                KeyedRoot::Text(t) => {
                    v.push(("kind".into(), Json::from("text")));
                    v.push(("text".into(), Json::from(t.as_str())));
                }
                KeyedRoot::List { live, hidden, .. } => {
                    v.push(("kind".into(), Json::from("list")));
                    v.push((
                        "live".into(),
                        Json::List(live.iter().map(elem_pair).collect()),
                    ));
                    v.push((
                        "hidden".into(),
                        Json::List(hidden.iter().map(|(k, x)| elem_pair_ref(k, x)).collect()),
                    ));
                }
                KeyedRoot::Map(m) => {
                    v.push(("kind".into(), Json::from("map")));
                    v.push((
                        "entries".into(),
                        Json::List(
                            m.iter()
                                .map(|(k, x)| Json::List(vec![Json::from(k.as_str()), value(x)]))
                                .collect(),
                        ),
                    ));
                }
            }
            Json::Obj(v)
        })
        .collect();
    Json::obj([("roots", Json::List(roots))]).to_json()
}

/// A doc's keyed state as a watcher holds it, as canonical JSON: [`keyed_state_json`]'s form
/// with every element named as the frames name it, by its address ([`Pack::address_of`]; a
/// composite map key is its element's address, `\x1f`, the field), deleted elements and map
/// entries in their addresses' UTF-8 byte order. What a watcher's state is compared with (the
/// client's `keyedStateJson`). For a pack whose addresses are its keys (the default), the same
/// as [`keyed_state_json`].
pub fn watch_state_json<P: Pack + ?Sized>(pack: &P, state: &KeyedState) -> String {
    let composites = pack.composite();
    let pairs = |v: Vec<(String, &ElemValue)>| -> Json {
        Json::List(v.into_iter().map(|(k, x)| str_pair_ref(&k, x)).collect())
    };
    let roots = state
        .roots
        .iter()
        .map(|(name, r)| {
            let mut v = vec![("name".to_owned(), Json::from(name.as_str()))];
            match r {
                KeyedRoot::Text(t) => {
                    v.push(("kind".into(), Json::from("text")));
                    v.push(("text".into(), Json::from(t.as_str())));
                }
                KeyedRoot::List { live, hidden, .. } => {
                    v.push(("kind".into(), Json::from("list")));
                    let live = live.iter().map(|(k, x)| (pack.address_of(name, k), x));
                    v.push(("live".into(), pairs(live.collect())));
                    let mut h: Vec<(String, &ElemValue)> = hidden
                        .iter()
                        .map(|(k, x)| (pack.address_of(name, k), x))
                        .collect();
                    h.sort_by(|a, b| a.0.cmp(&b.0));
                    v.push(("hidden".into(), pairs(h)));
                }
                KeyedRoot::Map(m) => {
                    v.push(("kind".into(), Json::from("map")));
                    let mut e: Vec<(String, &ElemValue)> = m
                        .iter()
                        .map(|(k, x)| {
                            let (key, field) = split_map_key(pack, composites, name, k);
                            let k = match field {
                                Some(f) => format!("{key}{}{f}", crate::table::CELL_SEP),
                                None => key,
                            };
                            (k, x)
                        })
                        .collect();
                    e.sort_by(|a, b| a.0.cmp(&b.0));
                    v.push(("entries".into(), pairs(e)));
                }
            }
            Json::Obj(v)
        })
        .collect();
    Json::obj([("roots", Json::List(roots))]).to_json()
}

// ---------- the encodings ----------

fn value(v: &ElemValue) -> Json {
    match v {
        ElemValue::Str(s) => Json::from(s.as_str()),
        ElemValue::Null => Json::Null,
    }
}

fn elem_pair((k, v): &(Key, ElemValue)) -> Json {
    elem_pair_ref(k, v)
}

fn elem_pair_ref(k: &Key, v: &ElemValue) -> Json {
    Json::List(vec![Json::from(k.as_str()), value(v)])
}

fn str_pair((k, v): &(String, ElemValue)) -> Json {
    str_pair_ref(k, v)
}

fn str_pair_ref(k: &str, v: &ElemValue) -> Json {
    Json::List(vec![Json::from(k), value(v)])
}

fn entry_triple(k: &str, f: Option<&str>, v: &ElemValue) -> Json {
    Json::List(vec![
        Json::from(k),
        f.map_or(Json::Null, Json::from),
        value(v),
    ])
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Text => "text",
        Kind::MovableList => "list",
        Kind::Map => "map",
        Kind::RichText => "rich_text",
        Kind::List => "plain_list",
        Kind::Tree => "tree",
    }
}

fn schema_json(s: &Schema, composite: &[(String, String)]) -> Json {
    Json::obj([
        ("name", Json::from(s.name)),
        (
            "roots",
            Json::List(
                s.roots
                    .iter()
                    .map(|(n, k)| {
                        Json::obj([
                            ("name", Json::from(*n)),
                            ("kind", Json::from(kind_name(*k))),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "composite",
            Json::List(
                composite
                    .iter()
                    .map(|(m, l)| {
                        Json::obj([
                            ("map", Json::from(m.as_str())),
                            ("list", Json::from(l.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// A page's derived values as `{<name>: {<unit>: {<field>: value}}}`, in the order given.
fn page_derived_json(d: &PageExtra) -> Json {
    // per name, per unit, the unit's fields
    type Units = Vec<(String, Vec<(String, Json)>)>;
    let mut names: Vec<(String, Units)> = Vec::new();
    for (name, unit, field, value) in d {
        let n = match names.iter().position(|(x, _)| x == name) {
            Some(i) => i,
            None => {
                names.push(((*name).to_owned(), Vec::new()));
                names.len() - 1
            }
        };
        let units = &mut names[n].1;
        let u = match units.iter().position(|(x, _)| x == unit) {
            Some(i) => i,
            None => {
                units.push((unit.clone(), Vec::new()));
                units.len() - 1
            }
        };
        units[u].1.push((field.clone(), Json::from(value.as_str())));
    }
    Json::Obj(
        names
            .into_iter()
            .map(|(n, us)| {
                (
                    n,
                    Json::Obj(us.into_iter().map(|(u, fs)| (u, Json::Obj(fs))).collect()),
                )
            })
            .collect(),
    )
}

fn page_head(seq: u64, upto: Seq, page: u32, pages: u32, root: &str) -> Json {
    Json::obj([
        ("type", Json::from("keyed_page")),
        ("seq", Json::from(seq)),
        ("upto", Json::from(upto)),
        ("page", Json::from(page)),
        ("pages", Json::from(pages)),
        ("root", Json::from(root)),
    ])
}

/// The actor's kind on the wire: `ParticipantKind` in lower case, but `file` for the file's own
/// saves (the rendering contract's word; the roster says `external`).
fn actor_kind(k: ParticipantKind) -> &'static str {
    match k {
        ParticipantKind::Person => "person",
        ParticipantKind::Agent => "agent",
        ParticipantKind::App => "app",
        ParticipantKind::External => "file",
        ParticipantKind::Restore => "restore",
        ParticipantKind::Import => "import",
    }
}

fn list_op_name(k: ListOpKind) -> &'static str {
    match k {
        ListOpKind::Insert => "insert",
        ListOpKind::Move => "move",
        ListOpKind::Delete => "delete",
        ListOpKind::Restore => "restore",
        ListOpKind::Set => "set",
        ListOpKind::Drop => "drop",
    }
}

fn opt_index(i: Option<usize>) -> Json {
    i.map_or(Json::Null, Json::from)
}

fn op_json(o: &WatchOp) -> Json {
    let mut v: Vec<(String, Json)> = Vec::new();
    let vals = |v: &mut Vec<(String, Json)>, b: &Option<ElemValue>, a: &Option<ElemValue>| {
        if let Some(b) = b {
            v.push(("before".into(), value(b)));
        }
        if let Some(a) = a {
            v.push(("after".into(), value(a)));
        }
    };
    match o {
        WatchOp::Text { root, hunks } => {
            v.push(("root".into(), Json::from(root.as_str())));
            v.push(("kind".into(), Json::from("text")));
            v.push((
                "hunks".into(),
                Json::List(
                    hunks
                        .iter()
                        .map(|h| {
                            Json::obj([
                                ("start", Json::from(h.start)),
                                ("end", Json::from(h.end)),
                                ("insert", Json::from(h.insert.as_str())),
                            ])
                        })
                        .collect(),
                ),
            ));
        }
        WatchOp::List {
            root,
            op,
            key,
            index_before,
            index_after,
            before,
            after,
            hidden,
        } => {
            v.push(("root".into(), Json::from(root.as_str())));
            v.push(("kind".into(), Json::from("list")));
            v.push(("op".into(), Json::from(list_op_name(*op))));
            v.push(("key".into(), Json::from(key.as_str())));
            v.push(("index_before".into(), opt_index(*index_before)));
            v.push(("index_after".into(), opt_index(*index_after)));
            vals(&mut v, before, after);
            v.push((
                "effect".into(),
                Json::from(if *hidden { "hidden" } else { "visible" }),
            ));
        }
        WatchOp::Map {
            root,
            op,
            key,
            field,
            before,
            after,
        } => {
            v.push(("root".into(), Json::from(root.as_str())));
            v.push(("kind".into(), Json::from("map")));
            v.push((
                "op".into(),
                Json::from(match op {
                    MapOpKind::Set => "set",
                    MapOpKind::Delete => "delete",
                }),
            ));
            v.push(("key".into(), Json::from(key.as_str())));
            v.push((
                "field".into(),
                field.as_deref().map_or(Json::Null, Json::from),
            ));
            vals(&mut v, before, after);
        }
    }
    Json::Obj(v)
}

fn actor_json(a: &WatchActor) -> Json {
    Json::obj([
        ("id", Json::from(a.id.as_str())),
        ("name", Json::from(a.name.as_str())),
        ("kind", Json::from(actor_kind(a.kind))),
        ("colour", a.colour.as_deref().map_or(Json::Null, Json::from)),
    ])
}

fn refusal_json(r: &WatchRefusal) -> Json {
    let opt = |s: &Option<String>| s.as_deref().map_or(Json::Null, Json::from);
    Json::obj([
        ("type", Json::from("verb_refused")),
        ("at_ms", Json::from(r.at_ms)),
        ("actor", actor_json(&r.actor)),
        ("client", Json::from(r.client)),
        (
            "turn",
            r.turn
                .as_ref()
                .map_or(Json::Null, |t| Json::from(t.as_str())),
        ),
        ("verb", Json::from(r.verb.as_str())),
        ("unit", opt(&r.unit)),
        ("code", Json::from(r.code)),
        ("holder", r.holder.as_ref().map_or(Json::Null, actor_json)),
        ("claim", opt(&r.claim)),
    ])
}

fn outside_save_json(o: &WatchOutsideSave) -> Json {
    let mut v = vec![
        ("type".to_owned(), Json::from("outside_save")),
        ("at_ms".to_owned(), Json::from(o.at_ms)),
        ("doc".to_owned(), Json::from(o.doc.as_str())),
        ("outcome".to_owned(), Json::from(o.outcome.as_str())),
    ];
    match &o.outcome {
        OutsideSaveOutcome::Ingested { rev } => v.push(("rev".into(), Json::from(*rev))),
        OutsideSaveOutcome::KeptBoth { rev, kept, copy } => {
            v.push(("rev".into(), Json::from(*rev)));
            v.push(("kept".into(), Json::from(*kept)));
            let copy = copy.as_ref().map_or(Json::Null, |c| {
                let mut f = vec![
                    ("name".to_owned(), Json::from(c.name.as_str())),
                    ("path".to_owned(), Json::from(c.path.as_str())),
                ];
                if let Some(d) = &c.doc {
                    f.push(("doc".into(), Json::from(d.as_str())));
                }
                Json::Obj(f)
            });
            v.push(("copy".into(), copy));
        }
        OutsideSaveOutcome::Declined { code, message } => {
            v.push(("code".into(), Json::from(*code)));
            v.push(("message".into(), Json::from(message.as_str())));
        }
        OutsideSaveOutcome::Missing => {}
    }
    Json::Obj(v)
}

fn change_json(c: &WatchChange) -> Json {
    let opt = |s: &Option<String>| s.as_deref().map_or(Json::Null, Json::from);
    let mut v = Json::obj([
        ("type", Json::from("ops_change")),
        ("seq", Json::from(c.seq)),
        ("rev", Json::from(c.rev)),
        ("at_ms", Json::from(c.at_ms)),
        ("change_kind", Json::from(c.kind.as_str())),
        ("actor", actor_json(&c.actor)),
        ("client", Json::from(c.client)),
        ("via", Json::from(c.via.as_str())),
        ("on_behalf_of", opt(&c.on_behalf_of)),
        (
            "turn",
            c.turn
                .as_ref()
                .map_or(Json::Null, |t| Json::from(t.as_str())),
        ),
        ("verb", opt(&c.verb)),
        ("claim", opt(&c.claim)),
        ("ops", Json::List(c.ops.iter().map(op_json).collect())),
        ("derived", c.extras.derived.clone().unwrap_or(Json::Null)),
        (
            "render_hashes",
            match c.extras.render_hashes.is_empty() {
                true => Json::Null,
                false => Json::Obj(
                    c.extras
                        .render_hashes
                        .iter()
                        .map(|(k, h)| (k.clone(), Json::from(h.as_str())))
                        .collect(),
                ),
            },
        ),
    ]);
    if let (Some(sp), Json::Obj(m)) = (&c.space_after, &mut v) {
        m.push(("view_space".to_owned(), sp.to_value()));
    }
    // the view deltas, right after the ops, only for a pack with a view schema
    if let (Some(d), Json::Obj(m)) = (&c.deltas, &mut v) {
        let at = m
            .iter()
            .position(|(k, _)| k == "ops")
            .map_or(m.len(), |i| i + 1);
        let hint = d.hint(c.extras.render_hashes.iter().map(|(k, _)| k.as_str()));
        let fields = [
            (
                "units",
                Json::List(d.units.iter().map(|u| u.to_value()).collect()),
            ),
            (
                "containers_changed",
                Json::List(d.containers.iter().map(|x| x.to_value()).collect()),
            ),
            ("containers", Json::list(hint)),
        ];
        for (i, (k, x)) in fields.into_iter().enumerate() {
            m.insert(at + i, (k.to_owned(), x));
        }
    }
    v
}

// ---------- the snapshot's pages ----------

/// The JSON length of `v` as an element of a list: its own, plus the comma before it.
fn item_len(v: &Json) -> usize {
    v.to_json().len() + 1
}

/// Splits `text` into runs whose JSON string (quotes and escapes included) is at most `room`
/// bytes, at character boundaries. A character always fits (`room` is far above 8).
fn text_runs(text: &str, room: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut len = 2; // the quotes
    for ch in text.chars() {
        let mut b = [0u8; 4];
        let n = Json::from(&*ch.encode_utf8(&mut b)).to_json().len() - 2;
        if len + n > room && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            len = 2;
        }
        cur.push(ch);
        len += n;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One root's entries, each with its JSON length as a list item, packed in order into runs
/// whose page is at most `page_bytes` (`base`: the page's own bytes); an entry too big for a page
/// has one of its own. `Err(n)`: an entry of `n` bytes whose page would pass `frame_max`.
fn runs<T>(
    items: Vec<(T, usize)>,
    base: usize,
    page_bytes: usize,
    frame_max: usize,
) -> std::result::Result<Vec<Vec<T>>, usize> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut len = base;
    for (e, n) in items {
        if base + n > frame_max {
            return Err(n);
        }
        if len + n > page_bytes && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            len = base;
        }
        cur.push(e);
        len += n;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// The snapshot of `state` at `upto` as frames numbered from `seq0`: the head, then the roots'
/// content in schema order, packed into pages of at most `page_bytes` of JSON. One entry over
/// that has a page of its own; one over `frame_max` refuses the snapshot.
#[allow(clippy::too_many_arguments)]
fn snapshot_frames(
    pack: &dyn Pack,
    schema: Schema,
    upto: Seq,
    state: &KeyedState,
    view: Option<&(ViewSchema, ViewIndex)>,
    seq0: u64,
    page_bytes: usize,
    frame_max: usize,
) -> Result<Vec<WatchFrame>> {
    let composites = pack.composite();
    // a page's own fields at their widest (numbers at u64::MAX), so every page is under the cap
    let overhead = |root: &str, part: &str| {
        let mut v = page_head(u64::MAX, u64::MAX, u32::MAX, u32::MAX, root);
        if let Json::Obj(m) = &mut v {
            m.push((part.to_owned(), Json::List(Vec::new())));
        }
        v.to_json().len()
    };
    let mut parts: Vec<Part> = Vec::new();
    // the units' fields map: the pack's per-unit derived values ride on its pages
    let units_map = view.and_then(|(vs, _)| {
        composites
            .iter()
            .find(|(_, l)| *l == vs.units)
            .map(|(m, _)| (*m, vs.units))
    });
    let mut unit_derived: UnitDerived = HashMap::new();
    if let Some((map, list)) = units_map {
        if let Some(KeyedRoot::Map(m)) = state.roots.iter().find(|(r, _)| r == map).map(|(_, r)| r)
        {
            let mut units: Vec<Key> = Vec::new();
            for k in m.keys() {
                if let Some((l, e)) = elem_of_map_key(composites, map, k) {
                    if l == list && units.last() != Some(&e) && !units.contains(&e) {
                        units.push(e);
                    }
                }
            }
            if !units.is_empty() {
                for d in pack.page_derived(state, &units) {
                    for (field, value) in d.values {
                        unit_derived
                            .entry((d.unit.clone(), field))
                            .or_default()
                            .push((d.name, value));
                    }
                }
            }
        }
    }
    let too_large = |root: &str, bytes: usize| {
        SessionError::Declined(Declined {
            code: "too_large",
            message: format!(
                "one entry of root {root} is {bytes} bytes of JSON, over the frame cap of \
                 {frame_max}: the doc can't be watched"
            ),
        })
    };
    for (root, r) in &state.roots {
        match r {
            KeyedRoot::Text(t) => {
                if t.is_empty() {
                    continue;
                }
                let room = page_bytes.saturating_sub(overhead(root, "text")).max(64);
                for run in text_runs(t, room) {
                    parts.push((root.clone(), WatchPart::Text(run), Vec::new()));
                }
            }
            KeyedRoot::List { live, hidden, .. } => {
                let items = live
                    .iter()
                    .map(|(k, v)| {
                        let e = (pack.address_of(root, k), v.clone());
                        let n = item_len(&str_pair(&e));
                        (e, n)
                    })
                    .collect();
                let base = overhead(root, "live");
                for run in
                    runs(items, base, page_bytes, frame_max).map_err(|n| too_large(root, n))?
                {
                    parts.push((root.clone(), WatchPart::Live(run), Vec::new()));
                }
                let items = hidden
                    .iter()
                    .map(|(k, v)| {
                        let e = (pack.address_of(root, k), v.clone());
                        let n = item_len(&str_pair(&e));
                        (e, n)
                    })
                    .collect();
                let base = overhead(root, "hidden");
                for run in
                    runs(items, base, page_bytes, frame_max).map_err(|n| too_large(root, n))?
                {
                    parts.push((root.clone(), WatchPart::Hidden(run), Vec::new()));
                }
            }
            KeyedRoot::Map(m) => {
                let ours = units_map.is_some_and(|(map, _)| map == root);
                let items = m
                    .iter()
                    .map(|(k, v)| {
                        let (key, field) = split_map_key(pack, composites, root, k);
                        let mut n = item_len(&entry_triple(&key, field.as_deref(), v));
                        // the entry's derived values, counted with it (their names, the unit's
                        // address and the field again, at most)
                        let mut extra: PageExtra = Vec::new();
                        if ours {
                            if let (Some((_, e)), Some(f)) =
                                (elem_of_map_key(composites, root, k), field.as_ref())
                            {
                                for (name, value) in
                                    unit_derived.get(&(e, f.clone())).into_iter().flatten()
                                {
                                    n += Json::obj([(
                                        *name,
                                        Json::obj([(
                                            key.as_str(),
                                            Json::obj([(f.as_str(), Json::from(value.as_str()))]),
                                        )]),
                                    )])
                                    .to_json()
                                    .len()
                                        + 1;
                                    extra.push((*name, key.clone(), f.clone(), value.clone()));
                                }
                            }
                        }
                        (((key, field, v.clone()), extra), n)
                    })
                    .collect();
                // the derived field's own name, once a page
                let base = overhead(root, "entries") + if ours { 12 } else { 0 };
                for run in
                    runs(items, base, page_bytes, frame_max).map_err(|n| too_large(root, n))?
                {
                    let (entries, extras): (Vec<_>, Vec<PageExtra>) = run.into_iter().unzip();
                    parts.push((
                        root.clone(),
                        WatchPart::Entries(entries),
                        extras.into_iter().flatten().collect(),
                    ));
                }
            }
        }
    }
    // the view index's units, container by container, in its own pages (at least its minimum
    // budget)
    if let Some((vs, ix)) = view {
        let head = {
            let mut v = page_head(u64::MAX, u64::MAX, u32::MAX, u32::MAX, vs.units);
            if let Json::Obj(m) = &mut v {
                m.push(("units".to_owned(), Json::Obj(Vec::new())));
            }
            v.to_json().len() - 2
        };
        let budget = page_bytes.saturating_sub(head).max(VIEW_PAGE_MIN);
        for c in &ix.containers {
            let mut from = 0;
            loop {
                let page = ix
                    .units_page(&c.key, from, budget)
                    .map_err(|e| SessionError::Pack(PackError(e.to_string())))?;
                if page.total == 0 {
                    break;
                }
                let n = head + page.to_value().to_json().len();
                if n > frame_max {
                    return Err(too_large(vs.units, n));
                }
                let next = page.next;
                parts.push((vs.units.to_owned(), WatchPart::Units(page), Vec::new()));
                match next {
                    Some(f) => from = f,
                    None => break,
                }
            }
        }
    }
    let pages = u32::try_from(parts.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(parts.len() + 1);
    let head = WatchFrame::Snapshot(Box::new(WatchSnapshot {
        seq: seq0,
        upto,
        schema,
        composite: composites
            .iter()
            .map(|(m, l)| ((*m).to_owned(), (*l).to_owned()))
            .collect(),
        view: view.map(|(vs, ix)| (*vs, ix.snapshot())),
        pages,
    }));
    // the head holds the view index's snapshot whole (its containers): under the frame cap
    let n = head.to_json().len();
    if n > frame_max {
        return Err(SessionError::Declined(Declined {
            code: "too_large",
            message: format!(
                "the snapshot's head is {n} bytes of JSON, over the frame cap of {frame_max}: \
                 the doc can't be watched"
            ),
        }));
    }
    out.push(head);
    for (n, (root, part, derived)) in parts.into_iter().enumerate() {
        out.push(WatchFrame::Page(WatchPage {
            seq: seq0 + 1 + n as u64,
            upto,
            page: n as u32 + 1,
            pages,
            root,
            part,
            derived,
        }));
    }
    Ok(out)
}

/// A snapshot page's derived values ([`WatchPage::derived`]): `(name, unit address, field,
/// value)`.
pub type PageExtra = Vec<(&'static str, String, String, String)>;

/// One page of a snapshot before it is numbered: its root, its content, its derived values.
type Part = (String, WatchPart, PageExtra);

/// The pack's per-unit derived values by `(unit, field)`: `(name, value)` each.
type UnitDerived = HashMap<(Key, String), Vec<(&'static str, String)>>;

/// A map key as a watcher gets it: `(the element's address, Some(field))` on a composite root
/// when the key names an element, else `(the whole key, None)`.
fn split_map_key<P: Pack + ?Sized>(
    pack: &P,
    composites: &[(&'static str, &'static str)],
    root: &str,
    key: &str,
) -> (String, Option<String>) {
    match elem_of_map_key(composites, root, key) {
        Some((list, elem)) => {
            let field = key[elem.as_str().len() + crate::table::CELL_SEP.len_utf8()..].to_owned();
            (pack.address_of(list, &elem), Some(field))
        }
        None => (key.to_owned(), None),
    }
}

/// A change's ops as a watcher replays them: every root's, in order; ops with no effect left
/// out.
fn ops_of(pack: &dyn Pack, a: &ElemApplied, out: &mut Vec<WatchOp>) {
    let composites = pack.composite();
    match a {
        ElemApplied::Text { root, hunks } => {
            if !hunks.is_empty() {
                out.push(WatchOp::Text {
                    root: root.clone(),
                    hunks: hunks.clone(),
                });
            }
        }
        ElemApplied::List { root, ops } => {
            for o in ops {
                let hidden = match o.effect {
                    Effect::None { .. } => continue,
                    Effect::Visible => false,
                    Effect::Hidden => true,
                };
                out.push(WatchOp::List {
                    root: root.clone(),
                    op: o.kind,
                    key: pack.address_of(root, &o.key),
                    index_before: o.index_before,
                    index_after: o.index_after,
                    before: o.value_before.clone(),
                    after: o.value_after.clone(),
                    hidden,
                });
            }
        }
        ElemApplied::Map { root, ops } => {
            for o in ops {
                if matches!(o.effect, Effect::None { .. }) {
                    continue;
                }
                let (key, field) = split_map_key(pack, composites, root, &o.key);
                out.push(WatchOp::Map {
                    root: root.clone(),
                    op: o.kind,
                    key,
                    field,
                    before: o.before.clone(),
                    after: o.after.clone(),
                });
            }
        }
    }
}

// ---------- the session's side ----------

/// A refused verb, as [`Session::watch_refused`] takes it from the call that refused it.
pub(crate) struct VerbRefused {
    pub(crate) conn: ConnId,
    pub(crate) verb: String,
    pub(crate) turn: Option<Turn>,
    pub(crate) code: &'static str,
    /// The unit the verb named ([`crate::tools::StructOutput::addressing`]).
    pub(crate) unit: Option<String>,
    /// The elements its edits touched, in order, when it made some ([`VerbRefused::touched_of`]).
    pub(crate) touched: Vec<(String, Key)>,
    /// The pid, name and claim (none for a lease) of who holds what refused it.
    pub(crate) holder: Option<(String, String, Option<awareness::ClaimId>)>,
    /// The refusing claim's list root and keys, for a claim on elements.
    pub(crate) claimed: Option<(String, Vec<Key>)>,
}

/// At most this many touched elements are kept for a refusal's unit.
const REFUSAL_TOUCHED_MAX: usize = 64;

impl VerbRefused {
    pub(crate) fn new(conn: ConnId, verb: &str, turn: Option<Turn>, code: &'static str) -> Self {
        VerbRefused {
            conn,
            verb: verb.to_owned(),
            turn,
            code,
            unit: None,
            touched: Vec::new(),
            holder: None,
            claimed: None,
        }
    }

    /// Who holds what `e` refused: another's claim (on elements: its root and keys) or lease.
    pub(crate) fn held_by(mut self, e: &SessionError) -> Self {
        match e {
            SessionError::Claimed { id, by, name, .. } => {
                self.holder = Some((by.clone(), name.clone(), Some(*id)));
            }
            SessionError::ElemsClaimed(c) => {
                self.holder = Some((c.by.clone(), c.name.clone(), Some(c.id)));
                self.claimed = Some((c.root.clone(), c.keys.clone()));
            }
            SessionError::NodesClaimed(c) => {
                self.holder = Some((c.by.clone(), c.name.clone(), Some(c.id)));
            }
            SessionError::Leased { holder, name, .. } => {
                self.holder = Some((holder.clone(), name.clone(), None));
            }
            _ => {}
        }
        self
    }

    /// The elements `edits` touch, in order, by list root and key (a composite map key as its
    /// element), at most [`REFUSAL_TOUCHED_MAX`].
    pub(crate) fn touched_of(pack: &dyn Pack, edits: &[Edit]) -> Vec<(String, Key)> {
        let composites = pack.composite();
        let mut out = Vec::new();
        for e in edits {
            match e {
                Edit::List { root, ops } => {
                    let room = REFUSAL_TOUCHED_MAX - out.len();
                    out.extend(
                        ops.iter()
                            .take(room)
                            .map(|o| (root.clone(), o.key().clone())),
                    );
                }
                Edit::Map { root, ops } => {
                    let room = REFUSAL_TOUCHED_MAX - out.len();
                    out.extend(
                        ops.iter()
                            .filter_map(|o| elem_of_map_key(composites, root, o.key()))
                            .take(room)
                            .map(|(l, k)| (l.to_owned(), k)),
                    );
                }
                _ => {}
            }
            if out.len() >= REFUSAL_TOUCHED_MAX {
                break;
            }
        }
        out
    }

    /// The unit it names, by address: the verb's own, else the first touched element under the
    /// refusing claim, else the first touched element.
    fn unit_address(&self, pack: &dyn Pack) -> Option<String> {
        if let Some(u) = &self.unit {
            return Some(u.clone());
        }
        let under = self.claimed.as_ref().and_then(|(root, keys)| {
            self.touched
                .iter()
                .find(|(r, k)| r == root && keys.contains(k))
        });
        under
            .or(self.touched.first())
            .map(|(r, k)| pack.address_of(r, k))
    }
}

/// A snapshot's reading of the doc ([`Session::watch_state`]): its version, its keyed state,
/// and the view index when the pack has one.
pub(super) type WatchState = (Seq, Arc<KeyedState>, Option<(ViewSchema, ViewIndex)>);

/// What a watch's queue holds: a change, or a refused verb (unnumbered).
enum Queued {
    Change(Box<WatchChange>),
    Refused(Box<WatchRefusal>),
    OutsideSave(Box<WatchOutsideSave>),
}

/// One watch's subscription.
struct WatchSub {
    id: u64,
    conn: ConnId,
    /// An App viewer's pid: its frames go through its handles.
    app: Option<String>,
    tx: SyncSender<Queued>,
}

/// The session's keyed watches: their subscriptions, and the change events of the change being
/// committed, held until it is done so each change goes out as one frame.
#[derive(Default)]
pub(super) struct WatchHub {
    subs: Vec<WatchSub>,
    buf: Vec<ChangeEvent>,
    next_id: u64,
}

impl WatchHub {
    /// Whether any watch is open (only then are events held).
    pub(super) fn active(&self) -> bool {
        !self.subs.is_empty()
    }

    /// Hold a change event for [`Session::flush_watch`].
    pub(super) fn hold(&mut self, ev: &ChangeEvent) {
        self.buf.push(ev.clone());
    }

    /// The connection left: its watches end (their receivers see the channel close).
    pub(super) fn drop_conn(&mut self, conn: ConnId) {
        self.subs.retain(|s| s.conn != conn);
    }

    /// The session closed: every watch ends.
    pub(super) fn clear(&mut self) {
        self.subs.clear();
        self.buf.clear();
    }
}

/// A keyed watch on one connection ([`Session::watch_keyed`]): it numbers the frames and holds
/// what the next snapshot needs. The host drains it ([`KeyedWatch::try_frames`]) and sends every
/// frame in order; when [`KeyedWatch::needs_snapshot`] says so, or the client asks (`watch`),
/// it sends [`Session::watch_snapshot`]'s frames.
pub struct KeyedWatch {
    id: u64,
    conn: ConnId,
    rx: Receiver<Queued>,
    /// The last number handed out.
    seq: u64,
    /// The last snapshot's version: changes at or before it are in it.
    upto: Option<Seq>,
    page_bytes: usize,
    frame_max: usize,
    /// Cut off (fell behind, or the session ended it) or a change over the frame cap: nothing
    /// more goes out until a snapshot.
    cut: bool,
}

impl std::fmt::Debug for KeyedWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyedWatch")
            .field("conn", &self.conn)
            .field("seq", &self.seq)
            .field("upto", &self.upto)
            .field("cut", &self.cut)
            .finish()
    }
}

/// The planted fault [`crate::differential::ViewMutant::FrameMissesRenderHash`].
fn planted_frame_misses_render_hash() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::FrameMissesRenderHash)
    )
}

/// The planted fault [`crate::differential::ViewMutant::FrameMissesUnitDerived`].
fn planted_frame_misses_unit_derived() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::FrameMissesUnitDerived)
    )
}

/// A change frame's `derived` with the units' or the containers' derived values the change set
/// or removed ([`ViewChange::unit_derived`], [`ViewChange::container_derived`]) joined in under
/// `part` (`units` or `containers`): `<part>.<address>.<name>`, the text or `null`. The pack's
/// own `derived` (from [`Pack::watch_extras`]) keeps every key it has, and a name it set for an
/// element wins; a pack's `derived` that isn't an object (nor `null`) is left as it is.
fn merge_derived(
    derived: Option<Json>,
    part: &str,
    units: crate::view_index::DerivedChanges,
) -> Option<Json> {
    let mut top = match derived {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Obj(m)) => m,
        Some(other) => return Some(other),
    };
    let at = match top.iter().position(|(k, _)| k == part) {
        Some(i) => i,
        None => {
            top.push((part.into(), Json::Obj(Vec::new())));
            top.len() - 1
        }
    };
    if let Json::Obj(us) = &mut top[at].1 {
        for (unit, values) in units {
            let ui = match us.iter().position(|(k, _)| *k == unit) {
                Some(i) => i,
                None => {
                    us.push((unit, Json::Obj(Vec::new())));
                    us.len() - 1
                }
            };
            let Json::Obj(fs) = &mut us[ui].1 else {
                continue;
            };
            for (name, text) in values {
                if !fs.iter().any(|(k, _)| *k == name) {
                    fs.push((name, text.map_or(Json::Null, Json::from)));
                }
            }
        }
    }
    Some(Json::Obj(top))
}

/// The planted fault [`crate::differential::ViewMutant::FrameMissesContainerDerived`].
fn planted_frame_misses_container_derived() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::FrameMissesContainerDerived)
    )
}

/// The planted fault [`crate::differential::WatchMutant::SnapshotOffByOne`].
fn planted_snapshot_off_by_one() -> bool {
    plant_on!(
        crate::differential::watch_mutant()
            == Some(crate::differential::WatchMutant::SnapshotOffByOne)
    )
}

impl KeyedWatch {
    /// The connection it watches on.
    pub fn conn(&self) -> ConnId {
        self.conn
    }

    /// The last frame number handed out (0: none yet).
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// The host must send a fresh snapshot ([`Session::watch_snapshot`]) before anything else
    /// goes out: none sent yet, the watch fell behind (its queue of
    /// [`Config::subscriber_queue`] changes filled), or a change's frame was over the frame cap.
    pub fn needs_snapshot(&self) -> bool {
        self.upto.is_none() || self.cut
    }

    /// The change frames queued since the last call, numbered, without waiting.
    pub fn try_frames(&mut self) -> Vec<WatchFrame> {
        let mut out = Vec::new();
        while !self.cut {
            match self.rx.try_recv() {
                Ok(ch) => out.extend(self.take(ch)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => self.cut = true,
            }
        }
        out
    }

    /// As [`KeyedWatch::try_frames`], waiting up to `timeout` for the first change.
    pub fn recv_frames(&mut self, timeout: Duration) -> Vec<WatchFrame> {
        if self.cut {
            return Vec::new();
        }
        let mut out = match self.rx.recv_timeout(timeout) {
            Ok(ch) => self.take(ch).into_iter().collect(),
            Err(RecvTimeoutError::Timeout) => Vec::new(),
            Err(RecvTimeoutError::Disconnected) => {
                self.cut = true;
                Vec::new()
            }
        };
        out.extend(self.try_frames());
        out
    }

    /// A queued change as its frame, numbered; `None` when the last snapshot holds it, or its
    /// frame is over the cap (then a snapshot is needed). A refused verb as its frame,
    /// unnumbered, once a snapshot went out (one over the cap is dropped: it changes nothing).
    fn take(&mut self, q: Queued) -> Option<WatchFrame> {
        let mut ch = match q {
            Queued::Change(ch) => ch,
            Queued::Refused(r) => {
                self.upto?;
                let f = WatchFrame::Refused(r);
                return (f.to_json().len() <= self.frame_max).then_some(f);
            }
            Queued::OutsideSave(o) => {
                self.upto?;
                let f = WatchFrame::OutsideSave(o);
                return (f.to_json().len() <= self.frame_max).then_some(f);
            }
        };
        let upto = self.upto?;
        let held = match planted_snapshot_off_by_one() {
            true => ch.rev < upto,
            false => ch.rev <= upto,
        };
        if held {
            return None;
        }
        ch.seq = self.seq + 1;
        let f = WatchFrame::Change(ch);
        if f.to_json().len() > self.frame_max {
            self.cut = true;
            return None;
        }
        self.seq += 1;
        Some(f)
    }
}

impl Session {
    /// Whether this doc can be watched by key: a keyed doc that isn't a table (a text doc has
    /// the hunk channel, a table its own frames). A host lists [`KEYED_WATCH_FEATURE`] in a
    /// connection's welcome only when it is.
    pub fn keyed_watchable(&self) -> bool {
        self.keeps_view()
    }

    /// Open a keyed watch on `conn` (needs read), subscribed from now on. `None` when the host
    /// didn't list [`KEYED_WATCH_FEATURE`] in `opts` (it gets no watch, so none of its frames),
    /// or the doc isn't [watchable](Session::keyed_watchable). Then send
    /// [`Session::watch_snapshot`]'s frames, and after them [`KeyedWatch::try_frames`]'.
    pub fn watch_keyed(&self, conn: ConnId, opts: &WatchOptions) -> Result<Option<KeyedWatch>> {
        if !opts.listed || !self.keyed_watchable() {
            return Ok(None);
        }
        let now = self.now();
        let mut st = self.lock_for(conn)?;
        let i = self.admit(&mut st, conn, Need::Read, Action::Subscribe, now)?;
        let (page_bytes, frame_max) = opts.caps();
        let (id, rx) = self.watch_subscribe(&mut st, i, conn);
        Ok(Some(KeyedWatch {
            id,
            conn,
            rx,
            seq: 0,
            upto: None,
            page_bytes,
            frame_max,
            cut: false,
        }))
    }

    /// A fresh snapshot for `w`: the doc's keyed state now, as frames numbered after `w`'s last
    /// one. Every change at or before it is in it, so `w` drops those still queued; a watch
    /// cut off is subscribed again first. Needs read (checked again). Refused `MPC-E401` when
    /// one entry's page would pass the frame cap.
    ///
    /// A catch-up (the changes since the client's version, when they fit the cap) would be
    /// served here, before falling back to this snapshot; it isn't built.
    pub fn watch_snapshot(&self, w: &mut KeyedWatch) -> Result<Vec<WatchFrame>> {
        let now = self.now();
        let (upto, state, view) = {
            let mut st = self.lock()?;
            let i = self.admit(&mut st, w.conn, Need::Read, Action::Subscribe, now)?;
            if !st.watch.subs.iter().any(|s| s.id == w.id) {
                let (id, rx) = self.watch_subscribe(&mut st, i, w.conn);
                w.id = id;
                w.rx = rx;
            }
            self.watch_state(&mut st)?
        };
        let frames = snapshot_frames(
            &*self.pack,
            self.schema,
            upto,
            &state,
            view.as_ref(),
            w.seq + 1,
            w.page_bytes,
            w.frame_max,
        )?;
        w.seq += frames.len() as u64;
        w.upto = Some(upto);
        w.cut = false;
        Ok(frames)
    }

    /// What a watch's snapshot holds, at the doc's version: the one place a snapshot reads the
    /// doc. The live view's keyed state (shared, no copy), and the view index built from it, as
    /// [`Session::view_index`] builds it, when the pack declares a view schema (one that
    /// doesn't fit the doc is the pack's bug: [`SessionError::Pack`]).
    pub(super) fn watch_state(&self, st: &mut State) -> Result<WatchState> {
        let state = self.live_state(st)?;
        let view = match self.pack.view_schema() {
            Some(vs) => {
                vs.check(&self.schema, self.pack.composite())
                    .map_err(|m| SessionError::Pack(PackError(m)))?;
                // as `Session::view_index` gives it: through the kept index (the carrier too)
                self.kept_view_index(st)?.map(|ix| (vs, (*ix).clone()))
            }
            None => None,
        };
        Ok((st.version, state, view))
    }

    fn watch_subscribe(&self, st: &mut State, i: usize, conn: ConnId) -> (u64, Receiver<Queued>) {
        let (tx, rx) = sync_channel(self.config.subscriber_queue.max(1));
        let app = Self::app_viewer(st, i);
        st.watch.next_id += 1;
        let id = st.watch.next_id;
        st.watch.subs.push(WatchSub { id, conn, app, tx });
        (id, rx)
    }

    /// After a change is committed and the live view kept: each version's held events as one
    /// change frame to every watch. One that is full or gone is cut off (it asks for a
    /// snapshot).
    pub(super) fn flush_watch(&self, st: &mut State) {
        if st.watch.buf.is_empty() {
            return;
        }
        let buf = std::mem::take(&mut st.watch.buf);
        if st.watch.subs.is_empty() {
            return;
        }
        let state = self.live_state(st).ok();
        // the view index before and after, through the kept index: the frame's render hashes
        // and the pack's derived values read it (`Pack::watch_extras`)
        let before = Self::view_index_kept_before(st);
        let after = self.kept_view_index(st).ok().flatten();
        let view = after.as_deref().map(|a| ViewChange {
            before: before.as_deref(),
            after: a,
        });
        // each version's events, and their ops
        let mut groups: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let mut j = i + 1;
            while j < buf.len() && buf[j].version == buf[i].version {
                j += 1;
            }
            groups.push((i, j));
            i = j;
        }
        let applied: Vec<Vec<ElemApplied>> = groups
            .iter()
            .map(|&(i, j)| buf[i..j].iter().map(|e| e.applied.clone()).collect())
            .collect();
        // the view deltas of the run, from the ops and the view schema, read against the state
        // after the last change
        let mut deltas: Vec<Option<ViewDeltas>> = match (self.watch_view_schema(), &state) {
            (Some(vs), Some(s)) => {
                let changes: Vec<&[ElemApplied]> = applied.iter().map(Vec::as_slice).collect();
                view_deltas_run(&*self.pack, &vs, &changes, s)
                    .into_iter()
                    .map(Some)
                    .collect()
            }
            _ => vec![None; groups.len()],
        };
        let mut named: Vec<String> = Vec::new();
        for (g, &(i, j)) in groups.iter().enumerate() {
            let ch = self.watch_change(
                st,
                &buf[i..j],
                &applied[g],
                deltas[g].take(),
                state.as_deref(),
                view,
                g + 1 == groups.len(),
            );
            for d in ch.deltas.iter() {
                named.extend(
                    d.units
                        .iter()
                        .filter(|u| u.placement_after.is_some())
                        .map(|u| u.key.clone()),
                );
                named.extend(
                    d.containers
                        .iter()
                        .filter(|c| c.placement_after.is_some())
                        .map(|c| c.key.clone()),
                );
            }
            let handles = &mut st.handles;
            st.watch.subs.retain(|s| {
                let ch = match &s.app {
                    Some(v) => handles.lens(v).watch_change(ch.clone()),
                    None => ch.clone(),
                };
                s.tx.try_send(Queued::Change(Box::new(ch))).is_ok()
            });
        }
        // a new carrier taken in since the last frame, which moved placements these frames
        // don't name: every watch takes a fresh snapshot
        if let (Some(old), Some(new)) = (st.view_carrier_before.take(), after.as_deref()) {
            if Self::carrier_moved(&old, new, &named) {
                st.watch.subs.clear();
            }
        }
    }

    /// When a new carrier was taken in with no change frame to carry it (an outside save that
    /// changed only what the carrier holds): every watch takes a fresh snapshot if a placement
    /// moved or a container's parent changed (a parent the pack derives from the carrier). Called
    /// at the end of a disk check.
    pub(super) fn settle_carrier_watch(&self, st: &mut State) {
        let Some(old) = st.view_carrier_before.take() else {
            return;
        };
        if st.watch.subs.is_empty() {
            return;
        }
        if let Ok(Some(new)) = self.kept_view_index(st) {
            if Self::carrier_moved(&old, &new, &[]) || Self::parents_moved(&old, &new) {
                st.watch.subs.clear();
            }
        }
    }

    /// Whether any container in both `old` and `new` has another parent in `new`.
    fn parents_moved(old: &ViewIndex, new: &ViewIndex) -> bool {
        let was: HashMap<&str, Option<&String>> = old
            .containers
            .iter()
            .map(|c| (c.key.as_str(), c.parent.as_ref()))
            .collect();
        new.containers.iter().any(|c| {
            was.get(c.key.as_str())
                .is_some_and(|p| *p != c.parent.as_ref())
        })
    }

    /// Whether any unit's or container's placement differs between `old` and `new`, leaving out
    /// the addresses in `named` (a frame carries theirs).
    /// Elements in only one of the two came or went with a delta of their own.
    fn carrier_moved(old: &ViewIndex, new: &ViewIndex, named: &[String]) -> bool {
        let named: std::collections::HashSet<&str> = named.iter().map(String::as_str).collect();
        let was: HashMap<&str, &Option<crate::placement::Placement>> = old
            .units
            .iter()
            .map(|u| (u.key.as_str(), &u.placement))
            .chain(
                old.containers
                    .iter()
                    .map(|c| (c.key.as_str(), &c.placement)),
            )
            .collect();
        new.units
            .iter()
            .map(|u| (u.key.as_str(), &u.placement))
            .chain(
                new.containers
                    .iter()
                    .map(|c| (c.key.as_str(), &c.placement)),
            )
            .any(|(k, p)| !named.contains(k) && was.get(k).is_some_and(|w| *w != p))
    }

    /// The pack's view schema, when it has one that fits the doc (the snapshot refuses one
    /// that doesn't, [`Session::watch_state`]; a change frame then carries no deltas).
    fn watch_view_schema(&self) -> Option<ViewSchema> {
        self.pack
            .view_schema()
            .filter(|vs| vs.check(&self.schema, self.pack.composite()).is_ok())
    }

    /// One change's frame (unnumbered) from its events, one per root it touched. `last`: the
    /// last change of the flush, which carries the units' derived values the flush set or
    /// removed (read from the view index after it, as the render hashes are), and the parents
    /// of the containers whose parent changed with no delta of their own
    /// ([`crate::view_delta::parent_deltas`]).
    #[allow(clippy::too_many_arguments)]
    fn watch_change(
        &self,
        st: &State,
        evs: &[ChangeEvent],
        applied: &[ElemApplied],
        deltas: Option<ViewDeltas>,
        state: Option<&KeyedState>,
        view: Option<ViewChange<'_>>,
        last: bool,
    ) -> WatchChange {
        let e = &evs[0];
        let mut deltas = deltas;
        let mut space_after = None;
        if let (Some(vs), Some(v)) = (self.watch_view_schema(), view) {
            if let Some(d) = deltas.as_mut() {
                crate::view_delta::place_deltas(&vs, d, v.before, v.after);
                crate::view_delta::parent_deltas(&vs, d, v.before, v.after, last);
            }
            let was = v.before.and_then(|b| b.space.as_ref());
            if last && vs.space.is_some() && was != v.after.space.as_ref() {
                space_after = v.after.space.clone();
            }
        }
        let mut ops = Vec::new();
        for e in evs {
            ops_of(&*self.pack, &e.applied, &mut ops);
        }
        let mut extras = state.map_or_else(WatchExtras::default, |s| {
            self.pack.watch_extras(applied, s, view)
        });
        // the containers whose render hash changed, from the view index, unless the pack said
        if extras.render_hashes.is_empty() && !planted_frame_misses_render_hash() {
            if let Some(v) = view {
                extras.render_hashes = v
                    .render_hashes()
                    .into_iter()
                    .map(|(c, h)| (c, h.to_hex()))
                    .collect();
            }
        }
        // the units' derived values the change set or removed, beside the pack's own
        if let (true, Some(v)) = (last && !planted_frame_misses_unit_derived(), view) {
            let d = v.unit_derived();
            if !d.is_empty() {
                extras.derived = merge_derived(extras.derived.take(), "units", d);
            }
        }
        // and the containers' (`derived.containers.<container>.<name>`)
        if let (true, Some(v)) = (last && !planted_frame_misses_container_derived(), view) {
            let d = v.container_derived();
            if !d.is_empty() {
                extras.derived = merge_derived(extras.derived.take(), "containers", d);
            }
        }
        WatchChange {
            seq: 0,
            rev: e.version,
            at_ms: e.at_ms,
            kind: e.kind,
            actor: Self::watch_actor(st, &e.pid),
            client: e.client,
            via: e.via.clone(),
            on_behalf_of: e.on_behalf_of.clone(),
            turn: e.turn,
            verb: e.verb.clone(),
            claim: self.watch_claim(st, &e.pid, applied),
            ops,
            deltas,
            extras,
            space_after,
        }
    }

    /// A pack's structured edit verb run by `refused.conn` was refused: one [`WatchRefusal`] to
    /// every keyed watch whose connection may read the doc now (the host's rights for it are
    /// asked here, so a watcher whose rights went since it opened gets none), through an App
    /// viewer's handles. Nothing when no watch is open, or the refused connection isn't in the
    /// doc (any more). A watch whose queue is full misses it (it stays open: a refusal changes
    /// nothing).
    pub(crate) fn watch_refused(&self, refused: VerbRefused) {
        let now = self.now();
        let Ok(mut st) = self.lock() else {
            return;
        };
        let st = &mut *st;
        if !st.watch.active() {
            return;
        }
        let Some(m) = st.members.iter().find(|m| m.conn == refused.conn) else {
            return;
        };
        let actor = Self::watch_actor(st, &m.p.pid);
        let client = m.client;
        let holder = refused.holder.as_ref().map(|(pid, name, _)| {
            let mut h = Self::watch_actor(st, pid);
            if !st.members.iter().any(|m| m.p.pid == *pid) {
                // gone since: the name the refusal gave
                h.name.clone_from(name);
            }
            h
        });
        let r = WatchRefusal {
            at_ms: now,
            actor,
            client,
            turn: refused.turn,
            unit: refused.unit_address(&*self.pack),
            verb: refused.verb,
            code: refused.code,
            holder,
            claim: refused
                .holder
                .and_then(|(_, _, c)| c)
                .map(|c| c.to_string()),
        };
        let may_read: std::collections::HashSet<ConnId> = st
            .members
            .iter()
            .filter(|m| watched(self.host.rights(&m.p, &st.key), m.watcher).read)
            .map(|m| m.conn)
            .collect();
        let handles = &mut st.handles;
        st.watch.subs.retain(|s| {
            if !may_read.contains(&s.conn) {
                return true;
            }
            let r = match &s.app {
                Some(v) => handles.lens(v).watch_refusal(r.clone()),
                None => r.clone(),
            };
            !matches!(
                s.tx.try_send(Queued::Refused(Box::new(r))),
                Err(TrySendError::Disconnected(_))
            )
        });
    }

    /// What came of an outside save: one [`WatchOutsideSave`] to every keyed watch whose
    /// connection may read the doc now (as [`Session::watch_refused`]). Called under the lock,
    /// after the save's change frame went out; nothing when no watch is open.
    pub(super) fn watch_outside_save(&self, st: &mut State, outcome: OutsideSaveOutcome) {
        if !st.watch.active() {
            return;
        }
        let o = WatchOutsideSave {
            at_ms: self.now(),
            doc: st.key.path.clone(),
            outcome,
        };
        let may_read: std::collections::HashSet<ConnId> = st
            .members
            .iter()
            .filter(|m| watched(self.host.rights(&m.p, &st.key), m.watcher).read)
            .map(|m| m.conn)
            .collect();
        st.watch.subs.retain(|s| {
            if !may_read.contains(&s.conn) {
                return true;
            }
            !matches!(
                s.tx.try_send(Queued::OutsideSave(Box::new(o.clone()))),
                Err(TrySendError::Disconnected(_))
            )
        });
    }

    /// A kept-both save's conflict copy for its frame: `record` relative to the file's folder,
    /// `real` its path, and its doc address when the host says it is a kit document.
    pub(super) fn outside_save_copy(
        &self,
        record: Option<&str>,
        real: Option<&std::path::Path>,
    ) -> Option<OutsideSaveCopy> {
        let (record, real) = (record?, real?);
        Some(OutsideSaveCopy {
            name: real
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(record)
                .to_owned(),
            path: record.to_owned(),
            doc: self.host.document_at(real).map(|k| k.path),
        })
    }

    fn watch_actor(st: &State, pid: &str) -> WatchActor {
        if pid == EXTERNAL_PID {
            return WatchActor {
                id: pid.to_owned(),
                name: pid.to_owned(),
                kind: ParticipantKind::External,
                colour: None,
            };
        }
        match st.members.iter().find(|m| m.p.pid == pid) {
            Some(m) => WatchActor {
                id: pid.to_owned(),
                name: m.p.name.clone(),
                kind: m.p.kind,
                colour: Some(m.p.colour.clone()),
            },
            None => WatchActor {
                id: pid.to_owned(),
                name: pid.to_owned(),
                kind: ParticipantKind::Person,
                colour: None,
            },
        }
    }

    /// The actor's own element claim that the change touched: one naming an element it changed,
    /// or that element's container. The first such claim, in the order they were taken.
    fn watch_claim(&self, st: &State, pid: &str, applied: &[ElemApplied]) -> Option<String> {
        let mine = || {
            st.aware
                .claims
                .iter()
                .filter(|c| c.pid == pid)
                .filter_map(|c| c.elems.as_ref().map(|s| (c, s)))
        };
        mine().next()?;
        let mut touched = elem_claims::landed_elems(self.pack.composite(), applied);
        let containers: Vec<(String, Key)> = touched
            .iter()
            .flat_map(|(r, ks)| ks.iter().map(move |k| (r.clone(), k.clone())))
            .filter_map(|(r, k)| self.pack.container_of(&r, &k))
            .collect();
        for (r, k) in containers {
            touched.entry(r).or_default().insert(k);
        }
        mine()
            .find(|(_, s)| elem_claims::is_touched(&touched, s))
            .map(|(c, _)| c.id.to_string())
    }
}
