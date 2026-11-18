//! `cad-measure`: test tooling for the drawing packs, run in test jobs, never shipped.
//!
//! - `cad-measure drawings OUT` writes the drawings the handle-stability measurements start
//!   from (each with its spliced companion) into `OUT`, and prints each file's name, size and
//!   SHA-256.
//! - `cad-measure header-check DIR` prints, for each `.dxf` in `DIR`, what its header carries
//!   (every variable and its values) and every comment and text value outside the entities'
//!   own text, so a person can check that nothing names a user, a machine or a path.
//! - `cad-measure h1 PAIRS` judges save pairs from any writer: each folder in `PAIRS` holds
//!   `before.dxf`, `after.dxf` and `pair.toml` (`writer`, `writer_version`, `save`, `before_is`,
//!   `supplied_by`). It pairs each entity, layer and block of the save with its original (by
//!   the ground-truth serial our drawings carry, else by content), classifies its handle (kept,
//!   renumbered, reused, dropped, added, ambiguous), prints one row of counts per pair and
//!   class, and a verdict per writer and version. It prints counts and the files' version
//!   stamps only: never names, text or paths from the files.
//! - `cad-measure drawings OUT --referee` also writes the referee set: the sampler and the
//!   nested-blocks drawing at every version, and the block references and leaders drawings.
//! - `cad-measure referee DIR` compares, for each `X.dxf` with an `X.dxf.ezdxf.txt` beside it
//!   (`scripts/referee/ezdxf-summary.py`, a test checker), the pack's reading with the
//!   checker's: entities by handle, kind and layer (by kind alone in a file without handles),
//!   layers, block definitions, block references per block, and each anonymous block that
//!   draws a dynamic block with that block's name. It prints one row per file and fails on any
//!   difference.
//! - `cad-measure speed N…` times reading drawings of `N` entities (tokens, structure, model,
//!   index) and prints the throughput and the peak memory (Linux), from a release build with no
//!   test probes. It prints; it asserts nothing.
//! - `cad-measure referee-draw DIR` compares, for each `X.dxf` with an `X.dxf.ezdxf-draw.txt`
//!   beside it (`scripts/referee/ezdxf-draw-summary.py`, a test checker), the drawing-model
//!   read's resolved values with the checker's: the palette, model space's resolved colour,
//!   linetype, lineweight and linetype scale, block reference maps, text alignment points,
//!   multiline plain text, spline paths (each checker point within the path's tolerance),
//!   wipeout boundaries, hatch pattern lengths and the draw order. It prints one row per file
//!   and fails on any difference.
//! - `cad-measure speed-text N…` is `speed` on drawings half text (single-line texts, left and
//!   centred, and two-line multiline texts), as a drawing's tags, labels and notes are.
//! - `cad-measure draw-speed N…` times the drawing-model read on drawings of `N` entities: the
//!   header and every model-space page in turn with one opened drawing held (one page built at
//!   a time), and a page read by opening the drawing per call; it prints the peak memory of the
//!   held path (Linux) beside the read path's. It prints; it asserts nothing.

mod edits;
mod strict;

use std::path::Path;
use std::process::ExitCode;

use k2_mpc_base::index::Digest;
use k2_mpc_dxf::testgen::{h1_files, H1_NAMES};
use k2_mpc_dxf::{handle, structure, token, Limits};

fn hex(d: &Digest) -> String {
    d.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn drawings(out: &Path, referee: bool) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    if referee {
        // block references and leaders, as the reference reads resolve them
        for (file, spec) in [
            (
                "references-AC1032.dxf",
                k2_mpc_dxf::testgen::references_sampler(),
            ),
            ("mleader-AC1032.dxf", k2_mpc_dxf::testgen::mleader_sampler()),
        ] {
            let bytes = spec.build().bytes;
            std::fs::write(out.join(file), &bytes).map_err(|e| format!("{file}: {e}"))?;
            println!(
                "drawing {file} {} bytes sha256 {}",
                bytes.len(),
                hex(&Digest::of(&bytes))
            );
        }
        // the merge's outputs, as the merge rows make them, for the checker to read too
        use k2_mpc_cad_model::merge::MergeOutcome;
        use k2_mpc_dxf::testgen::{splice_insert, splice_move, splice_value};
        let b = k2_mpc_dxf::testgen::sampler(k2_mpc_dxf::testgen::Ver::R2018).build();
        let (wall, lamp) = (b.handle("wall"), b.handle("lamp"));
        let mut styled = k2_mpc_dxf::testgen::sampler(k2_mpc_dxf::testgen::Ver::R2018);
        styled.opts.eol = k2_mpc_dxf::testgen::Eol::CrLf;
        styled.opts.pad = true;
        let cases = [
            (
                "disjoint",
                splice_move(&b.bytes, wall, 1.0),
                splice_move(&b.bytes, lamp, 2.0),
            ),
            (
                "fields",
                splice_move(&b.bytes, wall, 1.0),
                splice_insert(&b.bytes, wall, 8, 62, "5"),
            ),
            (
                "layer",
                splice_value(&b.bytes, lamp, 8, "A-WALL"),
                splice_move(&b.bytes, lamp, 3.0),
            ),
            (
                "style",
                splice_insert(&splice_move(&b.bytes, wall, 1.0), wall, 8, 62, "3"),
                splice_move(&styled.build().bytes, lamp, 2.0),
            ),
        ];
        for (name, ours, theirs) in cases {
            let MergeOutcome::Merged { bytes, .. } =
                k2_mpc_dxf::merge::merge(&b.bytes, &ours, &theirs)
            else {
                return Err(format!("the {name} merge didn't merge"));
            };
            let file = format!("merge-{name}.dxf");
            std::fs::write(out.join(&file), &bytes).map_err(|e| format!("{file}: {e}"))?;
            println!(
                "drawing {file} {} bytes sha256 {}",
                bytes.len(),
                hex(&Digest::of(&bytes))
            );
        }
        use k2_mpc_dxf::testgen::{nested_blocks, sampler, Ver};
        for v in Ver::ALL {
            for (stem, spec) in [("sampler", sampler(v)), ("nested", nested_blocks(v))] {
                let bytes = spec.build().bytes;
                let file = format!("{stem}-{}.dxf", v.acadver());
                let path = out.join(&file);
                std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                println!(
                    "drawing {file} {} bytes sha256 {}",
                    bytes.len(),
                    hex(&Digest::of(&bytes))
                );
            }
        }
        // the drawing-model read's sampler, at the versions it is written for
        for v in [Ver::R2010, Ver::R2013, Ver::R2018] {
            // the kind no reader knows, and a hand-made multileader, stay out: the checker refuses
            // an entity it can't name or read
            let mut spec = k2_mpc_dxf::testgen::draw_sampler(v);
            spec.model
                .retain(|e| !matches!(e.label.as_deref(), Some("unknown" | "mleader")));
            let bytes = spec.build().bytes;
            let file = format!("draw-{}.dxf", v.acadver());
            let path = out.join(&file);
            std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            println!(
                "drawing {file} {} bytes sha256 {}",
                bytes.len(),
                hex(&Digest::of(&bytes))
            );
        }
    }
    for name in H1_NAMES {
        let files = h1_files(name).ok_or_else(|| format!("no drawing {name}"))?;
        for (file, bytes) in files {
            let path = out.join(&file);
            std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            println!(
                "drawing {file} {} bytes sha256 {}",
                bytes.len(),
                hex(&Digest::of(&bytes))
            );
        }
    }
    Ok(())
}

/// Codes whose values are names a drawing defines for itself (layers, blocks, styles, text),
/// not settings: listed apart from the header.
fn header_check(dir: &Path) -> Result<(), String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "dxf"))
        .collect();
    names.sort();
    let limits = Limits::default();
    for path in names {
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let t = token::tokenize(&bytes, &limits).map_err(|e| format!("{file}: {e}"))?;
        let s = structure::walk(t, &limits).map_err(|e| format!("{file}: {e}"))?;
        println!("== {file}");
        for v in &s.header {
            let vals: Vec<String> = (v.first + 1..v.end)
                .map(|i| format!("{}={}", s.tok.pairs[i].code, s.text(i)))
                .collect();
            println!("  header {} {}", v.name, vals.join(" "));
        }
        for &i in &s.comments {
            println!("  comment {}", s.text(i));
        }
        // every other string value in the file, once each: table names, block names, texts
        let mut strings: Vec<String> = Vec::new();
        for (i, p) in s.tok.pairs.iter().enumerate() {
            let c = p.code;
            let stringy = matches!(c, 1..=4 | 6..=9 | 300..=309 | 410..=419 | 1000 | 1001 | 1003);
            if stringy {
                let v = s.text(i).into_owned();
                if !v.is_empty() && !strings.contains(&v) {
                    strings.push(v);
                }
            }
        }
        println!("  strings {}", strings.join(" | "));
    }
    Ok(())
}

/// One object of a drawing, as the judge pairs it.
struct Obj {
    class: &'static str,
    handle: Option<u64>,
    serial: Option<String>,
    /// What it is, for pairing without a serial: kind, layer and box, or a table entry's name.
    sig: String,
}

fn objects(bytes: &[u8]) -> Result<(Vec<Obj>, String), String> {
    let limits = Limits::default();
    let t = token::tokenize(bytes, &limits).map_err(|e| e.summary())?;
    let s = structure::walk(t, &limits).map_err(|e| e.summary())?;
    let doc = k2_mpc_dxf::convert::doc(&s);
    let mut out = Vec::new();
    for (eu, e) in s.entities.iter().zip(&doc.entities) {
        let mut serial = None;
        let mut app = false;
        for i in eu.head.first..eu.head.end {
            let c = s.tok.pairs[i].code;
            if c == 1001 {
                app = s.text(i) == k2_mpc_dxf::testgen::SERIAL_APP;
            } else if app && c == 1071 {
                serial = Some(s.text(i).into_owned());
            }
        }
        let bx = e.bbox.map_or("-".into(), |b| {
            b.iter()
                .map(|v| format!("{v:.6}"))
                .collect::<Vec<_>>()
                .join(",")
        });
        out.push(Obj {
            class: "entity",
            handle: e.handle,
            serial,
            sig: format!("{}|{}|{bx}", e.kind, e.layer.to_ascii_uppercase()),
        });
    }
    for l in &doc.layers {
        out.push(Obj {
            class: "layer",
            handle: l.handle,
            serial: None,
            sig: l.name.to_ascii_uppercase(),
        });
    }
    for b in &doc.blocks {
        out.push(Obj {
            class: "block",
            handle: b.handle,
            serial: None,
            sig: b.name.to_ascii_uppercase(),
        });
    }
    let stamp = format!(
        "{} handseed {} hmax {}",
        doc.version,
        doc.handseed.map_or("-".into(), handle::norm),
        doc.hmax.map_or("-".into(), handle::norm)
    );
    Ok((out, stamp))
}

/// Counts for one class of one pair.
#[derive(Default)]
struct Counts {
    kept: u64,
    renumbered: u64,
    reused: u64,
    dropped: u64,
    added: u64,
    ambiguous: u64,
}

