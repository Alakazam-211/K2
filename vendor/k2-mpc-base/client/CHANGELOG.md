# k2-mpc-client changelog

The client's own history. The kit's `CHANGELOG.md` has each release in full; the client ships
with the kit's tags.

## 0.13.2 (kit tag v0.12.0)

The client for kit v0.12.0. The client's code, tests and wire contract are as in 0.13.1: no
export is added, removed or renamed, no frame changes, and a host changes nothing. The kit's
changes in this release (the log entry's body, the grid row limit, the grid paths) are on the
Rust side only.

- **Docs only:** 0.13.0's entry says it plainly: a held undo or redo isn't counted in `pending`.

## 0.13.1 (kit tag v0.11.1)

The client for kit v0.11.1. No export is added, removed or renamed, and no frame changes; a host
changes nothing.

- **`write --base` conflict regions: a doc and spec clarification, no behaviour change.** A
  refused write's conflicting hunks that overlap or touch are one region, its lines the union
  (undo spec O6). The regions a client reads are the same as before.
- **An undo with the same text as a lost reply's edit isn't taken for the edit.** After a reconnect,
  the `since` reply's change from this client is taken as the edit in flight only when its
  `kind` is `edit` (or, from a kit before v0.10.2, which sends no `kind`, by client ID and hunks
  as before). An undo or redo whose hunks equal the edit's, applied after the host refused the
  edit, is applied as a change instead, and the edit goes to `onRejected` (`MPC-E000`).
- **A closed `LiveText` sends nothing:** after `close()`, `undo()` and `redo()` throw (as
  `applyLocal` does), and `reset()`, `setCursor()`, `setVisible()` and `setView()` do nothing;
  `reset()` sent an undo or redo still held, and the others their frame. `HOST.md` lists it.
- **Tests only, no behaviour change:** `test/undo-held.test.ts` pins that a held undo or redo
  always goes out, on every path that takes the edits before it out of the queue (acked, acked
  past a version gap, refused, base expired, dropped by a rebase, composed away, several held,
  a reconnect with only the hold, a refused snapshot then a connect without resume), and that
  `close()` drops it with nothing sent after.

## 0.13.0 (kit tag v0.11.0)

The client for kit v0.11.0. Additive only: no export is removed or renamed, and a client talking
to an older host sends none of the new frames (`welcome.features`).

- **An undo or redo goes in order with the edits** (as client 0.12.1, kit v0.10.2.1). The server
  undoes the top of the participant's stack when the request arrives; `undo()` / `redo()` made
  while edits are queued are now held after them and sent once every edit before them has gone
  (no ack awaited), so the server undoes the step the person meant, and the edit typed before a
  redo clears it first, as in an editor. Typing after a held request starts a new entry. With
  nothing queued it goes at once; held while disconnected, it goes on the new connection in the
  same place; a held undo or redo isn't counted in `pending`. Hosts change nothing. Tests: unit
  and end-to-end, and the fake host's undo and redo stacks per participant, as the kit's.
- **`mint` helpers, new (structured live, slice 1):** `writerTag`, `parseTag`, `parseKey`,
  `formatKey`, `nextFor`, `markKey` and `retiredMark`, with `MAX_CLIENT_ID`, `MAX_KIT_CLIENT`,
  `MAX_KEY_LEN`, `MAX_PREFIX_LEN` and `MAX_TAG_LEN` and the types `TaggedKey` and `MintError`:
  the kit's tagged keys (`n` as a `bigint`), checked against the kit's shared vectors
  (`test/vectors/mint.json`).
- **`refs` helpers, new (slice 2):** `resolveRef`, `resolveRefs` and `resolveParents`, with the
  types `Ref`, `RefState` and `Lists`: a reference's state as the kit computes it, checked
  against `test/vectors/refs.json`.
- **`PresenceChannel.claim` and `release`:** claims on elements of a list root (`PRESENCE.md`).
  They resolve on `claimed` and `released`, and reject with `ClaimRejected` on `presence_refused`,
  `close()` or a reconnect; a request caught by the last two has an unknown fate, and a claim
  doesn't outlive its connection. `claimsSupported` is whether the host lists `"claims"`, without
  which nothing is sent.
- **New frames:** `claim`, `claimed`, `release` and `released`, with `req` and refused by
  `presence_refused` (`WIRE.md`, Claim frames). `PresenceFrameType` gains `claim` and `release`.
- **New roster kind:** `claim_started_elems`, and `elem_claims` on `RosterEntry`, present only when
  the member holds some. `claim_ended`'s `how` can be `deleted`.
- **`PresenceChannel`:** selection by element key, pointer and viewport over any `Transport`, data
  and events only (`PRESENCE.md`). It can share a transport with `LiveText`.
- **New frames:** `select`, `pointer` and `viewport`, with `presence_refused`. They carry their own
  request number, `req`, and sit outside the doc's numbering (`WIRE.md`, Presence frames).
- **`welcome.features`:** the frame families a host takes. A client sends a family's frames only
  when its name is listed, and ignores names it doesn't know.
- **New roster kinds:** `selection`, `pointer` and `viewport`, with the same three fields on
  `RosterEntry` and `Device`.
- A tier 0 doc has none of the three calls. The golden files hold whole coordinates only; the
  client's tests cover fractions.

## 0.12.0 (kit tag v0.10.2)

The client for kit v0.10.2.

**The client is a library only: the editor component is removed** (kit v0.10.2). Hosts own
their editor UI and draw it over the library: `HOST.md` has the calls an editor makes, the
rules it keeps (no local undo history; one `Transport` across reconnects; `connect` while open
replaces the connection; drop, don't buffer, while closed; one edit in flight; the retry loop
by `canRetry`; Re-apply with presence first) and the tests that cover each.

- **Removed:** `src/editor/*` (`mountLiveEditor`, `DEFAULT_STRINGS`, `HISTORY_ERROR`,
  `CLOSE_RETRY_ERROR`, `LiveEditorError`, `ByteIndex`, `readableOn`, `contrast`, `luminance`,
  `CSS_VARS` and their types), the `./editor` export, and `src/codemirror.ts`
  (`codemirrorLive`, `changesToHunks`, `hunksToChanges`, `remoteChange`,
  `CodemirrorLiveOptions`). `MPC-EDITOR-HISTORY` and `MPC-EDITOR-CLOSE-RETRY` were the
  component's and go with it. `EDITOR.md` is a pointer to `HOST.md`.
- **New exports:** `setDefaultLiveTimers` from the package root and `core` (`core` now
  re-exports the root: the same names); `k2-mpc-client/test` (`test/index.ts`): the test
  helpers (`FakeServer`, `FakeLive`, `Net`, `rng`, `FakeLiveHost`, `FakeLiveTransport`,
  `netTimers`, `microtasks`, `settle`, `randomEdit`, `randomText`, `boundaries`, `runrec`),
  each still at its own path; `k2-mpc-client/test/jsdom-shims` (a side-effect import). In
  `files` and `sideEffects`.
- **Dependencies:** no peer dependencies (the five `@codemirror/*` peers are gone). Dev:
  `@codemirror/commands`, `lang-markdown`, `language`, `view`, `@playwright/test` and `jsdom`
  removed; `@codemirror/state` 6.7.6 stays **dev-only, a test oracle** (the caret re-send's
  "maps the caret exactly as CodeMirror maps a selection"), never imported by `src`. The
  lockfile drops 85 packages and adds none; the licence check's one exception (MIT-0, through
  jsdom) is gone with it.
