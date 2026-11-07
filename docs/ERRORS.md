# Errors

<!-- Generated from crates/base/src/errors/registry.rs by `errors_markdown()`; don't edit by hand. Regenerate: MPC_BLESS=1 cargo test -p k2-mpc-base --test error_ids. -->

Every refusal the kit returns has a stable id, `MPC-Ennn` (the engine's are `MPCE-Ennn`). An id is never reused and never changes its exit code; a retired id stays listed here. Each entry names the rule it enforces: requirement ids (`K-…`, the requirements PRD), undo spec rules (`U…`, `docs/SPEC-UNDO.md`), conformance rules (`hist.…`, `docs/CONFORMANCE.md`), amendments (`A13`: `docs/SESSIONS.md`).

A host prints a refusal as **plain text**, the error's own wording with the id appended (`doc changed since 12 (now 14) (MPC-E110)`), or with `--json` as one object:

```json
{"id": "MPC-E110", "exit": 1, "message": "doc changed since 12 (now 14)",
 "next": "Someone changed the doc after …",
 "next_step": {"verb": "cat", "args": [], "why": "Someone changed the doc …"},
 "rule": "A14 (--if-version), K-T0-5", "details": {"expected": 12, "actual": 14}}
```

`details` is structured (numbers, values, lists, objects), never prose. `next_step` is `next` as data: `verb` a kit verb, `args` its arguments, one token each (`{key}`: the refusal's `details.key`), and `why` in prose; it has no host syntax, so a host renders it in its own (K-PAR-10). It is left out when there is nothing to run (wait, ask a person, report it, use ordinary file tools). `explain <ID>` prints an entry; `capabilities` lists the ids each verb can return. Exit codes:

| Exit | Name | Meaning |
|---|---|---|
| 0 | `ok` | success (diff, grep: no difference, a match) |
| 1 | `no` | the tool's own no: grep found nothing, diff found differences, a conflict, a patch that didn't apply; nothing applied unless the message says otherwise |
| 2 | `usage_or_held` | bad usage or an unsupported flag (fix the command), or held for now (MPC-E130: don't retry in a loop) |
| 3 | `refused` | the caller lacks a right; don't retry |
| 4 | `not_found` | no such doc, version, change or claim; or live editing is off |
| 5 | `leased` | someone else holds it for now (a lease or a claim); retry after the time left |
| 6 | `store_unavailable` | the store failed; nothing acknowledged |
| 7 | `link_lost` | the link dropped mid-verb (the host's) |
| 8 | `too_old` | too old (revert, an expired base); never retry the same request |

**Verb groups** in the tables: `*` any verb on a doc; `@change` every verb that changes the doc; `@history` every history verb; `@run_tool` every verb `run_tool` runs; `@pack` a pack's own verbs; `(sync)` an editor's sync connection; `(host)` a host call with no verb.

**File verbs**: a row whose verbs start with `@file` is a refusal of a file type's own verbs, which its host runs on the file and lists in `capabilities` from a file registry (`Registry::for_file`); the names after `@file` are those verbs (`(host)`: a host call on the file, no verb). The host maps the file type's refusals to these rows; a format refusal's exact code is in `details.code`.

## 0xx: exports

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E000` | `not_applied` | 1 | client/WIRE.md (a lost reply) | `(sync)` | live |
| `MPC-E001` | `export_unsupported` | 2 | X-ERR-1 | `(host)` | live |
| `MPC-E002` | `export_declined` | 2 | X-ERR-1 | `(host)` | live |
| `MPC-E003` | `export_limit` | 2 | X-LIM-1 | `(host)` | live |
| `MPC-E004` | `export_cancelled` | 1 | X-CAN-1 | `(host)` | live |
| `MPC-E005` | `export_decode` | 1 | X-ERR-1 | `(host)` | live |
| `MPC-E006` | `export_io` | 1 | X-ERR-1 | `(host)` | live |
| `MPC-E007` | `export_internal` | 1 | X-FUZZ-1 | `(host)` | live |

### MPC-E000 `not_applied`

- **Message:** `the change wasn't applied (its reply was lost)`
- **Next:** The connection dropped before the host answered this change, and the doc's record doesn't list it, so the client kept its text out of the doc and handed it back (a rejection). Re-apply it (the client checks the record first and re-applies only a change that never landed), or dismiss it. Nothing else changed.
- **Rule:** client/WIRE.md (a lost reply)
- **Exit:** 1
- **Reached by:** the JS client: a change whose reply was lost, settled by a since reply

### MPC-E001 `export_unsupported`

- **Message:** `no exporter writes {target} from {kind}`
- **Next:** That target isn't offered for this file. List the targets this file can be exported to (the host's export --list) and pick one of them. Nothing was written.
- **Rule:** X-ERR-1
- **Exit:** 2

### MPC-E002 `export_declined`

- **Message:** `the exporter declined ({code})`
- **Next:** The exporter refused this input or these options (details.code says which; bad_option: an option the target doesn't have, or a value it doesn't take). Fix the options, or export to another target. Nothing was written.
- **Rule:** X-ERR-1
- **Exit:** 2

### MPC-E003 `export_limit`

- **Message:** `export limit: {limit}`
- **Next:** The export would pass a limit (details.limit: input or output bytes, pixels, pages, time). Nothing was placed. Export fewer pages or a smaller size, or ask the host's operator about the limit.
- **Rule:** X-LIM-1
- **Exit:** 2

### MPC-E004 `export_cancelled`

- **Message:** `export cancelled`
- **Next:** The export was cancelled before it finished; nothing was placed. Run it again if you still want the file.
- **Next step:** `export`. The export was cancelled before it finished and nothing was placed; run it again if you still want the file.
- **Rule:** X-CAN-1
- **Exit:** 1

### MPC-E005 `export_decode`

- **Message:** `could not read the source`
- **Next:** The source version couldn't be read in its format (damaged, or a variant the exporter doesn't read). Nothing was written. Export as-is to get the bytes, or try another version.
- **Next step:** `export` with `--to` `as-is`. The source couldn't be read in its format; export it as it is to get the bytes, or try another version.
- **Rule:** X-ERR-1
- **Exit:** 1

### MPC-E006 `export_io`

- **Message:** `export i/o failed`
- **Next:** Reading the source or writing the output failed. Nothing was placed. Check the destination (space, permissions), then run it again.
- **Rule:** X-ERR-1
- **Exit:** 1

### MPC-E007 `export_internal`

- **Message:** `exporter {exporter} failed`
- **Next:** The exporter failed in a way it shouldn't: a bug, caught (nothing was placed and the doc is unchanged). Report it with details.exporter and the source's format; export to another target meanwhile.
- **Rule:** X-FUZZ-1
- **Exit:** 1

## 1xx: sessions and edits

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E101` | `forbidden_read` | 3 | K-PERM-2 | `*` | live |
| `MPC-E102` | `forbidden_edit` | 3 | K-PAR-6, K-PERM-2 | `@change` | live |
| `MPC-E103` | `forbidden_history` | 3 | K-PERM-9 | `@history`, `log --pending`, `approvals` | live |
| `MPC-E104` | `forbidden_approver` | 3 | K-ID-11 | `approve`, `decline` | live |
| `MPC-E105` | `forbidden_author_or_owner` | 3 | K-HIST-19 | `revert` | live |
| `MPC-E106` | `forbidden_owner` | 3 | K-PURGE-1, K-T0-2 | `lease --break`, `purge` | live |
| `MPC-E107` | `not_joined` | 2 | K-PRES-1 | `*` | live |
| `MPC-E108` | `no_doc` | 4 | K-EXIT-1 | `*` | live |
| `MPC-E109` | `pack_mismatch` | 4 | K-FMT-2 | `*` | live |
| `MPC-E110` | `stale` | 1 | A14 (--if-version), K-T0-5 | `@change`, `write` | live |
| `MPC-E111` | `write_conflict` | 1 | K-BR-2, K-BR-3 (O1–O5) | `write` | live |
| `MPC-E112` | `base_expired` | 8 | K-BR-4, K-EXIT-1 | `write`, `@pack` | live |
| `MPC-E113` | `bad_edit` | 2 | K-SCH-2 | `@change` | live |
| `MPC-E114` | `bad_position` | 2 | K-PRES-6 | `cursor`, `view`, `claim`, `blame`, `select`, `pointer`, `viewport` | live |
| `MPC-E115` | `bad_participant` | 2 | K-PRES-9 | `(host)` | live |
| `MPC-E120` | `tool_unsupported` | 2 | K-PAR-7 | `@run_tool` | live |
| `MPC-E121` | `tool_refused` | 2 | K-PAR-7, K-SEC-6 | `@run_tool` | live |
| `MPC-E122` | `tool_usage` | 2 | K-EXIT-1 | `@run_tool` | live |
| `MPC-E123` | `patch_failed` | 1 | K-PAR-8 | `patch`, `apply` | live |
| `MPC-E124` | `patch_rejects` | 1 | K-PAR-8, K-BR-9 | `patch`, `apply` | live |
| `MPC-E125` | `replace_no_match` | 1 | A12 (exact replace) | `replace` | live |
| `MPC-E126` | `replace_not_unique` | 1 | A12 (exact replace) | `replace` | live |
| `MPC-E127` | `pack_verb_no` | 1 | A6 (pack verbs) | `@pack` | live |
| `MPC-E130` | `held_backlog` | 2 | K-HIST-23, K-ID-10 | `@change` | live |
| `MPC-E131` | `no_approval` | 4 | K-ID-11 | `approve`, `decline` | live |
| `MPC-E132` | `bad_approval` | 2 | K-ID-11 | `approve`, `decline` | live |
| `MPC-E140` | `decode` | 2 | K-SEC-2 | `(sync)` | live |
| `MPC-E141` | `foreign_client` | 2 | K-ID-5 | `(sync)` | live |
| `MPC-E142` | `missing_deps` | 2 | K-SEC-17 | `(sync)` | live |
| `MPC-E143` | `pending_deps` | 2 | K-SEC-15 | `(sync)` | live |
| `MPC-E144` | `apply_failed` | 2 | K-SEC-2 | `(sync)` | live |
| `MPC-E145` | `engine_panic` | 2 | K-SEC-4 | `(sync)` | live |
| `MPC-E146` | `load_failed` | 2 | K-SEC-5 | `*` | live |
| `MPC-E147` | `bad_client` | 2 | K-ID-2, K-ID-3 | `(sync)` | live |
| `MPC-E148` | `no_root` | 2 | K-SCH-2 | `(host)` | live |
| `MPC-E149` | `wrong_kind` | 2 | K-SCH-2 | `(host)` | live |
| `MPC-E150` | `unsupported_kind` | 2 | K-SCH-1 | `(host)` | live |
| `MPC-E151` | `bad_hunk` | 2 | K-SCH-2 | `(host)` | live |
| `MPC-E152` | `unaccepted` | 3 | K-SEC-2 | `(sync)` | live |
| `MPC-E153` | `bad_root_name` | 2 | K-SCH-1 | `(host)` | live |
| `MPC-E154` | `malformed_doc` | 2 | K-SEC-5 | `*` | live |
| `MPC-E160` | `unknown_error_id` | 4 | the catalog (docs/ERRORS.md) | `explain` | live |
| `MPC-E161` | `engine_unlisted` | 2 | the MPCE seam (stable codes) | `*` | reserved |
| `MPC-E170` | `ambiguous_address` | 2 | K-ADDR-2, K-EXIT-1 | `@pack` | live |
| `MPC-E171` | `positional_needs_version` | 2 | K-ADDR-3 | `@change` | live |
| `MPC-E172` | `address_not_found` | 4 | K-READ-1, K-READ-3 | `@pack` | reserved |
| `MPC-E173` | `stale_cursor` | 1 | K-READ-2 | `@pack` | reserved |
| `MPC-E176` | `line_address_unreliable` | 2 | K-ADDR-4, K-PAR-7 | `@pack` | live |
| `MPC-E177` | `line_address_unknown` | 2 | K-ADDR-4 | `claim`, `@pack` | live |

### MPC-E101 `forbidden_read`

- **Message:** `refused: needs Read rights`
- **Next:** You can't read this doc, and you were removed from it. Don't retry: ask the doc's owner for read access (the host may print how). Nothing changed.
- **Rule:** K-PERM-2
- **Exit:** 3

### MPC-E102 `forbidden_edit`

- **Message:** `refused: needs Edit rights`
- **Next:** You can read this doc but not change it. Don't retry: ask the doc's owner for edit access (the host may print how), or hand the change to someone who can edit. Nothing changed.
- **Rule:** K-PAR-6, K-PERM-2
- **Exit:** 3

### MPC-E103 `forbidden_history`

- **Message:** `refused: needs History rights`
- **Next:** Reading history (log, diff --version, show --version, blame, versions, someone else's approvals) needs the history right, which you don't have. Don't retry: work from the live text (cat), or ask the doc's owner for history access.
- **Next step:** `cat`. Reading history needs a right you don't have; work from the live text instead, or ask the doc's owner for history access.
- **Rule:** K-PERM-9
- **Exit:** 3

### MPC-E104 `forbidden_approver`

- **Message:** `refused: needs Approver rights`
- **Next:** Only the person a change was made for can approve or decline it; an agent never can, even the one that made it. Ask that person to run approve or decline (list the pending changes with log --pending). Don't retry.
- **Next step:** `log --pending`. Only the person a change was made for can approve or decline it; list the pending changes and ask that person.
- **Rule:** K-ID-11
- **Exit:** 3

### MPC-E105 `forbidden_author_or_owner`

- **Message:** `refused: needs AuthorOrOwner rights`
- **Next:** Only the change's author, the person it was made for, or an owner can revert it. Don't retry: ask one of them, or undo your own changes with undo.
- **Next step:** `undo`. Only the change's author, the person it was made for or an owner can revert it; undo your own changes instead, or ask one of them.
- **Rule:** K-HIST-19
- **Exit:** 3

### MPC-E106 `forbidden_owner`

- **Message:** `refused: needs Owner rights`
- **Next:** Only an owner of the doc can do this (break someone else's lease, purge). Don't retry: ask an owner. To wait for a lease instead, retry after it expires (status shows the holder and expiry).
- **Next step:** `status`. Only an owner can do this; ask one, or see who holds the lease and until when, and retry after it expires.
- **Rule:** K-PURGE-1, K-T0-2
- **Exit:** 3

### MPC-E107 `not_joined`

- **Message:** `{conn} has not joined this doc`
- **Next:** Your connection left the doc (it was idle, kicked, or the doc reloaded). Run the verb again: any verb joins the doc first. Nothing was applied.
- **Next step:** `open`. Your connection left the doc; join it again, then run your verb. Nothing was applied.
- **Rule:** K-PRES-1
- **Exit:** 2

### MPC-E108 `no_doc`

- **Message:** `no doc at {key}`
- **Next:** There is no live doc at that path and no file to open it from. Check the path (the host resolves it inside the workspace); create the file first if it should exist.
- **Rule:** K-EXIT-1
- **Exit:** 4

### MPC-E109 `pack_mismatch`

- **Message:** `doc was stored by pack {stored}, not {pack}`
- **Next:** The stored doc belongs to another file type than the one asked for (the path's type changed, or the host routed it to the wrong pack). Open it as its stored type, or ask the host's operator; nothing changed.
- **Rule:** K-FMT-2
- **Exit:** 4

### MPC-E110 `stale`

- **Message:** `doc changed since {expected} (now {actual})`
- **Next:** Someone changed the doc after the version you named, so nothing was applied. Read it again (cat prints the version), redo your change on what you read, and pass the new version with --if-version (a blob write: the new base).
- **Next step:** `cat`. Someone changed the doc after the version you named; read it again, redo your change on what you read, and guard it with the new version.
- **Rule:** A14 (--if-version), K-T0-5
- **Exit:** 1

### MPC-E111 `write_conflict`

- **Message:** `your write touches lines someone else changed since version {base}: {count} region(s), your {lines} (now {lines} of version {current}); nothing applied. To retry: read the current text again (cat; it is version {current}), redo your change on it, then write --base {current}`
- **Next:** Someone else changed the same lines after the version you read (or added lines at the same place), so nothing was applied; a write never merges inside a line. Run cat to read the current text (the message and details.current give its version), make your change again on that text, and write it with write --base <that version>. Or change only the lines you need with replace --lines or patch, each with --if-version <that version> (they take --if-version, not --base). details.conflicts has each region's your_lines (in your text), current_lines (in the current text), base, ours and theirs.
- **Next step:** `cat`. Someone else changed the same lines since your base; read the current text, redo your change on it, then write it against that version.
- **Rule:** K-BR-2, K-BR-3 (O1–O5)
- **Exit:** 1

### MPC-E112 `base_expired`

- **Message:** `base version {base} is unknown or no longer cached; nothing applied`
- **Next:** The version you wrote against is too old to merge from (or was never read). Never retry the same request: read the doc again (cat), redo your change on what you read, and write with that version as --base.
- **Next step:** `cat`. The version you wrote against is too old to merge from; read the doc again, redo your change, and write against the version you read. Never retry the same request.
- **Rule:** K-BR-4, K-EXIT-1
- **Exit:** 8

### MPC-E113 `bad_edit`

- **Message:** `bad edit: {reason}`
- **Next:** The edit itself is malformed (the message says how). Fix the arguments and run it again; nothing was applied.
- **Rule:** K-SCH-2
- **Exit:** 2

### MPC-E114 `bad_position`

- **Message:** `bad position: {reason}`
- **Next:** A line, position or element isn't in the doc (the message says which). Read the doc and pass one that is.
- **Next step:** `cat`. A line, position or element isn't in the doc; read the doc and pass one that is.
- **Rule:** K-PRES-6
- **Exit:** 2

### MPC-E115 `bad_participant`

- **Message:** `bad participant: {reason}`
- **Next:** The host passed an identity the kit refuses (the message says which field). This is the host's to fix, not the caller's.
- **Rule:** K-PRES-9
- **Exit:** 2

### MPC-E120 `tool_unsupported`

- **Message:** `{flag}: not supported ({hint})`
- **Next:** That verb, flag or syntax is outside the supported subset; nothing is approximated. Use what the hint says, or see capabilities for each verb's flags. Nothing was applied.
- **Next step:** `capabilities`. That verb, flag or syntax is outside the supported subset; use what the hint says, or look up each verb's flags.
- **Rule:** K-PAR-7
- **Exit:** 2

### MPC-E121 `tool_refused`

- **Message:** `{what}: refused ({reason})`
- **Next:** The tools refuse this on purpose (sed commands that touch other files or run programs, backup suffixes, backreferences in a pattern). Do it another way: the reason says which. Nothing was applied.
- **Rule:** K-PAR-7, K-SEC-6
- **Exit:** 2

### MPC-E122 `tool_usage`

- **Message:** `{reason}`
- **Next:** The command line is malformed: a missing operand, a bad number, a bad script, pattern or patch (the message says which). Fix it and run again; nothing was applied.
- **Rule:** K-EXIT-1
- **Exit:** 2

### MPC-E123 `patch_failed`

- **Message:** `{failed} out of {hunks} hunk(s) FAILED; nothing applied`
- **Next:** The patch doesn't fit the live text (others changed those lines, or it was made against another version). Nothing was applied: patches are all or nothing. stdout shows each failed hunk beside the live lines. Read the text again and make a new patch, or apply the rest with --partial.
- **Next step:** `cat` with `-n`. The patch doesn't fit the live text, so nothing was applied; read the text again and make a new patch, or apply only the hunks that fit.
- **Rule:** K-PAR-8
- **Exit:** 1

### MPC-E124 `patch_rejects`

- **Message:** `{failed} out of {hunks} hunk(s) FAILED; the rest applied (--partial)`
- **Next:** --partial applied the hunks that fit; the failed ones (on stdout) were not applied. Read the lines they were for and make those changes again on the live text.
- **Next step:** `cat` with `-n`. The hunks that fit were applied and the failed ones were not; read the lines they were for and make those changes again on the live text.
- **Rule:** K-PAR-8, K-BR-9
- **Exit:** 1

### MPC-E125 `replace_no_match`

- **Message:** `replace: --old matches nothing in {doc}; nothing applied`
- **Next:** The exact text in --old isn't in the doc (it matches byte for byte, with \n line breaks). stdout lists near misses. Read the lines again (cat -n, grep -n) and copy the text exactly; nothing was applied.
- **Next step:** `cat` with `-n`. The exact old text isn't in the doc; read the lines again and copy the text byte for byte.
- **Rule:** A12 (exact replace)
- **Exit:** 1

### MPC-E126 `replace_not_unique`

- **Message:** `replace: --old matches {matches} times in {doc}; nothing applied`
- **Next:** --old must match exactly once. Add surrounding text to --old until it matches once, or pass --all to replace every match. stdout lists where each match is; nothing was applied.
- **Next step:** `cat` with `-n`. The old text matches more than once; add surrounding text until it matches once, or replace every match.
- **Rule:** A12 (exact replace)
- **Exit:** 1

### MPC-E127 `pack_verb_no`

- **Message:** `{verb}: nothing applied`
- **Next:** The file type's own verb found nothing to change (stdout says why: e.g. no such section). Check its target (cat, grep -n) and run it again; nothing was applied.
- **Next step:** `cat`. The file type's own verb found nothing to change; check its target and run it again.
- **Rule:** A6 (pack verbs)
- **Exit:** 1

### MPC-E130 `held_backlog`

- **Message:** `held: approvals_backlog`
- **Next:** Too many of your changes made for a person are waiting for their approval, so new ones are held (nothing applied). This is refused for now, not for good: don't retry in a loop. Ask the person to approve or decline the pending changes (log --pending lists them), then run the verb again.
- **Next step:** `log --pending`. Too many of your changes wait for a person's approval, so new ones are held for now; ask that person to settle the pending ones, then run the verb again. Don't retry in a loop.
- **Rule:** K-HIST-23, K-ID-10
- **Exit:** 2

### MPC-E131 `no_approval`

- **Message:** `change {seq} has no approval (it wasn't made on anyone's behalf)`
- **Next:** That change isn't waiting for anyone's approval (it wasn't made on someone's behalf, or doesn't exist). List the pending ones with log --pending and use one of those.
- **Next step:** `log --pending`. That change isn't waiting for anyone's approval; list the pending ones and use one of those.
- **Rule:** K-ID-11
- **Exit:** 4

### MPC-E132 `bad_approval`

- **Message:** `bad approval: {reason}`
- **Next:** An approval is settled once: pending → approved or declined. This one is already settled (the message says how). To reverse a declined change, use revert or restore --version.
- **Next step:** `log --pending` with `--all`. That approval is already settled; list the settled ones to see how, and revert or restore to reverse a declined change.
- **Rule:** K-ID-11
- **Exit:** 2

### MPC-E140 `decode`

- **Message:** `update could not be decoded: {reason}`
- **Next:** An editor sent bytes the kit can't decode; nothing was applied. The editor should resync (reload the doc).
- **Rule:** K-SEC-2
- **Exit:** 2
- **Reached by:** the sync wire: an editor's update bytes

### MPC-E141 `foreign_client`

- **Message:** `update inserts as client {found}, but this connection is client {allowed}`
- **Next:** An update inserted under another participant's client id and was refused whole. The editor should reload; it can't author as someone else.
- **Rule:** K-ID-5
- **Exit:** 2
- **Reached by:** the sync wire: an editor's update bytes

### MPC-E142 `missing_deps`

- **Message:** `update depends on changes this doc doesn't have (client {client} from clock {from}, doc has {have}); resync`
- **Next:** The update builds on changes this doc never received. The editor should resync (send its state vector, then the missing changes).
- **Rule:** K-SEC-17
- **Exit:** 2
- **Reached by:** the sync wire: an editor's update bytes

### MPC-E143 `pending_deps`

- **Message:** `update left unresolved dependencies after apply; the doc needs a reload or resync`
- **Next:** The in-memory doc refused the update and reloads from the store on its own. Retry after a moment; the editor should resync.
- **Rule:** K-SEC-15
- **Exit:** 2
- **Reached by:** the sync wire: an editor's update bytes

### MPC-E144 `apply_failed`

- **Message:** `update could not be applied: {reason}`
- **Next:** The engine refused the update; nothing was applied. The editor should resync.
- **Rule:** K-SEC-2
- **Exit:** 2
- **Reached by:** the sync wire: an editor's update bytes

### MPC-E145 `engine_panic`

- **Message:** `the engine panicked on this input`
- **Next:** The engine failed on this input and it was refused; the doc is unchanged. The editor should resync. Report it: it is a bug.
- **Rule:** K-SEC-4
- **Exit:** 2
- **Reached by:** the sync wire: an engine failure

### MPC-E146 `load_failed`

- **Message:** `state could not be loaded: {reason}`
- **Next:** A stored state couldn't be loaded into the engine. Nothing changed; report it to the host's operator.
- **Rule:** K-SEC-5
- **Exit:** 2
- **Reached by:** a damaged stored state

### MPC-E147 `bad_client`

- **Message:** `invalid client id {client}`
- **Next:** A client id is out of range or reserved. The editor should reconnect for a fresh one.
- **Rule:** K-ID-2, K-ID-3
- **Exit:** 2
- **Reached by:** the sync wire: a client id the host passes

### MPC-E148 `no_root`

- **Message:** `schema {schema} has no root {root}`
- **Next:** The edit named a part of the doc this file type doesn't have. Text verbs need no root; a host passing one should use the pack's root names.
- **Rule:** K-SCH-2
- **Exit:** 2

### MPC-E149 `wrong_kind`

- **Message:** `root {root} is {actual}, not {wanted}`
- **Next:** The edit treats a part of the doc as the wrong kind (text vs table...). Use the verbs for that kind (capabilities lists them).
- **Next step:** `capabilities`. The edit treats a part of the doc as the wrong kind; use the verbs for that kind.
- **Rule:** K-SCH-2
- **Exit:** 2
- **Reached by:** a pack with a non-text root

### MPC-E150 `unsupported_kind`

- **Message:** `primitive {kind} is not implemented yet`
- **Next:** This file type needs a primitive this build doesn't have; it can't open live.
- **Rule:** K-SCH-1
- **Exit:** 2
- **Reached by:** a pack declaring an unimplemented kind

### MPC-E151 `bad_hunk`

- **Message:** `{reason}`
- **Next:** An edit's ranges are out of order, out of range, off a character boundary or empty. The host computes ranges from the text it read: read again and recompute.
- **Rule:** K-SCH-2
- **Exit:** 2

### MPC-E152 `unaccepted`

- **Message:** `update contains content this host doesn't accept`
- **Next:** An editor sent an update this build does not accept; it was refused whole and nothing was applied. Don't resend it: the editor should reload the doc. Report it if it happens again.
- **Rule:** K-SEC-2
- **Exit:** 3
- **Reached by:** the sync wire: an update this build does not accept

### MPC-E153 `bad_root_name`

- **Message:** `schema root name {root}: names must be non-empty, unique and free of '/'`
- **Next:** A pack's schema names a root the kit can't store (empty, holding '/', or twice). The pack has to fix its schema; this file type can't open live until then.
- **Rule:** K-SCH-1
- **Exit:** 2
- **Reached by:** a pack with a bad schema (C1)

### MPC-E154 `malformed_doc`

- **Message:** `the doc's state is malformed: {reason}`
- **Next:** The doc holds something the kit never writes (a bug, or a damaged store). Nothing was applied. Don't retry: report it to the host's operator with this message.
- **Rule:** K-SEC-5
- **Exit:** 2
- **Reached by:** a damaged doc state (C1)

### MPC-E160 `unknown_error_id`

- **Message:** `no error id {id}`
- **Next:** That id isn't in this build's catalog. Run explain with no id to list every id, or check the id you copied (MPC-E and three digits).
- **Next step:** `explain`. That id isn't in this build's catalog; list every id, or check the one you copied.
- **Rule:** the catalog (docs/ERRORS.md)
- **Exit:** 4

### MPC-E161 `engine_unlisted`

- **Message:** `the engine refused this ({code}), and this build's catalog doesn't list the code`
- **Next:** A newer engine refused something this kit build can't explain. Nothing was applied. Report the code; the engine's own docs describe it.
- **Rule:** the MPCE seam (stable codes)
- **Exit:** 2
- **Status:** reserved for the native engine (MPCE ranges at G2-core); nothing returns it yet

### MPC-E170 `ambiguous_address`

- **Message:** `{given} names more than one {what}`
- **Next:** The address you gave names more than one thing (a name or a value two of them share), so nothing was read or applied; the kit never picks the first one. details.candidates lists each one's own address (its key or ID) and where it is now: run the verb again with the address of the one you mean.
- **Rule:** K-ADDR-2, K-EXIT-1
- **Exit:** 2

### MPC-E171 `positional_needs_version`

- **Message:** `{address} is a position; an agent's edit by position names the version it read`
- **Next:** Positions (#N, LN, line numbers, the Nth match) move when others edit, so an agent's edit that uses one must say which version it counted them in. Nothing was applied. Read the doc again (the read prints its version) and run the edit with --if-version <that version> (a grid edit: --base <that version>), or name the thing by its key.
- **Rule:** K-ADDR-3
- **Exit:** 2

### MPC-E172 `address_not_found`

- **Message:** `{count} of the addresses name nothing in version {version}`
- **Next:** Some of the addresses you asked for name nothing in this version. The output still lists every address, with null for each missing one; details.missing gives each missing address with what does exist there (candidates, its parent's children). Check them against outline, or ask for one of the candidates.
- **Rule:** K-READ-1, K-READ-3
- **Exit:** 4
- **Status:** reserved for the node verb (K-READ-1); nothing returns it yet

### MPC-E173 `stale_cursor`

- **Message:** `the cursor is for another version, doc or call; nothing was read`
- **Next:** A cursor continues one read of one doc at one version with the same arguments. The doc changed since (or the cursor is from another doc, or the arguments differ), so nothing was read. Read again without --cursor and page on with the cursor that read returns.
- **Rule:** K-READ-2
- **Exit:** 1
- **Status:** reserved for read budgets and cursors (K-READ-2); nothing returns it yet

### MPC-E176 `line_address_unreliable`

- **Message:** `{address}: this file holds a lone CR, so its line numbers have no one meaning; name the row by its key or #N (row list shows both)`
- **Next:** A lone CR (a carriage return with no line feed after it, inside a value or ending the records) ends a line for the table's rendered lines but not for the tools that read like cat -n, so an LN address could name a row other than the one you read. Nothing was read or applied. Name the row by its key (r17) or its #N: row list shows both for every row.
- **Next step:** `row list`. Line numbers have no one meaning in this file, so list the rows to get each one's key and position, then address the row by one of those.
- **Rule:** K-ADDR-4, K-PAR-7
- **Exit:** 2

### MPC-E177 `line_address_unknown`

- **Message:** `{address}: the file can't be rendered now, so its line numbers can't be checked; name the row by its key or #N`
- **Next:** The pack can't render the file at the moment, so the kit can't tell whether it holds a lone CR, and a line number might name a row other than the one you read. Nothing was read or applied. Name the row by its key (r17) or its #N, which don't depend on rendering; blame --structure lists both. A line number works again once the file renders.
- **Next step:** `blame --structure`. Line numbers can't be checked while the file can't be rendered, so list the rows to get each one's key and position, then address the row by one of those.
- **Rule:** K-ADDR-4
- **Exit:** 2

## 2xx: history and undo

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E201` | `undo_conflict` | 1 | U13, U11, hist.undo_atomic | `undo`, `redo`, `revert` | live |
| `MPC-E202` | `undo_blocked` | 1 | U14, U15, hist.undo_partial | `undo --partial`, `revert` | live |
| `MPC-E203` | `undo_skipped` | 1 | U14, hist.undo_partial | `undo --partial`, `revert` | live |
| `MPC-E204` | `revert_too_old` | 8 | K-HIST-23, K-EXIT-1 | `revert` | live |
| `MPC-E205` | `no_such_change` | 4 | K-HIST-19, K-HIST-9 | `revert`, `restore`, `show --version`, `diff --version`, `version --name` | live |
| `MPC-E206` | `history_lost` | 0 | U6, U18, K-HIST-17 | `undo`, `redo`, `undo --partial`, `undo --drop` | live |
| `MPC-E207` | `cleared_by_purge` | 0 | U19, K-PURGE-4 | `undo`, `redo`, `undo --partial`, `undo --drop` | live |

### MPC-E201 `undo_conflict`

- **Message:** `step {step} conflicts in {count} region(s) at {where}; nothing applied`
- **Next:** Someone else changed text your change touched, so undoing it whole would overwrite their work. Nothing was applied. Either run undo --partial (undoes the regions nobody touched and reports the rest; revert --partial for revert), or read the lines in details.conflicts (cat -n) and fix them by hand with replace.
- **Next step:** `undo --partial`. Undoing the change whole would overwrite others' work; undo only the regions nobody touched, or fix the conflicting lines by hand.
- **Rule:** U13, U11, hist.undo_atomic
- **Exit:** 1

### MPC-E202 `undo_blocked`

- **Message:** `every region of the top step conflicts ({count} at {where}); nothing applied`
- **Next:** Others changed every region of your newest change, so nothing could be undone and it stays on top of your undo history. Fix those lines by hand (replace --lines), then skip the step with undo --drop to reach older ones.
- **Next step:** `undo --drop`. Others changed every region of your newest change; fix those lines by hand, then skip the step to reach older ones.
- **Rule:** U14, U15, hist.undo_partial
- **Exit:** 1

### MPC-E203 `undo_skipped`

- **Message:** `undid {steps} step(s); {skipped} region(s) were left alone because others changed them: {where}`
- **Next:** The regions nobody touched were undone; the ones listed were not (others changed them). Read those lines (cat -n) and decide by hand: replace --lines to finish them, or leave them.
- **Next step:** `cat` with `-n`. The regions others changed were left alone; read those lines and finish them by hand, or leave them.
- **Rule:** U14, hist.undo_partial
- **Exit:** 1

### MPC-E204 `revert_too_old`

- **Message:** `change {seq} is too old to revert (revert_too_old); use restore --version`
- **Next:** The change is outside the revert window. Never retry the same request. Find a version from before it (log) and restore --version that version, or edit the text by hand.
- **Next step:** `log`. The change is outside the revert window; find a version from before it and restore that version. Never retry the same request.
- **Rule:** K-HIST-23, K-EXIT-1
- **Exit:** 8

### MPC-E205 `no_such_change`

- **Message:** `no change or version {seq} in this doc`
- **Next:** There's no change or version by that number. List them with log (changes and versions, newest first) and use one it prints.
- **Next step:** `log`. There's no change or version by that number; list them and use one shown.
- **Rule:** K-HIST-19, K-HIST-9
- **Exit:** 4

### MPC-E206 `history_lost`

- **Message:** `nothing to undo: your undo history was lost when the doc reloaded`
- **Next:** Undo history lives in memory and was dropped when the doc unloaded or the daemon restarted. Your changes are still in the log: find one with log --by <you> and reverse it with revert <change>.
- **Next step:** `log`. Undo history was lost when the doc reloaded, but your changes are still in the log; find yours and revert it.
- **Rule:** U6, U18, K-HIST-17
- **Exit:** 0

### MPC-E207 `cleared_by_purge`

- **Message:** `nothing to undo: a purge cleared the undo history`
- **Next:** An owner purged content from this doc, which clears every undo history. Reverse a change with revert <change> (log lists them), or edit the text by hand.
- **Next step:** `log`. A purge cleared every undo history; find the change in the log and revert it, or edit the text by hand.
- **Rule:** U19, K-PURGE-4
- **Exit:** 0

## 3xx: presence and claims

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E301` | `claimed` | 5 | A13 (claims), K-PRES-12 | `claim`, `@change` | live |
| `MPC-E302` | `no_claim` | 4 | A13 (claims) | `claim --renew`, `claim --release` | live |
| `MPC-E303` | `claim_contended` | 2 | A13 (claims) | `claim --renew` | live |
| `MPC-E304` | `bad_claim` | 2 | A13 (claims) | `claim`, `claim --renew`, `claim --release` | live |
| `MPC-E305` | `watching` | 3 | K11 (participants_max) | `@change` | live |
| `MPC-E310` | `selection_too_large` | 2 | K-PRES-6 | `select` | live |

### MPC-E301 `claimed`

- **Message:** `{lines} claimed by {name} ({by}); {remaining_ms} ms left`
- **Next:** Someone else is working on those lines right now (a live claim). Nothing was applied. Wait and retry after details.remaining_ms, or work on other lines; activity and who show who is where.
- **Next step:** `activity`. Someone else holds a live claim on those lines; see who is where, then wait for the time left and retry, or work on other lines.
- **Rule:** A13 (claims), K-PRES-12
- **Exit:** 5

### MPC-E302 `no_claim`

- **Message:** `no live claim {claim}`
- **Next:** That claim already ended (it expired, or your edit ended it), or you hold none. Take a new one with claim --lines a,b if you still need it.
- **Next step:** `claim`. That claim already ended, or you hold none; take a new one if you still need it.
- **Rule:** A13 (claims)
- **Exit:** 4

### MPC-E303 `claim_contended`

- **Message:** `{claim} has {waiters} waiting; it can't be renewed or held ({remaining_ms} ms left)`
- **Next:** Others are waiting for your claim, so it can't be extended. Finish your edit before it ends (details.remaining_ms), or release it with claim --release.
- **Next step:** `claim --release` with `--id` `{claim}`. Others are waiting for your claim, so it can't be extended; finish your edit before it ends, or release it.
- **Rule:** A13 (claims)
- **Exit:** 2

### MPC-E304 `bad_claim`

- **Message:** `bad claim: {reason}`
- **Next:** The claim call isn't valid as given (the message says why: not your claim, a zero ttl, several claims and no --id...). Fix the arguments; claim --release with no id releases all of yours.
- **Next step:** `claim --release`. The claim call isn't valid as given; fix its arguments, or release all of your claims.
- **Rule:** A13 (claims)
- **Exit:** 2

### MPC-E305 `watching`

- **Message:** `watching: the doc has its {max} participants; read-only until the host lets you in`
- **Next:** This doc already has as many participants as it allows, so you joined as a watcher: you can read it and follow changes, but not edit or show presence. This is refused for now, not for good: don't retry in a loop. When someone leaves, the host can let you in; until then, read.
- **Next step:** `cat`. The doc has all the participants it allows, so you joined as a watcher; read it until the host lets you in, and don't retry in a loop.
- **Rule:** K11 (participants_max)
- **Exit:** 3

### MPC-E310 `selection_too_large`

- **Message:** `selection too large: {len} keys (the most is {max})`
- **Next:** A selection holds at most 256 elements, and nothing changed. Select fewer (details.max).
- **Next step:** `select` with `--clear`. A selection holds at most 256 elements; clear it, then select fewer.
- **Rule:** K-PRES-6
- **Exit:** 2

## 4xx: disk and outside edits

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E401` | `declined_too_large` | 4 | K-PACK-4, K-DISK-8 | `*` | live |
| `MPC-E402` | `declined_not_utf8` | 4 | K-PACK-4, K-COD-6 | `*` | live |
| `MPC-E403` | `declined_binary` | 4 | K-PACK-4, K-COD-6 | `*` | live |
| `MPC-E404` | `declined_mixed_line_endings` | 4 | K-PACK-4, K-COD-7 | `*` | live |
| `MPC-E405` | `declined_off` | 4 | K-DISK-8 | `@change` | live |
| `MPC-E406` | `declined_unreadable` | 4 | K-PACK-4 | `(host)` | live |
| `MPC-E407` | `declined_pack` | 4 | K-PACK-4, K-COD-6 | `*` | live |
| `MPC-E410` | `disk` | 1 | K-DISK-2, K-DISK-10 | `flush`, `mv`, `(host)` | live |
| `MPC-E411` | `bad_path` | 2 | K-SEC-6, K-DISK-14 | `mv`, `(host)` | live |
| `MPC-E412` | `declined_hard_linked` | 4 | K-DISK-14 (K9) | `(host)` | live |
| `MPC-E413` | `save_mismatch` | 1 | K-T0-5, K-DISK-2 | `(host)` | live |
| `MPC-E414` | `save_ticket_invalid` | 2 | K-T0-5 | `(host)` | live |

### MPC-E401 `declined_too_large`

- **Message:** `live editing is off: {reason} (too_large)`
- **Next:** The file is over the size limit for live editing (capabilities shows the limits). It opens the ordinary way: edit it with ordinary file tools, without live merging. Don't retry the live verb.
- **Rule:** K-PACK-4, K-DISK-8
- **Exit:** 4

### MPC-E402 `declined_not_utf8`

- **Message:** `live editing is off: {reason} (not_utf8)`
- **Next:** The file isn't UTF-8 text, so it can't be a live doc. Edit it with ordinary file tools, or convert it to UTF-8 and open it again. Don't retry the live verb as is.
- **Rule:** K-PACK-4, K-COD-6
- **Exit:** 4

### MPC-E403 `declined_binary`

- **Message:** `live editing is off: {reason} (binary)`
- **Next:** The file holds binary data (a NUL byte), so it can't be a live text doc. Edit it with ordinary file tools. Don't retry the live verb.
- **Rule:** K-PACK-4, K-COD-6
- **Exit:** 4

### MPC-E404 `declined_mixed_line_endings`

- **Message:** `live editing is off: {reason} (mixed_line_endings)`
- **Next:** The file mixes line endings (\n and \r\n), which a live doc can't keep byte for byte, so it isn't live. Either edit it with ordinary file tools, or make its line endings consistent (all \n, or all \r\n) with an ordinary tool and run the verb again: it then opens live.
- **Rule:** K-PACK-4, K-COD-7
- **Exit:** 4

### MPC-E405 `declined_off`

- **Message:** `live editing is off: {reason} (declined)`
- **Next:** The file changed on disk into something that can't be a live doc (the reason says what), so live editing stopped. Reads still work. Edit the file with ordinary tools, or fix what the reason names; it goes live again when it decodes.
- **Next step:** `cat`. The file changed on disk into something that can't be live; reads still work, and it goes live again once it decodes.
- **Rule:** K-DISK-8
- **Exit:** 4

### MPC-E406 `declined_unreadable`

- **Message:** `live editing is off: {reason} (unreadable)`
- **Next:** The file couldn't be read to promote it to a live text doc. It stays a plain file; check it exists and is readable.
- **Rule:** K-PACK-4
- **Exit:** 4
- **Reached by:** a promotion whose file read fails

### MPC-E407 `declined_pack`

- **Message:** `live editing is off: {reason} ({code})`
- **Next:** This file type declined the file (details.code says why, e.g. a CSV it can't keep byte for byte). It opens the ordinary way: edit it with ordinary file tools. Don't retry the live verb as is.
- **Rule:** K-PACK-4, K-COD-6
- **Exit:** 4
- **Reached by:** a pack's own decline code (the kit's text pack uses 401–404)

### MPC-E410 `disk`

- **Message:** `disk: {reason}`
- **Next:** Reading or writing the file failed. Nothing is lost: the doc keeps its changes and write-back retries. Check the disk (space, permissions), then flush.
- **Next step:** `flush`. Reading or writing the file failed, but the doc keeps its changes; check the disk's space and permissions, then write it back.
- **Rule:** K-DISK-2, K-DISK-10
- **Exit:** 1
- **Reached by:** an I/O failure

### MPC-E411 `bad_path`

- **Message:** `bad path: {reason}`
- **Next:** The path isn't a regular file inside the workspace (a folder, a link pointing outside it...). Use a path to a regular file inside the workspace.
- **Rule:** K-SEC-6, K-DISK-14
- **Exit:** 2

### MPC-E412 `declined_hard_linked`

- **Message:** `live editing is off: {reason} (hard_linked)`
- **Next:** The file has more than one name (a hard link), so edits made live through one name would change the others unseen. It opens the ordinary way: edit it with ordinary file tools, or give it a single name (copy it, remove the extra link) and open it again.
- **Rule:** K-DISK-14 (K9)
- **Exit:** 4

### MPC-E413 `save_mismatch`

- **Message:** `the file doesn't hold the bytes saved ({size} bytes given, {file_size} on disk); nothing recorded`
- **Next:** The host recorded a save it made, but the file holds other bytes (details.file_size: what it holds, null when there is no file): another program saved after it, or the save didn't land. Nothing was recorded and the file wasn't touched. Let the file's own change be taken in as an outside save, or save again and record that.
- **Rule:** K-T0-5, K-DISK-2
- **Exit:** 1

### MPC-E414 `save_ticket_invalid`

- **Message:** `bad save ticket: a ticket is good for one record_saved, by the connection that asked for it, on its doc`
- **Next:** A host bug: a save ticket is good for one record_saved, by the connection that asked for it, on the doc that issued it. This one was already recorded (or refused), is another connection's, or isn't this doc's (or is too old to be remembered). Nothing was recorded. A lapsed, replaced or reloaded ticket is not this: it returns with not_recorded set.
- **Rule:** K-T0-5
- **Exit:** 2

## 5xx: store

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E501` | `store_failed` | 6 | K-OFF-7 | `*` | live |
| `MPC-E502` | `store_newer` | 6 | K-FMT-1, K-FMT-10 | `*` | live |
| `MPC-E503` | `store_corrupt` | 6 | K-SEC-5 | `*` | live |
| `MPC-E504` | `path_taken` | 6 | K-DISK-14 | `mv` | live |
| `MPC-E505` | `store_locked` | 6 | K-OFF-2 | `*` | live |
| `MPC-E506` | `named_version` | 6 | K-HIST-6 | `(host)` | live |
| `MPC-E507` | `key_change` | 6 | K-DISK-14 | `(host)` | live |
| `MPC-E508` | `log_moved` | 6 | K-OFF-1 | `*` | live |
| `MPC-E509` | `no_random` | 6 | K-T0-4, K-PURGE-13 | `*` | live |
| `MPC-E510` | `store_no_doc` | 4 | K-EXIT-1 | `*` | live |
| `MPC-E511` | `store_no_version` | 4 | K-HIST-9 | `show --version`, `restore`, `version --name` | live |
| `MPC-E512` | `store_no_approval` | 4 | K-ID-11 | `approve`, `decline` | live |
| `MPC-E513` | `unloaded` | 1 | K-PERF-7 | `*` | live |
| `MPC-E514` | `released` | 1 | K-OFF-2 | `*` | live |
| `MPC-E515` | `broken` | 1 | K-OFF-7 | `*` | live |
| `MPC-E516` | `deleted` | 4 | K-PURGE-8 | `*` | live |
| `MPC-E517` | `pack_error` | 1 | K-COD-2 | `*` | live |
| `MPC-E518` | `quarantined` | 3 | K-SEC-5 | `*` | live |
| `MPC-E519` | `closed` | 1 | K-OFF-2 | `*` | live |
| `MPC-E520` | `engine_mismatch` | 6 | K-FMT-2, K-FMT-4 | `*` | live |

### MPC-E501 `store_failed`

- **Message:** `store: {reason}`
- **Next:** The doc store failed (disk full, fsync failed). Nothing was acknowledged or applied. Retry later; if it persists, the host's operator must free space or fix the disk.
- **Rule:** K-OFF-7
- **Exit:** 6
- **Reached by:** a store I/O failure

### MPC-E502 `store_newer`

- **Message:** `store is format {found}; this build reads up to {max}`
- **Next:** The store was written by a newer build. Nothing changed. Use the newer build.
- **Rule:** K-FMT-1, K-FMT-10
- **Exit:** 6
- **Reached by:** a store from a newer build

### MPC-E503 `store_corrupt`

- **Message:** `store record is damaged: {what}`
- **Next:** A stored record is damaged; nothing was applied. Report it to the host's operator (a backup or the file itself may be needed).
- **Rule:** K-SEC-5
- **Exit:** 6
- **Reached by:** a damaged store record

### MPC-E504 `path_taken`

- **Message:** `path already has a doc: {key}`
- **Next:** Another live doc already has that path. Pick a path no doc uses, or move that doc first.
- **Rule:** K-DISK-14
- **Exit:** 6

### MPC-E505 `store_locked`

- **Message:** `store still locked by another process after waiting`
- **Next:** Another process holds the store (a daemon hand-off in progress). Retry in a few seconds.
- **Rule:** K-OFF-2
- **Exit:** 6
- **Reached by:** a second process holding the store

### MPC-E506 `named_version`

- **Message:** `version {version} of doc {doc} is named; retention never deletes named versions`
- **Next:** Named versions are never deleted by retention. Nothing changed.
- **Rule:** K-HIST-6
- **Exit:** 6
- **Reached by:** retention asked to drop a named version (a kit bug)

### MPC-E507 `key_change`

- **Message:** `put_meta can't change a doc's key; use rekey`
- **Next:** A host call tried to change a doc's path the wrong way; nothing changed. Hosts use rekey (mv).
- **Rule:** K-DISK-14
- **Exit:** 6
- **Reached by:** a store misuse (a kit bug)

### MPC-E508 `log_moved`

- **Message:** `doc {doc}'s log moved under the session: expected position {expected}, found {found}`
- **Next:** The doc's log changed under the session (another process wrote it). Nothing was applied; the doc reloads. Retry.
- **Rule:** K-OFF-1
- **Exit:** 6
- **Reached by:** a second writer of one store

### MPC-E509 `no_random`

- **Message:** `no randomness source: {reason}`
- **Next:** The system's randomness source is unavailable, so no key could be made. Report it to the host's operator.
- **Rule:** K-T0-4, K-PURGE-13
- **Exit:** 6
- **Reached by:** no /dev/urandom

### MPC-E510 `store_no_doc`

- **Message:** `no doc {doc}`
- **Next:** The stored doc is gone (deleted meanwhile). Open the path again.
- **Next step:** `open`. The stored doc was deleted meanwhile; open the path again.
- **Rule:** K-EXIT-1
- **Exit:** 4
- **Reached by:** a doc deleted between calls

### MPC-E511 `store_no_version`

- **Message:** `no version {version} for doc {doc}`
- **Next:** There's no such version. List versions with log and use one it prints.
- **Next step:** `log`. There's no such version; list the versions and use one shown.
- **Rule:** K-HIST-9
- **Exit:** 4

### MPC-E512 `store_no_approval`

- **Message:** `no approval for change {seq} of doc {doc}`
- **Next:** That change has no approval record. List pending ones with log --pending.
- **Next step:** `log --pending`. That change has no approval record; list the pending ones.
- **Rule:** K-ID-11
- **Exit:** 4
- **Reached by:** an approval record removed between calls

### MPC-E513 `unloaded`

- **Message:** `session unloaded; open the doc again`
- **Next:** The doc was unloaded while you held it (idle, or memory pressure). Run the verb again: it reopens the doc. Nothing was applied.
- **Next step:** `open`. The doc was unloaded while you held it; open it again, then run your verb. Nothing was applied.
- **Rule:** K-PERF-7
- **Exit:** 1

### MPC-E514 `released`

- **Message:** `sessions released`
- **Next:** The daemon is handing over to a new process. Retry in a few seconds. Nothing was applied.
- **Rule:** K-OFF-2
- **Exit:** 1

### MPC-E515 `broken`

- **Message:** `session failed: {reason}`
- **Next:** A store write failed and the doc couldn't reload. Retry once the store is healthy; nothing acknowledged is lost.
- **Rule:** K-OFF-7
- **Exit:** 1
- **Reached by:** a store failure during reload

### MPC-E516 `deleted`

- **Message:** `the doc was deleted`
- **Next:** The doc was deleted while you were working on it. Nothing was applied. Check whether the file should exist; open it again to start a new doc.
- **Next step:** `open`. The doc was deleted while you worked on it; check whether the file should exist, and open it again to start a new doc.
- **Rule:** K-PURGE-8
- **Exit:** 4

### MPC-E517 `pack_error`

- **Message:** `pack: {reason}`
- **Next:** The file type couldn't write the doc back (a pack bug or damaged settings). Nothing was lost from the doc. Report it.
- **Rule:** K-COD-2
- **Exit:** 1
- **Reached by:** a pack bug or damaged codec settings

### MPC-E518 `quarantined`

- **Message:** `stored doc contains content this host doesn't accept; it was not loaded`
- **Next:** A stored doc holds content this build does not accept, so it was set aside for review and not loaded. Nothing changed. Don't retry: report it to the host's operator.
- **Rule:** K-SEC-5
- **Exit:** 3
- **Reached by:** a stored doc set aside for review

### MPC-E519 `closed`

- **Message:** `the host closed this doc's live session ({reason})`
- **Next:** The host closed this doc's live session (its reason code says why: a file type switched off, for instance). Everything acknowledged was kept and written to the file. The host decides when it opens again.
- **Rule:** K-OFF-2
- **Exit:** 1

### MPC-E520 `engine_mismatch`

- **Message:** `this doc was written by engine {stored} (format {stored_format}); this kit runs engine {engine} (format {engine_format})`
- **Next:** The stored doc was written by another engine than this build runs (a build on another engine, or one rolled back from it), so it was not loaded. Nothing changed. Open it with a build that runs that engine, or ask the host's operator to migrate it; don't retry.
- **Rule:** K-FMT-2, K-FMT-4
- **Exit:** 6
- **Reached by:** a doc stored by a build on another engine

## 6xx: erasure

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E601` | `purge_current_version` | 2 | K-T0-9, K-PURGE-1 | `purge` | live |
| `MPC-E602` | `purged_version` | 4 | K-PURGE-4 | `show --version`, `restore` | live |

### MPC-E601 `purge_current_version`

- **Message:** `version {version} of doc {doc} is its current content`
- **Next:** The current version of a file can't be purged. Write or restore another version first, then purge this one.
- **Next step:** `restore`. A file's current version can't be purged; write or restore another version first, then purge this one.
- **Rule:** K-T0-9, K-PURGE-1
- **Exit:** 2

### MPC-E602 `purged_version`

- **Message:** `version {version} of doc {doc} was purged`
- **Next:** An owner purged that version; its content is gone for good. Pick another version (log).
- **Next step:** `log`. That version was purged for good; pick another version.
- **Rule:** K-PURGE-4
- **Exit:** 4

## 7xx: tier 0 and blobs

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E701` | `leased` | 5 | K-T0-2 | `lease`, `write`, `restore` | live |
| `MPC-E702` | `no_lease` | 2 | K-T0-7 | `lease --renew` | live |
| `MPC-E703` | `blob_too_large` | 4 | K-T0-1 | `write`, `(host)` | live |
| `MPC-E704` | `blob_not_found` | 4 | K-T0-1 | `cat`, `show --version`, `restore` | live |
| `MPC-E705` | `blob_damaged` | 1 | K-T0-4 | `cat`, `show --version`, `restore` | live |
| `MPC-E706` | `blob_io` | 1 | K-T0-1 | `*` | live |
| `MPC-E707` | `blob_no_random` | 1 | K-T0-4 | `*` | live |
| `MPC-E708` | `no_blob_store` | 4 | K-T0-1 | `(host)` | live |
| `MPC-E709` | `no_blob_space` | 4 | K-T0-4, K-PURGE-13 | `*` | live |
| `MPC-E710` | `no_blob` | 4 | K-T0-1 | `*` | live |
| `MPC-E720` | `blob_ref_unknown` | 4 | structured live, slice 5 | `@pack`, `(host)` | live |

### MPC-E701 `leased`

- **Message:** `leased by {name} ({holder}) until {expires_ms}`
- **Next:** Someone else holds the file's lease (a lock while they change it). Nothing was written. Retry after it expires (details.expires_ms, host clock), or ask them to release it; only an owner can break it (lease --break).
- **Next step:** `status`. Someone else holds the file's lease; see who holds it and until when, then retry after it expires or ask them to release it.
- **Rule:** K-T0-2
- **Exit:** 5

### MPC-E702 `no_lease`

- **Message:** `no lease held`
- **Next:** You hold no lease to renew (it expired, or the daemon restarted). Take a new one with lease.
- **Next step:** `lease`. You hold no lease to renew; take a new one.
- **Rule:** K-T0-7
- **Exit:** 2

### MPC-E703 `blob_too_large`

- **Message:** `blob is over the {limit}-byte limit`
- **Next:** The file is over the blob store's size limit, so it isn't versioned. It opens the ordinary way; edit it with ordinary file tools.
- **Rule:** K-T0-1
- **Exit:** 4

### MPC-E704 `blob_not_found`

- **Message:** `no blob {blob} in this space`
- **Next:** The bytes of that version are gone (collected, or the space was purged). Pick another version (log).
- **Next step:** `log`. The bytes of that version are gone; pick another version.
- **Rule:** K-T0-1
- **Exit:** 4
- **Reached by:** a blob file collected under a live version

### MPC-E705 `blob_damaged`

- **Message:** `blob {blob} is damaged (its bytes don't match its id)`
- **Next:** The stored bytes of that version are damaged and are never served. Pick another version (log), and report it.
- **Next step:** `log`. The stored bytes of that version are damaged and never served; pick another version, and report it.
- **Rule:** K-T0-4
- **Exit:** 1
- **Reached by:** damaged blob bytes on disk

### MPC-E706 `blob_io`

- **Message:** `blob i/o: {reason}`
- **Next:** The blob store couldn't read or write. Nothing was recorded. Check the disk, retry.
- **Rule:** K-T0-1
- **Exit:** 1
- **Reached by:** a blob store I/O failure

### MPC-E707 `blob_no_random`

- **Message:** `no randomness source: {reason}`
- **Next:** No randomness source for a space key. Report it to the host's operator.
- **Rule:** K-T0-4
- **Exit:** 1
- **Reached by:** no /dev/urandom

### MPC-E708 `no_blob_store`

- **Message:** `no blob store configured`
- **Next:** This host has no blob store, so files without a live type can't be versioned here. Edit the file with ordinary file tools.
- **Rule:** K-T0-1
- **Exit:** 4

### MPC-E709 `no_blob_space`

- **Message:** `no blob key for this space (never used, or purged)`
- **Next:** This workspace's blob versions were purged (or never existed). Older versions are gone; the current file is unaffected.
- **Rule:** K-T0-4, K-PURGE-13
- **Exit:** 4
- **Reached by:** a purged blob space

### MPC-E710 `no_blob`

- **Message:** `no blob {blob}`
- **Next:** The blob store has no record of those bytes. Pick another version (log).
- **Next step:** `log`. The blob store has no record of those bytes; pick another version.
- **Rule:** K-T0-1
- **Exit:** 4
- **Reached by:** a blob index row removed under a version

### MPC-E720 `blob_ref_unknown`

- **Message:** `no blob {blob} in this doc's space`
- **Next:** The value names a blob this doc's space doesn't hold: it was never stored here, it was collected, or it belongs to another space. Nothing was applied. Store the bytes in this doc's space, then send the change again with the id that returned.
- **Rule:** structured live, slice 5
- **Exit:** 4

## 8xx: structured and CSV
It covers structured file types too, a board's own verbs included (MPC-E830 to E859; E860 to E869 are kept for the board's live verbs).

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E801` | `needs_base` | 2 | K-BR-12 | `cell set`, `@change` | live |
| `MPC-E802` | `at_limit` | 2 | K-CSV-7 (KC4) | `@change` | live |
| `MPC-E803` | `facet_conflict` | 1 | K-BR-12 (KC7) | `@change` | live |
| `MPC-E804` | `op_refused` | 2 | K-SCH-2 (C1) | `@change` | live |
| `MPC-E805` | `no_element` | 4 | CSV contract §3.3 (C1) | `@pack` | live |
| `MPC-E806` | `header_names_unrendered` | 2 | CSV contract §1.4 (C1) | `@pack` | live |
| `MPC-E807` | `header_only_row_blank` | 2 | CSV contract §1.4 (C1) | `@pack` | live |
| `MPC-E820` | `key_not_own` | 2 | structured live, slice 1 | `@change` | live |
| `MPC-E821` | `key_space_full` | 2 | structured live, slice 1 | `(host)` | live |
| `MPC-E830` | `conflict` | 1 | board file: object revs | `@file`, `update`, `delete`, `resolve`, `lock`, `check-set`, `check-remove`, `table-cell`, `table-remove-row`, `table-remove-column`, `move`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `apply`, `inverse` | live |
| `MPC-E831` | `exists` | 1 | board file: unique ids | `@file`, `put-back`, `apply`, `inverse` | live |
| `MPC-E832` | `busy` | 1 | board file: the save guard | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `apply`, `inverse` | live |
| `MPC-E833` | `merge_conflict` | 1 | board file: three-way merge | `@file`, `(host)` | live |
| `MPC-E834` | `board_conflict` | 1 | board file: groups apply whole | `@file`, `(host)` | live |
| `MPC-E835` | `locked` | 5 | board file: locked objects | `@file`, `update`, `delete`, `reply`, `resolve`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `connected-copy`, `lock`, `apply`, `inverse` | live |
| `MPC-E836` | `voting_off` | 5 | board file: voting | `@file`, `vote` | live |
| `MPC-E837` | `no_votes_left` | 5 | board file: voting | `@file`, `vote` | live |
| `MPC-E838` | `no_object` | 4 | board file: object ids | `@file`, `read`, `update`, `delete`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `connected-copy`, `mind-child`, `apply`, `inverse` | live |
| `MPC-E839` | `no_template` | 4 | board file: templates | `@file`, `template` | live |
| `MPC-E840` | `no_such_item` | 4 | board file: item ids | `@file`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column` | live |
| `MPC-E841` | `blob_missing` | 4 | board file: pictures | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `apply`, `inverse` | live |
| `MPC-E842` | `board_zip` | 4 | board file: the package | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `list`, `read`, `search`, `references`, `apply`, `inverse` | live |
| `MPC-E843` | `board_manifest` | 4 | board file: the manifest | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `list`, `read`, `search`, `references`, `apply`, `inverse` | live |
| `MPC-E844` | `newer_format` | 4 | board file: format versions | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `list`, `read`, `search`, `references`, `apply`, `inverse` | live |
| `MPC-E845` | `board_invalid` | 4 | board file: the board's data | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `list`, `read`, `search`, `references`, `apply`, `inverse` | live |
| `MPC-E846` | `board_too_large` | 4 | board file: limits | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `list`, `read`, `search`, `references`, `apply`, `inverse` | live |
| `MPC-E847` | `bad_field` | 2 | board file: fields | `@file`, `add`, `update`, `connect`, `comment`, `reply`, `check-add`, `check-set`, `check-remove`, `table-cell`, `mind-child`, `meta`, `react`, `paste`, `put-back`, `apply`, `inverse`, `fields` | live |
| `MPC-E848` | `board_limit` | 2 | board file: limits | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `apply`, `inverse` | live |
| `MPC-E849` | `bad_ops` | 2 | board file: raw ops | `@file`, `apply` | live |
| `MPC-E850` | `unknown_field` | 2 | board file: fields | `@file`, `add`, `update` | live |
| `MPC-E851` | `unknown_type` | 2 | board file: types | `@file`, `add`, `update` | live |
| `MPC-E852` | `bad_value` | 2 | board file: verb values | `@file`, `vote`, `comment` | live |
| `MPC-E853` | `wrong_type` | 2 | board file: types | `@file`, `reply`, `resolve`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `align`, `distribute`, `mind-child`, `connected-copy` | live |
| `MPC-E854` | `too_few` | 2 | board file: arranging | `@file`, `group`, `align`, `distribute`, `tidy`, `paste` | live |
| `MPC-E855` | `needs_actor` | 2 | board file: who acts | `@file`, `react`, `vote`, `reply` | live |
| `MPC-E856` | `no_free_id` | 1 | board file: unique ids | `@file`, `add`, `connect`, `comment`, `reply`, `check-add`, `table-add-row`, `table-add-column`, `group`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `apply` | live |
| `MPC-E857` | `board_io` | 1 | board file: reading and writing | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `list`, `read`, `search`, `references`, `apply`, `inverse` | live |
| `MPC-E858` | `settings_conflict` | 1 | board file: settings revs | `@file`, `meta`, `apply`, `inverse` | live |
| `MPC-E859` | `person_exists` | 1 | board file: people | `@file`, `add`, `update`, `delete`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `resolve`, `meta`, `lock`, `check-add`, `check-set`, `check-remove`, `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column`, `move`, `order`, `frame-order`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template`, `clear-blob`, `apply` | live |

### MPC-E801 `needs_base`

- **Message:** `an agent's grid edit needs a base: read first and pass --base`
- **Next:** Agents edit tables against what they read. Read the table first (it prints the version), then run the edit again with --base <version>.
- **Next step:** `cat`. Agents edit tables against what they read; read the table first, then run the edit again against the version it printed.
- **Rule:** K-BR-12
- **Exit:** 2

### MPC-E802 `at_limit`

- **Message:** `this table is at its size limit ({limit} {what}, deleted rows included); the owner can free space with `rows drop-deleted``
- **Next:** The table can't grow past its limit (details.what: cells, columns or rows); nothing was applied and nothing was dropped. Remove rows or columns first, or ask an owner to run rows drop-deleted to free the space deleted rows still hold.
- **Next step:** `rows drop-deleted`. The table is at its size limit; remove rows or columns first, or have an owner free the space deleted rows still hold.
- **Rule:** K-CSV-7 (KC4)
- **Exit:** 2

### MPC-E803 `facet_conflict`

- **Message:** `{count} element(s) changed since your base: {where}`
- **Next:** Someone changed the same cells or rows after the version you read. Nothing was applied. Read again, redo your change on the new values, and pass the new --base.
- **Next step:** `cat`. Someone changed the same cells or rows since your base; read again, redo your change on the new values, and pass the new base.
- **Rule:** K-BR-12 (KC7)
- **Exit:** 1

### MPC-E804 `op_refused`

- **Message:** `op {op} on root {root}: {problem}`
- **Next:** One of the edit's row, column or cell operations can't apply (an unknown or reused key, a move next to itself, dropping a live element); nothing of the change was applied. List the rows or columns again (row list, col list) and use the keys shown.
- **Next step:** `row list`. An operation names an unknown or reused key; list the rows or columns again and use the keys shown.
- **Rule:** K-SCH-2 (C1)
- **Exit:** 2

### MPC-E805 `no_element`

- **Message:** `{verb}: {line}`
- **Next:** The row, column or cell the verb names is unknown or deleted (stdout says which, and who deleted it when you may read history). A raw keyed cell write naming a row or column the table doesn't know (dropped, or never in it) is refused the same way, whole. Nothing was applied. List the live ones (row list, col list), or restore a deleted one first (row restore, col restore).
- **Next step:** `row list`. The row, column or cell is unknown or deleted; list the live ones, or restore a deleted one first.
- **Rule:** CSV contract §3.3 (C1)
- **Exit:** 4

### MPC-E806 `header_names_unrendered`

- **Message:** `the columns have names that would appear as a header line; clear them or add a row first`
- **Next:** The table has no row, and its columns still hold names from before the header was switched off: switching it on would write them as a header line, changing the file. Nothing was applied. Clear the names (col rename) or add a row first, then switch.
- **Next step:** `col rename`. The columns still hold names that would be written as a header line; clear them or add a row first, then switch.
- **Rule:** CSV contract §1.4 (C1)
- **Exit:** 2

### MPC-E807 `header_only_row_blank`

- **Message:** `header on: the only row is a blank line, and a blank header line with nothing after it isn't written (the file would change); add a non-blank row first`
- **Next:** The table's only row is blank: as the header it would be a blank header line with nothing after it, which isn't written, so the file would change. Nothing was applied. Add a row with values first (row insert), then switch the header on.
- **Next step:** `row insert`. A blank only row can't be written as the header; add a row with values first, then switch the header on.
- **Rule:** CSV contract §1.4 (C1)
- **Exit:** 2

### MPC-E820 `key_not_own`

- **Message:** `insert {key} on {root} doesn't carry your writer tag {tag}`
- **Next:** This list's keys carry their writer: an insert must name <prefix><n>.<tag> with one of the list's prefixes and your own writer tag (details.tag). Nothing was applied. Insert through a verb or Session::mint_key, which mints a key with your own writer tag; never reuse another writer's tag.
- **Rule:** structured live, slice 1
- **Exit:** 2

### MPC-E821 `key_space_full`

- **Message:** `no key fits 24 bytes for prefix {prefix} and tag {tag}`
- **Next:** Every key of this prefix and writer tag that fits a list key's 24 bytes is taken. Nothing was written. The pack's key prefix is too long for a tagged key: use a shorter prefix (the pack's fix).
- **Rule:** structured live, slice 1
- **Exit:** 2

### MPC-E830 `conflict`

- **Message:** `{target} changed since the rev you read`
- **Next:** Someone changed it after you read it (details.target says what; the refusal carries what is there now). Nothing was applied. Read it again, redo your change on what is there now, and pass the new rev.
- **Next step:** `read`. It changed since the rev you read; read it again, redo your change on what is there now, and pass the new rev.
- **Rule:** board file: object revs
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E831 `exists`

- **Message:** `{target} is already in use`
- **Next:** The id you gave is taken (details.target; the refusal carries what is there now). Nothing was applied. Read what is there, then give another id, or leave the id out so a free one is chosen.
- **Next step:** `read`. The id is taken; read what is there, then give another id or let a free one be chosen.
- **Rule:** board file: unique ids
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E832 `busy`

- **Message:** `the file changed on disk since it was read, and again on each of the host's retries`
- **Next:** Someone else saved the file between your read and your save. The host already retried the save, and the file changed again each time. Nothing was written. Read what you are changing again, then run your change again on what is there now.
- **Next step:** `read`. The board changed while saving; read it again, then retry your change on what is there now.
- **Rule:** board file: the save guard
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E833 `merge_conflict`

- **Message:** `{count} thing(s) changed on both sides`
- **Next:** Your copy and the file both changed the same things (settings, people, top-level fields or objects) since the copy was made (details lists each, with every side kept). Nothing was written. Read the board again and redo your changes to those things on what is there now.
- **Rule:** board file: three-way merge
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E834 `board_conflict`

- **Message:** `{count} group(s) held: what they change moved since they were made`
- **Next:** Some groups of edits didn't apply, because what they change moved since they were made (details lists each held group and why). Each held group changed nothing and is kept whole; the others applied. Read the board again and make the held changes again on what is there now.
- **Rule:** board file: groups apply whole
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E835 `locked`

- **Message:** `{id} is locked`
- **Next:** The object is locked, so no verb changes, moves or deletes it until it is unlocked. Nothing was applied. Read it to see its state, and ask whoever locked it; unlock it (lock) only if the lock is yours.
- **Next step:** `read`. The object is locked; read it to see its state, ask whoever locked it, and unlock it only if the lock is yours.
- **Rule:** board file: locked objects
- **Exit:** 5
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E836 `voting_off`

- **Message:** `voting is off on this board`
- **Next:** Votes count only while voting is on, and a person running the session usually starts it. Nothing was applied. Ask for voting to be started (meta turns it on), then vote again.
- **Next step:** `meta`. Voting is off; it is turned on in the board's settings, usually by the person running the session.
- **Rule:** board file: voting
- **Exit:** 5
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E837 `no_votes_left`

- **Message:** `no votes left`
- **Next:** You have used every vote this round allows. Nothing was applied. Take a vote back from another object (vote, with a negative delta) to move it, or wait for the next round.
- **Next step:** `vote`. Every vote this round allows is used; take one back from another object to move it, or wait for the next round.
- **Rule:** board file: voting
- **Exit:** 5
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E838 `no_object`

- **Message:** `no object {id}`
- **Next:** The board has no object with that id: it was deleted, or the id is mistyped. Nothing was applied. List the objects and use an id shown; a deleted one comes back with put-back, from what its delete returned.
- **Next step:** `list`. No object has that id; list the objects and use an id shown.
- **Rule:** board file: object ids
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E839 `no_template`

- **Message:** `no template {name}`
- **Next:** No template has that name. Nothing was applied. List the templates and use a name shown.
- **Next step:** `templates`. No template has that name; list the templates and use a name shown.
- **Rule:** board file: templates
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E840 `no_such_item`

- **Message:** `{id} has no item {item}`
- **Next:** The object has no check item, row or column with that id. Nothing was applied. Read the object to see its items and use an id shown.
- **Next step:** `read`. The object has no item with that id; read the object to see its items and use an id shown.
- **Rule:** board file: item ids
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E841 `blob_missing`

- **Message:** `picture bytes {hash} aren't in the file`
- **Next:** A picture on the board names bytes the file doesn't hold, so it can't be saved as is. Nothing was written. Clear the pictures whose bytes are missing (clear-blob), or add the bytes again, then run your change again.
- **Next step:** `clear-blob`. A picture's bytes are missing from the file; clear that picture, or add its bytes again, then run your change again.
- **Rule:** board file: pictures
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E842 `board_zip`

- **Message:** `the file isn't a package this build reads ({code})`
- **Next:** The file's container is damaged, or uses something the format doesn't allow (details.code says which). Nothing was read or changed. Restore the file from a good copy; no board verb opens it as it is.
- **Rule:** board file: the package
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E843 `board_manifest`

- **Message:** `the file's manifest or parts don't hold together ({code})`
- **Next:** The file's manifest is missing or damaged, or its parts don't match it (details.code says which). Nothing was read or changed. Restore the file from a good copy; no board verb opens it as it is.
- **Rule:** board file: the manifest
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E844 `newer_format`

- **Message:** `the file needs a newer reader (version {needs})`
- **Next:** A newer build wrote this file, and it asks for a reader newer than this one. Nothing was read or changed. Open it with a build that reads that version.
- **Rule:** board file: format versions
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E845 `board_invalid`

- **Message:** `the board in the file isn't valid ({code})`
- **Next:** The file opens, but the board it holds breaks the format (details.code says how, and where when it can). Nothing was read or changed. Restore the file from a good copy, or fix the board's data with the tool that wrote it.
- **Rule:** board file: the board's data
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E846 `board_too_large`

- **Message:** `the file is over a limit ({code})`
- **Next:** The file passes one of the limits this host reads boards within (details.code says which). Nothing was read or changed. Ask the host's operator about the limit, or split the board with the tool that wrote it.
- **Rule:** board file: limits
- **Exit:** 4
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E847 `bad_field`

- **Message:** `a value you gave isn't valid ({code})`
- **Next:** A field, id, type or number you gave doesn't fit the format (details.code says which). Nothing was applied. Check the fields that type takes, fix the value and run the verb again.
- **Next step:** `fields`. A value you gave doesn't fit the format; check the fields each type takes, fix it and run the verb again.
- **Rule:** board file: fields
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E848 `board_limit`

- **Message:** `the change would pass a limit ({code})`
- **Next:** Your input, or the board after your change, would pass one of the limits this host works within (details.code says which). Nothing was applied or written. Make a smaller change, or remove objects first.
- **Rule:** board file: limits
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E849 `bad_ops`

- **Message:** `the ops aren't valid ({code})`
- **Next:** The ops you passed aren't valid JSON, or not in the shape the format takes (details.code says which). Nothing was applied. Check them against the fields each type takes, fix them and apply again.
- **Next step:** `fields`. The ops don't fit the format; check them against the fields each type takes, fix them and apply again.
- **Rule:** board file: raw ops
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E850 `unknown_field`

- **Message:** `{type} has no field {field}`
- **Next:** That type of object has no field by that name. Nothing was applied. List the fields it takes and use one of those names.
- **Next step:** `fields`. That type has no field by that name; list the fields it takes and use one of them.
- **Rule:** board file: fields
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E851 `unknown_type`

- **Message:** `no object type {type}`
- **Next:** The format has no type of object by that name. Nothing was applied. List the types and use one shown.
- **Next step:** `fields`. The format has no type by that name; list the types and use one shown.
- **Rule:** board file: types
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E852 `bad_value`

- **Message:** `{verb} can't use that value: {reason}`
- **Next:** The verb can't use a value you gave (a vote of 0, a comment with no text). Nothing was applied. Run it again with a value it can use.
- **Rule:** board file: verb values
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E853 `wrong_type`

- **Message:** `{verb} doesn't apply to {id}, a {type}`
- **Next:** That verb works on other types of object (a reply on a comment, a check item on a card, a row on a table). Nothing was applied. Read the object to see its type, then pick the object or the verb that fits.
- **Next step:** `read`. The verb doesn't work on that type of object; read it to see its type, then pick the object or the verb that fits.
- **Rule:** board file: types
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E854 `too_few`

- **Message:** `{verb} needs at least {min} objects`
- **Next:** The verb needs more objects than you named (two to group or align, three to distribute). Nothing was applied. List the objects and name enough of them.
- **Next step:** `list`. The verb needs more objects than you named; list the objects and name enough of them.
- **Rule:** board file: arranging
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E855 `needs_actor`

- **Message:** `{verb} needs to know who is acting`
- **Next:** This verb records who acted, and the host didn't say. Nothing was applied. This is the host's to fix, not the caller's.
- **Rule:** board file: who acts
- **Exit:** 2
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E856 `no_free_id`

- **Message:** `no free id: the host's random ids keep landing on ids in use`
- **Next:** Every new id the host's randomness gave was already in use. Nothing was applied. A try with fresh random bytes can succeed, and the host makes it itself; if this keeps coming back, report it to the host's operator.
- **Rule:** board file: unique ids
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E857 `board_io`

- **Message:** `reading or writing the file failed: {reason}`
- **Next:** The file couldn't be read or written (the reason says why). Nothing was changed. Check the disk (space, permissions), then run the verb again.
- **Rule:** board file: reading and writing
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E858 `settings_conflict`

- **Message:** `the settings changed since the rev you read`
- **Next:** The settings changed since you read them (details.theirs holds what is there now). Nothing was applied. Read them again with meta, then redo your change on what is there now and pass the new rev.
- **Next step:** `meta`. The settings changed since the rev you read; read them again, redo your change on what is there now, and pass the new rev.
- **Rule:** board file: settings revs
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

### MPC-E859 `person_exists`

- **Message:** `token {id} already names another person`
- **Next:** The token you gave already names another person in this file (details.id is the token, details.theirs the person it names). Nothing was applied. Use another token, or leave the person op out and let the host mint one.
- **Rule:** board file: people
- **Exit:** 1
- **Reached by:** a board file verb, through its host's bridge (the board's refusal, by id)

## 9xx: limits

| Id | Name | Exit | Rule | Verbs | Status |
|---|---|---|---|---|---|
| `MPC-E901` | `rate_limited` | 2 | K-SEC-9, K-PERM-14 | `@change` | live |
| `MPC-E902` | `tool_limit` | 2 | K-PAR-7, K-SEC-4 | `@run_tool` | live |
| `MPC-E903` | `claims_doc_cap` | 2 | A13 (claims_max) | `claim` | live |
| `MPC-E904` | `claims_participant_cap` | 2 | A13 (claims_per_participant) | `claim` | live |
| `MPC-E905` | `claim_waiters_cap` | 2 | A13 (claim_waiters_max) | `(host)` | live |

### MPC-E901 `rate_limited`

- **Message:** `too many edits; retry in {retry_after_ms} ms`
- **Next:** You are over the edit rate for this doc (capabilities: edit_rate_per_s, edit_burst). Nothing was applied. Wait details.retry_after_ms, then retry; better, combine your changes into fewer calls (one patch, or one write --base).
- **Rule:** K-SEC-9, K-PERM-14
- **Exit:** 2

### MPC-E902 `tool_limit`

- **Message:** `limit: {reason}`
- **Next:** The call would go over a bound (output size, pattern size or work). Nothing was applied. Narrow it: fewer lines (head, view --lines), a simpler pattern, smaller edits.
- **Next step:** `head`. The call would pass a bound; narrow it: fewer lines, a simpler pattern, smaller edits.
- **Rule:** K-PAR-7, K-SEC-4
- **Exit:** 2

### MPC-E903 `claims_doc_cap`

- **Message:** `bad claim: this doc has {count} live claims (the most)`
- **Next:** This doc has as many live claims as it allows. Wait for some to end (activity and who show them), or edit without a claim.
- **Next step:** `activity`. This doc has all the live claims it allows; wait for some to end, or edit without a claim.
- **Rule:** A13 (claims_max)
- **Exit:** 2

### MPC-E904 `claims_participant_cap`

- **Message:** `bad claim: you hold {count} claims here (the most); release one`
- **Next:** You hold as many claims on this doc as you may. Release one you no longer need (claim --release --id ID), then claim again.
- **Next step:** `claim --release`. You hold all the claims you may on this doc; release one you no longer need, then claim again.
- **Rule:** A13 (claims_per_participant)
- **Exit:** 2

### MPC-E905 `claim_waiters_cap`

- **Message:** `bad claim: {count} already waiting for {claim}`
- **Next:** That claim already has as many waiters as it allows. Retry after it ends, or work elsewhere.
- **Next step:** `activity`. That claim already has as many waiters as it allows; see where others are working and work elsewhere, or retry after it ends.
- **Rule:** A13 (claim_waiters_max)
- **Exit:** 2

## Engine ids (MPCE-Ennn)

Reserved for the engine, which allocates its ranges (decode, admission and security, schema, history, store) at G2-core. Its codes cross the seam as these strings, never as enum ordinals; a code this build doesn't list is reported as `MPC-E161` with `details.code`.

- `MPCE-E199` `ref_not_in_past`
- `MPCE-E980` `change_too_large`
- `MPCE-E566` `store_secret_missing`
- `MPCE-E567` `checkpoint_mac`
- `MPCE-E568` `cut_purge_pending`
