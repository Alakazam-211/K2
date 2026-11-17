//! What a live change may be: one rule ([`check_edit`]) for the write-back
//! ([`super::splice::encode_from_paired`]) and for the kit's check of every live change before
//! it applies ([`super::BlendLive`]'s `Pack::check_edit`, [`EditRefusal::pack_refusal`]), so
//! the two never disagree: the doc never holds a change its write-back can't write.
//!
//! The live tier writes back exactly what the writer patches in place (an object's `xform`, its
//! three visibility bits, its name with its `nid`), and only where the live verbs write it. A
//! change is judged rule by rule, the first one broken refusing the whole change
//! ([`EditRefusal`], its [`Rule`], and the registered code when the rule has one):
//!
//! 1. **A doc read but not written back** ([`super::model::writes_back`]): any change,
//!    `live_read_only`. The writer writes every release read, so no doc is one.
//! 2. **The lists:** no collection or object added, removed, moved or retyped (any op on `colls`
//!    or `objs`).
//! 3. **The collections:** no field of one.
//! 4. **The objects**, in the list's order, each one's fields in name order:
//!    - `edit` `0` (linked from a library, or a field the writer can't find): any change,
//!      `blend_not_editable`; an object with no file key is refused too;
//!    - `xform`: kept, nine finite numbers; on `edit` `a` (animation, a driver or a constraint
//!      drives the transform) any change, bit for bit, `blend_driven`; on `edit` `t` (it rotates
//!      by quaternion or axis and angle) a change of its rotation, `blend_rotation_mode`;
//!    - `hidden`, `noselect`, `norender`: `0` or `1`; on an object with an animated visibility
//!      bit (`visanim`), any change, `blend_driven`, the animated bits named;
//!    - `name`: kept, not empty, no NUL, within the file's name field (the scene map's
//!      `name_max`), and no other object's of the file at that point in the list's order (the
//!      writer's renames go in that order), `blend_name`; with `nid` set to the new name's file
//!      key;
//!    - `nid`: only with its name;
//!    - any other field: refused (live v1 doesn't write it).
//!
//! The scene map isn't written back and isn't judged, nor is a field of an element the list
//! doesn't hold. [`check_edit`] refuses an edit of a root the live schema doesn't have, which the
//! write-back never sees. A change that keeps every value (an `xform` written with the same
//! floats) changes nothing and is taken.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::doc::Edit;
use k2_mpc_base::elem::{Key, ListOp, MapOp, Value};
use k2_mpc_base::errors::ErrorEntry;
use k2_mpc_base::session::{PackRefusal, View};
use k2_mpc_base::table::CELL_SEP;

use super::errors;
use super::model::{self, f, parse_floats, s, EditMode, COLLS, COLL_PROPS, OBJS, OBJ_PROPS, SCENE};
use super::view::address_of;
use crate::mutant;
use crate::scene::key_of;
use crate::write::{self, Change as WriterChange};

/// Which rule a change breaks ([`EditRefusal::rule`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Rule {
    /// A change to a doc read but not written back: `live_read_only`.
    ReadOnly,
    /// An op on a list root (`colls`, `objs`): an element added, removed, moved or retyped.
    List,
    /// A collection's field.
    Collection,
    /// A change to an object nothing writes live (`edit` `0`): `blend_not_editable`.
    NotEditable,
    /// A change to an object with no file key: the writer can't name it.
    NoFileKey,
    /// A change to a driven transform (`edit` `a`), or to the visibility of an object with an
    /// animated bit (`visanim`): `blend_driven`.
    Driven,
    /// A rotation change of an object that rotates by quaternion or axis and angle (`edit`
    /// `t`): `blend_rotation_mode`.
    RotationMode,
    /// A value the field can't hold: an `xform` removed or not nine finite numbers, a
    /// visibility bit not `0` or `1`, a name removed.
    Value,
    /// A name the file can't hold: empty, with a NUL, longer than the name field, or another
    /// object's: `blend_name`.
    Name,
    /// A name and its `nid` that don't go together: a `nid` changed without its name, or a new
    /// name whose `nid` isn't its file key.
    NameKey,
    /// A field live v1 doesn't write.
    Field,
    /// An edit of a root the live schema doesn't have, or of a root by another kind's ops.
    Root,
}

