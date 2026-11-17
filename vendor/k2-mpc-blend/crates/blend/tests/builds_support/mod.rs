//! The program's builds of the pack's test scenes against our reads of them, shared by the
//! round trip's builds (`tests/roundtrip.rs`, a build node's folder) and the builds kept in this
//! repo (`tests/builds.rs`, `tests/builds/`): the program built each scene of a spec of our test
//! scenes itself, saved it, reopened the save and reported it in `values.tsv`; our reader, mesh
//! read and live tier on each save are checked against that report ([`built_matches`],
//! [`builds_held`]).
//!
//! `values.tsv`: one line per object of every file, tab-separated, in the file's object order
//! (a line starting `#` is a comment): file, name, type, data, location, Euler rotation, scale
//! (each `x,y,z`), hidden in viewports, not selectable, hidden in renders (`True`/`False`),
//! rotation mode, the evaluated world matrix (16 floats, column-major), parent, a camera's
//! vertical field of view in radians at the scene's render size, eight camera cross-check
//! columns, a mesh object's `vertices,polygons,triangles` (the base mesh), the collections
//! holding it (JSON: `[[name, hidden], ...]`), and `world_eval`: the world matrix the driver
//! composes itself for every object, evaluated or not (16 floats, column-major: the parent's,
//! its parent inverse, its own transform with the delta transforms; no constraint applied;
//! `!PARENT_TYPE` for a parent that isn't an object). Lines come in any order and are matched by
//! file and object name. A file the program couldn't open (`!OPEN_FAILED`) or whose run failed
//! or ran out of time (`!EXIT`) is one line `file`, the word, a reason: the row fails on it. A
//! camera with no perspective field of view says `!ORTHO` or `!PANO` in its field-of-view
//! column. The program evaluates no object disabled in viewports or in no scene: its evaluated
//! world matrix is then exactly the identity.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::path::Path;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_base::session::{Pack, PackError, SpliceError};
use k2_mpc_blend::live::model::{floats_text, parse_floats};
use k2_mpc_blend::live::{BlendLive, OBJ_PROPS};
use k2_mpc_blend::meshread::check::check;
use k2_mpc_blend::meshread::{mesh_read, MeshReadLimits};
use k2_mpc_blend::scene::READ_TO;
use k2_mpc_blend::write::{decompress, writes};
use k2_mpc_blend::{index_blend, read_blend, read_scene, verbs, ReadLimits, SceneModel};

/// The columns of a line of `values.tsv`.
pub const COLUMNS: usize = 25;

/// Lines of `values.tsv` that are notes on the outputs (the run's RECORD), not values: the
/// run's own scan of a saved file for paths of the build node, and for an asset it embeds.
pub const MARKERS: &[&str] = &["!PATH_LEAK", "!EMBEDDED_ASSET"];

/// One object as the program printed it.
#[derive(Debug)]
pub struct Seen {
    pub file: String,
    pub name: String,
    pub kind: String,
    pub loc: [f32; 3],
    /// The Euler rotation, in radians.
    pub rot: [f64; 3],
    pub scale: [f32; 3],
    pub hidden: [bool; 3],
    /// The rotation mode, as the program names it (`XYZ`, `QUATERNION`).
    pub rotmode: String,
    pub world: [f64; 16],
    /// The parent object's name; empty for none.
    pub parent: String,
    /// `world_eval`: `None` for `!PARENT_TYPE`.
    pub world_eval: Option<[f64; 16]>,
    pub yfov: Option<f64>,
    /// `!ORTHO` or `!PANO` in the field-of-view column: a camera with no perspective field of
    /// view; empty otherwise.
    pub fov_mark: String,
    pub triangles: Option<u64>,
    /// The collections holding it, as the report's JSON (`[[name, hidden], ...]`).
    pub collections: String,
}

/// A name column as the report escapes it: `\\`, `\t`, `\n`, `\r`.
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            other => panic!("an escape the report doesn't use: \\{other:?} in {s:?}"),
        }
    }
    out
}

pub fn floats<const N: usize>(s: &str, what: &str) -> [f64; N] {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| {
            x.trim()
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("{what}: {s:?}"))
        })
        .collect();
    v.try_into()
        .unwrap_or_else(|_| panic!("{what}: {N} numbers, got {s:?}"))
}

pub fn f32s(s: &str, what: &str) -> [f32; 3] {
    floats::<3>(s, what).map(|x| x as f32)
}

pub fn seen(dir: &Path) -> Vec<Seen> {
    seen_lines(dir, None)
}

/// The objects of one file in `dir/values.tsv`, by its file column: the lines of other files
/// (a build of another file that failed among them) are left unread.
pub fn seen_of(dir: &Path, file: &str) -> Vec<Seen> {
    seen_lines(dir, Some(file))
}

