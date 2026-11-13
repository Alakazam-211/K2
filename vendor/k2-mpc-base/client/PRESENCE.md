# Presence and marks as data

`LiveText` gives a host the data to draw other people's carets and the person's rejected
changes in its own editor UI. The library has no DOM: it holds the data, says when it changed,
and acts on the host's request. The host owns all look and feel: it draws its own editor
(HOST.md has the calls and rules; the kit's own editor component was removed in v0.10.2).

**Offsets are UTF-16 code units of the replica's current text** (`live.text`): what CodeMirror
and a JS string index use. The wire, `carets()` and `Rejection.region` stay in UTF-8 bytes. The
library converts through its rope's index (a binary search and one chunk's scan per offset,
never a scan of the text), after mapping each caret through every change since its version.

## Participants

`participants(): Participant[]` is a snapshot: one entry a **connection**, in roster order.

| field | meaning |
|---|---|
| `handle` | the participant (the roster's `handle`, a change's `pid`) |
| `client?` | the connection (`Device.client`), from a host that sends per-connection carets (kit v0.10.1); absent from an older host and on a caret-less entry |
| `name` | the roster's `name` |
| `colour` | the roster's `colour`, exactly as sent: a stable identity value for the host to map to its own palette, not a style instruction |
| `kind` | `"person"`, `"agent"` or `"app"` (stable; see below) |
| `rawKind` | the roster's own `kind` string |
| `isSelf` | this connection |
| `sameUser` | another connection of this connection's participant (another tab or device) |
| `label?` | the host's label for the connection (`Device.label`); never doc text |
| `caret` | `{anchor, head}` in UTF-16, or `null` (none to show) |
| `selections` | every range of the connection: one element (the caret) today, as the wire carries one range a connection; empty with no caret. Draw from it to keep working when the wire carries several |
| `lastActiveAt` | when the connection's caret last moved or it last changed the text, on the replica's clock (`LiveOptions.timers.now()`, default `Date.now()`); when first seen until then. A caret re-sent unmoved after other people's changes doesn't count. For fading idle carets |
| `away` | the roster's `away` |

Who is listed:

- **This connection: once,** with `isSelf: true`, `caret: null` and no selections (the host
  draws its own caret), at its participant's place in the roster, so a list can show "you".
  It is never given a caret. It is absent until the roster lists its participant.
- **The person's other connections:** `sameUser: true`, `isSelf: false`, with their carets.
  This needs per-connection carets from the host and this connection's client ID
  (`LiveOptions.client`, `welcome.client`); without the ID none are shown, since this
  connection can't tell which is its own.
- **Everyone else:** one entry a connection with a caret (as `carets()`: most active first),
  or one caret-less entry (no `client`) for a participant with no caret to show. An older host
  (no `devices`) gives one entry a participant.

**Kinds.** The roster's `kind` (`ParticipantKind` in lower case) maps `person` to `person` and
`agent` to `agent`; everything else (`app`, `external`, `restore`, `import`, and any added
later) is `app`: a program acting on the doc. `participantKind(raw)` does the mapping;
`rawKind` keeps the original.

## Marks

`marks(): Mark[]` is a snapshot of the open rejected changes (`live.rejected`), oldest first.

| field | meaning |
|---|---|
| `id` | `Rejection.id`: what the actions take |
| `range` | `{from, to}` in UTF-16: where the change was (it moves with every change). Empty for a deletion's place, or after a reload (no place) |
| `text` | what the person typed (may be empty) |
| `reason` | `{code, message}`: the server's error when it sent one (`MPC-E111` write conflict, `MPC-E000` reply lost, `MPC-E901` rate, ...); else `MPC-E111` when the client saw the change meet competing text, `MPC-E000` when the replica started again from the server's text first |
| `error` | the server's full refusal (`details` included), or `null` |
| `mayHaveApplied` | its reply was lost (`MPC-E000` with a record): it may have landed |
| `presence` | `presence(id)` for a lost reply; else `"absent"` (refused) |
| `placement` | `placeRejection(id)`: `"ok"`, `"in-text"` (all of it is in the text: Re-apply only dismisses), `"conflict"`, `"cant-place"` |
| `canReapply` | `presence` is `"absent"` and `placement` is `"ok"` |
| `canInsertHere` | "Insert my text here" would go ahead now |
| `removed` | the change also removed text (Insert here puts back only the typed text) |

A change whose reply was lost and that lands after all leaves the marks (`onLanded`).

**Actions:**

- `reapply(id)`: Re-apply by the server's rule (unchanged). It doesn't check presence: offer
  it when `canReapply`.
- `dismiss(id)`: forget the mark.
- `insertHerePreview(id)`: where "Insert my text here" would put the typed text, as
  `{at, text, removed}` (`at` in UTF-16), or `{refused}`. Changes nothing.
