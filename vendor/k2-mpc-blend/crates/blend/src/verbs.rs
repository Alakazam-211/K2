//! The agent's verbs: their definitions as data for the kit's registry ([`VERBS`],
//! [`registry`]), what each one does to a file ([`list`], [`apply`]), and each run through a
//! blob session with the version the agent read ([`run`], [`run_list`]).
//!
//! **Version checked.** Every edit verb takes the version the agent read (`--base V`). [`run`]
//! refuses at once when the doc has moved on, and writes with that version as the base, so the
//! kit refuses again at commit if a write landed meanwhile ([`SessionError::Stale`], exit 1):
//! never last-writer-wins. A person's app save that lands later keeps both (the kit's conflict
//! copy) until the kit says what the app had open; the merge by object and field
//! ([`crate::merge`]) waits on that ([`crate::pack`]).
//!
//! **Refusals are registered** ([`crate::live::errors::ERRORS`], ids `BLEND-Ennn`): each
//! [`VerbError`] the writer, the reader or the mesh read gives has a code ([`VerbError::code`])
//! and a report by its entry ([`VerbError::report`]), as the live verbs' refusals have. A usage
//! error, a stale base and the session's own refusals are the kit's entries.
//!
//! **Fixed-size fields only.** A verb is a list of [`crate::write::Edit`]s: it patches the
//! fields it names in place and keeps every other byte, or is refused, typed.
//!
//! **Whole-file limits.** Each verb decodes the whole file in memory under the limits it is
//! given. A host without bounds of its own passes [`crate::BlendPack`]'s default `limits`: the
//! live tier's, 512 MiB after decompression.
//!
//! **Addresses** are the index's keys: `@ob:Cube`; a collection `@gr:Props`. Locations and
//! scales in the file's units, rotations in radians (Euler, in the object's own order).
//!
//! **Results as data** ([`Typed`]): each verb's answer as data, for the call's result a host
//! builds (`data`, with `data_schema` `typed` and `data_spec` `blend.file.<verb>/1`, as
//! `blend.file.scene-list/1`; the file verbs' shapes are their own, apart from the live verbs'
//! `blend.<verb>/1`): [`SceneList::typed`], [`Verb::typed`] and [`meshes_typed`]. The text form
//! ([`SceneList::lines`], [`MeshReport::lines`](crate::meshread::MeshReport::lines)) is
//! unchanged. Each shape is pinned in `tests/results_golden/`.
//!
//! **Warnings** ([`WriteWarning`]): an edit that is written can come with heads-ups, the
//! writer's [`Written::warnings`] ([`apply`]), [`Ran::warnings`] ([`run`]): `blend_constrained`,
//! a transform written to an object with constraints, which decide where the program shows it.
//! The kit has no warning a pack's call returns (its `ToolOutput::warnings` are the kit's own
//! `note: ` lines about a call), so they are the pack's own, in its shape: each edit verb's data
//! holds them as `warnings` ([`warning_data`]), and each gives its `note: ` line for stderr
//! ([`WriteWarning::note`]), as the kit's warnings read. A warning is no refusal: its code is
//! no entry of the pack's refusals, and the exit is 0.

use std::io::Read;

use k2_mpc_base::errors::{ErrorEntry, ErrorReport, Value as Json};
use k2_mpc_base::parity::{FileVerb, Registry, RegistryError, VerbKind};
use k2_mpc_base::session::{BlobSession, BlobWriteResult, ConnId, SessionError};
use k2_mpc_base::store::VersionId;
use k2_mpc_base::tools::exit::{NO, NOT_FOUND, OK, USAGE};
use k2_mpc_base::tools::{session_exit, ToolError};

use crate::error::{ReadError, ReadLimits};
use crate::live::errors as reg;
use crate::meshread::{mesh_read, MeshRead, MeshReadError, MeshReadLimits};
use crate::reader::read_blend;
use crate::scene::{read_scene, Detail, SceneCamera, SceneModel};
use crate::write::{decompress, write, Change, Edit, WriteError, WriteWarning, Written};

/// The person actions the verbs serve (the host's parity rows, by name).
pub const ROWS: &[&str] = &[
    "browse the scene",
    "move, rotate or scale an object",
    "show or hide an object",
    "rename an object",
    "see the scene in 3D",
];

const RD: VerbKind = VerbKind::Read;
const ED: VerbKind = VerbKind::Edit;
const READ: &[u8] = &[OK, USAGE, NOT_FOUND];
const EDIT: &[u8] = &[OK, NO, USAGE, NOT_FOUND];

