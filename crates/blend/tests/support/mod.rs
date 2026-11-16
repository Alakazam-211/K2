//! A scene shared by the pack's tests: built here (`k2_mpc_blend::testgen`), never from another
//! program's files.
#![allow(dead_code)]

pub mod zstd;

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_blend::testgen::{ObjectSpec, SceneBuilder};

/// Generous index limits.
pub fn limits() -> IndexLimits {
    IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX)
}

/// An object with data `data` (`ME:Cube`) and nothing else set.
pub fn spec(data: Option<&str>) -> ObjectSpec {
    ObjectSpec {
        data: data.map(str::to_owned),
        ..ObjectSpec::default()
    }
}

/// What may differ between two versions of the studio.
#[derive(Debug, Clone)]
pub struct Studio {
    /// Added to every address: another save of the same scene.
    pub shift: u64,
    /// The cube object's location.
    pub cube_loc: [f32; 3],
    /// The cube object's name.
    pub cube: String,
}

impl Default for Studio {
    fn default() -> Self {
        Self {
            shift: 0,
            cube_loc: [0.0, 0.0, 0.25],
            cube: "Cube".into(),
        }
    }
}

/// A scene with a camera object and two collections (one nested, one object in two),
/// objects with data, a parent and material slots, a library and a material linked from it.
pub fn studio(s: &Studio) -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.shift = s.shift;
    let cube = s.cube.as_str();
    b.ui();
    b.library("lib", "//assets/lib.blend");
    b.scene("Scene", &["Camera"], &["Set", "Props"], Some("Camera"));
    b.collection("Set", &[cube, "Light"], &["Inner"]);
    b.collection("Inner", &["Child"], &[]);
    b.collection("Props", &[cube], &[]);
    b.object(
        "Camera",
        &ObjectSpec {
            loc: [1.0, 2.0, 3.0],
            ..spec(Some("CA:Cam"))
        },
    );
    b.object(
        cube,
        &ObjectSpec {
            loc: s.cube_loc,
            materials: vec![Some("Red".into()), None],
            ..spec(Some("ME:Cube"))
        },
    );
    b.object(
        "Child",
        &ObjectSpec {
            parent: Some(cube.into()),
            restrict: 5,
            ..spec(None)
        },
    );
    b.object("Light", &spec(Some("LA:Lamp")));
    b.camera("Cam", 50.0);
    b.light("Lamp", 10.0);
    b.mesh("Cube", &[Some("Blue".into())], 4);
    b.material("Red", [1.0, 0.0, 0.0], None);
    b.material("Blue", [0.0, 0.0, 1.0], None);
    b.material("Shared", [0.5, 0.5, 0.5], Some("lib"));
    b.world("World");
    b.bytes()
}

/// Two named cameras with a mesh between them, each camera's name not its data's (`Wide Shot`
/// on `Lens 24`, `Close Up` on `Lens 85`); `Close Up` is the scene's camera.
pub fn two_cameras() -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.ui();
    b.scene(
        "Scene",
        &["Wide Shot", "Table", "Close Up"],
        &[],
        Some("Close Up"),
    );
    b.object("Wide Shot", &spec(Some("CA:Lens 24")));
    b.object("Table", &spec(Some("ME:Table")));
    b.object("Close Up", &spec(Some("CA:Lens 85")));
    b.camera("Lens 24", 24.0);
    b.camera("Lens 85", 85.0);
    b.mesh("Table", &[], 4);
    b.bytes()
}

/// `scene list` on [`two_cameras`] names each camera as the file stores its object's name, in
/// the objects' order, with its data, the scene it is active in and its lens; its text line
/// says the name too. Each step's failure names the step.
pub fn row_cameras_named() -> Result<(), String> {
    use k2_mpc_blend::verbs::list;
    let l = list(&two_cameras(), &k2_mpc_blend::ReadLimits::default())
        .map_err(|e| format!("setup: scene list: {e}"))?;
    let step = |ok: bool, name: &str, why: String| {
        if ok {
            Ok(())
        } else {
            Err(format!("cameras, step {name}: {why}"))
        }
    };
    let n = l.cameras.len();
    step(n == 2, "listed", format!("{n} camera(s) listed, want 2"))?;
    let names: Vec<&str> = l.cameras.iter().map(|c| c.name.as_str()).collect();
    step(
        names == ["Wide Shot", "Close Up"],
        "names",
        format!("names {names:?}"),
    )?;
    let objects: Vec<&str> = l.cameras.iter().map(|c| c.object.as_str()).collect();
    let in_order: Vec<&str> = l
        .objects
        .iter()
        .filter(|o| o.data.as_ref().is_some_and(|(_, kind)| kind == "camera"))
        .map(|o| o.address.as_str())
        .collect();
    step(
        objects == ["@ob:Wide%20Shot", "@ob:Close%20Up"] && objects == in_order,
        "order",
        format!("cameras {objects:?}, the objects' cameras {in_order:?}"),
    )?;
    let data: Vec<&str> = l.cameras.iter().map(|c| c.data.as_str()).collect();
    step(
        data == ["@ca:Lens%2024", "@ca:Lens%2085"],
        "data",
        format!("data {data:?}"),
    )?;
    let active: Vec<Vec<String>> = l.cameras.iter().map(|c| c.active_in.clone()).collect();
    step(
        active == [vec![], vec!["@sc:Scene".to_owned()]],
        "active",
        format!("active in {active:?}"),
    )?;
    let field = |i: usize, f: &str| {
        l.cameras[i]
            .fields
            .iter()
            .find(|(k, _)| k == f)
            .map(|(_, v)| v.clone())
    };
    let lens = [field(0, "lens"), field(1, "lens")];
    step(
        lens == [Some("24".to_owned()), Some("85".to_owned())],
        "lens",
        format!("lens {lens:?}"),
    )?;
    let kind = [field(0, "camera_type"), field(1, "camera_type")];
    step(
        kind == [
            Some("perspective".to_owned()),
            Some("perspective".to_owned()),
        ],
        "type",
        format!("camera_type {kind:?}"),
    )?;
    let lines = l.lines();
    let line = lines
        .iter()
        .find(|s| s.starts_with("@ob:Close%20Up camera "))
        .cloned()
        .unwrap_or_default();
    step(
        line.contains("\"Close Up\"") && line.contains("lens 85"),
        "line",
        format!("the line {line:?} in {lines:?}"),
    )?;
    println!("cameras: {n} listed, named {names:?}");
    Ok(())
}

