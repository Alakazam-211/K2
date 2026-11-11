//! The items of a structure read's pages (K-READ-2): each item's text form, rendered from the
//! same value its JSON is, and the shortening that makes any item fit a page.
//!
//! **Every page fits the budget, in text and in JSON.** An item counts as the larger of its
//! two forms, so a page cut once is the same page in both modes, and the cursor means the same
//! thing. An item that couldn't fit a page by itself is shortened, a step at a time, until it
//! does; each step says what it left out and names the read that returns it:
//!
//! 1. a node's text (`text_omitted`, `text_bytes`, `text_read`: `<show> KEY --text`);
//! 2. a node's descendants, cut to the first that fit (`descendants_total`,
//!    `descendants_read`: `<list> --under KEY --depth D`, paged);
//! 3. every field: strings cut to [`FIELD_SHOWN`] bytes (`<field>_bytes`), lists and the
//!    attributes to [`LIST_SHOWN`] entries (`<field>_total`, `attrs_cut`), with `read`: the
//!    read that returns the item whole (`<show> KEY --whole`; a diff's change, both versions;
//!    a blame row, `log`);
//! 4. a stub: the item's key and place, `omitted`, its size, and `read`.
//!
//! A stub is a handful of bounded fields, and [`crate::index::READ_BUDGET_MIN`] leaves room for
//! one beside any frame, so the last step always fits.

use crate::errors::Value;
use crate::store::VersionId;

/// The longest string a shortened item keeps, in bytes.
pub(super) const FIELD_SHOWN: usize = 256;
/// The most entries a shortened item keeps of a list or of its attributes.
pub(super) const LIST_SHOWN: usize = 16;

/// What a read's items are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    /// An outline node (the list verb), indented by depth with `--tree`.
    Outline { tree: bool },
    /// A node read (the show verb).
    Node,
    /// A diff's change.
    Change,
    /// A blame row.
    Blame,
}

/// The reads a shortened item names.
pub(super) struct Hints {
    pub show: &'static str,
    pub list: &'static str,
    /// The version read (a diff: its later one).
    pub version: VersionId,
    /// A diff's earlier version.
    pub from: VersionId,
    /// The depth of descendants asked (a node read).
    pub depth: u16,
}

impl Hints {
    fn whole(&self, key: &str, version: VersionId) -> String {
        format!("{} {key} --version {version} --whole", self.show)
    }
}