/// Every verb, as data for [`Registry::for_file`]. The index's own verbs (`object list`,
/// `object show`, `object diff`, `object blame`) come from the kit ([`crate::pack::VERBS`]).
pub const VERBS: &[FileVerb] = &[
    FileVerb::new(
        "scene list",
        RD,
        "(each collection: address, name, holders, objects, visibility; each object: address, \
         name, data, parent, collections, location, rotation, rotation mode, scale, visibility; \
         each camera: its object's address and name, data, the scenes it is active in, its \
         fields)",
        READ,
        &["browse the scene"],
    ),
    FileVerb::new(
        "object place",
        ED,
        "@ob:NAME --base V [--loc X,Y,Z] [--rot X,Y,Z] [--scale X,Y,Z] (radians, Euler; an \
         object rotating by quaternion or axis and angle: exit 2; a change to a channel an \
         action or a driver animates: exit 2; an object with constraints: written, with the \
         warning blend_constrained; a stale V: exit 1)",
        EDIT,
        &["move, rotate or scale an object"],
    ),
    FileVerb::new(
        "object visibility",
        ED,
        "@ob:NAME --base V [--viewport on|off] [--select on|off] [--render on|off] (a bit an \
         action or a driver animates, flipped: exit 2; a stale V: exit 1)",
        EDIT,
        &["show or hide an object"],
    ),
    FileVerb::new(
        "object rename",
        ED,
        "@ob:NAME --base V --name NEW (unique among the file's objects, inside the name \
         field's length; a stale V: exit 1)",
        EDIT,
        &["rename an object"],
    ),
    FileVerb::new(
        "scene meshes",
        RD,
        "(the scene drawn and how it was chosen; each object drawn: name, address, mesh, \
         vertices, triangles, hidden, modifiers, transform not evaluated; each camera: name, \
         address, vertical field of view, aspect, sensor fit, active; each mesh object or \
         camera left out, with its reason; the K2SCENE1 bytes, for a host that writes them)",
        READ,
        &["see the scene in 3D"],
    ),
];

/// The verbs as the kit's file-verb registry.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::for_file(VERBS)
}

/// The version of every file verb's data shape ([`Typed::spec`], `blend.file.<verb>/1`): a new
/// one when a field is renamed, removed or given another type; a new field keeps it.
pub const DATA_VERSION: u32 = 1;

/// A file verb's answer as data, for the call's result a host builds: [`Typed::data`] is the
/// result's `data` (`data_schema` `typed`), [`Typed::spec`] its `data_spec`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Typed {
    /// The shape's name: `blend.file.` and the verb, its spaces as `-`
    /// (`blend.file.scene-list`).
    pub name: &'static str,
    /// The shape's version ([`DATA_VERSION`]).
    pub version: u32,
    /// The data.
    pub data: Json,
}

impl Typed {
    fn new(name: &'static str, data: Json) -> Typed {
        Typed {
            name,
            version: DATA_VERSION,
            data,
        }
    }

    /// The shape's name and version, `<name>/<version>`: the result's `data_spec`.
    pub fn spec(&self) -> String {
        format!("{}/{}", self.name, self.version)
    }
}

/// Floats as the index shows them (`x,y,z`) as data: a list of numbers, or null when the text
/// holds a part that isn't a finite number.
fn floats_data(text: Option<&str>) -> Json {
    text.and_then(|t| {
        t.split(',')
            .map(|x| x.trim().parse::<f64>().ok().and_then(Json::float))
            .collect::<Option<Vec<Json>>>()
    })
    .map_or(Json::Null, Json::List)
}

/// A float as data: a number, its shortest text read back (so it is the same `f32`), or null
/// when it isn't finite.
fn float_data(x: f32) -> Json {
    x.to_string()
        .parse::<f64>()
        .ok()
        .and_then(Json::float)
        .unwrap_or(Json::Null)
}

/// Three floats a verb was given, as data: a list of numbers (null for one that isn't finite:
/// the writer refuses it).
fn triple_data(v: Option<[f32; 3]>) -> Json {
    v.map_or(Json::Null, |v| {
        Json::List(v.iter().map(|x| float_data(*x)).collect())
    })
}

