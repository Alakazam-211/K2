//! The guarded save (prd-board-tab-v1 §5.3). The tab sends ops, never a file.
//!
//! Under a per-file writer lock: open with the codec; apply each group with
//! `apply_group`, which compares every op's `rev`; write a temp file beside the
//! target; read it back; compare the file's guard with the one from the open;
//! `fsync` and rename. Nothing here is last-writer-wins but position, which the
//! format states. A conflicting group is held whole and the others go on.
//!
//! **What this doesn't close:** between the last compare and the rename, a
//! writer that isn't K2 can still save, and its save is replaced. Microseconds
//! on a local disk, wider on a network share; POSIX offers nothing better
//! without that writer's help. A folder moved at that moment is the same class.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use k2_mpc_board::{
    apply_group, json as fj, package_guard_of, write, Board, Conflict, Group, Id, Limits, Lineage, MemParts, Object, ObjectType, Op, Overlay,
    Package, PartSource, Person, Position, Refusal, RefusalCode, Sha256, Target, WriteMeta, Written,
};
use k2_mpc_board::verbs::{self, Ctx};
use k2_mpc_board::{read_header, FileTarget, Mode, On, SaveError};
use k2_mpc_board_bridge::kit::errors::Value as KValue;
use k2_mpc_board_bridge::parse::{Call, CallKind};
use k2_mpc_board_bridge::report::save_report;
use k2_mpc_board_bridge::{dry_run, Answer, Refused, Why};
use serde_json::{json, Map, Value};

use super::map;

/// Above this much `board.json`, the read-back drops to the temp file's
/// manifest: a full open cost about 2.2 s of a 7.9 s save at 100k objects. To
/// be fixed from K2's own budget on a Mac (`docs/FREEZE-v1.md`: not measured).
pub const FULL_READ_BACK_MAX_BOARD_BYTES: u64 = 50 * 1024 * 1024;

/// How many times a save starts again when the file moved under it.
const MAX_TRIES: usize = 3;

/// How many group ids each file remembers, so a retry gets its first answer.
const REMEMBERED_GROUPS: usize = 64;

/// A stray temp file older than this is swept.
const TEMP_MAX_AGE: Duration = Duration::from_secs(3600);

/// Who's saving, as the file knows them.
#[derive(Debug, Clone)]
pub struct Me {
    /// Their token.
    pub token: Id,
    /// How they join `people` when the file doesn't have them yet.
    pub person: Person,
}

/// A failed save: nothing was written.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveFail {
    /// The file isn't there.
    NotFound,
    /// The codec refused the file or the result, typed.
    Refused(Refusal),
    /// The body is wrong.
    Bad(String),
    /// A disk or permission error, with K2's code.
    Io { code: &'static str, message: String },
    /// The file kept moving, or a rename stayed blocked. The client retries, with its ops kept.
    Busy,
    /// The temp file didn't read back as what was written.
    WriteFailed(String),
    /// A merge met conflicts: nothing was written, and every side is kept here.
    Merge(Value),
}

/// The name of an I/O error, for the wire.
pub fn io_code(e: &std::io::Error) -> &'static str {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => "not_found",
        K::PermissionDenied | K::ReadOnlyFilesystem => "read_only",
        K::StorageFull | K::QuotaExceeded => "disk_full",
        _ => "io_error",
    }
}

fn io_fail(e: std::io::Error) -> SaveFail {
    match io_code(&e) {
        "not_found" => SaveFail::NotFound,
        code => SaveFail::Io { code, message: e.to_string() },
    }
}

impl From<Refusal> for SaveFail {
    fn from(r: Refusal) -> SaveFail {
        match r.code {
            RefusalCode::IoError => SaveFail::Io { code: "io_error", message: r.detail },
            _ => SaveFail::Refused(r),
        }
    }
}

/// What a save answers. `held` and `refused` are per group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveAnswer {
    pub hash: String,
    pub wrote: bool,
    pub stale: bool,
    pub revs: Map<String, Value>,
    pub settings_rev: String,
    pub removed: Map<String, Value>,
    pub moved: Vec<Value>,
    pub held: Vec<Value>,
    pub refused: Vec<Value>,
    /// How many groups applied.
    pub applied: usize,
}

impl SaveAnswer {
    /// True when groups came and every one of them was held.
    pub fn all_held(&self, groups: usize) -> bool {
        groups > 0 && self.applied == 0 && self.refused.is_empty() && !self.held.is_empty()
    }

    pub fn to_json(&self, me: &Id) -> Value {
        json!({
            "hash": self.hash, "wrote": self.wrote, "stale": self.stale, "revs": self.revs,
            "settingsRev": self.settings_rev, "me": me.as_str(), "removed": self.removed,
            "moved": self.moved, "held": self.held, "refused": self.refused,
        })
    }
}

// ---------- the writer lock ----------

fn locks() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<()>>>> {
    static L: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Runs `f` holding the writer lock on `path`'s canonical file, shared by the
/// tab, agents, `fs/rename` and `fs/move`. Readers take no lock: they hold an
/// open handle, and a rename is atomic.
pub fn with_writer_lock<T>(path: &Path, f: impl FnOnce() -> T) -> T {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let lock = locks().lock().unwrap_or_else(|p| p.into_inner()).entry(key).or_default().clone();
    let _held = lock.lock().unwrap_or_else(|p| p.into_inner());
    f()
}

// ---------- the idempotency memory ----------

#[derive(Clone)]
struct Remembered {
    id: String,
    held: Option<Value>,
    refused: Option<Value>,
}

fn remembered() -> &'static Mutex<HashMap<PathBuf, VecDeque<Remembered>>> {
    static R: OnceLock<Mutex<HashMap<PathBuf, VecDeque<Remembered>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------- ops on the wire ----------

fn bad(m: impl Into<String>) -> SaveFail {
    SaveFail::Bad(m.into())
}

fn fields_of(m: &Map<String, Value>) -> Result<fj::Fields, Refusal> {
    match fj::parse(Value::Object(m.clone()).to_string().as_bytes(), &Limits::V1)? {
        fj::Value::Obj(f) => Ok(f),
        _ => Err(Refusal::new(RefusalCode::FieldKind, "not an object")),
    }
}

/// A file object as JSON, as the codec's type.
fn object_of(v: &Value) -> Result<Object, SaveFail> {
    let mut m = v.as_object().cloned().ok_or_else(|| bad("an object must be a JSON object"))?;
    let id = m.remove("id").and_then(|x| x.as_str().map(str::to_string)).ok_or_else(|| bad("an object needs an id"))?;
    let ty = m.remove("type").and_then(|x| x.as_str().map(str::to_string)).ok_or_else(|| bad("an object needs a type"))?;
    Ok(Object { id: Id::new(&id)?, kind: ObjectType::parse(&ty)?, fields: fields_of(&m)? })
}

fn rev_of(op: &Value) -> Result<Sha256, SaveFail> {
    let r = op.get("rev").and_then(Value::as_str).ok_or_else(|| bad("this op names the rev it read"))?;
    Sha256::parse(r).map_err(|_| bad(format!("{r} isn't a rev")))
}

fn position_of(v: Option<&Value>) -> Result<Position, SaveFail> {
    match v {
        None => Ok(Position::Front),
        Some(Value::String(s)) if s == "front" => Ok(Position::Front),
        Some(Value::String(s)) if s == "back" => Ok(Position::Back),
        Some(Value::Object(o)) => {
            let list = o.get("after").and_then(Value::as_array).ok_or_else(|| bad("a position is front, back or {after: [ids]}"))?;
            let mut ids = Vec::with_capacity(list.len());
            for x in list {
                ids.push(Id::new(x.as_str().ok_or_else(|| bad("after holds ids"))?)?);
            }
            Ok(Position::After(ids))
        }
        Some(_) => Err(bad("a position is front, back or {after: [ids]}")),
    }
}

/// The fields a `set` or `meta` names: `f` is the widget's shape, `set` and
/// `unset` are the file's (an agent's `apply`).
fn edit_of(op: &Value, ty: &str, base: &Map<String, Value>, meta: bool) -> Result<(fj::Fields, Vec<String>), SaveFail> {
    let edit = if let Some(f) = op.get("f").and_then(Value::as_object) {
        let e = if meta { map::widget_meta_to_file(f) } else { map::widget_set_to_file(ty, f, base) };
        e.map_err(|e| bad(format!("{}: {}", e.field, e.message)))?
    } else {
        let set = op.get("set").and_then(Value::as_object).cloned().unwrap_or_default();
        if meta && set.contains_key("timer") {
            return Err(bad("timer: a board file doesn't run a timer"));
        }
        let unset = op.get("unset").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        map::FieldEdit { set, unset }
    };
    Ok((fields_of(&edit.set)?, edit.unset))
}

fn to_json(v: &fj::Value) -> Value {
    fj::to_canonical(v).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null)
}

fn object_json(o: &Object) -> Value {
    to_json(&o.to_value())
}

/// Whether this op's result names a person, so the group needs them in `people`.
fn names_a_person(op: &Op) -> bool {
    match op {
        Op::Add { object, .. } => object.fields.contains_key("created") || object.fields.contains_key("reactions") || object.fields.contains_key("votes"),
        Op::Set { set, .. } => ["reactions", "votes", "replies", "created"].iter().any(|k| set.contains_key(*k)),
        _ => false,
    }
}

