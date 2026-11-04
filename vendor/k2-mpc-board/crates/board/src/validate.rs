//! The board checks, driven by the schema tables. Refuse, typed; never cut. A field this
//! version doesn't know is kept; it only has to be a legal name.

use crate::hash::Sha256;
use crate::json::{self, Fields, Value};
use crate::limits::Limits;
use crate::model::{Board, Id, Object, ObjectType, Person};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema::{self, Field, Kind};
use std::collections::{BTreeMap, BTreeSet};

/// A name: 1 to 40 bytes of `[a-z0-9-]`.
pub(crate) fn is_name(s: &str) -> bool {
    (1..=schema::NAME_MAX_BYTES).contains(&s.len())
        && s.bytes()
            .all(|c| matches!(c, b'a'..=b'z' | b'0'..=b'9' | b'-'))
}

fn is_color(s: &str) -> bool {
    if let Some(hex) = s.strip_prefix('#') {
        return hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit());
    }
    is_name(s)
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn kind_err(at: &str, want: &str) -> Refusal {
    Refusal::at(RefusalCode::FieldKind, at, format!("must be {want}"))
}

fn value_err(at: &str, what: impl Into<String>) -> Refusal {
    Refusal::at(RefusalCode::FieldValue, at, what)
}

fn limit_err(at: &str, what: impl Into<String>) -> Refusal {
    Refusal::at(RefusalCode::LimitField, at, what)
}

fn id_at(s: &str, at: &str) -> Result<Id, Refusal> {
    Id::new(s).map_err(|mut r| {
        r.at = Some(at.to_string());
        r
    })
}

fn check_field_name(name: &str, at: &str) -> Result<(), Refusal> {
    let ok = (1..=schema::FIELD_NAME_MAX_BYTES).contains(&name.len())
        && !name.chars().any(|c| (c as u32) < 0x20);
    if ok {
        Ok(())
    } else {
        Err(Refusal::at(
            RefusalCode::FieldNameInvalid,
            at,
            format!(
                "a field name is 1 to {} bytes with no control characters",
                schema::FIELD_NAME_MAX_BYTES
            ),
        ))
    }
}

/// Numbers and nesting anywhere in a value, unknown fields too. Parsing checks these already; this
/// also covers a board that was edited in memory.
fn check_value(v: &Value, at: &str, depth: u64) -> Result<(), Refusal> {
    match v {
        Value::Num(n) => {
            json::number_to_string(*n).map_err(|mut r| {
                r.at = Some(at.to_string());
                r
            })?;
        }
        Value::Arr(a) => {
            check_depth(depth, at)?;
            for x in a {
                check_value(x, at, depth + 1)?;
            }
        }
        Value::Obj(f) => {
            check_depth(depth, at)?;
            for x in f.values() {
                check_value(x, at, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_depth(depth: u64, at: &str) -> Result<(), Refusal> {
    if depth > Limits::V1.depth {
        return Err(Refusal::at(
            RefusalCode::LimitDepth,
            at,
            format!("nested deeper than {}", Limits::V1.depth),
        ));
    }
    Ok(())
}

struct Ctx<'a> {
    people: &'a BTreeMap<Id, Person>,
}

impl Ctx<'_> {
    fn person(&self, s: &str, at: &str) -> Result<(), Refusal> {
        let id = id_at(s, at)?;
        if self.people.contains_key(&id) {
            Ok(())
        } else {
            Err(Refusal::at(
                RefusalCode::PersonUnknown,
                at,
                format!("{s} is not in people"),
            ))
        }
    }
}

fn as_num(v: &Value, at: &str) -> Result<f64, Refusal> {
    match v {
        Value::Num(n) => Ok(*n),
        _ => Err(kind_err(at, "a number")),
    }
}

fn as_str<'v>(v: &'v Value, at: &str) -> Result<&'v str, Refusal> {
    match v {
        Value::Str(s) => Ok(s),
        _ => Err(kind_err(at, "text")),
    }
}

fn as_obj<'v>(v: &'v Value, at: &str, want: &str) -> Result<&'v Fields, Refusal> {
    match v {
        Value::Obj(f) => Ok(f),
        _ => Err(kind_err(at, want)),
    }
}

