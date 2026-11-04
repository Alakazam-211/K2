//! The friendly verbs on a board file, hand-wired for 0.46.8: `{verb, args}`
//! to the pack's [`Verb`], with exactly the argument names of
//! [`k2_mpc_board::verbs::ARGS`]. **Thin on purpose:** in 0.46.9 the kit bridge
//! (k2-mpc-board v0.1.2) brings its own parser, built on that same table, and
//! this module goes. `wired_args_match_the_pack` fails if a name drifts, and if
//! the pack gains a verb this module neither wires nor lists as later.
//!
//! The wire, as the CLI sends it: args by name, every value as the command line
//! gave it (text), except where a list is natural (`ids`, `revs`, `unset`) or a
//! pair (`point`). Field values go through the pack's own `verbs::flags` /
//! `field_from_text`, so a number takes the format's number rule and nothing is
//! rounded.

use k2_mpc_board::verbs::{self, End, Layer, ObjRev, Verb, VerbCode, VerbError};
use k2_mpc_board::{json as fj, Board, Id, Limits, ObjectType, Position, Sha256};
use serde_json::{Map, Value};

/// The verbs wired here, with the argument names each takes from the pack's
/// table (a name ending in `?` may be left out).
pub const WIRED: &[(&str, &[&str])] = &[
    ("add", &["kind", "fields", "at"]),
    ("update", &["id", "rev", "set", "unset"]),
    ("delete", &["id", "rev", "cascade"]),
    ("connect", &["from", "to", "fields"]),
    ("react", &["id", "emoji", "on?"]),
    ("vote", &["id", "delta"]),
    ("comment", &["target?", "text", "point?"]),
    ("meta", &["rev", "set", "unset"]),
    ("move", &["ids", "dx", "dy"]),
    ("order", &["ids", "to"]),
];

/// The pack's verbs that wait for the bridge's parser (0.46.9), and the ones K2
/// answers on other routes (`list`, `read`, `search`, `fields`: `read`; `apply`:
/// `save`). Every verb in `ARGS` is in `WIRED` or here.
pub const LATER: &[&str] = &[
    "put-back", "reply", "resolve", "lock", "check-add", "check-set", "check-remove", "table-cell", "table-add-row",
    "table-remove-row", "table-add-column", "table-remove-column", "frame-order", "group", "ungroup", "align",
    "distribute", "tidy", "copy-style", "duplicate", "paste", "connected-copy", "mind-child", "template", "clear-blob",
    "list", "read", "search", "fields", "templates", "references", "apply", "inverse",
];

/// Why the args don't make a verb. Typed, with the code the CLI maps to an exit.
#[derive(Debug, Clone, PartialEq)]
pub struct ArgError {
    pub code: &'static str,
    pub message: String,
}

fn bad(code: &'static str, m: impl Into<String>) -> ArgError {
    ArgError { code, message: m.into() }
}

fn usage(m: impl Into<String>) -> ArgError {
    bad("bad_request", m)
}

fn text<'a>(a: &'a Map<String, Value>, k: &str) -> Result<&'a str, ArgError> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| usage(format!("{k} is required")))
}

fn opt_text<'a>(a: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn id_of(s: &str) -> Result<Id, ArgError> {
    Id::new(s).map_err(|_| bad("id_invalid", format!("{s:?} isn't an id: 1 to 24 of a-z, 0-9, . _ -")))
}

fn rev_of(s: &str) -> Result<Sha256, ArgError> {
    Sha256::parse(s).map_err(|_| usage(format!("{s:?} isn't a rev: read the object first, its rev is sha256-…")))
}

fn num(a: &Map<String, Value>, k: &str) -> Result<f64, ArgError> {
    let t = text(a, k)?;
    let v = fj::number_value(t).map_err(|_| bad("bad_value", format!("{k} must be a number")))?;
    Ok(v)
}

fn list<'a>(a: &'a Map<String, Value>, k: &str) -> Result<Vec<&'a str>, ArgError> {
    match a.get(k) {
        Some(Value::Array(l)) => l.iter().map(|x| x.as_str().ok_or_else(|| usage(format!("{k} holds text")))).collect(),
        Some(Value::String(s)) => Ok(s.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()),
        None => Ok(Vec::new()),
        Some(_) => Err(usage(format!("{k} is a list"))),
    }
}

fn pairs(a: &Map<String, Value>, k: &str) -> Result<Vec<(String, String)>, ArgError> {
    match a.get(k) {
        None => Ok(Vec::new()),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(n, v)| match v {
                Value::String(s) => Ok((n.clone(), s.clone())),
                other => Ok((n.clone(), other.to_string())),
            })
            .collect(),
        Some(_) => Err(usage(format!("{k} is {{field: value}}"))),
    }
}

