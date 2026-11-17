//! Whole scene files for the tests and for a folder another program can open and save again:
//! each written by [`SceneBuilder`] from numbers written here, never from another program's
//! files. Every mesh is a box stand-in (eight corners, six quads) with the bounds its object
//! needs. The files hold no path but a library's relative one (`//plants.blend`), and no user,
//! machine, date or environment text.
//!
//! | File | What it holds |
//! |---|---|
//! | `living_room.blend` | [`living_room`]: a room of 26 objects in three collections |
//! | `living_room_zstd.blend` | [`living_room_zstd`]: the same room, stored zstd |
//! | `living_room_hidden.blend` | [`living_room_hidden`]: the same room, its `Props` collection hidden |
//! | `three_cameras.blend` | [`three_cameras`]: three perspective cameras, one orthographic, a mesh |
//! | `driven_quat.blend` | [`driven_quat`]: an object whose location's z an action animates, one turned by quaternion, one plain |
//! | `instanced.blend` | [`instanced`]: one mesh four objects share, and one object with its own |
//! | `large.blend` | [`large`]: 2,000 mesh objects in 100 collections, sharing 50 meshes |
//! | `empty_scene.blend` | [`empty_scene`]: a camera and an empty, no mesh |

use std::f32::consts::{FRAC_1_SQRT_2, FRAC_PI_2, FRAC_PI_4, PI};

use super::{
    zstd_seekable, CameraSpec, Driven, MeshLayouts, MeshSpec, ObjectSpec, Render, SceneBuilder,
};

/// A function that writes one scene file's bytes.
pub type WriteScene = fn() -> Vec<u8>;

/// Every scene: its file name and the function that writes it.
pub const ALL: &[(&str, WriteScene)] = &[
    ("living_room.blend", living_room),
    ("living_room_zstd.blend", living_room_zstd),
    ("living_room_hidden.blend", living_room_hidden),
    ("three_cameras.blend", three_cameras),
    ("driven_quat.blend", driven_quat),
    ("instanced.blend", instanced),
    ("large.blend", large),
    ("empty_scene.blend", empty_scene),
    ("camera_fits_wide.blend", camera_fits_wide),
    ("camera_fits_tall.blend", camera_fits_tall),
];

/// The version every scene is written at: 4.02, attribute-layer meshes, the classic header.
pub const VERSION: u16 = 402;

/// The zstd frame size [`living_room_zstd`] is stored in.
pub const ZSTD_FRAME: usize = 1 << 20;

const EMPTY: i16 = 0;
const MESH: i16 = 1;
const LIGHT: i16 = 10;
const CAMERA: i16 = 11;
/// `rotmode`: quaternion, and Euler XYZ.
const QUATERNION: i16 = 0;
const XYZ: i16 = 1;
/// `restrictflag` bits: hidden in viewports, and not rendered.
const HIDE_VIEWPORT: i16 = 1;
const HIDE_RENDER: i16 = 4;
/// A light's area type.
const AREA: i16 = 4;

/// A box from `min` to `max`: eight corners, six quads wound outwards.
pub fn box_mesh(min: [f32; 3], max: [f32; 3]) -> MeshSpec {
    let positions = (0..8)
        .map(|i| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        })
        .collect();
    let faces = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ]
    .iter()
    .map(|f| f.to_vec())
    .collect();
    MeshSpec {
        positions,
        faces,
        ..MeshSpec::default()
    }
}

/// A builder for a scene file at [`VERSION`]: meshes in attribute layers, cameras with their
/// sensors, scenes with their render settings (1920 by 1080, square pixels).
fn builder() -> SceneBuilder {
    SceneBuilder::saved(VERSION, MeshLayouts::LAYERS)
}

/// An object of type `ty` with data `data` (`ME:Cube`), placed by `xform` (location, Euler
/// rotation, scale).
fn placed(ty: i16, data: Option<&str>, xform: [f32; 9]) -> ObjectSpec {
    ObjectSpec {
        data: data.map(str::to_owned),
        ty,
        rotmode: XYZ,
        loc: [xform[0], xform[1], xform[2]],
        rot: [xform[3], xform[4], xform[5]],
        scale: [xform[6], xform[7], xform[8]],
        ..ObjectSpec::default()
    }
}

