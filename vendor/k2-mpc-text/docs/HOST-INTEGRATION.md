# Host integration

This page is for a host: an application that holds files and wants them live through this
pack. It assumes the host already runs the kit, as `k2-mpc-base`'s `docs/SESSIONS.md`
describes. That means a `SessionManager` on a `DocStore`, a `Host` that answers rights and
the clock, and a file watcher that calls `file_changed()`. This page covers only what is
specific to plain-text files.

## API

| Item | What |
|---|---|
| `k2_mpc_text::lookup(path) -> Lookup` | `Known { group, by }`, `Unknown`, or `NotText`. Only the file name counts. |
| `TextPack::for_path(path) -> Option<TextPack>` | The pack for that file's type. `None` means the file is not text. |
| `TextPack::family()` / `file_type()` / `settings()` | The type's family and codec settings. |
| `PACK` | The pack for an unknown type (the strict guard). |
| `SCHEMA` (name `text`), `ROOT` (`source`) | The doc model. Editors bind to the `source` root. |
| `types::TABLE`, `types::NOT_TEXT`, `types::UNKNOWN` | The table, as data. |
| `codec::decode(bytes, &settings)` | Optional. It gives the file's `Format` (BOM, LF/CRLF/CR) for a status bar. |

## 1. Open by file name

1. **Ask the more specific packs first.** For example, `.md` and `.markdown` go to the
   Markdown pack, and later `.ipynb` goes to its own. This pack is the fallback for text.
   It does not list the files other packs serve. Given one anyway, it opens it as plain
   text. `.csv` and `.tsv` are the exception for now: see [CSV and TSV](#csv-and-tsv-until-a-grid-pack).
2. Call `TextPack::for_path(path)`:
   - `None` (`Lookup::NotText`: images, media, office files, archives, binaries, fonts,
     databases): not for this pack. Open the file the host's non-live way (for example,
     tier 0).
   - `Some(pack)`: `SessionManager::open_file(Arc::new(pack), &key, path, root)`.
3. **If open fails with `SessionError::Declined { code, message }`,** the file stays as it
   is. Nothing was stored or written. Open it the non-live way and show the message (P25).
   See the refusal codes below.

An unknown name (`Lookup::Unknown`) is opened like the others, with `PACK`. It becomes live
only if it passes the strict guard. A host may show "opened as plain text" for it.

## 2. Family names

`Family::name()` gives one of these stable strings. A host can use them to choose an editor
mode or a preview:

