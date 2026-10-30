//! The registry: every refusal the kit can return, one row each. **Append only**: a new
//! refusal takes the next free id in its area; an id is never reused, renamed or given
//! another exit code, and a retired one stays here as `.retired("0.x.y")`. The snapshot
//! test (`crates/base/tests/golden/errors/registry.snapshot`) holds this.
//!
//! Message templates use `{key}` for the report's details. `next` is written for a caller
//! that has only the CLI: what happened, then what to run.

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

registry! { KIT:

// ---------------------------------------------------------------- 0xx exports
// a host's export of a doc's version (crate::export, ExportRegistry::export)

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
     still want the file.");

EXPORT_DECODE = e("MPC-E005", "export_decode", Export, 1, "X-ERR-1", HOST,
    "could not read the source",
    "The source version couldn't be read in its format (damaged, or a variant the exporter \
     doesn't read). Nothing was written. Export as-is to get the bytes, or try another \
     version.");

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
     live text (cat), or ask the doc's owner for history access.");

FORBIDDEN_APPROVER = e("MPC-E104", "forbidden_approver", Session, 3, "K-ID-11",
    &["approve", "decline"],
    "refused: needs Approver rights",
    "Only the person a change was made for can approve or decline it; an agent never can, \
     even the one that made it. Ask that person to run approve or decline (list the \
     pending changes with log --pending). Don't retry.");

FORBIDDEN_AUTHOR_OR_OWNER = e("MPC-E105", "forbidden_author_or_owner", Session, 3, "K-HIST-19",
    &["revert"],
    "refused: needs AuthorOrOwner rights",
    "Only the change's author, the person it was made for, or an owner can revert it. \
     Don't retry: ask one of them, or undo your own changes with undo.");

FORBIDDEN_OWNER = e("MPC-E106", "forbidden_owner", Session, 3, "K-PURGE-1, K-T0-2",
    &["lease --break", "purge"],
    "refused: needs Owner rights",
    "Only an owner of the doc can do this (break someone else's lease, purge). Don't \
     retry: ask an owner. To wait for a lease instead, retry after it expires (status shows \
     the holder and expiry).");

NOT_JOINED = e("MPC-E107", "not_joined", Session, 2, "K-PRES-1", ANY,
    "{conn} has not joined this doc",
    "Your connection left the doc (it was idle, kicked, or the doc reloaded). Run the verb \
     again: any verb joins the doc first. Nothing was applied.");

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

STALE = e("MPC-E110", "stale", Session, 1, "A14 (--if-version), K-T0-5",
    &["@change", "write"],
    "doc changed since {expected} (now {actual})",
    "Someone changed the doc after the version you named, so nothing was applied. Read it \
     again (cat prints the version), redo your change on what you read, and pass the new \
     version with --if-version (a blob write: the new base).");

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
     change only the lines you need with replace --lines or patch. details.conflicts has \
     each region's your_lines (in your text), current_lines (in the current text), base, \
     ours and theirs.");

BASE_EXPIRED = e("MPC-E112", "base_expired", Session, 8, "K-BR-4, K-EXIT-1",
    &["write"],
    "base version {base} is unknown or no longer cached; nothing applied",
    "The version you wrote against is too old to merge from (or was never read). Never \
     retry the same request: read the doc again (cat), redo your change on what you read, \
     and write with that version as --base.");

BAD_EDIT = e("MPC-E113", "bad_edit", Session, 2, "K-SCH-2", CHANGE,
    "bad edit: {reason}",
    "The edit itself is malformed (the message says how). Fix the arguments and run it \
     again; nothing was applied.");

BAD_POSITION = e("MPC-E114", "bad_position", Session, 2, "K-PRES-6",
    &["cursor", "view", "claim", "blame"],
    "bad position: {reason}",
    "A line or position is outside the doc. Read the doc's length first (wc -l, or cat -n) \
     and pass lines inside it.");

BAD_PARTICIPANT = e("MPC-E115", "bad_participant", Session, 2, "K-PRES-9",
    &["(host)"],
    "bad participant: {reason}",
    "The host passed an identity the kit refuses (the message says which field). This is \
     the host's to fix, not the caller's.");

// 120–129: the text tools

TOOL_UNSUPPORTED = e("MPC-E120", "tool_unsupported", Session, 2, "K-PAR-7", RUN_TOOL,
    "{flag}: not supported ({hint})",
    "That verb, flag or syntax is outside the supported subset; nothing is approximated. \
     Use what the hint says, or see capabilities for each verb's flags. Nothing was \
     applied.");

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
     patch, or apply the rest with --partial.");

PATCH_REJECTS = e("MPC-E124", "patch_rejects", Session, 1, "K-PAR-8, K-BR-9",
    &["patch", "apply"],
    "{failed} out of {hunks} hunk(s) FAILED; the rest applied (--partial)",
    "--partial applied the hunks that fit; the failed ones (on stdout) were not applied. \
     Read the lines they were for and make those changes again on the live text.");

REPLACE_NO_MATCH = e("MPC-E125", "replace_no_match", Session, 1, "A12 (exact replace)",
    &["replace"],
    "replace: --old matches nothing in {doc}; nothing applied",
    "The exact text in --old isn't in the doc (it matches byte for byte, with \\n line \
     breaks). stdout lists near misses. Read the lines again (cat -n, grep -n) and copy the \
     text exactly; nothing was applied.");

REPLACE_NOT_UNIQUE = e("MPC-E126", "replace_not_unique", Session, 1, "A12 (exact replace)",
    &["replace"],
    "replace: --old matches {matches} times in {doc}; nothing applied",
    "--old must match exactly once. Add surrounding text to --old until it matches once, or \
     pass --all to replace every match. stdout lists where each match is; nothing was \
     applied.");

PACK_VERB_NO = e("MPC-E127", "pack_verb_no", Session, 1, "A6 (pack verbs)", &["@pack"],
    "{verb}: nothing applied",
    "The file type's own verb found nothing to change (stdout says why: e.g. no such \
     section). Check its target (cat, grep -n) and run it again; nothing was applied.");

// 130–139: approvals

HELD_BACKLOG = e("MPC-E130", "held_backlog", Session, 2, "K-HIST-23, K-ID-10", CHANGE,
    "held: approvals_backlog",
    "Too many of your changes made for a person are waiting for their approval, so new \
     ones are held (nothing applied). This is refused for now, not for good: don't retry in \
     a loop. Ask the person to approve or decline the pending changes (log --pending lists \
     them), then run the verb again.");

NO_APPROVAL = e("MPC-E131", "no_approval", Session, 4, "K-ID-11", &["approve", "decline"],
    "change {seq} has no approval (it wasn't made on anyone's behalf)",
    "That change isn't waiting for anyone's approval (it wasn't made on someone's behalf, \
     or doesn't exist). List the pending ones with log --pending and use one of those.");

BAD_APPROVAL = e("MPC-E132", "bad_approval", Session, 2, "K-ID-11", &["approve", "decline"],
    "bad approval: {reason}",
    "An approval is settled once: pending → approved or declined. This one is already \
     settled (the message says how). To reverse a declined change, use revert or restore \
     --version.");

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
    .indirect("a pack with a non-text root");

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

// 160–169: the catalog itself and the engine's seam

UNKNOWN_ID = e("MPC-E160", "unknown_error_id", Session, 4, "the catalog (docs/ERRORS.md)",
    &["explain"],
    "no error id {id}",
    "That id isn't in this build's catalog. Run explain with no id to list every id, or \
     check the id you copied (MPC-E and three digits).");

ENGINE_UNLISTED = e("MPC-E161", "engine_unlisted", Session, 2, "the MPCE seam (stable codes)",
    ANY,
    "the engine refused this ({code}), and this build's catalog doesn't list the code",
    "A newer engine refused something this kit build can't explain. Nothing was applied. \
     Report the code; the engine's own docs describe it.")
    .reserved("the native engine (MPCE ranges at G2-core)");

// ---------------------------------------------------------------- 2xx history and undo

UNDO_CONFLICT = e("MPC-E201", "undo_conflict", History, 1,
    "U13, U11, hist.undo_atomic", &["undo", "redo", "revert"],
    "step {step} conflicts in {count} region(s) at {where}; nothing applied",
    "Someone else changed text your change touched, so undoing it whole would overwrite \
     their work. Nothing was applied. Either run undo --partial (undoes the regions nobody \
     touched and reports the rest; revert --partial for revert), or read the lines in \
     details.conflicts (cat -n) and fix them by hand with replace.");

UNDO_BLOCKED = e("MPC-E202", "undo_blocked", History, 1,
    "U14, U15, hist.undo_partial", &["undo --partial", "revert"],
    "every region of the top step conflicts ({count} at {where}); nothing applied",
    "Others changed every region of your newest change, so nothing could be undone and it \
     stays on top of your undo history. Fix those lines by hand (replace --lines), then \
     skip the step with undo --drop to reach older ones.");

UNDO_SKIPPED = e("MPC-E203", "undo_skipped", History, 1, "U14, hist.undo_partial",
    &["undo --partial", "revert"],
    "undid {steps} step(s); {skipped} region(s) were left alone because others changed \
     them: {where}",
    "The regions nobody touched were undone; the ones listed were not (others changed \
     them). Read those lines (cat -n) and decide by hand: replace --lines to finish them, or \
     leave them.");

REVERT_TOO_OLD = e("MPC-E204", "revert_too_old", History, 8, "K-HIST-23, K-EXIT-1",
    &["revert"],
    "change {seq} is too old to revert (revert_too_old); use restore --version",
    "The change is outside the revert window. Never retry the same request. Find a version \
     from before it (log) and restore --version that version, or edit the text by hand.");

NO_SUCH_CHANGE = e("MPC-E205", "no_such_change", History, 4, "K-HIST-19, K-HIST-9",
    &["revert", "restore", "show --version", "diff --version", "version --name"],
    "no change or version {seq} in this doc",
    "There's no change or version by that number. List them with log (changes and \
     versions, newest first) and use one it prints.");

HISTORY_LOST = e("MPC-E206", "history_lost", History, 0, "U6, U18, K-HIST-17", UNDO,
    "nothing to undo: your undo history was lost when the doc reloaded",
    "Undo history lives in memory and was dropped when the doc unloaded or the daemon \
     restarted. Your changes are still in the log: find one with log --by <you> and \
     reverse it with revert <change>.");

CLEARED_BY_PURGE = e("MPC-E207", "cleared_by_purge", History, 0, "U19, K-PURGE-4", UNDO,
    "nothing to undo: a purge cleared the undo history",
    "An owner purged content from this doc, which clears every undo history. Reverse a \
     change with revert <change> (log lists them), or edit the text by hand.");

// ---------------------------------------------------------------- 3xx presence and claims

CLAIMED = e("MPC-E301", "claimed", Presence, 5, "A13 (claims), K-PRES-12",
    &["claim", "@change"],
    "{lines} claimed by {name} ({by}); {remaining_ms} ms left",
    "Someone else is working on those lines right now (a live claim). Nothing was applied. \
     Wait and retry after details.remaining_ms, or work on other lines; activity and who \
     show who is where.");

NO_CLAIM = e("MPC-E302", "no_claim", Presence, 4, "A13 (claims)",
    &["claim --renew", "claim --release"],
    "no live claim {claim}",
    "That claim already ended (it expired, or your edit ended it), or you hold none. Take a \
     new one with claim --lines a,b if you still need it.");

CLAIM_CONTENDED = e("MPC-E303", "claim_contended", Presence, 2, "A13 (claims)",
    &["claim --renew"],
    "{claim} has {waiters} waiting; it can't be renewed or held ({remaining_ms} ms left)",
    "Others are waiting for your claim, so it can't be extended. Finish your edit before it \
     ends (details.remaining_ms), or release it with claim --release.");

BAD_CLAIM = e("MPC-E304", "bad_claim", Presence, 2, "A13 (claims)",
    &["claim", "claim --renew", "claim --release"],
    "bad claim: {reason}",
    "The claim call isn't valid as given (the message says why: not your claim, a zero \
     ttl, several claims and no --id...). Fix the arguments; claim --release with no id \
     releases all of yours.");

WATCHING = e("MPC-E305", "watching", Presence, 3, "K11 (participants_max)", CHANGE,
    "watching: the doc has its {max} participants; read-only until the host lets you in",
    "This doc already has as many participants as it allows, so you joined as a watcher: \
     you can read it and follow changes, but not edit or show presence. This is refused for \
     now, not for good: don't retry in a loop. When someone leaves, the host can let you in; \
     until then, read.");

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
     or fix what the reason names; it goes live again when it decodes.");

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
    .indirect("an I/O failure");

BAD_PATH = e("MPC-E411", "bad_path", Disk, 2, "K-SEC-6, K-DISK-14", &["mv", "(host)"],
    "bad path: {reason}",
    "The path isn't a regular file inside the workspace (a folder, a link pointing outside \
     it...). Use a path to a regular file inside the workspace.");

DECLINED_HARD_LINKED = e("MPC-E412", "declined_hard_linked", Disk, 4, "K-DISK-14 (K9)",
    &["(host)"],
    "live editing is off: {reason} (hard_linked)",
    "The file has more than one name (a hard link), so edits made live through one name \
     would change the others unseen. It opens the ordinary way: edit it with ordinary file \
     tools, or give it a single name (copy it, remove the extra link) and open it again.");

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
    "store still locked by another process after waiting",
    "Another process holds the store (a daemon hand-off in progress). Retry in a few \
     seconds.")
    .indirect("a second process holding the store");

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
    .indirect("a doc deleted between calls");

