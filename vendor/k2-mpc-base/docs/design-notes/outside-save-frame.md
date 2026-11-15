# What the kit emits for an outside save (keyed docs)

An outside save is the file changed on disk by another program while the doc is live (a desktop
app saving a deck, a timeline, a board). This note lists exactly what the kit emits for one,
field by field, with where each is made. File and line references are to this branch.

**The frame to forward is `outside_save`** (section 0): on a keyed watch the kit sends one
unnumbered frame saying what came of each save, after the save's own change frame
(`ops_change`, section 3), which is unchanged. A host forwards it as it is. The host also gets the
outcome as the return value of its disk call (`DiskCheck`, section 1) and as audit events
(`AuditEvent`, section 2).

## 0. The `outside_save` frame, on the keyed watch

`WatchFrame::OutsideSave(WatchOutsideSave)` (`crates/base/src/session/live/watch.rs:281`), its
outcome `OutsideSaveOutcome` (`watch.rs:292`) and copy `OutsideSaveCopy`
(`watch.rs:324`); its JSON is `outside_save_json` (`watch.rs:827`). It is sent by
`Session::watch_outside_save` (`watch.rs:2005`) to every watch whose connection may read the
doc then (the host's rights, asked then), after the watch's first snapshot. Unnumbered: no `seq`,
never counted, never asked for again. `client/WIRE.md`, "Outside saves".

| Field | Type | Meaning | When present |
|---|---|---|---|
| `type` | string | `"outside_save"` | always |
| `at_ms` | number | when the kit handled the save (host clock) | always |
| `doc` | string | the doc's address: its `DocKey::path` | always |
| `outcome` | string | `ingested`, `kept_both`, `declined` or `missing` | always |
| `rev` | number | the doc's version after the save (its change frame's `rev`) | `ingested`, `kept_both` |
| `kept` | number | the stored version id holding what was live before the save | `kept_both` |
| `copy` | object or null | the conflict copy, `null` when it couldn't be written | `kept_both` |
| `copy.name` | string | the copy's file name | in `copy` |
| `copy.path` | string | the copy's path relative to the doc's file's folder, as the kit records it | in `copy` |
| `copy.doc` | string | the copy's doc address, when the copy is itself a kit document (the host's `Host::document_at`, `crates/base/src/session/host.rs:146`) | in `copy`, when the host says so |
| `code` | string | the pack's decline code (`too_large`, `not_utf8`, `malformed`...) | `declined` |
| `message` | string | why, in plain words | `declined` |

When each is sent:

- `ingested` and `kept_both`: one per save the kit takes in, from the keyed save path
  (`crates/base/src/session/live/grid.rs:4187`; the copy at `grid.rs:4146`) and a keyed
  doc's text roots (`crates/base/src/session/live.rs:6934`).
- `declined` and `missing`: once, when the file stops decoding or goes (`live.rs:6336`,
  `live.rs:6313`), not on every check after.
- Nothing for a check that finds the file unchanged, or holding the kit's own write.
- **Keep both means the save wins.** On `kept_both` the live doc *is* the save; what was live
  before is the version `kept` and the copy.

## 1. The outcome: `DiskCheck`, returned to the host

The host calls `Session::file_changed()` (`crates/base/src/session/live.rs:5549`) or
`Session::check_disk()` (`live.rs:5564`) when the file may have changed. Both return
`DiskCheck` (`crates/base/src/session/disk.rs:34`), a Rust enum (no JSON form of its own):

| Variant | Fields (type) | Meaning | When |
|---|---|---|---|
| `Unchanged` (`disk.rs:36`) | none | nothing to take in | the file holds the doc (or what the kit last synced), or a save of the same content in other bytes |
| `OwnWrite` (`disk.rs:38`) | none | the file holds one of the kit's own recent writes; not taken in, and the next write-back puts the doc back | an app saved a stale buffer, or a late echo |
| `Ingested` (`disk.rs:40`) | `version: Seq` (u64): the doc's version after the save | the save was applied onto the live doc, by key, as the file's change | the kit did not write the file since the last outside save, and the doc had no changes the file lacks |
| `Conflict` (`disk.rs:54-58`) | `version: Seq`: the doc's version after the save; `kept: Seq`: the id of the stored version holding the kit's version (the live doc just before the save); `copy: Option<PathBuf>`: the conflict copy's path, `None` when it couldn't be written | **keep both**: the save became the live doc, and the kit's version is kept as a version and as a conflict copy | a keyed doc that isn't a table, when the kit wrote the file since the last outside save, or the doc had changes the file lacks (`crates/base/src/session/live/grid.rs:3908-3914`, built at `grid.rs:4144-4173`) |
| `Merged` (`disk.rs:61`) | `version: Seq` | tier 0.5 only: a pack merged the save | not on a live keyed doc |
| `Declined` (`disk.rs:44`) | `Declined { code: &'static str, message: String }` (`crates/base/src/session/pack.rs:25-30`) | the file can't be a live doc any more (`code`: `too_large`, `not_utf8`, `binary`, `mixed_line_endings`, or a pack's own); nothing taken in, write-back stops, edits are refused until it decodes | the save doesn't decode |
| `Missing` (`disk.rs:42`) | none | the file is gone; the doc is kept | deleted, or replaced by a folder |

Notes:

- **Keep both means the save wins.** On `Conflict` the live doc *is* the save (the change
  frame below carries it); what the live doc held before is the kept version and the copy.
  The live view does not stay as it was.
