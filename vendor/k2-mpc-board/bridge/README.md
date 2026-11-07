# k2-mpc-board-bridge

The board file verbs (`k2-mpc-board`) plugged into the multiplayer kit's shared verb features
(`k2-mpc-base`), so a host serves board files with the same habits as its other docs:

- **Stable error ids.** Every refusal the board crate can give maps to exactly one of the kit's
  rows, `MPC-E830` to `MPC-E859`. A format refusal's exact code goes in `details.code`.
- **`explain` and `capabilities`**, through the kit, with the board's file verbs as a file registry.
- **JSON reports**: the kit's report, the row's name as `code`, and the file it was about.
- **Dry runs**: everything a real run does, with the save written to nowhere.

The bridge is its own workspace, never a member of the repo's, so the board crate and the repo's
lock stay free of the kit (`scripts/check-no-kit.sh`). It holds no host's names, routes or
commands: a host's command line calls it.

## The command line (`parse`)

`parse::parse(argv, names, limits)` turns a host's argv, from the verb on, into a `Call`; a host
that builds a board `Verb` itself makes the same call with `Call::from_verb` (or `Call::from_read`,
`Call::apply`, `Call::inverse`). A host strips its own flags (the file, `--json`, `--dry-run`)
before calling it.

- **Names are the board crate's.** Verbs and their arguments are `verbs::ARGS`'s. A required
  argument is positional, in that order, or `--<arg> <value>`, or `--<arg>=<value>`; an optional
  one is a flag only. After `--` every token is positional. A flag always takes a value
  (`--cascade true`).
- **Fields.** A verb that takes fields (`add`, `update`, `connect`, `meta`) takes any other
  `--<field> <text>` as one, read by the field's kind in the format (`verbs::flags`, and
  `verbs::settings_flags` for `meta`). The text `null` removes a field; on a new object, which has
  none to remove, it's refused. `update` reads its fields by the type of the object on the board.
- **JSON by design.** The arguments in `verbs::JSON_ARGS` are JSON: `put-back`'s `removed`,
  `paste`'s `objects`, `inverse`'s `before` and `after` (whole boards), and `apply`'s `ops`.
- **`apply`** takes `{"ops": [...]}` or the bare list, labelled (`verbs::labelled_from_json`):
  each `add`'s id is a label for the call, and the new objects get fresh ids. There's no form with
  ids as given here; a host relaying a canvas's own ops calls `verbs::group_from_json` itself.
- **`meta`** with nothing to change reads the settings; a change names `--rev` and a field.
- **`inverse`** plans the group that takes `after` back to `before`. Its ops name the revisions of
  `after`, so if someone has changed an object since, applying it conflicts and changes nothing.

**The text forms** of compound values (ids are `[0-9a-z._-]`, so the separators can't clash):

| Value | Form | Example |
|---|---|---|
| ids | comma-separated | `s-1,s-2` |
| objects with the revision read | `<id>@<rev>`, comma-separated | `s-1@sha256-…,s-2@sha256-…` |
| a position (`add`'s `at`, `frame-order`'s `to`) | `front`, `back` or `after:<ids>` | `after:s-2` |
| a layer (`order`'s `to`) | `front`, `back`, `forward`, `backward` or `after:<ids>` | `forward` |
| a connector end | `<id>`, `<id>:<anchor>` or `<x>,<y>` | `s-1:top`, `1.5,-2` |
| a point (`comment`'s `point`) | `<x>,<y>` | `0,300` |
| a number | the format's number rule (`json::parse` of the literal), so nothing is rounded | `-10.5` |
| `align`, `distribute`, `connected-copy` | the board crate's words (`AlignTo`, `Axis`, `Side`) | `center` |

`parse::usage(verb)` is each verb's usage line for `capabilities`, and `parse::example(verb)` an
example command line that parses.

**A host's own spellings** come in as `parse::Names`, as data: a command that runs a board verb
(`verb`), another name for one (`alias`), a word for an argument (`arg`), a flag that fills a
connector end's anchor (`anchor`), its positional order (`positional`), a value the command sets
itself (`preset`; a call giving another is refused), and a flag that changes nothing on a file
(`ignore`). The board's own names always work too.

**Refusals.** An unknown verb or flag is the kit's `MPC-E120` (`tool_unsupported`: `flag`,
`hint`); a missing, extra or bad argument is `MPC-E122` (`tool_usage`: `reason`). Both exit 2, and
both carry `verb`, and `arg` where there is one. The message never quotes the caller's value: a bad
one is in `details.value`, with the board crate's code in `details.code` where it judged it. A
field's value is the board crate's to judge, so a bad one is that refusal's row, as the caller's
input; `apply`'s ops are judged as raw ops (`MPC-E849` for their JSON).

## Running a call (`dry_run::run`, `dry_run::dry_run`)

Under the host's writer lock, a change runs in this order:

1. **Open** the file as read. A refusal here is the file's (exit 4, or the row's), and its echo has
   no version: no guard was read.
2. **Read the guard** of the file on disk now, apart, before anything is planned. A refusal here is
   the file's too (exit 4), never the write's. If the file moved on since it was read, the answer is
   `MPC-E832` (`busy`, exit 1) with `read` and `on_disk`: the host starts again from the open, and
   after its own retries shows `MPC-E832` as it is.
3. **Plan**, then **apply** in memory. A refusal is the caller's input (exit 2) or a conflict (1, 4,
   5).
