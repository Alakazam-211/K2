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
//!   (location, Euler rotation, scale), its three visibility bits, and its name with its `nid`,
//!   by the live rules ([`super::check`], the ones the kit's edit check asks of every live
//!   change before it applies): anything else that differs (an object or collection added,
//!   removed or moved, another field, a driven transform, a visibility with an animated bit, a
//!   rotation on an object that rotates another way, an object nothing writes, a name the file
//!   can't hold) is a [`PackError`] whose text starts with the rule's registered code when it
//!   has one (`blend_driven: `): never written by guess. A writer's refusal past them reads the same way for a driven channel
//!   (`blend_driven: location[2] is driven (...)`), as `the writer refused: ...` for any other.
//! - **Addresses are the carrier's file keys** (its `nid` before a rename), as the writer takes
//!   them.
//! - **Untouched is byte for byte:** with no difference, the carrier is returned as it is,
//!   compressed or not. A compressed file with a change is stored again whole (the writer's own
//!   rule): its content is the patched file's, its outer bytes differ.
//! - **The result is read again** and must hold the live content under the same pairing.
//! - **A file of a release read but not written would be read-only** ([`model::writes_back`]):
//!   untouched, it is the carrier; any change is a [`PackError`] whose text starts with
//!   `live_read_only: ` ([`super::errors::READ_ONLY`]), and nothing is written. The writer writes
//!   every release read, so no file is one.

use std::collections::BTreeMap;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Key, Value};
use k2_mpc_base::session::{PackError, Pairing, SpliceError};

use super::check::{self, fields_of};
use super::model::{self, f, LiveLimits, LiveScene, COLLS, COLL_PROPS, OBJS, OBJ_PROPS, SCENE};
use crate::mutant;
use crate::write::{write, WriteError};

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

/// The change from the keyed carrier `old` to the live `content`, as the live rules take it
/// ([`check`]), or why `content` can't be read as one.
fn changes<'a>(
    old: &'a [(String, Neutral)],
    content: &'a [(String, Neutral)],
) -> Result<check::Change<'a>, String> {
    let live = list(old, OBJS)?;
    let props = map(old, OBJ_PROPS)?;
    let mut c = check::Change {
        list: None,
        colls: false,
        objs: Vec::new(),
        live,
        props,
        name_max: check::name_max(map(old, SCENE).ok()),
    };
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
            c.list = Some(format!(
                "the live `{root}` differ from the file's in their elements, order or kinds \
                 (at {at}: the file's {}, the live {}; {} and {} in all): live v1 adds, removes \
                 and moves none",
                show(a),
                show(b),
                a.len(),
                b.len()
            ));
            return Ok(c);
        }
    }
    if map(old, COLL_PROPS)? != map(content, COLL_PROPS)? {
        c.colls = true;
        return Ok(c);
    }
    let op_new = map(content, OBJ_PROPS)?;
    if props != op_new {
        for (k, _) in live {
            let (before, after) = (fields_of(props, k), fields_of(op_new, k));
            if before != after {
                c.objs.push(check::Touched {
                    key: k,
                    before,
                    after,
                });
            }
        }
    }
    Ok(c)
}

/// The writer's refusal as the write-back's `PackError` text: a driven channel's with its
/// registered code first (`blend_driven: location[2] is driven (...)`), as the live rules'
/// refusals read; any other in the writer's words after `the writer refused: ` (each has a
/// registered code as a file verb's refusal, [`crate::verbs::VerbError::entry`], none written
/// here: the live rules refuse those changes first).
pub(crate) fn writer_refused(e: &WriteError) -> String {
    match e {
        WriteError::Driven { .. } => format!("{}: {e}", super::errors::DRIVEN.name),
        _ => format!("the writer refused: {e}"),
    }
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
    let version = scene.version();
    // the live rules, one for the write-back and the kit's edit check (`check_edit`); a file of a
    // release read but not written: untouched is the carrier, any change refused with the code
    // first (no release read is one: the writer writes them all)
    let edits = match changes(&old, content) {
        Ok(c) => check::judge(version, &c).map_err(|r| r.text()),
        Err(why) => Err(check::read_only(version).map_or(why, |r| r.text())),
    };
    let edits = edits.map_err(err)?;
    if edits.is_empty() && !mutant::active("EncodeReserialises") {
        return Ok(carrier.to_vec());
    }
    let written = write(carrier, &edits, &limits.read).map_err(|e| err(writer_refused(&e)))?;
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

#[cfg(test)]
mod tests {
    use super::writer_refused;
    use crate::write::WriteError;

    /// The writer's driven refusal reaches the kit with its registered code first
    /// (`blend_driven: `), as the read-only refusal does; another refusal keeps the writer's
    /// words after `the writer refused: `.
    #[test]
    fn a_driven_refusal_carries_its_code() {
        let driven = WriteError::Driven {
            channel: "location[2]".into(),
            why: "an action drives it".into(),
        };
        let text = writer_refused(&driven);
        assert!(
            text.starts_with("blend_driven: location[2] is driven (an action drives it)"),
            "{text}"
        );
        let mode = WriteError::RotationMode {
            mode: "quaternion".into(),
        };
        let text = writer_refused(&mode);
        assert_eq!(
            text,
            "the writer refused: the object rotates by quaternion, not by Euler angles"
        );
    }
}