/// The studio ([`studio`]) with one data block of `body` bytes of `fill` before its end block, as
/// one zstd frame of raw and RLE blocks: a few KiB stored, any size after decoding.
pub fn studio_padded(body: u64, fill: u8) -> Vec<u8> {
    let plain = studio(&Studio::default());
    let end = plain.len() - 24;
    assert_eq!(
        &plain[end..end + 4],
        b"ENDB",
        "the studio ends with its end block"
    );
    let mut head = plain[..end].to_vec();
    head.extend_from_slice(b"DATA");
    head.extend_from_slice(&i32::try_from(body).expect("under 2 GiB").to_le_bytes());
    head.extend_from_slice(&0u64.to_le_bytes());
    head.extend_from_slice(&0i32.to_le_bytes());
    head.extend_from_slice(&1i32.to_le_bytes());
    let mut blocks: Vec<zstd::Block<'_>> = head
        .chunks(zstd::BLOCK_MAX as usize)
        .map(zstd::Block::Raw)
        .collect();
    let mut left = body;
    while left > 0 {
        let n = left.min(u64::from(zstd::BLOCK_MAX));
        blocks.push(zstd::Block::Rle(fill, n as u32));
        left -= n;
    }
    blocks.push(zstd::Block::Raw(&plain[end..]));
    zstd::frame(17, &blocks)
}

/// The whole-file paths' default bound: a zstd file of a few KiB whose raw bytes are 1 MiB over
/// the live tier's raw cap (512 MiB) is refused by that cap, under the pack's default limits
/// (`BlendPack::default().limits`), by `scene list`, `object place` and the merge, before
/// anything past the cap is decoded. Each step's failure names the step.
pub fn row_whole_file_default_bounded() -> Result<(), String> {
    use k2_mpc_blend::merge::{merge3, Outcome};
    use k2_mpc_blend::verbs::{apply, list, Verb};
    let limits = k2_mpc_blend::BlendPack::default().limits;
    let cap = k2_mpc_blend::live::LiveLimits::default().read.max_raw_bytes;
    let fail = |name: &str, why: String| Err(format!("whole-file default, step {name}: {why}"));
    let want = format!("over a reader limit: bytes after decompression (limit {cap})");
    let file = |fill| studio_padded(cap + (1 << 20), fill);
    let base = file(0);
    if base.len() >= 64 << 10 {
        return fail("a small file", format!("{} bytes stored", base.len()));
    }
    match list(&base, &limits) {
        Err(e) if e.to_string().contains(&want) => {}
        Err(e) => return fail("scene list refused by the cap", e.to_string()),
        Ok(l) => {
            let n = l.objects.len();
            return fail(
                "scene list refused by the cap",
                format!("listed {n} objects"),
            );
        }
    }
    let verb = Verb::Place {
        object: "@ob:Cube".into(),
        loc: Some([1.0, 2.0, 3.0]),
        rot: None,
        scale: None,
    };
    match apply(&base, &verb, &limits) {
        Err(e) if e.to_string().contains(&want) => {}
        Err(e) => return fail("object place refused by the cap", e.to_string()),
        Ok(w) => {
            let n = w.bytes.len();
            return fail(
                "object place refused by the cap",
                format!("wrote {n} bytes"),
            );
        }
    }
    match merge3(&base, &file(1), &file(2), &limits) {
        Outcome::Unsupported(why) if why.contains(&want) => Ok(()),
        Outcome::Unsupported(why) => fail("the merge refused by the cap", why),
        Outcome::Merged(_) => fail("the merge refused by the cap", "merged".into()),
        other => fail("the merge refused by the cap", format!("{other:?}")),
    }
}