fn judge(before: &[Obj], after: &[Obj], class: &str) -> Counts {
    use std::collections::BTreeMap;
    let b: Vec<&Obj> = before.iter().filter(|o| o.class == class).collect();
    let a: Vec<&Obj> = after.iter().filter(|o| o.class == class).collect();
    let mut c = Counts::default();
    // the key each object pairs by: its serial where both sides have serials, else its content
    let key = |o: &Obj| match &o.serial {
        Some(s) => format!("serial:{s}"),
        None => format!("sig:{}", o.sig),
    };
    let mut bk: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, o) in b.iter().enumerate() {
        bk.entry(key(o)).or_default().push(i);
    }
    let mut ak: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, o) in a.iter().enumerate() {
        ak.entry(key(o)).or_default().push(i);
    }
    let mut pair_of_after: Vec<Option<usize>> = vec![None; a.len()];
    let mut paired_before = vec![false; b.len()];
    for (k, bi) in &bk {
        let Some(ai) = ak.get(k) else { continue };
        if bi.len() == 1 && ai.len() == 1 {
            pair_of_after[ai[0]] = Some(bi[0]);
            paired_before[bi[0]] = true;
        } else {
            c.ambiguous += (bi.len() + ai.len()) as u64;
        }
    }
    let before_by_handle: BTreeMap<u64, usize> = b
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.handle.map(|h| (h, i)))
        .collect();
    for (ai, o) in a.iter().enumerate() {
        match pair_of_after[ai] {
            Some(bi) if b[bi].handle == o.handle => c.kept += 1,
            Some(_) => c.renumbered += 1,
            None => c.added += 1,
        }
        if let Some(&bi) = o.handle.and_then(|h| before_by_handle.get(&h)) {
            if pair_of_after[ai] != Some(bi) {
                c.reused += 1;
            }
        }
    }
    c.dropped = paired_before.iter().filter(|p| !**p).count() as u64;
    c
}

fn toml(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_owned(), v.trim().trim_matches('"').to_owned()))
        })
        .collect()
}

fn h1(dir: &Path) -> Result<(), String> {
    use std::collections::BTreeMap;
    let mut pairs: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    pairs.sort();
    // per writer and version: (reused, entity renumbered, table renumbered, ground-truth pairs)
    let mut verdicts: BTreeMap<String, [u64; 4]> = BTreeMap::new();
    for p in pairs {
        let meta = toml(&std::fs::read_to_string(p.join("pair.toml")).unwrap_or_default());
        let get = |k: &str| meta.get(k).cloned().unwrap_or_else(|| "-".into());
        let id = p
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let read = |f: &str| -> Result<(Vec<Obj>, String), String> {
            let bytes = std::fs::read(p.join(f)).map_err(|e| format!("{id}/{f}: {e}"))?;
            objects(&bytes).map_err(|e| format!("{id}/{f}: {e}"))
        };
        let (before, sb) = match read("before.dxf") {
            Ok(x) => x,
            Err(e) => {
                println!("h1 pair {id} unreadable: {e}");
                continue;
            }
        };
        let (after, sa) = match read("after.dxf") {
            Ok(x) => x,
            Err(e) => {
                println!("h1 pair {id} unreadable: {e}");
                continue;
            }
        };
        let ground = before.iter().any(|o| o.serial.is_some());
        let writer = format!("{} {}", get("writer"), get("writer_version"));
        let v = verdicts.entry(writer.clone()).or_default();
        if ground {
            v[3] += 1;
        }
        println!(
            "h1 pair {id} writer {writer} save {} kind {} before [{sb}] after [{sa}]",
            get("save"),
            if ground { "ground-truth" } else { "content" }
        );
        for class in ["entity", "layer", "block"] {
            let c = judge(&before, &after, class);
            println!(
                "h1 row {id} {class} kept {} renumbered {} reused {} dropped {} added {} ambiguous {}",
                c.kept, c.renumbered, c.reused, c.dropped, c.added, c.ambiguous
            );
            v[0] += c.reused;
            if class == "entity" {
                v[1] += c.renumbered;
            } else {
                v[2] += c.renumbered;
            }
        }
    }
    for (w, [reused, ent, tab, ground]) in verdicts {
        let verdict = if reused > 0 || ent > 0 {
            "reissues handles: refuse --base, merge and diff across its saves"
        } else if ground == 0 {
            "no handle reissued in content pairs: confirm with ground-truth pairs before proving"
        } else if tab > 0 {
            "keeps entity handles, renumbers tables or blocks"
        } else {
            "keeps every handle"
        };
        println!("h1 verdict {w}: {verdict} (ground-truth pairs {ground})");
    }
    Ok(())
}

/// `cad-measure probe DIR`: each `.dxf` in `DIR`, its structure in counts and names and its
/// `check` findings by code and count, then the set's tags whose values pair up across it;
/// never a value ([`k2_mpc_dxf::probe`], [`k2_mpc_cad_model::probe::probe_set`]). A sheet that
/// doesn't read is named with its refusal's id.
fn probe(dir: &Path) -> Result<(), String> {
    let mut docs: Vec<(String, k2_mpc_cad_model::Doc)> = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dxf")))
        .collect();
    files.sort();
    for path in files {
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(&path).map_err(|e| format!("{name}: {e}"))?;
        match k2_mpc_dxf::probe(&name, &bytes) {
            Ok((text, doc)) => {
                print!("{text}");
                docs.push((name, doc));
            }
            Err(e) => println!("sheet {name} unreadable {}", e.code.id),
        }
    }
    let set: Vec<(&str, &k2_mpc_cad_model::Doc)> =
        docs.iter().map(|(n, d)| (n.as_str(), d)).collect();
    print!("{}", k2_mpc_cad_model::probe::probe_set(&set));
    Ok(())
}

/// `cad-measure dwg-check DIR`: each `.dwg` in `DIR`, its container read and every section read
/// back, in counts: version, pages, gaps, sections, sections read, bytes, and each refusal's id.
/// Fails when a file in the 2004 to 2018 family doesn't read whole.
fn dwg_check(dir: &Path) -> Result<(), String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dwg")))
        .collect();
    files.sort();
    let limits = k2_mpc_dwg::Limits::default();
    let mut failed = 0;
    for path in &files {
        let name = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let c = match k2_mpc_dwg::read_container(&bytes, &limits) {
            Ok(c) => c,
            Err(e) => {
                println!("dwg {name} refused {}", e.code.id);
                continue;
            }
        };
        let (mut read, mut total) = (0usize, 0usize);
        let mut refusals: std::collections::BTreeMap<&str, usize> = Default::default();
        for s in &c.sections {
            match k2_mpc_dwg::read_section(&bytes, &c, &s.name, &limits) {
                Ok(b) => {
                    read += 1;
                    total += b.len();
                }
                Err(e) => *refusals.entry(e.code.id).or_default() += 1,
            }
        }
        if read < c.sections.len() {
            failed += 1;
        }
        let objects = dwg_objects(&bytes, &c, &limits);
        if !objects.ends_with("refusals -") {
            failed += 1;
        }
        let refusals: Vec<String> = refusals.iter().map(|(k, n)| format!("{k}:{n}")).collect();
        println!(
            "dwg {name} version {} maintenance {} pages {} gaps {} sections {} read {read} bytes {total} refusals {}",
            c.version,
            c.maintenance,
            c.pages.len(),
            c.gaps.len(),
            c.sections.len(),
            if refusals.is_empty() { "-".into() } else { refusals.join(",") }
        );
        println!("dwg {name} {objects}");
    }
    println!(
        "dwg-check: {} files, {failed} with a section, an object or a class that didn't read",
        files.len()
    );
    if failed > 0 {
        return Err(format!("{failed} files with something that didn't read"));
    }
    Ok(())
}

/// One file's object map, every object's head and the class list, in counts: `objects N
/// heads-ok M handles-match K classes C refusals ...` (each refusal's id with its count).
fn dwg_objects(bytes: &[u8], c: &k2_mpc_dwg::Container, limits: &k2_mpc_dwg::Limits) -> String {
    use k2_mpc_dwg::{classes, objects, read_section};
    let mut refusals: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut note = |e: k2_mpc_dwg::CadError| *refusals.entry(e.code.id).or_default() += 1;
    let (mut n, mut ok, mut same) = (0usize, 0usize, 0usize);
    match (
        read_section(bytes, c, "AcDb:Handles", limits),
        read_section(bytes, c, "AcDb:AcDbObjects", limits),
    ) {
        (Ok(map), Ok(objs)) => match objects::object_map(&map, limits) {
            Ok(entries) => {
                n = entries.len();
                for e in &entries {
                    match objects::object_head(&objs, e.location, &c.version) {
                        Ok(h) => {
                            ok += 1;
                            same += usize::from(h.handle == e.handle);
                        }
                        Err(err) => note(err),
                    }
                }
            }
            Err(err) => note(err),
        },
        (Err(err), _) | (_, Err(err)) => note(err),
    }
    let classes = match read_section(bytes, c, "AcDb:Classes", limits)
        .and_then(|s| classes::classes(&s, &c.version, c.maintenance))
    {
        Ok(list) => list.len().to_string(),
        Err(err) => {
            let why = format!("- ({})", err.summary());
            note(err);
            why
        }
    };
    let refusals: Vec<String> = refusals.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    format!(
        "objects {n} heads-ok {ok} handles-match {same} classes {classes} refusals {}",
        if refusals.is_empty() {
            "-".into()
        } else {
            refusals.join(",")
        }
    )
}

