//! Scene files for the tests: a schema of ours shaped like the files' own (the field names the
//! reader looks up), and helpers that write scenes, collections, objects and their data.

use super::{put, Addresses, Body, FileBuilder};
use crate::header::HeaderForm;

/// What an object written by [`SceneBuilder::object`] holds.
#[derive(Debug, Clone)]
pub struct ObjectSpec {
    /// Its data, as `ME:Cube`.
    pub data: Option<String>,
    /// Its parent object's name.
    pub parent: Option<String>,
    /// `loc`.
    pub loc: [f32; 3],
    /// `rot`.
    pub rot: [f32; 3],
    /// `quat`, `w,x,y,z` (read when `rotmode` is 0).
    pub quat: [f32; 4],
    /// `size`.
    pub scale: [f32; 3],
    /// `restrictflag`.
    pub restrict: i16,
    /// The object's own material slots, by material name.
    pub materials: Vec<Option<String>>,
    /// The ID header's user count (the save's own state).
    pub users: i32,
    /// `rotmode` (0 quaternion, 1 XYZ...).
    pub rotmode: i16,
    /// The library it is linked from, by name.
    pub lib: Option<String>,
    /// What drives its transform, in a file whose schema has the animation fields
    /// ([`SceneBuilder::animated`]); ignored otherwise.
    pub driven: Driven,
    /// Its `type` (0 empty, 1 mesh, 10 light, 11 camera...).
    pub ty: i16,
    /// `partype`: how it is parented (0: to an object).
    pub partype: i16,
    /// `parentinv`, column-major, in a file whose schema has the placement fields
    /// ([`SceneBuilder::placed`]); ignored otherwise.
    pub parentinv: [f32; 16],
    /// `rotAxis` and `rotAngle` (a placed file's).
    pub axis_angle: ([f32; 3], f32),
    /// `dloc`, a delta location (a placed file's; its other deltas are identity).
    pub delta_loc: [f32; 3],
    /// Its modifiers, by struct name (`SubsurfModifierData`), in a file with meshes
    /// ([`SceneBuilder::meshes`]); ignored otherwise.
    pub modifiers: Vec<String>,
    /// The world matrix the object caches (`obmat`), in a file with meshes; identity when
    /// `None`.
    pub obmat: Option<[f32; 16]>,
}

/// The identity matrix, column-major.
pub const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// What an object's animation data and constraints hold ([`ObjectSpec::driven`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Driven {
    /// No animation data, no constraint.
    #[default]
    None,
    /// Animation data that holds nothing: no action, no NLA track, no driver.
    EmptyAnimData,
    /// An action, through its animation data.
    Action,
    /// An NLA track.
    Nla,
    /// A driver.
    Driver,
    /// A constraint.
    Constraint,
}

impl Default for ObjectSpec {
    fn default() -> Self {
        Self {
            data: None,
            parent: None,
            loc: [0.0; 3],
            rot: [0.0; 3],
            quat: [1.0, 0.0, 0.0, 0.0],
            scale: [1.0; 3],
            restrict: 0,
            materials: Vec::new(),
            users: 1,
            rotmode: 0,
            lib: None,
            driven: Driven::None,
            ty: 0,
            partype: 0,
            parentinv: IDENTITY,
            axis_angle: ([0.0, 1.0, 0.0], 0.0),
            delta_loc: [0.0; 3],
            modifiers: Vec::new(),
            obmat: None,
        }
    }
}

/// A scene file to write. `legacy` writes the older shape: `Group` and `GroupObject` for
/// collections, a scene's objects as `Base` elements, `Lamp` for lights, a library's path in
/// `name`.
#[derive(Debug)]
pub struct SceneBuilder {
    /// The file.
    pub file: FileBuilder,
    addr: Addresses,
    legacy: bool,
    /// The schema has the objects' animation fields ([`SceneBuilder::animated`]).
    anim: bool,
    /// The schema has the objects' placement fields and the scenes' unit settings
    /// ([`SceneBuilder::placed`]).
    place: bool,
    /// The unit scale (`unit.scale_length`) of each scene written after it is set, in a placed
    /// file.
    pub unit_scale: f32,
    /// The unit system (`unit.system`) of each scene written after it is set, in a placed file
    /// (1 by default).
    pub unit_system: u8,
    /// Added to every address: the same builder calls with another shift are the same file
    /// saved again, with everything at other places in memory.
    pub shift: u64,
    /// The mesh layouts the schema has, in a file with meshes ([`SceneBuilder::meshes`]).
    mesh: Option<MeshLayouts>,
    /// The render size of each scene written after it is set, in a file with meshes.
    pub render: Render,
}

/// A scene's render size ([`SceneBuilder::render`]): `r.xsch`, `r.ysch`, `r.xasp`, `r.yasp`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Render {
    /// Width, in pixels.
    pub xsch: i32,
    /// Height, in pixels.
    pub ysch: i32,
    /// The pixel's width.
    pub xasp: f32,
    /// The pixel's height.
    pub yasp: f32,
}

impl Default for Render {
    /// 1920 x 1080, square pixels.
    fn default() -> Self {
        Self {
            xsch: 1920,
            ysch: 1080,
            xasp: 1.0,
            yasp: 1.0,
        }
    }
}

/// A camera's data ([`SceneBuilder::camera_with`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraSpec {
    /// `type`: 0 perspective, 1 orthographic, 2 panoramic.
    pub ty: u8,
    /// `lens`, in mm.
    pub lens: f32,
    /// `sensor_x`, `sensor_y` (the height only in a file with meshes).
    pub sensor: [f32; 2],
    /// `sensor_fit` (a file with meshes only).
    pub fit: u8,
    /// `clipsta`.
    pub clipsta: f32,
}

impl Default for CameraSpec {
    /// Perspective, 50 mm, a 36 x 24 mm sensor fitting automatically, clipping from 0.1.
    fn default() -> Self {
        Self {
            ty: 0,
            lens: 50.0,
            sensor: [36.0, 24.0],
            fit: 0,
            clipsta: 0.1,
        }
    }
}

impl SceneBuilder {
    /// A file of this form, with the scene schema. Names hold 63 bytes before version 5.00,
    /// 255 from it.
    pub fn new(form: HeaderForm, pointer_size: u8, big: bool, version: u16, legacy: bool) -> Self {
        Self::build(form, pointer_size, big, version, legacy, false, false, None)
    }

