//! The verbs that place things. The rules derive from Board, Copyright (c) 2026 Baden Ballard, MIT
//! (see `THIRD-PARTY-NOTICES.md`). They work on stored boxes: a renderer may draw tall text or a
//! measured table larger than its box, and a verb can't see that.

use super::edit::is_locked;
use super::{
    one, refuse_with, s, AlignTo, Axis, Facts, Layer, ObjRev, Planner, VerbCode, VerbError,
};
use crate::json::{Fields, Value};
use crate::model::{Id, Object, ObjectType};
use crate::ops::{Op, Position};
use crate::schema::{self, Default as FieldDefault};
use std::collections::BTreeSet;

/// A box on the board.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

/// ECMAScript's `Math.round`, so a placement lands where a person's would.
pub(crate) fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

pub(crate) fn num_field(o: &Object, name: &str) -> f64 {
    if let Some(Value::Num(n)) = o.fields.get(name) {
        return *n;
    }
    match schema::field_of(o.kind.as_str(), name).and_then(|f| f.default) {
        Some(FieldDefault::Num(n)) => n,
        _ => 0.0,
    }
}

/// An object's stored box. A connector has none. A pen's comes from its points.
pub(crate) fn box_of(o: &Object) -> Option<Rect> {
    if o.kind == ObjectType::Connector {
        return None;
    }
    let (x, y) = (num_field(o, "x"), num_field(o, "y"));
    if o.kind == ObjectType::Pen {
        let (mut x1, mut y1) = (f64::INFINITY, f64::INFINITY);
        let (mut x2, mut y2) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        if let Some(Value::Arr(points)) = o.fields.get("points") {
            for p in points {
                if let Value::Arr(xy) = p {
                    if let (Some(Value::Num(a)), Some(Value::Num(b))) = (xy.first(), xy.get(1)) {
                        (x1, y1) = (x1.min(*a), y1.min(*b));
                        (x2, y2) = (x2.max(*a), y2.max(*b));
                    }
                }
            }
        }
        if x1.is_infinite() {
            return Some(Rect {
                x,
                y,
                w: 1.0,
                h: 1.0,
            });
        }
        return Some(Rect {
            x: x + x1,
            y: y + y1,
            w: (x2 - x1).max(1.0),
            h: (y2 - y1).max(1.0),
        });
    }
    Some(Rect {
        x,
        y,
        w: num_field(o, "w"),
        h: num_field(o, "h"),
    })
}