/// A JSON string, quoted and escaped.
pub(super) fn quoted(s: &str) -> String {
    Value::from(s).to_json()
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn num_of(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

fn yes(v: &Value, k: &str) -> bool {
    matches!(v.get(k), Some(Value::Bool(true)))
}

fn set(v: &mut Value, k: &str, x: Value) {
    if let Value::Obj(f) = v {
        match f.iter_mut().find(|(n, _)| n == k) {
            Some((_, slot)) => *slot = x,
            None => f.push((k.to_owned(), x)),
        }
    }
}

/// A read hint as text: a string, or several joined.
fn read_text(v: &Value) -> String {
    match v.get("read") {
        Some(Value::Str(s)) => s.clone(),
        Some(Value::List(l)) => l
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("; "),
        _ => String::new(),
    }
}

/// `\t(name cut from N bytes)`, `\twhole: R`: what a shortened item says in text.
fn cut_notes(v: &Value) -> String {
    let mut s = String::new();
    for (k, x) in match v {
        Value::Obj(f) => f.as_slice(),
        _ => &[],
    } {
        if let (Some(field), Some(n)) = (k.strip_suffix("_bytes"), x.as_u64()) {
            if field != "text" && field != "node" {
                s.push_str(&format!("\t({field} cut from {n} bytes)"));
            }
        }
        if let (Some(field), Some(n)) = (k.strip_suffix("_total"), x.as_u64()) {
            if field != "descendants" && field != "missing" && field != "candidates" {
                s.push_str(&format!("\t({field}: {n} in all)"));
            }
        }
    }
    if v.get("read").is_some() {
        s.push_str(&format!("\twhole: {}", read_text(v)));
    }
    s
}

fn outline_line(v: &Value, tree: bool, indent_by: u64) -> String {
    let depth = num_of(v, "depth").unwrap_or(0).saturating_sub(indent_by);
    let indent = if tree {
        "  ".repeat(depth.min(u64::from(crate::index::INDEX_DEPTH_MAX)) as usize)
    } else {
        String::new()
    };
    let pos = num_of(v, "position").unwrap_or(0);
    let key = str_of(v, "key");
    if yes(v, "omitted") {
        return format!(
            "{indent}#{pos}\t{key}\t(left out: {} bytes; read: {})",
            num_of(v, "bytes").unwrap_or(0),
            read_text(v)
        );
    }
    let hidden = if matches!(v.get("visible_in_tree"), Some(Value::Bool(false))) {
        "\thidden"
    } else {
        ""
    };
    format!(
        "{indent}#{pos}\t{key}\t{}\t{}{hidden}{}",
        str_of(v, "kind"),
        quoted(str_of(v, "name")),
        cut_notes(v)
    )
}

fn node_lines(v: &Value) -> Vec<String> {
    let addr = quoted(str_of(v, "address"));
    let Some(n) = v.get("node").filter(|n| !matches!(n, Value::Null)) else {
        return vec![format!("{addr}: names no node")];
    };
    let mut out = Vec::new();
    let pos = num_of(n, "position").unwrap_or(0);
    let key = str_of(n, "key");
    if yes(n, "node_omitted") {
        out.push(format!(
            "#{pos}\t{key}\t{}\t(left out: {} bytes; whole: {})",
            str_of(n, "kind"),
            num_of(n, "node_bytes").unwrap_or(0),
            str_of(n, "node_read")
        ));
    } else {
        let hidden = if matches!(n.get("visible_in_tree"), Some(Value::Bool(false))) {
            "\thidden"
        } else {
            ""
        };
        out.push(format!(
            "#{pos}\t{key}\t{}\t{}{hidden}{}",
            str_of(n, "kind"),
            quoted(str_of(n, "name")),
            cut_notes(n)
        ));
        if let Some(Value::List(a)) = v.get("ancestors") {
            if !a.is_empty() {
                let keys: Vec<&str> = a.iter().map(|x| str_of(x, "key")).collect();
                let more = num_of(v, "ancestors_total")
                    .map(|t| format!(" (of {t})"))
                    .unwrap_or_default();
                out.push(format!("  ancestors: {}{more}", keys.join(" > ")));
            }
        }
        if let Some(b @ Value::Obj(_)) = n.get("bounds") {
            out.push(format!(
                "  bounds: {},{},{},{}",
                b.get("left").map(Value::to_json).unwrap_or_default(),
                b.get("top").map(Value::to_json).unwrap_or_default(),
                b.get("right").map(Value::to_json).unwrap_or_default(),
                b.get("bottom").map(Value::to_json).unwrap_or_default()
            ));
        }
        match n.get("text") {
            Some(Value::Str(t)) => out.push(format!("  text: {}", quoted(t))),
            _ if yes(n, "text_omitted") => out.push(format!(
                "  text: left out ({} bytes); read it with: {}",
                num_of(n, "text_bytes").unwrap_or(0),
                str_of(n, "text_read")
            )),
            _ => {}
        }
        if let Some(a @ Value::Obj(f)) = n.get("attrs") {
            if !f.is_empty() {
                out.push(format!("  attrs: {}", a.to_json()));
            }
        }
        if let Some(Value::List(f)) = n.get("flags") {
            if !f.is_empty() {
                let names: Vec<&str> = f.iter().filter_map(Value::as_str).collect();
                out.push(format!("  flags: {}", names.join(", ")));
            }
        }
    }
    let base = num_of(n, "depth").unwrap_or(0);
    if let Some(Value::List(d)) = v.get("descendants") {
        for x in d {
            out.push(format!("  {}", outline_line(x, true, base)));
        }
    }
    if let Some(t) = num_of(v, "descendants_total") {
        let shown = match v.get("descendants") {
            Some(Value::List(d)) => d.len(),
            _ => 0,
        };
        out.push(format!(
            "  descendants: {shown} of {t} shown; the rest: {}",
            str_of(v, "descendants_read")
        ));
    }
    if v.get("read").is_some() {
        out.push(format!("  whole: {}", read_text(v)));
    }
    out
}

fn change_line(v: &Value) -> String {
    let key = str_of(v, "key");
    let kinds: Vec<&str> = match v.get("changes") {
        Some(Value::List(l)) => l.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    if yes(v, "omitted") {
        return format!(
            "{key}\t{}\t(left out: {} bytes; read: {})",
            kinds.join(","),
            num_of(v, "bytes").unwrap_or(0),
            read_text(v)
        );
    }
    let now = v.get("name_now").and_then(Value::as_str);
    let then = v.get("name_then").and_then(Value::as_str);
    let mut l = format!(
        "{key}\t{}\t{}",
        kinds.join(","),
        quoted(now.or(then).unwrap_or(""))
    );
    if let (Some(now), Some(then)) = (now, then) {
        if now != then {
            l.push_str(&format!("\t(was {})", quoted(then)));
        }
    }
    if let Some(Value::List(p)) = v.get("props") {
        if !p.is_empty() {
            let names: Vec<&str> = p.iter().filter_map(Value::as_str).collect();
            l.push_str(&format!("\tprops: {}", names.join(",")));
        }
    }
    l.push_str(&cut_notes(v));
    l
}

fn blame_line(v: &Value) -> String {
    let key = str_of(v, "key");
    if yes(v, "omitted") {
        return format!(
            "{key}\t(left out: {} bytes; read: {})",
            num_of(v, "bytes").unwrap_or(0),
            read_text(v)
        );
    }
    format!(
        "{key}\tv{}\t{} ({})\t{}{}{}",
        num_of(v, "version").unwrap_or(0),
        quoted(str_of(v, "author_name")),
        str_of(v, "author"),
        str_of(v, "via"),
        if matches!(v.get("settled"), Some(Value::Bool(false))) {
            "\tunsettled"
        } else {
            ""
        },
        cut_notes(v)
    )
}

/// The item's text form: its lines, each ending in a newline.
pub(super) fn text(shape: Shape, v: &Value) -> String {
    let lines = match shape {
        Shape::Outline { tree } => vec![outline_line(v, tree, 0)],
        Shape::Node => node_lines(v),
        Shape::Change => vec![change_line(v)],
        Shape::Blame => vec![blame_line(v)],
    };
    let mut s = String::new();
    for l in lines {
        s.push_str(&l);
        s.push('\n');
    }
    s
}

/// What an item counts for on a page: the larger of its JSON and its text.
pub(super) fn size(shape: Shape, v: &Value) -> usize {
    v.to_json().len().max(text(shape, v).len())
}

/// `s` cut to [`FIELD_SHOWN`] bytes, on a character boundary.
fn cut_str(s: &str) -> &str {
    let mut n = FIELD_SHOWN.min(s.len());
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    &s[..n]
}

/// Step 3 on any value (see the module docs).
fn compact(v: Value) -> Value {
    match v {
        Value::Obj(f) => Value::Obj(compact_fields(f)),
        Value::List(l) => Value::List(l.into_iter().map(compact).collect()),
        Value::Str(s) if s.len() > FIELD_SHOWN => Value::Str(cut_str(&s).to_owned()),
        x => x,
    }
}

fn compact_fields(f: Vec<(String, Value)>) -> Vec<(String, Value)> {
    let names: Vec<String> = f.iter().map(|(k, _)| k.clone()).collect();
    let mut out = Vec::with_capacity(f.len());
    let mark = |out: &mut Vec<(String, Value)>, k: String, x: Value| {
        if !names.contains(&k) {
            out.push((k, x));
        }
    };
    for (k, x) in f {
        match x {
            Value::Str(s) if s.len() > FIELD_SHOWN => {
                let n = s.len();
                out.push((k.clone(), Value::Str(cut_str(&s).to_owned())));
                mark(&mut out, format!("{k}_bytes"), Value::from(n));
            }
            Value::List(l) if l.len() > LIST_SHOWN => {
                let n = l.len();
                let kept = l.into_iter().take(LIST_SHOWN).map(compact).collect();
                out.push((k.clone(), Value::List(kept)));
                mark(&mut out, format!("{k}_total"), Value::from(n));
            }
            // a map's entries are names, not fields: cut it whole, and say so beside it
            Value::Obj(m) if k == "attrs" => {
                let n = m.len();
                let mut was_cut = n > LIST_SHOWN;
                let kept: Vec<(String, Value)> = m
                    .into_iter()
                    .take(LIST_SHOWN)
                    .map(|(mk, mv)| match mv {
                        Value::Str(s) if s.len() > FIELD_SHOWN => {
                            was_cut = true;
                            (mk, Value::Str(cut_str(&s).to_owned()))
                        }
                        o => (mk, o),
                    })
                    .collect();
                out.push((k.clone(), Value::Obj(kept)));
                if was_cut {
                    mark(&mut out, format!("{k}_total"), Value::from(n));
                    mark(&mut out, format!("{k}_cut"), Value::from(true));
                }
            }
            other => out.push((k, compact(other))),
        }
    }
    out
}

/// The item's key (a node read's: its node's), for the reads a shortened item names.
fn key_of(shape: Shape, v: &Value) -> String {
    let at = match shape {
        Shape::Node => v.get("node").unwrap_or(&Value::Null),
        _ => v,
    };
    str_of(at, "key").to_owned()
}

/// `v`, shortened until it counts for at most `room` bytes (see the module docs). Returns it
/// and whether anything was left out.
pub(super) fn fit(shape: Shape, v: Value, room: usize, h: &Hints) -> (Value, bool) {
    if size(shape, &v) <= room {
        return (v, false);
    }
    #[cfg(any(test, feature = "differential"))]
    if crate::differential::index_mutant()
        == Some(crate::differential::IndexMutant::ItemsNotShortened)
    {
        return (v, false);
    }
    let full = v.to_json().len();
    let key = key_of(shape, &v);
    let fits = |x: &Value| size(shape, x) <= room;
    let mut v = v;
    let read = match shape {
        Shape::Node => {
            let missing = matches!(v.get("node"), Some(Value::Null) | None);
            if !missing {
                // 1. the text
                let mut node = v.get("node").cloned().unwrap_or(Value::Null);
                if let Some(Value::Str(t)) = node.get("text") {
                    let len = t.len();
                    set(&mut node, "text", Value::Null);
                    set(&mut node, "text_omitted", Value::from(true));
                    set(&mut node, "text_bytes", Value::from(len));
                    set(
                        &mut node,
                        "text_read",
                        Value::from(format!("{} {key} --version {} --text", h.show, h.version)),
                    );
                    set(&mut v, "node", node.clone());
                    if fits(&v) {
                        return (v, true);
                    }
                }
                // 2. the descendants, as many as fit
                if let Some(Value::List(d)) = v.get("descendants").cloned() {
                    if !d.is_empty() {
                        let total = d.len();
                        let depth = num_of(&node, "depth").unwrap_or(0) + u64::from(h.depth);
                        let with = |k: usize| {
                            let mut x = v.clone();
                            set(&mut x, "descendants", Value::List(d[..k].to_vec()));
                            set(&mut x, "descendants_total", Value::from(total));
                            set(
                                &mut x,
                                "descendants_read",
                                Value::from(format!(
                                    "{} --version {} --under {key} --depth {depth}",
                                    h.list, h.version
                                )),
                            );
                            x
                        };
                        // the largest prefix that fits (none may)
                        let (mut lo, mut hi) = (0, total);
                        while lo < hi {
                            let mid = (lo + hi).div_ceil(2);
                            if fits(&with(mid)) {
                                lo = mid;
                            } else {
                                hi = mid - 1;
                            }
                        }
                        v = with(lo);
                        if fits(&v) {
                            return (v, true);
                        }
                    }
                }
            }
            Value::from(h.whole(&key, h.version))
        }
        Shape::Outline { .. } => Value::from(h.whole(&key, h.version)),
        Shape::Change => Value::List(vec![
            Value::from(h.whole(&key, h.from)),
            Value::from(h.whole(&key, h.version)),
        ]),
        Shape::Blame => Value::from("log"),
    };
    // 3. every field cut, with the read that returns the item whole
    let mut c = compact(v.clone());
    if !(shape == Shape::Node && matches!(v.get("node"), Some(Value::Null) | None)) {
        set(&mut c, "read", read.clone());
    }
    if fits(&c) {
        return (c, true);
    }
    // 4. a stub
    let stub = match shape {
        Shape::Node => {
            let n = v.get("node").cloned().unwrap_or(Value::Null);
            let addr = str_of(&v, "address");
            let mut s = Value::obj([
                ("address", Value::from(cut_str(addr))),
                (
                    "node",
                    Value::obj([
                        ("position", Value::from(num_of(&n, "position").unwrap_or(0))),
                        ("key", Value::from(key.as_str())),
                        ("kind", Value::from(cut_str(str_of(&n, "kind")))),
                        ("node_omitted", Value::from(true)),
                        ("node_bytes", Value::from(full)),
                        ("node_read", read),
                    ]),
                ),
                ("ancestors", Value::List(Vec::new())),
                ("descendants", Value::List(Vec::new())),
            ]);
            for k in ["descendants_total", "descendants_read"] {
                if let Some(x) = v.get(k) {
                    set(&mut s, k, x.clone());
                }
            }
            s
        }
        _ => {
            let mut s = Value::obj([
                ("key", Value::from(key.as_str())),
                ("omitted", Value::from(true)),
                ("bytes", Value::from(full)),
                ("read", read),
            ]);
            if let Some(p) = v.get("position") {
                set(&mut s, "position", p.clone());
            }
            if let Some(ch) = v.get("changes") {
                set(&mut s, "changes", ch.clone());
            }
            s
        }
    };
    (stub, true)
}