fn seen_lines(dir: &Path, file: Option<&str>) -> Vec<Seen> {
    let text = std::fs::read_to_string(dir.join("values.tsv")).expect("the program's values");
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| match file {
            None => true,
            Some(f) => unescape(l.split('\t').next().unwrap_or_default()) == f,
        })
        .filter(|l| !l.split('\t').nth(1).is_some_and(|w| MARKERS.contains(&w)))
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            // `!OPEN_FAILED` (it couldn't open the file) or `!EXIT` (it failed or ran out of time)
            assert!(
                !c.get(1).is_some_and(|w| w.starts_with('!')),
                "the program's run failed: {l}"
            );
            assert_eq!(c.len(), COLUMNS, "a line of {} columns: {l}", c.len());
            let flag = |s: &str| match s {
                "True" => true,
                "False" => false,
                _ => panic!("a flag: {s:?}"),
            };
            Seen {
                file: unescape(c[0]),
                name: unescape(c[1]),
                kind: c[2].into(),
                loc: f32s(c[4], "location"),
                rot: floats::<3>(c[5], "rotation"),
                scale: f32s(c[6], "scale"),
                hidden: [flag(c[7]), flag(c[8]), flag(c[9])],
                rotmode: c[10].into(),
                world: floats::<16>(c[11], "world matrix"),
                parent: unescape(c[12]),
                world_eval: (c[24] != "!PARENT_TYPE").then(|| floats::<16>(c[24], "world_eval")),
                // `!ORTHO`, `!PANO`: a camera with no perspective field of view
                yfov: (!c[13].is_empty() && !c[13].starts_with('!'))
                    .then(|| floats::<1>(c[13], "field of view")[0]),
                fov_mark: if c[13].starts_with('!') {
                    c[13].to_owned()
                } else {
                    String::new()
                },
                triangles: (!c[22].is_empty()).then(|| floats::<3>(c[22], "mesh counts")[2] as u64),
                collections: c[23].to_owned(),
            }
        })
        .collect()
}

pub fn ours(path: &Path) -> SceneModel {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let (_, raw) = decompress(&bytes, &ReadLimits::default()).expect("check: decompresses");
    let f = read_blend(&mut &raw[..], &ReadLimits::default()).expect("check: reads");
    read_scene(&f).expect("check: models")
}

pub fn value(m: &SceneModel, key: &str, name: &str) -> Option<String> {
    let id = m.by_key(key).unwrap_or_else(|| panic!("no {key}"));
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

/// Within the rounding of `f32` values composed in `f32` steps.
pub fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-5 * a.abs().max(b.abs()).max(1.0)
}

/// What one file's checks compared.
#[derive(Default, Debug)]
pub struct Compared {
    pub objects: usize,
    pub matrices: usize,
    pub meshes: usize,
    pub cameras: usize,
    pub values: usize,
    pub hidden: usize,
    pub linked: usize,
    pub scanned: usize,
    pub declined: usize,
    pub named: usize,
    pub live: usize,
    pub composed: usize,
    pub world_eval: usize,
    pub referee: usize,
    pub referee_skipped: usize,
    /// Live write-backs of a change taken and found by our reader ([`live_matches`]).
    pub live_written: usize,
    /// Objects the program doesn't evaluate (disabled in viewports, by their own flag or every
    /// collection holding them), their world exactly the identity.
    pub unevaluated: usize,
    /// Objects in no scene (no collection holds them): not evaluated, their world exactly the
    /// identity, never drawn.
    pub no_scene: usize,
}

impl Compared {
    /// Adds `c`'s counts to these.
    pub fn add(&mut self, c: &Compared) {
        self.objects += c.objects;
        self.matrices += c.matrices;
        self.meshes += c.meshes;
        self.cameras += c.cameras;
        self.values += c.values;
        self.hidden += c.hidden;
        self.linked += c.linked;
        self.scanned += c.scanned;
        self.declined += c.declined;
        self.named += c.named;
        self.live += c.live;
        self.composed += c.composed;
        self.world_eval += c.world_eval;
        self.referee += c.referee;
        self.referee_skipped += c.referee_skipped;
        self.live_written += c.live_written;
        self.unevaluated += c.unevaluated;
        self.no_scene += c.no_scene;
    }
}

/// The identity, column-major.
pub const IDENTITY: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Whether the program evaluated `s`: not disabled in viewports by its own flag, and held by a
/// collection not disabled (a scene's own collection among them). An object no collection
/// holds is in no scene, and isn't evaluated.
pub fn evaluated(s: &Seen) -> Result<bool, String> {
    let held = holders(&s.collections)?;
    Ok(!s.hidden[0] && held.iter().any(|h| !h.1))
}

/// `a * b`, column-major 4x4.
pub fn mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

/// A rotation of `angle` radians about axis `axis` (0 x, 1 y, 2 z), column-major.
pub fn axis_rotation(axis: usize, angle: f64) -> [f64; 16] {
    let (sn, cs) = angle.sin_cos();
    let mut m = IDENTITY;
    let (i, j) = ((axis + 1) % 3, (axis + 2) % 3);
    m[i * 4 + i] = cs;
    m[i * 4 + j] = sn;
    m[j * 4 + i] = -sn;
    m[j * 4 + j] = cs;
    m
}

