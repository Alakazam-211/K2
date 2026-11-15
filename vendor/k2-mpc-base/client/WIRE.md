# The hunk channel's frames

The frames a host carries between `LiveText` (this package) and the kit's session, for one
text root of one doc. The host chooses the transport (a WebSocket, say) and implements
`Transport` (`send`, `onFrame`), or `LiveTransport` when its editor also reconnects (the host
draws its own editor over the library: [HOST.md](HOST.md)); frames are JSON objects with a
`type`. The types are in `src/wire.ts`; this is the spec. The kit's encodings are pinned by
golden files (`test/golden/*.json`, written by `crates/base/tests/wire_golden.rs`; see [JSON
encodings](#json-encodings)).

**Extra fields and error ids.**

- A host may add fields to any frame; a client ignores fields (and frame types, and roster
  event kinds) it doesn't know. Extra fields never carry doc text: the text travels only in the
  fields this spec names.
- A close's `reason` and the [Closing](#closing) table decide whether to retry. A `retry`
  field, if a host sends one, must agree with the table; the client goes by the table.
- Clients key on error ids (`error.id`, `MPC-E…`), never on `next` or on message text: those
  are for people and may change.

## Wire versions

**This spec is wire 3** (the kit's `session::WIRE_VERSION`, the client's `WIRE_VERSION`). A host
says which it speaks in `welcome.wire`; a welcome without it is wire 2.

| Wire | Kits | What it added |
|---|---|---|
| 2 | v0.9.8 to v0.14.0 | the frames below without the wire-3 items (K2 pins its frames to this one) |
| 3 | after v0.14.0 | every addition is a field, a roster event kind, a client frame behind a feature name, or a welcome field: a change's `turn` and `verb`; the roster's `focus` and `range` (event kinds, and fields on entries and devices); the claim events' `actor` and times (`started_ms`; `at_ms` and `at_rev` on `claim_ended`); the client frames `focus` and `range` (features `focus` and `range`); `welcome.wire`; and, after v0.15.1, [the keyed watch](#the-keyed-watch) (feature `keyed_watch`: the client frames `watch` and `report`, the host frames `keyed_snapshot`, `keyed_page`, `ops_change` and `watch_refused`), with its [view deltas](#view-deltas) (`units`, `containers_changed` and `containers` on `ops_change`; `composite` in the snapshot's `schema`); and the roster's elements [by address](#presence-frames), filled by the kit; and [the live grid](#the-live-grid) (features `cells` and `cell_claims`: the client frame `cells` and the claim frame's `cells`; the roster's `cells` and `claim_started_cells`, and `cells` and `cell_claims` on entries and devices; `grid` and `field_class_prefix` in the view schema, the class and unit-delta op `cell`, a rows page's `derived`, and `derived` typed) |

**N−1, both ways.** Nothing in wire 3 changes or removes a wire-2 field, frame or meaning:

- **A wire-2 client on a wire-3 host** keeps working as before. It ignores the fields and roster
  kinds it doesn't know ([the rule above](#the-hunk-channels-frames)); it never sends `focus` or
  `range`, and a host never needs them. It shows no focus or range, groups nothing by turn, and
  names no verb.
- **A client before the keyed watch** (wire 3 up to v0.15.1) never sends `watch`, so a host never
  sends it the watch's frames; one that got them anyway ignores them (frame types it doesn't know).
  **A client with it on an older host** (no `keyed_watch` in `welcome.features`) sends nothing
  and has no keyed state. The keyed watch is additive within wire 3: `WIRE_VERSION` stays 3.
- **View deltas** (after v0.15.1) are fields on `ops_change` and in the snapshot's `schema`.
  **A client before them** ignores the fields; its `view` stays as the snapshot gave it, as
  before. **A client with them on a kit before them** (no `units` or `containers_changed` on a
  change, or no `schema.composite`) keeps the snapshot's view, applies no change to it, and says
  so: `KeyedWatch.viewCurrent` is `false` until the next snapshot. The keyed state is applied the
  same either way.
