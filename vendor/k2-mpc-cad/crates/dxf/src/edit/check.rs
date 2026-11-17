//! The self-check: before a plan returns, the pack reads its own output with the same reader the
//! index uses and checks it against what the plan meant ([`Intent`]). Six checks, each named by
//! its letter in a refusal (`DXF-E090`), never with a value:
//!
//! - **(a)** the output's pair count is the input's, plus those inserted, minus those removed;
//! - **(b)** every unit the plan didn't name (header variables, table entries, blocks, entities
//!   with their followers, objects) has the input's bytes, keyed as the merge keys them, and the
//!   frame between them is the input's; a unit the plan removed is gone, and a new one is one it
//!   created;
//! - **(c)** each value the plan meant reads back: a pair's value by code and place, an entity's
//!   layer, colour, true colour, linetype, an attribute's value by tag;
//! - **(d)** every handle created is above the input's highest and appears once, and `$HANDSEED`
//!   is above them all;
//! - **(e)** no pointer anywhere names a handle the plan removed;
//! - **(f)** each entity changed or created is one the edit rules allow ([`super::rules`]).

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{CadError, SELF_CHECK};
use crate::merge::{unit_key, unit_map, UnitKey};
use crate::{codes, convert, handle, mutant, structure, token, Limits};

/// What a plan meant, for the self-check.
#[derive(Debug, Clone, Default)]
pub struct Intent {
    /// Entities changed in place, by handle.
    pub changed: BTreeSet<u64>,
    /// Entities created.
    pub created: BTreeSet<u64>,
    /// Followers created with them (a new reference's attributes and end marker): their handles.
    pub minted: BTreeSet<u64>,
    /// Entities removed, with their followers' handles.
    pub removed: BTreeSet<u64>,
    /// Objects changed or removed (a sort table mended, an extension dictionary removed).
    pub objects: BTreeSet<u64>,
    /// Objects removed.
    pub objects_removed: BTreeSet<u64>,
    /// Header variables changed (`$HANDSEED`).
    pub header: BTreeSet<String>,
    /// Table entries changed: their handles (a block record's list of references mended).
    pub entries: BTreeSet<u64>,
    /// Values that must read back.
    pub wants: Vec<Want>,
    /// The input's highest handle, for (d).
    pub hmax: u64,
}

/// One value that must read back in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Want {
    /// The `nth` pair (from 0) of `code` in entity `h`'s pairs (its followers' included) holds
    /// `value`.
    Pair {
        h: u64,
        code: i32,
        nth: usize,
        value: Vec<u8>,
    },
    /// Entity `h`'s layer.
    Layer { h: u64, name: String },
    /// Entity `h`'s colour number as written, or none.
    Color { h: u64, value: Option<String> },
    /// Its true colour as written, or none.
    Rgb { h: u64, value: Option<String> },
    /// Its linetype as written, or none.
    Ltype { h: u64, value: Option<String> },
    /// Entity `h`'s text reads back as `value` (escapes read).
    Text { h: u64, value: String },
    /// Its `nth` attribute (from 0) tagged `tag` holds `value`.
    Attrib {
        h: u64,
        tag: String,
        nth: usize,
        value: String,
    },
}

fn fail(check: &str, why: impl std::fmt::Display) -> CadError {
    CadError::new(
        SELF_CHECK,
        format!("the edit didn't read back as planned: ({check}) {why}"),
        "nothing was written; report it with the file",
    )
}

