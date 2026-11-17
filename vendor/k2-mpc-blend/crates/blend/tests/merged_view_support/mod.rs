//! An app's save of the session's own written-back version: the kit's answer, and the pack's
//! check of a doc merged field by field from the version the app opened
//! (`Pack::check_merged_view`). Each row on the living room, after one live edit written back,
//! with the app's re-save made by the pack's writer from the written-back bytes. Each row
//! returns `Err("<row>: <step>: ...")`, a `control` step first that its plant leaves passing.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::Value;
use k2_mpc_base::session::{ConnId, DiskCheck, OpenedAt, Pack, Session, View};
use k2_mpc_blend::live::{BlendLive, OBJS, OBJ_PROPS};
use k2_mpc_blend::scene::key_of;
use k2_mpc_blend::testgen::scenes::living_room;
use k2_mpc_blend::{write, Change, Edit, ReadLimits};

use crate::live_support::{app_save, fx, run_based, stdout, Fx};

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

/// The live doc's roots.
fn roots(s: &Session, c: ConnId) -> Vec<(String, Neutral)> {
    s.read(c).expect("the doc reads").content
}

fn get(c: &[(String, Neutral)], root: &str, elem: &str, fld: &str) -> Option<String> {
    let k = format!("{elem}\u{1f}{fld}");
    c.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == root => m.get(&k).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    })
}

fn set(c: &mut [(String, Neutral)], root: &str, elem: &str, fld: &str, v: &str) {
    for (r, n) in c.iter_mut() {
        if let Neutral::Map(m) = n {
            if r == root {
                m.insert(format!("{elem}\u{1f}{fld}"), Value::str(v));
            }
        }
    }
}

/// The key of the object named `name`.
fn obj(c: &[(String, Neutral)], name: &str) -> Option<String> {
    c.iter()
        .find_map(|(r, n)| match n {
            Neutral::List(l) if r == OBJS => Some(l),
            _ => None,
        })?
        .iter()
        .map(|(k, _)| k.as_str().to_owned())
        .find(|k| get(c, OBJ_PROPS, k, "name").as_deref() == Some(name))
}

/// The location in an object's `xform`.
fn loc(c: &[(String, Neutral)], name: &str) -> Option<Vec<f32>> {
    let k = obj(c, name)?;
    let x = get(c, OBJ_PROPS, &k, "xform")?;
    Some(
        x.split(',')
            .take(3)
            .filter_map(|v| v.parse().ok())
            .collect(),
    )
}

/// The living room live, the sofa placed and written back; the version the app opened noted
/// right after when `pin`. The fixture, the session, the agent, and the written-back bytes.
fn written_back(
    tag: &str,
    row: &str,
    pin: bool,
) -> Result<(Fx, Arc<Session>, ConnId, Vec<u8>), String> {
    let f = fx(tag, |_| {});
    f.write(&living_room());
    let s = f.open();
    let a = f.agent(&s);
    let person = f.person(&s);
    let o = run_based(
        &s,
        a,
        &["object", "place", "@obj:15", "--loc", "-1.5,-1,0.1"],
    )
    .map_err(|e| format!("{row}: setup: the place: {e}"))?;
    step(o.exit == 0, row, "setup", || stdout(&o))?;
    let out = s
        .flush(a)
        .map_err(|e| format!("{row}: setup: the write-back: {e}"))?;
    step(out.written, row, "setup", || {
        "the write-back wrote nothing".into()
    })?;
    if pin {
        s.note_opened(person, OpenedAt::FileNow)
            .map_err(|e| format!("{row}: setup: note_opened: {e}"))?;
    }
    let wb = f.bytes();
    Ok((f, s, a, wb))
}

/// The app's re-save: `wb` with `edits` patched in by the pack's writer.
fn resave(row: &str, wb: &[u8], edits: &[Edit]) -> Result<Vec<u8>, String> {
    write(wb, edits, &ReadLimits::default())
        .map(|w| w.bytes)
        .map_err(|e| format!("{row}: setup: the app's re-save: {e}"))
}

const RUG_AT: [f32; 3] = [0.4, -0.5, 0.005];

fn rug_moved() -> Edit {
    Edit::new("@ob:Rug", Change::Location(RUG_AT))
}

