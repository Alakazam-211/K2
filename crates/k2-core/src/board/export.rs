//! The Board's JSON export (`k2-board`, version 1): the same envelope the
//! widget's in-frame export writes. It's K2's own, for the Garden. It is
//! **not** the board file: `.k5board` is another format, which K2 Cores'
//! codec converts to and from the Garden's keys.
//!
//! ```text
//! { "format": "k2-board", "version": 1, "title": …,
//!   "meta": {voting?, timer?},
//!   "objects": [ … ],   // by (z, id); no `batch`
//!   "people": { "p1": {"name", "kind": "person"|"agent"}, … },
//!   "blobs": { "sha256-<hex>": {"mime", "name", "data": <base64>} } }
//! ```
//!
//! **No K2 identifiers** in it: no Garden board id, project, `k2blob:`
//! scheme or K2 person key. An image names its bytes `{"blob":
//! "sha256-<hex>"}`; people are per-file tokens `p1`, `p2`, … given in
//! order of first appearance (objects in order; in each, `createdBy`,
//! `updatedBy`, reaction keys, vote keys (each sorted), then reply
//! authors).

use std::collections::BTreeMap;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::ops::Board;
use super::{schema, BoardError, FORMAT};

pub const ENVELOPE_FORMAT: &str = "k2-board";

/// One blob's bytes as the store keeps them.
pub struct BlobBytes {
    pub mime: String,
    pub name: Option<String>,
    pub bytes: Vec<u8>,
}

struct People {
    tokens: BTreeMap<String, String>,
    table: Map<String, Value>,
}

impl People {
    fn token(&mut self, key: &str, name_of: &dyn Fn(&str) -> (String, &'static str)) -> String {
        if let Some(t) = self.tokens.get(key) {
            return t.clone();
        }
        let t = format!("p{}", self.tokens.len() + 1);
        let (name, kind) = name_of(key);
        self.tokens.insert(key.to_string(), t.clone());
        self.table.insert(t.clone(), json!({ "name": name, "kind": kind }));
        t
    }
}

/// The objects in export order: by `z`, then id.
pub fn ordered(board: &Board) -> Vec<Value> {
    let mut objs: Vec<Value> = board.objects.values().map(|s| schema::read_shape(&s.value)).collect();
    objs.sort_by(|a, b| {
        let za = a.get("z").and_then(Value::as_f64).unwrap_or(0.0);
        let zb = b.get("z").and_then(Value::as_f64).unwrap_or(0.0);
        za.partial_cmp(&zb).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
            a.get("id").and_then(Value::as_str).unwrap_or("").cmp(b.get("id").and_then(Value::as_str).unwrap_or(""))
        })
    });
    objs
}

fn data_url(src: &str) -> Option<(String, Vec<u8>)> {
    let rest = src.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?;
    (!bytes.is_empty()).then(|| (mime.to_string(), bytes))
}

/// Turn every exact `k2blob:` string under `v` into `{"blob": …}`, adding
/// its bytes to `blobs` once.
fn blobify(
    v: &mut Value,
    blobs: &mut Map<String, Value>,
    blob: &mut dyn FnMut(&str) -> Result<BlobBytes, BoardError>,
    fail: &mut Option<BoardError>,
) {
    match v {
        Value::String(s) if schema::is_blob_ref(s) => {
            let hex = s[crate::garden_data::blobs::REF_PREFIX.len()..].to_string();
            let name = format!("sha256-{hex}");
            if !blobs.contains_key(&name) {
                match blob(&hex) {
                    Ok(b) => {
                        blobs.insert(
                            name.clone(),
                            json!({ "mime": b.mime, "name": b.name, "data": base64::engine::general_purpose::STANDARD.encode(&b.bytes) }),
                        );
                    }
                    Err(e) => {
                        fail.get_or_insert(e);
                        return;
                    }
                }
            }
            *v = json!({ "blob": name });
        }
        Value::Array(a) => a.iter_mut().for_each(|x| blobify(x, blobs, blob, fail)),
        Value::Object(m) => m.iter_mut().for_each(|(_, x)| blobify(x, blobs, blob, fail)),
        _ => {}
    }
}

/// Build the envelope. `name_of(person key)` → (display name, kind);
/// `blob(hash hex)` → the bytes of `k2blob:sha256-<hex>`.
pub fn envelope(
    board: &Board,
    title: &str,
    name_of: &dyn Fn(&str) -> (String, &'static str),
    blob: &mut dyn FnMut(&str) -> Result<BlobBytes, BoardError>,
) -> Result<Value, BoardError> {
    // It says version 1, so it can't speak for a board this build can't read.
    board.check_format()?;
    let mut people = People { tokens: BTreeMap::new(), table: Map::new() };
    let mut blobs = Map::new();
    let mut objects = Vec::new();
    for mut v in ordered(board) {
        let Some(o) = v.as_object_mut() else { continue };
        o.remove("batch");
        for k in ["createdBy", "updatedBy"] {
            if let Some(who) = o.get(k).and_then(Value::as_str).map(str::to_string) {
                o.insert(k.into(), json!(people.token(&who, name_of)));
            }
        }
        for k in ["reactions", "votes"] {
            if let Some(Value::Object(m)) = o.get(k) {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for who in keys {
                    out.insert(people.token(who, name_of), m[who].clone());
                }
                o.insert(k.into(), Value::Object(out));
            }
        }
        if let Some(Value::Array(rs)) = o.get_mut("replies") {
            for r in rs.iter_mut() {
                if let Some(who) = r.get("by").and_then(Value::as_str).map(str::to_string) {
                    r["by"] = json!(people.token(&who, name_of));
                }
            }
        }
        // Any exact `k2blob:` string anywhere in the object becomes
        // `{"blob": "sha256-…"}` (the widget's rule); a legacy `data:` image
        // source too. The first object in order names a shared blob.
        let mut fail = None;
        for (_, val) in o.iter_mut() {
            blobify(val, &mut blobs, blob, &mut fail);
        }
        if let Some(e) = fail {
            return Err(e);
        }
        if o.get("type").and_then(Value::as_str) == Some("image") {
            let src = o.get("src").and_then(Value::as_str).unwrap_or("").to_string();
            if let Some((mime, bytes)) = data_url(&src) {
                let hex: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
                let name = format!("sha256-{hex}");
                if !blobs.contains_key(&name) {
                    blobs.insert(name.clone(), json!({ "mime": mime, "name": Value::Null, "data": base64::engine::general_purpose::STANDARD.encode(&bytes) }));
                }
                o.insert("src".into(), json!({ "blob": name }));
            }
        }
        objects.push(v);
    }
    let mut meta = board.meta_value();
    for k in ["format", "title", "batch"] {
        meta.remove(k);
    }
    Ok(json!({
        "format": ENVELOPE_FORMAT,
        "version": FORMAT,
        "title": title,
        "meta": meta,
        "objects": objects,
        "people": people.table,
        "blobs": blobs,
    }))
}