fn from_verb(e: VerbError) -> ArgError {
    let (code, message) = refusal_of(&e);
    ArgError { code, message }
}

/// A verb's refusal as a wire code. Every `VerbError` the pack has, and a loud
/// one for any it adds (`#[non_exhaustive]` from v0.1.2).
#[allow(unreachable_patterns)]
pub fn refusal_of(e: &VerbError) -> (&'static str, String) {
    match e {
        VerbError::Conflict(k2_mpc_board::Conflict::Refused(r)) => (r.code.as_str(), r.detail.clone()),
        VerbError::Conflict(_) => ("board_conflict", "it changed since you read it: read it again, then use the new rev".to_string()),
        VerbError::Refused { code, detail, .. } => (code_str(*code), detail.clone()),
        other => ("verb_error_unknown", format!("this K2 doesn't know that answer from the board pack: {other:?}")),
    }
}

#[allow(unreachable_patterns)]
fn code_str(c: VerbCode) -> &'static str {
    match c {
        VerbCode::NeedsActor => "needs_actor",
        VerbCode::VotingOff => "voting_off",
        VerbCode::NoVotesLeft => "no_votes_left",
        VerbCode::WrongType => "wrong_type",
        VerbCode::TooFew => "too_few",
        VerbCode::NoTemplate => "no_template",
        VerbCode::UnknownField => "unknown_field",
        VerbCode::UnknownType => "unknown_type",
        VerbCode::NoSuchItem => "no_such_item",
        VerbCode::NoFreeId => "no_free_id",
        VerbCode::BadValue => "bad_value",
        _ => "verb_code_unknown",
    }
}

fn type_of(board: &Board, id: &Id) -> Result<String, ArgError> {
    board
        .objects
        .iter()
        .find(|o| o.id == *id)
        .map(|o| o.kind.as_str().to_string())
        .ok_or_else(|| bad("no_object", format!("no object {id} in this file")))
}

fn edits(type_name: &str, a: &Map<String, Value>, set_key: &str) -> Result<(k2_mpc_board::Fields, Vec<String>), ArgError> {
    let ps = pairs(a, set_key)?;
    let refs: Vec<(&str, &str)> = ps.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let mut e = verbs::flags(type_name, &refs, &Limits::V1).map_err(from_verb)?;
    for u in list(a, "unset")? {
        e.unset.push(u.to_string());
    }
    Ok((e.set, e.unset))
}

fn end_of(v: Option<&Value>, k: &str) -> Result<End, ArgError> {
    match v {
        Some(Value::String(s)) => Ok(End::Object { id: id_of(s)?, anchor: None }),
        Some(Value::Object(m)) => {
            if let Some(id) = m.get("id").and_then(Value::as_str) {
                return Ok(End::Object { id: id_of(id)?, anchor: m.get("anchor").and_then(Value::as_str).map(str::to_string) });
            }
            let x = m.get("x").and_then(|x| x.as_str().map(str::to_string).or_else(|| Some(x.to_string()))).unwrap_or_default();
            let y = m.get("y").and_then(|x| x.as_str().map(str::to_string).or_else(|| Some(x.to_string()))).unwrap_or_default();
            let p = |t: &str| fj::number_value(t).map_err(|_| bad("bad_value", format!("{k} is an object id or x,y")));
            Ok(End::Point { x: p(&x)?, y: p(&y)? })
        }
        _ => Err(usage(format!("{k} is an object id"))),
    }
}

fn obj_revs(a: &Map<String, Value>) -> Result<Vec<ObjRev>, ArgError> {
    let ids = list(a, "ids")?;
    let revs = list(a, "revs")?;
    if ids.is_empty() {
        return Err(usage("ids is required: one or more object ids"));
    }
    if ids.len() != revs.len() {
        return Err(usage("one rev for each id, in the same order (--rev a,b)"));
    }
    ids.iter().zip(revs).map(|(i, r)| Ok(ObjRev { id: id_of(i)?, rev: rev_of(r)? })).collect()
}

