//! The agent's verbs on a live scene, as the kit's structured verbs: each computes ops by
//! key on the live store under the session lock, and the kit applies them as one change with its
//! guards (an agent's edit needs `--base` or `--if-version`; a facet written since the base is
//! refused, by name).
//!
//! | Verb | Ops |
//! |---|---|
//! | `scene list [--after @coll:N] [--limit N]` | read: one line per collection (its kind, name, holder, objects, visibility), at most 200 a page |
//! | `collection show @coll:N [--after @obj:N] [--limit N]` | read: the collection, then its objects (type, name, transform, visibility, edit), at most 200 a page |
//! | `object show @obj:N` | read: every field |
//! | `object place @obj:N [--loc X,Y,Z] [--rot X,Y,Z] [--scale X,Y,Z]` | set `xform` (one field) |
//! | `object visibility @obj:N [--viewport on\|off] [--select on\|off] [--render on\|off]` | set `hidden`, `noselect`, `norender` |
//! | `object rename @obj:N --name NAME --break-links` | set `name` and `nid` in one change |
//!
//! **Refusals are typed and registered** ([`super::errors::ERRORS`], ids `BLEND-Ennn`): each
//! prints `refused: <code>: <why>` and a `next:` line naming a verb, and exits 2; nothing
//! applies. A missing collection or object is `blend_not_found`, exit 4. The host gets the
//! entry's report, with the details `code`, `next`, `verb` and `address` (and `name` for
//! `blend_breaks_links`).
//!
//! **Results as data.** Each verb gives its payload as data (`StructOutput::data`), which the
//! kit puts in a call's result (`mpc.result/1`, asked for with `--json` or by the host) as
//! `data`, its shape named (`data_schema` `typed`, `data_spec` `blend.<verb>/1`, as
//! `blend.scene-list/1`); the text form is unchanged. An object is `{address, kind, name,
//! transform, visibility, edit}`: `transform` is `{location, rotation, scale}` (three numbers
//! each, as the doc stores them) or null; `visibility` is `{viewport, select, render}` (`true`:
//! on, as the verb's flags say); `edit` is what the live verbs write (`full`, `no_rotation`,
//! `driven`, `never`). A collection is `{address, kind, name, parent, objects, hidden}`
//! (`objects` its count). A list's next page is the result's `page.next` too. Each verb's shape
//! is pinned in `tests/results_golden/`.
//!
//! - `object place` on an object whose transform is driven (`edit` `a`: animation, drivers or a
//!   constraint; the app would recompute it on load) is refused, `blend_driven`; `--rot` on an
//!   object that rotates by quaternion or axis and angle (`t`), `blend_rotation_mode`; anything
//!   on an object nothing writes (`0`: linked from a library), `blend_not_editable`.
//! - `object rename` sets the file key (`nid`) with the name, so the next outside save still
//!   pairs the object by name. Other files that link the object by its name don't follow
//!   a rename: the verb says so and is refused (`blend_breaks_links`) unless `--break-links`
//!   names the loss. A name that is empty, holds a NUL, is longer than the file's name field or
//!   another object of the file has is refused (`blend_name`).

use std::collections::{BTreeMap, HashMap};

use k2_mpc_base::doc::Edit;
use k2_mpc_base::elem::{Applied, Key, MapOp, Value};
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::View;
use k2_mpc_base::table::CELL_SEP;
use k2_mpc_base::tools::exit::{NOT_FOUND, OK, USAGE};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};

use super::model::{
    f, floats_text, parse_floats, prop, s, EditMode, COLLS, COLL_PROPS, OBJS, OBJ_PROPS, SCENE,
};
use super::view::{address_of, key_of, xform_text};
use crate::mutant;
use crate::scene::key_of as file_key;

/// The most collections or objects one page holds: the default and the most.
pub const PAGE_MAX: usize = 200;

const EXITS: &[u8] = &[OK, USAGE, NOT_FOUND];

