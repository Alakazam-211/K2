//! The mapper between a `.k5board` file's objects and the shape Board's
//! widget draws (prd-board-tab-v1 §5.1). The two differ on purpose: a file
//! holds array order, reactions by emoji, table rows with ids and cells, and
//! `{blob}` references; the widget holds `z`, reactions by person, parallel
//! `rows`/`rowIds`/`colIds`/`colW`, and `k2blob:` refs in the frame's memory
//! only. Never on disk.
//!
//! **It is per object and writes only what's touched.** A byte-identical
//! round trip through the widget's shape can't work (`(p − min) + min != p`
//! for doubles, reaction order and sparse cells have nowhere to live, and the
//! widget's checker drops sub-item fields it doesn't know). So reading is one
//! way, for drawing, and an edit is turned into the fields it names:
//! [`widget_set_to_file`] gives only those, and the format's `apply_group`
//! keeps everything else in the object as it was. What a value the widget
//! rounded for display does to the file: nothing.
//!
//! In a tab a person key **is** the file's token: the frame shows names from
//! `people`. [`garden_board_to_file`] is the Garden's way out, where keys are
//! `person:owner` and `agent:<handle>` and each gets a fresh random token.

use std::collections::{BTreeMap, BTreeSet};

use k2_core::board::schema;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

/// The widget shape `read` answers with. A frame that knows only a lower one
/// (a Connect client on an older build) says "Update K2".
pub const SHAPE: u32 = 1;

/// The frame's image ref: `k2blob:sha256-<hex>`.
const BLOB_SCHEME: &str = "k2blob:";

/// A person's edit that can't be put in the file. Typed: nothing is cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError {
    /// A stable code for the wire.
    pub code: &'static str,
    /// Which field.
    pub field: String,
    /// A sentence for people.
    pub message: String,
}

fn bad(field: &str, message: impl Into<String>) -> MapError {
    MapError { code: "bad_field", field: field.to_string(), message: message.into() }
}

/// The fields of one edit, in the file's shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldEdit {
    /// Fields to set.
    pub set: Map<String, Value>,
    /// Fields to remove.
    pub unset: Vec<String>,
}

/// An id a file can carry: 1 to 24 of `[0-9a-z._-]`.
pub fn is_id(s: &str) -> bool {
    schema::valid_item_id(s)
}

// ---------- reading: a file object to what the widget draws ----------

/// Fields where the widget's new-object default is a placeholder word and the
/// file's is empty: an object that leaves one out is blank, so it stays blank.
const BLANK_IN_A_FILE: &[(&str, &str)] = &[("text", "text"), ("frame", "title"), ("card", "title"), ("mind", "text")];

fn end_to_widget(v: &Value) -> Value {
    let Some(o) = v.as_object() else { return v.clone() };
    let Some(id) = o.get("id") else { return v.clone() };
    let mut e = Map::new();
    e.insert("id".into(), id.clone());
    e.insert("anchor".into(), o.get("anchor").cloned().unwrap_or_else(|| json!("auto")));
    if let Some(h) = o.get("hint").and_then(Value::as_object) {
        if let (Some(x), Some(y)) = (h.get("x"), h.get("y")) {
            e.insert("x".into(), x.clone());
            e.insert("y".into(), y.clone());
        }
    }
    Value::Object(e)
}

/// `{emoji: [token]}` to `{token: [emoji]}`, in the widget's list order. A
/// reaction outside the widget's list isn't shown: its checker refuses the
/// whole field otherwise, and a save merges by token, so it stays in the file.
fn reactions_to_widget(v: &Value) -> Value {
    let Some(r) = v.as_object() else { return json!({}) };
    let mut by: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for emoji in schema::REACTIONS {
        for token in r.get(*emoji).and_then(Value::as_array).into_iter().flatten() {
            if let Some(t) = token.as_str() {
                by.entry(t.to_string()).or_default().push(json!(emoji));
            }
        }
    }
    Value::Object(by.into_iter().map(|(k, v)| (k, Value::Array(v))).collect())
}

fn table_to_widget(o: &mut Map<String, Value>) {
    let cols = o.remove("columns").and_then(|c| c.as_array().cloned()).unwrap_or_default();
    let rows = o.remove("rows").and_then(|c| c.as_array().cloned()).unwrap_or_default();
    let col_ids: Vec<&str> = cols.iter().filter_map(|c| c.get("id").and_then(Value::as_str)).collect();
    let widths: Vec<Value> = cols.iter().map_while(|c| c.get("width").cloned()).collect();
    let mut out = Vec::with_capacity(rows.len());
    let mut row_ids = Vec::with_capacity(rows.len());
    for r in &rows {
        row_ids.push(r.get("id").cloned().unwrap_or(Value::Null));
        let cells = r.get("cells").and_then(Value::as_object);
        // A short row stays short; a gap before its last cell reads as empty.
        let last = col_ids.iter().rposition(|c| cells.is_some_and(|m| m.contains_key(*c)));
        let line: Vec<Value> = col_ids[..last.map_or(0, |l| l + 1)]
            .iter()
            .map(|c| cells.and_then(|m| m.get(*c)).cloned().unwrap_or_else(|| json!("")))
            .collect();
        out.push(Value::Array(line));
    }
    o.insert("rows".into(), Value::Array(out));
    o.insert("rowIds".into(), Value::Array(row_ids));
    o.insert("colIds".into(), Value::Array(col_ids.iter().map(|c| json!(c)).collect()));
    o.insert("colW".into(), Value::Array(widths));
}