/// The world matrix of `s` composed in `f64` from the program's own columns, in this test alone:
/// translation, then the Euler rotation in the order its rotation mode names (`XYZ`: X applied
/// first), then scale, under its parent's composed matrix (the spec's parent inverses are the
/// identity). `None` when the program's report can't give it: a rotation mode with no Euler
/// angles (the report has no quaternion or axis and angle).
pub fn composed(s: &Seen, mine: &[&Seen], depth: usize) -> Result<Option<[f64; 16]>, String> {
    if depth > 64 {
        return Err(format!("{}: a parent chain over 64 deep", s.name));
    }
    let order = s.rotmode.as_bytes();
    if order.len() != 3 || !order.iter().all(|b| matches!(b, b'X' | b'Y' | b'Z')) {
        return Ok(None);
    }
    let mut m = IDENTITY;
    m[12] = f64::from(s.loc[0]);
    m[13] = f64::from(s.loc[1]);
    m[14] = f64::from(s.loc[2]);
    // `XYZ` applies X first: R = Rz * Ry * Rx
    for &b in order.iter().rev() {
        let axis = usize::from(b - b'X');
        m = mul(&m, &axis_rotation(axis, s.rot[axis]));
    }
    let mut sc = IDENTITY;
    for k in 0..3 {
        sc[k * 4 + k] = f64::from(s.scale[k]);
    }
    m = mul(&m, &sc);
    if s.parent.is_empty() {
        return Ok(Some(m));
    }
    let p = mine
        .iter()
        .find(|x| x.name == s.parent)
        .ok_or_else(|| format!("{}: the program has no parent {}", s.name, s.parent))?;
    Ok(composed(p, mine, depth + 1)?.map(|pm| mul(&pm, &m)))
}

/// The referee checked against the program: for each object it evaluates, the driver's own
/// composition (`world_eval`) equals the program's evaluated world matrix. A row whose
/// `world_eval` can't be one (a parent that isn't an object, `!PARENT_TYPE`) is skipped and
/// counted.
pub fn referee_matches(mine: &[&Seen], c: &mut Compared) -> Result<(), String> {
    for s in mine {
        if !evaluated(s)? {
            // not evaluated: the program's world is exactly the identity
            if s.world != IDENTITY {
                return Err(format!(
                    "{}: not evaluated by the program, but its world isn't the identity",
                    s.name
                ));
            }
            if holders(&s.collections)?.is_empty() {
                c.no_scene += 1;
            } else {
                c.unevaluated += 1;
            }
            continue;
        }
        let Some(want) = &s.world_eval else {
            c.referee_skipped += 1;
            continue;
        };
        for (i, (&a, &b)) in want.iter().zip(&s.world).enumerate() {
            if !near(a, b) {
                return Err(format!(
                    "{}: world_eval [{i}] {a}, the program's evaluated world {b}",
                    s.name
                ));
            }
        }
        c.referee += 1;
    }
    Ok(())
}

/// The program's view of one of our inputs against our read of the same bytes: every object our
/// mesh read draws is there, a mesh object, with our triangle count; its evaluated world matrix
/// equals our composition (driven objects left out); each camera's vertical field of view
/// equals ours.
pub fn input_matches(
    file: &str,
    bytes: &[u8],
    mine: &[&Seen],
    c: &mut Compared,
) -> Result<(), String> {
    let one = |name: &str| -> Result<&Seen, String> {
        mine.iter()
            .copied()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("the program has no {name}"))
    };
    let read =
        mesh_read(bytes, &MeshReadLimits::default()).map_err(|e| format!("our read: {e}"))?;
    let scene = check(&read.k2scene, &MeshReadLimits::default())
        .map_err(|e| format!("our output: {e}"))?
        .scene;
    for o in &scene.objects {
        let s = one(&o.id)?;
        if s.kind != "MESH" {
            return Err(format!("{}: the program's type {}", o.id, s.kind));
        }
        let t = scene.meshes[o.mesh as usize].triangles.len() as u64;
        if s.triangles != Some(t) {
            return Err(format!(
                "{}: triangles ours {t}, the program's {:?}",
                o.id, s.triangles
            ));
        }
        c.meshes += 1;
        if o.transform_not_evaluated {
            continue;
        }
        if !evaluated(s)? {
            // the program reports no evaluated matrix for an object it doesn't evaluate (disabled
            // in viewports, or every collection holding it disabled): its world is exactly the
            // identity, and ours is checked against the driver's own composition (`world_eval`)
            // and, beside it, one this test makes from the report's columns
            if s.world != IDENTITY {
                return Err(format!(
                    "{}: not evaluated by the program, but its world isn't the identity",
                    o.id
                ));
            }
            if let Some(want) = &s.world_eval {
                for (i, (&a, &b)) in o.transform.iter().zip(want).enumerate() {
                    if !near(f64::from(a), b) {
                        return Err(format!(
                            "{}: world matrix [{i}]: ours {a}, the driver's world_eval {b}",
                            o.id
                        ));
                    }
                }
                c.world_eval += 1;
            }
            let Some(want) = composed(s, mine, 0)? else {
                continue;
            };
            for (i, (&a, &b)) in o.transform.iter().zip(&want).enumerate() {
                if !near(f64::from(a), b) {
                    return Err(format!(
                        "{}: world matrix [{i}]: ours {a}, composed from the program's values {b}",
                        o.id
                    ));
                }
            }
            c.composed += 1;
            continue;
        }
        for (i, (&a, &b)) in o.transform.iter().zip(&s.world).enumerate() {
            if !near(f64::from(a), b) {
                return Err(format!(
                    "{}: world matrix [{i}]: ours {a}, the program's {b}",
                    o.id
                ));
            }
        }
        c.matrices += 1;
    }
    for cam in &scene.cameras {
        let s = one(&cam.id)?;
        let y = s
            .yfov
            .ok_or_else(|| format!("{}: no field of view from the program", cam.id))?;
        if (f64::from(cam.yfov) - y).abs() > 1e-6 {
            return Err(format!(
                "{}: vertical field of view ours {}, the program's {y}",
                cam.id, cam.yfov
            ));
        }
        c.cameras += 1;
    }
    let _ = file;
    Ok(())
}

