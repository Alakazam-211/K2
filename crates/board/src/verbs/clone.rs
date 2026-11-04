//! Copies of objects with fresh ids. The rules derive from Board, Copyright (c) 2026 Baden Ballard,
//! MIT (see `THIRD-PARTY-NOTICES.md`).

use super::arrange::{bounds, box_of, drawn_box, js_round, num_field};
use super::edit::{end_id, is_locked};
use super::{one, refuse_with, s, Facts, Planner, Side, VerbCode, VerbError};
use crate::json::{Fields, Value};
use crate::model::{Id, Object, ObjectType};
use crate::ops::{Op, Position};
use crate::schema;
use std::collections::{BTreeMap, BTreeSet};

const QUICK: [&ObjectType; 3] = [&ObjectType::Sticky, &ObjectType::Shape, &ObjectType::Card];

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Top => "top",
        Side::Right => "right",
        Side::Bottom => "bottom",
        Side::Left => "left",
    }
}

impl Planner<'_, '_> {
    /// Clones a set. `lookup` finds an end's object when it isn't in the set.
    fn clone_set(&mut self, objects: &[Object], dx: f64, dy: f64) -> Result<(), VerbError> {
        let mut map: BTreeMap<&Id, Id> = BTreeMap::new();
        for o in objects {
            let id = self.mint(super::id_prefix(&o.kind))?;
            map.insert(&o.id, id);
        }
        let mut groups: BTreeMap<String, String> = BTreeMap::new();
        for o in objects {
            let mut fields = o.fields.clone();
            for gone in ["votes", "reactions", "created"] {
                fields.remove(gone);
            }
            if o.kind != ObjectType::Connector {
                fields.insert("x".into(), Value::Num(js_round(num_field(o, "x") + dx)));
                fields.insert("y".into(), Value::Num(js_round(num_field(o, "y") + dy)));
            }
            if let Some(Value::Str(g)) = fields.get("group").cloned() {
                let new = match groups.get(&g) {
                    Some(n) => n.clone(),
                    None => {
                        let n = self.mint_sub("g")?;
                        groups.insert(g, n.clone());
                        n
                    }
                };
                fields.insert("group".into(), Value::Str(new));
            }
            if o.kind == ObjectType::Connector {
                for name in ["from", "to"] {
                    if let Some(end) = o.fields.get(name) {
                        fields.insert(name.to_string(), self.cloned_end(end, &map, dx, dy));
                    }
                }
            }
            for name in ["parent", "target"] {
                let relevant = (name == "parent" && o.kind == ObjectType::Mind)
                    || (name == "target" && o.kind == ObjectType::Comment);
                if !relevant {
                    continue;
                }
                let inside = match o.fields.get(name) {
                    Some(Value::Str(t)) => {
                        Id::new(t).ok().and_then(|t| map.get(&t).map(Id::to_string))
                    }
                    _ => None,
                };
                match inside {
                    Some(new) => {
                        fields.insert(name.to_string(), Value::Str(new));
                    }
                    None => {
                        fields.remove(name);
                    }
                }
            }
            if let Some(Value::Str(f)) = o.fields.get("frame") {
                if let Some(new) = Id::new(f).ok().and_then(|f| map.get(&f).map(Id::to_string)) {
                    fields.insert("frame".into(), Value::Str(new));
                }
            }
            self.stamp(&mut fields);
            let id = map[&o.id].clone();
            self.ops.push(Op::Add {
                object: Object {
                    id: id.clone(),
                    kind: o.kind.clone(),
                    fields,
                },
                at: Position::Front,
            });
            self.ids.push(id);
        }
        Ok(())
    }

    /// An end in the set follows the copy. One outside it becomes a free point where its object is.
    fn cloned_end(&self, end: &Value, map: &BTreeMap<&Id, Id>, dx: f64, dy: f64) -> Value {
        let Value::Obj(f) = end else {
            return end.clone();
        };
        if let Some(target) = end_id(Some(end)) {
            let target = Id::new(target).ok();
            if let Some(new) = target.as_ref().and_then(|t| map.get(t)) {
                let mut out = f.clone();
                out.insert("id".into(), Value::Str(new.to_string()));
                out.remove("hint");
                return Value::Obj(out);
            }
            let there = target
                .as_ref()
                .and_then(|t| self.index.get(t.as_str()))
                .and_then(|o| box_of(o));
            let (x, y) = match there {
                Some(b) => (
                    js_round(b.x + b.w / 2.0 + dx),
                    js_round(b.y + b.h / 2.0 + dy),
                ),
                None => (0.0, 0.0),
            };
            let mut out = one("x", Value::Num(x));
            out.insert("y".into(), Value::Num(y));
            return Value::Obj(out);
        }
        let at = |name: &str| match f.get(name) {
            Some(Value::Num(n)) => *n,
            _ => 0.0,
        };
        let mut out = f.clone();
        out.insert("x".into(), Value::Num(at("x") + dx));
        out.insert("y".into(), Value::Num(at("y") + dy));
        Value::Obj(out)
    }

