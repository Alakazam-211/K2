//! The headless round trip (`PROVENANCE.md`): on a build node, the program that writes these
//! files opens each of our inputs (the generated scenes, `testgen::scenes`, but the living room's
//! family, see [`NOT_INPUTS`]; and the writer's patches of public samples the program itself
//! saved, `examples/blend_patch.rs`), prints what it reads of every object, and saves it again. Here both are checked, per release: the program read our values and our
//! composition, and our reader finds the values held in the program's own save. It fails unless
//! `BLEND_ROUNDTRIP_DIR` names the folder (one subfolder per release, each with `values.tsv` and
//! `resaved/`). The gate's round-trip part (`scripts/roundtrip.sh`) runs it, and fails unless
//! every release was checked with objects seen; `scripts/check.sh` runs that part when a change
//! lands. Nothing here is committed but this test.
//!
//! The same run's builds (the folder beside the releases', `OUT-build/<release>/`): the program
//! built each scene of a spec of our test scenes itself (the living room's family among them),
//! saved it, reopened the save and reported it; `built/` holds the saves and the library they
//! link. Our reader, mesh read and live tier on each save are checked against that report
//! (`builds_support`, shared with `tests/builds.rs`, which holds what `values.tsv` holds).
//!
//! The program's own saves ([`the_program_reopens_our_writes_of_its_own_saves`]): the writer patched the
//! program's own saves at 5.2.4 and 5.3.0 (its builds, and its re-saves of the public samples:
//! [`OWN_SAVES`]), and the program reopened each, reported it and saved it again. Its folder is
//! `BLEND_OWN_SAVES_DIR` (one subfolder per release, each with `values.tsv` and `resaved/`); the
//! reports of the same files before the patches are the round trip's (`BLEND_ROUNDTRIP_DIR`).
//!
//! Our animated and constrained edits, reopened by the program
//! ([`the_program_reopens_our_animated_and_constrained_edits`]): a row comparing our edits with
//! the program's report after reopening, run where the private outputs are. The writer and the
//! live tier edited the program's builds of a scene with an animated visibility bit and of a scene with a
//! constrained object ([`REOPENED`]), and the program reopened each, reported it, logged its
//! animation and constraints, and saved it again. Its folder is `BLEND_REOPENED_DIR` (one
//! subfolder per release, each with `values.tsv`, a `<file>.log` per file and `resaved/`); our
//! edited files and their record are `BLEND_REOPENED_IN` (`PATCHES.tsv`, one subfolder per
//! release); the program's reports of its builds before the edits are
//! `BLEND_REOPENED_ANIMATED_BEFORE` and `BLEND_REOPENED_CONSTRAINED_BEFORE`.

mod builds_support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use builds_support::{
    builds_held, composed, evaluated, input_matches, named_match, near, no_paths, ours, ours_of,
    ours_raw, referee_matches, seen, seen_of, value, version_of, Compared, Seen, COLUMNS, MARKERS,
};
use k2_mpc_blend::scene::READ_TO;
use k2_mpc_blend::testgen::scenes;
use k2_mpc_blend::SceneModel;

const RELEASES: &[&str] = &["5.2.4", "5.3.0"];

/// Generated scenes that are not round-trip inputs: the program files the generated living
/// room's local data as library data (its library block comes before them), so the round trip
/// takes the program's own build of that scene instead; the reader's rows keep these.
const NOT_INPUTS: &[&str] = &[
    "living_room.blend",
    "living_room_zstd.blend",
    "living_room_hidden.blend",
];

/// The writer's patches of public samples the program itself saved (the input folder's `demo/`,
/// fetched by sha256, never committed): file, the object's name before and after, its location,
/// its scale when patched, and its `restrictflag` bits after (viewport 1, select 2, render 4).
/// One of [`DEMO`]: the file, the object's name before and after, its location, its scale when
/// patched, and its `restrictflag` bits after.
type DemoPatch = (
    &'static str,
    &'static str,
    &'static str,
    [f32; 3],
    Option<[f32; 3]>,
    i64,
);

const DEMO: &[DemoPatch] = &[
    (
        "demo_jiggly_pudding.blend",
        "Empty",
        "Empty Moved",
        [1.0, 2.0, 3.0],
        None,
        2,
    ),
    (
        "demo_gizmo_array.blend",
        "Cube",
        "Cube Moved",
        [0.5, -1.0, 2.0],
        Some([1.5; 3]),
        0,
    ),
    (
        "demo_mandelbrot_grow.blend",
        "Sun",
        "Sun Moved",
        [3.0, -4.0, 6.0],
        None,
        4,
    ),
    (
        "demo_skeleton_arm_xray.blend",
        "Camera",
        "Camera Moved",
        [2.0, -6.0, 3.0],
        None,
        2,
    ),
];

/// `restrictflag` of `key` in a model, as a number (named values stop at the pinned versions).
fn restrict(m: &SceneModel, key: &str) -> Result<i64, String> {
    value(m, key, "restrictflag")
        .ok_or_else(|| format!("{key}: no restrictflag"))?
        .parse()
        .map_err(|_| format!("{key}: restrictflag not a number"))
}

fn loc_of(m: &SceneModel, key: &str) -> Result<String, String> {
    value(m, key, "loc").ok_or_else(|| format!("{key}: no loc"))
}

/// The key a model gives an object named `name` (`@ob:` and the name, percent-encoded as the
/// model writes it): found by name in the model, never built by hand.
fn key_named(m: &SceneModel, name: &str) -> Result<String, String> {
    m.ids
        .iter()
        .find(|i| i.name == name && i.key.as_deref().is_some_and(|k| k.starts_with("@ob:")))
        .and_then(|i| i.key.clone())
        .ok_or_else(|| format!("our reader finds no object {name:?}"))
}

/// A patched file: the program read the values written, and our reader finds them held in the
/// program's own save. `loc`, `scale` and the visibility bits, and the name.
#[allow(clippy::too_many_arguments)]
fn patch_held(
    d: &Path,
    file: &str,
    mine: &[&Seen],
    was: &str,
    name: &str,
    loc: [f32; 3],
    scale: Option<[f32; 3]>,
    bits: i64,
    c: &mut Compared,
) -> Result<(), String> {
    if mine.iter().any(|s| s.name == was) {
        return Err(format!("{was} is still in the program's view"));
    }
    let s = mine
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("the program has no {name}"))?;
    if s.loc != loc {
        return Err(format!("{name}: location {:?}", s.loc));
    }
    if let Some(k) = scale {
        if s.scale != k {
            return Err(format!("{name}: scale {:?}", s.scale));
        }
    }
    let want = [bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
    if s.hidden != want {
        return Err(format!("{name}: viewport, select, render {:?}", s.hidden));
    }
    c.values += 1;
    let m = ours(&d.join("resaved").join(file));
    let k = key_named(&m, name)?;
    let floats_of =
        |t: String| -> Vec<f32> { t.split(',').filter_map(|x| x.parse::<f32>().ok()).collect() };
    if floats_of(loc_of(&m, &k)?) != loc.to_vec() {
        return Err(format!("our reader: {k} loc {:?}", loc_of(&m, &k)));
    }
    if restrict(&m, &k)? & 7 != bits {
        return Err(format!(
            "our reader: {k} restrictflag {}",
            restrict(&m, &k)?
        ));
    }
    if key_named(&m, was).is_ok() {
        return Err(format!("our reader still finds {was}"));
    }
    c.values += 1;
    Ok(())
}