/// A warning as data: `{code, object, kinds, text}`, `text` its `note: ` line
/// ([`WriteWarning::note`]); `kinds` each kind of constraint, by the file's schema.
pub fn warning_data(w: &WriteWarning) -> Json {
    let kinds = match w {
        WriteWarning::Constrained { kinds, .. } => kinds,
    };
    Json::obj([
        ("code", Json::from(w.code())),
        ("object", Json::from(w.object())),
        (
            "kinds",
            Json::List(kinds.iter().map(|k| Json::from(k.as_str())).collect()),
        ),
        ("text", Json::from(w.note().as_str())),
    ])
}

/// Warnings as data: a list of [`warning_data`].
fn warnings_data(warnings: &[WriteWarning]) -> Json {
    Json::List(warnings.iter().map(warning_data).collect())
}

/// The visibility bits as data: `{viewport, select, render}`, each `true` when hidden (off),
/// null when the file doesn't name it.
fn hidden_data(h: &[Option<bool>; 3]) -> Json {
    Json::obj([
        ("viewport", Json::from(h[0])),
        ("select", Json::from(h[1])),
        ("render", Json::from(h[2])),
    ])
}

/// One edit verb.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Verb {
    /// `object place`.
    Place {
        /// The object's key.
        object: String,
        /// `--loc`.
        loc: Option<[f32; 3]>,
        /// `--rot`.
        rot: Option<[f32; 3]>,
        /// `--scale`.
        scale: Option<[f32; 3]>,
    },
    /// `object visibility`.
    Visibility {
        /// The object's key.
        object: String,
        /// `--viewport off` hides it in viewports (`Some(true)`: hidden).
        viewport: Option<bool>,
        /// `--select off`: not selectable.
        select: Option<bool>,
        /// `--render off`: disabled in renders.
        render: Option<bool>,
    },
    /// `object rename`.
    Rename {
        /// The object's key.
        object: String,
        /// `--name`.
        name: String,
    },
}

/// Why a verb didn't run.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VerbError {
    /// The writer refused (not found: exit 4; anything else: exit 2).
    #[error(transparent)]
    Write(#[from] WriteError),
    /// A bad argument (exit 2).
    #[error("{0}")]
    Usage(String),
    /// The doc moved on from the version the agent read (exit 1): re-read and retry.
    #[error("the doc is at version {actual}, not {expected}: re-read it")]
    Stale {
        /// The version the agent read.
        expected: VersionId,
        /// The doc's version now.
        actual: VersionId,
    },
    /// The session refused (its own exit).
    #[error(transparent)]
    Session(#[from] SessionError),
    /// The mesh read refused (exit 2): a limit, or a file the live tier doesn't read.
    #[error(transparent)]
    MeshRead(#[from] MeshReadError),
}

impl VerbError {
    /// The exit code, from the kit's table.
    pub fn exit(&self) -> u8 {
        match self {
            VerbError::Write(WriteError::NotFound(_)) => NOT_FOUND,
            VerbError::Write(_) | VerbError::Usage(_) | VerbError::MeshRead(_) => USAGE,
            VerbError::Stale { .. } => NO,
            VerbError::Session(e) => session_exit(e),
        }
    }

    /// The pack's registered refusal this is ([`crate::live::errors::ERRORS`]), by its code;
    /// `None` for a usage error, a stale base or the session's own refusal (the kit's).
    pub fn entry(&self) -> Option<&'static ErrorEntry> {
        let code = match self {
            VerbError::Write(w) => match w {
                WriteError::Read(r) => read_code(r),
                WriteError::Version { .. } => "blend_version",
                WriteError::NotFound(_) => "blend_not_found",
                WriteError::Ambiguous(_) => "blend_ambiguous",
                WriteError::NotAnObject(_) => "blend_not_an_object",
                WriteError::Linked(_) | WriteError::Field { .. } => "blend_not_editable",
                WriteError::RotationMode { .. } => "blend_rotation_mode",
                WriteError::Driven { .. } => "blend_driven",
                WriteError::Name { .. } => "blend_name",
                WriteError::Value => "blend_value",
                WriteError::Check { .. } => "blend_write_check",
            },
            VerbError::MeshRead(m) => match m {
                MeshReadError::TooLarge { .. } => "blend_too_large",
                MeshReadError::Declined { code } => match *code {
                    "not_blend" => "blend_not_blend",
                    "unknown_form" => "blend_unknown_form",
                    "unreadable" => "blend_unreadable",
                    "live_too_large" => "blend_too_large",
                    "live_version" => "blend_version",
                    _ => "blend_mesh_declined",
                },
                MeshReadError::Invalid { .. } => "blend_mesh_invalid",
                MeshReadError::Pairing { .. } => "blend_mesh_pairing",
            },
            VerbError::Usage(_) | VerbError::Stale { .. } | VerbError::Session(_) => return None,
        };
        reg::by_code(code)
    }

    /// The pack's code for this refusal ([`VerbError::entry`]'s name), or `None` (the kit's).
    pub fn code(&self) -> Option<&'static str> {
        self.entry().map(|e| e.name)
    }

    /// **What a host reports** for `verb`'s refusal: the pack's registered entry, the refusal's
    /// own text as the message (fixed text: nothing quoted from the file), and the details
    /// `code` and `verb`, with `address` (the key the call named), `format` (the file's
    /// version, `5.02`), `channel` (a driven channel refused, as its F-curve names it:
    /// `location[2]`, `hide_viewport`) or `decline` (the mesh read's decline code) when there
    /// is one. A usage
    /// error, a stale base and the session's refusals are the kit's own reports.
    pub fn report(&self, verb: &str) -> ErrorReport {
        let Some(entry) = self.entry() else {
            return match self {
                VerbError::Usage(m) => ToolError::usage(m.clone()).report(),
                VerbError::Stale { expected, actual } => SessionError::Stale {
                    expected: *expected,
                    actual: *actual,
                }
                .report(),
                VerbError::Session(e) => e.report(),
                // every other refusal has an entry
                _ => ToolError::usage(self.to_string()).report(),
            };
        };
        let message = match self {
            VerbError::Write(WriteError::Version { version }) => {
                reg::file_version_message(Some(&reg::release(*version)), true)
            }
            VerbError::MeshRead(MeshReadError::Declined {
                code: "live_version",
            }) => reg::file_version_message(None, false),
            _ => self.to_string(),
        };
        let mut r = ErrorReport::new(entry, message)
            .with("code", entry.name)
            .with("verb", verb);
        match self {
            VerbError::Write(
                WriteError::NotFound(k)
                | WriteError::Ambiguous(k)
                | WriteError::NotAnObject(k)
                | WriteError::Linked(k),
            ) => r = r.with("address", k.as_str()),
            VerbError::Write(WriteError::Version { version }) => {
                r = r.with("format", reg::release(*version));
            }
            VerbError::Write(WriteError::Driven { channel, .. }) => {
                r = r.with("channel", channel.as_str());
            }
            VerbError::MeshRead(MeshReadError::Declined { code }) => r = r.with("decline", *code),
            _ => {}
        }
        r
    }
}

