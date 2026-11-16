//! **A call's result as data** (schema [`super::RESULT_SCHEMA`], `mpc.result/1`): what
//! [`super::ToolOutput::result`] holds when the call asked for it
//! ([`super::ToolOptions::result_json`], or a pack verb's own `--json`). One JSON object, its
//! fields in [`RESULT_FIELDS`]' order:
//!
//! - `schema`; `verb`, the verb's registry name; `kind`, `read` or `edit` (null when the call
//!   named no verb the kit knows); `ok` (the exit is 0) and `exit`, the text form's exit.
//! - `version`: a read's `{read}`, the version read; an edit's `{base, after, if_version,
//!   base_given}`: the version the verb computed on, the version it made (null when it made
//!   none: a refusal, no change), and the guards the call gave (`--if-version`, `--base`).
//! - `changed`: null when nothing changed; else `{counts, units, units_more, lines}`. `counts`
//!   per structured root; `units` each unit the change wrote, by address and field
//!   ([`ChangedUnit`], at most [`UNITS_MAX`], the rest counted in `units_more`); `lines` a text
//!   edit's line ranges and counts (null for a structured one).
//! - `data`, `data_schema` and `data_spec`: the verb's own payload
//!   ([`super::StructOutput::data`]; `data_schema` is `typed`), else its stdout as
//!   `{"text": ...}` (`text`), so an agent can always tell typed data from text. `data_spec` names
//!   the typed data's shape and its version, `<name>/<version>`
//!   ([`super::StructOutput::data_spec`]), or is null (text, or a shape the verb doesn't name).
//! - `page`: `{next}`, the call that reads the next page ([`super::StructOutput::next_page`]),
//!   or null.
//! - `error`: the refusal as `explain` has it ([`crate::errors::ErrorReport::to_value`]), or
//!   null. A refusal the call returns as an error is this field too, with its exit: with the
//!   result asked for, every outcome is one object.
//! - `conflicts`: a `--base` conflict's units, by address and field, who wrote each and in which
//!   change (the error's `details.conflicts` stays as it was).
//! - `notes` (each heads-up as data, with its line as `text`) and `warnings` (lines).
//!
//! A new field never changes the version; a field renamed, removed or given another type does.

use crate::doc::Edit;
use crate::elem::{Key, ListOp, MapOp};
use crate::errors::{ErrorReport, Value};
use crate::schema::Kind;
use crate::session::{Pack, View};
use crate::store::Seq;

/// The result's fields, in order ([`super::ToolOutput::result`]).
pub const RESULT_FIELDS: &[&str] = &[
    "schema",
    "verb",
    "kind",
    "ok",
    "exit",
    "version",
    "changed",
    "data",
    "data_schema",
    "data_spec",
    "page",
    "error",
    "conflicts",
    "notes",
    "warnings",
];

/// The most changed units one result lists; the rest are counted (`units_more`).
pub const UNITS_MAX: usize = 1_000;

/// One unit a change wrote, by address and field: a list element (`address` the pack's,
/// [`Pack::address_of`]; no field), or a field of a composite map ([`Pack::composite`]: the
/// key `<element>\x1f<field>` split into the element's address and the field), or a key of
/// another map (no address, no field). `before` and `after` are the values (a string, or none),
/// as the doc held them before the change and as the change wrote them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChangedUnit {
    pub address: Option<String>,
    pub root: String,
    /// The element's key, or the map key (a composite map's: the element's part).
    pub key: String,
    pub field: Option<String>,
    /// `insert`, `move`, `delete`, `restore`, `set` or `drop`.
    pub op: &'static str,
    pub before: Option<String>,
    pub after: Option<String>,
}

