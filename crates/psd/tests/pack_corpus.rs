//! The pack on the public test corpus (fetched by reference, `scripts/fetch-corpus.sh`): each
//! file streamed from disk into a draft, checked against the full reader's own tree, and, when
//! `PSD_TOOLS_ORACLE` names a folder of psd-tools' trees, against psd-tools node for node (the
//! test checker that writes them is on the repository's main branch; a release tag and a vendored
//! copy leave it out). Prints counts
//! only. Skips, loudly, when `PSD_PUBLIC_CORPUS` is not set; a listed file that is missing
//! fails, and so does a missing oracle tree when the oracle is set.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::Path;

use k2_mpc_base::index::{IndexDraft, IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_psd::differential::compare_index_with_full;
use k2_mpc_psd::index_psd;

/// psd-tools' kind, in the pack's words.
fn kind_of(psd_tools: &str) -> &str {
    match psd_tools {
        "group" => "group",
        "artboard" => "artboard",
        "pixel" => "raster",
        "type" => "text",
        "smartobject" => "smart",
        "shape" => "shape",
        "solidcolorfill" | "gradientfill" | "patternfill" => "fill",
        _ => "adjustment",
    }
}

/// Each node's path: its position among its siblings, bottom first (psd-tools' order), per
/// level from the root, joined by `/`.
fn paths(d: &IndexDraft) -> Vec<String> {
    let mut siblings: HashMap<Option<u32>, u32> = HashMap::new();
    for n in &d.nodes {
        *siblings.entry(n.parent).or_default() += 1;
    }
    let mut seen: HashMap<Option<u32>, u32> = HashMap::new();
    let mut out: Vec<String> = Vec::with_capacity(d.nodes.len());
    for n in &d.nodes {
        let o = seen.entry(n.parent).or_default();
        let bottom_first = siblings[&n.parent] - 1 - *o;
        *o += 1;
        let p = match n.parent {
            None => bottom_first.to_string(),
            Some(i) => format!("{}/{bottom_first}", out[i as usize]),
        };
        out.push(p);
    }
    out
}

fn unhex(s: &str) -> Option<String> {
    let b: Option<Vec<u8>> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok())
        .collect();
    String::from_utf8(b?).ok()
}

/// Where the pack and psd-tools differ, per field: how many nodes, out of how many compared.
#[derive(Default)]
struct Tally(BTreeMap<&'static str, (u32, u32)>);

impl Tally {
    fn check(&mut self, field: &'static str, same: bool) {
        let e = self.0.entry(field).or_default();
        e.1 += 1;
        if !same {
            e.0 += 1;
        }
    }
}

fn compare_with_oracle(name: &str, d: &IndexDraft, tsv: &str, tally: &mut Tally) {
    let paths = paths(d);
    let ours: HashMap<&str, usize> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i))
        .collect();
    let mut rows = 0;
    for line in tsv.lines() {
        rows += 1;
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 12, "{name}: an oracle row has 12 fields");
        let Some(&i) = ours.get(f[0]) else {
            eprintln!("oracle: {name} {}: no node at this path", f[0]);
            tally.check("path", false);
            continue;
        };
        tally.check("path", true);
        let n = &d.nodes[i];
        let at = f[0];
        let mut chk = |field: &'static str, same: bool| {
            if !same {
                eprintln!("oracle: {name} {at}: {field} differs");
            }
            tally.check(field, same);
        };
        if f[1] != "?" {
            let want = (f[1] != "-").then(|| format!("@lyid:{}", f[1]));
            chk("layer_id", n.key == want);
        }
        if f[2] != "?" {
            chk("kind", n.kind == kind_of(f[2]));
        }
        if f[3] != "?" {
            chk("visible", n.visible == (f[3] == "1"));
        }
        if f[4] != "?" && !matches!(n.kind.as_str(), "group" | "artboard") {
            let r: Vec<i64> = f[4].split(',').filter_map(|v| v.parse().ok()).collect();
            let want =
                (r.len() == 4 && r[2] > r[0] && r[3] > r[1]).then(|| [r[0], r[1], r[2], r[3]]);
            let got = n.bounds.map(|b| [b.left, b.top, b.right, b.bottom]);
            chk("bounds", got == want);
        }
        if f[5] != "?" {
            chk("opacity", n.attrs["opacity"] == f[5]);
        }
        if f[6] != "?" {
            chk("blend", n.attrs["blend"] == f[6].trim_end());
        }
        if f[7] != "?" {
            chk("clipped", (n.attrs["clipped"] == "true") == (f[7] == "1"));
        }
        if let Some(want) = unhex(f[8]).filter(|_| f[8] != "?") {
            chk("name", n.name == want);
        }
        if f[9] != "?" && f[9] != "-" {
            chk("text", n.text.as_deref() == unhex(f[9]).as_deref());
        }
        if f[10] != "?" && f[10] != "-" {
            chk(
                "placed_id",
                n.attrs.get("placed_id").map(String::as_str) == Some(f[10]),
            );
        }
        if f[11] != "?" && f[11] != "-" {
            let want = match f[11] {
                "data" => Some("embedded"),
                _ => None,
            };
            let got = n.attrs.get("smart_source").map(String::as_str);
            let same = match want {
                Some(w) => got == Some(w),
                None => got.is_some_and(|g| g.starts_with("linked:")),
            };
            chk("smart_source", same);
        }
    }
    tally.check("node_count", rows == d.nodes.len());
}

