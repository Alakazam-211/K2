//! The pack's own refusals ([`k2_mpc_base::session::Pack::errors`]): each "no" the pack's verbs
//! and reads can give, by a stable id (`BLEND-Ennn`, never reused) and the code its text prints
//! (`refused: <code>: <why>`, then `next: <call>`). Nothing applies on any of them.
//!
//! - **The live verbs' refusals** (E001 to E006): a verb refuses through
//!   `StructOutput::refused` ([`super::verbs`]), so the host gets the entry's report, its
//!   message the refusal's `why`, with the details `code`, `next` (the call to run next, as the
//!   text's `next:` line), `verb`, and `address` (the collection or object the call named).
//! - **The live tier's declines** (E007 to E011): a file the live tier doesn't open
//!   ([`super::model::read`]). The kit reports a decline as its own `declined_pack`
//!   (`MPC-E407`, exit 4) with the code as `details.code`; the entry here, by that code
//!   ([`by_code`]), says what it means and what to do. The kit's own decline codes
//!   (`live_too_large`, `unreadable`) are the kit's entries, not the pack's.
//! - **The file verbs' refusals** (E012 to E023): [`crate::verbs`]' verbs run by a host on a
//!   file's bytes, each [`crate::verbs::VerbError`] reported by its entry
//!   ([`crate::verbs::VerbError::report`]). A usage error, a stale base and the session's own
//!   refusals are the kit's entries.
//! - **A live doc that is read-only** (E024): kept for a release the live tier reads before the
//!   writer writes it. Such a doc would refuse each live edit verb with it, and each write-back
//!   of a change would fail with its code first (`live_read_only: ...`, the kit's `PackError`).
//!   The writer writes every release the live tier reads (3.06 to 5.03), so no file reaches it.
//! - **A live change the live rules refuse** (the kit's edit check, [`super::BlendLive`]'s
//!   `Pack::check_edit`): the kit refuses it at the call as its own `pack_refused` (`MPC-E823`,
//!   exit 2), the code here as `details.code` and why as `details.reason`. A rule with an entry
//!   gives its code (`blend_driven`, `blend_rotation_mode`, `blend_not_editable`, `blend_name`,
//!   `live_read_only`); every other rule (a list op, a collection's field, a field live v1
//!   doesn't write, a value its field can't hold) gives `blend_live_change` (E025).
//!
//! The releases a file is read from, `3.06 to 5.03` in the texts below, are
//! [`crate::scene::READ_FROM`] and [`crate::scene::READ_TO`]; the releases it is written to,
//! `3.06 to 5.03` too, are [`crate::write::WRITE_FROM`] and [`crate::write::WRITE_TO`]
//! (`tests/results.rs` holds the texts to them).

use k2_mpc_base::errors::ErrorEntry;
use k2_mpc_base::tools::exit::{NOT_FOUND, USAGE};

/// The live edit verbs, and the host's write-back.
const LIVE_EDITS: &[&str] = &[
    "object place",
    "object visibility",
    "object rename",
    "(host)",
];

/// The verbs of the file verbs' entries: the kit's `@file`, then each file verb by name.
const FILE_VERBS: &[&str] = &[
    "@file",
    "scene list",
    "object place",
    "object visibility",
    "object rename",
    "scene meshes",
];

/// The file verbs' edits only.
const FILE_EDITS: &[&str] = &[
    "@file",
    "object place",
    "object visibility",
    "object rename",
];

/// A decline is the host's open of a file, not a verb's.
const OPEN: &[&str] = &["(host)"];

/// A pack entry ([`ErrorEntry::pack`]) that `verbs` return, in place of any of the live verbs.
const fn with_verbs(entry: ErrorEntry, verbs: &'static [&'static str]) -> ErrorEntry {
    ErrorEntry { verbs, ..entry }
}

// ---------- the live verbs ----------

