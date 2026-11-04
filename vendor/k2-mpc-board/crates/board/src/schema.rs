// Portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT; see THIRD-PARTY-NOTICES.md.
//! The field tables for format v1. The validator, `schema/board-schema.json` and the field tables
//! in `docs/FORMAT.md` all come from these, so they can't drift apart.

use crate::json::{self, Fields, Value};
use crate::limits::Limits;
use crate::refusal::RefusalCode;
use crate::{FORMAT, MEDIA_TYPE, MIN_READER, VERSION};

/// What a field holds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A number.
    Num,
    /// `true` or `false`.
    Bool,
    /// Text, at most this many code points.
    Str(u32),
    /// A name: these are known, and any name of `[a-z0-9-]`, 1 to 40 bytes, is kept.
    Open(&'static [&'static str]),
    /// A colour: a name as for `Open`, or `#rrggbb`.
    Color(&'static [&'static str]),
    /// An object's id. A target that isn't there is a state, not an error.
    ObjectRef,
    /// A group's id. Objects with the same one are grouped; a group is not an object.
    GroupId,
    /// A list of short texts.
    Tags,
    /// A list of `[x, y]` points.
    Points,
    /// `{id, anchor?, hint?}` or `{x, y}`.
    End,
    /// A list of `{id, text, done}`.
    Checks,
    /// A list of `{id, width?}`.
    Columns,
    /// A list of `{id, cells?}`.
    Rows,
    /// `{<reaction>: [person token, ...]}`, sorted and unique.
    Reactions,
    /// `{<person token>: <votes>}`.
    Votes,
    /// A list of `{id, by, at, text}`.
    Replies,
    /// `{x, y, w, h}`, fractions of the image.
    Crop,
    /// An http or https address.
    Link,
    /// `{"blob": "sha256-<hex>"}`.
    Blob,
    /// `{by, at}`.
    Created,
    /// `{active, perPerson?}`.
    Voting,
}

/// A field's value when it is absent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Default {
    /// A number.
    Num(f64),
    /// A boolean.
    Bool(bool),
    /// A text or a name.
    Str(&'static str),
}

/// One field of an object type, or of the settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    /// Its name.
    pub name: &'static str,
    /// What it holds.
    pub kind: Kind,
    /// Its value when absent; `None` means absent is simply none.
    pub default: Option<Default>,
}

const fn f(name: &'static str, kind: Kind, default: Option<Default>) -> Field {
    Field {
        name,
        kind,
        default,
    }
}
const fn num(n: f64) -> Option<Default> {
    Some(Default::Num(n))
}
const fn text(s: &'static str) -> Option<Default> {
    Some(Default::Str(s))
}
const fn off() -> Option<Default> {
    Some(Default::Bool(false))
}

/// The most ids a refusal lists.
pub const BAD_IDS_MAX: usize = 1000;
/// How much of one bad id a refusal lists, in code points. Longer ones end with an ellipsis.
pub const BAD_ID_MAX_CHARS: usize = 64;
/// The grammar every id follows: 1 to this many bytes.
pub const ID_MAX_BYTES: usize = 24;
/// The longest object or settings field name, in bytes.
pub const FIELD_NAME_MAX_BYTES: usize = 200;
/// The longest open name or object type name, in bytes.
pub const NAME_MAX_BYTES: usize = 40;
/// Tags on one object.
pub const TAGS_MAX: usize = 200;
/// One tag, in code points.
pub const TAG_MAX_CHARS: usize = 80;
/// Points in one stroke.
pub const POINTS_MAX: usize = 5000;
/// Check items in one list.
pub const CHECKS_MAX: usize = 50;
/// One check item's text, in code points.
pub const CHECK_TEXT_MAX_CHARS: usize = 500;
/// Columns in one table.
pub const COLUMNS_MAX: usize = 30;
/// Rows in one table.
pub const ROWS_MAX: usize = 100;
/// One table cell, in code points.
pub const CELL_MAX_CHARS: usize = 2000;
/// Replies on one comment.
pub const REPLIES_MAX: usize = 1000;
/// One reply's text, in code points.
pub const REPLY_TEXT_MAX_CHARS: usize = 5000;
/// A reaction's key, in bytes.
pub const REACTION_KEY_MAX_BYTES: usize = 32;
/// Different reactions on one object.
pub const REACTIONS_MAX: usize = 64;
/// People under one reaction.
pub const REACTION_PEOPLE_MAX: usize = 10_000;
/// People with votes on one object.
pub const VOTERS_MAX: usize = 10_000;
/// The most votes one person may put on one object.
pub const VOTES_MAX: f64 = 1_000_000.0;
/// A link, in code points.
pub const LINK_MAX_CHARS: usize = 2000;
/// A timestamp, in bytes.
pub const TIMESTAMP_MAX_BYTES: usize = 40;
/// Known values of a person's `kind`. Others are kept.
pub const PERSON_KINDS: &[&str] = &["person", "agent"];
/// A person's display name, in code points.
pub const PERSON_NAME_MAX_CHARS: usize = 80;
/// The smallest crop width or height.
pub const CROP_MIN: f64 = 0.01;
/// How far a crop may pass the image's edge (a rounding margin in existing data).
pub const CROP_EDGE: f64 = 1.0001;