/// The pack's code for a reader's refusal of a file verb's file.
fn read_code(e: &ReadError) -> &'static str {
    match e {
        ReadError::NotBlend => "blend_not_blend",
        ReadError::Unknown { .. } => "blend_unknown_form",
        ReadError::Limit { .. } => "blend_too_large",
        _ => "blend_unreadable",
    }
}

impl Verb {
    /// What the verb wrote, as data, once it has run, with the `warnings` its write came with
    /// ([`Written::warnings`], [`Ran::warnings`]): `blend.file.object-place/1` `{object,
    /// location, rotation, scale, warnings}` (three numbers each, null for one not given),
    /// `blend.file.object-visibility/1` `{object, hidden: {viewport, select, render},
    /// warnings}` (`true`: off, null: left as it was), `blend.file.object-rename/1` `{object,
    /// name, warnings}` (`object` the key the call named; `warnings` a list of
    /// [`warning_data`], empty for most writes).
    pub fn typed(&self, warnings: &[WriteWarning]) -> Typed {
        let mut t = self.typed_data();
        if let Json::Obj(fields) = &mut t.data {
            fields.push(("warnings".into(), warnings_data(warnings)));
        }
        t
    }

    /// [`Verb::typed`] without its warnings.
    fn typed_data(&self) -> Typed {
        match self {
            Verb::Place {
                object,
                loc,
                rot,
                scale,
            } => Typed::new(
                "blend.file.object-place",
                Json::obj([
                    ("object", Json::from(object.as_str())),
                    ("location", triple_data(*loc)),
                    ("rotation", triple_data(*rot)),
                    ("scale", triple_data(*scale)),
                ]),
            ),
            Verb::Visibility {
                object,
                viewport,
                select,
                render,
            } => Typed::new(
                "blend.file.object-visibility",
                Json::obj([
                    ("object", Json::from(object.as_str())),
                    ("hidden", hidden_data(&[*viewport, *select, *render])),
                ]),
            ),
            Verb::Rename { object, name } => Typed::new(
                "blend.file.object-rename",
                Json::obj([
                    ("object", Json::from(object.as_str())),
                    ("name", Json::from(name.as_str())),
                ]),
            ),
        }
    }
}

