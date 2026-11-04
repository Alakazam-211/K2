//! Raw ops as JSON: full power, with revisions, and ids as given. `docs/VERBS.md` has the form.
//! Strict: a member an op doesn't know is refused, so a misspelt one can't be dropped unnoticed.

use super::{one, refuse_with, Ctx, Facts, Planned, Planner, VerbCode, VerbError};
use crate::hash::Sha256;
use crate::json::{self, Fields, Value};
use crate::limits::Limits;
use crate::model::{Id, Object, ObjectType, Person};
use crate::ops::{Group, Op, Position};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema::{self, Kind};
use std::collections::{BTreeMap, BTreeSet};

fn bad(at: &str, what: impl Into<String>) -> Refusal {
    Refusal::at(RefusalCode::FieldValue, at, what)
}

fn only(f: &Fields, at: &str, names: &[&str]) -> Result<(), Refusal> {
    for key in f.keys() {
        if !names.contains(&key.as_str()) {
            return Err(bad(at, format!("this op has no member {key:?}")));
        }
    }
    Ok(())
}

fn str_member<'v>(f: &'v Fields, name: &str, at: &str) -> Result<&'v str, Refusal> {
    match f.get(name) {
        Some(Value::Str(s)) => Ok(s),
        _ => Err(bad(at, format!("{name} must be text"))),
    }
}

fn id_member(f: &Fields, name: &str, at: &str) -> Result<Id, Refusal> {
    Id::new(str_member(f, name, at)?)
}

fn rev_member(f: &Fields, at: &str) -> Result<Sha256, Refusal> {
    Sha256::parse(str_member(f, "rev", at)?)
}

fn obj_member(f: &Fields, name: &str, at: &str) -> Result<Fields, Refusal> {
    match f.get(name) {
        None => Ok(Fields::new()),
        Some(Value::Obj(o)) => Ok(o.clone()),
        Some(_) => Err(bad(at, format!("{name} must be an object"))),
    }
}

fn unset_member(f: &Fields, at: &str) -> Result<Vec<String>, Refusal> {
    match f.get("unset") {
        None => Ok(Vec::new()),
        Some(Value::Arr(a)) => a
            .iter()
            .map(|v| match v {
                Value::Str(s) => Ok(s.clone()),
                _ => Err(bad(at, "unset must be a list of names")),
            })
            .collect(),
        Some(_) => Err(bad(at, "unset must be a list of names")),
    }
}

fn position(v: Option<&Value>, at: &str) -> Result<Position, Refusal> {
    match v {
        None => Ok(Position::Front),
        Some(Value::Str(s)) if s == "front" => Ok(Position::Front),
        Some(Value::Str(s)) if s == "back" => Ok(Position::Back),
        Some(Value::Obj(f)) => {
            only(f, at, &["after"])?;
            let Some(Value::Arr(a)) = f.get("after") else {
                return Err(bad(at, "after must be a list of ids"));
            };
            let mut ids = Vec::with_capacity(a.len());
            for v in a {
                let Value::Str(s) = v else {
                    return Err(bad(at, "after must be a list of ids"));
                };
                ids.push(Id::new(s)?);
            }
            Ok(Position::After(ids))
        }
        Some(_) => Err(bad(
            at,
            "a position is \"front\", \"back\" or {\"after\": [ids]}",
        )),
    }
}

fn object(v: Option<&Value>, at: &str) -> Result<Object, Refusal> {
    let Some(Value::Obj(f)) = v else {
        return Err(bad(at, "obj must be an object"));
    };
    let mut fields = f.clone();
    let id = id_member(f, "id", at)?;
    let kind = ObjectType::parse(str_member(f, "type", at)?)?;
    fields.remove("id");
    fields.remove("type");
    Ok(Object { id, kind, fields })
}

impl Object {
    /// An object from its JSON, as [`Object::to_value`] gives it: `id` and `type` with its fields.
    /// The fields are kept as given; they're checked when a group with the object applies.
    pub fn from_value(v: &Value) -> Result<Object, Refusal> {
        object(Some(v), "/")
    }
}

fn id_list(f: &Fields, name: &str, at: &str) -> Result<Vec<Id>, Refusal> {
    match f.get(name) {
        None => Ok(Vec::new()),
        Some(Value::Arr(a)) => a
            .iter()
            .map(|v| match v {
                Value::Str(s) => Id::new(s),
                _ => Err(bad(at, format!("{name} must be a list of ids"))),
            })
            .collect(),
        Some(_) => Err(bad(at, format!("{name} must be a list of ids"))),
    }
}

