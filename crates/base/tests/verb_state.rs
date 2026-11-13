//! A pack's **verb state** (`Pack::verb_state_build`, `Pack::verb_state_change`): what its
//! structured verbs read beside a live keyed doc, kept by the kit in step with every change.
//!
//! The test pack, `track-lite`: items (a MovableList) with a `track` and a `start` field each
//! (a composite Map keyed `<item>\x1f<field>`); the file is one item a line, `key track
//! start`. Its verb state is a per-track index of the live items by start, the shape a
//! timeline's verbs need; its verbs read it (`track show`) and move an item (`item move`).
//!
//! - a verb reads the kept index, built once and kept through every change source: a pack
//!   verb, a host's edit (a set, an insert, a delete), an outside save, an undo and a redo, a
//!   restore. After each, the verbs' answer is the doc's (from its rendering), the kit's own
//!   checks (test builds) compare the kept index with a fresh build, and the doc's index is
//!   built once;
//! - a pack that keeps none: its verbs get none and nothing is built;
//! - the planted fault "an outside save skips the hook" (`VerbStateMutant`) and a pack whose
//!   hook says it kept the index but left it stale are caught by the kit's compare, loudly.
//!
//! Each row is a function returning `Err("<row name>: ...")`. Public API only.

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use k2_mpc_base::differential::{take_verb_state_builds, with_verb_state_mutant, VerbStateMutant};
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, Applied, ListOp, MapOp};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, DiskCheck, EditOptions, Host, Pack, PackContent,
    PackError, Participant, ParticipantKind, Reconciled, Rights, Session, SessionManager, Stay,
    VerbState, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};
use k2_mpc_base::{Edit, Key, Value};

const ITEMS: &str = "items";
const FIELDS: &str = "fields";
const SEP: char = '\u{1f}';

// ---------- the test pack ----------

/// `track-lite`, keeping its index (`true`) or none (`false`).
struct Tracks(bool);

const SCHEMA: Schema = Schema {
    name: "track-lite",
    roots: &[(ITEMS, Kind::MovableList), (FIELDS, Kind::Map)],
};

type Parsed = (Vec<(Key, Value)>, BTreeMap<String, Value>);

fn parse(b: &[u8]) -> Result<Parsed, Declined> {
    let bad = |message: String| Declined {
        code: "malformed",
        message,
    };
    let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
    let (mut items, mut fields) = (Vec::new(), BTreeMap::new());
    for line in s.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        let [k, track, start] = w[..] else {
            return Err(bad(format!("not `key track start`: {line:?}")));
        };
        let key = Key::new(k).map_err(|e| bad(e.to_string()))?;
        fields.insert(format!("{k}{SEP}track"), Value::str(track));
        fields.insert(format!("{k}{SEP}start"), Value::str(start));
        items.push((key, Value::Null));
    }
    Ok((items, fields))
}

