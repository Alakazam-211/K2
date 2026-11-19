//! The registry: every refusal the kit can return, one row each. **Append only**: a new
//! refusal takes the next free id in its area; an id is never reused, renamed or given
//! another exit code, and a retired one stays here as `.retired("0.x.y")`. The snapshot
//! test (`crates/base/tests/golden/errors/registry.snapshot`) holds this.
//!
//! Message templates use `{key}` for the report's details. `next` is prose: what happened,
//! then what to run. `.step(verb, args, why)` is the same as data ([`super::NextStep`]):
//! a kit verb by its registry name, its arguments as argv tokens (`{key}`: the
//! refusal's `details.key`), and why, in prose naming no command line. An entry with nothing
//! to run (wait, ask a person, report it, use ordinary file tools) has no step.
//! Neither names a host's command (its program name, the doc path): the host renders those.
//!
//! Rule (each entry's fifth field): a stable key for the requirement this error enforces.

use super::Area::{
    self, Blob, Disk, Erasure, Export, History, Limits, Presence, Session, Store, Structured,
};
use super::ErrorEntry as E;

macro_rules! registry {
    ($table:ident: $($k:ident = $e:expr;)*) => {
        $( pub static $k: E = $e; )*
        /// Every kit entry, in id order.
        pub static $table: &[&E] = &[ $( &$k ),* ];
    };
}

#[allow(clippy::too_many_arguments)]
const fn e(
    id: &'static str,
    name: &'static str,
    area: Area,
    exit: u8,
    rule: &'static str,
    verbs: &'static [&'static str],
    message: &'static str,
    next: &'static str,
) -> E {
    E::new(id, name, area, exit, rule, verbs, message, next)
}

const ANY: &[&str] = &["*"];
const CHANGE: &[&str] = &["@change"];
const RUN_TOOL: &[&str] = &["@run_tool"];
const UNDO: &[&str] = &["undo", "redo", "undo --partial", "undo --drop"];
const SYNC: &[&str] = &["(sync)"];
const HOST: &[&str] = &["(host)"];
/// The annotation verbs that change a thread (and need the comment right).
const ANNOT_CHANGE: &[&str] = &[
    "comment",
    "reply",
    "resolve",
    "resolve --reopen",
    "reanchor",
];
/// A structure index's verbs (`Registry::for_blob_pack`: the pack names them), and the host
/// calls under them.
const INDEX_HOST: &[&str] = &["@index", "(host)"];
/// A file type's own verbs on a blob doc (the tier-0 verb path; `Registry::for_blob_pack`).
const BLOB_VERB: &[&str] = &["@blob_verb"];

// The board file type's verbs (830–869): data, as its rows name them.
/// The verbs whose change names the settings' rev: `meta`, and raw ops (`apply`, `inverse`).
const BOARD_SETTINGS: &[&str] = &["@file", "meta", "apply", "inverse"];
/// How the board's rows are reached: the host's bridge, by id.
const BOARD_BRIDGE: &str =
    "a board file verb, through its host's bridge (the board's refusal, by id)";
/// Every verb that reads the board from the file (all but `fields` and `templates`).
const BOARD_OPEN: &[&str] = &[
    "@file",
    "add",
    "update",
    "delete",
    "put-back",
    "connect",
    "react",
    "vote",
    "comment",
    "reply",
    "resolve",
    "meta",
    "lock",
    "check-add",
    "check-set",
    "check-remove",
    "table-cell",
    "table-add-row",
    "table-remove-row",
    "table-add-column",
    "table-remove-column",
    "move",
    "order",
    "frame-order",
    "group",
    "ungroup",
    "align",
    "distribute",
    "tidy",
    "copy-style",
    "duplicate",
    "paste",
    "connected-copy",
    "mind-child",
    "template",
    "clear-blob",
    "list",
    "read",
    "search",
    "references",
    "apply",
    "inverse",
];
/// Every verb that changes the board and saves the file.
const BOARD_CHANGE: &[&str] = &[
    "@file",
    "add",
    "update",
    "delete",
    "put-back",
    "connect",
    "react",
    "vote",
    "comment",
    "reply",
    "resolve",
    "meta",
    "lock",
    "check-add",
    "check-set",
    "check-remove",
    "table-cell",
    "table-add-row",
    "table-remove-row",
    "table-add-column",
    "table-remove-column",
    "move",
    "order",
    "frame-order",
    "group",
    "ungroup",
    "align",
    "distribute",
    "tidy",
    "copy-style",
    "duplicate",
    "paste",
    "connected-copy",
    "mind-child",
    "template",
    "clear-blob",
    "apply",
    "inverse",
];
/// The verbs whose change names an object's rev the caller read (`meta`'s rev is the
/// settings', MPC-E858).
const BOARD_REV: &[&str] = &[
    "@file",
    "update",
    "delete",
    "resolve",
    "lock",
    "check-set",
    "check-remove",
    "table-cell",
    "table-remove-row",
    "table-remove-column",
    "move",
    "group",
    "ungroup",
    "align",
    "distribute",
    "tidy",
    "copy-style",
    "apply",
    "inverse",
];
/// The verbs whose change can add a person: every change verb with a person to add (its
/// group adds them first), and `apply`'s person op; not `inverse`, which adds no one.
const BOARD_PERSON: &[&str] = &[
    "@file",
    "add",
    "update",
    "delete",
    "put-back",
    "connect",
    "react",
    "vote",
    "comment",
    "reply",
    "resolve",
    "meta",
    "lock",
    "check-add",
    "check-set",
    "check-remove",
    "table-cell",
    "table-add-row",
    "table-remove-row",
    "table-add-column",
    "table-remove-column",
    "move",
    "order",
    "frame-order",
    "group",
    "ungroup",
    "align",
    "distribute",
    "tidy",
    "copy-style",
    "duplicate",
    "paste",
    "connected-copy",
    "mind-child",
    "template",
    "clear-blob",
    "apply",
];