/// The writer's edits a verb makes.
pub fn edits(verb: &Verb) -> Result<Vec<Edit>, VerbError> {
    let out: Vec<Edit> = match verb {
        Verb::Place {
            object,
            loc,
            rot,
            scale,
        } => [
            loc.map(Change::Location),
            rot.map(Change::Rotation),
            scale.map(Change::Scale),
        ]
        .into_iter()
        .flatten()
        .map(|c| Edit::new(object.clone(), c))
        .collect(),
        Verb::Visibility {
            object,
            viewport,
            select,
            render,
        } => {
            if viewport.is_none() && select.is_none() && render.is_none() {
                Vec::new()
            } else {
                vec![Edit::new(
                    object.clone(),
                    Change::Visibility {
                        viewport: *viewport,
                        select: *select,
                        render: *render,
                    },
                )]
            }
        }
        Verb::Rename { object, name } => {
            vec![Edit::new(object.clone(), Change::Name(name.clone()))]
        }
    };
    if out.is_empty() {
        return Err(VerbError::Usage("nothing to change: give a field".into()));
    }
    Ok(out)
}

/// What a verb does to the file `bytes` holds: a pure function.
pub fn apply(bytes: &[u8], verb: &Verb, limits: &ReadLimits) -> Result<Written, VerbError> {
    Ok(write(bytes, &edits(verb)?, limits)?)
}

/// One collection, as `scene list` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListedCollection {
    /// `@gr:Name`.
    pub address: String,
    /// Its name.
    pub name: String,
    /// The scenes and collections that hold it, by key.
    pub holders: Vec<String>,
    /// The objects it holds, by key.
    pub objects: Vec<String>,
    /// `hide_viewport`, `hide_select`, `hide_render`, when named.
    pub hidden: [Option<bool>; 3],
}

/// One object, as `scene list` shows it. A link placeholder of an object (see
/// [`crate::scene`]) has its address, name, holders and `linked` only: what it holds is in its
/// library.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ListedObject {
    /// `@ob:Name`.
    pub address: String,
    /// Its name.
    pub name: String,
    /// Its data's key and kind, or `None` for an empty or a link placeholder.
    pub data: Option<(String, String)>,
    /// Its parent's key.
    pub parent: Option<String>,
    /// The scenes and collections that hold it, by key.
    pub collections: Vec<String>,
    /// `loc`, as the index shows it (`x,y,z`).
    pub location: Option<String>,
    /// `rot` (Euler, radians).
    pub rotation: Option<String>,
    /// `rotation_mode`, when named.
    pub rotation_mode: Option<String>,
    /// `size` (or `scale`).
    pub scale: Option<String>,
    /// `hide_viewport`, `hide_select`, `hide_render`, when named.
    pub hidden: [Option<bool>; 3],
    /// Linked from a library: no verb writes it.
    pub linked: bool,
}

/// `scene list`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct SceneList {
    /// Every collection, in file order.
    pub collections: Vec<ListedCollection>,
    /// Every object, in file order, link placeholders of objects among them.
    pub objects: Vec<ListedObject>,
    /// Every camera ([`SceneModel::cameras`]): each object whose data is a camera, in file
    /// order, with its name and the camera's fields.
    pub cameras: Vec<SceneCamera>,
}

