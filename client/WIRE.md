# The hunk channel's frames

The frames a host carries between `LiveText` (this package) and the kit's session, for one
text root of one doc. The host chooses the transport (a WebSocket, say) and implements
`Transport` (`send`, `onFrame`), or `LiveTransport` for a live editor component that also
reconnects; frames are JSON objects with a `type`. The types are in `src/wire.ts`; this is the
spec. The kit's encodings are pinned by golden files (`test/golden/*.json`, written by
`crates/base/tests/wire_golden.rs`; see [JSON encodings](#json-encodings)).

**Extra fields and error ids.**

- A host may add fields to any frame; a client ignores fields (and frame types, and roster
  event kinds) it doesn't know. Extra fields never carry doc text: the text travels only in the
  fields this spec names.
- A close's `reason` and the [Closing](#closing) table decide whether to retry. A `retry`
  field, if a host sends one, must agree with the table; the client goes by the table.
- Clients key on error ids (`error.id`, `MPC-E…`), never on `next` or on message text: those
  are for people and may change.

## Connecting

The kit expects nothing first on the wire. Everything before the first frame is the host's:

1. **Authenticate the socket.** How (a header, a cookie, a token checked before the upgrade)
   is the host's choice. The component never sees the token, and the host keeps it out of its
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
  client: number;    // Joined.client of THIS connection: new on every connection
  self: string;      // this participant as the roster names them (pid; an App viewer's handle)
  rights: "edit" | "read" | "none";
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
  to UTF-16 only for the editor (`src/codemirror.ts`).
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

- `op` is the client's request number; replies carry it back.
- **One edit in flight (rule 0).** The client sends an `edit` only when its previous one was
  answered. `base` is the latest version it has seen, its own acknowledged edits included.
  `hunks` are on that version's text.
- `group` is an undo group token (K4): consecutive edits with one token can merge into one undo
  step; the session closes groups on idle, word boundaries and others' changes.
- A `cursor` is sent only when the client has nothing pending, so its version's text is the
  client's text.
- Frames with no reply (`cursor`, `visible`, `view`) are fire-and-forget: a refusal of one (a
  caret off a boundary, `MPC-E114`; a reader's caret, `MPC-E102`) is dropped by the host,
  silently. The client re-sends its caret after its next change.

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
| `roster_event` | `event` | one roster change as the kit pushes it (`subscribe_roster`, K5): the client folds them |
| `roster` | `entries` | the whole roster at once, for a host that folds the deltas itself |
| `error` | `error` | a client frame the host couldn't take that has no `op` to answer (over the cap, unreadable) |
| `closed` | `reason`, `code?`, `path?`, `error?` | the last frame; then the host closes the socket ([Closing](#closing)) |

- `event` is the kit's `ChangeEvent`: `{version, pid, client, via, root, hunks, at_ms,
  on_behalf_of}` (`pid` and `on_behalf_of` are opaque handles for App viewers, K5).
- `error` is the kit's `ErrorReport` JSON (`ErrorReport::to_json`): `{id, exit, message,
  next, rule, details}`. The ids the client acts on:
  - `MPC-E111` (`write_conflict`): the edit met competing text. The client takes the change out
    of its text and hands it to `onRejected` (the typed text, the hunks, the base, the region);
    `details.conflicts` is passed along.
  - `MPC-E112` (`base_expired`): `base` (or `since`'s version) is past the live window
    (1,000 changes, 4 MiB of events by default), or the changes since don't fit one frame. The
    client hands everything pending to `onRejected` and emits `onReload`; the host (or the
    component) calls `LiveText.reload()`, which sends `snapshot` and resets to the reply.
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
- A refused caret (`MPC-E114` off a boundary, `MPC-E102` for a reader) gets no reply.
- A versioned `set_cursor_at(conn, root, version, anchor, head)` that maps in the kit is
  planned for v0.9.9; a host can then pass the frame through.
- Carets coming the other way carry their version: [JSON encodings](#json-encodings).

## Reconnecting

- **A reconnect is a new connection:** the host joins again, so the connection has a **new
  client ID** (`welcome.client`); the pid is the same (an App viewer's handles too, while the
  doc stays loaded). The host calls `leave` for the old connection when its socket goes.
- **The old connection goes first (the host's rule).** A reconnect carries `prev=<client ID>`,
  the client ID of the client's previous connection (the last `welcome.client`; the component
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
  - The kit's own join that replaces a connection is planned; until then the host does this
    with `leave`.
  - Because a host may get this wrong, the live editor marks an `MPC-E000` after a reconnect
    as `mayHaveApplied`, recognises the late change by its client ID if it lands (`landed`),
    and re-applies only when the record shows it absent (`EDITOR.md`, Rejections).
- **Frames sent while disconnected are dropped, never replayed** on the next socket: the wire
  has no idempotency key, so a replayed `edit` could apply twice. `Transport.send` drops
  while no socket is open; it never buffers.
- **Edits made while disconnected wait, unsent:** the component calls
  `LiveText.disconnected()` when the connection goes (`onClose`); `LiveText` then keeps new
  edits in its queue instead of handing them to the transport, and `reconnected` sends them on
  the new connection (with its client ID), after the `since`. An edit handed over before the
  close stays in flight, and the `since` settles it.
- The component then calls **`LiveText.reconnected(welcome)`**:
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
  close: the component goes read-only.

## Keepalive and waking up

- **No app-level ping.** The host pings at the socket level (WebSocket ping frames, every
  20 s or so) and closes a connection whose subscriber lags (`closed {reason: "cut_off"}`).
- **The component wakes and resyncs** when the page regains focus, comes back `online`, or
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
    (`details: {limit: "frame", bytes, frame_max}`) and closes; no retry. The component shows the
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
  when the host marks it); `cursor` is `{root, anchor, head, version, anchor_lc, head_lc}` or
  `null`. `self` marks this connection's own participant (or the client compares `handle` with
  `LiveOptions.self`, i.e. `welcome.self`).
- **A roster event** mirrors the kit's `RosterEvent`, tagged by `kind`: `snapshot {version,
  members, watching, overwritten?}` (always first; a later one replaces everything;
  `overwritten` only when this subscriber has waiting notes, [below](#the-overwritten-event)), `joined {who,
  participant: {name, colour, kind, via}, rights}`, `left {who, reason}`, `rights {who,
  rights}`, `role {who, role}`, `away {who, away}`, `cursor {who, cursor, version}`,
  `claim_started {who, claim, lines, note, hold, for_edit, expires_ms, version}`,
  `claim_ended {who, claim, how, lines, version}`, `note {to, from, note, lines, claim}`,
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
- **A caret's `version`:** the doc version its offsets are in. The kit gives it beside the
  caret (`RosterEvent::Cursor.version`; a snapshot's carets are at the snapshot's `version`);
  the host puts it inside the caret object too. Without one, the client takes its own version
  when the frame arrives. The client maps it to its text through the changes between.

## JSON encodings

The kit has no serde: a host encodes these by hand. The golden files pin them, written from
a real session by `crates/base/tests/wire_golden.rs` (the kit's own JSON writer, as
`ErrorReport::to_json`; `MPC_BLESS=1` regenerates) and read by `test/golden.test.ts`, which
parses them with these types, replays the changes to the kit's text and folds the roster to
the kit's own final roster.

| Value | Encoding |
|---|---|
| `ChangeEvent` | `{version, pid, client, via, root, hunks: [{start, end, insert}], at_ms, on_behalf_of}`; `on_behalf_of` is `null` when absent, never left out |
| `client` | the 53-bit client ID as a JSON number (`MAX_CLIENT_ID` = 2^53 − 1 = `Number.MAX_SAFE_INTEGER`, exact in JS) |
| strings | JSON escapes: `"` `\` and control characters escaped (`\n`, `\t`, `\r`, else `\u00XX`); everything else as UTF-8 |
| `who`, `pid`, `handle`, `on_behalf_of` | `Who`'s `Display`: the pid for a person's or agent's connection; for an App viewer's (`ParticipantKind::App`), the opaque per-doc handle (`p1`, `p2`...), its own included. One lens per connection, as the kit applies it; the client treats them as opaque and compares them with `welcome.self` |
| `rights` | `"edit"` when the kit's `Rights` has `read` and `edit`; `"read"` with `read` only; `"none"` otherwise. `history` and `owner` aren't sent |
| `role` | `"editor"` / `"viewer"` (`Role`) |
| `kind` | `ParticipantKind` in lower case: `person`, `agent`, `app`, `external`, `restore`, `import` |
| `reason` (`left`) | `LeaveReason`: `left`, `idle`, `kicked`, `rights_lost` |
| `how` (`claim_ended`) | `ClaimEnd::as_str`: `released`, `edited`, `expired`, `left`, `reset` |
| `note` (`note`) | `NoteKind::as_str` |
| `claim` | `claim-N` (`ClaimId`'s `Display`) |
| `lines` | `{root, first, last}`, 1-based, inclusive |
| a caret (`CursorAt`) | `{root, anchor, head, version, anchor_lc, head_lc}`: `anchor`/`head` bytes at `version`; `anchor_lc`/`head_lc` the kit's `LineCol` as `"line:col"`, both 1-based, the column counted in characters (Unicode scalar values), for display only |
| `RosterMember` → entry | `handle` = `who`; `name`, `colour`, `kind` from `entry.participant`; `role`, `away` from `entry`; `cursor` = `entry.cursor` at the snapshot's version, or `null`; `rights` = `rights` |
| `ErrorReport` | exactly `ErrorReport::to_json()`: `{id, exit, message, next, rule, details}`. Clients key on `id`; `message` and `next` are for people |
| `RosterEvent::Overwritten {to, overwrite}` → `overwritten` | `{kind: "overwritten", to, change, version, restore_from, source, rule, row, line, cells, more, not_shown, withheld, no_history, message}`, every field always present: `to` = `to` (`Who`'s `Display`, the subscriber's own, since the kit sends it only to its author's subscriptions); `change`, `source`, `rule`, `more`, `not_shown`, `withheld`, `no_history` = the `Overwrite`'s fields of those names; `version`, `restore_from`, `row`, `line` = the `Overwrite`'s `Option`s, `null` when `None`; `cells` (below); `message` = the `Overwrite`'s `Display` (`to_string()`), which holds the person's text |
| `cells` | in order: first `{region, col, yours, file, cut}` from the `Overwrite`'s own top-level `region`, `col`, `yours`, `file`, `cut` (its first place), then one `{region, col, yours, file, cut}` per entry of `also` (an `OverwriteCell`, same fields), in `also`'s order. `col` is `null` when `None`. A roll-up (`rule: "rollup"`, `Overwrite::is_rollup`) has no place: `cells: []` (its `region` is empty; don't send it) |
| `Snapshot.overwritten` | a bare `Vec<Overwrite>`, this subscriber's own waiting notes, with no `to`. The host encodes each entry as the live `overwritten` event above, adding `kind: "overwritten"` and `to` = the subscriber's own `who` (its pid, or for an App viewer its handle, as `welcome.self`), so every entry has the live event's shape. The field `overwritten` is present only when the vector isn't empty: a missing field means none |

## The live transport (for a component)

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
  when refused (`declined`, `not_found`, `auth`...). The component owns the retry loop: back
  off, `connect({resume: live.version})`, then `live.reconnected(welcome)`.
- **`connect()` while a connection is open replaces it:** the host closes the old socket (and
  leaves its connection: frames in flight on it are lost) and the attempt goes on as any
  other; no `onClose` is reported for the old one. The live editor relies on it when a
  `snapshot` is refused or not answered: it connects again without `resume`, and the welcome's
  `text` starts it again.
- `send` **drops** while not open ([Reconnecting](#reconnecting)).
- **Authentication refused before the upgrade:** a browser's WebSocket API reports a refused
  upgrade (say, an HTTP 401) as a plain failed dial, which reads as `network`. After a failed
  dial, a transport may make one cheap HTTP pre-check of the same route to tell `auth` (no
  retry) from `network`. Only after a failed dial, never on a timer.
- **`MPC-EDITOR-HISTORY` (the live editor's, not a frame):** `mountLiveEditor` refuses to mount
  (`LiveEditorError`, `code: "MPC-EDITOR-HISTORY"`) when the host's extensions include
  CodeMirror's history, and reports one `error` event with that id when a reconfiguration adds
  one. The server owns undo ([Undo and redo](#undo-and-redo)); a local history would undo
  outside the shared order, and CodeMirror's history commands can't be intercepted (they
  dispatch with `filter: false`). Leave `history()` and `historyKeymap` out: undo and redo are
  the editor's own (`undo()` / `redo()` on its handle, `Mod-z`, `Mod-Shift-z`, `Mod-y`). See
  `EDITOR.md`.
- A `closed` frame is the transport's: it reports it through `onClose` (a host using the plain
  `Transport` may pass it on; `LiveText.onClosed` then reports it).

## The `overwritten` event

An outside save may have removed or replaced text you wrote through the kit. It goes only to its
author's own subscriptions (and the person an assistant acted for), at the next roster flush, and
is never coalesced. It carries the person's own text so they can paste it back: no single call
restores the text and keeps the saver's change.

```ts
{ kind: "overwritten", to: Handle,
  change: number,               // the save
  version: number | null,       // the version holding the text, THAT THIS READER CAN OPEN: link from this only
  restore_from: number | null,  // the same version, to tell someone with history access; never a link for this reader
  source: "an outside save",
  rule: "removed_text" | "replaced_text" | "rollup",
  row: null,                    // a grid's row key; null on a text doc
  line: number | null,          // first line, in the text before the save
  cells: [ { region: string,    // "L12" or "L12–L14"
             col: null,
             yours: string,     // this person's own removed characters, exactly, newlines included
             file: string,      // those lines now, at most 256 bytes
             cut: boolean } ],
  more: number,                 // places not listed
  not_shown: number,            // characters of their own text the note doesn't carry
  withheld: boolean,            // the text is no longer held: a purge, the 24 h hold, the doc's cap
  no_history: boolean,          // this reader has no history right
  message: string }             // the note's one line, ready to show; holds the text
```

- **"May have".** The kit can't tell a removal from a move, so `message` never says the text is
  gone. See `docs/SESSIONS.md`, "When an outside save removes someone's text".
- **Only their own text.** `yours` never holds someone else's characters from the same block.
- **Sizes.** At most 16 KiB of text a note (`OVERWRITE_TEXT_MAX`), 32 places (`OVERWRITE_CELLS_MAX`),
  the rest counted in `not_shown` and `more`.
- **Roll-up.** A participant has at most 64 waiting notes on a doc; past that the last is
  `rule: "rollup"`, `cells: []`, `more: N`. It carries no text. A note is never dropped silently.
- **Pending notes are also in `snapshot`,** as `overwritten: [...]`, this subscriber's own. A push
  doesn't consume a note: it waits until `take_notes` or a verb delivers it. Clients dedupe on
  `(change, row)`; a save makes one note per person. `overwritten` is present only when this
  subscriber has waiting notes; treat a missing field as none.
- **Someone who can't read the doc gets nothing, not even a count.** The note stays and arrives if
  they can read again.
- **Logs.** The kit's `Debug` doesn't print `yours`, `file` or `message`'s text; don't log
  `message` yourself. The audit entry holds no content.

## What the client guarantees

- Its pending change is carried **as sent** through every incoming change (E-LIVE r2.3, the
  kit's `Carried`): never re-canonicalized between steps, so it is refused exactly when the
  session refuses it. No frame changed.
- Its rebase is the kit's (`session::rebase`, amendment E-LIVE): an incoming change goes
  through the client's pending change with the incoming one first at a tie; the pending change
  goes through incoming ones as the server will carry it. **Each hunk is carried as sent**
  (E-LIVE r2.3): the pending change keeps its hunks' identities between steps (the kit's
  `rebase::Carried`), never re-made canonical until it is applied, and an incoming change goes
  through the pending change's gap form (`Carried::gaps`). Re-canonicalizing on one side only
  makes the client and the server disagree. The shared vectors
  (`test/vectors/*.json`, written by the Rust kit) pin both.
- When everything is acknowledged, its text is the session's text at its version.
- A local edit is never dropped silently: each is acknowledged (`onAck`) or rejected
  (`onRejected`), exactly once, across reconnects too. A rejected change keeps its typed text
  and a region that moves with later changes, so the host can show it and offer `reapply`.
- It never sends a frame over the cap.