registry! { KIT:

// ---------------------------------------------------------------- 0xx exports (and the client's 000)
// 000: the JS client's own (client/src/wire.ts `ERR.NOT_APPLIED`): no kit verb returns it
// Its area reads `exports`: the hundreds digit names the area (a test holds that), and this
// block is 0xx. Retitling the area would change E001-E007's explain goldens, so it stays.

NOT_APPLIED = e("MPC-E000", "not_applied", Export, 1, "client/WIRE.md (a lost reply)", SYNC,
    "the change wasn't applied (its reply was lost)",
    "The connection dropped before the host answered this change, and the doc's record doesn't \
     list it, so the client kept its text out of the doc and handed it back (a rejection). \
     Re-apply it (the client checks the record first and re-applies only a change that never \
     landed), or dismiss it. Nothing else changed.")
    .indirect("the JS client: a change whose reply was lost, settled by a since reply");

// 001-: a host's export of a doc's version (crate::export, ExportRegistry::export)

EXPORT_UNSUPPORTED = e("MPC-E001", "export_unsupported", Export, 2, "X-ERR-1", HOST,
    "no exporter writes {target} from {kind}",
    "That target isn't offered for this file. List the targets this file can be exported \
     to (the host's export --list) and pick one of them. Nothing was written.");

EXPORT_DECLINED = e("MPC-E002", "export_declined", Export, 2, "X-ERR-1", HOST,
    "the exporter declined ({code})",
    "The exporter refused this input or these options (details.code says which; \
     bad_option: an option the target doesn't have, or a value it doesn't take). Fix the \
     options, or export to another target. Nothing was written.");

EXPORT_LIMIT = e("MPC-E003", "export_limit", Export, 2, "X-LIM-1", HOST,
    "export limit: {limit}",
    "The export would pass a limit (details.limit: input or output bytes, pixels, pages, \
     time). Nothing was placed. Export fewer pages or a smaller size, or ask the host's \
     operator about the limit.");

EXPORT_CANCELLED = e("MPC-E004", "export_cancelled", Export, 1, "X-CAN-1", HOST,
    "export cancelled",
    "The export was cancelled before it finished; nothing was placed. Run it again if you \
     still want the file.")
    .step("export", &[],
        "The export was cancelled before it finished and nothing was placed; run it again if you \
         still want the file.");

EXPORT_DECODE = e("MPC-E005", "export_decode", Export, 1, "X-ERR-1", HOST,
    "could not read the source",
    "The source version couldn't be read in its format (damaged, or a variant the exporter \
     doesn't read). Nothing was written. Export as-is to get the bytes, or try another \
     version.")
    .step("export", &["--to", "as-is"],
        "The source couldn't be read in its format; export it as it is to get the bytes, or try \
         another version.");

EXPORT_IO = e("MPC-E006", "export_io", Export, 1, "X-ERR-1", HOST,
    "export i/o failed",
    "Reading the source or writing the output failed. Nothing was placed. Check the \
     destination (space, permissions), then run it again.");

EXPORT_INTERNAL = e("MPC-E007", "export_internal", Export, 1, "X-FUZZ-1", HOST,
    "exporter {exporter} failed",
    "The exporter failed in a way it shouldn't: a bug, caught (nothing was placed and the \
     doc is unchanged). Report it with details.exporter and the source's format; export to \
     another target meanwhile.");

// ---------------------------------------------------------------- 1xx sessions and edits
// 101–109: rights and joining

FORBIDDEN_READ = e("MPC-E101", "forbidden_read", Session, 3, "K-PERM-2", ANY,
    "refused: needs Read rights",
    "You can't read this doc, and you were removed from it. Don't retry: ask the doc's \
     owner for read access (the host may print how). Nothing changed.");

FORBIDDEN_EDIT = e("MPC-E102", "forbidden_edit", Session, 3, "K-PAR-6, K-PERM-2", CHANGE,
    "refused: needs Edit rights",
    "You can read this doc but not change it. Don't retry: ask the doc's owner for edit \
     access (the host may print how), or hand the change to someone who can edit. Nothing \
     changed.");

FORBIDDEN_HISTORY = e("MPC-E103", "forbidden_history", Session, 3, "K-PERM-9",
    &["@history", "log --pending", "approvals"],
    "refused: needs History rights",
    "Reading history (log, diff --version, show --version, blame, versions, someone else's \
     approvals) needs the history right, which you don't have. Don't retry: work from the \
     live text (cat), or ask the doc's owner for history access.")
    .step("cat", &[],
        "Reading history needs a right you don't have; work from the live text instead, or ask \
         the doc's owner for history access.");

FORBIDDEN_APPROVER = e("MPC-E104", "forbidden_approver", Session, 3, "K-ID-11",
    &["approve", "decline"],
    "refused: needs Approver rights",
    "Only the person a change was made for can approve or decline it; an agent never can, \
     even the one that made it. Ask that person to run approve or decline (list the \
     pending changes with log --pending). Don't retry.")
    .step("log --pending", &[],
        "Only the person a change was made for can approve or decline it; list the pending \
         changes and ask that person.");

FORBIDDEN_AUTHOR_OR_OWNER = e("MPC-E105", "forbidden_author_or_owner", Session, 3, "K-HIST-19",
    &["revert"],
    "refused: needs AuthorOrOwner rights",
    "Only the change's author, the person it was made for, or an owner can revert it. \
     Don't retry: ask one of them, or undo your own changes with undo.")
    .step("undo", &[],
        "Only the change's author, the person it was made for or an owner can revert it; undo \
         your own changes instead, or ask one of them.");

FORBIDDEN_OWNER = e("MPC-E106", "forbidden_owner", Session, 3, "K-PURGE-1, K-T0-2",
    &["lease --break", "purge"],
    "refused: needs Owner rights",
    "Only an owner of the doc can do this (break someone else's lease, purge). Don't \
     retry: ask an owner. To wait for a lease instead, retry after it expires (status shows \
     the holder and expiry).")
    .step("status", &[],
        "Only an owner can do this; ask one, or see who holds the lease and until when, and retry \
         after it expires.");

NOT_JOINED = e("MPC-E107", "not_joined", Session, 2, "K-PRES-1", ANY,
    "{conn} has not joined this doc",
    "Your connection left the doc (it was idle, kicked, or the doc reloaded). Run the verb \
     again: any verb joins the doc first. Nothing was applied.")
    .step("open", &[],
        "Your connection left the doc; join it again, then run your verb. Nothing was applied.");

NO_DOC = e("MPC-E108", "no_doc", Session, 4, "K-EXIT-1", ANY,
    "no doc at {key}",
    "There is no live doc at that path and no file to open it from. Check the path (the \
     host resolves it inside the workspace); create the file first if it should exist.");

PACK_MISMATCH = e("MPC-E109", "pack_mismatch", Session, 4, "K-FMT-2", ANY,
    "doc was stored by pack {stored}, not {pack}",
    "The stored doc belongs to another file type than the one asked for (the path's type \
     changed, or the host routed it to the wrong pack). Open it as its stored type, or ask \
     the host's operator; nothing changed.");

// 110–119: edit guards and conflicts

STALE = e("MPC-E110", "stale", Session, 1, "the version guard (--if-version), K-T0-5",
    &["@change", "write"],
    "doc changed since {expected} (now {actual})",
    "Someone changed the doc after the version you named, so nothing was applied. Read it \
     again (cat prints the version), redo your change on what you read, and pass the new \
     version with --if-version (a blob write: the new base).")
    .step("cat", &[],
        "Someone changed the doc after the version you named; read it again, redo your change on \
         what you read, and guard it with the new version.");

WRITE_CONFLICT = e("MPC-E111", "write_conflict", Session, 1, "K-BR-2, K-BR-3 (O1–O5)",
    &["write"],
    "your write touches lines someone else changed since version {base}: {count} \
     region(s), your {lines} (now {lines} of version {current}); nothing applied. To retry: \
     read the current text again (cat; it is version {current}), redo your change on it, \
     then write --base {current}",
    "Someone else changed the same lines after the version you read (or added lines at \
     the same place), so nothing was applied; a write never merges inside a line. Run cat \
     to read the current text (the message and details.current give its version), make \
     your change again on that text, and write it with write --base <that version>. Or \
     change only the lines you need with replace --lines or patch, each with \
     --if-version <that version> (they take --if-version, not --base). details.conflicts has \
     each region's your_lines (in your text), current_lines (in the current text), base, \
     ours and theirs.")
    .step("cat", &[],
        "Someone else changed the same lines since your base; read the current text, redo your \
         change on it, then write it against that version.");

BASE_EXPIRED = e("MPC-E112", "base_expired", Session, 8, "K-BR-4, K-EXIT-1",
    &["write", "@pack"],
    "base version {base} is unknown or no longer cached; nothing applied",
    "The version you wrote against is too old to merge from (or was never read). Never \
     retry the same request: read the doc again (cat), redo your change on what you read, \
     and write with that version as --base.")
    .step("cat", &[],
        "The version you wrote against is too old to merge from; read the doc again, redo your \
         change, and write against the version you read. Never retry the same request.");

BAD_EDIT = e("MPC-E113", "bad_edit", Session, 2, "K-SCH-2", CHANGE,
    "bad edit: {reason}",
    "The edit itself is malformed (the message says how). Fix the arguments and run it \
     again; nothing was applied.");

BAD_POSITION = e("MPC-E114", "bad_position", Session, 2, "K-PRES-6",
    &["cursor", "view", "claim", "blame", "select", "pointer", "viewport"],
    "bad position: {reason}",
    "A line, position or element isn't in the doc (the message says which). Read the doc and \
     pass one that is.")
    .step("cat", &[],
        "A line, position or element isn't in the doc; read the doc and pass one that is.");

BAD_PARTICIPANT = e("MPC-E115", "bad_participant", Session, 2, "K-PRES-9",
    &["(host)"],
    "bad participant: {reason}",
    "The host passed an identity the kit refuses (the message says which field). This is \
     the host's to fix, not the caller's.");

// 116–117: comment rights (annotations)

FORBIDDEN_COMMENT = e("MPC-E116", "forbidden_comment", Session, 3, "K-ANN-6",
    ANNOT_CHANGE,
    "refused: needs Comment rights",
    "You can read this doc but not comment on it: commenting needs edit, or the comment right \
     the doc's owner grants a read-only share. Don't retry: read the threads, or ask the owner. \
     Nothing changed.")
    .step("comments", &[],
        "You can't comment on this doc; read its threads instead, or ask the doc's owner for the \
         comment right.");

FORBIDDEN_COMMENT_AUTHOR = e("MPC-E117", "forbidden_comment_author", Session, 3, "K-ANN-6",
    &["comment --delete"],
    "refused: only its author, the person it was made for, or an owner can delete comment \
     {comment} of {thread}",
    "Only a comment's author, the person it was made for, or an owner can delete it. Don't \
     retry: reply on the thread instead, or ask one of them. Nothing changed.")
    .step("reply", &["{thread}"],
        "Only the comment's author, the person it was made for or an owner can delete it; reply \
         on the thread instead, or ask one of them.");

// 120–129: the text tools

TOOL_UNSUPPORTED = e("MPC-E120", "tool_unsupported", Session, 2, "K-PAR-7", RUN_TOOL,
    "{flag}: not supported ({hint})",
    "That verb, flag or syntax is outside the supported subset; nothing is approximated. \
     Use what the hint says, or see capabilities for each verb's flags. Nothing was \
     applied.")
    .step("capabilities", &[],
        "That verb, flag or syntax is outside the supported subset; use what the hint says, or \
         look up each verb's flags.");

TOOL_REFUSED = e("MPC-E121", "tool_refused", Session, 2, "K-PAR-7, K-SEC-6", RUN_TOOL,
    "{what}: refused ({reason})",
    "The tools refuse this on purpose (sed commands that touch other files or run \
     programs, backup suffixes, backreferences in a pattern). Do it another way: the reason \
     says which. Nothing was applied.");

TOOL_USAGE = e("MPC-E122", "tool_usage", Session, 2, "K-EXIT-1", RUN_TOOL,
    "{reason}",
    "The command line is malformed: a missing operand, a bad number, a bad script, pattern \
     or patch (the message says which). Fix it and run again; nothing was applied.");

PATCH_FAILED = e("MPC-E123", "patch_failed", Session, 1, "K-PAR-8", &["patch", "apply"],
    "{failed} out of {hunks} hunk(s) FAILED; nothing applied",
    "The patch doesn't fit the live text (others changed those lines, or it was made \
     against another version). Nothing was applied: patches are all or nothing. stdout \
     shows each failed hunk beside the live lines. Read the text again and make a new \
     patch, or apply the rest with --partial.")
    .step("cat", &["-n"],
        "The patch doesn't fit the live text, so nothing was applied; read the text again and \
         make a new patch, or apply only the hunks that fit.");

PATCH_REJECTS = e("MPC-E124", "patch_rejects", Session, 1, "K-PAR-8, K-BR-9",
    &["patch", "apply"],
    "{failed} out of {hunks} hunk(s) FAILED; the rest applied (--partial)",
    "--partial applied the hunks that fit; the failed ones (on stdout) were not applied. \
     Read the lines they were for and make those changes again on the live text.")
    .step("cat", &["-n"],
        "The hunks that fit were applied and the failed ones were not; read the lines they were \
         for and make those changes again on the live text.");

REPLACE_NO_MATCH = e("MPC-E125", "replace_no_match", Session, 1, "exact replace",
    &["replace"],
    "replace: --old matches nothing in {doc}; nothing applied",
    "The exact text in --old isn't in the doc (it matches byte for byte, with \\n line \
     breaks). stdout lists near misses. Read the lines again (cat -n, grep -n) and copy the \
     text exactly; nothing was applied.")
    .step("cat", &["-n"],
        "The exact old text isn't in the doc; read the lines again and copy the text byte for \
         byte.");

REPLACE_NOT_UNIQUE = e("MPC-E126", "replace_not_unique", Session, 1, "exact replace",
    &["replace"],
    "replace: --old matches {matches} times in {doc}; nothing applied",
    "--old must match exactly once. Add surrounding text to --old until it matches once, or \
     pass --all to replace every match. stdout lists where each match is; nothing was \
     applied.")
    .step("cat", &["-n"],
        "The old text matches more than once; add surrounding text until it matches once, or \
         replace every match.");

PACK_VERB_NO = e("MPC-E127", "pack_verb_no", Session, 1, "pack verbs", &["@pack"],
    "{verb}: nothing applied",
    "The file type's own verb found nothing to change (stdout says why: e.g. no such \
     section). Check its target (cat, grep -n) and run it again; nothing was applied.")
    .step("cat", &[],
        "The file type's own verb found nothing to change; check its target and run it again.");

// 130–139: approvals

HELD_BACKLOG = e("MPC-E130", "held_backlog", Session, 2, "K-HIST-23, K-ID-10", CHANGE,
    "held: approvals_backlog",
    "Too many of your changes made for a person are waiting for their approval, so new \
     ones are held (nothing applied). This is refused for now, not for good: don't retry in \
     a loop. Ask the person to approve or decline the pending changes (log --pending lists \
     them), then run the verb again.")
    .step("log --pending", &[],
        "Too many of your changes wait for a person's approval, so new ones are held for now; ask \
         that person to settle the pending ones, then run the verb again. Don't retry in a loop.");

NO_APPROVAL = e("MPC-E131", "no_approval", Session, 4, "K-ID-11", &["approve", "decline"],
    "change {seq} has no approval (it wasn't made on anyone's behalf)",
    "That change isn't waiting for anyone's approval (it wasn't made on someone's behalf, \
     or doesn't exist). List the pending ones with log --pending and use one of those.")
    .step("log --pending", &[],
        "That change isn't waiting for anyone's approval; list the pending ones and use one of \
         those.");

BAD_APPROVAL = e("MPC-E132", "bad_approval", Session, 2, "K-ID-11", &["approve", "decline"],
    "bad approval: {reason}",
    "An approval is settled once: pending → approved or declined. This one is already \
     settled (the message says how). To reverse a declined change, use revert or restore \
     --version.")
    .step("log --pending", &["--all"],
        "That approval is already settled; list the settled ones to see how, and revert or \
         restore to reverse a declined change.");

// 140–159: the sync wire and the doc model

DECODE = e("MPC-E140", "decode", Session, 2, "K-SEC-2", SYNC,
    "update could not be decoded: {reason}",
    "An editor sent bytes the kit can't decode; nothing was applied. The editor should \
     resync (reload the doc).")
    .indirect("the sync wire: an editor's update bytes");

FOREIGN_CLIENT = e("MPC-E141", "foreign_client", Session, 2, "K-ID-5", SYNC,
    "update inserts as client {found}, but this connection is client {allowed}",
    "An update inserted under another participant's client id and was refused whole. The \
     editor should reload; it can't author as someone else.")
    .indirect("the sync wire: an editor's update bytes");

MISSING_DEPS = e("MPC-E142", "missing_deps", Session, 2, "K-SEC-17", SYNC,
    "update depends on changes this doc doesn't have (client {client} from clock {from}, \
     doc has {have}); resync",
    "The update builds on changes this doc never received. The editor should resync (send \
     its state vector, then the missing changes).")
    .indirect("the sync wire: an editor's update bytes");

PENDING_DEPS = e("MPC-E143", "pending_deps", Session, 2, "K-SEC-15", SYNC,
    "update left unresolved dependencies after apply; the doc needs a reload or resync",
    "The in-memory doc refused the update and reloads from the store on its own. Retry \
     after a moment; the editor should resync.")
    .indirect("the sync wire: an editor's update bytes");

APPLY_FAILED = e("MPC-E144", "apply_failed", Session, 2, "K-SEC-2", SYNC,
    "update could not be applied: {reason}",
    "The engine refused the update; nothing was applied. The editor should resync.")
    .indirect("the sync wire: an editor's update bytes");

ENGINE_PANIC = e("MPC-E145", "engine_panic", Session, 2, "K-SEC-4", SYNC,
    "the engine panicked on this input",
    "The engine failed on this input and it was refused; the doc is unchanged. The editor \
     should resync. Report it: it is a bug.")
    .indirect("the sync wire: an engine failure");

LOAD_FAILED = e("MPC-E146", "load_failed", Session, 2, "K-SEC-5", ANY,
    "state could not be loaded: {reason}",
    "A stored state couldn't be loaded into the engine. Nothing changed; report it to the \
     host's operator.")
    .indirect("a damaged stored state");

BAD_CLIENT = e("MPC-E147", "bad_client", Session, 2, "K-ID-2, K-ID-3", SYNC,
    "invalid client id {client}",
    "A client id is out of range or reserved. The editor should reconnect for a fresh one.")
    .indirect("the sync wire: a client id the host passes");

NO_ROOT = e("MPC-E148", "no_root", Session, 2, "K-SCH-2", &["(host)"],
    "schema {schema} has no root {root}",
    "The edit named a part of the doc this file type doesn't have. Text verbs need no root; \
     a host passing one should use the pack's root names.");

WRONG_KIND = e("MPC-E149", "wrong_kind", Session, 2, "K-SCH-2", &["(host)"],
    "root {root} is {actual}, not {wanted}",
    "The edit treats a part of the doc as the wrong kind (text vs table...). Use the verbs \
     for that kind (capabilities lists them).")
    .indirect("a pack with a non-text root")
    .step("capabilities", &[],
        "The edit treats a part of the doc as the wrong kind; use the verbs for that kind.");

UNSUPPORTED_KIND = e("MPC-E150", "unsupported_kind", Session, 2, "K-SCH-1", &["(host)"],
    "primitive {kind} is not implemented yet",
    "This file type needs a primitive this build doesn't have; it can't open live.")
    .indirect("a pack declaring an unimplemented kind");

BAD_HUNK = e("MPC-E151", "bad_hunk", Session, 2, "K-SCH-2", &["(host)"],
    "{reason}",
    "An edit's ranges are out of order, out of range, off a character boundary or empty. \
     The host computes ranges from the text it read: read again and recompute.");

UNACCEPTED = e("MPC-E152", "unaccepted", Session, 3, "K-SEC-2", SYNC,
    "update contains content this host doesn't accept",
    "An editor sent an update this build does not accept; it was refused whole and nothing \
     was applied. Don't resend it: the editor should reload the doc. Report it if it \
     happens again.")
    .indirect("the sync wire: an update this build does not accept");

BAD_ROOT_NAME = e("MPC-E153", "bad_root_name", Session, 2, "K-SCH-1", &["(host)"],
    "schema root name {root}: names must be non-empty, unique and free of '/'",
    "A pack's schema names a root the kit can't store (empty, holding '/', or twice). The \
     pack has to fix its schema; this file type can't open live until then.")
    .indirect("a pack with a bad schema");

MALFORMED_DOC = e("MPC-E154", "malformed_doc", Session, 2, "K-SEC-5", ANY,
    "the doc's state is malformed: {reason}",
    "The doc holds something the kit never writes (a bug, or a damaged store). Nothing was \
     applied. Don't retry: report it to the host's operator with this message.")
    .indirect("a damaged doc state");

// 160–169: the catalog itself and the engine's seam

UNKNOWN_ID = e("MPC-E160", "unknown_error_id", Session, 4, "the catalog (docs/ERRORS.md)",
    &["explain"],
    "no error id {id}",
    "That id isn't in this build's catalog. Run explain with no id to list every id, or \
     check the id you copied (MPC-E and three digits).")
    .step("explain", &[],
        "That id isn't in this build's catalog; list every id, or check the one you copied.");

ENGINE_UNLISTED = e("MPC-E161", "engine_unlisted", Session, 2, "the MPCE seam (stable codes)",
    ANY,
    "the engine refused this ({code}), and this build's catalog doesn't list the code",
    "A newer engine refused something this kit build can't explain. Nothing was applied. \
     Report the code; the engine's own docs describe it.")
    .reserved("the native engine (its own MPCE ranges)");

ROOT_KIND_NOT_NATIVE = e("MPC-E162", "root_kind_not_native", Session, 2,
    "the native engine: text roots only", &["(host)"],
    "root {root} is {kind}: this file type isn't switched to the native engine yet",
    "The host chose the native engine for a file type with a list or map root. Nothing was \
     made or loaded. This file type isn't switched to the native engine yet: open it on Yrs.")
    .indirect("a host choosing the native engine for a file type with a list or map root \
     (feature native)");

REPLICA_LAGGING = e("MPC-E163", "replica_lagging", Session, 2,
    "the MPCE seam (stable codes): one host, one engine version", SYNC,
    "this replica's engine refused the host's merge: it is behind the host's engine",
    "The host relayed a change this replica's engine can't take (MPCE-E995 from the host): \
     this replica runs an older engine than the host. Nothing was applied. Upgrade this \
     replica, then resync it from the host; don't treat the host as hostile.")
    .indirect("a replica behind the host's engine (feature native)");

VECTOR_NEEDS_PARTS = e("MPC-E164", "vector_needs_parts", Session, 2,
    "the MPCE seam (stable codes): a vector is never sent smaller or empty", SYNC,
    "the version vector ({entries} writers) doesn't fit one frame: send it in parts",
    "Nothing was sent. The doc's version vector is too large for one frame: send it as \
     parts (the kit's state_vector_parts), and the peer answers each with diff parts. Never \
     send a smaller or empty vector.")
    .indirect("a native doc with about 10,000 writers or more (feature native)");

NEEDS_STATE_TRANSFER = e("MPC-E165", "needs_state_transfer", Session, 2,
    "the MPCE seam (stable codes): a replica behind a cut", SYNC,
    "the peer's history is compacted past this doc's state: it needs a state transfer",
    "Nothing was applied. The peer has compacted the history this doc asked for, so no \
     diff can bring it up to date: it needs the peer's state (a state transfer). Don't retry \
     the catch-up and don't close the peer: it isn't at fault.")
    .indirect("a native replica behind a peer's cut (feature native)");

NATIVE_ONE_REPLICA = e("MPC-E166", "native_one_replica", Session, 2,
    "the native engine (a Beta): one replica, the host's", SYNC,
    "this doc is on the native engine (a Beta), which keeps one copy, on the host; it can't be \
     synced to another copy",
    "Nothing was sent or applied. People and agents edit this doc through the host's verbs, \
     on the host's one copy. To sync copies of it, switch the doc back to the standard engine.")
    .indirect("sync bytes for a native doc: a second replica (feature native)");

NATIVE_BETA_OFF = e("MPC-E167", "native_beta_off", Session, 2,
    "the native engine (a Beta): off by default, chosen only when a doc is made", HOST,
    "the native engine (a Beta) is off on this host: a new doc can't be made on it",
    "Nothing was made. Make the doc on the standard engine, or ask the host's owner to turn the \
     native engine Beta on. Docs already on the native engine open and save as usual.")
    .indirect("a host making a doc on the native engine with the Beta off (feature native)");

NATIVE_NO_YJS = e("MPC-E168", "native_no_yjs", Session, 2,
    "the native engine (a Beta): no Yjs sync", SYNC,
    "this doc is on the native engine; Yjs sync bytes can't reach it",
    "Nothing was applied. A doc on the native engine takes no Yjs sync: edit it through the \
     host's verbs, or switch the doc back to the standard engine to sync it with Yjs.")
    .indirect("Yjs sync bytes for a native doc (feature native)");

NATIVE_ENGINE_MISSING = e("MPC-E169", "native_engine_missing", Session, 6,
    "the native engine: a build without it refuses its docs one by one", ANY,
    "this doc is on the native engine (format {stored_format}), which this build \
     doesn't have",
    "The doc was made on the native engine, and this build doesn't include that engine, so the \
     doc was not opened. Nothing in it changed. The store and its other docs work as usual. \
     Open this doc with a build that has the native engine; don't retry here.")
    .indirect("a native doc in a store opened by a build without the native engine");

// 170–179: addresses and cursors (src/addr.rs)

AMBIGUOUS_ADDRESS = e("MPC-E170", "ambiguous_address", Session, 2, "K-ADDR-2, K-EXIT-1",
    &["@pack"],
    "{given} names more than one {what}",
    "The address you gave names more than one thing (a name or a value two of them share), \
     so nothing was read or applied; the kit never picks the first one. details.candidates \
     lists each one's own address (its key or ID) and where it is now: run the verb again \
     with the address of the one you mean.");

POSITIONAL_NEEDS_VERSION = e("MPC-E171", "positional_needs_version", Session, 2, "K-ADDR-3",
    CHANGE,
    "{address} is a position; an agent's edit by position names the version it read",
    "Positions (#N, LN, line numbers, the Nth match) move when others edit, so an agent's \
     edit that uses one must say which version it counted them in. Nothing was applied. \
     Read the doc again (the read prints its version) and run the edit with --if-version \
     <that version> (a grid edit: --base <that version>), or name the thing by its key.");

ADDRESS_NOT_FOUND = e("MPC-E172", "address_not_found", Session, 4, "K-READ-1, K-READ-3",
    &["@pack", "@index"],
    "{count} of the addresses name nothing in version {version}",
    "Some of the addresses you asked for name nothing in this version. The output still \
     lists every address, with null for each missing one; details.missing gives each \
     missing address with what does exist there (candidates, its parent's children). Check \
     them against outline, or ask for one of the candidates.");

STALE_CURSOR = e("MPC-E173", "stale_cursor", Session, 1, "K-READ-2",
    &["@pack", "@index"],
    "the cursor is for another version, doc or call; nothing was read",
    "A cursor continues one read of one doc at one version with the same arguments. The doc \
     changed since (or the cursor is from another doc, or the arguments differ), so nothing \
     was read. Read again without --cursor and page on with the cursor that read returns.");

BAD_CURSOR = e("MPC-E174", "bad_cursor", Session, 2, "K-READ-2",
    &["@pack", "@index"],
    "not a cursor ({reason}); nothing was read",
    "A cursor is the opaque string a paged read printed after truncated: true, passed back \
     unchanged. This one isn't such a string (cut short, changed, or from somewhere else), so \
     nothing was read. Read again without --cursor and page on with the cursor it returns.");

OVER_BUDGET = e("MPC-E175", "over_budget", Session, 2, "K-READ-2",
    &["@pack", "@index"],
    "the result is {bytes} bytes, over the {budget}-byte read budget; --no-truncate refuses a page",
    "With --no-truncate a read returns everything or nothing, and everything is more than one \
     read may return. Nothing was read. Read without --no-truncate and page on with the \
     cursor each page returns, or narrow the read (a depth, a kind, fewer addresses).");

LINE_ADDRESS_UNRELIABLE = e("MPC-E176", "line_address_unreliable", Session, 2, "K-ADDR-4, K-PAR-7",
    &["@pack"],
    "{address}: this file holds a lone CR, so its line numbers have no one meaning; name the row by its key or #N (row list shows both)",
    "A lone CR (a carriage return with no line feed after it, inside a value or ending the \
     records) ends a line for the table's rendered lines but not for the tools that read like \
     cat -n, so an LN address could name a row other than the one you read. Nothing was read or \
     applied. Name the row by its key (r17) or its #N: row list shows both for every row.")
    .step("row list", &[],
        "Line numbers have no one meaning in this file, so list the rows to get each one's key \
         and position, then address the row by one of those.");

LINE_ADDRESS_UNKNOWN = e("MPC-E177", "line_address_unknown", Session, 2, "K-ADDR-4",
    &["claim", "@pack"],
    "{address}: the file can't be rendered now, so its line numbers can't be checked; name the row by its key or #N",
    "The pack can't render the file at the moment, so the kit can't tell whether it holds a lone \
     CR, and a line number might name a row other than the one you read. Nothing was read or \
     applied. Name the row by its key (r17) or its #N, which don't depend on rendering; blame \
     --structure lists both. A line number works again once the file renders.")
    .step("blame --structure", &[],
        "Line numbers can't be checked while the file can't be rendered, so list the rows to get \
         each one's key and position, then address the row by one of those.");

// 180–189: the version a person's app opened (`note_opened`, a host call)

OPENED_UNKNOWN = e("MPC-E180", "opened_version_unknown", Session, 4,
    "outside saves: the version the app opened", HOST,
    "no version {version} of this doc",
    "The host named a version this doc never had. Nothing was noted: a save still merges as \
     before. Note the file as it is now (FileNow), or name a version a read printed.")
    .indirect("a host's note_opened call (tests/opened.rs)");

OPENED_GONE = e("MPC-E181", "opened_version_gone", Session, 8,
    "outside saves: the version the app opened", HOST,
    "version {version} is gone (purged or not kept)",
    "The version was purged or isn't kept, so a save can't merge from it. Nothing was noted. \
     Note the file as it is now (FileNow); never retry with the same version.")
    .indirect("a host's note_opened call (tests/opened.rs)");

OPENED_NO_FILE = e("MPC-E182", "opened_no_file", Session, 2,
    "outside saves: the version the app opened", HOST,
    "this doc has no file",
    "Only a doc with a file sees an app's saves, so there is nothing to note. Nothing \
     changed.")
    .indirect("a host's note_opened call (tests/opened.rs)");

NATIVE_EQUIVOCATION = e("MPC-E183", "native_equivocation", Session, 2,
    "the native engine (a Beta): one line per writer", SYNC,
    "this update reuses writer {agent}'s change {seq} with other content",
    "Nothing was applied: a change this copy already holds came again with other content, so \
     the copy that sent it continued that writer from an older point (a store restored or \
     copied without rotating its writers). Drop the update, keep this copy as it is, and resync \
     the sender from the host's store; don't retry the same bytes.")
    .indirect("a native update re-emitting a known change with other content (feature native)");

// ---------------------------------------------------------------- 2xx history and undo

UNDO_CONFLICT = e("MPC-E201", "undo_conflict", History, 1,
    "U13, U11, hist.undo_atomic", &["undo", "redo", "revert"],
    "step {step} conflicts in {count} region(s) at {where}; nothing applied",
    "Someone else changed text your change touched, so undoing it whole would overwrite \
     their work. Nothing was applied. Either run undo --partial (undoes the regions nobody \
     touched and reports the rest; revert --partial for revert), or read the lines in \
     details.conflicts (cat -n) and fix them by hand with replace.")
    .step("undo --partial", &[],
        "Undoing the change whole would overwrite others' work; undo only the regions nobody \
         touched, or fix the conflicting lines by hand.");

UNDO_BLOCKED = e("MPC-E202", "undo_blocked", History, 1,
    "U14, U15, hist.undo_partial", &["undo --partial", "revert"],
    "every region of the top step conflicts ({count} at {where}); nothing applied",
    "Others changed every region of your newest change, so nothing could be undone and it \
     stays on top of your undo history. Fix those lines by hand (replace --lines), then \
     skip the step with undo --drop to reach older ones.")
    .step("undo --drop", &[],
        "Others changed every region of your newest change; fix those lines by hand, then skip \
         the step to reach older ones.");

UNDO_SKIPPED = e("MPC-E203", "undo_skipped", History, 1, "U14, hist.undo_partial",
    &["undo --partial", "revert"],
    "undid {steps} step(s); {skipped} region(s) were left alone because others changed \
     them: {where}",
    "The regions nobody touched were undone; the ones listed were not (others changed \
     them). Read those lines (cat -n) and decide by hand: replace --lines to finish them, or \
     leave them.")
    .step("cat", &["-n"],
        "The regions others changed were left alone; read those lines and finish them by hand, or \
         leave them.");

REVERT_TOO_OLD = e("MPC-E204", "revert_too_old", History, 8, "K-HIST-23, K-EXIT-1",
    &["revert"],
    "change {seq} is too old to revert (revert_too_old); use restore --version",
    "The change is outside the revert window. Never retry the same request. Find a version \
     from before it (log) and restore --version that version, or edit the text by hand.")
    .step("log", &[],
        "The change is outside the revert window; find a version from before it and restore that \
         version. Never retry the same request.");

NO_SUCH_CHANGE = e("MPC-E205", "no_such_change", History, 4, "K-HIST-19, K-HIST-9",
    &["revert", "restore", "show --version", "diff --version", "version --name"],
    "no change or version {seq} in this doc",
    "There's no change or version by that number. List them with log (changes and \
     versions, newest first) and use one it prints.")
    .step("log", &[],
        "There's no change or version by that number; list them and use one shown.");

HISTORY_LOST = e("MPC-E206", "history_lost", History, 0, "U6, U18, K-HIST-17", UNDO,
    "nothing to undo: your undo history was lost when the doc reloaded",
    "Undo history lives in memory and was dropped when the doc unloaded or the daemon \
     restarted. Your changes are still in the log: find one with log --by <you> and \
     reverse it with revert <change>.")
    .step("log", &[],
        "Undo history was lost when the doc reloaded, but your changes are still in the log; find \
         yours and revert it.");

CLEARED_BY_PURGE = e("MPC-E207", "cleared_by_purge", History, 0, "U19, K-PURGE-4", UNDO,
    "nothing to undo: a purge cleared the undo history",
    "An owner purged content from this doc, which clears every undo history. Reverse a \
     change with revert <change> (log lists them), or edit the text by hand.")
    .step("log", &[],
        "A purge cleared every undo history; find the change in the log and revert it, or edit \
         the text by hand.");

VERSION_NO_CARRIER = e("MPC-E208", "version_no_carrier", History, 4, "the carrier's bytes",
    &["show --version", "(host)"],
    "version {version} has no copy of its file's own bytes (version_no_carrier)",
    "This file type is written back by splicing into the file's own bytes, and the kit keeps \
     those bytes with each version it cuts. This version has none: it was cut before the doc \
     kept them (by an older kit, or before this build), or a purge erased them. Its file \
     can't be rebuilt exactly, so none is given. Its content is still there: read it with \
     show --version; for the file as it is now, read the file.")
    .step("log", &[],
        "This version's file can't be rebuilt exactly; its content is still readable, and a \
         later version or the file now may serve instead.");

// ---------------------------------------------------------------- 3xx presence and claims

CLAIMED = e("MPC-E301", "claimed", Presence, 5, "claims, K-PRES-12",
    &["claim", "@change"],
    "{lines} claimed by {name} ({by}); {remaining_ms} ms left",
    "Someone else is working on those lines right now (a live claim). Nothing was applied. \
     Wait and retry after details.remaining_ms, or work on other lines; activity and who \
     show who is where.")
    .step("activity", &[],
        "Someone else holds a live claim on those lines; see who is where, then wait for the time \
         left and retry, or work on other lines.");

NO_CLAIM = e("MPC-E302", "no_claim", Presence, 4, "claims",
    &["claim --renew", "claim --release"],
    "no live claim {claim}",
    "That claim already ended (it expired, or your edit ended it), or you hold none. Take a \
     new one with claim --lines a,b if you still need it.")
    .step("claim", &[],
        "That claim already ended, or you hold none; take a new one if you still need it.");

CLAIM_CONTENDED = e("MPC-E303", "claim_contended", Presence, 2, "claims",
    &["claim --renew"],
    "{claim} has {waiters} waiting; it can't be renewed or held ({remaining_ms} ms left)",
    "Others are waiting for your claim, so it can't be extended. Finish your edit before it \
     ends (details.remaining_ms), or release it with claim --release.")
    .step("claim --release", &["--id", "{claim}"],
        "Others are waiting for your claim, so it can't be extended; finish your edit before it \
         ends, or release it.");

BAD_CLAIM = e("MPC-E304", "bad_claim", Presence, 2, "claims",
    &["claim", "claim --renew", "claim --release"],
    "bad claim: {reason}",
    "The claim call isn't valid as given (the message says why: not your claim, a zero \
     ttl, several claims and no --id...). Fix the arguments; claim --release with no id \
     releases all of yours.")
    .step("claim --release", &[],
        "The claim call isn't valid as given; fix its arguments, or release all of your claims.");

WATCHING = e("MPC-E305", "watching", Presence, 3, "the participant cap (participants_max)", CHANGE,
    "watching: the doc has its {max} participants; read-only until the host lets you in",
    "This doc already has as many participants as it allows, so you joined as a watcher: \
     you can read it and follow changes, but not edit or show presence. This is refused for \
     now, not for good: don't retry in a loop. When someone leaves, the host can let you in; \
     until then, read.")
    .step("cat", &[],
        "The doc has all the participants it allows, so you joined as a watcher; read it until \
         the host lets you in, and don't retry in a loop.");

HOLD_UNINDEXABLE = e("MPC-E306", "hold_unindexable", Presence, 5,
    "claims on nodes: an agent's whole-file write",
    &["write", "(host)"],
    "the write can't be checked against the holds on this doc ({why}); nothing applied",
    "Others hold nodes in this doc (details.holders), and the file type's index couldn't read \
     your new version within its limits (details.why), so the kit can't tell which nodes it \
     changes. Nothing was applied. Make the change through the file type's verbs, which are \
     checked node by node, or ask the holders to release their holds and write again.")
    .step("activity", &[],
        "Others hold nodes in this doc and your write couldn't be checked against them; make \
         the change through the file type's verbs, or ask the holders to release.");

SELECTION_TOO_LARGE = e("MPC-E310", "selection_too_large", Presence, 2, "K-PRES-6",
    &["select"],
    "selection too large: {len} elements or cell ranges (the most is {max})",
    "A selection holds at most 256 elements, and a grid's cells at most 16 ranges; nothing \
     changed. Select fewer (details.max).")
    .step("select", &["--clear"],
        "A selection holds at most 256 elements and a grid's cells at most 16 ranges; clear it, \
         then select fewer.");

// 320–329: comment threads (annotations)

NO_ANNOTATION = e("MPC-E320", "no_annotation", Presence, 4, "K-ANN-1",
    &["reply", "resolve", "resolve --reopen", "comment --delete", "reanchor", "comments --log"],
    "no annotation {thread}",
    "There is no comment thread by that id in this doc (or no comment by that number on it). \
     Nothing changed. List the threads and use an id from there.")
    .step("comments", &[],
        "That thread or comment isn't in this doc; list the threads and use an id from there.");

BAD_ANNOTATION = e("MPC-E321", "bad_annotation", Presence, 2, "K-ANN-1, K-ANN-6",
    &["comment", "reply", "reanchor", "comments"],
    "bad annotation: {why}",
    "The comment call was refused as given (details.why: an empty body, a bad mention, \
     attachment or hint, a node the version doesn't have, or a key several nodes hold). \
     Nothing changed. Fix it and run it again.");

// ---------------------------------------------------------------- 4xx disk and outside edits

DECLINED_TOO_LARGE = e("MPC-E401", "declined_too_large", Disk, 4, "K-PACK-4, K-DISK-8",
    ANY,
    "live editing is off: {reason} (too_large)",
    "The file is over the size limit for live editing (capabilities shows the limits). It \
     opens the ordinary way: edit it with ordinary file tools, without live merging. Don't \
     retry the live verb.");

DECLINED_NOT_UTF8 = e("MPC-E402", "declined_not_utf8", Disk, 4, "K-PACK-4, K-COD-6", ANY,
    "live editing is off: {reason} (not_utf8)",
    "The file isn't UTF-8 text, so it can't be a live doc. Edit it with ordinary file tools, \
     or convert it to UTF-8 and open it again. Don't retry the live verb as is.");

DECLINED_BINARY = e("MPC-E403", "declined_binary", Disk, 4, "K-PACK-4, K-COD-6", ANY,
    "live editing is off: {reason} (binary)",
    "The file holds binary data (a NUL byte), so it can't be a live text doc. Edit it with \
     ordinary file tools. Don't retry the live verb.");

DECLINED_MIXED_EOL = e("MPC-E404", "declined_mixed_line_endings", Disk, 4,
    "K-PACK-4, K-COD-7", ANY,
    "live editing is off: {reason} (mixed_line_endings)",
    "The file mixes line endings (\\n and \\r\\n), which a live doc can't keep byte for byte, \
     so it isn't live. Either edit it with ordinary file tools, or make its line endings \
     consistent (all \\n, or all \\r\\n) with an ordinary tool and run the verb again: it \
     then opens live.");

DECLINED_OFF = e("MPC-E405", "declined_off", Disk, 4, "K-DISK-8", CHANGE,
    "live editing is off: {reason} (declined)",
    "The file changed on disk into something that can't be a live doc (the reason says \
     what), so live editing stopped. Reads still work. Edit the file with ordinary tools, \
     or fix what the reason names; it goes live again when it decodes.")
    .step("cat", &[],
        "The file changed on disk into something that can't be live; reads still work, and it \
         goes live again once it decodes.");

DECLINED_UNREADABLE = e("MPC-E406", "declined_unreadable", Disk, 4, "K-PACK-4", &["(host)"],
    "live editing is off: {reason} (unreadable)",
    "The file couldn't be read to promote it to a live text doc. It stays a plain file; \
     check it exists and is readable.")
    .indirect("a promotion whose file read fails");

DECLINED_PACK = e("MPC-E407", "declined_pack", Disk, 4, "K-PACK-4, K-COD-6", ANY,
    "live editing is off: {reason} ({code})",
    "This file type declined the file (details.code says why, e.g. a CSV it can't keep \
     byte for byte). It opens the ordinary way: edit it with ordinary file tools. Don't \
     retry the live verb as is.")
    .indirect("a pack's own decline code (the kit's text pack uses 401–404)");

DISK = e("MPC-E410", "disk", Disk, 1, "K-DISK-2, K-DISK-10", &["flush", "mv", "(host)"],
    "disk: {reason}",
    "Reading or writing the file failed. Nothing is lost: the doc keeps its changes and \
     write-back retries. Check the disk (space, permissions), then flush.")
    .indirect("an I/O failure")
    .step("flush", &[],
        "Reading or writing the file failed, but the doc keeps its changes; check the disk's \
         space and permissions, then write it back.");

BAD_PATH = e("MPC-E411", "bad_path", Disk, 2, "K-SEC-6, K-DISK-14", &["mv", "(host)"],
    "bad path: {reason}",
    "The path isn't a regular file inside the workspace (a folder, a link pointing outside \
     it...). Use a path to a regular file inside the workspace.");

DECLINED_HARD_LINKED = e("MPC-E412", "declined_hard_linked", Disk, 4, "K-DISK-14 (hard links)",
    &["(host)"],
    "live editing is off: {reason} (hard_linked)",
    "The file has more than one name (a hard link), so edits made live through one name \
     would change the others unseen. It opens the ordinary way: edit it with ordinary file \
     tools, or give it a single name (copy it, remove the extra link) and open it again.");

// a host's record of a save it made itself (BlobSession::expect_save, record_saved)
SAVE_MISMATCH = e("MPC-E413", "save_mismatch", Disk, 1, "K-T0-5, K-DISK-2", HOST,
    "the file doesn't hold the bytes saved ({size} bytes given, {file_size} on disk); nothing recorded",
    "The host recorded a save it made, but the file holds other bytes (details.file_size: what \
     it holds, null when there is no file): another program saved after it, or the save \
     didn't land. Nothing was recorded and the file wasn't touched. Let the file's own change \
     be taken in as an outside save, or save again and record that.");

SAVE_TICKET_INVALID = e("MPC-E414", "save_ticket_invalid", Disk, 2, "K-T0-5", HOST,
    "bad save ticket: a ticket is good for one record_saved, by the connection that asked for it, on its doc",
    "A host bug: a save ticket is good for one record_saved, by the connection that asked for \
     it, on the doc that issued it. This one was already recorded (or refused), is another \
     connection's, or isn't this doc's (or is too old to be remembered). Nothing was recorded. \
     A lapsed, replaced or reloaded ticket is not this: it returns with not_recorded set.");

// a keyed doc that isn't a table, over its live size limit (`Config::keyed_limit`)
DECLINED_LIVE_TOO_LARGE = e("MPC-E415", "declined_live_too_large", Disk, 4,
    "K-PACK-4, K-DISK-8, K-CSV-7", ANY,
    "live editing is off: {reason} (live_too_large)",
    "The file holds more elements and fields than a live doc of its type may hold \
     (details.limit; details.limit_source says whose limit it is: host_per_pack, the host's \
     for this file type; pack_default, the file type's own default; or host_wide, the host's \
     for every file type). It isn't live: it stays at tier 0 (or 0.5 where \
     its type merges whole files) and opens the ordinary way, so its file verbs and ordinary \
     file tools work on it, with nothing merged live. Don't retry the live verb.")
    .step("cat", &[],
        "The file is over the live size limit, so it stays at tier 0 (or 0.5) and opens the \
         ordinary way; read it there, and change it with its file verbs or ordinary file tools.");

// ---------------------------------------------------------------- 5xx store

STORE_FAILED = e("MPC-E501", "store_failed", Store, 6, "K-OFF-7", ANY,
    "store: {reason}",
    "The doc store failed (disk full, fsync failed). Nothing was acknowledged or applied. \
     Retry later; if it persists, the host's operator must free space or fix the disk.")
    .indirect("a store I/O failure");

STORE_NEWER = e("MPC-E502", "store_newer", Store, 6, "K-FMT-1, K-FMT-10", ANY,
    "store is format {found}; this build reads up to {max}",
    "The store was written by a newer build. Nothing changed. Use the newer build.")
    .indirect("a store from a newer build");

STORE_CORRUPT = e("MPC-E503", "store_corrupt", Store, 6, "K-SEC-5", ANY,
    "store record is damaged: {what}",
    "A stored record is damaged; nothing was applied. Report it to the host's operator \
     (a backup or the file itself may be needed).")
    .indirect("a damaged store record");

PATH_TAKEN = e("MPC-E504", "path_taken", Store, 6, "K-DISK-14", &["mv"],
    "path already has a doc: {key}",
    "Another live doc already has that path. Pick a path no doc uses, or move that doc \
     first.");

STORE_LOCKED = e("MPC-E505", "store_locked", Store, 6, "K-OFF-2", ANY,
    "Another process has this document open: its store is held by another opener",
    "Another process has this document open. Nothing was read or changed. Close it there (or \
     let its hand-off finish), then open it again; a host can open with a wait instead \
     (DocStore::open_with_retry), which takes the store once the other opener lets it go.")
    .indirect("a second opener of a store another holds (another process, or this one)");

NAMED_VERSION = e("MPC-E506", "named_version", Store, 6, "K-HIST-6", &["(host)"],
    "version {version} of doc {doc} is named; retention never deletes named versions",
    "Named versions are never deleted by retention. Nothing changed.")
    .indirect("retention asked to drop a named version (a kit bug)");

KEY_CHANGE = e("MPC-E507", "key_change", Store, 6, "K-DISK-14", &["(host)"],
    "put_meta can't change a doc's key; use rekey",
    "A host call tried to change a doc's path the wrong way; nothing changed. Hosts use \
     rekey (mv).")
    .indirect("a store misuse (a kit bug)");

LOG_MOVED = e("MPC-E508", "log_moved", Store, 6, "K-OFF-1", ANY,
    "doc {doc}'s log moved under the session: expected position {expected}, found {found}",
    "The doc's log changed under the session (another process wrote it). Nothing was \
     applied; the doc reloads. Retry.")
    .indirect("a second writer of one store");

NO_RANDOM = e("MPC-E509", "no_random", Store, 6, "K-T0-4, K-PURGE-13", ANY,
    "no randomness source: {reason}",
    "The system's randomness source is unavailable, so no key could be made. Report it to \
     the host's operator.")
    .indirect("no /dev/urandom");

STORE_NO_DOC = e("MPC-E510", "store_no_doc", Store, 4, "K-EXIT-1", ANY,
    "no doc {doc}",
    "The stored doc is gone (deleted meanwhile). Open the path again.")
    .indirect("a doc deleted between calls")
    .step("open", &[],
        "The stored doc was deleted meanwhile; open the path again.");

STORE_NO_VERSION = e("MPC-E511", "store_no_version", Store, 4, "K-HIST-9",
    &["show --version", "restore", "version --name"],
    "no version {version} for doc {doc}",
    "There's no such version. List versions with log and use one it prints.")
    .step("log", &[],
        "There's no such version; list the versions and use one shown.");

STORE_NO_APPROVAL = e("MPC-E512", "store_no_approval", Store, 4, "K-ID-11",
    &["approve", "decline"],
    "no approval for change {seq} of doc {doc}",
    "That change has no approval record. List pending ones with log --pending.")
    .indirect("an approval record removed between calls")
    .step("log --pending", &[],
        "That change has no approval record; list the pending ones.");

UNLOADED = e("MPC-E513", "unloaded", Store, 1, "K-PERF-7", ANY,
    "session unloaded; open the doc again",
    "The doc was unloaded while you held it (idle, or memory pressure). Run the verb \
     again: it reopens the doc. Nothing was applied.")
    .step("open", &[],
        "The doc was unloaded while you held it; open it again, then run your verb. Nothing was \
         applied.");

RELEASED = e("MPC-E514", "released", Store, 1, "K-OFF-2", ANY,
    "sessions released",
    "The daemon is handing over to a new process. Retry in a few seconds. Nothing was \
     applied.");

BROKEN = e("MPC-E515", "broken", Store, 1, "K-OFF-7", ANY,
    "session failed: {reason}",
    "A store write failed and the doc couldn't reload. Retry once the store is healthy; \
     nothing acknowledged is lost.")
    .indirect("a store failure during reload");

DELETED = e("MPC-E516", "deleted", Store, 4, "K-PURGE-8", ANY,
    "the doc was deleted",
    "The doc was deleted while you were working on it. Nothing was applied. Check whether \
     the file should exist; open it again to start a new doc.")
    .step("open", &[],
        "The doc was deleted while you worked on it; check whether the file should exist, and \
         open it again to start a new doc.");

PACK_ERROR = e("MPC-E517", "pack_error", Store, 1, "K-COD-2", ANY,
    "pack: {reason}",
    "The file type couldn't write the doc back (a pack bug or damaged settings). Nothing \
     was lost from the doc. Report it.")
    .indirect("a pack bug or damaged codec settings");

QUARANTINED = e("MPC-E518", "quarantined", Store, 3, "K-SEC-5", ANY,
    "stored doc contains content this host doesn't accept; it was not loaded",
    "A stored doc holds content this build does not accept, so it was set aside for review \
     and not loaded. Nothing changed. Don't retry: report it to the host's operator.")
    .indirect("a stored doc set aside for review");

CLOSED = e("MPC-E519", "closed", Store, 1, "K-OFF-2", ANY,
    "the host closed this doc's live session ({reason})",
    "The host closed this doc's live session (its reason code says why: a file type switched \
     off, for instance). Everything acknowledged was kept and written to the file. The host \
     decides when it opens again.");

ENGINE_MISMATCH = e("MPC-E520", "engine_mismatch", Store, 6, "K-FMT-2, K-FMT-4", ANY,
    "this doc was written by engine {stored} (format {stored_format}); this kit runs engine \
     {engine} (format {engine_format})",
    "The stored doc was written by another engine than this build runs (a build on another \
     engine, or one rolled back from it), so it was not loaded. Nothing changed. Open it with \
     a build that runs that engine, or ask the host's operator to migrate it; don't retry.")
    .indirect("a doc stored by a build on another engine");

PAIRING_MISMATCH = e("MPC-E521", "pairing_mismatch", Store, 1, "the carrier's pairing", ANY,
    "the file's pairing doesn't fit it: {reason} (pairing_mismatch)",
    "The file type writes the doc back by splicing into the file's own bytes, pairing each unit \
     of the file with a key the kit kept for it. The keys kept don't fit the file, so the file \
     type can't tell which unit is which and wrote nothing rather than guess. The file is as it \
     was and the doc keeps every change. Report it; an app's next save of the file sets the \
     pairing again.")
    .indirect("a file type's pairing that doesn't fit its file");

STORE_NONCE_REUSED = e("MPC-E522", "store_nonce_reused", Store, 6, "the store nonce", ANY,
    "the engine store's nonce is already doc {doc}'s in this store; a store nonce is never \
     reused, so nothing was written",
    "Each engine store (a native doc's records) is made with its own nonce, and one store never \
     uses a nonce twice, so two engine stores under the same keys never verify each other's \
     records. This one's nonce is already another doc's here (a deleted doc's too), so nothing \
     was written, or, at open, the store holds two engine stores with one nonce and is \
     refused. Report it: the kit makes a fresh nonce for every new engine store.")
    .indirect("an engine store whose nonce this store already used");

PURGE_DOC_OPEN = e("MPC-E523", "purge_doc_open", Store, 6, "no purge of an open doc", &["purge"],
    "doc {doc} is open in another live instance; a native doc is purged only while the \
     purging instance alone holds it, so nothing was written",
    "A purge is refused, with nothing written, while any live instance other than the \
     purging one holds the doc open: that instance (another session's load of the doc, or a \
     copy of its records) would still hold the purged content in memory. Unload the doc's \
     other instances, then purge again.")
    .indirect("a purge of a native doc another live instance holds open");

CUT_PENDING_RECORDS = e("MPC-E524", "cut_pending_records", Store, 6, "a cut after the commit", HOST,
    "doc {doc} holds {changes} changes not yet committed; a native doc is cut only once its \
     changes are in the store, so nothing was cut",
    "A cut folds a native doc's engine records into a new generation, and a change whose \
     record isn't committed yet would be folded in without its log row, so nothing changed. \
     Commit the doc's pending changes (or its pending purge), then cut again.")
    .indirect("a native doc's cut asked before its pending changes were committed");

CUT_DOC_OPEN = e("MPC-E525", "cut_doc_open", Store, 6, "no cut of an open doc", HOST,
    "doc {doc} is open in another live instance; a native doc is cut only while the cutting \
     instance alone holds it, so nothing was cut",
    "A cut replaces a native doc's engine records with a new generation, and another live \
     instance of the doc (another session's load of it, or a copy of its records) still reads \
     the one it replaces, so nothing was written. Unload the doc's other instances; its next \
     idle tick or unload cuts it.")
    .indirect("a cut of a native doc another live instance holds open");

STORE_RECOVERING = e("MPC-E526", "store_recovering", Store, 1, "grouped commits", ANY,
    "the store is recovering after a commit failed at the disk; retry",
    "The store groups its commits, and one failed at the disk, so the changes not yet on disk \
     are gone from it. Until the host reopens the store at its durable state and reloads every \
     live doc from it, nothing of the store is read or written, so no one acts on changes that \
     are about to be dropped. Nothing was applied. Retry: the doc is then at its durable state, \
     and a change after it may be missing; read the doc again before writing at a base.")
    .indirect("a read or a write of a grouped store between a commit failing at the disk and \
     the host's next tick or flush");

// ---------------------------------------------------------------- 6xx erasure

PURGE_CURRENT = e("MPC-E601", "purge_current_version", Erasure, 2, "K-T0-9, K-PURGE-1",
    &["purge"],
    "version {version} of doc {doc} is its current content",
    "The current version of a file can't be purged. Write or restore another version \
     first, then purge this one.")
    .step("restore", &[],
        "A file's current version can't be purged; write or restore another version first, then \
         purge this one.");

PURGED_VERSION = e("MPC-E602", "purged_version", Erasure, 4, "K-PURGE-4",
    &["show --version", "restore"],
    "version {version} of doc {doc} was purged",
    "An owner purged that version; its content is gone for good. Pick another version \
     (log).")
    .step("log", &[],
        "That version was purged for good; pick another version.");

CACHE_KEY_DESTROYED = e("MPC-E603", "cache_key_destroyed", Erasure, 4, "K-PURGE-13",
    HOST,
    "the space key of cache generation {generation} was destroyed",
    "The space's key was destroyed (its blobs were purged), so nothing sealed under a key \
     derived from it can be read again, and no key of that generation is given again. Drop \
     the cache: delete every entry sealed under that generation (details.generation), then \
     seal new entries under the space's current key.");

// ---------------------------------------------------------------- 7xx tier 0 and blobs

LEASED = e("MPC-E701", "leased", Blob, 5, "K-T0-2", &["lease", "write", "restore"],
    "leased by {name} ({holder}) until {expires_ms}",
    "Someone else holds the file's lease (a lock while they change it). Nothing was \
     written. Retry after it expires (details.expires_ms, host clock), or ask them to \
     release it; only an owner can break it (lease --break).")
    .step("status", &[],
        "Someone else holds the file's lease; see who holds it and until when, then retry after \
         it expires or ask them to release it.");

NO_LEASE = e("MPC-E702", "no_lease", Blob, 2, "K-T0-7", &["lease --renew"],
    "no lease held",
    "You hold no lease to renew (it expired, or the daemon restarted). Take a new one with \
     lease.")
    .step("lease", &[],
        "You hold no lease to renew; take a new one.");

BLOB_TOO_LARGE = e("MPC-E703", "blob_too_large", Blob, 4, "K-T0-1", &["write", "(host)"],
    "blob is over the {limit}-byte limit",
    "The file is over the blob store's size limit, so it isn't versioned. It opens the \
     ordinary way; edit it with ordinary file tools.");

BLOB_NOT_FOUND = e("MPC-E704", "blob_not_found", Blob, 4, "K-T0-1",
    &["cat", "show --version", "restore"],
    "no blob {blob} in this space",
    "The bytes of that version are gone (collected, or the space was purged). Pick another \
     version (log).")
    .indirect("a blob file collected under a live version")
    .step("log", &[],
        "The bytes of that version are gone; pick another version.");

BLOB_DAMAGED = e("MPC-E705", "blob_damaged", Blob, 1, "K-T0-4",
    &["cat", "show --version", "restore"],
    "blob {blob} is damaged (its bytes don't match its id)",
    "The stored bytes of that version are damaged and are never served. Pick another \
     version (log), and report it.")
    .indirect("damaged blob bytes on disk")
    .step("log", &[],
        "The stored bytes of that version are damaged and never served; pick another version, and \
         report it.");

BLOB_IO = e("MPC-E706", "blob_io", Blob, 1, "K-T0-1", ANY,
    "blob i/o: {reason}",
    "The blob store couldn't read or write. Nothing was recorded. Check the disk, retry.")
    .indirect("a blob store I/O failure");

BLOB_NO_RANDOM = e("MPC-E707", "blob_no_random", Blob, 1, "K-T0-4", ANY,
    "no randomness source: {reason}",
    "No randomness source for a space key. Report it to the host's operator.")
    .indirect("no /dev/urandom");

NO_BLOB_STORE = e("MPC-E708", "no_blob_store", Blob, 4, "K-T0-1", &["(host)"],
    "no blob store configured",
    "This host has no blob store, so files without a live type can't be versioned here. \
     Edit the file with ordinary file tools.");

NO_BLOB_SPACE = e("MPC-E709", "no_blob_space", Blob, 4, "K-T0-4, K-PURGE-13", ANY,
    "no blob key for this space (never used, or purged)",
    "This workspace's blob versions were purged (or never existed). Older versions are \
     gone; the current file is unaffected.")
    .indirect("a purged blob space");

NO_BLOB = e("MPC-E710", "no_blob", Blob, 4, "K-T0-1", ANY,
    "no blob {blob}",
    "The blob store has no record of those bytes. Pick another version (log).")
    .indirect("a blob index row removed under a version")
    .step("log", &[],
        "The blob store has no record of those bytes; pick another version.");

INDEX_UNSUPPORTED = e("MPC-E711", "index_unsupported", Blob, 2, "structure index",
    INDEX_HOST,
    "this file type has no structure index",
    "This file type has no index of its structure, so there is nothing to list here. Read \
     the file itself.")
    .step("cat", &[],
        "This file type has no structure index; read the file itself.");

NOT_INDEXED = e("MPC-E712", "not_indexed", Blob, 4, "structure index", INDEX_HOST,
    "version {version} has no index: {why}",
    "That version has no index, and won't get one by waiting: it is past a limit, the file's \
     structure couldn't be read, or it was purged (details.why). The file itself is untouched. \
     Comments anchored to the whole doc still work.")
    .step("status", &[],
        "That version has no index; see the doc's state and which versions it has.");

INDEX_PENDING = e("MPC-E713", "index_pending", Blob, 1, "structure index", INDEX_HOST,
    "the index of version {version} isn't ready; retry in {retry_after_ms} ms",
    "The version's structure hasn't been read yet (a job is queued or running). Nothing \
     changed. Retry after details.retry_after_ms.")
    .step("status", &[],
        "That version's index isn't ready yet; see the doc's state, then retry after a wait.");

INDEX_BAD_RESULT = e("MPC-E714", "index_bad_result", Blob, 1, "structure index",
    &["(host)"],
    "the indexer's result was refused: {reason}",
    "The worker that read the file returned something the kit refuses: it was not trusted, \
     and nothing was recorded. Report it to the host's operator. The host may run a new job, \
     backing off between tries: the refusal isn't stored and the kit keeps no count of \
     failures, so a worker that always fails would be run again on every try.");

AGENT_WRITE_REFUSED = e("MPC-E715", "agent_write_refused", Blob, 2, "structure index",
    &["write"],
    "an agent can't write this file type",
    "Agents don't write this file type. Nothing was written. Make a new file with the \
     change and comment on the doc instead (the file type's pack says so).");

EXPORT_NEEDS_RENDERER = e("MPC-E716", "export_needs_renderer", Blob, 4, "structure index",
    INDEX_HOST,
    "a {kind} node has no stored content of its own to export",
    "This node's look is made when the file is drawn (text, a placed or adjusted node, an \
     effect), and the exporter writes only content the file stores. Nothing was written. \
     Export the whole file, or a node of another kind.")
    .indirect("a file type's node exporter refusing a kind (the kit runs no exporter)");

INDEX_STALE = e("MPC-E717", "index_stale", Blob, 1, "structure index", &["(host)"],
    "the index job for version {version} is out of date: {reason}",
    "The version the job was reading changed under it (it was purged, deleted or rewritten). \
     Nothing was recorded; the host may take a new job.")
    .step("status", &[],
        "That version changed while its index was being built; see which versions the doc has \
         now.");

NEEDS_LEASE = e("MPC-E718", "needs_lease", Blob, 2, "the tier-0 verb path: merge or lease",
    BLOB_VERB,
    "{verb} edits this file only under your own lease; take it first ({ttl_ms} ms by default)",
    "This verb's changes aren't covered by the file type's merge, so it runs only while you \
     hold the file's lease: a person's app saving meanwhile then keeps both copies instead of \
     merging your work away. Nothing was written. Take the lease (details.ttl_ms is the \
     default length), run the verb again, and release the lease when done.")
    .step("lease", &["--ttl", "{ttl_ms}"],
        "This verb runs only under your own lease; take it, then run the verb again.");

BASE_ACROSS_UNPROVEN_SAVE = e("MPC-E719", "base_across_unproven_save", Blob, 1,
    "the tier-0 verb path: --base across an outside save", BLOB_VERB,
    "version {save} is another program's save, not proven to keep node keys: --base {base} \
     can't pair {addresses} across it (head {head})",
    "Another program saved the file after the version you read (details.save), and the file \
     type's pack can't prove that program keeps node keys: a key may name another thing now. \
     Nothing was written. Read the nodes again at head (details.head), redo your change on \
     what you read, and call again with --base details.head.")
    .step("node", &["{addresses}"],
        "Another program saved the file since the version you read; read the nodes again at \
         head and redo the change on what you read.");

BLOB_REF_UNKNOWN = e("MPC-E720", "blob_ref_unknown", Blob, 4, "blob references in keyed docs",
    &["@pack", "(host)"],
    "no blob {blob} in this doc's space",
    "The value names a blob this doc's space doesn't hold: it was never stored here, it was \
     collected, or it belongs to another space. Nothing was applied. Store the bytes in this \
     doc's space, then send the change again with the id that returned.");

NATIVE_NO_TIER0 = e("MPC-E721", "native_no_tier0", Blob, 4,
    "the native engine (a Beta): no move to or from tier 0", HOST,
    "this doc is on the native engine (a Beta), which doesn't move to or from tier 0 ({verb})",
    "Nothing changed: the doc is still on the native engine, as it was. In the Beta a doc on \
     the native engine can't become a tier-0 doc, or be made from one. Switch the doc back to \
     the standard engine first, then move it.")
    .indirect("a host demoting or promoting a doc on the native engine (feature native)");

KEYS_REISSUED = e("MPC-E722", "keys_reissued", Blob, 1, "the tier-0 reads across an outside save",
    INDEX_HOST,
    "version {save}'s writer reissues node keys: a diff across it pairs nothing",
    "Another program saved the file at version details.save, and the file type's pack says \
     that program renumbers node keys: across that save a key may name another thing, so a \
     diff across it would pair the wrong nodes. Nothing was read. Diff from details.safe_from \
     to details.to instead, or compare the two versions' outlines.");

BASE_MOVED = e("MPC-E723", "base_moved", Blob, 1, "the tier-0 verb path: --base per node",
    BLOB_VERB,
    "{addresses} changed since --base {base} (head {head}); nothing applied",
    "Someone changed the nodes your call changes after the version you read (details.base), \
     in a way that overlaps your change: each is in details.conflicts with the field, who \
     changed it and in which version. Nothing was written. Read them again at head, redo your \
     change on what you read, and call again with --base details.head.")
    .step("node", &["{addresses}"],
        "The nodes you change were changed since the version you read; read them again at \
         head and redo the change on what you read.");

BASE_WEAK_KEY = e("MPC-E724", "base_weak_key", Blob, 2, "the tier-0 verb path: --base per node",
    BLOB_VERB,
    "{keys} have no key of the file's own: --base can't pair them; guard the call with \
     --if-version {head}",
    "The nodes your call changes have no ID in the file, so the kit named them itself, and it \
     can't be sure such a name means the same node in another version. Nothing was written. \
     Read the doc at head (details.head) and call again with --if-version details.head \
     instead of --base.")
    .step("status", &[],
        "These nodes can't be paired across versions; check the doc's head, then guard the \
         call with that version instead.");

BAD_PLAN = e("MPC-E725", "bad_plan", Blob, 2, "the tier-0 verb path: the plan", BLOB_VERB,
    "the verb's plan is refused: {why}",
    "The file type's verb described its change in a way the kit refuses (its byte ranges out \
     of order, overlapping, outside the file or over a limit, or a read verb that changes \
     bytes). Nothing was written. This is a bug in the file type's pack: report it.");

STRAYED = e("MPC-E726", "strayed", Blob, 2, "the tier-0 verb path: the output check", BLOB_VERB,
    "the verb's output changed nodes its plan didn't name: {keys}",
    "The kit read the verb's output with the file type's own index and found changes to nodes \
     the verb didn't say it would touch (details.keys). Nothing was written. This is a bug in \
     the file type's pack: report it.");

OUTPUT_UNREADABLE = e("MPC-E727", "output_unreadable", Blob, 2,
    "the tier-0 verb path: the output check", BLOB_VERB,
    "the file type's index refuses the verb's output",
    "The file the verb would write can't be read back by the file type's own index. Nothing \
     was written. This is a bug in the file type's pack: report it.");

VERB_TOO_LARGE = e("MPC-E728", "verb_too_large", Blob, 2, "the tier-0 verb path: the bounds",
    BLOB_VERB,
    "{what} is {size}, over the limit of {limit}",
    "The call is over one of the kit's bounds for a verb on this file (details.what and \
     details.limit name it). Nothing was written. Narrow the call (fewer nodes at a time).");

NOT_AS_PREVIEWED = e("MPC-E729", "not_as_previewed", Blob, 1,
    "the tier-0 verb path: the dry-run contract", BLOB_VERB,
    "the bytes this call would store are not the ones previewed (expected {expected}, now \
     {actual})",
    "The call carried the digest its dry run showed (--expect), and the bytes it would store \
     now differ: the file changed since, or the call did. Nothing was written. Run the dry run \
     again, show it, and use its digest.");

REVERT_UNSUPPORTED = e("MPC-E730", "revert_unsupported", Blob, 2,
    "tier-0 revert, undo and redo: the file type's merge", HOST,
    "version {version} can't be reverted alone: {why} (version {before} is the one before it)",
    "Taking one version's change away needs the file type's own merge, and it has none or it \
     can't take these bytes. Nothing was written. Restore the version \
     before it instead (details.before): that also replaces every change made since.")
    .step("restore", &["--version", "{before}"],
        "This file type can't take one version's change away alone; restoring the version before \
         it also replaces every change made since.");

REVERT_CONFLICT = e("MPC-E731", "revert_conflict", Blob, 1,
    "tier-0 revert, undo and redo: changed since", HOST,
    "version {version}'s change to {addresses} was changed since (head {head}); nothing \
     reverted",
    "Someone changed what that version changed after it, in a way that overlaps: each is in \
     details.conflicts with the field, who changed it and in which version (empty when the file \
     type's merge found the overlap). Nothing was written. Read them at head and change them \
     back yourself, or ask whoever changed them.");

// ---------------------------------------------------------------- 8xx structured and CSV
// the CSV grid work: wired in v0.10.0; the walk in
// tests/error_ids.rs reaches them on a grid doc (the grid test pack).

NEEDS_BASE = e("MPC-E801", "needs_base", Structured, 2, "K-BR-12", &["cell set", "@change"],
    "an agent's grid edit needs a base: read first and pass --base",
    "Agents edit tables against what they read. Read the table first (it prints the \
     version), then run the edit again with --base <version>.")
    .step("cat", &[],
        "Agents edit tables against what they read; read the table first, then run the edit again \
         against the version it printed.");

AT_LIMIT = e("MPC-E802", "at_limit", Structured, 2, "K-CSV-7 (table limits)", &["@change"],
    "this table is at its size limit ({limit} {what}, deleted rows included); the owner can \
     free space with `rows drop-deleted`",
    "The table can't grow past its limit (details.what: cells, columns or rows); nothing was \
     applied and nothing was dropped. Remove rows or columns first, or ask an owner to run \
     rows drop-deleted to free the space deleted rows still hold.")
    .step("rows drop-deleted", &[],
        "The table is at its size limit; remove rows or columns first, or have an owner free the \
         space deleted rows still hold.");

FACET_CONFLICT = e("MPC-E803", "facet_conflict", Structured, 1, "K-BR-12 (writes with no identity)",
    &["@change"],
    "{count} element(s) changed since your base: {where}",
    "Someone changed the same cells or rows after the version you read. Nothing was \
     applied. Read again, redo your change on the new values, and pass the new --base.")
    .step("cat", &[],
        "Someone changed the same cells or rows since your base; read again, redo your change on \
         the new values, and pass the new base.");

OP_REFUSED = e("MPC-E804", "op_refused", Structured, 2, "K-SCH-2", &["@change"],
    "op {op} on root {root}: {problem}",
    "One of the edit's row, column or cell operations can't apply (an unknown or reused \
     key, a move next to itself, dropping a live element); nothing of the change was \
     applied. List the rows or columns again (row list, col list) and use the keys shown.")
    .step("row list", &[],
        "An operation names an unknown or reused key; list the rows or columns again and use the \
         keys shown.");

NO_ELEMENT = e("MPC-E805", "no_element", Structured, 4, "CSV contract §3.3",
    &["@pack"],
    "{verb}: {line}",
    "The row, column or cell the verb names is unknown or deleted (stdout says which, and \
     who deleted it when you may read history). A raw keyed cell write naming a row or \
     column the table doesn't know (dropped, or never in it) is refused the same way, whole. \
     Nothing was applied. List the live ones (row list, col list), or restore a deleted one \
     first (row restore, col restore).")
    .step("row list", &[],
        "The row, column or cell is unknown or deleted; list the live ones, or restore a deleted \
         one first.");

HEADER_NAMES_UNRENDERED = e("MPC-E806", "header_names_unrendered", Structured, 2,
    "CSV contract §1.4", &["@pack"],
    "the columns have names that would appear as a header line; clear them or add a row first",
    "The table has no row, and its columns still hold names from before the header was \
     switched off: switching it on would write them as a header line, changing the file. \
     Nothing was applied. Clear the names (col rename) or add a row first, then switch.")
    .step("col rename", &[],
        "The columns still hold names that would be written as a header line; clear them or add a \
         row first, then switch.");

HEADER_ONLY_ROW_BLANK = e("MPC-E807", "header_only_row_blank", Structured, 2,
    "CSV contract §1.4", &["@pack"],
    "header on: the only row is a blank line, and a blank header line with nothing after it \
     isn't written (the file would change); add a non-blank row first",
    "The table's only row is blank: as the header it would be a blank header line with \
     nothing after it, which isn't written, so the file would change. Nothing was applied. \
     Add a row with values first (row insert), then switch the header on.")
    .step("row insert", &[],
        "A blank only row can't be written as the header; add a row with values first, then \
         switch the header on.");

// 820-: tagged keys (crate::mint; the writer-tag rule on a pack's declared roots)

KEY_NOT_OWN = e("MPC-E820", "key_not_own", Structured, 2, "writer-tagged keys",
    &["@change"],
    "insert {key} on {root} doesn't carry your writer tag {tag}",
    "This list's keys carry their writer: an insert must name <prefix><n>.<tag> with one of \
     the list's prefixes and your own writer tag (details.tag). Nothing was applied. Insert \
     through a verb or Session::mint_key, which mints a key with your own writer tag; never \
     reuse another writer's tag.");

KEY_SPACE_FULL = e("MPC-E821", "key_space_full", Structured, 2, "writer-tagged keys",
    HOST,
    "no key fits 24 bytes for prefix {prefix} and tag {tag}",
    "Every key of this prefix and writer tag that fits a list key's 24 bytes is taken. \
     Nothing was written. The pack's key prefix is too long for a tagged key: use a shorter \
     prefix (the pack's fix).");

// a keyed doc that isn't a table, at its live size limit (`Config::keyed_limit`)
KEYED_AT_LIMIT = e("MPC-E822", "keyed_at_limit", Structured, 2, "K-CSV-7 (keyed docs)",
    &["@change"],
    "this doc is at its live size limit ({limit} {what}, deleted elements included); nothing \
     was applied",
    "The doc can't grow past its live size limit (details.limit elements and fields: every \
     list element, deleted ones included until dropped, plus every field; \
     details.limit_source says whose limit it is: host_per_pack, the host's for this file \
     type; pack_default, the file type's own default; or host_wide, the host's for every file \
     type). Nothing was applied and nothing was dropped. Read the doc, remove \
     elements or fields it doesn't need any more, then make the change again; a change that \
     doesn't add elements or fields always goes through.")
    .step("cat", &[],
        "The doc is at its live size limit; read it, remove elements or fields it doesn't \
         need any more, then make the change again.");

// 823: a pack's refusal by its own code: a verb's in the text form (`refused: <code>: <why>`,
// exit 2) whose code the pack registers no entry for (Pack::errors), the stop-gap until each
// pack registers its codes; and a live change the pack's check refuses (Pack::check_edit,
// whoever makes it: a person's edit, a host's, an agent's verb, undo, restore).

PACK_REFUSED = e("MPC-E823", "pack_refused", Structured, 2,
    "a pack's refusal by code (its own ids: Pack::errors; its check of a change: \
     Pack::check_edit)", &["@pack", "@change", "(sync)", "(host)"],
    "{code}: the pack refused the call",
    "The file type refused the call or the change: details.code is its own code for why \
     (details.reason, for a change its check refused), details.next its own way forward when \
     it gives one. Nothing was applied and the version didn't move. Do what details.next \
     says; for a change the file type can't write to its file, make it through the file \
     type's own verbs, or leave that part of the doc as it is.")
    .indirect("a keyed pack's verb printing its refusal as text (tests/agent_results.rs), or \
               its check of a live change (tests/pack_check_edit.rs)");

// 824: a whole-file write from a read given while the file couldn't be rendered
// (`ReadResult::render_failed`: the bytes it gave are the file as last synced)

BASE_UNRENDERED = e("MPC-E824", "base_unrendered", Structured, 2, "render net",
    &["write"],
    "version {base} was read while the file couldn't be written back; nothing applied",
    "Nothing was applied: the read of version details.base was given while the file couldn't \
     be rendered, so its bytes were the file as last synced, not that version, and a write \
     made from them can't be merged at it. Change the doc through the file type's verbs, \
     which never need the file's bytes; once the file renders again, read it and write from \
     that read.");

// 825–829: free, for structured docs.

// 830–869: the board file type's own agent verbs (k2-mpc-board), run by its host on the file
// itself and listed with Registry::for_file. Each row names `@file`, then the board verbs
// that can return it (data only: the kit doesn't depend on the board crate). The host's
// bridge maps the board's refusals here by id; a format refusal's exact code goes in
// details.code, as for a pack's declines (E407). 830–859 now; 860–869 reserved for the
// board's live verbs.

FILE_CONFLICT = e("MPC-E830", "conflict", Structured, 1, "board file: object revs",
    BOARD_REV,
    "{target} changed since the rev you read",
    "Someone changed it after you read it (details.target says what; the refusal carries \
     what is there now). Nothing was applied. Read it again, redo your change on what is \
     there now, and pass the new rev.")
    .indirect(BOARD_BRIDGE)
    .step("read", &[],
        "It changed since the rev you read; read it again, redo your change on what is there \
         now, and pass the new rev.");

FILE_EXISTS = e("MPC-E831", "exists", Structured, 1, "board file: unique ids",
    &["@file", "put-back", "apply", "inverse"],
    "{target} is already in use",
    "The id you gave is taken (details.target; the refusal carries what is there now). \
     Nothing was applied. Read what is there, then give another id, or leave the id out so a \
     free one is chosen.")
    .indirect(BOARD_BRIDGE)
    .step("read", &[],
        "The id is taken; read what is there, then give another id or let a free one be chosen.");

FILE_BUSY = e("MPC-E832", "busy", Structured, 1, "board file: the save guard",
    BOARD_CHANGE,
    "the file changed on disk since it was read, and again on each of the host's retries",
    "Someone else saved the file between your read and your save. The host already retried \
     the save, and the file changed again each time. Nothing was written. Read what you are \
     changing again, then run your change again on what is there now.")
    .indirect(BOARD_BRIDGE)
    .step("read", &[],
        "The board changed while saving; read it again, then retry your change on what is there \
         now.");

FILE_MERGE_CONFLICT = e("MPC-E833", "merge_conflict", Structured, 1,
    "board file: three-way merge", &["@file", "(host)"],
    "{count} thing(s) changed on both sides",
    "Your copy and the file both changed the same things (settings, people, top-level fields \
     or objects) since the copy was made (details lists each, with every side kept). Nothing \
     was written. Read the board again and redo your changes to those things on what is there \
     now.")
    .indirect(BOARD_BRIDGE);

FILE_HELD = e("MPC-E834", "board_conflict", Structured, 1, "board file: groups apply whole",
    &["@file", "(host)"],
    "{count} group(s) held: what they change moved since they were made",
    "Some groups of edits didn't apply, because what they change moved since they were made \
     (details lists each held group and why). Each held group changed nothing and is kept \
     whole; the others applied. Read the board again and make the held changes again on what \
     is there now.")
    .indirect(BOARD_BRIDGE);

FILE_LOCKED = e("MPC-E835", "locked", Structured, 5, "board file: locked objects",
    &["@file", "update", "delete", "reply", "resolve", "check-add", "check-set",
      "check-remove", "table-cell", "table-add-row", "table-remove-row", "table-add-column",
      "table-remove-column", "move", "order", "frame-order", "group", "ungroup", "align",
      "distribute", "tidy", "copy-style", "connected-copy", "lock", "apply", "inverse"],
    "{id} is locked",
    "The object is locked, so no verb changes, moves or deletes it until it is unlocked. \
     Nothing was applied. Read it to see its state, and ask whoever locked it; unlock it \
     (lock) only if the lock is yours.")
    .indirect(BOARD_BRIDGE)
    .step("read", &[],
        "The object is locked; read it to see its state, ask whoever locked it, and unlock it \
         only if the lock is yours.");

FILE_VOTING_OFF = e("MPC-E836", "voting_off", Structured, 5, "board file: voting",
    &["@file", "vote"],
    "voting is off on this board",
    "Votes count only while voting is on, and a person running the session usually starts \
     it. Nothing was applied. Ask for voting to be started (meta turns it on), then vote \
     again.")
    .indirect(BOARD_BRIDGE)
    .step("meta", &[],
        "Voting is off; it is turned on in the board's settings, usually by the person running \
         the session.");

FILE_NO_VOTES_LEFT = e("MPC-E837", "no_votes_left", Structured, 5, "board file: voting",
    &["@file", "vote"],
    "no votes left",
    "You have used every vote this round allows. Nothing was applied. Take a vote back from \
     another object (vote, with a negative delta) to move it, or wait for the next round.")
    .indirect(BOARD_BRIDGE)
    .step("vote", &[],
        "Every vote this round allows is used; take one back from another object to move it, or \
         wait for the next round.");

FILE_NO_OBJECT = e("MPC-E838", "no_object", Structured, 4, "board file: object ids",
    &["@file", "read", "update", "delete", "connect", "react", "vote", "comment", "reply",
      "resolve", "lock", "check-add", "check-set", "check-remove", "table-cell",
      "table-add-row", "table-remove-row", "table-add-column", "table-remove-column", "move",
      "order", "frame-order", "group", "ungroup", "align", "distribute", "tidy", "copy-style",
      "duplicate", "connected-copy", "mind-child", "apply", "inverse"],
    "no object {id}",
    "The board has no object with that id: it was deleted, or the id is mistyped. Nothing \
     was applied. List the objects and use an id shown; a deleted one comes back with \
     put-back, from what its delete returned.")
    .indirect(BOARD_BRIDGE)
    .step("list", &[],
        "No object has that id; list the objects and use an id shown.");

FILE_NO_TEMPLATE = e("MPC-E839", "no_template", Structured, 4, "board file: templates",
    &["@file", "template"],
    "no template {name}",
    "No template has that name. Nothing was applied. List the templates and use a name \
     shown.")
    .indirect(BOARD_BRIDGE)
    .step("templates", &[],
        "No template has that name; list the templates and use a name shown.");

FILE_NO_SUCH_ITEM = e("MPC-E840", "no_such_item", Structured, 4, "board file: item ids",
    &["@file", "check-set", "check-remove", "table-cell", "table-add-row",
      "table-remove-row", "table-add-column", "table-remove-column"],
    "{id} has no item {item}",
    "The object has no check item, row or column with that id. Nothing was applied. Read the \
     object to see its items and use an id shown.")
    .indirect(BOARD_BRIDGE)
    .step("read", &[],
        "The object has no item with that id; read the object to see its items and use an id \
         shown.");

FILE_BLOB_MISSING = e("MPC-E841", "blob_missing", Structured, 4, "board file: pictures",
    BOARD_CHANGE,
    "picture bytes {hash} aren't in the file",
    "A picture on the board names bytes the file doesn't hold, so it can't be saved as is. \
     Nothing was written. Clear the pictures whose bytes are missing (clear-blob), or add \
     the bytes again, then run your change again.")
    .indirect(BOARD_BRIDGE)
    .step("clear-blob", &[],
        "A picture's bytes are missing from the file; clear that picture, or add its bytes again, \
         then run your change again.");

FILE_ZIP = e("MPC-E842", "board_zip", Structured, 4, "board file: the package",
    BOARD_OPEN,
    "the file isn't a package this build reads ({code})",
    "The file's container is damaged, or uses something the format doesn't allow \
     (details.code says which). Nothing was read or changed. Restore the file from a good \
     copy; no board verb opens it as it is.")
    .indirect(BOARD_BRIDGE);

FILE_MANIFEST = e("MPC-E843", "board_manifest", Structured, 4, "board file: the manifest",
    BOARD_OPEN,
    "the file's manifest or parts don't hold together ({code})",
    "The file's manifest is missing or damaged, or its parts don't match it (details.code \
     says which). Nothing was read or changed. Restore the file from a good copy; no board \
     verb opens it as it is.")
    .indirect(BOARD_BRIDGE);

FILE_NEWER_FORMAT = e("MPC-E844", "newer_format", Structured, 4, "board file: format versions",
    BOARD_OPEN,
    "the file needs a newer reader (version {needs})",
    "A newer build wrote this file, and it asks for a reader newer than this one. Nothing \
     was read or changed. Open it with a build that reads that version.")
    .indirect(BOARD_BRIDGE);

FILE_INVALID = e("MPC-E845", "board_invalid", Structured, 4, "board file: the board's data",
    BOARD_OPEN,
    "the board in the file isn't valid ({code})",
    "The file opens, but the board it holds breaks the format (details.code says how, and \
     where when it can). Nothing was read or changed. Restore the file from a good copy, or \
     fix the board's data with the tool that wrote it.")
    .indirect(BOARD_BRIDGE);

FILE_TOO_LARGE = e("MPC-E846", "board_too_large", Structured, 4, "board file: limits",
    BOARD_OPEN,
    "the file is over a limit ({code})",
    "The file passes one of the limits this host reads boards within (details.code says \
     which). Nothing was read or changed. Ask the host's operator about the limit, or split \
     the board with the tool that wrote it.")
    .indirect(BOARD_BRIDGE);

FILE_BAD_FIELD = e("MPC-E847", "bad_field", Structured, 2, "board file: fields",
    &["@file", "add", "update", "connect", "comment", "reply", "check-add", "check-set",
      "check-remove", "table-cell", "mind-child", "meta", "react", "paste", "put-back",
      "apply", "inverse", "fields"],
    "a value you gave isn't valid ({code})",
    "A field, id, type or number you gave doesn't fit the format (details.code says which). \
     Nothing was applied. Check the fields that type takes, fix the value and run the verb \
     again.")
    .indirect(BOARD_BRIDGE)
    .step("fields", &[],
        "A value you gave doesn't fit the format; check the fields each type takes, fix it and run \
         the verb again.");

FILE_LIMIT = e("MPC-E848", "board_limit", Structured, 2, "board file: limits",
    BOARD_CHANGE,
    "the change would pass a limit ({code})",
    "Your input, or the board after your change, would pass one of the limits this host \
     works within (details.code says which). Nothing was applied or written. Make a smaller \
     change, or remove objects first.")
    .indirect(BOARD_BRIDGE);

FILE_BAD_OPS = e("MPC-E849", "bad_ops", Structured, 2, "board file: raw ops",
    &["@file", "apply"],
    "the ops aren't valid ({code})",
    "The ops you passed aren't valid JSON, or not in the shape the format takes (details.code \
     says which). Nothing was applied. Check them against the fields each type takes, fix \
     them and apply again.")
    .indirect(BOARD_BRIDGE)
    .step("fields", &[],
        "The ops don't fit the format; check them against the fields each type takes, fix them \
         and apply again.");

FILE_UNKNOWN_FIELD = e("MPC-E850", "unknown_field", Structured, 2, "board file: fields",
    &["@file", "add", "update"],
    "{type} has no field {field}",
    "That type of object has no field by that name. Nothing was applied. List the fields it \
     takes and use one of those names.")
    .indirect(BOARD_BRIDGE)
    .step("fields", &[],
        "That type has no field by that name; list the fields it takes and use one of them.");

FILE_UNKNOWN_TYPE = e("MPC-E851", "unknown_type", Structured, 2, "board file: types",
    &["@file", "add", "update"],
    "no object type {type}",
    "The format has no type of object by that name. Nothing was applied. List the types and \
     use one shown.")
    .indirect(BOARD_BRIDGE)
    .step("fields", &[],
        "The format has no type by that name; list the types and use one shown.");

FILE_BAD_VALUE = e("MPC-E852", "bad_value", Structured, 2, "board file: verb values",
    &["@file", "vote", "comment"],
    "{verb} can't use that value: {reason}",
    "The verb can't use a value you gave (a vote of 0, a comment with no text). Nothing was \
     applied. Run it again with a value it can use.")
    .indirect(BOARD_BRIDGE);

FILE_WRONG_TYPE = e("MPC-E853", "wrong_type", Structured, 2, "board file: types",
    &["@file", "reply", "resolve", "check-add", "check-set", "check-remove", "table-cell",
      "table-add-row", "table-remove-row", "table-add-column", "table-remove-column", "move",
      "align", "distribute", "mind-child", "connected-copy"],
    "{verb} doesn't apply to {id}, a {type}",
    "That verb works on other types of object (a reply on a comment, a check item on a card, \
     a row on a table). Nothing was applied. Read the object to see its type, then pick the \
     object or the verb that fits.")
    .indirect(BOARD_BRIDGE)
    .step("read", &[],
        "The verb doesn't work on that type of object; read it to see its type, then pick the \
         object or the verb that fits.");

FILE_TOO_FEW = e("MPC-E854", "too_few", Structured, 2, "board file: arranging",
    &["@file", "group", "align", "distribute", "tidy", "paste"],
    "{verb} needs at least {min} objects",
    "The verb needs more objects than you named (two to group or align, three to distribute). \
     Nothing was applied. List the objects and name enough of them.")
    .indirect(BOARD_BRIDGE)
    .step("list", &[],
        "The verb needs more objects than you named; list the objects and name enough of them.");

FILE_NEEDS_ACTOR = e("MPC-E855", "needs_actor", Structured, 2, "board file: who acts",
    &["@file", "react", "vote", "reply"],
    "{verb} needs to know who is acting",
    "This verb records who acted, and the host didn't say. Nothing was applied. This is the \
     host's to fix, not the caller's.")
    .indirect(BOARD_BRIDGE);

FILE_NO_FREE_ID = e("MPC-E856", "no_free_id", Structured, 1, "board file: unique ids",
    &["@file", "add", "connect", "comment", "reply", "check-add", "table-add-row",
      "table-add-column", "group", "duplicate", "paste", "connected-copy", "mind-child",
      "template", "apply"],
    "no free id: the host's random ids keep landing on ids in use",
    "Every new id the host's randomness gave was already in use. Nothing was applied. A \
     try with fresh random bytes can succeed, and the host makes it itself; if this keeps \
     coming back, report it to the host's operator.")
    .indirect(BOARD_BRIDGE);

FILE_IO = e("MPC-E857", "board_io", Structured, 1, "board file: reading and writing",
    BOARD_OPEN,
    "reading or writing the file failed: {reason}",
    "The file couldn't be read or written (the reason says why). Nothing was changed. Check \
     the disk (space, permissions), then run the verb again.")
    .indirect(BOARD_BRIDGE);

FILE_SETTINGS_CONFLICT = e("MPC-E858", "settings_conflict", Structured, 1,
    "board file: settings revs", BOARD_SETTINGS,
    "the settings changed since the rev you read",
    "The settings changed since you read them (details.theirs holds what is there now). \
     Nothing was applied. Read them again with meta, then redo your change on what is there \
     now and pass the new rev.")
    .indirect(BOARD_BRIDGE)
    .step("meta", &[],
        "The settings changed since the rev you read; read them again, redo your change on what \
         is there now, and pass the new rev.");

FILE_PERSON_EXISTS = e("MPC-E859", "person_exists", Structured, 1, "board file: people",
    BOARD_PERSON,
    "token {id} already names another person",
    "The token you gave already names another person in this file (details.id is the token, \
     details.theirs the person it names). Nothing was applied. Use another token, or leave \
     the person op out and let the host mint one.")
    .indirect(BOARD_BRIDGE);

// ---------------------------------------------------------------- 9xx limits

RATE_LIMITED = e("MPC-E901", "rate_limited", Limits, 2, "K-SEC-9, K-PERM-14", CHANGE,
    "too many edits; retry in {retry_after_ms} ms",
    "You are over the edit rate for this doc (capabilities: edit_rate_per_s, edit_burst). \
     Nothing was applied. Wait details.retry_after_ms, then retry; better, combine your \
     changes into fewer calls (one patch, or one write --base).");

TOOL_LIMIT = e("MPC-E902", "tool_limit", Limits, 2, "K-PAR-7, K-SEC-4", RUN_TOOL,
    "limit: {reason}",
    "The call would go over a bound (output size, pattern size or work). Nothing was \
     applied. Narrow it: fewer lines (head, view --lines), a simpler pattern, smaller \
     edits.")
    .step("head", &[],
        "The call would pass a bound; narrow it: fewer lines, a simpler pattern, smaller edits.");

CLAIMS_DOC_CAP = e("MPC-E903", "claims_doc_cap", Limits, 2, "claims (claims_max)", &["claim"],
    "bad claim: this doc has {count} live claims (the most)",
    "This doc has as many live claims as it allows. Wait for some to end (activity and who \
     show them), or edit without a claim.")
    .step("activity", &[],
        "This doc has all the live claims it allows; wait for some to end, or edit without a \
         claim.");

CLAIMS_PARTICIPANT_CAP = e("MPC-E904", "claims_participant_cap", Limits, 2,
    "claims (claims_per_participant)", &["claim"],
    "bad claim: you hold {count} claims here (the most); release one",
    "You hold as many claims on this doc as you may. Release one you no longer need \
     (claim --release --id ID), then claim again.")
    .step("claim --release", &[],
        "You hold all the claims you may on this doc; release one you no longer need, then claim \
         again.");

CLAIM_WAITERS_CAP = e("MPC-E905", "claim_waiters_cap", Limits, 2,
    "claims (claim_waiters_max)", &["(host)"],
    "bad claim: {count} already waiting for {claim}",
    "That claim already has as many waiters as it allows. Retry after it ends, or work \
     elsewhere.")
    .step("activity", &[],
        "That claim already has as many waiters as it allows; see where others are working and \
         work elsewhere, or retry after it ends.");

// 906: `connection_work_throttled`, reserved for the native engine's sessions

ANNOTATION_CAP = e("MPC-E907", "annotation_cap", Limits, 2, "K-ANN-1 (annot_* limits)",
    &["comment", "reply", "resolve", "resolve --reopen", "reanchor"],
    "annotation limit: at most {limit} {what}",
    "The call would pass a limit (details.what: body bytes, threads in the doc, comments in \
     the thread, or history rows of the thread, past which only replies, deletes and purges \
     still fit; capabilities lists the annot_* limits). Nothing changed. Write a shorter \
     comment, or go on in a new thread.");

WRITERS_DOC_CAP = e("MPC-E908", "writers_doc_cap", Limits, 2, "writers (writers_max)", CHANGE,
    "read-only: this doc has {count} writers (the most is {max})",
    "This doc already holds as many writers as it allows, and writers stay in the doc for \
     good, so this connection can read but not edit. Nothing changed. Don't retry in a loop: \
     a reconnect from a device that has edited this doc before may edit again.")
    .step("cat", &[],
        "The doc holds all the writers it allows, so this connection is read-only; read it, and \
         don't retry in a loop.");

NATIVE_TEXT_CAP = e("MPC-E909", "native_text_cap", Limits, 2,
    "the native engine (a Beta): a doc's live text", CHANGE,
    "this doc's live text would be {would_be} bytes, past the native engine's {limit}-byte cap",
    "Nothing was applied, and nothing in the doc was dropped. A doc on the native engine (a \
     Beta) holds at most details.limit bytes of live text, all its text together. Make the \
     change smaller or remove text first; or switch the doc back to the standard engine.")
    .indirect("an edit past a native doc's live-text cap (feature native)");
}