    /// The modern shape at `version`, its objects with the fields that say what drives their
    /// transform: `adt` (their animation data: an action, NLA tracks, drivers) and
    /// `constraints`, named as the files name them ([`ObjectSpec::driven`]).
    pub fn animated(version: u16) -> Self {
        Self::build(
            HeaderForm::Classic,
            8,
            false,
            version,
            false,
            true,
            false,
            None,
        )
    }

    /// [`SceneBuilder::animated`] with the fields a placement reads beyond the transform, named
    /// as the files name them: an object's `parentinv` (4x4), `rotAxis`, `rotAngle` and delta
    /// transforms (`dloc`, `drot`, `dquat`, `dscale`, `drotAxis`, `drotAngle`), and a scene's
    /// unit settings (`unit`, with its `scale_length`).
    pub fn placed(version: u16) -> Self {
        Self::build(
            HeaderForm::Classic,
            8,
            false,
            version,
            false,
            true,
            true,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        form: HeaderForm,
        pointer_size: u8,
        big: bool,
        version: u16,
        legacy: bool,
        anim: bool,
        place: bool,
        mesh: Option<MeshLayouts>,
    ) -> Self {
        let mut file = FileBuilder::new(form, pointer_size, big, version);
        let s = &mut file.schema;
        let name = if version >= 500 {
            "name[258]"
        } else {
            "name[66]"
        };
        s.add_struct("ListBase", &[("void", "*first"), ("void", "*last")]);
        s.add_struct(
            "ID_Runtime",
            &[("void", "*cache"), ("int", "counter"), ("int", "_pad")],
        );
        s.add_struct(
            "ID",
            &[
                ("void", "*next"),
                ("void", "*prev"),
                ("ID", "*newid"),
                ("Library", "*lib"),
                ("char", name),
                ("short", "flag"),
                ("short", "_pad0"),
                ("int", "tag"),
                ("int", "us"),
                ("int", "session_uid"),
                ("ID_Runtime", "runtime"),
            ],
        );
        s.add_struct(
            "Library",
            &[
                ("ID", "id"),
                (
                    "char",
                    if legacy {
                        "name[1024]"
                    } else {
                        "filepath[1024]"
                    },
                ),
            ],
        );
        s.add_struct(
            "Material",
            &[("ID", "id"), ("float", "r"), ("float", "g"), ("float", "b")],
        );
        let mut camera: Vec<(&str, &str)> = vec![
            ("ID", "id"),
            ("char", "type"),
            ("char", "_pad[3]"),
            ("float", "lens"),
            ("float", "clipsta"),
            ("float", "clipend"),
            ("float", "sensor_x"),
        ];
        if mesh.is_some() {
            // a file with meshes: the sensor's height and fit too
            camera.extend([
                ("float", "sensor_y"),
                ("char", "sensor_fit"),
                ("char", "_pad2[3]"),
            ]);
        }
        s.add_struct("Camera", &camera);
        s.add_struct(
            if legacy { "Lamp" } else { "Light" },
            &[
                ("ID", "id"),
                ("short", "type"),
                ("short", "_pad"),
                ("float", "r"),
                ("float", "g"),
                ("float", "b"),
                ("float", "energy"),
            ],
        );
        match mesh {
            None => {
                s.add_struct(
                    "Mesh",
                    &[
                        ("ID", "id"),
                        ("Material", "**mat"),
                        ("short", "totcol"),
                        ("short", "_pad1"),
                        ("int", "totvert"),
                        ("float", "*vert"),
                    ],
                );
            }
            Some(l) => mesh_schema(s, l),
        }
        if anim {
            s.add_struct("bAction", &[("ID", "id"), ("int", "flag"), ("int", "_pad")]);
            s.add_struct(
                "AnimData",
                &[
                    ("bAction", "*action"),
                    ("bAction", "*tmpact"),
                    ("ListBase", "nla_tracks"),
                    ("ListBase", "drivers"),
                    ("int", "flag"),
                    ("int", "_pad"),
                ],
            );
            s.add_struct(
                "bConstraint",
                &[
                    ("bConstraint", "*next"),
                    ("bConstraint", "*prev"),
                    ("short", "type"),
                    ("short", "flag"),
                    ("int", "_pad"),
                ],
            );
        }
        let mut object: Vec<(&str, &str)> = vec![
            ("ID", "id"),
            ("short", "type"),
            ("short", "partype"),
            ("int", "_pad2"),
            ("Object", "*parent"),
            ("ID", "*data"),
            ("Material", "**mat"),
            ("char", "*matbits"),
            ("int", "totcol"),
            ("float", "loc[3]"),
            ("float", "rot[3]"),
            ("float", "quat[4]"),
            ("float", "size[3]"),
            ("short", "rotmode"),
            ("short", "restrictflag"),
            ("void", "(*callback)()"),
        ];
        if anim {
            object.push(("AnimData", "*adt"));
            object.push(("ListBase", "constraints"));
        }
        if place {
            object.extend([
                ("float", "parentinv[4][4]"),
                ("float", "rotAxis[3]"),
                ("float", "rotAngle"),
                ("float", "dloc[3]"),
                ("float", "drot[3]"),
                ("float", "dquat[4]"),
                ("float", "dscale[3]"),
                ("float", "drotAxis[3]"),
                ("float", "drotAngle"),
            ]);
            s.add_struct(
                "UnitSettings",
                &[
                    ("float", "scale_length"),
                    ("char", "system"),
                    ("char", "system_rotation"),
                    ("short", "flag"),
                ],
            );
        }
        if mesh.is_some() {
            s.add_struct(
                "ModifierData",
                &[
                    ("ModifierData", "*next"),
                    ("ModifierData", "*prev"),
                    ("int", "type"),
                    ("int", "mode"),
                    ("char", "name[64]"),
                ],
            );
            for m in MODIFIER_STRUCTS {
                s.add_struct(m, &[("ModifierData", "modifier"), ("int", "levels")]);
            }
            object.push(("ListBase", "modifiers"));
            object.push(("float", "obmat[4][4]"));
        }
        s.add_struct("Object", &object);
        if legacy {
            s.add_struct(
                "GroupObject",
                &[
                    ("GroupObject", "*next"),
                    ("GroupObject", "*prev"),
                    ("Object", "*ob"),
                ],
            );
            s.add_struct("Group", &[("ID", "id"), ("ListBase", "gobject")]);
            s.add_struct(
                "Base",
                &[
                    ("Base", "*next"),
                    ("Base", "*prev"),
                    ("int", "lay"),
                    ("int", "flag"),
                    ("Object", "*object"),
                ],
            );
            s.add_struct(
                "Scene",
                &[
                    ("ID", "id"),
                    ("Object", "*camera"),
                    ("World", "*world"),
                    ("ListBase", "base"),
                ],
            );
        } else {
            s.add_struct(
                "CollectionObject",
                &[
                    ("CollectionObject", "*next"),
                    ("CollectionObject", "*prev"),
                    ("Object", "*ob"),
                ],
            );
            s.add_struct(
                "CollectionChild",
                &[
                    ("CollectionChild", "*next"),
                    ("CollectionChild", "*prev"),
                    ("Collection", "*collection"),
                ],
            );
            s.add_struct(
                "Collection",
                &[
                    ("ID", "id"),
                    ("ListBase", "gobject"),
                    ("ListBase", "children"),
                    ("uchar", "flag"),
                    ("uchar", "color_tag"),
                    ("short", "_pad"),
                    ("int", "_pad1"),
                ],
            );
            let mut scene: Vec<(&str, &str)> = vec![
                ("ID", "id"),
                ("Object", "*camera"),
                ("World", "*world"),
                ("Collection", "*master_collection"),
            ];
            if place {
                scene.push(("UnitSettings", "unit"));
            }
            if mesh.is_some() {
                // the render size: pixels and the pixel aspect
                s.add_struct(
                    "RenderData",
                    &[
                        ("int", "xsch"),
                        ("int", "ysch"),
                        ("float", "xasp"),
                        ("float", "yasp"),
                    ],
                );
                scene.push(("RenderData", "r"));
            }
            s.add_struct("Scene", &scene);
            if mesh.is_some() {
                s.add_struct(
                    "FileGlobal",
                    &[
                        ("char", "subvstr[4]"),
                        ("short", "subversion"),
                        ("short", "minversion"),
                        ("Scene", "*curscene"),
                        ("int", "fileflags"),
                        ("int", "_pad"),
                    ],
                );
            }
        }
        s.add_struct("World", &[("ID", "id"), ("float", "horr")]);
        s.add_struct("wmWindowManager", &[("ID", "id"), ("int", "winactive")]);
        Self {
            file,
            addr: Addresses::default(),
            legacy,
            anim,
            place,
            unit_scale: 1.0,
            unit_system: 1,
            shift: 0,
            mesh,
            render: Render::default(),
        }
    }

    /// [`SceneBuilder::placed`] (the newer header form from version 5.00) with meshes in the
    /// `layouts` the schema has, named as the files name them: `vdata`/`ldata` attribute
    /// layers (`CustomData`, `CustomDataLayer`), the `mvert`/`mpoly`/`mloop` structs, and the
    /// attribute store (`attribute_storage`: `AttributeStorage`, `Attribute`,
    /// `AttributeArray`); written by [`SceneBuilder::mesh_with`].
    pub fn meshes(version: u16, layouts: MeshLayouts) -> Self {
        let form = if version >= 500 {
            HeaderForm::Large
        } else {
            HeaderForm::Classic
        };
        Self::build(form, 8, false, version, false, true, true, Some(layouts))
    }

    /// The modern shape, classic header, 8-byte pointers, little-endian, version 4.03.
    pub fn modern() -> Self {
        Self::new(HeaderForm::Classic, 8, false, 403, false)
    }

    /// The address of `name` (`OB:Cube`), shifted.
    pub fn addr(&mut self, name: &str) -> u64 {
        self.addr.of(name) + self.shift
    }

    fn ptr_or_null(&mut self, name: Option<&str>) -> u64 {
        name.map_or(0, |n| self.addr(n))
    }

    /// A body of struct `st` with its ID header: `code` and `name`, linked from library `lib`.
    pub fn id_body(
        &mut self,
        st: &str,
        code: &str,
        name: &[u8],
        lib: Option<&str>,
        users: i32,
    ) -> Body {
        let ps = self.file.pointer_size;
        let big = self.file.big;
        let s = &self.file.schema;
        let mut b = Body::new(s.len_of(st), big, ps);
        let id = s.offset(st, "id");
        let name_at = id + s.offset("ID", "name");
        let us = id + s.offset("ID", "us");
        let uid = id + s.offset("ID", "session_uid");
        let next = id + s.offset("ID", "next");
        let lib_at = id + s.offset("ID", "lib");
        let cache = id + s.offset("ID", "runtime");
        let mut full = code.as_bytes().to_vec();
        full.extend(name);
        b.raw(name_at, &full);
        b.i32(us, users);
        // the save's own state: different on every save
        b.i32(
            uid,
            (self.shift as i32)
                .wrapping_mul(7)
                .wrapping_add(name.len() as i32),
        );
        b.ptr(next, 0x5555_0000 + self.shift);
        b.ptr(cache, 0x6666_0000 + self.shift);
        let l = lib.map(|l| format!("LI:{l}"));
        let lp = self.ptr_or_null(l.as_deref());
        b.ptr(lib_at, lp);
        b
    }

    /// A raw block of pointers to the named data-blocks (`None` for an empty slot), owned by the
    /// data-block written last.
    fn pointer_array(&mut self, owner: &str, field: &str, targets: &[Option<String>]) {
        let ps = usize::from(self.file.pointer_size);
        let mut b = Body::new(targets.len() * ps, self.file.big, self.file.pointer_size);
        for (i, t) in targets.iter().enumerate() {
            let p = self.ptr_or_null(t.as_deref());
            b.ptr(i * ps, p);
        }
        let a = self.addr(&format!("{owner}/{field}"));
        let n = targets.len() as u64;
        self.file.raw(b"DATA", 0, a, n.max(1), b.bytes);
    }

    /// A library.
    pub fn library(&mut self, name: &str, path: &str) {
        let mut b = self.id_body("Library", "LI", name.as_bytes(), None, 1);
        let f = if self.legacy { "name" } else { "filepath" };
        let at = self.file.schema.offset("Library", f);
        b.raw(at, path.as_bytes());
        let a = self.addr(&format!("LI:{name}"));
        self.file.block(b"LI\0\0", "Library", a, b.bytes);
    }

    /// A material, linked from library `lib` when given.
    pub fn material(&mut self, name: &str, rgb: [f32; 3], lib: Option<&str>) {
        let mut b = self.id_body("Material", "MA", name.as_bytes(), lib, 1);
        let at = self.file.schema.offset("Material", "r");
        b.f32s(at, &rgb);
        let a = self.addr(&format!("MA:{name}"));
        self.file.block(b"MA\0\0", "Material", a, b.bytes);
    }

    /// A camera.
    pub fn camera(&mut self, name: &str, lens: f32) {
        self.camera_with(
            name,
            &CameraSpec {
                lens,
                ..CameraSpec::default()
            },
        )
    }

    /// A camera of `spec`'s data (its sensor's height and fit in a file with meshes only).
    pub fn camera_with(&mut self, name: &str, spec: &CameraSpec) {
        let mut b = self.id_body("Camera", "CA", name.as_bytes(), None, 1);
        let s = &self.file.schema;
        let (t, l, cs, ce, sx) = (
            s.offset("Camera", "type"),
            s.offset("Camera", "lens"),
            s.offset("Camera", "clipsta"),
            s.offset("Camera", "clipend"),
            s.offset("Camera", "sensor_x"),
        );
        let more = self.mesh.map(|_| {
            (
                s.offset("Camera", "sensor_y"),
                s.offset("Camera", "sensor_fit"),
            )
        });
        b.raw(t, &[spec.ty])
            .f32s(l, &[spec.lens])
            .f32s(cs, &[spec.clipsta])
            .f32s(ce, &[100.0])
            .f32s(sx, &[spec.sensor[0]]);
        if let Some((sy, fit)) = more {
            b.f32s(sy, &[spec.sensor[1]]).raw(fit, &[spec.fit]);
        }
        let a = self.addr(&format!("CA:{name}"));
        self.file.block(b"CA\0\0", "Camera", a, b.bytes);
    }

    /// A light.
    pub fn light(&mut self, name: &str, energy: f32) {
        self.light_of(name, 2, energy)
    }

    /// A light of `type` `ty` (0 point, 1 sun, 2 spot, 4 area).
    pub fn light_of(&mut self, name: &str, ty: i16, energy: f32) {
        let st = if self.legacy { "Lamp" } else { "Light" };
        let mut b = self.id_body(st, "LA", name.as_bytes(), None, 1);
        let s = &self.file.schema;
        let (r, e, t) = (
            s.offset(st, "r"),
            s.offset(st, "energy"),
            s.offset(st, "type"),
        );
        b.f32s(r, &[1.0, 0.5, 0.25]).f32s(e, &[energy]).i16(t, ty);
        let a = self.addr(&format!("LA:{name}"));
        self.file.block(b"LA\0\0", st, a, b.bytes);
    }

    /// A world.
    pub fn world(&mut self, name: &str) {
        let b = self.id_body("World", "WO", name.as_bytes(), None, 1);
        let a = self.addr(&format!("WO:{name}"));
        self.file.block(b"WO\0\0", "World", a, b.bytes);
    }

    /// A window manager: the program's own interface, which gets no node.
    pub fn ui(&mut self) {
        let b = self.id_body("wmWindowManager", "WM", b"WinMan", None, 1);
        let a = self.addr("WM:WinMan");
        self.file.block(b"WM\0\0", "wmWindowManager", a, b.bytes);
    }

    /// A mesh with material slots (by material name) and `verts` vertices (a raw block of 12
    /// bytes each).
    pub fn mesh(&mut self, name: &str, materials: &[Option<String>], verts: usize) {
        let mut b = self.id_body("Mesh", "ME", name.as_bytes(), None, 1);
        let s = &self.file.schema;
        let (mat, totcol, totvert, vert) = (
            s.offset("Mesh", "mat"),
            s.offset("Mesh", "totcol"),
            s.offset("Mesh", "totvert"),
            s.offset("Mesh", "vert"),
        );
        let key = format!("ME:{name}");
        let mats: Vec<Option<String>> = materials
            .iter()
            .map(|m| m.as_ref().map(|m| format!("MA:{m}")))
            .collect();
        let arr = if mats.is_empty() {
            0
        } else {
            self.addr(&format!("{key}/mat"))
        };
        let va = self.addr(&format!("{key}/vert"));
        b.ptr(mat, arr)
            .i16(totcol, mats.len() as i16)
            .i32(totvert, verts as i32)
            .ptr(vert, va);
        let a = self.addr(&key);
        self.file.block(b"ME\0\0", "Mesh", a, b.bytes);
        if !mats.is_empty() {
            self.pointer_array(&key, "mat", &mats);
        }
        let mut v = Body::new(verts * 12, self.file.big, self.file.pointer_size);
        for i in 0..verts {
            v.f32s(i * 12, &[i as f32, 0.5, -1.0]);
        }
        self.file.raw(b"DATA", 0, va, verts.max(1) as u64, v.bytes);
    }

    /// An object.
    pub fn object(&mut self, name: &str, spec: &ObjectSpec) {
        self.object_named(name.as_bytes(), spec)
    }

    /// An object whose name is any bytes; its addresses are keyed by the name read lossily.
    pub fn object_named(&mut self, name: &[u8], spec: &ObjectSpec) {
        let mut b = self.id_body("Object", "OB", name, spec.lib.as_deref(), spec.users);
        let key = format!("OB:{}", String::from_utf8_lossy(name));
        let s = &self.file.schema;
        let o = |f: &str| s.offset("Object", f);
        let at = [
            o("parent"),
            o("data"),
            o("mat"),
            o("totcol"),
            o("loc"),
            o("rot"),
            o("quat"),
            o("size"),
            o("restrictflag"),
            o("callback"),
            o("rotmode"),
        ];
        let pp = spec.parent.as_ref().map(|p| format!("OB:{p}"));
        let pp = self.ptr_or_null(pp.as_deref());
        let dp = self.ptr_or_null(spec.data.as_deref());
        let mats: Vec<Option<String>> = spec
            .materials
            .iter()
            .map(|m| m.as_ref().map(|m| format!("MA:{m}")))
            .collect();
        let arr = if mats.is_empty() {
            0
        } else {
            self.addr(&format!("{key}/mat"))
        };
        b.ptr(at[0], pp)
            .ptr(at[1], dp)
            .ptr(at[2], arr)
            .i32(at[3], mats.len() as i32)
            .f32s(at[4], &spec.loc)
            .f32s(at[5], &spec.rot)
            .f32s(at[6], &spec.quat)
            .f32s(at[7], &spec.scale)
            .i16(at[8], spec.restrict)
            .ptr(at[9], 0x7777_0000 + self.shift)
            .i16(at[10], spec.rotmode)
            .i16(self.file.schema.offset("Object", "type"), spec.ty);
        let ps = usize::from(self.file.pointer_size);
        let anim = self.anim.then(|| {
            let s = &self.file.schema;
            (s.offset("Object", "adt"), s.offset("Object", "constraints"))
        });
        if let Some((adt, cons)) = anim {
            match spec.driven {
                Driven::None => {}
                Driven::Constraint => {
                    let p = self.addr(&format!("{key}/constraint"));
                    b.ptr(cons, p).ptr(cons + ps, p);
                }
                _ => {
                    let p = self.addr(&format!("{key}/adt"));
                    b.ptr(adt, p);
                }
            }
        }
        b.i16(self.file.schema.offset("Object", "partype"), spec.partype);
        if self.place {
            let s = &self.file.schema;
            let o = |f: &str| s.offset("Object", f);
            let (pi, ax, an, dl, dq, ds) = (
                o("parentinv"),
                o("rotAxis"),
                o("rotAngle"),
                o("dloc"),
                o("dquat"),
                o("dscale"),
            );
            b.f32s(pi, &spec.parentinv)
                .f32s(ax, &spec.axis_angle.0)
                .f32s(an, &[spec.axis_angle.1])
                .f32s(dl, &spec.delta_loc)
                .f32s(dq, &[1.0, 0.0, 0.0, 0.0])
                .f32s(ds, &[1.0; 3]);
        }
        if self.mesh.is_some() {
            let s = &self.file.schema;
            let (mo, om) = (s.offset("Object", "modifiers"), s.offset("Object", "obmat"));
            b.f32s(om, &spec.obmat.unwrap_or(IDENTITY));
            let (f, l) = self.heads(&key, "modifiers", spec.modifiers.len());
            b.ptr(mo, f).ptr(mo + ps, l);
        }
        let a = self.addr(&key);
        self.file.block(b"OB\0\0", "Object", a, b.bytes);
        if !mats.is_empty() {
            self.pointer_array(&key, "mat", &mats);
        }
        if self.mesh.is_some() {
            self.modifiers(&key, &spec.modifiers);
        }
        if anim.is_some() {
            self.driven(&key, spec.driven);
        }
    }

    /// The modifier list of object `key`: one DATA block per modifier, of its own struct, linked
    /// by `modifier.next` and `modifier.prev`.
    fn modifiers(&mut self, key: &str, structs: &[String]) {
        let addrs: Vec<u64> = (0..structs.len())
            .map(|i| self.addr(&format!("{key}/modifiers/{i}")))
            .collect();
        let (big, ps) = (self.file.big, self.file.pointer_size);
        for (i, st) in structs.iter().enumerate() {
            let s = &self.file.schema;
            let base = s.offset(st, "modifier");
            let (next, prev, name) = (
                base + s.offset("ModifierData", "next"),
                base + s.offset("ModifierData", "prev"),
                base + s.offset("ModifierData", "name"),
            );
            let mut b = Body::new(s.len_of(st), big, ps);
            b.ptr(next, addrs.get(i + 1).copied().unwrap_or(0))
                .ptr(prev, if i == 0 { 0 } else { addrs[i - 1] })
                .raw(name, format!("M{i}").as_bytes());
            self.file.block(b"DATA", st, addrs[i], b.bytes);
        }
    }

    /// The file's global block (`GLOB`, a `FileGlobal`) naming `scene` as the current one (by
    /// name; `Some("")` names an address no block has; `None` a null pointer), in a file with
    /// meshes.
    pub fn file_global(&mut self, scene: Option<&str>) {
        let s = &self.file.schema;
        let mut b = Body::new(
            s.len_of("FileGlobal"),
            self.file.big,
            self.file.pointer_size,
        );
        let at = s.offset("FileGlobal", "curscene");
        let p = match scene {
            None => 0,
            Some("") => 0xbad_0000 + self.shift,
            Some(n) => self.addr(&format!("SC:{n}")),
        };
        b.ptr(at, p);
        let sdna = self.file.schema.index("FileGlobal");
        self.file.raw(b"GLOB", sdna, 0, 1, b.bytes);
    }

    /// The animation data or constraint `driven` names, as DATA blocks of object `key`.
    fn driven(&mut self, key: &str, driven: Driven) {
        let ps = usize::from(self.file.pointer_size);
        let (big, psz) = (self.file.big, self.file.pointer_size);
        if driven == Driven::None {
            return;
        }
        if driven == Driven::Constraint {
            let s = &self.file.schema;
            let mut c = Body::new(s.len_of("bConstraint"), big, psz);
            c.i16(s.offset("bConstraint", "type"), 2);
            let a = self.addr(&format!("{key}/constraint"));
            self.file.block(b"DATA", "bConstraint", a, c.bytes);
            return;
        }
        let s = &self.file.schema;
        let (act, nla, drv, len) = (
            s.offset("AnimData", "action"),
            s.offset("AnimData", "nla_tracks"),
            s.offset("AnimData", "drivers"),
            s.len_of("AnimData"),
        );
        let mut d = Body::new(len, big, psz);
        match driven {
            Driven::Action => {
                let p = self.addr("AC:Walk");
                d.ptr(act, p);
            }
            Driven::Nla => {
                let p = self.addr(&format!("{key}/nla"));
                d.ptr(nla, p).ptr(nla + ps, p);
            }
            Driven::Driver => {
                let p = self.addr(&format!("{key}/driver"));
                d.ptr(drv, p).ptr(drv + ps, p);
            }
            _ => {}
        }
        let a = self.addr(&format!("{key}/adt"));
        self.file.block(b"DATA", "AnimData", a, d.bytes);
    }

    /// The action an object's [`Driven::Action`] names: `AC:Walk`.
    pub fn action(&mut self) {
        let b = self.id_body("bAction", "AC", b"Walk", None, 1);
        let a = self.addr("AC:Walk");
        self.file.block(b"AC\0\0", "bAction", a, b.bytes);
    }

    /// A list of elements of struct `elem`, each pointing at one of `targets` (addresses by
    /// name) through `field`, written as DATA blocks of the data-block written last.
    fn elements(&mut self, owner: &str, list: &str, elem: &str, field: &str, targets: &[String]) {
        let addrs: Vec<u64> = (0..targets.len())
            .map(|i| self.addr(&format!("{owner}/{list}/{i}")))
            .collect();
        let s = &self.file.schema;
        let (next, prev, tf, len) = (
            s.offset(elem, "next"),
            s.offset(elem, "prev"),
            s.offset(elem, field),
            s.len_of(elem),
        );
        for (i, t) in targets.iter().enumerate() {
            let mut b = Body::new(len, self.file.big, self.file.pointer_size);
            let tp = self.addr(t);
            b.ptr(next, addrs.get(i + 1).copied().unwrap_or(0))
                .ptr(prev, if i == 0 { 0 } else { addrs[i - 1] })
                .ptr(tf, tp);
            self.file.block(b"DATA", elem, addrs[i], b.bytes);
        }
    }

    /// The first and last element addresses of a list of `n` elements.
    fn heads(&mut self, owner: &str, list: &str, n: usize) -> (u64, u64) {
        if n == 0 {
            return (0, 0);
        }
        (
            self.addr(&format!("{owner}/{list}/0")),
            self.addr(&format!("{owner}/{list}/{}", n - 1)),
        )
    }

    /// A collection holding `objects` and child `collections` (by name).
    pub fn collection(&mut self, name: &str, objects: &[&str], collections: &[&str]) {
        self.collection_flagged(name, objects, collections, 0)
    }

    /// [`SceneBuilder::collection`] with its `flag` (bit 1: disabled in viewports); the modern
    /// shape only (the legacy shape's groups have no flag).
    pub fn collection_flagged(
        &mut self,
        name: &str,
        objects: &[&str],
        collections: &[&str],
        flag: u8,
    ) {
        let st = if self.legacy { "Group" } else { "Collection" };
        let mut b = self.id_body(st, "GR", name.as_bytes(), None, 1);
        let key = format!("GR:{name}");
        let a = self.addr(&key);
        let ps = usize::from(self.file.pointer_size);
        let go = self.file.schema.offset(st, "gobject");
        let obs: Vec<String> = objects.iter().map(|o| format!("OB:{o}")).collect();
        let cols: Vec<String> = collections.iter().map(|c| format!("GR:{c}")).collect();
        let (f, l) = self.heads(&key, "gobject", obs.len());
        b.ptr(go, f).ptr(go + ps, l);
        if !self.legacy {
            let fl = self.file.schema.offset(st, "flag");
            b.raw(fl, &[flag]);
        }
        if !self.legacy {
            let ch = self.file.schema.offset(st, "children");
            let (f, l) = self.heads(&key, "children", cols.len());
            b.ptr(ch, f).ptr(ch + ps, l);
        }
        self.file.block(b"GR\0\0", st, a, b.bytes);
        let elem = if self.legacy {
            "GroupObject"
        } else {
            "CollectionObject"
        };
        self.elements(&key, "gobject", elem, "ob", &obs);
        if !self.legacy {
            self.elements(&key, "children", "CollectionChild", "collection", &cols);
        }
    }

    /// A scene holding `objects` and child `collections` (in its master collection, or as
    /// bases in the legacy shape), with a camera object.
    pub fn scene(
        &mut self,
        name: &str,
        objects: &[&str],
        collections: &[&str],
        camera: Option<&str>,
    ) {
        let mut b = self.id_body("Scene", "SC", name.as_bytes(), None, 1);
        let key = format!("SC:{name}");
        let a = self.addr(&key);
        let ps = usize::from(self.file.pointer_size);
        let cam = camera.map(|c| format!("OB:{c}"));
        let cp = self.ptr_or_null(cam.as_deref());
        let co = self.file.schema.offset("Scene", "camera");
        b.ptr(co, cp);
        let obs: Vec<String> = objects.iter().map(|o| format!("OB:{o}")).collect();
        if self.legacy {
            let bo = self.file.schema.offset("Scene", "base");
            let (f, l) = self.heads(&key, "base", obs.len());
            b.ptr(bo, f).ptr(bo + ps, l);
            self.file.block(b"SC\0\0", "Scene", a, b.bytes);
            self.elements(&key, "base", "Base", "object", &obs);
            return;
        }
        let mo = self.file.schema.offset("Scene", "master_collection");
        let master = format!("{key}/master");
        let ma = self.addr(&master);
        b.ptr(mo, ma);
        if self.place {
            let s = &self.file.schema;
            let unit = s.offset("Scene", "unit");
            let (at, sys) = (
                unit + s.offset("UnitSettings", "scale_length"),
                unit + s.offset("UnitSettings", "system"),
            );
            b.f32s(at, &[self.unit_scale]).raw(sys, &[self.unit_system]);
        }
        if self.mesh.is_some() {
            let s = &self.file.schema;
            let r = s.offset("Scene", "r");
            let o = |f: &str| r + s.offset("RenderData", f);
            let (x, y, xa, ya) = (o("xsch"), o("ysch"), o("xasp"), o("yasp"));
            let rd = self.render;
            b.i32(x, rd.xsch)
                .i32(y, rd.ysch)
                .f32s(xa, &[rd.xasp])
                .f32s(ya, &[rd.yasp]);
        }
        self.file.block(b"SC\0\0", "Scene", a, b.bytes);
        // the master collection: embedded, a DATA block of the scene's
        let mut m = self.id_body("Collection", "GR", b"Scene Collection", None, 1);
        let go = self.file.schema.offset("Collection", "gobject");
        let ch = self.file.schema.offset("Collection", "children");
        let cols: Vec<String> = collections.iter().map(|c| format!("GR:{c}")).collect();
        let (f, l) = self.heads(&master, "gobject", obs.len());
        m.ptr(go, f).ptr(go + ps, l);
        let (f, l) = self.heads(&master, "children", cols.len());
        m.ptr(ch, f).ptr(ch + ps, l);
        self.file.block(b"DATA", "Collection", ma, m.bytes);
        self.elements(&master, "gobject", "CollectionObject", "ob", &obs);
        self.elements(&master, "children", "CollectionChild", "collection", &cols);
    }

    /// The file's bytes.
    pub fn bytes(&self) -> Vec<u8> {
        self.file.bytes()
    }
}

/// The modifier structs a file with meshes has ([`ObjectSpec::modifiers`]): each a
/// `ModifierData` then a field of its own.
pub const MODIFIER_STRUCTS: &[&str] = &[
    "SubsurfModifierData",
    "MirrorModifierData",
    "ArrayModifierData",
];

/// Which mesh layouts a schema has ([`SceneBuilder::meshes`]), or a mesh is written in
/// ([`SceneBuilder::mesh_with`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshLayouts {
    /// Named attribute layers (`vdata`, `ldata`) and polygon offsets.
    pub layers: bool,
    /// The `mvert`, `mpoly` and `mloop` structs.
    pub structs: bool,
    /// The attribute store (`attribute_storage`) and polygon offsets.
    pub store: bool,
}