impl Rule {
    /// The rule's registered code ([`super::errors::ERRORS`]), when it has one.
    pub fn code(self) -> Option<&'static str> {
        match self {
            Rule::ReadOnly => Some(errors::READ_ONLY.name),
            Rule::NotEditable => Some(errors::NOT_EDITABLE.name),
            Rule::Driven => Some(errors::DRIVEN.name),
            Rule::RotationMode => Some(errors::ROTATION_MODE.name),
            Rule::Name => Some(errors::NAME.name),
            _ => None,
        }
    }
}

/// Why a live change is refused: the first rule it breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditRefusal {
    /// The rule broken.
    pub rule: Rule,
    /// The object it is about (`@obj:3`), when it is about one.
    pub address: Option<String>,
    /// The channel refused, for a driven change: `xform`, or the animated visibility bits as
    /// their F-curves name them (`hide_viewport,hide_render`).
    pub channel: Option<String>,
    /// Why, in words (without the code).
    pub message: String,
}

impl EditRefusal {
    fn new(rule: Rule, address: Option<&str>, message: String) -> Self {
        EditRefusal {
            rule,
            address: address.map(str::to_owned),
            channel: None,
            message,
        }
    }

    fn channel(mut self, channel: &str) -> Self {
        self.channel = Some(channel.to_owned());
        self
    }

    /// The registered code ([`Rule::code`]), when the rule has one.
    pub fn code(&self) -> Option<&'static str> {
        self.rule.code()
    }

    /// The registered entry of the code, when the rule has one.
    pub fn entry(&self) -> Option<&'static ErrorEntry> {
        self.code().and_then(errors::by_code)
    }

    /// The code the kit's edit check gives ([`EditRefusal::pack_refusal`]): the rule's
    /// registered code ([`EditRefusal::code`]), or `blend_live_change`
    /// ([`super::errors::LIVE_CHANGE`]) for a rule with none.
    pub fn pack_code(&self) -> &'static str {
        self.code().unwrap_or(errors::LIVE_CHANGE.name)
    }

    /// The refusal as the kit's edit check gives it (`Pack::check_edit`; the kit's
    /// `pack_refused`, `MPC-E823`): [`EditRefusal::pack_code`], and the message with the channel
    /// refused when the message doesn't name it (`(channel xform)`).
    pub fn pack_refusal(&self) -> PackRefusal {
        let message = match &self.channel {
            Some(c) if !self.message.contains(c.as_str()) => {
                format!("{} (channel {c})", self.message)
            }
            _ => self.message.clone(),
        };
        PackRefusal {
            code: self.pack_code(),
            message,
        }
    }

    /// The refusal as one line, the code first when the rule has one (`blend_driven: ...`): the
    /// write-back's `PackError` text.
    pub fn text(&self) -> String {
        match self.code() {
            Some(c) => format!("{c}: {}", self.message),
            None => self.message.clone(),
        }
    }
}

impl std::fmt::Display for EditRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

impl std::error::Error for EditRefusal {}

/// One element's fields: `field -> value`.
pub(crate) type Fields<'a> = BTreeMap<&'a str, &'a str>;

/// One element's fields of a composite map.
pub(crate) fn fields_of<'a>(m: &'a BTreeMap<String, Value>, k: &Key) -> Fields<'a> {
    let start = format!("{}{CELL_SEP}", k.as_str());
    m.range(start.clone()..)
        .take_while(|(key, _)| key.starts_with(&start))
        .map(|(key, v)| (&key[start.len()..], v.as_str().unwrap_or("")))
        .collect()
}

/// An object a change touches: its fields before and after.
pub(crate) struct Touched<'a> {
    pub key: &'a Key,
    pub before: Fields<'a>,
    pub after: Fields<'a>,
}

/// A change as both the write-back and [`check_edit`] see it, the doc's state before it
/// included.
pub(crate) struct Change<'a> {
    /// Why a list root changed, when one did.
    pub list: Option<String>,
    /// Whether a collection's field changed.
    pub colls: bool,
    /// The objects whose fields changed, in the list's order.
    pub objs: Vec<Touched<'a>>,
    /// The objects before the change, in the list's order, and their fields: the names the
    /// file holds.
    pub live: &'a [(Key, Value)],
    pub props: &'a BTreeMap<String, Value>,
    /// The scene map's `name_max`.
    pub name_max: Option<usize>,
}

/// The refusal of any change to a doc of `version` read but not written back, or `None`.
pub(crate) fn read_only(version: &str) -> Option<EditRefusal> {
    if model::writes_back(version) || mutant::active("ReadOnlyCodeDropped") {
        return None;
    }
    Some(EditRefusal::new(
        Rule::ReadOnly,
        None,
        errors::read_only_message(version),
    ))
}

