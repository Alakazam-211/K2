//! The three-way merge of a person's app save against its base, by object and field: `ours` is
//! the doc's current version (where agents' edits land, through [`crate::write`](fn@crate::write)), `theirs`
//! what the person's app saved, `base` what both started from.
//!
//! **The person's save is the file.** The app wrote `theirs` whole, so the merge starts from it
//! and replays onto it what the agent changed: `ours` against `base`, field by field. The agent
//! only ever changes fixed-size fields (an object's location, rotation, scale, visibility bits,
//! name), so `ours` must differ from `base` only inside those fields; anything else is a
//! whole-file conflict (the kit keeps both). Each agent change is written onto `theirs` with
//! the writer, so every other byte of the person's save stays.
//!
//! **Objects are matched by key**, the type code and name. An object of `base` whose key is
//! gone from `theirs` was renamed by the person when exactly one new object of `theirs` (a key
//! `base` doesn't have) has the same data, or, without data, the same parent: the carried key
//! rule. Several candidates never pair: the object is taken as deleted.
//!
//! **Never last-writer-wins.** A field both sides changed, differently, keeps the person's
//! value and holds the agent's ([`FieldConflict`], [`ConflictWhy::BothChanged`]). An agent
//! change to an object the person deleted is held ([`ConflictWhy::PersonDeleted`]). An agent
//! change the writer refuses on the person's save (a name the person gave another object, a
//! rotation mode the person changed, a version or field the save doesn't have) is held with the
//! writer's reason ([`ConflictWhy::Refused`]). The same change on both sides is no conflict.
//! Each visibility bit is its own field.
//!
//! **Where a held change goes.** [`merge3`] reports each held change ([`Merged::conflicts`]);
//! it keeps nothing itself. Until the kit keeps held conflicts durably, the pack turns a merge
//! with any of them into a whole-file conflict ([`crate::pack::merge_known_base`]), so the
//! agent's version is kept whole by the kit and survives a restart. The records are what the
//! kit's store will take when it lands.
//!
//! **`base` must be the version the app had open.** Against any other, a save made before a
//! kit write reads as the person undoing it. The kit hands a pack what the file last held, so
//! the pack doesn't merge in a session yet ([`crate::pack`]).

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_base::index::Digest;

use crate::error::ReadLimits;
use crate::mutant;
use crate::reader::read_blend;
use crate::scene::{read_scene, Detail, IdBlock, SceneModel};
use crate::write::{decompress, write, Change, Edit};

/// Why an agent change is held.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConflictWhy {
    /// The person and the agent both changed the field, differently.
    BothChanged,
    /// The person deleted the object; the agent changed it.
    PersonDeleted,
    /// The writer refused the agent's change on the person's save (its reason).
    Refused(String),
}

/// One held conflict: an object's field as the base, the person (in the file) and the agent had
/// it. Values are text, as the index shows them (`1,2,3`, `true`, a name).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldConflict {
    /// Stable: the same clash has the same id.
    pub id: String,
    /// The object's key in the person's save (in the base, when the person deleted it).
    pub object: String,
    /// `loc`, `rot`, `scale`, `hide_viewport`, `hide_select`, `hide_render` or `name`.
    pub field: &'static str,
    /// Why it is held.
    pub why: ConflictWhy,
    /// The base's value.
    pub base: Option<String>,
    /// The person's value (`None`: deleted).
    pub person: Option<String>,
    /// The agent's value.
    pub agent: String,
}

/// An agent change that landed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Applied {
    /// The object's key in the person's save before the merge.
    pub object: String,
    /// The field.
    pub field: &'static str,
    /// The agent's value.
    pub value: String,
}

/// What a merge made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Merged {
    /// The merged file, stored as the person's save is.
    pub bytes: Vec<u8>,
    /// The agent's changes that landed.
    pub applied: Vec<Applied>,
    /// The agent's changes held, the person's kept.
    pub conflicts: Vec<FieldConflict>,
}

/// What [`merge3`] came to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// Merged, with any conflicts held.
    Merged(Merged),
    /// The changes can't be placed for sure; why. The kit keeps both.
    Conflict(String),
    /// One of the files is one this pack doesn't read; why.
    Unsupported(String),
}

/// The fields the merge knows, in the order it writes them.
pub const FIELDS: &[&str] = &[
    "name",
    "loc",
    "rot",
    "scale",
    "hide_viewport",
    "hide_select",
    "hide_render",
];

/// An object's merged fields, as text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Obj {
    index: usize,
    key: String,
    values: BTreeMap<&'static str, String>,
    data: Option<String>,
    parent: Option<String>,
}

