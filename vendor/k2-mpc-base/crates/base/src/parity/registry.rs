//! The verb registry (K-PAR-2): every verb an agent can run, what it serves and needs.

use std::fmt;

use super::action::PersonAction;
use super::positions::{PositionForm, Unit};
use crate::session::{Need, Pack};
use crate::tools::exit::{NO, NOT_FOUND, OK, TOO_OLD};

/// Which kind of doc a verb (or a person action) applies to. Hosts classify verbs from
/// this data ([`VerbSpec::tiers`], [`PersonAction::tiers`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Tier {
    /// A text doc: a text pack's [`crate::session::Session`] (merged live, tier 1).
    Text,
    /// Any file: a [`crate::session::BlobSession`] (tier 0, and tier 0.5 with a merging
    /// [`crate::session::BlobPack`]).
    Blob,
    /// A grid doc: a [`crate::table`] pack's [`crate::session::Session`] (structured, merged
    /// by key; CSV contract §3.5).
    Grid,
}

impl Tier {
    /// Every tier.
    pub const ALL: &'static [Tier] = &[Tier::Text, Tier::Blob, Tier::Grid];
}

/// What a verb does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerbKind {
    /// Reads the doc or presence (`cat`, `who`, `status`).
    Read,
    /// Changes the doc, presence or its records (`insert`, `cursor`, `undo`, `approve`).
    Edit,
    /// Reads history (`log`, `diff --version`, `show --version`, `blame`).
    History,
}

/// A session call a verb is bound to (the "Session API" column of `docs/PARITY.md`). On a
/// blob doc the same name is the [`crate::session::BlobSession`] call of that name (`Show`
/// is `read_version`, `WriteBase` is `write`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Api {
    /// `Session::join` (`Stay::Held`).
    Join,
    /// `Session::join_shared` (one-shot verbs).
    JoinShared,
    Leave,
    Read,
    /// `Session::run_tool` (kit tools and packs' verbs).
    RunTool,
    /// `Session::edit_with` (what `run_tool` runs edit verbs through).
    EditWith,
    Roster,
    SetCursor,
    SetView,
    SetSelection,
    SetPointer,
    SetViewport,
    WriteBase,
    ReplaceText,
    UndoWith,
    RedoWith,
    Status,
    Flush,
    Subscribe,
    Versions,
    Log,
    DiffVersion,
    Show,
    Restore,
    NameVersion,
    /// `SessionManager::rekey`.
    Rekey,
    FileMoved,
    SetApproval,
    Revert,
    /// `Session::blame`.
    Blame,
    /// Tier 0: `BlobSession::acquire_lease`.
    AcquireLease,
    /// Tier 0: `BlobSession::renew_lease`.
    RenewLease,
    /// Tier 0: `BlobSession::release_lease`.
    ReleaseLease,
    /// Tier 0: `BlobSession::break_lease`.
    BreakLease,
    /// `Session::claim` (amendment 13).
    Claim,
    /// `Session::renew_claim`.
    RenewClaim,
    /// `Session::release_claim`.
    ReleaseClaim,
    /// `Session::activity`.
    Activity,
    /// `Session::edit_ops_with` (what `run_tool` runs a pack's structured edit verbs
    /// through, A9).
    EditOps,
    /// `Session::drop_deleted` (`rows drop-deleted`, K-CSV-7).
    DropDeleted,
    /// `Session::blame_cell` (`blame --cell R,C`).
    BlameCell,
    /// `Session::blame_structure` (`blame --structure`).
    BlameStructure,
    /// `Session::file_now` (tier 0: `BlobSession::file_now_to`), then the export registry.
    FileNow,
    /// `Session::file_at` (tier 0: `BlobSession::file_at_to`), then the export registry.
    FileAt,
    /// `BlobSession::comment` (annotations, [`ANNOTATION_VERBS`]).
    Comment,
    /// `BlobSession::reply`.
    Reply,
    /// `BlobSession::resolve`.
    Resolve,
    /// `BlobSession::reopen`.
    Reopen,
    /// `BlobSession::delete_comment`.
    DeleteComment,
    /// `BlobSession::comments`.
    Comments,
    /// `BlobSession::annotation_log`.
    AnnotationLog,
    /// `BlobSession::reanchor`.
    Reanchor,
    /// Tier 0, a structure index: `BlobSession::index_list` (`Registry::for_blob_pack`).
    IndexList,
    /// Tier 0, a structure index: `BlobSession::index_nodes`.
    IndexNodes,
    /// Tier 0, a structure index: `BlobSession::index_diff`.
    IndexDiff,
    /// Tier 0, a structure index: `BlobSession::index_blame`.
    IndexBlame,
    /// Tier 0, a structure index: `BlobSession::claim_nodes`.
    ClaimNodes,
}

/// Where a verb comes from.
///
/// `non_exhaustive`: another crate's `match` on it needs a wildcard arm, so a later source is
/// not a breaking change; `==` reads it as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// The kit's verbs (every text pack has them).
    Kit,
    /// A pack's own verb ([`crate::session::Pack::verbs`]).
    Pack,
    /// A file type's own verb, from data its host fills in ([`FileVerb`],
    /// [`Registry::for_file`]): the host runs it on the file itself, not through a kit
    /// session, so it makes no session call and is in no kit group (`@change`, `@pack`...).
    File,
}

/// One verb: its name (the CLI form, e.g. `undo --partial`), kind, rights, the session
/// calls it makes, the person actions it serves, its flags and its exit codes.
///
/// `non_exhaustive`: the kit builds these; other crates read the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerbSpec {
    /// The form an agent types after `<host> doc`: a verb, or a verb and the flag that makes
    /// it a different row (`undo --partial`, `log --pending`, `sed -i`). See [`verb_form`].
    pub name: &'static str,
    pub kind: VerbKind,
    /// The rights it needs (asked of the host on every call).
    pub needs: &'static [Need],
    /// The session calls it makes.
    pub api: &'static [Api],
    /// The person actions it serves (at least one).
    pub serves: &'static [PersonAction],
    /// Its flags, one line.
    pub usage: &'static str,
    /// Every exit code it can end with by itself; the session's refusals (exit 2, 3, 4, 5,
    /// 6, 8 for a revert too old...) come on top, mapped by `tools::session_exit`.
    pub exits: &'static [u8],
    pub source: Source,
    /// The kinds of doc it works on: text docs, blob docs (tier 0), or both. A pack's own
    /// verbs are text verbs.
    pub tiers: &'static [Tier],
    /// The position forms a pack verb declares ([`crate::tools::PackVerb::positions`]); `None`
    /// for a kit verb and a pack verb that declares nothing. Read through
    /// [`super::positions::stated`], which also knows the kit's table.
    pub positions: Option<&'static [super::positions::PositionForm]>,
    /// The host's own parity rows a file verb serves, by the host's names ([`FileVerb::serves`]);
    /// empty for a kit or pack verb, whose rows are [`VerbSpec::serves`].
    pub serves_named: &'static [&'static str],
    /// The verb this one is another name for, when that verb's name is a pack's
    /// ([`Registry::for_blob_pack`]'s `outline` and `node`); [`ALIASES`] holds the kit's own.
    alias: Option<&'static str>,
}

impl VerbSpec {
    /// Whether it works on `tier`'s docs.
    pub fn applies_to(&self, tier: Tier) -> bool {
        self.tiers.contains(&tier)
    }

    const fn on(self, tiers: &'static [Tier]) -> Self {
        VerbSpec { tiers, ..self }
    }