/// `{verb, args}` as the pack's [`Verb`]. `board` is the file as it is under the
/// writer lock, for the types `update` and `connect` read fields by.
pub fn verb_of(name: &str, args: &Value, board: &Board) -> Result<Verb, ArgError> {
    let empty = Map::new();
    let a = args.as_object().unwrap_or(&empty);
    Ok(match name {
        "add" => {
            let kind = text(a, "kind")?;
            let (fields, unset) = edits(kind, a, "fields")?;
            if !unset.is_empty() {
                return Err(usage("add sets fields; null removes nothing on a new object"));
            }
            let at = match opt_text(a, "at") {
                None | Some("front") => Position::Front,
                Some("back") => Position::Back,
                Some(other) => return Err(usage(format!("at is front or back, not {other:?}"))),
            };
            Verb::Add { kind: ObjectType::parse(kind).map_err(|_| bad("unknown_type", format!("no type {kind}: k2 board fields")))?, fields, at }
        }
        "update" => {
            let id = id_of(text(a, "id")?)?;
            let (set, unset) = edits(&type_of(board, &id)?, a, "set")?;
            if set.is_empty() && unset.is_empty() {
                return Err(usage("update needs at least one --<field> <value> (null removes it)"));
            }
            Verb::Update { id, rev: rev_of(text(a, "rev")?)?, set, unset }
        }
        "delete" => Verb::Delete {
            id: id_of(text(a, "id")?)?,
            rev: rev_of(text(a, "rev")?)?,
            cascade: !matches!(opt_text(a, "cascade"), Some("false")),
        },
        "connect" => {
            let (fields, _) = edits("connector", a, "fields")?;
            Verb::Connect { from: end_of(a.get("from"), "from")?, to: end_of(a.get("to"), "to")?, fields }
        }
        "react" => Verb::React {
            id: id_of(text(a, "id")?)?,
            emoji: text(a, "emoji")?.to_string(),
            on: match opt_text(a, "on") {
                None => None,
                Some("true") => Some(true),
                Some("false") => Some(false),
                Some(o) => return Err(usage(format!("on is true or false, not {o:?}"))),
            },
        },
        "vote" => {
            let d = opt_text(a, "delta").unwrap_or("1");
            Verb::Vote { id: id_of(text(a, "id")?)?, delta: d.parse().map_err(|_| bad("bad_value", "delta is a whole number, like 1 or -1"))? }
        }
        "comment" => {
            let point = match opt_text(a, "point") {
                None => None,
                Some(p) => {
                    let xy: Vec<&str> = p.split(',').map(str::trim).collect();
                    let n = |t: &str| fj::number_value(t).map_err(|_| bad("bad_value", "point is x,y"));
                    match xy.as_slice() {
                        [x, y] => Some((n(x)?, n(y)?)),
                        _ => return Err(usage("point is x,y")),
                    }
                }
            };
            let target = opt_text(a, "target").map(id_of).transpose()?;
            if target.is_none() && point.is_none() {
                return Err(usage("a comment is on an object (its id) or at a point (--point x,y)"));
            }
            Verb::Comment { target, text: text(a, "text")?.to_string(), point }
        }
        "meta" => {
            let ps = pairs(a, "set")?;
            let mut set = k2_mpc_board::Fields::new();
            for (n, v) in &ps {
                if n == "timer" {
                    return Err(usage("timer: a board file doesn't run a timer"));
                }
                if v != "null" {
                    set.insert(n.clone(), verbs::field_from_text("", n, v, &Limits::V1).map_err(|r| bad(r.code.as_str(), r.detail))?);
                }
            }
            let mut unset: Vec<String> = ps.iter().filter(|(_, v)| v == "null").map(|(n, _)| n.clone()).collect();
            unset.extend(list(a, "unset")?.into_iter().map(str::to_string));
            Verb::Meta { rev: rev_of(text(a, "rev")?)?, set, unset }
        }
        "move" => Verb::Move { ids: obj_revs(a)?, dx: num(a, "dx")?, dy: num(a, "dy")? },
        "order" => {
            let ids = list(a, "ids")?.into_iter().map(id_of).collect::<Result<Vec<_>, _>>()?;
            if ids.is_empty() {
                return Err(usage("ids is required: one or more object ids"));
            }
            let to = match text(a, "to")? {
                "front" => Layer::Front,
                "back" => Layer::Back,
                "forward" => Layer::Forward,
                "backward" => Layer::Backward,
                t => match t.strip_prefix("after:") {
                    Some(rest) => Layer::After(rest.split(',').map(str::trim).map(id_of).collect::<Result<Vec<_>, _>>()?),
                    None => return Err(usage("to is front, back, forward, backward or after:<id>")),
                },
            };
            Verb::Order { ids, to }
        }
        other if LATER.contains(&other) => {
            return Err(usage(format!("{other} isn't wired for files in this K2 yet: use apply with raw ops")));
        }
        other => return Err(usage(format!("no verb {other}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The drift check: every wired verb takes exactly the pack's argument names,
    /// and every verb the pack has is wired or listed as later. A verb the pack
    /// adds fails here until it's one or the other.
    #[test]
    fn wired_args_match_the_pack() {
        for (name, names) in WIRED {
            let theirs = verbs::ARGS.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("the pack has no verb {name}"));
            assert_eq!(*names, theirs.1, "{name}'s arguments");
        }
        for (name, _) in verbs::ARGS {
            let wired = WIRED.iter().any(|(n, _)| n == name);
            let later = LATER.contains(name);
            assert!(wired ^ later, "{name}: wire it, or list it as later (not both)");
        }
        for n in LATER {
            assert!(verbs::ARGS.iter().any(|(a, _)| a == n), "{n} is listed as later, and the pack has no such verb");
        }
        assert_eq!(WIRED.len() + LATER.len(), verbs::ARGS.len());
    }

    /// Every argument name a wired verb uses on the wire is one of the pack's,
    /// and `revs` (the CLI's list of revs for `ids`) is the one addition.
    #[test]
    fn the_wire_uses_only_the_packs_names() {
        let board = Board::from_json(br#"{"objects":[{"id":"s-1","type":"sticky"}],"frames":[],"people":{},"settings":{}}"#, &Limits::V1).expect("board");
        let rev = board.objects[0].rev().expect("rev").to_string();
        let calls = [
            ("add", json!({ "kind": "sticky", "fields": { "text": "hi", "x": "1.25" }, "at": "front" })),
            ("update", json!({ "id": "s-1", "rev": rev, "set": { "text": "new" }, "unset": [] })),
            ("delete", json!({ "id": "s-1", "rev": rev, "cascade": "true" })),
            ("connect", json!({ "from": "s-1", "to": { "x": "1", "y": "2" }, "fields": { "label": "l" } })),
            ("react", json!({ "id": "s-1", "emoji": "👍", "on": "true" })),
            ("vote", json!({ "id": "s-1", "delta": "1" })),
            ("comment", json!({ "target": "s-1", "text": "why", "point": "1,2" })),
            ("meta", json!({ "rev": rev, "set": { "title": "T" }, "unset": [] })),
            ("move", json!({ "ids": ["s-1"], "revs": [rev], "dx": "10", "dy": "-0" })),
            ("order", json!({ "ids": ["s-1"], "to": "back" })),
        ];
        assert_eq!(calls.len(), WIRED.len(), "a call for every wired verb");
        for (name, args) in calls {
            let names = WIRED.iter().find(|(n, _)| *n == name).expect("wired").1;
            for k in args.as_object().expect("o").keys() {
                assert!(names.iter().any(|n| n.trim_end_matches('?') == k) || (k == "revs" && names.contains(&"ids")), "{name}: {k} isn't the pack's name");
            }
            verb_of(name, &args, &board).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    #[test]
    fn a_number_is_never_rounded_and_null_removes() {
        let board = Board::from_json(br#"{"objects":[{"id":"s-1","type":"sticky","link":"https://x.test"}],"frames":[],"people":{},"settings":{}}"#, &Limits::V1).expect("board");
        let rev = board.objects[0].rev().expect("rev").to_string();
        match verb_of("update", &json!({ "id": "s-1", "rev": rev, "set": { "x": "1.25", "link": "null" } }), &board).expect("verb") {
            Verb::Update { set, unset, .. } => {
                assert_eq!(set.get("x"), Some(&fj::Value::Num(1.25)));
                assert_eq!(unset, ["link"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bad_args_refuse_typed() {
        let board = Board::from_json(br#"{"objects":[{"id":"s-1","type":"sticky"}],"frames":[],"people":{},"settings":{}}"#, &Limits::V1).expect("board");
        let cases = [
            ("update", json!({ "id": "s-1", "rev": "nope", "set": { "text": "x" } }), "bad_request"),
            ("update", json!({ "id": "s-9", "rev": "nope", "set": { "text": "x" } }), "no_object"),
            ("update", json!({ "id": "s-1", "rev": format!("sha256-{}", "0".repeat(64)), "set": { "colour": "x" } }), "unknown_field"),
            ("add", json!({ "kind": "hologram" }), "unknown_type"),
            ("move", json!({ "ids": ["s-1"], "revs": [], "dx": "1", "dy": "1" }), "bad_request"),
            ("meta", json!({ "rev": format!("sha256-{}", "0".repeat(64)), "set": { "timer": "{}" } }), "bad_request"),
            ("order", json!({ "ids": ["s-1"], "to": "sideways" }), "bad_request"),
            ("tidy", json!({}), "bad_request"),
            ("frobnicate", json!({}), "bad_request"),
        ];
        for (name, args, code) in cases {
            let e = verb_of(name, &args, &board).expect_err(name);
            assert_eq!(e.code, code, "{name}: {e:?}");
        }
    }

    #[test]
    fn every_verb_refusal_code_has_a_wire_name() {
        for c in VerbCode::ALL {
            assert_eq!(code_str(*c), c.as_str(), "K2's names follow the pack's");
        }
    }
}
