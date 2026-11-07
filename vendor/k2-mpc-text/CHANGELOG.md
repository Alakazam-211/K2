# Changelog

## v0.2.16 (2026-11-07): pairs with kit v0.12.0

**Requires `k2-mpc-base` v0.12.0** (the tag at its public cut 178b449e; release commit
33b41c81). This pack's code and tests are unchanged from v0.2.15.

- Pairs with kit v0.12.0: the log entry's body (`LogEntry.body: EntryBody`), the grid row
  limit (`Config::grid_max_rows`, `MPC-E802` `what` `rows`) and the grid paths made linear.
  This pack's code builds and reads no `LogEntry` and holds no grid, so nothing here changes.
- `Cargo.lock`: k2-mpc-base 0.12.0 (only the kit's crate version moves; no `mpce` entry); the
  crate version.

## v0.2.15 (2026-11-04): pairs with kit v0.11.1

**Requires `k2-mpc-base` v0.11.1** (the tag at its public cut d9927a6e; release commit
ae2ac947). This pack's code and tests are unchanged from v0.2.14.

- `Cargo.lock`: k2-mpc-base 0.11.1 (only the kit's crate version moves; no `mpce` entry); the
  crate version.

## v0.2.14 (2026-11-04): pairs with kit v0.11.0

**Requires `k2-mpc-base` v0.11.0** (the tag at its public cut 386ccd77; release commit
49e94018). This pack's code and tests are unchanged from v0.2.13.

- `Cargo.lock`: k2-mpc-base 0.11.0 (only the kit's crate version moves; no `mpce` entry); the
  crate version. The kit's new pack-facing items (position units for pack verbs, `VerbSpec` as
  `#[non_exhaustive]`) need nothing here: this pack has no verbs of its own and builds no
  `VerbSpec`.

## v0.2.13 (2026-11-03): pairs with kit v0.10.2

**Requires `k2-mpc-base` v0.10.2** (unchanged from v0.2.12).

- The refused-file test case `nul.binary.json` is now `nul-byte.binary.json`: `nul` is a reserved name on
  Windows, and a tree holding that name can't be checked out there.
- `.k5board` is on the not-text list: the file is a zip, so `TextPack::for_path` returns `None` for it and a
  host opens it some other way.
- `scripts/check-windows-names.sh` and its self-test, in `scripts/check.sh`: every path must be one Windows can
  check out (no reserved name or refused character, no trailing dot or space, no case collision, and up
  to 200 bytes).

## v0.2.12 (2026-11-02): pairs with kit v0.10.2

**Requires `k2-mpc-base` v0.10.2** (the first host release after v0.10.0.3; it keeps the
disk-sync fix). This pack's code and tests are unchanged from v0.2.11.

- Pairs with kit v0.10.2: the client as a library only, presence and marks as data,
  per-connection carets and the caret re-send. The kit's `join`, `join_shared` and
  `join_replacing` are unchanged, and this pack's code doesn't build or match the roster types
  that changed.
- `Cargo.lock`: k2-mpc-base 0.10.2 (only the kit's crate version moves); the crate version.

## v0.2.11 (2026-11-01): pairs with kit v0.10.0.3

**Requires `k2-mpc-base` v0.10.0.3** (v0.10.0.2 plus the disk-sync fix). This pack's code and
tests are unchanged from v0.2.10.

- Pairs with kit v0.10.0.3: the disk-sync fix.
- `Cargo.lock`: k2-mpc-base 0.10.0+3 (only the kit's crate version moves); the crate version.

## v0.2.10 (2026-10-31): pairs with kit v0.10.0.2

**Requires `k2-mpc-base` v0.10.0.2** (v0.10.0.1 plus the licence; no kit code change). This
pack's code and tests are unchanged from v0.2.9.

- **Licence: MIT OR Apache-2.0**, as the kit and the engine: `LICENSE-MIT` and `LICENSE-APACHE`
  at the root, `license` in the crate (by `license.workspace`). README and NOTICE.md say so. The
  test corpus's `Cargo.toml` and `package.json` are test data and are unchanged. No code
  change.
- `Cargo.lock`: k2-mpc-base 0.10.0+2 (only the kit's crate version moves); the crate version.

## v0.2.9 (2026-10-30): pairs with kit v0.10.0

**Requires `k2-mpc-base` v0.10.0** (C1: structured kinds and the grid, for CSV, on top of
kit v0.9.7.1, whose whole-line `write --base` this pack took in v0.2.8: that entry stands).
This pack gets no grid and no new verb; what changed for it:
- **The grid's arrival doesn't affect it.** The kit's grid tier (structured roots, the grid
  verbs, presence by key, element blame) is used only by packs that declare structured roots;
  a text doc stays a text root, and its tests and goldens are unchanged (checked on a build
  node).
- **Breaking items absorbed:**
  - `PersonAction` gained the grid tier's rows P35-P44 (`RowInsert` … `DropDeleted`):
    `parity_row` maps them **N/A** (a text doc has no rows or columns), listing the grid verbs
    as absent (`row insert` … `rows drop-deleted`); `docs/PARITY.md` has the row.
  - `At` is no longer `Copy` (it gained `At::Elem`, a grid cell by key): nothing here
    copied one, so no code change.
- **Store format 6 is lazy:** a store is stamped 6 (with the `elem_writes` table) only when
  it writes its first structured doc. A store holding only this pack's docs stays at format 5
  byte for byte, so a kit v0.9.7.1 build still reads and writes it.
- **Error ids:** `MPC-E806 header_names_unrendered` and `MPC-E807 header_only_row_blank`
  come from the grid's `header on` verb, which this pack doesn't list: not visible here.
  P24 (`export`, `export --version`) stays live as in v0.2.7.
- `Cargo.lock`: k2-mpc-base 0.10.0; the crate version.

## v0.2.8 (2026-10-30): pairs with kit v0.9.7.1

**Requires `k2-mpc-base` v0.9.7.1** (crate version `0.9.7+1`). The kit's `write --base` now
merges by whole lines (it could splice two lines that start alike into one; possible text
corruption, kit v0.1.0 through v0.9.7). What changes for CSV-as-text users:
- **Two agents editing different cells of the same row: the second is refused and retries.**
  A row is one line, and agent writes that touch a line someone else changed since the
  agent's version are refused as a conflict; the agent re-reads and writes again. Different
  rows, adjacent ones too, and appended rows still merge. People typing live are unchanged.
- Tests: `concurrent_row_and_cell_edits_merge_in_every_style` keeps adjacent rows and two
  appended rows and now has cells in different rows; new
  `two_cells_of_one_row_conflict_and_the_retry_lands` (the second write conflicts on row 3,
  the retry from the current version lands, in LF, CRLF and CR). README and
  docs/HOST-INTEGRATION.md say so.
- `Cargo.lock`: k2-mpc-base 0.9.7+1; the crate version 0.2.8.

## v0.2.7 (2026-10-29): pairs with kit v0.9.7

**Requires `k2-mpc-base` v0.9.7** (it skips v0.9.6, a one-fix release). Pack changes:
- **Parity row P24 "Export" is live:** `ParityRow::live(a, &["export", "export --version"])`.
  The kit now exports: `export` (needs read) and `export --version V` (needs history) give the
  file through the kit's export registry (as-is in the kit; format exporters are the host's).
  The old N/A row listed `export` as a verb that must not exist, which the kit's registry now
  has, so the parity check would fail against it. `docs/PARITY.md`'s P24 row (the parity test
  checks the table names each verb) and the parity test's counts (28 live rows, no N/A) follow.
- `Cargo.lock`: k2-mpc-base 0.9.7; the crate version. No other pack code change: the kit's
  v0.9.7 changes (stable error ids, K5 handles in `run_tool` output, D1's undo, outside saves
  merged line by line) leave this pack's tests and goldens as they were (`scripts/check.sh`
  on a build node, every step green, nothing re-blessed). No store format change.

The kit's changes since v0.9.5 (its CHANGELOG has the details):
- **v0.9.6:** an outside save that deleted a line and typed it back could leave the line
  twice; the outside-save merge now diffs each side on whole lines.
- **v0.9.7, the Live-tab release:** the live typing APIs (the hunk channel, `edit_at_base`,
  `events_since`, undo over the channel, handles instead of pids in `run_tool` output for App
  viewers, roster push, participant overflow); **D1**: an undo never changes a word someone
  else wrote, and says so instead; speed work (incremental write-back and blame, the
  engine's position index); outside saves merged line by line; exports P0 (`Session::file_at`,
  the export contract, the P24 verbs); stable error ids (`MPC-Ennn`, `docs/ERRORS.md`); host
  guidance per surface, relative paths in refusals and hard-linked files declined (K6, K7,
  K9); the vendored engine at k2-yrs k2.6.3.

## v0.2.6 (2026-10-29)

**Requires `k2-mpc-base` v0.9.5** (a hotfix). No pack code change: `Cargo.lock` and the crate
version only. Nothing to do for this pack; no store format change. The kit's changes:
- **Outside saves:** v0.9.4 narrowed the window in which an outside edit could be corrupted
  after a change that moved no byte; v0.9.5 closes it.
  - A plain-editor save that arrived within the write delay after such a change and an edit
    not yet written back could still land beside the restored text (`LINE three3`).
  - An outside save is now a three-way merge onto the live doc: `write --base`'s merge when
    clean.
  - Where the save overlaps an unwritten change, both are kept, as before, now as whole
    lines and in a deterministic order (the live side first).
- **Echo detection:** a person's save that recreated the bytes of an earlier kit write is no
  longer discarded once they have saved in between. Suppressed echoes are reported
  (`AuditEvent::EchoSuppressed`).
- **Hardening:** client updates carrying subdocuments are refused (the kit never uses them),
  with `DocError::Unaccepted`. A stored doc holding one is quarantined
  (`DocError::Quarantined`). The vendored engine is updated to k2-yrs k2.2.2.
- K-AUD-1 gains `SyncDrift::Pending`.

## v0.2.5 (2026-10-29)

**Requires `k2-mpc-base` v0.9.4** (a hotfix). No pack code change: `Cargo.lock` and the crate
version only. The kit fixes an outside edit that could be corrupted after a change that moved
no byte (since v0.1.0): an edit and its undo, a delete and a retype, or a rewrite and back,
then a plain-editor edit of the same text, could land beside the restored copy (`LINE
three3`) while the doc reported clean. The kit now keeps its disk replica on the doc's own
elements, and adds `Session::audit_disk_sync()` (K-AUD-1's sync check). Nothing to do for
this pack; no store format change.

## v0.2.4 (2026-10-28)

**Requires `k2-mpc-base` v0.9.3.** No pack code change: `Cargo.lock` and the crate version
only. The kit now makes every edit on the doc itself, as the participant's client (no
throwaway replica per edit: byte-identical results, far cheaper as history grows), with a
sampled runtime check against the old path (`Config::edit_check_every`, two new
`AuditEvent` variants). It builds its engine from `vendor/yrs` (Yrs v0.28.0 plus the fork's
patches, MIT) by path, so the lock's `yrs` entry loses its registry source; nothing else
changes for this pack. No store format change.

## v0.2.3 (2026-10-28)

**Requires `k2-mpc-base` v0.9.2** (additive: tier 0 → text promotion keeping history,
`approvals --all`, claims that never overlap at a shared edge, the engine's text cache; no
store format change). No pack code change: `Cargo.lock` and the crate version only.

## v0.2.2 (2026-10-28)

Docs only: `HOST-INTEGRATION.md` and `PARITY.md` no longer tell hosts to strip a leading BOM from `write` bodies; base v0.9.1 does it (`Pack::writes_bom`). Requires `k2-mpc-base` v0.9.1. `scripts/check.sh` checks formatting of this crate only (`cargo fmt -p k2-mpc-text`), so the kit's own style and its vendored model don't fail the pack's check.

## v0.2.1 (2026-10-28)

**Requires `k2-mpc-base` v0.9.1** (its double-BOM fix covers this pack with no code change). `Cargo.lock` and the crate version only.

## v0.2.0 (2026-10-28)

Planned as v0.2.0. Still requires `k2-mpc-base` v0.9.0.

- **CSV and TSV as text (`csv-as-text`):** an early multiplayer path for tables, until a
  grid pack exists.
  - A new group `csv` (family `Family::CsvAsText`, name `csv-as-text`) for `.csv` and
    `.tsv`: 4 MiB, LF for new line breaks, the same byte-exact codec.
  - `.psv`, `.tab` and `.dsv` stay unknown (plain text under the strict guard), so a grid
    pack takes over exactly `.csv` and `.tsv`.
  - **Breaking:** `Family` has a new variant. A host that matches on it exhaustively adds an
    arm.
- **Fidelity:** a CSV/TSV corpus of 7 files and 1 refusal.
  - The accepted files: RFC 4180 with CRLF, quoted fields with line breaks inside, an
    Excel "CSV UTF-8" export (BOM, CRLF), 240 columns, the semicolon dialect with decimal
    commas, a TSV, and a classic-Mac (CR) Excel file.
  - The refusal is a CRLF file with a bare LF inside a quoted cell, as Excel on Windows
    writes it (`mixed_line_endings`).
  - The whole corpus is now 53 files (46 accepted across six families, 7 refused).
  - A second random-edit property test runs on the CSV files, with inserts made of
    delimiters, quotes and cells.
  - `PROPTEST_CASES` sets the case count of both random-edit tests (512 by default).
- **Session tests for tables:**
  - Two agents read one version and edit different rows with `write --base`, at the same
    moment from two threads. Both edits land, and the BOM and CRLF are kept.
  - Adjacent rows, two cells of one row, and two appended rows merge in LF, CRLF and CR
    files.
  - A row moved by one agent and edited by another from the same base conflicts, then
    lands on a re-read. A column swap against a cell edit conflicts too. These show that
    text mode has no row or column identity.
- **Docs:**
  - README: the table row, and what text-mode CSV does and doesn't do (no row or column
    identity, no shape checks).
  - `docs/HOST-INTEGRATION.md`: the routing order (`.csv`/`.tsv` go to this pack until a
    grid pack ships, then to the grid pack first), the family, and the checklist.

