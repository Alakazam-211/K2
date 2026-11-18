//! `cad-measure dwg-strict [--detail] FILE...`: the DWG reader's strict check, test tooling only.
//! For every object of each file it asks what the reader itself never refuses on:
//!
//! - the kind's read ends exactly at the object's data's end (the string stream's start, else
//!   its flag bit);
//! - after every handle the format names for the kind, the handle stream holds at most the
//!   last byte's padding (data past that is tolerated by the reader and counted here as
//!   `extra`, never failed);
//! - every field of the kinds the reader leaves unread (dictionaries, the placeholder, the
//!   multiline style, the viewport table entry) reads to those same ends, by this check's own
//!   decoders.
//!
//! The header section's variables are read to their data's end by the reader itself (it
//! refuses otherwise). Each file is named by its place in the arguments (`F1`, `F2`, ...), never
//! by its name. Printed: counts per kind; for each table entry's 2007 short, how many
//! entries hold each value and how many of those have the external-reference separator `|`
//! in their name (a count, never the name); the block records read as external references
//! (attached, overlaid); for a block record that fails, its five flag bits as read (counted
//! per pattern); and with `--detail` (for our own files only) each object that fails, by
//! handle, kind and what failed; never a value.

use std::collections::BTreeMap;
use std::path::Path;

use k2_mpc_dwg::frame::{self, object_common, Obj, Ver};
use k2_mpc_dwg::{tables, CadError};

#[derive(Default)]
struct Tally {
    objects: usize,
    ok: usize,
    refused: usize,
    data_end: usize,
    extra: usize,
    not_decoded: usize,
}

/// A table entry's common fields, as the reader reads them (`tables`' private `entry`): from
/// 2007 the short after the name, which it returns.
fn entry(o: &mut Obj<'_>) -> Result<u16, CadError> {
    object_common(o)?;
    o.tv()?;
    if o.ver.strings() {
        return o.d.bs();
    }
    o.d.b()?;
    o.d.bs()?;
    o.d.b()?;
    Ok(0)
}

/// A dictionary (`with_default`: the dictionary with a default entry): its count, cloning flag,
/// hard-owner flag and names; its entries' handles (and the default's).
fn dictionary(o: &mut Obj<'_>, with_default: bool) -> Result<(), CadError> {
    object_common(o)?;
    let n = o.d.bl()? as usize;
    o.d.bs()?;
    o.d.rc()?;
    for _ in 0..n.min(1 << 16) {
        o.tv()?;
    }
    o.finish()?;
    for _ in 0..n.min(1 << 16) {
        o.h()?;
    }
    if with_default {
        o.h()?;
    }
    Ok(())
}

/// A multiline style (2018: each line's linetype by handle).
fn mlinestyle(o: &mut Obj<'_>) -> Result<(), CadError> {
    object_common(o)?;
    o.tv()?;
    o.tv()?;
    o.d.bs()?;
    o.cmc()?;
    o.d.bd()?;
    o.d.bd()?;
    let n = usize::from(o.d.rc()?);
    for _ in 0..n {
        o.d.bd()?;
        o.cmc()?;
        if o.ver < Ver::R2018 {
            o.d.bs()?;
        }
    }
    o.finish()?;
    if o.ver >= Ver::R2018 {
        for _ in 0..n {
            o.h()?;
        }
    }
    Ok(())
}