fn as_arr<'v>(v: &'v Value, at: &str, want: &str, max: usize) -> Result<&'v [Value], Refusal> {
    match v {
        Value::Arr(a) => {
            if a.len() > max {
                return Err(limit_err(at, format!("{} items, above {max}", a.len())));
            }
            Ok(a)
        }
        _ => Err(kind_err(at, want)),
    }
}

fn required<'v>(f: &'v Fields, key: &str, at: &str) -> Result<&'v Value, Refusal> {
    f.get(key)
        .ok_or_else(|| Refusal::at(RefusalCode::FieldKind, format!("{at}/{key}"), "required"))
}

fn sub_id(item: &Fields, at: &str, seen: &mut BTreeSet<Id>) -> Result<Id, Refusal> {
    let key_at = format!("{at}/id");
    let id = id_at(as_str(required(item, "id", at)?, &key_at)?, &key_at)?;
    if !seen.insert(id.clone()) {
        return Err(Refusal::at(
            RefusalCode::IdDuplicate,
            key_at,
            format!("{id} appears twice in this list"),
        ));
    }
    Ok(id)
}

fn text_max(v: &Value, at: &str, max: usize) -> Result<(), Refusal> {
    let s = as_str(v, at)?;
    if chars(s) > max {
        return Err(limit_err(at, format!("over {max} code points")));
    }
    Ok(())
}

fn point(f: &Fields, at: &str) -> Result<(), Refusal> {
    as_num(required(f, "x", at)?, &format!("{at}/x"))?;
    as_num(required(f, "y", at)?, &format!("{at}/y"))?;
    Ok(())
}

fn check_timestamp(s: &str, at: &str) -> Result<(), Refusal> {
    // RFC 3339: YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM)
    let bad = || value_err(at, "must be an RFC 3339 timestamp");
    let b = s.as_bytes();
    if b.len() > schema::TIMESTAMP_MAX_BYTES || b.len() < 20 {
        return Err(bad());
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let shape = digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b'T'
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19);
    if !shape {
        return Err(bad());
    }
    let mut i = 19;
    if b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return Err(bad());
        }
    }
    let zone = &b[i..];
    let zone_ok = zone == b"Z"
        || (zone.len() == 6
            && (zone[0] == b'+' || zone[0] == b'-')
            && zone[1].is_ascii_digit()
            && zone[2].is_ascii_digit()
            && zone[3] == b':'
            && zone[4].is_ascii_digit()
            && zone[5].is_ascii_digit());
    if !zone_ok {
        return Err(bad());
    }
    let two = |at: usize| u32::from(b[at] - b'0') * 10 + u32::from(b[at + 1] - b'0');
    let in_range = (1..=12).contains(&two(5))
        && (1..=31).contains(&two(8))
        && two(11) <= 23
        && two(14) <= 59
        && two(17) <= 60;
    if !in_range {
        return Err(bad());
    }
    Ok(())
}

fn check_end(v: &Value, at: &str) -> Result<(), Refusal> {
    let f = as_obj(v, at, "a connector end")?;
    match (f.get("id"), f.contains_key("x") || f.contains_key("y")) {
        (Some(id), false) => {
            let key_at = format!("{at}/id");
            id_at(as_str(id, &key_at)?, &key_at)?;
            if let Some(a) = f.get("anchor") {
                let a_at = format!("{at}/anchor");
                if !is_name(as_str(a, &a_at)?) {
                    return Err(value_err(&a_at, "must be a name"));
                }
            }
            if let Some(h) = f.get("hint") {
                let h_at = format!("{at}/hint");
                point(as_obj(h, &h_at, "a point")?, &h_at)?;
            }
            Ok(())
        }
        (None, true) => point(f, at),
        _ => Err(value_err(at, "must be {id, anchor?, hint?} or {x, y}")),
    }
}