/// The writer's edits for `c` on a doc of `version`, or the first rule it breaks.
pub(crate) fn judge(version: &str, c: &Change<'_>) -> Result<Vec<write::Edit>, EditRefusal> {
    let out = rules(c);
    if matches!(&out, Ok(e) if e.is_empty()) {
        return out;
    }
    match read_only(version) {
        Some(r) => Err(r),
        None => out,
    }
}

fn rules(c: &Change<'_>) -> Result<Vec<write::Edit>, EditRefusal> {
    if let Some(why) = &c.list {
        return Err(EditRefusal::new(Rule::List, None, why.clone()));
    }
    if c.colls {
        return Err(EditRefusal::new(
            Rule::Collection,
            None,
            "a collection's field differs from the file's: live v1 writes none".into(),
        ));
    }
    let mut names = Names {
        live: c.live,
        props: c.props,
        renamed: BTreeMap::new(),
    };
    let mut edits = Vec::new();
    for t in &c.objs {
        object(t, &mut names, c.name_max, &mut edits)?;
    }
    Ok(edits)
}

/// The names the file's own objects hold at a point of the list's order: the renames before
/// it applied, as the writer applies them.
struct Names<'a> {
    live: &'a [(Key, Value)],
    props: &'a BTreeMap<String, Value>,
    renamed: BTreeMap<&'a Key, &'a str>,
}

impl Names<'_> {
    /// Whether an object of the file other than `k` (not linked from a library, with a file
    /// key) holds `name` now.
    fn taken(&self, k: &Key, name: &str) -> bool {
        let field =
            |o: &Key, fld: &str| self.props.get(&model::prop(o, fld)).and_then(Value::as_str);
        self.live.iter().any(|(o, _)| {
            let nid = field(o, f::NID).unwrap_or("");
            let now = self.renamed.get(o).copied().or_else(|| field(o, f::NAME));
            o != k && !nid.is_empty() && !nid.starts_with("@li:") && now == Some(name)
        })
    }
}

/// Why an object nothing writes live is refused (the live verbs' words).
const NOT_EDITABLE_WHY: &str =
    "nothing writes this object live (linked from a library, or a field the writer can't find)";