registry! { ENGINE:

// The native engine's own codes (feature `native`), as the engine allocates them; reached
// through the adapter (`engine/mpce.rs`). Append-only, as the kit's.

REF_NOT_IN_PAST = e("MPCE-E199", "ref_not_in_past", Session, 2,
    "the MPCE seam (stable codes); E-CORE-3", SYNC,
    "the engine refused the update: an op names an element outside its own causal past",
    "The update depends on content it doesn't carry or the doc doesn't hold. Nothing was \
     applied. The editor should resync.")
    .indirect("the native engine's admission (feature native)");

CHANGE_TOO_LARGE = e("MPCE-E980", "change_too_large", Limits, 2,
    "the MPCE seam (stable codes); E-CORE-10", CHANGE,
    "the engine refused the change: it would pass a decoder bound",
    "The change is larger than one update may be (in events, bytes or one op's text). \
     Nothing was stored or applied. Make it in smaller changes.")
    .indirect("the native engine's local edit (feature native)");

STORE_SECRET_MISSING = e("MPCE-E566", "store_secret_missing", Store, 6,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused: this store has no secret configured",
    "This store can't write a checkpoint without its secret. Nothing was written. The host \
     supplies the store's secret and keeps it outside the store; pass it on every open. Never \
     make a new secret for a store that has native records.")
    .indirect("the native engine's store (feature native)");

CHECKPOINT_MAC = e("MPCE-E567", "checkpoint_mac", Store, 6,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused a checkpoint record: it failed its keyed check",
    "Nothing was loaded from it. Restore the doc from its log, or reload it.")
    .indirect("the native engine's store (feature native)");

NATIVE_STORE_CORRUPT = e("MPCE-E561", "native_store_corrupt", Store, 6,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused a stored record: it fails the load's checks",
    "Nothing was opened past this record. If the host lowered its limits, raise them again; if \
     it releases records, check it settled their digests first. Otherwise restore the document \
     from another copy: the host's state, for a replica; for the host's own store, a peer \
     replica, a backup, or the document's Yrs store where one is kept.")
    .indirect("the native engine's store (feature native)");

CUT_PURGE_PENDING = e("MPCE-E568", "cut_purge_pending", Store, 1,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine didn't adopt a cut: a purge happened since its position, or a rewrite isn't \
     finished",
    "Nothing changed. Finish the rewrite, then cut again from a new fork.")
    .indirect("the native engine's store (feature native)");

STORE_KEYS_WRONG = e("MPCE-E571", "store_keys_wrong", Store, 6,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused the store: it was written under another secret or digest key",
    "Nothing was read and nothing changed. Check the host's keys for this document: restore \
     its own secret and digest key from where the store came from. Never make new keys for a \
     store that has native records.")
    .indirect("the native engine's store (feature native)");

STABLE_PART_OF_CHANGE = e("MPCE-E573", "stable_part_of_change", Store, 2,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused the stable point: it splits a change",
    "Nothing changed. Pass a stable point that is an apply-order prefix of whole changes. At \
     a load, the store holds a stable record no released writer makes: restore the document \
     from another copy (the host's state, a peer replica, a backup).")
    .indirect("the native engine's stable point (feature native)");

S_RECORD_MISSING = e("MPCE-E580", "s_record_missing", Store, 1,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused the write: the stable record it returned isn't in the store yet",
    "Nothing was written. Append the stable record set_stable returned, then retry. If its \
     bytes were lost before they were written, reload the document from the store and set the \
     stable point again.")
    .indirect("the native engine's stable point (feature native)");

CHECKPOINT_DROPS_S_RECORD = e("MPCE-E581", "checkpoint_drops_s_record", Store, 6,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused a cut: the checkpoint drops a stable record its kept records name",
    "Nothing was written: the store is as it was. This is an engine bug: report it with the \
     record index in the message.")
    .indirect("the native engine's store (feature native)");

STORE_NONCE_INVALID = e("MPCE-E582", "store_nonce_invalid", Store, 6,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused the store nonce: missing, all zero, or equal to a key",
    "Nothing was created. A new store needs a fresh random 16-byte nonce, never reused and \
     never equal to its keys: the kit makes one for every store, so this is a kit bug: report \
     it.")
    .indirect("the native engine's new store (feature native)");

PART_CAP_BELOW_ENTRY = e("MPCE-E987", "part_cap_below_entry", Limits, 2,
    "the MPCE seam (stable codes)", &["(host)"],
    "the engine refused its limits: a part size is below the largest checkpoint entry",
    "Nothing was made or loaded. Raise checkpoint_part_bytes (and record_bytes) to at least \
     the largest checkpoint entry the limits admit, or lower string_bytes_per_op.")
    .indirect("the native engine's limits (feature native)");

MERGE_NOT_A_FOLD = e("MPCE-E995", "merge_not_a_fold", Limits, 2,
    "the MPCE seam (stable codes)", SYNC,
    "the engine refused the update: a merge in it doesn't fold",
    "Nothing was applied. From a peer the update is malformed or forged: drop it, and if the \
     peer keeps sending such updates, close it as hostile and resync from the host. (From the \
     host it is MPC-E163: this replica is behind.)")
    .indirect("the native engine's admission (feature native)");

CAUSAL_CHECK_OVER_LIMIT = e("MPCE-E997", "causal_check_over_limit", Limits, 2,
    "the MPCE seam (stable codes)", SYNC,
    "the engine refused the update: its causal check is over a limit",
    "Nothing was applied. Split the update at run boundaries into parts each within the \
     limits (one run always is), and send them in causal order.")
    .indirect("the native engine's admission (feature native)");

OWN_LINE_PARENTS = e("MPCE-E1000", "own_line_parents", Session, 2,
    "the MPCE seam (stable codes)", SYNC,
    "the engine refused the change: it names more than one parent on one agent's line",
    "Nothing changed. Name only the latest parent on each agent's line. At a load, the store \
     holds a run no released writer makes: restore the document from another copy.")
    .indirect("the native engine's admission (feature native)");

OUTBOUND_FRAME_TOO_LARGE = e("MPCE-E982", "outbound_frame_too_large", Limits, 2,
    "the MPCE seam (stable codes)", SYNC,
    "the engine refused to send an update as one frame: it is over a limit",
    "Nothing was sent or changed. The update is over frame_bytes or past a limit of the \
     receiver's causal check (details.cause; for a causal limit, details.limit, details.value \
     and details.max). Send it in parts: the kit's parts encoders cut it at run boundaries, \
     each part within every limit.")
    .indirect("the native engine's encoders (feature native)");

REQUEST_MALFORMED = e("MPCE-E986", "request_malformed", Limits, 2,
    "the MPCE seam (stable codes)", SYNC,
    "the engine refused a request of parts: it is malformed",
    "Nothing changed. A request of parts is empty, or holds several parts outside a relay: the \
     kit made it wrong, so don't retry it as it is; drop the answer and report it.")
    .indirect("the native engine's admission (feature native)");
}