fn check_kind(kind: &Kind, v: &Value, at: &str, ctx: &Ctx<'_>) -> Result<(), Refusal> {
    match kind {
        Kind::Num => {
            as_num(v, at)?;
        }
        Kind::Bool => {
            if !matches!(v, Value::Bool(_)) {
                return Err(kind_err(at, "true or false"));
            }
        }
        Kind::Str(max) => text_max(v, at, *max as usize)?,
        Kind::Open(_) => {
            if !is_name(as_str(v, at)?) {
                return Err(value_err(at, "must be a name of [a-z0-9-], 1 to 40 bytes"));
            }
        }
        Kind::Color(_) => {
            if !is_color(as_str(v, at)?) {
                return Err(value_err(at, "must be a colour name or #rrggbb"));
            }
        }
        Kind::ObjectRef | Kind::GroupId => {
            id_at(as_str(v, at)?, at)?;
        }
        Kind::Tags => {
            for (i, t) in as_arr(v, at, "a list of tags", schema::TAGS_MAX)?
                .iter()
                .enumerate()
            {
                text_max(t, &format!("{at}/{i}"), schema::TAG_MAX_CHARS)?;
            }
        }
        Kind::Points => {
            for (i, p) in as_arr(v, at, "a list of points", schema::POINTS_MAX)?
                .iter()
                .enumerate()
            {
                let p_at = format!("{at}/{i}");
                let Value::Arr(xy) = p else {
                    return Err(kind_err(&p_at, "[x, y]"));
                };
                if xy.len() != 2 {
                    return Err(value_err(&p_at, "must be exactly [x, y]"));
                }
                as_num(&xy[0], &p_at)?;
                as_num(&xy[1], &p_at)?;
            }
        }
        Kind::End => check_end(v, at)?,
        Kind::Checks => {
            let mut seen = BTreeSet::new();
            for (i, c) in as_arr(v, at, "a list of check items", schema::CHECKS_MAX)?
                .iter()
                .enumerate()
            {
                let c_at = format!("{at}/{i}");
                let f = as_obj(c, &c_at, "a check item")?;
                sub_id(f, &c_at, &mut seen)?;
                text_max(
                    required(f, "text", &c_at)?,
                    &format!("{c_at}/text"),
                    schema::CHECK_TEXT_MAX_CHARS,
                )?;
                if !matches!(required(f, "done", &c_at)?, Value::Bool(_)) {
                    return Err(kind_err(&format!("{c_at}/done"), "true or false"));
                }
            }
        }
        Kind::Columns => {
            let mut seen = BTreeSet::new();
            for (i, c) in as_arr(v, at, "a list of columns", schema::COLUMNS_MAX)?
                .iter()
                .enumerate()
            {
                let c_at = format!("{at}/{i}");
                let f = as_obj(c, &c_at, "a column")?;
                sub_id(f, &c_at, &mut seen)?;
                if let Some(w) = f.get("width") {
                    let w_at = format!("{c_at}/width");
                    if as_num(w, &w_at)? < 0.0 {
                        return Err(value_err(&w_at, "must not be negative"));
                    }
                }
            }
        }
        Kind::Rows => {
            let mut seen = BTreeSet::new();
            for (i, r) in as_arr(v, at, "a list of rows", schema::ROWS_MAX)?
                .iter()
                .enumerate()
            {
                let r_at = format!("{at}/{i}");
                let f = as_obj(r, &r_at, "a row")?;
                sub_id(f, &r_at, &mut seen)?;
                if let Some(cells) = f.get("cells") {
                    let c_at = format!("{r_at}/cells");
                    for (col, cell) in as_obj(cells, &c_at, "cells by column id")? {
                        let cell_at = format!("{c_at}/{col}");
                        id_at(col, &cell_at)?;
                        text_max(cell, &cell_at, schema::CELL_MAX_CHARS)?;
                    }
                }
            }
        }
        Kind::Reactions => {
            let f = as_obj(v, at, "reactions")?;
            if f.len() > schema::REACTIONS_MAX {
                return Err(limit_err(
                    at,
                    format!("over {} reactions", schema::REACTIONS_MAX),
                ));
            }
            for (key, tokens) in f {
                let k_at = format!("{at}/{key}");
                let key_ok = (1..=schema::REACTION_KEY_MAX_BYTES).contains(&key.len())
                    && !key.chars().any(|c| (c as u32) < 0x20);
                if !key_ok {
                    return Err(value_err(&k_at, "a reaction key is 1 to 32 bytes of text"));
                }
                let list = as_arr(
                    tokens,
                    &k_at,
                    "a list of person tokens",
                    schema::REACTION_PEOPLE_MAX,
                )?;
                let mut last: Option<&str> = None;
                for t in list {
                    let t = as_str(t, &k_at)?;
                    ctx.person(t, &k_at)?;
                    if last.is_some_and(|l| l >= t) {
                        return Err(value_err(&k_at, "tokens must be sorted and unique"));
                    }
                    last = Some(t);
                }
            }
        }
        Kind::Votes => {
            let f = as_obj(v, at, "votes")?;
            if f.len() > schema::VOTERS_MAX {
                return Err(limit_err(at, format!("over {} voters", schema::VOTERS_MAX)));
            }
            for (token, n) in f {
                let t_at = format!("{at}/{token}");
                ctx.person(token, &t_at)?;
                let n = as_num(n, &t_at)?;
                if n.fract() != 0.0 || !(1.0..=schema::VOTES_MAX).contains(&n) {
                    return Err(value_err(&t_at, "votes are a whole number from 1"));
                }
            }
        }
        Kind::Replies => {
            let mut seen = BTreeSet::new();
            for (i, r) in as_arr(v, at, "a list of replies", schema::REPLIES_MAX)?
                .iter()
                .enumerate()
            {
                let r_at = format!("{at}/{i}");
                let f = as_obj(r, &r_at, "a reply")?;
                sub_id(f, &r_at, &mut seen)?;
                let by_at = format!("{r_at}/by");
                ctx.person(as_str(required(f, "by", &r_at)?, &by_at)?, &by_at)?;
                let at_at = format!("{r_at}/at");
                check_timestamp(as_str(required(f, "at", &r_at)?, &at_at)?, &at_at)?;
                text_max(
                    required(f, "text", &r_at)?,
                    &format!("{r_at}/text"),
                    schema::REPLY_TEXT_MAX_CHARS,
                )?;
            }
        }
        Kind::Crop => {
            let f = as_obj(v, at, "{x, y, w, h}")?;
            let get = |k: &str| as_num(required(f, k, at)?, &format!("{at}/{k}"));
            let (x, y, w, h) = (get("x")?, get("y")?, get("w")?, get("h")?);
            let in_unit = |n: f64| (0.0..=1.0).contains(&n);
            if !(in_unit(x) && in_unit(y) && in_unit(w) && in_unit(h))
                || w < schema::CROP_MIN
                || h < schema::CROP_MIN
                || x + w > schema::CROP_EDGE
                || y + h > schema::CROP_EDGE
            {
                return Err(value_err(at, "must be a crop inside the image"));
            }
        }
        Kind::Link => {
            let s = as_str(v, at)?;
            if chars(s) > schema::LINK_MAX_CHARS {
                return Err(limit_err(
                    at,
                    format!("over {} code points", schema::LINK_MAX_CHARS),
                ));
            }
            let lower = s.get(..8).unwrap_or(s).to_ascii_lowercase();
            let rest = if lower.starts_with("https://") {
                &s[8..]
            } else if lower.starts_with("http://") {
                &s[7..]
            } else {
                return Err(value_err(at, "must be an http or https address"));
            };
            if rest.is_empty() || rest.chars().any(|c| c.is_whitespace() || c == '\u{FEFF}') {
                return Err(value_err(at, "must be an http or https address"));
            }
        }
        Kind::Blob => {
            let f = as_obj(v, at, "{\"blob\": \"sha256-<hex>\"}")?;
            if f.len() != 1 {
                return Err(value_err(at, "a blob reference has only the member blob"));
            }
            let b_at = format!("{at}/blob");
            Sha256::parse(as_str(required(f, "blob", at)?, &b_at)?).map_err(|mut r| {
                r.at = Some(b_at.clone());
                r
            })?;
        }
        Kind::Created => {
            let f = as_obj(v, at, "{by, at}")?;
            let by_at = format!("{at}/by");
            ctx.person(as_str(required(f, "by", at)?, &by_at)?, &by_at)?;
            let at_at = format!("{at}/at");
            check_timestamp(as_str(required(f, "at", at)?, &at_at)?, &at_at)?;
        }
        Kind::Voting => {
            let f = as_obj(v, at, "{active, perPerson?}")?;
            if !matches!(required(f, "active", at)?, Value::Bool(_)) {
                return Err(kind_err(&format!("{at}/active"), "true or false"));
            }
            if let Some(p) = f.get("perPerson") {
                let p_at = format!("{at}/perPerson");
                let n = as_num(p, &p_at)?;
                if n.fract() != 0.0 || n < 1.0 {
                    return Err(value_err(&p_at, "must be a whole number from 1"));
                }
            }
        }
    }
    Ok(())
}