/// One touched object's rules, its writer edits pushed onto `edits`.
fn object<'a>(
    t: &Touched<'a>,
    names: &mut Names<'a>,
    name_max: Option<usize>,
    edits: &mut Vec<write::Edit>,
) -> Result<(), EditRefusal> {
    let (a, b) = (&t.before, &t.after);
    if a == b {
        return Ok(());
    }
    let addr = address_of(OBJS, t.key);
    let refuse = |rule: Rule, why: String| EditRefusal::new(rule, Some(addr.as_str()), why);
    let mode = a
        .get(f::EDIT)
        .copied()
        .and_then(EditMode::parse)
        .unwrap_or(EditMode::Never);
    if mode == EditMode::Never {
        return Err(refuse(
            Rule::NotEditable,
            format!("{addr}: {NOT_EDITABLE_WHY}"),
        ));
    }
    let nid = a.get(f::NID).copied().unwrap_or("");
    if nid.is_empty() {
        return Err(refuse(
            Rule::NoFileKey,
            format!("{addr} has no file key: the writer can't name it"),
        ));
    }
    let mut fields: Vec<&str> = a.keys().chain(b.keys()).copied().collect();
    fields.sort_unstable();
    fields.dedup();
    let mut vis: [Option<bool>; 3] = [None; 3];
    for n in fields {
        let (x, y) = (a.get(n).copied(), b.get(n).copied());
        if x == y {
            continue;
        }
        match n {
            f::XFORM => {
                let (Some(x), Some(y)) = (x, y) else {
                    return Err(refuse(
                        Rule::Value,
                        format!("{addr}: its xform was added or removed"),
                    ));
                };
                let (Some(p), Some(q)) = (parse_floats::<9>(x), parse_floats::<9>(y)) else {
                    return Err(refuse(
                        Rule::Value,
                        format!("{addr}: an xform isn't nine finite numbers"),
                    ));
                };
                let part = |v: &[f32; 9], i: usize| [v[i], v[i + 1], v[i + 2]];
                let same = |i: usize| {
                    part(&p, i)
                        .iter()
                        .zip(part(&q, i))
                        .all(|(u, w)| u.to_bits() == w.to_bits())
                };
                let changed = [!same(0), !same(3), !same(6)];
                if !changed.contains(&true) {
                    continue;
                }
                if mode == EditMode::Driven && !mutant::active("CheckEditPassesDriven") {
                    return Err(refuse(
                        Rule::Driven,
                        format!(
                            "{addr}: its transform is driven (animation, drivers or a \
                             constraint): the app would recompute it on load"
                        ),
                    )
                    .channel(f::XFORM));
                }
                if mode == EditMode::NoRotation && changed[1] {
                    return Err(refuse(
                        Rule::RotationMode,
                        format!(
                            "{addr}: it rotates by {}, not by Euler angles",
                            a.get(f::ROTMODE).copied().unwrap_or("another mode")
                        ),
                    ));
                }
                let nid = nid.to_owned();
                if changed[0] {
                    edits.push(write::Edit::new(
                        nid.clone(),
                        WriterChange::Location(part(&q, 0)),
                    ));
                }
                if changed[1] {
                    edits.push(write::Edit::new(
                        nid.clone(),
                        WriterChange::Rotation(part(&q, 3)),
                    ));
                }
                if changed[2] {
                    edits.push(write::Edit::new(nid, WriterChange::Scale(part(&q, 6))));
                }
            }
            f::HIDDEN | f::NOSELECT | f::NORENDER => {
                let on = match y {
                    Some("1") => true,
                    Some("0") => false,
                    _ => return Err(refuse(Rule::Value, format!("{addr}: {n} isn't 0 or 1"))),
                };
                if let Some(bits) = a.get(f::VISANIM).copied().filter(|v| !v.is_empty()) {
                    return Err(refuse(
                        Rule::Driven,
                        format!(
                            "{addr}: its visibility is animated ({bits}): the app would \
                             recompute it on load, and live any animated bit refuses the change"
                        ),
                    )
                    .channel(bits));
                }
                let i = [f::HIDDEN, f::NOSELECT, f::NORENDER]
                    .iter()
                    .position(|x| *x == n)
                    .expect("one of the three");
                vis[i] = Some(on);
            }
            f::NAME => {
                let Some(new) = y else {
                    return Err(refuse(Rule::Value, format!("{addr}: its name was removed")));
                };
                let bad = |why: String| refuse(Rule::Name, format!("{addr}: {why}"));
                if new.is_empty() {
                    return Err(bad("the name is empty".into()));
                }
                if new.contains('\0') {
                    return Err(bad("the name holds a NUL".into()));
                }
                if let Some(max) = name_max.filter(|m| new.len() > *m) {
                    return Err(bad(format!(
                        "the name is {} bytes, over the file's name field ({max})",
                        new.len()
                    )));
                }
                if names.taken(t.key, new) {
                    return Err(bad("another object of the file has it".into()));
                }
                let want = key_of("OB", new.as_bytes(), None);
                if b.get(f::NID).copied() != want.as_deref() {
                    return Err(refuse(
                        Rule::NameKey,
                        format!("{addr}: renamed, but its nid isn't the new name's file key"),
                    ));
                }
            }
            f::NID => {
                if a.get(f::NAME) == b.get(f::NAME) {
                    return Err(refuse(
                        Rule::NameKey,
                        format!("{addr}: its nid changed without its name"),
                    ));
                }
            }
            other => {
                return Err(refuse(
                    Rule::Field,
                    format!(
                        "{addr}: its field {other} differs from the file's: live v1 doesn't \
                         write it"
                    ),
                ))
            }
        }
    }
    if vis.iter().any(Option::is_some) {
        edits.push(write::Edit::new(
            nid.to_owned(),
            WriterChange::Visibility {
                viewport: vis[0],
                select: vis[1],
                render: vis[2],
            },
        ));
    }
    if let Some(new) = b
        .get(f::NAME)
        .copied()
        .filter(|n| a.get(f::NAME) != Some(n))
    {
        names.renamed.insert(t.key, new);
        edits.push(write::Edit::new(
            nid.to_owned(),
            WriterChange::Name(new.to_owned()),
        ));
    }
    Ok(())
}

/// The scene map's `name_max`, when it holds one.
pub(crate) fn name_max(scene: Option<&BTreeMap<String, Value>>) -> Option<usize> {
    scene?.get(s::NAME_MAX)?.as_str()?.parse().ok()
}