| Family | Files | A host might |
|---|---|---|
| `text` | `.txt`, `.log`, `README`, `LICENSE`, subtitles, unknown names | Show a plain editor |
| `code` | Languages, shells, build files, stylesheets, templates, `Makefile`, `Dockerfile` | Show a code editor; highlight by extension (the host's own map) |
| `config` | JSON (with comments), YAML, TOML, INI, `.env`, dotfiles, lockfiles | Show a code editor; check syntax |
| `markup` | HTML, XML, SVG source, reStructuredText, AsciiDoc, TeX | Show source plus a sandboxed preview (HTML, SVG) |
| `diagram` | Mermaid, PlantUML, Graphviz, draw.io XML | Show source plus a rendered preview (keep the last good render while the source is invalid) |
| `csv-as-text` | `.csv`, `.tsv` | Show a text editor (a read-only grid preview is fine). Say it is text mode: see below |

The family is a hint for the host. The codec treats every family the same way.

### CSV and TSV until a grid pack

`.csv` and `.tsv` are in this pack's table (group `csv`, family `csv-as-text`) as an early
multiplayer path for tables. A grid pack, with rows and columns that have identity, will
serve them later. The routing order is then:

1. **The grid pack, when the host has it.** It takes `.csv` and `.tsv` first, as the
   Markdown pack takes `.md`.
2. **Otherwise this pack,** as `csv-as-text`. `TextPack::for_path` already gives that type,
   so a host needs no code for it now and only puts the grid pack ahead of it later.

A doc opened as `csv-as-text` uses the `text` schema. When a grid pack takes the file over,
its next open is `PackMismatch`, as for any change of pack, and the host decides whether to
re-import (section 5, renames). A host should say the file is in text mode, so people
expect what it does and doesn't do (README, "CSV and TSV as text"):

- Edits to different rows and different cells merge, for people live and for agents
  through `write --base`.
- A moved or sorted row has no identity. A concurrent edit to it conflicts (agents) or
  leaves a fragment where the row was (people).
- Column moves rewrite every line.
- Nothing checks the table's shape.
- A CSV with CRLF rows and a bare LF inside a quoted cell (Excel on Windows) is refused
  `mixed_line_endings` and opens the non-live way, as any mixed file does.

## 3. Refusals to show

The codes are the kit's. A host can map them to messages, or show `message` as it is
(it names the problem and where it is).

| `code` | When | Example `message` |
|---|---|---|
| `too_large` | The file is over its type's cap | `file is 1048577 bytes; live editing supports up to 1048576 for this type` |
| `not_utf8` | The file has a UTF-16 or UTF-32 BOM, or invalid UTF-8 | `file is UTF-16LE (byte-order mark); live editing supports UTF-8 only`; `file is not valid UTF-8 (first bad byte at offset 3); ...` |
| `binary` | A NUL byte; or, for an unknown type, a control byte other than tab, LF, VT, FF, CR or ESC | `file contains a NUL byte at offset 9 (binary)` |
| `mixed_line_endings` | More than one line-ending style in one file | `file mixes line-ending styles (2 CRLF, 1 LF, 0 CR); live editing needs one style, so it is left untouched` |

**At open:** `open_file` returns `Err(SessionError::Declined(d))`.

**While live:** another program can make the file refusable, for example by saving it with
mixed line endings. The kit then reports `DiskCheck::Declined(d)` and
`AuditEvent::FileDeclined { code }`, and the session status gets `declined: Some(..)`. Edits
are refused with `Declined` (exit 4), and **the file is never written** while it is
refused.

**Recovery:** once the file decodes again, it is merged in (`FileRestored`). If it holds the
same text in other bytes, it is adopted with no edit. For example, a file of the same text
in one style again takes that style.

## 4. Size caps

| Group | Cap |
|---|---|
| `logs` (`.log`) | 1 MiB (logs are append streams written by programs) |
| Every other group (`csv` included), and unknown names | 4 MiB |

The kit's `Config::disk_read_max_bytes` (64 MiB by default) is checked first. Any file over
it is refused `too_large` without being read. A host that wants a lower ceiling can set it
there. It applies to every type.

## 5. How a tab maps to this pack

- **A tab is a connection.** When a person opens a text file in a tab, the host:
  1. resolves the pack as in section 1;
  2. calls `open_file`;
  3. joins with `Stay::Held` for the tab's socket.

  The editor binds to the `source` root through the kit's JS client. Closing the tab
  leaves.
- **The editor binding must keep line breaks as `\n`.** This is the default for CodeMirror
  6. Don't set a custom line separator. The pack writes each `\n` in the file's own style
  (LF, CRLF or CR) and puts the BOM back, so the editor never sees either. A doc that ends
  up holding a `\r` is refused at write-back with a `PackError` (exit 1) instead of being
  written as a mixed file. The kit passes every edit, write and restore through the pack's
  normalisation, which strips `\r`. Raw editor updates arriving over the sync protocol
  don't pass through it, so the binding has to keep this rule itself.
- **Sessions are keyed by file identity.** One doc serves every tab and agent on the file.
- **Renames:** use `SessionManager::rekey` and `Session::file_moved`, as for any pack.
  Every text type shares the schema, so renaming `notes.txt` to `notes.toml` keeps the doc,
  its history and its blame. A loaded session keeps the type it was opened with (its cap
  and guard) until it is reloaded. Renaming to another pack's type, such as `.md`, is
  `PackMismatch` on the next open. The host decides whether to re-import there.
- **Not-text files** (`NotText`, or a refusal) open the host's non-live way. They should not
  open as an empty, editable buffer.

## 6. Agents

The command-line verbs are the kit's text verbs. `docs/PARITY.md` maps each person action to
its verb. This pack adds none. A host exposes them as it does for Markdown, behind its own
permission check.

Two notes for the host's CLI:

- **`cat` gives the file's exact bytes, BOM included.** A `write --base V` body made from
  those bytes should have its leading BOM (U+FEFF) removed before it reaches
  `write_base`/`replace_text`. If it is kept, it is written as a second BOM, because edit
  verbs never see the first one. Line endings in bodies don't matter, since they are
  normalised in.
- **Policies on file contents belong to the host.** For example, whether `.env` files or
  key files may be read is not decided by the pack. The table lists `.env` as `config`
  because it is text. Visibility is the host's call.

## 7. Host checklist

- [ ] Route by name: the more specific packs first, then `TextPack::for_path`.
- [ ] `.csv`/`.tsv`: a grid pack first once it exists. Until then, show `csv-as-text` files
      as text mode.
- [ ] `None` and `Declined` open the non-live way with the reason shown, never as an empty
      editable buffer.
- [ ] Show `FileDeclined` and `FileRestored` to the participants.
- [ ] The editor binding keeps `\n` line breaks (no custom separator) and binds to `source`.
- [ ] The watcher calls `file_changed()` and ignores `is_write_temp` names.
- [ ] `rekey` and `file_moved` on renames.
- [ ] The CLI strips a leading BOM from `write` bodies, and applies the host's secret policy.