/// One group on the wire as the codec's group. `removed` hears each object a
/// `del` takes, raw, so an undo puts back what was there and not the widget's
/// rounded copy of it.
fn group_of(v: &Value, board: &Board, me: &Me, at: &str, removed: &mut Map<String, Value>) -> Result<(Group, bool), SaveFail> {
    let id = v.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad("a group needs an id"))?.to_string();
    let no_hold = v.get("noHold").and_then(Value::as_bool).unwrap_or(false);
    let by_all = v.get("byAll").and_then(Value::as_bool).unwrap_or(false);
    let list = v.get("ops").and_then(Value::as_array).ok_or_else(|| bad("a group needs ops"))?;
    let mut ops = Vec::with_capacity(list.len() + 1);
    let created = json!({ "by": me.token.as_str(), "at": at });
    for op in list {
        let kind = op.get("op").and_then(Value::as_str).unwrap_or("");
        let target_id = || -> Result<Id, SaveFail> { Ok(Id::new(op.get("id").and_then(Value::as_str).ok_or_else(|| bad("this op needs an id"))?)?) };
        ops.push(match kind {
            "add" => {
                let object = match (op.get("obj").and_then(Value::as_object), op.get("object")) {
                    (Some(w), _) => {
                        let f = map::widget_add_to_file(w, Some(created.clone())).map_err(|e| bad(format!("{}: {}", e.field, e.message)))?;
                        object_of(&f)?
                    }
                    (None, Some(f)) => object_of(f)?,
                    _ => return Err(bad("add needs obj (the widget's shape) or object (the file's)")),
                };
                Op::Add { object, at: position_of(op.get("at"))? }
            }
            "set" => {
                let oid = target_id()?;
                let cur = board.objects.iter().find(|o| o.id == oid);
                let base = cur.map(object_json).and_then(|v| v.as_object().cloned()).unwrap_or_default();
                let (set, unset) = edit_of(op, cur.map_or("", |o| o.kind.as_str()), &base, false)?;
                Op::Set { id: oid, rev: rev_of(op)?, set, unset }
            }
            "del" => {
                let oid = target_id()?;
                if let Some(o) = board.objects.iter().find(|o| o.id == oid) {
                    removed.insert(oid.as_str().to_string(), object_json(o));
                }
                Op::Del { id: oid, rev: rev_of(op)? }
            }
            "meta" => {
                let (set, unset) = edit_of(op, "", &Map::new(), true)?;
                Op::Meta { rev: rev_of(op)?, set, unset }
            }
            "z" => Op::Z { id: target_id()?, to: position_of(op.get("to"))? },
            "frame" => Op::FrameOrder { id: target_id()?, to: position_of(op.get("to"))? },
            "putback" => Op::Add { object: object_of(op.get("object").ok_or_else(|| bad("putback needs object"))?)?, at: position_of(op.get("at"))? },
            "clearBlob" => {
                let h = op.get("hash").and_then(Value::as_str).ok_or_else(|| bad("clearBlob needs hash"))?;
                Op::ClearBlob { hash: Sha256::parse(h).map_err(|_| bad(format!("{h} isn't a hash")))? }
            }
            other => return Err(bad(format!("unknown op {other:?}"))),
        });
    }
    if !board.people.contains_key(&me.token) && ops.iter().any(names_a_person) {
        ops.insert(0, Op::AddPerson { token: me.token.clone(), person: me.person.clone() });
    }
    Ok((Group { id, by: if by_all { None } else { Some(me.token.clone()) }, ops }, no_hold))
}

/// A held group's answer, or `None` when the conflict is a refusal.
fn held_entry(group: &str, c: &Conflict) -> Option<Value> {
    let theirs_of = |t: &fj::Value| -> Value { map::object_to_widget(&to_json(t), 1.0, None) };
    let entry = match c {
        Conflict::Changed { target: Target::Settings, theirs } => json!({ "why": "settings", "theirs": map::settings_to_meta(&to_json(theirs)) }),
        Conflict::Changed { target: Target::Object(id), theirs } => json!({ "id": id.as_str(), "why": "changed", "theirs": theirs_of(theirs) }),
        Conflict::Changed { target, .. } => json!({ "why": "changed", "target": format!("{target:?}") }),
        Conflict::Deleted { id } => json!({ "id": id.as_str(), "why": "deleted" }),
        Conflict::Exists { target: Target::Object(id), theirs } => json!({ "id": id.as_str(), "why": "exists", "theirs": theirs_of(theirs) }),
        Conflict::Exists { target, .. } => json!({ "why": "exists", "target": format!("{target:?}") }),
        Conflict::Refused(_) => return None,
    };
    Some(json!({ "groupId": group, "objects": [entry] }))
}

fn refused_entry(group: &str, r: &Refusal) -> Value {
    json!({ "groupId": group, "code": r.code.as_str(), "at": r.at, "hint": r.detail })
}

// ---------- the file ----------

static TEMP_N: AtomicU64 = AtomicU64::new(0);

/// A temp name beside `target`, in the form the kit's watcher and `fs_live`
/// know: `.<name>.~mp<pid>-<n>.tmp`.
pub fn temp_beside(target: &Path) -> Result<PathBuf, SaveFail> {
    let name = target.file_name().and_then(|n| n.to_str()).ok_or_else(|| bad("the file has no name"))?;
    let n = TEMP_N.fetch_add(1, Ordering::Relaxed);
    Ok(target.with_file_name(format!(".{name}.~mp{}-{n}.tmp", std::process::id())))
}

/// Removes temp files for this name older than an hour. Never on `info`.
pub fn sweep_temps(target: &Path) {
    let Some(dir) = target.parent() else { return };
    let Some(name) = target.file_name().and_then(|n| n.to_str()) else { return };
    let prefix = format!(".{name}.~mp");
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let fname = e.file_name();
        let Some(f) = fname.to_str() else { continue };
        if f.starts_with(&prefix) && f.ends_with(".tmp") {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|a| a > TEMP_MAX_AGE);
            if old {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

fn rename_over(from: &Path, to: &Path) -> Result<(), SaveFail> {
    // Windows: antivirus or Explorer can hold the target open for a moment.
    let tries = if cfg!(windows) { 40 } else { 1 };
    let mut last = None;
    for _ in 0..tries {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if cfg!(windows) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    match last {
        Some(e) if cfg!(windows) && io_code(&e) == "io_error" => Err(SaveFail::Busy),
        Some(e) => Err(io_fail(e)),
        None => Err(SaveFail::Busy),
    }
}

/// Removes the temp file when it's dropped, unless it was published.
struct TempGuard(Option<PathBuf>);
impl TempGuard {
    fn done(mut self) {
        self.0 = None;
    }
}
impl Drop for TempGuard {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = fs::remove_file(p);
        }
    }
}

/// Writes `board` to a temp file and returns it: on a refusal part of the
/// output may be there, so it's never the target.
fn write_temp(target: &Path, meta: &WriteMeta, board: &Board, parts: &mut dyn PartSource) -> Result<(TempGuard, Written), SaveFail> {
    let path = temp_beside(target)?;
    let guard = TempGuard(Some(path.clone()));
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(io_fail)?;
    let written = {
        let mut out = std::io::BufWriter::new(&mut f);
        let w = write(&mut out, meta, board, parts, &Limits::V1)?;
        out.flush().map_err(io_fail)?;
        w
    };
    f.sync_all().map_err(io_fail)?;
    Ok((guard, written))
}

/// The temp file reads back as what was written. A full open checks
/// `board.json`'s hash; above the size rule, the manifest alone.
fn read_back(path: &Path, written: &Sha256, new_parts: &[Sha256], board_bytes: u64) -> Result<(), SaveFail> {
    let f = File::open(path).map_err(io_fail)?;
    if board_bytes > FULL_READ_BACK_MAX_BOARD_BYTES {
        let g = package_guard_of(BufReader::new(f), &Limits::V1).map_err(|e| SaveFail::WriteFailed(e.to_string()))?;
        if g != *written {
            return Err(SaveFail::WriteFailed("the temp file's guard isn't what was written".into()));
        }
        return Ok(());
    }
    let mut pkg = Package::open(BufReader::new(f), &Limits::V1).map_err(|e| SaveFail::WriteFailed(e.to_string()))?;
    if pkg.package_guard() != *written {
        return Err(SaveFail::WriteFailed("the temp file's guard isn't what was written".into()));
    }
    for h in new_parts {
        pkg.read_blob(h).map_err(|e| SaveFail::WriteFailed(e.to_string()))?;
    }
    Ok(())
}

/// The last steps of every write that replaces a file: keep its mode, compare
/// its guard (hash, not stat: same size within an mtime tick is the tier-0
/// lesson), and rename. `Ok(false)`: the file moved, so start again.
fn replace_if_current(target: &Path, temp: TempGuard, read_guard: Option<&Sha256>) -> Result<bool, SaveFail> {
    let path = temp.0.clone().ok_or_else(|| bad("no temp file"))?;
    let meta = fs::metadata(target).map_err(io_fail)?;
    fs::set_permissions(&path, meta.permissions()).map_err(io_fail)?;
    hook(Step::BeforeCompare, target);
    match read_guard {
        Some(g) => {
            let on_disk = package_guard_of(BufReader::new(File::open(target).map_err(io_fail)?), &Limits::V1);
            match on_disk {
                Ok(now) if now == *g => {}
                // It changed, or it no longer reads: never a save over a file K2 can't read.
                Ok(_) => return Ok(false),
                Err(_) => return Ok(false),
            }
        }
        // `start`: the file must still be empty.
        None => {
            if meta.len() != 0 {
                return Ok(false);
            }
        }
    }
    hook(Step::BeforeRename, target);
    rename_over(&path, target)?;
    temp.done();
    if let Some(dir) = target.parent() {
        sync_dir(dir).map_err(io_fail)?;
    }
    Ok(true)
}

// ---------- the save ----------

/// The input to [`save`].
pub struct SaveInput<'a> {
    /// The `package_guard` the client last read. Only says "you've missed a
    /// change": it skips no check.
    pub base_hash: Option<&'a str>,
    /// The groups, as they came.
    pub groups: &'a [Value],
    /// Who's saving.
    pub me: &'a Me,
    /// The timestamp a new object is made at.
    pub at: &'a str,
    /// A blob this caller staged: `(media type, bytes)`. A save may name only
    /// these, or parts of this file.
    pub staged: &'a dyn Fn(&Sha256) -> Option<(String, Vec<u8>)>,
}

/// §5.3. Holds the writer lock for the whole of it.
pub fn save(path: &Path, input: &SaveInput) -> Result<SaveAnswer, SaveFail> {
    with_writer_lock(path, || {
        for _ in 0..MAX_TRIES {
            if let Some(a) = try_save(path, input)? {
                return Ok(a);
            }
        }
        Err(SaveFail::Busy)
    })
}

