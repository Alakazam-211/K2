//! Catalog lint (prd-zen-user-widgets-v2 UWA1, UWA3, UWA6, TUWA1).
//!
//! [`lint`] returns every problem in a catalog as one plain sentence that
//! names the verb (and the field). The checked-in catalog must lint clean
//! (`contract::tests`); seeded bad rows prove each rule fails.
//!
//! **The JSON Schema subset.** Rows describe arguments and answers with a
//! small, closed subset of JSON Schema, so the generator can turn every
//! exposed schema into TypeScript and a guide line without guessing:
//! `type` (one name or a list), `enum`, `properties`, `required`,
//! `additionalProperties`, `items`, `title`, `description`, `default`,
//! `minimum`, `maximum`, `minLength`, `maxLength`. Any other keyword on an
//! exposed row is an error.
//!
//! - A `request` item's `title` is the argument's name (lower camel case);
//!   `default` marks it optional. A `subscribe` or `event` row's callback
//!   is implied and never listed.
//! - A response object's `title` names its TypeScript interface (upper
//!   camel case). One title is one shape across the whole catalog.
//! - **Closed (UWA6):** on an exposed row every object schema has
//!   `additionalProperties: false`, except a **map**: an object with no
//!   `properties` whose `additionalProperties` is a scalar schema (the
//!   theme's `--zen-*` values). A map's keys are data, never fields.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value as J;

use super::{Catalog, Effect, Exposure, HttpMethod, RendererImpl, VerbKind, VerbRow};

/// Keywords the generator understands (see the module docs).
pub const SCHEMA_KEYWORDS: &[&str] = &[
    "type",
    "enum",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "title",
    "description",
    "default",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
];

/// JSON Schema type names the subset allows.
pub const SCHEMA_TYPES: &[&str] = &["string", "number", "integer", "boolean", "null", "array", "object"];

/// Every problem in `c`, in file order. Empty = clean.
pub fn lint(c: &Catalog) -> Vec<String> {
    let mut out = Vec::new();
    lint_caps(c, &mut out);
    lint_errors(c, &mut out);
    let mut seen = BTreeSet::new();
    let mut titles: BTreeMap<String, (String, J)> = BTreeMap::new();
    for v in &c.verbs {
        if !seen.insert(v.verb.as_str()) {
            out.push(format!("{}: listed twice", v.verb));
        }
        lint_row(c, v, &mut out);
        if !v.exposure.is_empty() {
            for a in request_items(v) {
                // An argument's own title is its name, not a type.
                collect_titles(&v.verb, a, &mut titles, &mut out, true);
            }
            collect_titles(&v.verb, &v.response, &mut titles, &mut out, false);
        }
    }
    out
}

fn lint_caps(c: &Catalog, out: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for cap in &c.caps {
        if !seen.insert(cap.name.as_str()) {
            out.push(format!("cap {}: listed twice", cap.name));
        }
        if !valid_cap_name(&cap.name) {
            out.push(format!("cap {}: a cap is <surface>:<action>, lower case", cap.name));
        }
        if cap.label.trim().is_empty() {
            out.push(format!("cap {}: no label", cap.name));
        }
        if cap.exposure.contains(&Exposure::Widget) && cap.sentence.as_deref().is_none_or(|s| s.trim().is_empty()) {
            out.push(format!("cap {}: a widget cap needs K2's sentence for the review dialog", cap.name));
        }
    }
}

fn lint_errors(c: &Catalog, out: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    for e in &c.errors {
        if !seen.insert(e.code.as_str()) {
            out.push(format!("error {}: listed twice", e.code));
        }
        for f in &e.fields {
            if !["cap", "room", "feature"].contains(&f.as_str()) {
                out.push(format!("error {}: field '{f}' isn't cap, room or feature", e.code));
            }
        }
    }
}

fn valid_cap_name(s: &str) -> bool {
    let Some((a, b)) = s.split_once(':') else { return false };
    let ok = |p: &str| !p.is_empty() && p.chars().all(|ch| ch.is_ascii_lowercase());
    ok(a) && ok(b)
}

