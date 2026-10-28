//! The verb registry (K-PAR-2): every verb an agent can run, what it serves and needs.

use std::fmt;

use super::action::PersonAction;
use crate::session::{Need, Pack};
use crate::tools::exit::{NO, OK, TOO_OLD};

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
}

impl Tier {
    /// Every tier.
    pub const ALL: &'static [Tier] = &[Tier::Text, Tier::Blob];
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
}

/// Where a verb comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The kit's verbs (every text pack has them).
    Kit,
    /// A pack's own verb ([`crate::session::Pack::verbs`]).
    Pack,
}

/// One verb: its name (the CLI form, e.g. `undo --partial`), kind, rights, the session
/// calls it makes, the person actions it serves, its flags and its exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl VerbSpec {
    /// Whether it works on `tier`'s docs.
    pub fn applies_to(&self, tier: Tier) -> bool {
        self.tiers.contains(&tier)
    }

    const fn on(self, tiers: &'static [Tier]) -> Self {
        VerbSpec { tiers, ..self }
    }

    /// Whether it runs through [`crate::session::Session::run_tool`] on a text doc: the
    /// kit's verbs in [`RUN_TOOL_VERBS`] and every pack verb. A host routes these to
    /// `run_tool` (or `run_tool_with`) and the others to their own session calls
    /// ([`VerbSpec::api`]). On a blob doc no verb runs through `run_tool` (`cat` there is
    /// `BlobSession::read`).
    pub fn runs_through_run_tool(&self) -> bool {
        self.source == Source::Pack || RUN_TOOL_VERBS.contains(&self.name)
    }

    /// The verb this one is another name for ([`ALIASES`]), if any: the same row, the same
    /// binding, under a host-neutral name.
    pub fn alias_of(&self) -> Option<&'static str> {
        ALIASES
            .iter()
            .find(|(a, _)| *a == self.name)
            .map(|(_, of)| *of)
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
const BOTH: &[Tier] = &[Tier::Text, Tier::Blob];
const BLOB: &[Tier] = &[Tier::Blob];

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
    .on(BOTH),
    kit("leave", R, NR, &[A::Leave], &[P::Close], "leave", OK0).on(BOTH),
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
    .on(BOTH),
    kit(
        "wc",
        R,
        NR,
        TOOL,
        &[P::ReadAll],
        "wc [-l] [-w] [-c] [-m]",
        OK0,
    ),
    kit(
        "head",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "head [-n N] [-c N] [--for-edit [--ttl MS]]",
        OK0,
    ),
    kit(
        "tail",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "tail [-n [+]N] [-c [+]N] [--for-edit [--ttl MS]]",
        OK0,
    ),
    kit(
        "sed",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "sed -n 'a,bp' (any script without -i) [--for-edit [--ttl MS]]",
        OK0,
    ),
    kit(
        "view",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "view --lines a,b [--for-edit [--ttl MS]]",
        OK0,
    ),
    kit(
        "grep",
        R,
        NR,
        TOOL,
        &[P::Find],
        "grep [-EFGivwxcoqlLHhn] [-m N] [-A/-B/-C N] [--for-edit [--ttl MS]] PATTERN",
        OK01,
    ),
    kit("who", R, NR, &[A::Roster], &[P::Who], "who", OK0).on(BOTH),
    // agent presence (amendment 13): where others are working, and saying where I am
    kit(
        "activity",
        R,
        NR,
        &[A::Activity],
        &[P::Who],
        "activity [-n N] (who did what, where, lately)",
        OK0,
    ),
    kit(
        "claim",
        E,
        NE,
        &[A::Claim],
        &[P::Caret],
        "claim --lines a,b [--ttl MS] [--note TEXT] [--hold] (someone else's claim there: exit 5)",
        OK0,
    ),
    kit(
        "claim --renew",
        E,
        NE,
        &[A::RenewClaim],
        &[P::Caret],
        "claim --renew [--id ID] [--ttl MS] [--hold | --no-hold] (others waiting: exit 2; none: 4)",
        OK0,
    ),
    kit(
        "claim --release",
        E,
        NR,
        &[A::ReleaseClaim],
        &[P::Caret],
        "claim --release [--id ID] (all of mine without an id; 1: none held)",
        OK01,
    ),
    kit(
        "follow",
        R,
        NR,
        &[A::Roster],
        &[P::Follow],
        "follow <pid> (1: not here or no caret)",
        OK01,
    ),
    kit(
        "cursor",
        E,
        NE,
        &[A::SetCursor, A::SetView],
        &[P::Caret],
        "cursor --at L:C | --select L:C-L:C (read only: a viewing range)",
        OK0,
    ),
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
        "write --base V | --base live (new text on stdin)",
        &[OK, NO, TOO_OLD],
    )
    .on(BOTH),
    kit(
        "undo",
        E,
        NE,
        &[A::UndoWith],
        &[P::Undo],
        "undo [--steps N] [--for P]",
        OK01,
    ),
    kit(
        "redo",
        E,
        NE,
        &[A::RedoWith],
        &[P::Undo],
        "redo [--steps N]",
        OK01,
    ),
    kit(
        "undo --drop",
        E,
        NE,
        &[A::UndoWith],
        &[P::Undo],
        "undo --drop",
        OK0,
    ),
    kit(
        "undo --partial",
        E,
        NE,
        &[A::UndoWith],
        &[P::UndoPartial],
        "undo --partial [--steps N] [--for P]",
        OK01,
    ),
    kit(
        "status",
        R,
        NR,
        &[A::Status],
        &[P::Saved, P::WhyOff],
        "status",
        OK0,
    )
    .on(BOTH),
    kit("flush", R, NR, &[A::Flush], &[P::Saved], "flush", OK0).on(BOTH),
    kit(
        "watch",
        R,
        NR,
        &[A::Join, A::Subscribe],
        &[P::Watch],
        "watch",
        OK0,
    ),
    kit(
        "tail -f",
        R,
        NR,
        &[A::Join, A::Subscribe],
        &[P::Watch],
        "tail -f (the same as watch)",
        OK0,
    ),
    kit(
        "log",
        H,
        NH,
        &[A::Log],
        &[P::History],
        "log [--by P] [-n N]",
        OK0,
    )
    .on(BOTH),
    kit(
        "diff --version",
        H,
        NH,
        &[A::DiffVersion],
        &[P::History],
        "diff --version V [--to W] [-U N]",
        OK01,
    ),
    kit(
        "show --version",
        H,
        NH,
        &[A::Show],
        &[P::History],
        "show --version V",
        OK0,
    )
    .on(BOTH),
    kit(
        "diff",
        R,
        NR,
        TOOL,
        &[P::History],
        "diff [-u] [-U N] (my copy on stdin vs live)",
        OK01,
    ),
    kit(
        "blame",
        H,
        NH,
        &[A::Blame],
        &[P::Blame],
        "blame [-L a,b]",
        OK0,
    )
    .on(BOTH),
    kit(
        "restore",
        E,
        &[Need::Read, Need::Edit, Need::History],
        &[A::Restore],
        &[P::Restore],
        "restore --version V",
        OK0,
    )
    .on(BOTH),
    kit(
        "version --name",
        E,
        NE,
        &[A::NameVersion],
        &[P::Restore],
        "version --name NAME [--version V]",
        OK0,
    )
    .on(BOTH),
    kit(
        "mv",
        E,
        NE,
        &[A::Rekey, A::FileMoved],
        &[P::Rename],
        "mv <new path>",
        OK0,
    )
    .on(BOTH),
    kit(
        "approve",
        E,
        &[Need::Read, Need::Approver],
        &[A::SetApproval],
        &[P::Approve],
        "approve <change> (a blob doc: <version>)",
        OK0,
    )
    .on(BOTH),
    kit(
        "decline",
        E,
        &[Need::Read, Need::Approver],
        &[A::SetApproval],
        &[P::Approve],
        "decline <change> (a blob doc: <version>; then restore --version to reverse it)",
        OK0,
    )
    .on(BOTH),
    kit(
        "log --pending",
        H,
        NH,
        &[A::Log],
        &[P::Pending],
        "log --pending [--on-behalf-of me|P]",
        OK0,
    )
    .on(BOTH),
    // the same list under a host-neutral name: what `log --pending` does, flag for flag
    kit(
        "approvals",
        H,
        NH,
        &[A::Log],
        &[P::Pending],
        "approvals [--on-behalf-of me|P] (the same as log --pending)",
        OK0,
    )
    .on(BOTH),
    kit(
        "revert",
        E,
        &[Need::Read, Need::Edit, Need::AuthorOrOwner],
        &[A::Revert],
        &[P::Revert],
        "revert <change> [--partial] (too old: exit 8)",
        OK01,
    ),
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
/// `lease --release`, `lease --break`). A host names verbs the same way when it checks its
/// command table against the registry.
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
        "version" => "version --name",
        "tail" if has("-f") || has("--follow") => "tail -f",
        "lease" if has("--renew") => "lease --renew",
        "claim" if has("--renew") => "claim --renew",
        "claim" if has("--release") => "claim --release",
        "lease" if has("--release") => "lease --release",
        "lease" if has("--break") => "lease --break",
        v => v,
    };
    form.to_owned()
}