fn objects(m: &SceneModel) -> Vec<Obj> {
    let key = |i: usize| m.ids[i].key.clone();
    m.ids
        .iter()
        .enumerate()
        .filter(|(_, id)| id.kind == "object" && id.struct_name == "Object" && id.library.is_none())
        .filter_map(|(i, id)| {
            let k = id.key.clone()?;
            let (data, parent) = match &id.detail {
                Detail::Object { data, parent } => (data.and_then(key), parent.and_then(key)),
                _ => (None, None),
            };
            Some(Obj {
                index: i,
                key: k,
                values: values(id),
                data,
                parent,
            })
        })
        .collect()
}

fn values(id: &IdBlock) -> BTreeMap<&'static str, String> {
    let mut v = BTreeMap::new();
    v.insert("name", id.name.clone());
    let get = |n: &str| {
        id.fields
            .iter()
            .find(|(f, _)| f == n)
            .map(|(_, x)| x.clone())
    };
    for (field, names) in [
        ("loc", &["loc"][..]),
        ("rot", &["rot"][..]),
        ("scale", &["scale", "size"][..]),
    ] {
        if let Some(x) = names.iter().find_map(|n| get(n)) {
            v.insert(field, x);
        }
    }
    for f in ["hide_viewport", "hide_select", "hide_render"] {
        if let Some((_, x)) = id.named.iter().find(|(n, _)| n == f) {
            v.insert(f, x.clone());
        }
    }
    v
}

/// The model of a file, with its decompressed bytes.
fn open(bytes: &[u8], limits: &ReadLimits) -> Result<(Vec<u8>, SceneModel), String> {
    let (_, raw) = decompress(bytes, limits).map_err(|e| e.to_string())?;
    let file = read_blend(&mut &raw[..], limits).map_err(|e| e.to_string())?;
    let model = read_scene(&file).map_err(|e| e.to_string())?;
    Ok((raw, model))
}

/// The change that sets `field` to `value` (text as the model shows it).
fn change_of(field: &str, value: &str) -> Option<Change> {
    let floats = || -> Option<[f32; 3]> {
        let v: Vec<f32> = value
            .split(',')
            .map(|x| x.parse().ok())
            .collect::<Option<_>>()?;
        <[f32; 3]>::try_from(v).ok()
    };
    let bit = || match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    Some(match field {
        "name" => Change::Name(value.to_owned()),
        "loc" => Change::Location(floats()?),
        "rot" => Change::Rotation(floats()?),
        "scale" => Change::Scale(floats()?),
        "hide_viewport" => Change::Visibility {
            viewport: Some(bit()?),
            select: None,
            render: None,
        },
        "hide_select" => Change::Visibility {
            viewport: None,
            select: Some(bit()?),
            render: None,
        },
        "hide_render" => Change::Visibility {
            viewport: None,
            select: None,
            render: Some(bit()?),
        },
        _ => return None,
    })
}