STORE_NO_VERSION = e("MPC-E511", "store_no_version", Store, 4, "K-HIST-9",
    &["show --version", "restore", "version --name"],
    "no version {version} for doc {doc}",
    "There's no such version. List versions with log and use one it prints.");

STORE_NO_APPROVAL = e("MPC-E512", "store_no_approval", Store, 4, "K-ID-11",
    &["approve", "decline"],
    "no approval for change {seq} of doc {doc}",
    "That change has no approval record. List pending ones with log --pending.")
    .indirect("an approval record removed between calls");

UNLOADED = e("MPC-E513", "unloaded", Store, 1, "K-PERF-7", ANY,
    "session unloaded; open the doc again",
    "The doc was unloaded while you held it (idle, or memory pressure). Run the verb \
     again: it reopens the doc. Nothing was applied.");

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
     the file should exist; open it again to start a new doc.");

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

// ---------------------------------------------------------------- 6xx erasure

PURGE_CURRENT = e("MPC-E601", "purge_current_version", Erasure, 2, "K-T0-9, K-PURGE-1",
    &["purge"],
    "version {version} of doc {doc} is its current content",
    "The current version of a file can't be purged. Write or restore another version \
     first, then purge this one.");

PURGED_VERSION = e("MPC-E602", "purged_version", Erasure, 4, "K-PURGE-4",
    &["show --version", "restore"],
    "version {version} of doc {doc} was purged",
    "An owner purged that version; its content is gone for good. Pick another version \
     (log).");