/// An object no verb writes live: linked from a library, or a field the writer can't find.
pub const NOT_EDITABLE: ErrorEntry = ErrorEntry::pack(
    "BLEND-E001",
    "blend_not_editable",
    USAGE,
    "nothing writes this object live",
    "The object is linked from a library (its data belongs to another file), or the file's \
     schema has no field the writer can find for it, so no verb writes it. Nothing was applied. \
     Change it in the file it comes from, or in the app that saved this one.",
)
.step(
    "object show",
    &["{address}"],
    "Read the object's fields: its key says where it comes from, `edit` what is written.",
);

/// A transform that animation, drivers or a constraint drive: a live placement of it; a live
/// `object visibility` on an object with an animated visibility bit; or a file verb's (or the
/// writer's) change to a channel an action or a driver animates (a component of the transform,
/// or a visibility bit it flips).
pub const DRIVEN: ErrorEntry = ErrorEntry::pack(
    "BLEND-E002",
    "blend_driven",
    USAGE,
    "a channel it changes is driven",
    "An action, an NLA strip or a driver animates the channel (a component of the transform, \
     or a visibility bit), or a constraint acts on the transform, so the program recomputes it \
     when it opens the file and a value written here wouldn't hold. Nothing was applied. Change \
     what drives it in the app. A file verb still writes a channel nothing drives and a bit it \
     keeps as stored (`details.channel` names the one refused: `location[2]`, \
     `hide_viewport`), and a transform under constraints (with the warning \
     `blend_constrained`). Live, a driven transform refuses the whole placement, and any \
     animated visibility bit the whole `object visibility` (`details.channel` names the bits: \
     `hide_viewport,hide_render`). Its name is still written.",
)
.step(
    "object show",
    &["{address}"],
    "Read the object's fields: `edit` is `a` when its transform is driven; `visanim` names its \
     animated visibility bits.",
);

/// `--rot` on an object that rotates by quaternion or by axis and angle.
pub const ROTATION_MODE: ErrorEntry = ErrorEntry::pack(
    "BLEND-E003",
    "blend_rotation_mode",
    USAGE,
    "it rotates by another mode than Euler angles",
    "The object rotates by quaternion or by axis and angle, and a rotation given here is Euler \
     angles. Nothing was applied. Place it without a rotation (its location and scale are \
     written), or change its rotation mode in the app.",
)
.step(
    "object show",
    &["{address}"],
    "Read the object's rotation mode and transform.",
);

/// A name that can't be written.
pub const NAME: ErrorEntry = ErrorEntry::pack(
    "BLEND-E004",
    "blend_name",
    USAGE,
    "the name can't be written",
    "An object's name is not empty, holds no NUL, fits the file's name field, and is no other \
     object's of the file. Nothing was applied. Give another name.",
)
.step(
    "scene list",
    &[],
    "List the collections, then each one's objects, with the names they hold.",
);

/// A rename that other files' links by name won't follow, with no `--break-links`.
pub const BREAKS_LINKS: ErrorEntry = ErrorEntry::pack(
    "BLEND-E005",
    "blend_breaks_links",
    USAGE,
    "other files that link the object by its name won't follow a rename",
    "Other files link an object by its name, so a rename breaks those links: they lose the \
     object. Nothing was applied. Run the rename again with --break-links to accept that loss.",
)
.step(
    "object rename",
    &["{address}", "--name", "{name}", "--break-links"],
    "Rename it, accepting that other files linking it by its old name lose it.",
);

/// An address that names nothing in the scene.
pub const NOT_FOUND_ENTRY: ErrorEntry = ErrorEntry::pack(
    "BLEND-E006",
    "blend_not_found",
    NOT_FOUND,
    "the address names nothing in the scene",
    "The address names no collection or object in the scene as it is now. Nothing was applied. \
     List them again and use an address from the list.",
)
.step(
    "scene list",
    &[],
    "List the collections, then each one's objects.",
);

// ---------- the live tier's declines ----------

/// A file from a release whose saves this pack hasn't checked.
pub const LIVE_VERSION: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E007",
        "live_version",
        NOT_FOUND,
        "Nothing was read: this file was saved by a newer (or older) release of the program that \
         wrote it (format {format}) than this pack has checked (3.06 to 5.03).",
        "Its saves are being checked; until then, open it read-only as a file, or save a copy \
         with a release from 3.06 to 5.03 if you have one.",
    )
    .step(
        "object list",
        &[],
        "Read the file's data-blocks read-only, as a file.",
    ),
    OPEN,
);