- **Elements by address in the roster** (after v0.15.1): the kit fills each selection's,
  focus's, range's and element claim's addresses, and the wire carries them as `keys` / `key`.
  With a pack whose addresses are its keys (the default, `Pack::address_of`) they are the same
  strings as before, byte for byte, so **a client of either age sees no change**. With a pack
  that maps them (a deck's `s256-7` is `@sld:256/sp:7`), a client always got addresses where a
  host converted them; a host on a kit before this converted them itself
  (`Session::address_of`), and on this kit sends the kit's.
- **The live grid** (after v0.15.1) is additive within wire 3 too. **A client before it** never
  sends `cells` or a claim with `cells` (it knows neither feature), ignores the roster's `cells`
  and `claim_started_cells` (kinds it doesn't know) and their fields on entries and devices, and
  ends nothing it never started on `claim_ended`; it reads `grid`, `field_class_prefix`, a page's
  `derived` and a unit delta's `cell` op as fields it doesn't know, or as an op with no view
  change. **A client with it on an older host** (no `cells` or `cell_claims` in
  `welcome.features`, no `grid` in the view schema) sends neither, shows no cells, and its grid
  helpers have no grid to read.
- **A wire-3 client on a wire-2 host** (no `welcome.wire`, no `focus` or `range` in
  `welcome.features`): it sends no `focus` or `range` frame (the setters record the value, and
  send it on a later connection whose host lists the feature, as for `presence`); a change event
  without `turn` or `verb` has none (`undefined`, read as `null`); a claim event without `actor` is
  attributed from the roster (`joined`); `started_ms`, `at_ms` and `at_rev` are absent.
- Both are pinned by rows: the kit's goldens (wire 3) read by the client's wire-2 code
  (`tools/n1/n1.sh wire`), and the client's wire-3 code on wire-2 goldens (the same job).

## Connecting

The kit expects nothing first on the wire. Everything before the first frame is the host's:

1. **Authenticate the socket.** How (a header, a cookie, a token checked before the upgrade)
   is the host's choice. The client library never sees the token, and the host keeps it out of its
   logs. The host stamps the kit's `Participant` (pid, name, colour, kind) from the identity
   it resolved; the client never sends one.
2. Open the doc and **join** (`Session::join(participant, Stay::Held)`: a tab never idles
   out while its socket is open).
3. **Subscribe first** (`Session::subscribe`, `Session::subscribe_roster`), **then read**
   (`Session::read`: the root's text and the version). Nothing can fall between the read and
   the stream; the client ignores events at or before the version it starts from.
4. Send **`welcome`**: the first frame on every connection.

```ts
interface Welcome {
  root: string;      // the text root this connection edits ("source" for Markdown)
  version: number;   // the version `text` is at (a resume: the doc's version now)
  text?: string;     // on a first connect, or when resuming was impossible; absent on a resume
  client: number;    // Joined.client of THIS connection: new on every connection, or kept
  lease?: string;    // Joined.lease (kit v0.14.0, a host that reuses writers): 64 bits, decimal
  kept?: boolean;    // Joined.kept: the reconnect took the client's own writer back
  prevEnded?: number; // Joined.prev_ended: a reconnect that didn't keep it: when its lease ended
  self: string;      // this participant as the roster names them (pid; an App viewer's handle)
  rights: "edit" | "read" | "none";
  features?: string[]; // the frame families this host takes (Feature names)
  wire?: number;     // the wire version the host speaks (Wire versions); absent: 2
}
```

- The doc, the root, a resume point and the previous connection's client ID travel in the
  host's connect request. An example route:
  `GET /docs/live?doc=<doc>&root=source&resume=<version>&prev=<client>` (upgraded to a
  WebSocket), whose first server frame is `welcome`. A host whose transport can't carry them
  there takes `root` and `resume` from an optional first client frame, `hello {root, resume?}`.
- `prev` is the client's previous connection: see [Reconnecting](#reconnecting).
- `resume` is the version the client has. The host sends no `text` when it can serve the
  changes since (`Session::events_since` would not refuse); the client then catches up with
  `since`. Otherwise it sends `text` and the client resets to it.
- A doc whose text doesn't fit one frame is refused at `welcome`: see [Frame cap](#frame-cap).

## Units and ordering

- **Offsets are UTF-8 byte offsets** on character boundaries, as in the kit. The client converts
  to UTF-16 only at the edges: the host's editor, and the UTF-16 data in `src/presence.ts`.
- **A hunk** is `{start, end, insert}` on the wire (an object, as the kit's `text::diff::Hunk`):
  replace bytes `start..end` with `insert`. The shared test vectors write hunks as
  `[start, end, insert]` arrays to keep the files small; that's the vectors' format only. A frame's hunks are canonical: sorted, non-overlapping, touching hunks
  joined; an insertion may be followed by a deletion starting at the same point.
- **Versions** are the doc's `Seq`: consecutive; a change touching several roots gives one event
  per root, all with the same version, in any order. The client applies each (root, version)
  once: a root's event at a version it has already seen for another root is applied, a second
  copy of one it has applied is ignored.
- **Order (the host's rule):** one writer per connection sends every frame, in version order:
  a reply for a request that applied at version V (`ack`, `undone`) goes only after every
  `change` before V on that connection. The kit emits `ChangeEvent`s in version order per
  subscriber, and `edit_at_base` returns after its commit.
- **What the client does when that slips** (a lossy link, a reconnect):
  - a `change` past the next version, or an `ack` past it: it asks `since` and applies the
    reply (its own change in the reply settles the edit in flight) before going on;
  - while a `since` is out, `change` frames are dropped (the reply covers them);
  - a `since` sent after an edit settles it: listed, it was applied; not listed, it wasn't (a
    lost refusal), and the change goes to `onRejected` with error id `MPC-E000`;
  - late or duplicate replies to an edit already settled are ignored;
  - `LiveText.resync()` asks again; the host calls it when it suspects lost frames (a lost
    `since` reply included). Client-to-host frames must not be lost or reordered on one
    socket (a WebSocket gives this); across sockets see [Reconnecting](#reconnecting).
  - To recognise its own change in a `since` reply, the client compares hunks, and also the
    event's `client` when it knows its client ID (`LiveOptions.client`, `reconnected`).

## The host's writer: the own-edit filter

The connection's own `edit` is answered by `ack` and must not also arrive as a `change`
(applying it twice doubles the text). The host filters it **by version, never by client ID**:

- On an `edit` that applied at version V (`edit_at_base` returned): drain the connection's
  `ChangeEvent` receiver, **forward every event with version < V**, **skip the event with
  version == V** (on this connection only: every other connection gets it as a `change`),
  then send `ack {op, version: V, hunks}`.
- **Nothing applied** (the edit was already in the text: `EditAtBase::hunks` empty, the version
  unchanged): forward everything queued, **skip nothing**, then `ack` with the version
  unchanged and `hunks: []`.
- **Refused:** forward everything queued, then `refused`.
- **Undo and redo are not filtered.** The participant's undo or redo, made through this very
  connection, has this connection's client ID, and it **does** come back as a `change` (the
  client applies it like anyone's): that is why the filter is by the edit's version and never
  by `client`. The same holds for anything the participant does through another path (an agent
  verb, another tab).

## Client → host

| `type` | Fields | Session call | Reply |
|---|---|---|---|
| `hello` | `root`, `resume?` | (optional; for a host that can't take them in its connect request) | `welcome` |
| `edit` | `op`, `root`, `base`, `hunks`, `group?` | `edit_at_base(conn, root, base, hunks, EditOptions::group(group))` | `ack` or `refused` |
| `since` | `op`, `version` | `events_since(conn, version)` | `events` or `refused` |
| `snapshot` | `op` | `read(conn)`: the root's text and the version | `snapshot`, or `closed` (too large) |
| `cursor` | `root`, `version`, `anchor`, `head` | `set_cursor(conn, root, At::Byte(anchor), At::Byte(head))`, mapped to now first ([Carets](#carets)) | none |
| `undo` / `redo` | `op` | the participant's undo / redo, one step | `change` frames, then `undone`; or `refused` |
| `visible` | `visible` | `set_visible(conn, visible)` | none |
| `view` | `root?`, `first`, `last` | `set_view(conn, LineRange {root, first, last})` (1-based, inclusive; `root` defaults to the connection's) | none |
| `bye` | | `release(conn)` (kit v0.14.0, a host that reuses writers), then close the socket | none |

- `op` is the client's request number; replies carry it back.
- **One edit in flight (rule 0).** The client sends an `edit` only when its previous one was
  answered. `base` is the latest version it has seen, its own acknowledged edits included.
  `hunks` are on that version's text.
- `group` is an undo group token: consecutive edits with one token can merge into one undo
  step; the session closes groups on idle, word boundaries and others' changes.
- A `cursor` is sent only when the client has nothing pending, so its version's text is the
  client's text.
- Frames with no reply (`cursor`, `visible`, `view`) are fire-and-forget: a refusal of one (a
  caret off a boundary, `MPC-E114`; a reader's caret, `MPC-E102`) is dropped by the host,
  silently. The client re-sends its caret after any change that advances its version, its own
  or a remote one, throttled ([Carets](#carets)).

## Presence frames

Selection by element key, pointer and viewport (`PresenceChannel`, [PRESENCE.md](PRESENCE.md)).
They sit outside the doc's numbering: none of them takes or moves a version.

| `type` | Fields | Reply |
|---|---|---|
| `select` | `req`, `selection`: `{root, keys}` or `null` | only when refused |
| `pointer` | `req`, `point`: `{x, y}` or `null` | only when refused |
| `viewport` | `req`, `rect`: `{x, y, w, h}` or `null` | only when refused |
| `focus` (wire 3) | `req`, `focus`: `{root, key}` or `null` | only when refused |
| `range` (wire 3) | `req`, `range`: `{root, key, field, anchor, head}` or `null` | only when refused |
| `cells` (live grid) | `req`, `cells`: `{container, active, ranges}` or `null` ([The live grid](#the-live-grid)) | only when refused |
| `view_address` | `req`, `address`: an address or `null` | only when refused |
| `time_position` | `req`, `time_position`: `{at, state, in?, rate?}` or `null` | only when refused |

| Frame | Session call |
|---|---|
| `select` | `set_selection(conn, selection)` |
| `pointer` | `set_pointer(conn, point)`, from `Point::new(x, y)` |
| `viewport` | `set_viewport(conn, rect)`, from `Rect::new(x, y, w, h)` |
| `focus` | `set_focus(conn, focus)`, from `ElemFocus::new(root, key)` |
| `range` | `set_range(conn, range)`, from `ElemRange::new(root, key, field, anchor, head)` |
| `view_address` | `set_view_address(conn, address)` |
| `time_position` | `set_time_position(conn, time)`, from `TimePosition::new(at, state)`, then `.within(in)` and `.rate(rate)` when given |

- **`focus`**: the container this connection has in view (a
  slide), an element of a MovableList root, live (`MPC-E114` otherwise). Needs read. It leaves
  when its element stops being live.
- **`range`**: a caret or selection inside one element's text field: `field` of element `key`
  of `root`, whose text is the string at `<key>\x1f<field>` in the root's composite map.
  `anchor` and `head` are **UTF-8 bytes** of that text, on character boundaries (`MPC-E114`
  otherwise); a client converts from its UTF-16. Needs edit, as a caret. **Setting a range sets
  the focus** to its element's container (the pack's `container_of`). A range is **never
  moved**: when its field changes (anyone's change) or its element stops being live, the kit
  drops it (`range: null` on the roster), and the client sends a new one.
- **`cells`**: a connection's cells in a live grid, by corner ([The live grid](#the-live-grid)).
  **Setting cells sets the focus** to their container (the kit sends the `focus` event too; the
  client sets its own focus there, sending no `focus` frame). A connection's focus moving to
  another container drops its cells (`cells: null` in the same flush); the client clears its own
  when it sets that focus. Refusals: `MPC-E114` (a corner not live, not in normal form or outside
  the container's rows or columns, an active cell outside the ranges, another container's
  corner), `MPC-E310` (more than 16 ranges: the client refuses it itself, unsent), `MPC-E305` (a
  watcher).
- **`view_address`**: the view this connection is looking at, by address (any address the kit's
  grammar reads, a bare key included; at most 512 bytes, `PRESENCE_ADDRESS_MAX`; `MPC-E114`
  otherwise). The kit doesn't resolve it. Needs read; a watcher is refused (`MPC-E305`). A pack's
  verb sets its agent's (`StructOutput::viewing`), and a host sets one for any connection (an
  agent's, when it renders a still for it). Not to be confused with `view`, a text doc's lines.
- **`time_position`**: this connection's playhead. `at` is a decimal integer string (an optional
  `-`, digits, no leading zero, within 64 bits) in the pack's unit, the view schema's `time_unit`
  (a timeline's ticks): never a float. `state` is `playing` or `paused`; `in` (optional) what it
  is in, by address; `rate` (optional) the playback rate as a decimal ratio, `n` or `n/d` with `d`
  above 0 (`2`, `1/2`, `-1`, `1001/1000`), 1 when absent. The kit stamps `at_ms` (the host clock)
  when it takes it, so a follower extrapolates a playing one from `at_ms` at `rate` (the time
  since `at_ms`, times `rate`, in the pack's unit). Anything else is `MPC-E114`. A pack's verb sets its
  agent's (`StructOutput::time_position`). Needs read; a watcher is refused (`MPC-E305`).
- **Elements by address.** On the wire an element is named by its address (`@sld:256`,
  `@sld:256/sp:4`), the pack's `Pack::address_of` of its key (by default the key itself, so a
  pack that maps nothing sees no change). The host converts at the kit's edge: the setters by
  address (`set_selection_at`, `set_focus_at`, `set_range_at`, `claim_elems_at`) take what the
  client sent and return what they set by address; `Session::resolve_address(conn, address)`
  and `Session::address_of(root, key)` convert either way. **Out, the kit converts** (after
  v0.15.1): every selection, focus, range and element claim it hands out (the roster's rows and
  devices, `Snapshot`, the `selection`, `focus`, `range` and `claim_started_elems` events, the
  setters' replies, `ClaimInfo`) carries its elements' addresses beside the keys
  (`ElemSelection::addresses`, `ElemFocus::address`, `ElemRange::address`,
  `ClaimInfo::addresses`, `RosterEvent::ClaimStartedElems::addresses`), and the host sends those
  as the wire's `keys` / `key`; it calls no `address_of` per frame. They are the keys themselves
  for a pack that maps nothing, so the wire is unchanged there. Two values are equal by their
  keys (the addresses follow from them). The keyed watch's frames carry addresses already ([The
  keyed watch](#the-keyed-watch)).

- `keys` are element keys of `root`, a MovableList root: at most 256 (`MPC-E310`), each live
  (`MPC-E114`, which also covers a root that isn't a list). No keys means clear. Coordinates are
  finite numbers in the doc's own space, which the kit doesn't interpret; `w` and `h` aren't
  negative (`MPC-E114`).
- `select`, `pointer`, `viewport` and `focus` need read; `range` needs edit. A reader may send
  the first four; **a watcher is refused all five** (`MPC-E305`), and has no roster row.
- A tier 0 doc (`BlobSession`) has none of the three calls, so a host lists no `"presence"` on a
  connection to one.
- The host answers a refused one with `presence_refused {req, error}` and changes nothing. It may
  cross a deletion: a client that selects an element just as it goes gets `MPC-E114`, and the
  selection stays as it was.
- The client sends the pointer and the viewport at most once a frame, the latest value. The kit
  keeps the latest and reports it at the next roster flush.
- A connection's values end with the connection: a client sends them again after a reconnect.
- **Outside the doc's numbering.** These frames and their roster kinds carry no `seq`, no
  `version` and no `base`, take no number from the doc and write nothing to its log. Nothing that
  counts down frames by `seq` counts them.

**Request numbers: `req`, never `op`.** These frames carry no `op`, and their refusal is not a
`refused` frame:

- `req` is the channel's own counter, independent of `LiveText`'s `op`s: the same number on both
  means nothing. Later frames of this channel number the same way.
- **The two sides of a shared transport can't reach each other.** `LiveText` reads `op` replies
  and ignores `presence_refused`, as any frame type it doesn't know. `PresenceChannel` reads
  `presence_refused` and ignores `refused`, so an edit's refusal never reaches it.
- **Why not `op`:** a host that doesn't know a frame type may refuse it as unreadable
  (`MPC-E113`), by the frame's `op` when it has one. An `op` on these frames would reach
  `LiveText` as the refusal of whichever edit has that number. Without one, that host answers
  with an `error` frame, which refuses nothing.
- **Sending:** a client sends these frames only on a connection whose `welcome.features` lists
  `"presence"` ([Feature names](#feature-names)), so an older host sees none of them. Otherwise
  the channel records its values and sends them if a later connection's `welcome` lists it.

### Feature names

`welcome.features` is an optional list of strings: the frame families this host takes.

- A client sends a family's frames only when the host lists its name. No field, an empty list
  and a list of only other names all mean the same: none of that family is sent.
- A client ignores names it doesn't know. A host may list names this spec doesn't.
- The name is checked per connection: a reconnect's `welcome` replaces the last one's list.

| Name | Frames |
|---|---|
| `presence` | `select`, `pointer`, `viewport` |
| `claims` | `claim`, `release` |
| `focus` (wire 3) | `focus` |
| `range` (wire 3) | `range` |
| `keyed_watch` (wire 3, after v0.15.1) | `watch`, `report` ([The keyed watch](#the-keyed-watch)) |
| `cells` (wire 3, after v0.15.1) | `cells` ([The live grid](#the-live-grid)) |
| `cell_claims` (wire 3, after v0.15.1) | `claim` with `cells` ([Claim frames](#claim-frames)) |
| `view_address` (wire 3, after v0.18.0) | `view_address` |
| `time_position` (wire 3, after v0.18.0) | `time_position` |

## Claim frames

Claims on elements of a list root (`Session::claim_elems`). They sit outside the doc's numbering:
none of them takes or moves a version. They are the host's to route: the kit has the calls and
this spec, no socket.

| `type` | Fields | The kit's call | Reply |
|---|---|---|---|
| `claim` | `req`, `root`, `keys`, `ttl_ms?`, `note?`, `hold?` | `claim_elems(conn, root, keys, ttl_ms, note, hold)` | `claimed {req, claim}`, or `presence_refused {req, error}` |
| `release` | `req`, `claim?` (none: all of this participant's here) | `release_claim(conn, claim)` | `released {req, claims}`, or `presence_refused {req, error}` |

- **Refusals:** `MPC-E301` `claimed` (exit 5: `details` has `claim`, `by`, `name`, `root`, `keys`,
  `remaining_ms`, `note`), `MPC-E303` `claim_contended`, `MPC-E304` `bad_claim` (no keys, more than
  256, a `note` over 1024 bytes, a table doc, a zero TTL), `MPC-E114` `bad_position` (a key unknown
  or deleted, a root that isn't a list), `MPC-E903` and `MPC-E904` (the caps). Nothing changes.
- **`hold`:** without it a claim ends on its owner's first edit touching it (`how: "edited"`). A
  client making several edits under one claim (a drag) sets it, and releases when done. A held
  claim still runs out: `ttl_ms` is clamped to the host's most (`Config::claim_max_ms`).
- **An edit a claim refuses** (`MPC-E301`, under `ClaimPolicy::Respect`, agents only) is refused
  before anything applies: it takes no version and moves nothing.
- **Who sees it:** everyone, by the roster kinds `claim_started_elems` and `claim_ended`; a later
  subscriber by the snapshot entries' `elem_claims` ([The roster](#the-roster)).
- **A claim whose elements went** ends `how: "deleted"` when a live change removed the last of
  them, and `how: "gone_outside_save"` (after v0.18.0) when an **outside save** did: the file
  changed on disk and its apply took the elements out (an app deleted them, or a pack's pairing
  gave a renamed element a new key, so the old key went). Its `claim_ended` carries `keys`, the
  claim's elements by address as they were when it was taken, so a view can say what went;
  `keys` is absent for a claim on lines or cells (and from an older kit).
- **A reason a client doesn't know** is an ended claim like any other: the client drops the claim
  and reads the reason as `deleted` (`claimEndReason` in `src/presence.ts`). No feature name
  gates `gone_outside_save`: a v0.18.0 client folds `claim_ended` by its `claim` alone, so it
  drops the claim as before (`test/n1-wire.test.ts`). The client hands each end on with
  `PresenceChannel.onClaimEnded` (`ClaimEnded {claim, who, how, keys}`).
- **A claim doesn't outlive the connection that took it,** held or not: it ends `how: "left"` when
  that connection closes. A new connection that names the old one (`join_replacing`) closes it at
  once; one that doesn't leaves the claim live until the old one closes or its time runs out.
  Unlike a selection, a client doesn't send a claim again after a reconnect: it claims again.
- **Request numbers: `req`, never `op`,** from the same counter as the [presence frames](#presence-frames),
  and for the same reason: a host that doesn't know a frame type may refuse it by the frame's `op`,
  which would reach `LiveText` as the refusal of whichever in-flight edit has that number. Replies
  route by frame type.
- **Sending:** a client sends these frames only on a connection whose `welcome.features` lists
  `"claims"` ([Feature names](#feature-names)), so an older host sees none of them.
- **A claim on cells** (a live grid; feature `cell_claims`, sent only when listed): the same
  `claim` frame with `cells: {container, ranges: [range]}` in place of `root` and `keys` (and
  `note`, `hold`, `ttl_ms` as above). One range: more is `MPC-E304`, which the client refuses
  itself, unsent. The reply is `claimed {req, claim}`; `release` releases it as any claim. Everyone
  sees it by the roster's `claim_started_cells` and `claim_ended`, a later subscriber by a
  snapshot entry's `cell_claims` ([The roster](#the-roster)). Under the host's Respect policy it
  holds off other **agents** (`MPC-E301`): a write to a cell inside it, a delete of a row or
  column inside it. An insert inside it grows it.


## Host → client

| `type` | Fields | When |
|---|---|---|
| `welcome` | `root`, `version`, `text?`, `client`, `self`, `rights` | first, on every connection ([Connecting](#connecting)) |
| `ack` | `op`, `version`, `hunks` | an `edit` applied: `hunks` as applied (`EditAtBase::hunks`, on the text at `version - 1`), now at `version`. Nothing applied (already in the text): `hunks: []`, `version` unchanged |
| `refused` | `op`, `error` | an `edit`, `since`, `snapshot`, `undo` or `redo` refused; nothing applied |
| `change` | `event` | every applied change by anyone, in order, **except** the client's own `edit` (answered by `ack`); its undo and redo do come as `change` |
| `events` | `op`, `events` | the reply to `since`: every change after that version, in order |
| `snapshot` | `op`, `text`, `version` | the reply to `snapshot`: the root's text at `version` |
| `undone` | `op`, `steps`, `version`, `notice?` | the reply to `undo` / `redo`, after the `change` frames it made ([Undo and redo](#undo-and-redo)) |
| `roster_event` | `event` | one roster change as the kit pushes it (`subscribe_roster`): the client folds them |
| `roster` | `entries` | the whole roster at once, for a host that folds the deltas itself |
| `claimed` | `req`, `claim` | the reply to `claim` ([Claim frames](#claim-frames)) |
| `released` | `req`, `claims` | the reply to `release` ([Claim frames](#claim-frames)) |
| `presence_refused` | `req`, `error` | a presence or claim frame refused; nothing changed ([Presence frames](#presence-frames), [Claim frames](#claim-frames)) |
| `error` | `error` | a client frame the host couldn't take that has no `op` to answer (over the cap, unreadable) |
| `closed` | `reason`, `code?`, `path?`, `error?` | the last frame; then the host closes the socket ([Closing](#closing)) |
| `keyed_snapshot`, `keyed_page`, `ops_change`, `watch_refused` | see [The keyed watch](#the-keyed-watch) | the keyed watch's frames, only on a connection whose host lists `keyed_watch` |

- `event` is the kit's `ChangeEvent`: `{version, pid, client, via, kind, root, hunks, at_ms,
  on_behalf_of}` (`pid` and `on_behalf_of` are opaque handles for App viewers; `kind`
  since kit v0.10.1).
- `error` is the kit's `ErrorReport` JSON (`ErrorReport::to_json`): `{id, exit, message,
  next, next_step, rule, details}`. `next_step` (kit v0.10.1) is `next` as data,
  `{verb, args, why}`: `verb` a kit verb, `args` argv tokens (`{key}`: the refusal's
  `details.key`), `why` prose; it has no host syntax, so a host renders it in its own. It is
  absent when there is nothing to run, and from an older host. The ids the client acts on:
  - `MPC-E111` (`write_conflict`): the edit met competing text. The client takes the change out
    of its text and hands it to `onRejected` (the typed text, the hunks, the base, the region);
    `details.conflicts` is passed along.
  - `MPC-E112` (`base_expired`): `base` (or `since`'s version) is past the live window
    (1,000 changes, 4 MiB of events by default), or the changes since don't fit one frame. The
    client hands everything pending to `onRejected` and emits `onReload`; the host (its editor) calls `LiveText.reload()`, which sends `snapshot` and resets to the reply.
  - `MPC-E113` (`bad_edit`): an `edit` frame over the cap ([Frame cap](#frame-cap)): as
    `MPC-E111`. The client makes this refusal itself before sending, so a host sees it only
    from another client.
  - any other refusal of an `edit` (`MPC-E151`, `MPC-E901`, ...): as `MPC-E111`.
  - a refused `undo`, `redo` or `snapshot`, and every `error` frame: `onError`.

## Undo and redo

- `undo {op}` / `redo {op}` undo or redo one step of **this participant's** history (the kit's
  undo is per participant, across their connections).
- Applied: the change arrives as a `change` (not filtered, above), then **`undone {op, steps:
  1, version}`** with the doc's version after it.
- Nothing to undo or redo (the kit's `UndoOutcome::Nothing`, `HistoryLost`, `ClearedByPurge`):
  **`undone {op, steps: 0, version}`**, the version unchanged, no `change`. For `HistoryLost` and
  `ClearedByPurge` the host adds `notice`: the report of `MPC-E206 history_lost` or `MPC-E207
  cleared_by_purge` (exit 0: a notice, not a refusal). A UI learns "nothing to undo" from
  `steps === 0` (`LiveText.onUndo`).
- Conflicts (`UndoOutcome::Conflicts`): `refused {op, error}` with `MPC-E201 undo_conflict`
  (`details.conflicts`); `onError`. The live wire offers neither `--partial` nor `--drop`.
- `LiveText.undo()` / `redo()` return the op; a reply that never comes (the socket dropped) is
  forgotten on `reconnected`.
- The server undoes the top of the participant's stack when the request arrives, so
  `LiveText` sends an `undo` or `redo` in order with the edits: behind every edit typed before
  it (queued edits first, without waiting for their acks), never composed with one typed after
  it. With nothing queued it goes at once. Held while disconnected, it goes on the new
  connection in the same place.
- An undo's or redo's `change` says `kind: "undo"` / `"redo"` (kit v0.10.1). An undo still
  unanswered when a connection drops can land late under that connection's client ID, as can
  an edit whose reply was lost: the client tells them apart by `kind` (`HOST.md`,
  presence first). From an older kit (no `kind`) it can't, and says "check and retype".

## Carets

- The client sends `cursor {root, version, anchor, head}`: bytes of the root's text **at
  `version`**, sent only when nothing is pending, so `version` is its latest.
- **The kit's `set_cursor` takes no version**: it resolves `At::Byte` offsets against the
  root's **current** text (`crates/base/src/session/live.rs:1557-1575`: `text_of`, then
  `At::resolve`, which checks only that the offset is a character boundary,
  `session/position.rs:153-161`). Once set, the kit carries a caret itself through every later
  change (`live.rs:3401-3405`, `Bias::After` for anchor and head).
- **So the host maps a caret from an older version before setting it.** When the frame's
  `version` is behind the doc's: take `events_since(conn, version)` (`session/live/at_base.rs:196`),
  and for each event on the frame's root, in order, move `anchor` and `head` with
  `position::transform(pos, &event.hunks, Bias::After)` (`session/position.rs:32`), the bias
  the kit carries carets with. Then `set_cursor` with the result. If `events_since` refuses
  (past the window) or the frame's version is ahead of the doc's, drop the frame. Dropping
  any stale caret instead of mapping it is also correct (it only shows the caret a change
  late): the client re-sends its caret after its next change.
- **The client re-sends its caret after any change that advances its version**, its own or a
  remote one (a change to another root included), so a frame the host dropped as behind is
  replaced at the new version even when the person is idle:
  - after its own edit is acknowledged, at once (as before);
  - after remote changes, at most once per 250 ms per connection (`LiveOptions.caretResendMs`),
    with a trailing send so the last version always goes out;
  - never while an edit of its own is in flight or queued (its ack sends it);
  - at the caret's place in that version's text: the client moves the caret through every
    change that isn't the person's own typing as CodeMirror moves a selection (an empty caret
    stays before text inserted at it; a range doesn't grow), so an editor echoing the moved
    caret back sends no second frame;
  - deduplicated by `version:anchor:head`.
- A refused caret (`MPC-E114` off a boundary, `MPC-E102` for a reader) gets no reply.
- **Since kit v0.10.1 the kit does this mapping:** a host passes the frame through as
  `set_cursor_at(conn, root, version, anchor, head)` (`session/live/at_base.rs`). It admits as
  `set_cursor` does (a reader is refused), maps the offsets through the changes since `version`
  on that root as above, and sets the caret, under one lock (no change lands between).
  `CursorAtVersion::Set(cursor)` is the caret as set; `CursorAtVersion::Dropped(_)` (`Ahead`:
  the frame's version is ahead of the doc's; `PastWindow`: the changes since can't be read)
  means nothing was set and the host sends **no reply**, as for a frame it drops today. The
  mapping by hand above stays correct for a host on an older kit.
- Carets coming the other way carry their version: [JSON encodings](#json-encodings).
- **Per-connection carets (kit v0.10.1, additive).** The same person on two devices or in two
  tabs sees their other connection's caret, as one login's other tabs show in Google Docs.
  The kit's roster rows stay one per participant; on a text doc each row gains `devices` (one
  `{client, cursor, label?}` a connection, in join order), and each `cursor` event gains
  `client`, the connection whose caret moved ([The roster](#the-roster)). `carets()` then
  returns one caret a connection, another connection of this participant's own with
  `sameUser: true`, and never this connection's own (`welcome.client`); with no `devices` (an
  older host) it returns one caret a participant and skips its own, as before. The library
  draws nothing and has no text for these: the host shows them.
- **A connection's label** is the host's, given at join (`Session::join_with` with
  `JoinOptions::label`: "another tab", "Chrome on Mac"); the kit cleans it (control characters
  out, trimmed, at most 64 characters) and carries it on `Device.label` (the wire's `label`,
  left out when there is none). It is never doc text.
- **A connection's label arrives with the next roster snapshot** (`cursor` events carry none).

## Reconnecting

- **A reconnect is a new connection:** the host joins again, so the connection has a **new
  client ID** (`welcome.client`); the pid is the same (an App viewer's handles too, while the
  doc stays loaded). The host calls `leave` for the old connection when its socket goes.
- **The old connection goes first (the host's rule).** A reconnect carries `prev=<client ID>`,
  the client ID of the client's previous connection (the last `welcome.client`; the host's editor
  sends it on every reconnect, `connect({resume, prev})`). Before it sends the new
  connection's `welcome`, or answers any `since` on it, the host marks that old connection
  dead and calls `leave(conn)` for it; every frame still arriving on the old socket is
  dropped unapplied, and the host closes that socket. Without this, an edit still in flight
  on a half-open old socket can land after the new connection's `since` reply, which didn't
  list it: the client reports `MPC-E000` for an edit that was applied.
  - The host acts on `prev` only when the old connection is the **same login's**, on the
    **same doc and root**, on **this host**. Otherwise it **ignores `prev` silently**: no
    error, no close, the same reply. A guessed client ID can't end anyone's connection or tell
    whether one exists.
  - Without `prev`, a second connection from the same login is a second tab: both stay live.
  - A half-open connection that no `prev` names ends at the host's keepalive timeout ([Keepalive
    and waking up](#keepalive-and-waking-up)).
  - **Since kit v0.10.1 the kit does the leave:** the host calls
    `join_replacing(participant, stay, prev)` for the new connection instead of `join`, and the
    kit closes the connection holding client ID `prev`, as `leave` does, in the same lock as
    the join (`Joined.replaced` names it; the pid stays present, no `left`). The kit acts only
    when that connection is the **same pid's** on this doc, and otherwise ignores `prev`
    silently (`replaced: null`, that connection untouched). The host still checks the rest of
    the rule above itself (the same login, the same root, this host) before passing `prev`,
    still drops frames arriving on the old socket and closes it. On an older kit the host
    calls `leave(conn)` itself.
  - Because a host may get this wrong, the client keeps a record for an `MPC-E000` after a
    reconnect (`Rejection.asOf`), recognises the late change by its client ID if it lands
    (`onLanded`), and a host's Re-apply places it only when `presence` shows it absent
    (`HOST.md`).
- **Writer leases (kit v0.14.0, `Config::reuse_person_writers`).** A host that reuses a person's
  writer per device joins with a device key (`JoinOptions::device`) only for a client that says
  it can (a connect parameter of the host's choosing, say `writer=lease`); every other client
  gets a fresh writer per connection, as before. The device key is not a security boundary: it
  only chooses among the authenticated person's own free writers, so a host may take it from the
  client (a browser's random id) and scopes it to the person by the pid it stamps.
  - The welcome carries `lease` (`Joined.lease`). The client sends it back with `prev` on its
    next reconnect (`connect({resume, prev, lease})`, an example route
    `…&prev=<client>&lease=<lease>`), and the host passes it as `JoinOptions::reclaim`.
  - When that lease is still the writer's, the kit gives the client its writer back (`kept`):
    a connection still holding it is closed first, under the session lock (`Joined.replaced`;
    the host marks that socket dead and closes it, as for `prev`). Nothing of the previous
    connection lands after the welcome, so the `since` reply settles the edit in flight for good.
  - Otherwise the kit closes nothing (`replaced: null`) and joins as without it; `prevEnded`
    says the version the previous lease ended at, when known: changes under the old writer after
    it are another connection's (the client caps its lost-change watches there).
  - **With a device key, `prev` alone closes nothing**, in the kit and in the host: the writer
    may be a sibling tab's by now. A host that retires a previous connection itself matches the
    lease too (client and token), never the client ID alone.
  - **`bye`**: the client sends it when the page goes and it holds nothing unsent; the host
    calls `Session::release(conn)` and closes the socket, and the writer is free for the
    person's next connection on that device. Any other end (a dropped socket, a timeout, a
    close) parks the writer for the client's own reclaim.
- **Frames sent while disconnected are dropped, never replayed** on the next socket: the wire
  has no idempotency key, so a replayed `edit` could apply twice. `Transport.send` drops
  while no socket is open; it never buffers.
- **Edits made while disconnected wait, unsent:** the host's editor calls
  `LiveText.disconnected()` when the connection goes (`onClose`); `LiveText` then keeps new
  edits in its queue instead of handing them to the transport, and `reconnected` sends them on
  the new connection (with its client ID), after the `since`. An edit handed over before the
  close stays in flight, and the `since` settles it.
- The host's editor then calls **`LiveText.reconnected(welcome)`**:
  - the client ID becomes `welcome.client`; the edit in flight is still recognised under the
    ID it was sent with (`isOwn` takes either);
  - the caret (its dedupe cleared), `visible` and `view` are sent again;
  - with `text` (resuming was impossible): `reset` to it; everything pending goes to
    `onRejected`;
  - without: `resync()`, whose reply settles the edit in flight: **listed, it was applied**
    (acknowledged); **not listed, it wasn't** (`onRejected` with `MPC-E000`, its text kept for
    `reapply`). Queued edits are sent after it. A `snapshot` still out is asked again.
- A `since` past the window gets `MPC-E112`: `onReload`, then `reload()` (a `snapshot`).

## Closing

`closed {reason, code?, path?, error?}` is the host's last frame; the host then closes the
socket. The transport reports it, or a dropped socket, through `LiveTransport.onClose({reason,
retry, error?})`.

- `code` (optional) refines the reason with a host-defined name, for the host's own UI and
  logs; it never changes the retry column. For example, `closed` with `code: "core_off"` for a
  host's switch that turns live editing off for one kind of file.
- `path` (optional, with `moved`): the doc's new path, when the host can say it.

| `reason` | From | Retry |
|---|---|---|
| `left` | this connection left (`close()`) | no |
| `idle` | the kit's `LeaveReason::Idle` | no |
| `kicked` | `LeaveReason::Kicked` | no |
| `rights_lost` | `LeaveReason::RightsLost`: can't read the doc any more | no |
| `deleted` | the doc was deleted | no |
| `moved` | the doc was renamed (`rekey`); open it at its new key (`path`, when sent) | no |
| `released` | the doc moved to another host for good: open it there | no |
| `cut_off` | the connection's change queue overflowed (`Config::subscriber_queue`, 1,024 events): reconnect and resume | yes |
| `declined` | live editing is off for the doc: its pack declined it, or its text doesn't fit one frame (`error`: `MPC-E401`) | no |
| `not_found` | no such doc | no |
| `auth` | the socket's authentication failed or expired | no (until signed in again) |
| `network` | the socket dropped (the transport's own; never in a frame) | yes |
| `closed` | the host closed the doc's live session (`SessionManager::close`, v0.9.7.2) | no |

- `canRetry(reason)` (`src/wire.ts`) gives the table's last column.
- **A host restart or hand-off that sends no frame is `network`** (retry): the socket just
  drops, and the reconnect finds the doc again (on the same host, or where the host's routing
  sends it). `released` is only for a doc that moved to another host for good.
- **An oversized frame:** a host may hard-close the connection with `cut_off` instead of
  answering a frame over the cap ([Frame cap](#frame-cap)). That is safe because the client
  never replays a frame: it reconnects, resyncs with `since`, and reports what didn't land.
- A rights downgrade that keeps read arrives as a `rights` roster event for `self`, not a
  close: the host's editor goes read-only.

## Keepalive and waking up

- **No app-level ping.** The host pings at the socket level (WebSocket ping frames, every
  20 s or so) and closes a connection whose subscriber lags (`closed {reason: "cut_off"}`).
- **The host's editor wakes and resyncs** when the page regains focus, comes back `online`, or
  becomes visible: if the socket is gone it reconnects; if not, it calls `resync()` (a
  laptop lid can stall a socket that never reported closing).
- `visible {visible}` tells the kit whether the tab is in front (`set_visible`): a participant
  whose every connection has been hidden for `away_after_ms` shows as away.

## Frame cap

**4 MiB** (`FRAME_MAX`, 4,194,304 bytes of UTF-8 JSON) in both directions. Neither side sends
a frame over it, and the host refuses one cleanly:

- **An `edit` over the cap:** `refused {op, error}` with **`MPC-E113 bad_edit`**,
  `details: {bytes, frame_max}`. `MPC-E113` says the edit as a whole is malformed and must not be
  retried as is ("bad edit: {reason}"); `MPC-E151 bad_hunk` is about one hunk's ranges (order,
  bounds, boundaries) and tells the host to recompute them, which won't help.
- **Any other frame over the cap:** `error {error}` with `MPC-E113`; the frame is dropped.
- **Or a hard close:** instead of reading an oversized frame, a host may close the connection
  with `closed {reason: "cut_off"}` (or just drop the socket). Safe: the client never replays
  a frame; it reconnects, resyncs, and reports an edit that didn't land as `MPC-E000`.
- **The client never sends one:** an edit whose frame would pass `FRAME_MAX - FRAME_MARGIN`
  (64 KiB left for its `ack`) is refused by the client itself, before sending, as `MPC-E113`
  with `details: {bytes, limit, frame_max}`: it leaves the text and goes to `onRejected`, its
  text kept. (Pasting more than about 4 MiB at once is therefore refused.)
- **A frame the host can't send whole:**
  - a `change`: not sent. The client sees the version gap and asks `since`;
  - `events` (a `since` reply): `refused {op}` with `MPC-E112`, `details.limit: "frame"`: the
    client reloads;
  - `welcome` with `text`, or `snapshot`: the doc is too big for live editing over this host.
    The host sends `closed {reason: "declined", error}` with **`MPC-E401 declined_too_large`**
    (`details: {limit: "frame", bytes, frame_max}`) and closes; no retry. The host shows the
    file the ordinary way (read-only, or opened outside the live editor). Nothing is chunked:
    one frame per message keeps the client's state machine whole.
  - a `roster_event` snapshot carrying waiting notes: the host drops notes from the frame,
    oldest first, until it fits. A push doesn't consume a note, so the trimmed ones still
    reach the person through the verb that delivers them.
- **What fits** (measured by `test/wire.test.ts`, "sizes", run record `client_frame_sizes`;
  JSON as `JSON.stringify` writes it, escapes included):

  | Frame | JSON bytes | Of the cap |
  |---|---|---|
  | `welcome` with a 1 MiB Markdown note (quotes, tabs, backslashes, `é`, `—`) | 1,157,413 | 28% |
  | `change` replacing that whole note | 1,157,470 | 28% |
  | `roster_event` snapshot: 32 members with carets, and 64 waiting notes of 16 KiB (1 MiB of note text, the per-doc bound; each note's text also inside its message) | 2,448,703 | 58% |
  | `welcome` with 1 MiB of control characters (the worst escape: 6 bytes each) | 6,291,572 | 150%: declined |

  The worst case doesn't fit. A host that sets its Markdown pack's size limit to
  1 MiB keeps every ordinary doc inside the cap, and the rules above handle the rest.

## The roster

- **A roster entry** is `{handle, name, colour, kind, role, away, cursor, rights}` (+ `self`
  when the host marks it, + `devices` on a text doc from kit v0.10.1); `cursor` is `{root,
  anchor, head, version, anchor_lc, head_lc}` or `null`, the participant's most active
  connection's caret. `self` marks this connection's own participant (or the client compares
  `handle` with `LiveOptions.self`, i.e. `welcome.self`).
- **`devices`** (kit v0.10.1, additive: absent from an older host, on a grid, and for tier 0):
  one `{client, cursor, label?}` per connection of the participant, in join order: `client`
  its client ID (`welcome.client` on that connection), `cursor` its own caret or `null`,
  `label` the host's label for it when there is one. A client knows its own connection by
  `client` and hides only that one.
- **A roster event** mirrors the kit's `RosterEvent`, tagged by `kind`: `snapshot {version,
  members, watching, overwritten?}` (always first; a later one replaces everything;
  `overwritten` only when this subscriber has waiting notes, [below](#the-overwritten-event)), `joined {who,
  participant: {name, colour, kind, via}, rights}`, `left {who, reason}`, `rights {who,
  rights}`, `role {who, role}`, `away {who, away}`, `cursor {who, cursor, version, client?}`,
  `selection {who, selection, client?}`, `pointer {who, pointer, client?}`, `viewport {who,
  viewport, client?}`, `focus {who, focus, client?}` and `range {who, range, client?}` (wire 3)
  ([below](#selection-pointer-and-viewport-in-the-roster)),
  `cells {who, cells, client?}` (a live grid, [below](#the-live-grid)),
  `claim_started {who, claim, lines, note, hold, for_edit, expires_ms, version, actor,
  started_ms}`, `claim_started_elems {who, claim, root, keys, note, hold, expires_ms, version,
  actor, started_ms}`, `claim_started_cells {who, claim, container, ranges, note, hold,
  expires_ms, version, actor, started_ms}` (a live grid), `claim_ended {who, claim, how, lines,
  version, actor, at_ms, at_rev, keys?}`
  (`actor`, `started_ms`, `at_ms` and `at_rev`: wire 3, [below](#claim-events-wire-3); `keys`:
  a claim on elements, its elements by address as they were, after v0.18.0),
  `note {to, from, note, lines, claim}`,
  `overwritten` ([below](#the-overwritten-event)), `watching {count}`, `closed {reason}` (the
  host closed the doc's live session, `SessionManager::close`: the last event, then the
  subscription ends; `reason` is the host's code). Claims, notes, `overwritten` and rights reach
  the host through `onRosterEvent`; the rest also update `roster`.
- **Notes of kind `note` are passed through untyped.** The kit's heads-up notes grow by
  release: for a `note`, the client types only `to` and `from` and hands the rest to
  `onRosterEvent` as it came. The removed-text notes an outside save leaves (`removed_text` /
  `replaced_text` / `rollup`) are not `note`s: they are the `overwritten` event, typed in full
  ([below](#the-overwritten-event)), and so are their waiting copies in a snapshot's
  `overwritten`.
- **Members' order isn't significant.** The kit lists editors first, then viewers, then away
  (`Session::roster`); a client that folds the events may keep join order. Compare rosters as a
  set keyed by `handle`.
- **`cursor`'s `client`** (kit v0.10.1, additive): the connection whose caret this is. The kit
  sends one event per connection whose caret changed (a connection that left with a caret:
  `cursor: null`), so a client keeps carets per `(who, client)`. **A client that ignores
  `client`** and keeps one caret a participant (the last event's) stays right: within a flush
  a participant's events end with the one whose caret is the participant's (`entry.cursor`),
  and when the last per-connection event isn't that caret (a connection left, or dropped its
  caret, while another still has one) the kit sends one more `cursor` **without `client`**
  carrying the participant's caret. A participant with one connection gets the same events as
  before, each with its `client`.
- **A caret's `version`:** the doc version its offsets are in. The kit gives it beside the
  caret (`RosterEvent::Cursor.version`; a snapshot's carets are at the snapshot's `version`);
  the host puts it inside the caret object too. Without one, the client takes its own version
  when the frame arrives. The client maps it to its text through the changes between.

### Claim events (wire 3)

The claim events K2's rendering data asks for are the roster's own claim kinds, with
the actor and the times added, so a view draws a claim and words it without a roster lookup,
even after the claimant left:

- **`actor`** `{id, name, kind, colour}` on every claim event: `id` is the event's `who`; the
  name, kind and colour are the claimant's when they took it (`colour` exactly as the host
  stamped it on the participant).
- **`started_ms`** on `claim_started` and `claim_started_elems`: when it was taken (host clock);
  also on a snapshot's `elem_claims`.
- **`at_ms`** on `claim_ended`: when it ended; **`at_rev`**: the change (the doc's version) that
  ended it when one did (`how: "edited"`, `"deleted"` or `"gone_outside_save"`), else `null`.
- **An agent's claim on elements moves its presence:** at the same flush, after the claim's
  start, its `focus` goes to the claim's first container in document order and its
  `selection` is **replaced** by the claimed keys (in the order it gave them). A person's claim
  leaves their presence alone. Later calls override both.
- A claim's `scope` and `container` follow from the pack's view schema
  (`Pack::view_schema`: which list root holds the containers, which the units); a claim's keys
  go out as addresses, as every element on the wire: the kit fills them
  (`RosterEvent::ClaimStartedElems::addresses`, a snapshot's `ClaimInfo::addresses`) and the host
  sends them as `keys` ([Presence frames](#presence-frames)).
- **A claim on cells** (`claim_started_cells`) always carries `actor` and `started_ms`. It ends
  with `claim_ended`, whose `lines` is `{root, first, last}`: the rows root and the range's first
  and last row **numbers** at that version. A snapshot entry lists its live ones in `cell_claims:
  [{claim, container, ranges, note, hold, expires_ms, started_ms}]`, present only when there are
  some.

### Selection, pointer and viewport in the roster

- A roster entry and a `devices` element gain `selection`, `pointer` and `viewport`, and (wire 3)
  `focus`, `range` and, in a live grid, `cells`, each left out when there is none. The entry's
  are the most active connection's that has one; a `devices` element's are that connection's own.
- Within a flush, a participant's `focus` comes before their `selection`, then `cells`, `range`,
  `pointer` and `viewport`: a view moves to the container before it draws what is selected there.
- A `range` is `null` after its field changed or its element went: the kit dropped it. A `focus`
  is `null` after its element went.
- A `selection`, `pointer`, `viewport`, `focus` or `range` event carries the value or `null`
  (gone). The kit sends
  one per connection per flush, the last value, by the rule for `cursor`: the connection's own
  with its `client`; when the last of those isn't the participant's value (a connection left, or
  cleared, while another still has one), one more with no `client` carrying the participant's.
- A grid has no `devices`: its events carry no `client`, and there is one value a participant.
- An element that stops being live leaves every selection (the event gives what is left; `null`
  when nothing is). Bringing it back doesn't select it again.

## JSON encodings

The kit has no serde: a host encodes these by hand. The golden files pin them, written from
a real session by `crates/base/tests/wire_golden.rs` (the kit's own JSON writer, as
`ErrorReport::to_json`; `MPC_BLESS=1` regenerates) and read by `test/golden.test.ts`, which
parses them with these types, replays the changes to the kit's text and folds the roster to
the kit's own final roster.

| Value | Encoding |
|---|---|
| `ChangeEvent` | `{version, pid, client, via, kind, root, hunks: [{start, end, insert}], at_ms, on_behalf_of, turn, verb}`; `on_behalf_of` is `null` when absent, never left out; `kind` (kit v0.10.1) = `ChangeKind::as_str()`: `"edit"` \| `"undo"` \| `"redo"` \| `"revert"` \| `"restore"` \| `"external"` \| `"import"`, always present (a host on an older kit sends none); `turn` (wire 3) = `ChangeEvent::turn` as its string, the host-given turn of the agent step that made it (at most 64 bytes, printable ASCII), or `null`: a person's change, an outside save, a call the host gave none, and always for an App viewer; never left out (a host before wire 3 sends none); `verb` (wire 3, kit after v0.14.0) = `ChangeEvent::verb`, the id of the pack verb that made it as the pack names it (`shape move`, `replace-section`; at most 64 bytes), or `null`: a raw write, a kit tool, an outside save, an undo, a redo, a revert, a restore and the import; shown to an App viewer too (it names a verb, not a person); never left out (an older host sends none). `via` stays the participant's client, never the verb |
| `client` | the 53-bit client ID as a JSON number (`MAX_CLIENT_ID` = 2^53 − 1 = `Number.MAX_SAFE_INTEGER`, exact in JS) |
| strings | JSON escapes: `"` `\` and control characters escaped (`\n`, `\t`, `\r`, else `\u00XX`); everything else as UTF-8 |
| `who`, `pid`, `handle`, `on_behalf_of` | `Who`'s `Display`: the pid for a person's or agent's connection; for an App viewer's (`ParticipantKind::App`), the opaque per-doc handle (`p1`, `p2`...), its own included. One lens per connection, as the kit applies it; the client treats them as opaque and compares them with `welcome.self` |
| `rights` | `"edit"` when the kit's `Rights` has `read` and `edit`; `"read"` with `read` only; `"none"` otherwise. `history` and `owner` aren't sent |
| `role` | `"editor"` / `"viewer"` (`Role`) |
| `kind` | `ParticipantKind` in lower case: `person`, `agent`, `app`, `external`, `restore`, `import` |
| `reason` (`left`) | `LeaveReason`: `left`, `idle`, `kicked`, `rights_lost` |
| `how` (`claim_ended`) | `ClaimEnd::as_str`: `released`, `edited`, `expired`, `left`, `reset`, `deleted`, `gone_outside_save` (after v0.18.0; a client reads a reason it doesn't know as `deleted`) |
| `keys` (`claim_ended`) | `RosterEvent::ClaimEnded::addresses`, only when not empty: a claim on elements (or a blob doc's nodes), its elements by address as they were when it was taken, in key order |
| `note` (`note`) | `NoteKind::as_str` |
| `claim` | `claim-N` (`ClaimId`'s `Display`) |
| `lines` | `{root, first, last}`, 1-based, inclusive. For a claim on elements (`claim_ended`), the list root and the live positions of its first and last live elements (`1, 1` with none live), for display only |
| a caret (`CursorAt`) | `{root, anchor, head, version, anchor_lc, head_lc}`: `anchor`/`head` bytes at `version`; `anchor_lc`/`head_lc` the kit's `LineCol` as `"line:col"`, both 1-based, the column counted in characters (Unicode scalar values), for display only |
| `RosterMember` → entry | `handle` = `who`; `name`, `colour`, `kind` from `entry.participant`; `role`, `away` from `entry`; `cursor` = `entry.cursor` at the snapshot's version, or `null`; `rights` = `rights`; then `devices` (v0.10.1) only when `entry.devices` isn't empty: one `{client, cursor, label}` per `Device`, `cursor` at the snapshot's version or `null`, `label` left out when `None`; `selection`, `pointer` and `viewport`, on the entry and on each `Device`, left out when `None`; then `elem_claims` only when the member holds claims on elements (`ClaimSpan::Elems` in `entry.claims`): one `{claim, root, keys, note, hold, expires_ms, started_ms}` a claim (`started_ms` = `ClaimInfo::taken_ms`, wire 3) |
| `ClaimActor` (wire 3) | `actor` = `{id, name, kind, colour}`: `id` = the event's `who` (`Who`'s `Display`), `kind` as `ParticipantKind` in lower case |
| claim times (wire 3) | `started_ms`, `at_ms` as JSON numbers (host clock); `at_rev` a JSON number or `null` |
| `RosterEvent::Cursor` → `cursor` | `{kind: "cursor", who, cursor, version}`, then `client` (v0.10.1) only when the event's `client` is `Some`: the connection's client ID |
| `ElemSelection` | `{root, keys}`: `keys` as `Key`'s text, in the order given, no repeats |
| `Point`, `Rect` | `{x, y}`, `{x, y, w, h}`, as JSON numbers (`Point::x()` and so on), never `-0`. Coordinates may have fractions. The golden files hold whole numbers only; parity cases and the client's tests cover fractions, and a host encodes them with its own writer or the kit's (`errors::Value::float`) |
| `RosterEvent::Selection`, `Pointer`, `Viewport` | `{kind: "selection" \| "pointer" \| "viewport", who, <kind>: value or null}`, then `client` only when the event's `client` is `Some` |
| `ElemFocus` (wire 3) | `{root, key}`: `key` as `Key`'s text |
| `ElemRange` (wire 3) | `{root, key, field, anchor, head}`: `anchor` and `head` as JSON numbers, UTF-8 bytes of the field's text |
| `RosterEvent::Focus`, `Range` (wire 3) | `{kind: "focus" \| "range", who, <kind>: value or null}`, then `client` only when the event's `client` is `Some` |
| `TimePosition` (after v0.18.0) | `{at, state, at_ms}`, then `in` and `rate` only when it has them: `at` and `rate` strings exactly as set, `state` `PlayState::as_str` (`playing`, `paused`), `at_ms` a JSON number (host clock) |
| `RosterEvent::ViewAddress`, `TimePosition` (after v0.18.0) | `{kind: "view_address" \| "time_position", who, <kind>: value or null}` (the address a string), then `client` only when the event's `client` is `Some`. A roster entry and a device carry `view_address` and `time_position` only when they have one |
| `ErrorReport` | exactly `ErrorReport::to_json()`: `{id, exit, message, next, next_step, rule, details}`, `next_step` = `{verb, args, why}`, absent when there is nothing to run (and before kit v0.10.1). Clients key on `id`; `message`, `next` and `next_step` are for people (a host renders `next_step` in its own syntax) |
| `RosterEvent::Overwritten {to, overwrite}` → `overwritten` | `{kind: "overwritten", to, change, version, restore_from, source, rule, row, line, cells, more, not_shown, withheld, no_history, message}`, every field always present: `to` = `to` (`Who`'s `Display`, the subscriber's own, since the kit sends it only to its author's subscriptions); `change`, `source`, `rule`, `more`, `not_shown`, `withheld`, `no_history` = the `Overwrite`'s fields of those names; `version`, `restore_from`, `row`, `line` = the `Overwrite`'s `Option`s, `null` when `None`; `cells` (below); `message` = the `Overwrite`'s `Display` (`to_string()`), which holds the person's text |
| `cells` | in order: first `{region, col, yours, file, cut}` from the `Overwrite`'s own top-level `region`, `col`, `yours`, `file`, `cut` (its first place), then one `{region, col, yours, file, cut}` per entry of `also` (an `OverwriteCell`, same fields), in `also`'s order. `col` is `null` when `None`. A roll-up (`rule: "rollup"`, `Overwrite::is_rollup`) has no place: `cells: []` (its `region` is empty; don't send it) |
| `Snapshot.overwritten` | a bare `Vec<Overwrite>`, this subscriber's own waiting notes, with no `to`. The host encodes each entry as the live `overwritten` event above, adding `kind: "overwritten"` and `to` = the subscriber's own `who` (its pid, or for an App viewer its handle, as `welcome.self`), so every entry has the live event's shape. The field `overwritten` is present only when the vector isn't empty: a missing field means none |

## The live transport (for a host's editor that reconnects)

```ts
interface LiveTransport extends Transport {
  connect(opts: { resume?: number; prev?: number; signal: AbortSignal }): Promise<Welcome>; // one attempt (ConnectOptions)
  onClose(cb: (e: { reason: CloseReason; retry: boolean; error?: ErrorReport }) => void): () => void;
  close(): void; // the tab closed: the host calls the kit's `leave`
}
```

- **One stable object** for the replica's life: `LiveText` subscribes to `onFrame` once; the
  transport swaps sockets inside it.
- `connect` is one attempt (open, authenticate, join, `welcome`); it rejects with a close event
  when refused (`declined`, `not_found`, `auth`...). The host's editor owns the retry loop:
  back off, `connect({resume: live.version, prev})`, then `live.reconnected(welcome)`
  (`HOST.md`).
- **`connect()` while a connection is open replaces it:** the host closes the old socket (and
  leaves its connection: frames in flight on it are lost) and the attempt goes on as any
  other; no `onClose` is reported for the old one. A host's editor can rely on it when a
  `snapshot` is refused or not answered: it connects again without `resume`, and the welcome's
  `text` starts it again.
- `send` **drops** while not open ([Reconnecting](#reconnecting)).
- **Authentication refused before the upgrade:** a browser's WebSocket API reports a refused
  upgrade (say, an HTTP 401) as a plain failed dial, which reads as `network`. After a failed
  dial, a transport may make one cheap HTTP pre-check of the same route to tell `auth` (no
  retry) from `network`. Only after a failed dial, never on a timer.
- **No local undo history in the host's editor:** the server owns undo ([Undo and
  redo](#undo-and-redo)); a local history would undo outside the shared order. Map the undo and
  redo keys to `LiveText.undo()` / `redo()` (`HOST.md`). (`MPC-EDITOR-HISTORY` was the kit's
  own editor component's refusal of a CodeMirror history, until v0.10.2.)
- A `closed` frame is the transport's: it reports it through `onClose` (a host using the plain
  `Transport` may pass it on; `LiveText.onClosed` then reports it).

## The `overwritten` event

A change overrode someone's live work. Two kinds, one per doc kind (a doc only makes its own):

- **A text doc:** an outside save may have removed or replaced text you wrote through the kit.
  It carries the person's own text so they can paste it back: no single call restores the text
  and keeps the saver's change.
- **A grid:** a change overrode a live value you wrote: an outside save, a stale write, a
  revert, a restore or `rows drop-deleted`.

It goes only to its author's own subscriptions (on a text doc, also the person an assistant
acted for), at the next roster flush (at most `roster_interval_ms` after the change), and is
never coalesced. Every field is always present, on both kinds.

```ts
{ kind: "overwritten", to: Handle,
  change: number,               // the change that did it (a text doc: the save)
  version: number | null,       // a text doc: the version holding the text, THAT THIS READER CAN OPEN: link from this only
                                // a grid: the version holding the live value (show --version); null for a drop,
                                // whose note carries the value itself (a drop can't be undone)
  restore_from: number | null,  // a text doc: the same version, to tell someone with history access; never a link
                                // for this reader. null on a grid
  source: string,               // a text doc: "an outside save"
                                // a grid: "an outside save" | "a write" | "a revert" | "a restore" | "rows drop-deleted"
  rule: string,                 // a text doc: "removed_text" | "replaced_text" | "rollup"
                                // a grid: "cell" | "edited_row" | "deleted_row" | "moved_row" | "dropped_row" | "dropped_col" | "rollup" | ...
  row: string | null,           // a grid: the row's key now (it may differ from the key the author saw); null on a text doc
  line: number | null,          // a text doc: first line, in the text before the save
                                // a grid: its line in the live file before the change: show this, not the key
  cells: [ { region: string,    // a text doc: "L12" or "L12–L14"; a grid: the element by key
             col: number | null,// null on a text doc
             yours: string,     // a text doc: this person's own removed characters, exactly, newlines included
                                // a grid: the live value
             file: string,      // a text doc: those lines now; a grid: the file's value. At most 256 bytes
             cut: boolean } ],
  more: number,                 // places (a text doc) or cells (a grid) not listed
  not_shown: number,            // a text doc: characters of their own text the note doesn't carry; 0 on a grid
  withheld: boolean,            // the values are no longer held: yours and file are empty, the fact stays
                                // (a purge; on a text doc also the 24 h hold, the doc's cap)
  no_history: boolean,          // a text doc: this reader has no history right; false on a grid
  message: string }             // the note's one line, ready to show; holds the text or values
```

- **"May have" (text docs).** The kit can't tell a removal from a move, so a text note's
  `message` never says the text is gone. See `docs/SESSIONS.md`, "When an outside save removes
  someone's text".
- **Only their own text (text docs).** `yours` never holds someone else's characters from the
  same block.
- **One note per row, per author (grids).** `cells` lists that row's overridden cells.
- **Sizes.** `file` (and on a grid `yours`) is at most 256 bytes, cut at a character boundary,
  with `cut: true`; on a grid, `version` serves the full value. `cells` has at most 32 entries,
  the rest counted in `more` (`OVERWRITE_VALUE_MAX`, `OVERWRITE_CELLS_MAX`); so one grid event is
  at most about 20 KiB. A text note carries at most 16 KiB of text (`OVERWRITE_TEXT_MAX`), the
  rest counted in `not_shown`.
- **Roll-up.** A participant has at most 64 waiting notes on a doc; past that the last is
  `rule: "rollup"`, `row: null`, `cells: []`, `more: N`. It carries no text or values (on a grid,
  `message: "N more cells overwritten, see the log"`). A note is never dropped silently.
- **Pending notes are also in `snapshot`,** as `overwritten: [...]`, this subscriber's own. A push
  doesn't consume a note: it waits until `take_notes` or a verb delivers it, so a slow receiver
  that gets a fresh `snapshot` still finds it. Clients dedupe on `(change, row)`; on a text doc a
  save makes one note per person. `overwritten` is present only when this subscriber has waiting
  notes; treat a missing field as none.
- **Someone who can't read the doc gets nothing, not even a count:** no event, no snapshot entry,
  no note. The note stays and arrives if they can read the doc again.
- **Where they wait.** A grid's notes are saved in the store and live as long as the revert
  window (`Config::revert_keep_ms`, 30 days by default); past it, one is gone undelivered (the
  audit entry and the version remain). A text doc's are in memory: an unload keeps them, a
  restart doesn't.
- **Logs.** The kit's `Debug` doesn't print `yours`, `file` or `message`'s text; don't log
  `message` yourself. The audit entry holds no content.

## The keyed watch

What a person watching a live keyed doc that isn't a table (a deck, a board) needs: its keyed state,
then every change as it lands. Data only: a view (the host's) draws from it. The kit's side is
`Session::watch_keyed`, `Session::watch_snapshot` and `KeyedWatch` (`session/live/watch.rs`); the
client's is `KeyedWatch` (`src/keyed.ts`). A text doc has the hunk channel instead, and a table
its own frames: `Session::keyed_watchable` says which docs have it.

**The gate.** A host lists `"keyed_watch"` in `welcome.features` only for a doc that has it. A
client sends `watch` and `report` only when it is listed; a host that doesn't list it never sends
the frames below (the kit gives it no watch: `watch_keyed` with options built from that
connection's features returns none).

**Client → host** (no `op`, no `req`; no reply but the frames below):

| `type` | Fields | The kit's call |
|---|---|---|
| `watch` | | the first frame on a connection that lists the feature, and again after a gap: open the watch if there is none (`watch_keyed`), then send `watch_snapshot`'s frames |
| `report` | `seq` | the last numbered frame the client applied; after each whole snapshot and every 64 changes (`REPORT_EVERY`). A host may ignore it on this engine |

**Host → client, numbered.** Every frame of the watch takes the next `seq` on this connection, from
1: a snapshot's head, each of its pages, each change. Presence, roster and claim frames are never
numbered and never counted. A client that sees a `seq` other than the next one (a lost frame)
asks again (`watch`) and drops every frame until a snapshot's head; a frame that doesn't fit its
state (an op on what isn't there, a change at or before its version) is treated the same way.
Nothing is guessed.

| `type` | Fields | What |
|---|---|---|
| `keyed_snapshot` | `seq`, `upto`, `schema {name, roots: [{name, kind}], composite: [{map, list}]}`, `view`, `pages` | the doc at version `upto` follows in `pages` pages; `kind` is `text`, `list` or `map`; `composite` the map roots whose keys are a list's elements' fields (`Pack::composite`; after v0.15.1). `view`: the view index's snapshot at `upto` when the pack declares a view schema (`ViewSnapshot::to_value`: the view schema, `tier`, `rev`, `page`, `fonts`, `layouts`, `containers`), else `null`. Always taken, whatever came before: it replaces the client's state |
| `keyed_page` | `seq`, `upto`, `page` (from 1), `pages`, `root`, and one of `live`, `hidden`, `entries`, `text`, `units`; `derived?` | the keyed state's runs, one root each, in schema order: `live` `[[address, value], ...]` (elements in order, appended), `hidden` (deleted elements), `entries` `[[key, field, value], ...]` (map entries, split as ops are), `text` (appended); then the view index's units, container by container: `units` `{container, from, units, next, total}` (`ViewIndex::units_page`; `root` is the units root). `derived`: on a page of a live grid's rows map, the pack's values kept by cell field for that page's entries ([The live grid](#the-live-grid)); absent when there are none |
| `ops_change` | `seq`, `rev`, `at_ms`, `change_kind`, `actor {id, name, kind, colour}`, `client`, `via`, `on_behalf_of`, `turn`, `verb`, `claim`, `ops`, then for a pack with a view schema `units`, `containers_changed` and `containers` ([View deltas](#view-deltas)), then `derived`, `render_hashes` | one committed change, now at version `rev` |
| `watch_refused` | `error` | unnumbered: the host can't serve the watch (`MPC-E401`, below) |
| `verb_refused` | `at_ms`, `actor {id, name, kind, colour}`, `client`, `turn`, `verb`, `unit`, `code`, `holder`, `claim` | unnumbered: a verb on the doc was refused and applied nothing ([Refused verbs](#refused-verbs)) |
| `outside_save` | `at_ms`, `doc`, `outcome`, and by outcome `rev`, `kept`, `copy`, `code`, `message` | unnumbered: what came of an outside save of the doc's file ([Outside saves](#outside-saves)) |

- **The snapshot is the fallback for everything:** a client's gap, a reconnect (numbering starts
  again at 1 on every connection), a watch that fell behind (its queue of
  `Config::subscriber_queue` changes filled), and a change whose frame would pass the frame cap
  (it isn't sent; the next frames are a snapshot). `KeyedWatch::needs_snapshot` tells the host
  when. A change already in a snapshot (at or before its `upto`) is never sent after it.
- **Pages** are at most the host's page cap of JSON (`WatchOptions::page_bytes`, 1 MiB by default,
  at least 1 KiB), under the 4 MiB frame; a `units` page is at most the larger of the cap and the
  view index's least page budget (`VIEW_PAGE_MIN`, 16 KiB), and holds at least one unit. The head
  holds the view index's snapshot whole (the view schema and every container), under the frame
  cap. An entry bigger than a page has a page of its own; one
  whose page would pass the frame cap refuses the snapshot, `MPC-E401` (`declined_too_large`), sent
  as `watch_refused`. A text root is split at character boundaries.
- **A catch-up** (the changes since the client's version, when they fit the cap) would answer a
  `watch` before the snapshot it replaces; it isn't built, so every `watch` gets a snapshot.

- **What the snapshot reads:** the live doc's keyed state and the view index built from it at
  the same version (`Session::watch_state`; the index as `Session::view_index` gives it). The
  keyed state is there because the change frames are ops on it; the view index's `view` and
  `units` are as of the snapshot, and each change frame's [view deltas](#view-deltas) keep them
  (the client's `KeyedView`); what only the pack works out rides on the frame's `derived`.
- **Elements by address** everywhere in these frames: a list element is its address
  (`Pack::address_of`), a composite map key is its element's address, `\x1f`, the field; a plain
  map's keys are as they are.

**A change frame's fields:**

- `rev`: the doc's version after it (`ChangeEvent.version`); one frame per change, every root it
  touched in it. `change_kind`: `ChangeKind::as_str` (`edit`, `undo`, `redo`, `revert`, `restore`,
  `external`, `import`).
- `actor`: who made it, so a view words it without the roster, even after they left: `id` the pid
  (an App viewer gets a handle), `name`, `kind` (`person`, `agent`, `app`; **`file`** for an outside
  save, whose `id` is `kit:external`) and `colour` as the host stamped it (`null` for the file: the
  host picks its own).
- `client`, `via`, `on_behalf_of`, `turn`, `verb`: as on `ChangeEvent` (`turn` is `null` for an App
  viewer).
- `claim`: the actor's own claim on what it touched (an element it changed, or that element's
  container), `claim-N`, else `null`.
- `ops`: every root's, in apply order; ops with no effect are left out, a `drop` is carried:
  - text: `{root, kind: "text", hunks: [{start, end, insert}]}`, UTF-8 bytes of the root's text
    before;
  - list: `{root, kind: "list", op, key, index_before, index_after, before?, after?, effect}`, `key`
    the element's address: `op`
    one of `insert`, `move`, `delete`, `restore`, `set`, `drop`; the indexes are positions in the
    live order just before and just after it (`null`: not shown); `effect` `visible`, or `hidden`
    for a write to a deleted element (its value, or its place);
  - map: `{root, kind: "map", op: "set" | "delete", key, field, before?, after?}`: on a root the pack
    declares composite, `key` is the element's address and `field` the rest of the map key
    (`<key>\x1f<field>`); otherwise `key` is the whole map key and `field` is `null`.
  - `before` and `after` are values (a string, or `null` for none) and are left out when the op has
    none (before an insert, after a map delete).
- `derived` and `render_hashes`: the pack's (`Pack::watch_extras`): its derived values (`null`
  when empty; typed below) and `{container: hash}` for the containers whose render hash changed
  (`null` when none).
  For a pack with a view schema, both are read from the view index before and after the change
  (`ViewChange`), built through the index the kit keeps for reads; when the pack gives no
  `render_hashes`, the kit's are the containers whose index render hash changed.

**Applying them** (the client's `KeyedWatch`, and the kit's own live view): a list op's element is
at `index_before` before it and at `index_after` after it; a delete moves the element and its value
to the deleted set, a restore back, a `drop` takes it out of the deleted set. The client's state
compared with the kit's is canonical JSON (`watch_state_json` / `keyedStateJson`): `{"roots":[...]}`,
each `{name, kind, ...}` in schema order, elements by address, a list's `live` in order and
`hidden` by address, a map's `entries` by key (a composite one `<address>\x1f<field>`) in UTF-8
byte order, a text whole; no spaces; strings escaped as the kit's writer
does: `"`, `\`, `\n`, `\r`, `\t`, then every other control character and U+2028, U+2029 as
`\u00xx` (lower case), everything else as UTF-8.

**JSON.** Each frame is exactly `WatchFrame::to_json()`. Goldens: `test/golden/keyed_snapshot.json`
(a snapshot in pages of the smallest cap, and under `view` a deck's with the view index's snapshot
and units) and `test/golden/keyed_frames.json` (a scripted session's
frames after its snapshot), written by `crates/base/tests/keyed_watch.rs`; the seeded traces in
`test/vectors/keyed_watch.json` replay in `test/keyed-watch.test.ts` to the kit's own final state,
byte for byte.

### Refused verbs

A pack's structured edit verb (`Session::run_tool`) that a refusal stopped, whatever refused it
(another's claim, a stale `--if-version`, a size limit, rights, its arguments, or the verb's own
"no" with its id), is one `verb_refused` frame (`WatchFrame::Refused`, `WatchRefusal`), so a view
can say who couldn't do what and why. It changes nothing.

- **Unnumbered:** no `seq`, never counted; no snapshot holds it, and a lost one is never asked
  for again. It goes out only after the watch's first snapshot, and a watch whose queue is full
  misses it (the watch stays open).
- **Who sees it:** the watches whose connection may read the doc when the verb is refused (the
  kit asks the host's rights then, so a watcher whose rights went since it opened gets none),
  the same viewers as the change frames. An App viewer gets `actor.id` and `holder.id` as
  handles and `turn` `null`, as on a change frame.
- **The fields:**
  - `actor`, `client`, `turn`: who was refused, as a change frame's.
  - `verb`: the verb's name (`shape move`).
  - `unit`: the unit it addressed, by address: the one the verb names (a pack sets it,
    `StructOutput::addressing`), else the first element its edits touched under the claim that
    refused it, else the first element its edits touched; `null` when it was refused before it
    named one.
  - `code`: the refusal's id (`MPC-E301` for a claim; `ErrorReport::id` of what the call
    returned).
  - `holder`: `{id, name, kind, colour}` of who holds the claim or lease that refused it, else
    `null`; `claim`: that claim's id, `claim-N`, else `null`.

### Outside saves

When the kit handles an outside save of the doc's file (another program saved it), one
`outside_save` frame (`WatchFrame::OutsideSave`, `WatchOutsideSave`) says what came of it, so a host
forwards it as it is. The save's change, when it made one, comes first as its own `ops_change`
(actor `kit:external`), unchanged.

- **Unnumbered**, as `verb_refused`: no `seq`, never counted, after the watch's first snapshot,
  and only to the watches whose connection may read the doc then.
- **One a save**: one frame for each save the kit takes in; `declined` and `missing` once, when
  the file stops decoding or goes, not on every check after. Nothing for a check that finds the
  file unchanged or holding the kit's own write.
- **Every frame**: `at_ms` (the host clock when the kit handled it), `doc` (the doc's address,
  `DocKey::path`), `outcome`:
  - `ingested`: taken in by key; `rev` the doc's version after it (`DiskCheck::Ingested`).
  - `kept_both`: the save is the doc at `rev`; what was live before is kept as the stored version
    `kept` and as the conflict copy `copy {name, path, doc?}` (`DiskCheck::Conflict`): `name` its
    file name, `path` relative to the doc's file's folder as the kit records it, `doc` its doc
    address when the copy is itself a kit document (the host's `Host::document_at`); `copy` is
    `null` when it couldn't be written.
  - `declined`: the file can't be a live doc any more (`DiskCheck::Declined`): `code` the pack's
    (`too_large`, `not_utf8`, `malformed`...) and `message` in plain words.
  - `missing`: the file is gone (`DiskCheck::Missing`).
- The client: `OutsideSaveFrame`, `KeyedWatch.onOutsideSave`. An older client ignores the frame.

### View deltas

What a change did to the view, so a watcher keeps its view index without running pack code
(K2's ask). The kit works them out from the change's ops and the
pack's view schema (`view_delta::view_deltas`), generically; a pack with no view schema gets none
of the three fields (the frame is as before, byte for byte).

- **`containers_changed`**: `[{key, op, position_before, position_after, hidden_after, fields}]`.
  One per op on the containers' list, in apply order: `add` (insert), `delete`, `restore`,
  `reorder` (move), `props` (a new list value), with the container's 1-based positions in the
  live order just before and just after the op (`null`: not shown). Then one per container whose
  fields changed: `hide_show` when a visibility field did (the schema's `field_class`, or its
  hidden field), with `hidden_after`; else `props`; `fields` names them (never values).
  Positions and `hidden_after` are `null` where they don't apply. A container's fields set by the
  change that adds it fold into its `add`; one that shifted only because another came or went
  isn't listed: the order says where it is.
- **`units`**: `[{key, container, ops, geom_before, geom_after, fields, hidden_after,
  placeholder_type}]`, one per unit whose place in the view changed, in the order the change
  first touched them. A unit is **shown** when it is live and its container field names a live
  container. One that comes into view gets `add` (inserted by this change) or `restore`; one
  that leaves it `delete`, also when it comes or goes because its container did (or its
  container's id changed). A shown unit that stays gets one op per kind of change, in this
  order: `reorder` (moved in the units' list), `move_resize`, `rotate` and `flip` (the geometry
  field compared part by part: `x`, `y`, `w`, `h`; `rot`; `fh`, `fv`), `text`, `style`,
  `hide_show` (with `hidden_after`), `props` (a meta field, a new list value, or a new
  container), `cell` (a field of class `cell`: a row's cell value in a live grid). A field's class
  is its `field_class` entry, else the class of the first `field_class_prefix` rule it starts
  with, else `visibility` for the hidden field and `meta` otherwise (the geometry field is always
  the geometry; `fieldClass` in `src/grid.ts`). `fields`: the fields that changed (on an `add` or a `restore`, every field the
  change set). `container`: its container's address (before a `delete`, after anything else).
  `geom_before` / `geom_after`: the geometry field's own string, `null` when the geometry wasn't
  touched, before an add, after a delete, and for an empty one (a box from a layout). A field
  of class `geometry` that isn't the geometry slot's (a scene's 3D transform, with no 2D slot)
  is a `move_resize` naming the field, with both `null`.
  `placeholder_type`: the type part of the schema's placeholder field, or `null`.
- **`containers`**: the render hint: every container a delta names, then those in
  `render_hashes`, each once.
- Ops with no effect and writes to deleted elements give nothing. Several changes committed by
  one call (an undo of a group) are worked out one by one, each against the doc as it was right
  after it.

**Applying them** (the client's `KeyedView`, after the frame's ops are in the keyed state):
containers in order, by their positions (a delta that doesn't fit is an inconsistent frame: a
fresh snapshot); a container's and a unit's fields are read again from the keyed state for the
fields named, by the view schema; the geometry from `geom_after`; a unit that comes into view is
built from the keyed state in its `container`; each container a delta touched lists its shown
units again in the units' list order (`z` from 0). A view field the pack derives (the schema's
`derived`) keeps the snapshot's value. The client's view then equals a fresh snapshot's view and
units, which `test/view-deltas.test.ts` checks against the kit's vectors
(`test/vectors/view_deltas.json`, written by `crates/base/tests/view_deltas.rs`, which also
checks every change's deltas against the view index before and after it).

### Derived values

A change frame's `derived` is the pack's, `null` when empty, in parts the kit gives packs
builders for (`FrameDerived` in `src/wire.ts`); a pack may add other keys, which a client
passes over:

```ts
interface FrameDerived { units?: Record<string, UnitDerived>; containers?: Record<string, ContainerDerived>;
  written?: Written; bands?: Band[]; [k: string]: unknown }
interface UnitDerived { a1?: Record<string, string | null>; [k: string]: unknown }      // by unit address, then field
interface ContainerDerived { merges?: CellRange[]; styles?: Record<string, StyleHint>; [k: string]: unknown }
interface StyleHint { bold: boolean; fill: string | null; numfmt: string }               // by style id
interface Written { container: string; ranges: CellRange[] }                             // the cells a verb wrote
interface Band { container: string; axis: "rows" | "cols"; op: "insert" | "delete";
  from: number; to: number; numbers: "before" | "after" }
```

- `units.<unit>.a1.<field>`: a field's value as people read it in A1 terms (a formula's text,
  `=C4+D4`); `null` when it has none any more.
- `bands`: an insert's `from`..`to` are row (column) numbers after the change (`numbers:
  "after"`), a delete's are numbers before it (`"before"`).
- **Kept by cell field.** The names a grid lists in `cell_derived` (`["a1"]`) are kept by the
  client per row and field: a snapshot's rows pages carry them for their entries (`derived: {a1:
  {<row address>: {<field>: <text>}}}`), and each change's `units.<row>.<name>` updates them, a
  `null` removing one. `KeyedWatch.cellDerived(name, row, field)` reads them; a fresh snapshot
  replaces them all.

### Derived values per unit

Small texts only the pack works out for each unit (a scene object's transform, a label as
people read it), so a view shows them with no pack code. Additive within wire 3: `WIRE_VERSION`
stays 3, and a client that doesn't know them reads every frame as before.

- **The names**: the view schema's `unit_derived` (`["transform","label"]`), at most 8, each 1
  to 64 bytes of ASCII letters, digits, `_`, `-` or `.`; absent when the pack declares none (and
  from an older kit). A name is never also one of the grid's `cell_derived`.
- **On the units pages**: each unit carries its own as `derived` (`{"transform":"loc 1,0,2",
  "label":"Cube"}`), each text at most 1,024 bytes (`VIEW_TEXT_MAX`); a unit with none has no
  `derived`.
- **On a change frame**: `derived.units.<unit address>.<name>` is the text the change set, or
  `null` when the unit has none any more, for exactly the values the change set or removed
  (none for a unit it deleted); `derived` stays `null` when the change moved no value. They sit
  beside the pack's own keys (`a1`), which the kit never overwrites. A unit restored or added
  gets all its values.
- **The client**: `KeyedWatch.unitDerived(name, unit)` reads them (`null` for none), and
  `KeyedWatch.unit(address)` the unit, with them in `derived`. A fresh snapshot replaces them
  all. A unit nested under another (a group's child) the pack adds after the snapshot isn't
  shown until the next snapshot, as before; its values come with it.

### Derived values per container

The container half of [Derived values per unit](#derived-values-per-unit), under the same rules
(a timeline track's label and kind). Additive within wire 3.

- **The names**: the view schema's `container_derived` (`["label","kind"]`), at most 8, each 1 to
  64 bytes of ASCII letters, digits, `_`, `-` or `.`; absent when the pack declares none (and
  from an older kit).
- **On the snapshot's containers**: each carries its own as `derived` (`{"label":"Drums",
  "kind":"audio"}`), in the schema's order, each text at most 1,024 bytes; a container with none
  has no `derived`.
- **On a change frame**: `derived.containers.<container address>.<name>` is the text the change
  set, or `null` when the container has none any more, for exactly the values the change set or
  removed; beside the pack's own keys there (a sheet's `merges`, `styles`), which the kit never
  overwrites. A container added or restored gets all its values.
- **The client**: `KeyedWatch.containerDerived(name, container)` reads them (`null` for none), and
  each `ViewContainer.derived`.

### Nesting

Which unit kinds nest inside which (a collection's items, a group's shapes), so a view can draw
the hierarchy. Additive within wire 3: a client that doesn't know it reads every frame as before.

- **The relation**: the view schema's `nesting` (`ViewSchema::nesting`, `Nesting`):
  `[{"kind":"box","parent":"group"}, ...]` in the pack's order, each `kind` and `parent` a unit
  kind as units carry it (`kind`), 1 to 64 bytes, at most 16 (`NESTING_MAX`), no pair twice;
  absent when the pack declares none (and from an older kit), so such a schema is the same bytes
  as before.
- **Which unit is in which** stays on the unit: its `parent`, by address (`null`: right under its
  container). The relation is a declaration only: the kit builds the index as before.
- **The client**: `ViewSchemaJson.nesting` (`NestingJson`).

### Containers in containers

A container inside another (a scene's collections in collections, a document's sections), so an
outliner draws the containers' tree from the view alone. Additive within wire 3: a client that
doesn't know it reads every frame as before.

- **On a container** (the snapshot's `containers`): `parent`, the address of the container it sits
  inside; absent at the top level (and from an older kit). Always another container of the same
  view, never in a cycle, at most 32 levels deep (`CONTAINER_DEPTH_MAX`; the top level is 1): the
  kit checks it when it builds the view and leaves out a parent that fails. `position` stays the
  flat order: draw the tree by `parent`, each level in `position` order.
- **On a change frame**: a container delta's `parent_after`, the parent's address after the
  change (`null`: at the top level now), on `add` and `restore` always and on any other delta
  when the parent changed. A container whose parent changed with no op of its own (a parent the
  pack derives from another root) gets a `props` delta naming no field, with `parent_after`.
  Absent: unchanged.
- **The schema**: `container_fields.parent` names where the parent comes from (as the other
  view fields); absent when the pack declares none. A client never reads a parent from the
  keyed state: the store holds the parent's key, the view its address, and only the kit maps one
  to the other.
- **The client**: `ViewContainer.parent`, `ContainerDelta.parent_after`, `CONTAINER_DEPTH_MAX`;
  `KeyedView` keeps them. Tests: `test/container-parent.test.ts` on the kit's golden
  (`test/golden/keyed_frames_container_parent.json`, written by
  `crates/base/tests/view_containers.rs`).

### Placements in 3D

A unit's (and a container's) place in a 3D scene, in the file's own form, for a pack whose view
schema has a 3D slot (`docs/design/view-3d-slot-r1.1.md` in the kit). Additive within wire 3:
`WIRE_VERSION` stays 3, and a client that doesn't know it reads every frame as before.

- **The slot**: the view schema's `space` (`{unit_fields, container_fields, precision}`,
  precision `f32` or `f64`); the snapshot's view `space` (`{length_unit, metres_per_unit, up,
  handed}`), the doc's space; absent from a pack with no slot and from an older kit.
- **A placement**: `{relative_to, steps, approx?}`. `relative_to` is the address of a unit or a
  container it is placed relative to, or `null`: **the doc's space** (a unit's container plays no
  part unless it is named). Steps, the first outermost: `{"trs": {loc, rot, scale}}` (scale, then
  rotate, then translate), `{"axes": {origin, z, x}}` (built as IFC builds its axes; a `null`
  direction is the default), `{"matrix": [16 numbers, column-major]}`. A rotation is
  `{"euler": "XYZ", "angles": [x, y, z]}` (turned about the word's first axis first, each about
  the parent's fixed axes: `XYZ` is `Rz · Ry · Rx`), `{"quat": [w, x, y, z]}` (normalised by the
  reader) or `{"axis": [x, y, z], "angle": a}`. Radians. `approx` is a reason word (`driven`)
  when the file can't say exactly. An `f32` slot's numbers are the shortest decimals that read
  back as the stored `f32` (`Math.fround` gives it).
- **Where it rides**: `placement` on each unit of a units page and each container of the head
  (absent or `null`: not placed by the slot; one meaning only, never "at the origin");
  `placement_after` on a unit delta with the op `place` (it moved) and on every `add` and
  `restore` (`null`: none now); on a container delta when its placement changed or it came;
  `view_space` on a change that changed the doc's space; the render hint `containers` also names
  the containers of everything placed through a moved element. A change that only the file's
  carrier held (an outside save that moved an anonymous frame) makes the host ask the watch for
  a fresh snapshot.
- **Readers**: a step, rotation, order or space word a reader doesn't know makes the placement
  unknown (`null`), never a skipped step; there is no step limit in the client.
- **The client**: `KeyedWatch.placement(address)`, `KeyedWatch.space` and
  `KeyedWatch.worldMatrix(address)`; `placementMatrix(p, precision)` and
  `worldMatrix(lookup, address, precision)` (`src/placement.ts`): column-major `Float64Array`,
  `null` for an unknown or degenerate placement and for a broken chain (an element with none, one
  not shown, a cycle), never identity for a missing link. The kit's Rust helper
  (`Placement::matrix`, `ViewIndex::world_matrix`) gives the same numbers on one shared set of
  vectors (`test/vectors/placement.json`; JS within 1e-12 of the largest entry, plus 1e-12).

## The live grid

A spreadsheet's sheets, rows and cells as a keyed doc whose units are a grid's rows. Everything is
by address on the wire; numbers and A1 are worked out by each side from the keyed state, never
sent for a corner and never read out of an address. Additive within wire 3: `WIRE_VERSION` stays 3.

**The view schema's `grid`** (`null`, or absent from an older kit, when the pack has none;
`GridSlot`):

```json
{"rows":"rows","cols":"cols","cells":"cells","col_fields":"col_props",
 "col_container_field":"sheet","row_fields_start":"@",
 "col_of_cell_field":"{container}/col:{field}",
 "numbering":{"rows":{"gap_field":"@gap","first":1},"cols":{"gap_field":"gap","first":1}},
 "col_label":"letters","max":{"rows":1048576,"cols":16384},
 "cell":{"encoding":"json","type":"t","value":"v","formula":"f","formula_kind":"fk","style":"s",
         "types":{"n":"number","s":"text","b":"bool","e":"error","d":"date"}},
 "row_fields":{"hidden":"@hidden","height":"@ht","height_unit":"pt"},
 "col_field_names":{"hidden":"hidden","width":"w","width_unit":"max digit widths"},
 "viewport":"cells",
 "style_hint":{"field":"s","default":"0","table":"styles","parts":["bold","fill","numfmt"]},
 "cell_derived":["a1"]}
```

- `rows` is the schema's `units` root. A row's container is found as a unit's is: its
  `unit_container_field` value matched against the container's `container_id_field` (or the
  container's own address when that is `null`). A column's: its `col_container_field` value in
  `col_fields`, matched the same way.
- `cells` is the rows' composite map, keyed `<row address>\x1f<field>`. A field starting with
  `row_fields_start` is a row field; any other is a cell, whose column address is
  `col_of_cell_field` with `{container}` the container's address and `{field}` the field
  (`colOfCellField`).
- **Numbering** (`gridNumbers`): within one container, in the list order of `rows` (`cols`), each
  live element's number is the previous one's plus its gap plus 1; the first one's is its gap plus
  `first`. The gap is the element's `gap_field` value in its composite map read as a whole number
  at least 0; missing or not a number, 0. Deleted elements don't count.
- `col_label`: `letters` (1 = A, 26 = Z, 27 = AA, 16384 = XFD) or `numbers`, for a view's column
  headers. A1 always uses letters.
- `max.rows`, `max.cols`: the highest row and column numbers.
- `viewport: "cells"`: a peer's `viewport` `{x, y, w, h}` is in cells of its focus container: `x`
  the first column's number, `y` the first row's, `w` columns, `h` rows; the rows it shows are
  `floor(y)` to `ceil(y + h) - 1`, the columns the same. It is positional: the kit never moves it.
- **`field_class_prefix`** in the view schema: `[["c", "cell"]]`, ordered `[prefix, class]`
  rules ([View deltas](#view-deltas)); class `cell` is a cell's value.

**Corners, ranges, cells.**

- A **corner** `{row, row_off, col, col_off}`: `row` is the address of the last keyed row of the
  container at or before the cell (`null` when there is none), `row_off` how many rows past it
  (with `row: null`, `row_off` is the row number itself, so at least 1); `col` and `col_off` the
  same for columns.
- **Normal form**, required: no keyed row (column) of the container lies strictly between `row`
  and the cell; that is, with `row`'s number n and the next keyed row's gap g, `row_off <= g`
  (no next row: unbounded); with `row: null`, `row_off` is from 1 to the first row's gap (no
  rows: unbounded). The cell's number, n + `row_off` (or `row_off`), is at most `max.rows`; the same for
  columns. `cornerProblem` says why a corner isn't one; `cornerAt` gives the corner for a cell's
  numbers; `cornerNumbers` the other way.
- A **range** `{start, end}`: two corners, either above or left of the other.
- **Cells** `{container, active, ranges}`: the active cell (`null`: none) and 0 to 16 ranges
  (`CELLS_RANGES_MAX`); `active` lies inside a range when there are ranges.
- **A1**: `<letters><number>` (`cornerA1`); a range `A1:B4`, the start's then the end's as given,
  never swapped (`rangeA1`); a whole column runs to the most rows (`D1:D1048576`), a whole row to
  the most columns (`A9:XFD9`). `cellsA1` gives the active cell's and each range's.

**Cells presence** (feature `cells`): the client frame `{type: "cells", req, cells}` ([Presence
frames](#presence-frames)) and the roster event `{kind: "cells", who, cells, client?}`, `cells`
`null` when gone; `cells` on a roster entry and a `devices` element.

- **Kept by keys:** an insert inside a range grows it, with no event. **Dropped, never moved:** a
  range whose corner's row or column stops being live, or whose corner's form breaks, is removed
  (an `active` likewise, on its own); the kit sends a `cells` event with what is left (`null` when
  nothing is).
- **An agent's cells move with its work** (the kit's; a person's are never moved): after an
  agent's verb or claim on cells, in the same flush:
  - a verb that wrote cells: its written ranges, `active` the first range's start;
  - an insert: the inserted band as whole rows (columns 1 to the most) or whole columns (rows 1
    to the most), `active` the band's start;
  - a delete: a caret on the first visible cell after the band (in the first column, or the
    first row), no ranges;
  - a sheet added or renamed: its focus to that container; its cells kept if they are on it, else
    a caret at A1 there;
  - a claim on cells: the claimed range, `active` at its start.

**Claims on cells** (feature `cell_claims`): [Claim frames](#claim-frames) and [Claim
events](#claim-events-wire-3).

**Rows.** `test/vectors/grid_view.tsv` (shared with the kit's rows) replays in
`test/grid-view.test.ts`: numbers, letters, blank rows, A1 and refused corners at each rev it
names. The kit's roster golden `test/golden/roster_events_cells.json` (`{self, client, flushes:
[{rev, events}]}`) folds in `test/grid-presence.test.ts` to the vector's agent cells after each
rev and its claim.

## What the client guarantees

- Its pending change is carried **as sent** through every incoming change (the kit's
  `Carried`): never re-canonicalized between steps, so it is refused exactly when the
  session refuses it. No frame changed.
- Its rebase is the kit's (`session::rebase`): an incoming change goes
  through the client's pending change with the incoming one first at a tie; the pending change
  goes through incoming ones as the server will carry it. **Each hunk is carried as sent**:
  the pending change keeps its hunks' identities between steps (the kit's
  `rebase::Carried`), never re-made canonical until it is applied, and an incoming change goes
  through the pending change's gap form (`Carried::gaps`). Re-canonicalizing on one side only
  makes the client and the server disagree. The shared vectors
  (`test/vectors/*.json`, written by the Rust kit) pin both.
- When everything is acknowledged, its text is the session's text at its version.
- A local edit is never dropped silently: each is acknowledged (`onAck`) or rejected
  (`onRejected`), exactly once, across reconnects too. A rejected change keeps its typed text
  and a region that moves with later changes, so the host can show it and offer `reapply`.
- Re-apply (`LiveText.placeRejection`, `LiveText.reapply`) puts a rejected change back where
  the server would put it: its `edit` frame rebased from its base with the server's rule, sent as
  a fresh `edit` at the current version (no frame changed). Otherwise it refuses.
- It never sends a frame over the cap.