fn conflict_id(object: &str, field: &str, person: &Option<String>, agent: &str) -> String {
    let mut d = Digest::stream();
    for part in [
        object,
        field,
        person.as_deref().unwrap_or("\u{0}gone"),
        agent,
    ] {
        d.update(&(part.len() as u64).to_le_bytes());
        d.update(part.as_bytes());
    }
    d.finish().as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Merges `ours` and `theirs` from `base`. A pure function of its inputs.
pub fn merge3(base: &[u8], ours: &[u8], theirs: &[u8], limits: &ReadLimits) -> Outcome {
    let plain = |b: &[u8]| {
        Outcome::Merged(Merged {
            bytes: b.to_vec(),
            ..Merged::default()
        })
    };
    if ours == theirs || base == theirs {
        return plain(ours);
    }
    if base == ours {
        return plain(theirs);
    }
    let (b, o, t) = match (open(base, limits), open(ours, limits), open(theirs, limits)) {
        (Ok(b), Ok(o), Ok(t)) => (b, o, t),
        (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return Outcome::Unsupported(e),
    };
    match merge_open(&b, &o, &t, theirs, limits) {
        Ok(m) => Outcome::Merged(m),
        Err(why) => Outcome::Conflict(why),
    }
}

fn merge_open(
    (base_raw, base): &(Vec<u8>, SceneModel),
    (ours_raw, ours): &(Vec<u8>, SceneModel),
    (_, theirs): &(Vec<u8>, SceneModel),
    theirs_bytes: &[u8],
    limits: &ReadLimits,
) -> Result<Merged, String> {
    // the agent's side: the base with fixed-size fields changed, nothing else
    if base_raw.len() != ours_raw.len() || base.ids.len() != ours.ids.len() {
        return Err("the agent's version isn't the base with fields changed".into());
    }
    let writable: Vec<(usize, usize)> = base
        .ids
        .iter()
        .filter(|id| id.kind == "object" && id.struct_name == "Object")
        .flat_map(|id| id.spans.iter())
        .filter(|s| {
            ["name", "loc", "rot", "size", "scale", "restrictflag"].contains(&s.field.as_str())
        })
        .map(|s| (s.offset as usize, (s.offset + u64::from(s.len)) as usize))
        .collect();
    let inside = |i: usize| writable.iter().any(|(a, z)| (*a..*z).contains(&i));
    if base_raw
        .iter()
        .zip(ours_raw)
        .enumerate()
        .any(|(i, (x, y))| x != y && !inside(i))
        && !mutant::active("merge_trusts_ours")
    {
        return Err("the agent's version changes more than the fields it may write".into());
    }
    let base_objs = objects(base);
    let ours_objs = objects(ours);
    let theirs_objs = objects(theirs);
    // the agent's changes, by the base's key
    let mut agent: Vec<(&Obj, &'static str, String)> = Vec::new();
    for bo in &base_objs {
        let Some(oo) = ours_objs.iter().find(|o| o.index == bo.index) else {
            return Err("an object the agent changed doesn't read".into());
        };
        for f in FIELDS {
            let (was, now) = (bo.values.get(f), oo.values.get(f));
            if was != now {
                let Some(now) = now else {
                    return Err("a field the agent changed doesn't read".into());
                };
                agent.push((bo, f, now.clone()));
            }
        }
    }
    // the person's side: each base object in their save, by key, else by the carried key rule
    let base_keys: BTreeSet<&str> = base_objs.iter().map(|o| o.key.as_str()).collect();
    let new_in_theirs: Vec<&Obj> = theirs_objs
        .iter()
        .filter(|o| !base_keys.contains(o.key.as_str()))
        .collect();
    let matched = |bo: &Obj| -> Option<&Obj> {
        if let Some(t) = theirs_objs.iter().find(|t| t.key == bo.key) {
            return Some(t);
        }
        if mutant::active("merge_no_carried_keys") {
            return None;
        }
        let mut c = new_in_theirs.iter().filter(|t| match (&bo.data, &t.data) {
            (Some(a), Some(b)) => a == b,
            (None, None) => bo.parent == t.parent,
            _ => false,
        });
        match (c.next(), c.next()) {
            (Some(t), None) => Some(t),
            _ => None,
        }
    };
    // the agent's changes that land, by the person's keys; each is tried with those before it
    let mut accepted: Vec<Edit> = Vec::new();
    let mut out = theirs_bytes.to_vec();
    let mut merged = Merged::default();
    for (bo, field, value) in agent {
        let base_value = bo.values.get(field).cloned();
        let Some(to) = matched(bo) else {
            let person = None;
            merged.conflicts.push(FieldConflict {
                id: conflict_id(&bo.key, field, &person, &value),
                object: bo.key.clone(),
                field,
                why: ConflictWhy::PersonDeleted,
                base: base_value,
                person,
                agent: value,
            });
            continue;
        };
        let person = to.values.get(field).cloned();
        if person.as_ref() == Some(&value) {
            continue; // the same change on both sides
        }
        let hold = |why: ConflictWhy| FieldConflict {
            id: conflict_id(&to.key, field, &person, &value),
            object: to.key.clone(),
            field,
            why,
            base: base_value.clone(),
            person: person.clone(),
            agent: value.clone(),
        };
        if person != base_value && !mutant::active("merge_agent_wins") {
            merged.conflicts.push(hold(ConflictWhy::BothChanged));
            continue;
        }
        let Some(change) = change_of(field, &value) else {
            merged
                .conflicts
                .push(hold(ConflictWhy::Refused("the value doesn't read".into())));
            continue;
        };
        accepted.push(Edit::new(to.key.clone(), change));
        match write(theirs_bytes, &accepted, limits) {
            Ok(w) => {
                out = w.bytes;
                merged.applied.push(Applied {
                    object: to.key.clone(),
                    field,
                    value,
                });
            }
            Err(e) => {
                accepted.pop();
                merged
                    .conflicts
                    .push(hold(ConflictWhy::Refused(e.to_string())));
            }
        }
    }
    merged.bytes = out;
    Ok(merged)
}
