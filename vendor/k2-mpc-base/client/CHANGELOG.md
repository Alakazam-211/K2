# k2-mpc-client changelog

The client's own history. The kit's `CHANGELOG.md` has each release in full; the client ships
with the kit's tags.

## Unreleased

**Per-connection carets.** The same person on two devices or in two tabs sees their other
connection's caret, as Google Docs shows one login's other tabs. Library only (`live.ts`,
`wire.ts`, `index.ts`, `core.ts`): `src/editor/*` is unchanged, and the library marks carets and
draws nothing (no UI text). Additive: from an older host the client does exactly what 0.11.1 did.

- **Wire types:** `RosterEntry.devices?: Device[]` (`Device = {client, cursor, label?}`, exported)
  and `cursor` events' `client?` (kit v0.10.1, `WIRE.md` "The roster").
- **The roster fold** keeps carets per (handle, client): a `cursor` event with `client` updates
  that connection's device (a `null` caret removes it: the connection left or dropped its
  caret); every `cursor` event still sets the entry's `cursor`, as before.
- **`carets()`** with `devices` on an entry: one caret a connection, most active first (so
  `find` by handle gets the caret an older host showed); this connection's own
  (`LiveOptions.client`, `welcome.client`) never; another connection of this participant with
  `sameUser: true`; without a client ID, every connection of its own participant is hidden.
  With no `devices` (an older host): one caret a participant, its own skipped, as 0.11.1.