/// The verbs, as the kit routes them.
pub const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "scene list",
        "[--after @coll:N] [--limit N] (each collection: address, kind, name, holder, objects, visibility; at most 200 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_scene_list,
    ),
    PackVerb::structured_read(
        "collection show",
        "@coll:N [--after @obj:N] [--limit N] (the collection, then its objects: type, name, transform, visibility, edit; at most 200 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_collection_show,
    ),
    PackVerb::structured_read(
        "object show",
        "@obj:N (every field)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_object_show,
    ),
    PackVerb::structured_edit(
        "object place",
        "@obj:N [--loc X,Y,Z] [--rot X,Y,Z] [--scale X,Y,Z] (scene units; radians, Euler in the object's own order; a driven transform, or --rot on a quaternion or axis-angle rotation: exit 2)",
        &[PersonAction::RowMove],
        EXITS,
        parse_object_place,
    ),
    PackVerb::structured_edit(
        "object visibility",
        "@obj:N [--viewport on|off] [--select on|off] [--render on|off]",
        &[PersonAction::Format],
        EXITS,
        parse_object_visibility,
    ),
    PackVerb::structured_edit(
        "object rename",
        "@obj:N --name NAME --break-links (other files linking it by name lose it: --break-links names that loss; unique among the file's objects, inside its name field)",
        &[PersonAction::TypeOver],
        EXITS,
        parse_object_rename,
    ),
];

// ---------- the verb state: what the verbs would otherwise read every object for ----------

/// The objects indexed for the verbs (the kit's verb state, kept through every change): each
/// live object's position, each collection's objects in the objects' order, and how many
/// objects of the file (not linked from a library) have each name. Without it (outside a
/// session) the verbs read every object as before.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObjIndex {
    pos: HashMap<Key, usize>,
    by_coll: HashMap<String, Vec<Key>>,
    names: HashMap<String, usize>,
}

fn linked(nid: Option<&str>) -> bool {
    nid.unwrap_or("").starts_with("@li:")
}

impl ObjIndex {
    /// The index of `view`'s objects.
    pub fn build(view: &View) -> ObjIndex {
        let d = Doc::of(view, None);
        let mut ix = ObjIndex::default();
        for (i, (k, _)) in d.objs.iter().enumerate() {
            ix.pos.insert(k.clone(), i);
            if let Some(c) = d.of_(k, f::COLL) {
                ix.by_coll.entry(c.to_owned()).or_default().push(k.clone());
            }
            if !linked(d.of_(k, f::NID)) {
                if let Some(n) = d.of_(k, f::NAME) {
                    *ix.names.entry(n.to_owned()).or_default() += 1;
                }
            }
        }
        ix
    }

    /// The index after one change, `view` the doc after it: built again when objects came,
    /// went or moved, or an object's collection or its being linked changed (outside saves);
    /// a rename moves one name's count.
    pub fn change(&mut self, applied: &[Applied], view: &View) -> bool {
        fn field(key: &str) -> Option<(&str, &str)> {
            key.split_once(CELL_SEP)
        }
        let rebuild = applied.iter().any(|a| match a {
            Applied::List { root, ops } => root == OBJS && !ops.is_empty(),
            Applied::Map { root, ops } if root == OBJ_PROPS => {
                ops.iter().any(|o| match field(&o.key) {
                    Some((_, x)) if x == f::COLL => true,
                    Some((_, x)) if x == f::NID => {
                        linked(o.before.as_ref().and_then(Value::as_str))
                            != linked(o.after.as_ref().and_then(Value::as_str))
                    }
                    Some(_) => false,
                    None => true,
                })
            }
            _ => false,
        });
        if rebuild {
            *self = ObjIndex::build(view);
            return true;
        }
        let d = Doc::of(view, None);
        for a in applied {
            let Applied::Map { root, ops } = a else {
                continue;
            };
            if root != OBJ_PROPS {
                continue;
            }
            for o in ops {
                let Some((e, x)) = field(&o.key) else {
                    continue;
                };
                let Ok(k) = Key::new(e) else { continue };
                if x != f::NAME || linked(d.of_(&k, f::NID)) {
                    continue;
                }
                if let Some(n) = o.before.as_ref().and_then(Value::as_str) {
                    if let Some(c) = self.names.get_mut(n) {
                        *c -= 1;
                        if *c == 0 {
                            self.names.remove(n);
                        }
                    }
                }
                if let Some(n) = o.after.as_ref().and_then(Value::as_str) {
                    *self.names.entry(n.to_owned()).or_default() += 1;
                }
            }
        }
        true
    }
}

// ---------- the doc, as a verb reads it ----------

static EMPTY: BTreeMap<String, Value> = BTreeMap::new();

struct Doc<'v> {
    colls: &'v [(Key, Value)],
    objs: &'v [(Key, Value)],
    cp: &'v BTreeMap<String, Value>,
    op: &'v BTreeMap<String, Value>,
    scene: &'v BTreeMap<String, Value>,
    ix: Option<&'v ObjIndex>,
}