impl ChangedUnit {
    /// `{address, root, key, field, op, before, after}`.
    pub fn to_value(&self) -> Value {
        Value::obj([
            ("address", Value::from(self.address.as_deref())),
            ("root", Value::from(self.root.as_str())),
            ("key", Value::from(self.key.as_str())),
            ("field", Value::from(self.field.as_deref())),
            ("op", Value::from(self.op)),
            ("before", Value::from(self.before.as_deref())),
            ("after", Value::from(self.after.as_deref())),
        ])
    }
}

/// What an edit's run kept for its result: the version it computed on and its units.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Capture {
    pub(crate) base: Option<Seq>,
    pub(crate) units: Vec<ChangedUnit>,
    pub(crate) more: usize,
}

/// The parts of a result the run path gives beside a [`super::ToolOutput`]'s fields, and the
/// result once built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResultData {
    pub(crate) capture: Capture,
    pub(crate) data: Option<Value>,
    /// The typed data's shape, `(name, version)` ([`super::StructOutput::data_spec`]).
    pub(crate) data_spec: Option<(&'static str, u32)>,
    pub(crate) next_page: Option<(&'static str, Vec<String>)>,
    pub(crate) value: Option<Value>,
}

/// Whether the units are planted as losing their addresses (test-only).
fn planted_units_drop_address() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::UnitsDropAddress)
    )
}

/// Whether the result is planted as leaving out the version computed on (test-only).
fn planted_drops_base() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::EnvelopeDropsBase)
    )
}

/// Whether the typed data's shape is planted as dropped from the result (test-only).
fn planted_data_spec_dropped() -> bool {
    plant_on!(
        crate::differential::ergo_mutant()
            == Some(crate::differential::ErgoMutant::DataSpecDropped)
    )
}

/// The address, key and field of key `key` of root `root`: a list element is its address; a
/// composite map's key splits at [`crate::table::CELL_SEP`] into its element's address and the
/// field; any other key is itself, with neither.
pub(crate) fn unit_of(
    pack: &dyn Pack,
    root: &str,
    key: &str,
) -> (Option<String>, String, Option<String>) {
    match pack.schema().kind_of(root) {
        Some(Kind::MovableList | Kind::List) => match Key::new(key) {
            Ok(k) => (Some(pack.address_of(root, &k)), key.to_owned(), None),
            Err(_) => (None, key.to_owned(), None),
        },
        Some(Kind::Map) => {
            let list = pack
                .composite()
                .iter()
                .find(|(m, _)| *m == root)
                .map(|(_, l)| *l);
            let split = key.split_once(crate::table::CELL_SEP);
            match (list, split) {
                (Some(list), Some((elem, field))) => match Key::new(elem) {
                    Ok(k) => (
                        Some(pack.address_of(list, &k)),
                        elem.to_owned(),
                        Some(field.to_owned()),
                    ),
                    Err(_) => (None, key.to_owned(), None),
                },
                _ => (None, key.to_owned(), None),
            }
        }
        _ => (None, key.to_owned(), None),
    }
}