    /// Whether it runs through `run_tool`: on a text doc
    /// [`crate::session::Session::run_tool`], the kit's verbs in [`RUN_TOOL_VERBS`] and every
    /// pack verb; on a blob doc [`crate::session::BlobSession::run_tool`], the structure
    /// index's verbs and node claims ([`Registry::for_blob_pack`]; `cat` there is
    /// `BlobSession::read`). A host routes these to `run_tool` and the others to their own
    /// session calls ([`VerbSpec::api`]).
    pub fn runs_through_run_tool(&self) -> bool {
        match self.source {
            Source::Pack => true,
            Source::Kit => {
                RUN_TOOL_VERBS.contains(&self.name)
                    || ((self.is_index() || self.is_annotation())
                        && self.api.contains(&Api::RunTool))
            }
            Source::File => false,
        }
    }

    /// Whether it is one of the annotation verbs ([`ANNOTATION_VERBS`]).
    pub fn is_annotation(&self) -> bool {
        self.source == Source::Kit && ANNOTATION_VERBS.iter().any(|v| v.name == self.name)
    }

    /// Whether [`Registry::for_blob_pack`] added it for a structure index (its verbs, their
    /// other names, node claims): the error registry's `@index` group.
    pub fn is_index(&self) -> bool {
        self.source == Source::Kit
            && self.tiers == BLOB
            && self.api.iter().any(|a| {
                matches!(
                    a,
                    Api::IndexList
                        | Api::IndexNodes
                        | Api::IndexDiff
                        | Api::IndexBlame
                        | Api::ClaimNodes
                        | Api::RenewClaim
                        | Api::ReleaseClaim
                )
            })
    }

    /// The verb this one is another name for ([`ALIASES`]), if any: the same row, the same
    /// binding, under a host-neutral name.
    pub fn alias_of(&self) -> Option<&'static str> {
        if self.source == Source::File {
            return None;
        }
        if self.alias.is_some() {
            return self.alias;
        }
        ALIASES
            .iter()
            .find(|(a, _)| *a == self.name)
            .map(|(_, of)| *of)
    }
}

/// The words [`Registry::for_blob_pack`] gives a structure index besides the pack's names.
const INDEX_KIT_WORDS: &[&str] = &["outline", "node", "claim"];

const LIST_USAGE: &str = "[--version V] [--depth N] [--kind K] [--tree] [--cursor C] \
     [--no-truncate] [--json] (the outline, each node's #N; another version: +history)";
const SHOW_USAGE: &str = "ADDR... [--version V] [--depth N] [--cursor C] [--no-truncate] \
     [--json] | ADDR --text (ADDR: @k:KEY, a native ID, #N, name:NAME; any missing: exit 4, \
     every address listed; --text with no text: 1)";
const SHOW_EXITS: &[u8] = &[OK, NO, NOT_FOUND];
const LIST_POSITIONS: &[PositionForm] = &[PositionForm::new("stdout", Unit::PackNth)];
const SHOW_POSITIONS: &[PositionForm] = &[
    PositionForm::new("ADDR", Unit::PackNth),
    PositionForm::new("stdout", Unit::PackNth),
];
const EXPORT_POSITIONS: &[PositionForm] = &[PositionForm::new("ADDR", Unit::PackNth)];
const CLAIM_POSITIONS: &[PositionForm] = &[PositionForm::new("--node", Unit::PackNth)];

/// A structure index's verb ([`Registry::for_blob_pack`]).
#[allow(clippy::too_many_arguments)]
const fn index_verb(
    name: &'static str,
    kind: VerbKind,
    needs: &'static [Need],
    api: &'static [Api],
    serves: &'static [PersonAction],
    usage: &'static str,
    exits: &'static [u8],
    positions: Option<&'static [PositionForm]>,
) -> VerbSpec {
    VerbSpec {
        name,
        kind,
        needs,
        api,
        serves,
        usage,
        exits,
        source: Source::Kit,
        tiers: BLOB,
        positions,
        serves_named: &[],
        alias: None,
    }
}

/// The kit's verbs that run through [`crate::session::Session::run_tool`] (a flag per verb,
/// as data; [`VerbSpec::runs_through_run_tool`]). Kept consistent with `run_tool` itself
/// by tests: each one runs there, and every other registered verb is refused there.
pub const RUN_TOOL_VERBS: &[&str] = &[
    "cat", "wc", "head", "tail", "sed", "view", "grep", "insert", "replace", "delete", "sed -i",
    "patch", "apply", "append", "diff",
];

/// Verbs that are another name for a verb (alias, verb): the same rights, calls, actions,
/// flags, exits, tiers and binding. A pack's parity list may list an alias or leave it out
/// (an alias is never required); the kit's cases cover both names.
pub const ALIASES: &[(&str, &str)] = &[("approvals", "log --pending")];

use Api as A;
use PersonAction as P;
use VerbKind::{Edit as E, History as H, Read as R};

const NR: &[Need] = &[Need::Read];
const NE: &[Need] = &[Need::Read, Need::Edit];
const NH: &[Need] = &[Need::Read, Need::History];
const NO_: &[Need] = &[Need::Read, Need::Owner];
const OK0: &[u8] = &[OK];
const OK01: &[u8] = &[OK, NO];
const TEXT: &[Tier] = &[Tier::Text];
const BLOB: &[Tier] = &[Tier::Blob];
const GRID: &[Tier] = &[Tier::Grid];
/// Text and grid docs: a grid's reads see its rendered file (CSV contract §3.2).
const TG: &[Tier] = &[Tier::Text, Tier::Grid];
/// Every tier.
const ALL3: &[Tier] = &[Tier::Text, Tier::Blob, Tier::Grid];

const fn kit(
    name: &'static str,
    kind: VerbKind,
    needs: &'static [Need],
    api: &'static [Api],
    serves: &'static [PersonAction],
    usage: &'static str,
    exits: &'static [u8],
) -> VerbSpec {
    VerbSpec {
        name,
        kind,
        needs,
        api,
        serves,
        usage,
        exits,
        source: Source::Kit,
        tiers: TEXT,
        positions: None,
        serves_named: &[],
        alias: None,
    }
}

const TOOL: &[Api] = &[A::RunTool];
const TOOL_EDIT: &[Api] = &[A::RunTool, A::EditWith];

