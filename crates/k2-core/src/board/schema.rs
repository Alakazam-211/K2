//! The Board's object schema, format 1: types, fields, kinds, defaults and
//! the checks every write passes (`k2 board`, and the widget's `schema.js`
//! holds the same table).
//!
//! **No silent loss** (design-board-file-type §4.2, Garden work now): a
//! value over its limit is refused with a typed error, never cut; a list
//! over its cap is refused, never shortened. Numbers round to 0.1 (the
//! canonical precision; pen points to whole units).

use serde_json::{json, Map, Value};

use super::BoardError;

/// Sticky colours (the widget draws them light and dark).
pub const STICKY: &[&str] = &["yellow", "orange", "pink", "red", "violet", "blue", "cyan", "green", "lime", "gray", "white"];
/// Ink colours for shapes, lines, text and pen (or `#rrggbb`).
pub const INK: &[&str] = &["ink", "gray", "red", "orange", "yellow", "green", "teal", "blue", "violet", "pink", "white"];
/// Fills for shapes and frames (or `none`, or `#rrggbb`).
pub const FILL: &[&str] = &["white", "gray", "red", "orange", "yellow", "green", "teal", "blue", "violet", "pink", "ink"];
pub const SHAPES: &[&str] = &[
    "rect", "round", "ellipse", "triangle", "diamond", "parallelogram", "trapezoid", "pentagon", "hexagon", "octagon", "star",
    "cross", "arrow-right", "arrow-left", "chevron", "cylinder", "cloud", "speech", "document", "flow-start", "flow-process",
    "flow-decision", "flow-data", "flow-predefined",
];
pub const CAPS: &[&str] = &["none", "arrow", "triangle", "circle", "diamond", "bar"];
pub const ROUTES: &[&str] = &["straight", "elbow", "curved"];
pub const DASH: &[&str] = &["solid", "dashed", "dotted"];
pub const ANCHORS: &[&str] = &["auto", "top", "right", "bottom", "left"];
pub const ALIGN: &[&str] = &["left", "center", "right"];
pub const REACTIONS: &[&str] = &["👍", "❤️", "😂", "🎉", "🤔", "👀", "🔥", "✅", "❌", "❓", "⭐"];

/// One field's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Num,
    Bool,
    /// A string of at most this many code points (as the widget counts).
    Str(usize),
    Enum(&'static [&'static str]),
    Sticky,
    Ink,
    Fill,
    /// Tags: at most 200 items, each a number or a string of ≤ 80.
    List,
    /// Pen points `[[x, y], …]`, at most 5000.
    Points,
    /// Table cells `[[text, …], …]`: ≤ 100 rows, ≤ 30 columns, ≤ 2000 each.
    Rows,
    /// Table row or column ids, kept in step with `rows`.
    Ids(usize),
    /// Column widths (numbers), at most 30.
    Widths,
    /// A connector end: an object id with an anchor, or a free point.
    End,
    /// Checklist items `[{id, text, done}]`, at most 50.
    Checks,
    /// Image crop, fractions of the image.
    Crop,
    /// An http(s) address.
    Link,
    /// Per-person reactions `{person: [emoji]}`.
    Reactions,
    /// Per-person votes `{person: n}`.
    Votes,
    /// Comment replies `[{id, by, at, text}]`.
    Replies,
    /// An image source: a Garden blob ref (`k2blob:sha256-…`).
    ImageSrc,
    /// Board settings JSON (voting, timer), at most 20 KB.
    Json,
}

/// Fields every object has.
pub const COMMON: &[(&str, Kind)] = &[
    ("x", Kind::Num),
    ("y", Kind::Num),
    ("w", Kind::Num),
    ("h", Kind::Num),
    ("rot", Kind::Num),
    ("z", Kind::Num),
    ("locked", Kind::Bool),
    ("group", Kind::Str(40)),
    ("tags", Kind::List),
    ("reactions", Kind::Reactions),
    ("votes", Kind::Votes),
    ("link", Kind::Link),
];