/// Whether a live edit `edits` on the doc `view` holds is one the write-back writes: `Ok`, or
/// the first rule it breaks (the module's rules), the same verdict the write-back gives the
/// doc with the edit applied. Pure: it reads `view` and `edits`, and nothing else.
///
/// `view` is the doc as the kit's session holds it before the edit (the kit's edit check gives
/// [`super::BlendLive`]'s `Pack::check_edit` the same [`View`]), and `edits` the kit's ops, as a
/// person's or an agent's edit, an undo or a restore carries them (the live verbs' edits are
/// taken). The rules read the objects'
/// `edit` and `visanim` from `view`: what the file says drives them.
pub fn check_edit(view: &View, edits: &[Edit]) -> Result<(), EditRefusal> {
    let empty = BTreeMap::new();
    let mut list: [Option<String>; 2] = [None, None];
    // each map key's value once the edits apply (`None`: removed), in the edits' order
    let mut colls: BTreeMap<&str, Option<&Value>> = BTreeMap::new();
    let mut objs: BTreeMap<&str, Option<&Value>> = BTreeMap::new();
    for e in edits {
        match e {
            Edit::List { root, ops } if root == COLLS || root == OBJS => {
                let i = usize::from(root == OBJS);
                if list[i].is_none() {
                    list[i] = ops.first().map(|op| list_why(root, op));
                }
            }
            Edit::Map { root, ops } if root == COLL_PROPS || root == OBJ_PROPS => {
                let into = if root == COLL_PROPS {
                    &mut colls
                } else {
                    &mut objs
                };
                for op in ops {
                    match op {
                        MapOp::Set { key, value } => {
                            into.insert(key.as_str(), Some(value));
                        }
                        MapOp::Delete { key } => {
                            into.insert(key.as_str(), None);
                        }
                        _ => {
                            return Err(EditRefusal::new(
                                Rule::Root,
                                None,
                                format!("an op on `{root}` the live tier doesn't know"),
                            ))
                        }
                    }
                }
            }
            Edit::Map { root, .. } if root == SCENE => {}
            other => {
                return Err(EditRefusal::new(
                    Rule::Root,
                    None,
                    format!(
                        "an edit of `{}` the live schema doesn't take: live v1 writes the \
                         collections' and objects' fields only",
                        other.root()
                    ),
                ))
            }
        }
    }
    let cp = view.map(COLL_PROPS).unwrap_or(&empty);
    let op = view.map(OBJ_PROPS).unwrap_or(&empty);
    let live = view.list(OBJS).unwrap_or(&[]);
    let elems: BTreeSet<&str> = objs
        .keys()
        .filter_map(|k| k.split_once(CELL_SEP).map(|(e, _)| e))
        .collect();
    let mut touched = Vec::new();
    if !elems.is_empty() {
        for (k, _) in live {
            if !elems.contains(k.as_str()) {
                continue;
            }
            let before = fields_of(op, k);
            let mut after = before.clone();
            let start = format!("{}{CELL_SEP}", k.as_str());
            // the edit's own keys: a few, against each object it touches
            for (key, v) in &objs {
                let Some(fld) = key.strip_prefix(start.as_str()) else {
                    continue;
                };
                match v {
                    Some(v) => after.insert(fld, v.as_str().unwrap_or("")),
                    None => after.remove(fld),
                };
            }
            if before != after {
                touched.push(Touched {
                    key: k,
                    before,
                    after,
                });
            }
        }
    }
    let change = Change {
        list: list.into_iter().flatten().next(),
        colls: colls.iter().any(|(k, v)| cp.get(*k) != *v),
        objs: touched,
        live,
        props: op,
        name_max: name_max(view.map(SCENE)),
    };
    let version = view
        .map(SCENE)
        .and_then(|m| m.get(s::VERSION))
        .and_then(Value::as_str)
        .unwrap_or("");
    judge(version, &change).map(|_| ())
}

/// Why an op on a list root is refused.
fn list_why(root: &str, op: &ListOp) -> String {
    let what = match op {
        ListOp::Insert { .. } => "an insert",
        ListOp::Move { .. } => "a move",
        ListOp::Delete { .. } => "a delete",
        ListOp::Restore { .. } => "a restore",
        ListOp::Set { .. } => "a kind set",
        ListOp::Drop { .. } => "a drop",
        _ => "an op",
    };
    format!(
        "the live `{root}` differ from the file's in their elements, order or kinds ({what} of \
         {}): live v1 adds, removes and moves none",
        address_of(root, op.key())
    )
}