/// A data-block holding pointers the reader couldn't resolve.
pub const LIVE_AMBIGUOUS: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E008",
        "live_ambiguous",
        NOT_FOUND,
        "Nothing was read live: {key} holds pointers the reader couldn't resolve.",
        "The scene's collections and objects can't be paired to the file without guessing, so \
         it doesn't open live. Open it read-only as a file.",
    )
    .step(
        "object list",
        &[],
        "Read the file's data-blocks read-only, as a file.",
    ),
    OPEN,
);

/// An object whose transform holds a value that isn't finite.
pub const LIVE_VALUE: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E009",
        "live_value",
        NOT_FOUND,
        "Nothing was read live: {key}: a transform value isn't finite.",
        "A live scene holds finite numbers only, and one of the file's transforms doesn't. Open it \
         read-only as a file, or fix the object's transform in the app.",
    )
    .step(
        "object list",
        &[],
        "Read the file's data-blocks read-only, as a file.",
    ),
    OPEN,
);

/// Not a BLEND file.
pub const NOT_BLEND: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E010",
        "not_blend",
        NOT_FOUND,
        "Nothing was read: the file isn't a BLEND file.",
        "The file starts with no BLEND header, plain or compressed. Open it as an ordinary \
         file.",
    ),
    OPEN,
);

/// A BLEND file in a form this pack doesn't know.
pub const UNKNOWN_FORM: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E011",
        "unknown_form",
        NOT_FOUND,
        "Nothing was read: a BLEND file in a form this pack doesn't know ({what}).",
        "Its header names a form this pack hasn't seen in a saved file, so it is refused rather \
         than guessed. Open it as an ordinary file.",
    ),
    OPEN,
);

// ---------- the file verbs ----------

/// A file verb on bytes that aren't a BLEND file.
pub const FILE_NOT_BLEND: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E012",
        "blend_not_blend",
        USAGE,
        "the file isn't a BLEND file",
        "The file starts with no BLEND header, plain or compressed, so no verb of this pack \
         reads it. Nothing was applied.",
    ),
    FILE_VERBS,
);

/// A file verb on a BLEND file in a form this pack doesn't know.
pub const FILE_UNKNOWN_FORM: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E013",
        "blend_unknown_form",
        USAGE,
        "a BLEND file in a form this pack doesn't know",
        "Its header names a form this pack hasn't seen in a saved file, so it is refused rather \
         than guessed. Nothing was applied.",
    ),
    FILE_VERBS,
);

/// A file that ends early, breaks its own structure, or a stream that failed.
pub const FILE_UNREADABLE: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E014",
        "blend_unreadable",
        USAGE,
        "the file can't be read",
        "The file ends before its last block or breaks its own structure (or the stream reading \
         it failed). Nothing was applied. A save of the file that reads whole is needed.",
    ),
    FILE_VERBS,
);

/// A file over one of the reader's or the mesh read's bounds.
pub const FILE_TOO_LARGE: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E015",
        "blend_too_large",
        USAGE,
        "over a bound the read stays under",
        "The file, or a count in it, is over one of the bounds a read stays under (the bound \
         and its limit are in the message), and a file over one is never read in part. Nothing \
         was applied.",
    )
    .step(
        "object list",
        &[],
        "Read the file's data-blocks from its index, read-only.",
    ),
    FILE_VERBS,
);

/// A file verb on a file from a release whose saves this pack hasn't checked.
pub const FILE_VERSION: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E016",
        "blend_version",
        USAGE,
        "Nothing was applied: this file was saved by a newer (or older) release of the program \
         that wrote it than this pack has checked (3.06 to 5.03).",
        "Its saves are being checked; until then, read it read-only as a file, or save a copy \
         with a release from 3.06 to 5.03 if you have one.",
    )
    .step(
        "scene list",
        &[],
        "Read the scene's collections, objects and cameras, read-only.",
    ),
    FILE_VERBS,
);

