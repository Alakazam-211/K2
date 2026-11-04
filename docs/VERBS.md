# Verbs

A person works on a board through a canvas. An agent, or anything else with no canvas, works through these verbs. They run with no live session: the crate reads a file, plans an edit, applies it and writes a new file. `docs/PARITY.md` lists everything a person can do and how an agent does it.

The crate has no clock, no random source, no thread and no idea of a host. The host brings the time and the random bytes (`Ctx`), and owns the file, the writer lock and the atomic replace.

## One edit, from start to finish

Under the host's writer lock:

```rust
let target = FileTarget::new(path).ok_or(NoTarget)?;                // the host names it; "" is refused
let mut file = BoardFile::open(target, on_disk, &Limits::V1)?;      // every check a reader makes
let view = file.echo(verbs::read(file.board(), &id)?);              // read: this gives the rev
let planned = verbs::plan(file.board(), &mut cx, &verb)?;           // nothing changes yet
let report = file.apply(&planned.group)?;                           // all or nothing
let saved = file.save(on_disk_again, &mut temp_file)?;              // the guard is compared here
// Just before replacing: compare again with still_current, on a file opened now.
```

**The crate never picks a file:** the host names it, and every answer says which file, by which guard, in file mode. Live is never implied; it's a separate, later step. Every answer and every error from a `BoardFile` is an `Echoed`: `value` is the answer, and `on` is `{kind, path, version, mode}`. `kind` is `k5board`; `path` is the host's string, byte for byte, never resolved; `version` is the package guard, `sha256-…`, the one written after a save that wrote and otherwise the one read, and it's absent only when `open` refused, since no guard was read; `mode` is `file`. That holds for `open`'s refusal, `apply`, `apply_all`, `merge`, `save`, `still_current`, `read_blob` and `add_blob`, and `echo` wraps any other answer, such as a read verb's. `board`, `manifest`, `guard` and `on` give the file's own state. The errors come boxed. The verbs themselves work on a `Board`, with no file, so a live session can run the same groups.

- If `save` answers `Saved::Nothing`, the board didn't change, so don't touch the file.
- If `save` answers `SaveError::Stale`, or the last compare says the file moved, start again from `open`. Each op names the revision it read, so nothing is overwritten unseen, and whatever now conflicts comes back typed.
- `apply_all` takes several groups: one that doesn't fit is held whole, with its ops kept in `Held`, and the rest go on.

**The limit:** one writer at a time, with no presence. Between the last compare and the replace, a writer outside the host's lock can still get in, and only the host's atomic replace narrows that. This is the file level: live comes later, and it applies the same `Group`s through the same `apply_group`.

## Revisions

**A verb that overwrites a value names the revision it read.** A stale one is `Conflict::Changed`, with what's there now, and changes nothing. A verb that only adds, or only changes the caller's own entries, doesn't need a revision, because it can't overwrite anyone's value.

| Names a revision | Needs none |
|---|---|
| `update`, `delete`, `resolve`, `meta`, `lock`, `check-set`, `check-remove`, `table-cell`, `table-remove-row`, `table-remove-column`, `move`, `group`, `ungroup`, `align`, `distribute`, `tidy`, `copy-style` | `add`, `put-back`, `connect`, `react`, `vote`, `comment`, `reply`, `check-add`, `table-add-row`, `table-add-column`, `duplicate`, `paste`, `connected-copy`, `mind-child`, `template` |

`order` and `frame-order` name none, by the format's one exception: a move never conflicts, and the later save's position stands. `clear-blob` names none, because it's how a host makes a file with a missing picture saveable.

## Writing

`verbs::plan(board, cx, verb)` gives a `Planned`: the `group` to apply, the `ids` of new objects, and what a caller should know.