/// One file object as the widget draws it, at z-order position `z` and, for a
/// frame, presentation position `order`. A type this build doesn't know passes
/// through with its fields: the frame draws a placeholder and locks it.
pub fn object_to_widget(file: &Value, z: f64, order: Option<f64>) -> Value {
    let mut o = file.as_object().cloned().unwrap_or_default();
    let ty = o.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    if let Some(Value::Object(c)) = o.remove("created") {
        if let Some(by) = c.get("by") {
            o.insert("createdBy".into(), by.clone());
        }
        if let Some(at) = c.get("at") {
            o.insert("createdAt".into(), at.clone());
        }
    }
    for k in ["from", "to"] {
        if let Some(e) = o.get(k).map(end_to_widget) {
            o.insert(k.into(), e);
        }
    }
    if let Some(r) = o.get("reactions").map(reactions_to_widget) {
        o.insert("reactions".into(), r);
    }
    if let Some(Value::Object(src)) = o.get("src") {
        if let Some(h) = src.get("blob").and_then(Value::as_str) {
            let r = format!("{BLOB_SCHEME}{h}");
            o.insert("src".into(), json!(r));
        }
    }
    if ty == "table" {
        table_to_widget(&mut o);
    }
    if ty == "pen" {
        let (mut w, mut h) = (1.0f64, 1.0f64);
        for p in o.get("points").and_then(Value::as_array).into_iter().flatten() {
            w = w.max(p[0].as_f64().unwrap_or(0.0));
            h = h.max(p[1].as_f64().unwrap_or(0.0));
        }
        o.insert("w".into(), json!(w));
        o.insert("h".into(), json!(h));
    }
    // What the widget needs for a field the file leaves out.
    if let Some(d) = schema::defaults(&ty).as_object() {
        for (k, v) in d {
            if !o.contains_key(k) {
                let blank = BLANK_IN_A_FILE.contains(&(ty.as_str(), k.as_str()));
                o.insert(k.clone(), if blank { json!("") } else { v.clone() });
            }
        }
    }
    for k in ["x", "y"] {
        o.entry(k.to_string()).or_insert_with(|| json!(0));
    }
    o.insert("z".into(), json!(z));
    if let Some(n) = order {
        o.insert("order".into(), json!(n));
    }
    Value::Object(o)
}

/// A file's `settings` as the widget's `meta`. **Never `timer`:** the widget's
/// HUD writes on view when a running timer has run out, so an untouched file
/// would change on open. The daemon keeps it on the file.
pub fn settings_to_meta(settings: &Value) -> Value {
    let mut m = settings.as_object().cloned().unwrap_or_default();
    m.remove("timer");
    Value::Object(m)
}

// ---------- writing: a person's edit to the fields it names ----------

fn end_to_file(v: &Value) -> Value {
    let Some(o) = v.as_object() else { return v.clone() };
    let Some(id) = o.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) else { return v.clone() };
    let mut e = Map::new();
    e.insert("id".into(), json!(id));
    if let Some(a) = o.get("anchor") {
        e.insert("anchor".into(), a.clone());
    }
    // Where the end was last drawn is what `hint` means.
    if let (Some(x), Some(y)) = (o.get("x"), o.get("y")) {
        e.insert("hint".into(), json!({ "x": x, "y": y }));
    }
    Value::Object(e)
}

fn blob_of(field: &str, v: &str) -> Result<Value, MapError> {
    let h = v.strip_prefix(BLOB_SCHEME).ok_or_else(|| bad(field, "an image ref (k2blob:sha256-…)"))?;
    Ok(json!({ "blob": h }))
}

/// Every exact `k2blob:` string under `v` becomes a `{blob}` reference, so an
/// unknown field that holds one keeps it and no host form reaches the disk.
fn blobify(v: &mut Value, found: &mut BTreeSet<String>) {
    match v {
        Value::String(s) if s.starts_with("k2blob:sha256-") => {
            let h = s[BLOB_SCHEME.len()..].to_string();
            found.insert(h.clone());
            *v = json!({ "blob": h });
        }
        Value::Array(a) => a.iter_mut().for_each(|x| blobify(x, found)),
        Value::Object(m) => m.iter_mut().for_each(|(_, x)| blobify(x, found)),
        _ => {}
    }
}

/// `{token: [emoji]}` to `{emoji: [token, …]}`, sorted and unique.
fn reactions_to_file(v: &Value, token_of: &mut dyn FnMut(&str) -> String) -> Result<Map<String, Value>, MapError> {
    let r = v.as_object().ok_or_else(|| bad("reactions", "{\"<person>\": [\"👍\", …]}"))?;
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (who, list) in r {
        let t = token_of(who);
        for e in list.as_array().ok_or_else(|| bad("reactions", "{\"<person>\": [\"👍\", …]}"))? {
            let e = e.as_str().ok_or_else(|| bad("reactions", "{\"<person>\": [\"👍\", …]}"))?;
            out.entry(e.to_string()).or_default().insert(t.clone());
        }
    }
    Ok(out.into_iter().map(|(k, v)| (k, json!(v.into_iter().collect::<Vec<_>>()))).collect())
}

fn string_list<'a>(v: Option<&'a Value>) -> Option<Vec<&'a str>> {
    v?.as_array()?.iter().map(Value::as_str).collect()
}

/// The table fields of one edit, from the widget's parallel lists. `base` is
/// the object in the file, for the ids an edit of `colW` alone doesn't repeat.
fn table_edit(f: &Map<String, Value>, base: &Map<String, Value>, edit: &mut FieldEdit) -> Result<(), MapError> {
    let base_cols: Vec<&str> =
        base.get("columns").and_then(Value::as_array).map(|c| c.iter().filter_map(|x| x.get("id").and_then(Value::as_str)).collect()).unwrap_or_default();
    let base_rows: Vec<&str> =
        base.get("rows").and_then(Value::as_array).map(|c| c.iter().filter_map(|x| x.get("id").and_then(Value::as_str)).collect()).unwrap_or_default();
    let cols: Vec<&str> = match f.get("colIds") {
        Some(v) => string_list(Some(v)).ok_or_else(|| bad("colIds", "a list of ids"))?,
        None => base_cols,
    };
    if let Some(rows) = f.get("rows") {
        let rows = rows.as_array().ok_or_else(|| bad("rows", "a list of rows"))?;
        let ids: Vec<&str> = match f.get("rowIds") {
            Some(v) => string_list(Some(v)).ok_or_else(|| bad("rowIds", "a list of ids"))?,
            None => base_rows,
        };
        if ids.len() != rows.len() {
            return Err(bad("rowIds", "one id per row"));
        }
        let mut out = Vec::with_capacity(rows.len());
        for (i, row) in rows.iter().enumerate() {
            let row = row.as_array().ok_or_else(|| bad("rows", "a list of rows"))?;
            if row.len() > cols.len() {
                return Err(bad("colIds", "one id per column"));
            }
            // `cells` is always sent, because a row that leaves it out keeps
            // the cells it had. An item merges by id.
            let cells: Map<String, Value> = row.iter().enumerate().map(|(j, c)| (cols[j].to_string(), c.clone())).collect();
            out.push(json!({ "id": ids[i], "cells": cells }));
        }
        edit.set.insert("rows".into(), Value::Array(out));
    }
    if f.contains_key("colIds") || f.contains_key("colW") {
        let widths = f.get("colW").and_then(Value::as_array);
        if f.contains_key("colW") && widths.is_none() {
            return Err(bad("colW", "a list of widths"));
        }
        let mut out = Vec::with_capacity(cols.len());
        for (i, c) in cols.iter().enumerate() {
            let mut col = Map::new();
            col.insert("id".into(), json!(c));
            if let Some(w) = widths.and_then(|l| l.get(i)).filter(|w| !w.is_null()) {
                col.insert("width".into(), w.clone());
            }
            out.push(Value::Object(col));
        }
        edit.set.insert("columns".into(), Value::Array(out));
    }
    Ok(())
}

