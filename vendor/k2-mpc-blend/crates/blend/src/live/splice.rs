//! The live content written back into the file ([`encode_from_paired`]): the carrier the kit
//! hands over is read, the carrier's pairing puts the live keys on its collections and
//! objects in the order the read yields them, that keyed carrier is diffed against the live
//! content by key, and the difference is patched in place by the existing writer
//! ([`crate::write::write`]), every other byte kept.
//!
//! - **Every value comes from the carrier.** The pairing holds keys only; a pairing whose count
//!   differs from the carrier's collections or objects is [`SpliceError::PairingMismatch`], and
//!   nothing is spliced (the kit refuses the write-back, `MPC-E521`, the file as it was).
//! - **Live v1 writes exactly what the writer patches in place:** an object's `xform`
//!   (location, Euler rotation, scale), its three visibility bits, and its name with its `nid`.
//!   Anything else that differs (an object or collection added, removed or moved, another field)
//!   is a [`PackError`]: never written by guess.
//! - **Addresses are the carrier's file keys** (its `nid` before a rename), as the writer takes
//!   them.
//! - **Untouched is byte for byte:** with no difference, the carrier is returned as it is,
//!   compressed or not. A compressed file with a change is stored again whole (the writer's own
//!   rule): its content is the patched file's, its outer bytes differ.
//! - **The result is read again** and must hold the live content under the same pairing.

use std::collections::BTreeMap;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::session::{PackError, Pairing, SpliceError};
use k2_mpc_base::table::CELL_SEP;

use super::model::{
    self, f, parse_floats, LiveLimits, LiveScene, COLLS, COLL_PROPS, OBJS, OBJ_PROPS,
};
use crate::mutant;
use crate::scene::key_of;
use crate::write::{write, Change, Edit};

fn err(m: impl Into<String>) -> SpliceError {
    SpliceError::Pack(PackError(m.into()))
}

fn list<'a>(content: &'a [(String, Neutral)], root: &str) -> Result<&'a [(Key, Value)], String> {
    match content.iter().find(|(r, _)| r == root) {
        Some((_, Neutral::List(l))) => Ok(l),
        _ => Err(format!("the content has no list root {root}")),
    }
}

fn map<'a>(
    content: &'a [(String, Neutral)],
    root: &str,
) -> Result<&'a BTreeMap<String, Value>, String> {
    match content.iter().find(|(r, _)| r == root) {
        Some((_, Neutral::Map(m))) => Ok(m),
        _ => Err(format!("the content has no map root {root}")),
    }
}

/// One element's fields of a composite map: `field -> value`.
fn fields_of<'a>(m: &'a BTreeMap<String, Value>, k: &Key) -> BTreeMap<&'a str, &'a str> {
    let start = format!("{}{CELL_SEP}", k.as_str());
    m.range(start.clone()..)
        .take_while(|(key, _)| key.starts_with(&start))
        .map(|(key, v)| (&key[start.len()..], v.as_str().unwrap_or("")))
        .collect()
}

/// The keys the carrier's units take: the pairing's, checked against the carrier's counts.
fn paired_keys(
    scene: &LiveScene,
    pairing: &Pairing,
    content: &[(String, Neutral)],
) -> Result<(Vec<Key>, Vec<Key>), SpliceError> {
    let ck = pairing
        .keys(COLLS)
        .ok_or_else(|| SpliceError::PairingMismatch(format!("no keys for `{COLLS}`")))?;
    let ok = pairing
        .keys(OBJS)
        .ok_or_else(|| SpliceError::PairingMismatch(format!("no keys for `{OBJS}`")))?;
    if mutant::active("PairingUnchecked") {
        // planted: a pairing that doesn't fit is taken anyway, the keys an import mints
        return Ok(scene.minted_keys());
    }
    if ck.len() != scene.colls.len() || ok.len() != scene.objs.len() {
        return Err(SpliceError::PairingMismatch(format!(
            "{} collection keys and {} object keys for {} collections and {} objects",
            ck.len(),
            ok.len(),
            scene.colls.len(),
            scene.objs.len()
        )));
    }
    if mutant::active("PairByPosition") {
        // planted: the pairing ignored, the keys an import would mint
        return Ok(scene.minted_keys());
    }
    if mutant::active("PairByName") {
        // planted: pair the carrier's objects with the live ones by name
        let live = list(content, OBJS).map_err(err)?;
        let props = map(content, OBJ_PROPS).map_err(err)?;
        let by_name: BTreeMap<&str, &Key> = live
            .iter()
            .filter_map(|(k, _)| Some((fields_of(props, k).get(f::NAME).copied()?, k)))
            .collect();
        let ok = scene
            .objs
            .iter()
            .zip(ok)
            .map(|(o, k)| by_name.get(o.name.as_str()).map_or(k, |x| *x).clone())
            .collect();
        return Ok((ck.to_vec(), ok));
    }
    Ok((ck.to_vec(), ok.to_vec()))
}

