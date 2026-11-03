//! Board ops (add, set, del, meta) checked and turned into key writes,
//! the delete cascade, `apply`'s id mapping, and the read helpers. Pure:
//! the daemon reads the board's keys into a [`Board`], calls [`plan`], and
//! writes the result with each key's `ifRev` (all or nothing).

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Map, Value};

use super::schema::{self, clean, clean_meta, normalize, table_ids, upgrade_object};
use super::{meta_key, obj_key, BoardError, FORMAT};

/// One stored key.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub key: String,
    pub rev: i64,
    pub value: Value,
}

/// One board as read from the store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Board {
    pub id: String,
    /// Object id → its key.
    pub objects: BTreeMap<String, Stored>,
    pub meta: Option<Stored>,
}

impl Board {
    /// A board from its keys under `board/<b>/` (other keys are ignored;
    /// an object is known by the `id` in its value).
    pub fn from_entries(id: &str, entries: impl IntoIterator<Item = Stored>) -> Board {
        let mut b = Board { id: id.to_string(), ..Board::default() };
        let mk = meta_key(id);
        let obj_prefix = format!("{}obj/", super::board_prefix(id));
        for e in entries {
            if e.key == mk {
                b.meta = Some(e);
            } else if let Some(tail) = e.key.strip_prefix(&obj_prefix).map(str::to_string) {
                let mut e = e;
                // An older object known only by its key.
                if e.value.get("id").is_none_or(Value::is_null) && schema::valid_item_id(&tail) {
                    if let Some(o) = e.value.as_object_mut() {
                        o.insert("id".into(), json!(tail));
                    }
                }
                if let Some(oid) = e.value.get("id").and_then(Value::as_str).map(str::to_string) {
                    if e.key == obj_key(id, &oid) {
                        b.objects.insert(oid, e);
                    }
                }
            }
        }
        b
    }

    pub fn object(&self, id: &str) -> Option<&Map<String, Value>> {
        self.objects.get(id).and_then(|s| s.value.as_object())
    }

    pub fn meta_value(&self) -> Map<String, Value> {
        self.meta.as_ref().and_then(|m| m.value.as_object().cloned()).unwrap_or_default()
    }

    /// Refused when this board's meta names a format this build can't read.
    pub fn check_format(&self) -> Result<(), BoardError> {
        check_format(self.meta.as_ref().and_then(|m| m.value.get("format")))
    }
}

/// A `format` value this build can write over: none (from before format 1)
/// or 1..=[`FORMAT`]. Anything else is `newer_format`: reading still works,
/// and nothing is written over it.
pub fn check_format(format: Option<&Value>) -> Result<(), BoardError> {
    let readable = |f: &Value| f.is_null() || f.as_f64().is_some_and(|n| n.fract() == 0.0 && n >= 1.0 && n <= FORMAT as f64);
    match format {
        Some(f) if !readable(f) => Err(BoardError::new(
            "newer_format",
            format!("a newer K2 made this board (format {f}; this K2 knows up to {FORMAT}), so it's read only here. Update K2 to change it. Nothing was written."),
        )),
        _ => Ok(()),
    }
}

/// The writes that upgrade a board from before `format`, once: each object
/// in today's shape ([`upgrade_object`]), then `format` on its meta and list
/// row, last, so a stop part way runs again next time. No author or time
/// changes, and a value that wouldn't fit stays as it is (readers' rules
/// still hold for it). Empty when the meta and row already say a format.
pub fn upgrade_plan(board_id: &str, entries: &[Stored], row: Option<&Stored>, batch: &str) -> Vec<Write> {
    let mk = meta_key(board_id);
    let meta = entries.iter().find(|e| e.key == mk);
    let stamps: Vec<&Stored> = meta.into_iter().chain(row).filter(|e| e.value.is_object() && e.value.get("format").is_none_or(Value::is_null)).collect();
    if stamps.is_empty() {
        return Vec::new();
    }
    let obj_prefix = format!("{}obj/", super::board_prefix(board_id));
    let mut out = Vec::new();
    for e in entries {
        let Some(tail) = e.key.strip_prefix(&obj_prefix) else { continue };
        let mut v = upgrade_object(&e.value, Some(tail));
        if v == e.value {
            continue;
        }
        if let Some(o) = v.as_object_mut() {
            o.insert("batch".into(), json!(batch));
        }
        if v.to_string().len() <= schema::MAX_VALUE {
            out.push(Write { key: e.key.clone(), value: Some(v), if_rev: e.rev });
        }
    }
    for e in stamps {
        let mut v = e.value.clone();
        v["format"] = json!(FORMAT);
        out.push(Write { key: e.key.clone(), value: Some(v), if_rev: e.rev });
    }
    out
}