// ---------------------------------------------------------------- 7xx tier 0 and blobs

LEASED = e("MPC-E701", "leased", Blob, 5, "K-T0-2", &["lease", "write", "restore"],
    "leased by {name} ({holder}) until {expires_ms}",
    "Someone else holds the file's lease (a lock while they change it). Nothing was \
     written. Retry after it expires (details.expires_ms, host clock), or ask them to \
     release it; only an owner can break it (lease --break).");

NO_LEASE = e("MPC-E702", "no_lease", Blob, 2, "K-T0-7", &["lease --renew"],
    "no lease held",
    "You hold no lease to renew (it expired, or the daemon restarted). Take a new one with \
     lease.");

BLOB_TOO_LARGE = e("MPC-E703", "blob_too_large", Blob, 4, "K-T0-1", &["write", "(host)"],
    "blob is over the {limit}-byte limit",
    "The file is over the blob store's size limit, so it isn't versioned. It opens the \
     ordinary way; edit it with ordinary file tools.");

BLOB_NOT_FOUND = e("MPC-E704", "blob_not_found", Blob, 4, "K-T0-1",
    &["cat", "show --version", "restore"],
    "no blob {blob} in this space",
    "The bytes of that version are gone (collected, or the space was purged). Pick another \
     version (log).")
    .indirect("a blob file collected under a live version");