- `insertHere(id, {group?})`: insert it there as a new local edit in its own undo group (a
  fresh `insert-here-N` unless `group` is given; never composed with the edits around it), and
  dismiss the mark. Returns `{op, at, text}` or `{refused}`.

"Insert my text here" is the person's own choice, not the server's rule: it inserts the typed
text at the change's place as the replica tracked it (`Rejection.region.start`) and never
deletes. The checks, in order: a rejection with typed text (else `unavailable`), one hunk
(else `multi-hunk`: no one place), then for a lost reply the record (`present`:
`already-present`, and `insertHere` dismisses it; not `absent`: `cant-tell`). The host checks
its own conditions first (edit rights, no composition in progress).

## Events and batching

`onParticipants(cb)` and `onMarks(cb)` call back when the snapshot changes. Each returns an
unsubscribe.

- **At most one callback a listener a delivery,** with the snapshot as it is then (never a
  stale one); none when it equals the last one that listener had. The first comparison is with
  the snapshot when it subscribed: read `participants()` / `marks()` for the start.
- **This connection's own activity is no change by itself:** the person typing or moving the
  caret updates its own entry's `lastActiveAt` (read it from `participants()`), but
  `onParticipants` doesn't call back for that alone; the value rides along with a callback made
  for another reason (its `away` flipping, someone else moving). Own edits that move other
  people's carets in UTF-16 (typing before them) do change the snapshot and call back.
- **Deliveries:** every change asks for one; changes until it runs share it. When it runs is
  `LiveOptions.schedule`:
  - default: `requestAnimationFrame` when the global has one, else a `FRAME_MS` (16 ms) timer
    on `LiveOptions.timers`;
  - `(deliver) => deliver()`: immediate delivery, one callback a change (an agent, a Node tool);
  - any other function that runs `deliver` once, later.
- After `close()` nothing is delivered.

## Selection by element, pointer and viewport

`PresenceChannel` is for docs made of keyed elements. It takes any `Transport` and the
connection's `welcome`, can share them with a `LiveText`, and has no DOM.

```ts
const presence = new PresenceChannel(transport, welcome);
presence.setSelection({ root: "items", keys: ["i1", "i2"] }); // null or no keys: clear
presence.setPointer({ x: 120, y: -40 });                      // null: clear
presence.setViewport({ x: 0, y: 0, w: 800, h: 600 });         // null: clear
presence.onChange((peers) => draw(peers));
presence.onRefused(({ frame, error }) => report(frame, error));
presence.reconnected(nextWelcome);                            // after every reconnect
```

- **Frames go out only when the host takes them:** `presence.supported` is whether
  `welcome.features` lists `"presence"`. Otherwise the setters only record the value, and
  `reconnected` sends it if a later connection lists the feature.
- **The pointer and the viewport are batched:** at most one frame per `schedule` call
  (`PresenceChannelOptions.schedule`; the default as for `LiveText`), the latest value. A selection
  goes at once.
- **Refusals:** `onRefused` gets the frame's type and the error. The host refuses a key that isn't
  live (`MPC-E114`), which can happen when a deletion crosses the frame. The channel refuses itself,
  unsent, more than 256 keys (`MPC-E310`) and coordinates the host would refuse (`MPC-E114`). A
  refused value isn't sent again on a reconnect.

`peers()` is a snapshot, in roster order:

| field | meaning |
|---|---|
| `handle`, `name`, `colour`, `kind`, `rawKind`, `away` | as on `Participant` above |
| `client?` | the connection; absent on a participant's row with nothing to show, and on a grid (one row a participant) |
| `isSelf`, `sameUser` | as on `Participant` above |
| `selection` | `{root, keys}` or `null` |
| `pointer` | `{x, y}` or `null`, in the doc's own coordinates |
| `viewport` | `{x, y, w, h}` or `null`, in the doc's own coordinates |

There is one row for each connection that has a value, one for a participant with none, and this
connection's row. The self row holds what the host last reported, so it lags the setters: the host
draws its own selection from its own state.

## Focus and range (wire 3)

`focus` (the container in view, a slide) and `range` (a caret or selection inside one element's
text field), as K2's rendering data asks. Both need a wire-3 host that lists their feature.

```ts
presence.setFocus({ root: "slides", key: "s259" });           // null: clear
presence.setRange(
  { root: "shapes", key: "s259-3", field: "text", anchor: 0, head: 4 }, // UTF-16 into `text`
  fieldText,                                                    // the field's text you show
);
```

- **Frames go out only when the host takes them:** `focusSupported` and `rangeSupported` are
  whether `welcome.features` lists `"focus"` and `"range"`. Otherwise the setters record the value,
  and `reconnected` sends it (the focus before the range) if a later connection lists them. A
  wire-2 host (no `welcome.wire`; `presence.wire` is 2) takes neither.
