//! The pack's check of a doc the kit merged field by field from an outside save
//! ([`check_merged`], the live pack's `Pack::check_merged_view`).
//!
//! **When it runs.** The host noted the version an app opened (the kit's `note_opened`), and the
//! app saved: the kit reconciles the save against that version and merges each of its changes
//! onto the live doc field by field (kept where the live doc still holds the opened value,
//! skipped where it holds the save's, a clash otherwise). Two changes each valid alone can make
//! a doc the format forbids; this check refuses such a doc, and the kit then keeps both (the
//! save as the doc, the kit's version as a version and a copy beside the file), as with no
//! version noted. Nothing is dropped either way.
//!
//! **What it sees: the merged doc only.** The kit hands no pinned version, save or carrier, so
//! the check is of what the merged doc itself must hold, never of what changed. That is enough:
//! the save taken in becomes the carrier, and the merged doc differs from the save only where
//! the live doc changed since the version the app opened, changes the live doc already held.
//!
//! **The rules** (each refusal a fixed code; the text names addresses, never a name from the
//! file):
//!
//! - `blend_merge_name_taken`: two objects of the file (not linked from a library) with one
//!   name;
//! - `blend_merge_nid`: an object of the file whose file key (`nid`) isn't its name's (the key a
//!   rename sets with the name);
//! - `blend_merge_name`: a name that is missing, empty, holds a NUL, or is longer than the
//!   file's name field (the scene map's `name_max`);
//! - `blend_merge_value`: an `xform` that isn't nine finite numbers, a visibility bit that isn't
//!   `0` or `1`, a `quat` that isn't four finite numbers or is on an object that doesn't rotate
//!   by quaternion, an `edit` that isn't one of its words, a `visanim` naming anything but the
//!   visibility bits' properties;
//! - `blend_merge_holder`: a `coll` or `also` naming a collection the doc doesn't list;
//! - `blend_merge_parent`: a `parent` naming an object key (`oN`) the doc doesn't list.

use std::collections::{HashMap, HashSet};

use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::session::{Declined, View};

use super::model::{f, key_num, parse_floats, prop, s, EditMode, COLLS, OBJS, OBJ_PROPS, SCENE};
use super::view::address_of;
use crate::anim::VISIBILITY_PATHS;
use crate::mutant;
use crate::scene::key_of;

fn refuse(code: &'static str, message: String) -> Declined {
    Declined { code, message }
}

/// Whether `view`, a doc the kit merged field by field from an outside save, holds the pack's
/// rules (the module's); `Err` names the first one broken.
pub fn check_merged(view: &View) -> Result<(), Declined> {
    if mutant::active("MergedViewRefusesAll") {
        // planted: every merged doc refused, as the kit's default does
        return Err(refuse("blend_merge_name", "planted".into()));
    }
    if mutant::active("MergedViewUnchecked") {
        // planted: every merged doc taken
        return Ok(());
    }
    let keys = |root: &str| -> HashSet<&str> {
        view.list(root)
            .unwrap_or(&[])
            .iter()
            .map(|(k, _)| k.as_str())
            .collect()
    };
    let colls = keys(COLLS);
    let objs_listed = keys(OBJS);
    let props = view.map(OBJ_PROPS);
    let fld = |k: &Key, name: &str| -> Option<&str> {
        props?.get(&prop(k, name)).and_then(Value::as_str)
    };
    let name_max = view
        .map(SCENE)
        .and_then(|m| m.get(s::NAME_MAX))
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<usize>().ok());
    let mut names: HashMap<&str, &Key> = HashMap::new();
    for (k, _) in view.list(OBJS).unwrap_or(&[]) {
        let addr = address_of(OBJS, k);
        let nid = fld(k, f::NID);
        let linked = nid.is_some_and(|n| n.starts_with("@li:"));
        let name = fld(k, f::NAME)
            .ok_or_else(|| refuse("blend_merge_name", format!("{addr} has no name")))?;
        if !linked {
            if let Some(other) = names.insert(name, k) {
                return Err(refuse(
                    "blend_merge_name_taken",
                    format!("{addr} and {} have one name", address_of(OBJS, other)),
                ));
            }
            if let Some(n) = nid {
                if key_of("OB", name.as_bytes(), None).as_deref() != Some(n) {
                    return Err(refuse(
                        "blend_merge_nid",
                        format!("{addr}: its file key isn't its name's"),
                    ));
                }
            }
            if name.is_empty() || name.contains('\0') || name_max.is_some_and(|m| name.len() > m) {
                return Err(refuse(
                    "blend_merge_name",
                    format!("{addr}: its name doesn't fit the file's name field"),
                ));
            }
        }
        let value = |what: &str| refuse("blend_merge_value", format!("{addr}: its {what}"));
        if fld(k, f::XFORM).is_some_and(|x| parse_floats::<9>(x).is_none()) {
            return Err(value("xform isn't nine finite numbers"));
        }
        for bit in [f::HIDDEN, f::NOSELECT, f::NORENDER] {
            if fld(k, bit).is_some_and(|v| v != "0" && v != "1") {
                return Err(value("visibility isn't 0 or 1"));
            }
        }
        if let Some(q) = fld(k, f::QUAT) {
            if parse_floats::<4>(q).is_none() || fld(k, f::ROTMODE) != Some("quaternion") {
                return Err(value(
                    "quaternion isn't four numbers on a quaternion rotation",
                ));
            }
        }
        if fld(k, f::EDIT).is_some_and(|e| EditMode::parse(e).is_none()) {
            return Err(value("edit isn't one of its words"));
        }
        if fld(k, f::VISANIM).is_some_and(|v| {
            v.split(',')
                .any(|b| !VISIBILITY_PATHS.iter().any(|(p, _)| *p == b))
        }) {
            return Err(value("animated bits aren't visibility properties"));
        }
        let held = fld(k, f::COLL)
            .into_iter()
            .chain(fld(k, f::ALSO).into_iter().flat_map(|a| a.split(',')));
        for c in held {
            if !colls.contains(c) {
                return Err(refuse(
                    "blend_merge_holder",
                    format!("{addr}: a collection holding it isn't listed"),
                ));
            }
        }
        if let Some(p) = fld(k, f::PARENT) {
            if key_num(p, 'o').is_some() && !objs_listed.contains(p) {
                return Err(refuse(
                    "blend_merge_parent",
                    format!("{addr}: its parent isn't listed"),
                ));
            }
        }
    }
    Ok(())
}
