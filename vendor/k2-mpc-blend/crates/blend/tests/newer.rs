//! Files of format versions 5.02 and 5.03 (`newer_support`): named, drawn, written and opened
//! live writable; past 5.03 declined and refused by the writer; a link placeholder linked, never
//! drawn and never written, of its stored type code's kind. The rows are in
//! `newer_support/mod.rs`, shared with the planted bugs (`tests/mutants.rs`). Here too: the live
//! edit tool (`examples/blend_live_edit.rs`) on a generated 5.03 file and on the program's own
//! 5.3.0 save kept in `tests/builds/`, its output read back by our reader.

mod driven_support;
mod edit_hook_support;
mod live_support;
mod meshread_support;
mod newer_support;

#[path = "../examples/blend_live_edit.rs"]
#[allow(dead_code)]
mod blend_live_edit;

use k2_mpc_base::session::Pack;
use k2_mpc_blend::live::{BlendLive, SCENE};
use k2_mpc_blend::{read_blend, read_scene, write::decompress, ReadLimits, SceneModel};

fn pass(row: fn() -> Result<(), String>) {
    // one line naming the row and its step, so a log shows where it failed
    if let Err(e) = row() {
        panic!("row failed at its step: {}", e.replace('\n', " | "));
    }
}

#[test]
fn mesh_read_draws_newer_files() {
    pass(newer_support::row_mesh_read_newer);
}

#[test]
fn newer_files_are_named() {
    pass(newer_support::row_names_newer);
}

#[test]
fn live_tier_opens_newer_files_writable() {
    pass(newer_support::row_live_newer_writable);
}

#[test]
fn person_edit_of_a_driven_channel_is_refused_at_the_call() {
    pass(newer_support::row_person_edit_of_driven_channel);
}

#[test]
fn writer_writes_newer_files() {
    pass(newer_support::row_writer_newer);
}

#[test]
fn live_edit_tool_writes_back_newer_files() {
    pass(row_live_edit_tool);
}

#[test]
fn link_placeholder_is_linked_never_drawn_or_written() {
    pass(newer_support::row_link_placeholder);
}

#[test]
fn link_placeholder_takes_its_type_codes_kind() {
    pass(newer_support::row_placeholder_kind);
}

fn model_of(bytes: &[u8]) -> Result<SceneModel, String> {
    let l = ReadLimits::default();
    let (_, raw) = decompress(bytes, &l).map_err(|e| e.to_string())?;
    let f = read_blend(&mut &raw[..], &l).map_err(|e| e.to_string())?;
    read_scene(&f).map_err(|e| e.to_string())
}

/// `field` of the object `key` as our reader prints it (`1,2,3`), or one of its named values.
fn field(m: &SceneModel, key: &str, field: &str) -> Option<String> {
    let id = m.by_key(key)?;
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == field)
        .map(|(_, v)| v.clone())
}

/// One run of the live edit tool on `input`: `Ok((the tool's line, the file written))`, or
/// `Err(the tool's line)`.
fn tool(input: &[u8], argv: &[&str]) -> Result<(String, Vec<u8>), String> {
    let argv: Vec<String> = argv.iter().map(|a| (*a).to_owned()).collect();
    let out = blend_live_edit::live_edit(input, &argv);
    match out {
        Ok((version, bytes)) => Ok((format!("LIVE ok {version} {}", bytes.len()), bytes)),
        Err(why) => Err(format!("LIVE FAIL {why}")),
    }
}