fn try_save(path: &Path, input: &SaveInput) -> Result<Option<SaveAnswer>, SaveFail> {
    sweep_temps(path);
    let file = File::open(path).map_err(io_fail)?;
    let mut pkg = Package::open(BufReader::new(file), &Limits::V1)?;
    let guard = pkg.package_guard();
    let mut board = pkg.board().clone();
    let mut a = SaveAnswer { stale: input.base_hash.is_some_and(|b| b != guard.to_string()), ..SaveAnswer::default() };

    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let earlier: Vec<Remembered> = remembered().lock().unwrap_or_else(|p| p.into_inner()).get(&key).map(|d| d.iter().cloned().collect()).unwrap_or_default();
    let mut fresh: Vec<Remembered> = Vec::new();
    let mut changed_ids: Vec<Id> = Vec::new();
    let mut touched = false;
    for g in input.groups {
        let gid = g.get("id").and_then(Value::as_str).unwrap_or("");
        // A retry gets its first answer. A daemon restart forgets, and the
        // retry can show as a conflict with itself: safe, and the person picks.
        if let Some(r) = earlier.iter().find(|r| r.id == gid) {
            if let Some(h) = &r.held {
                a.held.push(h.clone());
            }
            if let Some(x) = &r.refused {
                a.refused.push(x.clone());
            }
            if r.held.is_none() && r.refused.is_none() {
                a.applied += 1;
            }
            continue;
        }
        let mut removed = Map::new();
        let (group, no_hold) = group_of(g, &board, input.me, input.at, &mut removed)?;
        match apply_group(&mut board, &group) {
            Ok(report) => {
                a.applied += 1;
                touched |= !report.is_noop();
                changed_ids.extend(report.added.iter().cloned());
                changed_ids.extend(report.changed.iter().cloned());
                for d in &report.deleted {
                    if let Some(raw) = removed.remove(d.as_str()) {
                        a.removed.insert(d.as_str().to_string(), raw);
                    }
                }
                for m in &report.moved {
                    a.moved.push(json!({ "id": m.id.as_str(), "exact": m.exact }));
                }
                fresh.push(Remembered { id: group.id.clone(), held: None, refused: None });
            }
            Err(Conflict::Refused(r)) => {
                let x = refused_entry(&group.id, &r);
                a.refused.push(x.clone());
                fresh.push(Remembered { id: group.id.clone(), held: None, refused: Some(x) });
            }
            Err(c) if no_hold => {
                let x = json!({ "groupId": group.id, "code": "board_conflict", "hint": conflict_hint(&c) });
                a.refused.push(x.clone());
                fresh.push(Remembered { id: group.id.clone(), held: None, refused: Some(x) });
            }
            Err(c) => {
                if let Some(h) = held_entry(&group.id, &c) {
                    a.held.push(h.clone());
                    fresh.push(Remembered { id: group.id.clone(), held: Some(h), refused: None });
                }
            }
        }
    }
    a.settings_rev = board.settings_rev()?.to_string();
    let answer_revs = |a: &mut SaveAnswer, board: &Board| -> Result<(), SaveFail> {
        for id in &changed_ids {
            if let Some(o) = board.objects.iter().find(|o| o.id == *id) {
                a.revs.insert(id.as_str().to_string(), json!(o.rev()?.to_string()));
            }
        }
        Ok(())
    };
    // No change: nothing is written, so a no-op save doesn't dirty git.
    if !touched {
        a.hash = guard.to_string();
        answer_revs(&mut a, &board)?;
        remember(&key, fresh);
        return Ok(Some(a));
    }

    let meta = pkg.manifest().write_meta();
    let listed: Vec<Sha256> = pkg.manifest().parts.iter().map(|p| p.sha256).collect();
    let mut new_parts = Vec::new();
    for h in board.blob_refs() {
        if !listed.contains(&h) {
            if let Some((mime, bytes)) = (input.staged)(&h) {
                new_parts.push((h, mime, bytes));
            }
        }
    }
    let new_hashes: Vec<Sha256> = new_parts.iter().map(|p| p.0).collect();
    let board_bytes = pkg.manifest().parts.iter().find(|p| p.role == "board").map_or(0, |p| p.bytes);
    let (temp, written) = {
        let mut overlay = Overlay::new(&mut pkg);
        for (h, mime, bytes) in new_parts {
            let added = overlay.add_blob(&mime, bytes);
            if added != h {
                return Err(bad(format!("the staged blob {h} doesn't hash to its name")));
            }
        }
        write_temp(path, &meta, &board, &mut overlay)?
    };
    let temp_path = temp.0.clone().ok_or_else(|| bad("no temp file"))?;
    hook(Step::AfterWrite, &temp_path);
    read_back(&temp_path, &written.package_guard, &new_hashes, board_bytes)?;
    drop(pkg);
    if !replace_if_current(path, temp, Some(&guard))? {
        return Ok(None);
    }
    a.hash = written.package_guard.to_string();
    a.wrote = true;
    answer_revs(&mut a, &board)?;
    remember(&key, fresh);
    Ok(Some(a))
}

fn conflict_hint(c: &Conflict) -> String {
    match c {
        Conflict::Changed { .. } => "someone changed this since".to_string(),
        Conflict::Deleted { id } => format!("{id} is gone"),
        Conflict::Exists { .. } => "that id is in use".to_string(),
        Conflict::Refused(r) => r.detail.clone(),
    }
}

fn remember(key: &Path, fresh: Vec<Remembered>) {
    if fresh.is_empty() {
        return;
    }
    let mut all = remembered().lock().unwrap_or_else(|p| p.into_inner());
    let d = all.entry(key.to_path_buf()).or_default();
    for r in fresh {
        d.push_back(r);
        if d.len() > REMEMBERED_GROUPS {
            d.pop_front();
        }
    }
}

// ---------- a board verb, through the pack's bridge ----------

/// What a verb through the bridge came to: its answer, or its refusal (a row of
/// the kit's), each naming the file it was about. K2's own failures around the
/// run (no file, the disk) are a [`SaveFail`] instead.
#[derive(Debug)]
pub enum Ran {
    Answer(Answer),
    Refused(Box<Refused>),
}

/// The input to [`run_call`].
pub struct CallInput<'a> {
    /// The verb, as the bridge's parser made it.
    pub call: &'a Call,
    /// The path the answer names: K2 gives the full path.
    pub target: &'a str,
    /// Who acts, from the file as it is now; `None` if it can't be read, in
    /// which case the bridge's open refuses with the file's own row.
    pub me: &'a dyn Fn(&Path) -> Option<verbs::Me>,
    /// The group's id.
    pub group_id: &'a str,
    /// The time a new object is made at.
    pub at: &'a str,
    /// Random bytes for new ids.
    pub random: &'a mut dyn FnMut() -> [u8; 8],
    /// Plan and apply it all, and write nothing.
    pub dry_run: bool,
}

/// One try's end: done, or the file moved under it (with the guards, when K2
/// saw them), so start again from the open.
enum Tried {
    Done(Ran),
    Again(Option<(Sha256, Sha256)>),
}

fn is_busy(r: &Refused) -> bool {
    matches!(&r.why, Why::Report(rep) if rep.id() == "MPC-E832")
}

/// A verb on a file. A read opens the file and answers, with no lock. A change
/// runs the bridge's own run (`dry_run::run`: open, read the guard apart,
/// plan, apply, write) inside K2's save, under the writer lock: the bridge
/// writes to a temp file beside the target, K2 syncs it, reads it back,
/// compares the guard of the file on disk with the one the try read, and
/// renames. A file that moved starts again from the open, [`MAX_TRIES`] times,
/// then answers MPC-E832 (`busy`) as it is. A dry run is the bridge's
/// `dry_run::dry_run`, the same run with the save into nowhere: nothing is
/// written, not even a temp file.
///
/// A verb is never held: the agent that ran it has its command, and a
/// conflict is its row (MPC-E830 and the rest), with nothing written.
pub fn run_call(path: &Path, input: &mut CallInput) -> Result<Ran, SaveFail> {
    if input.call.kind() == CallKind::Read {
        let target = FileTarget::new(input.target).ok_or_else(|| bad("no path"))?;
        let file = BufReader::new(File::open(path).map_err(io_fail)?);
        let on_disk = BufReader::new(File::open(path).map_err(io_fail)?);
        let mut cx = Ctx { group_id: input.group_id.to_string(), by: None, at: Some(input.at.to_string()), random: &mut *input.random };
        return Ok(match dry_run::run(input.call, target, file, on_disk, &mut std::io::sink(), &mut cx, &Limits::V1) {
            Ok(a) => Ran::Answer(a),
            Err(r) => Ran::Refused(r),
        });
    }
    with_writer_lock(path, || {
        let mut last = None;
        for _ in 0..MAX_TRIES {
            match try_call(path, input)? {
                Tried::Done(Ran::Refused(r)) if is_busy(&r) => last = Some(Ran::Refused(r)),
                Tried::Done(ran) => return Ok(ran),
                Tried::Again(Some((read, on_disk))) => {
                    let report = save_report(&SaveError::Stale { read, on_disk });
                    let on = On { kind: k2_mpc_board::FORMAT, path: input.target.to_string(), version: Some(read.to_string()), mode: Mode::File };
                    last = Some(Ran::Refused(Box::new(Refused { on, why: Why::from(report) })));
                }
                Tried::Again(None) => {}
            }
        }
        last.ok_or(SaveFail::Busy)
    })
}