impl MeshLayouts {
    /// The attribute layers only.
    pub const LAYERS: Self = Self {
        layers: true,
        structs: false,
        store: false,
    };
    /// The structs only.
    pub const STRUCTS: Self = Self {
        layers: false,
        structs: true,
        store: false,
    };
    /// The attribute store only.
    pub const STORE: Self = Self {
        layers: false,
        structs: false,
        store: true,
    };
    /// None (a mesh with no geometry arrays).
    pub const NONE: Self = Self {
        layers: false,
        structs: false,
        store: false,
    };
}

/// A mesh to write ([`SceneBuilder::mesh_with`]): positions and polygons (each a list of
/// vertex indices, in corner order), and what to write wrong.
#[derive(Debug, Clone, Default)]
pub struct MeshSpec {
    /// Every vertex's position.
    pub positions: Vec<[f32; 3]>,
    /// Every polygon's vertex indices.
    pub faces: Vec<Vec<u32>>,
    /// The vertex count the mesh's struct claims, when not the positions' (the arrays hold the
    /// positions as given).
    pub claimed_vertices: Option<i32>,
    /// The polygon offsets written, when not the polygons'.
    pub offsets: Option<Vec<i32>>,
    /// Point the positions at an address no block has.
    pub dangling_positions: bool,
}