/// With no version noted, a re-save of the written-back version with the rug moved keeps both:
/// the kit wrote the file since the last outside save, and nothing says what the app opened.
pub fn row_resave_unpinned_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_resave_unpinned_keeps_both";
    let (f, s, a, wb) = written_back("mv-unpinned", ROW, false)?;
    let save = resave(ROW, &wb, &[rug_moved()])?;
    step(save != wb, ROW, "control", || {
        "the re-save is the same bytes".into()
    })?;
    app_save(&f.path(), &save);
    let got = s.file_changed();
    step(
        matches!(got, Ok(DiskCheck::Conflict { .. })),
        ROW,
        "keeps both",
        || format!("{got:?}"),
    )?;
    let c = roots(&s, a);
    step(
        loc(&c, "Rug").as_deref() == Some(&RUG_AT[..]),
        ROW,
        "the save",
        || format!("the rug is at {:?}", loc(&c, "Rug")),
    )
}

/// The written-back bytes saved again as they are: nothing.
pub fn row_resave_identical_unchanged() -> Result<(), String> {
    const ROW: &str = "row_resave_identical_unchanged";
    let (f, s, _, wb) = written_back("mv-identical", ROW, false)?;
    app_save(&f.path(), &wb);
    let got = s.file_changed();
    step(
        matches!(got, Ok(DiskCheck::Unchanged)),
        ROW,
        "unchanged",
        || format!("{got:?}"),
    )
}

/// The files beside the doc's (a conflict copy is one).
fn beside(f: &Fx) -> Vec<String> {
    std::fs::read_dir(&f.ws)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != crate::live_support::NAME)
                .collect()
        })
        .unwrap_or_default()
}

/// With the version the app opened noted, a re-save of the written-back version with the rug
/// moved merges: taken in (no copy kept), the rug where the save put it, the session's own
/// placed sofa kept.
pub fn row_pinned_resave_merges() -> Result<(), String> {
    const ROW: &str = "row_pinned_resave_merges";
    let (f, s, a, wb) = written_back("mv-pinned", ROW, true)?;
    let save = resave(ROW, &wb, &[rug_moved()])?;
    step(save != wb, ROW, "control", || {
        "the re-save is the same bytes".into()
    })?;
    app_save(&f.path(), &save);
    let got = s.file_changed();
    step(
        matches!(got, Ok(DiskCheck::Ingested { .. })),
        ROW,
        "merged",
        || format!("the kit answered {got:?}"),
    )?;
    let c = roots(&s, a);
    step(
        loc(&c, "Rug").as_deref() == Some(&RUG_AT[..]),
        ROW,
        "the save",
        || format!("the rug is at {:?}", loc(&c, "Rug")),
    )?;
    step(
        loc(&c, "Sofa").as_deref() == Some(&[-1.5, -1.0, 0.1][..]),
        ROW,
        "the session's edit",
        || format!("the sofa is at {:?}", loc(&c, "Sofa")),
    )?;
    step(beside(&f).is_empty(), ROW, "no copy", || {
        format!("beside the file: {:?}", beside(&f))
    })?;
    println!("{ROW}: {got:?}");
    Ok(())
}

/// With the version the app opened noted, the session renames one chair `Seat` (not yet written
/// back) and the app's re-save renames another `Seat`: each change valid alone, the merge two
/// objects of one name. The kit keeps both (the save as the doc, the kit's version as a copy):
/// never a merge the format forbids, never a change dropped.
pub fn row_pinned_duplicate_name_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_pinned_duplicate_name_keeps_both";
    let (f, s, a, wb) = written_back("mv-names", ROW, true)?;
    let c = roots(&s, a);
    let chair = obj(&c, "Chair.001").ok_or(format!("{ROW}: no Chair.001"))?;
    let addr = format!("@obj:{}", &chair[1..]);
    let o = run_based(
        &s,
        a,
        &["object", "rename", &addr, "--name", "Seat", "--break-links"],
    )
    .map_err(|e| format!("{ROW}: control: the rename: {e}"))?;
    step(o.exit == 0, ROW, "control", || stdout(&o))?;
    let save = resave(
        ROW,
        &wb,
        &[Edit::new("@ob:Chair.002", Change::Name("Seat".into()))],
    )?;
    app_save(&f.path(), &save);
    let got = s.file_changed();
    let copy = match &got {
        Ok(DiskCheck::Conflict { copy, .. }) => copy.clone(),
        other => return Err(format!("{ROW}: keeps both: the kit answered {other:?}")),
    };
    step(
        copy.as_deref().is_some_and(|p| Path::new(p).exists()),
        ROW,
        "the copy",
        || format!("no conflict copy on disk: {copy:?}"),
    )?;
    let c = roots(&s, a);
    let named: Vec<String> = ["Chair.001", "Chair.002", "Seat"]
        .iter()
        .filter(|n| obj(&c, n).is_some())
        .map(|n| (*n).to_owned())
        .collect();
    step(
        named == ["Chair.001", "Seat"],
        ROW,
        "the save is the doc",
        || format!("names present: {named:?}"),
    )
}