/// A list element's value in `view` (live or deleted), as a string. A scan of the list: list
/// values are rare (a pack keeps an element's fields in maps).
fn list_value(view: &View, root: &str, key: &Key) -> Option<String> {
    let live = view
        .list(root)
        .and_then(|l| l.iter().find(|(k, _)| k == key).map(|(_, v)| v));
    live.or_else(|| view.hidden(root).and_then(|h| h.get(key)))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// The units `edits` (normalized, about to apply on `view`) write, at most [`UNITS_MAX`], and
/// how many more. O(edits), but a list element's value before a `set` or `delete` (a scan).
pub(crate) fn units_of(pack: &dyn Pack, view: &View, edits: &[Edit]) -> (Vec<ChangedUnit>, usize) {
    let mut units = Vec::new();
    let mut more = 0usize;
    let mut push = |u: ChangedUnit| {
        if units.len() < UNITS_MAX {
            units.push(u);
        } else {
            more += 1;
        }
    };
    let s = |v: &crate::elem::Value| v.as_str().map(str::to_owned);
    for e in edits {
        match e {
            Edit::List { root, ops } => {
                for op in ops {
                    let key = op.key();
                    let (word, before, after) = match op {
                        ListOp::Insert { value, .. } => ("insert", None, s(value)),
                        ListOp::Move { .. } => ("move", None, None),
                        ListOp::Delete { .. } => ("delete", list_value(view, root, key), None),
                        ListOp::Restore { .. } => ("restore", None, None),
                        ListOp::Set { value, .. } => ("set", list_value(view, root, key), s(value)),
                        ListOp::Drop { .. } => ("drop", None, None),
                    };
                    let (address, key, field) = unit_of(pack, root, key.as_str());
                    push(ChangedUnit {
                        address,
                        root: root.clone(),
                        key,
                        field,
                        op: word,
                        before,
                        after,
                    });
                }
            }
            Edit::Map { root, ops } => {
                for op in ops {
                    let k = op.key();
                    let before = view.map(root).and_then(|m| m.get(k)).and_then(s);
                    let (word, after) = match op {
                        MapOp::Set { value, .. } => ("set", s(value)),
                        MapOp::Delete { .. } => ("delete", None),
                    };
                    let (address, key, field) = unit_of(pack, root, k);
                    push(ChangedUnit {
                        address,
                        root: root.clone(),
                        key,
                        field,
                        op: word,
                        before,
                        after,
                    });
                }
            }
            _ => {}
        }
    }
    if planted_units_drop_address() {
        for u in &mut units {
            u.address = None;
        }
    }
    (units, more)
}

/// The call a result is for: its verb, kind and arguments (after the verb's words), and the
/// pack that reads its keys (none on a blob doc).
pub(crate) struct Call<'a> {
    pub(crate) verb: String,
    pub(crate) kind: Option<&'static str>,
    pub(crate) args: &'a [String],
    pub(crate) pack: Option<&'a dyn Pack>,
}

/// A `--base` conflict's units ([`ErrorReport`]'s `details.conflicts`), by address and field.
fn conflicts(pack: Option<&dyn Pack>, error: Option<&ErrorReport>) -> Value {
    let Some(Value::List(cs)) = error.and_then(|r| r.detail("conflicts")) else {
        return Value::List(Vec::new());
    };
    Value::List(
        cs.iter()
            .map(|c| {
                let root = c.get("root").and_then(Value::as_str).unwrap_or("");
                let key = c.get("key").and_then(Value::as_str).unwrap_or("");
                let (address, key, field) = match pack {
                    Some(p) => unit_of(p, root, key),
                    None => (None, key.to_owned(), None),
                };
                let at = |k: &str| c.get(k).cloned().unwrap_or(Value::Null);
                Value::obj([
                    ("address", Value::from(address)),
                    ("root", Value::from(root)),
                    ("key", Value::from(key)),
                    ("field", Value::from(field)),
                    ("facet", at("facet")),
                    ("by", at("by")),
                    ("change", at("change")),
                ])
            })
            .collect(),
    )
}

/// One heads-up note as data.
fn note_value(n: &crate::session::Note) -> Value {
    Value::obj([
        ("kind", Value::from(n.kind.as_str())),
        ("pid", Value::from(n.pid.as_str())),
        ("name", Value::from(n.name.as_str())),
        (
            "lines",
            Value::obj([
                ("root", Value::from(n.lines.root.as_str())),
                ("first", Value::from(n.lines.first)),
                ("last", Value::from(n.lines.last)),
            ]),
        ),
        ("age_ms", Value::from(n.age_ms)),
        (
            "claim",
            Value::from(n.claim.as_ref().map(ToString::to_string)),
        ),
        ("remaining_ms", Value::from(n.remaining_ms)),
        ("text", Value::from(n.to_string())),
    ])
}