/// The kit's verbs: every text pack's, and tier 0's (each verb's [`VerbSpec::tiers`]).
pub const KIT_VERBS: &[VerbSpec] = &[
    kit(
        "open",
        R,
        NR,
        &[A::Join],
        &[P::Open],
        "open (stays until leave)",
        OK0,
    )
    .on(ALL3),
    kit("leave", R, NR, &[A::Leave], &[P::Close], "leave", OK0).on(ALL3),
    // on a blob doc, `cat` is the file's bytes (`BlobSession::read`)
    kit(
        "cat",
        R,
        NR,
        &[A::RunTool, A::Read],
        &[P::ReadAll],
        "cat [-n] [-b] [--for-edit [--ttl MS]] (tier 0: the file's bytes, no flags)",
        OK0,
    )
    .on(ALL3),
    kit(
        "wc",
        R,
        NR,
        TOOL,
        &[P::ReadAll],
        "wc [-l] [-w] [-c] [-m]",
        OK0,
    )
    .on(TG),
    kit(
        "head",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "head [-n N] [-c N] [--for-edit [--ttl MS]]",
        OK0,
    )
    .on(TG),
    kit(
        "tail",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "tail [-n [+]N] [-c [+]N] [--for-edit [--ttl MS]]",
        OK0,
    )
    .on(TG),
    kit(
        "sed",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "sed -n 'a,bp' (any script without -i) [--for-edit [--ttl MS]]",
        OK0,
    )
    .on(TG),
    kit(
        "view",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "view --lines a,b [--for-edit [--ttl MS]]",
        OK0,
    )
    .on(TG),
    kit(
        "grep",
        R,
        NR,
        TOOL,
        &[P::Find],
        "grep [-EFGivwxcoqlLHhn] [-m N] [-A/-B/-C N] [--for-edit [--ttl MS]] PATTERN",
        OK01,
    )
    .on(TG),
    kit("who", R, NR, &[A::Roster], &[P::Who], "who", OK0).on(ALL3),
    // agent presence (amendment 13): where others are working, and saying where I am
    kit(
        "activity",
        R,
        NR,
        &[A::Activity],
        &[P::Who],
        "activity [-n N] (who did what, where, lately)",
        OK0,
    )
    .on(TG),
    kit(
        "claim",
        E,
        NE,
        &[A::Claim],
        &[P::Caret],
        "claim --lines a,b | (grid) --rows R1..R2 | --cols C1..C2 | --cells R1,C1..R2,C2 \
         [--ttl MS] [--note TEXT] [--hold] (someone else's claim there: exit 5)",
        OK0,
    )
    .on(TG),
    kit(
        "claim --renew",
        E,
        NE,
        &[A::RenewClaim],
        &[P::Caret],
        "claim --renew [--id ID] [--ttl MS] [--hold | --no-hold] (others waiting: exit 2; none: 4)",
        OK0,
    )
    .on(TG),
    kit(
        "claim --release",
        E,
        NR,
        &[A::ReleaseClaim],
        &[P::Caret],
        "claim --release [--id ID] (all of mine without an id; 1: none held)",
        OK01,
    )
    .on(TG),
    kit(
        "follow",
        R,
        NR,
        &[A::Roster],
        &[P::Follow],
        "follow <pid> (1: not here or no caret)",
        OK01,
    )
    .on(TG),
    kit(
        "cursor",
        E,
        NE,
        &[A::SetCursor, A::SetView],
        &[P::Caret],
        "cursor --at L:C | --select L:C-L:C | (grid) --cell R,C [--to R,C] \
         (read only: a viewing range)",
        OK0,
    )
    .on(TG),
    kit(
        "select",
        R,
        NR,
        &[A::SetSelection],
        &[P::Caret],
        "select --root ROOT --keys K1,K2 | --clear (at most 256; unknown or deleted: exit 2)",
        OK0,
    )
    .on(GRID),
    kit(
        "pointer",
        R,
        NR,
        &[A::SetPointer],
        &[P::Caret],
        "pointer --at X,Y | --clear (the doc's own coordinates)",
        OK0,
    )
    .on(GRID),
    kit(
        "viewport",
        R,
        NR,
        &[A::SetViewport],
        &[P::ViewRange],
        "viewport --rect X,Y,W,H | --clear (the doc's own coordinates)",
        OK0,
    )
    .on(GRID),
    kit(
        "insert",
        E,
        NE,
        TOOL_EDIT,
        &[P::Type, P::Format],
        "insert --line N | --after N | --at L:C [--if-version V] (text on stdin)",
        OK0,
    ),
    kit(
        "replace",
        E,
        NE,
        TOOL_EDIT,
        &[P::TypeOver, P::FindReplace, P::Format],
        "replace --lines a,b | --range L:C-L:C (text on stdin) | --old TEXT --new TEXT [--all] \
         (exact; '-': that one from stdin; 1: no match or not unique, nothing applied) \
         [--if-version V]",
        OK01,
    ),
    kit(
        "delete",
        E,
        NE,
        TOOL_EDIT,
        &[P::Delete],
        "delete --lines a,b | --range L:C-L:C [--if-version V]",
        OK0,
    ),
    kit(
        "sed -i",
        E,
        NE,
        TOOL_EDIT,
        &[P::Delete, P::FindReplace, P::Format],
        "sed -i SCRIPT (s///, d, a, i, c, y...) [--if-version V]",
        OK0,
    ),
    kit(
        "patch",
        E,
        NE,
        TOOL_EDIT,
        &[P::Paste],
        "patch [-p N] [-R] [--dry-run] [-F N] [--partial] [--if-version V] (diff on stdin)",
        OK01,
    ),
    kit(
        "apply",
        E,
        NE,
        TOOL_EDIT,
        &[P::Paste],
        "apply: as patch",
        OK01,
    ),
    kit(
        "append",
        E,
        NE,
        TOOL_EDIT,
        &[P::Paste],
        "append [--ensure-newline] [--if-version V] (text on stdin)",
        OK0,
    ),
    kit(
        "write",
        E,
        NE,
        &[A::WriteBase, A::ReplaceText],
        &[P::Rewrite],
        "write --base V | --base live (new text on stdin; a grid: --base V [--partial] [--respect-claims], merged by key)",
        &[OK, NO, TOO_OLD],
    )
    .on(ALL3),
    kit(
        "undo",
        E,
        NE,
        &[A::UndoWith],
        &[P::Undo],
        "undo [--steps N] [--for P]",
        OK01,
    )
    .on(TG),
    kit(
        "redo",
        E,
        NE,
        &[A::RedoWith],
        &[P::Undo],
        "redo [--steps N]",
        OK01,
    )
    .on(TG),
    kit(
        "undo --drop",
        E,
        NE,
        &[A::UndoWith],
        &[P::Undo],
        "undo --drop",
        OK0,
    )
    .on(TG),
    kit(
        "undo --partial",
        E,
        NE,
        &[A::UndoWith],
        &[P::UndoPartial],
        "undo --partial [--steps N] [--for P]",
        OK01,
    )
    .on(TG),
    kit(
        "status",
        R,
        NR,
        &[A::Status],
        &[P::Saved, P::WhyOff],
        "status",
        OK0,
    )
    .on(ALL3),
    kit("flush", R, NR, &[A::Flush], &[P::Saved], "flush", OK0).on(ALL3),
    kit(
        "watch",
        R,
        NR,
        &[A::Join, A::Subscribe],
        &[P::Watch],
        "watch",
        OK0,
    )
    .on(TG),
    kit(
        "tail -f",
        R,
        NR,
        &[A::Join, A::Subscribe],
        &[P::Watch],
        "tail -f (the same as watch)",
        OK0,
    )
    .on(TG),
    kit(
        "log",
        H,
        NH,
        &[A::Log],
        &[P::History],
        "log [--by P] [-n N]",
        OK0,
    )
    .on(ALL3),
    kit(
        "diff --version",
        H,
        NH,
        &[A::DiffVersion],
        &[P::History],
        "diff --version V [--to W] [-U N]",
        OK01,
    )
    .on(TG),
    kit(
        "show --version",
        H,
        NH,
        &[A::Show],
        &[P::History],
        "show --version V",
        OK0,
    )
    .on(ALL3),
    // P24: the file's bytes through the export registry (the kit's target is `as-is`; a
    // host registers format exporters beside it)
    kit(
        "export",
        R,
        NR,
        &[A::FileNow],
        &[P::Export],
        "export [--to as-is] (the file now, through the export registry)",
        OK0,
    )
    .on(ALL3),
    kit(
        "export --version",
        H,
        NH,
        &[A::FileAt],
        &[P::Export],
        "export --version V [--to as-is] (the file at version V)",
        OK0,
    )
    .on(ALL3),
    kit(
        "diff",
        R,
        NR,
        TOOL,
        &[P::History],
        "diff [-u] [-U N] (my copy on stdin vs live)",
        OK01,
    )
    .on(TG),
    kit(
        "blame",
        H,
        NH,
        &[A::Blame],
        &[P::Blame],
        "blame [-L a,b] (a grid: rendered lines, each to its row's latest cell write)",
        OK0,
    )
    .on(ALL3),
    kit(
        "restore",
        E,
        &[Need::Read, Need::Edit, Need::History],
        &[A::Restore],
        &[P::Restore],
        "restore --version V",
        OK0,
    )
    .on(ALL3),
    kit(
        "version --name",
        E,
        NE,
        &[A::NameVersion],
        &[P::Restore],
        "version --name NAME [--version V]",
        OK0,
    )
    .on(ALL3),
    kit(
        "mv",
        E,
        NE,
        &[A::Rekey, A::FileMoved],
        &[P::Rename],
        "mv <new path>",
        OK0,
    )
    .on(ALL3),
    kit(
        "approve",
        E,
        &[Need::Read, Need::Approver],
        &[A::SetApproval],
        &[P::Approve],
        "approve <change> (a blob doc: <version>)",
        OK0,
    )
    .on(ALL3),
    kit(
        "decline",
        E,
        &[Need::Read, Need::Approver],
        &[A::SetApproval],
        &[P::Approve],
        "decline <change> (a blob doc: <version>; then restore --version to reverse it)",
        OK0,
    )
    .on(ALL3),
    kit(
        "log --pending",
        H,
        NH,
        &[A::Log],
        &[P::Pending],
        "log --pending [--all] [--on-behalf-of me|P] (--all: settled ones too, newest first)",
        OK0,
    )
    .on(ALL3),
    // the same list under a host-neutral name: what `log --pending` does, flag for flag
    kit(
        "approvals",
        H,
        NH,
        &[A::Log],
        &[P::Pending],
        "approvals [--all] [--on-behalf-of me|P] (the same as log --pending)",
        OK0,
    )
    .on(ALL3),
    kit(
        "revert",
        E,
        &[Need::Read, Need::Edit, Need::AuthorOrOwner],
        &[A::Revert],
        &[P::Revert],
        "revert <change> [--partial] (too old: exit 8)",
        OK01,
    )
    .on(TG),
    // grids (CSV contract §3.5, §5.5; K-CSV-7)
    kit(
        "rows drop-deleted",
        E,
        NO_,
        &[A::DropDeleted],
        &[P::DropDeleted],
        "rows drop-deleted (owner only: frees the size limit; history keeps the rows)",
        OK0,
    )
    .on(GRID),
    kit(
        "blame --cell",
        H,
        NH,
        &[A::BlameCell],
        &[P::Blame],
        "blame --cell R,C (the cell's current write; none recorded: exit 2, bad_position, as \
         blame -L past the end)",
        OK0,
    )
    .on(GRID),
    kit(
        "blame --structure",
        H,
        NH,
        &[A::BlameStructure],
        &[P::Blame],
        "blame --structure (per row: who inserted it, who last moved it)",
        OK0,
    )
    .on(GRID),
    // tier 0: the lease (K-T0-2)
    kit(
        "lease",
        E,
        NE,
        &[A::AcquireLease],
        &[P::Lease],
        "lease [--ttl MS] (take it, or extend yours; someone else's: exit 5)",
        OK0,
    )
    .on(BLOB),
    kit(
        "lease --renew",
        E,
        NE,
        &[A::RenewLease],
        &[P::Lease],
        "lease --renew [--ttl MS] (nobody holds it: exit 2)",
        OK0,
    )
    .on(BLOB),
    kit(
        "lease --release",
        E,
        NR,
        &[A::ReleaseLease],
        &[P::ReleaseLease],
        "lease --release (1: you didn't hold it)",
        OK01,
    )
    .on(BLOB),
    kit(
        "lease --break",
        E,
        NO_,
        &[A::BreakLease],
        &[P::BreakLease],
        "lease --break (owner only; 1: nobody held it)",
        OK01,
    )
    .on(BLOB),
];