/// Sticky note colours.
pub const STICKY: &[&str] = &[
    "yellow", "orange", "pink", "red", "violet", "blue", "cyan", "green", "lime", "gray", "white",
];
/// Ink colours.
pub const INK: &[&str] = &[
    "ink", "gray", "red", "orange", "yellow", "green", "teal", "blue", "violet", "pink", "white",
];
/// Fills. `none` is transparent.
pub const FILL: &[&str] = &[
    "none", "white", "gray", "red", "orange", "yellow", "green", "teal", "blue", "violet", "pink",
    "ink",
];
/// Shapes.
pub const SHAPES: &[&str] = &[
    "rect",
    "round",
    "ellipse",
    "triangle",
    "diamond",
    "parallelogram",
    "trapezoid",
    "pentagon",
    "hexagon",
    "octagon",
    "star",
    "cross",
    "arrow-right",
    "arrow-left",
    "chevron",
    "cylinder",
    "cloud",
    "speech",
    "document",
    "flow-start",
    "flow-process",
    "flow-decision",
    "flow-data",
    "flow-predefined",
];
/// Line ends.
pub const CAPS: &[&str] = &["none", "arrow", "triangle", "circle", "diamond", "bar"];
/// Connector routes.
pub const ROUTES: &[&str] = &["straight", "elbow", "curved"];
/// Line styles.
pub const DASH: &[&str] = &["solid", "dashed", "dotted"];
/// Where a connector meets an object.
pub const ANCHORS: &[&str] = &["auto", "top", "right", "bottom", "left"];
/// Text alignment.
pub const ALIGN: &[&str] = &["left", "center", "right"];
/// Reactions a reader should know how to draw. Others are kept.
pub const REACTIONS: &[&str] = &[
    "\u{1F44D}",
    "\u{2764}\u{FE0F}",
    "\u{1F602}",
    "\u{1F389}",
    "\u{1F914}",
    "\u{1F440}",
    "\u{1F525}",
    "\u{2705}",
    "\u{274C}",
    "\u{2753}",
    "\u{2B50}",
];

/// Fields on every object, of any type.
pub static COMMON: &[Field] = &[
    f("x", Kind::Num, num(0.0)),
    f("y", Kind::Num, num(0.0)),
    f("rot", Kind::Num, num(0.0)),
    f("locked", Kind::Bool, off()),
    f("group", Kind::GroupId, None),
    f("frame", Kind::ObjectRef, None),
    f("tags", Kind::Tags, None),
    f("reactions", Kind::Reactions, None),
    f("votes", Kind::Votes, None),
    f("link", Kind::Link, None),
    f("created", Kind::Created, None),
];