/// `cad-measure dwg-entities FILE`: one DWG file's objects by kind, and each entity kind's
/// decode in counts: `dwg-kind FILE KIND n N ok K gaps G:C,... refusals ID:C,...` (an entity
/// kind) or `dwg-object FILE KIND n N`, then the process's peak.
fn dwg_entities(path: &Path) -> Result<(), String> {
    use k2_mpc_dwg::{classes, entities, frame, objects, read_section};
    use std::collections::BTreeMap;
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let limits = k2_mpc_dwg::Limits::default();
    let c = k2_mpc_dwg::read_container(&bytes, &limits).map_err(|e| e.summary())?;
    let ver = frame::Ver::of(&c.version).map_err(|e| e.summary())?;
    let map = read_section(&bytes, &c, "AcDb:Handles", &limits).map_err(|e| e.summary())?;
    let objs = read_section(&bytes, &c, "AcDb:AcDbObjects", &limits).map_err(|e| e.summary())?;
    let cls = read_section(&bytes, &c, "AcDb:Classes", &limits)
        .and_then(|s| classes::classes(&s, &c.version, c.maintenance))
        .map_err(|e| e.summary())?;
    let entries = objects::object_map(&map, &limits).map_err(|e| e.summary())?;
    #[derive(Default)]
    struct K {
        name: String,
        n: usize,
        ok: usize,
        entity: bool,
        gaps: BTreeMap<i64, usize>,
        refusals: BTreeMap<&'static str, usize>,
    }
    impl K {
        fn kind_name(&self) -> &str {
            &self.name
        }
    }
    let mut kinds: BTreeMap<String, K> = BTreeMap::new();
    let mut unnamed = 0usize;
    let mut colour_stats: BTreeMap<String, usize> = BTreeMap::new();
    for e in &entries {
        let o = match frame::open(&objs, e.location, ver) {
            Ok(o) => o,
            Err(err) => {
                *kinds
                    .entry("(unopened)".into())
                    .or_default()
                    .refusals
                    .entry(err.code.id)
                    .or_default() += 1;
                continue;
            }
        };
        let Some((kind, is_entity)) = entities::kind_of(o.kind, &cls) else {
            unnamed += 1;
            continue;
        };
        let k = kinds.entry(kind.clone()).or_default();
        k.name = kind;
        k.n += 1;
        k.entity = is_entity;
        if !is_entity {
            use k2_mpc_dwg::tables;
            let mut o = o;
            let r = match k.kind_name() {
                "LAYER" => tables::layer(&mut o).map(|l| {
                    if l.colour.rgb.is_some() {
                        *colour_stats
                            .entry(format!(
                                "layer-true-colour-index-zero:{}",
                                l.colour.index == 0
                            ))
                            .or_default() += 1;
                    }
                }),
                "LTYPE" => tables::ltype(&mut o).map(|_| ()),
                "STYLE" => tables::style(&mut o).map(|_| ()),
                "APPID" => tables::appid(&mut o).map(|_| ()),
                "BLOCK_HEADER" => tables::block_header(&mut o).map(|_| ()),
                "LAYOUT" => tables::layout(&mut o).map(|_| ()),
                "DBCOLOR" => tables::dbcolor(&mut o).map(|_| ()),
                "DIMSTYLE" => tables::dimstyle(&mut o).map(|_| ()),
                "IMAGEDEF" => tables::imagedef(&mut o).map(|_| ()),
                "BLOCK_CONTROL" | "LTYPE_CONTROL" => tables::control(&mut o, 2).map(|_| ()),
                "LAYER_CONTROL" | "STYLE_CONTROL" | "APPID_CONTROL" | "DIMSTYLE_CONTROL" => {
                    tables::control(&mut o, 0).map(|_| ())
                }
                _ => continue,
            };
            match r {
                Ok(()) => k.ok += 1,
                Err(err) => *k.refusals.entry(err.code.id).or_default() += 1,
            }
            continue;
        }
        match entities::entity(&objs, e.location, ver, &cls) {
            Ok(ent) => {
                k.ok += 1;
                let c = &ent.common.colour;
                let t = c
                    .transparency
                    .map_or("none".to_owned(), |t| format!("kind{:X}", t >> 24));
                *colour_stats.entry(format!("transparency:{t}")).or_default() += 1;
                if let Some(v) = c.rgb {
                    *colour_stats
                        .entry(format!("rgb:{:X}", v >> 24))
                        .or_default() += 1;
                }
                if c.book {
                    *colour_stats.entry("book".into()).or_default() += 1;
                }
                if let k2_mpc_dwg::entities::Body::Viewport(v) = &ent.body {
                    *colour_stats
                        .entry(format!(
                            "viewport-on-bit:{}-invisible:{}",
                            v.status & 0x20000 != 0,
                            ent.common.invisible
                        ))
                        .or_default() += 1;
                }
                *colour_stats
                    .entry(format!("eed-apps:{}", ent.eed.len().min(3)))
                    .or_default() += 1;
                // the layer handle names a LAYER object
                let layer_ok = ent
                    .common
                    .layer
                    .and_then(|h| entries.iter().find(|m| m.handle == h))
                    .and_then(|m| frame::open(&objs, m.location, ver).ok())
                    .is_some_and(|l| l.kind == 0x33);
                if !layer_ok {
                    *k.refusals.entry("layer-not-a-layer").or_default() += 1;
                }
            }
            Err(err) => {
                let mut o = frame::open(&objs, e.location, ver).map_err(|e| e.summary())?;
                let stage = match frame::entity_common(&mut o) {
                    Err(_) => "common".to_owned(),
                    Ok(c) => format!(
                        "after-common rgb {} book {} alpha {} graphic-ok",
                        c.colour.rgb.is_some(),
                        c.colour.book,
                        c.colour.transparency.is_some()
                    ),
                };
                println!("dwg-fail {name} {} {stage}", err.code.id);
                *k.refusals.entry(err.code.id).or_default() += 1;
                if let Ok((_, Some(g))) = entities::entity_gap(&objs, e.location, ver, &cls) {
                    *k.gaps.entry(g).or_default() += 1;
                }
            }
        }
    }
    let list = |m: Vec<String>| {
        if m.is_empty() {
            "-".to_owned()
        } else {
            m.join(",")
        }
    };
    let (mut ents, mut ents_ok) = (0usize, 0usize);
    for (kind, k) in &kinds {
        if k.entity {
            ents += k.n;
            ents_ok += k.ok;
            let mut gaps: Vec<(&i64, &usize)> = k.gaps.iter().collect();
            gaps.sort_by(|a, b| b.1.cmp(a.1));
            println!(
                "dwg-kind {name} {kind} n {} ok {} gaps {} refusals {}",
                k.n,
                k.ok,
                list(
                    gaps.iter()
                        .take(4)
                        .map(|(g, c)| format!("{g}:{c}"))
                        .collect()
                ),
                list(k.refusals.iter().map(|(r, c)| format!("{r}:{c}")).collect())
            );
        } else {
            println!(
                "dwg-object {name} {kind} n {} ok {} refusals {}",
                k.n,
                k.ok,
                list(k.refusals.iter().map(|(r, c)| format!("{r}:{c}")).collect())
            );
        }
    }
    let cs: Vec<String> = colour_stats
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    println!("dwg-colours {name} {}", cs.join(" "));
    println!(
        "dwg-entities {name} objects {} entities {ents} decoded {ents_ok} unnamed {unnamed} peak_mib {}",
        entries.len(),
        peak_mib().map_or("n/a".into(), |p| p.to_string())
    );
    Ok(())
}

/// Every row of one space's drawing-model pages.
fn space_rows(
    d: &dyn k2_mpc_cad_model::draw::DrawSource,
    space: &str,
) -> Result<Vec<k2_mpc_base::errors::Value>, String> {
    use k2_mpc_base::errors::Value;
    let mut out = Vec::new();
    let mut args: Vec<String> = vec![
        "--version".into(),
        "1".into(),
        "--space".into(),
        space.into(),
    ];
    loop {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let p = k2_mpc_cad_model::draw::page(d, &a).map_err(|e| e.summary())?;
        if let Some(Value::List(l)) = p.data.get("entities") {
            out.extend(l.iter().cloned());
        }
        match p.next {
            Some(n) => args = n,
            None => return Ok(out),
        }
    }
}

/// The spaces a drawing pages: model space, then every other block record by its address.
fn spaces(d: &dyn k2_mpc_cad_model::draw::DrawSource) -> Vec<String> {
    let doc = d.doc();
    let mut out = vec!["model".to_owned()];
    for (i, b) in doc.blocks.iter().enumerate() {
        if b.kind != k2_mpc_cad_model::BlockKind::Model {
            out.push(doc.block_address(i));
        }
    }
    out
}

/// `cad-measure dwg-draw FILE [--container]`: one DWG file's drawing-model read: its header
/// and every space's pages, in counts (rows, pages, rows not drawn by why), then the
/// process's peak. With `--container`, the container and its sections only (the read before
/// the entities), for the peak.
fn dwg_draw(path: &Path, container_only: bool) -> Result<(), String> {
    use k2_mpc_base::errors::Value;
    use k2_mpc_cad_model::draw::DrawSource;
    let name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let limits = k2_mpc_dwg::Limits::default();
    if container_only {
        let c = k2_mpc_dwg::read_container(&bytes, &limits).map_err(|e| e.summary())?;
        let mut total = 0usize;
        for s in &c.sections {
            total += k2_mpc_dwg::read_section(&bytes, &c, &s.name, &limits)
                .map_err(|e| e.summary())?
                .len();
        }
        println!(
            "dwg-draw {name} container bytes {} sections {total} peak_mib {}",
            bytes.len(),
            peak_mib().map_or("n/a".into(), |p| p.to_string())
        );
        return Ok(());
    }
    {
        let c = k2_mpc_dwg::read_container(&bytes, &limits).map_err(|e| e.summary())?;
        let ver = k2_mpc_dwg::frame::Ver::of(&c.version).map_err(|e| e.summary())?;
        let r = k2_mpc_dwg::read_section(&bytes, &c, "AcDb:Header", &limits)
            .and_then(|s| k2_mpc_dwg::header::header(&s, ver, c.maintenance));
        match r {
            Ok(h) => println!("dwg-header {name} read trailing_bits {}", h.trailing_bits),
            Err(e) => println!("dwg-header {name} refused {}", e.summary()),
        }
    }
    let d = k2_mpc_dwg::drawing::DwgDrawing::open(&bytes, &limits).map_err(|e| e.summary())?;
    let h =
        k2_mpc_cad_model::draw::header(&d, &["--version", "1"], &k2_mpc_cad_model::draw::NoXrefs)
            .map_err(|e| e.summary())?;
    let (mut rows, mut spaces_n) = (0usize, 0usize);
    let mut why: std::collections::BTreeMap<String, usize> = Default::default();
    for sp in spaces(&d) {
        spaces_n += 1;
        for r in space_rows(&d, &sp)? {
            rows += 1;
            if let Some(w) = r.get("why").and_then(Value::as_str) {
                *why.entry(w.to_owned()).or_default() += 1;
            }
        }
    }
    let why: Vec<String> = why.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    let mut block_names: Vec<String> = d
        .doc()
        .blocks
        .iter()
        .map(|b| b.name.to_ascii_uppercase())
        .collect();
    let all = block_names.len();
    block_names.sort();
    block_names.dedup();
    println!(
        "dwg-draw {name} block names repeated {}",
        all - block_names.len()
    );
    println!(
        "dwg-draw {name} bytes {} entities {} layers {} blocks {} spaces {spaces_n} rows {rows} header_bytes {} not-drawn {} peak_mib {}",
        bytes.len(),
        d.doc().entities.len(),
        d.doc().layers.len(),
        d.doc().blocks.len(),
        h.data.to_json().len(),
        if why.is_empty() { "-".into() } else { why.join(",") },
        peak_mib().map_or("n/a".into(), |p| p.to_string())
    );
    Ok(())
}

/// Where two drawing-model values differ: each differing leaf's path (list places as `[]`),
/// numbers equal within a relative 1e-6 (or 1e-9 absolute), closed rings up to a repeated
/// closing point.
fn value_diffs(
    a: &k2_mpc_base::errors::Value,
    b: &k2_mpc_base::errors::Value,
    path: &str,
    out: &mut Vec<String>,
) {
    use k2_mpc_base::errors::Value;
    let num = |v: &Value| match v {
        Value::Int(i) => Some(*i as f64),
        Value::UInt(u) => Some(*u as f64),
        Value::Float(f) => Some(f.get()),
        _ => None,
    };
    if let (Some(x), Some(y)) = (num(a), num(b)) {
        if (x - y).abs() > 1e-9_f64.max(1e-6 * x.abs().max(y.abs())) {
            out.push(format!(
                "{path} (ours {} theirs)",
                if x < y { "<" } else { ">" }
            ));
        }
        return;
    }
    let class = |v: &Value| match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Int(_) | Value::UInt(_) | Value::Float(_) => "num",
        Value::Str(_) => "str",
        Value::List(_) => "list",
        Value::Obj(_) => "obj",
        _ => "other",
    };
    match (a, b) {
        (Value::Obj(x), Value::Obj(y)) => {
            for (k, va) in x {
                match y.iter().find(|(kb, _)| kb == k) {
                    Some((_, vb)) => value_diffs(va, vb, &format!("{path}.{k}"), out),
                    None => out.push(format!("{path}.{k} (ours only)")),
                }
            }
            for (k, _) in y {
                if !x.iter().any(|(ka, _)| ka == k) {
                    out.push(format!("{path}.{k} (theirs only)"));
                }
            }
        }
        (Value::List(x), Value::List(y))
            if !x.is_empty()
                && x.iter()
                    .chain(y.iter())
                    .all(|v| v.get("handle").and_then(Value::as_str).is_some()) =>
        {
            // records with handles (layers, blocks, layouts, linetypes): matched by handle
            for va in x {
                let h = va.get("handle").and_then(Value::as_str);
                match y
                    .iter()
                    .find(|vb| vb.get("handle").and_then(Value::as_str) == h)
                {
                    Some(vb) => value_diffs(va, vb, &format!("{path}[]"), out),
                    None => out.push(format!("{path}[] (ours only)")),
                }
            }
            if y.len() > x.len() {
                out.push(format!("{path}[] (theirs more)"));
            }
        }
        (Value::List(x), Value::List(y)) => {
            // a ring with its first point repeated at its end reads as the ring
            let trim = |l: &[Value]| -> usize {
                if l.len() > 1 && l.first() == l.last() && matches!(l.first(), Some(Value::List(_)))
                {
                    l.len() - 1
                } else {
                    l.len()
                }
            };
            let (n, m) = if x.len() == y.len() {
                (x.len(), y.len())
            } else {
                (trim(x), trim(y))
            };
            if n != m {
                out.push(format!("{path} (lengths)"));
                return;
            }
            for (va, vb) in x.iter().zip(y.iter()).take(n) {
                value_diffs(va, vb, &format!("{path}[]"), out);
            }
        }
        _ => {
            if a != b {
                out.push(format!("{path} (ours {} theirs {})", class(a), class(b)));
            }
        }
    }
}

