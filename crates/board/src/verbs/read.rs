//! The reading verbs: a view of the board, with each object's revision, for an edit to name.

use crate::json::{Fields, Value};
use crate::model::{Board, Id, Object, ObjectType, RefState, Reference};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema;

/// One object as a caller sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectView {
    /// The object.
    pub object: Object,
    /// Its revision: what an edit names.
    pub rev: crate::hash::Sha256,
    /// Its place in z-order, 0 at the back.
    pub position: usize,
    /// References from it and to it. Only [`read`] fills this in.
    pub references: Vec<Reference>,
}

impl ObjectView {
    /// The view as JSON: `{object, rev, position, references}`.
    ///
    /// The tree keeps a number as it is in memory, so a -0.0 stays -0.0 here, and
    /// [`crate::json::to_canonical`] writes it as `0`, as the file does. A host printing this tree
    /// with its own JSON printer normalises -0.0 to 0, or prints through `to_canonical`.
    pub fn to_value(&self) -> Value {
        let mut f = Fields::new();
        f.insert("object".into(), self.object.to_value());
        f.insert("rev".into(), Value::Str(self.rev.to_string()));
        f.insert("position".into(), Value::Num(self.position as f64));
        f.insert(
            "references".into(),
            Value::Arr(self.references.iter().map(reference_value).collect()),
        );
        Value::Obj(f)
    }
}

/// A reference as JSON: `{from, field, target, state, hint?}`.
pub fn reference_value(r: &Reference) -> Value {
    let mut f = Fields::new();
    f.insert("from".into(), Value::Str(r.from.to_string()));
    f.insert("field".into(), Value::Str(r.field.to_string()));
    f.insert("target".into(), Value::Str(r.target.to_string()));
    let state = match r.state {
        RefState::Attached => "attached",
        RefState::Suspect => "suspect",
        RefState::Orphaned => "orphaned",
    };
    f.insert("state".into(), Value::Str(state.to_string()));
    if let Some((x, y)) = r.hint {
        let mut h = Fields::new();
        h.insert("x".into(), Value::Num(x));
        h.insert("y".into(), Value::Num(y));
        f.insert("hint".into(), Value::Obj(h));
    }
    Value::Obj(f)
}

fn view(o: &Object, position: usize) -> Result<ObjectView, Refusal> {
    Ok(ObjectView {
        object: o.clone(),
        rev: o.rev()?,
        position,
        references: Vec::new(),
    })
}

/// Every object back to front, or those of one type.
pub fn list(board: &Board, kind: Option<&ObjectType>) -> Result<Vec<ObjectView>, Refusal> {
    let mut out = Vec::new();
    for (i, o) in board.objects.iter().enumerate() {
        if kind.map_or(true, |k| *k == o.kind) {
            out.push(view(o, i)?);
        }
    }
    Ok(out)
}

/// One object with its references, or `None` if it isn't there.
pub fn read(board: &Board, id: &Id) -> Result<Option<ObjectView>, Refusal> {
    let Some(position) = board.objects.iter().position(|o| o.id == *id) else {
        return Ok(None);
    };
    let mut out = view(&board.objects[position], position)?;
    out.references = board
        .references()
        .into_iter()
        .filter(|r| r.from == *id || r.target == *id)
        .collect();
    Ok(Some(out))
}

/// The board's settings, and the revision a change to them names.
#[derive(Clone, Debug, PartialEq)]
pub struct MetaView {
    /// The settings, unknown ones included.
    pub settings: Fields,
    /// Their revision: what [`super::Verb::Meta`] names.
    pub rev: crate::hash::Sha256,
}

impl MetaView {
    /// The view as JSON: `{settings, rev}`.
    ///
    /// The tree keeps a number as it is in memory, so a -0.0 stays -0.0 here, and
    /// [`crate::json::to_canonical`] writes it as `0`, as the file does. A host printing this tree
    /// with its own JSON printer normalises -0.0 to 0, or prints through `to_canonical`.
    pub fn to_value(&self) -> Value {
        let mut f = Fields::new();
        f.insert("settings".into(), Value::Obj(self.settings.clone()));
        f.insert("rev".into(), Value::Str(self.rev.to_string()));
        Value::Obj(f)
    }
}

/// The board's settings, with their revision.
pub fn meta(board: &Board) -> Result<MetaView, Refusal> {
    Ok(MetaView {
        settings: board.settings.clone(),
        rev: board.settings_rev()?,
    })
}

/// Every template [`super::Verb::Template`] takes: its name, and its title.
pub fn templates() -> &'static [(&'static str, &'static str)] {
    super::TEMPLATES
}

fn push_text(out: &mut String, v: Option<&Value>) {
    if let Some(Value::Str(s)) = v {
        out.push_str(s);
        out.push(' ');
    }
}

fn search_text(o: &Object) -> String {
    let mut out = String::new();
    for name in ["text", "title", "label", "assignee", "status"] {
        push_text(&mut out, o.fields.get(name));
    }
    if let Some(Value::Arr(tags)) = o.fields.get("tags") {
        tags.iter().for_each(|t| push_text(&mut out, Some(t)));
    }
    if let Some(Value::Arr(rows)) = o.fields.get("rows") {
        for row in rows {
            if let Value::Obj(r) = row {
                if let Some(Value::Obj(cells)) = r.get("cells") {
                    cells.values().for_each(|c| push_text(&mut out, Some(c)));
                }
            }
        }
    }
    if let Some(Value::Arr(checks)) = o.fields.get("checks") {
        for item in checks {
            if let Value::Obj(c) = item {
                push_text(&mut out, c.get("text"));
            }
        }
    }
    if let Some(Value::Arr(replies)) = o.fields.get("replies") {
        for reply in replies {
            if let Value::Obj(r) = reply {
                push_text(&mut out, r.get("text"));
            }
        }
    }
    out.to_lowercase()
}

/// Objects whose text holds this, ignoring case: text, titles, labels, assignees, statuses, tags,
/// table cells, check items and replies. An empty search finds nothing.
pub fn search(board: &Board, text: &str) -> Result<Vec<ObjectView>, Refusal> {
    let wanted = text.trim().to_lowercase();
    let mut out = Vec::new();
    if wanted.is_empty() {
        return Ok(out);
    }
    for (i, o) in board.objects.iter().enumerate() {
        if search_text(o).contains(&wanted) {
            out.push(view(o, i)?);
        }
    }
    Ok(out)
}

/// The fields a type holds, with kinds and defaults: `{common, types: {name: fields}}`, every type
/// or one. A type this version doesn't know is the verbs' `unknown_type`.
pub fn fields(type_name: Option<&str>) -> Result<Value, super::VerbError> {
    let Value::Obj(mut top) = schema::schema_value() else {
        return Err(Refusal::new(RefusalCode::FieldValue, "no schema").into());
    };
    let common = top.remove("common").unwrap_or(Value::Null);
    let Some(Value::Obj(mut types)) = top.remove("types") else {
        return Err(Refusal::new(RefusalCode::FieldValue, "no types").into());
    };
    if let Some(name) = type_name {
        let Some(one) = types.remove(name) else {
            return Err(super::refuse_with(
                super::VerbCode::UnknownType,
                format!("{name:?} isn't a type this version knows"),
                super::Facts::type_field(name, None),
            ));
        };
        types = Fields::new();
        types.insert(name.to_string(), one);
    }
    let mut out = Fields::new();
    out.insert("common".into(), common);
    out.insert("types".into(), Value::Obj(types));
    Ok(Value::Obj(out))
}