fn field<'a>(fields: &'a BTreeMap<String, Value>, k: &Key, f: &str) -> &'a str {
    fields
        .get(&format!("{k}{SEP}{f}"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

impl Pack for Tracks {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(FIELDS, ITEMS)]
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (items, fields) = parse(b)?;
        Ok(PackContent {
            roots: vec![
                (ITEMS.into(), Neutral::List(items)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        let root = |n: &str| c.iter().find(|(r, _)| r == n).map(|(_, x)| x);
        let (Some(Neutral::List(items)), Some(Neutral::Map(fields))) = (root(ITEMS), root(FIELDS))
        else {
            return Err(PackError("no items".into()));
        };
        let mut out = String::new();
        for (k, _) in items {
            let (t, s) = (field(fields, k, "track"), field(fields, k, "start"));
            out.push_str(&format!("{k} {t} {s}\n"));
        }
        Ok(out.into_bytes())
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let (items, fields) = parse(bytes)?;
        let old: &[(Key, Value)] = synced.list(ITEMS).unwrap_or(&[]);
        let hidden = synced.hidden(ITEMS);
        let empty = BTreeMap::new();
        let old_fields = synced.map(FIELDS).unwrap_or(&empty);
        let old_keys: BTreeSet<&Key> = old.iter().map(|(k, _)| k).collect();
        let new_keys: BTreeSet<&Key> = items.iter().map(|(k, _)| k).collect();
        let mut list = Vec::new();
        for (k, _) in old.iter().filter(|(k, _)| !new_keys.contains(k)) {
            list.push(ListOp::Delete { key: k.clone() });
        }
        let mut prev: Option<Key> = None;
        for (k, _) in &items {
            if !old_keys.contains(k) {
                if hidden.is_some_and(|h| h.contains_key(k)) {
                    list.push(ListOp::Restore { key: k.clone() });
                } else {
                    list.push(ListOp::Insert {
                        key: k.clone(),
                        at: prev.clone().map_or(Anchor::Start, Anchor::After),
                        value: Value::Null,
                    });
                }
            }
            prev = Some(k.clone());
        }
        let mut map = Vec::new();
        for (k, v) in &fields {
            if old_fields.get(k) != Some(v) {
                map.push(MapOp::Set {
                    key: k.clone(),
                    value: v.clone(),
                });
            }
        }
        let mut edits = Vec::new();
        if !list.is_empty() {
            edits.push(Edit::List {
                root: ITEMS.into(),
                ops: list,
            });
        }
        if !map.is_empty() {
            edits.push(Edit::Map {
                root: FIELDS.into(),
                ops: map,
            });
        }
        Ok(Reconciled {
            edits,
            codec: vec![1],
            ..Reconciled::default()
        })
    }
    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        self.0
            .then(|| Box::new(Starts::build(view)) as Box<dyn VerbState>)
    }
    fn verb_state_change(
        &self,
        state: &mut (dyn VerbState + 'static),
        applied: &[Applied],
        view: &View,
    ) -> bool {
        let Some(s) = state.downcast_mut::<Starts>() else {
            return false;
        };
        if STALE_HOOK.with(|p| p.get()) {
            // the planted pack bug: says it kept the index, changed nothing
            return true;
        }
        s.change(applied, view);
        true
    }
}

thread_local! {
    /// The pack bug "the hook returns true and leaves the index as it was", planted on this
    /// thread (the session runs the hook on the caller's thread, under its lock).
    static STALE_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The verb state: each track's live items by start, and where each item sits.
#[derive(Debug, Default, PartialEq)]
struct Starts {
    by_track: BTreeMap<String, BTreeSet<(i64, Key)>>,
    of_item: BTreeMap<Key, (String, i64)>,
}

impl Starts {
    fn build(view: &View) -> Starts {
        let mut s = Starts::default();
        for (k, _) in view.list(ITEMS).unwrap_or(&[]) {
            s.put(view, k);
        }
        s
    }

    fn put(&mut self, view: &View, k: &Key) {
        let empty = BTreeMap::new();
        let fields = view.map(FIELDS).unwrap_or(&empty);
        let track = field(fields, k, "track").to_owned();
        let start = field(fields, k, "start").parse().unwrap_or(0);
        self.by_track
            .entry(track.clone())
            .or_default()
            .insert((start, k.clone()));
        self.of_item.insert(k.clone(), (track, start));
    }

    fn take(&mut self, k: &Key) -> bool {
        let Some((track, start)) = self.of_item.remove(k) else {
            return false;
        };
        if let Some(set) = self.by_track.get_mut(&track) {
            set.remove(&(start, k.clone()));
            if set.is_empty() {
                self.by_track.remove(&track);
            }
        }
        true
    }

    /// One change: an item a list op left live is put again from the view, one it left
    /// deleted or dropped is taken out; a live item whose fields changed is put again.
    fn change(&mut self, applied: &[Applied], view: &View) {
        for a in applied {
            match a {
                Applied::List { root, ops } if root == ITEMS => {
                    for o in ops {
                        self.take(&o.key);
                        if o.index_after.is_some() {
                            self.put(view, &o.key);
                        }
                    }
                }
                Applied::Map { root, ops } if root == FIELDS => {
                    for o in ops {
                        let Some((item, _)) = o.key.split_once(SEP) else {
                            continue;
                        };
                        let Ok(k) = Key::new(item) else {
                            continue;
                        };
                        if self.take(&k) {
                            self.put(view, &k);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

const S: &[PersonAction] = &[PersonAction::Format];

const VERBS: &[PackVerb] = &[
    PackVerb::structured_read("track show", "track show <track>", S, &[0, 2], track_show)
        .positions(&[]),
    PackVerb::structured_read("state show", "state show", S, &[0], state_show).positions(&[]),
    PackVerb::structured_edit(
        "item move",
        "item move <item> <track> <start>",
        S,
        &[0, 2],
        item_move,
    )
    .positions(&[]),
];

/// `track show <track>`: the track's items by start, `start key` a line, from the kept index.
fn track_show(args: &[String]) -> Result<StructEdit, ToolError> {
    let [track] = args else {
        return Err(ToolError::usage("name one track"));
    };
    let track = track.clone();
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let Some(s) = ctx.verb_state::<Starts>() else {
            return Err(ToolError::usage("no verb state"));
        };
        let mut out = String::new();
        for (start, k) in s.by_track.get(&track).into_iter().flatten() {
            out.push_str(&format!("{start} {k}\n"));
        }
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0))
    }))
}

/// `state show`: `kept` when the verb gets the index, else `none`.
fn state_show(_: &[String]) -> Result<StructEdit, ToolError> {
    Ok(Box::new(|ctx: &VerbCtx<'_>| {
        let out = match ctx.verb_state::<Starts>() {
            Some(_) => "kept\n",
            None => "none\n",
        };
        Ok(StructOutput::new(Vec::new(), out.into(), 0))
    }))
}

/// `item move <item> <track> <start>`: sets both fields; prints how many items the track held
/// before, from the kept index.
fn item_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let [item, track, start] = args else {
        return Err(ToolError::usage("item move <item> <track> <start>"));
    };
    let (item, track, start) = (item.clone(), track.clone(), start.clone());
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let Some(s) = ctx.verb_state::<Starts>() else {
            return Err(ToolError::usage("no verb state"));
        };
        let held = s.by_track.get(&track).map_or(0, BTreeSet::len);
        let set = |f: &str, v: &str| MapOp::Set {
            key: format!("{item}{SEP}{f}"),
            value: Value::str(v),
        };
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![set("track", &track), set("start", &start)],
            }],
            format!("{track} held {held}\n").into_bytes(),
            0,
        ))
    }))
}

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _who: &Participant, _key: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        self.now.fetch_add(1_000, Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct Fx {
    dir: PathBuf,
    ws: PathBuf,
    mgr: SessionManager,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const NAME: &str = "cut.tracks";
const FILE: &str = "a A 30\nb A 10\nc B 20\nd B 5\n";

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-verb-state-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = std::fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_000_000),
    });
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host, c);
    Fx { dir, ws, mgr }
}