impl SceneList {
    /// The list as data (`blend.file.scene-list/1`): `{collections, objects, cameras}`. A
    /// collection is `{address, name, holders, objects, hidden}`; an object `{address, name,
    /// data, parent, collections, location, rotation, rotation_mode, scale, hidden, linked}`
    /// (`data` `{key, kind}` or null; `location`, `rotation` and `scale` three numbers or
    /// null); a camera `{object, name, data, active_in, fields, linked}` (`fields` each camera
    /// field as the index's text). `hidden` is `{viewport, select, render}`, `true` when off.
    pub fn typed(&self) -> Typed {
        let colls = self.collections.iter().map(|c| {
            Json::obj([
                ("address", Json::from(c.address.as_str())),
                ("name", Json::from(c.name.as_str())),
                ("holders", Json::list(c.holders.iter())),
                ("objects", Json::list(c.objects.iter())),
                ("hidden", hidden_data(&c.hidden)),
            ])
        });
        let objs = self.objects.iter().map(|o| {
            Json::obj([
                ("address", Json::from(o.address.as_str())),
                ("name", Json::from(o.name.as_str())),
                (
                    "data",
                    o.data.as_ref().map_or(Json::Null, |(k, kind)| {
                        Json::obj([
                            ("key", Json::from(k.as_str())),
                            ("kind", Json::from(kind.as_str())),
                        ])
                    }),
                ),
                ("parent", Json::from(o.parent.as_deref())),
                ("collections", Json::list(o.collections.iter())),
                ("location", floats_data(o.location.as_deref())),
                ("rotation", floats_data(o.rotation.as_deref())),
                ("rotation_mode", Json::from(o.rotation_mode.as_deref())),
                ("scale", floats_data(o.scale.as_deref())),
                ("hidden", hidden_data(&o.hidden)),
                ("linked", Json::from(o.linked)),
            ])
        });
        let cams = self.cameras.iter().map(|c| {
            Json::obj([
                ("object", Json::from(c.object.as_str())),
                ("name", Json::from(c.name.as_str())),
                ("data", Json::from(c.data.as_str())),
                ("active_in", Json::list(c.active_in.iter())),
                (
                    "fields",
                    Json::obj(c.fields.iter().map(|(k, v)| (k.as_str(), Json::from(v)))),
                ),
                ("linked", Json::from(c.linked)),
            ])
        });
        Typed::new(
            "blend.file.scene-list",
            Json::obj([
                ("collections", Json::List(colls.collect())),
                ("objects", Json::List(objs.collect())),
                ("cameras", Json::List(cams.collect())),
            ]),
        )
    }

    /// One line per collection, object and camera, for a person or an agent reading text.
    pub fn lines(&self) -> Vec<String> {
        let vis = |h: &[Option<bool>; 3]| {
            let names = ["viewport", "select", "render"];
            let off: Vec<&str> = names
                .iter()
                .zip(h)
                .filter(|(_, x)| **x == Some(true))
                .map(|(n, _)| *n)
                .collect();
            if off.is_empty() {
                "shown".to_owned()
            } else {
                format!("off: {}", off.join(","))
            }
        };
        let mut out = Vec::new();
        for c in &self.collections {
            out.push(format!(
                "{} collection {:?} in [{}] objects {} {}",
                c.address,
                c.name,
                c.holders.join(" "),
                c.objects.len(),
                vis(&c.hidden)
            ));
        }
        for o in &self.objects {
            out.push(format!(
                "{} object {:?} data {} parent {} in [{}] loc {} rot {} ({}) scale {} {}{}",
                o.address,
                o.name,
                o.data
                    .as_ref()
                    .map_or("-".into(), |(k, kind)| format!("{k} ({kind})")),
                o.parent.as_deref().unwrap_or("-"),
                o.collections.join(" "),
                o.location.as_deref().unwrap_or("-"),
                o.rotation.as_deref().unwrap_or("-"),
                o.rotation_mode.as_deref().unwrap_or("-"),
                o.scale.as_deref().unwrap_or("-"),
                vis(&o.hidden),
                if o.linked { " linked" } else { "" }
            ));
        }
        for c in &self.cameras {
            let fields: Vec<String> = c.fields.iter().map(|(k, v)| format!("{k} {v}")).collect();
            out.push(format!(
                "{} camera {:?} data {} active in [{}] {}{}",
                c.object,
                c.name,
                c.data,
                c.active_in.join(" "),
                fields.join(" "),
                if c.linked { " linked" } else { "" }
            ));
        }
        out
    }
}

fn model_of(bytes: &[u8], limits: &ReadLimits) -> Result<SceneModel, VerbError> {
    let (_, raw) = decompress(bytes, limits).map_err(WriteError::from)?;
    let file = read_blend(&mut &raw[..], limits).map_err(WriteError::from)?;
    Ok(read_scene(&file).map_err(WriteError::from)?)
}