/// The verb form an `argv` names: `argv[0]`, or `argv[0]` plus the flag that makes it a
/// separate registry entry (`sed -i`, `undo --partial`, `undo --drop`, `log --pending`,
/// `diff --version`, `show --version`, `version --name`, `tail -f`, `lease --renew`,
/// `lease --release`, `lease --break`, `blame --cell`, `blame --structure`, `comment --delete`,
/// `resolve --reopen`, `comments --log`), or its two
/// words (`rows drop-deleted`). A host names verbs the same way when it checks its command
/// table against the registry; a pack's two-word structured verb (`row insert`) is named by
/// [`Registry::form_of`].
pub fn verb_form(argv: &[String]) -> String {
    let Some((verb, args)) = argv.split_first() else {
        return String::new();
    };
    let has = |f: &str| {
        args.iter()
            .any(|a| a == f || a.starts_with(&format!("{f}=")))
    };
    let form = match verb.as_str() {
        "sed" => match crate::tools::Tool::parse(argv) {
            Ok(t) if t.kind() == crate::tools::Kind::Edit => "sed -i",
            Ok(_) => "sed",
            // unparseable: name it by its flag, so the registry still decides
            Err(_)
                if args
                    .iter()
                    .any(|a| a.starts_with("-i") || a == "--in-place") =>
            {
                "sed -i"
            }
            Err(_) => "sed",
        },
        "undo" if has("--partial") => "undo --partial",
        "undo" if has("--drop") => "undo --drop",
        "log" if has("--pending") => "log --pending",
        "diff" if has("--version") => "diff --version",
        "show" => "show --version",
        "export" if has("--version") => "export --version",
        "version" => "version --name",
        "tail" if has("-f") || has("--follow") => "tail -f",
        "lease" if has("--renew") => "lease --renew",
        "claim" if has("--renew") => "claim --renew",
        "claim" if has("--release") => "claim --release",
        "lease" if has("--release") => "lease --release",
        "lease" if has("--break") => "lease --break",
        "blame" if has("--cell") => "blame --cell",
        "blame" if has("--structure") => "blame --structure",
        "rows" if args.first().is_some_and(|a| a == "drop-deleted") => "rows drop-deleted",
        "comment" if has("--delete") => "comment --delete",
        "resolve" if has("--reopen") => "resolve --reopen",
        "comments" if has("--log") => "comments --log",
        v => v,
    };
    form.to_owned()
}

const NC: &[Need] = &[Need::Read, Need::Comment];

/// An annotation verb: no tier of its own ([`Registry::with_annotations`] adds them).
const fn annot(
    name: &'static str,
    kind: VerbKind,
    needs: &'static [Need],
    api: &'static [Api],
    serves: &'static [PersonAction],
    usage: &'static str,
) -> VerbSpec {
    VerbSpec {
        tiers: &[],
        ..kit(name, kind, needs, api, serves, usage, OK0)
    }
}