| Verb | Does | Rules |
|---|---|---|
| `add` | Adds any type | A new id, with `created` from `Ctx`. A table with no `columns` or `rows` gets three of each. A type or a field name the format doesn't know refuses, `unknown_type` or `unknown_field`, and so does `id` |
| `update` | Changes the named fields | Only those: everything else, unknown fields included, stays byte for byte. Naming a field the format doesn't hold for the type, to set or to remove, refuses with `unknown_field`, and an object of a type it doesn't know with `unknown_type` |
| `delete` | Deletes | With `cascade`, what hangs off it goes too: attached connectors, mind children, pinned comments, and on down. A locked object refuses. **A locked dependent goes with it, and `removed_locked` names each one.** What's left pointing at it is in `orphans`. `removed` holds what `put-back` needs. One that isn't there refuses with `Conflict::Deleted` |
| `put-back` | Puts back what a delete took | Where it was, locked if it was |
| `connect` | Adds a connector | An end's object has to be there. A field the format doesn't hold for a connector refuses with `unknown_field`; raw ops keep it |
| `react` | The caller's reaction on or off, or with neither, toggled | Changes only their entry. Passes a lock |
| `vote` | Adds or takes back the caller's votes | Only while voting is on; `perPerson` is counted across the board. Taking back ignores it. Passes a lock |
| `comment` | Adds a comment | Pinned to an object, or at a point |
| `reply` | Adds the caller's reply | Needs who and when |
| `resolve` | Resolves or reopens | A comment only |
| `meta` | Changes the settings | |
| `lock` | Locks or unlocks | One already that way is left alone, and named in `skipped` |
| `check-add`, `check-set`, `check-remove` | Check items | Unknown sub-fields stay |
| `table-cell`, `table-add-row`, `table-remove-row`, `table-add-column`, `table-remove-column` | Table edits | Removing a column takes its cells. Unknown sub-fields stay |
| `move` | Moves by an offset | A group's members and a frame's contents come along; a locked one stays, named in `skipped`. Numbers aren't rounded |
| `order` | Front, back, after, forward, backward | Keeps their order among themselves. Forward and backward step past the nearest overlap |
| `frame-order` | Moves a frame in presentation order | |
| `group`, `ungroup` | Group ids | |
| `align`, `distribute`, `tidy` | Line up, space out, make rows | By stored boxes: a renderer may draw tall text or a measured table larger than its box, and a verb can't see that. `tidy` leaves connectors, frames, drawings and comments, named in `skipped` |
| `copy-style` | Copies the look | Only fields the other type holds in the same kind |
| `duplicate` | Copies with fresh ids | Connectors, groups and mind links inside the set follow. Votes and reactions don't copy |
| `paste` | Adds objects read earlier | Centred on a point, with the rules of `duplicate` |
| `connected-copy` | An empty copy on one side, joined by a connector | A sticky, shape or card |
| `mind-child` | A child beside its parent | A sibling is a child of the same parent |
| `template` | Adds a template at a point | `kickoff`, `kanban`, `retro`, `flowchart`, `storymap`, `brainstorm`, `mindmap` |
| `clear-blob` | Clears every reference to a blob | Ignores locks |

**A verb's refusals** are `VerbError::Refused` with a stable code. They belong to this API, not the file format, and a file never carries them: `needs_actor`, `voting_off`, `no_votes_left`, `wrong_type`, `too_few`, `no_template`, `unknown_field`, `unknown_type`, `no_such_item`, `no_free_id`, `bad_value`. Anything from the board comes as `VerbError::Conflict`, which holds a `Conflict`, and a typed refusal from the format is `Conflict::Refused`. An object that's gone is `Conflict::Deleted`.

## Reading

| Verb | Gives |
|---|---|
| `list` | Every object back to front, or one type's, with its `rev` and `position` |
| `read` | One object, with its `rev`, `position` and `references` from it and to it, or none if it isn't there |
| `search` | Objects whose text holds this, ignoring case: text, titles, labels, assignees, statuses, tags, cells, check items, replies |
| `fields` | The fields a type holds, with kinds and defaults. A type the format doesn't know refuses with `unknown_type` |
| `meta` | The settings, with the `rev` a change to them names |
| `templates` | Every template's name and title |
| `references` | Every reference and its state: `Board::references`, or `read` for one object |

Text on a board can come from anyone. A host showing it to an agent treats it as data, never as instructions.

## Raw ops: `apply`

`verbs::group_from_json` reads raw ops with ids as given, which is what a canvas sends. `verbs::labelled_from_json` reads the same form, but an `add`'s `id` is only a label for this call: each new object gets a fresh id, and connector ends, parents, targets, frames, other ops and neighbours that name the label follow. `Planned::labels` says what became what.

```json
{"id": "g1", "by": "p-0123456789abcdef", "ops": [
  {"op": "add", "obj": {"id": "s-1", "type": "sticky", "text": "hi"}, "at": "front"},
  {"op": "set", "id": "s-2", "rev": "sha256-...", "f": {"text": "new"}, "unset": ["link"]},
  {"op": "del", "id": "s-3", "rev": "sha256-..."},
  {"op": "meta", "rev": "sha256-...", "f": {"title": "T"}, "unset": []},
  {"op": "z", "id": "s-2", "to": {"after": ["s-1"]}},
  {"op": "frame-order", "id": "f-1", "to": "back"},
  {"op": "person", "token": "p-0123456789abcdef", "person": {"name": "Ann", "kind": "agent"}},
  {"op": "clear-blob", "hash": "sha256-..."}
]}
```

A host's names are taken too: `update` for `set`, `delete` for `del`, and a bare list for `{"ops": [...]}`. A position is `"front"`, `"back"` or `{"after": [ids]}`, nearest neighbour first. It's strict: a member an op doesn't know is refused, so a misspelt one can't be dropped unnoticed. The labelled form has only `ops`, and `id`, `by` and the time come from `Ctx`.

