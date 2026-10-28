//! The verb registry (K-PAR-2): every verb an agent can run, what it serves and needs.

use std::fmt;

use super::action::PersonAction;
use crate::session::{Need, Pack};
use crate::tools::exit::{NO, OK, TOO_OLD};

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

/// A session call a verb is bound to (the "Session API" column of `docs/PARITY.md`).
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
    /// Every exit code it can end with by itself; the session's refusals (exit 2, 3, 4, 6,
    /// 8 for a revert too old...) come on top, mapped by `tools::session_exit`.
    pub exits: &'static [u8],
    pub source: Source,
}

use Api as A;
use PersonAction as P;
use VerbKind::{Edit as E, History as H, Read as R};

const NR: &[Need] = &[Need::Read];
const NE: &[Need] = &[Need::Read, Need::Edit];
const NH: &[Need] = &[Need::Read, Need::History];
const OK0: &[u8] = &[OK];
const OK01: &[u8] = &[OK, NO];

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
    }
}

const TOOL: &[Api] = &[A::RunTool];
const TOOL_EDIT: &[Api] = &[A::RunTool, A::EditWith];

/// The kit's verbs, for every text pack.
pub const KIT_VERBS: &[VerbSpec] = &[
    kit(
        "open",
        R,
        NR,
        &[A::Join],
        &[P::Open],
        "open (stays until leave)",
        OK0,
    ),
    kit("leave", R, NR, &[A::Leave], &[P::Close], "leave", OK0),
    kit("cat", R, NR, TOOL, &[P::ReadAll], "cat [-n] [-b]", OK0),
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
        "head [-n N] [-c N]",
        OK0,
    ),
    kit(
        "tail",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "tail [-n [+]N] [-c [+]N]",
        OK0,
    ),
    kit(
        "sed",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "sed -n 'a,bp' (any script without -i)",
        OK0,
    ),
    kit(
        "view",
        R,
        NR,
        TOOL,
        &[P::ViewRange],
        "view --lines a,b",
        OK0,
    ),
    kit(
        "grep",
        R,
        NR,
        TOOL,
        &[P::Find],
        "grep [-EFGivwxcoqlLHhn] [-m N] [-A/-B/-C N] PATTERN",
        OK01,
    ),
    kit("who", R, NR, &[A::Roster], &[P::Who], "who", OK0),
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
        "insert --line N | --after N | --at L:C (text on stdin)",
        OK0,
    ),
    kit(
        "replace",
        E,
        NE,
        TOOL_EDIT,
        &[P::TypeOver, P::Format],
        "replace --lines a,b | --range L:C-L:C (text on stdin)",
        OK0,
    ),
    kit(
        "delete",
        E,
        NE,
        TOOL_EDIT,
        &[P::Delete],
        "delete --lines a,b | --range L:C-L:C",
        OK0,
    ),
    kit(
        "sed -i",
        E,
        NE,
        TOOL_EDIT,
        &[P::Delete, P::FindReplace, P::Format],
        "sed -i SCRIPT (s///, d, a, i, c, y...)",
        OK0,
    ),
    kit(
        "patch",
        E,
        NE,
        TOOL_EDIT,
        &[P::Paste],
        "patch [-p N] [-R] [--dry-run] [-F N] [--partial] (diff on stdin)",
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
        "append [--ensure-newline] (text on stdin)",
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
    ),
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
    ),
    kit("flush", R, NR, &[A::Flush], &[P::Saved], "flush", OK0),
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
    ),
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
    ),
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
    ),
    kit(
        "restore",
        E,
        &[Need::Read, Need::Edit, Need::History],
        &[A::Restore],
        &[P::Restore],
        "restore --version V",
        OK0,
    ),
    kit(
        "version --name",
        E,
        NE,
        &[A::NameVersion],
        &[P::Restore],
        "version --name NAME [--version V]",
        OK0,
    ),
    kit(
        "mv",
        E,
        NE,
        &[A::Rekey, A::FileMoved],
        &[P::Rename],
        "mv <new path>",
        OK0,
    ),
    kit(
        "approve",
        E,
        &[Need::Read, Need::Approver],
        &[A::SetApproval],
        &[P::Approve],
        "approve <change>",
        OK0,
    ),
    kit(
        "decline",
        E,
        &[Need::Read, Need::Approver],
        &[A::SetApproval],
        &[P::Approve],
        "decline <change>",
        OK0,
    ),
    kit(
        "log --pending",
        H,
        NH,
        &[A::Log],
        &[P::Pending],
        "log --pending [--on-behalf-of me|P]",
        OK0,
    ),
    kit(
        "revert",
        E,
        &[Need::Read, Need::Edit, Need::AuthorOrOwner],
        &[A::Revert],
        &[P::Revert],
        "revert <change> [--partial] (too old: exit 8)",
        OK01,
    ),
];

/// The verb form an `argv` names: `argv[0]`, or `argv[0]` plus the flag that makes it a
/// separate registry entry (`sed -i`, `undo --partial`, `undo --drop`, `log --pending`,
/// `diff --version`, `show --version`, `version --name`, `tail -f`). A host names verbs the
/// same way when it checks its command table against the registry.
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

/// The verbs of one pack: the kit's ([`KIT_VERBS`]) plus the pack's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    verbs: Vec<VerbSpec>,
}

impl Registry {
    /// The kit's verbs only.
    pub fn kit() -> Self {
        Registry {
            verbs: KIT_VERBS.to_vec(),
        }
    }

    /// The kit's verbs plus the pack's ([`crate::session::Pack::verbs`]): edit verbs that
    /// need read and edit and run through `run_tool` / `edit_with`. Refused: a pack verb
    /// named like a kit verb (the kit's would win), twice, with spaces, or serving nothing.
    pub fn for_pack(pack: &dyn Pack) -> Result<Self, RegistryError> {
        let mut r = Self::kit();
        for v in pack.verbs() {
            let name = v.name();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return Err(RegistryError(format!("pack verb {name:?}: one word")));
            }
            if crate::tools::is_kit_verb(name) || r.get(name).is_some() {
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
            (&["blame", "-L", "2,4"], "blame"),
            (&["diff", "--version", "3"], "diff --version"),
            (&["diff", "-u"], "diff"),
            (&["show", "--version", "2"], "show --version"),
            (&["version", "--name", "x"], "version --name"),
            (&["tail", "-f"], "tail -f"),
            (&["tail", "-n", "3"], "tail"),
            (&["replace-section", "--heading", "## X"], "replace-section"),
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
}