#[test]
#[ignore = "files kept on a build node: scripts/roundtrip.sh runs it"]
fn the_program_reads_our_writes_and_keeps_them() {
    let dir = std::env::var_os("BLEND_ROUNDTRIP_DIR")
        .map(PathBuf::from)
        .expect("BLEND_ROUNDTRIP_DIR: the round trip's folder on the build node (PROVENANCE.md)");
    // the inputs the program was given: the folder beside the releases' (`../IN`)
    let input = dir.parent().map(|p| p.join("IN")).filter(|p| p.is_dir());
    let mut inputs: BTreeSet<&str> = scenes::ALL
        .iter()
        .map(|(f, _)| *f)
        .filter(|f| !NOT_INPUTS.contains(f))
        .collect();
    inputs.extend(DEMO.iter().map(|d| d.0));
    let mut failed = Vec::new();
    let mut checked = 0;
    let mut built_checked = 0;
    for r in RELEASES {
        let d = dir.join(r);
        let seen = seen(&d);
        let files: BTreeSet<&str> = seen.iter().map(|s| s.file.as_str()).collect();
        let mut total = Compared::default();
        let mut fails = 0;
        for f in &inputs {
            let mine: Vec<&Seen> = seen.iter().filter(|s| s.file == *f).collect();
            let mut c = Compared {
                objects: mine.len(),
                ..Compared::default()
            };
            let result = (|| -> Result<(), String> {
                if !files.contains(f) {
                    return Err("the program printed nothing of it".into());
                }
                let resaved = d.join("resaved").join(f);
                if !resaved.is_file() {
                    return Err("no save".into());
                }
                let _ = ours(&resaved);
                let saved = std::fs::read(&resaved).map_err(|e| format!("the save: {e}"))?;
                let past = version_of(&saved)? > READ_TO;
                no_paths(&saved, past, &mut c)?;
                if !past {
                    let (_, m) = ours_raw(&saved)?;
                    named_match(&m, &mine, &mut c)?;
                }
                referee_matches(&mine, &mut c)?;
                for s in &mine {
                    if !(s.collections.starts_with('[') && s.collections.ends_with(']')) {
                        return Err(format!("{}: collections not a JSON list", s.name));
                    }
                }
                if let Some((_, write)) = scenes::ALL.iter().find(|(n, _)| n == f) {
                    input_matches(f, &write(), &mine, &mut c)?;
                }
                if *f == "instanced.blend"
                    && !mine
                        .iter()
                        .any(|s| s.collections.contains("[\"Props, A: B\","))
                {
                    return Err("no object held by \"Props, A: B\"".into());
                }
                if let Some(&(_, was, name, loc, scale, bits)) = DEMO.iter().find(|x| x.0 == *f) {
                    patch_held(&d, f, &mine, was, name, loc, scale, bits, &mut c)?;
                    if let Some(i) = &input {
                        let bytes = std::fs::read(i.join("demo").join(f))
                            .map_err(|e| format!("the input: {e}"))?;
                        input_matches(f, &bytes, &mine, &mut c)?;
                    }
                }
                if c.objects == 0 {
                    return Err("no object seen".into());
                }
                Ok(())
            })();
            match result {
                Ok(()) => eprintln!(
                    "round trip {r} {f}: ok: {} objects, {} world matrices, {} meshes, {} cameras, {} patched values, {} named in the save, {} against world_eval, {} composed from the program's values, {} referee checked, {} referee skipped, {} outputs without a path",
                    c.objects, c.matrices, c.meshes, c.cameras, c.values, c.named, c.world_eval, c.composed, c.referee, c.referee_skipped, c.scanned
                ),
                Err(e) => {
                    eprintln!("round trip {r} {f}: FAIL: {e}");
                    failed.push(format!("{r} {f}: {e}"));
                    fails += 1;
                }
            }
            total.objects += c.objects;
            total.matrices += c.matrices;
            total.meshes += c.meshes;
            total.cameras += c.cameras;
            total.values += c.values;
            total.named += c.named;
        }
        eprintln!(
            "round trip {r}: {} files, {fails} failed; {} world matrices, {} meshes, {} cameras, {} patched values, {} objects named in the saves compared",
            inputs.len(),
            total.matrices,
            total.meshes,
            total.cameras,
            total.values,
            total.named
        );
        if fails == 0
            && total.matrices > 0
            && total.cameras > 0
            && total.values > 0
            && total.named > 0
        {
            checked += 1;
            eprintln!("round trip {r}: {} objects seen, values held", seen.len());
        }
        if built(&dir, r, &mut failed) {
            built_checked += 1;
        }
    }
    assert!(
        failed.is_empty(),
        "round trip failures:\n{}",
        failed.join("\n")
    );
    assert_eq!(checked, RELEASES.len());
    assert_eq!(built_checked, RELEASES.len());
}

/// Release `r`'s builds (the folder beside the releases', `../OUT-build/<r>`): each file the
/// program built from the scene spec, `built/<file>`, against its report
/// ([`builds_support::builds_held`]). True when every file passed with every comparison run.
fn built(dir: &Path, r: &str, failed: &mut Vec<String>) -> bool {
    let d = dir.parent().map(|p| p.join("OUT-build").join(r));
    let Some(d) = d.filter(|d| d.join("values.tsv").is_file()) else {
        eprintln!(
            "round trip {r} built: FAIL: no OUT-build/{r}/values.tsv beside the releases' folder"
        );
        failed.push(format!("{r} built: no folder"));
        return false;
    };
    let (t, held) = builds_held(
        &format!("round trip {r} built"),
        &d,
        &d.join("built"),
        failed,
    );
    // the line the gate's round-trip part counts (`scripts/roundtrip.sh`)
    if held {
        eprintln!(
            "round trip {r}: {} objects of the program's builds seen, read as it reads them",
            t.objects
        );
    }
    held
}

// ---------- own saves: the writer's patches of the program's own saves ----------