impl<'v> Doc<'v> {
    fn of(view: &'v View, ix: Option<&'v ObjIndex>) -> Doc<'v> {
        Doc {
            colls: view.list(COLLS).unwrap_or(&[]),
            objs: view.list(OBJS).unwrap_or(&[]),
            cp: view.map(COLL_PROPS).unwrap_or(&EMPTY),
            op: view.map(OBJ_PROPS).unwrap_or(&EMPTY),
            scene: view.map(SCENE).unwrap_or(&EMPTY),
            ix,
        }
    }

    /// The doc a verb reads: its view, with the verb state when the kit keeps it.
    fn at(ctx: &VerbCtx<'v>) -> Doc<'v> {
        Doc::of(ctx.view(), ctx.verb_state::<ObjIndex>())
    }

    fn coll_pos(&self, k: &Key) -> Option<usize> {
        self.colls.iter().position(|(x, _)| x == k)
    }

    fn obj_pos(&self, k: &Key) -> Option<usize> {
        match self.ix {
            Some(ix) => ix.pos.get(k).copied(),
            None => self.objs.iter().position(|(x, _)| x == k),
        }
    }

    /// The objects of collection `c`, in the objects' order, with their list values.
    fn objs_of(&self, c: &Key) -> Vec<&'v (Key, Value)> {
        match self.ix {
            Some(ix) => ix
                .by_coll
                .get(c.as_str())
                .into_iter()
                .flatten()
                .filter_map(|k| self.objs.get(*ix.pos.get(k)?))
                .collect(),
            None => self
                .objs
                .iter()
                .filter(|(k, _)| self.of_(k, f::COLL) == Some(c.as_str()))
                .collect(),
        }
    }

    /// How many objects collection `c` holds.
    fn count_of(&self, c: &Key) -> usize {
        match self.ix {
            Some(ix) => ix.by_coll.get(c.as_str()).map_or(0, Vec::len),
            None => self
                .objs
                .iter()
                .filter(|(k, _)| self.of_(k, f::COLL) == Some(c.as_str()))
                .count(),
        }
    }

    /// Whether an object of the file other than `k` (not linked from a library) has `name`.
    fn name_taken(&self, k: &Key, name: &str) -> bool {
        match self.ix {
            Some(ix) => {
                let own = self.of_(k, f::NAME) == Some(name) && !linked(self.of_(k, f::NID));
                ix.names.get(name).copied().unwrap_or(0) > usize::from(own)
            }
            None => self.objs.iter().any(|(o, _)| {
                o != k && self.of_(o, f::NAME) == Some(name) && !linked(self.of_(o, f::NID))
            }),
        }
    }

    fn cf(&self, k: &Key, fld: &str) -> Option<&'v str> {
        self.cp.get(&prop(k, fld)).and_then(Value::as_str)
    }

    fn of_(&self, k: &Key, fld: &str) -> Option<&'v str> {
        self.op.get(&prop(k, fld)).and_then(Value::as_str)
    }

    fn edit(&self, k: &Key) -> EditMode {
        self.of_(k, f::EDIT)
            .and_then(EditMode::parse)
            .unwrap_or(EditMode::Never)
    }
}

// ---------- arguments and outputs ----------

type R = Result<StructOutput, ToolError>;

fn usage(m: impl Into<String>) -> ToolError {
    ToolError::usage(m)
}

/// Why an object no verb writes live is refused.
const NOT_EDITABLE_WHY: &str =
    "nothing writes this object live (linked from a library, or a field the writer can't find)";

/// A typed refusal by its code, one of the pack's registered entries
/// ([`super::errors::ERRORS`]), of the collection or object at `addr`: nothing applies; its
/// text is `refused: <code>: <why>` and `next: <next>`, its exit the entry's, and the host gets
/// the entry's report with `addr` as the detail `address`.
fn refuse(code: &str, why: &str, next: &str, addr: &str) -> R {
    match super::errors::by_code(code) {
        Some(entry) if !mutant::active("RefusalAsText") => {
            Ok(StructOutput::refused(entry, why, next).detail("address", addr))
        }
        // every code the verbs use is registered (`tests/results.rs` reads them out of this
        // file); this keeps the text form if one isn't
        _ => Ok(StructOutput::new(
            Vec::new(),
            format!("refused: {code}: {why}\nnext: {next}\n").into_bytes(),
            USAGE,
        )),
    }
}