/// **The annotation verbs** (`docs/SESSIONS.md`, "Annotations"): comment threads, run by
/// [`crate::session::BlobSession::run_annotation`] over the typed calls (and by
/// [`crate::session::BlobSession::run_tool`], which hands them there). They carry no tier, so
/// [`Registry::for_tier`] doesn't list them and a host that hasn't adopted comments sees no new
/// verb; [`Registry::for_blob_pack`] lists them, and [`Registry::with_annotations`] adds them
/// to any other. Their parity is
/// [`super::check_annotations`]'s (P23 `Annotate` is a text and grid row; on blob docs these
/// are its verbs). The rights: read for the reads, history for the log, the comment right
/// (implied by edit) for the rest, and `comment --delete` also its author, the person it was
/// for, or an owner. An agent's `reanchor` is a proposal a person confirms (K-ANN-9).
pub const ANNOTATION_VERBS: &[VerbSpec] = &[
    annot(
        "comment",
        E,
        NC,
        &[A::RunTool, A::Comment],
        &[P::Annotate, P::Mention],
        "comment (--doc | --node KEY [--version V] [--hint H]) --body TEXT [--mention PID]... [--proposal PATH]...",
    ),
    annot(
        "reply",
        E,
        NC,
        &[A::RunTool, A::Reply],
        &[P::Annotate, P::Mention],
        "reply ann-N --body TEXT [--mention PID]... [--proposal PATH]...",
    ),
    annot(
        "resolve",
        E,
        NC,
        &[A::RunTool, A::Resolve],
        &[P::Annotate],
        "resolve ann-N",
    ),
    annot(
        "resolve --reopen",
        E,
        NC,
        &[A::RunTool, A::Reopen],
        &[P::Annotate],
        "resolve ann-N --reopen",
    ),
    annot(
        "comment --delete",
        E,
        NC,
        &[A::RunTool, A::DeleteComment],
        &[P::Annotate],
        "comment --delete ann-N [--n N] (its author, the person it was for, or an owner)",
    ),
    annot(
        "comments",
        R,
        NR,
        &[A::RunTool, A::Comments],
        &[P::Annotate],
        "comments [--open | --resolved] [--version V] [--on KEY] [--since SEQ] [--after ann-N] [--limit N]",
    ),
    annot(
        "comments --log",
        H,
        NH,
        &[A::RunTool, A::AnnotationLog],
        &[P::Annotate],
        "comments --log [ann-N] [--since SEQ] [--limit N]",
    ),
    annot(
        "reanchor",
        E,
        NC,
        &[A::RunTool, A::Reanchor],
        &[P::Annotate],
        "reanchor ann-N (--doc | --node KEY [--version V] [--hint H]) (an agent's is a proposal)",
    ),
];

/// Why a registry can't be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError(pub String);

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "verb registry: {}", self.0)
    }
}

impl std::error::Error for RegistryError {}

/// **A file type's own verb, as data its host fills in** ([`Registry::for_file`]): for a file
/// the kit doesn't hold as a doc, whose verbs a format crate defines and the host runs on the
/// file itself. `capabilities` then lists them as it lists the kit's, and the error rows that
/// name them (`@file`, then the verbs: [`crate::errors::ErrorEntry::verbs`]) as the ids they
/// can return.
///
/// `non_exhaustive`: build one with [`FileVerb::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileVerb {
    /// Its name as the host's command line spells it after the file type: one or two words of
    /// lowercase letters, digits and `-`. It may share a kit verb's name: it is listed
    /// apart, as a file verb.
    pub name: &'static str,
    /// Whether it reads the file or changes it ([`VerbKind::Read`], [`VerbKind::Edit`]); the
    /// rights it needs follow from it.
    pub kind: VerbKind,
    /// Its arguments and flags, one line.
    pub usage: &'static str,
    /// Every exit code it can end with (the kit's table, [`crate::tools::exit`]).
    pub exits: &'static [u8],
    /// The host's own parity rows it serves, by the host's names (at least one). They aren't
    /// the kit's [`PersonAction`]s: a file type's person actions are its own list.
    pub serves: &'static [&'static str],
}

impl FileVerb {
    pub const fn new(
        name: &'static str,
        kind: VerbKind,
        usage: &'static str,
        exits: &'static [u8],
        serves: &'static [&'static str],
    ) -> Self {
        FileVerb {
            name,
            kind,
            usage,
            exits,
            serves,
        }
    }
}

/// The verbs of one tier: the kit's ([`KIT_VERBS`] that apply to it), plus a text pack's
/// own; or a file type's own verbs ([`Registry::for_file`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    verbs: Vec<VerbSpec>,
    tier: Tier,
    /// Built by [`Registry::for_file`]: the kit's parity harnesses refuse it.
    file: bool,
}

impl Registry {
    /// The kit's text verbs ([`Registry::for_tier`]`(Tier::Text)`).
    pub fn kit() -> Self {
        Self::for_tier(Tier::Text)
    }

    /// The kit's verbs that work on `tier`'s docs. A host serving tier 0 (any file) checks
    /// its command table against `for_tier(Tier::Blob)`.
    pub fn for_tier(tier: Tier) -> Self {
        Registry {
            verbs: KIT_VERBS
                .iter()
                .filter(|v| v.applies_to(tier))
                .copied()
                .collect(),
            tier,
            file: false,
        }
    }

    /// **A file type's own verbs, from its host's data** ([`FileVerb`]), and no kit verb: what
    /// `capabilities` lists for that file type ([`crate::errors::run_meta`] with this registry),
    /// each verb with the error ids whose rows name it (`@file`).
    ///
    /// - Each verb is [`Source::File`]: its rights follow from its kind (a read needs Read, an
    ///   edit Read and Edit, a history read Read and History), it makes no kit session call
    ///   ([`VerbSpec::api`] empty), it works on no kit tier ([`VerbSpec::tiers`] empty), and
    ///   it serves the host's own parity rows by name ([`VerbSpec::serves_named`]), no
    ///   [`PersonAction`].
    /// - [`Registry::tier`] is [`Tier::Blob`]: to the kit, the file is any file (tier 0). The
    ///   kit's parity harnesses refuse a file registry: its verbs are the host's to test.
    ///
    /// Refused: a name that isn't one or two words of lowercase letters, digits or `-`, a name
    /// twice, a verb serving no row (or a blank row name), and an exit outside the kit's
    /// table (0 to 8) or none.
    pub fn for_file(verbs: &[FileVerb]) -> Result<Self, RegistryError> {
        let mut r = Registry {
            verbs: Vec::with_capacity(verbs.len()),
            tier: Tier::Blob,
            file: true,
        };
        for v in verbs {
            let name = v.name;
            let words: Vec<&str> = name.split(' ').collect();
            let word_ok = |w: &str| {
                !w.is_empty()
                    && !w.starts_with('-')
                    && w.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            };
            if words.len() > 2 || !words.iter().all(|w| word_ok(w)) {
                return Err(RegistryError(format!(
                    "file verb {name:?}: one or two lowercase word(s)"
                )));
            }
            if r.get(name).is_some() {
                return Err(RegistryError(format!("file verb {name:?} twice")));
            }
            if v.serves.is_empty() || v.serves.iter().any(|s| s.trim().is_empty()) {
                return Err(RegistryError(format!(
                    "file verb {name:?} serves no parity row"
                )));
            }
            if v.exits.is_empty() || v.exits.iter().any(|x| *x > TOO_OLD) {
                return Err(RegistryError(format!(
                    "file verb {name:?}: its exits must be from the kit's table (0 to {TOO_OLD})"
                )));
            }
            r.verbs.push(VerbSpec {
                name,
                kind: v.kind,
                needs: match v.kind {
                    VerbKind::Read => NR,
                    VerbKind::Edit => NE,
                    VerbKind::History => NH,
                },
                api: &[],
                serves: &[],
                usage: v.usage,
                exits: v.exits,
                source: Source::File,
                tiers: &[],
                positions: None,
                serves_named: v.serves,
                alias: None,
            });
        }
        Ok(r)
    }

    /// Whether [`Registry::for_file`] built it.
    pub(crate) fn is_file(&self) -> bool {
        self.file
    }