/// Every type: (name, id letter, its own fields).
pub const TYPES: &[(&str, char, &[(&str, Kind)])] = &[
    ("sticky", 's', &[("text", Kind::Str(5000)), ("color", Kind::Sticky), ("fontSize", Kind::Num), ("align", Kind::Enum(ALIGN))]),
    (
        "text",
        't',
        &[
            ("text", Kind::Str(20000)),
            ("color", Kind::Ink),
            ("fontSize", Kind::Num),
            ("bold", Kind::Bool),
            ("italic", Kind::Bool),
            ("align", Kind::Enum(ALIGN)),
            ("checks", Kind::Checks),
        ],
    ),
    (
        "shape",
        'h',
        &[
            ("shape", Kind::Enum(SHAPES)),
            ("text", Kind::Str(5000)),
            ("fill", Kind::Fill),
            ("stroke", Kind::Ink),
            ("strokeWidth", Kind::Num),
            ("dash", Kind::Enum(DASH)),
            ("color", Kind::Ink),
            ("fontSize", Kind::Num),
        ],
    ),
    (
        "connector",
        'c',
        &[
            ("from", Kind::End),
            ("to", Kind::End),
            ("route", Kind::Enum(ROUTES)),
            ("stroke", Kind::Ink),
            ("strokeWidth", Kind::Num),
            ("dash", Kind::Enum(DASH)),
            ("startCap", Kind::Enum(CAPS)),
            ("endCap", Kind::Enum(CAPS)),
            ("label", Kind::Str(500)),
        ],
    ),
    ("pen", 'p', &[("points", Kind::Points), ("stroke", Kind::Ink), ("strokeWidth", Kind::Num), ("highlighter", Kind::Bool)]),
    ("frame", 'f', &[("title", Kind::Str(200)), ("fill", Kind::Fill), ("order", Kind::Num)]),
    (
        "card",
        'k',
        &[
            ("title", Kind::Str(500)),
            ("text", Kind::Str(5000)),
            ("color", Kind::Ink),
            ("assignee", Kind::Str(80)),
            ("due", Kind::Str(40)),
            ("status", Kind::Str(40)),
            ("checks", Kind::Checks),
        ],
    ),
    (
        "table",
        'b',
        &[("rows", Kind::Rows), ("rowIds", Kind::Ids(MAX_ROWS)), ("colIds", Kind::Ids(MAX_COLS)), ("colW", Kind::Widths), ("fill", Kind::Fill)],
    ),
    ("mind", 'm', &[("text", Kind::Str(1000)), ("parent", Kind::Str(40)), ("color", Kind::Ink)]),
    ("comment", 'q', &[("text", Kind::Str(5000)), ("replies", Kind::Replies), ("resolved", Kind::Bool), ("target", Kind::Str(40))]),
    ("image", 'i', &[("src", Kind::ImageSrc), ("alt", Kind::Str(300)), ("crop", Kind::Crop)]),
];

/// Board settings (`board/<b>/meta`).
pub const META: &[(&str, Kind)] = &[("voting", Kind::Json), ("timer", Kind::Json), ("title", Kind::Str(200))];

pub const MAX_TAGS: usize = 200;
pub const MAX_POINTS: usize = 5000;
pub const MAX_ROWS: usize = 100;
pub const MAX_COLS: usize = 30;
pub const MAX_CHECKS: usize = 50;
pub const MAX_REPLIES: usize = 200;
pub const MAX_JSON: usize = 20_000;
/// A stored object (one key's value) is at most this big.
pub const MAX_VALUE: usize = 60 * 1024;

/// Fields where null means "none": the key is removed, not kept as null.
pub const NULL_IS_NONE: &[&str] = &["checks", "crop", "link"];

/// Bookkeeping kept with an object, not schema fields.
pub const OWN: &[&str] = &["batch", "createdBy", "createdAt", "updatedBy", "updatedAt"];

/// The defaults a new object of `ty` starts from.
pub fn defaults(ty: &str) -> Value {
    match ty {
        "sticky" => json!({ "w": 200, "h": 200, "text": "", "color": "yellow", "fontSize": 0, "align": "center" }),
        "text" => json!({ "w": 240, "h": 40, "text": "Text", "color": "ink", "fontSize": 24, "align": "left" }),
        "shape" => json!({ "w": 180, "h": 120, "shape": "round", "text": "", "fill": "white", "stroke": "ink", "strokeWidth": 2, "dash": "solid", "color": "ink", "fontSize": 18 }),
        "connector" => json!({ "w": 0, "h": 0, "route": "elbow", "stroke": "ink", "strokeWidth": 2, "dash": "solid", "startCap": "none", "endCap": "arrow", "label": "" }),
        "pen" => json!({ "w": 1, "h": 1, "points": [], "stroke": "ink", "strokeWidth": 3, "highlighter": false }),
        "frame" => json!({ "w": 960, "h": 600, "title": "Frame", "fill": "white", "order": 0 }),
        "card" => json!({ "w": 280, "h": 150, "title": "Card", "text": "", "color": "blue", "assignee": "", "due": "", "status": "" }),
        "table" => json!({ "w": 480, "h": 160, "rows": [["", "", ""], ["", "", ""], ["", "", ""]], "colW": [], "fill": "white" }),
        "mind" => json!({ "w": 180, "h": 48, "text": "Idea", "parent": "", "color": "violet" }),
        "comment" => json!({ "w": 32, "h": 32, "text": "", "replies": [], "resolved": false, "target": "" }),
        "image" => json!({ "w": 320, "h": 240, "src": "", "alt": "" }),
        _ => json!({}),
    }
}