/// The self-check of `after`, the plan applied to `before` (module docs). `inserted` and
/// `removed` are the pairs the plan inserted and removed.
pub fn self_check(
    before: &[u8],
    after: &[u8],
    intent: &Intent,
    inserted: usize,
    removed: usize,
) -> Result<(), CadError> {
    if mutant::active("SelfCheckSkipped") {
        return Ok(());
    }
    let ub = unit_map(before).map_err(|e| fail("b", format!("the input: {e}")))?;
    let ua = unit_map(after).map_err(|e| fail("b", format!("the output doesn't key: {e}")))?;
    // (a)
    if ua.pairs + removed != ub.pairs + inserted {
        return Err(fail(
            "a",
            format!(
                "{} pairs, where {} were due",
                ua.pairs,
                (ub.pairs + inserted).saturating_sub(removed)
            ),
        ));
    }
    // (b)
    let ent = |h: u64| unit_key(UnitKey::Entity(h));
    let obj = |h: u64| unit_key(UnitKey::Object(h));
    let mut named: BTreeSet<String> = BTreeSet::new();
    for &h in intent.changed.iter().chain(&intent.removed) {
        named.insert(ent(h));
    }
    for &h in intent.objects.iter().chain(&intent.objects_removed) {
        named.insert(obj(h));
    }
    for v in &intent.header {
        named.insert(unit_key(UnitKey::Header(v)));
    }
    // a table entry's key ends with its handle: `Entry("BLOCK_RECORD", "h:1F")`
    let entries: Vec<String> = intent
        .entries
        .iter()
        .map(|h| format!("\"h:{}\")", handle::norm(*h)))
        .collect();
    let is_entry =
        |k: &str| k.starts_with("Entry(") && entries.iter().any(|e| k.ends_with(e.as_str()));
    for (k, b) in &ub.by_key {
        if named.contains(k) || is_entry(k) {
            continue;
        }
        match ua.by_key.get(k) {
            Some(a) if a == b => {}
            Some(_) => return Err(fail("b", format!("{} changed, outside the plan", label(k)))),
            None => return Err(fail("b", format!("{} went, outside the plan", label(k)))),
        }
    }
    let created: BTreeSet<String> = intent.created.iter().map(|&h| ent(h)).collect();
    for k in ua.by_key.keys() {
        if !ub.by_key.contains_key(k) && !created.contains(k) {
            return Err(fail("b", format!("{} is new, outside the plan", label(k))));
        }
    }
    for k in &created {
        if !ua.by_key.contains_key(k) {
            return Err(fail("b", format!("{} wasn't written", label(k))));
        }
    }
    for &h in intent
        .removed
        .iter()
        .filter(|h| ub.by_key.contains_key(&ent(**h)))
    {
        if ua.by_key.contains_key(&ent(h)) {
            return Err(fail("b", format!("@h:{h:X} is still there")));
        }
    }
    for &h in &intent.objects_removed {
        if ua.by_key.contains_key(&obj(h)) {
            return Err(fail("b", format!("object @h:{h:X} is still there")));
        }
    }
    if ua.frame != ub.frame {
        return Err(fail("b", "the frame between the units changed"));
    }
    // the output, read as the index reads it
    let limits = Limits::default();
    let t = token::tokenize(after, &limits).map_err(|e| fail("c", e.summary()))?;
    let s = structure::walk(t, &limits).map_err(|e| fail("c", e.summary()))?;
    let doc = convert::doc(&s);
    let by_h: BTreeMap<u64, usize> = doc
        .entities
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.handle.map(|h| (h, i)))
        .collect();
    let find = |h: u64| {
        by_h.get(&h)
            .copied()
            .ok_or_else(|| fail("c", format!("@h:{h:X} isn't in the output")))
    };
    // (c)
    for w in &intent.wants {
        match w {
            Want::Pair {
                h,
                code,
                nth,
                value,
            } => {
                let eu = &s.entities[find(*h)?];
                let got = std::iter::once(&eu.head)
                    .chain(&eu.followers)
                    .flat_map(|u| u.first..u.end)
                    .filter(|&p| s.tok.pairs[p].code == *code)
                    .nth(*nth)
                    .map(|p| s.tok.value(p));
                if got != Some(value.as_slice()) {
                    return Err(fail(
                        "c",
                        format!("@h:{h:X}'s pair {code} #{nth} isn't the value planned"),
                    ));
                }
            }
            Want::Layer { h, name } => {
                if &*doc.entities[find(*h)?].layer != name.as_str() {
                    return Err(fail("c", format!("@h:{h:X}'s layer isn't the one planned")));
                }
            }
            Want::Color { h, value } | Want::Rgb { h, value } | Want::Ltype { h, value } => {
                let e = &doc.entities[find(*h)?];
                let p = e.props.as_deref();
                let (got, what) = match w {
                    Want::Color { .. } => (p.and_then(|p| p.color.as_deref()), "colour"),
                    Want::Rgb { .. } => (p.and_then(|p| p.rgb.as_deref()), "true colour"),
                    _ => (p.and_then(|p| p.ltype.as_deref()), "linetype"),
                };
                if got.map(str::trim) != value.as_deref() {
                    return Err(fail(
                        "c",
                        format!("@h:{h:X}'s {what} isn't the one planned"),
                    ));
                }
            }
            Want::Text { h, value } => {
                if doc.entities[find(*h)?].text.as_deref() != Some(value.as_str()) {
                    return Err(fail(
                        "c",
                        format!("@h:{h:X}'s text isn't the value planned"),
                    ));
                }
            }
            Want::Attrib { h, tag, nth, value } => {
                let e = &doc.entities[find(*h)?];
                let got = e
                    .extra
                    .as_deref()
                    .and_then(|x| {
                        x.attribs
                            .iter()
                            .filter(|a| a.tag.eq_ignore_ascii_case(tag))
                            .nth(*nth)
                    })
                    .map(|a| &*a.value);
                if got != Some(value.as_str()) {
                    return Err(fail(
                        "c",
                        format!("@h:{h:X}'s attribute {tag} isn't the value planned"),
                    ));
                }
            }
        }
    }
    // (d)
    let mut seen: BTreeMap<u64, u32> = BTreeMap::new();
    for p in 0..s.tok.pairs.len() {
        if matches!(s.tok.pairs[p].code, 5 | 105) {
            if let Some(h) = handle::parse(s.tok.value(p)) {
                *seen.entry(h).or_default() += 1;
            }
        }
    }
    if let Some(&top) = intent.created.iter().chain(&intent.minted).max() {
        for &h in intent.created.iter().chain(&intent.minted) {
            if h <= intent.hmax {
                return Err(fail(
                    "d",
                    format!("@h:{h:X} isn't above the drawing's highest handle"),
                ));
            }
            if seen.get(&h).copied().unwrap_or(0) != 1 {
                return Err(fail("d", format!("@h:{h:X} isn't written once")));
            }
        }
        if let Some(seed) = s
            .var_value("$HANDSEED", 5)
            .and_then(|v| handle::parse(v.as_bytes()))
        {
            if seed <= top {
                return Err(fail("d", "$HANDSEED isn't above the handles created"));
            }
        }
    }
    // (e)
    let gone: BTreeSet<u64> = intent
        .removed
        .iter()
        .chain(&intent.objects_removed)
        .copied()
        .collect();
    if !gone.is_empty() {
        for p in 0..s.tok.pairs.len() {
            if codes::is_pointer(s.tok.pairs[p].code) {
                if let Some(h) = handle::parse(s.tok.value(p)).filter(|h| gone.contains(h)) {
                    return Err(fail("e", format!("a pointer still names @h:{h:X}")));
                }
            }
        }
    }
    // (f)
    for &h in intent.changed.iter().chain(&intent.created) {
        let i = find(h)?;
        super::rules::entity(&s, &doc, i).map_err(|e| {
            fail(
                "f",
                format!("@h:{h:X} isn't one the edit rules allow ({})", e.code.name),
            )
        })?;
    }
    Ok(())
}

/// A unit key as a refusal names it: an entity's or object's address, else the key's kind.
fn label(k: &str) -> String {
    for (pre, what) in [("Entity(", ""), ("Object(", "object ")] {
        if let Some(n) = k.strip_prefix(pre).and_then(|r| r.strip_suffix(')')) {
            if let Ok(h) = n.parse::<u64>() {
                return format!("{what}@h:{h:X}");
            }
        }
    }
    match k.split('(').next() {
        Some("Header") => "a header variable".into(),
        Some("Entry") => "a table entry".into(),
        Some("TableHead") => "a table's head".into(),
        Some(_) | None => "a block".into(),
    }
}