/// A key two data-blocks of the file have.
pub const FILE_AMBIGUOUS: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E017",
        "blend_ambiguous",
        USAGE,
        "several data-blocks have the key",
        "The file names two data-blocks alike, so the key doesn't say which one. Nothing was \
         written. Rename one of them in the app.",
    )
    .step("scene list", &[], "List the objects by their keys."),
    FILE_EDITS,
);

/// A key that names a data-block that isn't an object.
pub const FILE_NOT_AN_OBJECT: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E018",
        "blend_not_an_object",
        USAGE,
        "the key names a data-block that isn't an object",
        "The verbs that write place, visibility and names write objects only (`@ob:NAME`). \
         Nothing was written. Give an object's key.",
    )
    .step("scene list", &[], "List the objects by their keys."),
    FILE_EDITS,
);

/// A value that isn't finite.
pub const FILE_VALUE: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E019",
        "blend_value",
        USAGE,
        "a value isn't finite",
        "A location, rotation or scale is three finite numbers. Nothing was written. Give finite \
         numbers.",
    )
    .step(
        "scene list",
        &[],
        "Read the object's location, rotation and scale as they are.",
    ),
    FILE_EDITS,
);

/// The writer's own read-back of its result disagreed.
pub const FILE_WRITE_CHECK: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E020",
        "blend_write_check",
        USAGE,
        "the written file didn't check",
        "The writer reads every result again before returning it, and this one didn't read \
         back as written, so nothing was written; the file is as it was. This is a fault of the \
         pack: report it to the pack's maintainers, with the file's format version and the verb.",
    ),
    FILE_EDITS,
);

/// A file the mesh read reads only as the live tier does, which the live tier declines.
pub const MESH_DECLINED: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E021",
        "blend_mesh_declined",
        USAGE,
        "the mesh read reads only what the live tier reads",
        "The mesh read decodes a file as the live tier does, and the live tier declines this \
         one (`details.decline` says why: pointers the reader couldn't resolve, or a transform \
         that isn't finite). Nothing was drawn.",
    )
    .step(
        "scene list",
        &[],
        "Read the scene's collections, objects and cameras, read-only.",
    ),
    &["@file", "scene meshes"],
);

/// What the mesh read was to write breaks the layout's rules.
pub const MESH_INVALID: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E022",
        "blend_mesh_invalid",
        USAGE,
        "a value the mesh read was to write breaks the layout's rules",
        "The mesh read checks every value before it writes, and one isn't finite or points past \
         its mesh's vertices. Nothing was drawn. This is a fault of the pack: report it to the \
         pack's maintainers, with the file's format version and the verb.",
    ),
    &["@file", "scene meshes"],
);

/// The keys a host gave for a version's objects (a carrier's pairing) don't fit its decode.
pub const MESH_PAIRING: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E023",
        "blend_mesh_pairing",
        USAGE,
        "the keys given for the version don't fit its objects",
        "The mesh read names each object by the key the host gave for it, and the keys given \
         don't fit the version's objects (their count differs), so none is guessed. Nothing was \
         drawn. Give the pairing kept with this version.",
    ),
    &["@file", "scene meshes"],
);

/// A live doc of a file of a release the live tier reads but the writer doesn't write: kept
/// for a release read before it is written. The writer writes every release the live tier
/// reads today ([`crate::write::WRITE_TO`] is [`crate::scene::READ_TO`]), so no file reaches
/// it; `{written}` in its message is the releases written ([`written_releases`]).
pub const READ_ONLY: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E024",
        "live_read_only",
        USAGE,
        "Nothing was applied: this pack reads files of this release of the program that wrote \
         them (format {format}) live, but doesn't write them back yet (it writes {written}).",
        "The scene opens live and reads as its file holds it; a change is written back once \
         this pack has checked the release's saves. Read it; to change it here, save a copy with \
         a release this pack writes, if you have one.",
    )
    .step(
        "scene list",
        &[],
        "Read the scene's collections and objects, as the file holds them.",
    ),
    LIVE_EDITS,
);

// ---------- the kit's edit check ----------

/// The verbs a live change comes from: an edit's (a person's, a host's), a pack verb's, and
/// the history's (undo, redo, revert, restore).
const CHANGES: &[&str] = &["@change", "@history", "(host)"];

