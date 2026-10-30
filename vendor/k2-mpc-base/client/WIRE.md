# The hunk channel's frames

The frames a host carries between `LiveText` (this package) and the kit's session, for one
text root of one doc. The host chooses the transport (a WebSocket, say) and implements
`Transport` (`send`, `onFrame`); frames are JSON objects with a `type`. The types are in
`src/wire.ts`; this is the spec.

## Units and ordering

- **Offsets are UTF-8 byte offsets** on character boundaries, as in the kit. The client converts
  to UTF-16 only for the editor (`src/codemirror.ts`).
- **A hunk** is `{start, end, insert}` on the wire (an object, as the kit's `text::diff::Hunk`):
  replace bytes `start..end` with `insert`. The shared test vectors write hunks as
  `[start, end, insert]` arrays to keep the files small; that's the vectors' format only. A frame's hunks are canonical: sorted, non-overlapping, touching hunks
  joined; an insertion may be followed by a deletion starting at the same point.
- **Versions** are the doc's `Seq`: consecutive; a change touching several roots gives one event
  per root, all with the same version.
- **Order (the host's rule):** frames go out in version order, and the `ack` for version V is
  sent only after every `change` before V on that connection. The kit emits `ChangeEvent`s in
  version order per subscriber, and `edit_at_base` returns after its commit.
- **What the client does when that slips** (a lossy link, a reconnect):
  - a `change` past the next version, or an `ack` past it: it asks `since` and applies the
    reply (its own change in the reply settles the edit in flight) before going on;
  - while a `since` is out, `change` frames are dropped (the reply covers them);
  - a `since` sent after an edit settles it: listed, it was applied; not listed, it wasn't (a
    lost refusal), and the change goes to `onRejected` with error id `MPC-E000`;
  - late or duplicate replies to an edit already settled are ignored;
  - `LiveText.resync()` asks again; the host calls it after a reconnect or when it suspects
    lost frames (a lost `since` reply included). Client-to-host frames must not be lost or
    reordered (a WebSocket gives this).
  - To recognise its own change in a `since` reply, the client compares hunks, and also the
    event's `client` when the host passes this connection's client ID (`LiveOptions.client`).

## Client → host

| `type` | Fields | Session call |
|---|---|---|
| `edit` | `op`, `root`, `base`, `hunks`, `group?` | `edit_at_base(conn, root, base, hunks, EditOptions::group(group))` |
| `since` | `op`, `version` | `events_since(conn, version)` |
| `cursor` | `root`, `version`, `anchor`, `head` | the connection's caret, in bytes of `root`'s text at `version` |
| `undo` / `redo` | `op` | the participant's undo / redo |

- `op` is the client's request number; replies carry it back.
- **One edit in flight (rule 0).** The client sends an `edit` only when its previous one was
  answered. `base` is the latest version it has seen, its own acknowledged edits included.
  `hunks` are on that version's text.
- `group` is an undo group token (K4): consecutive edits with one token can merge into one undo
  step; the session closes groups on idle, word boundaries and others' changes.
- A `cursor` is sent only when the client has nothing pending, so its version's text is the
  client's text.

## Host → client

| `type` | Fields | When |
|---|---|---|
| `ack` | `op`, `version`, `hunks` | an `edit` applied: `hunks` as applied (`EditAtBase::hunks`, on the text at `version - 1`), now at `version`. Nothing applied (already in the text): `hunks: []`, `version` unchanged |
| `refused` | `op`, `error` | an `edit`, `since`, `undo` or `redo` refused; nothing applied |
| `change` | `event` | every applied change by anyone, in order, **except** the client's own `edit` (answered by `ack`); its undo and redo do come as `change` |
| `events` | `op`, `events` | the reply to `since`: every change after that version, in order |
| `roster_event` | `event` | one roster change as the kit pushes it (`subscribe_roster`, K5): the client folds them |
| `roster` | `entries` | the whole roster at once, for a host that folds the deltas itself |

- `event` is the kit's `ChangeEvent`: `{version, pid, client, via, root, hunks, at_ms,
  on_behalf_of?}` (`pid` may be an opaque handle for App viewers, K5).
- `error` is the kit's `ErrorReport` JSON: `{id, exit, message, next, rule, details}`. The ids the
  client acts on:
  - `MPC-E111` (`write_conflict`): the edit met competing text. The client takes the change out
    of its text and hands it to `onRejected` (the typed text, the hunks, the base, the region);
    `details.conflicts` is passed along.
  - `MPC-E112` (`base_expired`): `base` (or `since`'s version) is past the live window. The
    client hands everything pending to `onRejected` and emits `onReload`: the host fetches a
    snapshot (`{text, version}`) and calls `reset`.
  - any other refusal of an `edit` (`MPC-E151`, `MPC-E901`, ...): as `MPC-E111`.
  - a refused `undo`, `redo`: `onError`.
- **A roster entry** is `{handle, name, colour, kind, role, away, cursor?, self?}`; `cursor` is
  `{root, anchor, head, version?}` (bytes of that root's text at `version`); `self` marks this
  connection's own participant (or pass `LiveOptions.self`). The host builds it from the kit's
  `RosterMember` (`who`, `entry.participant.name`/`colour`, the caret's `Cursor`).
- **A roster event** mirrors the kit's `RosterEvent`, tagged by `kind`: `snapshot {version,
  members, watching}` (always first; a later one replaces everything), `joined {who,
  participant: {name, colour, kind}}`, `left {who}`, `rights {who, rights}`, `role {who, role}`,
  `away {who, away}`, `cursor {who, cursor | null}`, `claim_started`, `claim_ended`, `note`,
  `watching {count}`. `who` is `Who::Pid` or `Who::Handle` as a string. Claims, notes and rights
  reach the host through `onRosterEvent`; the rest also update `roster`.
- **A caret's `version`:** the doc version its offsets are in (the kit adds it to
  `RosterEvent::Cursor`). Without one, the client takes its own version when the frame arrives.

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
  (`onRejected`), exactly once. A rejected change keeps its typed text and a region that moves
  with later changes, so the host can show it and offer `reapply`.