/// `surface.action`, both lower camel case.
pub fn valid_verb_name(s: &str) -> bool {
    let Some((a, b)) = s.split_once('.') else { return false };
    let ok = |p: &str| {
        p.chars().next().is_some_and(|ch| ch.is_ascii_lowercase()) && p.chars().all(|ch| ch.is_ascii_alphanumeric())
    };
    ok(a) && ok(b)
}

/// The argument schemas of a row (`request` is an array).
pub fn request_items(v: &VerbRow) -> Vec<&J> {
    v.request.as_array().map(|a| a.iter().collect()).unwrap_or_default()
}

fn lint_row(c: &Catalog, v: &VerbRow, out: &mut Vec<String>) {
    let name = &v.verb;
    if !valid_verb_name(name) {
        out.push(format!("{name}: a verb is surface.action, lower camel case"));
    }
    if let Some(cap) = &v.cap {
        match c.cap(cap) {
            None => out.push(format!("{name}: cap '{cap}' isn't in the cap table")),
            Some(row) => {
                for who in &v.exposure {
                    if !row.exposure.contains(who) {
                        out.push(format!("{name}: exposed to {who:?} but its cap '{cap}' isn't"));
                    }
                }
            }
        }
    }
    for e in &v.errors {
        if c.error(e).is_none() {
            out.push(format!("{name}: error '{e}' isn't in the shared error list"));
        }
    }
    if v.feature.trim().is_empty() {
        out.push(format!("{name}: no feature key"));
    }
    if v.bindings.is_empty() {
        out.push(format!("{name}: no binding (renderer, http or socket)"));
    }
    if v.doc.trim().is_empty() || v.doc.contains('\n') {
        out.push(format!("{name}: doc is one line"));
    }
    if v.example.trim().is_empty() || v.example.contains('\n') {
        out.push(format!("{name}: example is one line"));
    }
    // UWA3: no GET side effects; sockets are read-only.
    if let Some(h) = &v.bindings.http {
        if v.effect == Effect::Write && h.method != HttpMethod::Post {
            out.push(format!("{name}: a write verb's http binding must be POST (no GET side effects)"));
        }
        if !h.route.starts_with("/cli/") {
            out.push(format!("{name}: http route '{}' isn't a /cli/ route", h.route));
        }
    }
    if matches!(v.kind, VerbKind::Subscribe | VerbKind::Event) && v.effect == Effect::Write {
        out.push(format!("{name}: a {:?} row can't write", v.kind));
    }
    if v.bindings.socket.is_some() && v.kind != VerbKind::Subscribe {
        out.push(format!("{name}: only a subscribe row binds a socket"));
    }
    if let Some(s) = &v.bindings.socket {
        if let Some(r) = &s.refetch {
            if c.verb(r).is_none() {
                out.push(format!("{name}: socket refetch '{r}' isn't a verb"));
            }
        }
    }
    let frame = v.bindings.renderer.as_ref().map(|r| r.implementation) == Some(RendererImpl::Frame);
    if v.kind == VerbKind::Event && !frame {
        out.push(format!("{name}: an event row is pushed by the frame host (renderer impl \"frame\")"));
    }
    if frame && !v.exposed_to(Exposure::Widget) {
        out.push(format!("{name}: a frame row exists only for custom widgets; expose it to widget"));
    }
    let Some(items) = v.request.as_array() else {
        out.push(format!("{name}: request is an array of argument schemas"));
        return;
    };
    let mut arg_names = BTreeSet::new();
    let mut optional_seen = false;
    for (i, a) in items.iter().enumerate() {
        let title = a.get("title").and_then(J::as_str).unwrap_or("");
        if !title.chars().next().is_some_and(|ch| ch.is_ascii_lowercase()) || !title.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            out.push(format!("{name}: argument {i} needs a lower camel case title (its name)"));
        }
        if !arg_names.insert(title.to_string()) {
            out.push(format!("{name}: argument name '{title}' is used twice"));
        }
        let optional = a.get("default").is_some();
        if optional_seen && !optional {
            out.push(format!("{name}: argument '{title}' is required after an optional one"));
        }
        optional_seen |= optional;
    }
    if v.exposure.is_empty() {
        return;
    }
    // Exposed rows: the subset, closed shapes, no banned field.
    for (i, a) in items.iter().enumerate() {
        check_schema(c, name, &format!("argument {i}"), a, out);
    }
    check_schema(c, name, "response", &v.response, out);
    if v.response.as_object().is_some_and(|o| o.is_empty()) {
        out.push(format!("{name}: an exposed row says what it answers (response is empty)"));
    }
}