impl Fx {
    fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    fn open(&self, keeps: bool) -> Arc<Session> {
        std::fs::write(self.path(), FILE).unwrap();
        let key = DocKey {
            space: "ws".into(),
            path: NAME.into(),
        };
        self.mgr
            .open_file(Arc::new(Tracks(keeps)), &key, &self.path(), &self.ws)
            .unwrap()
    }
}

fn person(s: &Session) -> ConnId {
    s.join(
        Participant::new("ana", "Ana", "#123456", ParticipantKind::Person, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// An app's save: written beside the file, then renamed over it.
fn app_save(path: &Path, bytes: &str) {
    let tmp = path.with_file_name(".app-save.tmp");
    std::fs::write(&tmp, bytes).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

/// `track show <track>` as the verb prints it.
fn shown(s: &Session, c: ConnId, track: &str) -> Result<String, String> {
    let o = s
        .run_tool(c, ITEMS, &argv(&["track", "show", track]), b"")
        .map_err(|e| format!("track show {track}: {e}"))?;
    if o.exit != 0 {
        return Err(format!(
            "track show {track}: exit {} ({})",
            o.exit,
            String::from_utf8_lossy(&o.stdout)
        ));
    }
    Ok(String::from_utf8(o.stdout).unwrap())
}

/// What `track show <track>` must print: the doc's own items, from its rendering.
fn want(s: &Session, c: ConnId, track: &str) -> String {
    let bytes = s.read(c).unwrap().bytes;
    let (items, fields) = parse(&bytes).unwrap();
    let mut on: Vec<(i64, Key)> = items
        .iter()
        .filter(|(k, _)| field(&fields, k, "track") == track)
        .map(|(k, _)| (field(&fields, k, "start").parse().unwrap_or(0), k.clone()))
        .collect();
    on.sort();
    on.iter().map(|(s, k)| format!("{s} {k}\n")).collect()
}

/// Both tracks as the verb prints them are the doc's.
fn verbs_agree(s: &Session, c: ConnId, row: &str, after: &str) -> Result<(), String> {
    for track in ["A", "B", "C"] {
        let got = shown(s, c, track).map_err(|e| format!("{row}: after {after}: {e}"))?;
        let want = want(s, c, track);
        if got != want {
            return Err(format!(
                "{row}: after {after}: track {track} shows {got:?}, the doc holds {want:?}"
            ));
        }
    }
    Ok(())
}

fn set(item: &str, f: &str, v: &str) -> MapOp {
    MapOp::Set {
        key: format!("{item}{SEP}{f}"),
        value: Value::str(v),
    }
}

// ---------- the rows ----------

/// Every change source keeps the index: an outside save taken in by key, a pack verb, a host's
/// set, insert and delete, an undo and a redo, a restore; then an outside save over an
/// unwritten change (keep both). After each the verbs show the doc's own order (and the kit's
/// checks compare the kept index with a fresh build); through the first eight the index is
/// built once.
fn row_every_change_source_keeps_the_index() -> Result<(), String> {
    const ROW: &str = "row_every_change_source_keeps_the_index";
    let f = fx("sources");
    let s = f.open(true);
    let c = person(&s);
    take_verb_state_builds();
    verbs_agree(&s, c, ROW, "the open")?;
    // an outside save, taken in by key (no kit write or unwritten change since the open, so
    // it applies plainly): b moves to C, c goes, a new item f
    app_save(&f.path(), "a A 30\nb C 7\nd B 5\nf A 3\n");
    match s.file_changed() {
        Ok(DiskCheck::Ingested { .. }) => {}
        other => return Err(format!("{ROW}: the outside save: {other:?}")),
    }
    verbs_agree(&s, c, ROW, "an outside save")?;
    // a pack verb (it reads the index too)
    let o = s
        .run_tool(c, ITEMS, &argv(&["item", "move", "d", "A", "20"]), b"")
        .map_err(|e| format!("{ROW}: item move: {e}"))?;
    if o.exit != 0 || o.stdout != b"A held 2\n" {
        return Err(format!(
            "{ROW}: item move: exit {}, {:?}",
            o.exit,
            String::from_utf8_lossy(&o.stdout)
        ));
    }
    verbs_agree(&s, c, ROW, "a pack verb")?;
    // a host's edits: a set, an insert with its fields, a delete
    let host = |edits: Vec<Edit>, what: &str| {
        s.edit_ops(c, edits, &EditOptions::new())
            .map(|_| ())
            .map_err(|e| format!("{ROW}: {what}: {e}"))
    };
    host(
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![set("d", "start", "40")],
        }],
        "a set",
    )?;
    verbs_agree(&s, c, ROW, "a host's set")?;
    host(
        vec![
            Edit::List {
                root: ITEMS.into(),
                ops: vec![ListOp::Insert {
                    key: Key::new("e").unwrap(),
                    at: Anchor::End,
                    value: Value::Null,
                }],
            },
            Edit::Map {
                root: FIELDS.into(),
                ops: vec![set("e", "track", "C"), set("e", "start", "1")],
            },
        ],
        "an insert",
    )?;
    verbs_agree(&s, c, ROW, "a host's insert")?;
    host(
        vec![Edit::List {
            root: ITEMS.into(),
            ops: vec![ListOp::Delete {
                key: Key::new("a").unwrap(),
            }],
        }],
        "a delete",
    )?;
    verbs_agree(&s, c, ROW, "a host's delete")?;
    // an undo and a redo of the person's last change (the delete)
    s.undo(c, 1).map_err(|e| format!("{ROW}: undo: {e}"))?;
    verbs_agree(&s, c, ROW, "an undo")?;
    s.redo(c, 1).map_err(|e| format!("{ROW}: redo: {e}"))?;
    verbs_agree(&s, c, ROW, "a redo")?;
    // a restore of the first version
    let first = s
        .versions(c)
        .map_err(|e| format!("{ROW}: versions: {e}"))?
        .first()
        .map(|v| v.id)
        .ok_or_else(|| format!("{ROW}: no version"))?;
    s.restore(c, first)
        .map_err(|e| format!("{ROW}: restore: {e}"))?;
    verbs_agree(&s, c, ROW, "a restore")?;
    let builds = take_verb_state_builds();
    if builds != 1 {
        return Err(format!(
            "{ROW}: the index was built {builds} times: it wasn't kept through the changes"
        ));
    }
    // an outside save over a change the file never got keeps both: the save becomes the
    // live doc (the kit's state kept as a version and a conflict copy)
    s.edit_ops(
        c,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![set("d", "start", "2")],
        }],
        &EditOptions::new(),
    )
    .map_err(|e| format!("{ROW}: a set before the save: {e}"))?;
    app_save(&f.path(), "d B 9\ng B 1\n");
    match s.file_changed() {
        Ok(DiskCheck::Conflict { .. }) => {}
        other => {
            return Err(format!(
                "{ROW}: the save over an unwritten change: {other:?}"
            ))
        }
    }
    verbs_agree(&s, c, ROW, "a save over an unwritten change (keep both)")
}