/// One op, as `apply` takes them.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Add(Map<String, Value>),
    Set { id: String, f: Map<String, Value> },
    Del { id: String },
    Meta(Map<String, Value>),
}

/// Who and when, stamped on what a plan writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ctx {
    /// A person key: `agent:<handle>` or `person:owner`.
    pub by: String,
    /// ISO time.
    pub at: String,
    /// The batch id stamped on every value (one per command).
    pub batch: String,
}

/// One key write: `value: None` deletes. `if_rev` is the rev read (0 for a
/// new key), so a write made meanwhile is a conflict, never lost.
#[derive(Debug, Clone, PartialEq)]
pub struct Write {
    pub key: String,
    pub value: Option<Value>,
    pub if_rev: i64,
}

/// Parse `{"ops": [...]}` (or the list itself).
pub fn parse_ops(v: &Value) -> Result<Vec<Op>, BoardError> {
    let list = v.get("ops").unwrap_or(v).as_array().ok_or_else(|| BoardError::new("bad_ops", "apply needs {\"ops\":[...]}"))?;
    let mut out = Vec::with_capacity(list.len());
    for (i, op) in list.iter().enumerate() {
        let bad = |why: &str| BoardError::new("bad_ops", format!("op {i}: {why}"));
        let kind = op.get("op").and_then(Value::as_str).ok_or_else(|| bad("needs \"op\": add, set, del or meta"))?;
        let obj = |k: &str| op.get(k).and_then(Value::as_object).cloned();
        let id = || op.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| bad("needs \"id\""));
        out.push(match kind {
            "add" => Op::Add(obj("obj").ok_or_else(|| bad("add needs \"obj\": {\"type\": …}"))?),
            "set" | "update" => Op::Set { id: id()?, f: obj("f").ok_or_else(|| bad("set needs \"f\": {fields}"))? },
            "del" | "delete" => Op::Del { id: id()? },
            "meta" => Op::Meta(obj("f").ok_or_else(|| bad("meta needs \"f\": {title|voting|timer}"))?),
            other => return Err(bad(&format!("unknown op {other} (add, set, del, meta)"))),
        });
    }
    Ok(out)
}

/// `apply`'s id mapping (bin/board's rule): every added object gets a
/// fresh board id; an id the caller gave is a label for this call only,
/// and `set`/`del` ids, connector ends, mind parents and comment targets
/// in the same call follow it. Answers label → id.
pub fn map_ids(ops: &mut [Op]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for op in ops.iter_mut() {
        if let Op::Add(o) = op {
            let ty = o.get("type").and_then(Value::as_str).unwrap_or("");
            let fresh = schema::new_id(ty);
            if let Some(label) = o.get("id").and_then(Value::as_str) {
                map.insert(label.to_string(), fresh.clone());
            }
            o.insert("id".into(), Value::String(fresh));
        }
    }
    let re = |s: &str| map.get(s).cloned().unwrap_or_else(|| s.to_string());
    for op in ops.iter_mut() {
        let fields = match op {
            Op::Add(o) => Some(o),
            Op::Set { id, f } => {
                *id = re(id);
                Some(f)
            }
            Op::Del { id } => {
                *id = re(id);
                None
            }
            Op::Meta(_) => None,
        };
        let Some(o) = fields else { continue };
        for k in ["from", "to"] {
            match o.get_mut(k) {
                Some(Value::String(s)) => *s = re(s),
                Some(Value::Object(e)) => {
                    if let Some(Value::String(s)) = e.get_mut("id") {
                        *s = re(s);
                    }
                }
                _ => {}
            }
        }
        for k in ["parent", "target"] {
            if let Some(Value::String(s)) = o.get_mut(k) {
                *s = re(s);
            }
        }
    }
    map
}