/// How a differing row differs, in relations and counts only (never a value): a colour's
/// channels swapped or not, a pattern's line counts, a dimension's type number, which side a
/// definition is constant on, a polyline's widths, a background's fields.
fn row_detail(
    kind: &str,
    ours: &k2_mpc_base::errors::Value,
    theirs: &k2_mpc_base::errors::Value,
    diffs: &[String],
) -> Vec<String> {
    use k2_mpc_base::errors::Value;
    let at = |v: &Value, path: &[&str]| -> Option<Value> {
        let mut x = v;
        for k in path {
            x = x.get(k)?;
        }
        Some(x.clone())
    };
    let nums = |v: Option<Value>| -> Vec<f64> {
        match v {
            Some(Value::List(l)) => l
                .iter()
                .filter_map(|x| match x {
                    Value::Int(i) => Some(*i as f64),
                    Value::UInt(u) => Some(*u as f64),
                    Value::Float(f) => Some(f.get()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let len = |v: Option<Value>| match v {
        Some(Value::List(l)) => l.len(),
        _ => 0,
    };
    let keys = |v: Option<Value>| match v {
        Some(Value::Obj(o)) => o
            .iter()
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>()
            .join("+"),
        Some(Value::Null) | None => "none".into(),
        _ => "other".into(),
    };
    let mut out = Vec::new();
    let has = |p: &str| diffs.iter().any(|d| d.starts_with(p));
    if has(".style.rgb") {
        let (a, b) = (
            nums(at(ours, &["style", "rgb"])),
            nums(at(theirs, &["style", "rgb"])),
        );
        let swapped = a.len() == 3 && b.len() == 3 && a[0] == b[2] && a[2] == b[0];
        let gray = b.len() == 3 && b[0] == b[1] && b[1] == b[2];
        out.push(format!("{kind} rgb swapped {swapped} theirs-gray {gray}"));
    }
    if has(".geom.pattern.lines") {
        out.push(format!(
            "{kind} pattern lines ours {} theirs {}",
            len(at(ours, &["geom", "pattern", "lines"])),
            len(at(theirs, &["geom", "pattern", "lines"]))
        ));
    }
    if kind == "DIMENSION" {
        out.push(format!(
            "{kind} type {}",
            at(ours, &["geom", "kind"]).map_or("?".into(), |v| v.to_json())
        ));
    }
    if has(".drawn") || has(".geom (ours only)") {
        out.push(format!(
            "{kind} drawn ours {} theirs {}",
            at(ours, &["drawn"]).map_or("?".into(), |v| v.to_json()),
            at(theirs, &["drawn"]).map_or("?".into(), |v| v.to_json())
        ));
    }
    if has(".geom.verts[].w") {
        out.push(format!(
            "{kind} widths: ours const {} theirs const {}",
            keys(at(ours, &["geom", "const_w"])),
            keys(at(theirs, &["geom", "const_w"]))
        ));
    }
    if kind == "MTEXT" && has(".geom.columns.heights") {
        // relations only (no value leaves the node): how ours and theirs differ, per column
        let (h_a, h_b) = (
            nums(at(ours, &["geom", "columns", "heights"])),
            nums(at(theirs, &["geom", "columns", "heights"])),
        );
        let one = |path: &[&str]| {
            nums(at(ours, path).map(|v| Value::List(vec![v])))
                .first()
                .copied()
        };
        let eq = |x: f64, y: f64| (x - y).abs() <= 1e-9 * (1.0 + x.abs().max(y.abs()));
        let diffs: Vec<f64> = h_a.iter().zip(&h_b).map(|(x, y)| y - x).collect();
        let ratios: Vec<f64> = h_a
            .iter()
            .zip(&h_b)
            .filter(|(x, _)| **x != 0.0)
            .map(|(x, y)| y / x)
            .collect();
        let all_eq = |v: &[f64]| v.windows(2).all(|w| eq(w[0], w[1]));
        let diff_is = |x: Option<f64>| {
            x.is_some_and(|x| !diffs.is_empty() && diffs.iter().all(|d| eq(*d, x)))
        };
        let line_pitch = one(&["geom", "height"]).map(|h| h * 5.0 / 3.0);
        out.push(format!(
            "{kind} column heights: count ours {} theirs {}, ours all equal {}, theirs all equal {}, difference constant {}, ratio constant {}, difference is the gutter {}, the width {}, the text height {}, a line pitch {}, ours zero {}",
            h_a.len(),
            h_b.len(),
            all_eq(&h_a),
            all_eq(&h_b),
            all_eq(&diffs),
            all_eq(&ratios),
            diff_is(one(&["geom", "columns", "gutter"])),
            diff_is(one(&["geom", "columns", "width"])),
            diff_is(one(&["geom", "height"])),
            diff_is(line_pitch),
            h_a.iter().all(|x| *x == 0.0)
        ));
    }
    if kind == "MTEXT" && has(".geom.paragraphs") {
        // counts only: paragraphs, those starting a column, and characters
        let shape = |v: &Value| -> (usize, usize, usize) {
            match at(v, &["geom", "paragraphs"]) {
                Some(Value::List(ps)) => {
                    let breaks = ps
                        .iter()
                        .filter(|p| p.get("column_break") == Some(&Value::Bool(true)))
                        .count();
                    let chars = ps
                        .iter()
                        .filter_map(|p| match p.get("runs") {
                            Some(Value::List(rs)) => Some(
                                rs.iter()
                                    .filter_map(|r| r.get("text").and_then(Value::as_str))
                                    .map(|t| t.chars().count())
                                    .sum::<usize>(),
                            ),
                            _ => None,
                        })
                        .sum();
                    (ps.len(), breaks, chars)
                }
                _ => (0, 0, 0),
            }
        };
        let (a, b) = (shape(ours), shape(theirs));
        out.push(format!(
            "{kind} paragraphs ours {} theirs {}, column breaks ours {} theirs {}, characters ours {} theirs {}",
            a.0, b.0, a.1, b.1, a.2, b.2
        ));
    }
    if has(".geom.rotation") {
        let (a, b) = (
            nums(at(ours, &["geom", "rotation"]).map(|v| Value::List(vec![v]))),
            nums(at(theirs, &["geom", "rotation"]).map(|v| Value::List(vec![v]))),
        );
        let turn = std::f64::consts::TAU;
        let same = a.len() == 1
            && b.len() == 1
            && ((a[0] - b[0]).rem_euclid(turn) < 1e-9
                || turn - (a[0] - b[0]).rem_euclid(turn) < 1e-9);
        let zero = |v: &[f64]| v.first().is_some_and(|x| x.abs() < 1e-12);
        out.push(format!(
            "{kind} rotation equal by whole turns {same} ours-zero {} theirs-zero {} with-columns {}",
            zero(&a),
            zero(&b),
            has(".geom.columns")
        ));
    }
    if has(".geom.columns") {
        out.push(format!(
            "{kind} columns ours {} theirs {}",
            keys(at(ours, &["geom", "columns"])),
            keys(at(theirs, &["geom", "columns"]))
        ));
    }
    if has(".geom.background") {
        out.push(format!(
            "{kind} background ours {} theirs {}",
            keys(at(ours, &["geom", "background"])),
            keys(at(theirs, &["geom", "background"]))
        ));
    }
    out
}

/// Whether row `r` (a DWG read's) has the `read-only` edit every DWG row has: a known,
/// format-specific difference from the DXF read's.
fn read_only_edit(r: &k2_mpc_base::errors::Value) -> bool {
    r.get("edit").and_then(k2_mpc_base::errors::Value::as_str) == Some("read-only")
}

/// Whether block addresses `x` (in `a`) and `y` (in `b`) are both anonymous blocks whose rows
/// are equal but for their handles and owner (and a DWG row's `read-only` edit).
fn same_anonymous(
    a: &dyn k2_mpc_cad_model::draw::DrawSource,
    b: &dyn k2_mpc_cad_model::draw::DrawSource,
    x: &str,
    y: &str,
) -> Result<bool, String> {
    use k2_mpc_base::errors::Value;
    let anon = |s: &dyn k2_mpc_cad_model::draw::DrawSource, addr: &str| {
        (0..s.doc().blocks.len())
            .find(|&i| s.doc().block_address(i) == addr)
            .is_some_and(|i| s.doc().blocks[i].anon)
    };
    if !anon(a, x) || !anon(b, y) {
        return Ok(false);
    }
    // the handle and owner differ by design; so does a DWG row's `read-only` edit
    let strip = |v: &Value, edit: bool| match v {
        Value::Obj(o) => Value::Obj(
            o.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "h" | "owner") && (edit || k != "edit"))
                .cloned()
                .collect(),
        ),
        other => other.clone(),
    };
    let (ra, rb) = (space_rows(a, x)?, space_rows(b, y)?);
    if ra.len() != rb.len() {
        return Ok(false);
    }
    Ok(ra.iter().zip(&rb).all(|(p, q)| {
        let edit = !read_only_edit(p);
        let mut d = Vec::new();
        value_diffs(&strip(p, edit), &strip(q, edit), "", &mut d);
        d.is_empty()
    }))
}

/// The equal-draw row's one tolerance: a hatch's pattern line count may differ by one (a line
/// meeting its boundary), and only where the two reads' raw values are equal within 1e-9.
fn hatch_lines_tolerated(ours: i64, theirs: i64, inputs_equal: bool) -> bool {
    inputs_equal && ours >= 0 && theirs >= 0 && (ours - theirs).abs() <= 1
}

/// Whether the entity at address `h` is the same hatch in both reads' raw values: its pattern
/// lines and its boundary, every number within a relative 1e-9.
fn hatch_inputs_equal(
    a: &dyn k2_mpc_cad_model::draw::DrawSource,
    b: &dyn k2_mpc_cad_model::draw::DrawSource,
    h: &str,
) -> bool {
    use k2_mpc_cad_model::draw::Geom;
    let raw = |s: &dyn k2_mpc_cad_model::draw::DrawSource| {
        let i = (0..s.doc().entities.len()).find(|&i| s.doc().entity_address(i) == h)?;
        match s.entity(i).ok()?.geom {
            Geom::Hatch(x) => Some(x),
            _ => None,
        }
    };
    let (Some(x), Some(y)) = (raw(a), raw(b)) else {
        return false;
    };
    let near = |p: f64, q: f64| (p - q).abs() <= 1e-9 * (1.0 + p.abs().max(q.abs()));
    let all =
        |p: &[f64], q: &[f64]| p.len() == q.len() && p.iter().zip(q).all(|(p, q)| near(*p, *q));
    let defs = x.defs.len() == y.defs.len()
        && x.defs.iter().zip(&y.defs).all(|(p, q)| {
            near(p.angle, q.angle)
                && all(&p.base, &q.base)
                && all(&p.offset, &q.offset)
                && all(&p.dashes, &q.dashes)
        });
    let loops = x.loops.len() == y.loops.len()
        && x.loops.iter().zip(&y.loops).all(|(p, q)| {
            let pts = |l: &k2_mpc_cad_model::draw::LoopRaw| -> Vec<f64> {
                let mut v: Vec<f64> = Vec::new();
                if let Some((ps, _)) = &l.poly {
                    for (pt, bu) in ps {
                        v.extend([pt[0], pt[1], *bu]);
                    }
                }
                for e in &l.edges {
                    if let k2_mpc_cad_model::draw::Edge::Line(s, t) = e {
                        v.extend([s[0], s[1], t[0], t[1]]);
                    }
                }
                v
            };
            p.edges.len() == q.edges.len() && all(&pts(p), &pts(q))
        });
    defs && loops && near(x.angle, y.angle) && near(x.scale, y.scale)
}

/// Kinds the DXF read draws that the DWG read doesn't decode yet: rows of these that the DWG
/// read leaves undrawn are counted as known gaps, not compared.
const DWG_GAP_KINDS: [&str; 0] = [];

/// `cad-measure dwg-draw-compare DWG DXF`: the same drawing as DWG and as DXF, their
/// drawing-model reads compared: every space's rows by handle, and the header, in counts and
/// field paths only (never a value).
///
/// - Known gaps are counted per kind and printed (`draw-gap`), never compared: a kind on the
///   gap list the DWG read leaves undrawn, a dimension's shown text (dimension style settings
///   unread).
/// - A row the DXF lacks whose kind neither read draws is printed (`draw-referee-omits`).
/// - Draw order is compared as each row's place among the rows both sides have.
/// - Anything else that differs fails the run, and so does a row the DWG read leaves undrawn
///   whose kind isn't on the gap list.
fn dwg_draw_compare(dwg: &Path, dxf: &Path) -> Result<(), String> {
    use k2_mpc_base::errors::Value;
    use k2_mpc_cad_model::draw::DrawSource;
    use std::collections::{BTreeMap, BTreeSet};
    let a_bytes = std::fs::read(dwg).map_err(|e| e.to_string())?;
    let b_bytes = std::fs::read(dxf).map_err(|e| e.to_string())?;
    let a = k2_mpc_dwg::drawing::DwgDrawing::open(&a_bytes, &k2_mpc_dwg::Limits::default())
        .map_err(|e| e.summary())?;
    let b = k2_mpc_dxf::draw::DxfDrawing::open(&b_bytes, &Limits::default())
        .map_err(|e| e.summary())?;
    let s = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_owned);
    let (mut rows_a, mut rows_b, mut matched, mut differ, mut only_a, mut only_b) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut paths: BTreeMap<String, usize> = BTreeMap::new();
    let mut gaps: BTreeMap<String, usize> = BTreeMap::new();
    let mut omits: BTreeMap<String, usize> = BTreeMap::new();
    let mut details: BTreeMap<String, usize> = BTreeMap::new();
    let mut tolerated: BTreeMap<String, usize> = BTreeMap::new();
    let mut known_rows: BTreeMap<&str, usize> = BTreeMap::new();
    let mut renamed: BTreeSet<String> = BTreeSet::new();
    let mut renamed_b: BTreeSet<String> = BTreeSet::new();
    let b_spaces = spaces(&b);
    let a_spaces = spaces(&a);
    let mut compared_spaces = 0usize;
    for sp in &a_spaces {
        if !b_spaces.contains(sp) {
            *paths.entry("space (ours only)".into()).or_default() += 1;
            let n = space_rows(&a, sp)?.len();
            let anon = (0..a.doc().blocks.len())
                .find(|&i| a.doc().block_address(i) == *sp)
                .is_some_and(|i| a.doc().blocks[i].anon);
            println!("draw-space ours-only rows {n} anonymous {anon}");
            continue;
        }
        compared_spaces += 1;
        let ra = space_rows(&a, sp)?;
        let rb = space_rows(&b, sp)?;
        rows_a += ra.len();
        rows_b += rb.len();
        let key = |r: &Value| s(r.get("h"));
        let keys_a: BTreeSet<String> = ra.iter().filter_map(key).collect();
        let keys_b: BTreeSet<String> = rb.iter().filter_map(key).collect();
        // each row's place among the rows both sides have
        let place = |rows: &[Value]| -> BTreeMap<String, usize> {
            rows.iter()
                .filter_map(key)
                .filter(|k| keys_a.contains(k) && keys_b.contains(k))
                .enumerate()
                .map(|(i, k)| (k, i))
                .collect()
        };
        let (pa, pb) = (place(&ra), place(&rb));
        let by_b: BTreeMap<String, &Value> =
            rb.iter().filter_map(|r| key(r).map(|k| (k, r))).collect();
        for r in &ra {
            let kind = s(r.get("kind")).unwrap_or_else(|| "?".into());
            let why = s(r.get("why"));
            let Some(k) = key(r) else {
                only_a += 1;
                *paths
                    .entry(format!("row without a handle {kind}"))
                    .or_default() += 1;
                continue;
            };
            let Some(other) = by_b.get(&k) else {
                if why.as_deref() == Some("not_modelled") && !DWG_GAP_KINDS.contains(&kind.as_str())
                {
                    *omits.entry(kind).or_default() += 1;
                } else {
                    only_a += 1;
                    *paths.entry(format!("row (ours only) {kind}")).or_default() += 1;
                }
                continue;
            };
            matched += 1;
            let their_why = s(other.get("why"));
            // a kind on the gap list the DWG read doesn't decode yet
            if DWG_GAP_KINDS.contains(&kind.as_str()) && why.as_deref() == Some("not_modelled") {
                *gaps.entry(kind).or_default() += 1;
                continue;
            }
            let mut d = Vec::new();
            value_diffs(r, other, "", &mut d);
            // known difference, by format, set aside first: a DWG row's edit is `read-only`
            // (DWG has no edit verbs), whatever the DXF verbs would say of the same entity
            if read_only_edit(r) && d.iter().any(|p| p.starts_with(".edit ")) {
                d.retain(|p| !p.starts_with(".edit "));
                *known_rows
                    .entry("a DWG row's edit (read-only: no DWG edit verbs)")
                    .or_default() += 1;
            }
            if why.as_deref() == Some("not_modelled")
                && their_why.as_deref() != Some("not_modelled")
            {
                d.push(".why (ours not decoded, not on the gap list)".into());
            }
            // a dimension's shown text: its dimension style's settings aren't read yet
            if kind == "DIMENSION" && d.iter().any(|p| p.starts_with(".geom.text.shown ")) {
                d.retain(|p| !p.starts_with(".geom.text.shown "));
                *gaps.entry("dimension shown text".into()).or_default() += 1;
            }
            // an anonymous block's name is a label a reader gives: equal when the block is
            let same_block = r.get("geom").and_then(|g| g.get("block"))
                == other.get("geom").and_then(|g| g.get("block"));
            let anon =
                s(r.get("geom").and_then(|g| g.get("name"))).is_some_and(|n| n.starts_with('*'));
            if same_block && anon {
                d.retain(|p| !p.starts_with(".geom.name "));
            }
            // a hatch whose raw values are equal within 1e-9 may draw a pattern line more or
            // fewer where one meets its boundary: within tolerance
            if kind == "HATCH"
                && d.iter()
                    .all(|p| p.starts_with(".geom.pattern.lines (lengths)"))
                && !d.is_empty()
            {
                let n = |x: &Value| match x
                    .get("geom")
                    .and_then(|g| g.get("pattern"))
                    .and_then(|p| p.get("lines"))
                {
                    Some(Value::List(l)) => l.len() as i64,
                    _ => -1,
                };
                if hatch_lines_tolerated(n(r), n(other), hatch_inputs_equal(&a, &b, &k)) {
                    d.clear();
                    *tolerated
                        .entry(
                            "HATCH pattern line at its boundary (raw values equal within 1e-9)"
                                .into(),
                        )
                        .or_default() += 1;
                }
            }
            // a reference to an anonymous block the other file holds under another handle
            if let (Some(x), Some(y)) = (
                s(r.get("geom").and_then(|g| g.get("block"))),
                s(other.get("geom").and_then(|g| g.get("block"))),
            ) {
                if x != y && same_anonymous(&a, &b, &x, &y)? {
                    d.retain(|p| !p.starts_with(".geom.block ") && !p.starts_with(".geom.name "));
                    renamed.insert(x.clone());
                    renamed_b.insert(y.clone());
                    *details
                        .entry("an anonymous block under another handle, its rows equal".into())
                        .or_default() += 1;
                }
            }
            // proposed known difference: a spline the file stores by fit points only, drawn by
            // the drawing model's interpolation, where the other file also carries the control
            // points its program computed
            let len = |x: &Value, k: &str| match x.get("geom").and_then(|g| g.get(k)) {
                Some(Value::List(l)) => l.len(),
                _ => 0,
            };
            if kind == "SPLINE"
                && len(r, "ctrl") == 0
                && len(r, "fit") > 0
                && len(other, "ctrl") > 0
            {
                d.retain(|p| !p.starts_with(".geom.") && !p.starts_with(".bbox"));
                *known_rows.entry("a fit-point spline").or_default() += 1;
            }
            // known difference, narrow: a leader's hook line, which the file doesn't store and
            // the program computes: its DXF save has one vertex more, just before the last, at
            // the arrow size from the last vertex, and is otherwise the same path
            if kind == "LEADER"
                && d.iter().any(|p| p.starts_with(".geom.path"))
                && hook_vertex_only(r, other)
            {
                d.retain(|p| !p.starts_with(".geom.path"));
                *known_rows
                    .entry("a leader's hook line vertex (the program's, not stored)")
                    .or_default() += 1;
            }
            // draw order: the row's place among the rows both sides have
            d.retain(|p| !p.starts_with(".z "));
            if pa.get(&k) != pb.get(&k) {
                d.push(".z (place among shared rows)".into());
            }
            if !d.is_empty() {
                differ += 1;
                d.sort();
                d.dedup();
                for x in row_detail(&kind, r, other, &d) {
                    *details.entry(x).or_default() += 1;
                }
                if kind == "HATCH" {
                    let eq = hatch_inputs_equal(&a, &b, &k);
                    *details
                        .entry(format!("HATCH inputs equal within 1e-9 {eq}"))
                        .or_default() += 1;
                }
                for p in d {
                    *paths.entry(format!("{kind} {p}")).or_default() += 1;
                }
            }
        }
        for r in &rb {
            if key(r).is_some_and(|k| !keys_a.contains(&k)) {
                only_b += 1;
                let kind = s(r.get("kind")).unwrap_or_else(|| "?".into());
                *paths
                    .entry(format!("row (theirs only) {kind}"))
                    .or_default() += 1;
            }
        }
    }
    for sp in &b_spaces {
        if !a_spaces.contains(sp) {
            let n = space_rows(&b, sp)?.len();
            let anon = (0..b.doc().blocks.len())
                .find(|&i| b.doc().block_address(i) == *sp)
                .is_some_and(|i| b.doc().blocks[i].anon);
            println!("draw-space theirs-only rows {n} anonymous {anon}");
            *paths.entry("space (theirs only)".into()).or_default() += 1;
        }
    }
    // spaces matched through their references: not "ours only" / "theirs only"
    for (sp, label) in renamed
        .iter()
        .map(|x| (x, "space (ours only)"))
        .chain(renamed_b.iter().map(|x| (x, "space (theirs only)")))
    {
        let _ = sp;
        if let Some(n) = paths.get_mut(label) {
            *n -= 1;
            if *n == 0 {
                paths.remove(label);
            }
        }
    }
    // the layers whose colour number differs: whether ours is a true colour
    for l in &a.doc().layers {
        if let Some(t) = b.doc().layers.iter().find(|t| t.handle == l.handle) {
            if l.color != t.color {
                let li = a
                    .doc()
                    .layers
                    .iter()
                    .position(|x| x.handle == l.handle)
                    .unwrap_or(0);
                let rgb = a.tables().layers.get(li).and_then(|d| d.rgb).is_some();
                println!("draw-detail layer colour number differs: ours true colour {rgb}");
            }
        }
    }
    let nx = &k2_mpc_cad_model::draw::NoXrefs;
    let ha =
        k2_mpc_cad_model::draw::header(&a, &["--version", "1"], nx).map_err(|e| e.summary())?;
    let hb =
        k2_mpc_cad_model::draw::header(&b, &["--version", "1"], nx).map_err(|e| e.summary())?;
    let mut hd = Vec::new();
    value_diffs(&ha.data, &hb.data, "header", &mut hd);
    // the format's own name, and each format's own digests, differ by design
    hd.retain(|p| !p.starts_with("header.format ") && !p.contains(".digest "));
    hd.sort();
    hd.dedup();
    // named known differences (each counted): a true-colour layer's colour number (the file
    // stores none; ours is the palette's nearest), a viewport's on flag (not in its status
    // bits; not found elsewhere)
    let true_colour_layer_differs = a.doc().layers.iter().enumerate().any(|(i, l)| {
        a.tables().layers.get(i).and_then(|d| d.rgb).is_some()
            && b.doc()
                .layers
                .iter()
                .any(|t| t.handle == l.handle && t.color != l.color)
    });
    let mut known: BTreeMap<&str, usize> = BTreeMap::new();
    hd.retain(|p| {
        if p.starts_with("header.layers[].aci ") && true_colour_layer_differs {
            *known
                .entry("a true-colour layer's colour number")
                .or_default() += 1;
            false
        } else if p.starts_with("header.layouts[].viewports[].on ") {
            *known.entry("a viewport's on flag").or_default() += 1;
            false
        } else {
            true
        }
    });
    known
        .entry("a true-colour layer's colour number")
        .or_default();
    known.entry("a viewport's on flag").or_default();
    for (k, n) in &known {
        println!("draw-known {k} {n}");
    }
    // the header's records of the anonymous block held under another handle
    if !renamed.is_empty() {
        hd.retain(|p| {
            !p.starts_with("header.blocks[] (ours only)") && !p.starts_with("header.blocks[].name ")
        });
    }
    let header_diffs = hd.len();
    for p in hd {
        *paths.entry(p).or_default() += 1;
    }
    // every set-aside class, its count printed every run (0 included), so one that stops
    // being reached shows
    for g in DWG_GAP_KINDS
        .iter()
        .map(|k| k.to_string())
        .chain(["dimension shown text"].map(String::from))
    {
        gaps.entry(g).or_default();
    }
    tolerated
        .entry("HATCH pattern line at its boundary (raw values equal within 1e-9)".into())
        .or_default();
    known_rows.entry("a fit-point spline").or_default();
    known_rows
        .entry("a DWG row's edit (read-only: no DWG edit verbs)")
        .or_default();
    known_rows
        .entry("a leader's hook line vertex (the program's, not stored)")
        .or_default();
    for (g, n) in &gaps {
        println!("draw-gap {g} {n}");
    }
    for (g, n) in &tolerated {
        println!("draw-tolerance {g} {n}");
    }
    for (g, n) in &known_rows {
        println!("draw-known {g} {n}");
    }
    println!(
        "draw-referee-omits (rows) {}",
        omits.values().sum::<usize>()
    );
    for (k, n) in &omits {
        println!("draw-referee-omits {k} {n}");
    }
    for (p, n) in &paths {
        println!("draw-diff {p} {n}");
    }
    for (p, n) in &details {
        println!("draw-detail {p} {n}");
    }
    println!(
        "draw-compare spaces {compared_spaces} rows ours {rows_a} theirs {rows_b} matched {matched} differ {differ} only-ours {only_a} only-theirs {only_b} header-diffs {header_diffs} gaps {}",
        gaps.values().sum::<usize>()
    );
    if differ + only_a + only_b + header_diffs > 0 || matched == 0 {
        return Err("the two reads differ outside the known gaps".into());
    }
    Ok(())
}

/// What a reader sees in one drawing, as the referee compares it.
#[derive(Default, PartialEq, Eq, Debug)]
struct Seen {
    /// (handle, kind, layer), sorted; or, in a file without handles, (0, kind, "").
    entities: Vec<(u64, String, String)>,
    layers: Vec<String>,
    /// Block definitions (not model space or layouts), upper case, sorted.
    blocks: Vec<String>,
    /// Block references per block name, upper case.
    inserts: std::collections::BTreeMap<String, u64>,
    /// Each anonymous block that draws a dynamic block, and that block's name, upper case.
    dynamic: std::collections::BTreeMap<String, String>,
}

fn ours(bytes: &[u8]) -> Result<Seen, String> {
    let doc = k2_mpc_dxf::read_doc(bytes, &Limits::default()).map_err(|e| e.summary())?;
    let keyed = doc.handles != "off";
    let mut s = Seen::default();
    for e in &doc.entities {
        let (h, layer) = match (keyed, e.handle) {
            (true, Some(h)) => (h, e.layer.to_ascii_uppercase()),
            _ => (0, String::new()),
        };
        s.entities.push((h, e.kind.to_string(), layer));
        if let Some(b) = &e.block {
            *s.inserts.entry(b.to_ascii_uppercase()).or_default() += 1;
        }
    }
    s.layers = doc
        .layers
        .iter()
        .map(|l| l.name.to_ascii_uppercase())
        .collect();
    s.blocks = doc
        .blocks
        .iter()
        .filter(|b| !b.name.starts_with('*'))
        .map(|b| b.name.to_ascii_uppercase())
        .collect();
    s.dynamic = doc
        .blocks
        .iter()
        .filter_map(|b| {
            let r = b.dynamic_of?;
            Some((
                b.name.to_ascii_uppercase(),
                doc.blocks[r].name.to_ascii_uppercase(),
            ))
        })
        .collect();
    Ok(s)
}

fn theirs(text: &str, keyed: bool) -> Seen {
    let mut s = Seen::default();
    for line in text.lines() {
        let mut w = line.splitn(2, ' ');
        let (tag, rest) = (w.next().unwrap_or(""), w.next().unwrap_or(""));
        match tag {
            "E" => {
                let mut p = rest.splitn(3, ' ');
                let (h, kind, layer) = (
                    p.next().unwrap_or(""),
                    p.next().unwrap_or(""),
                    p.next().unwrap_or(""),
                );
                if keyed {
                    let h = handle::parse(h.as_bytes()).unwrap_or(u64::MAX);
                    s.entities
                        .push((h, kind.to_owned(), layer.to_ascii_uppercase()));
                } else {
                    s.entities.push((0, kind.to_owned(), String::new()));
                }
            }
            "L" => s.layers.push(rest.to_ascii_uppercase()),
            "B" if !rest.starts_with('*') => s.blocks.push(rest.to_ascii_uppercase()),
            "I" => *s.inserts.entry(rest.to_ascii_uppercase()).or_default() += 1,
            "D" => {
                if let Some((anon, real)) = rest.split_once(' ') {
                    s.dynamic
                        .insert(anon.to_ascii_uppercase(), real.to_ascii_uppercase());
                }
            }
            _ => {}
        }
    }
    s
}

fn referee(dir: &Path) -> Result<(), String> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "dxf"))
        .collect();
    files.sort();
    let (mut compared, mut differ, mut entities) = (0u64, 0u64, 0u64);
    for path in files {
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let summary = dir.join(format!("{file}.ezdxf.txt"));
        let Ok(text) = std::fs::read_to_string(&summary) else {
            println!("referee {file}: no checker summary");
            differ += 1;
            continue;
        };
        if let Some(why) = text.lines().find_map(|l| l.strip_prefix("X ")) {
            println!("referee {file}: REFUSED by the checker: {why}");
            differ += 1;
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{file}: {e}"))?;
        let mut a = ours(&bytes).map_err(|e| format!("{file}: {e}"))?;
        let keyed = a.entities.iter().any(|e| e.0 != 0);
        let mut b = theirs(&text, keyed);
        for s in [&mut a, &mut b] {
            s.entities.sort();
            s.layers.sort();
            s.layers.dedup();
            s.blocks.sort();
        }
        // a reader may add the layers every drawing has; ours lists only the file's
        b.layers
            .retain(|l| a.layers.contains(l) || (l != "0" && l != "DEFPOINTS"));
        compared += 1;
        entities += a.entities.len() as u64;
        let mut what = Vec::new();
        if a.entities != b.entities {
            what.push(format!(
                "entities ours {} theirs {} (first difference: {:?} / {:?})",
                a.entities.len(),
                b.entities.len(),
                a.entities.iter().find(|e| !b.entities.contains(e)),
                b.entities.iter().find(|e| !a.entities.contains(e))
            ));
        }
        if a.layers != b.layers {
            what.push(format!("layers ours {:?} theirs {:?}", a.layers, b.layers));
        }
        if a.blocks != b.blocks {
            what.push(format!("blocks ours {:?} theirs {:?}", a.blocks, b.blocks));
        }
        if a.inserts != b.inserts {
            what.push(format!(
                "inserts ours {:?} theirs {:?}",
                a.inserts, b.inserts
            ));
        }
        if a.dynamic != b.dynamic {
            what.push(format!(
                "dynamic blocks ours {:?} theirs {:?}",
                a.dynamic, b.dynamic
            ));
        }
        if what.is_empty() {
            println!(
                "referee {file}: same ({} entities{}, {} layers, {} blocks, {} block references, {} dynamic)",
                a.entities.len(),
                if keyed { " by handle" } else { " by kind" },
                a.layers.len(),
                a.blocks.len(),
                a.inserts.values().sum::<u64>(),
                a.dynamic.len()
            );
        } else {
            differ += 1;
            println!("referee {file}: DIFFERS: {}", what.join("; "));
        }
    }
    println!("referee: {compared} files compared, {entities} entities, {differ} differ");
    if differ > 0 || compared == 0 {
        return Err(format!(
            "{differ} of {compared} files differ from the checker"
        ));
    }
    Ok(())
}