/// The collections column (`[["name", hidden], ...]`) as (name, hidden) pairs.
pub fn holders(json: &str) -> Result<Vec<(String, bool)>, String> {
    let bad = || format!("collections not a list of [name, hidden]: {json}");
    let c: Vec<char> = json.chars().collect();
    let mut i = 0;
    // the next character that isn't white space, taken
    let next = |i: &mut usize| -> Option<char> {
        while c.get(*i).is_some_and(|x| x.is_whitespace()) {
            *i += 1;
        }
        *i += 1;
        c.get(*i - 1).copied()
    };
    let mut out = Vec::new();
    if next(&mut i) != Some('[') {
        return Err(bad());
    }
    let mut j = i;
    if next(&mut j) == Some(']') {
        return Ok(out);
    }
    loop {
        if next(&mut i) != Some('[') || next(&mut i) != Some('"') {
            return Err(bad());
        }
        let mut name = String::new();
        loop {
            let x = *c.get(i).ok_or_else(bad)?;
            i += 1;
            match x {
                '"' => break,
                '\\' => {
                    let e = *c.get(i).ok_or_else(bad)?;
                    i += 1;
                    match e {
                        'u' => {
                            let hex: String = c.get(i..i + 4).ok_or_else(bad)?.iter().collect();
                            i += 4;
                            let u = u32::from_str_radix(&hex, 16).map_err(|_| bad())?;
                            name.push(char::from_u32(u).unwrap_or('\u{fffd}'));
                        }
                        'n' => name.push('\n'),
                        't' => name.push('\t'),
                        'r' => name.push('\r'),
                        e => name.push(e),
                    }
                }
                x => name.push(x),
            }
        }
        if next(&mut i) != Some(',') {
            return Err(bad());
        }
        let hidden = match next(&mut i) {
            Some('t') if c.get(i..i + 3) == Some(&['r', 'u', 'e'][..]) => {
                i += 3;
                true
            }
            Some('f') if c.get(i..i + 4) == Some(&['a', 'l', 's', 'e'][..]) => {
                i += 4;
                false
            }
            _ => return Err(bad()),
        };
        if next(&mut i) != Some(']') {
            return Err(bad());
        }
        out.push((name, hidden));
        match next(&mut i) {
            Some(',') => continue,
            Some(']') => break,
            _ => return Err(bad()),
        }
    }
    Ok(out)
}

/// Absolute paths a saved file can carry outside its data (a file browser's last folder, the
/// save's own folder): none may reach what we hand a host.
pub const PATH_MARKS: &[&str] = &[
    "/home/", "/Users/", "/mnt/", "/var/", "/tmp/", "/root/", "/opt/", "/srv/", ":\\",
];

/// What we hand a host of `bytes`: the structure index, the scene list (lines and typed), the
/// mesh read (K2SCENE1's header, the report's lines, typed) and the reader's model: none holds a path of
/// [`PATH_MARKS`]. Counts the outputs scanned.
pub fn no_paths(bytes: &[u8], past: bool, c: &mut Compared) -> Result<(), String> {
    let mut outputs: Vec<(&str, String)> = Vec::new();
    let limits = IndexLimits::new(200_000, 64, 256 << 20, INDEX_TEXT_MAX);
    let draft = index_blend(&mut &bytes[..], &limits).map_err(|e| format!("the index: {e}"))?;
    outputs.push(("the index", format!("{draft:?}")));
    let list = verbs::list(bytes, &ReadLimits::default()).map_err(|e| format!("list: {e:?}"))?;
    outputs.push(("list", list.lines().join("\n")));
    outputs.push(("list, typed", list.typed().spec()));
    // a file past the mesh read's versions has no mesh read to scan (it is declined)
    if !past {
        let read = verbs::meshes(bytes, &MeshReadLimits::default())
            .map_err(|e| format!("meshes: {e:?}"))?;
        // its text is the JSON header (magic, length, header): the data section after it is
        // numbers only, whose bytes can spell anything; every drawn object's and camera's key
        // must be in the text scanned
        let header = k2scene_header(&read.k2scene)?;
        for key in read
            .report
            .drawn
            .iter()
            .map(|d| d.key.as_str())
            .chain(read.report.cameras.iter().map(|d| d.key.as_str()))
        {
            if !header.contains(&format!("\"{key}\"")) {
                return Err(format!("K2SCENE1's header doesn't hold the key {key}"));
            }
        }
        outputs.push(("K2SCENE1's header", header));
        outputs.push(("meshes", read.report.lines().join("\n")));
        outputs.push(("meshes, typed", verbs::meshes_typed(&read).spec()));
    }
    outputs.push(("our model", format!("{:?}", ours_of(bytes)?)));
    for (what, text) in &outputs {
        if let Some(m) = PATH_MARKS.iter().find(|m| text.contains(**m)) {
            let at = text.find(m).unwrap_or(0);
            let from = text[..at]
                .char_indices()
                .rev()
                .nth(40)
                .map_or(0, |(i, _)| i);
            let to = text[at..]
                .char_indices()
                .nth(40)
                .map_or(text.len(), |(i, _)| at + i);
            return Err(format!(
                "{what} holds a path ({m:?}): …{}…",
                &text[from..to]
            ));
        }
        c.scanned += 1;
    }
    Ok(())
}