/// The writer's edits that make the keyed carrier `old` (under `ok`) into the live `content`, or
/// why it can't be written.
fn diff(
    scene: &LiveScene,
    old: &[(String, Neutral)],
    content: &[(String, Neutral)],
    ok: &[Key],
) -> Result<Vec<Edit>, String> {
    for root in [COLLS, OBJS] {
        let (a, b) = (list(old, root)?, list(content, root)?);
        if a != b {
            let at = a
                .iter()
                .zip(b)
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            let show = |l: &[(Key, Value)]| {
                l.get(at).map_or_else(
                    || "nothing".to_owned(),
                    |(k, v)| format!("{} ({})", k.as_str(), v.as_str().unwrap_or("")),
                )
            };
            return Err(format!(
                "the live `{root}` differ from the file's in their elements, order or kinds \
                 (at {at}: the file's {}, the live {}; {} and {} in all): live v1 adds, removes \
                 and moves none",
                show(a),
                show(b),
                a.len(),
                b.len()
            ));
        }
    }
    if map(old, COLL_PROPS)? != map(content, COLL_PROPS)? {
        return Err("a collection's field differs from the file's: live v1 writes none".into());
    }
    let (op_old, op_new) = (map(old, OBJ_PROPS)?, map(content, OBJ_PROPS)?);
    if op_old == op_new {
        return Ok(Vec::new());
    }
    let mut edits = Vec::new();
    for (o, k) in scene.objs.iter().zip(ok) {
        let (a, b) = (fields_of(op_old, k), fields_of(op_new, k));
        if a == b {
            continue;
        }
        let addr = super::address_of(OBJS, k);
        if o.nid.is_empty() {
            return Err(format!("{addr} has no file key: the writer can't name it"));
        }
        let mut names: Vec<&str> = a.keys().chain(b.keys()).copied().collect();
        names.sort_unstable();
        names.dedup();
        let mut vis: [Option<bool>; 3] = [None; 3];
        for n in names {
            let (x, y) = (a.get(n).copied(), b.get(n).copied());
            if x == y {
                continue;
            }
            match n {
                f::XFORM => {
                    let (Some(x), Some(y)) = (x, y) else {
                        return Err(format!("{addr}: its xform was added or removed"));
                    };
                    let (Some(p), Some(q)) = (parse_floats::<9>(x), parse_floats::<9>(y)) else {
                        return Err(format!("{addr}: an xform isn't nine finite numbers"));
                    };
                    let part = |v: &[f32; 9], i: usize| [v[i], v[i + 1], v[i + 2]];
                    let same = |i: usize| {
                        part(&p, i)
                            .iter()
                            .zip(part(&q, i))
                            .all(|(u, w)| u.to_bits() == w.to_bits())
                    };
                    if !same(0) {
                        edits.push(Edit::new(o.nid.clone(), Change::Location(part(&q, 0))));
                    }
                    if !same(3) {
                        edits.push(Edit::new(o.nid.clone(), Change::Rotation(part(&q, 3))));
                    }
                    if !same(6) {
                        edits.push(Edit::new(o.nid.clone(), Change::Scale(part(&q, 6))));
                    }
                }
                f::HIDDEN | f::NOSELECT | f::NORENDER => {
                    let on = match y {
                        Some("1") => true,
                        Some("0") => false,
                        _ => return Err(format!("{addr}: {n} isn't 0 or 1")),
                    };
                    let i = [f::HIDDEN, f::NOSELECT, f::NORENDER]
                        .iter()
                        .position(|x| *x == n)
                        .expect("one of the three");
                    vis[i] = Some(on);
                }
                f::NAME => {
                    let Some(new) = y else {
                        return Err(format!("{addr}: its name was removed"));
                    };
                    let want = key_of("OB", new.as_bytes(), None);
                    if b.get(f::NID).copied() != want.as_deref() {
                        return Err(format!(
                            "{addr}: renamed, but its nid isn't the new name's file key"
                        ));
                    }
                }
                f::NID => {
                    if a.get(f::NAME) == b.get(f::NAME) {
                        return Err(format!("{addr}: its nid changed without its name"));
                    }
                }
                other => {
                    return Err(format!(
                        "{addr}: its field {other} differs from the file's: live v1 doesn't \
                         write it"
                    ))
                }
            }
        }
        if vis.iter().any(Option::is_some) {
            edits.push(Edit::new(
                o.nid.clone(),
                Change::Visibility {
                    viewport: vis[0],
                    select: vis[1],
                    render: vis[2],
                },
            ));
        }
        if a.get(f::NAME) != b.get(f::NAME) {
            let new = b.get(f::NAME).copied().unwrap_or("");
            edits.push(Edit::new(o.nid.clone(), Change::Name(new.to_owned())));
        }
    }
    Ok(edits)
}

/// The live `content` spliced into `carrier` under `pairing` (the module's rules).
pub fn encode_from_paired(
    carrier: &[u8],
    pairing: &Pairing,
    content: &[(String, Neutral)],
    limits: &LiveLimits,
) -> Result<Vec<u8>, SpliceError> {
    let scene =
        model::read(carrier, limits).map_err(|d| err(format!("the carrier doesn't read: {d}")))?;
    let (ck, ok) = paired_keys(&scene, pairing, content)?;
    let old = scene.to_content(&ck, &ok, (0, 0));
    let edits = diff(&scene, &old, content, &ok).map_err(err)?;
    if edits.is_empty() && !mutant::active("EncodeReserialises") {
        return Ok(carrier.to_vec());
    }
    let written = write(carrier, &edits, &limits.read)
        .map_err(|e| err(format!("the writer refused: {e}")))?;
    // the result holds the live content under the same pairing
    let back = model::read(&written.bytes, limits)
        .map_err(|d| err(format!("the written file doesn't read: {d}")))?;
    let again = back.to_content(&ck, &ok, (0, 0));
    for root in [COLLS, OBJS, COLL_PROPS, OBJ_PROPS] {
        let same = match root {
            COLLS | OBJS => list(&again, root).map_err(err)? == list(content, root).map_err(err)?,
            _ => map(&again, root).map_err(err)? == map(content, root).map_err(err)?,
        };
        if !same {
            return Err(err(format!(
                "the written file doesn't read back as the live content (`{root}`)"
            )));
        }
    }
    Ok(written.bytes)
}