/// A live change the live rules refuse with no code of their own ([`super::check::Rule::code`]
/// is `None`): an element of the lists added, removed, moved or retyped; a collection's field;
/// an object field live v1 doesn't write; a value its field can't hold; a name without its
/// `nid`; an object with no file key; a root the live schema doesn't have. The kit's edit check
/// gives it as `details.code` of its `pack_refused`.
pub const LIVE_CHANGE: ErrorEntry = with_verbs(
    ErrorEntry::pack(
        "BLEND-E025",
        "blend_live_change",
        USAGE,
        "the live scene's write-back can't write this change",
        "Live, a scene is written back by patching only an object's transform, its three \
         visibility bits and its name into the file: no collection or object is added, removed, \
         moved or retyped, and no collection's field or other object field changes. Nothing was \
         applied. Change the objects through the live verbs (`object place`, `object \
         visibility`, `object rename`), or anything else in the app.",
    )
    .step(
        "scene list",
        &[],
        "Read the scene's collections and objects, as the file holds them.",
    ),
    CHANGES,
);

/// The pack's own refusals, in id order.
pub const ERRORS: &[ErrorEntry] = &[
    NOT_EDITABLE,
    DRIVEN,
    ROTATION_MODE,
    NAME,
    BREAKS_LINKS,
    NOT_FOUND_ENTRY,
    LIVE_VERSION,
    LIVE_AMBIGUOUS,
    LIVE_VALUE,
    NOT_BLEND,
    UNKNOWN_FORM,
    FILE_NOT_BLEND,
    FILE_UNKNOWN_FORM,
    FILE_UNREADABLE,
    FILE_TOO_LARGE,
    FILE_VERSION,
    FILE_AMBIGUOUS,
    FILE_NOT_AN_OBJECT,
    FILE_VALUE,
    FILE_WRITE_CHECK,
    MESH_DECLINED,
    MESH_INVALID,
    MESH_PAIRING,
    READ_ONLY,
    LIVE_CHANGE,
];

/// The entry whose code is `code`, among [`ERRORS`]: a verb's code, or a decline's
/// (`live_version`).
pub fn by_code(code: &str) -> Option<&'static ErrorEntry> {
    ERRORS.iter().find(|e| e.name == code)
}

/// A file version as people read a release: `501` is `5.01`.
pub fn release(version: u16) -> String {
    format!("{}.{:02}", version / 100, version % 100)
}

/// The releases a file is read from, as the texts name them: `3.06 to 5.03`.
pub fn checked_releases() -> String {
    format!(
        "{} to {}",
        release(crate::scene::READ_FROM),
        release(crate::scene::READ_TO)
    )
}

/// The releases a file is written to, as the texts name them: `3.06 to 5.03`.
pub fn written_releases() -> String {
    format!(
        "{} to {}",
        release(crate::write::WRITE_FROM),
        release(crate::write::written_to())
    )
}

/// [`LIVE_VERSION`]'s message for a file of format `format` (`5.04`).
pub fn live_version_message(format: &str) -> String {
    format!(
        "Nothing was read: this file was saved by a newer (or older) release of the program \
         that wrote it (format {format}) than this pack has checked ({}).",
        checked_releases()
    )
}

/// [`READ_ONLY`]'s message for a file of format `format` (`5.04`).
pub fn read_only_message(format: &str) -> String {
    format!(
        "Nothing was applied: this pack reads files of this release of the program that wrote \
         them (format {format}) live, but doesn't write them back yet (it writes {}).",
        written_releases()
    )
}

/// [`FILE_VERSION`]'s message, for a file of format `format` when it is known: a writer's
/// refusal names the releases written (`written`), a read's the releases read.
pub fn file_version_message(format: Option<&str>, written: bool) -> String {
    let of = format.map_or_else(String::new, |f| format!(" (format {f})"));
    let range = if written {
        written_releases()
    } else {
        checked_releases()
    };
    format!(
        "Nothing was applied: this file was saved by a newer (or older) release of the program \
         that wrote it{of} than this pack has checked ({range}).",
    )
}