/// Keys the widget carries that the file doesn't: order lives in the array,
/// who changed what and when isn't kept, and a pen's box comes from its points.
fn is_widget_only(ty: &str, k: &str) -> bool {
    matches!(k, "z" | "order" | "id" | "type" | "batch" | "createdBy" | "createdAt" | "updatedBy" | "updatedAt")
        || (ty == "pen" && matches!(k, "w" | "h"))
}

/// One person's `set` of an object, as the file's fields. `by` is the group's
/// token: the widget sends its whole `reactions` or `votes`, and only the
/// caller's entry may change (`apply_group` merges by it).
pub fn widget_set_to_file(ty: &str, f: &Map<String, Value>, base: &Map<String, Value>) -> Result<FieldEdit, MapError> {
    let mut edit = FieldEdit::default();
    if ty == "table" {
        table_edit(f, base, &mut edit)?;
    }
    for (k, v) in f {
        if is_widget_only(ty, k) || (ty == "table" && matches!(k.as_str(), "rows" | "rowIds" | "colIds" | "colW")) {
            continue;
        }
        match k.as_str() {
            // None is absent in a file.
            "group" | "parent" | "target" | "frame" if v.is_null() || v.as_str() == Some("") => edit.unset.push(k.clone()),
            "link" | "crop" | "checks" if v.is_null() || v.as_str() == Some("") => edit.unset.push(k.clone()),
            "src" if v.is_null() || v.as_str() == Some("") => edit.unset.push(k.clone()),
            "src" => {
                let s = v.as_str().ok_or_else(|| bad(k, "an image ref (k2blob:sha256-…)"))?;
                edit.set.insert(k.clone(), blob_of(k, s)?);
            }
            "from" | "to" => {
                if v.is_null() {
                    edit.unset.push(k.clone());
                } else {
                    edit.set.insert(k.clone(), end_to_file(v));
                }
            }
            "reactions" => {
                edit.set.insert(k.clone(), Value::Object(reactions_to_file(v, &mut |who| who.to_string())?));
            }
            _ if v.is_null() => edit.unset.push(k.clone()),
            _ => {
                let mut x = v.clone();
                blobify(&mut x, &mut BTreeSet::new());
                edit.set.insert(k.clone(), x);
            }
        }
    }
    Ok(edit)
}

/// A person's new object as a file object. `created` is `{by, at}`, from the
/// daemon: the frame doesn't say who it is.
pub fn widget_add_to_file(obj: &Map<String, Value>, created: Option<Value>) -> Result<Value, MapError> {
    let ty = obj.get("type").and_then(Value::as_str).ok_or_else(|| bad("type", "an object type"))?;
    let id = obj.get("id").and_then(Value::as_str).ok_or_else(|| bad("id", "an object id"))?;
    let edit = widget_set_to_file(ty, obj, &Map::new())?;
    let mut out = edit.set;
    out.insert("id".into(), json!(id));
    out.insert("type".into(), json!(ty));
    if let Some(c) = created {
        out.insert("created".into(), c);
    }
    Ok(Value::Object(out))
}

/// A person's change to the board's settings. The widget's own bookkeeping
/// (`format`, `batch`) isn't the file's, and the tab has no timer.
pub fn widget_meta_to_file(f: &Map<String, Value>) -> Result<FieldEdit, MapError> {
    let mut edit = FieldEdit::default();
    for (k, v) in f {
        match k.as_str() {
            "format" | "batch" => {}
            "timer" => return Err(bad("timer", "a board file doesn't run a timer")),
            _ if v.is_null() => edit.unset.push(k.clone()),
            _ => {
                edit.set.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(edit)
}

// ---------- the Garden's way out: a whole board, with K2's person keys ----------

/// A Garden `group` as a file's group id. Ones made by the widget and the agent
/// verbs already fit. `k2 board --group "Team A"` doesn't, and such boards exist:
/// a group is only ever compared and never shown, so it maps to a stable id, and
/// the same label on two objects stays one group.
pub fn group_id(g: &str) -> String {
    if is_id(g) {
        return g.to_string();
    }
    let d = Sha256::digest(g.as_bytes());
    format!("g-{}", d.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>())
}

/// One person, as a Garden stores them: `person:owner`, `agent:<handle>`, or an
/// older plain name.
fn person_of(key: &str, owner_name: &str) -> Value {
    if key == k2_core::board::PERSON_OWNER {
        return json!({ "name": owner_name, "kind": "person" });
    }
    if let Some(h) = key.strip_prefix("agent:") {
        return json!({ "name": h, "kind": "agent" });
    }
    json!({ "name": key.strip_prefix("person:").unwrap_or(key), "kind": "person" })
}

/// Tokens for a Garden's person keys. Random, never `p1`, `p2`: two copies of a
/// board must not give one token to two different people when they merge.
pub struct People<'a> {
    random: &'a mut dyn FnMut() -> [u8; 8],
    owner_name: &'a str,
    tokens: BTreeMap<String, String>,
    table: Map<String, Value>,
}

impl<'a> People<'a> {
    /// With a source of random bytes, and the name for `person:owner`.
    pub fn new(random: &'a mut dyn FnMut() -> [u8; 8], owner_name: &'a str) -> People<'a> {
        People { random, owner_name, tokens: BTreeMap::new(), table: Map::new() }
    }

    /// With the tokens fixed in advance (tests).
    #[cfg(test)]
    fn seeded(random: &'a mut dyn FnMut() -> [u8; 8], owner_name: &'a str, known: &BTreeMap<String, String>) -> People<'a> {
        let mut p = People::new(random, owner_name);
        for (k, t) in known {
            p.tokens.insert(k.clone(), t.clone());
            p.table.insert(t.clone(), person_of(k, owner_name));
        }
        p
    }

    fn token(&mut self, key: &str) -> String {
        if let Some(t) = self.tokens.get(key) {
            return t.clone();
        }
        let t = k2_mpc_board::mint_person_token((self.random)()).as_str().to_string();
        self.tokens.insert(key.to_string(), t.clone());
        self.table.insert(t.clone(), person_of(key, self.owner_name));
        t
    }
}

