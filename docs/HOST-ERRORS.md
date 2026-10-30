# Host errors: how a host's CLI surfaces the kit's error ids

For the host team (the reference is K2's `k2 doc`). The kit gives every refusal a stable id
(`MPC-Ennn`; the engine's will be `MPCE-Ennn`), a next step and the rule it enforces, all in
one registry (`docs/ERRORS.md`, generated). This note says what a host does with them. The kit
stays host-agnostic: everything below about `k2 doc`, permission cards and exit lines is the
host's, and the kit doesn't know it.

## What the kit gives a host

| Call | When | Gives |
|---|---|---|
| `ToolRunError::report()` | `run_tool` / `run_tool_with` returned `Err` | the refusal |
| `ToolOutput::refusal` | `run_tool` returned `Ok` with an edit verb's "no" (a patch that didn't apply, exact `replace` with no or several matches, a pack verb that applied nothing, `patch --partial` rejects) | `Some(report)`; `stderr_summary` already ends with ` (MPC-Ennn)` |
| `SessionError::report()` | any other session call failed (`write_base`, `undo_with`, `revert`, `set_approval`, `claim`, `open_file`, blob calls...) | the refusal |
| `WriteOutcome::refusal(base)` | `write --base V` returned a conflict (exit 1) or an expired base (exit 8) | `Some(report)`; `None` when applied |
| `UndoOutcome::refusal()` | `undo`, `redo`, `revert` conflicted, were blocked, skipped regions (`--partial`), or found the history lost or purged | `Some(report)`; `None` when applied cleanly, dropped, or nothing to do |
| `Declined::report()` | a disk check (`DiskCheck::Declined`) or an open declined the file | the refusal (`MPC-E401`–`E407`, `details.code`) |
| `ErrorReport::engine(code, message)` | the engine returned a refusal code across the seam | the engine's entry, or `MPC-E161` with `details.code` |
| `errors::run_meta(argv, json, registries, config)` | `explain` / `capabilities` | stdout, exit, or a refusal |
| `errors::lookup(id)`, `errors::entries()` | a host's own catalog | the registry rows |

**Prefer the inherent calls**: `SessionError::report()`, `ToolError::report()` and
`ToolRunError::report()` need no import. The trait (`errors::Refusal`) is for the other types
(`StoreError`, `BlobError`, `DocError`, `Declined`, `PackError`) and for generic code.

An `ErrorReport` has `id()`, `exit()`, `message()` (the error's own wording, unchanged),
`next()`, `rule()`, `details()`; `plain()` and `to_json()` are the two printed forms. **An id
never changes an exit code**: `report.exit()` is always the code the host already prints
(`tools::session_exit`, `ToolError::exit`, the outcomes' `exit()`); a test holds every path.

## Plain text (the default)

Print the message as it always read, with the id appended, then the host's own lines as they
are today:

```text
$ k2 doc notes.md insert --line 3 < note.txt
k2 doc: refused: needs Edit rights (MPC-E102)
Ask with: k2 permission request …            ← K2's line, unchanged (exit 3)
```

- One line, `k2 doc: <report.plain()>`, on stderr. Nothing else changes: the stdout of a
  verb that printed a report (a failed patch's hunks, exact `replace`'s near misses) stays as
  it is, and its `stderr_summary` already ends with the id.
- **K2's permission lines are K2's** (`Ask with: k2 permission request …` / `It can't be
  requested.`): printed exactly as today, after the id line, never replaced by it. The id
  sits alongside.
- Notes (`note: …`) and the read fence stay on stderr as today.
- Don't print `next` in plain text by default (agents run `explain`); a host may add it
  behind a verbosity flag.

## `--json`

`--json` is **the host's flag**, never passed to `run_tool` (the kit would refuse it as an
unsupported flag, `MPC-E120`). Accept it before the path (`k2 doc --json notes.md undo`);
taking it from elsewhere in argv is risky (`grep -- --json`, `replace --new --json`), so if
K2 also accepts a trailing `--json`, take it only as the very last argument and only when no
`--` precedes it.

With `--json`, a refusal prints **exactly one line on stdout**, the object, and nothing else
on stdout; the exit code is unchanged:

```json
{"id":"MPC-E301","exit":5,"message":"lines 3–6 claimed by Bot (bot) \"rewriting the Sync section\"; 44131 ms left","next":"Someone else is working on those lines right now …","rule":"A13 (claims), K-PRES-12","details":{"claim":"claim-1","by":"bot","name":"Bot","lines":[3,6],"remaining_ms":44131,"note":"rewriting the Sync section"}}
```

- The six keys and their order are stable. `details` is structured (numbers, values, lists,
  objects), never prose; `explain <id>` lists what each id carries (its message template
  names them).
- **K2's own information** (the permission card line, whether it can be requested, a hold
  id) goes in **one extra key, `"host"`**, which the kit never writes: e.g. `"host":
  {"ask_with": "k2 permission request …"}` or `"host": {"can_request": false}`. Never change
  or add to the kit's six keys. The plain `Ask with:` line may still go to stderr.
- Success output with `--json` is K2's existing shape (K-BR-1: the version read); this slice
  doesn't change it.

## `k2 doc explain [ID] [--json]`

Needs no path, no doc, no daemon session: call `errors::run_meta(["explain", id], json, …)`.
`explain` with no id lists every id by area. Accepts `MPC-E301`, `mpc-e301`, `E301`, `301`.
An unknown id exits 4 with `MPC-E160` (printed as any refusal). Print `stdout` as is.

## `k2 doc capabilities [--json]`

`errors::run_meta(["capabilities"], json, registries, config)`, where:

- `registries`: what the daemon serves: `Registry::for_pack(&pack)` for each text pack it
  routes (Markdown, text, CSV...: a pack's own verbs then appear, with their ids) and
  `Registry::for_tier(Tier::Blob)` for tier 0. An empty list means the kit's verbs of both
  tiers.
- `config`: the `Config` the daemon's `SessionManager` runs with, so the limits printed are
  the ones in effect.
- Optional: `k2 doc capabilities <path>` with only that path's pack and tier.

It prints the kit version, the exit codes, every verb (kind, tiers, needs, usage, exits, the
person actions it serves, the ids it can return; `errors_any_verb` lists the ones every verb
can return), the parity rows, the limits and the id index. K2 adds its own verbs and flags
(`--fence`, `--presence`, `--respect-claims`, `--json`, `watch`'s streaming) in its own
section of `k2 doc --help`; the kit's part is generated, so K2 shouldn't copy it by hand.

## The catalog rows

- **MPC ids are the kit's.** K2's catalog (`k2 study`, the CLI docs) lists them by reading
  `errors::entries()` at build time (id, name, exit, area, status, `next`), so it can't drift.
  Retired ids stay listed; reserved ones (`MPC-E801`–`E803`, the CSV grid refusals; `E161`)
  are marked reserved.
- **K2's own refusals are K2's**: a path outside the workspace, a vault-managed path
  (K-SEC-12), a held permission card, a link lost mid-verb (exit 7), no daemon. They are not
  `MPC-` ids. If K2 wants ids for them, a K2 prefix in K2's own catalog keeps the two apart.
- **Engine ids** (`MPCE-Ennn`, allocated by the engine at G2-core) pass through as strings:
  `ErrorReport::engine(code, message)`; a code this kit build doesn't list prints as
  `MPC-E161` with `details.code` set.

## Guidance per surface, paths and hard links (v0.9.8: K6, K7, K9)

- **`next` per surface (K6).** The registry's `next` is written for an agent with only the
  CLI. A surface that shows refusals to people (a tab, an App) sets its own:
  `report.with_next("app", "Ask the doc's owner to let you edit.")`, then prints
  `report.next_for("app")` or `report.to_json_for("app")`. Only `next` changes: the id, exit
  code, message, rule and details are the same on every surface, and `next()` /
  `to_json()` stay the registry's. Key the override by the id (`report.id()`), never by the
  message.
- **No absolute paths (K7).** Every refusal the kit builds from an OS error (a file
  session's disk errors, `open_file` and `open_blob_file`) shows paths relative to the
  workspace root the session was opened with, or `<outside>`. For anything else, pass the
  workspace root: `report.redacted(root)` (or `errors::redact(text, root)` for the host's
  own strings, or `SessionError::redacted(root)`). The id and exit code don't change. The
  audit events (`AuditEvent::DiskError`) keep the real paths for the operator: never
  forward them to a guest.
- **Hard links (K9).** `open_file` and `open_blob_file` decline a file with more than one
  name: `Declined` `hard_linked`, `MPC-E412` (exit 4). Key sessions by the file's identity
  as well as its path (K-DISK-14): `session::file_identity(path)` (device and inode, before
  opening) and `Session::file_identity()` / `BlobSession::file_identity()` (the open
  session's file), so one file reached by two paths never gets two live docs.

## Routing summary for `k2 doc <path> <verb>`

| Verb | Kit call | Refusal |
|---|---|---|
| `RUN_TOOL_VERBS` and pack verbs | `run_tool_with` | `Err(e)` → `e.report()`; `Ok(out)` with `out.refusal` → that |
| `write --base V` | `write_base_with` | `outcome.refusal(V)`, or `Err(e).report()` |
| `undo` / `redo` (all forms), `revert` | `undo_with` / `redo_with` / `revert` | `outcome.refusal()`; a partial undo that applied some prints its applied summary, then the refusal line |
| everything else | its session call | `Err(e)` → `e.report()` |
| `explain`, `capabilities` | `errors::run_meta` | `out.refusal` |

## Tests on the host side

- K-PAR-4's table run through `k2 doc`: every refusal ends with ` (MPC-Ennn)` in plain text,
  and with `--json` stdout is one object with the six keys whose `exit` equals the process's
  exit code.
- Exit 3 still ends with K2's permission line (or `It can't be requested.`).
- `k2 doc capabilities --json` parses, lists every verb K2 routes, and its limits equal the
  daemon's `Config`.
- The blind-agent check, K2's way: a fresh agent with only `k2 doc --help`, `k2 doc explain`
  and `k2 doc capabilities` recovers from a held edit, an undo conflict and a declined file
  (the kit's own run: `docs/evidence/error-surface-blind.md`).

The kit's reference host, `crates/base/examples/mpc.rs`, does all of this in a few hundred
lines.