4. **Save** to `out`. A refusal while writing is the write's (exit 2).

The host then asks `BoardFile::still_current` once more, just before it replaces the file. A read
verb opens the file and answers; it never reads `on_disk` or writes `out`.

**A dry run** is the same run with the save written to nowhere: every refusal a real run gives
shows, with the same row and exit, and the file is never written. Its answer says `dry_run: true`,
its echo's `version` is the guard read (the file didn't change), and its value names what the save
would have written as `would_write: {guard, revs}`, or `null` when nothing changed. A real run's
value has `saved` in that place, and its echo's version is the guard written.

A call is one verb: a read (`Call::from_read`), or a change (`Call::from_verb`, `Call::apply`,
`Call::inverse`), or one `parse::parse` gives. Merging a copy in
(`BoardFile::merge`) and applying several groups with some held (`BoardFile::apply_all`) are not
calls: a host uses the board crate's library calls for them, and the bridge's reports
(`report::merge_report`, `report::held_report`) for their refusals.

**Rights are per call.** A host checks them with `Call::kind()`: a read needs read, a change needs
read and edit. `capabilities` lists each verb once, by the board crate's verb lists, so a verb that
can change the file is listed as an edit, `meta` among them; `meta` with nothing to change is a
read call, and `Call::kind()` says so.

## JSON (`json::answer_json`, `json::refused_json`)

A refusal is the kit's report whole, with the row's name as `code` after `id`, and the echo under
`on`:

```json
{"id": "MPC-E830", "code": "conflict", "exit": 1, "message": "…", "next": "…",
 "next_step": {…}, "rule": "…", "details": {…},
 "on": {"kind": "k5board", "path": "…", "version": "sha256-…", "mode": "file"}}
```

`on.version` is `null` before the file was read. A host bug has no row, so no `id` or `code`; its
`exit` is `null`, for the host to give its own:

```json
{"exit": null, "host_bug": "…", "on": {…}}
```

An answer is the echo, whether it was a dry run, and the value:

```json
{"on": {…}, "dry_run": false, "value": …}
```

Every board value in either goes through `value_out`, so a number prints as `read` prints it
(12.5 as `12.5`), and -0.0 as `0`, as the file writes it.

## `capabilities` (`registry::registry`)

The board's file verbs as the kit's file registry (`Registry::for_file`), built once: every verb
of the board crate's `verbs::ARGS`, in that order. Each verb's exits are 0, the exit of every kit
row marked `@file` that names it, and those of the parser's usage refusals (`MPC-E120`,
`MPC-E122`), all read from the kit's rows. The parity rows each serves are the board crate's
(`parity::ROWS`). A host passes it to the kit's `run_meta` for `capabilities` and `explain`.

## The kit

The kit is a path dependency, its checkout beside this repo's (`../../k2-mpc-base` from here),
at the kit's `v0.12.0` (`178b449e`). `Cargo.lock` here records that resolve; it moves with
the pin, by `cargo update -p k2-mpc-base`. The kit's optional engine crates aren't in it.

## Licence

MIT OR Apache-2.0, as the repo (`../LICENSE-MIT`, `../LICENSE-APACHE`).