fn types_of(s: &J) -> Vec<&str> {
    match s.get("type") {
        Some(J::String(t)) => vec![t.as_str()],
        Some(J::Array(a)) => a.iter().filter_map(J::as_str).collect(),
        _ => Vec::new(),
    }
}

fn is_scalar(s: &J) -> bool {
    let t = types_of(s);
    (!t.is_empty() && t.iter().all(|t| !matches!(*t, "object" | "array"))) || s.get("enum").is_some()
}

fn check_schema(c: &Catalog, verb: &str, at: &str, s: &J, out: &mut Vec<String>) {
    let Some(o) = s.as_object() else {
        out.push(format!("{verb}: {at} is not a schema object"));
        return;
    };
    for k in o.keys() {
        if !SCHEMA_KEYWORDS.contains(&k.as_str()) {
            out.push(format!("{verb}: {at}: keyword '{k}' is outside the catalog's schema subset"));
        }
    }
    match o.get("type") {
        None if o.get("enum").is_none() => out.push(format!("{verb}: {at}: needs a type or an enum")),
        Some(J::String(t)) if !SCHEMA_TYPES.contains(&t.as_str()) => {
            out.push(format!("{verb}: {at}: unknown type '{t}'"))
        }
        Some(J::Array(ts)) => {
            for t in ts {
                if !t.as_str().is_some_and(|t| SCHEMA_TYPES.contains(&t)) {
                    out.push(format!("{verb}: {at}: unknown type {t}"));
                }
            }
        }
        Some(J::String(_)) | None => {}
        Some(other) => out.push(format!("{verb}: {at}: type {other} is neither a name nor a list")),
    }
    if let Some(e) = o.get("enum") {
        if !e.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|x| x.is_string() || x.is_null())) {
            out.push(format!("{verb}: {at}: enum is a list of strings (and null)"));
        }
    }
    let types = types_of(s);
    if types.contains(&"array") {
        match o.get("items") {
            Some(items) => check_schema(c, verb, &format!("{at}[]"), items, out),
            None => out.push(format!("{verb}: {at}: an array says its items")),
        }
    }
    if types.contains(&"object") {
        let props = o.get("properties").and_then(J::as_object);
        match (props, o.get("additionalProperties")) {
            (Some(_), Some(J::Bool(false))) => {}
            (None, Some(m)) if m.is_object() && is_scalar(m) => {
                check_schema(c, verb, &format!("{at}{{map}}"), m, out);
            }
            _ => out.push(format!(
                "{verb}: {at}: an exposed object is closed (additionalProperties: false), or a map of scalars with no properties"
            )),
        }
        if let Some(props) = props {
            let required: Vec<&str> =
                o.get("required").and_then(J::as_array).map(|a| a.iter().filter_map(J::as_str).collect()).unwrap_or_default();
            if o.get("required").is_none() {
                out.push(format!("{verb}: {at}: an object lists its required keys (may be [])"));
            }
            for r in &required {
                if !props.contains_key(*r) {
                    out.push(format!("{verb}: {at}: required '{r}' isn't a property"));
                }
            }
            for (k, p) in props {
                if c.banned_fields.iter().any(|b| b == k) {
                    out.push(format!("{verb}: {at}: field '{k}' is banned from exposed shapes (UWA6)"));
                }
                check_schema(c, verb, &format!("{at}.{k}"), p, out);
            }
        }
    } else if o.contains_key("properties") || o.contains_key("additionalProperties") {
        out.push(format!("{verb}: {at}: properties on a schema that isn't an object"));
    }
}