/// The mesh structs of a schema with `l`'s layouts, before `Mesh` itself.
fn mesh_schema(s: &mut super::SchemaBuilder, l: MeshLayouts) {
    s.add_type("int8_t", 1);
    s.add_struct(
        "CustomDataLayer",
        &[
            ("int", "type"),
            ("int", "offset"),
            ("int", "flag"),
            ("int", "active"),
            ("char", "name[68]"),
            ("char", "_pad1[4]"),
            ("void", "*data"),
        ],
    );
    s.add_struct(
        "CustomData",
        &[
            ("CustomDataLayer", "*layers"),
            ("int", "typemap[53]"),
            ("int", "totlayer"),
            ("int", "maxlayer"),
            ("int", "totsize"),
            ("BLI_mempool", "*pool"),
            ("CustomDataExternal", "*external"),
        ],
    );
    s.add_struct("vec3f", &[("float", "x"), ("float", "y"), ("float", "z")]);
    s.add_struct("MIntProperty", &[("int", "i")]);
    let mut mesh: Vec<(&str, &str)> = vec![
        ("ID", "id"),
        ("Material", "**mat"),
        ("int", "totvert"),
        ("int", "totedge"),
        ("int", "totpoly"),
        ("int", "totloop"),
        ("int", "*poly_offset_indices"),
    ];
    if l.store {
        s.add_struct(
            "Attribute",
            &[
                ("char", "*name"),
                ("short", "data_type"),
                ("int8_t", "domain"),
                ("int8_t", "storage_type"),
                ("char", "_pad[4]"),
                ("void", "*data"),
            ],
        );
        s.add_struct(
            "AttributeArray",
            &[
                ("void", "*data"),
                ("ImplicitSharingInfoHandle", "*sharing_info"),
                ("int64_t", "size"),
                ("int8_t", "is_single"),
                ("char", "_pad[7]"),
            ],
        );
        s.add_struct(
            "AttributeStorage",
            &[
                ("Attribute", "*dna_attributes"),
                ("int", "dna_attributes_num"),
                ("char", "_pad[4]"),
                ("AttributeStorageRuntime", "*runtime"),
            ],
        );
        mesh.push(("AttributeStorage", "attribute_storage"));
    }
    mesh.extend([
        ("CustomData", "vdata"),
        ("CustomData", "edata"),
        ("CustomData", "pdata"),
        ("CustomData", "ldata"),
    ]);
    if l.structs {
        s.add_struct(
            "MVert",
            &[
                ("float", "co[3]"),
                ("char", "flag"),
                ("char", "bweight"),
                ("char", "_pad[2]"),
            ],
        );
        s.add_struct(
            "MPoly",
            &[
                ("int", "loopstart"),
                ("int", "totloop"),
                ("short", "mat_nr"),
                ("char", "flag"),
                ("char", "_pad"),
            ],
        );
        s.add_struct("MLoop", &[("int", "v"), ("int", "e")]);
        mesh.extend([
            ("MPoly", "*mpoly"),
            ("MLoop", "*mloop"),
            ("MVert", "*mvert"),
        ]);
    }
    mesh.extend([("short", "totcol"), ("short", "_pad1"), ("int", "_pad2")]);
    s.add_struct("Mesh", &mesh);
}