/// `scene list` on the file `bytes` holds.
pub fn list(bytes: &[u8], limits: &ReadLimits) -> Result<SceneList, VerbError> {
    let m = model_of(bytes, limits)?;
    let key = |i: usize| m.ids[i].key.clone().unwrap_or_else(|| format!("#{i}"));
    let keys = |v: &[usize]| v.iter().map(|&i| key(i)).collect::<Vec<_>>();
    let holders = |i: usize| -> Vec<String> {
        m.ids
            .iter()
            .enumerate()
            .filter(|(_, h)| match &h.detail {
                Detail::Scene {
                    objects,
                    collections,
                    ..
                }
                | Detail::Collection {
                    objects,
                    collections,
                } => objects.contains(&i) || collections.contains(&i),
                _ => false,
            })
            .map(|(h, _)| key(h))
            .collect()
    };
    let named = |i: usize, n: &str| {
        m.ids[i]
            .named
            .iter()
            .find(|(f, _)| f == n)
            .map(|(_, v)| v.clone())
    };
    let hidden = |i: usize| {
        ["hide_viewport", "hide_select", "hide_render"].map(|n| named(i, n).map(|v| v == "true"))
    };
    let field = |i: usize, names: &[&str]| {
        names.iter().find_map(|n| {
            m.ids[i]
                .fields
                .iter()
                .find(|(f, _)| f == n)
                .map(|(_, v)| v.clone())
        })
    };
    let mut out = SceneList::default();
    for (i, id) in m.ids.iter().enumerate() {
        match &id.detail {
            Detail::Collection { objects, .. } => out.collections.push(ListedCollection {
                address: key(i),
                name: id.name.clone(),
                holders: holders(i),
                objects: keys(objects),
                hidden: hidden(i),
            }),
            Detail::Object { data, parent } => out.objects.push(ListedObject {
                address: key(i),
                name: id.name.clone(),
                data: data.map(|d| (key(d), m.ids[d].kind.clone())),
                parent: parent.map(key),
                collections: holders(i),
                location: field(i, &["loc"]),
                rotation: field(i, &["rot"]),
                rotation_mode: named(i, "rotation_mode"),
                scale: field(i, &["scale", "size"]),
                hidden: hidden(i),
                linked: id.library.is_some(),
            }),
            // a link placeholder of an object: what it holds is in its library
            _ if id.kind == "object" && id.struct_name == "ID" => {
                out.objects.push(ListedObject {
                    address: key(i),
                    name: id.name.clone(),
                    data: None,
                    parent: None,
                    collections: holders(i),
                    location: None,
                    rotation: None,
                    rotation_mode: None,
                    scale: None,
                    hidden: [None; 3],
                    linked: id.library.is_some(),
                });
            }
            _ => {}
        }
    }
    out.cameras = m.cameras();
    Ok(out)
}

/// The doc's current bytes and version, read through the session.
fn current(
    session: &BlobSession,
    conn: ConnId,
    limits: &ReadLimits,
) -> Result<(VersionId, Vec<u8>), VerbError> {
    let r = session.read(conn)?;
    let mut bytes = Vec::new();
    r.reader
        .take(limits.max_raw_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| VerbError::Usage(format!("reading the doc: {e}")))?;
    Ok((r.version, bytes))
}

/// `scene list` through a session: the version it read, and the list.
pub fn run_list(
    session: &BlobSession,
    conn: ConnId,
    limits: &ReadLimits,
) -> Result<(VersionId, SceneList), VerbError> {
    let (v, bytes) = current(session, conn, limits)?;
    Ok((v, list(&bytes, limits)?))
}

/// `scene meshes`: the mesh read of `bytes` ([`crate::meshread::mesh_read`]); its report's
/// lines are the answer, its K2SCENE1 bytes for a host that writes them.
pub fn meshes(bytes: &[u8], limits: &MeshReadLimits) -> Result<MeshRead, VerbError> {
    Ok(mesh_read(bytes, limits)?)
}

