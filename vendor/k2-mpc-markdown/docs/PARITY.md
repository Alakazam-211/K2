# Agent parity

Agents are first-class collaborators. **Everything a person can do in an editor, an agent can
do through a command-line verb**, as a live participant: with a name, colour, presence,
attribution, and its own undo. The host application exposes these verbs (for example as
`<host> doc <verb> <path>`) behind its own permission check. This crate and the kit provide
the API each verb needs.

Rule: when the editor gains a command, the matching verb and its row here land in the same
change. The same list is data in the crate (`k2_mpc_markdown::parity::parity_row`, one
exhaustive `match` over the kit's `PersonAction`, so a new action doesn't compile until it is
mapped). `tests/agent_parity.rs` runs every row through the kit's parity harness: the person
path (a tab, the session API) and the agent path (registered verbs only) on two identical docs,
comparing text, file, attribution, versions, approvals, presence, exit code and output. It
fails if a row has no verb, a verb is missing from the registry, or a verb behaves differently.
It also checks this table names every verb the data lists.

Rights: **R** read · **E** edit · **H** history · **C** comment · **O** owner. Status:
**live** (verb and parity case pass), **later** (deferred: the verb must not exist yet),
**N/A** (the type has no such action).

| # | Person action | Verb | Rights | Session API | Status |
|---|---|---|---|---|---|
| P1 | Open the file (join; appear in presence) | `open` (any verb joins) | R | `join` | live |
| P2 | Close it (leave) | `leave` (or idle timeout) | R | `leave` | live |
| P3 | Read the whole doc | `cat [-n]`, `wc` | R | `read` (`run_tool`) | live |
| P4 | Look at a range (viewing highlight) | `head`, `tail`, `sed -n a,bp`, `view --lines a,b` | R | `run_tool` (read + viewing range) | live |
| P5 | Find | `grep` | R | `run_tool` | live |
| P6 | See who's here | `who` | R | `roster` | live |
| P7 | Follow someone | `follow <name>` | R | `roster` (carets as line:col) | live |
| P8 | Place the caret / select; say what I'm working on | `cursor --at L:C` / `--select …`; `intent --lines a,b` (later, K-PRES-12) | E (R: a viewing range) | `set_cursor`; readers `set_view` | live (`intent` later) |
| P9 | Type at a position | `insert --at L:C` / `--line N` / `--after N` | E | `run_tool` (`edit_with`) | live |
| P10 | Select and type over | `replace --lines a,b` / `--range …` | E | `run_tool` (`edit_with`) | live |
| P11 | Delete a range | `delete --lines a,b`; `sed -i 'a,bd'` | E | `run_tool` (`edit_with`) | live |
| P12 | Find and replace | `sed -i 's/…/…/g'` | E | `run_tool` (`edit_with`) | live |
| P13 | Paste a block / apply a change | `patch`, `apply`, `append`, `replace-section --heading '## X' [--nth K] [--include-heading]` | E | `run_tool` (`edit_with`); `replace-section` is this pack's (`Pack::verbs`) | live |
| P14 | Rewrite from what I last saw | `write --base V` (three-way; conflict exit 1, expired base exit 8); `--base live` | E | `write_base`; `replace_text` | live |
| P15 | Format (bold, heading, list) | Markdown formatting is text: `insert`, `replace`, `sed -i` | E | `run_tool` (`edit_with`) | live |
| P16 | Undo / redo my own edits (all or nothing; refused with the conflict regions if others changed any) | `undo [--steps N]`, `redo`, `undo --drop` (skip a blocked step) | E | `undo_with`, `redo_with` | live |
| P17 | See that it saved | `status`, `flush` | R | `status`, `flush` | live |
| P18 | Watch others edit live | `watch`, `tail -f` | R | `join` (held) + `subscribe` | live |
| P19 | History, diff, show a version | `log`, `diff --version V`, `show --version V`; `diff` (my copy on stdin) | H (`diff`: R) | `log`, `diff_version`, `show`, `versions` | live |
| P20 | Restore / name a version | `restore --version V`, `version --name N` | E (+H to restore) | `restore`, `name_version` | live |
| P21 | Rename / move | `mv <new>` (a raw rename is also picked up) | E | `SessionManager::rekey` + `file_moved` | live |
| P22 | Blame: who last wrote each line, by which change, when (the blame gutter) | `blame [-L a,b]` (a `git blame`-like listing) | H | `blame(conn, root, lines)` | live |
| P23 | Comment, reply, resolve; suggest; accept / reject per hunk | `comment`, `reply`, `resolve`, `suggest`, `accept`, `reject`, `comments` | C / R (suggest) / E (accept, reject) | later (M5, the kit's annotation layer) | later |
| P24 | Export | N/A: the file is the export; `cat` gives its exact bytes | R | `read` | N/A |
| P25 | See why live editing is off | `status`; verbs exit 4 with the reason | R | `status` (`declined`, `disk`) | live |
| P26 | Ctrl+Z when others have since edited part of my change: undo the regions nobody touched, report the rest; a step whose every region was touched stays on top | `undo --partial` | E | `undo_with(UndoMode::Partial)` | live |
| P27 | Mention someone | `comment --mention @name` | C | later (M5) | later |
| P28 | Purge a pasted secret or personal data (owner) | `purge --range / --version / --text` | O | later (the purge slice) | later |
| P29 | Approve or decline an assistant's edit made on my behalf | `approve <change>` / `decline <change>` | the named person | `set_approval` | live |
| P30 | See which edits are waiting for my approval | `log --pending --on-behalf-of me` | H | `log(LogQuery::pending().on_behalf_of(..))` (a tab: `approvals`) | live |
| P31 | Reverse one specific change, even days later, inside the revert window (older: `restore --version`) | `revert <change> [--partial]` (too old: exit 8) | E (author, the `on_behalf_of` person, or owner) | `revert` | live |

## `replace-section` (P13)

`replace-section --heading '## X' [--nth K] [--include-heading]`, the new body on stdin.

- Replaces the body under the heading, up to the next heading of the same or a higher level
  (subsections belong to it), keeping the body's leading and trailing blank lines. With
  `--include-heading`, the heading and its body up to the last non-blank line.
- `## X` names the level and the text; `X` alone names the text at any level. ATX (`## X`,
  optional closing `#`s) and setext (`X` underlined by `===` or `---`) headings count;
  headings inside fenced code and YAML front matter don't.
- Not found: exit 1, nothing applied. Several match without `--nth`: exit 1, nothing applied,
  and the report lists their line numbers. Bad flags: exit 2.
- One change, like every edit verb: one `undo` reverts it.