fn check_fields(
    fields: &Fields,
    at: &str,
    ctx: &Ctx<'_>,
    lookup: impl Fn(&str) -> Option<&'static Field>,
) -> Result<(), Refusal> {
    for (name, v) in fields {
        let f_at = format!("{at}/{name}");
        check_field_name(name, &f_at)?;
        check_value(v, &f_at, 2)?;
        if let Some(spec) = lookup(name) {
            check_kind(&spec.kind, v, &f_at, ctx)?;
        }
    }
    Ok(())
}

fn check_object(o: &Object, at: &str, ctx: &Ctx<'_>) -> Result<(), Refusal> {
    let type_name = o.kind.as_str();
    check_fields(&o.fields, at, ctx, |name| schema::field_of(type_name, name))?;
    // A cell names a column of its own table.
    if o.kind == ObjectType::Table {
        let mut columns = BTreeSet::new();
        if let Some(Value::Arr(cols)) = o.fields.get("columns") {
            for c in cols {
                if let Value::Obj(f) = c {
                    if let Some(Value::Str(id)) = f.get("id") {
                        columns.insert(id.as_str());
                    }
                }
            }
        }
        if let Some(Value::Arr(rows)) = o.fields.get("rows") {
            for (i, r) in rows.iter().enumerate() {
                if let Value::Obj(f) = r {
                    if let Some(Value::Obj(cells)) = f.get("cells") {
                        for col in cells.keys() {
                            if !columns.contains(col.as_str()) {
                                return Err(Refusal::at(
                                    RefusalCode::CellColumnUnknown,
                                    format!("{at}/rows/{i}/cells/{col}"),
                                    "no such column in this table",
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// One person's fields.
pub(crate) fn check_person(token: &Id, p: &Person) -> Result<(), Refusal> {
    let at = format!("/people/{token}");
    if let Some(name) = &p.name {
        if chars(name) > schema::PERSON_NAME_MAX_CHARS {
            return Err(limit_err(&format!("{at}/name"), "name too long"));
        }
    }
    if let Some(kind) = &p.kind {
        if !is_name(kind) {
            return Err(value_err(&format!("{at}/kind"), "must be a name"));
        }
    }
    for (k, v) in &p.extra {
        check_value(v, &format!("{at}/{k}"), 3)?;
    }
    Ok(())
}

/// One object's type and fields, against these people.
pub(crate) fn check_one_object(
    o: &Object,
    at: &str,
    people: &BTreeMap<Id, Person>,
) -> Result<(), Refusal> {
    if let ObjectType::Unknown(t) = &o.kind {
        if !is_name(t) {
            return Err(Refusal::at(
                RefusalCode::TypeInvalid,
                format!("{at}/type"),
                "not a type name",
            ));
        }
    }
    check_object(o, at, &Ctx { people })
}

/// The settings, against these people.
pub(crate) fn check_settings(
    settings: &Fields,
    people: &BTreeMap<Id, Person>,
) -> Result<(), Refusal> {
    check_fields(settings, "/settings", &Ctx { people }, |name| {
        schema::SETTINGS.iter().find(|f| f.name == name)
    })
}

pub(crate) fn validate_board(b: &Board) -> Result<(), Refusal> {
    for (name, v) in &b.extra {
        check_value(v, &format!("/{name}"), 2)?;
    }
    check_settings(&b.settings, &b.people)?;
    for (token, p) in &b.people {
        check_person(token, p)?;
    }

    let mut ids = BTreeSet::new();
    for (i, o) in b.objects.iter().enumerate() {
        let at = format!("/objects/{i}");
        if !ids.insert(&o.id) {
            return Err(Refusal::at(
                RefusalCode::IdDuplicate,
                format!("{at}/id"),
                format!("{} appears twice", o.id),
            ));
        }
        check_one_object(o, &at, &b.people)?;
    }

    // Every frame object is listed once, and everything listed is a frame object.
    let frame_objects: BTreeSet<&Id> = b
        .objects
        .iter()
        .filter(|o| o.kind == ObjectType::Frame)
        .map(|o| &o.id)
        .collect();
    let listed: BTreeSet<&Id> = b.frames.iter().collect();
    if listed.len() != b.frames.len() || listed != frame_objects {
        return Err(Refusal::at(
            RefusalCode::FramesMismatch,
            "/frames",
            "frames must list every frame object once, and nothing else",
        ));
    }
    Ok(())
}