/// A collection or object that isn't there: the registered `blend_not_found`, exit 4, with the
/// address as a detail.
fn not_found(addr: &str) -> R {
    refuse(
        "blend_not_found",
        &format!("{addr} names nothing in the scene"),
        "scene list",
        addr,
    )
}

/// The version of every verb's data shape (the result's `data_spec`, `blend.<verb>/1`): a new
/// one when a field is renamed, removed or given another type; a new field keeps it.
pub const DATA_VERSION: u32 = 1;

/// A verb's run, its result's data named `spec` at [`DATA_VERSION`] (the kit's
/// `StructOutput::data_spec`; a refusal has no data, and names none).
fn typed(spec: &'static str, f: impl FnOnce(&VerbCtx<'_>) -> R + 'static) -> StructEdit {
    Box::new(move |ctx: &VerbCtx<'_>| f(ctx).map(|o| o.data_spec(spec, DATA_VERSION)))
}

/// A verb that worked, with its payload as data (the call's result's `data`, when asked for).
fn done(edits: Vec<Edit>, out: String, data: Json) -> R {
    Ok(StructOutput::new(edits, out.into_bytes(), OK).data(data))
}

struct Args {
    pos: Vec<String>,
    vals: BTreeMap<String, String>,
    switches: Vec<String>,
}

fn parse_args(args: &[String], with_value: &[&str], switches: &[&str]) -> Result<Args, ToolError> {
    let mut a = Args {
        pos: Vec::new(),
        vals: BTreeMap::new(),
        switches: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        if with_value.contains(&x.as_str()) {
            let v = args
                .get(i + 1)
                .ok_or_else(|| usage(format!("{x} needs a value")))?;
            if a.vals.insert(x.clone(), v.clone()).is_some() {
                return Err(usage(format!("{x} given twice")));
            }
            i += 2;
        } else if switches.contains(&x.as_str()) {
            a.switches.push(x.clone());
            i += 1;
        } else if x.starts_with("--") {
            return Err(usage(format!("unknown flag {x}")));
        } else {
            a.pos.push(x.clone());
            i += 1;
        }
    }
    Ok(a)
}

impl Args {
    fn one(&self, what: &str) -> Result<&str, ToolError> {
        match self.pos.as_slice() {
            [one] => Ok(one),
            _ => Err(usage(format!("give one {what}"))),
        }
    }
    fn none(&self, verb: &str) -> Result<(), ToolError> {
        match self.pos.is_empty() {
            true => Ok(()),
            false => Err(usage(format!("{verb} takes no address"))),
        }
    }
    fn has(&self, s: &str) -> bool {
        self.switches.iter().any(|x| x == s)
    }
    fn val(&self, k: &str) -> Option<&str> {
        self.vals.get(k).map(String::as_str)
    }
}

fn coll_addr(s: &str) -> Result<Key, ToolError> {
    key_of(COLLS, s).ok_or_else(|| usage(format!("{s:?} isn't a collection (@coll:N)")))
}

fn obj_addr(s: &str) -> Result<Key, ToolError> {
    key_of(OBJS, s).ok_or_else(|| usage(format!("{s:?} isn't an object (@obj:N)")))
}

fn limit(a: &Args) -> Result<usize, ToolError> {
    match a.val("--limit") {
        None => Ok(PAGE_MAX),
        Some(v) => match v.parse::<usize>() {
            Ok(n) if (1..=PAGE_MAX).contains(&n) => Ok(n),
            _ => Err(usage(format!("--limit is 1 to {PAGE_MAX}"))),
        },
    }
}

fn on_off(a: &Args, flag: &str) -> Result<Option<bool>, ToolError> {
    match a.val(flag) {
        None => Ok(None),
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(_) => Err(usage(format!("{flag} is on or off"))),
    }
}

fn triple(a: &Args, flag: &str) -> Result<Option<[f32; 3]>, ToolError> {
    a.val(flag)
        .map(|v| {
            parse_floats::<3>(v)
                .ok_or_else(|| usage(format!("{flag} is three finite numbers X,Y,Z")))
        })
        .transpose()
}

