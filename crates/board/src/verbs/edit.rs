//! The verbs that add, change and delete. Rules for delete, votes and comments derive from Board,
//! Copyright (c) 2026 Baden Ballard, MIT (see `THIRD-PARTY-NOTICES.md`).

use super::{
    known_fields, no_id_or_type, one, refuse, refuse_with, s, End, Facts, ObjRev, Planner, Removed,
    VerbCode, VerbError,
};
use crate::hash::Sha256;
use crate::json::{Fields, Value};
use crate::model::{Id, Object, ObjectType, RefState, Reference};
use crate::ops::{Conflict, Op, Position};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema;
use std::collections::{BTreeSet, HashMap};

/// How many neighbours a removed object remembers.
const NEIGHBOURS: usize = 32;

pub(crate) fn is_locked(o: &Object) -> bool {
    matches!(o.fields.get("locked"), Some(Value::Bool(true)))
}

/// An end's object id, for a connector end in either form.
pub(crate) fn end_id(v: Option<&Value>) -> Option<&str> {
    match v {
        Some(Value::Obj(f)) => match f.get("id") {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

fn str_of<'v>(o: &'v Object, name: &str) -> Option<&'v str> {
    match o.fields.get(name) {
        Some(Value::Str(s)) => Some(s),
        _ => None,
    }
}

fn list_of(o: &Object, name: &str) -> Vec<Value> {
    match o.fields.get(name) {
        Some(Value::Arr(a)) => a.clone(),
        _ => Vec::new(),
    }
}

fn item_id(v: &Value) -> Option<&str> {
    match v {
        Value::Obj(f) => match f.get("id") {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

/// `object` has no item `id` in its list of `what`.
fn no_item(object: &Id, what: &str, id: &str) -> VerbError {
    refuse_with(
        VerbCode::NoSuchItem,
        format!("no {what} {id:?}"),
        Facts {
            id: Some(object.clone()),
            item: Some(id.to_string()),
            ..Facts::default()
        },
    )
}

fn place_of(object: &Id, list: &[Value], id: &str, what: &str) -> Result<usize, VerbError> {
    list.iter()
        .position(|v| item_id(v) == Some(id))
        .ok_or_else(|| no_item(object, what, id))
}

/// Just past a named item, or at the start with none.
fn after_place(
    object: &Id,
    list: &[Value],
    after: Option<&str>,
    what: &str,
) -> Result<usize, VerbError> {
    match after {
        Some(id) => Ok(place_of(object, list, id, what)? + 1),
        None => Ok(0),
    }
}

fn wrong_type(o: &Object, what: &str) -> VerbError {
    refuse_with(
        VerbCode::WrongType,
        format!("{} is a {}: {what}", o.id, o.kind.as_str()),
        Facts::object(&o.id, o.kind.as_str()),
    )
}

impl<'a> Planner<'a, '_> {
    pub(crate) fn add(
        &mut self,
        kind: &ObjectType,
        fields: &Fields,
        at: &Position,
    ) -> Result<(), VerbError> {
        no_id_or_type(fields)?;
        known_fields(kind, fields.keys().map(String::as_str))?;
        let mut fields = fields.clone();
        if *kind == ObjectType::Table
            && !fields.contains_key("columns")
            && !fields.contains_key("rows")
        {
            let mut columns = Vec::new();
            for _ in 0..3 {
                columns.push(Value::Obj(one("id", Value::Str(self.mint_sub("cl")?))));
            }
            let mut rows = Vec::new();
            for _ in 0..3 {
                rows.push(Value::Obj(one("id", Value::Str(self.mint_sub("rw")?))));
            }
            fields.insert("columns".into(), Value::Arr(columns));
            fields.insert("rows".into(), Value::Arr(rows));
        }
        self.add_new(kind, fields, at)?;
        Ok(())
    }

    /// What hangs off an object: attached connectors, mind children, pinned comments, and on down.
    fn dependents(&self, start: &Id) -> BTreeSet<Id> {
        let mut edges: HashMap<&str, Vec<&Id>> = HashMap::new();
        for o in &self.board.objects {
            let mut targets: Vec<&str> = Vec::new();
            match o.kind {
                ObjectType::Connector => {
                    targets.extend(end_id(o.fields.get("from")));
                    targets.extend(end_id(o.fields.get("to")));
                }
                ObjectType::Mind => targets.extend(str_of(o, "parent")),
                ObjectType::Comment => targets.extend(str_of(o, "target")),
                _ => {}
            }
            for t in targets {
                edges.entry(t).or_default().push(&o.id);
            }
        }
        let mut gone = BTreeSet::new();
        let mut walk = vec![start.clone()];
        while let Some(next) = walk.pop() {
            for dep in edges.get(next.as_str()).into_iter().flatten() {
                if **dep != *start && gone.insert((*dep).clone()) {
                    walk.push((*dep).clone());
                }
            }
        }
        gone
    }

    /// Changes the named fields of an object. One that isn't there is left to the op, which
    /// conflicts.
    pub(crate) fn update(
        &mut self,
        id: &Id,
        rev: Sha256,
        set: &Fields,
        unset: &[String],
    ) -> Result<(), VerbError> {
        if let Some(o) = self.index.get(id.as_str()) {
            let names = set.keys().map(String::as_str);
            known_fields(&o.kind, names.chain(unset.iter().map(String::as_str)))?;
        }
        self.set_op(id, rev, set.clone(), unset.to_vec());
        Ok(())
    }

    pub(crate) fn delete(&mut self, id: &Id, rev: Sha256, cascade: bool) -> Result<(), VerbError> {
        if !self.index.contains_key(id.as_str()) {
            // A raw del of it changes nothing. Asked for by name, it's a conflict: the caller read
            // something that isn't there now.
            return Err(Conflict::Deleted { id: id.clone() }.into());
        }
        self.ops.push(Op::Del {
            id: id.clone(),
            rev,
        });
        let deps = if cascade {
            self.dependents(id)
        } else {
            BTreeSet::new()
        };
        for o in &self.board.objects {
            if deps.contains(&o.id) {
                let rev = o.rev()?;
                if is_locked(o) {
                    // The rule is that it follows what it hangs off, and the report names it.
                    self.set_op(&o.id, rev, one("locked", Value::Bool(false)), Vec::new());
                    self.removed_locked.push(o.id.clone());
                }
                self.ops.push(Op::Del {
                    id: o.id.clone(),
                    rev,
                });
            }
        }
        let frames = &self.board.frames;
        for (i, o) in self.board.objects.iter().enumerate() {
            if o.id == *id || deps.contains(&o.id) {
                let after = self.board.objects[..i]
                    .iter()
                    .rev()
                    .take(NEIGHBOURS)
                    .map(|n| n.id.clone())
                    .collect();
                let frame_after = match frames.iter().position(|f| *f == o.id) {
                    Some(at) => frames[..at]
                        .iter()
                        .rev()
                        .take(NEIGHBOURS)
                        .cloned()
                        .collect(),
                    None => Vec::new(),
                };
                self.removed.push(Removed {
                    object: o.clone(),
                    after,
                    frame_after,
                });
            }
        }
        for r in self.board.references() {
            let target_goes = r.target == *id || deps.contains(&r.target);
            let holder_goes = r.from == *id || deps.contains(&r.from);
            if target_goes && !holder_goes {
                self.orphans.push(Reference {
                    state: RefState::Orphaned,
                    ..r
                });
            }
        }
        Ok(())
    }

    pub(crate) fn put_back(&mut self, removed: &Removed) -> Result<(), VerbError> {
        let mut object = removed.object.clone();
        let is_frame = object.kind == ObjectType::Frame;
        // A move of a locked frame refuses, so it's locked again after its move.
        let relock = is_frame && is_locked(&object);
        if relock {
            object.fields.remove("locked");
        }
        let id = object.id.clone();
        let rev = object.rev()?;
        self.ops.push(Op::Add {
            object,
            at: Position::After(removed.after.clone()),
        });
        if is_frame {
            // With nothing before it, this is the first place.
            self.ops.push(Op::FrameOrder {
                id: id.clone(),
                to: Position::After(removed.frame_after.clone()),
            });
        }
        if relock {
            self.set_op(&id, rev, one("locked", Value::Bool(true)), Vec::new());
        }
        self.ids.push(id);
        Ok(())
    }

    fn end_value(&self, end: &End) -> Result<Value, VerbError> {
        match end {
            End::Object { id, anchor } => {
                self.obj(id)?;
                let mut f = one("id", Value::Str(id.to_string()));
                f.insert(
                    "anchor".into(),
                    Value::Str(anchor.clone().unwrap_or_else(|| "auto".to_string())),
                );
                Ok(Value::Obj(f))
            }
            End::Point { x, y } => {
                let mut f = one("x", Value::Num(*x));
                f.insert("y".into(), Value::Num(*y));
                Ok(Value::Obj(f))
            }
        }
    }

    pub(crate) fn connect(
        &mut self,
        from: &End,
        to: &End,
        fields: &Fields,
    ) -> Result<(), VerbError> {
        no_id_or_type(fields)?;
        for name in ["from", "to"] {
            if fields.contains_key(name) {
                return Err(Refusal::at(
                    RefusalCode::FieldValue,
                    format!("/{name}"),
                    "the verb sets this itself",
                )
                .into());
            }
        }
        known_fields(&ObjectType::Connector, fields.keys().map(String::as_str))?;
        let mut fields = fields.clone();
        fields.insert("from".into(), self.end_value(from)?);
        fields.insert("to".into(), self.end_value(to)?);
        self.add_new(&ObjectType::Connector, fields, &Position::Front)?;
        Ok(())
    }

    pub(crate) fn react(
        &mut self,
        id: &Id,
        emoji: &str,
        on: Option<bool>,
    ) -> Result<(), VerbError> {
        let o = self.obj(id)?;
        let token = self.token()?;
        let mine = Value::Arr(vec![Value::Str(token.to_string())]);
        let mut entries = Fields::new();
        if let Some(Value::Obj(all)) = o.fields.get("reactions") {
            for (key, list) in all {
                let has = matches!(list, Value::Arr(a)
                    if a.iter().any(|v| matches!(v, Value::Str(t) if t == token.as_str())));
                if has {
                    entries.insert(key.clone(), mine.clone());
                }
            }
        }
        let on = on.unwrap_or(!entries.contains_key(emoji));
        if on {
            entries.insert(emoji.to_string(), mine);
        } else {
            entries.remove(emoji);
        }
        let rev = o.rev()?;
        self.set_op(id, rev, one("reactions", Value::Obj(entries)), Vec::new());
        Ok(())
    }

    fn votes_of(o: &Object, token: &str) -> f64 {
        match o.fields.get("votes") {
            Some(Value::Obj(v)) => match v.get(token) {
                Some(Value::Num(n)) => *n,
                _ => 0.0,
            },
            _ => 0.0,
        }
    }

    pub(crate) fn vote(&mut self, id: &Id, delta: i32) -> Result<(), VerbError> {
        let o = self.obj(id)?;
        let token = self.token()?;
        if delta == 0 {
            return Err(refuse(VerbCode::BadValue, "a vote changes by at least one"));
        }
        let Some(Value::Obj(voting)) = self.board.settings.get("voting") else {
            return Err(refuse(VerbCode::VotingOff, "voting isn't on"));
        };
        if voting.get("active") != Some(&Value::Bool(true)) {
            return Err(refuse(VerbCode::VotingOff, "voting isn't on"));
        }
        if delta > 0 {
            if let Some(Value::Num(per)) = voting.get("perPerson") {
                let used: f64 = self
                    .board
                    .objects
                    .iter()
                    .map(|x| Self::votes_of(x, token.as_str()))
                    .sum();
                if used + f64::from(delta) > *per {
                    return Err(refuse(
                        VerbCode::NoVotesLeft,
                        format!("{used} of {per} votes used"),
                    ));
                }
            }
        }
        let next = Self::votes_of(o, token.as_str()) + f64::from(delta);
        let entry = if next > 0.0 {
            one(token.as_str(), Value::Num(next))
        } else {
            Fields::new()
        };
        let rev = o.rev()?;
        self.set_op(id, rev, one("votes", Value::Obj(entry)), Vec::new());
        Ok(())
    }

    pub(crate) fn comment(
        &mut self,
        target: Option<&Id>,
        text: &str,
        point: Option<(f64, f64)>,
    ) -> Result<(), VerbError> {
        if text.is_empty() {
            return Err(refuse(VerbCode::BadValue, "a comment needs text"));
        }
        let mut fields = one("text", s(text));
        let (x, y) = match (target, point) {
            (_, Some(p)) => p,
            (Some(t), None) => {
                let o = self.obj(t)?;
                let w = match o.fields.get("w") {
                    Some(Value::Num(w)) => *w,
                    _ => 0.0,
                };
                let at = |name: &str| match o.fields.get(name) {
                    Some(Value::Num(n)) => *n,
                    _ => 0.0,
                };
                (at("x") + w - 12.0, at("y") - 20.0)
            }
            (None, None) => (0.0, 0.0),
        };
        if let Some(t) = target {
            self.obj(t)?;
            fields.insert("target".into(), Value::Str(t.to_string()));
        }
        fields.insert("x".into(), Value::Num(x));
        fields.insert("y".into(), Value::Num(y));
        self.add_new(&ObjectType::Comment, fields, &Position::Front)?;
        Ok(())
    }

    pub(crate) fn reply(&mut self, id: &Id, text: &str) -> Result<(), VerbError> {
        let o = self.obj(id)?;
        if o.kind != ObjectType::Comment {
            return Err(wrong_type(o, "only a comment takes replies"));
        }
        let token = self.token()?;
        let Some(at) = self.cx.at.clone() else {
            return Err(refuse(VerbCode::NeedsActor, "a reply needs the time"));
        };
        let reply_id = self.mint_sub("rp")?;
        let mut item = one("id", Value::Str(reply_id));
        item.insert("by".into(), Value::Str(token.to_string()));
        item.insert("at".into(), Value::Str(at));
        item.insert("text".into(), s(text));
        let mut list = list_of(o, "replies");
        list.push(Value::Obj(item));
        let rev = o.rev()?;
        self.set_op(id, rev, one("replies", Value::Arr(list)), Vec::new());
        Ok(())
    }

    pub(crate) fn resolve(
        &mut self,
        id: &Id,
        rev: Sha256,
        resolved: bool,
    ) -> Result<(), VerbError> {
        let o = self.obj(id)?;
        if o.kind != ObjectType::Comment {
            return Err(wrong_type(o, "only a comment resolves"));
        }
        self.set_op(id, rev, one("resolved", Value::Bool(resolved)), Vec::new());
        Ok(())
    }

    pub(crate) fn lock(&mut self, ids: &[ObjRev], locked: bool) -> Result<(), VerbError> {
        for t in ids {
            let o = self.obj(&t.id)?;
            if is_locked(o) == locked {
                self.skipped.push(t.id.clone());
            } else {
                self.set_op(&t.id, t.rev, one("locked", Value::Bool(locked)), Vec::new());
            }
        }
        Ok(())
    }

    fn with_checks(&self, id: &Id) -> Result<&'a Object, VerbError> {
        let o = self.obj(id)?;
        if schema::field_of(o.kind.as_str(), "checks").is_none() {
            return Err(wrong_type(o, "it holds no check items"));
        }
        Ok(o)
    }

    pub(crate) fn check_add(&mut self, id: &Id, text: &str) -> Result<(), VerbError> {
        let o = self.with_checks(id)?;
        let mut item = one("id", Value::Str(self.mint_sub("ck")?));
        item.insert("text".into(), s(text));
        item.insert("done".into(), Value::Bool(false));
        let mut list = list_of(o, "checks");
        list.push(Value::Obj(item));
        let rev = o.rev()?;
        self.set_op(id, rev, one("checks", Value::Arr(list)), Vec::new());
        Ok(())
    }

    pub(crate) fn check_set(
        &mut self,
        id: &Id,
        rev: Sha256,
        item: &str,
        text: Option<&str>,
        done: Option<bool>,
    ) -> Result<(), VerbError> {
        let o = self.with_checks(id)?;
        let mut list = list_of(o, "checks");
        let at = place_of(id, &list, item, "check item")?;
        if let Value::Obj(f) = &mut list[at] {
            if let Some(text) = text {
                f.insert("text".into(), s(text));
            }
            if let Some(done) = done {
                f.insert("done".into(), Value::Bool(done));
            }
        }
        self.set_op(id, rev, one("checks", Value::Arr(list)), Vec::new());
        Ok(())
    }

    pub(crate) fn check_remove(
        &mut self,
        id: &Id,
        rev: Sha256,
        item: &str,
    ) -> Result<(), VerbError> {
        let o = self.with_checks(id)?;
        let mut list = list_of(o, "checks");
        let at = place_of(id, &list, item, "check item")?;
        list.remove(at);
        self.set_op(id, rev, one("checks", Value::Arr(list)), Vec::new());
        Ok(())
    }

    fn table(&self, id: &Id) -> Result<&'a Object, VerbError> {
        let o = self.obj(id)?;
        if o.kind != ObjectType::Table {
            return Err(wrong_type(o, "it isn't a table"));
        }
        Ok(o)
    }

    pub(crate) fn table_cell(
        &mut self,
        id: &Id,
        rev: Sha256,
        row: &str,
        column: &str,
        text: &str,
    ) -> Result<(), VerbError> {
        let o = self.table(id)?;
        place_of(id, &list_of(o, "columns"), column, "column")?;
        let mut rows = list_of(o, "rows");
        let at = place_of(id, &rows, row, "row")?;
        if let Value::Obj(f) = &mut rows[at] {
            match f
                .entry("cells".to_string())
                .or_insert(Value::Obj(Fields::new()))
            {
                Value::Obj(cells) => {
                    cells.insert(column.to_string(), s(text));
                }
                other => *other = Value::Obj(one(column, s(text))),
            }
        }
        self.set_op(id, rev, one("rows", Value::Arr(rows)), Vec::new());
        Ok(())
    }

    pub(crate) fn table_add_row(&mut self, id: &Id, after: Option<&str>) -> Result<(), VerbError> {
        let o = self.table(id)?;
        let mut rows = list_of(o, "rows");
        let at = after_place(id, &rows, after, "row")?;
        rows.insert(at, Value::Obj(one("id", Value::Str(self.mint_sub("rw")?))));
        let rev = o.rev()?;
        self.set_op(id, rev, one("rows", Value::Arr(rows)), Vec::new());
        Ok(())
    }

    pub(crate) fn table_remove_row(
        &mut self,
        id: &Id,
        rev: Sha256,
        row: &str,
    ) -> Result<(), VerbError> {
        let o = self.table(id)?;
        let mut rows = list_of(o, "rows");
        let at = place_of(id, &rows, row, "row")?;
        rows.remove(at);
        self.set_op(id, rev, one("rows", Value::Arr(rows)), Vec::new());
        Ok(())
    }

    pub(crate) fn table_add_column(
        &mut self,
        id: &Id,
        after: Option<&str>,
        width: Option<f64>,
    ) -> Result<(), VerbError> {
        let o = self.table(id)?;
        let mut columns = list_of(o, "columns");
        let at = after_place(id, &columns, after, "column")?;
        let mut item = one("id", Value::Str(self.mint_sub("cl")?));
        if let Some(w) = width {
            item.insert("width".into(), Value::Num(w));
        }
        columns.insert(at, Value::Obj(item));
        let rev = o.rev()?;
        self.set_op(id, rev, one("columns", Value::Arr(columns)), Vec::new());
        Ok(())
    }

    pub(crate) fn table_remove_column(
        &mut self,
        id: &Id,
        rev: Sha256,
        column: &str,
    ) -> Result<(), VerbError> {
        let o = self.table(id)?;
        let mut columns = list_of(o, "columns");
        let at = place_of(id, &columns, column, "column")?;
        columns.remove(at);
        let mut rows = list_of(o, "rows");
        for row in &mut rows {
            if let Value::Obj(f) = row {
                if let Some(Value::Obj(cells)) = f.get_mut("cells") {
                    cells.remove(column);
                }
            }
        }
        let mut set = one("columns", Value::Arr(columns));
        set.insert("rows".into(), Value::Arr(rows));
        self.set_op(id, rev, set, Vec::new());
        Ok(())
    }
}