/// Response objects with a `title` become named TypeScript interfaces; one
/// title must be one shape everywhere (ignoring nullability and
/// `description`).
fn collect_titles(verb: &str, s: &J, titles: &mut BTreeMap<String, (String, J)>, out: &mut Vec<String>, is_arg: bool) {
    let Some(o) = s.as_object() else { return };
    if !is_arg {
        if let (Some(t), true) = (o.get("title").and_then(J::as_str), types_of(s).contains(&"object")) {
            if !t.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
                out.push(format!("{verb}: type title '{t}' starts with a capital letter"));
            }
            let shape = shape_key(s);
            match titles.get(t) {
                Some((first, other)) if *other != shape => {
                    out.push(format!("{verb}: type '{t}' differs from the one {first} answers"))
                }
                Some(_) => {}
                None => {
                    titles.insert(t.to_string(), (verb.to_string(), shape));
                }
            }
        }
    }
    if let Some(items) = o.get("items") {
        collect_titles(verb, items, titles, out, false);
    }
    if let Some(props) = o.get("properties").and_then(J::as_object) {
        for p in props.values() {
            collect_titles(verb, p, titles, out, false);
        }
    }
}

/// A schema with `type` reduced to its non-null part and every
/// `description` dropped, for title equality.
pub fn shape_key(s: &J) -> J {
    match s {
        J::Object(o) => {
            let mut m = serde_json::Map::new();
            for (k, v) in o {
                if k == "description" {
                    continue;
                }
                if k == "type" {
                    let ts: Vec<J> = match v {
                        J::Array(a) => a.iter().filter(|t| t.as_str() != Some("null")).cloned().collect(),
                        other => vec![other.clone()],
                    };
                    m.insert(k.clone(), if ts.len() == 1 { ts[0].clone() } else { J::Array(ts) });
                    continue;
                }
                m.insert(k.clone(), shape_key(v));
            }
            J::Object(m)
        }
        J::Array(a) => J::Array(a.iter().map(shape_key).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{catalog, parse};

    fn with_rows(rows: &str) -> Catalog {
        let base = catalog();
        let mut c = base.clone();
        let extra: Vec<VerbRow> = serde_json::from_str(rows).expect("seeded rows parse");
        c.verbs.extend(extra);
        c
    }

    fn assert_one(c: &Catalog, want: &str) {
        let probs = lint(c);
        assert_eq!(probs.len(), 1, "want exactly one problem containing {want:?}, got {probs:#?}");
        assert!(probs[0].contains(want), "want {want:?} in {:?}", probs[0]);
    }

    const GOOD: &str = r#"{"verb":"seed.good","cap":"thread:read","exposure":["widget"],"reach":"local","effect":"read",
        "kind":"call","feature":"zen-v1","bindings":{"renderer":{"impl":"registered"}},
        "request":[{"title":"address","type":"string"}],
        "response":{"title":"SeedGood","type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false},
        "errors":["failed"],"doc":"d","example":"e"}"#;

    #[test]
    fn the_checked_in_catalog_lints_clean() {
        let probs = lint(catalog());
        assert!(probs.is_empty(), "catalog.json problems:\n{}", probs.join("\n"));
    }

    #[test]
    fn a_good_seed_row_is_clean() {
        let c = with_rows(&format!("[{GOOD}]"));
        assert!(lint(&c).is_empty(), "{:#?}", lint(&c));
    }

    #[test]
    fn an_open_response_schema_fails() {
        let bad = GOOD.replace(r#""additionalProperties":false"#, r#""additionalProperties":true"#);
        assert_one(&with_rows(&format!("[{bad}]")), "seed.good: response: an exposed object is closed");
    }

    #[test]
    fn a_banned_field_fails_naming_verb_and_field() {
        let bad = GOOD.replace(r#""properties":{"a""#, r#""properties":{"sessionId""#).replace(r#""required":["a"]"#, r#""required":[]"#);
        assert_one(&with_rows(&format!("[{bad}]")), "seed.good: response: field 'sessionId' is banned");
    }

    #[test]
    fn a_banned_field_deep_in_an_array_fails() {
        let bad = GOOD.replace(
            r#""response":{"title":"SeedGood","type":"object","properties":{"a":{"type":"string"}}"#,
            r#""response":{"title":"SeedGood","type":"object","properties":{"a":{"type":"array","items":{"type":"object","properties":{"cwd":{"type":"string"}},"required":[],"additionalProperties":false}}}"#,
        );
        assert_one(&with_rows(&format!("[{bad}]")), "field 'cwd' is banned");
    }

    #[test]
    fn a_write_verb_bound_to_get_fails() {
        let bad = GOOD
            .replace(r#""effect":"read""#, r#""effect":"write""#)
            .replace(r#""bindings":{"#, r#""bindings":{"http":{"method":"GET","route":"/cli/thread/post"},"#);
        assert_one(&with_rows(&format!("[{bad}]")), "must be POST");
    }

    #[test]
    fn unknown_cap_error_and_keyword_fail() {
        let bad = GOOD.replace(r#""cap":"thread:read""#, r#""cap":"nope:read""#);
        assert_one(&with_rows(&format!("[{bad}]")), "cap 'nope:read' isn't in the cap table");
        let bad = GOOD.replace(r#""errors":["failed"]"#, r#""errors":["not_open"]"#);
        assert_one(&with_rows(&format!("[{bad}]")), "error 'not_open'");
        let bad = GOOD.replace(r#"{"title":"address","type":"string"}"#, r#"{"title":"address","type":"string","format":"uri"}"#);
        assert_one(&with_rows(&format!("[{bad}]")), "keyword 'format'");
    }

    #[test]
    fn a_widget_row_with_an_unexposed_cap_fails() {
        let bad = GOOD.replace(r#""cap":"thread:read""#, r#""cap":"gardens:manage""#);
        assert_one(&with_rows(&format!("[{bad}]")), "its cap 'gardens:manage' isn't");
    }

    #[test]
    fn a_subscribe_row_that_writes_fails() {
        let bad = GOOD.replace(r#""effect":"read""#, r#""effect":"write""#).replace(r#""kind":"call""#, r#""kind":"subscribe""#);
        assert_ne!(bad, GOOD);
        assert_one(&with_rows(&format!("[{bad}]")), "can't write");
    }

    #[test]
    fn one_title_is_one_shape() {
        let other = GOOD.replace("seed.good", "seed.other").replace(r#"{"a":{"type":"string"}}"#, r#"{"a":{"type":"integer"}}"#);
        assert_one(&with_rows(&format!("[{GOOD},{other}]")), "type 'SeedGood' differs");
    }

    #[test]
    fn an_untitled_argument_fails() {
        let bad = GOOD.replace(r#"{"title":"address","type":"string"}"#, r#"{"type":"string"}"#);
        assert_one(&with_rows(&format!("[{bad}]")), "argument 0 needs a lower camel case title");
    }

    #[test]
    fn duplicate_rows_fail() {
        let probs = lint(&with_rows(&format!("[{GOOD},{GOOD}]")));
        assert!(probs.iter().any(|p| p.contains("seed.good: listed twice")), "{probs:#?}");
    }

    #[test]
    fn a_map_of_scalars_is_closed_enough() {
        let c = parse(crate::contract::CATALOG_JSON).expect("parses");
        let theme = c.verb("theme.get").expect("theme.get row");
        let vars = &theme.response["properties"]["theme"]["properties"]["vars"];
        assert!(vars.get("properties").is_none() && vars["additionalProperties"]["type"] == "string", "{vars}");
        let bad = GOOD.replace(
            r#""response":{"title":"SeedGood","type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false}"#,
            r#""response":{"type":"object","additionalProperties":{"type":"object"}}"#,
        );
        let probs = lint(&with_rows(&format!("[{bad}]")));
        assert!(probs.iter().any(|p| p.contains("map of scalars")), "{probs:#?}");
    }
}