/// K2SCENE1's text: its JSON header, by the layout (`docs/WIRE.md`: the magic, a `u32` length,
/// the header).
pub fn k2scene_header(k2scene: &[u8]) -> Result<String, String> {
    let bad = || "K2SCENE1: no header".to_owned();
    if k2scene.get(..8) != Some(&b"K2SCENE1"[..]) {
        return Err(bad());
    }
    let len = u32::from_le_bytes(
        k2scene
            .get(8..12)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    );
    let end = 12usize.checked_add(len as usize).ok_or_else(bad)?;
    let text = k2scene.get(12..end).ok_or_else(bad)?;
    String::from_utf8(text.to_vec()).map_err(|_| "K2SCENE1: a header that isn't UTF-8".to_owned())
}

/// The file version of `bytes` (`502` for 5.2).
pub fn version_of(bytes: &[u8]) -> Result<u16, String> {
    let (_, raw) =
        decompress(bytes, &ReadLimits::default()).map_err(|e| format!("decompresses: {e}"))?;
    let f = read_blend(&mut &raw[..], &ReadLimits::default())
        .map_err(|e| format!("our reader: {e}"))?;
    Ok(f.header.version)
}

pub fn ours_of(bytes: &[u8]) -> Result<SceneModel, String> {
    let (_, raw) =
        decompress(bytes, &ReadLimits::default()).map_err(|e| format!("decompresses: {e}"))?;
    let f = read_blend(&mut &raw[..], &ReadLimits::default())
        .map_err(|e| format!("our reader: {e}"))?;
    read_scene(&f).map_err(|e| format!("our model: {e}"))
}

/// Our reader's model of `bytes` and its decompressed bytes.
pub fn ours_raw(bytes: &[u8]) -> Result<(Vec<u8>, SceneModel), String> {
    let (_, raw) =
        decompress(bytes, &ReadLimits::default()).map_err(|e| format!("decompresses: {e}"))?;
    let f = read_blend(&mut &raw[..], &ReadLimits::default())
        .map_err(|e| format!("our reader: {e}"))?;
    let m = read_scene(&f).map_err(|e| format!("our model: {e}"))?;
    Ok((raw, m))
}

/// The program's object type and rotation mode, and each object's three flags (hidden in
/// viewports, not selectable, hidden in renders), against what our reader names for the file's
/// own object of the same name (`object_type`, `rotation_mode`, the `restrictflag` bits, and
/// `hidden`): the values named for this version, checked on the program's own saves. Counts
/// the objects compared.
pub fn named_match(m: &SceneModel, mine: &[&Seen], c: &mut Compared) -> Result<(), String> {
    for s in mine {
        let Some(id) = m
            .ids
            .iter()
            .find(|i| i.kind == "object" && i.library.is_none() && i.name == s.name)
        else {
            continue;
        };
        let get = |n: &str| {
            id.named
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.to_uppercase())
        };
        let flags = s.hidden.map(|b| Some(b.to_string().to_uppercase()));
        let ours = [get("hide_viewport"), get("hide_select"), get("hide_render")];
        if get("object_type").as_deref() != Some(s.kind.as_str())
            || get("rotation_mode").as_deref() != Some(s.rotmode.as_str())
            || ours != flags
            || id.hidden != Some(s.hidden[0])
        {
            return Err(format!(
                "{}: named {:?}, hidden {:?}; the program's {} {} {:?}",
                s.name, id.named, id.hidden, s.kind, s.rotmode, s.hidden
            ));
        }
        c.named += 1;
    }
    Ok(())
}