/// One of the writer's patches of the program's own saves, by the object's name in the program's save before the patches.
#[derive(Clone, Copy, Debug)]
enum Patch {
    /// Its location.
    Loc(&'static str, [f32; 3]),
    /// Its scale.
    Scale(&'static str, [f32; 3]),
    /// A visibility bit (0 disabled in viewports, 1 not selectable, 2 disabled in renders) set
    /// (`true`) or cleared.
    Bit(&'static str, usize, bool),
    /// A new name.
    Name(&'static str, &'static str),
}

impl Patch {
    fn object(&self) -> &'static str {
        match *self {
            Patch::Loc(o, _) | Patch::Scale(o, _) | Patch::Bit(o, _, _) | Patch::Name(o, _) => o,
        }
    }
}

/// Where an own-saves file's report before the patches is: the program's report of its own build
/// (`OUT-build/<release>/values.tsv` beside the round trip's releases), or of its re-save of a
/// public sample (the round trip's `<release>/values.tsv`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Before {
    Built,
    Sample,
}

/// The program's own saves the writer patched (the same at both releases) and the writer's patches of each.
const OWN_SAVES: &[(&str, Before, &[Patch])] = &[
    (
        "living_room.blend",
        Before::Built,
        &[
            Patch::Loc("Table", [1.5, -0.5, 0.0]),
            Patch::Name("Mug", "Mug Moved"),
            Patch::Bit("Sofa", 0, true),
        ],
    ),
    (
        "living_room_hidden.blend",
        Before::Built,
        &[Patch::Loc("Vase", [0.25, -0.5, 1.0])],
    ),
    (
        "three_cameras.blend",
        Before::Built,
        &[Patch::Loc("Camera.Long", [-10.0, -14.0, 6.0])],
    ),
    (
        "camera_fits_wide.blend",
        Before::Built,
        &[Patch::Loc("Fit.Vertical", [-6.0, -9.0, 4.5])],
    ),
    (
        "camera_fits_tall.blend",
        Before::Built,
        &[Patch::Loc("Fit.Vertical", [-6.0, -9.0, 4.5])],
    ),
    (
        "driven_quat.blend",
        Before::Built,
        &[
            Patch::Loc("Turned", [1.0, 2.0, 0.5]),
            Patch::Loc("Driven", [-3.0, 2.0, 1.0]),
        ],
    ),
    (
        "instanced.blend",
        Before::Built,
        &[Patch::Loc("Copy.002", [-1.0, 2.5, 0.5])],
    ),
    (
        "empty_scene.blend",
        Before::Built,
        &[Patch::Name("Camera", "Camera Renamed")],
    ),
    (
        "large.blend",
        Before::Built,
        &[Patch::Loc("Obj.1999", [-98.0, -78.0, 2.5])],
    ),
    (
        "demo_jiggly_pudding.blend",
        Before::Sample,
        &[
            Patch::Loc("Empty Moved", [-1.0, -2.0, -3.0]),
            Patch::Bit("Empty Moved", 1, false),
            Patch::Name("Empty Moved", "Empty Moved Again"),
        ],
    ),
    (
        "demo_gizmo_array.blend",
        Before::Sample,
        &[
            Patch::Loc("Cube Moved", [-0.5, 1.0, -2.0]),
            Patch::Scale("Cube Moved", [0.75; 3]),
            Patch::Name("Cube Moved", "Cube Moved Again"),
        ],
    ),
    (
        "demo_mandelbrot_grow.blend",
        Before::Sample,
        &[
            Patch::Loc("Sun Moved", [-3.0, 4.0, -6.0]),
            Patch::Bit("Sun Moved", 2, false),
            Patch::Name("Sun Moved", "Sun Moved Again"),
        ],
    ),
    (
        "demo_skeleton_arm_xray.blend",
        Before::Sample,
        &[
            Patch::Loc("Camera Moved", [-2.0, 6.0, -3.0]),
            Patch::Bit("Camera Moved", 1, false),
            Patch::Name("Camera Moved", "Camera Moved Again"),
        ],
    ),
];

/// The one patched value the program doesn't keep: `Driven`'s location z, which an action
/// animates (the writer refuses that edit, `blend_driven`). The program evaluates the action
/// when it opens the file and keeps the result: z is 0 in its report and its save.
const REEVALUATED: (&str, &str, usize, f32) = ("driven_quat.blend", "Driven", 2, 0.0);

/// The report's raw lines of `dir/values.tsv` by file, then object name, every column as
/// printed (marker lines left out, as [`seen`] leaves them).
fn raw_lines(dir: &Path) -> std::collections::BTreeMap<(String, String), Vec<String>> {
    raw_lines_of(dir, None)
}

/// [`raw_lines`] of one file (by its file column as printed) when `file` names one: the lines of
/// other files are left unread.
fn raw_lines_of(
    dir: &Path,
    file: Option<&str>,
) -> std::collections::BTreeMap<(String, String), Vec<String>> {
    let text = std::fs::read_to_string(dir.join("values.tsv")).expect("the program's values");
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| match file {
            None => true,
            Some(f) => l.split('\t').next() == Some(printed(f).as_str()),
        })
        .map(|l| l.split('\t').map(str::to_owned).collect::<Vec<String>>())
        .filter(|c| !c.get(1).is_some_and(|w| MARKERS.contains(&w.as_str())))
        .map(|c| {
            assert_eq!(c.len(), COLUMNS, "a line of {} columns", c.len());
            ((c[0].clone(), c[1].clone()), c)
        })
        .collect()
}

/// What one own-saves file's checks counted.
#[derive(Default, Debug)]
struct OwnSaves {
    /// Objects of the report before the patches found after them, every column compared.
    objects: usize,
    /// Patched values as the program reports them after reopening.
    held: usize,
    /// Patched values our reader finds in the program's re-save.
    in_save: usize,
    /// Objects whose evaluated world (and `world_eval`) isn't compared with the report before:
    /// a moved object and its descendants, and an object whose viewport bit was patched.
    world_skipped: usize,
    /// Of those not evaluated, `world_eval` equal to the composition from the program's own
    /// values after the patches.
    composed: usize,
    /// Objects the referee rule checked after the patches (evaluated world equal to the
    /// driver's `world_eval`; identity when not evaluated).
    referee: usize,
    /// The patched value the program re-evaluates, found as [`REEVALUATED`] says.
    reevaluated: usize,
}