impl super::Removed {
    /// As JSON: `{object, after, frame_after}`, what [`super::Verb::PutBack`] takes back.
    pub fn to_value(&self) -> Value {
        let ids =
            |list: &[Id]| Value::Arr(list.iter().map(|i| Value::Str(i.to_string())).collect());
        let mut f = one("object", self.object.to_value());
        f.insert("after".into(), ids(&self.after));
        f.insert("frame_after".into(), ids(&self.frame_after));
        Value::Obj(f)
    }

    /// From its JSON, as [`super::Removed::to_value`] gives it. `after` and `frame_after` may be
    /// left out, as empty; a member it doesn't know is refused.
    pub fn from_value(v: &Value) -> Result<super::Removed, Refusal> {
        let Value::Obj(f) = v else {
            return Err(bad("/", "removed must be an object"));
        };
        only(f, "/", &["object", "after", "frame_after"])?;
        Ok(super::Removed {
            object: object(f.get("object"), "/object")?,
            after: id_list(f, "after", "/after")?,
            frame_after: id_list(f, "frame_after", "/frame_after")?,
        })
    }
}

fn parse_op(v: &Value, at: &str) -> Result<Op, Refusal> {
    let Value::Obj(f) = v else {
        return Err(bad(at, "an op must be an object"));
    };
    let name = str_member(f, "op", at)?;
    match name {
        "add" => {
            only(f, at, &["op", "obj", "at"])?;
            Ok(Op::Add {
                object: object(f.get("obj"), at)?,
                at: position(f.get("at"), at)?,
            })
        }
        "set" | "update" => {
            only(f, at, &["op", "id", "rev", "f", "unset"])?;
            Ok(Op::Set {
                id: id_member(f, "id", at)?,
                rev: rev_member(f, at)?,
                set: obj_member(f, "f", at)?,
                unset: unset_member(f, at)?,
            })
        }
        "del" | "delete" => {
            only(f, at, &["op", "id", "rev"])?;
            Ok(Op::Del {
                id: id_member(f, "id", at)?,
                rev: rev_member(f, at)?,
            })
        }
        "meta" => {
            only(f, at, &["op", "rev", "f", "unset"])?;
            Ok(Op::Meta {
                rev: rev_member(f, at)?,
                set: obj_member(f, "f", at)?,
                unset: unset_member(f, at)?,
            })
        }
        "z" => {
            only(f, at, &["op", "id", "to"])?;
            Ok(Op::Z {
                id: id_member(f, "id", at)?,
                to: position(f.get("to"), at)?,
            })
        }
        "frame-order" => {
            only(f, at, &["op", "id", "to"])?;
            Ok(Op::FrameOrder {
                id: id_member(f, "id", at)?,
                to: position(f.get("to"), at)?,
            })
        }
        "person" => {
            only(f, at, &["op", "token", "person"])?;
            let mut p = obj_member(f, "person", at)?;
            let text = |v: Option<Value>| match v {
                None => Ok(None),
                Some(Value::Str(s)) => Ok(Some(s)),
                Some(_) => Err(bad(at, "a person's name and kind are text")),
            };
            let name = text(p.remove("name"))?;
            let kind = text(p.remove("kind"))?;
            Ok(Op::AddPerson {
                token: id_member(f, "token", at)?,
                person: Person {
                    name,
                    kind,
                    extra: p,
                },
            })
        }
        "clear-blob" => {
            only(f, at, &["op", "hash"])?;
            Ok(Op::ClearBlob {
                hash: Sha256::parse(str_member(f, "hash", at)?)?,
            })
        }
        other => Err(bad(at, format!("no op {other:?}"))),
    }
}

fn parse_ops(top: &Fields) -> Result<Vec<Op>, Refusal> {
    let Some(Value::Arr(ops)) = top.get("ops") else {
        return Err(bad("/ops", "ops must be a list"));
    };
    let mut out = Vec::with_capacity(ops.len());
    for (i, v) in ops.iter().enumerate() {
        out.push(parse_op(v, &format!("/ops/{i}"))?);
    }
    Ok(out)
}

/// The top of a call: an object, or a bare list, which is its `ops`.
fn top_of(bytes: &[u8], limits: &Limits) -> Result<Fields, Refusal> {
    match json::parse(bytes, limits)? {
        Value::Obj(top) => Ok(top),
        Value::Arr(ops) => Ok(one("ops", Value::Arr(ops))),
        _ => Err(bad("/", "a group must be an object, or a list of ops")),
    }
}