/// The live tier on a save: it opens (no decline), and its write-back of nothing is the file's
/// own bytes. A file of a release the writer writes (every save kept today: 5.2.4 and 5.3.0
/// save 5.02 and 5.03) takes a change: the first object the live tier places (`edit` `1` or
/// `t`) moved to (7, 7, 7), its rotation and scale kept, is written back and our reader finds it
/// there. Past the writer's releases a change is refused, `live_read_only`, nothing written.
pub fn live_matches(bytes: &[u8], c: &mut Compared) -> Result<(), String> {
    let pack = BlendLive::default();
    let d = pack
        .decode(bytes)
        .map_err(|e| format!("the live tier: declined {}", e.code))?;
    let pairing = pack
        .pairing(&d.roots, &d.codec)
        .map_err(|e| format!("the live pairing: {e}"))?;
    let same = pack
        .encode_from_paired(bytes, &pairing, &d.roots, &d.codec)
        .map_err(|e| format!("the live write-back of nothing: {e}"))?;
    if same != bytes {
        return Err("the live write-back of nothing isn't the file".into());
    }
    let mut changed = d.roots.clone();
    let mut moved: Option<(String, String)> = None;
    for (r, n) in changed.iter_mut() {
        let (true, Neutral::Map(m)) = (r == OBJ_PROPS, n) else {
            continue;
        };
        let placed: Vec<String> = m
            .iter()
            .filter(|(k, v)| {
                k.ends_with("\u{1f}edit") && matches!(v.as_str(), Some("1") | Some("t"))
            })
            .map(|(k, _)| k.trim_end_matches("\u{1f}edit").to_owned())
            .collect();
        for obj in placed {
            let key = format!("{obj}\u{1f}xform");
            let Some(x) = m
                .get(&key)
                .and_then(Value::as_str)
                .and_then(parse_floats::<9>)
            else {
                continue;
            };
            let mut y = x;
            y[..3].copy_from_slice(&[7.0; 3]);
            let nid = m
                .get(&format!("{obj}\u{1f}nid"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            m.insert(key, Value::str(floats_text(&y)));
            moved = Some((obj, nid));
            break;
        }
    }
    let Some((obj, nid)) = moved else {
        c.live += 1;
        return Ok(());
    };
    let out = pack.encode_from_paired(bytes, &pairing, &changed, &d.codec);
    if writes(version_of(bytes)?) {
        let written = out.map_err(|e| format!("the live write-back of {obj} moved: {e}"))?;
        let m = ours_of(&written)?;
        let loc = (!nid.is_empty())
            .then(|| m.by_key(&nid))
            .flatten()
            .and_then(|i| {
                i.fields
                    .iter()
                    .find(|(n, _)| n == "loc")
                    .map(|(_, v)| v.clone())
            });
        if loc.as_deref() != Some("7,7,7") {
            return Err(format!(
                "the live write-back of {obj} ({nid}) moved: our reader finds loc {loc:?}"
            ));
        }
        c.live_written += 1;
    } else {
        match out {
            Err(SpliceError::Pack(PackError(m))) if m.starts_with("live_read_only: ") => {}
            other => {
                return Err(format!(
                    "the live write-back of a change: {:?}, not live_read_only",
                    other.map(|b| b.len())
                ))
            }
        }
    }
    c.live += 1;
    Ok(())
}

/// One of the program's own builds of the scene spec, as it reopened it, against our read of
/// the same bytes: [`input_matches`] and [`live_matches`]; our reader's own objects (no library)
/// are the program's, by name, and each other object of the program's is an object linked in
/// our reader (a whole object with a library, or a link placeholder, kind `object` by its stored
/// name's type code) and never drawn; [`named_match`]; each mesh object and camera the program has in a collection
/// is drawn by our mesh read or listed as left out with its reason; and each drawn object's
/// `hidden` agrees with the program's flags: `self` exactly when its own viewport flag is set,
/// `collection` only without it, and `collection` whenever every collection holding it is
/// hidden.
pub fn built_matches(
    file: &str,
    bytes: &[u8],
    mine: &[&Seen],
    c: &mut Compared,
) -> Result<(), String> {
    // a save newer than the versions the mesh read takes is declined, typed, and everything
    // else is still checked
    let past = version_of(bytes)? > READ_TO;
    if past {
        match mesh_read(bytes, &MeshReadLimits::default()) {
            Err(e) if e.to_string() == "declined: live_version" => c.declined += 1,
            Err(e) => return Err(format!("our read: {e}, not the version's decline")),
            Ok(_) => return Err("our read took a save past its versions".into()),
        }
    } else {
        referee_matches(mine, c)?;
        input_matches(file, bytes, mine, c)?;
        live_matches(bytes, c)?;
        // the living rooms hold objects the program doesn't evaluate (disabled in viewports)
        // and ones it does: both comparisons must have run
        if file.starts_with("living_room")
            && (c.matrices == 0 || c.composed == 0 || c.world_eval == 0)
        {
            return Err(format!(
                "{} world matrices, {} against world_eval and {} composed from the program's values: all must run",
                c.matrices, c.world_eval, c.composed
            ));
        }
    }
    no_paths(bytes, past, c)?;
    let (_, model) = ours_raw(bytes)?;
    // the file's own objects, and the objects it links from libraries (a whole object, or a link
    // placeholder), by name
    let local: BTreeSet<&str> = model
        .ids
        .iter()
        .filter(|i| i.kind == "object" && i.library.is_none())
        .map(|i| i.name.as_str())
        .collect();
    let linked: BTreeSet<&str> = model
        .ids
        .iter()
        .filter(|i| i.library.is_some() && i.kind == "object")
        .map(|i| i.name.as_str())
        .collect();
    let theirs: BTreeSet<&str> = mine.iter().map(|s| s.name.as_str()).collect();
    let ours_only: Vec<&&str> = local.difference(&theirs).collect();
    let neither: Vec<&&str> = theirs
        .iter()
        .filter(|n| !local.contains(*n) && !linked.contains(*n))
        .collect();
    if !ours_only.is_empty() || !neither.is_empty() {
        return Err(format!(
            "objects: ours only {ours_only:?}, the program's neither ours nor linked {neither:?}"
        ));
    }
    let theirs_linked: Vec<&str> = theirs
        .iter()
        .copied()
        .filter(|n| !local.contains(n))
        .collect();
    c.linked += theirs_linked.len();
    if !past {
        named_match(&model, mine, c)?;
    }
    if past {
        return Ok(());
    }
    let read =
        mesh_read(bytes, &MeshReadLimits::default()).map_err(|e| format!("our read: {e}"))?;
    let drawn = |name: &str| read.report.drawn.iter().any(|d| d.name == name);
    let camera = |name: &str| read.report.cameras.iter().any(|d| d.name == name);
    let left = |name: &str| {
        read.report
            .skipped
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.reason)
    };
    for n in &theirs_linked {
        if drawn(n) || camera(n) {
            return Err(format!("{n}: linked, and drawn"));
        }
    }
    for s in mine {
        if holders(&s.collections)?.is_empty() && (drawn(&s.name) || camera(&s.name)) {
            return Err(format!("{}: in no scene, and drawn", s.name));
        }
    }
    for s in mine {
        if holders(&s.collections)?.is_empty() {
            continue;
        }
        let shown = match s.kind.as_str() {
            "MESH" => drawn(&s.name),
            "CAMERA" => camera(&s.name),
            _ => continue,
        };
        if !shown && left(&s.name).is_none() {
            return Err(format!(
                "{}: the program's {} is neither drawn nor listed as left out",
                s.name, s.kind
            ));
        }
        // a camera the program has no perspective field of view for: our camera type names it
        // so, and the mesh read leaves it out for it
        if s.kind == "CAMERA" && !s.fov_mark.is_empty() {
            if shown || left(&s.name) != Some("camera_not_perspective") {
                return Err(format!(
                    "{}: the program's {} camera, ours {:?}",
                    s.name,
                    s.fov_mark,
                    left(&s.name)
                ));
            }
            c.named += 1;
        }
    }
    for d in &read.report.drawn {
        let s = mine
            .iter()
            .find(|s| s.name == d.name)
            .ok_or_else(|| format!("the program has no {}", d.name))?;
        let own = s.hidden[0];
        let held = holders(&s.collections)?;
        let all_hidden = !held.is_empty() && held.iter().all(|h| h.1);
        let agrees = match d.hidden {
            Some("self") => own,
            Some("collection") => !own,
            None => !own && !all_hidden,
            Some(w) => return Err(format!("{}: hidden {w:?}", d.name)),
        };
        if !agrees {
            return Err(format!(
                "{}: hidden ours {:?}, the program's own flag {own}, its collections {}",
                d.name, d.hidden, s.collections
            ));
        }
        c.hidden += 1;
    }
    Ok(())
}

/// One release's builds, labelled `label` in the lines printed: each file `values_dir`'s
/// `values.tsv` reports, read from `blend_dir`, by [`built_matches`] against its report; a
/// `.blend` file of `blend_dir` the report doesn't name (a library the builds link,
/// `plants.blend`) is read by our reader too. Every failure is pushed to `failed`. The totals,
/// and whether every file passed with every comparison run (none of the counts the builds must
/// exercise zero).
pub fn builds_held(
    label: &str,
    values_dir: &Path,
    blend_dir: &Path,
    failed: &mut Vec<String>,
) -> (Compared, bool) {
    let seen = seen(values_dir);
    let files: BTreeSet<&str> = seen.iter().map(|s| s.file.as_str()).collect();
    let on_disk: BTreeSet<String> = std::fs::read_dir(blend_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".blend"))
                .collect()
        })
        .unwrap_or_default();
    let mut total = Compared::default();
    let mut fails = 0;
    for f in on_disk.iter().filter(|f| !files.contains(f.as_str())) {
        // a library the builds link: no report of its own, read all the same
        let r2 = std::panic::catch_unwind(|| ours(&blend_dir.join(f)));
        match r2 {
            Ok(m) if m.ids.iter().any(|i| i.kind == "object") => {
                eprintln!("{label} {f}: ok: a library, read")
            }
            _ => {
                eprintln!("{label} {f}: FAIL: not read, or no object");
                failed.push(format!("{label} {f}: not read"));
                fails += 1;
            }
        }
    }
    for f in &files {
        let mine: Vec<&Seen> = seen.iter().filter(|s| s.file == *f).collect();
        let mut c = Compared {
            objects: mine.len(),
            ..Compared::default()
        };
        let result = std::fs::read(blend_dir.join(f))
            .map_err(|e| format!("the build: {e}"))
            .and_then(|bytes| built_matches(f, &bytes, &mine, &mut c));
        match result {
            Ok(()) => eprintln!(
                "{label} {f}: ok: {} objects, {} world matrices, {} meshes, {} cameras, {} hidden, {} named, {} linked, {} live opens, {} live write-backs, {} against world_eval, {} composed from the program's values, {} referee checked, {} referee skipped, {} not evaluated, {} in no scene, {} outputs without a path, {} mesh reads declined by version",
                c.objects, c.matrices, c.meshes, c.cameras, c.hidden, c.named, c.linked, c.live, c.live_written, c.world_eval, c.composed, c.referee, c.referee_skipped, c.unevaluated, c.no_scene, c.scanned, c.declined
            ),
            Err(e) => {
                eprintln!("{label} {f}: FAIL: {e}");
                failed.push(format!("{label} {f}: {e}"));
                fails += 1;
            }
        }
        total.add(&c);
    }
    eprintln!(
        "{label}: {} files, {fails} failed; {} world matrices, {} meshes, {} cameras, {} hidden, {} named, {} linked, {} against world_eval, {} composed from the program's values, {} referee compared ({} skipped), {} not evaluated, {} in no scene; {} live opens, {} live write-backs; {} outputs without a path; {} mesh reads declined by version",
        files.len(),
        total.matrices,
        total.meshes,
        total.cameras,
        total.hidden,
        total.named,
        total.linked,
        total.world_eval,
        total.composed,
        total.referee,
        total.referee_skipped,
        total.unevaluated,
        total.no_scene,
        total.live,
        total.live_written,
        total.scanned,
        total.declined
    );
    // every file compared in full (opened live, its objects named), or (past the versions
    // read) declined by version
    let compared = total.matrices > 0
        && total.cameras > 0
        && total.hidden > 0
        && total.named > 0
        && total.world_eval > 0
        && total.composed > 0
        && total.referee > 0
        && total.unevaluated > 0
        && total.live + total.declined == files.len()
        && total.live_written > 0;
    let held = fails == 0
        && !files.is_empty()
        && (compared || total.declined == files.len())
        && total.linked > 0
        && total.scanned > 0;
    if held {
        eprintln!(
            "{label}: {} objects of the program's builds seen, read as it reads them",
            seen.len()
        );
    }
    (total, held)
}