- **Tests:** no browser tests (no Playwright, no Chromium or WebKit runs) and no DOM: the whole
  suite runs in Node. The editor's library coverage moved onto `test/live-host.ts`, a DOM-free
  host that keeps `HOST.md`'s rules over `LiveText`: `live-convergence.test.ts` (three hosts,
  random typing, outside edits, undo, dropped sockets and reconnects; Re-apply never doubling
  text on a host without the reconnect rule; fixed seeds each failing under its planted fault,
  re-picked by `live-seed-scan.test.ts`), and the caret re-send scenarios on `carets()`
  (`caret-resend.test.ts`). The planted faults moved to `test/plants.ts`. `exports.test.ts`
  pins every name and file a host builds against (a planted rename and a removed export are
  caught). The byte index's test (`ByteIndex`, the editor's) went with the editor, as did the
  bench's two binding models.

- **Licence:** `package.json` (and the lockfile's root entry) say `"license": "MIT OR
  Apache-2.0"`, the kit's licence (also in 0.11.0 as shipped with kit tag v0.10.0.2). The
  package now carries `LICENSE-MIT` and `LICENSE-APACHE` (copies of the kit's, in `files`), so
  `npm pack` includes them.

- **`ErrorReport.next_step`** (K-PAR-10): a refusal's `next` as data, `{verb, args, why}`
  (the new `NextStep` type; `verb` is always a kit verb). It is optional: absent when there is
  nothing to run, and from a host on an older kit, and such a report reads as before; `next`
  keeps its prose.
  `WIRE.md` lists it.
- Test tooling: the bench gate (`M2_BENCH_GATE`) waits for a quiet machine before each
  measurement. When other work used more than half the core it is pinned to (sampled from
  `/proc/stat`), or the 1-minute load is above half the machine's CPUs, it logs `BENCH LOADED`
  and re-checks every 15 s for up to 5 minutes. A target that never got a quiet machine ends
  `BENCH GATE LOADED` (exit 3), distinct from a miss (exit 1). Thresholds:
  `M2_BENCH_CORE_BUSY_MAX`, `M2_BENCH_LOAD_MAX`, `M2_BENCH_LOADED_WAIT_S`. Limits never change.
- Dev dependencies: vitest 4.1.12 (was 2.0.5), vite 6.4.5 (now pinned directly; vitest 2 brought
  5.4.21) and esbuild 0.28.2 (was 0.21.5), for the vitest, tinypool, vite and esbuild advisories;
  no runtime dependency changes. package-lock.json is written with npm 11 (npm 10.9's arborist
  crashes resolving vitest 4); npm ci works on npm 10. vitest 3 and later time out synchronous
  tests too, so the four long suites (`rope`, `realistic`, `convergence`, `editor-convergence`)
  set `{ timeout: 60_000 }` on their `describe`; every other test keeps the 5 s default. Two
  rope tests run about 2x slower under vitest 4 (runner overhead; bench-quick unchanged: the
  same source bundled by esbuild 0.21.5 and 0.28.2, three interleaved runs each, gives a median
  new/old p50 ratio of 1.05 over 36 measurements, range 0.75-1.33, inside the 1.33x median
  run-to-run spread of one build).
- **An undo in flight no longer forces "check and retype" on a kit v0.10.2 host.** A change
  whose reply was lost (`MPC-E000`) is recognised when it lands late by its client ID. An undo
  or redo still unanswered when that connection dropped lands under the same ID, so until now
  either kind of change left Re-apply answering `unknown`. Now `LiveText` reads the change
  event's `kind`: only an `edit` from that ID is the lost change (`landed`, `presence`
  `present`); an `undo` or `redo` is not (ignored; the change stays watched, and `presence` is
  `absent` when nothing else came from that ID, so Re-apply places it). Any other kind under a
  client's own ID (`revert`, `restore`, `external`, `import`) shouldn't happen and concludes
  nothing (`unknown`). **With an older kit** (no `kind`), or when some of those events have a
  kind and some don't, it is as before: an undo in flight gives `unknown` and no `landed`.
  No API change. `EDITOR.md` (then), "Rejections"; `WIRE.md`, "Undo and redo".
- **Correction to the v0.10.0.1 notes:** they said undo and redo would be marked in a change's
  `via`. They are marked in **`kind`** (`ChangeEvent.kind`, kit v0.10.2); **`via` is
  unchanged**.
- Tests: the fake host sends `kind` (`edit`, `undo`) on every change event, and
  `FakeServer.emitKind = false` models an older kit. Scripted cases, mounted and on
  `LiveText`: a late undo alone (absent, Re-apply applies once), a late edit alone (landed),
  both in either order (landed once, at the edit's version, no doubled text), events with and
  without a kind mixed (`unknown`), another kind under the client's own ID (`unknown`), and the
  older kit's rule unchanged. Planted faults (an undo read as the edit, kinds ignored, another
  kind read as an undo, a kind-less event read as an undo) must each fail them. The
  convergence runs and the Re-apply scan run with and without `kind`.
- A test fixture for a removed-text note to a reader without history now carries the kit's
  v0.10.2 message ("Someone with history access can restore it from version N.").

**Presence and marks as data** (`PRESENCE.md`). What a host draws its own editor UI from:
other people's carets and the person's rejected changes, with every offset in UTF-16 code units
of the current text. Library only (`live.ts`, `presence.ts` (new), `index.ts`, `core.ts`): no
DOM, and `src/editor/*` is unchanged. Additive: every existing name and signature is as before
(`presence(id)` still answers whether a lost change landed).

- **`participants()` / `onParticipants(cb)`:** one entry a connection (`Participant`: `handle`,
  `client?`, `name`, `colour` as the roster sent it, `kind` (`person` / `agent` / `app`; the
  kit's `external`, `restore`, `import` and unknown kinds are `app`), `rawKind`, `isSelf`,
  `sameUser`, `label?`, `caret`, `selections` (one element today), `lastActiveAt`, `away`).
  This connection is listed once with `caret: null`; the person's other connections are
  `sameUser`; a participant with no caret to show has one caret-less entry; an older host gives
  one entry a participant. Built on `carets()`, converted through the rope's index.
- **`marks()` / `onMarks(cb)`:** each open rejection as `Mark` (`id`, UTF-16 `range`, `text`,
  `reason {code, message}`, `error`, `mayHaveApplied`, `presence`, `placement`, `canReapply`,
  `canInsertHere`, `removed`).
- **"Insert my text here" in the library:** `insertHerePreview(id)` (where, in UTF-16, and
  what; changes nothing) and `insertHere(id, {group?})` (a local edit in its own undo group,
  the rejection dismissed). The frozen editor's semantics: one hunk only, presence checked for
  a lost reply, the typed text at the tracked place, never a deletion. The editor keeps its own
  copy.
- **Batching:** at most one callback a listener a delivery, with the latest snapshot, none when
  nothing changed. This connection's own activity (its entry's `lastActiveAt`) never calls back
  by itself: the person typing re-renders no participant list. **New option** `LiveOptions.schedule` (`FrameScheduler`): default
  `requestAnimationFrame` when present, else a 16 ms (`FRAME_MS`) timer on `LiveOptions.timers`;
  `(deliver) => deliver()` for immediate delivery (an agent, a Node tool).
- **New exports:** `participantKind`, `defaultScheduler`, `FRAME_MS`, and the types
  `Participant`, `ParticipantKind`, `Range16`, `Mark`, `MarkPlacement`, `InsertPreview`,
  `InsertHereResult`, `InsertRefusal`, `FrameScheduler` (from both entry points); `Presence`
  is now exported from `core` too.
- **Tests:** `test/presence.test.ts`: offsets across remote inserts before, after and inside
  carets, with CJK and emoji; this connection never given a caret, its other tab `sameUser`;
  50 caret moves in a frame give one callback with the latest; no callback when nothing
  changed; `lastActiveAt` on moves and typing, not on an unmoved re-send; typing alone gives no
  callback, going away and back one each; an older host; kinds
  and colour; an immediate scheduler, the timer default and the frame default; a mark's UTF-16
  range and reason; dismiss; Re-apply by the rule; a lost reply (`cant-tell`, landed); insert
  here where the preview said, once, in its own undo group; multi-hunk refused. Planted faults,
  each failing its check: byte offsets, no batching, a callback every delivery, a caret on this
  connection, activity by the raw caret, the own activity compared again, insert here without the one-hunk check, local edits
  without their group (`PRESENCE_PLANT=<name>` plants one in every test).

**Per-connection carets.** The same person on two devices or in two tabs sees their other
connection's caret, as Google Docs shows one login's other tabs. Library only (`live.ts`,
`wire.ts`, `index.ts`, `core.ts`): `src/editor/*` is unchanged, and the library marks carets and
draws nothing (no UI text). Additive: from an older host the client does exactly what 0.11.1 did.

- **Wire types:** `RosterEntry.devices?: Device[]` (`Device = {client, cursor, label?}`, exported)
  and `cursor` events' `client?` (kit v0.10.2, `WIRE.md` "The roster").
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

## 0.11.1 (first released in kit v0.10.2)

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