/// Every type name, in the widget's order.
pub fn type_names() -> Vec<&'static str> {
    TYPES.iter().map(|(n, _, _)| *n).collect()
}

/// The id letter of a type (`s` for sticky).
pub fn prefix(ty: &str) -> Option<char> {
    TYPES.iter().find(|(n, _, _)| *n == ty).map(|(_, p, _)| *p)
}

/// A type's whole field table (common first).
pub fn fields(ty: &str) -> Option<Vec<(&'static str, Kind)>> {
    let own = TYPES.iter().find(|(n, _, _)| *n == ty)?.2;
    Some(COMMON.iter().chain(own.iter()).copied().collect())
}

/// One field's kind for a type.
pub fn kind_of(ty: &str, field: &str) -> Option<Kind> {
    fields(ty)?.into_iter().find(|(f, _)| *f == field).map(|(_, k)| k)
}

/// A random lower-hex string of `n` characters.
pub fn hex(n: usize) -> String {
    let mut b = vec![0u8; n.div_ceil(2)];
    getrandom::getrandom(&mut b).expect("the OS random source");
    let s: String = b.iter().map(|x| format!("{x:02x}")).collect();
    s[..n].to_string()
}

/// A new object id for `ty`: `<letter>-<10 hex>`.
pub fn new_id(ty: &str) -> String {
    format!("{}-{}", prefix(ty).unwrap_or('o'), hex(10))
}

/// A new item id inside a field: `<p>-<8 hex>` (`ck`, `rp`, `rw`, `cl`).
pub fn new_item_id(p: &str) -> String {
    format!("{p}-{}", hex(8))
}

/// An id a file can carry (1–24 of `[0-9a-z._-]`).
pub fn valid_item_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-')
}

/// A person key: `person:<id>` or `agent:<handle>`; legacy plain names
/// (older boards) are allowed too (≤ 80, no control characters).
pub fn valid_person(s: &str) -> bool {
    !s.is_empty() && s.chars().count() <= 80 && !s.chars().any(char::is_control)
}

/// A text's length as the widget counts it: Unicode code points
/// (`Array.from(s).length`).
fn text_len(s: &str) -> usize {
    s.chars().count()
}