// ---------- the builds kept in this repo ----------

/// The builds kept in this repo (`NOTICE.md`): `MANIFEST.tsv`, the scene spec, and one folder
/// per release with its saves and its own report.
pub fn kept_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("builds")
}

/// The releases whose builds are kept.
pub const KEPT_RELEASES: &[&str] = &["5.2.4", "5.3.0"];

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every file under `dir`, as paths relative to it with `/`.
fn files_under(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), String> {
    let rd = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for e in rd {
        let e = e.map_err(|e| e.to_string())?;
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if e.path().is_dir() {
            files_under(&e.path(), &r, out)?;
        } else {
            out.push(r);
        }
    }
    Ok(())
}

/// `MANIFEST.tsv` against the folder: every listed file is there with its size and sha256, and
/// every file there (but the manifest) is listed. The files checked.
fn manifest_holds(dir: &Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(dir.join("MANIFEST.tsv"))
        .map_err(|e| format!("MANIFEST.tsv: {e}"))?;
    let mut listed = BTreeSet::new();
    for l in text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let c: Vec<&str> = l.split('\t').collect();
        let [_, path, bytes, sha] = c[..] else {
            return Err(format!("MANIFEST.tsv: a line of {} columns: {l}", c.len()));
        };
        let mut b = std::fs::read(dir.join(path)).map_err(|e| format!("{path}: listed, {e}"))?;
        if k2_mpc_blend::mutant::active("FixtureByteChanged") && path.ends_with(".blend") {
            // planted: a kept file changed on disk
            if let Some(x) = b.last_mut() {
                *x ^= 1;
            }
        }
        if b.len().to_string() != bytes {
            return Err(format!(
                "{path}: {} bytes, the manifest says {bytes}",
                b.len()
            ));
        }
        let got = hex(k2_mpc_base::index::Digest::of(&b).as_bytes());
        if got != sha {
            return Err(format!("{path}: sha256 {got}, the manifest says {sha}"));
        }
        if !listed.insert(path.to_owned()) {
            return Err(format!("{path}: listed twice"));
        }
    }
    let mut there = Vec::new();
    files_under(dir, "", &mut there)?;
    let unlisted: Vec<&String> = there
        .iter()
        .filter(|f| *f != "MANIFEST.tsv" && !listed.contains(*f))
        .collect();
    if !unlisted.is_empty() {
        return Err(format!("files the manifest doesn't list: {unlisted:?}"));
    }
    if listed.is_empty() {
        return Err("the manifest lists nothing".into());
    }
    Ok(listed.len())
}