/// One line of text: `\`, newline and tab escaped.
fn one_line(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn vis_text(d: &Doc<'_>, k: &Key) -> String {
    let off: Vec<&str> = [
        (f::HIDDEN, "viewport"),
        (f::NOSELECT, "select"),
        (f::NORENDER, "render"),
    ]
    .iter()
    .filter(|(fld, _)| d.of_(k, fld) == Some("1"))
    .map(|(_, w)| *w)
    .collect();
    if off.is_empty() {
        "shown".into()
    } else {
        format!("off: {}", off.join(","))
    }
}

// ---------- results as data ----------

/// Stored floats as data: each part of the comma-separated text a number, parsed as it is
/// written (the shortest text of an `f32`), so the number reads back to the same `f32`.
fn floats_data(text: &str) -> Option<Vec<Json>> {
    text.split(',')
        .map(|x| x.trim().parse::<f64>().ok().and_then(Json::float))
        .collect()
}

/// A transform as data: `{location, rotation, scale}`, three numbers each, or null.
fn xform_data(text: Option<&str>) -> Json {
    let Some(v) = text.and_then(floats_data).filter(|v| v.len() == 9) else {
        return Json::Null;
    };
    Json::obj([
        ("location", Json::List(v[0..3].to_vec())),
        ("rotation", Json::List(v[3..6].to_vec())),
        ("scale", Json::List(v[6..9].to_vec())),
    ])
}

/// An object's visibility as data: `{viewport, select, render}`, `true` when on.
fn vis_data(d: &Doc<'_>, k: &Key) -> Json {
    let on = |fld: &str| Json::from(d.of_(k, fld) != Some("1"));
    Json::obj([
        ("viewport", on(f::HIDDEN)),
        ("select", on(f::NOSELECT)),
        ("render", on(f::NORENDER)),
    ])
}

/// What the live verbs write of an object, as a word.
fn edit_word(m: EditMode) -> &'static str {
    match m {
        EditMode::Full => "full",
        EditMode::NoRotation => "no_rotation",
        EditMode::Driven => "driven",
        EditMode::Never => "never",
    }
}

/// An object as the reads give it: `{address, kind, name, transform, visibility, edit}`.
fn obj_data(d: &Doc<'_>, k: &Key, kind: &str) -> Json {
    Json::obj([
        ("address", Json::from(address_of(OBJS, k))),
        ("kind", Json::from(kind)),
        ("name", Json::from(d.of_(k, f::NAME).unwrap_or(""))),
        ("transform", xform_data(d.of_(k, f::XFORM))),
        ("visibility", vis_data(d, k)),
        ("edit", Json::from(edit_word(d.edit(k)))),
    ])
}

/// The address of collection `k`'s parent (the collection or scene holding it first), if any.
fn parent_addr(d: &Doc<'_>, k: &Key) -> Option<String> {
    d.cf(k, f::PARENT)
        .and_then(|p| Key::new(p).ok())
        .map(|p| address_of(COLLS, &p))
}

/// A collection as the reads give it: `{address, kind, name, parent, objects, hidden}`.
fn coll_data(d: &Doc<'_>, k: &Key, kind: &str) -> Json {
    Json::obj([
        ("address", Json::from(address_of(COLLS, k))),
        ("kind", Json::from(kind)),
        ("name", Json::from(d.cf(k, f::NAME).unwrap_or(""))),
        ("parent", Json::from(parent_addr(d, k))),
        ("objects", Json::from(d.count_of(k))),
        ("hidden", Json::from(d.cf(k, f::HIDDEN) == Some("1"))),
    ])
}

/// The arguments of a list verb's next page: `pos`, `--limit` when not the default, and
/// `--after` the last row shown.
fn page_args(pos: &[&str], n: usize, last: String) -> Vec<String> {
    let mut v: Vec<String> = pos.iter().map(|x| (*x).to_owned()).collect();
    if n != PAGE_MAX {
        v.push("--limit".to_owned());
        v.push(n.to_string());
    }
    v.push("--after".to_owned());
    v.push(last);
    v
}

// ---------- reads ----------