/// One object of the living room.
#[derive(Debug, Clone, Copy)]
pub struct RoomObject {
    /// Its name.
    pub name: &'static str,
    /// Its `type`.
    pub ty: i16,
    /// Its data, as `ME:Plane`.
    pub data: Option<&'static str>,
    /// Its parent's name.
    pub parent: Option<&'static str>,
    /// Location, Euler rotation, scale.
    pub xform: [f32; 9],
}

const fn room(
    name: &'static str,
    ty: i16,
    data: Option<&'static str>,
    parent: Option<&'static str>,
    xform: [f32; 9],
) -> RoomObject {
    RoomObject {
        name,
        ty,
        data,
        parent,
        xform,
    }
}

/// The living room's objects, in the order the file holds them.
pub const ROOM_OBJECTS: [RoomObject; 26] = [
    room(
        "Camera",
        CAMERA,
        Some("CA:Camera"),
        None,
        [-2.6, -3.4, 1.35, 1.4137167, 0.0, -0.62831855, 1.0, 1.0, 1.0],
    ),
    room(
        "Key light",
        LIGHT,
        Some("LA:Area"),
        None,
        [3.2, -2.4, 4.6, 0.6632251, 0.0, 0.55850536, 1.0, 1.0, 1.0],
    ),
    room(
        "Fill light",
        LIGHT,
        Some("LA:Area.001"),
        None,
        [-4.1, -1.6, 3.3, 0.9075712, 0.0, -1.012291, 1.0, 1.0, 1.0],
    ),
    room(
        "Floor",
        MESH,
        Some("ME:Plane"),
        None,
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 3.0, 2.5, 1.0],
    ),
    room(
        "Wall.Back",
        MESH,
        Some("ME:Wall"),
        None,
        [0.0, 2.5, 0.6, FRAC_PI_2, 0.0, 0.0, 3.0, 0.6, 1.0],
    ),
    room(
        "Wall.Left",
        MESH,
        Some("ME:Wall"),
        None,
        [-3.0, 0.0, 0.6, FRAC_PI_2, 0.0, FRAC_PI_2, 2.5, 0.6, 1.0],
    ),
    room(
        "Window",
        MESH,
        Some("ME:Window"),
        None,
        [1.2, 2.46, 0.72, FRAC_PI_2, 0.0, 0.0, 0.7, 0.3, 1.0],
    ),
    room(
        "Rug",
        MESH,
        Some("ME:Rug"),
        None,
        [0.2, -0.3, 0.005, 0.0, 0.0, 0.0, 1.6, 1.1, 1.0],
    ),
    room(
        "Ceiling fan",
        MESH,
        Some("ME:Fan"),
        None,
        [0.0, 0.0, 2.75, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Table",
        MESH,
        Some("ME:Table"),
        None,
        [0.6, 0.4, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Chair.001",
        MESH,
        Some("ME:Chair"),
        None,
        [0.6, -0.35, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Chair.002",
        MESH,
        Some("ME:Chair"),
        None,
        [0.6, 1.15, 0.0, 0.0, 0.0, PI, 1.0, 1.0, 1.0],
    ),
    room(
        "Chair.003",
        MESH,
        Some("ME:Chair"),
        None,
        [-0.15, 0.4, 0.0, 0.0, 0.0, -FRAC_PI_2, 1.0, 1.0, 1.0],
    ),
    room(
        "Chair.004",
        MESH,
        Some("ME:Chair"),
        None,
        [1.35, 0.4, 0.0, 0.0, 0.0, FRAC_PI_2, 1.0, 1.0, 1.0],
    ),
    room(
        "Sofa",
        MESH,
        Some("ME:Sofa"),
        None,
        [-1.9, -1.2, 0.0, 0.0, 0.0, FRAC_PI_2, 1.0, 1.0, 1.0],
    ),
    room(
        "Shelf",
        MESH,
        Some("ME:Shelf"),
        None,
        [2.6, 2.2, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Book.001",
        MESH,
        Some("ME:Book"),
        Some("Shelf"),
        [-0.3, 0.0, 1.12, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Book.002",
        MESH,
        Some("ME:Book"),
        Some("Shelf"),
        [-0.05, 0.0, 1.12, 0.0, 0.17453292, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Floor lamp",
        MESH,
        Some("ME:Lamp"),
        None,
        [-2.55, 1.95, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Vase",
        MESH,
        Some("ME:Vase"),
        Some("Table"),
        [0.0, 0.0, 0.76, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Mug",
        MESH,
        Some("ME:Mug"),
        None,
        [2.75, 2.15, 0.62, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Clock",
        EMPTY,
        None,
        None,
        [-0.6, 2.44, 0.86, FRAC_PI_2, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Clock hand",
        MESH,
        Some("ME:Hand"),
        Some("Clock"),
        [0.0, 0.0, 0.01, 0.0, 0.0, 0.5934119, 1.0, 1.0, 1.0],
    ),
    room(
        "Plant",
        EMPTY,
        None,
        None,
        [2.3, -1.9, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ),
    room(
        "Measure guide",
        EMPTY,
        None,
        None,
        [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5],
    ),
    room(
        "Camera.001",
        CAMERA,
        Some("CA:Camera.001"),
        None,
        [1.75, -0.75, 1.3, 1.2915436, 0.0, 0.7679449, 1.0, 1.0, 1.0],
    ),
];

/// The living room's meshes, in the order the file holds them: each a box from its first
/// corner to its second (its object's local bounds).
pub const ROOM_MESHES: [(&str, [f32; 3], [f32; 3]); 14] = [
    ("Plane", [-1.0, -1.0, -0.02], [1.0, 1.0, 0.0]),
    ("Wall", [-1.0, -1.0, -0.05], [1.0, 1.0, 0.0]),
    ("Window", [-1.0, -1.0, 0.0], [1.0, 1.0, 0.02]),
    ("Rug", [-1.0, -1.0, 0.0], [1.0, 1.0, 0.01]),
    ("Fan", [-0.65, -0.65, -0.1], [0.65, 0.65, 0.25]),
    ("Table", [-0.5, -0.35, 0.0], [0.5, 0.35, 0.76]),
    ("Chair", [-0.22, -0.22, 0.0], [0.22, 0.22, 0.92]),
    ("Sofa", [-0.9, -0.4, 0.0], [0.9, 0.4, 0.85]),
    ("Shelf", [-0.5, -0.15, 0.0], [0.5, 0.15, 1.12]),
    ("Book", [-0.02, -0.1, 0.0], [0.02, 0.1, 0.24]),
    ("Lamp", [-0.2, -0.2, 0.0], [0.2, 0.2, 1.75]),
    ("Vase", [-0.07, -0.07, 0.0], [0.07, 0.07, 0.28]),
    ("Mug", [-0.04, -0.04, -0.02], [0.04, 0.04, 0.08]),
    ("Hand", [-0.01, 0.0, -0.005], [0.01, 0.15, 0.005]),
];

/// The living room's library: its name and its path, relative to the file.
pub const ROOM_LIBRARY: (&str, &str) = ("plants.blend", "//plants.blend");

/// The living room: a scene `Scene` (its camera `Camera`) holding two cameras and two area
/// lights, with `Room` (floor, walls, window, rug, a ceiling fan turned by quaternion and off in
/// renders, the floor lamp) and `Furniture` (table, four chairs sharing a mesh, a sofa with a
/// subdivision modifier, a shelf with two books parented to it, the floor lamp again) holding
/// `Props` (a vase parented to the table, a mug, a clock with a hand an action drives, a plant
/// linked from `//plants.blend`); a measure guide no collection holds, off in renders. The left
/// wall is hidden in viewports. Stored plain.
pub fn living_room() -> Vec<u8> {
    living_room_edited(&RoomEdits::default())
}

/// [`living_room`] stored zstd, in seekable frames.
pub fn living_room_zstd() -> Vec<u8> {
    zstd_seekable(&living_room(), ZSTD_FRAME)
}

/// [`living_room`] with its `Props` collection hidden in viewports: the left wall hidden by its
/// own flag, the vase, the mug and the clock's hand by their collection.
pub fn living_room_hidden() -> Vec<u8> {
    living_room_edited(&RoomEdits {
        props_hidden: true,
        ..RoomEdits::default()
    })
}

/// Changes to the living room, written as a file that holds them ([`living_room_edited`]): the
/// file a live state over the room would read as, for the states a write-back doesn't write.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoomEdits {
    /// `Props` hidden in viewports.
    pub props_hidden: bool,
    /// An object of `Props` (its name) held by `Room` instead, after `Room`'s own objects.
    pub to_room: Option<&'static str>,
    /// An object (its name) left out: no block, and no collection or scene holds it.
    pub without: Option<&'static str>,
    /// A mesh (its name) whose box reaches twice as far: the same counts, other positions.
    pub grown: Option<&'static str>,
}

/// The living room with `e`'s changes; with none, [`living_room`] byte for byte.
pub fn living_room_edited(e: &RoomEdits) -> Vec<u8> {
    let keep = |names: &[&'static str]| -> Vec<&'static str> {
        names
            .iter()
            .copied()
            .filter(|n| Some(*n) != e.without && Some(*n) != e.to_room)
            .collect()
    };
    let mut b = builder();
    b.file_global(Some("Scene"));
    b.scene(
        "Scene",
        &keep(&["Camera", "Key light", "Fill light", "Camera.001"]),
        &["Room", "Furniture"],
        Some("Camera"),
    );
    let mut room = keep(&[
        "Floor",
        "Wall.Back",
        "Wall.Left",
        "Window",
        "Rug",
        "Ceiling fan",
        "Floor lamp",
    ]);
    room.extend(e.to_room.filter(|n| Some(*n) != e.without));
    b.collection("Room", &room, &[]);
    b.collection(
        "Furniture",
        &keep(&[
            "Table",
            "Chair.001",
            "Chair.002",
            "Chair.003",
            "Chair.004",
            "Sofa",
            "Shelf",
            "Book.001",
            "Book.002",
            "Floor lamp",
        ]),
        &["Props"],
    );
    b.collection_flagged(
        "Props",
        &keep(&["Vase", "Mug", "Clock", "Clock hand", "Plant"]),
        &[],
        u8::from(e.props_hidden),
    );
    let (lib, path) = ROOM_LIBRARY;
    for o in &ROOM_OBJECTS {
        if Some(o.name) == e.without {
            continue;
        }
        let mut spec = placed(o.ty, o.data, o.xform);
        spec.parent = o.parent.map(str::to_owned);
        match o.name {
            "Wall.Left" => spec.restrict = HIDE_VIEWPORT,
            "Ceiling fan" => {
                spec.rotmode = QUATERNION;
                spec.quat = [0.9238795, 0.0, 0.0, 0.38268343];
                spec.restrict = HIDE_RENDER;
            }
            "Sofa" => spec.modifiers = vec!["SubsurfModifierData".to_owned()],
            "Clock hand" => spec.driven = Driven::Action,
            "Plant" => {
                // linked data comes after its library
                b.library(lib, path);
                spec.lib = Some(lib.to_owned());
            }
            "Measure guide" => spec.restrict = HIDE_RENDER,
            _ => {}
        }
        b.object(o.name, &spec);
    }
    b.camera_with("Camera", &CameraSpec::default());
    b.camera_with("Camera.001", &CameraSpec::default());
    b.light_of("Area", AREA, 1000.0);
    b.light_of("Area.001", AREA, 400.0);
    for (name, min, max) in &ROOM_MESHES {
        let max = if Some(*name) == e.grown {
            max.map(|v| 2.0 * v)
        } else {
            *max
        };
        b.mesh_with(name, &box_mesh(*min, max), MeshLayouts::LAYERS);
    }
    // the clock's hand turns about Z
    b.action_with(&[(1, &[("rotation_euler", 2)])]);
    b.bytes()
}

/// Three perspective cameras (24, 50 and 85 mm, the sensor fitted automatically; `Camera.Normal`
/// the scene's camera) and an orthographic one looking down, around a box.
pub fn three_cameras() -> Vec<u8> {
    let mut b = builder();
    b.file_global(Some("Scene"));
    let cams = ["Camera.Wide", "Camera.Normal", "Camera.Long", "Camera.Top"];
    let mut all: Vec<&str> = cams.to_vec();
    all.push("Box");
    b.scene("Scene", &all, &[], Some("Camera.Normal"));
    let places = [
        [6.0, -6.0, 4.0, 1.1, 0.0, FRAC_PI_4, 1.0, 1.0, 1.0],
        [0.0, -9.0, 3.0, 1.3, 0.0, 0.0, 1.0, 1.0, 1.0],
        [-12.0, -12.0, 5.0, 1.35, 0.0, -FRAC_PI_4, 1.0, 1.0, 1.0],
        [0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    ];
    for (i, (name, xform)) in cams.iter().zip(places).enumerate() {
        let data = format!("CA:Lens.{}", i + 1);
        b.object(name, &placed(CAMERA, Some(data.as_str()), xform));
    }
    b.object(
        "Box",
        &placed(
            MESH,
            Some("ME:Box"),
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        ),
    );
    let specs = [
        CameraSpec {
            lens: 24.0,
            ..CameraSpec::default()
        },
        CameraSpec::default(),
        CameraSpec {
            lens: 85.0,
            sensor: [36.0, 20.25],
            ..CameraSpec::default()
        },
        CameraSpec {
            ty: 1,
            ..CameraSpec::default()
        },
    ];
    for (i, spec) in specs.iter().enumerate() {
        b.camera_with(&format!("Lens.{}", i + 1), spec);
    }
    b.mesh_with("Box", &box_mesh([-1.0; 3], [1.0; 3]), MeshLayouts::LAYERS);
    b.bytes()
}

/// A camera looking at three boxes: `Driven` (an action animates its location's z), `Turned`
/// (by quaternion, a quarter turn about Z) and `Lamp, B: 2` (plain; its name holds a comma and a
/// colon).
pub fn driven_quat() -> Vec<u8> {
    let mut b = builder();
    b.file_global(Some("Scene"));
    b.scene(
        "Scene",
        &["Camera", "Driven", "Turned", "Lamp, B: 2"],
        &[],
        Some("Camera"),
    );
    b.object(
        "Camera",
        &placed(
            CAMERA,
            Some("CA:Camera"),
            [0.0, -10.0, 3.0, 1.3, 0.0, 0.0, 1.0, 1.0, 1.0],
        ),
    );
    let at = |x: f32| [x, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    b.object(
        "Driven",
        &ObjectSpec {
            driven: Driven::Action,
            ..placed(MESH, Some("ME:Box"), at(-3.0))
        },
    );
    b.object(
        "Turned",
        &ObjectSpec {
            rotmode: QUATERNION,
            quat: [FRAC_1_SQRT_2, 0.0, 0.0, FRAC_1_SQRT_2],
            ..placed(MESH, Some("ME:Box"), at(0.0))
        },
    );
    b.object("Lamp, B: 2", &placed(MESH, Some("ME:Box"), at(3.0)));
    b.camera_with("Camera", &CameraSpec::default());
    b.mesh_with(
        "Box",
        &box_mesh([-0.5, -0.25, 0.0], [0.5, 0.25, 1.0]),
        MeshLayouts::LAYERS,
    );
    b.action_with(&[(1, &[("location", 2)])]);
    b.bytes()
}

/// A camera, four objects sharing the mesh `Shared` (`Copy.001` to `Copy.004`, in `Copies`)
/// and `Own` with the mesh `Single`, in a collection whose name holds a comma and a colon
/// (`Props, A: B`).
pub fn instanced() -> Vec<u8> {
    let mut b = builder();
    b.file_global(Some("Scene"));
    let copies = ["Copy.001", "Copy.002", "Copy.003", "Copy.004"];
    b.scene(
        "Scene",
        &["Camera"],
        &["Copies", "Props, A: B"],
        Some("Camera"),
    );
    b.collection("Copies", &copies, &[]);
    b.collection("Props, A: B", &["Own"], &[]);
    b.object(
        "Camera",
        &placed(
            CAMERA,
            Some("CA:Camera"),
            [0.0, -12.0, 4.0, 1.25, 0.0, 0.0, 1.0, 1.0, 1.0],
        ),
    );
    for (i, name) in copies.iter().enumerate() {
        let x = i as f32 * 2.0 - 3.0;
        b.object(
            name,
            &placed(
                MESH,
                Some("ME:Shared"),
                [x, 0.0, 0.0, 0.0, 0.0, i as f32 * 0.5, 1.0, 1.0, 1.0],
            ),
        );
    }
    b.object(
        "Own",
        &placed(
            MESH,
            Some("ME:Single"),
            [0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0],
        ),
    );
    b.camera_with("Camera", &CameraSpec::default());
    b.mesh_with(
        "Shared",
        &box_mesh([-0.5, -0.5, 0.0], [0.5, 0.5, 1.5]),
        MeshLayouts::LAYERS,
    );
    b.mesh_with(
        "Single",
        &box_mesh([-0.5, -0.5, 0.0], [0.5, 0.5, 0.5]),
        MeshLayouts::LAYERS,
    );
    b.bytes()
}

/// The large scene's shape: top collections, child collections under each, objects in each
/// collection (top and child alike), and meshes they share.
pub const LARGE_TOP: usize = 10;
/// Child collections under each top one.
pub const LARGE_CHILDREN: usize = 9;
/// Objects in each collection.
pub const LARGE_PER_COLLECTION: usize = 20;
/// Meshes the objects share.
pub const LARGE_MESHES: usize = 50;

/// The large scene's collection names, each top one followed by its children (`Block 01`,
/// `Block 01.1` to `Block 01.9`, `Block 02`...).
pub fn large_collections() -> Vec<(String, Vec<String>)> {
    (1..=LARGE_TOP)
        .map(|t| {
            let top = format!("Block {t:02}");
            let kids = (1..=LARGE_CHILDREN).map(|c| format!("{top}.{c}")).collect();
            (top, kids)
        })
        .collect()
}

/// A camera and 2,000 mesh objects (`Item.0001`...) in 100 collections nested two levels (10
/// top ones under the scene, 9 under each), 20 objects in each, sharing 50 box meshes
/// (`Part.01`...), on a grid.
pub fn large() -> Vec<u8> {
    let mut b = builder();
    b.file_global(Some("Scene"));
    let tree = large_collections();
    let tops: Vec<&str> = tree.iter().map(|(t, _)| t.as_str()).collect();
    b.scene("Scene", &["Camera"], &tops, Some("Camera"));
    let mut order: Vec<(&str, Vec<&str>)> = Vec::new();
    for (top, kids) in &tree {
        order.push((top.as_str(), kids.iter().map(String::as_str).collect()));
        for k in kids {
            order.push((k.as_str(), Vec::new()));
        }
    }
    let mut n = 0usize;
    let mut items: Vec<String> = Vec::new();
    for (name, kids) in &order {
        let held: Vec<String> = (0..LARGE_PER_COLLECTION)
            .map(|i| format!("Item.{:04}", n + i + 1))
            .collect();
        n += LARGE_PER_COLLECTION;
        let refs: Vec<&str> = held.iter().map(String::as_str).collect();
        b.collection(name, &refs, kids);
        items.extend(held);
    }
    b.object(
        "Camera",
        &placed(
            CAMERA,
            Some("CA:Camera"),
            [37.0, -40.0, 30.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        ),
    );
    for (i, name) in items.iter().enumerate() {
        let mesh = format!("ME:Part.{:02}", i % LARGE_MESHES + 1);
        let (x, y) = ((i % 50) as f32 * 1.5, (i / 50) as f32 * 1.5);
        b.object(
            name,
            &placed(
                MESH,
                Some(mesh.as_str()),
                [x, y, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            ),
        );
    }
    b.camera_with("Camera", &CameraSpec::default());
    for m in 0..LARGE_MESHES {
        let h = 0.2 + 0.01 * m as f32;
        b.mesh_with(
            &format!("Part.{:02}", m + 1),
            &box_mesh([-h, -h, 0.0], [h, h, 2.0 * h]),
            MeshLayouts::LAYERS,
        );
    }
    b.bytes()
}

/// A scene with no mesh object: its camera and an empty.
pub fn empty_scene() -> Vec<u8> {
    let mut b = builder();
    b.file_global(Some("Scene"));
    b.scene("Scene", &["Camera", "Origin"], &[], Some("Camera"));
    b.object(
        "Camera",
        &placed(
            CAMERA,
            Some("CA:Camera"),
            [7.0, -7.0, 5.0, 1.1, 0.0, FRAC_PI_4, 1.0, 1.0, 1.0],
        ),
    );
    b.object(
        "Origin",
        &placed(EMPTY, None, [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]),
    );
    b.camera_with("Camera", &CameraSpec::default());
    b.bytes()
}

/// The cameras of [`camera_fits_wide`] and [`camera_fits_tall`]: one per sensor fit (`0`
/// automatic, `1` horizontal, `2` vertical), each with its own lens and sensor, as
/// `(object, data, lens, sensor width, sensor height, fit)`.
pub const FIT_CAMERAS: [(&str, &str, f32, f32, f32, u8); 3] = [
    ("Fit.Auto", "Fit.Auto", 50.0, 36.0, 24.0, 0),
    ("Fit.Horizontal", "Fit.Horizontal", 35.0, 32.0, 18.0, 1),
    ("Fit.Vertical", "Fit.Vertical", 85.0, 30.0, 20.0, 2),
];

/// A box and the three [`FIT_CAMERAS`], `Fit.Auto` the scene's camera, rendered at `render`.
fn camera_fits(render: Render) -> Vec<u8> {
    let mut b = builder();
    b.render = render;
    b.file_global(Some("Scene"));
    let mut all: Vec<&str> = FIT_CAMERAS.iter().map(|c| c.0).collect();
    all.push("Box");
    b.scene("Scene", &all, &[], Some("Fit.Auto"));
    let places = [
        [7.0, -7.0, 5.0, 1.1, 0.0, FRAC_PI_4, 1.0, 1.0, 1.0],
        [0.0, -9.0, 3.0, 1.3, 0.0, 0.0, 1.0, 1.0, 1.0],
        [-8.0, -8.0, 4.0, 1.2, 0.0, -FRAC_PI_4, 1.0, 1.0, 1.0],
    ];
    for (c, xform) in FIT_CAMERAS.iter().zip(places) {
        let data = format!("CA:{}", c.1);
        b.object(c.0, &placed(CAMERA, Some(data.as_str()), xform));
    }
    b.object(
        "Box",
        &placed(
            MESH,
            Some("ME:Box"),
            [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        ),
    );
    for c in FIT_CAMERAS {
        b.camera_with(
            c.1,
            &CameraSpec {
                lens: c.2,
                sensor: [c.3, c.4],
                fit: c.5,
                ..CameraSpec::default()
            },
        );
    }
    b.mesh_with("Box", &box_mesh([-1.0; 3], [1.0; 3]), MeshLayouts::LAYERS);
    b.bytes()
}

/// The fit cameras rendered wide: 1920 x 1080, square pixels.
pub fn camera_fits_wide() -> Vec<u8> {
    camera_fits(Render::default())
}

/// The fit cameras rendered tall: 1080 x 1920, pixels twice as tall as wide (the image's aspect
/// under 1, so the automatic fit spans the height).
pub fn camera_fits_tall() -> Vec<u8> {
    camera_fits(Render {
        xsch: 1080,
        ysch: 1920,
        xasp: 1.0,
        yasp: 2.0,
    })
}