/// The kept builds: the manifest holds (every file's size and sha256, nothing unlisted), then
/// each release's saves against that release's own report, as the round trip's builds
/// ([`builds_held`]): every comparison run, objects in no scene seen, the linked plant counted.
pub fn row_kept_builds() -> Result<(), String> {
    const ROW: &str = "row_kept_builds";
    let dir = kept_dir();
    let n = manifest_holds(&dir).map_err(|e| format!("{ROW}: manifest: {e}"))?;
    eprintln!("builds: manifest: {n} files, each its size and sha256");
    for r in KEPT_RELEASES {
        let d = dir.join(r);
        // the living rooms' Plant, linked from plants.blend: the program's save holds it as a link
        // placeholder (struct `ID`), an object by its stored name's type code
        for f in ["living_room.blend", "living_room_hidden.blend"] {
            let bytes = std::fs::read(d.join(f)).map_err(|e| format!("{ROW}: {r}/{f}: {e}"))?;
            let (_, m) = ours_raw(&bytes).map_err(|e| format!("{ROW}: {r}/{f}: {e}"))?;
            let plant: Vec<String> = m
                .ids
                .iter()
                .filter(|i| i.name == "Plant")
                .map(|i| {
                    format!(
                        "{} {} {}",
                        i.kind,
                        i.struct_name,
                        i.library.map_or("-", |l| m.ids[l].kind.as_str())
                    )
                })
                .collect();
            if plant != ["object ID library"] {
                return Err(format!(
                    "{ROW}: {r}/{f}: Plant is {plant:?}, not one object, a link placeholder, linked"
                ));
            }
        }
        eprintln!("builds {r}: Plant an object, linked, a link placeholder, in both living rooms");
        let mut failed = Vec::new();
        let (t, held) = builds_held(&format!("builds {r}"), &d, &d, &mut failed);
        if !failed.is_empty() {
            return Err(format!("{ROW}: {r}: {}", failed.join(" | ")));
        }
        if !held || t.no_scene == 0 || t.linked == 0 || t.declined > 0 {
            return Err(format!(
                "{ROW}: {r}: a comparison didn't run (held {held}, {} in no scene, {} linked, {} declined)",
                t.no_scene, t.linked, t.declined
            ));
        }
    }
    Ok(())
}