fn is_hex_colour(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn round1(n: f64) -> Value {
    let r = (n * 10.0).round() / 10.0;
    if r.fract() == 0.0 && r.abs() < 9.0e15 {
        json!(r as i64)
    } else {
        json!(r)
    }
}

fn num_of(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A string from a CLI flag that holds JSON (`'{"x":1}'`, `[...]`).
fn maybe_json(v: &Value, field: &str) -> Result<Value, BoardError> {
    if let Value::String(s) = v {
        let t = s.trim_start();
        if t.starts_with('[') || t.starts_with('{') {
            return serde_json::from_str(s).map_err(|_| BoardError::bad_field(field, "valid JSON"));
        }
    }
    Ok(v.clone())
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn check_str(field: &str, v: &Value, max: usize) -> Result<Value, BoardError> {
    let s = text_of(v);
    if text_len(&s) > max {
        return Err(BoardError::too_long(field, max));
    }
    Ok(Value::String(s))
}

/// Check one field's value; `Ok(None)` means "none" (remove the key).
pub fn check(field: &str, kind: Kind, v: &Value) -> Result<Option<Value>, BoardError> {
    let bad = |why: &str| BoardError::bad_field(field, why);
    Ok(Some(match kind {
        Kind::Num => round1(num_of(v).ok_or_else(|| bad("a number"))?),
        Kind::Bool => match v {
            Value::Bool(b) => Value::Bool(*b),
            Value::String(s) if s == "true" => Value::Bool(true),
            Value::String(s) if s == "false" => Value::Bool(false),
            _ => return Err(bad("true or false")),
        },
        Kind::Str(max) => check_str(field, v, max)?,
        Kind::Enum(list) => match v.as_str() {
            Some(s) if list.contains(&s) => Value::String(s.into()),
            _ => return Err(bad(&format!("one of {}", list.join(", ")))),
        },
        Kind::Sticky => match v.as_str() {
            Some(s) if STICKY.contains(&s) => Value::String(s.into()),
            _ => return Err(bad(&format!("a sticky colour: {}", STICKY.join(", ")))),
        },
        Kind::Ink => match v.as_str() {
            Some(s) if INK.contains(&s) || is_hex_colour(s) => Value::String(s.into()),
            _ => return Err(bad(&format!("a colour: {} or #rrggbb", INK.join(", ")))),
        },
        Kind::Fill => match v.as_str() {
            Some(s) if s == "none" || FILL.contains(&s) || is_hex_colour(s) => Value::String(s.into()),
            _ => return Err(bad(&format!("a fill: none, {} or #rrggbb", FILL.join(", ")))),
        },
        Kind::List => {
            let v = match v {
                Value::String(s) if !s.trim_start().starts_with('[') => {
                    Value::Array(s.split(',').map(str::trim).filter(|x| !x.is_empty()).map(|x| Value::String(x.into())).collect())
                }
                other => maybe_json(other, field)?,
            };
            let a = v.as_array().ok_or_else(|| bad("a list"))?;
            if a.len() > MAX_TAGS {
                return Err(BoardError::too_many(field, MAX_TAGS));
            }
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                out.push(match x {
                    Value::Number(_) => x.clone(),
                    other => check_str(field, other, 80)?,
                });
            }
            Value::Array(out)
        }
        Kind::Points => {
            let v = maybe_json(v, field)?;
            let a = v.as_array().ok_or_else(|| bad("a list of [x, y]"))?;
            if a.len() > MAX_POINTS {
                return Err(BoardError::too_many(field, MAX_POINTS));
            }
            let mut out = Vec::with_capacity(a.len());
            for p in a {
                let p = p.as_array().filter(|p| p.len() >= 2).ok_or_else(|| bad("a list of [x, y]"))?;
                let x = num_of(&p[0]).ok_or_else(|| bad("a list of [x, y]"))?;
                let y = num_of(&p[1]).ok_or_else(|| bad("a list of [x, y]"))?;
                out.push(json!([x.round() as i64, y.round() as i64]));
            }
            Value::Array(out)
        }
        Kind::Rows => {
            let v = maybe_json(v, field)?;
            let rows = v.as_array().ok_or_else(|| bad("a list of rows"))?;
            if rows.len() > MAX_ROWS {
                return Err(BoardError::too_many(field, MAX_ROWS));
            }
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let r = r.as_array().ok_or_else(|| bad("a list of rows"))?;
                if r.len() > MAX_COLS {
                    return Err(BoardError::too_many("columns", MAX_COLS));
                }
                let mut row = Vec::with_capacity(r.len());
                for c in r {
                    row.push(check_str(field, c, 2000)?);
                }
                out.push(Value::Array(row));
            }
            Value::Array(out)
        }
        Kind::Ids(max) => {
            let v = maybe_json(v, field)?;
            let a = v.as_array().ok_or_else(|| bad("a list of ids"))?;
            if a.len() > max {
                return Err(BoardError::too_many(field, max));
            }
            let mut seen = std::collections::HashSet::new();
            for x in a {
                match x.as_str() {
                    Some(s) if valid_item_id(s) && seen.insert(s) => {}
                    _ => return Err(bad("a list of unique ids (1–24 of a-z, 0-9, . _ -)")),
                }
            }
            v.clone()
        }
        Kind::Widths => {
            let v = maybe_json(v, field)?;
            let a = v.as_array().ok_or_else(|| bad("a list of column widths"))?;
            if a.len() > MAX_COLS {
                return Err(BoardError::too_many(field, MAX_COLS));
            }
            let mut out = Vec::with_capacity(a.len());
            for x in a {
                out.push(round1(num_of(x).ok_or_else(|| bad("a list of column widths"))?));
            }
            Value::Array(out)
        }
        Kind::End => {
            let v = maybe_json(v, field)?;
            match &v {
                Value::String(id) => {
                    if text_len(id) > 40 {
                        return Err(BoardError::too_long(field, 40));
                    }
                    json!({ "id": id, "anchor": "auto" })
                }
                Value::Object(o) if o.get("id").is_some_and(|i| !i.is_null()) => {
                    let id = text_of(&o["id"]);
                    if text_len(&id) > 40 {
                        return Err(BoardError::too_long(field, 40));
                    }
                    let anchor = o.get("anchor").and_then(Value::as_str).unwrap_or("auto");
                    if !ANCHORS.contains(&anchor) {
                        return Err(bad(&format!("an anchor: {}", ANCHORS.join(", "))));
                    }
                    let mut e = json!({ "id": id, "anchor": anchor });
                    // The last point it was drawn at: where the end shows
                    // (faded) if its object goes.
                    for q in ["x", "y"] {
                        if let Some(n) = o.get(q).filter(|v| !v.is_null()) {
                            e[q] = round1(num_of(n).ok_or_else(|| bad(&format!("{q}: a number")))?);
                        }
                    }
                    e
                }
                Value::Object(o) => {
                    let x = o.get("x").and_then(num_of).ok_or_else(|| bad("an object id, {\"id\":..,\"anchor\":..} or {\"x\":..,\"y\":..}"))?;
                    let y = o.get("y").and_then(num_of).ok_or_else(|| bad("an object id, {\"id\":..,\"anchor\":..} or {\"x\":..,\"y\":..}"))?;
                    json!({ "x": round1(x), "y": round1(y) })
                }
                _ => return Err(bad("an object id, {\"id\":..,\"anchor\":..} or {\"x\":..,\"y\":..}")),
            }
        }
        Kind::Checks => {
            if v.is_null() {
                return Ok(None);
            }
            let v = maybe_json(v, field)?;
            let a = v.as_array().ok_or_else(|| bad("a list of {\"text\":..,\"done\":true|false}"))?;
            if a.len() > MAX_CHECKS {
                return Err(BoardError::too_many(field, MAX_CHECKS));
            }
            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::with_capacity(a.len());
            for c in a {
                let (id, text, done) = match c {
                    Value::Object(o) => (
                        o.get("id").and_then(Value::as_str).map(str::to_string),
                        o.get("text").map(text_of).unwrap_or_default(),
                        matches!(o.get("done"), Some(Value::Bool(true))) || o.get("done").and_then(Value::as_str) == Some("true"),
                    ),
                    other => (None, text_of(other), false),
                };
                if text_len(&text) > 500 {
                    return Err(BoardError::too_long("checks.text", 500));
                }
                let id = match id {
                    Some(i) if valid_item_id(&i) && !seen.contains(&i) => i,
                    Some(_) => return Err(bad("checklist items with unique ids (1–24 of a-z, 0-9, . _ -)")),
                    None => new_item_id("ck"),
                };
                seen.insert(id.clone());
                out.push(json!({ "id": id, "text": text, "done": done }));
            }
            Value::Array(out)
        }
        Kind::Crop => {
            if v.is_null() {
                return Ok(None);
            }
            let v = maybe_json(v, field)?;
            let o = v.as_object().ok_or_else(|| bad("{\"x\":..,\"y\":..,\"w\":..,\"h\":..}, fractions of the image from 0 to 1"))?;
            let mut c = Map::new();
            for q in ["x", "y", "w", "h"] {
                let x = o.get(q).and_then(num_of).ok_or_else(|| bad(&format!("crop.{q} a number from 0 to 1")))?;
                if !(0.0..=1.0).contains(&x) {
                    return Err(bad(&format!("crop.{q} a number from 0 to 1")));
                }
                c.insert(q.into(), json!((x * 10000.0).round() / 10000.0));
            }
            let f = |q: &str| c[q].as_f64().unwrap_or(0.0);
            if f("w") < 0.01 || f("h") < 0.01 || f("x") + f("w") > 1.0001 || f("y") + f("h") > 1.0001 {
                return Err(bad("a crop inside the image"));
            }
            Value::Object(c)
        }
        Kind::Link => {
            let s = text_of(v);
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            if text_len(s) > 2000 {
                return Err(BoardError::too_long(field, 2000));
            }
            let lower = s.to_ascii_lowercase();
            let ok = (lower.starts_with("http://") && s.len() > 7 || lower.starts_with("https://") && s.len() > 8)
                && !s.chars().any(char::is_whitespace);
            if !ok {
                return Err(bad("an http:// or https:// address"));
            }
            Value::String(s.into())
        }
        Kind::Reactions => {
            let v = maybe_json(v, field)?;
            let o = v.as_object().ok_or_else(|| bad("{\"<person>\": [\"👍\", …]}"))?;
            let mut out = Map::new();
            for (who, list) in o {
                if REACTIONS.contains(&who.as_str()) {
                    return Err(bad("per person: {\"<person>\": [\"👍\", …]} (the older {\"👍\": [names]} shape is read, never written)"));
                }
                if !valid_person(who) {
                    return Err(bad("person keys like agent:<handle> or person:owner"));
                }
                let a = list.as_array().ok_or_else(|| bad("{\"<person>\": [\"👍\", …]}"))?;
                let mut es: Vec<Value> = Vec::new();
                for e in a {
                    match e.as_str() {
                        Some(s) if REACTIONS.contains(&s) => {
                            if !es.iter().any(|x| x.as_str() == Some(s)) {
                                es.push(Value::String(s.into()));
                            }
                        }
                        _ => return Err(bad(&format!("reactions from {}", REACTIONS.join(" ")))),
                    }
                }
                if !es.is_empty() {
                    out.insert(who.clone(), Value::Array(es));
                }
            }
            Value::Object(out)
        }
        Kind::Votes => {
            let v = maybe_json(v, field)?;
            let o = v.as_object().ok_or_else(|| bad("{\"<person>\": n}"))?;
            let mut out = Map::new();
            for (who, n) in o {
                if !valid_person(who) {
                    return Err(bad("person keys like agent:<handle> or person:owner"));
                }
                let n = n.as_u64().filter(|n| *n <= 1000).ok_or_else(|| bad("a whole number of votes from 0 to 1000"))?;
                if n > 0 {
                    out.insert(who.clone(), json!(n));
                }
            }
            Value::Object(out)
        }
        Kind::Replies => {
            let v = maybe_json(v, field)?;
            let a = v.as_array().ok_or_else(|| bad("a list of {\"by\":..,\"at\":..,\"text\":..}"))?;
            if a.len() > MAX_REPLIES {
                return Err(BoardError::too_many(field, MAX_REPLIES));
            }
            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::with_capacity(a.len());
            for r in a {
                let o = r.as_object().ok_or_else(|| bad("a list of {\"by\":..,\"at\":..,\"text\":..}"))?;
                let text = o.get("text").map(text_of).unwrap_or_default();
                if text_len(&text) > 5000 {
                    return Err(BoardError::too_long("replies.text", 5000));
                }
                let by = o.get("by").map(text_of).unwrap_or_default();
                if !valid_person(&by) {
                    return Err(bad("each reply names who wrote it (by)"));
                }
                let at = o.get("at").map(text_of).unwrap_or_default();
                if at.len() > 40 {
                    return Err(BoardError::too_long("replies.at", 40));
                }
                let id = match o.get("id").and_then(Value::as_str) {
                    Some(i) if valid_item_id(i) && !seen.contains(i) => i.to_string(),
                    Some(_) => return Err(bad("replies with unique ids (1–24 of a-z, 0-9, . _ -)")),
                    None => new_item_id("rp"),
                };
                seen.insert(id.clone());
                out.push(json!({ "id": id, "by": by, "at": at, "text": text }));
            }
            Value::Array(out)
        }
        Kind::ImageSrc => {
            let s = text_of(v);
            if s.is_empty() || is_blob_ref(&s) {
                Value::String(s)
            } else if s.starts_with("data:") {
                return Err(bad(
                    "a Garden blob ref (k2blob:sha256-…): put the file in Board's data first (k2 store blob put <file> --widget k2:board prints the ref); data: images aren't written any more",
                ));
            } else {
                return Err(bad("a Garden blob ref (k2blob:sha256-…)"));
            }
        }
        Kind::Json => {
            let v = maybe_json(v, field)?;
            if v.to_string().len() > MAX_JSON {
                return Err(BoardError::value_too_large(field, MAX_JSON));
            }
            v
        }
    }))
}

/// `k2blob:sha256-<64 lower hex>`.
pub fn is_blob_ref(s: &str) -> bool {
    s.strip_prefix(crate::garden_data::blobs::REF_PREFIX)
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Check fields for a type. An unknown field is refused with the list.
/// `Ok` maps a field to `Some(value)`, or `None` for "remove it".
pub fn clean(ty: &str, f: &Map<String, Value>) -> Result<Vec<(String, Option<Value>)>, BoardError> {
    let spec = fields(ty).ok_or_else(|| BoardError::unknown_type(ty))?;
    let mut out = Vec::new();
    for (k, v) in f {
        if k == "id" || k == "type" {
            continue;
        }
        let Some((_, kind)) = spec.iter().find(|(n, _)| n == k) else {
            return Err(BoardError::new(
                "unknown_field",
                format!("unknown field {k} for {ty} (fields: {})", spec.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")),
            )
            .field(k));
        };
        out.push((k.clone(), check(k, *kind, v)?));
    }
    Ok(out)
}

/// Check board settings.
pub fn clean_meta(f: &Map<String, Value>) -> Result<Vec<(String, Value)>, BoardError> {
    let mut out = Vec::new();
    for (k, v) in f {
        let Some((_, kind)) = META.iter().find(|(n, _)| n == k) else {
            return Err(BoardError::new("unknown_field", format!("unknown board setting {k} (title, voting, timer)")).field(k));
        };
        if let Some(v) = check(k, *kind, v)? {
            out.push((k.clone(), v));
        }
    }
    Ok(out)
}

/// A pen's box from its points: the points shift to start at 0, `x`/`y`
/// move to match and `w`/`h` are their extent (schema.js `normalize`).
pub fn normalize(ty: &str, f: &Map<String, Value>, base: Option<&Map<String, Value>>) -> Map<String, Value> {
    let mut out = f.clone();
    if ty != "pen" {
        return out;
    }
    let Some(pts) = f.get("points") else { return out };
    let pts = match pts {
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(_) => return out,
        },
        v => v.clone(),
    };
    let Some(a) = pts.as_array() else { return out };
    if a.is_empty() {
        return out;
    }
    let mut xy = Vec::with_capacity(a.len());
    for p in a {
        let Some(p) = p.as_array().filter(|p| p.len() >= 2) else { return out };
        let (Some(x), Some(y)) = (num_of(&p[0]), num_of(&p[1])) else { return out };
        xy.push((x, y));
    }
    let x1 = xy.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let y1 = xy.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let x2 = xy.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let y2 = xy.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    out.insert("points".into(), Value::Array(xy.iter().map(|(x, y)| json!([(x - x1).round() as i64, (y - y1).round() as i64])).collect()));
    out.insert("w".into(), json!(((x2 - x1).round() as i64).max(1)));
    out.insert("h".into(), json!(((y2 - y1).round() as i64).max(1)));
    if x1 != 0.0 || y1 != 0.0 {
        let bx = f.get("x").and_then(num_of).or_else(|| base.and_then(|b| b.get("x")).and_then(num_of)).unwrap_or(0.0);
        let by = f.get("y").and_then(num_of).or_else(|| base.and_then(|b| b.get("y")).and_then(num_of)).unwrap_or(0.0);
        out.insert("x".into(), json!(bx + x1));
        out.insert("y".into(), json!(by + y1));
    }
    out
}

/// Keep a table's `rowIds`/`colIds` in step with its `rows`: ids it has
/// stay for the rows and columns still there, new ones are minted, extra
/// ones go. `obj` is the whole object after a write.
pub fn table_ids(obj: &mut Map<String, Value>) {
    if obj.get("type").and_then(Value::as_str) != Some("table") {
        return;
    }
    let rows = obj.get("rows").and_then(Value::as_array).cloned().unwrap_or_default();
    let ncols = rows.iter().filter_map(Value::as_array).map(Vec::len).max().unwrap_or(0);
    let fix = |have: Option<&Value>, n: usize, p: &str| -> Value {
        let mut ids: Vec<Value> = have.and_then(Value::as_array).cloned().unwrap_or_default();
        ids.truncate(n);
        while ids.len() < n {
            ids.push(Value::String(new_item_id(p)));
        }
        Value::Array(ids)
    };
    let r = fix(obj.get("rowIds"), rows.len(), "rw");
    let c = fix(obj.get("colIds"), ncols, "cl");
    obj.insert("rowIds".into(), r);
    obj.insert("colIds".into(), c);
}

/// Read side: a stored object in today's shapes. Older boards' reactions
/// (`{"👍": [names]}`) turn per person; checklist items, replies and table
/// rows and columns without ids get positional ids (`ck-0`, `rp-0`,
/// `rw-0`, `cl-0`), the same the widget gives them. Nothing is written.
pub fn read_shape(v: &Value) -> Value {
    let mut v = v.clone();
    let Some(o) = v.as_object_mut() else { return v };
    if let Some(Value::Object(r)) = o.get("reactions") {
        if r.keys().any(|k| REACTIONS.contains(&k.as_str())) {
            let mut per: Map<String, Value> = Map::new();
            for (emoji, names) in r {
                for n in names.as_array().into_iter().flatten().filter_map(Value::as_str) {
                    let e = per.entry(n.to_string()).or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(a) = e.as_array_mut() {
                        if !a.iter().any(|x| x.as_str() == Some(emoji)) {
                            a.push(Value::String(emoji.clone()));
                        }
                    }
                }
            }
            // In the reaction list's order, so every reader (this one and
            // the widget's) turns the same old value into the same lists.
            let rank = |e: &Value| REACTIONS.iter().position(|r| Some(*r) == e.as_str()).unwrap_or(usize::MAX);
            for list in per.values_mut() {
                if let Some(a) = list.as_array_mut() {
                    a.sort_by_key(rank);
                }
            }
            o.insert("reactions".into(), Value::Object(per));
        }
    }
    for (field, p) in [("checks", "ck"), ("replies", "rp")] {
        if let Some(Value::Array(items)) = o.get_mut(field) {
            for (i, it) in items.iter_mut().enumerate() {
                if let Value::Object(m) = it {
                    if !m.get("id").and_then(Value::as_str).is_some_and(valid_item_id) {
                        m.insert("id".into(), Value::String(format!("{p}-{i}")));
                    }
                }
            }
        }
    }
    if o.get("type").and_then(Value::as_str) == Some("table") {
        let rows = o.get("rows").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        let cols = o.get("rows").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_array).map(Vec::len).max().unwrap_or(0)).unwrap_or(0);
        for (field, n, p) in [("rowIds", rows, "rw"), ("colIds", cols, "cl")] {
            let ok = o.get(field).and_then(Value::as_array).is_some_and(|a| a.len() == n);
            if !ok {
                o.insert(field.into(), Value::Array((0..n).map(|i| Value::String(format!("{p}-{i}"))).collect()));
            }
        }
    }
    v
}