- **`RemoteCaret` gains** `client?` (the connection, when the host sends per-connection
  carets), `isSelf` (always `false`: this connection is never returned), `sameUser` and
  `label?` (the host's label for the connection). Existing fields and their meaning are
  unchanged.
- **A device's label** arrives with a snapshot (a `cursor` event carries none): a connection
  that joins after this one subscribed has no label until the next snapshot.
- **Tests:** `test/carets-per-conn.test.ts`: connections of one person each see the others'
  carets, mapped across remote edits; their own is never returned (snapshot or events); no
  client ID hides the own participant's; other people are one caret a connection; an older
  host is the 0.11.1 `carets()` exactly (a copy of it, 40 random runs); planted faults
  (skipping by handle again, one caret a person) fail. The fake host's caret route sends
  `devices` and `client` as the kit does (`caretRoute.olderHost` for the old route), through
  which two tabs of one person see each other's carets and a tab that leaves takes its caret
  with it. The goldens gain `devices` and `client`; `roster_events_devices.json` (new) folds to
  the kit's per-connection carets.

## 0.11.1 (unreleased; kit tag v0.10.0.3)

A fix for remote carets, client-side only: no wire change, no new dependency. `package.json`
stays at `0.11.0` for this patch (a host applies the diff to its vendored copy).

- **The caret is re-sent after remote changes, mapped and throttled.** A host that drops a
  `cursor` frame unless it is at the version that socket was last sent (and may forward a
  queued change just before comparing) could leave an idle person's caret unseen. 0.11.0 did
  re-send on each remote change of the root, but at the caret's old offset (the editor then
  sent a corrected second frame), not after a change to another root, and unthrottled. Now
  `LiveText` moves its caret through every change that isn't the person's own typing exactly
  as CodeMirror moves a selection, and re-sends it after any remote change that advances the
  version (another root's included): at most once per 250 ms, with a trailing send at the
  last version, never while an edit of its own is pending. The editor's echo of the moved
  caret dedupes to nothing (one frame per version). `setCursor` with the caret's current
  place sends nothing while a re-send is due.
- **New options:** `LiveOptions.timers` (`LiveTimers`: the throttle's clock; the system's by
  default) and `LiveOptions.caretResendMs` (default `CARET_RESEND_MS`, 250).
- **Tests:** the fake host gains an opt-in caret route (`FakeServer.caretRoute`): a `cursor`
  frame is accepted only at the version that connection was last sent, compared after the
  queue is forwarded, folded per participant as the kit's roster does, and pushed as roster
  events on `caretTick()`. `test/caret-resend.test.ts`: an idle caret read behind shows in the
  typist's editor and stays, at its real place (fails with the re-send planted out); recovery
  within one throttle window; the throttle (50 remote changes in 100 ms: 2 frames); nothing
  sent with an edit in flight; the mapping against CodeMirror's `SelectionRange.map`; and
  today's limit for one person on two devices (the other device's caret isn't drawn). The
  editor harness runs the throttle on its fake clock.
- **The editor harness's fixed seeds were picked again:** caret frames draw link latencies from
  a run's generator, so the re-send changed every run's schedule and the old seeds no longer
  reached their planted faults (seed 77 re-applied nothing). The new ones (7, 8; 1163, 1752;
  9, 10; slow 7003, 7023) each pass and fail under their fault, as before.

## 0.11.0, with its licence (kit tag v0.10.0.2)

The same code as 0.11.0: `package.json` (and the lockfile's root entry) now say
`"license": "MIT OR Apache-2.0"`, the kit's licence (the kit's `LICENSE-MIT` and
`LICENSE-APACHE`). The version is unchanged.

## 0.11.0 (kit tag v0.10.0.1)

The live editor's M2, on kit v0.10.0. No wire change: hosts built for 0.10.0 carry 0.11.0 as
is. `WIRE.md`: `set_cursor_at` is planned for kit v0.10.1 (was "v0.9.9"). What a host wiring
the editor in needs to know, with the items deferred to kit v0.10.1 and their fallbacks: the
kit's CHANGELOG under v0.10.0.1.

- **Browser tests run Chromium only by default:** `MPC_BROWSERS` (comma-separated: `chromium`,
  `webkit`) names others, and an unknown name fails the run. A bare `playwright test` no longer
  selects WebKit, which needs system libraries most runners lack. `tools/README.md` gives the
  full-checkout gate and the WebKit client-only job as two separate commands.
- **The replica's text is a rope** (M2): `LiveText` keeps its texts in a persistent chunked
  rope (`src/rope.ts`) instead of rebuilding a whole string on every edit. A keystroke, a remote
  change and a `since` replay no longer scan the document. Measured on a build node, one core,
  p95 per acknowledged keystroke: 0.37 ms at 1 MB (was 18.1 ms) and 0.77 ms at 4 MB (was
  73.2 ms); a 500-event `since` at 1 MB: 73 ms (was 6.9 s). The API is unchanged: `text` is still
  a string, built on its first read after a change and cached.
- **Re-apply by the server's rule** (M2): a rejected change goes back where the server would put
  it: the change as sent, rebased from its base through every change since (`rebaseOurs`), then
  through the person's own pending changes, sent fresh at the current version as its own undo
  step. Deletions, replacements, changes with several hunks and ties all go back in place;
  `already-present` when nothing is left to apply. Refused: `conflict` (it met someone else's
  change) and `cant-place` (a reload since, or past the 512-change history). This replaces
  v0.9.8.2's single-hunk guards; `LiveText.placeRejection` and `Placement` are new, and
  `reapplyGuard` is deprecated (below).
  The Re-apply scan (seeds 1–500) gives back every change the guards refused that the rule
  places, with 0 differences from the rule.
- **A faster `since` replay:** `recarry`'s self-check is a length check on the plain incoming
  path (nothing pending), and the rope's edit copies its unchanged chunk list instead of
  rebuilding it. A 500-event `since` at 1 MB, p50 of 20 runs, interleaved in one job on a busy
  build node: ASCII 78–98 ms before, 53–63 ms after; CJK/emoji 41–47 ms before, 17–21 ms after.
- **The rope has two levels** (M2, step d): chunks in blocks of about 32, each block with its
  own starts. An edit rebuilds the chunks it touches in at most three blocks and copies the list
  of blocks (about 30 at 1 MB), not the list of chunks. A 500-event `since` at 1 MB, p50 of 20
  runs interleaved in one job against the one-level rope: ASCII 50 ms → 23–33 ms; CJK/emoji
  13 ms → 16 ms (both well under the 100 ms target). The property test gains runs with tiny
  blocks (edits across block edges) and a planted fault in the block sums it must catch.
- **No re-index during an IME composition:** while remote changes are held, the binding keeps
  the held document (the editor's plus the held changes) and its byte index up to date change by
  change; a keystroke no longer builds a byte index of the whole document. A test checks the
  held document and index against `live.text` after every change of a composition; the bench
  prints the composition keystroke at 1 MB and 4 MB.
- **The editor's byte index keeps line lengths in blocks**: a keystroke copies the touched block
  and the list of blocks, not every line's length (O(lines / 256)).
- Tests: a differential property test of both against plain strings (random edits, CJK, emoji
  and surrogate pairs at chunk edges, empty and 1 MB documents, every offset conversion), with
  a planted fault (a chunk's cached UTF-8 length off by one) it must catch. The bench
  (`test/bench/live-text-bench.ts`, `M2_BENCH_GATE=1`) fails when a target is missed: p95 per
  keystroke under 1 ms at 1 MB and 2 ms at 4 MB, a 500-event `since` under 100 ms at 1 MB.
- **"Insert my text here"** (M2): a single-hunk action the person chooses, beside Re-apply. It
  first shows the typed text in the editor where it would go (announced in the note,
  `role="status"`; for a replacement or deletion the note adds that the removal isn't redone),
  and inserts it only on **Insert** (its own undo step); **Cancel** or Escape inserts nothing. Presence is checked first for a lost change. Handle: `insertHere(id)`,
  `confirmInsert()`, `cancelInsert()`, `InsertResult`; class `cm-mpc-insert-preview`.
- **Copy** on every rejection with text (`copyRejected(id)`): to the clipboard, or the text
  selected when the clipboard refuses. The row's text is selectable.
- New strings: `insertHere`, `insertConfirm`, `insertCancel`, `insertPreview`, `insertRemovedText`, `insertMultiHunk`,
  `copy`, `copied`, `copyFailed`, `conflictReapply`, `cantPlace`.

**Deprecated**
- `LiveText.reapplyGuard(id)` and the type `ReapplyGuard` (from v0.9.8.2): a stub over
  `placeRejection` for this release (`ok`, `conflict` or `cant-place`; the old reasons are never
  returned). Use `LiveText.placeRejection(id)` and `Placement`; `reapplyGuard` goes in the
  following release. `ReapplyResult`'s old reasons and their strings stay in the types, unused,
  and go with it.

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