/// Raw ops with ids as given: `{"id"?, "by"?, "ops": [...]}`, or the list of ops alone. This is
/// what a canvas sends: it has minted its own ids. `update` is another name for `set`, and
/// `delete` for `del`.
pub fn group_from_json(bytes: &[u8], limits: &Limits) -> Result<Group, Refusal> {
    let top = top_of(bytes, limits)?;
    only(&top, "/", &["id", "by", "ops"])?;
    let id = match top.get("id") {
        None => String::new(),
        Some(Value::Str(s)) => s.clone(),
        Some(_) => return Err(bad("/id", "must be text")),
    };
    let by = match top.get("by") {
        None => None,
        Some(Value::Str(s)) => Some(Id::new(s)?),
        Some(_) => return Err(bad("/by", "must be a token")),
    };
    Ok(Group {
        id,
        by,
        ops: parse_ops(&top)?,
    })
}

fn relabel(v: &mut Value, map: &BTreeMap<String, Id>) {
    if let Value::Str(s) = v {
        if let Some(id) = map.get(s.as_str()) {
            *s = id.to_string();
        }
    }
}

/// Follows labels in the fields that name an object.
fn relabel_fields(f: &mut Fields, map: &BTreeMap<String, Id>) {
    for name in ["parent", "target", "frame"] {
        if let Some(v) = f.get_mut(name) {
            relabel(v, map);
        }
    }
    for name in ["from", "to"] {
        if let Some(Value::Obj(end)) = f.get_mut(name) {
            if let Some(v) = end.get_mut("id") {
                relabel(v, map);
            }
        }
    }
}

fn relabel_position(v: Option<&mut Value>, map: &BTreeMap<String, Id>) {
    if let Some(Value::Obj(f)) = v {
        if let Some(Value::Arr(list)) = f.get_mut("after") {
            list.iter_mut().for_each(|x| relabel(x, map));
        }
    }
}

/// Raw ops where an `add`'s `id` is only a label for this call: `{"ops": [...]}`, or the list
/// alone. Each new object gets a fresh id, and whatever else in the call names it follows:
/// connector ends, parents, targets, frames, other ops and neighbours. Stamped like
/// [`super::Verb::Add`]. [`Planned::labels`] says what became what.
pub fn labelled_from_json(
    board: &crate::model::Board,
    cx: &mut Ctx<'_>,
    bytes: &[u8],
    limits: &Limits,
) -> Result<Planned, VerbError> {
    let mut top = top_of(bytes, limits)?;
    only(&top, "/", &["ops"])?;
    let mut p = Planner::new(board, cx);
    let Some(Value::Arr(ops)) = top.get_mut("ops") else {
        return Err(bad("/ops", "ops must be a list").into());
    };
    let mut labels = BTreeMap::new();
    for (i, op) in ops.iter_mut().enumerate() {
        let at = format!("/ops/{i}");
        let Value::Obj(f) = op else {
            return Err(bad(&at, "an op must be an object").into());
        };
        if f.get("op") != Some(&Value::Str("add".to_string())) {
            continue;
        }
        let Some(Value::Obj(obj)) = f.get_mut("obj") else {
            return Err(bad(&at, "obj must be an object").into());
        };
        let kind = ObjectType::parse(str_member(obj, "type", &at)?)?;
        let fresh = p.mint(super::id_prefix(&kind))?;
        if let Some(Value::Str(label)) = obj.get("id") {
            if labels.insert(label.clone(), fresh.clone()).is_some() {
                return Err(bad(&at, format!("the label {label:?} is used twice")).into());
            }
        }
        obj.insert("id".into(), Value::Str(fresh.to_string()));
        p.ids.push(fresh);
    }
    for op in ops.iter_mut() {
        let Value::Obj(f) = op else { continue };
        for name in ["obj", "f"] {
            if let Some(Value::Obj(inner)) = f.get_mut(name) {
                relabel_fields(inner, &labels);
            }
        }
        if let Some(v) = f.get_mut("id") {
            relabel(v, &labels);
        }
        relabel_position(f.get_mut("at"), &labels);
        relabel_position(f.get_mut("to"), &labels);
    }
    let mut parsed = parse_ops(&top)?;
    for op in &mut parsed {
        if let Op::Add { object, .. } = op {
            p.stamp(&mut object.fields);
        }
    }
    p.ops = parsed;
    p.labels = labels;
    Ok(p.finish())
}

/// What a command line's `--<field> <text>` pairs come to, for one type: fields to set, and fields
/// to remove.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldEdits {
    /// Fields to set, each value read by [`field_from_text`].
    pub set: Fields,
    /// Fields to remove: those given the text `null`.
    pub unset: Vec<String>,
}

