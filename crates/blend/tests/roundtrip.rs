//! The headless round trip (`PROVENANCE.md`): on a build node, the writer patches files the program
//! saved (`examples/blend_patch.rs`), the program opens each patched file, prints what it reads of
//! every object, and saves it again; here, both are checked: the program read our values, and our
//! reader finds them held in the program's own save. Three pinned releases; plain and zstd files.
//! It fails unless `BLEND_ROUNDTRIP_DIR` names the job's folder (one subfolder per release, each
//! with `values.tsv` and `resaved/`). The gate's round-trip part (`scripts/roundtrip.sh`) runs
//! it, and fails unless every release was checked with objects seen; `scripts/check.sh` runs
//! that part when a change lands. Nothing here is committed but this test.

use std::path::{Path, PathBuf};

use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, read_scene, ReadLimits, SceneModel};

const RELEASES: &[&str] = &["3.6.23", "4.5.9", "5.1.2"];

/// One object as the program printed it.
#[derive(Debug)]
struct Seen {
    file: String,
    name: String,
    kind: String,
    loc: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 3],
    hidden: [bool; 3],
}

fn floats(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s
        .split(',')
        .map(|x| x.parse::<f64>().expect("a number") as f32)
        .collect();
    [v[0], v[1], v[2]]
}

fn seen(dir: &Path) -> Vec<Seen> {
    let text = std::fs::read_to_string(dir.join("values.tsv")).expect("the program's values");
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            Seen {
                file: c[0].into(),
                name: c[1].into(),
                kind: c[2].into(),
                loc: floats(c[4]),
                rot: floats(c[5]),
                scale: floats(c[6]),
                hidden: [c[7] == "True", c[8] == "True", c[9] == "True"],
            }
        })
        .collect()
}

fn ours(path: &Path) -> SceneModel {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (_, raw) = decompress(&bytes, &ReadLimits::default()).expect("check: decompresses");
    let f = read_blend(&mut &raw[..], &ReadLimits::default()).expect("check: reads");
    read_scene(&f).expect("check: models")
}

fn value(m: &SceneModel, key: &str, name: &str) -> Option<String> {
    let id = m.by_key(key).unwrap_or_else(|| panic!("no {key}"));
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

#[test]
#[ignore = "files kept on a build node: scripts/roundtrip.sh runs it"]
fn the_program_reads_our_writes_and_keeps_them() {
    let dir = std::env::var_os("BLEND_ROUNDTRIP_DIR")
        .map(PathBuf::from)
        .expect("BLEND_ROUNDTRIP_DIR: the round trip's folder on the build node (PROVENANCE.md)");
    let mut checked = 0;
    for r in RELEASES {
        let d = dir.join(r);
        let seen = seen(&d);
        let one = |file: &str, name: &str| -> &Seen {
            seen.iter()
                .find(|s| s.file == file && s.name == name)
                .unwrap_or_else(|| panic!("{r}: the program has no {name} in {file}"))
        };
        let none = |file: &str, name: &str| {
            assert!(
                !seen.iter().any(|s| s.file == file && s.name == name),
                "{r}: {name} is still in {file}"
            )
        };
        // every change at once
        let p = one("patched_all.blend", "Probe Renamed");
        assert_eq!(p.kind, "MESH", "{r}");
        assert_eq!(p.loc, [1.5, -2.25, 3.0], "{r}: location");
        assert_eq!(p.rot, [0.1, 0.2, 0.3], "{r}: rotation");
        assert_eq!(p.scale, [2.0, 0.5, 1.0], "{r}: scale");
        assert_eq!(
            p.hidden,
            [false, true, true],
            "{r}: viewport, select, render"
        );
        none("patched_all.blend", "Probe");
        // disabled in viewports
        let h = one("patched_hidden.blend", "Probe");
        assert_eq!(h.hidden, [true, false, false], "{r}");
        assert_eq!(h.loc, [0.0; 3], "{r}");
        // a zstd file written back whole
        assert_eq!(
            one("patched_zstd.blend", "r_XYZ").loc,
            [4.0, 5.0, 6.0],
            "{r}"
        );
        assert_eq!(
            one("patched_zstd.blend", "Mesh Renamed").kind,
            "MESH",
            "{r}"
        );
        none("patched_zstd.blend", "t_MESH");

        // and our reader finds the values held in the program's own saves
        let m = ours(&d.join("resaved/patched_all.blend"));
        let k = "@ob:Probe%20Renamed";
        assert_eq!(value(&m, k, "loc").as_deref(), Some("1.5,-2.25,3"), "{r}");
        assert_eq!(value(&m, k, "rot").as_deref(), Some("0.1,0.2,0.3"), "{r}");
        assert_eq!(value(&m, k, "size").as_deref(), Some("2,0.5,1"), "{r}");
        assert_eq!(
            value(&m, k, "hide_viewport").as_deref(),
            Some("false"),
            "{r}"
        );
        assert_eq!(value(&m, k, "hide_select").as_deref(), Some("true"), "{r}");
        assert_eq!(value(&m, k, "hide_render").as_deref(), Some("true"), "{r}");
        assert!(m.by_key("@ob:Probe").is_none(), "{r}");
        let m = ours(&d.join("resaved/patched_hidden.blend"));
        assert_eq!(
            m.by_key("@ob:Probe").and_then(|i| i.hidden),
            Some(true),
            "{r}"
        );
        let m = ours(&d.join("resaved/patched_zstd.blend"));
        assert_eq!(
            value(&m, "@ob:r_XYZ", "loc").as_deref(),
            Some("4,5,6"),
            "{r}"
        );
        assert_eq!(
            value(&m, "@ob:Mesh%20Renamed", "object_type").as_deref(),
            Some("mesh"),
            "{r}"
        );
        checked += 1;
        eprintln!("round trip {r}: {} objects seen, values held", seen.len());
    }
    assert_eq!(checked, RELEASES.len());
}