/// What deleting `id` takes with it (the widget's rule): connectors
/// attached to it, its mind-map children and comments pinned to it, and
/// theirs in turn. `id` first, then in board order.
pub fn cascade(board: &Board, id: &str) -> Vec<String> {
    let mut all: Vec<String> = vec![id.to_string()];
    let mut set: HashSet<String> = HashSet::from([id.to_string()]);
    loop {
        let mut grew = false;
        for (oid, s) in &board.objects {
            if set.contains(oid) {
                continue;
            }
            let o = &s.value;
            let end = |k: &str| o.get(k).and_then(|e| e.get("id")).and_then(Value::as_str).is_some_and(|x| set.contains(x));
            let field = |k: &str| o.get(k).and_then(Value::as_str).is_some_and(|x| !x.is_empty() && set.contains(x));
            let hit = match o.get("type").and_then(Value::as_str) {
                Some("connector") => end("from") || end("to"),
                Some("mind") => field("parent"),
                Some("comment") => field("target"),
                _ => false,
            };
            if hit {
                set.insert(oid.clone());
                all.push(oid.clone());
                grew = true;
            }
        }
        if !grew {
            return all;
        }
    }
}

/// Fields a locked object still takes (the widget allows these too).
pub const LOCK_FREE: &[&str] = &["locked", "votes", "reactions"];

fn lock_msg(id: &str) -> BoardError {
    BoardError::new("locked", format!("{id} is locked: unlock it first (k2 board update {id} --locked false), or leave it")).field(id)
}

fn is_locked(v: &Map<String, Value>) -> bool {
    matches!(v.get("locked"), Some(Value::Bool(true)))
}