BLOB_DAMAGED = e("MPC-E705", "blob_damaged", Blob, 1, "K-T0-4",
    &["cat", "show --version", "restore"],
    "blob {blob} is damaged (its bytes don't match its id)",
    "The stored bytes of that version are damaged and are never served. Pick another \
     version (log), and report it.")
    .indirect("damaged blob bytes on disk");

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
    .indirect("a blob index row removed under a version");

// ---------------------------------------------------------------- 8xx structured and CSV
// reserved for the CSV grid work (C1c's refusals); C1 wires them.

NEEDS_BASE = e("MPC-E801", "needs_base", Structured, 2, "K-BR-12", &["cell set", "@change"],
    "an agent's grid edit needs a base: read first and pass --base",
    "Agents edit tables against what they read. Read the table first (it prints the \
     version), then run the edit again with --base <version>.")
    .reserved("CSV grids (C1c)");

AT_LIMIT = e("MPC-E802", "at_limit", Structured, 2, "K-CSV-7 (KC4)", &["@change"],
    "this table is at its size limit ({limit} {what}, deleted rows included); the owner can \
     free space with `rows drop-deleted`",
    "The table can't grow past its limit (details.what: cells or columns); nothing was \
     applied and nothing was dropped. Remove rows or columns first, or ask an owner to run \
     rows drop-deleted to free the space deleted rows still hold.")
    .reserved("CSV grids (C1c)");