/// `scene meshes`' answer as data (`blend.file.scene-meshes/1`): the report, never the
/// geometry. `{scene, scene_from, meshes, objects, cameras, hidden, transform_not_evaluated,
/// modifiers_truncated, collection_instances, heap, drawn, drawn_cameras, skipped,
/// triangulation, k2scene_bytes}`: `drawn` each object written `{name, key, mesh, mesh_hash,
/// vertices, triangles, hidden, modifiers, transform_not_evaluated}` (`key` its address,
/// `@obj:N`; `hidden` `self`, `collection` or null); `drawn_cameras` each camera written `{name,
/// key, yfov, aspect, sensor_fit, active}` (`key` `@cam:N`; `yfov` in radians); `skipped` each
/// mesh object or camera left out `{name, reason}`; `triangulation` what triangulating counted;
/// `heap` the bytes the read was worked out to need; `k2scene_bytes` the K2SCENE1 output's
/// length. The fields are built as a list, so a field added to the report joins them without
/// changing the version.
pub fn meshes_typed(read: &MeshRead) -> Typed {
    let r = &read.report;
    let drawn = r.drawn.iter().map(|d| {
        Json::obj([
            ("name", Json::from(d.name.as_str())),
            ("key", Json::from(d.key.as_str())),
            ("mesh", Json::from(d.mesh)),
            ("mesh_hash", Json::from(d.mesh_hash.as_str())),
            ("vertices", Json::from(d.vertices)),
            ("triangles", Json::from(d.triangles)),
            ("hidden", Json::from(d.hidden)),
            ("modifiers", Json::list(d.modifiers.iter())),
            (
                "transform_not_evaluated",
                Json::from(d.transform_not_evaluated),
            ),
        ])
    });
    let cameras = r.cameras.iter().map(|c| {
        Json::obj([
            ("name", Json::from(c.name.as_str())),
            ("key", Json::from(c.key.as_str())),
            ("yfov", float_data(c.yfov)),
            ("aspect", float_data(c.aspect)),
            ("sensor_fit", Json::from(c.sensor_fit)),
            ("active", Json::from(c.active)),
        ])
    });
    let skipped = r.skipped.iter().map(|s| {
        Json::obj([
            ("name", Json::from(s.name.as_str())),
            ("reason", Json::from(s.reason)),
        ])
    });
    let t = &r.tri;
    let tri = Json::obj([
        ("faces", Json::from(t.faces)),
        ("triangles", Json::from(t.triangles)),
        ("convex", Json::from(t.convex)),
        ("ear_clipped", Json::from(t.ear_clipped)),
        ("degenerate", Json::from(t.degenerate)),
        ("ear_clip_failed", Json::from(t.ear_clip_failed)),
        ("large_face_fanned", Json::from(t.large_face_fanned)),
        ("ear_tests", Json::from(t.ear_tests)),
    ]);
    let fields: Vec<(&str, Json)> = vec![
        ("scene", Json::from(r.scene.as_deref())),
        ("scene_from", Json::from(r.scene_from)),
        ("meshes", Json::from(r.meshes)),
        ("objects", Json::from(r.objects)),
        ("cameras", Json::from(r.cameras.len())),
        ("hidden", Json::from(r.hidden)),
        (
            "transform_not_evaluated",
            Json::from(r.transform_not_evaluated),
        ),
        ("modifiers_truncated", Json::from(r.modifiers_truncated)),
        ("collection_instances", Json::from(r.collection_instances)),
        ("heap", Json::from(r.heap)),
        ("drawn", Json::List(drawn.collect())),
        ("drawn_cameras", Json::List(cameras.collect())),
        ("skipped", Json::List(skipped.collect())),
        ("triangulation", tri),
        ("k2scene_bytes", Json::from(read.k2scene.len())),
    ];
    Typed::new("blend.file.scene-meshes", Json::obj(fields))
}

/// `scene meshes` through a session: the version read and its mesh read.
pub fn run_meshes(
    session: &BlobSession,
    conn: ConnId,
    limits: &MeshReadLimits,
) -> Result<(VersionId, MeshRead), VerbError> {
    let (v, bytes) = current(session, conn, &limits.live.read)?;
    Ok((v, meshes(&bytes, limits)?))
}

/// An edit verb run through a session ([`run`]): the kit's write, and the warnings the write
/// came with ([`Written::warnings`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ran {
    /// The kit's write.
    pub write: BlobWriteResult,
    /// The writer's warnings ([`WriteWarning`]); empty for most writes.
    pub warnings: Vec<WriteWarning>,
}

/// An edit verb through a session, on the version the agent read (`base`): refused when the
/// doc has moved on, written with `base` as the kit's base (checked again at commit), with the
/// warnings the write came with.
pub fn run(
    session: &BlobSession,
    conn: ConnId,
    base: VersionId,
    verb: &Verb,
    limits: &ReadLimits,
) -> Result<Ran, VerbError> {
    let (v, bytes) = current(session, conn, limits)?;
    if v != base {
        return Err(VerbError::Stale {
            expected: base,
            actual: v,
        });
    }
    let w = apply(&bytes, verb, limits)?;
    let write = session
        .write_bytes(conn, &w.bytes, base)
        .map_err(|e| match e {
            SessionError::Stale { expected, actual } => VerbError::Stale { expected, actual },
            e => VerbError::Session(e),
        })?;
    Ok(Ran {
        write,
        warnings: w.warnings,
    })
}