- **Offsets:** `setRange` takes UTF-16 indexes into the field's text and sends UTF-8 bytes, as
  every offset on the wire. A peer's `range` is in UTF-8 as the host sent it:
  `rangeToUtf16(range, text)` gives UTF-16 indexes into the text you show, or `null` when that text
  isn't the one the range was set on.
- **The host moves your focus with your range** (to the element's container), and **drops a range**
  (`range: null` on the roster) when its field changes: send a new one then. It is never moved.
- **Refusals:** the host refuses a key that isn't live (`MPC-E114`), offsets off the field's text
  (`MPC-E114`), a reader's range (`MPC-E102`), and a watcher's focus or range (`MPC-E305`). The
  channel refuses itself, unsent, offsets outside the text it was given (`MPC-E114`). A refused
  value isn't sent again on a reconnect.

`peers()` gains `focus` (`{root, key}` or `null`) and `range` (`{root, key, field, anchor, head}`
or `null`, UTF-8).

**Watchers.** Past the host's participant cap a connection watches: it has no row in `peers()`,
and the host refuses its presence frames (`MPC-E305`). `presence.watching` is how many are
watching (the roster's snapshot `watching` and its `watching {count}` events), and
`onWatching(cb)` says when it changes, so a view shows "+1 watching".

## Claims on elements

`PresenceChannel` also claims elements of a list root, so others know to keep off them while you
work. A claim is advisory: it refuses an agent's edit only when the host asks for that, and never a
person's.

```ts
if (presence.claimsSupported) {
  const claim = await presence.claim("items", ["i1", "i2"], { hold: true, note: "moving these" });
  // ... several edits ...
  await presence.release(claim); // no id: all of this participant's in the doc
}
```

- **Frames go out only when the host takes them:** `presence.claimsSupported` is whether
  `welcome.features` lists `"claims"`, separately from `supported`. Otherwise `claim` and `release`
  send nothing and reject with `unsupported`.
- **`hold`:** without it, the claim ends on your first edit touching it. Set it to make several
  edits under one claim, and release when done. It still runs out: `ttlMs` is clamped to the host's
  most.
- **Rejections:** both reject with a `ClaimRejected`, whose `reason` says why:

| `reason` | meaning | the claim |
|---|---|---|
| `refused` | `error` says why: from the host (`MPC-E301` someone else's claim, `MPC-E114` a key that isn't live or a root that isn't a list, `MPC-E303`, `MPC-E903`, `MPC-E904`), or from the channel, unsent (`MPC-E304`: no keys, more than 256, a note over 1024 bytes, a bad `ttlMs`). `onRefused` hears these too | not taken |
| `unsupported` | this connection's host doesn't list `"claims"`; nothing was sent | not taken |
| `reconnected` | the connection changed before the host answered | unknown: see below |
| `closed` | `close()` was called before the host answered | unknown: see below |

- **A claim doesn't outlive the connection that took it,** held or not. So when a request is caught
  by a reconnect, the claim may never have been taken, and if it was, it has ended or will end when
  the old connection closes. Read the roster, and claim again if you still need it. The channel
  never sends a claim again by itself.
- **A host that never answers:** the request waits until `reconnected` or `close()`. Call `close()`
  when the connection ends for good.
- **Who holds what** comes from the roster: `claim_started_elems` and `claim_ended` through
  `LiveText.onRosterEvent` or your own `roster_event` handling, and a snapshot's `elem_claims`.
  `presence.claims()` folds them for you (`onClaims` says when they change): one `ClaimView` a
  live claim, `{claim, actor: {id, name, kind, colour}, root, keys, note, hold, startedMs,
  expiresMs}`. A wire-3 host's events carry the actor; from a wire-2 host it comes from the
  claimant's roster row, and `startedMs` is `null`.
- **An agent's claim moves its presence** (wire 3): its focus goes to the claim's first
  container, and its selection becomes the claimed keys, on the roster right after the claim's
  start.
- **An agent's verbs move its presence too** (K2's ask): each pack verb an agent runs that
  changes elements sets its focus to the container of the last element it touched (the last
  container touched wins) and its selection to the live elements it touched. They arrive as the
  usual `focus` and `selection` roster events: nothing new to fold. A person's presence is never
  moved by the host, so `isSelf`'s own focus and selection stay the client's.

A selection is by key, so it needs no mapping through changes. A key whose element stops being live
leaves it, for everyone, and doesn't come back if the element does.

**Elements by address.** Every `key` and `keys` on these frames, both ways, is the element's
address (`@sld:256/sp:4` on a deck; the key itself for a pack that maps none): what you send to
`setSelection`, `setFocus`, `setRange` and `claim`, and what the roster's `selection`,
`focus`, `range`, `claim_started_elems` and `elem_claims` carry. The kit converts at its
edge, in and out (after v0.15.1 it fills the roster's addresses itself; WIRE.md, Presence
frames), so the client keeps no mapping and compares them as the opaque strings they are. Nothing
changes for a doc whose pack maps nothing: the strings are the keys, as before.

## Cells in a live grid

In a doc whose view schema has a `grid` (a workbook: sheets, rows, cells), a connection shows its
cells: the active cell and up to 16 ranges in one sheet, each corner by address (WIRE.md, "The
live grid"). Numbers and A1 are worked out from the keyed state the client keeps, never read out
of an address.

```ts
const grid = watch.grid!;                                    // KeyedWatch: the view schema's grid
const numbers = gridNumbers(watch.state!, watch.view!.schema, "@sheet:1");
const start = cornerAt(numbers, 4, 3)!;                      // row 4, column C, in normal form
const end = cornerAt(numbers, 15, 4)!;                       // D15
presence.setCells({ container: "@sheet:1", active: start, ranges: [{ start, end }] }, "sheets");
rangeA1(numbers, { start, end });                            // "C4:D15"
const claim = await presence.claimCells({ container: "@sheet:1", ranges: [{ start, end }] }, { note: "updating Q2" });
```

- **Frames go out only when the host takes them:** `cellsSupported` and `cellClaimsSupported` are
  whether `welcome.features` lists `"cells"` and `"cell_claims"`. Otherwise `setCells` records the
  value and `reconnected` sends it (after the focus) if a later connection lists it; `claimCells`
  sends nothing and rejects with `unsupported`.
- **Cells set the focus.** The host moves your focus to the cells' sheet; the channel sets its own
  focus there too, in the `root` you pass (the containers' list root; without it, the current
  focus's root), and sends no `focus` frame for it. **A focus on another sheet** (or none) clears
  your cells here, as the host drops them.
- **Refusals:** the channel refuses itself, unsent, more than 16 ranges (`MPC-E310`) and a claim
  on other than one range (`MPC-E304`). The host refuses a corner that isn't live, isn't in normal
  form or is outside the sheet, an active cell outside the ranges, another sheet's corner
  (`MPC-E114`), and a watcher (`MPC-E305`). A refused value isn't sent again on a reconnect.
- **Kept by keys, dropped, never moved:** an insert inside a range grows it; a range whose corner's
  row or column goes, or whose corner leaves normal form, is dropped (the roster's `cells` says
  what is left). Send new cells then.
- `peers()` gains `cells` (`{container, active, ranges}` or `null`), from the roster's `cells`
  events, entries and devices.
- **Claims on cells** are in `claims()` with the rest: a `ClaimView` with `cells: {container,
  ranges}`, `root` `""` and no `keys`, the actor from its `claim_started_cells` event (from the
  member's row in a snapshot's `cell_claims`). `claim_ended` ends it, its `lines` the range's first
  and last row numbers then. `release(claim)` releases it. Under the host's Respect policy a claim
  holds off other agents' writes to its cells and deletes of its rows or columns; an insert inside
  grows it.
- **An agent's cells follow its work** (the host's; a person's are never moved): a verb that wrote
  cells selects what it wrote; an insert selects the inserted rows or columns whole; a delete puts
  a caret on the first visible cell after it; adding or renaming a sheet moves its focus there (a
  caret at A1 unless its cells were already there); a claim on cells selects the claimed range.
  They arrive as the usual `focus`, `selection` and `cells` events, in that order in a flush.

**The helpers** (`src/grid.ts`, pure functions, no DOM):

| function | gives |
|---|---|
| `gridNumbers(state, schema, container)` | the sheet's keyed rows and columns with their numbers (`GridNumbers`) |
| `blankNumbers(lines, first?)` | the numbers no keyed row has, up to the last one |
| `colLetters(n)`, `colNumber(letters)` | 27 ↔ `AA`, 16384 ↔ `XFD` |
| `colLabel(grid, n)` | a column header by the grid's `col_label` |
| `cornerAt(numbers, row, col)` | the corner, in normal form, of a cell by its numbers (`null` outside the sheet) |
| `cornerNumbers(numbers, corner)` | a corner's row and column numbers (`null` when it isn't one) |
| `cornerProblem(numbers, corner)` | why it isn't one: `bad`, `row`, `col` (not live here), `row_form`, `col_form`, `rows_max`, `cols_max`; `null` when it is |
| `cornerA1`, `rangeA1`, `cellsA1` | A1 (always letters): `E18`, `C4:D15`, `{active, ranges}` |
| `colOfCellField(grid, container, field)` | a cell field's column address (`null` for a row field) |
| `fieldClass(schema, field, of?)` | a field's class: geometry, exact, prefix (`cell`), hidden, meta |

`KeyedWatch.cellDerived("a1", row, field)` gives a formula's text as people read it (`=C4+D4`), from
the snapshot's pages and each change's `derived`.