/// The object types of format v1 and their own fields.
pub static TYPES: &[(&str, &[Field])] = &[
    (
        "sticky",
        &[
            f("w", Kind::Num, num(200.0)),
            f("h", Kind::Num, num(200.0)),
            f("text", Kind::Str(5000), text("")),
            f("color", Kind::Color(STICKY), text("yellow")),
            f("fontSize", Kind::Num, num(0.0)),
            f("align", Kind::Open(ALIGN), text("center")),
        ],
    ),
    (
        "text",
        &[
            f("w", Kind::Num, num(240.0)),
            f("h", Kind::Num, num(40.0)),
            f("text", Kind::Str(20000), text("")),
            f("color", Kind::Color(INK), text("ink")),
            f("fontSize", Kind::Num, num(24.0)),
            f("bold", Kind::Bool, off()),
            f("italic", Kind::Bool, off()),
            f("align", Kind::Open(ALIGN), text("left")),
            f("checks", Kind::Checks, None),
        ],
    ),
    (
        "shape",
        &[
            f("w", Kind::Num, num(180.0)),
            f("h", Kind::Num, num(120.0)),
            f("shape", Kind::Open(SHAPES), text("round")),
            f("text", Kind::Str(5000), text("")),
            f("fill", Kind::Color(FILL), text("white")),
            f("stroke", Kind::Color(INK), text("ink")),
            f("strokeWidth", Kind::Num, num(2.0)),
            f("dash", Kind::Open(DASH), text("solid")),
            f("color", Kind::Color(INK), text("ink")),
            f("fontSize", Kind::Num, num(18.0)),
        ],
    ),
    (
        "connector",
        &[
            f("w", Kind::Num, num(0.0)),
            f("h", Kind::Num, num(0.0)),
            f("from", Kind::End, None),
            f("to", Kind::End, None),
            f("route", Kind::Open(ROUTES), text("elbow")),
            f("stroke", Kind::Color(INK), text("ink")),
            f("strokeWidth", Kind::Num, num(2.0)),
            f("dash", Kind::Open(DASH), text("solid")),
            f("startCap", Kind::Open(CAPS), text("none")),
            f("endCap", Kind::Open(CAPS), text("arrow")),
            f("label", Kind::Str(500), text("")),
        ],
    ),
    (
        "pen",
        &[
            f("points", Kind::Points, None),
            f("stroke", Kind::Color(INK), text("ink")),
            f("strokeWidth", Kind::Num, num(3.0)),
            f("highlighter", Kind::Bool, off()),
        ],
    ),
    (
        "frame",
        &[
            f("w", Kind::Num, num(960.0)),
            f("h", Kind::Num, num(600.0)),
            f("title", Kind::Str(200), text("")),
            f("fill", Kind::Color(FILL), text("white")),
        ],
    ),
    (
        "card",
        &[
            f("w", Kind::Num, num(280.0)),
            f("h", Kind::Num, num(150.0)),
            f("title", Kind::Str(500), text("")),
            f("text", Kind::Str(5000), text("")),
            f("color", Kind::Color(INK), text("blue")),
            f("assignee", Kind::Str(80), text("")),
            f("due", Kind::Str(40), text("")),
            f("status", Kind::Str(40), text("")),
            f("checks", Kind::Checks, None),
        ],
    ),
    (
        "table",
        &[
            f("w", Kind::Num, num(480.0)),
            f("h", Kind::Num, num(160.0)),
            f("columns", Kind::Columns, None),
            f("rows", Kind::Rows, None),
            f("fill", Kind::Color(FILL), text("white")),
        ],
    ),
    (
        "mind",
        &[
            f("w", Kind::Num, num(180.0)),
            f("h", Kind::Num, num(48.0)),
            f("text", Kind::Str(1000), text("")),
            f("parent", Kind::ObjectRef, None),
            f("color", Kind::Color(INK), text("violet")),
        ],
    ),
    (
        "comment",
        &[
            f("w", Kind::Num, num(32.0)),
            f("h", Kind::Num, num(32.0)),
            f("text", Kind::Str(5000), text("")),
            f("replies", Kind::Replies, None),
            f("resolved", Kind::Bool, off()),
            f("target", Kind::ObjectRef, None),
        ],
    ),
    (
        "image",
        &[
            f("w", Kind::Num, num(320.0)),
            f("h", Kind::Num, num(240.0)),
            f("src", Kind::Blob, None),
            f("alt", Kind::Str(300), text("")),
            f("crop", Kind::Crop, None),
        ],
    ),
];

/// The board's settings.
pub static SETTINGS: &[Field] = &[
    f("title", Kind::Str(200), text("")),
    f("voting", Kind::Voting, None),
];

/// A field of an object of this type, if the format knows it.
pub fn field_of(type_name: &str, name: &str) -> Option<&'static Field> {
    let own = TYPES
        .iter()
        .find(|(t, _)| *t == type_name)
        .and_then(|(_, fields)| fields.iter().find(|x| x.name == name));
    own.or_else(|| COMMON.iter().find(|x| x.name == name))
}

