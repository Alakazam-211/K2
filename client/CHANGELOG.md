# k2-mpc-client changelog

The client's own history. The kit's `CHANGELOG.md` has each release in full; the client ships
with the kit's tags.

## 0.10.0 (kit tag v0.9.8.2)

- **The live editor, M1:** `mountLiveEditor` (`k2-mpc-client/editor`). See `EDITOR.md`, and the
  kit's CHANGELOG under v0.9.8.2.
- **Goldens for `overwritten` and `closed`:** `test/golden.test.ts` reads the kit's real JSON for
  both roster events, a snapshot's waiting notes and the roll-up, and checks every field of
  `overwritten` against `wire.ts`'s type. WIRE.md: only notes of kind `note` are untyped; a
  snapshot's `overwritten` is present only when there are waiting notes.
- `@codemirror/commands` ^6.6, `@codemirror/language` ^6.10 and `@codemirror/lang-markdown`
  ^6.2 are new (optional) peer dependencies; `@codemirror/view` ≥ 6.27 (was ^6.26), `state`
  ^6.4.
- `mountLiveEditor` throws `LiveEditorError` (`MPC-EDITOR-HISTORY`) when the host's extensions
  include CodeMirror's history.
- Events `landed` and `duplicateRisk` (a change whose reply was lost, landing after all);
  `reapply` returns a `ReapplyResult` (`applied`, `already-present`, `unknown`, `unavailable`).
- Re-apply refuses a rejection with more than one hunk (`unknown`, `reason: "multi-hunk"`;
  `LiveText.reapply` returns `null`); per-hunk restore is planned.
- **Re-apply now applies only a plain lost insertion** (typed text whose reply was lost, or that
  was refused for a reason other than someone else's change, with nothing changed around it
  since); every other rejected change is refused with "check and retype" until M2, which puts
  them back where the server would.
- **Re-apply's guards (single hunk):** it re-inserts the typed text only for a pure insertion,
  with no reload since, nothing else inserted at its place since and no text around its place
  (or at its edge) deleted or replaced since; otherwise `unknown` with `reason` `not-insertion`, `reloaded`, `tie`
  or `deleted-around` (strings `notInsertion`, `reloaded`, `tie`, `deletedAround`), the
  rejection kept. Before, a deletion said `applied` and changed nothing, a replacement's text
  went in beside the old text, a change pending at a reload went in at the start of the
  document, at a tie the text went in before the other insertion where the server puts it
  after, and a place inside deleted text went to the deletion's start (and stayed there when
  that deletion was itself refused). An insertion refused for competing text (`MPC-E111`) is
  `deleted-around`. `LiveText.reapplyGuard(id)` and the type `ReapplyGuard` are new.
- **One event per root per version:** `LiveText` applies each (root, version) once. A root's
  event at a version it had already seen for another root of the same change was dropped (the
  change lost, or an own edit taken for lost: `MPC-E000`); it is applied now, inside a `since`
  reply too, and an own edit is recognised after its other root's event.
- **Whether to retry a close is the close table's** (`canRetry`), never the transport's own
  `retry` flag: `auth` with `retry: true` closes, `network` with `retry: false` reconnects; a
  disagreement is one `error` event (`MPC-EDITOR-CLOSE-RETRY`, exported as
  `CLOSE_RETRY_ERROR`). A close event without a `retry` is still a close.
- `wire.ts`: the `closed` frame and `CloseEvent` gain `code?` (a host's refinement of the
  reason) and `path?` (with `moved`); the editor's `closed` event carries them.
- `LiveText`: `disconnected()`, `presence(id)`, `noteReapplied(id)`, `onLanded`,
  `onDuplicateRisk`; `Rejection.asOf` on `MPC-E000`. `ConnectOptions.prev` in `wire.ts`.
- Dev: `@playwright/test` 1.47.2 and `esbuild` 0.21.5 for the browser tests; CodeMirror dev pins
  `state` 6.7.6, `view` 6.43.15 (one copy of each in the tree).
- The licence check allows only MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, plus
  one named dev-only exception.
- The fake server (tests) drops and closes connections, answers `snapshot`, and refuses `since`
  past a window.

## 0.9.9

The live wire contract (`WIRE.md`): the handshake, closing, reconnecting
(`LiveText.reconnected`), `undone`, carets at a version, keepalive, the frame cap, and
`LiveTransport` in `wire.ts`. See the kit's CHANGELOG under v0.9.8.2.

## 0.9.7

The hunk channel's client (K1): `LiveText`, the kit's rebase rules, the CodeMirror 6 binding
(`codemirrorLive`). See the kit's CHANGELOG, v0.9.7 and before.
