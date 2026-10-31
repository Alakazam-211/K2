# The live editor (`k2-mpc-client/editor`)

A mountable editor for one text root of a live doc: CodeMirror 6 in plain DOM (no framework),
with the connection's whole life inside it. The host supplies a transport and, if it likes,
CSS variables, strings and its own roster or rejection rendering. Nothing in the component
knows about the host's chat, threads or agents; the host links those through
`participantClick`, `reveal` and `follow`.

```ts
import { mountLiveEditor } from "k2-mpc-client/editor";

const ed = mountLiveEditor(el, { transport, mode: "markdown" });
ed.on("state", (s) => console.log("now", s)); // connecting, live, reconnecting, closed
ed.on("rejected", (r) => console.log("not applied:", r.text));
// ...
ed.destroy();
```

## Install

Every `@codemirror/*` package is a peer dependency, so the host's single CodeMirror is used:

| Peer | Range | Tested with (dev pin) |
|---|---|---|
| `@codemirror/view` | ≥ 6.27 (`^6.27.0`) | 6.43.15 |
| `@codemirror/state` | `^6.4.0` | 6.7.6 |
| `@codemirror/commands` | `^6.6.0` | 6.6.0 |
| `@codemirror/language` | `^6.10.0` | 6.10.2 |
| `@codemirror/lang-markdown` | `^6.2.0` | 6.2.5 |

They're optional for the package's root entry (`LiveText` alone) and needed by
`k2-mpc-client/editor`. Two copies of `@codemirror/state` or `@codemirror/view` in one page
break CodeMirror; the host's bundler must resolve one of each.

Don't add `history()` or `historyKeymap`: see `MPC-EDITOR-HISTORY` below.

## The transport (`LiveTransport`, `WIRE.md`)

The host implements `wire.ts`'s `LiveTransport`: one connection attempt; the component owns
retries. `WIRE.md` is the spec (Connecting, Reconnecting, Closing, Frame cap, The live
transport); in short:

| Member | |
|---|---|
| `connect({resume?, signal})` | Open, authenticate, join; resolve with the `welcome` (`{root, version, text?, client, self, rights}`). `resume` is the editor's version: the host sends no `text` when it can resume from there. A rejection with a `CloseEvent` whose `retry` is false ends the editor (e.g. `declined` with `MPC-E401` when the text doesn't fit one frame); anything else is retried. Called while a connection is open, it replaces it (the fallback after a refused snapshot). |
| `send(frame)` | On the open connection. **While none is open it drops the frame**, never keeps it: the wire has no idempotency key, so a replayed `edit` could apply twice. |
| `onFrame(cb)` | Frames from the host, in order, from every connection. |
| `onClose(cb)` | The connection ended: a `CloseEvent` `{reason, retry, error?}`. |
| `close()` | The editor is done (`destroy`): the host leaves the session. |

The transport must be **one object for the editor's whole life**; the host swaps sockets
inside it.

## The connection's states

`connecting` → (welcome) → `live`. On a close with `retry: true`: `reconnecting`, a backoff
(`reconnect: {minMs, maxMs}`, default 1–15 s, doubling, jittered; `false`: a drop closes), then
`connect({resume: version, prev: client})`. Each connection has a new client ID; `prev` (the
last welcome's `client`) tells the host to leave the previous connection first (WIRE.md,
Reconnecting).

- **The welcome on a resume** (no `text`): the component calls
  `LiveText.reconnected(welcome)`: the new client ID, the caret and visibility again, and
  `since` from its version, then the edits made while disconnected. The reply settles the edit that was in flight when the socket
  went: listed, it was applied (recognised as the client's own by the client ID it was sent
  with); not listed, it wasn't, and it goes to `rejected` with `MPC-E000`, its text kept to
  re-apply. **An edit already handed to the transport is never sent again.** Someone else's
  identical change is never taken for its own.
- **An edit made while the component knows no connection is open** (after `onClose`, before the
  next welcome) is never handed to the transport (`LiveText.disconnected()` keeps it queued);
  it goes out once after the welcome, on the new connection, through the normal pending and
  ack path (a rebase or an `MPC-E111` shows as usual). Typing goes on while reconnecting.
- **The welcome without a resume** (`text` present): the component starts again from that text;
  pending changes go to `rejected`; `reload` fires.