fn parse_scene_list(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after", "--limit"], &[])?;
    a.none("scene list")?;
    let after = a.val("--after").map(coll_addr).transpose()?;
    let n = limit(&a)?;
    Ok(typed("blend.scene-list", move |ctx: &VerbCtx<'_>| {
        let d = Doc::at(ctx);
        let start = match &after {
            None => 0,
            Some(k) => match d.coll_pos(k) {
                Some(p) => p + 1,
                None => return not_found(&address_of(COLLS, k)),
            },
        };
        let page: Vec<&(Key, Value)> = d.colls.iter().skip(start).take(n).collect();
        let mut out = String::new();
        let mut rows = Vec::new();
        for (k, v) in &page {
            rows.push(coll_data(&d, k, v.as_str().unwrap_or("")));
            let parent = parent_addr(&d, k).unwrap_or_else(|| "-".to_owned());
            out.push_str(&format!(
                "{}\t{}\t{}\tin {}\t{} objects\t{}\n",
                address_of(COLLS, k),
                v.as_str().unwrap_or(""),
                one_line(d.cf(k, f::NAME).unwrap_or("")),
                parent,
                d.count_of(k),
                if d.cf(k, f::HIDDEN) == Some("1") {
                    "hidden"
                } else {
                    "shown"
                }
            ));
        }
        let mut more = None;
        if start + page.len() < d.colls.len() {
            if let Some((k, _)) = page.last() {
                out.push_str(&format!(
                    "next: scene list --after {}\n",
                    address_of(COLLS, k)
                ));
                more = Some(page_args(&[], n, address_of(COLLS, k)));
            }
        }
        let o = done(
            Vec::new(),
            out,
            Json::obj([("collections", Json::List(rows))]),
        )?;
        Ok(match more {
            Some(a) => o.next_page("scene list", a),
            None => o,
        })
    }))
}

fn obj_line(d: &Doc<'_>, k: &Key, kind: &str) -> String {
    format!(
        "{}\t{kind}\t{}\t{}\t{}\tedit {}\n",
        address_of(OBJS, k),
        one_line(d.of_(k, f::NAME).unwrap_or("")),
        d.of_(k, f::XFORM)
            .and_then(xform_text)
            .unwrap_or_else(|| "no transform".into()),
        vis_text(d, k),
        d.of_(k, f::EDIT).unwrap_or("0"),
    )
}

fn parse_collection_show(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after", "--limit"], &[])?;
    let addr = a.one("collection")?.to_owned();
    let ck = coll_addr(&addr)?;
    let after = a.val("--after").map(obj_addr).transpose()?;
    let n = limit(&a)?;
    Ok(typed("blend.collection-show", move |ctx: &VerbCtx<'_>| {
        let d = Doc::at(ctx);
        let Some(ci) = d.coll_pos(&ck) else {
            return not_found(&addr);
        };
        let mine: Vec<&(Key, Value)> = d.objs_of(&ck);
        let start = match &after {
            None => 0,
            Some(k) => match mine.iter().position(|(x, _)| x == k) {
                Some(p) => p + 1,
                None => return not_found(&address_of(OBJS, k)),
            },
        };
        let mut out = format!(
            "{}\t{}\t{}\t{} objects\n",
            addr,
            d.colls[ci].1.as_str().unwrap_or(""),
            one_line(d.cf(&ck, f::NAME).unwrap_or("")),
            mine.len()
        );
        let page: Vec<&&(Key, Value)> = mine.iter().skip(start).take(n).collect();
        let mut rows = Vec::new();
        for (k, v) in page.iter().map(|x| (&x.0, &x.1)) {
            out.push_str(&obj_line(&d, k, v.as_str().unwrap_or("")));
            rows.push(obj_data(&d, k, v.as_str().unwrap_or("")));
        }
        let mut more = None;
        if start + page.len() < mine.len() {
            if let Some(last) = page.last() {
                out.push_str(&format!(
                    "next: collection show {addr} --after {}\n",
                    address_of(OBJS, &last.0)
                ));
                more = Some(page_args(&[addr.as_str()], n, address_of(OBJS, &last.0)));
            }
        }
        let data = Json::obj([
            (
                "collection",
                coll_data(&d, &ck, d.colls[ci].1.as_str().unwrap_or("")),
            ),
            ("objects", Json::List(rows)),
        ]);
        let o = done(Vec::new(), out, data)?;
        Ok(match more {
            Some(a) => o.next_page("collection show", a),
            None => o,
        })
    }))
}