/// The live edit tool (`examples/blend_live_edit.rs`), as the referee runs it: on a generated
/// 5.03 file, `object place`, `object visibility` and `object rename` by an object's name each
/// write the file back (`LIVE ok 5.03 <bytes>`), our reader finding the value; a placement of
/// an object an action drives is refused `blend_driven` and one at 5.04 `live_version`, as the
/// live tier refuses them; on the program's own 5.3.0 save of the three cameras
/// (`tests/builds/5.3.0/three_cameras.blend`) a camera placed live writes back, our reader
/// finding it there and every other object's place as it was.
fn row_live_edit_tool() -> Result<(), String> {
    const ROW: &str = "row_live_edit_tool";
    let fail = |at: &str, e: String| format!("{ROW}: {at}: {e}");
    let gen = live_support::outliner(&live_support::Opts {
        version: 503,
        ..live_support::Opts::default()
    });
    let version = BlendLive::default().decode(&gen).ok().and_then(|d| {
        d.roots.iter().find_map(|(r, n)| match n {
            k2_mpc_base::doc::Neutral::Map(m) if r == SCENE => m
                .get("version")
                .and_then(k2_mpc_base::elem::Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
    });
    if version.as_deref() != Some("5.03") {
        return Err(fail(
            "control",
            format!("the generated file's version {version:?}"),
        ));
    }
    for (argv, key, name, want) in [
        (
            vec!["object", "place", "@name:Cube", "--loc", "1,2,3"],
            "@ob:Cube",
            "loc",
            "1,2,3",
        ),
        (
            vec!["object", "visibility", "@name:Cube", "--viewport", "off"],
            "@ob:Cube",
            "hide_viewport",
            "true",
        ),
        (
            vec![
                "object",
                "rename",
                "@name:Cube",
                "--name",
                "Box",
                "--break-links",
            ],
            "@ob:Box",
            "loc",
            "0,0,0.25",
        ),
    ] {
        let at = format!("5.03 {}", argv[1]);
        let (line, bytes) = tool(&gen, &argv).map_err(|e| fail(&at, e))?;
        println!("{ROW}: {at}: {line}");
        if line != format!("LIVE ok 5.03 {}", bytes.len()) {
            return Err(fail(&at, line));
        }
        let m = model_of(&bytes).map_err(|e| fail(&at, e))?;
        let got = field(&m, key, name);
        if got.as_deref() != Some(want) {
            return Err(fail(
                &at,
                format!("our reader: {key} {name} {got:?}, want {want}"),
            ));
        }
    }
    let refused = tool(&gen, &["object", "place", "@name:Walker", "--loc", "1,1,1"]);
    match &refused {
        Err(l) if l.starts_with("LIVE FAIL blend_driven: ") => println!("{ROW}: 5.03 driven: {l}"),
        other => return Err(fail("5.03 driven", format!("{other:?}"))),
    }
    let past = live_support::outliner(&live_support::Opts {
        version: 504,
        ..live_support::Opts::default()
    });
    match tool(&past, &["object", "place", "@name:Cube", "--loc", "1,1,1"]) {
        Err(l) if l.starts_with("LIVE FAIL live_version: ") => println!("{ROW}: 5.04: {l}"),
        other => return Err(fail("5.04", format!("{other:?}"))),
    }
    // the program's own 5.3.0 save
    let kept = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/builds/5.3.0/three_cameras.blend");
    let input =
        std::fs::read(&kept).map_err(|e| fail("5.3.0", format!("{}: {e}", kept.display())))?;
    let before = model_of(&input).map_err(|e| fail("5.3.0", e))?;
    let argv = ["object", "place", "@name:Camera.Long", "--loc", "-10,-14,6"];
    let (line, bytes) = tool(&input, &argv).map_err(|e| fail("5.3.0 place", e))?;
    println!("{ROW}: 5.3.0 three_cameras.blend: {line}");
    if line != format!("LIVE ok 5.03 {}", bytes.len()) {
        return Err(fail("5.3.0 place", line));
    }
    let after = model_of(&bytes).map_err(|e| fail("5.3.0 place", e))?;
    let mut others = 0;
    for id in before
        .ids
        .iter()
        .filter(|i| i.kind == "object" && i.library.is_none())
    {
        let Some(key) = id.key.as_deref() else {
            continue;
        };
        let (was, now) = (field(&before, key, "loc"), field(&after, key, "loc"));
        if key == "@ob:Camera.Long" {
            if now.as_deref() != Some("-10,-14,6") {
                return Err(fail("5.3.0 read back", format!("{key} loc {now:?}")));
            }
        } else if now != was || now.is_none() {
            return Err(fail(
                "5.3.0 read back",
                format!("{key} loc {was:?} -> {now:?}"),
            ));
        } else {
            others += 1;
        }
    }
    if others == 0 {
        return Err(fail("5.3.0 read back", "no other object compared".into()));
    }
    println!("{ROW}: 5.3.0: Camera.Long placed, {others} other objects' places as they were");
    Ok(())
}