/// Why a registry can't be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError(pub String);

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "verb registry: {}", self.0)
    }
}

impl std::error::Error for RegistryError {}

/// The verbs of one tier: the kit's ([`KIT_VERBS`] that apply to it), plus a text pack's
/// own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    verbs: Vec<VerbSpec>,
    tier: Tier,
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
        }
    }

    /// The tier its verbs serve: what [`super::check`] (text) and [`super::check_blob`]
    /// (blob) run.
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// The kit's text verbs plus the pack's ([`crate::session::Pack::verbs`]): edit verbs that
    /// need read and edit and run through `run_tool` / `edit_with`. Refused: a pack verb
    /// named like a kit verb of any tier (the kit's would win), twice, with spaces, or serving
    /// nothing.
    pub fn for_pack(pack: &dyn Pack) -> Result<Self, RegistryError> {
        let mut r = Self::kit();
        for v in pack.verbs() {
            let name = v.name();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(RegistryError(format!("pack verb {name:?}: one word")));
            }
            if crate::tools::is_kit_verb(name)
                || r.get(name).is_some()
                || KIT_VERBS
                    .iter()
                    .any(|k| k.name.split(' ').next() == Some(name))
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
            r.verbs.push(VerbSpec {
                name,
                kind: VerbKind::Edit,
                needs: NE,
                api: TOOL_EDIT,
                serves: v.serves(),
                usage: v.usage(),
                exits: v.exits(),
                source: Source::Pack,
                tiers: TEXT,
            });
        }
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
            assert_eq!(text.get(n).map(|v| v.tiers), Some(BOTH), "{n}");
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
    }
}
