# Changelog

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