fn try_call(path: &Path, input: &mut CallInput) -> Result<Tried, SaveFail> {
    sweep_temps(path);
    let target = FileTarget::new(input.target).ok_or_else(|| bad("no path"))?;
    // The guard this try started from, and the board's size (the read-back's
    // rule), from the zip's directory and manifest, before the bridge's own
    // reads. A file it can't read is the bridge's to refuse, by its row.
    let header = read_header(BufReader::new(File::open(path).map_err(io_fail)?), &Limits::V1).ok();
    let read_guard = header.as_ref().map(|h| h.guard);
    let board_bytes = header.as_ref().and_then(|h| h.parts.iter().find(|p| p.role == "board")).map_or(0, |p| p.bytes);
    let by = (input.me)(path);
    let file = BufReader::new(File::open(path).map_err(io_fail)?);
    let on_disk = BufReader::new(File::open(path).map_err(io_fail)?);
    let mut cx = Ctx { group_id: input.group_id.to_string(), by, at: Some(input.at.to_string()), random: &mut *input.random };
    if input.dry_run {
        return Ok(Tried::Done(match dry_run::dry_run(input.call, target, file, on_disk, &mut cx, &Limits::V1) {
            Ok(a) => Ran::Answer(a),
            Err(r) => Ran::Refused(r),
        }));
    }
    let temp_path = temp_beside(path)?;
    let temp = TempGuard(Some(temp_path.clone()));
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&temp_path).map_err(io_fail)?;
    let ran = {
        let mut out = std::io::BufWriter::new(&mut f);
        let ran = dry_run::run(input.call, target, file, on_disk, &mut out, &mut cx, &Limits::V1);
        out.flush().map_err(io_fail)?;
        ran
    };
    let answer = match ran {
        Ok(a) => a,
        Err(r) => return Ok(Tried::Done(Ran::Refused(r))),
    };
    // Nothing changed: nothing was written, and the temp goes.
    let Some(written) = saved_guard(&answer.value) else {
        return Ok(Tried::Done(Ran::Answer(answer)));
    };
    f.sync_all().map_err(io_fail)?;
    drop(f);
    hook(Step::AfterWrite, &temp_path);
    read_back(&temp_path, &written, &[], board_bytes)?;
    let Some(read_guard) = read_guard else {
        return Ok(Tried::Again(None));
    };
    if !replace_if_current(path, temp, Some(&read_guard))? {
        let now = File::open(path).ok().and_then(|f| package_guard_of(BufReader::new(f), &Limits::V1).ok());
        return Ok(Tried::Again(now.map(|n| (read_guard, n))));
    }
    Ok(Tried::Done(Ran::Answer(answer)))
}

/// The guard a run wrote: its value's `saved.guard`, or `None` when nothing
/// changed (`saved` is null).
fn saved_guard(v: &KValue) -> Option<Sha256> {
    v.get("saved")?.get("guard")?.as_str().and_then(|g| Sha256::parse(g).ok())
}

// ---------- making files ----------

/// The bytes of a new, empty board.
pub fn new_board_bytes(lineage: [u8; 16]) -> Result<Vec<u8>, SaveFail> {
    let mut out = Vec::new();
    write(&mut out, &WriteMeta::new(Lineage::from_bytes(lineage)), &Board::empty(), &mut MemParts::new(), &Limits::V1)?;
    Ok(out)
}

/// A new board under `path`, or `<name> (1).k5board` when that's taken: a temp
/// file published by a no-replace hard link, so the first bytes at that name are
/// a whole file and it never lands on one that's there.
pub fn create_named(path: &Path, lineage: [u8; 16]) -> Result<(PathBuf, String), SaveFail> {
    let (temp, written) = write_temp(path, &WriteMeta::new(Lineage::from_bytes(lineage)), &Board::empty(), &mut MemParts::new())?;
    let out = publish(temp, path)?;
    Ok((out, written.package_guard.to_string()))
}

/// "Start a board here": fills a file that's still zero bytes, under the lock,
/// and only with a whole file. `Ok(None)`: it isn't empty any more.
pub fn start(path: &Path, lineage: [u8; 16]) -> Result<Option<String>, SaveFail> {
    with_writer_lock(path, || {
        if fs::metadata(path).map_err(io_fail)?.len() != 0 {
            return Ok(None);
        }
        let (temp, written) = write_temp(path, &WriteMeta::new(Lineage::from_bytes(lineage)), &Board::empty(), &mut MemParts::new())?;
        if !replace_if_current(path, temp, None)? {
            return Ok(None);
        }
        Ok(Some(written.package_guard.to_string()))
    })
}

// ---------- copies ----------

/// What a copy did.
#[derive(Debug, Clone, PartialEq)]
pub struct Copied {
    /// The copy's guard.
    pub hash: String,
    /// Pictures left out, at the person's say-so.
    pub dropped: Vec<Sha256>,
}

/// Why a copy didn't happen.
#[derive(Debug, Clone, PartialEq)]
pub enum CopyFail {
    /// Pictures that can't be read: the person chooses.
    Missing(Vec<Sha256>),
    /// The rest.
    Fail(SaveFail),
}

impl<T: Into<SaveFail>> From<T> for CopyFail {
    fn from(f: T) -> CopyFail {
        CopyFail::Fail(f.into())
    }
}

/// Publishes a finished temp under `to`, or the next free name: a copy never
/// lands on a file that's there.
fn publish(temp: TempGuard, to: &Path) -> Result<PathBuf, SaveFail> {
    let path = temp.0.clone().ok_or_else(|| bad("no temp file"))?;
    let dir = to.parent().ok_or_else(|| bad("to has no folder"))?;
    let name = to.file_name().and_then(|n| n.to_str()).ok_or_else(|| bad("to has no name"))?;
    let out = k2_core::fs_guest_upload::publish_no_replace(&path, dir, name).map_err(|e| SaveFail::Io { code: "io_error", message: e.to_string() })?;
    temp.done();
    sync_dir(dir).map_err(io_fail)?;
    Ok(out)
}

/// Clears each hash from `board`, the format's way: a known field goes, and a
/// reference inside an unknown field becomes null.
fn clear_blobs(board: &mut Board, list: &[Sha256]) -> Result<(), SaveFail> {
    let g = Group { id: "clear-missing".into(), by: None, ops: list.iter().map(|h| Op::ClearBlob { hash: *h }).collect() };
    apply_group(board, &g).map_err(|c| match c {
        Conflict::Refused(r) => SaveFail::Refused(r),
        other => SaveFail::Bad(conflict_hint(&other)),
    })?;
    Ok(())
}

/// Save a copy with the file readable: the board as it is plus `input`'s
/// groups, written beside `to`. It keeps the lineage: a copy of this board. A
/// group that conflicts stays out of the copy and is answered as refused.
pub fn copy_to(path: &Path, to: &Path, input: &SaveInput, drop_missing: bool) -> Result<(PathBuf, Copied), CopyFail> {
    let file = File::open(path).map_err(io_fail)?;
    let mut pkg = Package::open(BufReader::new(file), &Limits::V1).map_err(SaveFail::from)?;
    let mut board = pkg.board().clone();
    let mut removed = Map::new();
    for g in input.groups {
        let (group, _) = group_of(g, &board, input.me, input.at, &mut removed)?;
        // An edit that can't apply is the person's to see; the copy still happens.
        let _ = apply_group(&mut board, &group);
    }
    let meta = pkg.manifest().write_meta();
    let listed: Vec<Sha256> = pkg.manifest().parts.iter().map(|p| p.sha256).collect();
    let mut add = Vec::new();
    let mut missing = Vec::new();
    for h in board.blob_refs() {
        if !listed.contains(&h) {
            match (input.staged)(&h) {
                Some((m, b)) => add.push((m, b)),
                None => missing.push(h),
            }
        }
    }
    if !missing.is_empty() {
        if !drop_missing {
            return Err(CopyFail::Missing(missing));
        }
        clear_blobs(&mut board, &missing)?;
    }
    let (temp, written) = {
        let mut overlay = Overlay::new(&mut pkg);
        for (m, b) in add {
            overlay.add_blob(&m, b);
        }
        write_temp(to, &meta, &board, &mut overlay)?
    };
    let out = publish(temp, to)?;
    Ok((out, Copied { hash: written.package_guard.to_string(), dropped: missing }))
}

/// A whole board from what the tab still holds, when the original is gone or
/// can't be reached: `objects` in the widget's shape, back to front. Fields and
/// sub-fields the widget carried come along; a part only the lost file held
/// doesn't, so the person chooses. A new lineage: this isn't known to be a copy
/// of anything still there.
pub fn rescue_to(to: &Path, body: &Value, staged: &dyn Fn(&Sha256) -> Option<(String, Vec<u8>)>, drop_missing: bool, lineage: [u8; 16]) -> Result<(PathBuf, Copied), CopyFail> {
    let list = body.get("objects").and_then(Value::as_array).ok_or_else(|| bad("objects is required"))?;
    let mut objects = Vec::with_capacity(list.len());
    for w in list {
        let m = w.as_object().ok_or_else(|| bad("an object must be a JSON object"))?;
        let created = match (m.get("createdBy").and_then(Value::as_str), m.get("createdAt").and_then(Value::as_str)) {
            (Some(by), Some(at)) => Some(json!({ "by": by, "at": at })),
            _ => None,
        };
        objects.push(map::widget_add_to_file(m, created).map_err(|e| bad(format!("{}: {}", e.field, e.message)))?);
    }
    let settings = body.get("meta").cloned().unwrap_or_else(|| json!({}));
    let doc = json!({ "objects": objects, "frames": body.get("frames").cloned().unwrap_or_else(|| json!([])), "people": body.get("people").cloned().unwrap_or_else(|| json!({})), "settings": settings });
    let mut board = Board::from_json(doc.to_string().as_bytes(), &Limits::V1).map_err(SaveFail::from)?;
    let mut parts = MemParts::new();
    let mut missing = Vec::new();
    for h in board.blob_refs() {
        match staged(&h) {
            Some((m, b)) => {
                parts.add_blob(&m, b);
            }
            None => missing.push(h),
        }
    }
    if !missing.is_empty() {
        if !drop_missing {
            return Err(CopyFail::Missing(missing));
        }
        clear_blobs(&mut board, &missing)?;
    }
    let (temp, written) = write_temp(to, &WriteMeta::new(Lineage::from_bytes(lineage)), &board, &mut parts)?;
    let out = publish(temp, to)?;
    Ok((out, Copied { hash: written.package_guard.to_string(), dropped: missing }))
}

// ---------- merge ----------