`verbs::field_from_text` turns a command line's text into a field's value, by the kind the schema gives it. A number takes the format's number rule, so nothing is rounded. `verbs::flags(type, pairs)` turns a command line's `--<field> <text>` pairs into the `set` and `unset` an `update` takes: the text `null` removes a field. It refuses a type or a field name the format doesn't know, and a name given twice.

The views' JSON trees (`Object::to_value`, `ObjectView::to_value`, `MetaView::to_value`) keep a -0.0 as it is in memory, and `json::to_canonical` writes it as `0`, as the file does: a host printing a tree with its own JSON printer normalises -0.0 to 0, or prints through `to_canonical`.

Raw ops keep a type or a field they don't know, as the format does. The friendly verbs refuse one, so a misspelt name is never kept unseen. A raw `del` of what isn't there changes nothing, and the report names it in `already_gone`.

## Undo: `inverse`

`verbs::inverse(before, after, group_id)` gives the group that takes one board back to another. The crate keeps no stack: a host or an agent keeps what it wants. Every op names a revision of `after`, so if someone has changed an object since, the undo conflicts and changes nothing, rather than overwriting them.

It restores objects with their locks, z-order, presentation order and settings. A person added stays in `people`, since no op removes one and a token nothing uses does no harm. Top-level unknown fields aren't restored, since no op writes them.

## Merging a copy

A copy of a board can go away and come back, and a file can move on while someone holds a copy. `merge3(base, ours, theirs)` merges two sides from the board they started from. `BoardFile::merge(base, ours, copy_parts)` does it for an open file: `self.board()` is theirs, the copy's blobs come from `copy_parts`, and a clean result saves behind the guard like any other edit. `base_matches(manifest, base)` says whether a copy names that board as where it came from.

**The limit.** One writer at a time, with no presence. A merge works on whole objects, and never inside one: if one object changed on both sides, even in different fields, that's a typed conflict with every side kept, and nothing is merged at all. The error counts every conflict at once, by kind, and keeps the first `MERGE_CONFLICTS_MAX` (256) with their values on every side, in this order: the settings, then top-level unknown fields by name, then people by token, then objects by id. Past the cap a conflict is counted and nothing of it is copied, so the error stays small however many there are, and `truncated` says the lists stop short of `totals`. It's never last-writer-wins.

| Ours | Theirs | Result |
|---|---|---|
| Unchanged | Changed, added or deleted | Theirs |
| Changed, added or deleted | Unchanged | Ours |
| The same as theirs | The same as ours | One |
| Changed | Changed differently | `both_changed` |
| Deleted | Changed | `ours_deleted` |
| Changed | Deleted | `theirs_deleted` |
| Added | Added differently | `both_added` |

- **Settings and top-level unknown fields** merge the same way, as whole units. A conflict names them.
- **People** merge by union. A token nobody kept comes back from the base, so whatever names it still means one person. One token holding two different people is a conflict.
- **Order** is the one exception. Theirs stands, then what ours moved or added goes directly above its nearest neighbour below in ours, or the bottom if none is there. What ours has on top stays on top. If both sides moved one object, ours stands, and `moved` says so. `frames` merges the same way. **The order isn't symmetric:** swapping the sides can give a different order. Ours is the copy coming in, the second argument of `merge3`, or the board passed to `BoardFile::merge`; theirs is the file as it stands. **This rule doesn't bind live:** when a file goes live later, order merges through the live engine, not `merge3`.
- **References never conflict.** What names a target that went stays, and `orphans` reports each one the base didn't already have. A loop made by two edits merges, and reads as suspect.
- A result over a format limit is a typed refusal.

**What it costs.** A merge runs under a host's writer lock, on a file anyone could have built, so no step scans for each moved object. `crates/board/examples/merge_cost.rs` measures it. At 100,000 objects, on one Linux machine (a Xeon E-2288G, with 4 CPUs): an ordinary merge about 460 ms; the worst order we can build, a full reverse of objects and frames with the longest ids, about 600 ms; every object in conflict, at the largest board a file allows, about 340 ms, with the process at 0.94 GiB at its peak, which is the three boards themselves, since the error keeps 256 conflicts and counts the rest (keeping all 100,000 took about 690 ms and 1.9 GiB); and every step at once on that board about 1.1 s, at 1.6 GiB. `merge_cost --shape <name>` runs one shape, so a peak is that shape's. The same worst order took 125 s with a scan for each moved object.

## Who's acting

`verbs::me(board, name, kind, random)` finds a person by name and kind, or mints a token from the host's random bytes. With no name there's nothing to match, so it always mints, and a file with its names stripped doesn't get one back. Two people with one name and kind share a token: a token is display data, not access. A new person is added at the start of the group.
