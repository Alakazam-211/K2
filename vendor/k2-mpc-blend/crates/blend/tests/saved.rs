//! Rows on files the program itself saved in a headless run on a build node (`PROVENANCE.md`):
//! small scenes with known visibility states, and every object, light and camera type and rotation
//! mode, saved by three pinned releases (3.6.23, 4.5.9, 5.1.2). The files stay on the node and are
//! never committed; `tests/saved/manifest.tsv` pins each by sha256. It fails unless
//! `BLEND_SAVED_DIR` names the folder (one subfolder per release).
//!
//! Each file must index to the state it was saved with: an object's or a collection's
//! `hide_viewport`, `hide_select` and `hide_render`, `visible` as not disabled in viewports; a
//! view layer's own states (an object hidden in the layer, a collection excluded or hidden in
//! it) not read; every object type, rotation mode, light type and camera type by name.

use std::path::PathBuf;

use k2_mpc_base::index::{DraftNode, IndexDraft, IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_blend::index_blend;

const MANIFEST: &str = include_str!("saved/manifest.tsv");

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn node<'a>(d: &'a IndexDraft, key: &str) -> &'a DraftNode {
    d.nodes
        .iter()
        .find(|n| n.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("no node {key}"))
}

fn attr<'a>(n: &'a DraftNode, name: &str) -> Option<&'a str> {
    n.attrs.get(name).map(String::as_str)
}

/// `key`'s three states (disabled in viewports, not selectable, disabled in renders), and its
/// `visible`.
fn states(d: &IndexDraft, key: &str, viewport: bool, select: bool, render: bool, what: &str) {
    let n = node(d, key);
    let got = (
        attr(n, "hide_viewport"),
        attr(n, "hide_select"),
        attr(n, "hide_render"),
        n.visible,
    );
    let s = |b: bool| Some(if b { "true" } else { "false" });
    assert_eq!(
        got,
        (s(viewport), s(select), s(render), !viewport),
        "{what}: {key}"
    );
}

const OBJECT_TYPES: &[(&str, &str)] = &[
    ("EMPTY", "empty"),
    ("MESH", "mesh"),
    ("CURVE", "curve"),
    ("SURFACE", "surface"),
    ("FONT", "text"),
    ("META", "metaball"),
    ("LATTICE", "lattice"),
    ("ARMATURE", "armature"),
    ("CAMERA", "camera"),
    ("LIGHT", "light"),
    ("SPEAKER", "speaker"),
    ("LIGHT_PROBE", "light_probe"),
    ("CURVES", "curves"),
    ("POINTCLOUD", "point_cloud"),
    ("VOLUME", "volume"),
    ("GREASEPENCIL", "grease_pencil"),
    ("GPENCIL", "legacy_grease_pencil"),
];

fn types(d: &IndexDraft, what: &str) -> usize {
    let mut typed = 0;
    for n in &d.nodes {
        if n.kind == "object" {
            if let Some(t) = n.name.strip_prefix("t_") {
                let want = OBJECT_TYPES.iter().find(|(k, _)| *k == t).map(|(_, v)| *v);
                assert_eq!(attr(n, "object_type"), want, "{what}: {}", n.name);
                typed += usize::from(want.is_some());
            }
            if let Some(r) = n.name.strip_prefix("r_") {
                assert_eq!(
                    attr(n, "rotation_mode"),
                    Some(r.to_ascii_lowercase().as_str()),
                    "{what}: {}",
                    n.name
                );
            }
        }
        if n.kind == "light" {
            if let Some(l) = n.name.strip_prefix("light_") {
                assert_eq!(
                    attr(n, "light_type"),
                    Some(l.to_ascii_lowercase().as_str()),
                    "{what}: {}",
                    n.name
                );
            }
        }
        if n.kind == "camera" {
            if let Some(c) = n.name.strip_prefix("cam_") {
                let want = match c {
                    "PERSP" => "perspective",
                    "ORTHO" => "orthographic",
                    _ => "panoramic",
                };
                assert_eq!(attr(n, "camera_type"), Some(want), "{what}: {}", n.name);
            }
        }
    }
    typed
}

#[test]
#[ignore = "files kept on a build node: run by name: PROVENANCE.md"]
fn every_saved_state_indexes_as_it_was_set() {
    let dir = std::env::var_os("BLEND_SAVED_DIR")
        .map(PathBuf::from)
        .expect("BLEND_SAVED_DIR: the saved files' folder on the build node (PROVENANCE.md)");
    let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let mut files = 0;
    for line in MANIFEST
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let mut cols = line.split('\t');
        let (release, file, sha) = (
            cols.next().unwrap(),
            cols.next().unwrap(),
            cols.next().unwrap(),
        );
        let what = format!("{release}/{file}");
        let bytes =
            std::fs::read(dir.join(release).join(file)).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!(
            hex(k2_mpc_base::index::Digest::of(&bytes).as_bytes()),
            sha,
            "{what}: the file the manifest pins"
        );
        let d = index_blend(&mut &bytes[..], &limits)
            .unwrap_or_else(|e| panic!("check: {what} indexes: {e}"));
        let large = release.starts_with('5');
        assert_eq!(
            d.attrs.get("header").map(String::as_str),
            Some(if large { "large" } else { "classic" }),
            "{what}"
        );
        let stem = file.trim_end_matches(".blend");
        match stem {
            "ob_none" | "ob_eye_hidden" => states(&d, "@ob:Probe", false, false, false, &what),
            "ob_hide_viewport" => states(&d, "@ob:Probe", true, false, false, &what),
            "ob_hide_select" => states(&d, "@ob:Probe", false, true, false, &what),
            "ob_hide_render" => states(&d, "@ob:Probe", false, false, true, &what),
            "ob_all" => states(&d, "@ob:Probe", true, true, true, &what),
            s if s.starts_with("col_") || s.starts_with("lc_") => {
                let (v, sel, r) = match s {
                    "col_hide_viewport" => (true, false, false),
                    "col_hide_select" => (false, true, false),
                    "col_hide_render" => (false, false, true),
                    "col_all" => (true, true, true),
                    _ => (false, false, false),
                };
                states(&d, "@gr:Box", v, sel, r, &what);
                states(&d, "@ob:Inside", false, false, false, &what);
                let inside = node(&d, "@ob:Inside");
                let parent = inside.parent.map(|p| d.nodes[p as usize].key.clone());
                assert_eq!(parent, Some(Some("@gr:Box".to_owned())), "{what}");
            }
            "types" | "types_compressed" => {
                let typed = types(&d, &what);
                assert!(typed >= 12, "{what}: {typed} object types named");
                if stem == "types_compressed" {
                    assert_eq!(d.attrs.get("compression").map(String::as_str), Some("zstd"));
                }
            }
            other => panic!("{what}: no row for {other}"),
        }
        files += 1;
    }
    eprintln!("saved rows: {files} files checked");
    assert_eq!(
        files,
        MANIFEST
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .count()
    );
    assert!(files >= 45, "{files}");
}