/// The viewport table entry: its view, its lighting, its corners, its zoom, icon, grid and snap
/// settings, its coordinate system; then its external reference block, background, visual
/// style, sun, named and base coordinate systems.
fn vport(o: &mut Obj<'_>) -> Result<u16, CadError> {
    let state = entry(o)?;
    o.d.bd()?;
    o.d.bd()?;
    o.d.rd2()?;
    o.d.bd3()?;
    o.d.bd3()?;
    for _ in 0..4 {
        o.d.bd()?;
    }
    for _ in 0..4 {
        o.d.b()?;
    }
    o.d.rc()?;
    if o.ver.strings() {
        o.d.b()?;
        o.d.rc()?;
        o.d.bd()?;
        o.d.bd()?;
        o.cmc()?;
    }
    o.d.rd2()?;
    o.d.rd2()?;
    o.d.b()?;
    o.d.bs()?;
    o.d.b()?;
    o.d.bb()?;
    o.d.b()?;
    o.d.rd2()?;
    o.d.b()?;
    o.d.b()?;
    o.d.bs()?;
    o.d.bd()?;
    o.d.rd2()?;
    o.d.rd2()?;
    o.d.b()?;
    o.d.b()?;
    o.d.bd3()?;
    o.d.bd3()?;
    o.d.bd3()?;
    o.d.bd()?;
    o.d.bs()?;
    if o.ver.strings() {
        o.d.bs()?;
        o.d.bs()?;
    }
    o.finish()?;
    o.h()?;
    if o.ver.strings() {
        o.h()?;
        o.h()?;
        o.h()?;
    }
    o.h()?;
    o.h()?;
    Ok(state)
}

/// What a non-entity's read gives this check: a table entry's 2007 short (0 for other kinds),
/// whether its name has the external-reference separator, and for a block record whether it
/// is an attached or an overlaid external reference.
#[derive(Default)]
struct Read {
    state: u16,
    bar: bool,
    xref: bool,
    overlay: bool,
}

/// One non-entity object, read: `Ok(None)` for a kind this check has no read for, else
/// `Ok(Some(read))`; `Err` when the read refuses. The handle stream is left where the read
/// stopped.
fn object(o: &mut Obj<'_>, kind: &str) -> Result<Option<Read>, CadError> {
    let of = |e: &tables::Entry| Read {
        state: e.state,
        bar: e.name.contains('|'),
        ..Read::default()
    };
    let read = match kind {
        "LAYER" => of(&tables::layer(o)?.entry),
        "LTYPE" => of(&tables::ltype(o)?.entry),
        "STYLE" => of(&tables::style(o)?.entry),
        "APPID" => of(&tables::appid(o)?),
        "DIMSTYLE" => of(&tables::dimstyle(o)?.entry),
        "BLOCK_HEADER" => {
            let b = tables::block_header(o)?;
            Read {
                xref: b.xref,
                overlay: b.overlay,
                ..of(&b.entry)
            }
        }
        "VPORT" => Read {
            state: vport(o)?,
            ..Read::default()
        },
        _ => {
            object_kind(o, kind)?;
            Read::default()
        }
    };
    Ok(kind_decoded(kind).then_some(read))
}

/// A block record's five flag bits after its entry (anonymous, attribute definitions,
/// external reference, overlaid, loaded), as `0`/`1` in that order: read again for one that
/// fails, `None` when even they don't read.
fn block_flags(objects: &[u8], location: u64, ver: Ver) -> Option<String> {
    let mut o = frame::open(objects, location, ver).ok()?;
    entry(&mut o).ok()?;
    let mut out = String::new();
    for _ in 0..5 {
        out.push(if o.d.b().ok()? { '1' } else { '0' });
    }
    Some(out)
}

/// Whether `kind` is a table entry (it has the 2007 short).
fn kind_has_state(kind: &str) -> bool {
    matches!(
        kind,
        "LAYER" | "LTYPE" | "STYLE" | "APPID" | "DIMSTYLE" | "BLOCK_HEADER" | "VPORT"
    )
}

/// Whether this check reads `kind` (a non-entity).
fn kind_decoded(kind: &str) -> bool {
    matches!(
        kind,
        "LAYER"
            | "LTYPE"
            | "STYLE"
            | "APPID"
            | "DIMSTYLE"
            | "BLOCK_HEADER"
            | "VPORT"
            | "LAYOUT"
            | "SORTENTSTABLE"
            | "IMAGEDEF"
            | "DICTIONARY"
            | "ACDBDICTIONARYWDFLT"
            | "ACDBPLACEHOLDER"
            | "MLINESTYLE"
    ) || kind.ends_with("_CONTROL")
}