## v0.1.0 (2026-10-28)

First release of the plain-text pack for Multi-Player Cores (MPC). **Requires `k2-mpc-base`
v0.9.0**, checked out beside this repo (`../k2-mpc-base`).

- **`TextPack` / `PACK`:** one pack for code, configuration, markup and diagram sources.
  - One schema (`text`) with one text root (`source`), shared by every type, so a rename
    between text types keeps the doc.
  - `TextPack::for_path(path)` picks a file's type. `PACK` is the pack for unknown types.
- **File-type table (`types`):** data, not code.
  - 218 extensions in 7 groups and 5 families (`text`, `code`, `config`, `markup`,
    `diagram`), plus well-known file names, name prefixes and an 81-extension not-text list.
  - Settings that vary by type: the size cap (4 MiB; logs 1 MiB), the line-ending style for
    new line breaks in a file with none (CRLF for `.bat`/`.cmd`), and the control-byte
    guard (strict for unknown types).
- **Codec:** byte-exact.
  - The BOM, the line endings (LF, CRLF and, new over the kit's text codec, bare CR) and the
    final newline are restored. Nothing is normalised.
  - Refusals are typed, with the kit's codes: `too_large`; `not_utf8`, which names a
    UTF-16/32 BOM or gives the first bad offset; `binary` (NUL, or a control byte in an
    unknown type); and `mixed_line_endings`, with the counts.
  - Encoding a doc that holds a CR is a loud `PackError`, never a mixed file.
- **Fidelity:** a corpus of 39 accepted files across the five families and 6 refused files.
  Each accepted file is checked:
  - byte-exact through doc and store, and through live sessions on disk (`cat`, a no-op
    `replace`/`sed -i`/`write --base`, a one-word edit, write-back);
  - with a 512-case random-edit property test, checked against the original bytes.
- **Parity:** `docs/PARITY.md` lists P1–P31 as data (27 live, Export N/A, 3 deferred).
  `tests/agent_parity.rs` runs the kit harness on five of the pack's types, with negative
  controls. The pack adds no verbs.
- **`docs/HOST-INTEGRATION.md`:** what a host needs to open text files with this pack.
- **`scripts/check.sh`:** fmt, clippy (`-D warnings`), tests, the licence gate, and the
  kit's engine-free check.