/// One Garden object as a file object: everything [`widget_set_to_file`] does,
/// with every person key turned into a token. `blobs` hears each hash it refers
/// to. A number or id the file can't hold isn't touched here: the codec refuses
/// it, typed, and the caller names the object.
pub fn garden_object_to_file(w: &Value, people: &mut People, blobs: &mut BTreeSet<String>) -> Result<Value, MapError> {
    let mut o = w.as_object().cloned().ok_or_else(|| bad("object", "an object"))?;
    let id = o.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let ty = o.get("type").and_then(Value::as_str).unwrap_or("").to_string();
    let by = o.remove("createdBy");
    let at = o.remove("createdAt");
    if let Some(Value::String(g)) = o.get("group") {
        if !g.is_empty() {
            let mapped = group_id(g);
            o.insert("group".into(), json!(mapped));
        }
    }
    let mut out = Map::new();
    out.insert("id".into(), json!(id));
    out.insert("type".into(), json!(ty));
    if let (Some(by), Some(at)) = (by.as_ref().and_then(Value::as_str), at.as_ref().and_then(Value::as_str)) {
        if !by.is_empty() {
            out.insert("created".into(), json!({ "by": people.token(by), "at": at }));
        }
    }
    if let Some(r) = o.remove("reactions") {
        let m = reactions_to_file(&r, &mut |who| people.token(who))?;
        if !m.is_empty() {
            out.insert("reactions".into(), Value::Object(m));
        }
    }
    if let Some(Value::Object(v)) = o.remove("votes") {
        let m: Map<String, Value> = v.iter().filter(|(_, n)| n.as_u64().unwrap_or(0) > 0).map(|(k, n)| (people.token(k), n.clone())).collect();
        if !m.is_empty() {
            out.insert("votes".into(), Value::Object(m));
        }
    }
    if let Some(Value::Array(rs)) = o.remove("replies") {
        let mut list = Vec::with_capacity(rs.len());
        for mut r in rs {
            if let Some(b) = r.get("by").and_then(Value::as_str).map(str::to_string) {
                r["by"] = json!(people.token(&b));
            }
            list.push(r);
        }
        if !list.is_empty() {
            out.insert("replies".into(), Value::Array(list));
        }
    }
    if let Some(Value::Array(ts)) = o.get_mut("tags") {
        // A number in tags is the same text to a person.
        for t in ts.iter_mut() {
            if t.is_number() {
                *t = json!(t.to_string());
            }
        }
    }
    if o.get("tags").and_then(Value::as_array).is_some_and(Vec::is_empty) {
        o.remove("tags");
    }
    if ty == "table" {
        ensure_table_ids(&mut o);
    }
    let edit = widget_set_to_file(&ty, &o, &Map::new())?;
    let mut set = edit.set;
    if ty == "table" {
        // An absent cell is empty, and an empty list is no list: the file
        // doesn't fill anything in. (A tab's edit always sends `cells`.)
        if let Some(Value::Array(rows)) = set.get_mut("rows") {
            for r in rows.iter_mut() {
                if r.get("cells").and_then(Value::as_object).is_some_and(Map::is_empty) {
                    if let Some(m) = r.as_object_mut() {
                        m.remove("cells");
                    }
                }
            }
        }
        for k in ["rows", "columns"] {
            if set.get(k).and_then(Value::as_array).is_some_and(Vec::is_empty) {
                set.remove(k);
            }
        }
    }
    out.extend(set);
    let mut v = Value::Object(out);
    blobify(&mut v, blobs);
    // A `{blob}` value that came through `widget_set_to_file` was named there.
    collect_blobs(&v, blobs);
    Ok(v)
}

/// A Garden table as the widget reads one: older tables keep no ids, so each
/// gets what every reader gives it (`schema.js` `ensureIds`). A width the file
/// can't hold (a string, a negative: raw writes) describes nothing, and goes.
fn ensure_table_ids(o: &mut Map<String, Value>) {
    let Some(rows) = o.get("rows").and_then(Value::as_array) else { return };
    let (nr, nc) = (rows.len(), rows.iter().filter_map(Value::as_array).map(Vec::len).max().unwrap_or(0));
    if o.get("rowIds").and_then(Value::as_array).map(Vec::len) != Some(nr) {
        o.insert("rowIds".into(), Value::Array((0..nr).map(|i| json!(format!("rw-{i}"))).collect()));
    }
    if o.get("colIds").and_then(Value::as_array).map(Vec::len) != Some(nc) {
        o.insert("colIds".into(), Value::Array((0..nc).map(|i| json!(format!("cl-{i}"))).collect()));
    }
    if let Some(Value::Array(w)) = o.get_mut("colW") {
        for x in w.iter_mut() {
            if !x.as_f64().is_some_and(|n| n >= 0.0) {
                *x = Value::Null;
            }
        }
    }
}

fn collect_blobs(v: &Value, found: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if m.len() == 1 {
                if let Some(h) = m.get("blob").and_then(Value::as_str) {
                    found.insert(h.to_string());
                    return;
                }
            }
            m.values().for_each(|x| collect_blobs(x, found));
        }
        Value::Array(a) => a.iter().for_each(|x| collect_blobs(x, found)),
        _ => {}
    }
}