/// The non-entity kinds without a table entry's short.
fn object_kind(o: &mut Obj<'_>, kind: &str) -> Result<(), CadError> {
    match kind {
        "LAYOUT" => tables::layout(o).map(drop)?,
        "SORTENTSTABLE" => tables::sortents(o).map(drop)?,
        "IMAGEDEF" => tables::imagedef(o).map(drop)?,
        "LTYPE_CONTROL" | "BLOCK_CONTROL" => tables::control(o, 2).map(drop)?,
        k if k.ends_with("_CONTROL") => tables::control(o, 0).map(drop)?,
        "DICTIONARY" => dictionary(o, false)?,
        "ACDBDICTIONARYWDFLT" => dictionary(o, true)?,
        "ACDBPLACEHOLDER" => {
            object_common(o)?;
            o.finish()?;
        }
        "MLINESTYLE" => mlinestyle(o)?,
        _ => {}
    }
    Ok(())
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let (detail, files) = match args {
        ["--detail", rest @ ..] => (true, rest),
        rest => (false, rest),
    };
    if files.is_empty() {
        return Err("usage: cad-measure dwg-strict [--detail] FILE...".into());
    }
    let limits = k2_mpc_dwg::Limits::default();
    let mut failed = false;
    for (i, f) in files.iter().enumerate() {
        let path = Path::new(f);
        // each file by its place in the arguments, never by its name
        let name = format!("F{}", i + 1);
        let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let c = k2_mpc_dwg::read_container(&bytes, &limits)
            .map_err(|e| format!("{name}: {}", e.summary()))?;
        let ver = Ver::of(&c.version).map_err(|e| format!("{name}: {}", e.summary()))?;
        let section = |s: &str| k2_mpc_dwg::read_section(&bytes, &c, s, &limits);
        let objects = section("AcDb:AcDbObjects").map_err(|e| e.summary())?;
        let cls = section("AcDb:Classes")
            .and_then(|s| k2_mpc_dwg::classes::classes(&s, &c.version, c.maintenance))
            .map_err(|e| e.summary())?;
        let map = section("AcDb:Handles")
            .and_then(|s| k2_mpc_dwg::objects::object_map(&s, &limits))
            .map_err(|e| e.summary())?;
        let header =
            section("AcDb:Header").and_then(|s| k2_mpc_dwg::header::header(&s, ver, c.maintenance));
        match &header {
            Ok(h) => println!(
                "strict {name} header exact extra-string-bits {}",
                h.extra_string_bits
            ),
            Err(e) => println!("strict {name} header REFUSED {}", e.summary()),
        }
        failed |= header.is_err();
        match k2_mpc_dwg::drawing::DwgDrawing::open(&bytes, &limits) {
            Ok(d) => {
                let t = d.tolerated();
                println!(
                    "strict {name} drawing tolerated header-extra-string-bits {} objects-extra-handles {} entries-left-out {} dimstyles-name-only {} entities-common-only {} entities-left-out {}",
                    t.header_extra_string_bits,
                    t.objects_extra_handles,
                    t.entries_left_out,
                    t.dimstyles_name_only,
                    t.entities_common_only,
                    t.entities_left_out
                );
            }
            Err(e) => {
                failed = true;
                println!("strict {name} drawing REFUSED {}", e.summary());
            }
        }
        let mut by_kind: BTreeMap<String, Tally> = BTreeMap::new();
        // (kind, 2007 short) -> (entries, of them with `|` in the name)
        let mut states: BTreeMap<(String, u16), (usize, usize)> = BTreeMap::new();
        let (mut xrefs, mut overlays) = (0usize, 0usize);
        let mut failed_flags: BTreeMap<String, usize> = BTreeMap::new();
        for e in &map {
            let mut o = match frame::open(&objects, e.location, ver) {
                Ok(o) => o,
                Err(err) => {
                    let t = by_kind.entry("(unopened)".into()).or_default();
                    t.objects += 1;
                    t.refused += 1;
                    failed = true;
                    if detail {
                        println!("strict-detail {:X} (unopened) {}", e.handle, err.summary());
                    }
                    continue;
                }
            };
            let Some((kind, is_entity)) = k2_mpc_dwg::entities::kind_of(o.kind, &cls) else {
                by_kind.entry("(unnamed)".into()).or_default().objects += 1;
                continue;
            };
            // Ok(None): not read here; Ok(Some(extra bits)); Err: refused, or read off its end
            let verdict: Result<Option<usize>, String> = if is_entity {
                match k2_mpc_dwg::entities::entity_ends(&objects, e.location, ver, &cls) {
                    Ok((_, None, _, _)) => Ok(None),
                    Ok((_, Some(g), _, _)) if g != 0 => Err(format!("data-end {g}")),
                    Ok((_, Some(_), left, _)) => Ok(Some(if left >= 8 { left } else { 0 })),
                    Err(err) => Err(format!("refused {}", err.summary())),
                }
            } else {
                match object(&mut o, &kind) {
                    Ok(None) => Ok(None),
                    Ok(Some(r)) => {
                        if o.ver.strings() && kind_has_state(&kind) {
                            let n = states.entry((kind.clone(), r.state)).or_default();
                            n.0 += 1;
                            n.1 += usize::from(r.bar);
                        }
                        xrefs += usize::from(r.xref);
                        overlays += usize::from(r.overlay);
                        Ok(Some(tables::extra_handle_bits(&o)))
                    }
                    Err(err) => {
                        if kind == "BLOCK_HEADER" {
                            let f = block_flags(&objects, e.location, ver)
                                .unwrap_or_else(|| "unread".into());
                            *failed_flags.entry(f).or_default() += 1;
                        }
                        Err(format!("refused {}", err.summary()))
                    }
                }
            };
            let t = by_kind.entry(kind.clone()).or_default();
            t.objects += 1;
            match verdict {
                Ok(None) => t.not_decoded += 1,
                Ok(Some(extra)) => {
                    t.ok += 1;
                    if extra > 0 {
                        t.extra += 1;
                        if detail {
                            println!("strict-detail {:X} {kind} extra {extra}", e.handle);
                        }
                    }
                }
                Err(p) => {
                    if p.starts_with("data-end") {
                        t.data_end += 1;
                    } else {
                        t.refused += 1;
                    }
                    failed = true;
                    if detail {
                        println!("strict-detail {:X} {kind} {p}", e.handle);
                    }
                }
            }
        }
        let mut total = Tally::default();
        for (k, t) in &by_kind {
            println!(
                "strict {name} kind {k} objects {} ok {} refused {} data-end {} extra {} not-decoded {}",
                t.objects, t.ok, t.refused, t.data_end, t.extra, t.not_decoded
            );
            total.objects += t.objects;
            total.ok += t.ok;
            total.refused += t.refused;
            total.data_end += t.data_end;
            total.extra += t.extra;
            total.not_decoded += t.not_decoded;
        }
        for ((k, v), (n, bar)) in &states {
            println!("strict {name} entry-state {k} {v} objects {n} bar {bar}");
        }
        println!("strict {name} block-records xref {xrefs} overlay {overlays}");
        for (f, n) in &failed_flags {
            println!("strict {name} block-record-failed flags {f} objects {n}");
        }
        println!(
            "strict {name} total objects {} ok {} refused {} data-end {} extra {} not-decoded {}",
            total.objects, total.ok, total.refused, total.data_end, total.extra, total.not_decoded
        );
    }
    if failed {
        return Err("objects outside the strict check".into());
    }
    Ok(())
}
