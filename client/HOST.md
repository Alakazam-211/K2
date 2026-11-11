# Drawing your own editor over the client

The client (`k2-mpc-client`) is a library: a text replica (`LiveText`), the kit's rebase rules,
the wire and its transport contract, Re-apply, and presence and marks as data. It has no DOM and
no editor component. The host draws its editor (CodeMirror, a textarea, anything) and wires it
to the library with the calls below. `WIRE.md` is the frame spec; `PRESENCE.md` details the
presence and marks data.

Every offset is **UTF-8 bytes** of `live.text` unless it says UTF-16: `applyLocal`,
`setCursor`, `carets()`, `Rejection.region`. Presence and marks (`participants()`, `marks()`,
`insertHerePreview`) are **UTF-16 code units**, what a JS string and CodeMirror index. Convert
with `utf8Len`, `byteToIndex` and `indexToByte`.

## The calls an editor makes

| Call | What the editor does with it |
|---|---|
| `new LiveText({text, version}, transport, {root, client, self, timers?, caretResendMs?, schedule?})` | After the first `welcome`: `text` and `version` from it, `client` (`welcome.client`) and `self` (`welcome.self`). `transport` is the host's `Transport` (`send`, `onFrame`). `timers` drives the caret re-send throttle; `schedule` when `onParticipants` / `onMarks` deliver (default one animation frame). |
| `applyLocal(hunks, {group?})` | Every change the person makes, at once, as hunks on the current `text`. One edit is in flight; the rest queue and compose per `group`. Returns the op id (`onAck` and `onRejected` list it). |
| `onChange(cb)` | Apply every change the replica makes that isn't `local` (`{hunks, by, local}`, hunks on the text before it) to the editor. The editor's text is `live.text` after each one. |
| `setCursor(anchor, head)` | The person's caret or selection, after it moves and after their own edit. The replica maps it through remote changes itself (as CodeMirror maps a selection) and re-sends it at most once per 250 ms; echoing a mapped position back sends nothing. |
| `carets()` | Other connections' carets, mapped to the current text: draw them. Never this connection's own; `sameUser` marks the person's other tabs or devices. Read again on `onRoster` and `onChange`. |
| `participants()` / `onParticipants(cb)` | Everyone in the doc, one entry a connection, in UTF-16 (`PRESENCE.md`, Participants): roster strips, caret labels, idle fading. |
| `marks()` / `onMarks(cb)` | The person's rejected changes, placed in the current text in UTF-16 (`PRESENCE.md`, Marks): underline or ghost them. |
| `rejected` / `onRejected(cb)` | Changes the server refused (or that met competing text): out of the text, their typed text kept. Show each with Re-apply and Dismiss. |
| `reapply(id)` | Re-apply a rejection where the server would put it (`placeRejection`: rebased by the server's rule from its base), as a new edit, its own undo step. **Presence first** for a lost reply (below). Returns the op id, or `null` (refused, kept; or nothing left, dismissed). |
| `presence(id)` | For a rejection with `error.id === "MPC-E000"` and `asOf` set (its reply was lost): `present` (it landed: dismiss, say so), `absent` (Re-apply may place it), `unknown` (can't tell: don't re-apply; ask the person to check). |
| `noteReapplied(id)` | After re-applying a lost-reply rejection: if it lands late after all, `onDuplicateRisk` fires. |
| `dismiss(id)` | The person dismissed it. |
| `insertHerePreview(id)` / `insertHere(id)` | "Insert my text here" for a single-hunk rejection: show where its typed text would go, then insert it as its own edit (`PRESENCE.md`). Not the server's rule; never deletes. |
| `undo()` / `redo()` | Requests to the server, sent in order behind the edits typed before them. Their text arrives as a `change`; then `onUndo` (`steps: 0`: nothing to undo), or `onError` (`MPC-E201`-`E203`). |
| `onAck`, `onLanded`, `onReload`, `onError`, `onClosed` | Pending state; a lost change that landed after all; a reload (call `reload()`, which asks for a snapshot and resets); errors to show. |
| `disconnected()` / `reconnected(welcome)` | Around every reconnect (below). |
| `setVisible(v)`, `resync()`, `close()` | The view is (not) in front; the host suspects lost frames; the tab closed. After `close()` nothing is sent: `applyLocal`, `undo` and `redo` throw, every other call (`reset`, `reload`, `resync`, `reconnected`, `setCursor`, `setVisible`, `setView`) does nothing, frames are ignored, and an undo or redo still held is dropped. |

## Selection, pointer and viewport

For a doc made of keyed elements. `PRESENCE.md` has `PresenceChannel`'s calls, `WIRE.md` its frames.

- **Say the host takes them:** list `"presence"` in `welcome.features`. A client sends none of the
  frames otherwise. A tier 0 doc (`BlobSession`) has none of the three calls, so don't list it on a
  connection to one.
- **Coordinates may have fractions:** the golden files hold whole numbers only. Encode them with
  your own writer, or the kit's (`errors::Value::float`, which takes finite numbers only).
- **Route the frames:** `select`, `pointer` and `viewport` to `Session::set_selection`,
  `set_pointer` and `set_viewport`. Answer a refusal with `presence_refused {req, error}`, and
  nothing on success. Never answer one with `refused` or an `error` frame.
- **Carry the roster kinds:** `selection`, `pointer` and `viewport` as `roster_event`, and the new
  fields on roster entries and devices.
- **Size your inbound queue for them:** a client sends at most one pointer frame and one viewport
  frame per animation frame, per connection.


## Claims on elements

For a doc made of keyed elements. `PRESENCE.md` has the calls, `WIRE.md` the frames.

- **Say the host takes them:** list `"claims"` in `welcome.features`, separately from `"presence"`,
  and only once everything below is done. A client sends none of the frames otherwise.
- **Route the frames:** `claim` to `Session::claim_elems`, answering `claimed {req, claim}`, and
  `release` to `Session::release_claim`, answering `released {req, claims}`. Answer a refusal with
  `presence_refused {req, error}`, never `refused`.
- **Encode the roster:** `claim_started_elems` as a `roster_event`, and on a snapshot's entries
  `elem_claims`, which no Rust type carries: build it from `entry.claims` where `span` is
  `ClaimSpan::Elems`. `claim_ended` is unchanged. Encode `RosterEvent`'s other kinds as before.
- **Choose the policy:** an edit honours claims only under `ClaimPolicy::Respect`, on the edit's
  options, and it binds agents only.
- **Declare the composite roots:** `Pack::composite` names the Map roots whose keys belong to list
  elements. A key that maps to no element isn't covered by any claim.

## The rules a host editor keeps

- **No local undo history.** The server owns undo: an editor history would take changes out by
  its own record, outside the shared order. Don't add CodeMirror's `history()` (or any undo
  stack); map the undo and redo keys to `live.undo()` / `live.redo()`.
- **One `Transport` object for the editor's whole life,** across reconnects. The host swaps
  sockets inside it; `onFrame` and `onClose` listeners outlive connections. `LiveText` is
  built once, on it.
- **`connect` while a connection is open replaces it** (the old one is left; no `onClose` for
  it).
- **While no connection is open, `send` drops the frame; it never buffers it.** The wire has no
  idempotency key: a replayed `edit` could apply twice. Call `live.disconnected()` when the
  connection ends; edits typed meanwhile wait in the replica's queue and go out once, after the
  next welcome.
- **Rule 0: one edit in flight.** `LiveText` keeps it; the editor never sends frames itself and
  never resends an edit already handed to the transport.
- **The retry loop:** on a close, `canRetry(reason)` decides (never the transport's own
  `retry`); back off with jitter, then
  `connect({resume: live.version, prev: live.client, lease: live.lease})`, then
  `live.reconnected(welcome)`. `prev` lets the host leave the previous connection first; with a
  host that reuses writers (kit v0.14.0), `lease` lets it give this client its writer back
  (`welcome.kept`), and `prev` alone closes nothing (WIRE.md, Reconnecting).
- **Say `bye` when the page goes** (a host with writer leases): on `pagehide` (or the editor's
  close), call `live.bye()` before closing the transport. It sends `bye` only when the client
  holds nothing unsent; the host then frees the writer for the person's next tab on this device.
  When it returns false, just close: the writer stays reserved for this client's reconnect. A
  page restored from the back-forward cache connects as new (`live.lease` is gone after `bye`).
- **Re-apply checks presence first** for a lost reply (`MPC-E000` with `asOf`): only `absent`
  is placed; `present` is dismissed as already there; `unknown` is kept. Then `reapply(id)`,
  then `noteReapplied(id)`.

`test/live-host.ts` is a DOM-free host that keeps exactly these rules (no drawing): a reference
for the connect and retry loop and Re-apply.

## The tests that cover them

| Rule | Tests (`client/test/`) |
|---|---|
| Rule 0, composing, rebasing | `live.test.ts` ("keeps one change in flight ... (rule 0)"), `convergence.test.ts`, `realistic.test.ts` |
| No replay while closed; a lost reply is `MPC-E000`; the reconnect | `wire.test.ts` ("reconnect (G4, G5)"), `live-convergence.test.ts` (three DOM-free hosts with dropped sockets, reconnects and resumes) |
| One transport across reconnects; connect replaces | `fake-live.ts` (the fake transport the suites run on, the contract as code), `live-convergence.test.ts` |
| Undo is the server's | `wire.test.ts` ("the own-edit filter (G3) and undo (G9)") |
| Re-apply: the server's rule, presence first, kinds | `reapply.test.ts` (the kit's rebase vectors; kinds; planted faults), `live-convergence.test.ts` (fixed seeds, each failing under its planted fault) |
| The caret re-send | `caret-resend.test.ts`, `carets-per-conn.test.ts` |
| Presence and marks | `presence.test.ts` |
| Names and files a host builds against | `exports.test.ts` |

A host's own editor tests (its drawing, keys, IME) live with the host. They can run on the
kit's fakes: `k2-mpc-client/test` (`test/index.ts`: `FakeServer`, `Net`, `rng`,
`FakeLiveHost`, `FakeLiveTransport`, `netTimers`, `microtasks`, `settle`, `randomEdit`,
`randomText`, `runrec`), each also at its own path, and `test/jsdom-shims.ts` for an editor in
jsdom.