- `version` and `kept` are different kinds of number on `Conflict`: `version` is a doc
  version (a change), `kept` a stored version's id (`disk.rs:50-53`).
- The copy's name: `Host::conflict_copy_name(file_name, at_ms, who)`
  (`crates/base/src/session/host.rs:137`), else the pack's, else the kit's default
  `name (conflict <who> YYYY-MM-DD HHMMSS).ext`; in `Host::conflict_dir` (`host.rs:125`) or
  beside the file. `who` is the last writer of the kit's version.
- A save that changed only codec settings (a BOM, line endings, a delimiter; the same content)
  is not a change: `AuditEvent::OutsideSettings` below, and no change frame.

## 2. Audit events, to `Host::on_event`

`AuditEvent` (`crates/base/src/session/host.rs:292`), each with the doc's `key: DocKey`. None
carries content.

| Event | Fields | When |
|---|---|---|
| `ExternalEdit` (`host.rs:322`) | `seq: Seq`: the change the save became | every save taken in (`Ingested` and `Conflict`; emitted at `grid.rs:4114` and `live.rs:6925`) |
| `BlobConflict` (`host.rs:416-422`) | `current: VersionId`: the stored version holding the save; `kept: VersionId`: the one holding the kit's version; `copy: Option<String>`: the copy's path relative to the file's folder; `lease_holder: Option<String>`: always `None` on a live keyed doc | once per keep-both save, beside `DiskCheck::Conflict` (emitted at `grid.rs:4160-4166`) |
| `OutsideOverride` (`host.rs:342-349`) | `change: Seq`, `rule: &'static str`, `region: String`, `by: Option<String>`, `version: Option<VersionId>` | a save that overrode someone's live work: a table's element, or someone's text in a text root (a text doc's, or a keyed doc's text root); never for a deck's or a board's elements |
| `OutsideSettings` (`host.rs:362-366`) | `version: VersionId`, `description: String` | a save that changed only the codec settings |
| `EchoSuppressed` (`host.rs:372`) | none | `DiskCheck::OwnWrite` |
| `FileDeclined` (`host.rs:356`) | `code: &'static str` | `DiskCheck::Declined` |
| `FileMissing` (`host.rs:352`) | `unflushed: bool`: the doc had changes the file never got | `DiskCheck::Missing` |

## 3. On the keyed watch: the save's change frame (`ops_change`)

A save taken in (`Ingested` or `Conflict`) is one numbered change frame on every keyed watch,
sent once the save is the doc's carrier (`grid.rs:4175-4177`). It is the ordinary change frame
(`client/WIRE.md`, "The keyed watch"); its JSON is built by `change_json`
(`crates/base/src/session/live/watch.rs:860-922`). For an outside save its fields are:

| Field | Type | Value for an outside save | Where |
|---|---|---|---|
| `type` | string | `"ops_change"` | `watch.rs:863` |
| `seq` | number | the watch's next frame number | `watch.rs:864` |
| `rev` | number | the doc's version after the save: `DiskCheck`'s `version` | `watch.rs:865` |
| `at_ms` | number | when the kit took it in (host clock) | `watch.rs:866` |
| `change_kind` | string | `"external"` (`ChangeKind::External`, `crates/base/src/store/history.rs:192`) | `watch.rs:867` |
| `actor` | `{id, name, kind, colour}` | `{"id":"kit:external","name":"kit:external","kind":"file","colour":null}` (`EXTERNAL_PID`, `crates/base/src/session/participant.rs:58`; `watch_actor`, `watch.rs:2050-2057`; `file` is `ParticipantKind::External`, `watch.rs:696`) | `watch.rs:868` |
| `client` | number | the file's own client id in this doc | `watch.rs:869` |
| `via` | string | `"disk"` | `watch.rs:870` |
| `on_behalf_of` | string or null | `null` | `watch.rs:871` |
| `turn` | string or null | `null` | `watch.rs:872-877` |
| `verb` | string or null | `null` | `watch.rs:878` |
| `claim` | string or null | `null` (the file holds no claim) | `watch.rs:879` |
| `ops` | list | the save's ops by address, root by root | `watch.rs:880` |
| `units`, `containers_changed`, `containers` | lists | the view deltas, for a pack with a view schema only (absent otherwise) | `watch.rs:899-920` |
| `derived` | object or null | the pack's derived values | `watch.rs:881` |
| `render_hashes` | object or null | containers whose render hash changed | `watch.rs:882-894` |
| `view_space` | object | a 3D slot's space after the save, only when it changed | `watch.rs:896-898` |

The change frame carries no outcome: a `Conflict` save's change frame is the same shape as an
`Ingested` one. The outcome, the kept version and the copy are on the `outside_save` frame that
follows it (section 0).

Two more cases on the watch:

- **A save that changed only what the carrier holds** (no change to the live store, but a
  placement moved): no change frame; every watch is cut and must take a fresh snapshot
  (`Session::settle_carrier_watch`, `watch.rs:1821-1833`).
- **`Unchanged`, `OwnWrite`**: nothing on the watch. **`Declined`, `Missing`**: no change frame;
  the `outside_save` frame (section 0), once.

## 4. Text docs, for comparison

A text doc's save is a `ChangeEvent` on the hunk channel (`crates/base/src/session/live.rs:694-726`;
`kind` `external`, `pid` `kit:external`, `via` `disk`, `turn` and `verb` `null`), and a removal
of someone's text is an `overwritten` roster note to its author (`client/WIRE.md`, "The
`overwritten` event"). Neither is on the keyed watch.