    /// The tier its verbs serve: what [`super::check`] (text) and [`super::check_blob`]
    /// (blob) run. A file registry ([`Registry::for_file`]) says [`Tier::Blob`], and the
    /// harnesses refuse it.
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// The kit's verbs of the pack's tier plus the pack's own ([`crate::session::Pack::verbs`]).
    /// A [`crate::table`] pack is a grid pack ([`Tier::Grid`]), any other a text pack.
    ///
    /// - A text pack's verbs are edit verbs that need read and edit and run through
    ///   `run_tool` / `edit_with`; one word each.
    /// - A grid pack's verbs are structured verbs (A9): edit verbs need read and edit and run
    ///   through `run_tool` / `edit_ops_with`, read verbs need read and run through `run_tool`;
    ///   a name is one or two words (`row insert`).
    ///
    /// Refused: a pack verb whose first word is a kit verb's of any tier (the kit's would
    /// win), twice, with a word that isn't
    /// lowercase letters, digits or `-`, serving nothing, or of the other kind (a text verb on
    /// a grid pack, a structured verb on a text pack).
    pub fn for_pack(pack: &dyn Pack) -> Result<Self, RegistryError> {
        let grid = crate::table::is_table(&pack.schema());
        let mut r = Self::for_tier(if grid { Tier::Grid } else { Tier::Text });
        for v in pack.verbs() {
            let name = v.name();
            let words: Vec<&str> = name.split(' ').collect();
            let word_ok = |w: &str| {
                !w.is_empty()
                    && !w.starts_with('-')
                    && w.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            };
            let max_words = if v.is_structured() { 2 } else { 1 };
            if words.len() > max_words || !words.iter().all(|w| word_ok(w)) {
                return Err(RegistryError(format!(
                    "pack verb {name:?}: {} lowercase word(s)",
                    if max_words == 1 { "one" } else { "one or two" }
                )));
            }
            if v.is_structured() != grid {
                return Err(RegistryError(format!(
                    "pack verb {name:?}: a {} verb on a {} pack",
                    if v.is_structured() {
                        "structured"
                    } else {
                        "text"
                    },
                    if grid { "grid" } else { "text" }
                )));
            }
            // a kit verb's first word is the kit's, whatever follows (`blame x`, `rows x`)
            if crate::tools::is_kit_verb(words[0])
                || r.get(name).is_some()
                || KIT_VERBS
                    .iter()
                    .any(|k| k.name.split(' ').next() == Some(words[0]))
            {
                return Err(RegistryError(format!(
                    "pack verb {name:?} is already a verb (the kit's would win)"
                )));
            }
            if v.serves().is_empty() {
                return Err(RegistryError(format!(
                    "pack verb {name:?} serves no person action"
                )));
            }
            let read = v.kind() == crate::tools::Kind::Read;
            r.verbs.push(VerbSpec {
                name,
                kind: if read { VerbKind::Read } else { VerbKind::Edit },
                needs: if read { NR } else { NE },
                api: match (grid, read) {
                    (false, _) => TOOL_EDIT,
                    (true, false) => &[A::RunTool, A::EditOps],
                    (true, true) => TOOL,
                },
                serves: v.serves(),
                usage: v.usage(),
                exits: v.exits(),
                source: Source::Pack,
                tiers: if grid { GRID } else { TEXT },
                positions: v.declared_positions(),
                serves_named: &[],
                alias: None,
            });
        }
        Ok(r)
    }

    /// The registry form `argv` names: a registered two-word form (`row insert`, a grid
    /// pack's) when `argv[1]` completes one, else [`verb_form`].
    pub fn form_of(&self, argv: &[String]) -> String {
        if let [a, b, ..] = argv {
            let two = format!("{a} {b}");
            if self
                .get(&two)
                .is_some_and(|v| v.source == Source::Pack || v.is_index())
            {
                return two;
            }
        }
        verb_form(argv)
    }

    /// **The verbs of a blob doc of `pack`'s type**: [`Registry::for_tier`]`(Tier::Blob)`,
    /// the annotation verbs ([`ANNOTATION_VERBS`], any pack: a comment on the whole doc needs
    /// no index), plus, when the pack has a structure index ([`crate::session::BlobPack::index_spec`]),
    /// its verbs under the names the pack gives ([`crate::index::IndexVerbs`]), `outline` and
    /// `node` as other names for its list and show verbs (K-READ-1), and claims by node
    /// (`claim --node`, `claim --renew`, `claim --release`). A pack with no index gets exactly
    /// `for_tier(Tier::Blob).with_annotations()`, and `for_tier` never lists any of these: a
    /// host that hasn't adopted them sees no change. The annotation verbs serve P23 and P27,
    /// text and grid rows: a list checked against this registry lists them there.
    ///
    /// They are kit verbs ([`Source::Kit`], [`VerbSpec::is_index`]) of [`Tier::Blob`]: the
    /// rights, calls, exits and parity rows are the kit's, only the names the pack's. All but
    /// the export verb run through [`crate::session::BlobSession::run_tool`]; the export verb
    /// is the host's export with the node option ([`crate::export::NODE_OPTION`]). The claim
    /// verbs serve P8, a text and grid row: a pack that indexes lists it in its blob parity
    /// list too.
    ///
    /// Refused: a verb name that isn't two lowercase words (letters, digits, `-`), one whose
    /// first word is a kit verb's (any tier, `outline`, `node`, `claim` and the annotation
    /// verbs' included), and two
    /// verbs of one name.
    pub fn for_blob_pack(pack: &dyn crate::session::BlobPack) -> Result<Self, RegistryError> {
        let mut r = Self::for_tier(Tier::Blob).with_annotations();
        let Some(spec) = pack.index_spec() else {
            return Ok(r);
        };
        let v = spec.verbs;
        let mut names = vec![v.list, v.show, v.diff, v.blame];
        names.extend(v.export);
        for (k, name) in names.iter().enumerate() {
            let words: Vec<&str> = name.split(' ').collect();
            let word_ok = |w: &str| {
                !w.is_empty()
                    && !w.starts_with('-')
                    && w.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            };
            if words.len() != 2 || !words.iter().all(|w| word_ok(w)) {
                return Err(RegistryError(format!(
                    "index verb {name:?}: two lowercase words (the noun, then the verb)"
                )));
            }
            let taken = crate::tools::is_kit_verb(words[0])
                || INDEX_KIT_WORDS.contains(&words[0])
                || ANNOTATION_VERBS
                    .iter()
                    .any(|a| a.name.split(' ').next() == Some(words[0]))
                || KIT_VERBS
                    .iter()
                    .any(|kv| kv.name.split(' ').next() == Some(words[0]));
            if taken {
                return Err(RegistryError(format!(
                    "index verb {name:?}: {:?} is a kit verb's word (the kit's would win)",
                    words[0]
                )));
            }
            if names[..k].contains(name) {
                return Err(RegistryError(format!("index verb {name:?} twice")));
            }
        }
        let list = index_verb(
            v.list,
            R,
            NR,
            &[A::RunTool, A::IndexList],
            &[P::ReadAll],
            LIST_USAGE,
            OK0,
            Some(LIST_POSITIONS),
        );
        let show = index_verb(
            v.show,
            R,
            NR,
            &[A::RunTool, A::IndexNodes],
            &[P::ReadAll],
            SHOW_USAGE,
            SHOW_EXITS,
            Some(SHOW_POSITIONS),
        );
        r.verbs.push(list);
        r.verbs.push(show);
        r.verbs.push(index_verb(
            v.diff,
            H,
            NH,
            &[A::RunTool, A::IndexDiff],
            &[P::History],
            "V1 [V2] [--cursor C] [--no-truncate] [--json] (V2: the current version; node by node)",
            OK0,
            None,
        ));
        r.verbs.push(index_verb(
            v.blame,
            H,
            NH,
            &[A::RunTool, A::IndexBlame],
            &[P::Blame],
            "[--version V] [--cursor C] [--no-truncate] [--json] (per node: the version that \
             made it as it is)",
            OK0,
            None,
        ));
        if let Some(e) = v.export {
            r.verbs.push(index_verb(
                e,
                R,
                NR,
                &[A::IndexNodes, A::FileNow, A::FileAt],
                &[P::Export],
                "ADDR [--version V] --to TARGET [--option K=V]... (one node, through the export \
                 registry with the node option; another version: +history)",
                OK0,
                Some(EXPORT_POSITIONS),
            ));
        }
        r.verbs.push(VerbSpec {
            name: "outline",
            alias: Some(v.list),
            ..list
        });
        r.verbs.push(VerbSpec {
            name: "node",
            alias: Some(v.show),
            ..show
        });
        r.verbs.push(index_verb(
            "claim",
            E,
            NE,
            &[A::RunTool, A::ClaimNodes],
            &[P::Caret],
            "claim --node ADDR... [--version V] [--ttl MS] [--note TEXT] (advisory: never \
             refuses a write; someone else's claim on a node: exit 5)",
            OK0,
            Some(CLAIM_POSITIONS),
        ));
        r.verbs.push(index_verb(
            "claim --renew",
            E,
            NE,
            &[A::RunTool, A::RenewClaim],
            &[P::Caret],
            "claim --renew [--id ID] [--ttl MS] (none: exit 4)",
            OK0,
            None,
        ));
        r.verbs.push(index_verb(
            "claim --release",
            E,
            NR,
            &[A::RunTool, A::ReleaseClaim],
            &[P::Caret],
            "claim --release [--id ID] (all of mine without an id; 1: none held)",
            OK01,
            None,
        ));
        Ok(r)
    }