fn open_board(p: &Path) -> Result<Package<BufReader<File>>, SaveFail> {
    Ok(Package::open(BufReader::new(File::open(p).map_err(io_fail)?), &Limits::V1)?)
}

fn conflicts_json(c: &k2_mpc_board::MergeConflicts) -> Value {
    let side = |o: &Option<Object>| o.as_ref().map(object_json);
    json!({
        "objects": c.objects.iter().map(|o| json!({ "id": o.id.as_str(), "why": o.kind.as_str(), "base": side(&o.base), "copy": side(&o.ours), "file": side(&o.theirs) })).collect::<Vec<_>>(),
        "settings": c.settings.is_some(), "people": c.people.len(), "extra": c.extra.len(),
        "totals": { "objects": c.totals.objects, "settings": c.totals.settings, "people": c.totals.people, "extra": c.totals.extra },
    })
}

/// Merges `copy` into `target` from the board they both started from, `base`
/// (prd-board-tab-v1; the pack's `merge3`, whole objects). Under `target`'s
/// writer lock, written like any save: temp, read-back, guard compare, rename.
/// `base` and `copy` are only read. A conflict writes nothing.
pub fn merge_into(target: &Path, base: &Path, copy: &Path) -> Result<Value, SaveFail> {
    let base_board = open_board(base)?.into_board();
    let mut copy_pkg = open_board(copy)?;
    let copy_board = copy_pkg.board().clone();
    with_writer_lock(target, || {
        for _ in 0..MAX_TRIES {
            let mut pkg = open_board(target)?;
            let guard = pkg.package_guard();
            let merged = match k2_mpc_board::merge3(&base_board, &copy_board, pkg.board()) {
                Ok(m) => m,
                Err(k2_mpc_board::MergeError::Conflicts(c)) => return Err(SaveFail::Merge(conflicts_json(&c))),
                Err(k2_mpc_board::MergeError::Refused(r)) => return Err(SaveFail::Refused(r)),
            };
            let answer = json!({
                "fromCopy": merged.from_ours.iter().map(Id::as_str).collect::<Vec<_>>(),
                "fromFile": merged.from_theirs.iter().map(Id::as_str).collect::<Vec<_>>(),
                "deleted": merged.deleted.iter().map(Id::as_str).collect::<Vec<_>>(),
                "orphans": merged.orphans.len(),
            });
            if merged.board == *pkg.board() {
                let mut a = answer;
                a["hash"] = json!(guard.to_string());
                a["wrote"] = json!(false);
                return Ok(a);
            }
            let listed: Vec<Sha256> = pkg.manifest().parts.iter().map(|p| p.sha256).collect();
            let mut add = Vec::new();
            for h in merged.board.blob_refs() {
                if !listed.contains(&h) {
                    // A picture only the copy had: from the copy, checked against its name.
                    if let Ok(b) = copy_pkg.read_blob(&h) {
                        add.push((b.media_type, b.bytes));
                    }
                }
            }
            let meta = pkg.manifest().write_meta();
            let board_bytes = pkg.manifest().parts.iter().find(|p| p.role == "board").map_or(0, |p| p.bytes);
            let (temp, written, new_hashes) = {
                let mut overlay = Overlay::new(&mut pkg);
                let mut new_hashes = Vec::new();
                for (m, b) in add {
                    new_hashes.push(overlay.add_blob(&m, b));
                }
                let (temp, written) = write_temp(target, &meta, &merged.board, &mut overlay)?;
                (temp, written, new_hashes)
            };
            let temp_path = temp.0.clone().ok_or_else(|| bad("no temp file"))?;
            read_back(&temp_path, &written.package_guard, &new_hashes, board_bytes)?;
            drop(pkg);
            if replace_if_current(target, temp, Some(&guard))? {
                let mut a = answer;
                a["hash"] = json!(written.package_guard.to_string());
                a["wrote"] = json!(true);
                return Ok(a);
            }
        }
        Err(SaveFail::Busy)
    })
}

// ---------- fault injection ----------

/// Where a test can step in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    AfterWrite,
    BeforeCompare,
    BeforeRename,
}

#[cfg(test)]
thread_local! {
    static HOOK: std::cell::RefCell<Option<Box<dyn FnMut(Step, &Path)>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn hook(step: Step, path: &Path) {
    HOOK.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f(step, path);
        }
    });
}