/// The result of `o`, the output of `call` (its `result` parts as the run path left them).
pub(crate) fn value(call: &Call<'_>, o: &super::ToolOutput, err: bool) -> Value {
    let empty = ResultData::default();
    let rd = o.result.as_deref().unwrap_or(&empty);
    let (if_version, base_given) = super::args::take_guards(call.args)
        .map(|(i, b, _)| (i, b))
        .unwrap_or((None, None));
    let version = match call.kind {
        Some("read") => Value::obj([(
            "read",
            if err {
                Value::Null
            } else {
                Value::from(o.version)
            },
        )]),
        _ => {
            let base = match (&o.changed, rd.capture.base) {
                (_, Some(b)) => Some(b),
                (None, None) if !err => Some(o.version),
                _ => None,
            };
            let base = if planted_drops_base() { None } else { base };
            Value::obj([
                ("base", Value::from(base)),
                ("after", Value::from(o.changed.as_ref().map(|c| c.version))),
                ("if_version", Value::from(if_version)),
                ("base_given", Value::from(base_given)),
            ])
        }
    };
    let changed = match &o.changed {
        None => Value::Null,
        Some(c) => {
            let counts = c.elems.iter().map(|e| {
                Value::obj([
                    ("root", Value::from(e.root.as_str())),
                    ("inserted", Value::from(e.inserted)),
                    ("deleted", Value::from(e.deleted)),
                    ("moved", Value::from(e.moved)),
                    ("restored", Value::from(e.restored)),
                    ("set", Value::from(e.set)),
                    ("dropped", Value::from(e.dropped)),
                ])
            });
            let lines = if c.changed.is_empty() {
                Value::Null
            } else {
                Value::obj([
                    (
                        "ranges",
                        Value::List(
                            c.changed
                                .iter()
                                .map(|r| {
                                    Value::obj([
                                        ("root", Value::from(r.root.as_str())),
                                        ("first", Value::from(r.first)),
                                        ("last", Value::from(r.last)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    ("added", Value::from(c.lines_added)),
                    ("removed", Value::from(c.lines_removed)),
                ])
            };
            Value::obj([
                ("counts", Value::List(counts.collect())),
                (
                    "units",
                    Value::List(rd.capture.units.iter().map(ChangedUnit::to_value).collect()),
                ),
                ("units_more", Value::from(rd.capture.more)),
                ("lines", lines),
            ])
        }
    };
    let (data, data_schema, data_spec) = match &rd.data {
        Some(d) => (
            d.clone(),
            "typed",
            rd.data_spec
                .filter(|_| !planted_data_spec_dropped())
                .map(|(name, version)| format!("{name}/{version}")),
        ),
        None => (
            Value::obj([(
                "text",
                Value::from(String::from_utf8_lossy(&o.stdout).into_owned()),
            )]),
            "text",
            None,
        ),
    };
    let page = match &rd.next_page {
        Some((verb, args)) => Value::obj([(
            "next",
            Value::obj([
                ("verb", Value::from(*verb)),
                ("args", Value::list(args.iter().map(String::as_str))),
            ]),
        )]),
        None => Value::Null,
    };
    Value::obj([
        ("schema", Value::from(super::RESULT_SCHEMA)),
        ("verb", Value::from(call.verb.as_str())),
        ("kind", Value::from(call.kind)),
        ("ok", Value::from(o.exit == super::exit::OK)),
        ("exit", Value::from(o.exit)),
        ("version", version),
        ("changed", changed),
        ("data", data),
        ("data_schema", Value::from(data_schema)),
        ("data_spec", Value::from(data_spec)),
        ("page", page),
        (
            "error",
            o.refusal
                .as_ref()
                .map_or(Value::Null, ErrorReport::to_value),
        ),
        ("conflicts", conflicts(call.pack, o.refusal.as_ref())),
        (
            "notes",
            Value::List(o.notes.notes.iter().map(note_value).collect()),
        ),
        (
            "warnings",
            Value::list(o.warnings.lines().iter().map(String::as_str)),
        ),
    ])
}