    pub(crate) fn duplicate(&mut self, ids: &[Id], dx: f64, dy: f64) -> Result<(), VerbError> {
        let named: BTreeSet<&Id> = ids.iter().collect();
        for id in ids {
            self.obj(id)?;
        }
        // A connector with both ends in the set comes along.
        let both_inside = |o: &Object| {
            o.kind == ObjectType::Connector
                && ["from", "to"].iter().all(|name| {
                    end_id(o.fields.get(*name))
                        .and_then(|t| Id::new(t).ok())
                        .is_some_and(|t| named.contains(&t))
                })
        };
        let set: Vec<Object> = self
            .board
            .objects
            .iter()
            .filter(|o| named.contains(&o.id) || both_inside(o))
            .cloned()
            .collect();
        self.clone_set(&set, dx, dy)
    }

    pub(crate) fn paste(&mut self, objects: &[Object], x: f64, y: f64) -> Result<(), VerbError> {
        let held = objects
            .iter()
            .filter(|o| o.kind != ObjectType::Connector)
            .filter_map(box_of);
        let b = bounds(held)
            .ok_or_else(|| refuse_with(VerbCode::TooFew, "nothing to paste", Facts::min(1)))?;
        let dx = js_round(x - b.x - b.w / 2.0);
        let dy = js_round(y - b.y - b.h / 2.0);
        self.clone_set(objects, dx, dy)
    }

    pub(crate) fn connected_copy(&mut self, id: &Id, side: Side) -> Result<(), VerbError> {
        const GAP: f64 = 80.0;
        let o = self.obj(id)?;
        if !QUICK.contains(&&o.kind) {
            return Err(refuse_with(
                VerbCode::WrongType,
                format!(
                    "{id} is a {}: a sticky, shape or card only",
                    o.kind.as_str()
                ),
                Facts::object(id, o.kind.as_str()),
            ));
        }
        if is_locked(o) {
            return Err(crate::refusal::Refusal::at(
                crate::refusal::RefusalCode::ObjectLocked,
                format!("/objects/{id}"),
                "this object is locked: unlock it first",
            )
            .into());
        }
        let (dir_x, dir_y, opposite) = match side {
            Side::Top => (0.0, -1.0, Side::Bottom),
            Side::Right => (1.0, 0.0, Side::Left),
            Side::Bottom => (0.0, 1.0, Side::Top),
            Side::Left => (-1.0, 0.0, Side::Right),
        };
        let type_name = o.kind.as_str().to_string();
        let mut fields = Fields::new();
        for (name, v) in &o.fields {
            let known = schema::field_of(&type_name, name).is_some();
            let dropped = [
                "votes",
                "reactions",
                "group",
                "tags",
                "rot",
                "checks",
                "link",
                "crop",
                "created",
                "locked",
            ]
            .contains(&name.as_str());
            if known && !dropped {
                fields.insert(name.clone(), v.clone());
            }
        }
        let emptied: &[&str] = if o.kind == ObjectType::Card {
            &["title", "text", "status", "assignee", "due"]
        } else {
            &["text"]
        };
        for name in emptied {
            if fields.contains_key(*name) {
                fields.insert((*name).to_string(), s(""));
            }
        }
        let (w, h) = (num_field(o, "w"), num_field(o, "h"));
        let (ox, oy) = (num_field(o, "x"), num_field(o, "y"));
        let busy: Vec<_> = self
            .board
            .objects
            .iter()
            .filter(|q| {
                q.id != *id
                    && ![
                        ObjectType::Connector,
                        ObjectType::Frame,
                        ObjectType::Pen,
                        ObjectType::Comment,
                    ]
                    .contains(&q.kind)
            })
            .filter_map(drawn_box)
            .collect();
        let free = |x: f64, y: f64| {
            !busy
                .iter()
                .any(|b| x < b.x + b.w && x + w > b.x && y < b.y + b.h && y + h > b.y)
        };
        let (mut x, mut y) = (ox, oy);
        for k in 1..=8 {
            let k = f64::from(k);
            x = ox + dir_x * k * (w + GAP);
            y = oy + dir_y * k * (h + GAP);
            if free(x, y) {
                break;
            }
        }
        fields.insert("x".into(), Value::Num(js_round(x)));
        fields.insert("y".into(), Value::Num(js_round(y)));
        let copy = self.add_new(&o.kind, fields, &Position::Front)?;
        let end = |target: &Id, side: Side| {
            let mut f = one("id", Value::Str(target.to_string()));
            f.insert("anchor".into(), s(side_name(side)));
            Value::Obj(f)
        };
        let mut line = one("from", end(id, side));
        line.insert("to".into(), end(&copy, opposite));
        self.add_new(&ObjectType::Connector, line, &Position::Front)?;
        Ok(())
    }
}