fn parse_object_show(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let addr = a.one("object")?.to_owned();
    let k = obj_addr(&addr)?;
    Ok(typed("blend.object-show", move |ctx: &VerbCtx<'_>| {
        let d = Doc::at(ctx);
        let Some(i) = d.obj_pos(&k) else {
            return not_found(&addr);
        };
        let kind = d.objs[i].1.as_str().unwrap_or("");
        let mut out = format!("{addr}\t{kind}\n");
        let mut fields = Vec::new();
        let start = format!("{}{}", k.as_str(), k2_mpc_base::table::CELL_SEP);
        for (key, v) in d.op.range(start.clone()..) {
            let Some(fld) = key.strip_prefix(&start) else {
                break;
            };
            let v = v.as_str().unwrap_or("");
            let shown = match fld {
                f::COLL | f::PARENT => Key::new(v)
                    .ok()
                    .map(|x| address_of(if fld == f::COLL { COLLS } else { OBJS }, &x))
                    .unwrap_or_else(|| v.to_owned()),
                f::ALSO => v
                    .split(',')
                    .filter_map(|c| Key::new(c).ok())
                    .map(|c| address_of(COLLS, &c))
                    .collect::<Vec<_>>()
                    .join(","),
                _ => one_line(v),
            };
            out.push_str(&format!("{fld}\t{shown}\n"));
            let as_data = match fld {
                f::COLL | f::PARENT | f::ALSO => Json::from(shown),
                _ => Json::from(v),
            };
            fields.push((fld.to_owned(), as_data));
        }
        let data = Json::obj([
            ("object", obj_data(&d, &k, kind)),
            ("fields", Json::Obj(fields)),
        ]);
        done(Vec::new(), out, data)
    }))
}

// ---------- edits ----------

fn set(k: &Key, fld: &str, v: impl Into<String>) -> MapOp {
    MapOp::Set {
        key: prop(k, fld),
        value: Value::str(v.into()),
    }
}

fn props_edit(ops: Vec<MapOp>) -> Vec<Edit> {
    if ops.is_empty() {
        return Vec::new();
    }
    vec![Edit::Map {
        root: OBJ_PROPS.to_owned(),
        ops,
    }]
}

fn parse_object_place(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--loc", "--rot", "--scale"], &[])?;
    let addr = a.one("object")?.to_owned();
    let k = obj_addr(&addr)?;
    let v = [
        triple(&a, "--loc")?,
        triple(&a, "--rot")?,
        triple(&a, "--scale")?,
    ];
    if v.iter().all(Option::is_none) {
        return Err(usage("give --loc, --rot or --scale"));
    }
    Ok(typed("blend.object-place", move |ctx: &VerbCtx<'_>| {
        let d = Doc::at(ctx);
        if d.obj_pos(&k).is_none() {
            return not_found(&addr);
        }
        match d.edit(&k) {
            EditMode::Never => {
                return refuse(
                    "blend_not_editable",
                    NOT_EDITABLE_WHY,
                    &format!("object show {addr}"),
                    &addr,
                )
            }
            EditMode::Driven if !mutant::active("DrivenPlaced") => {
                return refuse(
                    "blend_driven",
                    "its transform is driven (animation, drivers or a constraint): the app would recompute it on load",
                    &format!("object show {addr}"),
                    &addr,
                )
            }
            EditMode::NoRotation if v[1].is_some() && !mutant::active("QuatWrittenAsEuler") => {
                return refuse(
                    "blend_rotation_mode",
                    &format!(
                        "it rotates by {}, not by Euler angles",
                        d.of_(&k, f::ROTMODE).unwrap_or("another mode")
                    ),
                    &format!("object place {addr} --loc X,Y,Z --scale X,Y,Z"),
                    &addr,
                )
            }
            _ => {}
        }
        let Some(old) = d.of_(&k, f::XFORM).and_then(parse_floats::<9>) else {
            return refuse(
                "blend_not_editable",
                "its transform isn't readable",
                &format!("object show {addr}"),
                &addr,
            );
        };
        let mut new = old;
        for (part, val) in v.iter().enumerate() {
            if let Some(t) = val {
                new[3 * part..3 * part + 3].copy_from_slice(t);
            }
        }
        let text = floats_text(&new);
        let ops = if new
            .iter()
            .zip(&old)
            .all(|(x, y)| x.to_bits() == y.to_bits())
        {
            Vec::new()
        } else {
            vec![set(&k, f::XFORM, text.clone())]
        };
        let line = format!("{addr}\t{}\n", xform_text(&text).unwrap_or_default());
        let data = Json::obj([
            ("object", Json::from(addr.as_str())),
            ("transform", xform_data(Some(text.as_str()))),
            ("changed", Json::from(!ops.is_empty())),
        ]);
        done(props_edit(ops), line, data)
    }))
}