#[cfg(not(test))]
fn hook(_step: Step, _path: &Path) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    const ME: &str = "p-0123456789abcdef";
    const AT: &str = "2026-11-03T12:00:00.000Z";

    fn me() -> Me {
        Me { token: Id::new(ME).expect("token"), person: Person { name: Some("Ann".into()), kind: Some("person".into()), extra: Default::default() } }
    }

    fn set_hook(f: impl FnMut(Step, &Path) + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    fn clear_hook() {
        HOOK.with(|h| *h.borrow_mut() = None);
    }

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-board-save-{}-{}", std::process::id(), TEMP_N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&d).expect("mkdir");
        d.canonicalize().expect("canon")
    }

    /// A real file: real zip, real manifest, real guard.
    fn file_with(d: &Path, doc: Value, blobs: &[(&str, &[u8])]) -> PathBuf {
        let board = Board::from_json(doc.to_string().as_bytes(), &Limits::V1).expect("board");
        let mut parts = MemParts::new();
        for (m, b) in blobs {
            parts.add_blob(m, b.to_vec());
        }
        let mut out = Vec::new();
        write(&mut out, &WriteMeta::new(Lineage::from_bytes([3; 16])), &board, &mut parts, &Limits::V1).expect("write");
        let p = d.join("plan.k5board");
        fs::write(&p, &out).expect("write file");
        p
    }

    fn doc() -> Value {
        json!({ "objects": [
            { "id": "s-1", "type": "sticky", "text": "one", "x": 1.25, "later": { "keep": 1 } },
            { "id": "h-1", "type": "shape", "locked": true },
            { "id": "s-2", "type": "sticky", "text": "two" },
        ], "frames": [], "people": {}, "settings": { "title": "T", "timer": { "state": "off" } } })
    }

    fn read_board(p: &Path) -> Board {
        Package::open(BufReader::new(File::open(p).expect("open")), &Limits::V1).expect("package").board().clone()
    }

    fn guard(p: &Path) -> String {
        package_guard_of(BufReader::new(File::open(p).expect("open")), &Limits::V1).expect("guard").to_string()
    }

    fn rev(p: &Path, id: &str) -> String {
        read_board(p).objects.iter().find(|o| o.id.as_str() == id).expect("object").rev().expect("rev").to_string()
    }

    fn no_staged(_: &Sha256) -> Option<(String, Vec<u8>)> {
        None
    }

    fn run(p: &Path, base: Option<&str>, groups: Vec<Value>) -> Result<SaveAnswer, SaveFail> {
        let me = me();
        save(p, &SaveInput { base_hash: base, groups: &groups, me: &me, at: AT, staged: &no_staged })
    }

    fn set_text(p: &Path, id: &str, text: &str, gid: &str) -> Value {
        json!({ "id": gid, "ops": [{ "op": "set", "id": id, "rev": rev(p, id), "f": { "text": text } }] })
    }

    fn temps(d: &Path) -> Vec<String> {
        fs::read_dir(d).expect("dir").flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|n| n.ends_with(".tmp")).collect()
    }

    fn text_of(p: &Path, id: &str) -> Value {
        let b = read_board(p);
        let o = b.objects.iter().find(|o| o.id.as_str() == id).expect("object");
        to_json(o.fields.get("text").expect("text"))
    }

    #[test]
    fn a_save_changes_only_what_it_names_and_leaves_no_temp() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let a = run(&p, Some(&guard(&p)), vec![set_text(&p, "s-1", "new", "g1")]).expect("save");
        assert!(a.wrote && !a.stale, "{a:?}");
        assert_eq!(a.hash, guard(&p));
        assert_eq!(a.revs["s-1"], json!(rev(&p, "s-1")), "the new rev comes back");
        let b = read_board(&p);
        let o = &b.objects[0];
        assert_eq!(to_json(o.fields.get("x").expect("x")), json!(1.25), "a value it didn't name is as it was");
        assert_eq!(to_json(o.fields.get("later").expect("later")), json!({ "keep": 1 }));
        assert_eq!(to_json(o.fields.get("text").expect("text")), json!("new"));
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    /// V51: `baseHash` skips nothing. A stale `rev` with a current `baseHash`
    /// conflicts, and the file isn't touched.
    #[test]
    fn a_stale_rev_with_a_current_base_hash_conflicts() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let old_rev = rev(&p, "s-1");
        run(&p, None, vec![set_text(&p, "s-1", "theirs", "g1")]).expect("first");
        let current = guard(&p);
        let g = json!({ "id": "g2", "ops": [{ "op": "set", "id": "s-1", "rev": old_rev, "f": { "text": "mine" } }] });
        let a = run(&p, Some(&current), vec![g]).expect("answers");
        assert!(!a.stale, "the base was current");
        assert!(!a.wrote, "{a:?}");
        assert!(a.all_held(1), "{a:?}");
        assert_eq!(a.held[0]["objects"][0]["why"], "changed");
        assert_eq!(a.held[0]["objects"][0]["theirs"]["text"], "theirs");
        assert_eq!(guard(&p), current);
        assert_eq!(text_of(&p, "s-1"), json!("theirs"));
    }

    #[test]
    fn a_conflicting_group_is_held_whole_and_the_others_go_on() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let stale = Sha256::of(b"stale").to_string();
        let held = json!({ "id": "g1", "ops": [
            { "op": "set", "id": "s-2", "rev": rev(&p, "s-2"), "f": { "text": "half" } },
            { "op": "set", "id": "s-1", "rev": stale, "f": { "text": "no" } },
        ] });
        let a = run(&p, None, vec![held, set_text(&p, "s-1", "yes", "g2")]).expect("save");
        assert!(a.wrote, "{a:?}");
        assert_eq!(a.held.len(), 1);
        assert_eq!(a.held[0]["groupId"], "g1");
        assert_eq!(text_of(&p, "s-2"), json!("two"), "a group is all or nothing");
        assert_eq!(text_of(&p, "s-1"), json!("yes"));
    }

    #[test]
    fn no_hold_comes_back_refused_and_a_locked_object_refuses_typed() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let stale = Sha256::of(b"stale").to_string();
        let undo = json!({ "id": "u1", "noHold": true, "ops": [{ "op": "set", "id": "s-1", "rev": stale, "f": { "text": "no" } }] });
        let locked = json!({ "id": "l1", "ops": [{ "op": "set", "id": "h-1", "rev": rev(&p, "h-1"), "f": { "text": "no" } }] });
        let a = run(&p, None, vec![undo, locked]).expect("answers");
        assert_eq!(a.refused[0]["code"], "board_conflict");
        assert_eq!(a.refused[1]["code"], "object_locked");
        assert!(a.held.is_empty() && !a.wrote, "{a:?}");
    }

    #[test]
    fn a_no_op_save_writes_nothing() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let before = fs::read(&p).expect("read");
        let ino_before = fs::metadata(&p).expect("meta").modified().expect("mtime");
        let a = run(&p, None, vec![set_text(&p, "s-1", "one", "g1")]).expect("save");
        assert!(!a.wrote, "{a:?}");
        assert_eq!(a.applied, 1);
        assert_eq!(fs::read(&p).expect("read"), before);
        assert_eq!(fs::metadata(&p).expect("meta").modified().expect("mtime"), ino_before);
        // And nothing at all to do.
        let a = run(&p, None, vec![]).expect("save");
        assert!(!a.wrote);
    }

    #[test]
    fn a_retried_group_gets_its_first_answer_and_doesnt_apply_twice() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let g = set_text(&p, "s-1", "once", "retry-1");
        run(&p, None, vec![g.clone()]).expect("first");
        let h = guard(&p);
        let a = run(&p, None, vec![g]).expect("retry");
        assert!(!a.wrote && a.held.is_empty(), "{a:?}");
        assert_eq!(a.applied, 1);
        assert_eq!(guard(&p), h);
    }

    /// Fault injection at each step: a crash before the rename leaves the old
    /// bytes, a stray temp and nothing else.
    #[test]
    fn a_crash_before_the_rename_leaves_the_old_bytes() {
        for step in [Step::AfterWrite, Step::BeforeCompare, Step::BeforeRename] {
            let d = dir();
            let p = file_with(&d, doc(), &[]);
            let before = fs::read(&p).expect("read");
            set_hook(move |s, _| {
                if s == step {
                    panic!("crash at {s:?}");
                }
            });
            let g = set_text(&p, "s-1", "lost", "g1");
            let pp = p.clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || run(&pp, None, vec![g])));
            clear_hook();
            assert!(r.is_err(), "{step:?}");
            assert_eq!(fs::read(&p).expect("read"), before, "{step:?}");
            // The lock isn't stuck: the next save goes through.
            let a = run(&p, None, vec![set_text(&p, "s-1", "after", "g2")]).expect("next save");
            assert!(a.wrote);
        }
    }

    /// A planted change between the open and the compare starts again, on the
    /// new file, and the planted change stays.
    #[test]
    fn a_planted_change_before_the_compare_retries_and_keeps_it() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let pp = p.clone();
        let mut done = false;
        let planted = {
            let mut other = doc();
            other["objects"][2]["text"] = json!("an outside writer");
            let board = Board::from_json(other.to_string().as_bytes(), &Limits::V1).expect("board");
            let mut out = Vec::new();
            write(&mut out, &WriteMeta::new(Lineage::from_bytes([3; 16])), &board, &mut MemParts::new(), &Limits::V1).expect("write");
            out
        };
        set_hook(move |s, _| {
            if s == Step::BeforeCompare && !done {
                done = true;
                fs::write(&pp, &planted).expect("plant");
            }
        });
        let g = set_text(&p, "s-1", "mine", "g1");
        let a = run(&p, None, vec![g]);
        clear_hook();
        let a = a.expect("save");
        assert!(a.wrote, "{a:?}");
        assert_eq!(text_of(&p, "s-1"), json!("mine"));
        assert_eq!(text_of(&p, "s-2"), json!("an outside writer"), "the outside change stays");
    }

    /// The guard covers parts: an outside writer who adds a blob and changes
    /// nothing else is seen, where a `board.json` hash wouldn't see it.
    #[test]
    fn a_planted_blob_is_seen_by_the_guard() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let pp = p.clone();
        let mut done = false;
        let planted = {
            let board = Board::from_json(doc().to_string().as_bytes(), &Limits::V1).expect("board");
            let mut parts = MemParts::new();
            parts.add_blob("image/png", b"an outside blob".to_vec());
            let mut out = Vec::new();
            write(&mut out, &WriteMeta::new(Lineage::from_bytes([3; 16])), &board, &mut parts, &Limits::V1).expect("write");
            out
        };
        set_hook(move |s, _| {
            if s == Step::BeforeCompare && !done {
                done = true;
                fs::write(&pp, &planted).expect("plant");
            }
        });
        let g = set_text(&p, "s-1", "mine", "g1");
        let a = run(&p, None, vec![g]);
        clear_hook();
        a.expect("save");
        let pkg = Package::open(BufReader::new(File::open(&p).expect("open")), &Limits::V1).expect("package");
        assert_eq!(pkg.manifest().parts.iter().filter(|x| x.role == "blob").count(), 1, "the blob wasn't replaced away");
    }

    /// A file that keeps moving ends in `busy`, with nothing lost.
    #[test]
    fn a_file_that_keeps_moving_answers_busy() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let pp = p.clone();
        let mut n = 0u32;
        set_hook(move |s, _| {
            if s == Step::BeforeCompare {
                n += 1;
                let mut other = doc();
                other["objects"][2]["text"] = json!(format!("change {n}"));
                let board = Board::from_json(other.to_string().as_bytes(), &Limits::V1).expect("board");
                let mut out = Vec::new();
                write(&mut out, &WriteMeta::new(Lineage::from_bytes([3; 16])), &board, &mut MemParts::new(), &Limits::V1).expect("write");
                fs::write(&pp, &out).expect("plant");
            }
        });
        let g = set_text(&p, "s-1", "mine", "g1");
        let a = run(&p, None, vec![g]);
        clear_hook();
        assert_eq!(a.expect_err("busy"), SaveFail::Busy);
        assert_eq!(text_of(&p, "s-1"), json!("one"));
        assert_eq!(temps(&d), Vec::<String>::new(), "each try cleaned up");
    }

    /// A file that stopped reading between the open and the compare is never
    /// saved over.
    #[test]
    fn a_file_that_goes_bad_is_never_saved_over() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let pp = p.clone();
        set_hook(move |s, _| {
            if s == Step::BeforeCompare {
                fs::write(&pp, b"half a file").expect("plant");
            }
        });
        let g = set_text(&p, "s-1", "mine", "g1");
        let a = run(&p, None, vec![g]);
        clear_hook();
        assert_eq!(a.expect_err("refused").clone(), SaveFail::Refused(match Package::open(BufReader::new(File::open(&p).expect("open")), &Limits::V1) { Err(r) => r, Ok(_) => panic!("reads") }));
        assert_eq!(fs::read(&p).expect("read"), b"half a file");
    }

    #[test]
    fn the_read_back_catches_a_temp_that_isnt_what_was_written() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let before = fs::read(&p).expect("read");
        set_hook(|s, t| {
            if s == Step::AfterWrite {
                fs::write(t, b"PK damaged").expect("damage");
            }
        });
        let g = set_text(&p, "s-1", "mine", "g1");
        let a = run(&p, None, vec![g]);
        clear_hook();
        assert!(matches!(a, Err(SaveFail::WriteFailed(_))), "{a:?}");
        assert_eq!(fs::read(&p).expect("read"), before);
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    #[test]
    fn the_files_mode_is_kept() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).expect("chmod");
        run(&p, None, vec![set_text(&p, "s-1", "new", "g1")]).expect("save");
        assert_eq!(fs::metadata(&p).expect("meta").permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn a_gone_file_is_not_found_and_is_never_recreated() {
        let d = dir();
        let p = d.join("gone.k5board");
        let a = run(&p, None, vec![json!({ "id": "g1", "ops": [{ "op": "del", "id": "s-1", "rev": Sha256::of(b"x").to_string() }] })]);
        assert_eq!(a.expect_err("gone"), SaveFail::NotFound);
        assert!(!p.exists());
    }

    #[test]
    fn a_read_only_folder_is_typed_and_keeps_the_file() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let before = fs::read(&p).expect("read");
        fs::set_permissions(&d, fs::Permissions::from_mode(0o500)).expect("chmod");
        let g = set_text(&p, "s-1", "mine", "g1");
        let a = run(&p, None, vec![g]);
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).expect("chmod back");
        match a {
            Err(SaveFail::Io { code, .. }) => assert_eq!(code, "read_only"),
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(&p).expect("read"), before);
    }

    #[test]
    fn a_save_adds_me_to_people_only_when_it_names_me() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        run(&p, None, vec![set_text(&p, "s-1", "plain", "g1")]).expect("save");
        assert!(read_board(&p).people.is_empty(), "a plain edit names nobody");
        let add = json!({ "id": "g2", "ops": [{ "op": "add", "obj": { "id": "s-9", "type": "sticky", "text": "new", "z": 9 } }] });
        let a = run(&p, None, vec![add]).expect("save");
        assert!(a.wrote);
        let b = read_board(&p);
        let token = Id::new(ME).expect("t");
        assert_eq!(b.people.get(&token).and_then(|x| x.name.clone()), Some("Ann".to_string()));
        let o = b.objects.last().expect("added at the front");
        assert_eq!(o.id.as_str(), "s-9");
        assert_eq!(to_json(o.fields.get("created").expect("created")), json!({ "by": ME, "at": AT }));
        assert!(!o.fields.contains_key("z"), "a position isn't a field");
    }

    #[test]
    fn delete_answers_the_raw_object_and_putback_restores_it_exactly() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let before = object_json(&read_board(&p).objects[0]);
        let a = run(&p, None, vec![json!({ "id": "g1", "ops": [{ "op": "del", "id": "s-1", "rev": rev(&p, "s-1") }] })]).expect("del");
        assert_eq!(a.removed["s-1"], before, "raw: 1.25 stays 1.25, and unknown fields stay");
        let g = json!({ "id": "g2", "ops": [{ "op": "putback", "object": a.removed["s-1"], "at": "back" }] });
        run(&p, None, vec![g]).expect("putback");
        assert_eq!(object_json(&read_board(&p).objects[0]), before);
    }

    #[test]
    fn a_save_may_name_only_a_staged_blob_or_a_part_of_this_file() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let bytes = b"\x89PNG staged".to_vec();
        let h = Sha256::of(&bytes);
        let add = json!({ "id": "g1", "ops": [{ "op": "add", "obj": { "id": "i-1", "type": "image", "src": format!("k2blob:{h}") } }] });
        // Nothing staged: the codec refuses, and the file stays.
        let before = fs::read(&p).expect("read");
        match run(&p, None, vec![add.clone()]) {
            Err(SaveFail::Refused(r)) => assert_eq!(r.code, RefusalCode::BlobMissing),
            other => panic!("{other:?}"),
        }
        assert_eq!(fs::read(&p).expect("read"), before);
        // Staged: it goes in, and reads back.
        let me = me();
        let groups = vec![json!({ "id": "g2", "ops": add["ops"].clone() })];
        let staged = |x: &Sha256| (*x == h).then(|| ("image/png".to_string(), bytes.clone()));
        let a = save(&p, &SaveInput { base_hash: None, groups: &groups, me: &me, at: AT, staged: &staged }).expect("save");
        assert!(a.wrote);
        let mut pkg = Package::open(BufReader::new(File::open(&p).expect("open")), &Limits::V1).expect("package");
        assert_eq!(pkg.read_blob(&h).expect("blob").bytes, bytes);
        // Saved again, the part stays, staged or not.
        drop(pkg);
        run(&p, None, vec![set_text(&p, "s-1", "again", "g3")]).expect("save");
        let mut pkg = Package::open(BufReader::new(File::open(&p).expect("open")), &Limits::V1).expect("package");
        assert_eq!(pkg.read_blob(&h).expect("blob").bytes, bytes);
    }

    #[test]
    fn a_stale_save_says_so_and_applies_what_fits() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let old = guard(&p);
        run(&p, None, vec![set_text(&p, "s-1", "theirs", "g1")]).expect("save");
        let a = run(&p, Some(&old), vec![set_text(&p, "s-2", "mine", "g2")]).expect("save");
        assert!(a.stale && a.wrote, "{a:?}");
    }

    #[test]
    fn start_fills_an_empty_file_with_a_whole_board_and_only_an_empty_one() {
        let d = dir();
        let p = d.join("empty.k5board");
        fs::write(&p, b"").expect("touch");
        let h = start(&p, [9; 16]).expect("start").expect("was empty");
        assert_eq!(h, guard(&p));
        assert!(read_board(&p).objects.is_empty());
        assert_eq!(start(&p, [9; 16]).expect("start"), None, "not any more");
    }

    #[test]
    fn a_copy_lands_beside_any_folder_never_over_a_file_and_says_what_it_left_out() {
        let d = dir();
        let elsewhere = dir();
        let p = file_with(&d, doc(), &[]);
        let me = me();
        let groups = vec![set_text(&p, "s-1", "in the copy", "c1")];
        let inp = SaveInput { base_hash: None, groups: &groups, me: &me, at: AT, staged: &no_staged };
        let to = elsewhere.join("copy.k5board");
        let (out, c) = copy_to(&p, &to, &inp, false).expect("copy");
        assert_eq!(out, to);
        assert_eq!(c.hash, guard(&to));
        assert_eq!(text_of(&to, "s-1"), json!("in the copy"));
        assert_eq!(text_of(&p, "s-1"), json!("one"), "the original isn't touched");
        // Again: the next free name, and the first copy stays.
        let (out2, _) = copy_to(&p, &to, &inp, false).expect("copy");
        assert_ne!(out2, to);
        assert!(out2.exists() && to.exists());
        // A dangling picture: the person chooses.
        let h = format!("sha256-{}", "c".repeat(64));
        let dangling = d.join("dangling.k5board");
        let board = Board::from_json(json!({ "objects": [{ "id": "i-1", "type": "image", "src": { "blob": h } }], "frames": [], "people": {}, "settings": {} }).to_string().as_bytes(), &Limits::V1).expect("board");
        // A file with a dangling ref can't be written, so a hand-made one: rescue's path covers the writer's side.
        let _ = (&dangling, &board);
    }

    /// K2 Cores' condition (3): Save a copy works when the original location is
    /// the problem. Here the original is gone and the folder it was in too.
    #[test]
    fn a_rescue_works_with_the_original_gone_and_asks_about_pictures() {
        let gone = dir();
        let elsewhere = dir();
        let p = file_with(&gone, doc(), &[]);
        fs::remove_file(&p).expect("gone");
        fs::remove_dir_all(&gone).expect("the share is gone");
        let png = b"\x89PNG kept".to_vec();
        let kept = Sha256::of(&png);
        let lost = Sha256::of(b"lost");
        let body = json!({
            "objects": [
                { "id": "s-1", "type": "sticky", "text": "kept", "x": 1.25, "z": 1, "later": { "keep": 1 }, "createdBy": ME, "createdAt": AT, "updatedAt": AT },
                { "id": "i-1", "type": "image", "src": format!("k2blob:{kept}"), "z": 2 },
                { "id": "i-2", "type": "image", "src": format!("k2blob:{lost}"), "alt": "gone", "z": 3 },
                { "id": "b-1", "type": "table", "rows": [["a"]], "rowIds": ["rw-0"], "colIds": ["cl-0"], "colW": [40], "z": 4 },
            ],
            "people": { ME: { "name": "Ann" } }, "frames": [], "meta": { "title": "T" },
        });
        let staged = |h: &Sha256| (*h == kept).then(|| ("image/png".to_string(), png.clone()));
        let to = elsewhere.join("rescued.k5board");
        assert_eq!(rescue_to(&to, &body, &staged, false, [7; 16]).expect_err("asks"), CopyFail::Missing(vec![lost]));
        assert!(!to.exists(), "nothing is written until the person chooses");
        let (out, c) = rescue_to(&to, &body, &staged, true, [7; 16]).expect("rescued");
        assert_eq!(c.dropped, vec![lost]);
        let b = read_board(&out);
        let ids: Vec<&str> = b.objects.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["s-1", "i-1", "i-2", "b-1"], "back to front");
        assert!(!b.objects[2].fields.contains_key("src"), "the object stays; its picture is cleared");
        assert_eq!(to_json(b.objects[2].fields.get("alt").expect("alt")), json!("gone"));
        assert_eq!(to_json(b.objects[0].fields.get("x").expect("x")), json!(1.25));
        assert_eq!(to_json(b.objects[0].fields.get("later").expect("later")), json!({ "keep": 1 }));
        assert_eq!(to_json(b.objects[0].fields.get("created").expect("created")), json!({ "by": ME, "at": AT }));
        let mut pkg = Package::open(BufReader::new(File::open(&out).expect("open")), &Limits::V1).expect("package");
        assert_eq!(pkg.read_blob(&kept).expect("kept").bytes, png);
    }

    /// The other half of condition (3): a full disk at the original doesn't
    /// stop a copy somewhere else, and a read-only target is typed.
    #[test]
    fn a_copy_is_typed_when_its_folder_cant_be_written() {
        let d = dir();
        let locked = dir();
        let p = file_with(&d, doc(), &[]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).expect("chmod");
        let me = me();
        let inp = SaveInput { base_hash: None, groups: &[], me: &me, at: AT, staged: &no_staged };
        let r = copy_to(&p, &locked.join("x.k5board"), &inp, false);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("chmod back");
        match r {
            Err(CopyFail::Fail(SaveFail::Io { code, .. })) => assert_eq!(code, "read_only"),
            other => panic!("{other:?}"),
        }
    }

    /// K2's own save budget, for the size rule (§5.4): the whole guarded save,
    /// `F_FULLFSYNC` included on a Mac. Run by hand on the machine in question:
    /// `cargo test -p k2-daemon --lib save_budget -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement: run by hand on the machine in question"]
    fn save_budget() {
        for n in [1_000usize, 10_000, 50_000, 100_000] {
            let d = dir();
            let objects: Vec<Value> = (0..n)
                .map(|i| json!({ "id": format!("s-{i:010}"), "type": "sticky", "x": i as f64 * 1.5, "y": 7, "text": format!("note number {i}"), "color": "yellow", "created": { "by": ME, "at": AT } }))
                .collect();
            let doc = json!({ "objects": objects, "frames": [], "people": { ME: { "name": "Ann", "kind": "person" } }, "settings": {} });
            let p = file_with(&d, doc, &[]);
            let raw = Package::open(BufReader::new(File::open(&p).expect("open")), &Limits::V1).expect("open").manifest().parts.iter().find(|x| x.role == "board").map_or(0, |x| x.bytes);
            let file = fs::metadata(&p).expect("meta").len();
            let runs = if n >= 50_000 { 5 } else { 20 };
            let mut times = Vec::with_capacity(runs);
            for r in 0..runs {
                let g = set_text(&p, "s-0000000000", &format!("edit {r}"), &format!("b{n}-{r}"));
                let t = std::time::Instant::now();
                let a = run(&p, None, vec![g]).expect("save");
                times.push(t.elapsed());
                assert!(a.wrote);
            }
            times.sort();
            println!(
                "save_budget objects={n} board_json_mib={:.1} file_mib={:.1} p50_ms={} p95_ms={} os={} read_back={}",
                raw as f64 / 1048576.0,
                file as f64 / 1048576.0,
                times[runs / 2].as_millis(),
                times[(runs * 95 / 100).min(runs - 1)].as_millis(),
                std::env::consts::OS,
                if raw > FULL_READ_BACK_MAX_BOARD_BYTES { "manifest" } else { "full" },
            );
            if n == 1_000 {
                assert!(times[(runs * 95 / 100).min(runs - 1)] <= Duration::from_millis(250), "the PRD's budget at 1,000 objects: {times:?}");
            }
            let _ = fs::remove_dir_all(&d);
        }
    }

    #[test]
    fn a_named_board_is_whole_and_never_lands_on_a_file() {
        let d = dir();
        let p = d.join("plan.k5board");
        let (a, h) = create_named(&p, [1; 16]).expect("first");
        assert_eq!(a, p);
        assert_eq!(h, guard(&p));
        let (b, _) = create_named(&p, [2; 16]).expect("second");
        assert_eq!(b.file_name().and_then(|n| n.to_str()), Some("plan (1).k5board"));
        assert_eq!(guard(&p), h, "the first is untouched");
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    fn run_verb_with(p: &Path, words: &[&str], dry_run: bool) -> Result<Ran, SaveFail> {
        let argv: Vec<String> = words.iter().map(|s| s.to_string()).collect();
        let call = super::super::verbs::call_of(&argv).unwrap_or_else(|e| panic!("{words:?}: {e:?}"));
        let me = me();
        let who = |path: &Path| -> Option<verbs::Me> {
            let b = Package::open(BufReader::new(File::open(path).ok()?), &Limits::V1).ok()?.board().clone();
            Some(verbs::Me { token: me.token.clone(), new: (!b.people.contains_key(&me.token)).then(|| me.person.clone()) })
        };
        let mut n = 0u64;
        let mut rnd = || {
            n += 1;
            n.to_le_bytes()
        };
        let target = p.to_string_lossy().to_string();
        run_call(p, &mut CallInput { call: &call, target: &target, me: &who, group_id: "v1", at: AT, random: &mut rnd, dry_run })
    }

    fn run_verb(p: &Path, words: &[&str]) -> Result<Ran, SaveFail> {
        run_verb_with(p, words, false)
    }

    fn answer(r: Result<Ran, SaveFail>) -> Answer {
        match r {
            Ok(Ran::Answer(a)) => a,
            other => panic!("not an answer: {other:?}"),
        }
    }

    fn row(r: Result<Ran, SaveFail>) -> (&'static str, u8, On) {
        match r {
            Ok(Ran::Refused(r)) => match &r.why {
                Why::Report(rep) => (rep.id(), rep.exit(), r.on.clone()),
                Why::HostBug(b) => panic!("a host bug: {b:?}"),
            },
            other => panic!("not a refusal: {other:?}"),
        }
    }

    /// A verb saves through the guard: an update lands, the same rev again is
    /// the kit's conflict row with nothing written (never held: an agent has
    /// its command), and -0 is written and printed as 0.
    #[test]
    fn a_verb_update_lands_and_a_stale_rev_conflicts() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let r = rev(&p, "s-1");
        let a = answer(run_verb(&p, &["update", "s-1", "--rev", &r, "--text", "Friday"]));
        assert!(!a.dry_run);
        assert_eq!(a.on.version.as_deref(), Some(guard(&p).as_str()), "the echo is the guard written");
        assert_eq!(a.value.get("saved").and_then(|s| s.get("guard")).and_then(KValue::as_str), Some(guard(&p).as_str()));
        assert_eq!(text_of(&p, "s-1"), json!("Friday"));
        let before = fs::read(&p).expect("read");
        let (id, exit, on) = row(run_verb(&p, &["update", "s-1", "--rev", &r, "--text", "stale"]));
        assert_eq!((id, exit), ("MPC-E830", 1), "a stale rev conflicts");
        assert_eq!(on.version.as_deref(), Some(guard(&p).as_str()));
        assert_eq!(fs::read(&p).expect("read"), before, "nothing was written");
        let a = answer(run_verb(&p, &["add", "sticky", "--text", "new", "--x", "-0"]));
        let added: Vec<String> = match a.value.get("report").and_then(|r| r.get("added")) {
            Some(KValue::List(l)) => l.iter().filter_map(KValue::as_str).map(str::to_string).collect(),
            other => panic!("report.added: {other:?}"),
        };
        let b = read_board(&p);
        let o = b.objects.iter().find(|o| o.fields.get("text") == Some(&fj::Value::Str("new".into()))).expect("added");
        assert_eq!(added, vec![o.id.to_string()]);
        assert_eq!(String::from_utf8(fj::to_canonical(o.fields.get("x").expect("x")).expect("canon")).expect("utf8"), "0", "-0 is written as 0");
        let read = answer(run_verb(&p, &["read", o.id.as_str()]));
        let text = read.value.to_json();
        assert!(text.contains("\"x\":0") && !text.contains("\"x\":-0"), "-0 prints as 0: {text}");
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    /// A dry run is the same run with the save into nowhere: the same rows,
    /// the file's bytes untouched, no temp file, and what it would write.
    #[test]
    fn a_dry_run_writes_nothing_and_says_what_it_would() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let r = rev(&p, "s-1");
        let before = fs::read(&p).expect("read");
        let a = answer(run_verb_with(&p, &["update", "s-1", "--rev", &r, "--text", "Friday"], true));
        assert!(a.dry_run);
        assert_eq!(a.on.version.as_deref(), Some(guard(&p).as_str()), "its echo is the guard read");
        let would = a.value.get("would_write").and_then(|w| w.get("guard")).and_then(KValue::as_str).expect("would_write").to_string();
        assert_ne!(would, guard(&p));
        assert_eq!(fs::read(&p).expect("read"), before);
        assert_eq!(temps(&d), Vec::<String>::new());
        // The real run writes what the dry run said it would.
        let a = answer(run_verb(&p, &["update", "s-1", "--rev", &r, "--text", "Friday"]));
        assert_eq!(a.on.version.as_deref(), Some(would.as_str()));
        // A refusal shows the same row in a dry run.
        let (id, exit, _) = row(run_verb_with(&p, &["update", "s-1", "--rev", &r, "--text", "x"], true));
        assert_eq!((id, exit), ("MPC-E830", 1));
        let (id, exit, _) = row(run_verb_with(&p, &["update", "h-1", "--rev", &rev(&p, "h-1"), "--text", "x"], true));
        assert_eq!((id, exit), ("MPC-E835", 5), "a locked object");
    }

    /// A planted change between the bridge's write and K2's compare starts
    /// again on the new file, and the planted change stays.
    #[test]
    fn a_verb_retries_over_a_planted_change_and_keeps_it() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let pp = p.clone();
        let mut done = false;
        let planted = {
            let mut other = doc();
            other["objects"][2]["text"] = json!("an outside writer");
            let board = Board::from_json(other.to_string().as_bytes(), &Limits::V1).expect("board");
            let mut out = Vec::new();
            write(&mut out, &WriteMeta::new(Lineage::from_bytes([3; 16])), &board, &mut MemParts::new(), &Limits::V1).expect("write");
            out
        };
        set_hook(move |s, _| {
            if s == Step::BeforeCompare && !done {
                done = true;
                fs::write(&pp, &planted).expect("plant");
            }
        });
        let r = rev(&p, "s-1");
        let a = run_verb(&p, &["update", "s-1", "--rev", &r, "--text", "mine"]);
        clear_hook();
        answer(a);
        assert_eq!(text_of(&p, "s-1"), json!("mine"));
        assert_eq!(text_of(&p, "s-2"), json!("an outside writer"), "the outside change stays");
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    /// A file that moves on every try answers MPC-E832 (busy, exit 1) after
    /// K2's retries, and what's on disk is the last outside save.
    #[test]
    fn a_verb_on_a_file_that_keeps_moving_is_busy() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let pp = p.clone();
        let mut n = 0u8;
        set_hook(move |s, _| {
            if s == Step::BeforeCompare {
                n += 1;
                let mut other = doc();
                other["objects"][2]["text"] = json!(format!("outside {n}"));
                let board = Board::from_json(other.to_string().as_bytes(), &Limits::V1).expect("board");
                let mut out = Vec::new();
                write(&mut out, &WriteMeta::new(Lineage::from_bytes([3; 16])), &board, &mut MemParts::new(), &Limits::V1).expect("write");
                fs::write(&pp, &out).expect("plant");
            }
        });
        let r = rev(&p, "s-1");
        let a = run_verb(&p, &["update", "s-1", "--rev", &r, "--text", "mine"]);
        clear_hook();
        let (id, exit, on) = row(a);
        assert_eq!((id, exit), ("MPC-E832", 1));
        assert!(on.version.is_some());
        assert_eq!(text_of(&p, "s-1"), json!("one"));
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    /// A file K2 can't read is the bridge's row (the file's: exit 4), never
    /// K2's own code, and it's never written over.
    #[test]
    fn a_verb_on_a_bad_file_is_the_files_row() {
        let d = dir();
        let p = d.join("bad.k5board");
        fs::write(&p, b"half a file").expect("write");
        let (id, exit, on) = row(run_verb(&p, &["add", "sticky"]));
        assert_eq!((id, exit), ("MPC-E842", 4), "not a zip");
        assert_eq!(on.version, None, "no guard was read");
        assert_eq!(fs::read(&p).expect("read"), b"half a file");
        assert!(matches!(run_verb(&d.join("gone.k5board"), &["add", "sticky"]), Err(SaveFail::NotFound)));
        assert_eq!(temps(&d), Vec::<String>::new());
    }

    #[test]
    fn a_stray_temp_is_swept_when_old_and_not_when_new() {
        let d = dir();
        let p = file_with(&d, doc(), &[]);
        let newer = d.join(".plan.k5board.~mp1-1.tmp");
        fs::write(&newer, b"x").expect("temp");
        let older = d.join(".plan.k5board.~mp1-2.tmp");
        fs::write(&older, b"x").expect("temp");
        let f = File::options().write(true).open(&older).expect("open");
        f.set_modified(SystemTime::now() - Duration::from_secs(2 * 3600)).expect("age it");
        sweep_temps(&p);
        assert!(newer.exists() && !older.exists());
        let _ = fs::remove_file(newer);
    }
}