#[test]
fn every_public_corpus_file_indexes_and_agrees_with_both_readers() {
    let Some(dir) = std::env::var_os("PSD_PUBLIC_CORPUS") else {
        eprintln!("public corpus: skipped (PSD_PUBLIC_CORPUS is not set)");
        return;
    };
    let oracle = std::env::var_os("PSD_TOOLS_ORACLE");
    if oracle.is_none() {
        eprintln!("public corpus: psd-tools oracle skipped (PSD_TOOLS_ORACLE is not set)");
    }
    let dir = Path::new(&dir);
    let manifest = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/corpus/manifest.tsv"
    ))
    .expect("check: the manifest is committed");
    let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let (mut files, mut nodes) = (0, 0);
    let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
    let mut tally = Tally::default();
    for line in manifest.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let name = line
            .split('\t')
            .next()
            .expect("check: a manifest row has fields");
        if !name.ends_with(".psd") && !name.ends_with(".psb") {
            continue;
        }
        let path = dir.join(name);
        assert!(
            path.is_file(),
            "{name}: listed in the manifest, missing from the corpus directory"
        );
        let draft = {
            let mut src = File::open(&path).expect("check: the corpus file opens");
            index_psd(&mut src, &limits).unwrap_or_else(|e| panic!("{name}: {e}"))
        };
        let bytes = std::fs::read(&path).expect("check: the corpus file reads");
        compare_index_with_full(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        files += 1;
        nodes += draft.nodes.len();
        for n in &draft.nodes {
            *kinds.entry(n.kind.clone()).or_default() += 1;
        }
        eprintln!("corpus: {name} nodes={}", draft.nodes.len());
        if let Some(o) = &oracle {
            let tsv = std::fs::read_to_string(Path::new(o).join(format!("{name}.tsv")))
                .unwrap_or_else(|_| panic!("{name}: no psd-tools tree in the oracle folder"));
            compare_with_oracle(name, &draft, &tsv, &mut tally);
        }
    }
    assert_eq!(files, 18, "every PSD and PSB of the manifest is checked");
    eprintln!("public corpus: {files} files, {nodes} nodes, kinds {kinds:?}");
    if oracle.is_some() {
        let mut bad = Vec::new();
        for (field, (differ, of)) in &tally.0 {
            eprintln!("oracle: {field} differs on {differ} of {of}");
            if *differ > 0 {
                bad.push(*field);
            }
        }
        assert!(bad.is_empty(), "the pack and psd-tools differ on {bad:?}");
    }
}
