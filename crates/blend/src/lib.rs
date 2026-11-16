//! Reads BLEND scene files: the file-type pack for 3D scenes in the multiplayer kit.
//!
//! - [`reader`]: the file, forward only and under [`ReadLimits`]: the header in both of its
//!   forms ([`header`]), plain, gzip or zstd storage ([`Compression`]), every file block, and the
//!   file's own schema ([`sdna`]), through which every struct and field is looked up by name.
//!   Anything this reader doesn't know or can't trust is refused with a typed [`ReadError`].
//! - [`scene`]: the scene model: every data-block with its key (type code and name), kind and
//!   name; what scenes and collections hold; objects' data, parents, transforms and material
//!   slots; a few fields of cameras, lights and materials; and a content digest of everything
//!   else, with pointers replaced by what they name.
//! - [`pack`]: [`BlendPack`], the kit's `BlobPack` for these files: each version indexed into
//!   the kit's structure index, a tree of scenes, collections, objects and every other
//!   data-block, so people and agents can comment on, diff and blame by data-block. Agents may
//!   write the file (`AgentWrites::Allowed`); their edits come through [`verbs`].
//! - [`write`](mod@write): fixed-size fields of an object (location, rotation, scale, the visibility bits,
//!   the name inside its field) patched in place, every other byte kept, checked by reading the
//!   result again; anything it can't prove safe is refused, typed.
//! - [`merge`]: the three-way merge of a person's app save against its base, by object and
//!   field: the person's save is the file, the agent's changes replayed onto it; a field both
//!   changed is reported, and kept both whole-file until the kit holds conflicts. It needs the
//!   version the app had open as its base, which the kit doesn't hand a pack yet, so a session
//!   keeps both on every such save ([`pack`]).
//! - [`verbs`]: the agent's verbs (`scene list`, `scene meshes`, `object place`,
//!   `object visibility`, `object rename`), as registry data and as edits run through a session
//!   on the version the agent read; each answer as data ([`verbs::Typed`]) and each refusal by
//!   its registered entry ([`live::errors`]).
//! - [`live`]: [`live::BlendLive`], the kit's live `Pack` for these files: an outliner of the
//!   scene's collections and objects on the kit's structured mode, agents' verbs on it, and the
//!   write-back that patches the changed fields into the file the kit keeps (its carrier),
//!   paired by the kit's pairing table.
//!
//! - [`meshread`]: the mesh read: a version's meshes as triangles, in the K2SCENE1 layout a
//!   host's renderer takes; every count bounded before anything is allocated for it.
//!
//! Every layout this crate reads comes from the file in hand or from openly licensed sample
//! files and public release notes (`PROVENANCE.md`).
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod compress;
pub mod error;
pub mod header;
pub mod live;
pub mod merge;
pub mod meshread;
pub mod mutant;
pub mod pack;
pub mod reader;
pub mod scene;
pub mod sdna;
pub mod verbs;
pub mod write;

#[cfg(feature = "fuzzing")]
pub mod fuzzing;
#[cfg(feature = "testgen")]
pub mod testgen;

pub use compress::Compression;
pub use error::{ReadError, ReadLimits};
pub use header::{Header, HeaderForm};
pub use pack::{index_blend, merge_known_base, BlendPack};
pub use reader::{read_blend, BlendFile, Block, BlockHead, Endian, Target};
pub use scene::{read_scene, Detail, FieldSpan, IdBlock, SceneCamera, SceneModel};
pub use sdna::{Field, Sdna, StructDef};
pub use write::{write, Change, Edit, WriteError, Written};