/// Check `ops` against `board` and turn them into key writes. All or
/// nothing: the first refusal answers and nothing is planned. `named`:
/// the ids the caller named (the lock rule applies to these); `None` =
/// every op is named. Objects a cascade takes follow their parent.
pub fn plan(board: &Board, ops: &[Op], named: Option<&HashSet<String>>, ctx: &Ctx) -> Result<Vec<Write>, BoardError> {
    board.check_format()?;
    // The working copy: object id → value (None = deleted here).
    let mut work: BTreeMap<String, Option<Map<String, Value>>> = BTreeMap::new();
    let mut meta: Option<Map<String, Value>> = None;
    let current = |work: &BTreeMap<String, Option<Map<String, Value>>>, id: &str| -> Option<Map<String, Value>> {
        match work.get(id) {
            Some(v) => v.clone(),
            None => board.object(id).cloned(),
        }
    };
    let mut z = board.objects.values().filter_map(|s| s.value.get("z").and_then(Value::as_f64)).fold(0.0_f64, f64::max);
    let is_named = |id: &str| named.is_none_or(|n| n.contains(id));
    let stamp = |o: &mut Map<String, Value>| {
        o.insert("updatedBy".into(), json!(ctx.by));
        o.insert("updatedAt".into(), json!(ctx.at));
        o.insert("batch".into(), json!(ctx.batch));
    };
    for op in ops {
        match op {
            Op::Add(obj) => {
                let ty = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if schema::fields(ty).is_none() {
                    return Err(BoardError::new("unknown_type", format!("add needs obj.type, one of {}", schema::type_names().join(", "))));
                }
                let id = match obj.get("id") {
                    None | Some(Value::Null) => schema::new_id(ty),
                    Some(Value::String(s)) if schema::valid_item_id(s) => s.clone(),
                    Some(_) => return Err(BoardError::bad_field("id", "1–24 of a-z, 0-9, . _ -")),
                };
                if current(&work, &id).is_some() {
                    return Err(BoardError::new("exists", format!("{id} is already on the board (new objects get new ids)")).field(&id));
                }
                let mut o = Map::new();
                o.insert("id".into(), json!(id));
                o.insert("type".into(), json!(ty));
                o.insert("x".into(), json!(0));
                o.insert("y".into(), json!(0));
                if let Value::Object(d) = schema::defaults(ty) {
                    o.extend(d);
                }
                z += 1.0;
                o.insert("z".into(), json!(z));
                for (k, v) in clean(ty, &normalize(ty, obj, None))? {
                    match v {
                        Some(v) => {
                            o.insert(k, v);
                        }
                        None => {
                            o.remove(&k);
                        }
                    }
                }
                if let Some(zv) = o.get("z").and_then(Value::as_f64) {
                    z = z.max(zv);
                }
                table_ids(&mut o);
                o.insert("createdBy".into(), json!(ctx.by));
                o.insert("createdAt".into(), json!(ctx.at));
                stamp(&mut o);
                work.insert(id, Some(o));
            }
            Op::Set { id, f } => {
                let Some(o) = current(&work, id) else {
                    return Err(BoardError::new("no_object", format!("no object {id} on this board")).field(id));
                };
                // An older object is written in today's shape, with the
                // ids its readers already give it.
                let mut o = upgrade_object(&Value::Object(o), None).as_object().cloned().unwrap_or_default();
                if is_named(id) && is_locked(&o) {
                    let ok = f.iter().all(|(k, v)| {
                        LOCK_FREE.contains(&k.as_str()) && (k != "locked" || matches!(v, Value::Bool(false)) || v.as_str() == Some("false"))
                    });
                    if !ok {
                        return Err(lock_msg(id));
                    }
                }
                let ty = o.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                for (k, v) in clean(&ty, &normalize(&ty, f, Some(&o)))? {
                    match v {
                        Some(v) => {
                            o.insert(k, v);
                        }
                        // checks, crop, link: null means none, so the
                        // key goes (`NULL_IS_NONE`), never kept as null.
                        None => {
                            o.remove(&k);
                        }
                    }
                }
                table_ids(&mut o);
                stamp(&mut o);
                work.insert(id.clone(), Some(o));
            }
            Op::Del { id } => {
                let Some(o) = current(&work, id) else {
                    return Err(BoardError::new("no_object", format!("no object {id} on this board")).field(id));
                };
                if is_named(id) && is_locked(&o) {
                    return Err(lock_msg(id));
                }
                work.insert(id.clone(), None);
            }
            Op::Meta(f) => {
                let mut m = meta.clone().unwrap_or_else(|| board.meta_value());
                for (k, v) in clean_meta(f)? {
                    m.insert(k, v);
                }
                m.insert("format".into(), json!(FORMAT));
                m.insert("batch".into(), json!(ctx.batch));
                meta = Some(m);
            }
        }
    }
    let mut out = Vec::new();
    for (id, v) in work {
        let key = obj_key(&board.id, &id);
        let before = board.objects.get(&id);
        match v {
            Some(v) => {
                let value = Value::Object(v);
                let bytes = value.to_string().len();
                if bytes > schema::MAX_VALUE {
                    return Err(BoardError::value_too_large(&id, schema::MAX_VALUE));
                }
                out.push(Write { key, value: Some(value), if_rev: before.map_or(0, |s| s.rev) });
            }
            None => {
                if let Some(s) = before {
                    out.push(Write { key, value: None, if_rev: s.rev });
                }
            }
        }
    }
    if let Some(m) = meta {
        out.push(Write { key: meta_key(&board.id), value: Some(Value::Object(m)), if_rev: board.meta.as_ref().map_or(0, |s| s.rev) });
    }
    Ok(out)
}

