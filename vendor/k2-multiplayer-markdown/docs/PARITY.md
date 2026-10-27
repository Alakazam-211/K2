# Agent parity

Agents are first-class collaborators. **Everything a person can do in an editor, an agent can
do through a command-line verb**, as a live participant: with a name, colour, presence,
attribution, and its own undo. The host application exposes these verbs (for example as
`<host> doc <verb> <path>`) behind its own permission check. This crate provides the API
each verb needs.

Rule: when the editor gains a command, the matching verb and its row here land in the same
change. `tests/agent_parity.rs` walks this table and fails if a row has no verb or behaves
differently.

Rights: **R** = may read, **E** = may edit.

| # | Person action | Verb | Rights | Session API |
|---|---|---|---|---|
| P1 | Open the file (join; appear in presence) | `open` (any verb joins) | R | `join` |
| P2 | Close it (leave) | `leave` (or idle timeout) | R | `leave` |
| P3 | Read the whole doc | `cat [-n]` | R | `read` |
| P4 | Look at a range (viewing highlight) | `head`, `tail`, `sed -n a,bp`, `view --lines a,b` | R | `read` + `set_view` |
| P5 | Find | `grep` | R | `read` + `tools::grep` |
| P6 | See who's here | `who` | R | `roster` |
| P7 | Follow someone | `follow <name>` | R | `roster` (cursors as line:col) |
| P8 | Place the caret / select | `cursor --at L:C` / `--select …` | E (R: viewing range) | `set_cursor` |
| P9 | Type at a position | `insert --at L:C` / `--line N` / `--after N` | E | `edit` |
| P10 | Select and type over | `replace --lines a,b` / `--range …` | E | `edit` |
| P11 | Delete a range | `delete --lines a,b`; `sed -i 'a,bd'` | E | `edit` |
| P12 | Find and replace | `sed -i 's/…/…/g'` | E | `tools::sed` + `edit` |
| P13 | Paste a block / apply a change | `patch`, `append`, `replace-section` | E | `tools::*` + `edit` |
| P14 | Rewrite from what I last saw | `write --base V` (three-way; conflict = exit 1) | E | `write_base` |
| P15 | Format (bold, heading, list) | Markdown formatting is text: P9–P13 | E | `edit` |
| P16 | Undo / redo my own edits | `undo [--steps N]`, `redo` | E | `undo`, `redo` |
| P17 | See that it saved | `status`, `flush` | R | `status`, `flush` |
| P18 | Watch others edit live | `watch`, `tail -f` | R | `subscribe` |
| P19 | History, diff, show a version | `log`, `diff --version V`, `show --version V` | R | `versions`, `version_text`, `diff` |
| P20 | Restore / name a version | `restore --version V`, `version --name` | E | `restore`, `name_version` |
| P21 | Rename / move | `mv <new>` (a raw rename is also picked up) | E | `rename` |
| P22 | Blame | `blame [-L a,b]` | R | `blame` (planned) |
| P23 | Comment / suggest / accept / reject | N/A for Markdown (plain-text format) | — | — |
| P24 | Export | N/A: the file is the export; `cat` gives its exact bytes | R | `read` |
| P25 | See why live editing is off | `status` | R | `status` |