    /// Every verb, kit verbs first.
    pub fn verbs(&self) -> &[VerbSpec] {
        &self.verbs
    }

    /// The verb by its form (see [`verb_form`]).
    pub fn get(&self, name: &str) -> Option<&VerbSpec> {
        self.verbs.iter().find(|v| v.name == name)
    }

    /// The verbs serving `action`.
    pub fn serving(&self, action: PersonAction) -> impl Iterator<Item = &VerbSpec> {
        self.verbs
            .iter()
            .filter(move |v| v.serves.contains(&action))
    }

    /// This registry plus the annotation verbs ([`ANNOTATION_VERBS`]), for a host that serves
    /// comment threads on its docs. Adding them twice changes nothing.
    pub fn with_annotations(mut self) -> Self {
        for v in ANNOTATION_VERBS {
            if self.get(v.name).is_none() {
                self.verbs.push(*v);
            }
        }
        self
    }

    /// This registry without `name`. For negative controls: a harness run on it must report
    /// the verb missing.
    pub fn without(mut self, name: &str) -> Self {
        self.verbs.retain(|v| v.name != name);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn forms() {
        for (a, f) in [
            (&["sed", "-n", "2,3p"][..], "sed"),
            (&["sed", "-i", "s/a/b/"], "sed -i"),
            (&["sed", "-i", "", "s/a/b/"], "sed -i"),
            (&["undo"], "undo"),
            (&["undo", "--partial"], "undo --partial"),
            (&["undo", "--drop"], "undo --drop"),
            (
                &["log", "--pending", "--on-behalf-of", "me"],
                "log --pending",
            ),
            (&["log", "-n", "3"], "log"),
            (&["approvals"], "approvals"),
            (&["approvals", "--on-behalf-of", "me"], "approvals"),
            (&["blame", "-L", "2,4"], "blame"),
            (&["diff", "--version", "3"], "diff --version"),
            (&["diff", "-u"], "diff"),
            (&["show", "--version", "2"], "show --version"),
            (&["export"], "export"),
            (
                &["export", "--version", "2", "--to", "as-is"],
                "export --version",
            ),
            (&["version", "--name", "x"], "version --name"),
            (&["tail", "-f"], "tail -f"),
            (&["tail", "-n", "3"], "tail"),
            (&["replace-section", "--heading", "## X"], "replace-section"),
            (&["lease"], "lease"),
            (&["lease", "--ttl", "60000"], "lease"),
            (&["lease", "--renew"], "lease --renew"),
            (&["lease", "--release"], "lease --release"),
            (&["lease", "--break"], "lease --break"),
        ] {
            assert_eq!(verb_form(&argv(a)), f, "{a:?}");
        }
        assert_eq!(verb_form(&[]), "");
    }

    #[test]
    fn kit_verbs_are_well_formed() {
        let r = Registry::kit();
        for (i, v) in r.verbs().iter().enumerate() {
            assert!(!v.serves.is_empty(), "{} serves nothing", v.name);
            assert!(!v.api.is_empty() && !v.exits.is_empty(), "{}", v.name);
            assert!(
                v.needs.contains(&Need::Read),
                "{}: every verb needs read",
                v.name
            );
            assert_eq!(
                v.kind == VerbKind::History,
                v.needs.contains(&Need::History) && !v.needs.contains(&Need::Edit),
                "{}: history verbs need history",
                v.name
            );
            assert!(
                r.verbs()[..i].iter().all(|w| w.name != v.name),
                "{} twice",
                v.name
            );
            // every text tool's declared kind matches the tool's own
            if v.api == TOOL || v.api == TOOL_EDIT {
                let mut a: Vec<String> = v.name.split(' ').map(str::to_owned).collect();
                if a[0] == "sed" {
                    a.push("p".into());
                }
                let t = crate::tools::Tool::parse(&a);
                let kind = match t {
                    Ok(t) => t.kind(),
                    // verbs needing operands: the kit's VERBS table is the reference
                    Err(_) => crate::tools::VERBS
                        .iter()
                        .find(|(n, _)| *n == v.name)
                        .map(|(_, k)| *k)
                        .expect("a tool verb is in tools::VERBS"),
                };
                let want = if v.kind == VerbKind::Edit {
                    crate::tools::Kind::Edit
                } else {
                    crate::tools::Kind::Read
                };
                assert_eq!(kind, want, "{}", v.name);
            }
        }
        // every kit tool is registered
        for (t, _) in crate::tools::VERBS {
            assert!(r.get(t).is_some(), "tool {t} is not registered");
        }
        assert!(r.clone().without("cat").get("cat").is_none());
        assert_eq!(r.serving(PersonAction::Undo).count(), 3);
    }

    /// The `run_tool` flag is data; here it must agree with the rest of the row and with the
    /// kit's tools (`tests/registry.rs` checks it against `Session::run_tool` itself).
    #[test]
    fn run_tool_flag_agrees_with_the_row() {
        let r = Registry::kit();
        for n in RUN_TOOL_VERBS {
            assert!(r.get(n).is_some(), "{n} is not a registered verb");
        }
        for v in KIT_VERBS {
            let t = v.runs_through_run_tool();
            assert_eq!(
                t,
                v.api.contains(&Api::RunTool),
                "{}: the flag and the RunTool call disagree",
                v.name
            );
            assert_eq!(
                t,
                crate::tools::VERBS.iter().any(|(n, _)| *n == v.name),
                "{}: the flag and the kit's tools disagree",
                v.name
            );
            if t {
                assert!(v.applies_to(Tier::Text), "{}", v.name);
            }
        }
    }

    /// An alias is its verb's row under another name.
    #[test]
    fn aliases_are_their_verbs_rows() {
        let r = Registry::for_tier(Tier::Blob);
        for (alias, of) in ALIASES {
            for r in [Registry::kit(), r.clone()] {
                let (a, v) = (r.get(alias).unwrap(), r.get(of).unwrap());
                assert_eq!(a.alias_of(), Some(*of));
                assert_eq!(v.alias_of(), None, "{of}: an alias of an alias");
                assert_eq!(
                    (a.kind, a.needs, a.api, a.serves, a.exits, a.tiers, a.source),
                    (v.kind, v.needs, v.api, v.serves, v.exits, v.tiers, v.source),
                    "{alias}"
                );
                assert_eq!(a.runs_through_run_tool(), v.runs_through_run_tool());
            }
        }
        assert_eq!(Registry::kit().serving(PersonAction::Pending).count(), 2);
    }

    /// Every verb's tiers are tiers of every action it serves (so a tier's list can list
    /// it), and the tier registries hold exactly their verbs.
    #[test]
    fn tiers_are_consistent() {
        for v in KIT_VERBS {
            assert!(!v.tiers.is_empty(), "{}: no tier", v.name);
            for t in v.tiers {
                for a in v.serves {
                    assert!(
                        a.in_tier(*t),
                        "{} is {t:?} but serves {a}, which isn't",
                        v.name
                    );
                }
            }
        }
        let text = Registry::kit();
        assert_eq!(text.tier(), Tier::Text);
        assert!(text.verbs().iter().all(|v| v.applies_to(Tier::Text)));
        assert!(text.get("lease").is_none(), "no lease on text docs");
        let blob = Registry::for_tier(Tier::Blob);
        assert_eq!(blob.tier(), Tier::Blob);
        let names: Vec<&str> = blob.verbs().iter().map(|v| v.name).collect();
        assert_eq!(
            names,
            [
                "open",
                "leave",
                "cat",
                "who",
                "write",
                "status",
                "flush",
                "log",
                "show --version",
                "export",
                "export --version",
                "blame",
                "restore",
                "version --name",
                "mv",
                "approve",
                "decline",
                "log --pending",
                "approvals",
                "lease",
                "lease --renew",
                "lease --release",
                "lease --break"
            ]
        );
        // the brief's "both" verbs, and the approvals (K-T0-9, v0.5.0)
        for n in [
            "log",
            "restore",
            "version --name",
            "blame",
            "show --version",
            "approve",
            "decline",
            "log --pending",
            "approvals",
        ] {
            assert_eq!(text.get(n).map(|v| v.tiers), Some(ALL3), "{n}");
        }
        assert_eq!(
            blob.get("lease --break").map(|v| v.needs),
            Some(&[Need::Read, Need::Owner][..])
        );
        // every blob-only action has a verb of its tier
        for a in PersonAction::for_tier(Tier::Blob) {
            if a.tiers() == BLOB {
                assert!(blob.serving(a).count() > 0, "{a}");
            }
        }
        // the grid tier (A11): the kit's reads, presence, history and its own three; no text
        // edit verbs (reconcile, C1i), no leases
        let grid = Registry::for_tier(Tier::Grid);
        assert_eq!(grid.tier(), Tier::Grid);
        let names: Vec<&str> = grid.verbs().iter().map(|v| v.name).collect();
        for n in [
            "open",
            "cat",
            "head",
            "grep",
            "claim",
            "cursor",
            "follow",
            "write",
            "undo --partial",
            "diff --version",
            "blame",
            "revert",
            "rows drop-deleted",
            "blame --cell",
            "blame --structure",
        ] {
            assert!(names.contains(&n), "{n} on a grid");
        }
        for n in [
            "insert", "replace", "delete", "sed -i", "patch", "apply", "append", "lease",
        ] {
            assert!(!names.contains(&n), "{n} on a grid");
        }
        for n in ["rows drop-deleted", "blame --cell", "blame --structure"] {
            assert!(
                text.get(n).is_none() && blob.get(n).is_none(),
                "{n} is grid only"
            );
        }
        // P44 is the owner's
        assert_eq!(
            grid.get("rows drop-deleted").map(|v| (v.needs, v.serves)),
            Some((
                &[Need::Read, Need::Owner][..],
                &[PersonAction::DropDeleted][..]
            ))
        );
        // the grid-only actions with a kit verb
        assert_eq!(grid.serving(PersonAction::DropDeleted).count(), 1);
    }

    #[test]
    fn grid_forms() {
        for (a, f) in [
            (&["rows", "drop-deleted"][..], "rows drop-deleted"),
            (&["rows"], "rows"),
            (&["blame", "--cell", "r1,c2"], "blame --cell"),
            (&["blame", "--structure"], "blame --structure"),
            (&["blame", "-L", "2,3"], "blame"),
        ] {
            assert_eq!(verb_form(&argv(a)), f, "{a:?}");
        }
        // a registry without pack verbs names two words only when they are a kit form
        let r = Registry::for_tier(Tier::Grid);
        assert_eq!(r.form_of(&argv(&["row", "insert"])), "row");
        assert_eq!(
            r.form_of(&argv(&["rows", "drop-deleted"])),
            "rows drop-deleted"
        );
    }

    /// A file registry ([`Registry::for_file`]) holds the host's data as given, as file verbs
    /// only, and refuses what a host's command line couldn't name or capabilities couldn't
    /// state.
    #[test]
    fn a_file_registry_is_the_hosts_data() {
        const S: &[&str] = &["row-a"];
        let ok = [
            FileVerb::new("add", VerbKind::Edit, "add <kind>", &[OK, NO], S),
            FileVerb::new("read", VerbKind::Read, "read <id>", &[OK], S),
            FileVerb::new(
                "table add-row",
                VerbKind::Edit,
                "table add-row <id>",
                &[OK],
                S,
            ),
        ];
        let r = Registry::for_file(&ok).unwrap();
        assert_eq!(r.verbs().len(), 3);
        assert!(r.is_file() && !Registry::kit().is_file());
        for (v, f) in r.verbs().iter().zip(&ok) {
            assert_eq!(
                (v.name, v.kind, v.usage, v.exits),
                (f.name, f.kind, f.usage, f.exits)
            );
            assert_eq!(v.source, Source::File);
            assert_eq!(v.serves_named, S);
            assert!(v.serves.is_empty() && v.api.is_empty() && v.tiers.is_empty());
            assert!(!v.runs_through_run_tool() && v.alias_of().is_none());
            assert_eq!(v.needs, if v.kind == VerbKind::Read { NR } else { NE });
        }
        // a kit verb's name is a file verb here, never the kit's
        assert!(r.get("cat").is_none());
        assert_eq!(r.get("read").map(|v| v.source), Some(Source::File));
        // the kit's harnesses run no file registry
        assert!(matches!(
            super::super::check_list(&[], &r, &[]).as_slice(),
            [super::super::Failure::Registry(m)] if m.contains("file type's own verbs")
        ));
        let bad = |v: FileVerb| Registry::for_file(&[ok[0], v]).unwrap_err().0;
        assert!(bad(FileVerb::new("Add", VerbKind::Edit, "", &[OK], S)).contains("lowercase"));
        assert!(bad(FileVerb::new("a b c", VerbKind::Edit, "", &[OK], S)).contains("lowercase"));
        assert!(bad(FileVerb::new("--x", VerbKind::Edit, "", &[OK], S)).contains("lowercase"));
        assert!(bad(FileVerb::new("", VerbKind::Edit, "", &[OK], S)).contains("lowercase"));
        assert!(bad(ok[0]).contains("twice"));
        assert!(bad(FileVerb::new("x", VerbKind::Edit, "", &[OK], &[])).contains("no parity row"));
        assert!(
            bad(FileVerb::new("x", VerbKind::Edit, "", &[OK], &[" "])).contains("no parity row")
        );
        assert!(bad(FileVerb::new("x", VerbKind::Edit, "", &[], S)).contains("kit's table"));
        assert!(bad(FileVerb::new("x", VerbKind::Edit, "", &[9], S)).contains("kit's table"));
    }
}