/// A command line's `--<field> <text>` pairs for an object of this type, as [`super::Verb::Update`]
/// takes them. The text `null` removes a field. A type or a field name the format doesn't know is
/// refused, and so is a name given twice.
pub fn flags(
    type_name: &str,
    pairs: &[(&str, &str)],
    limits: &Limits,
) -> Result<FieldEdits, VerbError> {
    let kind = ObjectType::parse(type_name)?;
    super::known_fields(&kind, pairs.iter().map(|(name, _)| *name))?;
    let mut out = FieldEdits::default();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (name, text) in pairs {
        if !seen.insert(*name) {
            return Err(super::refuse(
                VerbCode::BadValue,
                format!("{name:?} is given twice"),
            ));
        }
        if *text == "null" {
            out.unset.push(name.to_string());
        } else {
            let value = field_from_text(type_name, name, text, limits)?;
            out.set.insert(name.to_string(), value);
        }
    }
    Ok(out)
}

/// A command line's `--<setting> <text>` pairs, as [`super::Verb::Meta`] takes them: the settings'
/// counterpart of [`flags`]. Each name is read by the setting's kind ([`schema::SETTINGS`]), never
/// an object field's; the text `null` removes it. A name the settings don't hold is
/// `unknown_field`, with the facts `type_name` `settings` and `field`; a name given twice is
/// `bad_value`.
pub fn settings_flags(pairs: &[(&str, &str)], limits: &Limits) -> Result<FieldEdits, VerbError> {
    settings_flags_in(schema::SETTINGS, pairs, limits)
}

fn settings_flags_in(
    table: &[schema::Field],
    pairs: &[(&str, &str)],
    limits: &Limits,
) -> Result<FieldEdits, VerbError> {
    let known = |name: &str| table.iter().find(|f| f.name == name);
    for (name, _) in pairs {
        if known(name).is_none() {
            return Err(refuse_with(
                VerbCode::UnknownField,
                format!("the settings have no field {name:?}"),
                Facts::type_field("settings", Some(name)),
            ));
        }
    }
    let mut out = FieldEdits::default();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (name, text) in pairs {
        if !seen.insert(*name) {
            return Err(super::refuse(
                VerbCode::BadValue,
                format!("{name:?} is given twice"),
            ));
        }
        if *text == "null" {
            out.unset.push(name.to_string());
        } else {
            let value = by_kind(known(name), name, text, limits)?;
            out.set.insert(name.to_string(), value);
        }
    }
    Ok(out)
}

/// A field's value from text, as a command line gives it, by the kind the schema says. Text stays
/// text; a number takes the format's number rule, so nothing is rounded; the rest is JSON. A field
/// the format doesn't know is JSON if it starts like an object or a list, and text if not.
pub fn field_from_text(
    type_name: &str,
    field: &str,
    text: &str,
    limits: &Limits,
) -> Result<Value, Refusal> {
    let known = schema::field_of(type_name, field)
        .or_else(|| schema::SETTINGS.iter().find(|f| f.name == field));
    by_kind(known, field, text, limits)
}

/// A value from text by a field's kind; a field the format doesn't know is JSON if it starts like
/// an object or a list, and text if not.
fn by_kind(
    known: Option<&schema::Field>,
    field: &str,
    text: &str,
    limits: &Limits,
) -> Result<Value, Refusal> {
    let Some(known) = known else {
        return if text.starts_with(['{', '[']) {
            json::parse(text.as_bytes(), limits)
        } else {
            Ok(Value::Str(text.to_string()))
        };
    };
    match known.kind {
        Kind::Num => match json::parse(text.as_bytes(), limits)? {
            v @ Value::Num(_) => Ok(v),
            _ => Err(bad(field, "must be a number")),
        },
        Kind::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(bad(field, "must be true or false")),
        },
        Kind::Str(_)
        | Kind::Open(_)
        | Kind::Color(_)
        | Kind::Link
        | Kind::ObjectRef
        | Kind::GroupId => Ok(Value::Str(text.to_string())),
        _ => json::parse(text.as_bytes(), limits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A setting that shares its name with an object's common field is read by the setting's kind:
    /// here `x`, a number on every object, as a text setting.
    #[test]
    fn a_setting_is_never_read_by_an_object_fields_kind() {
        let table = [schema::Field {
            name: "x",
            kind: Kind::Str(10),
            default: None,
        }];
        let got = settings_flags_in(&table, &[("x", "5")], &Limits::V1)
            .expect("check: x is a setting here");
        assert_eq!(
            got.set.get("x"),
            Some(&Value::Str("5".into())),
            "the setting's kind"
        );
        assert_eq!(
            field_from_text("sticky", "x", "5", &Limits::V1),
            Ok(Value::Num(5.0)),
            "an object's x is a number"
        );
    }
}