/// The process's peak resident memory so far, in MiB (Linux; `None` elsewhere).
fn peak_mib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: u64 = s
        .lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1024)
}

fn speed(sizes: &[&str]) -> Result<(), String> {
    speed_of(sizes, k2_mpc_dxf::testgen::sized)
}

fn speed_of(
    sizes: &[&str],
    gen: fn(k2_mpc_dxf::testgen::Ver, usize) -> k2_mpc_dxf::testgen::Spec,
) -> Result<(), String> {
    // each size in a process of its own, so the generator's memory isn't in the reader's peak
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("cad-measure-speed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for n in sizes {
        let n: usize = n.parse().map_err(|_| format!("not a size: {n}"))?;
        let path = dir.join(format!("sized-{n}.dxf"));
        let bytes = gen(k2_mpc_dxf::testgen::Ver::R2018, n).build().bytes;
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        drop(bytes);
        let out = std::process::Command::new(&me)
            .arg("read-speed")
            .arg(&path)
            .output()
            .map_err(|e| e.to_string())?;
        print!("{}", String::from_utf8_lossy(&out.stdout));
        let _ = std::fs::remove_file(&path);
        if !out.status.success() {
            return Err(format!(
                "read-speed {n}: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    let _ = std::fs::remove_dir(&dir);
    Ok(())
}

/// One drawing's read and index, in this process from its start: its peaks are the reader's.
fn read_speed(path: &Path) -> Result<(), String> {
    use std::time::Instant;
    let limits = Limits::default();
    let il = k2_mpc_base::index::IndexLimits::new(10_000_000, 64, 1 << 34, 64 << 10);
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mb = bytes.len() as f64 / (1 << 20) as f64;
    let t0 = Instant::now();
    let t = token::tokenize(&bytes, &limits).map_err(|e| e.summary())?;
    let t1 = Instant::now();
    let s = structure::walk(t, &limits).map_err(|e| e.summary())?;
    let t2 = Instant::now();
    let doc = k2_mpc_dxf::convert::doc(&s);
    let t3 = Instant::now();
    let read_peak = peak_mib();
    drop(s);
    let t4 = Instant::now();
    let d = k2_mpc_cad_model::index::draft(&doc, &il).map_err(|e| e.to_string())?;
    let t5 = Instant::now();
    let index_peak = peak_mib();
    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    let ratio = |p: Option<u64>| p.map_or("n/a".into(), |p| format!("{:.1}", p as f64 / mb));
    let peak = |p: Option<u64>| p.map_or("n/a".into(), |p| p.to_string());
    println!(
        "speed entities {} bytes {} tokens_ms {:.1} structure_ms {:.1} model_ms {:.1} index_ms {:.1} read_mb_per_s {:.1} read_peak_mib {} read_peak_x {} index_peak_mib {} index_peak_x {}",
        doc.entities.len(),
        bytes.len(),
        ms(t0, t1),
        ms(t1, t2),
        ms(t2, t3),
        ms(t4, t5),
        mb / (ms(t0, t3) / 1000.0),
        peak(read_peak),
        ratio(read_peak),
        peak(index_peak),
        ratio(index_peak)
    );
    drop(d);
    Ok(())
}

/// Every row of model space's drawing-model pages.
fn model_rows(
    d: &k2_mpc_dxf::draw::DxfDrawing<'_>,
) -> Result<Vec<k2_mpc_base::errors::Value>, String> {
    use k2_mpc_base::errors::Value;
    let mut out = Vec::new();
    let mut args: Vec<String> = vec![
        "--version".into(),
        "1".into(),
        "--space".into(),
        "model".into(),
    ];
    loop {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let p = k2_mpc_cad_model::draw::page(d, &a).map_err(|e| e.summary())?;
        if let Some(Value::List(l)) = p.data.get("entities") {
            out.extend(l.iter().cloned());
        }
        match p.next {
            Some(n) => args = n,
            None => return Ok(out),
        }
    }
}

fn nums(v: Option<&k2_mpc_base::errors::Value>) -> Vec<f64> {
    match v {
        Some(k2_mpc_base::errors::Value::List(l)) => l.iter().filter_map(|x| x.as_f64()).collect(),
        _ => Vec::new(),
    }
}

/// Whether leader row `theirs` is row `ours` with one vertex more, just before the last, at the
/// arrow's size from the last vertex (a hook line), its path otherwise the same within 1e-9.
fn hook_vertex_only(
    ours: &k2_mpc_base::errors::Value,
    theirs: &k2_mpc_base::errors::Value,
) -> bool {
    let path = |x: &k2_mpc_base::errors::Value| pts(x.get("geom").and_then(|g| g.get("path")));
    let (a, b) = (path(ours), path(theirs));
    let size = theirs
        .get("geom")
        .and_then(|g| g.get("arrow"))
        .and_then(|x| x.get("size"))
        .and_then(|x| x.as_f64());
    let (Some(size), true) = (size, a.len() >= 2 && b.len() == a.len() + 1) else {
        return false;
    };
    let near =
        |p: [f64; 2], q: [f64; 2]| (p[0] - q[0]).abs() <= 1e-9 && (p[1] - q[1]).abs() <= 1e-9;
    let mut rest = b.clone();
    let hook = rest.remove(b.len() - 2);
    let last = b[b.len() - 1];
    let len = ((last[0] - hook[0]).powi(2) + (last[1] - hook[1]).powi(2)).sqrt();
    rest.iter().zip(&a).all(|(p, q)| near(*p, *q)) && (len - size).abs() <= 1e-9 * (1.0 + size)
}

fn pts(v: Option<&k2_mpc_base::errors::Value>) -> Vec<[f64; 2]> {
    match v {
        Some(k2_mpc_base::errors::Value::List(l)) => l
            .iter()
            .map(|p| {
                let n = nums(Some(p));
                [
                    n.first().copied().unwrap_or(f64::NAN),
                    n.get(1).copied().unwrap_or(f64::NAN),
                ]
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs()))
}

fn referee_draw(dir: &Path) -> Result<(), String> {
    use k2_mpc_base::errors::Value;
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "dxf"))
        .collect();
    files.sort();
    println!(
        "referee-draw: read as equal: the checker's STANDARD linetype and our CONTINUOUS; our non-breaking spaces are left out of the multiline plain text compared (the checker's drops them); closed rings equal up to a repeated closing point"
    );
    let (mut compared, mut differ, mut values) = (0u64, 0u64, 0u64);
    for path in files {
        let file = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Ok(text) = std::fs::read_to_string(dir.join(format!("{file}.ezdxf-draw.txt"))) else {
            continue;
        };
        if let Some(why) = text.lines().find_map(|l| l.strip_prefix("X ")) {
            println!("referee-draw {file}: REFUSED by the checker: {why}");
            differ += 1;
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{file}: {e}"))?;
        let d = k2_mpc_dxf::draw::DxfDrawing::open(&bytes, &Limits::default())
            .map_err(|e| e.summary())?;
        let rows = model_rows(&d)?;
        let by_h = |h: &str| {
            let want = format!("@h:{}", h.to_ascii_uppercase());
            rows.iter()
                .find(|r| r.get("h").and_then(Value::as_str) == Some(want.as_str()))
        };
        let mut diffs: Vec<String> = Vec::new();
        let mut counts: std::collections::BTreeMap<&str, u64> = std::collections::BTreeMap::new();
        let mut flat = 0.0;
        for line in text.lines() {
            let w: Vec<&str> = line.split(' ').collect();
            match w.as_slice() {
                ["F", x] => flat = x.parse().unwrap_or(0.0),
                ["P", n, r, g, b] => {
                    let n: i64 = n.parse().unwrap_or(0);
                    let theirs = [
                        r.parse::<u8>().unwrap_or(0),
                        g.parse().unwrap_or(0),
                        b.parse().unwrap_or(0),
                    ];
                    *counts.entry("palette").or_default() += 1;
                    let ours = usize::try_from(n)
                        .ok()
                        .and_then(|n| k2_mpc_cad_model::draw::aci::TABLE.get(n));
                    if ours != Some(&theirs) {
                        diffs.push(format!("colour {n}: ours {ours:?} theirs {theirs:?}"));
                    }
                }
                ["Y", h, "ERROR", why] => {
                    diffs.push(format!("{h}: the checker's resolution failed: {why}"))
                }
                ["Y", h, hex, lt, lw, lts] => {
                    let Some(r) = by_h(h) else { continue };
                    *counts.entry("styles").or_default() += 1;
                    let st = r.get("style");
                    let ours_rgb = nums(st.and_then(|s| s.get("rgb")));
                    let ours_hex = if ours_rgb.len() == 3 {
                        format!(
                            "#{:02x}{:02x}{:02x}",
                            ours_rgb[0] as u8, ours_rgb[1] as u8, ours_rgb[2] as u8
                        )
                    } else {
                        "insert".into()
                    };
                    if ours_hex != *hex {
                        diffs.push(format!("{h} colour: ours {ours_hex} theirs {hex}"));
                    }
                    let ours_lt = st
                        .and_then(|s| s.get("ltype"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    // the checker names its default solid line STANDARD; ours is CONTINUOUS
                    let same_lt = ours_lt.eq_ignore_ascii_case(lt)
                        || (ours_lt.eq_ignore_ascii_case("CONTINUOUS")
                            && lt.eq_ignore_ascii_case("STANDARD"));
                    if !same_lt {
                        diffs.push(format!("{h} linetype: ours {ours_lt} theirs {lt}"));
                    }
                    let ours_lw = st
                        .and_then(|s| s.get("lw_mm"))
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::NAN);
                    if !near(ours_lw, lw.parse().unwrap_or(f64::NAN)) {
                        diffs.push(format!("{h} lineweight: ours {ours_lw} theirs {lw}"));
                    }
                    let ours_lts = st
                        .and_then(|s| s.get("lt_scale"))
                        .and_then(Value::as_f64)
                        .unwrap_or(f64::NAN);
                    if !near(ours_lts, lts.parse().unwrap_or(f64::NAN)) {
                        diffs.push(format!("{h} linetype scale: ours {ours_lts} theirs {lts}"));
                    }
                }
                ["M", h, rest @ ..] => {
                    let Some(r) = by_h(h) else { continue };
                    *counts.entry("matrices").or_default() += 1;
                    let ours = nums(r.get("geom").and_then(|g| g.get("m")));
                    let theirs: Vec<f64> =
                        rest.iter().map(|x| x.parse().unwrap_or(f64::NAN)).collect();
                    if ours.len() != 6 || ours.iter().zip(&theirs).any(|(a, b)| !near(*a, *b)) {
                        diffs.push(format!("{h} matrix: ours {ours:?} theirs {theirs:?}"));
                    }
                }
                ["T", h, align, x1, y1, x2, y2] => {
                    let Some(r) = by_h(h) else { continue };
                    *counts.entry("texts").or_default() += 1;
                    let g = r.get("geom");
                    let p1 = [
                        x1.parse().unwrap_or(f64::NAN),
                        y1.parse().unwrap_or(f64::NAN),
                    ];
                    let p2 = [
                        x2.parse().unwrap_or(f64::NAN),
                        y2.parse().unwrap_or(f64::NAN),
                    ];
                    let field = match *align {
                        "ALIGNED" | "FIT" => "to",
                        _ => "anchor",
                    };
                    let want = if *align == "LEFT" { p1 } else { p2 };
                    let got = g
                        .and_then(|g| g.get(field))
                        .map(|a| nums(Some(a)))
                        .filter(|n| n.len() == 2)
                        .map(|n| [n[0], n[1]]);
                    match got {
                        Some(q) if near(q[0], want[0]) && near(q[1], want[1]) => {}
                        other => {
                            diffs.push(format!("{h} text {align}: ours {other:?} theirs {want:?}"))
                        }
                    }
                }
                ["Q", h, ..] => {
                    let Some(r) = by_h(h) else { continue };
                    *counts.entry("mtexts").or_default() += 1;
                    let theirs: String = line[3 + h.len()..].trim().to_owned();
                    let theirs = theirs
                        .trim_matches('"')
                        .replace("\\n", "\n")
                        .replace("\\u00a0", " ");
                    let ours: Vec<String> = match r.get("geom").and_then(|g| g.get("paragraphs")) {
                        Some(Value::List(ps)) => ps
                            .iter()
                            .map(|p| match p.get("runs") {
                                Some(Value::List(rs)) => rs
                                    .iter()
                                    .filter_map(|r| r.get("text").and_then(Value::as_str))
                                    .collect::<String>(),
                                _ => String::new(),
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    // the checker's plain text drops a non-breaking space (`\~`); ours keeps it, so it is
                    // left out here
                    let ours = ours.join("\n").replace('\u{a0}', "");
                    if ours != theirs {
                        diffs.push(format!(
                            "{h} multiline text: ours {ours:?} theirs {theirs:?}"
                        ));
                    }
                }
                ["S", h, "ERROR", why] => {
                    diffs.push(format!("{h}: the checker's flattening failed: {why}"))
                }
                ["S", h, rest @ ..] => {
                    let Some(r) = by_h(h) else { continue };
                    *counts.entry("splines").or_default() += 1;
                    let g = r.get("geom");
                    let path = pts(g.and_then(|g| g.get("path")));
                    let tol = g.and_then(|g| g.get("tol")).and_then(Value::as_f64);
                    let theirs: Vec<f64> =
                        rest.iter().map(|x| x.parse().unwrap_or(f64::NAN)).collect();
                    let Some(tol) = tol else {
                        // a path through fit points only: not compared with an interpolated curve
                        continue;
                    };
                    let worst = theirs
                        .chunks_exact(2)
                        .map(|p| {
                            path.windows(2)
                                .map(|w| {
                                    k2_mpc_cad_model::geom::to_segment([p[0], p[1]], w[0], w[1])
                                })
                                .fold(f64::INFINITY, f64::min)
                        })
                        .fold(0.0f64, f64::max);
                    if worst > tol * (1.0 + 1e-9) + flat {
                        diffs.push(format!("{h} spline: a checker point is {worst} from our path (tolerance {tol})"));
                    }
                }
                ["W", h, rest @ ..] => {
                    let Some(r) = by_h(h) else { continue };
                    *counts.entry("wipeouts").or_default() += 1;
                    let g = r.get("geom");
                    let outline = g
                        .and_then(|g| g.get("mask"))
                        .or(g.and_then(|g| g.get("clip")));
                    // a closed ring: equal up to a repeated closing point (ours never repeats it)
                    let ring = |mut v: Vec<f64>| {
                        while v.len() > 2 && v[..2] == v[v.len() - 2..] {
                            v.truncate(v.len() - 2);
                        }
                        v
                    };
                    let ours: Vec<f64> =
                        ring(pts(outline).iter().flat_map(|p| [p[0], p[1]]).collect());
                    let theirs: Vec<f64> =
                        ring(rest.iter().map(|x| x.parse().unwrap_or(f64::NAN)).collect());
                    if ours.len() != theirs.len()
                        || ours.iter().zip(&theirs).any(|(a, b)| !near(*a, *b))
                    {
                        diffs.push(format!("{h} wipeout: ours {ours:?} theirs {theirs:?}"));
                    }
                }
                ["H", h, "ERROR", why] => {
                    diffs.push(format!("{h}: the checker's hatching failed: {why}"))
                }
                ["H", h, count, length] => {
                    let Some(r) = by_h(h) else { continue };
                    let pat = r.get("geom").and_then(|g| g.get("pattern"));
                    if pat.and_then(|p| p.get("dense")) == Some(&Value::Bool(true)) {
                        // dense: no lines of ours to compare
                        continue;
                    }
                    *counts.entry("hatches").or_default() += 1;
                    let segs = match pat.and_then(|p| p.get("lines")) {
                        Some(Value::List(l)) => l.iter().map(|s| nums(Some(s))).collect::<Vec<_>>(),
                        _ => Vec::new(),
                    };
                    let ours_len: f64 = segs
                        .iter()
                        .filter(|s| s.len() == 4)
                        .map(|s| (s[2] - s[0]).hypot(s[3] - s[1]))
                        .sum();
                    let theirs_len: f64 = length.parse().unwrap_or(f64::NAN);
                    if (ours_len - theirs_len).abs() > 1e-6 * (1.0 + theirs_len) {
                        diffs.push(format!("{h} hatch: ours {} lines, length {ours_len}; theirs {count} lines, length {theirs_len}", segs.len()));
                    }
                }
                ["O", hs @ ..] => {
                    *counts.entry("order").or_default() += 1;
                    let ours: Vec<String> = rows
                        .iter()
                        .filter_map(|r| r.get("h").and_then(Value::as_str))
                        .map(|s| s.trim_start_matches("@h:").to_owned())
                        .collect();
                    let theirs: Vec<String> = hs.iter().map(|s| s.to_ascii_uppercase()).collect();
                    if ours != theirs {
                        diffs.push(format!("draw order: ours {ours:?} theirs {theirs:?}"));
                    }
                }
                _ => {}
            }
        }
        compared += 1;
        let n: u64 = counts.values().sum();
        values += n;
        let what: Vec<String> = counts.iter().map(|(k, v)| format!("{k} {v}")).collect();
        if diffs.is_empty() {
            println!(
                "referee-draw {file}: same ({n} values: {}; the checker's STANDARD linetype read as CONTINUOUS, our non-breaking spaces left out of the plain text compared, closed rings equal up to a repeated closing point)",
                what.join(", ")
            );
        } else {
            differ += 1;
            println!(
                "referee-draw {file}: DIFFERS in {} of {n} values ({})",
                diffs.len(),
                what.join(", ")
            );
            for x in diffs.iter().take(40) {
                println!("referee-draw {file}:   {x}");
            }
        }
    }
    println!("referee-draw: {compared} files compared, {values} values, {differ} differ");
    if differ > 0 || compared == 0 || values == 0 {
        return Err(format!(
            "{differ} of {compared} files differ from the checker"
        ));
    }
    Ok(())
}

fn draw_speed(sizes: &[&str]) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join(format!("cad-measure-draw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for n in sizes {
        let n: usize = n.parse().map_err(|_| format!("not a size: {n}"))?;
        let path = dir.join(format!("sized-{n}.dxf"));
        let bytes = k2_mpc_dxf::testgen::sized(k2_mpc_dxf::testgen::Ver::R2018, n)
            .build()
            .bytes;
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        drop(bytes);
        let out = std::process::Command::new(&me)
            .arg("draw-speed-one")
            .arg(&path)
            .output()
            .map_err(|e| e.to_string())?;
        print!("{}", String::from_utf8_lossy(&out.stdout));
        let _ = std::fs::remove_file(&path);
        if !out.status.success() {
            return Err(format!(
                "draw-speed {n}: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    let _ = std::fs::remove_dir(&dir);
    Ok(())
}

/// A drawing's drawing-model source: a DWG file's (`dwg`) or a DXF file's.
fn open_drawing(
    b: &[u8],
    dwg: bool,
) -> Result<Box<dyn k2_mpc_cad_model::draw::DrawSource + '_>, String> {
    if dwg {
        let d = k2_mpc_dwg::drawing::DwgDrawing::open(b, &k2_mpc_dwg::Limits::default())
            .map_err(|e| e.summary())?;
        Ok(Box::new(d))
    } else {
        let d =
            k2_mpc_dxf::draw::DxfDrawing::open(b, &Limits::default()).map_err(|e| e.summary())?;
        Ok(Box::new(d))
    }
}

/// One drawing's drawing-model read, in this process from its start: the held path's peak is
/// the opened drawing's plus one page at a time. A DWG file (by its version stamp) is read by
/// the DWG reader; then (`draw-all`) every space is paged too: the whole drawing's read, its
/// wall time the open, the header and every space's pages.
fn draw_speed_one(path: &Path) -> Result<(), String> {
    use k2_mpc_cad_model::draw;
    use std::time::Instant;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let dwg = bytes.starts_with(b"AC10");
    let open = || open_drawing(&bytes, dwg);
    let mb = bytes.len() as f64 / (1 << 20) as f64;
    let t0 = Instant::now();
    let d = open()?;
    let t1 = Instant::now();
    let h =
        draw::header(d.as_ref(), &["--version", "1"], &draw::NoXrefs).map_err(|e| e.summary())?;
    let t2 = Instant::now();
    drop(h);
    let mut args: Vec<String> = vec![
        "--version".into(),
        "1".into(),
        "--space".into(),
        "model".into(),
    ];
    let (mut pages, mut rows, mut max_bytes) = (0u64, 0u64, 0usize);
    loop {
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let p = draw::page(d.as_ref(), &a).map_err(|e| e.summary())?;
        pages += 1;
        if let Some(k2_mpc_base::errors::Value::List(l)) = p.data.get("entities") {
            rows += l.len() as u64;
        }
        max_bytes = max_bytes.max(p.data.to_json().len());
        match p.next {
            Some(n) => args = n,
            None => break,
        }
    }
    let t3 = Instant::now();
    let held_peak = peak_mib();
    // every space, the whole drawing's read
    let (mut all_rows, mut all_spaces) = (0usize, 0usize);
    for sp in spaces(d.as_ref()) {
        all_spaces += 1;
        all_rows += space_rows(d.as_ref(), &sp)?.len();
    }
    let t3b = Instant::now();
    let all_peak = peak_mib();
    drop(d);
    // a page by opening the drawing per call: the first three pages
    let t4 = Instant::now();
    let mut cursor: Option<String> = None;
    let mut stateless = 0u32;
    for _ in 0..3 {
        let d = open()?;
        let mut a = vec!["--version", "1", "--space", "model"];
        if let Some(c) = cursor.as_deref() {
            a.extend(["--cursor", c]);
        }
        let p = draw::page(d.as_ref(), &a).map_err(|e| e.summary())?;
        stateless += 1;
        cursor = p.next.as_ref().and_then(|n| n.last().cloned());
        if cursor.is_none() {
            break;
        }
    }
    let t5 = Instant::now();
    let ms = |a: Instant, b: Instant| (b - a).as_secs_f64() * 1000.0;
    let ratio = |p: Option<u64>| p.map_or("n/a".into(), |p| format!("{:.1}", p as f64 / mb));
    let peak = |p: Option<u64>| p.map_or("n/a".into(), |p| p.to_string());
    println!(
        "draw entities {} bytes {} open_ms {:.1} header_ms {:.1} pages {} rows {} pages_ms {:.1} held_page_ms {:.2} stateless_page_ms {:.1} max_page_bytes {} draw_peak_mib {} draw_peak_x {}",
        rows,
        bytes.len(),
        ms(t0, t1),
        ms(t1, t2),
        pages,
        rows,
        ms(t2, t3),
        ms(t2, t3) / pages.max(1) as f64,
        ms(t4, t5) / f64::from(stateless.max(1)),
        max_bytes,
        peak(held_peak),
        ratio(held_peak)
    );
    println!(
        "draw-all format {} bytes {} spaces {all_spaces} rows {all_rows} open_ms {:.1} all_spaces_ms {:.1} wall_ms {:.1} all_peak_mib {} all_peak_x {}",
        if dwg { "dwg" } else { "dxf" },
        bytes.len(),
        ms(t0, t1),
        ms(t3, t3b),
        ms(t0, t2) + ms(t3, t3b),
        peak(all_peak),
        ratio(all_peak)
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["drawings", out] => drawings(Path::new(out), false),
        ["drawings", out, "--referee"] => drawings(Path::new(out), true),
        ["referee", dir] => referee(Path::new(dir)),
        ["edits", out] => edits::write(Path::new(out)),
        ["edit-referee", dir] => edits::referee(Path::new(dir)),
        ["referee-draw", dir] => referee_draw(Path::new(dir)),
        ["header-check", dir] => header_check(Path::new(dir)),
        ["h1", dir] => h1(Path::new(dir)),
        ["probe", dir] => probe(Path::new(dir)),
        ["dwg-check", dir] => dwg_check(Path::new(dir)),
        ["dwg-entities", file] => dwg_entities(Path::new(file)),
        ["dwg-draw", file] => dwg_draw(Path::new(file), false),
        ["dwg-draw", file, "--container"] => dwg_draw(Path::new(file), true),
        ["dwg-draw-compare", dwg, dxf] => dwg_draw_compare(Path::new(dwg), Path::new(dxf)),
        ["dwg-strict", rest @ ..] => strict::run(rest),
        ["speed", sizes @ ..] if !sizes.is_empty() => speed(sizes),
        ["speed-text", sizes @ ..] if !sizes.is_empty() => {
            speed_of(sizes, k2_mpc_dxf::testgen::sized_text)
        }
        ["read-speed", file] => read_speed(Path::new(file)),
        ["draw-speed", sizes @ ..] if !sizes.is_empty() => draw_speed(sizes),
        ["draw-speed-one", file] => draw_speed_one(Path::new(file)),
        _ => {
            Err("usage: cad-measure drawings OUT | edits OUT | edit-referee DIR | header-check DIR | h1 PAIRS | probe DIR | dwg-check DIR | dwg-entities FILE | dwg-draw FILE [--container] | dwg-draw-compare DWG DXF | dwg-strict [--detail] FILE... | referee DIR | speed N...".into())
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cad-measure: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_mpc_dxf::testgen::{h1 as drawing, splice_move};

    #[test]
    fn the_hatch_tolerance_is_one_line_with_equal_inputs() {
        assert!(
            hatch_lines_tolerated(125, 126, true),
            "one line, equal inputs"
        );
        assert!(hatch_lines_tolerated(147, 146, true), "one line fewer");
        assert!(!hatch_lines_tolerated(125, 127, true), "two lines fail");
        assert!(
            !hatch_lines_tolerated(125, 126, false),
            "inputs that differ fail"
        );
        assert!(
            !hatch_lines_tolerated(-1, 0, true),
            "no pattern on one side fails"
        );
    }

    #[test]
    fn the_judge_sees_a_kept_save_and_a_renumbering_writer() {
        let b = drawing("h1-r2018").expect("a drawing").build();
        let (before, _) = objects(&b.bytes).expect("reads");
        // our own splice keeps every handle
        let spliced = splice_move(&b.bytes, b.handle("move-me"), 1.0);
        let (after, _) = objects(&spliced).expect("reads");
        let c = judge(&before, &after, "entity");
        let n = before.iter().filter(|o| o.class == "entity").count() as u64;
        assert_eq!(
            (c.kept, c.renumbered, c.reused, c.dropped, c.added),
            (n, 0, 0, 0, 0)
        );
        // a writer that numbers by position: DELETE-ME gone, every later entity renumbered,
        // and their old handles reused by others
        let mut s = drawing("h1-r2018").expect("a drawing");
        s.model.remove(2);
        let (after, _) = objects(&s.build().bytes).expect("reads");
        let c = judge(&before, &after, "entity");
        assert_eq!(c.dropped, 1, "DELETE-ME");
        assert!(c.renumbered > 5, "renumbered {}", c.renumbered);
        assert!(c.reused > 5, "reused {}", c.reused);
        assert_eq!(c.kept + c.renumbered, n - 1);
        println!(
            "the_judge: kept save {n} kept; renumbering writer {} renumbered, {} reused",
            c.renumbered, c.reused
        );
    }
}
