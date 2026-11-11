# IDs and addresses

<!-- Generated from crates/base/src/ids/mod.rs by `ids::kit_markdown()`; don't edit by hand. Regenerate: MPC_BLESS=1 cargo test -p k2-mpc-base --test ids. -->

Every ID or address kind the kit's verbs take or print, as data (`ids::KIT`, K-ID-13). An address is **(scope, id)**: an ID means something only inside its scope (`r17` is a row of one table, `v12` a version of one doc).

## Classes

Each kind has one class (P1), listed most durable first:

| Class | Meaning |
|---|---|
| durable | Survives saves, outside edits and an engine migration; never reused in its scope. |
| file-local | Unique within one file as written; another producer may renumber it on save, so across saves it is a hint. |
| unique-name | A name or a value: it addresses only while exactly one object has it; two or more are refused with the candidates, never "the first". |
| positional | An index, a line or an offset: it moves when anything before it changes. Resolve it against the version you read (`--if-version`). |
| session | Lives only while the session is in memory; never stored, never an anchor. |
| stamp | A change stamp: it says that something changed and never addresses an object; no verb accepts it. |

Anchors bind to the most durable ID an object has; weaker ones are hints for re-matching (its native ID, then kind, name and parent path, then position), and a weak or failed match changes the anchor's state, never where it points. A cell `R,C` is a row address and a column address, as durable as the weaker of the two.

## The kit's kinds

| Kind | Example | Class | Scope | Accepted by | Note |
|---|---|---|---|---|---|
| `line` | `--lines 3,7` | positional | a doc's lines (a grid: its rendered file) | `view`, `insert`, `replace`, `delete`, `sed`, `sed -i`, `claim`, `blame` | 1-based: `--lines a,b`, `--line N`, `--after N`, `-L a,b`, sed's own addresses; moves when a line above it is added or removed |
| `line-col` | `--at 12:5` | positional | a text root's text | `insert`, `replace`, `delete`, `cursor` | `L:C` or `L:C-L:C`: 1-based lines, columns in Unicode scalar values (UTF-8 bytes inside the kit); positions the kit stores move with every change |
| `exact-text` | `--old 'TODO: x'` | unique-name | a doc's text | `replace` | must match exactly once (unless `--all`); none or several apply nothing and exit 1 with each match's lines |
| `row-key` | `@k:r17` or `r17` or `key:r17` | durable | a grid doc's rows | `cell get`, `cell set`, `cells set`, `cells clear`, `row insert`, `row move`, `row delete`, `row restore`, `claim`, `cursor`, `blame --cell` | minted once, never reused; follows moves and sorts; kept through outside saves (the aligner re-matches rows, and a tie never pairs) and an engine migration; printed by `row insert` and `row list`; a deleted row is named by key only; `@k:` is the canonical spelling, `key:` the same, and a bare word is the key |
| `col-key` | `@k:c3` or `c3` or `key:c3` | durable | a grid doc's columns | `cell get`, `cell set`, `cells set`, `cells clear`, `row list`, `col insert`, `col move`, `col delete`, `col restore`, `col rename`, `sort`, `claim`, `cursor`, `blame --cell` | as a row key; a bare word that is a known column key is the key, `@k:` (canonical) or `key:` forces it; printed by `col insert` and `col list` |
| `row-nth` | `#3` | positional | a grid doc's live rows, in order | `cell get`, `cell set`, `cells set`, `cells clear`, `row insert`, `row move`, `row delete`, `row restore` | the Nth live row (`--row #N`, `--at #N`, `--to #N`); past the end exits 4 |
| `col-nth` | `#2` | positional | a grid doc's live columns, in order | `cell get`, `cell set`, `cells set`, `cells clear`, `row list`, `col insert`, `col move`, `col delete`, `col restore`, `col rename`, `sort` | the Nth live column; past the end exits 4 |
| `rendered-line` | `L5` | positional | a grid doc's rendered file | `cell get`, `cell set`, `cells set`, `cells clear`, `row insert`, `row move`, `row delete`, `row restore` | the row rendered line N belongs to; the header's or a preamble's line exits 4 |
| `col-name` | `email` or `name:email` | unique-name | a grid doc's header names (header on) | `cell get`, `cell set`, `cells set`, `cells clear`, `row list`, `col insert`, `col move`, `col delete`, `col restore`, `col rename`, `sort`, `row move`, `row delete` | a bare word that is no column key, or `name:`; two live columns with the name exit 2 listing their keys; with the header off, exit 4 |
| `where` | `--where sku=A7` | unique-name | a grid doc's live rows, by one column's value | `cell get`, `cell set`, `row move`, `row delete` | the one live row whose cell is the value; several exit 2 listing their keys, none exit 4 (`row list --where` filters instead) |
| `version` | `v1842` or `1842` | durable | a doc's history | `insert`, `replace`, `delete`, `sed -i`, `patch`, `apply`, `append`, `cell set`, `cells set`, `cells clear`, `row insert`, `row move`, `row delete`, `row restore`, `col insert`, `col move`, `col delete`, `col restore`, `col rename`, `sort`, `header`, `write`, `diff --version`, `show --version`, `export --version`, `restore`, `version --name` | a log position, printed by every read; `--if-version`, `--base`, `--version`, `--to`; retention may drop an unnamed stored version (a base then exits 8) |
| `change` | `revert 41` | durable | a doc's change log | `revert`, `approve`, `decline` | a change's log position, as `log` prints it (a blob doc: its version) |
| `participant` | `--by p2` | durable | the host's participants | `follow`, `undo`, `undo --partial`, `log`, `log --pending`, `approvals` | host-given (the kit never mints or renames one): `follow <pid>`, `--for`, `--by`, `--on-behalf-of` |
| `claim-id` | `claim-12` or `12` | session | the process | `claim --renew`, `claim --release` | printed by `claim`; unique within the process, so another doc's or an ended claim's id is simply not live |
| `annotation` | `ann-12` or `12` | durable | a doc's comment threads | `reply`, `resolve`, `resolve --reopen`, `comment --delete`, `reanchor`, `comments --log` | printed by `comment` and `comments`; per doc, never reused (a purged thread keeps its id); a comment within it is `--n N`, 0 the first |

## Packs

A pack declares the kinds its own codec and verbs read (`Pack::ids`, default none) in the same form, and generates its own `docs/IDS.md` from them with `ids::markdown`; `ids::check` holds the rows to its verb registry. A pack lists only its own kinds: the kit's apply to every pack of their tier.

**The ID-churn judge** (K-COD-10; `ids::churn::judge`, feature `test-support`): over a pack's corpus, a no-op save (decode, encode, read back) must give the file's bytes back and keep every ID, and each one-unit edit (a verb's edits written back, or an outside save read back) must keep the ID of every object it doesn't touch. It is a test harness: a write that would renumber an ID is not refused at run time yet.