/// The box as rotated, for spacing things by what's drawn.
pub(crate) fn drawn_box(o: &Object) -> Option<Rect> {
    let b = box_of(o)?;
    let angle = (num_field(o, "rot") % 180.0).to_radians();
    if angle == 0.0 {
        return Some(b);
    }
    let w = (b.w * angle.cos()).abs() + (b.h * angle.sin()).abs();
    let h = (b.w * angle.sin()).abs() + (b.h * angle.cos()).abs();
    Some(Rect {
        x: b.x + b.w / 2.0 - w / 2.0,
        y: b.y + b.h / 2.0 - h / 2.0,
        w,
        h,
    })
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

fn inside(a: Rect, b: Rect) -> bool {
    a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h
}

pub(crate) fn bounds(list: impl Iterator<Item = Rect>) -> Option<Rect> {
    let mut out: Option<(f64, f64, f64, f64)> = None;
    for b in list {
        out = Some(match out {
            None => (b.x, b.y, b.x + b.w, b.y + b.h),
            Some((x1, y1, x2, y2)) => (
                x1.min(b.x),
                y1.min(b.y),
                x2.max(b.x + b.w),
                y2.max(b.y + b.h),
            ),
        });
    }
    out.map(|(x1, y1, x2, y2)| Rect {
        x: x1,
        y: y1,
        w: x2 - x1,
        h: y2 - y1,
    })
}

const TIDY_STAYS: [&ObjectType; 4] = [
    &ObjectType::Connector,
    &ObjectType::Frame,
    &ObjectType::Pen,
    &ObjectType::Comment,
];

const STYLE: [&str; 12] = [
    "color",
    "fill",
    "stroke",
    "strokeWidth",
    "dash",
    "fontSize",
    "align",
    "bold",
    "italic",
    "route",
    "startCap",
    "endCap",
];

impl<'a> Planner<'a, '_> {
    /// Named objects with boxes. A connector has none, so naming one for this is refused.
    fn boxed(&self, ids: &[ObjRev]) -> Result<Vec<(&'a Object, ObjRev, Rect)>, VerbError> {
        let mut out = Vec::with_capacity(ids.len());
        for t in ids {
            let o = self.obj(&t.id)?;
            let Some(b) = box_of(o) else {
                return Err(refuse_with(
                    VerbCode::WrongType,
                    format!("{} is a connector: its ends place it", t.id),
                    Facts::object(&t.id, o.kind.as_str()),
                ));
            };
            out.push((o, t.clone(), b));
        }
        Ok(out)
    }

    /// Sets `x` or `y` to move a box to a place, only if that changes it.
    fn place_op(&mut self, o: &Object, t: &ObjRev, x_by: f64, y_by: f64) {
        let mut set = Fields::new();
        if x_by != 0.0 {
            set.insert("x".into(), Value::Num(num_field(o, "x") + x_by));
        }
        if y_by != 0.0 {
            set.insert("y".into(), Value::Num(num_field(o, "y") + y_by));
        }
        if !set.is_empty() {
            self.set_op(&t.id, t.rev, set, Vec::new());
        }
    }

    pub(crate) fn move_by(&mut self, ids: &[ObjRev], dx: f64, dy: f64) -> Result<(), VerbError> {
        let named: BTreeSet<&Id> = ids.iter().map(|t| &t.id).collect();
        let mut boxes = Vec::new();
        for (o, t, _) in self.boxed(ids)? {
            boxes.push((o, t));
        }
        // A group's members come along.
        let groups: BTreeSet<&str> = boxes
            .iter()
            .filter_map(|(o, _)| match o.fields.get("group") {
                Some(Value::Str(g)) => Some(g.as_str()),
                _ => None,
            })
            .collect();
        let mut along: BTreeSet<&Id> = BTreeSet::new();
        for o in &self.board.objects {
            let in_group =
                matches!(o.fields.get("group"), Some(Value::Str(g)) if groups.contains(g.as_str()));
            if in_group && o.kind != ObjectType::Connector && !named.contains(&o.id) {
                along.insert(&o.id);
            }
        }
        // A frame's contents come along: its members by field, or by sitting inside it.
        let frames: Vec<&Object> = self
            .board
            .objects
            .iter()
            .filter(|o| {
                o.kind == ObjectType::Frame && (named.contains(&o.id) || along.contains(&o.id))
            })
            .collect();
        for frame in &frames {
            let Some(fb) = box_of(frame) else { continue };
            for o in &self.board.objects {
                if o.id == frame.id
                    || o.kind == ObjectType::Frame
                    || o.kind == ObjectType::Connector
                    || named.contains(&o.id)
                {
                    continue;
                }
                let holds = match o.fields.get("frame") {
                    Some(Value::Str(f)) => f == frame.id.as_str(),
                    _ => box_of(o).is_some_and(|b| inside(b, fb)),
                };
                if holds {
                    along.insert(&o.id);
                }
            }
        }
        for (o, t) in &boxes {
            self.place_op(o, t, dx, dy);
        }
        for o in &self.board.objects {
            if along.contains(&o.id) {
                if is_locked(o) {
                    self.skipped.push(o.id.clone());
                } else {
                    let t = ObjRev {
                        id: o.id.clone(),
                        rev: o.rev()?,
                    };
                    self.place_op(o, &t, dx, dy);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn order(&mut self, ids: &[Id], to: &Layer) -> Result<(), VerbError> {
        let named: BTreeSet<&Id> = ids.iter().collect();
        for id in ids {
            self.obj(id)?;
        }
        // Back to front, so they keep their order among themselves.
        let list: Vec<(usize, &'a Object)> = self
            .board
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| named.contains(&o.id))
            .collect();
        let Some((lowest, highest)) = list.first().zip(list.last()).map(|(a, b)| (a.0, b.0)) else {
            return Ok(());
        };
        let chain = |first: Position, ops: &mut Vec<Op>| {
            let mut at = first;
            for (_, o) in &list {
                ops.push(Op::Z {
                    id: o.id.clone(),
                    to: at,
                });
                at = Position::After(vec![o.id.clone()]);
            }
        };
        match to {
            Layer::Front => chain(Position::Front, &mut self.ops),
            Layer::After(names) => chain(Position::After(names.clone()), &mut self.ops),
            Layer::Back => chain(Position::Back, &mut self.ops),
            Layer::Forward | Layer::Backward => {
                let selected = bounds(list.iter().filter_map(|(_, o)| box_of(o)));
                let Some(selected) = selected else {
                    return Ok(());
                };
                let overlaps = |o: &Object| {
                    o.kind != ObjectType::Frame
                        && !named.contains(&o.id)
                        && box_of(o).is_some_and(|b| intersects(b, selected))
                };
                let objects = &self.board.objects;
                if *to == Layer::Forward {
                    if let Some(t) = objects[highest..].iter().find(|o| overlaps(o)) {
                        chain(Position::After(vec![t.id.clone()]), &mut self.ops);
                    }
                } else if let Some(i) = objects[..lowest].iter().rposition(overlaps) {
                    let first = match i.checked_sub(1) {
                        Some(below) => Position::After(vec![objects[below].id.clone()]),
                        None => Position::Back,
                    };
                    chain(first, &mut self.ops);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn group(&mut self, ids: &[ObjRev]) -> Result<(), VerbError> {
        if ids.len() < 2 {
            return Err(refuse_with(
                VerbCode::TooFew,
                "a group needs at least two objects",
                Facts::min(2),
            ));
        }
        for t in ids {
            self.obj(&t.id)?;
        }
        let group = self.mint("g")?;
        for t in ids {
            self.set_op(
                &t.id,
                t.rev,
                one("group", Value::Str(group.to_string())),
                Vec::new(),
            );
        }
        Ok(())
    }

    pub(crate) fn ungroup(&mut self, ids: &[ObjRev]) -> Result<(), VerbError> {
        for t in ids {
            if self.obj(&t.id)?.fields.contains_key("group") {
                self.set_op(&t.id, t.rev, Fields::new(), vec!["group".to_string()]);
            } else {
                self.skipped.push(t.id.clone());
            }
        }
        Ok(())
    }

    pub(crate) fn align(&mut self, ids: &[ObjRev], to: AlignTo) -> Result<(), VerbError> {
        if ids.len() < 2 {
            return Err(refuse_with(
                VerbCode::TooFew,
                "lining up needs at least two objects",
                Facts::min(2),
            ));
        }
        let list = self.boxed(ids)?;
        let Some(all) = bounds(list.iter().map(|(_, _, b)| *b)) else {
            return Ok(());
        };
        for (o, t, b) in &list {
            let (x_by, y_by) = match to {
                AlignTo::Left => (all.x - b.x, 0.0),
                AlignTo::Right => (js_round(all.x + all.w - b.w) - b.x, 0.0),
                AlignTo::Center => (js_round(all.x + all.w / 2.0 - b.w / 2.0) - b.x, 0.0),
                AlignTo::Top => (0.0, all.y - b.y),
                AlignTo::Bottom => (0.0, js_round(all.y + all.h - b.h) - b.y),
                AlignTo::Middle => (0.0, js_round(all.y + all.h / 2.0 - b.h / 2.0) - b.y),
            };
            self.place_op(o, t, x_by, y_by);
        }
        Ok(())
    }

    pub(crate) fn distribute(&mut self, ids: &[ObjRev], axis: Axis) -> Result<(), VerbError> {
        if ids.len() < 3 {
            return Err(refuse_with(
                VerbCode::TooFew,
                "spacing out needs at least three objects",
                Facts::min(3),
            ));
        }
        let mut list = self.boxed(ids)?;
        let across = axis == Axis::Horizontal;
        let start = |b: &Rect| if across { b.x } else { b.y };
        let size = |b: &Rect| if across { b.w } else { b.h };
        list.sort_by(|a, b| start(&a.2).total_cmp(&start(&b.2)));
        let first = list[0].2;
        let last = list[list.len() - 1].2;
        let total: f64 = list.iter().map(|x| size(&x.2)).sum();
        let gap = (start(&last) + size(&last) - start(&first) - total) / (list.len() - 1) as f64;
        let mut pos = start(&first);
        for (o, t, b) in &list {
            let by = js_round(pos) - start(b);
            if across {
                self.place_op(o, t, by, 0.0);
            } else {
                self.place_op(o, t, 0.0, by);
            }
            pos += size(b) + gap;
        }
        Ok(())
    }

    pub(crate) fn tidy(&mut self, ids: &[ObjRev]) -> Result<(), VerbError> {
        const GAP: f64 = 24.0;
        struct Item<'o> {
            o: &'o Object,
            t: ObjRev,
            b: Rect,
        }
        let mut items = Vec::new();
        for t in ids {
            let o = self.obj(&t.id)?;
            match drawn_box(o) {
                Some(b) if !TIDY_STAYS.contains(&&o.kind) => {
                    items.push(Item { o, t: t.clone(), b })
                }
                _ => self.skipped.push(t.id.clone()),
            }
        }
        if items.len() < 2 {
            return Err(refuse_with(
                VerbCode::TooFew,
                "tidying needs at least two objects it can move",
                Facts::min(2),
            ));
        }
        let mut heights: Vec<f64> = items.iter().map(|i| i.b.h).collect();
        heights.sort_by(f64::total_cmp);
        let mid = heights[heights.len() / 2];
        let centre = |i: &Item| i.b.y + i.b.h / 2.0;
        let (mut y, x0) = items
            .iter()
            .fold((f64::INFINITY, f64::INFINITY), |(y, x), i| {
                (y.min(i.b.y), x.min(i.b.x))
            });
        items.sort_by(|a, b| centre(a).total_cmp(&centre(b)));
        // Keep the rows people made, by height, and their order in each.
        let mut rows: Vec<(f64, Vec<Item>)> = Vec::new();
        for item in items {
            match rows.last_mut() {
                Some((cy, row)) if centre(&item) - *cy < mid * 0.6 => row.push(item),
                _ => rows.push((centre(&item), vec![item])),
            }
        }
        for (_, mut row) in rows {
            row.sort_by(|a, b| a.b.x.total_cmp(&b.b.x));
            let row_h = row.iter().fold(0.0_f64, |h, i| h.max(i.b.h));
            let mut x = x0;
            for i in &row {
                let new_x = js_round(num_field(i.o, "x") + (x - i.b.x));
                let new_y = js_round(num_field(i.o, "y") + (y + (row_h - i.b.h) / 2.0 - i.b.y));
                self.place_op(
                    i.o,
                    &i.t,
                    new_x - num_field(i.o, "x"),
                    new_y - num_field(i.o, "y"),
                );
                x += i.b.w + GAP;
            }
            y += row_h + GAP;
        }
        Ok(())
    }

    pub(crate) fn copy_style(&mut self, from: &Id, to: &[ObjRev]) -> Result<(), VerbError> {
        let source = self.obj(from)?;
        let source_type = source.kind.as_str().to_string();
        for t in to {
            let o = self.obj(&t.id)?;
            let mut set = Fields::new();
            for name in STYLE {
                let Some(v) = source.fields.get(name) else {
                    continue;
                };
                let (Some(a), Some(b)) = (
                    schema::field_of(&source_type, name),
                    schema::field_of(o.kind.as_str(), name),
                ) else {
                    continue;
                };
                if a.kind == b.kind && o.fields.get(name) != Some(v) {
                    set.insert(name.to_string(), v.clone());
                }
            }
            if set.is_empty() {
                self.skipped.push(t.id.clone());
            } else {
                self.set_op(&t.id, t.rev, set, Vec::new());
            }
        }
        Ok(())
    }

    pub(crate) fn mind_child(&mut self, parent: &Id, text: &str) -> Result<(), VerbError> {
        const COLOURS: [&str; 6] = ["blue", "green", "orange", "pink", "teal", "violet"];
        let p = self.obj(parent)?;
        if p.kind != ObjectType::Mind {
            return Err(refuse_with(
                VerbCode::WrongType,
                format!(
                    "{parent} is a {}: only a mind node has children",
                    p.kind.as_str()
                ),
                Facts::object(parent, p.kind.as_str()),
            ));
        }
        let parent_of = |o: &Object| match o.fields.get("parent") {
            Some(Value::Str(s)) => Id::new(s).ok(),
            _ => None,
        };
        let kids: Vec<&Object> = self
            .board
            .objects
            .iter()
            .filter(|o| parent_of(o).as_ref() == Some(parent))
            .collect();
        let mut root = p;
        let mut hops = 0;
        while let Some(up) = parent_of(root).and_then(|u| self.index.get(u.as_str()).copied()) {
            root = up;
            hops += 1;
            if hops > self.board.objects.len() {
                break; // a loop: where it stops doesn't matter
            }
        }
        let px = num_field(p, "x");
        let has_parent = p.fields.contains_key("parent");
        let left = if has_parent {
            px < num_field(root, "x")
        } else {
            let on_left = kids.iter().filter(|k| num_field(k, "x") < px).count();
            on_left < kids.len() - on_left
        };
        let x = if left {
            px - 240.0
        } else {
            px + num_field(p, "w") + 80.0
        };
        let y = if kids.is_empty() {
            num_field(p, "y") + num_field(p, "h") / 2.0 - 24.0
        } else {
            kids.iter().fold(f64::NEG_INFINITY, |m, k| {
                m.max(num_field(k, "y") + num_field(k, "h"))
            }) + 20.0
        };
        let color = if has_parent {
            p.fields.get("color").cloned()
        } else {
            Some(s(COLOURS[kids.len() % COLOURS.len()]))
        };
        let mut fields = one("x", Value::Num(js_round(x)));
        fields.insert("y".into(), Value::Num(js_round(y)));
        fields.insert("w".into(), Value::Num(160.0));
        fields.insert("h".into(), Value::Num(48.0));
        fields.insert("text".into(), s(text));
        fields.insert("parent".into(), Value::Str(parent.to_string()));
        if let Some(color) = color {
            fields.insert("color".into(), color);
        }
        self.add_new(&ObjectType::Mind, fields, &Position::Front)?;
        Ok(())
    }
}