fn s(v: &str) -> Value {
    Value::Str(v.to_string())
}
fn n(v: usize) -> Value {
    Value::Num(v as f64)
}
fn obj(items: Vec<(&str, Value)>) -> Value {
    Value::Obj(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
fn names(list: &[&str]) -> Value {
    Value::Arr(list.iter().map(|x| s(x)).collect())
}

fn kind_name(k: &Kind) -> &'static str {
    match k {
        Kind::Num => "number",
        Kind::Bool => "bool",
        Kind::Str(_) => "text",
        Kind::Open(_) => "name",
        Kind::Color(_) => "colour",
        Kind::ObjectRef => "object_ref",
        Kind::GroupId => "group_id",
        Kind::Tags => "tags",
        Kind::Points => "points",
        Kind::End => "end",
        Kind::Checks => "checks",
        Kind::Columns => "columns",
        Kind::Rows => "rows",
        Kind::Reactions => "reactions",
        Kind::Votes => "votes",
        Kind::Replies => "replies",
        Kind::Crop => "crop",
        Kind::Link => "link",
        Kind::Blob => "blob",
        Kind::Created => "created",
        Kind::Voting => "voting",
    }
}

fn field_value(x: &Field) -> Value {
    let mut m = Fields::new();
    m.insert("name".into(), s(x.name));
    m.insert("kind".into(), s(kind_name(&x.kind)));
    match x.kind {
        Kind::Str(max) => {
            m.insert("max_chars".into(), n(max as usize));
        }
        Kind::Open(known) | Kind::Color(known) => {
            m.insert("known".into(), names(known));
        }
        _ => {}
    }
    match x.default {
        Some(Default::Num(v)) => {
            m.insert("default".into(), Value::Num(v));
        }
        Some(Default::Bool(v)) => {
            m.insert("default".into(), Value::Bool(v));
        }
        Some(Default::Str(v)) => {
            m.insert("default".into(), s(v));
        }
        None => {}
    }
    Value::Obj(m)
}

fn fields_value(list: &[Field]) -> Value {
    Value::Arr(list.iter().map(field_value).collect())
}

/// The schema as a value.
pub fn schema_value() -> Value {
    let limits: Fields = Limits::V1
        .named()
        .iter()
        .map(|(k, v)| (k.to_string(), Value::Num(*v as f64)))
        .chain([(
            "number_literal_bytes".to_string(),
            n(json::MAX_NUMBER_LITERAL),
        )])
        .collect();
    let types: Fields = TYPES
        .iter()
        .map(|(t, fields)| (t.to_string(), fields_value(fields)))
        .collect();
    obj(vec![
        ("format", s(FORMAT)),
        ("version", Value::Num(f64::from(VERSION))),
        ("min_reader", Value::Num(f64::from(MIN_READER))),
        ("media_type", s(MEDIA_TYPE)),
        (
            "notice",
            s("Portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT."),
        ),
        (
            "id",
            obj(vec![
                ("min_bytes", n(1)),
                ("max_bytes", n(ID_MAX_BYTES)),
                ("chars", s("0-9a-z._-")),
                ("refusal_lists_at_most", n(BAD_IDS_MAX)),
                ("refusal_lists_chars_of_each", n(BAD_ID_MAX_CHARS)),
            ]),
        ),
        ("field_name_max_bytes", n(FIELD_NAME_MAX_BYTES)),
        (
            "name",
            obj(vec![
                ("max_bytes", n(NAME_MAX_BYTES)),
                ("chars", s("a-z0-9-")),
            ]),
        ),
        ("limits", Value::Obj(limits)),
        ("max_number", Value::Num(json::MAX_NUMBER)),
        (
            "kinds",
            obj(vec![
                (
                    "tags",
                    obj(vec![
                        ("max_items", n(TAGS_MAX)),
                        ("max_item_chars", n(TAG_MAX_CHARS)),
                    ]),
                ),
                ("points", obj(vec![("max_items", n(POINTS_MAX))])),
                (
                    "checks",
                    obj(vec![
                        ("max_items", n(CHECKS_MAX)),
                        ("max_text_chars", n(CHECK_TEXT_MAX_CHARS)),
                    ]),
                ),
                ("columns", obj(vec![("max_items", n(COLUMNS_MAX))])),
                (
                    "rows",
                    obj(vec![
                        ("max_items", n(ROWS_MAX)),
                        ("max_cell_chars", n(CELL_MAX_CHARS)),
                    ]),
                ),
                (
                    "replies",
                    obj(vec![
                        ("max_items", n(REPLIES_MAX)),
                        ("max_text_chars", n(REPLY_TEXT_MAX_CHARS)),
                    ]),
                ),
                (
                    "reactions",
                    obj(vec![
                        ("known", names(REACTIONS)),
                        ("max_key_bytes", n(REACTION_KEY_MAX_BYTES)),
                        ("max_keys", n(REACTIONS_MAX)),
                        ("max_people", n(REACTION_PEOPLE_MAX)),
                    ]),
                ),
                (
                    "votes",
                    obj(vec![
                        ("max_people", n(VOTERS_MAX)),
                        ("max_votes", Value::Num(VOTES_MAX)),
                    ]),
                ),
                ("link", obj(vec![("max_chars", n(LINK_MAX_CHARS))])),
                (
                    "timestamp",
                    obj(vec![("max_bytes", n(TIMESTAMP_MAX_BYTES))]),
                ),
                (
                    "crop",
                    obj(vec![
                        ("min_size", Value::Num(CROP_MIN)),
                        ("max_edge", Value::Num(CROP_EDGE)),
                    ]),
                ),
                ("end", obj(vec![("anchors", names(ANCHORS))])),
            ]),
        ),
        ("common", fields_value(COMMON)),
        ("types", Value::Obj(types)),
        ("settings", fields_value(SETTINGS)),
        (
            "person",
            obj(vec![
                ("kinds", names(PERSON_KINDS)),
                ("name_max_chars", n(PERSON_NAME_MAX_CHARS)),
            ]),
        ),
        ("roles", names(&["board", "blob", "preview"])),
        (
            "refusals",
            Value::Arr(RefusalCode::ALL.iter().map(|c| s(c.as_str())).collect()),
        ),
    ])
}

/// The bytes of `schema/board-schema.json`: canonical JSON plus a final newline.
pub fn schema_json() -> String {
    let bytes = json::to_canonical(&schema_value()).unwrap_or_default();
    let mut text = String::from_utf8(bytes).unwrap_or_default();
    text.push('\n');
    text
}

fn kind_cell(k: &Kind) -> String {
    match k {
        Kind::Num => "number".to_string(),
        Kind::Bool => "bool".to_string(),
        Kind::Str(max) => format!("text, at most {max}"),
        Kind::Open(known) => format!("name: {}", known.join(", ")),
        Kind::Color(known) => format!("colour: {}", known.join(", ")),
        Kind::ObjectRef => "object reference".to_string(),
        Kind::GroupId => "group id".to_string(),
        Kind::Tags => "tags".to_string(),
        Kind::Points => "points".to_string(),
        Kind::End => "connector end".to_string(),
        Kind::Checks => "check items".to_string(),
        Kind::Columns => "columns".to_string(),
        Kind::Rows => "rows".to_string(),
        Kind::Reactions => "reactions".to_string(),
        Kind::Votes => "votes".to_string(),
        Kind::Replies => "replies".to_string(),
        Kind::Crop => "crop".to_string(),
        Kind::Link => "link".to_string(),
        Kind::Blob => "blob reference".to_string(),
        Kind::Created => "created".to_string(),
        Kind::Voting => "voting".to_string(),
    }
}

fn default_cell(d: &Option<Default>) -> String {
    match d {
        Some(Default::Num(v)) => format!("`{v}`"),
        Some(Default::Bool(v)) => format!("`{v}`"),
        Some(Default::Str(v)) => format!("`\"{v}\"`"),
        None => "none".to_string(),
    }
}

fn field_rows(out: &mut String, list: &[Field]) {
    out.push_str("| Field | Holds | When absent |\n|---|---|---|\n");
    for x in list {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            x.name,
            kind_cell(&x.kind),
            default_cell(&x.default)
        ));
    }
}

/// The generated part of `docs/FORMAT.md`: limits, field tables and refusal codes.
pub fn format_tables() -> String {
    let mut out = String::new();
    out.push_str("### Limits\n\n| Limit | Most |\n|---|---|\n");
    for (name, v) in Limits::V1.named() {
        out.push_str(&format!("| `{name}` | {v} |\n"));
    }
    out.push_str("\n### Fields on every object\n\n");
    field_rows(&mut out, COMMON);
    for (t, fields) in TYPES {
        out.push_str(&format!("\n### `{t}`\n\n"));
        field_rows(&mut out, fields);
    }
    out.push_str("\n### `settings`\n\n");
    field_rows(&mut out, SETTINGS);
    out.push_str("\n### Refusal codes\n\n");
    for c in RefusalCode::ALL {
        out.push_str(&format!("- `{}`\n", c.as_str()));
    }
    out
}