/// One broken rule: its name, the code it is refused by, and the fields set (element, field,
/// value).
type Case = (
    &'static str,
    &'static str,
    Vec<(String, &'static str, String)>,
);

/// The pack's rules on a merged doc, each broken on its own in the living room's content: the
/// content as decoded passes; two local objects of one name, a name and file key that don't
/// match, a name too long for its field, an xform that isn't nine numbers, a visibility bit
/// that isn't 0 or 1, a quaternion on an Euler object, animated bits that aren't visibility
/// properties, a collection that isn't listed, a parent that isn't listed: each refused with
/// its code.
pub fn row_merged_view_rules() -> Result<(), String> {
    const ROW: &str = "row_merged_view_rules";
    let pack = BlendLive::default();
    let c0 = pack
        .decode(&living_room())
        .map_err(|e| format!("{ROW}: setup: {e}"))?
        .roots;
    let check = |c: &[(String, Neutral)]| pack.check_merged_view(&View::of_content(c));
    let got = check(&c0);
    step(got.is_ok(), ROW, "control", || format!("{got:?}"))?;
    let k = |n: &str| obj(&c0, n).unwrap_or_default();
    let nid = |n: &str| key_of("OB", n.as_bytes(), None).unwrap_or_default();
    let long = "L".repeat(200);
    let cases: Vec<Case> = vec![
        (
            "one name twice",
            "blend_merge_name_taken",
            vec![
                (k("Chair.002"), "name", "Chair.001".into()),
                (k("Chair.002"), "nid", nid("Chair.001")),
            ],
        ),
        (
            "name and key apart",
            "blend_merge_nid",
            vec![(k("Sofa"), "name", "Couch".into())],
        ),
        (
            "name too long",
            "blend_merge_name",
            vec![
                (k("Sofa"), "name", long.clone()),
                (k("Sofa"), "nid", nid(&long)),
            ],
        ),
        (
            "xform",
            "blend_merge_value",
            vec![(k("Sofa"), "xform", "1,2".into())],
        ),
        (
            "visibility",
            "blend_merge_value",
            vec![(k("Sofa"), "hidden", "2".into())],
        ),
        (
            "quaternion on Euler",
            "blend_merge_value",
            vec![(k("Sofa"), "quat", "1,0,0,0".into())],
        ),
        (
            "animated bits",
            "blend_merge_value",
            vec![(k("Sofa"), "visanim", "hide_viewport,hide_all".into())],
        ),
        (
            "collection",
            "blend_merge_holder",
            vec![(k("Sofa"), "coll", "c99".into())],
        ),
        (
            "parent",
            "blend_merge_parent",
            vec![(k("Book.001"), "parent", "o999".into())],
        ),
    ];
    let mut n = 0;
    for (what, code, sets) in &cases {
        let mut c = c0.clone();
        for (elem, fld, v) in sets {
            set(&mut c, OBJ_PROPS, elem, fld, v);
        }
        let got = check(&c);
        step(
            got.as_ref().err().map(|d| d.code) == Some(*code),
            ROW,
            what,
            || format!("{got:?}, want {code}"),
        )?;
        n += 1;
    }
    println!("{ROW}: the decoded content passes; {n} broken rules refused by code");
    Ok(())
}