/// A whole Garden board as `board.json`: objects by `(z, id)`, frames by
/// `(order, id)`, and `meta` as `settings`.
pub fn garden_board_to_file(objects: &[Value], meta: &Value, people: &mut People, blobs: &mut BTreeSet<String>) -> Result<Value, MapError> {
    let num = |o: &Value, k: &str| o.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let idv = |o: &Value| o.get("id").and_then(Value::as_str).unwrap_or("").to_string();
    let mut list: Vec<&Value> = objects.iter().collect();
    list.sort_by(|a, b| num(a, "z").total_cmp(&num(b, "z")).then_with(|| idv(a).cmp(&idv(b))));
    let mut frames: Vec<&Value> = list.iter().copied().filter(|o| o.get("type").and_then(Value::as_str) == Some("frame")).collect();
    frames.sort_by(|a, b| num(a, "order").total_cmp(&num(b, "order")).then_with(|| idv(a).cmp(&idv(b))));
    let frames: Vec<String> = frames.iter().map(|o| idv(o)).collect();
    let mut objs = Vec::with_capacity(list.len());
    for o in list {
        objs.push(garden_object_to_file(o, people, blobs)?);
    }
    let mut settings = meta.as_object().cloned().unwrap_or_default();
    for k in ["format", "batch"] {
        settings.remove(k);
    }
    Ok(json!({ "objects": objs, "frames": frames, "people": people.table.clone(), "settings": settings }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_mpc_board::{apply_group, json as fj, Board, Conflict, Group, Id, Limits, Op, Sha256 as Rev};

    const VECTORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/k2-mpc-board/vectors");
    const ANN: &str = "p-0123456789abcdef";
    const BOT: &str = "p-1123456789abcdef";

    /// The neutral forms in `vectors/widget`, as K2 really stores them.
    fn real_forms(text: &str) -> String {
        text.replace("hostblob:", "k2blob:").replace("key-ann", "person:owner").replace("key-bot", "agent:pat")
    }

    fn pairs() -> Vec<(String, Value)> {
        let dir = format!("{VECTORS}/widget");
        let mut out = Vec::new();
        for e in std::fs::read_dir(&dir).expect("vectors/widget") {
            let p = e.expect("entry").path();
            if p.extension().and_then(|x| x.to_str()) == Some("json") {
                let text = std::fs::read_to_string(&p).expect("vector");
                let name = p.file_name().and_then(|n| n.to_str()).expect("name").to_string();
                out.push((name, serde_json::from_str(&real_forms(&text)).expect("json")));
            }
        }
        assert_eq!(out.len(), 18, "the conversion pairs at format-v1.0");
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn known_tokens(v: &Value) -> BTreeMap<String, String> {
        v["people_map"].as_object().expect("people_map").iter().map(|(k, t)| (k.clone(), t.as_str().expect("token").to_string())).collect()
    }

    fn no_random() -> [u8; 8] {
        panic!("a vector's people are all known")
    }

    fn file_object(v: &Value, w: &Value) -> Value {
        let known = known_tokens(v);
        let mut r = no_random;
        let mut people = People::seeded(&mut r, "Ann", &known);
        garden_object_to_file(w, &mut people, &mut BTreeSet::new()).expect("maps")
    }

    /// Runs a pair only in the direction it claims: `file_back` is what the way
    /// back gives where it differs, and `widget_back_refused` means it refuses.
    #[test]
    fn every_conversion_pair_widget_to_file_in_real_forms() {
        for (name, v) in pairs() {
            // reaction-unknown's file holds a reaction its widget side can't: this pair
            // claims the other direction.
            if v.get("widget_back_refused").is_some() || !v["widget"].is_object() {
                continue;
            }
            let got = file_object(&v, &v["widget"]);
            let mut want = v["file"]["objects"][0].clone();
            if let Some(back) = v.get("file_back").and_then(Value::as_object) {
                for (k, x) in back {
                    if k == "cells" {
                        want["rows"][0]["cells"] = x.clone();
                    } else {
                        want[k] = x.clone();
                    }
                }
            }
            assert_eq!(got, want, "{name}");
        }
    }

    fn matches_where_it_says(want: &Value, got: &Value, at: &str) {
        match want {
            Value::Object(m) => {
                for (k, x) in m {
                    matches_where_it_says(x, got.get(k).unwrap_or(&Value::Null), &format!("{at}.{k}"));
                }
            }
            _ => {
                let same = want == got || (want.is_number() && want.as_f64() == got.as_f64());
                assert!(same, "{at}: want {want} got {got}");
            }
        }
    }

    #[test]
    fn every_conversion_pair_file_to_widget_in_real_forms() {
        for (name, v) in pairs() {
            if v.get("widget_back_refused").is_some() || !v["widget"].is_object() {
                continue;
            }
            let known = known_tokens(&v);
            let inv: BTreeMap<&str, &str> = known.iter().map(|(k, t)| (t.as_str(), k.as_str())).collect();
            let got = object_to_widget(&v["file"]["objects"][0], 1.0, None);
            // In a tab a person's key is their token: put K2's keys back to compare.
            let mut text = got.to_string();
            for (t, k) in &inv {
                text = text.replace(t, k);
            }
            let got: Value = serde_json::from_str(&text).expect("json");
            let mut want = v["widget"].clone();
            let w = want.as_object_mut().expect("widget");
            // `updatedBy`/`updatedAt` and a none aren't in the file, on purpose.
            for k in ["updatedBy", "updatedAt"] {
                w.remove(k);
            }
            if w.get("parent") == Some(&json!("")) {
                w.remove("group");
            }
            if let Some(back) = v.get("widget_back").and_then(Value::as_object) {
                for (k, x) in back {
                    if w.contains_key(k) && !matches!(k.as_str(), "updatedBy" | "updatedAt") {
                        w.insert(k.clone(), x.clone());
                    }
                }
            }
            if let Some(absent) = v.get("widget_when_absent") {
                let bare = json!({ "id": absent["id"], "type": absent["type"] });
                let shown = object_to_widget(&bare, 1.0, None);
                matches_where_it_says(absent, &shown, &format!("{name} when absent"));
            }
            matches_where_it_says(&want, &got, &name);
        }
    }

    #[test]
    fn order_comes_out_by_z_and_frames_by_order() {
        for (name, v) in pairs() {
            let Some(list) = v["widget"].as_array() else { continue };
            let mut r = no_random;
            let known = known_tokens(&v);
            let mut people = People::seeded(&mut r, "Ann", &known);
            let got = garden_board_to_file(list, &json!({}), &mut people, &mut BTreeSet::new()).expect("maps");
            let ids = |d: &Value| d["objects"].as_array().expect("objects").iter().map(|o| o["id"].clone()).collect::<Vec<_>>();
            assert_eq!(ids(&got), ids(&v["file"]), "{name}");
            assert_eq!(got["frames"], v["file"]["frames"], "{name}");
        }
    }

    #[test]
    fn a_reaction_outside_the_widgets_list_isnt_shown_and_stays_in_the_file() {
        let b = board(json!([{ "id": "s-1", "type": "sticky", "reactions": { "party-parrot": [BOT], "👍": [BOT] } }]));
        let w = object_to_widget(&obj_json(&b, "s-1"), 1.0, None);
        assert_eq!(w["reactions"], json!({ BOT: ["👍"] }));
        // The person reacts: the widget sends its whole value, and only theirs moves.
        let mine = json!({ BOT: ["👍"], ANN: ["🔥"] });
        let edit = widget_set_to_file("sticky", json!({ "reactions": mine }).as_object().expect("o"), &Map::new()).expect("maps");
        let got = apply(&b, "s-1", &edit, Some(ANN));
        let r = fj_json(got.objects[0].fields.get("reactions").expect("reactions"));
        assert_eq!(r, json!({ "party-parrot": [BOT], "👍": [BOT], "🔥": [ANN] }));
    }

    #[test]
    fn a_file_never_gets_a_timer_from_a_tab_or_shows_one() {
        let meta = settings_to_meta(&json!({ "title": "T", "timer": { "state": "running", "endsAt": 1 }, "voting": { "active": true } }));
        assert_eq!(meta, json!({ "title": "T", "voting": { "active": true } }));
        let e = widget_meta_to_file(json!({ "timer": { "state": "off" } }).as_object().expect("o")).expect_err("no timer");
        assert_eq!(e.field, "timer");
        let m = widget_meta_to_file(json!({ "title": "x", "format": 1, "batch": "w-1" }).as_object().expect("o")).expect("maps");
        assert_eq!(Value::Object(m.set), json!({ "title": "x" }));
    }

    #[test]
    fn a_blank_field_stays_blank_where_the_widgets_default_is_a_word() {
        for (ty, field) in BLANK_IN_A_FILE {
            let w = object_to_widget(&json!({ "id": "x-1", "type": ty }), 1.0, None);
            assert_eq!(w[field], "", "{ty}.{field}");
        }
        let w = object_to_widget(&json!({ "id": "x-1", "type": "sticky" }), 2.0, None);
        assert_eq!(w["color"], "yellow");
        assert_eq!(w["w"], 200);
        assert_eq!(w["z"], json!(2.0));
    }

    #[test]
    fn a_type_the_build_doesnt_know_passes_through_with_every_field() {
        let w = object_to_widget(&json!({ "id": "x-1", "type": "hologram", "beam": { "a": 1 }, "x": 5 }), 1.0, None);
        assert_eq!(w["beam"], json!({ "a": 1 }));
        assert_eq!(w["type"], "hologram");
        assert_eq!(w["x"], 5);
    }

    // ---------- one op changes only what it names ----------

    fn board(objects: Value) -> Board {
        let frames: Vec<&Value> = objects.as_array().expect("list").iter().filter(|o| o["type"] == "frame").map(|o| &o["id"]).collect();
        let doc = json!({ "objects": objects, "frames": frames, "people": { ANN: { "name": "Ann" }, BOT: { "name": "Bot", "kind": "agent" } }, "settings": {} });
        Board::from_json(doc.to_string().as_bytes(), &Limits::V1).expect("a valid board")
    }

    fn fj_json(v: &fj::Value) -> Value {
        serde_json::from_slice(&fj::to_canonical(v).expect("canon")).expect("json")
    }

    fn obj_json(b: &Board, id: &str) -> Value {
        fj_json(&b.objects.iter().find(|o| o.id.as_str() == id).expect("object").to_value())
    }

    fn fields_of(m: &Map<String, Value>) -> fj::Fields {
        match fj::parse(Value::Object(m.clone()).to_string().as_bytes(), &Limits::V1).expect("parses") {
            fj::Value::Obj(f) => f,
            other => panic!("not an object: {other:?}"),
        }
    }

    fn try_apply(b: &Board, id: &str, edit: &FieldEdit, by: Option<&str>) -> Result<Board, Conflict> {
        let mut out = b.clone();
        let o = out.objects.iter().find(|o| o.id.as_str() == id).expect("object");
        let rev: Rev = o.rev().expect("rev");
        let g = Group {
            id: "g".into(),
            by: by.map(|t| Id::new(t).expect("token")),
            ops: vec![Op::Set { id: Id::new(id).expect("id"), rev, set: fields_of(&edit.set), unset: edit.unset.clone() }],
        };
        apply_group(&mut out, &g)?;
        Ok(out)
    }

    fn apply(b: &Board, id: &str, edit: &FieldEdit, by: Option<&str>) -> Board {
        try_apply(b, id, edit, by).expect("applies")
    }

    /// Test 1 in §5.1: on every object of every pair, an edit of one field
    /// changes only that field's bytes.
    #[test]
    fn an_edit_of_one_field_changes_only_that_fields_bytes_on_every_pairs_objects() {
        for (name, v) in pairs() {
            let mut b = Board::from_json(v["file"].to_string().as_bytes(), &Limits::V1).expect("valid");
            b.people.entry(Id::new(ANN).expect("t")).or_default();
            for o in b.objects.clone() {
                let id = o.id.as_str().to_string();
                let before = obj_json(&b, &id);
                let edit = widget_set_to_file(o.kind.as_str(), json!({ "rot": 12 }).as_object().expect("o"), &Map::new()).expect("maps");
                let after = obj_json(&apply(&b, &id, &edit, Some(ANN)), &id);
                let mut expected = before.clone();
                expected["rot"] = json!(12);
                assert_eq!(after, expected, "{name}/{id}");
            }
        }
    }

    #[test]
    fn a_rounded_number_and_unknown_fields_stay_when_another_field_changes() {
        let b = board(json!([{ "id": "s-1", "type": "sticky", "x": 1.25, "y": 0.30000000000000004, "text": "a", "zz_later": { "keep": [1, 2] }, "Foo": 9 }]));
        let w = object_to_widget(&obj_json(&b, "s-1"), 1.0, None);
        assert_eq!(w["x"], json!(1.25), "the widget gets the file's number, and rounds only what it edits");
        let edit = widget_set_to_file("sticky", json!({ "text": "b" }).as_object().expect("o"), &Map::new()).expect("maps");
        let got = obj_json(&apply(&b, "s-1", &edit, None), "s-1");
        assert_eq!(got, json!({ "id": "s-1", "type": "sticky", "x": 1.25, "y": 0.30000000000000004, "text": "b", "zz_later": { "keep": [1, 2] }, "Foo": 9 }));
    }

    #[test]
    fn a_table_edit_keeps_unknown_row_and_column_fields_and_can_empty_a_cell() {
        let t = json!([{ "id": "b-1", "type": "table",
            "columns": [{ "id": "cl-0", "width": 40, "later": 3 }, { "id": "cl-1" }],
            "rows": [{ "id": "rw-0", "cells": { "cl-0": "a", "cl-1": "b" }, "later": 4 }, { "id": "rw-1", "cells": { "cl-1": "c" } }] }]);
        let b = board(t);
        let base_v = obj_json(&b, "b-1");
        let base = base_v.as_object().expect("o");
        let w = object_to_widget(&base_v, 1.0, None);
        assert_eq!(w["rows"], json!([["a", "b"], ["", "c"]]));
        assert_eq!(w["rowIds"], json!(["rw-0", "rw-1"]));
        assert_eq!(w["colIds"], json!(["cl-0", "cl-1"]));
        assert_eq!(w["colW"], json!([40]));
        // Empty a row's every cell, and add a row and a column, as the toolbar does.
        let f = json!({ "rows": [["", "", ""], ["", "c", ""], ["x", "", "n"]], "rowIds": ["rw-0", "rw-1", "rw-22222222"], "colIds": ["cl-0", "cl-1", "cl-33333333"], "colW": [40, 60, 80] });
        let edit = widget_set_to_file("table", f.as_object().expect("o"), base).expect("maps");
        let got = obj_json(&apply(&b, "b-1", &edit, None), "b-1");
        assert_eq!(got["rows"][0]["later"], 4, "{got}");
        assert_eq!(got["rows"][0]["cells"], json!({ "cl-0": "", "cl-1": "", "cl-33333333": "" }));
        assert_eq!(got["columns"][0]["later"], 3, "{got}");
        assert_eq!(got["columns"][2], json!({ "id": "cl-33333333", "width": 80 }));
        // Remove the last column: rows and ids together, so nothing names a gone column.
        let f = json!({ "rows": [["a"], [""]], "rowIds": ["rw-0", "rw-1"], "colIds": ["cl-0"], "colW": [40] });
        let edit = widget_set_to_file("table", f.as_object().expect("o"), base).expect("maps");
        let got = obj_json(&apply(&b, "b-1", &edit, None), "b-1");
        assert_eq!(got["rows"][1]["cells"], json!({ "cl-0": "" }));
        assert_eq!(got["columns"], json!([{ "id": "cl-0", "later": 3, "width": 40 }]));
        // A resize alone repeats the ids it needs from the file.
        let edit = widget_set_to_file("table", json!({ "colW": [50, 70] }).as_object().expect("o"), base).expect("maps");
        let got = obj_json(&apply(&b, "b-1", &edit, None), "b-1");
        assert_eq!(got["columns"], json!([{ "id": "cl-0", "later": 3, "width": 50 }, { "id": "cl-1", "width": 70 }]));
    }

    #[test]
    fn a_stale_table_edit_that_names_too_few_ids_refuses() {
        let base = json!({ "columns": [{ "id": "cl-0" }], "rows": [{ "id": "rw-0" }] });
        let e = widget_set_to_file("table", json!({ "rows": [["a", "b"]] }).as_object().expect("o"), base.as_object().expect("o")).expect_err("too wide");
        assert_eq!(e.field, "colIds");
        let e = widget_set_to_file("table", json!({ "rows": [["a"], ["b"]] }).as_object().expect("o"), base.as_object().expect("o")).expect_err("too long");
        assert_eq!(e.field, "rowIds");
    }

    #[test]
    fn checks_and_replies_keep_unknown_sub_fields() {
        let b = board(json!([
            { "id": "k-1", "type": "card", "checks": [{ "id": "ck-1", "text": "a", "done": false, "later": 1 }] },
            { "id": "q-1", "type": "comment", "replies": [{ "id": "rp-1", "by": BOT, "at": "2026-11-03T10:00:00Z", "text": "x", "later": 2 }] },
        ]));
        let edit = widget_set_to_file("card", json!({ "checks": [{ "id": "ck-1", "text": "a2", "done": true }] }).as_object().expect("o"), &Map::new()).expect("maps");
        assert_eq!(obj_json(&apply(&b, "k-1", &edit, None), "k-1")["checks"], json!([{ "id": "ck-1", "text": "a2", "done": true, "later": 1 }]));
        let f = json!({ "replies": [{ "id": "rp-1", "by": BOT, "at": "2026-11-03T10:00:00Z", "text": "x" }, { "id": "rp-2", "by": ANN, "at": "2026-11-03T11:00:00.000Z", "text": "y" }] });
        let edit = widget_set_to_file("comment", f.as_object().expect("o"), &Map::new()).expect("maps");
        let got = obj_json(&apply(&b, "q-1", &edit, None), "q-1");
        assert_eq!(got["replies"][0]["later"], 2);
        assert_eq!(got["replies"][1]["text"], "y");
    }

    #[test]
    fn none_is_absent_and_a_ref_becomes_a_blob() {
        let h = format!("sha256-{}", "a".repeat(64));
        let f = json!({ "group": "", "parent": "", "link": null, "crop": null, "checks": null, "src": format!("k2blob:{h}"), "zz": format!("k2blob:{h}"), "frame": "" });
        let e = widget_set_to_file("image", f.as_object().expect("o"), &Map::new()).expect("maps");
        let mut un = e.unset.clone();
        un.sort();
        assert_eq!(un, ["checks", "crop", "frame", "group", "link", "parent"]);
        assert_eq!(e.set["src"], json!({ "blob": h }));
        assert_eq!(e.set["zz"], json!({ "blob": h }));
        let e = widget_set_to_file("image", json!({ "src": "" }).as_object().expect("o"), &Map::new()).expect("maps");
        assert_eq!(e.unset, ["src"]);
        let e = widget_set_to_file("image", json!({ "src": "data:image/png;base64,AAAA" }).as_object().expect("o"), &Map::new()).expect_err("no data urls");
        assert_eq!(e.field, "src");
    }

    #[test]
    fn a_pen_never_writes_its_box_and_a_widget_only_field_never_reaches_the_file() {
        let f = json!({ "x": 5, "w": 9, "h": 9, "z": 3, "order": 2, "points": [[0, 0], [1, 1]], "updatedAt": "t", "batch": "b" });
        let e = widget_set_to_file("pen", f.as_object().expect("o"), &Map::new()).expect("maps");
        assert_eq!(Value::Object(e.set), json!({ "x": 5, "points": [[0, 0], [1, 1]] }));
    }

    #[test]
    fn a_new_object_is_a_file_object_with_who_made_it() {
        let obj = json!({ "id": "s-0123456789", "type": "sticky", "x": 1, "text": "t", "z": 4, "group": "", "createdBy": "x", "updatedAt": "y" });
        let got = widget_add_to_file(obj.as_object().expect("o"), Some(json!({ "by": ANN, "at": "2026-11-03T10:00:00.000Z" }))).expect("maps");
        assert_eq!(got, json!({ "id": "s-0123456789", "type": "sticky", "x": 1, "text": "t", "created": { "by": ANN, "at": "2026-11-03T10:00:00.000Z" } }));
    }

    // ---------- Garden out ----------

    #[test]
    fn a_group_that_isnt_an_id_maps_to_one_stable_id() {
        assert_eq!(group_id("g-aa18e7c1e9"), "g-aa18e7c1e9", "one that fits stays");
        let a = group_id("Team A");
        assert_eq!(a, group_id("Team A"));
        assert_ne!(a, group_id("Team B"));
        assert!(is_id(&a), "{a}");
        let long = group_id(&"g".repeat(40));
        assert!(is_id(&long), "{long}");
    }

    #[test]
    fn a_gardens_real_forms_become_a_board_with_random_tokens_and_no_host_form() {
        let h = format!("sha256-{}", "b".repeat(64));
        let objects = vec![
            json!({ "id": "s-0000000001", "type": "sticky", "z": 2, "group": "Team A", "createdBy": "person:owner", "createdAt": "2026-11-03T10:00:00.000Z",
                    "updatedBy": "agent:pat", "updatedAt": "2026-11-03T11:00:00.000Z", "batch": "w-1",
                    "reactions": { "person:owner": ["🔥", "👍"], "agent:pat": ["👍"], "Pat Older": ["⭐"] }, "votes": { "agent:pat": 2, "person:owner": 0 }, "tags": ["a", 7] }),
            json!({ "id": "s-0000000002", "type": "sticky", "z": 1, "group": "Team A" }),
            json!({ "id": "i-0000000001", "type": "image", "z": 3, "src": format!("k2blob:{h}"), "note": format!("k2blob:{h}") }),
            json!({ "id": "q-0000000001", "type": "comment", "z": 4, "replies": [{ "id": "rp-1", "by": "agent:pat", "at": "2026-11-03T10:00:00.000Z", "text": "x" }], "target": "s-0000000001" }),
            json!({ "id": "b-0000000001", "type": "table", "z": 5, "rows": [["a"], ["b", "c"]] }),
        ];
        let mut n = 0u8;
        let mut rnd = || {
            n += 1;
            [n; 8]
        };
        let mut people = People::new(&mut rnd, "Rosson");
        let mut blobs = BTreeSet::new();
        let doc = garden_board_to_file(&objects, &json!({ "format": 1, "title": "T", "batch": "w-1", "voting": { "active": false } }), &mut people, &mut blobs).expect("maps");
        let text = doc.to_string();
        for banned in ["k2blob", "person:", "agent:", "updatedBy", "batch", "\"p1\"", "createdBy", "\"z\""] {
            assert!(!text.contains(banned), "{banned} in {text}");
        }
        assert!(text.contains("\"name\":\"Rosson\""), "{text}");
        assert!(text.contains("\"kind\":\"agent\""), "{text}");
        assert!(text.contains("Pat Older"), "an older plain name stays a person's name");
        assert_eq!(blobs.iter().collect::<Vec<_>>(), vec![&h]);
        let b = Board::from_json(text.as_bytes(), &Limits::V1).expect("the codec takes it");
        assert_eq!(b.objects.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(), ["s-0000000002", "s-0000000001", "i-0000000001", "q-0000000001", "b-0000000001"]);
        let g1 = fj_json(b.objects[0].fields.get("group").expect("group"));
        assert_eq!(g1, fj_json(b.objects[1].fields.get("group").expect("group")), "one label stays one group");
        assert_eq!(g1, json!(group_id("Team A")));
        assert_eq!(fj_json(b.objects[1].fields.get("tags").expect("tags")), json!(["a", "7"]));
    }

    #[test]
    fn every_template_from_k2s_own_table_reads_as_a_file() {
        for (name, _) in k2_core::board::templates::TEMPLATES {
            let list = k2_core::board::templates::build(name, 100.0, 200.0).expect("template");
            let mut n = 0u8;
            let mut rnd = || {
                n += 1;
                [n; 8]
            };
            let mut people = People::new(&mut rnd, "Rosson");
            let doc = garden_board_to_file(&list, &json!({}), &mut people, &mut BTreeSet::new()).expect("maps");
            Board::from_json(doc.to_string().as_bytes(), &Limits::V1).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}