/// A name column as printed (the report escapes `\`, tab, newline and return).
fn printed(name: &str) -> String {
    name.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// One own-saves file at one release: every patched value held as the program reports it after
/// reopening, nothing else of any object changed against the report before (by name, renames
/// mapped), the one re-evaluated value as [`REEVALUATED`] says, and our reader finding the
/// patched values in the program's re-save.
#[allow(clippy::too_many_arguments)]
fn own_saves_file(
    file: &str,
    patches: &[Patch],
    after: &[&Seen],
    after_raw: &std::collections::BTreeMap<(String, String), Vec<String>>,
    before: &[&Seen],
    before_raw: &std::collections::BTreeMap<(String, String), Vec<String>>,
    resaved: &Path,
    c: &mut OwnSaves,
) -> Result<(), String> {
    if after.is_empty() {
        return Err("the program printed nothing of it after the patches".into());
    }
    if before.is_empty() {
        return Err("no report of it before the patches".into());
    }
    let renamed = |n: &str| -> String {
        patches
            .iter()
            .find_map(|p| match p {
                Patch::Name(o, new) if *o == n => Some((*new).to_owned()),
                _ => None,
            })
            .unwrap_or_else(|| n.to_owned())
    };
    for p in patches {
        if !before.iter().any(|s| s.name == p.object()) {
            return Err(format!("the report before has no {}", p.object()));
        }
    }
    let want: BTreeSet<String> = before.iter().map(|s| renamed(&s.name)).collect();
    let got: BTreeSet<String> = after.iter().map(|s| s.name.clone()).collect();
    if want != got {
        return Err(format!(
            "objects after the patches: {} want {}, missing {:?}, new {:?}",
            got.len(),
            want.len(),
            want.difference(&got).collect::<Vec<_>>(),
            got.difference(&want).collect::<Vec<_>>()
        ));
    }
    // the objects whose evaluated world follows a patch: moved (location or scale) and their
    // descendants, and those whose viewport bit was patched (the program evaluates only
    // objects shown in viewports)
    let moved: BTreeSet<String> = patches
        .iter()
        .filter_map(|p| match p {
            Patch::Loc(o, _) | Patch::Scale(o, _) => Some(renamed(o)),
            _ => None,
        })
        .collect();
    let viewport: BTreeSet<String> = patches
        .iter()
        .filter_map(|p| match p {
            Patch::Bit(o, 0, _) => Some(renamed(o)),
            _ => None,
        })
        .collect();
    let in_moved_tree = |s: &Seen| -> Result<bool, String> {
        let mut at = s;
        for _ in 0..64 {
            if moved.contains(&at.name) {
                return Ok(true);
            }
            if at.parent.is_empty() {
                return Ok(false);
            }
            at = after
                .iter()
                .find(|x| x.name == at.parent)
                .ok_or_else(|| format!("{}: no parent {}", at.name, at.parent))?;
        }
        Err(format!("{}: a parent chain over 64 deep", s.name))
    };
    for b in before {
        let name = renamed(&b.name);
        let a = after
            .iter()
            .find(|x| x.name == name)
            .ok_or_else(|| format!("{name}: not in the report after"))?;
        let mine: Vec<&Patch> = patches.iter().filter(|p| p.object() == b.name).collect();
        let tree = in_moved_tree(a)?;
        let skip_world = tree || viewport.contains(&name);
        // the patched values, as the program reports them
        let mut loc_patched = false;
        let mut scale_patched = false;
        let mut bits_patched = [false; 3];
        for p in &mine {
            match **p {
                Patch::Loc(_, v) => {
                    loc_patched = true;
                    for (k, (&x, &w)) in a.loc.iter().zip(&v).enumerate() {
                        if (file, b.name.as_str(), k)
                            == (REEVALUATED.0, REEVALUATED.1, REEVALUATED.2)
                        {
                            if x != REEVALUATED.3 || x == w {
                                return Err(format!(
                                    "{name}: location[{k}] {x}: want it re-evaluated to {}",
                                    REEVALUATED.3
                                ));
                            }
                            c.reevaluated += 1;
                        } else if x != w {
                            return Err(format!("{name}: location {:?}, patched {v:?}", a.loc));
                        }
                    }
                    c.held += 1;
                }
                Patch::Scale(_, v) => {
                    scale_patched = true;
                    if a.scale != v {
                        return Err(format!("{name}: scale {:?}, patched {v:?}", a.scale));
                    }
                    c.held += 1;
                }
                Patch::Bit(_, i, on) => {
                    bits_patched[i] = true;
                    if a.hidden[i] != on {
                        return Err(format!("{name}: bit {i} {}, patched {on}", a.hidden[i]));
                    }
                    c.held += 1;
                }
                Patch::Name(_, new) => {
                    if a.name != new || after.iter().any(|x| x.name == b.name) {
                        return Err(format!("{}: not renamed {new}", b.name));
                    }
                    c.held += 1;
                }
            }
        }
        // nothing else changed: every column as printed before, the name and parent mapped
        let rb = before_raw
            .get(&(printed(file), printed(&b.name)))
            .ok_or_else(|| format!("{}: no raw line before", b.name))?;
        let ra = after_raw
            .get(&(printed(file), printed(&name)))
            .ok_or_else(|| format!("{name}: no raw line after"))?;
        for i in 0..COLUMNS {
            let same = match i {
                0 | 1 => true,
                4 => loc_patched || rb[i] == ra[i],
                6 => scale_patched || rb[i] == ra[i],
                7..=9 => bits_patched[i - 7] || rb[i] == ra[i],
                11 | 24 => skip_world || rb[i] == ra[i],
                12 => renamed(&b.parent) == a.parent,
                _ => rb[i] == ra[i],
            };
            if !same {
                return Err(format!("{name}: column {} changed", i + 1));
            }
        }
        if skip_world {
            c.world_skipped += 1;
            if !evaluated(a)? {
                // not evaluated: the driver's composition against ours from the program's
                // values after the patches
                let Some(m) = composed(a, after, 0)? else {
                    return Err(format!("{name}: not evaluated, and no composition of it"));
                };
                let Some(w) = &a.world_eval else {
                    return Err(format!("{name}: not evaluated, and no world_eval"));
                };
                if !m.iter().zip(w).all(|(&x, &y)| near(x, y)) {
                    return Err(format!("{name}: world_eval isn't the composition after"));
                }
                c.composed += 1;
            }
        }
        c.objects += 1;
    }
    let mut r = Compared::default();
    referee_matches(after, &mut r)?;
    c.referee += r.referee + r.unevaluated + r.no_scene;
    // our reader on the program's re-save
    let m = ours(resaved);
    for p in patches {
        let name = renamed(p.object());
        let k = key_named(&m, &name)?;
        match *p {
            Patch::Loc(o, v) => {
                let got: Vec<f32> = loc_of(&m, &k)?
                    .split(',')
                    .filter_map(|x| x.parse().ok())
                    .collect();
                let mut want = v.to_vec();
                if (file, o) == (REEVALUATED.0, REEVALUATED.1) {
                    want[REEVALUATED.2] = REEVALUATED.3;
                }
                if got != want {
                    return Err(format!("our reader: {k} loc {got:?}, want {want:?}"));
                }
            }
            Patch::Scale(_, v) => {
                let t = value(&m, &k, "size")
                    .or_else(|| value(&m, &k, "scale"))
                    .ok_or_else(|| format!("our reader: {k} has no scale"))?;
                let got: Vec<f32> = t.split(',').filter_map(|x| x.parse().ok()).collect();
                if got != v.to_vec() {
                    return Err(format!("our reader: {k} scale {got:?}"));
                }
            }
            Patch::Bit(_, i, on) => {
                if (restrict(&m, &k)? & [1, 2, 4][i] != 0) != on {
                    return Err(format!("our reader: {k} bit {i} not {on}"));
                }
            }
            Patch::Name(old, _) => {
                if key_named(&m, old).is_ok() {
                    return Err(format!("our reader still finds {old}"));
                }
            }
        }
        c.in_save += 1;
    }
    Ok(())
}

#[test]
#[ignore = "files kept on a build node: scripts/roundtrip.sh runs it"]
fn the_program_reopens_our_writes_of_its_own_saves() {
    let dir = std::env::var_os("BLEND_OWN_SAVES_DIR")
        .map(PathBuf::from)
        .expect("BLEND_OWN_SAVES_DIR: the own saves' folder on the build node (PROVENANCE.md)");
    let base = std::env::var_os("BLEND_ROUNDTRIP_DIR")
        .map(PathBuf::from)
        .expect(
            "BLEND_ROUNDTRIP_DIR: the round trip's folder, whose reports are the own saves' before",
        );
    let mut failed = Vec::new();
    let mut checked = 0;
    for r in RELEASES {
        let d = dir.join(r);
        let after = seen(&d);
        let after_raw = raw_lines(&d);
        let built_dir = base.parent().map(|p| p.join("OUT-build").join(r));
        let built_dir = built_dir.expect("the round trip's folder has a parent");
        let built = seen(&built_dir);
        let built_raw = raw_lines(&built_dir);
        let samples = seen(&base.join(r));
        let samples_raw = raw_lines(&base.join(r));
        let mut total = OwnSaves::default();
        let mut fails = 0;
        for (file, from, patches) in OWN_SAVES {
            let mine: Vec<&Seen> = after.iter().filter(|s| s.file == *file).collect();
            let (b, b_raw) = match from {
                Before::Built => (&built, &built_raw),
                Before::Sample => (&samples, &samples_raw),
            };
            let was: Vec<&Seen> = b.iter().filter(|s| s.file == *file).collect();
            let mut c = OwnSaves::default();
            let resaved = d.join("resaved").join(file);
            let result = if resaved.is_file() {
                own_saves_file(
                    file, patches, &mine, &after_raw, &was, b_raw, &resaved, &mut c,
                )
            } else {
                Err("no re-save".into())
            };
            match result {
                Ok(()) => eprintln!(
                    "own saves {r} {file}: ok: {} objects compared with the report before ({from:?}), {} patched values held as the program reports them, {} found by our reader in its re-save, {} re-evaluated as expected, {} worlds not compared with before (moved or their descendants, or a viewport bit patched), {} of those composed, {} referee checked",
                    c.objects, c.held, c.in_save, c.reevaluated, c.world_skipped, c.composed, c.referee
                ),
                Err(e) => {
                    eprintln!("own saves {r} {file}: FAIL: {e}");
                    failed.push(format!("{r} {file}: {e}"));
                    fails += 1;
                }
            }
            total.objects += c.objects;
            total.held += c.held;
            total.in_save += c.in_save;
            total.reevaluated += c.reevaluated;
            total.world_skipped += c.world_skipped;
            total.composed += c.composed;
            total.referee += c.referee;
        }
        eprintln!(
            "own saves {r}: {} files, {fails} failed; {} objects compared, {} patched values held, {} found in the re-saves, {} re-evaluated, {} worlds not compared ({} composed), {} referee checked",
            OWN_SAVES.len(),
            total.objects,
            total.held,
            total.in_save,
            total.reevaluated,
            total.world_skipped,
            total.composed,
            total.referee
        );
        let patched: usize = OWN_SAVES.iter().map(|(_, _, p)| p.len()).sum();
        if fails == 0
            && total.objects > 0
            && total.held == patched
            && total.in_save == patched
            && total.reevaluated == 1
            && total.composed > 0
            && total.referee > 0
        {
            checked += 1;
            // the line the gate's round-trip part counts (`scripts/roundtrip.sh`)
            eprintln!(
                "own saves {r}: {} files checked, values held",
                OWN_SAVES.len()
            );
        }
    }
    assert!(
        failed.is_empty(),
        "own saves failures:\n{}",
        failed.join("\n")
    );
    assert_eq!(checked, RELEASES.len());
}

// ---------- the program's reopening of our animated and constrained edits ----------

/// Which of the program's builds holds an edited file's report before the edits: its build of
/// the scene with an animated visibility bit, or of the scene with a constrained object.
#[derive(Clone, Copy, Debug)]
enum BuiltFrom {
    Animated,
    Constrained,
}

/// One of our edits of a program's build, by the object's name.
#[derive(Clone, Copy, Debug)]
enum Edit {
    /// Its location, written by the writer or the live tier.
    Loc(&'static str, [f32; 3]),
    /// Disabled in viewports, by the writer or the live tier.
    Hide(&'static str),
    /// Its animated viewport bit, cleared in our input by a raw byte patch after the writer
    /// refused the edit (`blend_driven`): the row expects the program's report to hold its keyed
    /// value (set).
    Restored(&'static str),
}

impl Edit {
    fn object(&self) -> &'static str {
        match *self {
            Edit::Loc(o, _) | Edit::Hide(o) | Edit::Restored(o) => o,
        }
    }

    /// The text the inputs' record of the edits (`PATCHES.tsv`, its `expected` column) holds
    /// for this edit.
    fn recorded(&self) -> String {
        match *self {
            Edit::Loc(o, v) => format!("@ob:{o} loc {},{},{}", v[0], v[1], v[2]),
            Edit::Hide(o) => format!("@ob:{o} hide_viewport true"),
            Edit::Restored(o) => format!("@ob:{o} hide_viewport false (raw)"),
        }
    }
}

/// One edited file: its name, the build it was made from (and that build's file name in the
/// report before), our edits of it, and how many objects in it are under constraints.
struct Reopened {
    file: &'static str,
    from: BuiltFrom,
    built: &'static str,
    edits: &'static [Edit],
    constrained: usize,
}

/// The program's builds we edited (the same at both releases): its build of a scene with an
/// animated visibility bit (`Blinking`'s viewport bit keyed by an action, `Ghost`'s render bit
/// driven), edited by the writer and by the live tier; and of a scene with an object under a
/// track-to and a child-of constraint (`Follower`), edited by the writer (which writes under
/// constraints, with its warning) and by the live tier (which refuses a constrained placement,
/// so only `Free` moved).
const REOPENED: &[Reopened] = &[
    Reopened {
        file: "visibility_animated.blend",
        from: BuiltFrom::Animated,
        built: "visibility_animated.blend",
        edits: &[Edit::Hide("Plain"), Edit::Restored("Blinking")],
        constrained: 0,
    },
    Reopened {
        file: "visibility_live.blend",
        from: BuiltFrom::Animated,
        built: "visibility_animated.blend",
        edits: &[Edit::Hide("Plain")],
        constrained: 0,
    },
    Reopened {
        file: "constrained.blend",
        from: BuiltFrom::Constrained,
        built: "constrained.blend",
        edits: &[
            Edit::Loc("Follower", [1.0, 1.0, 0.0]),
            Edit::Loc("Free", [-2.0, 1.0, 0.0]),
        ],
        constrained: 1,
    },
    Reopened {
        file: "constrained_live.blend",
        from: BuiltFrom::Constrained,
        built: "constrained.blend",
        edits: &[Edit::Loc("Free", [-2.0, 2.0, 0.0])],
        constrained: 1,
    },
];

/// The tolerance of the constrained composition against the program's evaluated world.
const STACK_TOLERANCE: f64 = 1e-5;

/// A field of one of the driver's `referee: anim {...}` lines: a string's text, or a number's
/// or literal's as printed. The lines are flat objects whose strings hold no escape.
fn anim_field(json: &str, key: &str) -> Result<Option<String>, String> {
    let pat = format!("\"{key}\":");
    let Some(at) = json.find(&pat) else {
        return Ok(None);
    };
    let rest = &json[at + pat.len()..];
    if let Some(s) = rest.strip_prefix('"') {
        let end = s
            .find('"')
            .ok_or_else(|| format!("an unclosed string in {json}"))?;
        if s[..end].contains('\\') {
            return Err(format!("an escaped string the row doesn't read: {json}"));
        }
        return Ok(Some(s[..end].to_owned()));
    }
    let end = rest
        .find([',', '}'])
        .ok_or_else(|| format!("an unclosed value in {json}"))?;
    Ok(Some(rest[..end].trim().to_owned()))
}

/// One constraint of an object, as the driver's log prints it, in the stack's order.
#[derive(Debug)]
struct Constraint {
    object: String,
    kind: String,
    target: String,
    track: String,
    up: String,
    inverse: [f64; 16],
}

/// What the driver's log of one file says of its animation: each constraint in stack order,
/// each object's constraint count, and the F-curves (object, data path, value at the frame).
#[derive(Debug, Default)]
struct AnimLog {
    constraints: Vec<Constraint>,
    counts: Vec<(String, usize)>,
    fcurves: Vec<(String, String, String)>,
}

fn anim_log(text: &str) -> Result<AnimLog, String> {
    let mut a = AnimLog::default();
    let need = |j: &str, k: &str| -> Result<String, String> {
        anim_field(j, k)?.ok_or_else(|| format!("no {k} in {j}"))
    };
    for l in text.lines() {
        if let Some(rest) = l.strip_prefix("referee: '") {
            // `referee: 'NAME' has N constraint(s); ...`
            if let Some((name, tail)) = rest.split_once("' has ") {
                if let Some((n, _)) = tail.split_once(" constraint(s)") {
                    let n: usize = n.parse().map_err(|_| format!("a count: {l}"))?;
                    a.counts.push((name.to_owned(), n));
                }
            }
            continue;
        }
        let Some(j) = l.strip_prefix("referee: anim ") else {
            continue;
        };
        match need(j, "kind")?.as_str() {
            "constraint" => {
                let kind = need(j, "type")?;
                if need(j, "mute")? != "false" || need(j, "influence")? != "1.0" {
                    return Err(format!(
                        "a muted constraint or one of partial influence: the row composes neither: {j}"
                    ));
                }
                let inverse = match anim_field(j, "inverse_matrix")? {
                    Some(t) => {
                        let v: Vec<f64> = t
                            .split(',')
                            .map(|x| x.parse::<f64>().map_err(|_| format!("a matrix: {j}")))
                            .collect::<Result<_, _>>()?;
                        v.try_into().map_err(|_| format!("a matrix of 16: {j}"))?
                    }
                    None => builds_support::IDENTITY,
                };
                a.constraints.push(Constraint {
                    object: need(j, "object")?,
                    target: need(j, "target")?,
                    track: anim_field(j, "track_axis")?.unwrap_or_default(),
                    up: anim_field(j, "up_axis")?.unwrap_or_default(),
                    kind,
                    inverse,
                });
            }
            "fcurve" => a.fcurves.push((
                need(j, "object")?,
                need(j, "data_path")?,
                need(j, "curve_at_frame")?,
            )),
            _ => {}
        }
    }
    for (name, n) in &a.counts {
        let listed = a.constraints.iter().filter(|c| &c.object == name).count();
        if listed != *n {
            return Err(format!(
                "{name}: the log counts {n} constraints and lists {listed}"
            ));
        }
    }
    Ok(a)
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit3(a: [f64; 3], what: &str) -> Result<[f64; 3], String> {
    let n = dot3(a, a).sqrt();
    if n < 1e-9 {
        return Err(format!("{what}: no direction"));
    }
    Ok([a[0] / n, a[1] / n, a[2] / n])
}

/// A track-to constraint on `m` (column-major, world space): the location kept, the track axis
/// pointed from it at `at` (negated for a `TRACK_NEGATIVE_` axis), the up axis toward the
/// world's +Z made perpendicular to it, the third axis completing a right-handed frame, each
/// axis keeping its scale.
fn track_to(m: &[f64; 16], at: [f64; 3], track: &str, up: &str) -> Result<[f64; 16], String> {
    let (ti, neg) = match track {
        "TRACK_X" => (0, false),
        "TRACK_Y" => (1, false),
        "TRACK_Z" => (2, false),
        "TRACK_NEGATIVE_X" => (0, true),
        "TRACK_NEGATIVE_Y" => (1, true),
        "TRACK_NEGATIVE_Z" => (2, true),
        t => return Err(format!("a track axis the row doesn't compose: {t:?}")),
    };
    let ui = match up {
        "UP_X" => 0,
        "UP_Y" => 1,
        "UP_Z" => 2,
        u => return Err(format!("an up axis the row doesn't compose: {u:?}")),
    };
    if ti == ui {
        return Err("the track axis is the up axis".into());
    }
    let col = |i: usize| [m[i * 4], m[i * 4 + 1], m[i * 4 + 2]];
    let scale = [0, 1, 2].map(|i| dot3(col(i), col(i)).sqrt());
    let pos = [m[12], m[13], m[14]];
    let mut t = unit3(sub3(at, pos), "track-to: the target at the owner")?;
    if neg {
        t = t.map(|x| -x);
    }
    let w = [0.0, 0.0, 1.0];
    let d = dot3(w, t);
    let u = unit3(
        [w[0] - d * t[0], w[1] - d * t[1], w[2] - d * t[2]],
        "track-to: the up axis along the track axis",
    )?;
    let k = 3 - ti - ui;
    let third = if (ti + 1) % 3 == ui {
        cross3(t, u)
    } else {
        cross3(u, t)
    };
    let mut out = builds_support::IDENTITY;
    for (i, v) in [(ti, t), (ui, u), (k, third)] {
        for (r, x) in v.iter().enumerate() {
            out[i * 4 + r] = x * scale[i];
        }
    }
    out[12..15].copy_from_slice(&pos);
    Ok(out)
}

/// `s`'s world under its constraint stack, composed here in `f64`, in stack order, from the
/// program's own values: its matrix without constraints ([`composed`]), then each constraint:
/// a track-to aimed at its target's world position, a child-of composing the target's world and
/// the constraint's inverse on it. A target under constraints of its own isn't composed.
fn stack_world(s: &Seen, stack: &[&Constraint], after: &[&Seen]) -> Result<[f64; 16], String> {
    let mut m = composed(s, after, 0)?
        .ok_or_else(|| format!("{}: a rotation mode the row doesn't compose", s.name))?;
    for c in stack {
        let t = after
            .iter()
            .find(|x| x.name == c.target)
            .ok_or_else(|| format!("{}: no target {}", s.name, c.target))?;
        let tw = composed(t, after, 0)?
            .ok_or_else(|| format!("{}: a rotation mode the row doesn't compose", t.name))?;
        m = match c.kind.as_str() {
            "TRACK_TO" => track_to(&m, [tw[12], tw[13], tw[14]], &c.track, &c.up)?,
            "CHILD_OF" => builds_support::mul(&builds_support::mul(&tw, &c.inverse), &m),
            k => {
                return Err(format!(
                    "{}: a constraint the row doesn't compose: {k}",
                    s.name
                ))
            }
        };
    }
    Ok(m)
}

/// What one edited file's checks counted.
#[derive(Default, Debug)]
struct ReopenedCounts {
    /// Objects of the report before found after, every column compared.
    objects: usize,
    /// Edits as the program reports them after reopening.
    held: usize,
    /// Edits our reader finds in our input.
    in_input: usize,
    /// Edits our reader finds in the program's re-save.
    in_save: usize,
    /// Animated bits cleared in our input that the program's report holds at their keyed value.
    restored: usize,
    /// Objects under constraints whose evaluated world equals the stack composed here.
    stacked: usize,
    /// Of edited objects whose world isn't compared with before, `world_eval` equal to the
    /// composition from the program's values.
    composed: usize,
    /// Objects whose evaluated world equals the driver's `world_eval`.
    referee: usize,
    /// Objects not evaluated, their world exactly the identity.
    unevaluated: usize,
    /// The constrained composition's largest difference from the program's evaluated world.
    stack_error: f64,
}

/// The program's report of one file: its objects, and its lines as printed.
struct Report<'a> {
    seen: &'a [&'a Seen],
    raw: &'a std::collections::BTreeMap<(String, String), Vec<String>>,
}

/// One edited file at one release: the record of the edits names each; every edit as the
/// program reports it after reopening (an animated bit cleared in our input, as our reader
/// finds it there, expected reported set: its keyed value); nothing else of any object changed
/// against the program's report of its build before the edits; each object under constraints
/// evaluated to the stack composed here, each other one evaluated to the driver's `world_eval`
/// (the identity when not evaluated), and an edited object's `world_eval` the composition of its
/// own values; our reader finding every edit in our input and in the program's re-save.
#[allow(clippy::too_many_arguments)]
fn reopened_file(
    spec: &Reopened,
    record: &str,
    log: &AnimLog,
    after: &Report<'_>,
    before: &Report<'_>,
    input: &Path,
    resaved: &Path,
    c: &mut ReopenedCounts,
) -> Result<(), String> {
    for e in spec.edits {
        if !record.contains(&e.recorded()) {
            return Err(format!(
                "the record of the edits doesn't name {:?}",
                e.recorded()
            ));
        }
    }
    if after.seen.is_empty() {
        return Err("the program printed nothing of it after the edits".into());
    }
    if before.seen.is_empty() {
        return Err("no report of its build before the edits".into());
    }
    let want: BTreeSet<&str> = before.seen.iter().map(|s| s.name.as_str()).collect();
    let got: BTreeSet<&str> = after.seen.iter().map(|s| s.name.as_str()).collect();
    if want != got {
        return Err(format!(
            "objects after the edits: missing {:?}, new {:?}",
            want.difference(&got).collect::<Vec<_>>(),
            got.difference(&want).collect::<Vec<_>>()
        ));
    }
    let under: BTreeSet<&str> = log.constraints.iter().map(|k| k.object.as_str()).collect();
    for b in before.seen {
        let a = after
            .seen
            .iter()
            .find(|x| x.name == b.name)
            .ok_or_else(|| format!("{}: not in the report after", b.name))?;
        let mut loc = false;
        let mut hide = false;
        for e in spec.edits.iter().filter(|e| e.object() == b.name) {
            match *e {
                Edit::Loc(_, v) => {
                    loc = true;
                    if a.loc != v {
                        return Err(format!("{}: location {:?}, written {v:?}", a.name, a.loc));
                    }
                }
                Edit::Hide(_) => {
                    hide = true;
                    if !a.hidden[0] {
                        return Err(format!("{}: not disabled in viewports", a.name));
                    }
                }
                Edit::Restored(o) => {
                    if !a.hidden[0] || !b.hidden[0] {
                        return Err(format!(
                            "{o}: disabled in viewports {} after, {} before: want its keyed value (set) both",
                            a.hidden[0], b.hidden[0]
                        ));
                    }
                    if !log
                        .fcurves
                        .iter()
                        .any(|(n, p, v)| n == o && p == "hide_viewport" && v == "1.0")
                    {
                        return Err(format!(
                            "{o}: the log names no F-curve keying hide_viewport set at the frame"
                        ));
                    }
                    c.restored += 1;
                }
            }
            c.held += 1;
        }
        let skip_world = loc || hide;
        let rb = before
            .raw
            .get(&(printed(spec.built), printed(&b.name)))
            .ok_or_else(|| format!("{}: no raw line before", b.name))?;
        let ra = after
            .raw
            .get(&(printed(spec.file), printed(&a.name)))
            .ok_or_else(|| format!("{}: no raw line after", a.name))?;
        for i in 1..COLUMNS {
            let same = match i {
                4 => loc || rb[i] == ra[i],
                7 => hide || rb[i] == ra[i],
                11 | 24 => skip_world || rb[i] == ra[i],
                _ => rb[i] == ra[i],
            };
            if !same {
                return Err(format!("{}: column {} changed", a.name, i + 1));
            }
        }
        if skip_world {
            let m = composed(a, after.seen, 0)?
                .ok_or_else(|| format!("{}: no composition of it", a.name))?;
            let w = a
                .world_eval
                .as_ref()
                .ok_or_else(|| format!("{}: no world_eval", a.name))?;
            if !m.iter().zip(w).all(|(&x, &y)| near(x, y)) {
                return Err(format!(
                    "{}: world_eval isn't the composition of its own values",
                    a.name
                ));
            }
            c.composed += 1;
        }
        c.objects += 1;
    }
    // the referee after the edits: an object under constraints against the stack composed
    // here (world_eval leaves constraints out), every other against world_eval
    for a in after.seen {
        if !evaluated(a)? {
            if a.world != builds_support::IDENTITY {
                return Err(format!(
                    "{}: not evaluated by the program, but its world isn't the identity",
                    a.name
                ));
            }
            c.unevaluated += 1;
            continue;
        }
        if under.contains(a.name.as_str()) {
            let stack: Vec<&Constraint> = log
                .constraints
                .iter()
                .filter(|k| k.object == a.name)
                .collect();
            if stack.iter().any(|k| under.contains(k.target.as_str())) {
                return Err(format!("{}: a target under constraints", a.name));
            }
            let m = stack_world(a, &stack, after.seen)?;
            let err = m
                .iter()
                .zip(&a.world)
                .map(|(&x, &y)| (x - y).abs())
                .fold(0.0, f64::max);
            c.stack_error = c.stack_error.max(err);
            if err > STACK_TOLERANCE {
                return Err(format!(
                    "{}: the program's evaluated world is {err:e} from the stack composed here",
                    a.name
                ));
            }
            c.stacked += 1;
            continue;
        }
        let w = a
            .world_eval
            .as_ref()
            .ok_or_else(|| format!("{}: no world_eval", a.name))?;
        if !w.iter().zip(&a.world).all(|(&x, &y)| near(x, y)) {
            return Err(format!(
                "{}: world_eval isn't the program's evaluated world",
                a.name
            ));
        }
        c.referee += 1;
    }
    if c.stacked != spec.constrained {
        return Err(format!(
            "{} objects under constraints composed, want {}",
            c.stacked, spec.constrained
        ));
    }
    // our reader on our input and on the program's re-save
    for (path, is_input) in [(input, true), (resaved, false)] {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let m = ours_of(&bytes)?;
        let side = if is_input { "our input" } else { "the re-save" };
        for e in spec.edits {
            let k = key_named(&m, e.object())?;
            match *e {
                Edit::Loc(_, v) => {
                    let got: Vec<f32> = loc_of(&m, &k)?
                        .split(',')
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    if got != v.to_vec() {
                        return Err(format!("our reader, {side}: {k} loc {got:?}, want {v:?}"));
                    }
                }
                Edit::Hide(_) => {
                    if restrict(&m, &k)? & 1 == 0 {
                        return Err(format!("our reader, {side}: {k} not disabled in viewports"));
                    }
                }
                Edit::Restored(_) => {
                    // our input holds the raw clear; the re-save is expected to hold the keyed value
                    if (restrict(&m, &k)? & 1 != 0) == is_input {
                        return Err(format!(
                            "our reader, {side}: {k} viewport bit {}, want {}",
                            restrict(&m, &k)? & 1,
                            u8::from(!is_input)
                        ));
                    }
                }
            }
            if is_input {
                c.in_input += 1;
            } else {
                c.in_save += 1;
            }
        }
    }
    Ok(())
}

/// A folder the row reads, named by `var`.
fn folder_of(var: &str, what: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{var}: {what} (PROVENANCE.md)"))
}

#[test]
#[ignore = "files kept on a build node: scripts/roundtrip.sh runs it"]
fn the_program_reopens_our_animated_and_constrained_edits() {
    let dir = folder_of(
        "BLEND_REOPENED_DIR",
        "the program's reports, logs and re-saves of our edited files",
    );
    let input = folder_of("BLEND_REOPENED_IN", "our edited files and their record");
    let animated = folder_of(
        "BLEND_REOPENED_ANIMATED_BEFORE",
        "the program's report of its build with an animated visibility bit",
    );
    let constrained = folder_of(
        "BLEND_REOPENED_CONSTRAINED_BEFORE",
        "the program's report of its build with a constrained object",
    );
    let record =
        std::fs::read_to_string(input.join("PATCHES.tsv")).expect("the record of the edits");
    let mut failed = Vec::new();
    let mut checked = 0;
    for r in RELEASES {
        let d = dir.join(r);
        let after = seen(&d);
        let after_raw = raw_lines(&d);
        let mut total = ReopenedCounts::default();
        let mut fails = 0;
        for spec in REOPENED {
            let mine: Vec<&Seen> = after.iter().filter(|s| s.file == spec.file).collect();
            // the report before: the build's lines of its file alone
            let b_dir = match spec.from {
                BuiltFrom::Animated => animated.join(r),
                BuiltFrom::Constrained => constrained.join(r),
            };
            let b = seen_of(&b_dir, spec.built);
            let b_raw = raw_lines_of(&b_dir, Some(spec.built));
            let was: Vec<&Seen> = b.iter().collect();
            let mut c = ReopenedCounts::default();
            let result = (|| -> Result<(), String> {
                // the record's line of this release and file: its `expected` column
                let line = record
                    .lines()
                    .map(|l| l.split('\t').collect::<Vec<_>>())
                    .find(|l| l.first() == Some(r) && l.get(1) == Some(&spec.file))
                    .ok_or("no line in the record of the edits")?;
                let expected = line
                    .get(4)
                    .ok_or("the record's line has no expected column")?;
                let log = std::fs::read_to_string(d.join(format!("{}.log", spec.file)))
                    .map_err(|e| format!("the driver's log: {e}"))?;
                let log = anim_log(&log)?;
                let resaved = d.join("resaved").join(spec.file);
                if !resaved.is_file() {
                    return Err("no re-save".into());
                }
                reopened_file(
                    spec,
                    expected,
                    &log,
                    &Report {
                        seen: &mine,
                        raw: &after_raw,
                    },
                    &Report {
                        seen: &was,
                        raw: &b_raw,
                    },
                    &input.join(r).join(spec.file),
                    &resaved,
                    &mut c,
                )
            })();
            match result {
                Ok(()) => eprintln!(
                    "reopened edits {r} {}: ok: {} objects compared with the build before ({:?}), {} edits held as the program reports them, {} found by our reader in our input, {} in its re-save, {} animated bits cleared in our input reported at their keyed value, {} objects under constraints composed (largest difference {:.1e}), {} edited worlds composed, {} referee checked, {} not evaluated",
                    spec.file,
                    c.objects,
                    spec.from,
                    c.held,
                    c.in_input,
                    c.in_save,
                    c.restored,
                    c.stacked,
                    c.stack_error,
                    c.composed,
                    c.referee,
                    c.unevaluated
                ),
                Err(e) => {
                    eprintln!("reopened edits {r} {}: FAIL: {e}", spec.file);
                    failed.push(format!("{r} {}: {e}", spec.file));
                    fails += 1;
                }
            }
            total.objects += c.objects;
            total.held += c.held;
            total.in_input += c.in_input;
            total.in_save += c.in_save;
            total.restored += c.restored;
            total.stacked += c.stacked;
            total.composed += c.composed;
            total.referee += c.referee;
            total.unevaluated += c.unevaluated;
            total.stack_error = total.stack_error.max(c.stack_error);
        }
        eprintln!(
            "reopened edits {r}: {} files, {fails} failed; {} objects compared, {} edits held, {} in our inputs, {} in the re-saves, {} at their keyed value, {} under constraints composed (largest difference {:.1e}), {} edited worlds composed, {} referee checked, {} not evaluated",
            REOPENED.len(),
            total.objects,
            total.held,
            total.in_input,
            total.in_save,
            total.restored,
            total.stacked,
            total.stack_error,
            total.composed,
            total.referee,
            total.unevaluated
        );
        let edits: usize = REOPENED.iter().map(|s| s.edits.len()).sum();
        let restorable = REOPENED
            .iter()
            .flat_map(|s| s.edits)
            .filter(|e| matches!(e, Edit::Restored(_)))
            .count();
        let under: usize = REOPENED.iter().map(|s| s.constrained).sum();
        if fails == 0
            && total.objects > 0
            && total.held == edits
            && total.in_input == edits
            && total.in_save == edits
            && total.restored == restorable
            && total.stacked == under
            && total.composed > 0
            && total.referee > 0
            && total.unevaluated > 0
        {
            checked += 1;
            // the line the gate's round-trip part counts (`scripts/roundtrip.sh`)
            eprintln!(
                "reopened edits {r}: {} files checked, values held",
                REOPENED.len()
            );
        }
    }
    assert!(
        failed.is_empty(),
        "reopened edits failures:\n{}",
        failed.join("\n")
    );
    assert_eq!(checked, RELEASES.len());
}