fn parse_object_visibility(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--viewport", "--select", "--render"], &[])?;
    let addr = a.one("object")?.to_owned();
    let k = obj_addr(&addr)?;
    // `on` shows it: the bit is set when it is off
    let want = [
        on_off(&a, "--viewport")?,
        on_off(&a, "--select")?,
        on_off(&a, "--render")?,
    ];
    if want.iter().all(Option::is_none) {
        return Err(usage("give --viewport, --select or --render"));
    }
    Ok(typed(
        "blend.object-visibility",
        move |ctx: &VerbCtx<'_>| {
            let d = Doc::at(ctx);
            if d.obj_pos(&k).is_none() {
                return not_found(&addr);
            }
            if d.edit(&k) == EditMode::Never {
                return refuse(
                    "blend_not_editable",
                    NOT_EDITABLE_WHY,
                    &format!("object show {addr}"),
                    &addr,
                );
            }
            let mut ops = Vec::new();
            let mut now = Vec::new();
            for ((on, fld), word) in want
                .iter()
                .zip([f::HIDDEN, f::NOSELECT, f::NORENDER])
                .zip(["viewport", "select", "render"])
            {
                if let Some(on) = on {
                    let bit = if *on { "0" } else { "1" };
                    if d.of_(&k, fld) != Some(bit) {
                        ops.push(set(&k, fld, bit));
                    }
                }
                // what the object's bit is once the call applies
                let shown = on.unwrap_or(d.of_(&k, fld) != Some("1"));
                now.push((word, Json::from(shown)));
            }
            let mut out = format!("{addr}\t");
            out.push_str(&vis_text(&d, &k));
            out.push('\n');
            let data = Json::obj([
                ("object", Json::from(addr.as_str())),
                ("visibility", Json::obj(now)),
                ("changed", Json::from(!ops.is_empty())),
            ]);
            done(props_edit(ops), out, data)
        },
    ))
}

fn parse_object_rename(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--name"], &["--break-links"])?;
    let addr = a.one("object")?.to_owned();
    let k = obj_addr(&addr)?;
    let name = a
        .val("--name")
        .ok_or_else(|| usage("give --name NAME"))?
        .to_owned();
    let breaks = a.has("--break-links");
    Ok(typed("blend.object-rename", move |ctx: &VerbCtx<'_>| {
        let d = Doc::at(ctx);
        if d.obj_pos(&k).is_none() {
            return not_found(&addr);
        }
        if d.edit(&k) == EditMode::Never {
            return refuse(
                "blend_not_editable",
                NOT_EDITABLE_WHY,
                &format!("object show {addr}"),
                &addr,
            );
        }
        let old_nid = d.of_(&k, f::NID).unwrap_or("");
        let next = format!("object rename {addr} --name NAME --break-links");
        let bad = |why: &str| refuse("blend_name", why, &next, &addr);
        if name.is_empty() {
            return bad("the name is empty");
        }
        if name.contains('\0') {
            return bad("the name holds a NUL");
        }
        let max: usize = d
            .scene
            .get(s::NAME_MAX)
            .and_then(Value::as_str)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if name.len() > max {
            return bad(&format!(
                "the name is {} bytes, over the file's name field ({max})",
                name.len()
            ));
        }
        let Some(nid) = file_key("OB", name.as_bytes(), None) else {
            return bad("the name makes a file key over the kit's length limit");
        };
        let taken = d.name_taken(&k, &name);
        if taken && !mutant::active("RenameNotUnique") {
            return bad("another object of the file has it");
        }
        if !breaks {
            return refuse(
                "blend_breaks_links",
                &format!(
                    "other files that link {old_nid} by its name won't follow the rename: they lose the object. Name that loss with --break-links"
                ),
                &format!("object rename {addr} --name {name:?} --break-links"),
                &addr,
            )
            .map(|o| o.detail("name", name.as_str()));
        }
        let data = |changed: bool| {
            Json::obj([
                ("object", Json::from(addr.as_str())),
                ("name", Json::from(name.as_str())),
                ("key", Json::from(nid.as_str())),
                ("was", Json::from(old_nid)),
                ("changed", Json::from(changed)),
            ])
        };
        if d.of_(&k, f::NAME) == Some(name.as_str()) {
            return done(
                Vec::new(),
                format!("{addr}\t{nid}\tunchanged\n"),
                data(false),
            );
        }
        let mut ops = vec![set(&k, f::NAME, name.clone())];
        if !mutant::active("RenameKeepsNid") {
            ops.push(set(&k, f::NID, nid.clone()));
        }
        done(
            props_edit(ops),
            format!(
                "{addr}\t{nid}\twas {old_nid}: links to {old_nid} from other files are broken\n"
            ),
            data(true),
        )
    }))
}