#[test]
fn every_change_source_keeps_the_index() {
    row_every_change_source_keeps_the_index().unwrap();
}

/// A pack that keeps no verb state: its verbs get none, and the kit builds none.
#[test]
fn a_pack_that_keeps_none_gets_none() {
    let f = fx("none");
    let s = f.open(false);
    let c = person(&s);
    take_verb_state_builds();
    let o = s
        .run_tool(c, ITEMS, &argv(&["state", "show"]), b"")
        .unwrap();
    assert_eq!(o.stdout, b"none\n");
    s.edit_ops(
        c,
        vec![Edit::Map {
            root: FIELDS.into(),
            ops: vec![set("a", "start", "1")],
        }],
        &EditOptions::new(),
    )
    .unwrap();
    let o = s
        .run_tool(c, ITEMS, &argv(&["state", "show"]), b"")
        .unwrap();
    assert_eq!(o.stdout, b"none\n");
    assert_eq!(take_verb_state_builds(), 0);
    // the same pack keeping it: the verb gets it
    let f = fx("kept");
    let s = f.open(true);
    let c = person(&s);
    let o = s
        .run_tool(c, ITEMS, &argv(&["state", "show"]), b"")
        .unwrap();
    assert_eq!(o.stdout, b"kept\n");
}

/// The assertion message that stopped `f`, which must be stopped by one.
fn stopped_by(f: impl FnOnce() -> Result<(), String>) -> String {
    let r = catch_unwind(AssertUnwindSafe(f));
    match r {
        Ok(Ok(())) => "not stopped: the row passed".to_owned(),
        Ok(Err(e)) => format!("not stopped: the row failed: {e}"),
        Err(p) => p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default(),
    }
}

/// The planted fault "an outside save skips the hook" (the state is marked in step with the
/// save, unchanged): the kit's compare catches it, loudly, before a verb reads the stale state.
#[test]
fn plant_outside_save_skipped_is_caught() {
    let m = stopped_by(|| {
        with_verb_state_mutant(
            VerbStateMutant::OutsideSaveSkipped,
            row_every_change_source_keeps_the_index,
        )
    });
    assert!(
        m.contains("verb state") && m.contains("not a fresh build"),
        "caught, but not by the verb state's compare: {m}"
    );
}

/// A pack whose hook returns `true` but leaves the state as it was: the kit's compare after
/// the change catches it, loudly, naming the change.
#[test]
fn a_hook_that_keeps_a_stale_state_is_caught() {
    let m = stopped_by(|| {
        STALE_HOOK.with(|p| p.set(true));
        let r = row_every_change_source_keeps_the_index();
        STALE_HOOK.with(|p| p.set(false));
        r
    });
    STALE_HOOK.with(|p| p.set(false));
    assert!(
        m.contains("verb state") && m.contains("not a fresh build"),
        "caught, but not by the verb state's compare: {m}"
    );
}
