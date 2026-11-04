//! Every id that breaks the id rule, for a refusal to name. A host that offers to re-key a board
//! needs the whole list, not the first. The list is capped, since a hostile file can hold many.

use crate::json::{Fields, Value};
use crate::model::Id;
use crate::refusal::{Refusal, RefusalCode};
use crate::schema::{self, Kind, BAD_IDS_MAX, BAD_ID_MAX_CHARS};
use std::collections::HashSet;

#[derive(Default)]
struct Found<'v> {
    seen: HashSet<&'v str>,
    order: Vec<&'v str>,
}

impl<'v> Found<'v> {
    fn note(&mut self, id: &'v str) {
        if Id::new(id).is_err() && self.seen.insert(id) {
            self.order.push(id);
        }
    }

    fn value(&mut self, v: Option<&'v Value>) {
        if let Some(Value::Str(s)) = v {
            self.note(s);
        }
    }

    fn keys(&mut self, fields: &'v Fields) {
        for k in sorted(fields) {
            self.note(k);
        }
    }

    fn item_ids(&mut self, list: &'v Value) {
        for item in items(list) {
            self.value(item.get("id"));
        }
    }

    fn field(&mut self, kind: Kind, v: &'v Value) {
        match kind {
            Kind::ObjectRef | Kind::GroupId => self.value(Some(v)),
            Kind::End => {
                if let Value::Obj(end) = v {
                    self.value(end.get("id"));
                }
            }
            Kind::Checks | Kind::Columns => self.item_ids(v),
            Kind::Rows => {
                for row in items(v) {
                    if let Some(Value::Obj(cells)) = row.get("cells") {
                        self.keys(cells);
                    }
                    self.value(row.get("id"));
                }
            }
            Kind::Replies => {
                for reply in items(v) {
                    self.value(reply.get("by"));
                    self.value(reply.get("id"));
                }
            }
            Kind::Reactions => {
                if let Value::Obj(by_reaction) = v {
                    for key in sorted(by_reaction) {
                        if let Value::Arr(tokens) = &by_reaction[key] {
                            for token in tokens {
                                self.value(Some(token));
                            }
                        }
                    }
                }
            }
            Kind::Votes => {
                if let Value::Obj(votes) = v {
                    self.keys(votes);
                }
            }
            Kind::Created => {
                if let Value::Obj(created) = v {
                    self.value(created.get("by"));
                }
            }
            _ => {}
        }
    }

    fn object(&mut self, o: &'v Fields) {
        let type_name = match o.get("type") {
            Some(Value::Str(t)) => t.as_str(),
            _ => "",
        };
        for name in sorted(o) {
            if name == "id" {
                self.value(o.get("id"));
            } else if let Some(spec) = schema::field_of(type_name, name) {
                self.field(spec.kind, &o[name]);
            }
        }
    }
}

/// Members in canonical order.
fn sorted(fields: &Fields) -> Vec<&String> {
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    keys
}

fn items(list: &Value) -> impl Iterator<Item = &Fields> {
    let slice: &[Value] = match list {
        Value::Arr(a) => a,
        _ => &[],
    };
    slice.iter().filter_map(|v| match v {
        Value::Obj(f) => Some(f),
        _ => None,
    })
}

fn shown(id: &str) -> String {
    if id.chars().count() <= BAD_ID_MAX_CHARS {
        return id.to_string();
    }
    let mut cut: String = id.chars().take(BAD_ID_MAX_CHARS).collect();
    cut.push('\u{2026}');
    cut
}

/// Fills a refusal's list from the board's JSON: each text id once, in the canonical form's order.
pub(crate) fn name_them(mut r: Refusal, board: &Value) -> Refusal {
    if r.code != RefusalCode::IdInvalid {
        return r;
    }
    let Value::Obj(top) = board else {
        return r;
    };
    let mut found = Found::default();
    if let Some(Value::Arr(frames)) = top.get("frames") {
        for f in frames {
            found.value(Some(f));
        }
    }
    if let Some(objects) = top.get("objects") {
        for o in items(objects) {
            found.object(o);
        }
    }
    if let Some(Value::Obj(people)) = top.get("people") {
        found.keys(people);
    }
    r.bad_ids_total = found.order.len() as u64;
    r.bad_ids_truncated = found.order.len() > BAD_IDS_MAX;
    r.bad_ids = found
        .order
        .iter()
        .take(BAD_IDS_MAX)
        .map(|id| shown(id))
        .collect();
    r
}