impl SceneBuilder {
    /// A mesh in the layouts `write` names (each must be in the schema's,
    /// [`SceneBuilder::meshes`]): the mesh block, then its arrays as data blocks of its own.
    pub fn mesh_with(&mut self, name: &str, spec: &MeshSpec, write: MeshLayouts) {
        let key = format!("ME:{name}");
        let (big, ps) = (self.file.big, self.file.pointer_size);
        let verts = spec.positions.len();
        let corners: Vec<u32> = spec.faces.iter().flatten().copied().collect();
        let mut offsets = vec![0i32];
        for f in &spec.faces {
            offsets.push(offsets.last().unwrap() + f.len() as i32);
        }
        let offsets = spec.offsets.clone().unwrap_or(offsets);
        let mut b = self.id_body("Mesh", "ME", name.as_bytes(), None, 1);
        let s = &self.file.schema;
        let o = |f: &str| s.offset("Mesh", f);
        let (tv, tp, tl, po) = (
            o("totvert"),
            o("totpoly"),
            o("totloop"),
            o("poly_offset_indices"),
        );
        b.i32(tv, spec.claimed_vertices.unwrap_or(verts as i32))
            .i32(tp, spec.faces.len() as i32)
            .i32(tl, corners.len() as i32);
        let addr = |me: &mut Self, what: &str| me.addr(&format!("{key}/{what}"));
        let pos_addr = if spec.dangling_positions {
            0xdead_0000 + self.shift
        } else {
            addr(self, "positions")
        };
        let mut blocks: Vec<(Vec<u8>, &str, u64, Vec<u8>)> = Vec::new();
        let floats = |stride: usize| {
            let mut v = Body::new(verts * stride, big, ps);
            for (i, p) in spec.positions.iter().enumerate() {
                v.f32s(i * stride, p);
            }
            v.bytes
        };
        let ints = |vals: &[i32], stride: usize, at: usize| {
            let mut v = Body::new(vals.len() * stride, big, ps);
            for (i, x) in vals.iter().enumerate() {
                v.i32(i * stride + at, *x);
            }
            v.bytes
        };
        let corner_ints: Vec<i32> = corners.iter().map(|&c| c as i32).collect();
        if (write.layers || write.store) && !spec.faces.is_empty() {
            let a = addr(self, "offsets");
            b.ptr(po, a);
            blocks.push((b"DATA".to_vec(), "", a, ints(&offsets, 4, 0)));
        }
        if write.layers {
            let s = &self.file.schema;
            let (vd, ld) = (s.offset("Mesh", "vdata"), s.offset("Mesh", "ldata"));
            let (layers, tot, maxl) = (
                s.offset("CustomData", "layers"),
                s.offset("CustomData", "totlayer"),
                s.offset("CustomData", "maxlayer"),
            );
            let ll = s.len_of("CustomDataLayer");
            let (lt, ln, ldp) = (
                s.offset("CustomDataLayer", "type"),
                s.offset("CustomDataLayer", "name"),
                s.offset("CustomDataLayer", "data"),
            );
            for (cd, lname, ltype, what, data) in [
                (vd, "position", 48, "vlayers", floats(12)),
                (ld, ".corner_vert", 11, "llayers", ints(&corner_ints, 4, 0)),
            ] {
                let la = addr(self, what);
                b.ptr(cd + layers, la).i32(cd + tot, 1).i32(cd + maxl, 1);
                let da = if lname == "position" {
                    pos_addr
                } else {
                    addr(self, "corner_vert")
                };
                let mut l = Body::new(ll, big, ps);
                l.i32(lt, ltype).raw(ln, lname.as_bytes()).ptr(ldp, da);
                blocks.push((b"DATA".to_vec(), "CustomDataLayer", la, l.bytes));
                let st = if lname == "position" {
                    "vec3f"
                } else {
                    "MIntProperty"
                };
                if lname != "position" || !spec.dangling_positions {
                    blocks.push((b"DATA".to_vec(), st, da, data));
                }
            }
        }
        if write.structs {
            let s = &self.file.schema;
            let (mv, mp, ml) = (
                s.offset("Mesh", "mvert"),
                s.offset("Mesh", "mpoly"),
                s.offset("Mesh", "mloop"),
            );
            let (vl, pl, lv) = (s.len_of("MVert"), s.len_of("MPoly"), s.len_of("MLoop"));
            let (ls, lt) = (s.offset("MPoly", "loopstart"), s.offset("MPoly", "totloop"));
            let mut polys = Body::new(spec.faces.len() * pl, big, ps);
            for (i, w) in offsets.windows(2).enumerate().take(spec.faces.len()) {
                polys.i32(i * pl + ls, w[0]).i32(i * pl + lt, w[1] - w[0]);
            }
            let (pa, la) = (addr(self, "mpoly"), addr(self, "mloop"));
            // its own block, apart from a layer's (a file written in both layouts)
            let va = if spec.dangling_positions {
                pos_addr
            } else {
                addr(self, "mvert")
            };
            b.ptr(mv, va).ptr(mp, pa).ptr(ml, la);
            if !spec.dangling_positions {
                blocks.push((b"DATA".to_vec(), "MVert", va, floats(vl)));
            }
            blocks.push((b"DATA".to_vec(), "MPoly", pa, polys.bytes));
            blocks.push((b"DATA".to_vec(), "MLoop", la, ints(&corner_ints, lv, 0)));
        }
        if write.store {
            let s = &self.file.schema;
            let st = s.offset("Mesh", "attribute_storage");
            let (list, num) = (
                s.offset("AttributeStorage", "dna_attributes"),
                s.offset("AttributeStorage", "dna_attributes_num"),
            );
            let al = s.len_of("Attribute");
            let (an, adt, adom, ad) = (
                s.offset("Attribute", "name"),
                s.offset("Attribute", "data_type"),
                s.offset("Attribute", "domain"),
                s.offset("Attribute", "data"),
            );
            let (rd, rs) = (
                s.offset("AttributeArray", "data"),
                s.offset("AttributeArray", "size"),
            );
            let rl = s.len_of("AttributeArray");
            let la = addr(self, "attributes");
            b.ptr(st + list, la).i32(st + num, 2);
            let mut attrs = Body::new(2 * al, big, ps);
            let mut tail = Vec::new();
            for (k, (aname, dtype, domain, count, data)) in [
                ("position", 7i16, 0u8, verts, floats(12)),
                (
                    ".corner_vert",
                    3,
                    3,
                    corners.len(),
                    ints(&corner_ints, 4, 0),
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let base = k * al;
                let (na, ra) = (
                    addr(self, &format!("{aname}/name")),
                    addr(self, &format!("{aname}/array")),
                );
                let da = if k == 0 {
                    pos_addr
                } else {
                    addr(self, "corner_vert")
                };
                attrs
                    .ptr(base + an, na)
                    .i16(base + adt, dtype)
                    .raw(base + adom, &[domain])
                    .ptr(base + ad, ra);
                let mut nm = aname.as_bytes().to_vec();
                nm.push(0);
                tail.push((b"DATA".to_vec(), "", na, nm));
                let mut arr = Body::new(rl, big, ps);
                arr.ptr(rd, da);
                put(&mut arr.bytes, rs, &(count as i64).to_le_bytes(), big);
                tail.push((b"DATA".to_vec(), "AttributeArray", ra, arr.bytes));
                if k != 0 || !spec.dangling_positions {
                    tail.push((b"DATA".to_vec(), "", da, data));
                }
            }
            blocks.push((b"DATA".to_vec(), "Attribute", la, attrs.bytes));
            blocks.extend(tail);
        }
        let a = self.addr(&key);
        self.file.block(b"ME\0\0", "Mesh", a, b.bytes);
        for (code, st, addr, body) in blocks {
            if st.is_empty() {
                self.file.raw(&code, 0, addr, 1, body);
            } else {
                self.file.block(&code, st, addr, body);
            }
        }
    }
}