FACET_CONFLICT = e("MPC-E803", "facet_conflict", Structured, 1, "K-BR-12 (KC7)",
    &["@change"],
    "{count} element(s) changed since your base: {where}",
    "Someone changed the same cells or rows after the version you read. Nothing was \
     applied. Read again, redo your change on the new values, and pass the new --base.")
    .reserved("CSV grids (C1c)");

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
     edits.");

CLAIMS_DOC_CAP = e("MPC-E903", "claims_doc_cap", Limits, 2, "A13 (claims_max)", &["claim"],
    "bad claim: this doc has {count} live claims (the most)",
    "This doc has as many live claims as it allows. Wait for some to end (activity and who \
     show them), or edit without a claim.");

CLAIMS_PARTICIPANT_CAP = e("MPC-E904", "claims_participant_cap", Limits, 2,
    "A13 (claims_per_participant)", &["claim"],
    "bad claim: you hold {count} claims here (the most); release one",
    "You hold as many claims on this doc as you may. Release one you no longer need \
     (claim --release --id ID), then claim again.");

CLAIM_WAITERS_CAP = e("MPC-E905", "claim_waiters_cap", Limits, 2,
    "A13 (claim_waiters_max)", &["(host)"],
    "bad claim: {count} already waiting for {claim}",
    "That claim already has as many waiters as it allows. Retry after it ends, or work \
     elsewhere.");
}

registry! { ENGINE:
}