/// What `k2 board fields [<type>]` prints: each type's fields with their
/// kind in words, and its defaults.
pub fn describe(ty: Option<&str>) -> Result<Value, BoardError> {
    let names: Vec<&str> = match ty {
        Some(t) => {
            fields(t).ok_or_else(|| BoardError::unknown_type(t))?;
            vec![t]
        }
        None => type_names(),
    };
    let mut out = Map::new();
    for t in names {
        let mut f = Map::new();
        for (n, k) in fields(t).unwrap_or_default() {
            f.insert(n.into(), Value::String(kind_words(k)));
        }
        out.insert(t.into(), json!({ "idPrefix": format!("{}-", prefix(t).unwrap_or('o')), "fields": f, "defaults": defaults(t) }));
    }
    Ok(Value::Object(out))
}

fn kind_words(k: Kind) -> String {
    match k {
        Kind::Num => "number".into(),
        Kind::Bool => "true|false".into(),
        Kind::Str(n) => format!("text (≤ {n})"),
        Kind::Enum(l) => l.join("|"),
        Kind::Sticky => format!("sticky colour: {}", STICKY.join("|")),
        Kind::Ink => format!("colour: {} or #rrggbb", INK.join("|")),
        Kind::Fill => format!("fill: none|{} or #rrggbb", FILL.join("|")),
        Kind::List => format!("list (≤ {MAX_TAGS}, comma-separated or JSON)"),
        Kind::Points => format!("[[x,y],…] (≤ {MAX_POINTS})"),
        Kind::Rows => format!("[[cell,…],…] (≤ {MAX_ROWS} rows × {MAX_COLS} columns; ids kept in rowIds/colIds)"),
        Kind::Ids(n) => format!("ids (≤ {n}; K2 keeps them in step with rows)"),
        Kind::Widths => format!("[number,…] (≤ {MAX_COLS})"),
        Kind::End => "an object id, {\"id\":..,\"anchor\":auto|top|right|bottom|left} or {\"x\":..,\"y\":..}".into(),
        Kind::Checks => format!("[{{\"text\":..,\"done\":true|false}},…] (≤ {MAX_CHECKS}; ids added) or null"),
        Kind::Crop => "{\"x\",\"y\",\"w\",\"h\"} fractions 0–1, or null".into(),
        Kind::Link => "http(s) address, or null".into(),
        Kind::Reactions => "{\"<person>\": [emoji,…]} (use k2 board react)".into(),
        Kind::Votes => "{\"<person>\": n} (use k2 board vote)".into(),
        Kind::Replies => "[{\"by\",\"at\",\"text\"},…] (ids added)".into(),
        Kind::ImageSrc => "k2blob:sha256-… (k2 store blob put <file> --widget k2:board)".into(),
        Kind::Json => "JSON (≤ 20 KB)".into(),
    }
}