/// An object to print: an image's `data:` source shown by size (a blob
/// ref is short and shown as is), unless `full`.
pub fn view(v: &Value, full: bool) -> Value {
    let mut v = schema::read_shape(v);
    if !full {
        if let Some(o) = v.as_object_mut() {
            o.remove("batch");
            if o.get("type").and_then(Value::as_str) == Some("image") {
                if let Some(src) = o.get("src").and_then(Value::as_str).filter(|s| s.starts_with("data:")) {
                    let kb = (src.len() as f64 / 1024.0).round() as i64;
                    o.insert("src".into(), json!(format!("<data: {kb} KB>")));
                }
            }
        }
    }
    v
}

/// Every text an object shows, for search.
pub fn text_of(v: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for k in ["text", "title", "label", "assignee", "status"] {
        if let Some(s) = v.get(k).and_then(Value::as_str) {
            parts.push(s.to_string());
        }
    }
    for t in v.get("tags").and_then(Value::as_array).into_iter().flatten() {
        parts.push(t.as_str().map(str::to_string).unwrap_or_else(|| t.to_string()));
    }
    for r in v.get("rows").and_then(Value::as_array).into_iter().flatten() {
        for c in r.as_array().into_iter().flatten().filter_map(Value::as_str) {
            parts.push(c.to_string());
        }
    }
    for r in v.get("replies").and_then(Value::as_array).into_iter().flatten() {
        if let Some(t) = r.get("text").and_then(Value::as_str) {
            parts.push(t.to_string());
        }
    }
    for c in v.get("checks").and_then(Value::as_array).into_iter().flatten() {
        if let Some(t) = c.get("text").and_then(Value::as_str) {
            parts.push(t.to_string());
        }
    }
    parts.join(" ")
}

/// Toggle `who`'s `emoji` on an object's reactions (per person).
pub fn toggle_reaction(obj: &Map<String, Value>, who: &str, emoji: &str) -> Result<Value, BoardError> {
    if !schema::REACTIONS.contains(&emoji) {
        return Err(BoardError::bad_field("reaction", &format!("one of {}", schema::REACTIONS.join(" "))));
    }
    let shaped = schema::read_shape(&Value::Object(obj.clone()));
    let mut r = shaped.get("reactions").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut mine: Vec<Value> = r.get(who).and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(i) = mine.iter().position(|e| e.as_str() == Some(emoji)) {
        mine.remove(i);
    } else {
        mine.push(json!(emoji));
    }
    if mine.is_empty() {
        r.remove(who);
    } else {
        r.insert(who.to_string(), Value::Array(mine));
    }
    Ok(Value::Object(r))
}

/// `who` votes once more on `id`, if voting is on and they have votes
/// left (`perPerson`).
pub fn add_vote(board: &Board, id: &str, who: &str) -> Result<Value, BoardError> {
    let voting = board.meta_value().get("voting").cloned().unwrap_or(Value::Null);
    if voting.get("active") != Some(&Value::Bool(true)) {
        return Err(BoardError::new(
            "voting_off",
            "voting is not on (the person starts it, or: k2 board meta --voting '{\"active\":true,\"perPerson\":5}')",
        ));
    }
    let o = board.object(id).ok_or_else(|| BoardError::new("no_object", format!("no object {id} on this board")).field(id))?;
    let used: u64 = board.objects.values().filter_map(|s| s.value.get("votes").and_then(|v| v.get(who)).and_then(Value::as_u64)).sum();
    if let Some(per) = voting.get("perPerson").and_then(Value::as_u64) {
        if per > 0 && used >= per {
            return Err(BoardError::new("no_votes_left", format!("you have used all {per} votes")));
        }
    }
    let mut votes = o.get("votes").and_then(Value::as_object).cloned().unwrap_or_default();
    let n = votes.get(who).and_then(Value::as_u64).unwrap_or(0) + 1;
    votes.insert(who.to_string(), json!(n));
    Ok(Value::Object(votes))
}