- **`since` refused (`MPC-E112`, past the server's window):** `reload` fires and the component
  asks for a `snapshot`. If the host refuses it or doesn't answer in `snapshotTimeoutMs`
  (10 s), it connects again without `resume`.
- **Closed** (a close whose reason doesn't allow a retry, a `closed` frame, or the kit's
  `closed` roster event): the state `closed`, a banner with the reason, the editor read-only
  (the text stays, selectable), the `closed` event. No reconnect.
- **Whether to retry is the close table's** (`canRetry(reason)`, WIRE.md, Closing), never the
  transport's own `retry`: a transport that reports `auth` with `retry: true` still closes the
  editor, and `network` with `retry: false` still reconnects. A disagreement is reported once
  per reason, as an `error` event with id `MPC-EDITOR-CLOSE-RETRY` (`CLOSE_RETRY_ERROR`).
- **`online`, and the page becoming visible**, reconnect at once when reconnecting, or resync
  when live. `setVisible(v)` tells the host (`visible` frame) and does the same.
- **Over the frame cap** (4 MiB): an edit too big for one frame is refused before it is sent
  (`MPC-E113`) and goes to `rejected` with its text; a `since` reply too big is refused with
  `MPC-E112` (`details.limit: "frame"`) and the editor reloads; a text too big to send at all
  closes the editor (`declined`, `MPC-E401`, no retry).

## Options

| Option | Default | |
|---|---|---|
| `transport` | (required) | Above. |
| `root` | `source` | The text root. A welcome for another root closes the editor. |
| `mode` | `markdown` | `markdown`: Markdown highlighting and list continuation. `plain`: neither. Both wrap lines and use CodeMirror's default keymap **without history**. |
| `extensions` | | More CodeMirror extensions (not `history()`: see "The binding"). |
| `readOnly` | `false` | Never edit, whatever the rights. |
| `labels` | `active` | Remote caret names: `always`, `active` (for 2.5 s after the caret moves), `hover`. |
| `reconnect` | `{minMs: 1000, maxMs: 15000}` | Or `false`. |
| `strings` | English | Every string the component shows (`DEFAULT_STRINGS`). |
| `render.roster` | a strip | `false`: none (draw your own from `roster`); or a function. |
| `render.rejection` | a row | `false`: no list; or a function given the rejection and `{reapply, dismiss, reveal}`. |
| `render.banner` | a line | `false`: never; or a function given `{state, rights, close, text, retry}`. |
| `timers`, `snapshotTimeoutMs` | | For tests. |

## The handle

`view` (the `EditorView`), `text`, `version`, `state`, `rights`, `roster`, `rejected`,
`pending`; `on(event, cb)` (returns an unsubscribe); `undo()` / `redo()` (the server's, for this
participant); `reapply(id)` (a `ReapplyResult`: `applied`, `already-present`, `unknown`,
`unavailable`; see Rejections) / `dismiss(id)`; `reveal({first, last})` (scroll to lines and mark
them for a moment); `follow(handle | null)` (keep a participant's caret in view);
`setVisible(v)`; `reconnectNow()`; `destroy()`.

## Events

| Event | Value |
|---|---|
| `state` | `connecting` / `live` / `reconnecting` / `closed` |
| `roster` | the roster after each change |
| `rights` | `edit` / `read` / `none` (the welcome's, or a `rights` roster event for this participant); anything but `edit` is read-only |
| `change` | `{hunks, by, local}` for every change to the text |
| `pending` | the number of local changes not yet acknowledged |
| `ack` | `{ops, version}` |
| `rejected` | a `Rejection` (`LiveText`'s): the text left the editor; offer re-apply |
| `conflict` | `{rejection, error}` when the refusal was `MPC-E111` (`error.details.conflicts`) |
| `landed` | `{rejection, version}`: a change whose reply was lost (`mayHaveApplied`) landed after all; it left `rejected` |
| `duplicateRisk` | `{rejection, landed, reapplied}`: a change the person re-applied had also landed (a host breaking WIRE.md's reconnect rule): it may be in twice |
| `note` | the kit's removed-text note (`overwritten`, rules `removed_text`, `replaced_text`, `rollup`), only those whose `to` is this participant, once per `(change, row)` |
| `claim` | `claim_started` / `claim_ended` |
| `reload` | `{reason, error}`: the editor started again from the server's text |
| `closed` | `{reason, retry, code?, path?, error?}`: `retry` as the close table says; `code` and `path` from the host's `closed` frame |
| `error` | an `ErrorReport` (a refused undo, a host `error` frame, a listener that threw) |
| `participantClick` | `{handle, entry}` from the roster strip |

A note's `message` holds the person's own text: show it to them, don't log it.

## `MPC-EDITOR-HISTORY`

- **What it means:** `mountLiveEditor` refused to mount (it throws `LiveEditorError`, `code:
  "MPC-EDITOR-HISTORY"`) because the host's `extensions` include CodeMirror's history; or a
  history was added later by a reconfiguration, and the editor reported one `error` event with
  that id.
- **Why:** the server owns undo. A local history would undo outside the shared order: it
  would take changes out by its own record, not the server's. CodeMirror's history commands
  dispatch with `filter: false`, so the editor can't intercept them; a history added later
  works as the person's own edits from then on (sent like typing), never the server's undo.
- **How to avoid it:** don't pass `history()` or `historyKeymap`. Undo and redo are the
  editor's own: `undo()` / `redo()` on the handle, and `Mod-z`, `Mod-Shift-z`, `Mod-y` and the
  browser's undo and redo in the editor.

## Rights and read-only

The editor is read-only before the first welcome, without the `edit` right, with
`readOnly: true`, and once closed. A `rights` roster event for this participant changes it
live. Read-only keeps selection and copying.

## Remote carets

Each caret is a coloured bar with the participant's name. The name's text is black or white,
whichever contrasts more with the participant's colour (`--mpc-user-fg`; the host can force
one with `--mpc-label-fg`). On the first line the name goes below the caret, so it isn't cut
off. Selections are the participant's colour at `--mpc-selection-alpha`.

## Rejections

A change the server refused (or that met competing text) leaves the text and is listed below
the editor: the text that wasn't applied, **Re-apply** and **Dismiss** (buttons, reachable with
Tab and Enter), and a click on the text reveals where it was. In the text, a wavy underline or a
struck-through ghost marks the place. `reapply(id)` inserts the typed text where it was as a new
edit and returns `{status: "applied"}`, but only where that is the server's place for it (below);
`unavailable`: read-only, no such rejection, or an IME composition under way.

**A change whose reply was lost** (`MPC-E000`: the resync after a reconnect didn't list it) is
marked `mayHaveApplied: true`, and its row says so ("This change may already be in the
document.", `strings.mayHaveApplied`). On a host that follows WIRE.md's reconnect rule it truly
wasn't applied; on one that doesn't, it may land later. The client tells by the server's own
record, never by matching text:

- **It lands after all:** a change from the client ID it went out on, at a version after the
  one it was refused at, is that change (that ID had at most this edit in flight, and client IDs
  are never reused on a doc). It leaves `rejected`, the `landed` event fires, and a note says so;
  Re-apply on it returns `already-present`. An earlier, acknowledged edit from the same ID
  doesn't count, and neither does someone else's identical change (another ID).
- **Re-apply** asks `LiveText.presence`: `present` (it landed): nothing applied, the rejection
  dismissed, `already-present`; `absent` (the history covers every version since and no change
  from that ID is in it): on to the guards below, as any rejection; `unknown` (the history doesn't
  reach back that far, or an undo or redo was unanswered on that connection, whose change would
  carry the same ID): nothing applied, the rejection kept, a note asks the person to check and
  retype, `unknown`. Re-apply applies only when the record shows the change absent.
- **The duplicate-risk notice, the one case only a host following the rule prevents:** on a host
  that breaks WIRE.md's reconnect rule, the person can re-apply (the change absent then) before
  the late frame lands. When it lands, `duplicateRisk {rejection, landed, reapplied}` fires and a
  note says the text may be in twice. A host that follows the rule never applies the late frame.

**Re-apply puts text back only where the server would put it.** It re-inserts the typed text at
the start of the change's region, which is the server's place for it only when the change is a
**single pure insertion**, its region has moved through **the server's own changes** (no reload
since), **nothing else was inserted at exactly that point** since, and **no text around that
point, or at its edge, was deleted or replaced** since. Anything else applies
nothing, keeps the rejection, shows a "check and retype" note, and returns `{status: "unknown",
reason}` (`LiveText.reapplyGuard(id)` gives the reason; `LiveText.reapply` returns `null`):

| `reason` | The change | Why Re-apply would get it wrong | Note |
|---|---|---|---|
| `multi-hunk` | more than one hunk | all its text would go back in one place, not each hunk's | `strings.multiHunk` |
| `not-insertion` | a deletion or a replacement | Re-apply only re-inserts typed text: a deletion would change nothing, a replacement's text would land beside the text it replaced | `strings.notInsertion` |
| `reloaded` | made before a reload (`MPC-E112`, a divergence, a welcome with `text`), or open across one | its place moved by a diff of the texts, not by the server's changes (a change pending at the reload has no place at all) | `strings.reloaded` |
| `tie` | something else was inserted at exactly its place since (by anyone, the person included) | the server puts a later change after the one at its place; Re-apply would put it before | `strings.tie` |
| `deleted-around` | text around its place, or at its edge, was deleted or replaced since (by anyone, the person included; a change that met competing text is one) | its place snapped to the deletion's start and stays there even if that deletion is refused or undone; the server refuses an insertion inside a deletion it applied, and never saw one it refused. The edge counts because a change reaches the editor mirrored around the person's own pending text: a deletion the server sees around the place can end exactly at it here | `strings.deletedAround` |
| `cant-tell` | (a lost reply) the record can't tell whether it landed | above | `strings.cantTell` |

Presence comes first: a lost change found `present` is `already-present` whatever its shape (a
deletion someone already made included). A later version restores every change through the
server's rebase rule, refusing on a conflict.

`partly-present` is gone from `ReapplyResult` (`applied`, `already-present`, `unknown`,
`unavailable`; `reason` says why `unknown`: `cant-tell`, `multi-hunk`, `not-insertion`,
`reloaded`, `tie` or `deleted-around`). An insertion refused for competing text (`MPC-E111`) was
inside a deletion, so it is `deleted-around`: what Re-apply still applies is a lost reply found
absent, a rate limit and the like.

## Theming

CSS variables on `.mpc-editor` (the defaults in parentheses): `--mpc-bg` (`Canvas`), `--mpc-fg`
(`CanvasText`), `--mpc-font`, `--mpc-font-size` (14px), `--mpc-line-height` (1.5),
`--mpc-caret-width` (2px), `--mpc-label-fg`, `--mpc-selection-alpha` (0.25), `--mpc-rejected`,
`--mpc-banner-bg`, `--mpc-banner-fg`, `--mpc-focus`. `--mpc-user` is set inline per participant.
Stable classes: `cm-mpc-caret`, `cm-mpc-caret-name`, `cm-mpc-selection`, `cm-mpc-rejected`,
`mpc-banner`, `mpc-roster`, `mpc-rejections`; the frame has `data-state`.

## The binding (what changed from `codemirrorLive`)

- **No `doc.toString()` per keystroke.** A byte index (`ByteIndex`) keeps each line's UTF-8
  length, updates only the lines a transaction touches, and converts offsets from the line.
- **Remote changes are held, then dispatched together** outside any update, and **never during
  an IME composition**: the editor's document is left alone until `compositionend`, while the
  person's composing text still goes to the replica (mapped over the held changes). Nothing is
  lost or doubled, and the editor shows the server's text once the composition ends.
- **The server owns undo; a CodeMirror history is refused.** `mountLiveEditor` throws a
  `LiveEditorError` with code `MPC-EDITOR-HISTORY` when the host's `extensions` include
  CodeMirror's history (`historyField`). If a later reconfiguration adds one, the editor
  reports an `error` with that id, once; from then on the history's commands apply as the
  person's own edits and go to the server like typing (CodeMirror's history dispatches with
  `filter: false`, so nothing can intercept them). In every case: remote changes never enter a
  history; `Mod-z` / `Mod-Shift-z` / `Mod-y` and the browser's undo and redo (`historyUndo`,
  `historyRedo`) go to the server, ahead of any host keymap.
- If the editor and the replica ever disagree, the editor is reset to the replica's text and an
  `error` reports it (the tests fail on any such report).

## Tests

- `npm test`: the unit and convergence suites, among them `test/editor.test.ts` (the states,
  rights, carets, rejections, reconnects, `MPC-E112`, closed, history, notes, an IME
  composition) and `test/editor-convergence.test.ts` (three mounted editors through random
  typing, outside edits, undo and dropped sockets; `EDITOR_RUNS` sets the run count). Both run
  in jsdom against the fake server (`test/fake-server.ts`, `test/fake-live.ts`).
- `npm run test:browser` (Playwright, pinned): the editor in real browsers. Everything is under
  `client/` (`playwright.config.ts`, `test/browser/`). Pick browsers by project:

  ```sh
  npx playwright install chromium         # or webkit
  npx playwright test --project=chromium  # or --project=webkit
  ```

  A selected browser that can't launch fails the run; nothing is skipped for it. Tests only
  for one browser say so (`test.skip` on the browser's name).

Browsers are downloaded into the job's folder when `PLAYWRIGHT_BROWSERS_PATH` points there; a
runner without a cache downloads them on every run.

### The IME tests and their limit

`test/browser/ime.spec.ts` composes text while remote changes arrive (before, after and at the
caret, a deletion, another editor on the same line), and checks the composed text is in the
result exactly once and every editor matches the server. It prints one line per case,
`IME <browser> <driver> <case>: OK ...`. Two drivers: `synthetic` (composition events and the
composing text written into the editor's DOM, on every browser) and `cdp` (Chromium's own IME
input).

**The caveat:** Linux WebKit is WebKitGTK, not the macOS system web view (WKWebView), and
macOS input methods (Japanese, Chinese) compose differently. A WebKit pass here is an early
signal only; the real IME check is the host application's, on macOS WKWebView, once the
component is mounted there.
