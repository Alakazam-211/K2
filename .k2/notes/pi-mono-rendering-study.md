# pi-mono Rendering Study — zero-lag / zero-flicker techniques

Date: 2026-07-02 · Studied: `badlogic/pi-mono` @ `114bacf` (2026-07-02, fresh
shallow clone at
`/Users/z3thon/DevProjects/Alakazam Labs/terminal-research-repos/pi-monorepo`;
the older sibling clone `../pi-Mono` was 3 months stale). Locally installed
`pi 0.70.2` (`/opt/homebrew/bin/pi`). Author is Mario Zechner (badlogic, libGDX).
The TUI package is `packages/tui` (`@earendil-works/pi-tui`), whose README
states the thesis outright: *"differential rendering and synchronized output
for flicker-free interactive CLI applications"* (`packages/tui/README.md:1-9`).

Citations: `pi:<path>:<line>` = the pi clone above; `k2:<path>:<line>` = this
repo; `alac:<path>:<line>` = cargo registry
`alacritty_terminal-0.26.0` (what K2 pins, `k2:crates/k2-core/Cargo.toml:118`).

> **Empirical-capture caveat**: a live pty capture (claude-study style) was
> attempted and is blocked in this harness — `openpty` fails system-wide
> ("out of pty devices" / "Device not configured", even unsandboxed; no sshd
> for the ssh-localhost workaround). Unlike the closed claude binary, pi is
> OSS, so §5's wire profile is *derived from the exact emitting code* and is
> deterministic; pi even ships its own capture switch (`PI_TUI_WRITE_LOG=…`
> logs every byte written, `pi:packages/tui/src/terminal.ts:111-124,454-463`)
> for whenever a real pty is available.

---

## 1. The rendering core: an immediate-mode line-diff renderer

The whole model is ~1700 lines in one file: `pi:packages/tui/src/tui.ts`.

**Frame model.** A component is just `render(width): string[]` — an array of
fully-styled, *pre-wrapped, width-padded* strings, one per terminal row
(`pi:packages/tui/src/tui.ts:64-88`). The root `TUI extends Container`
concatenates all children's lines into one logical frame — the entire session
transcript plus editor plus footer as a single `string[]`
(`pi:packages/tui/src/tui.ts:256-290,295`). This is **immediate-mode UI over a
retained line buffer**: components re-render every frame (each caches its own
lines keyed on `(content, width)`, e.g. `pi:packages/tui/src/components/text.ts:14-16,45-58`,
`markdown.ts:119-123,152-156`), and the TUI diffs the new `string[]` against
`previousLines` — plain string equality per line
(`pi:packages/tui/src/tui.ts:1367-1394`).

**Three render strategies** (README's "three-strategy system"), chosen per
frame in `doRender()` (`pi:packages/tui/src/tui.ts:1254-1620`):

1. **Full render** — first frame, width change, height change, or when the
   first changed line has scrolled above the viewport
   (`:1336-1356,1453-1459`). Emits `?2026h` + (optionally)
   `2J`+`H`+`3J` + every line + `?2026l` (`:1284-1309`). Order matters and was
   a shipped bug: clear the *screen first*, then wipe scrollback, else stale
   scrollback survives (`pi:packages/tui/CHANGELOG.md:388`, PR #2155).
2. **Differential range render** — the common case. Compute
   `[firstChanged..lastChanged]`, move the cursor *relatively*
   (`CSI nA`/`CSI nB`, never absolute `CUP`), `\r`, then for each changed line:
   `CSI 2K` (erase line) + write the new line (`:1461-1549`). It deliberately
   renders **only the changed range, not changed-to-end**: *"This reduces
   flicker when only a single line changes (e.g. spinner animation)"*
   (`:1490-1492`).
3. **Deleted-lines-only clear** — content got shorter with no rewrites: walk
   down, `2K` the orphaned rows, walk back (`:1404-1451`).

Every strategy is bracketed in **DEC 2026 synchronized output** (`?2026h` …
`?2026l`) and flushed as **one single `write()`** (`:1286,1308`, `:1463,1570`,
`:1407,1439`, single write at `:1602`) — the terminal is never given a
half-frame to paint.

**Scrolling.** There is no alt screen and no DECSTBM scroll region anywhere in
the repo (grep: zero hits for `?1049`/`[r`). Appending lines past the bottom
scrolls the *terminal itself* with `"\r\n".repeat(scroll)` at the last row
(`:1465-1478,1488`) — history flows into native scrollback, which is why the
scrollback wheel, cmd-F, etc. keep working in pi sessions. The renderer tracks
a logical `viewportTop` and only ever differentially touches rows still inside
the visible viewport; anything above it is immutable history (`:1453-1459`).

**Resize.** Width change ⇒ full render with clear ("wrapping changes",
`:1342-1347`). Height change ⇒ full render too, **except on Termux**, where
software-keyboard toggles resize height constantly and a full redraw replays
the entire history (`:1349-1356`, `isTermuxSession` `:163-165`,
`pi:packages/tui/CHANGELOG.md:356`). Note the churn in their history: height
full-redraws were removed (`CHANGELOG.md:583`), then reinstated to fix
editor/footer drift (`CHANGELOG.md:457`, #1844) — evidence this tradeoff has
no free answer, matching K2's own resize scars
(`k2:.k2/notes/kessel-hard-learnings.md` §2.8).

**Frame pacing.** `requestRender()` coalesces via `process.nextTick`, then a
**16 ms floor**: if ≥16 ms have passed since the last paint, render *now*
(zero added latency for an idle keystroke); if inside the window, one timer
fires at the boundary and absorbs the burst
(`pi:packages/tui/src/tui.ts:306-309,712-759`). No fixed tick, no vsync — it
is *render-on-demand with a 60 Hz cap*. This exact policy was retrofitted
after streaming melted the naive version: *"coalescing `requestRender()` calls
to a 16ms frame budget while preserving immediate `requestRender(true)`"*
(`pi:packages/tui/CHANGELOG.md:307`).

**Streaming updates.** Assistant deltas call `setText()` on a cached
`Markdown` component → full re-parse of that message per frame
(`pi:packages/tui/src/components/markdown.ts:139-156`) — but the *line diff*
bounds what hits the wire to the tail lines that actually changed. A dedicated
fix trims un-terminated code fences during streaming so blocks don't
shrink/flicker mid-stream (`CHANGELOG.md:27`, #5846). The spinner is a plain
80 ms `setInterval` that calls `requestRender()`
(`pi:packages/tui/src/components/loader.ts:11-12,77-91`) — one changed line
per tick ⇒ one `2K`+rewrite of one row.

---

## 2. Zero lag: the keypress→paint path

Everything is local and synchronous except exactly one macrotask:

1. Raw-mode stdin chunk → `StdinBuffer`, which reassembles split escape
   sequences and emits complete ones (10 ms flush timeout for stragglers)
   (`pi:packages/tui/src/stdin-buffer.ts:1-18,378-386`; based on OpenTUI, credited).
2. `TUI.handleInput` → focused component's `handleInput` mutates editor state
   **synchronously**, then `requestRender()`
   (`pi:packages/tui/src/tui.ts:825-834`).
3. Idle session ⇒ the 16 ms window has long expired ⇒ paint happens on the
   next tick — sub-millisecond echo. Only mid-burst does a keystroke wait
   (≤16 ms) and it rides the coalesced frame.

There is **no predictive/optimistic rendering and no echo strategy** — there
is nothing to predict; the editor *is* the source of truth and paints itself.
The relevant lesson is the negative one: pi achieves "zero lag" by having no
asynchronous hop at all, which a remote-grid system like K2 cannot copy — K2's
echo latency is daemon-RTT-bound by architecture (and K2's measured p50 echo
is already 1.2 ms locally, `k2:` commit `abeab5a` message).

Input-side polish that *is* transferable thinking: terminal-capability
negotiation is response-driven, not timer-driven — pi sends
`CSI >7u CSI ?u CSI c` and uses the **DA response as a sentinel** ("terminal
doesn't know kitty" ⇒ fall back to modifyOtherKeys) instead of waiting out a
startup timeout (`pi:packages/tui/src/terminal.ts:17,208-250`). Same
philosophy as Kessel's "timers guess; the stream tells you"
(`k2:.k2/notes/kessel-hard-learnings.md` §2.5).

---

## 3. Game-dev fingerprints

- **Double buffering, literally.** `previousLines` (front buffer) vs
  `newLines` (back buffer), diff, swap (`pi:packages/tui/src/tui.ts:297,1616`).
  The dirty region is the changed line range — dirty-rect discipline
  degenerated to 1-D, which is the correct dimensionality for a row-oriented
  medium.
- **Immediate-mode API over retained state** — the imgui pattern: components
  re-emit their full output every frame; caching + diffing make it cheap.
- **Frame budget as a constant**: `MIN_RENDER_INTERVAL_MS = 16`
  (`pi:packages/tui/src/tui.ts:309`) — a 60 fps cap treated as a budget, with
  the "isolated event renders immediately" escape hatch.
- **Perf counters as public API + tested invariants**: `tui.fullRedraws` is a
  readonly counter (`:316,336-338`; `CHANGELOG.md:669`) and the test suite
  *asserts full-redraw counts* against a real headless **xterm.js** emulator
  (`pi:packages/tui/test/virtual-terminal.ts:1-30`,
  `pi:packages/tui/test/tui-render.test.ts:336-398` — "Height change should
  trigger full redraw" / "should NOT"). Rendering regressions fail loudly in CI.
- **Crash-loud invariants**: a rendered line wider than the terminal doesn't
  get clamped-and-forgotten — it dumps every line + width to
  `~/.pi/agent/pi-crash.log`, restores the terminal, and **throws**
  (`pi:packages/tui/src/tui.ts:1519-1547`). Width math is load-bearing
  (overflow ⇒ unwanted wrap ⇒ the diff's row addressing corrupts the whole
  screen), so it's enforced like an assert.
- **Instrumentation culture**: `PI_DEBUG_REDRAW=1` logs every full-redraw
  trigger with its reason (`:1327-1334`), `PI_TUI_DEBUG=1` dumps
  per-frame before/after/buffer triples (`:1572-1599`), `PI_TUI_WRITE_LOG`
  taps the raw output stream (`terminal.ts:111-124`) — the same "raw
  observability pays for itself" lesson Kessel learned (§2 of the kessel note).
- **Hot-path micro-optimization**: `visibleWidth` has an ASCII fast path + a
  512-entry memo cache for non-ASCII (`pi:packages/tui/src/utils.ts:44-58`);
  overlay compositing is single-pass segment extraction
  (`pi:packages/tui/src/tui.ts:1175-1224`).
- **Interpolation**: none. No animation easing, no fractional scroll — change
  arrives, frame renders. The "game feel" comes entirely from latency and
  atomicity, not motion smoothing.

---

## 4. Anti-flicker techniques, enumerated

1. **Synchronized output around every write path** — `?2026h/l` brackets full
   renders (`pi:packages/tui/src/tui.ts:1286,1308`), diff renders
   (`:1463,1570`), and delete-only passes (`:1407,1439`). The terminal
   composites each update atomically.
2. **Clear-and-repaint inside the same atomic frame** — the classic
   clear-then-paint gap (K2 fought exactly this on resize) cannot appear
   because `2K`-erase and the replacement text ship in one `?2026` bracket in
   one `write()` (`:1519-1549,1602`).
3. **One buffered write per frame** — no interleaved small writes for the
   terminal to paint between (`:1601-1602`).
4. **Diff only the changed range** — a spinner tick repaints one row, not
   spinner-to-bottom (`:1490-1492`).
5. **No-change frames write zero bytes** — `firstChanged === -1` early-outs
   (only cursor repositioning may run) (`:1396-1402`). Frame dedupe, the same
   guard Kessel needed client-side (kessel note §2.7).
6. **Hardware cursor hidden by default, parked once per frame** — `?25l` at
   start (`:641`); the *logical* cursor is embedded in rendered output as a
   zero-width APC marker `\x1b_pi:c\x07` that the TUI extracts+strips, then
   positions the real cursor with relative moves after the frame lands
   (`:114-121,1234-1252,1627-1658`). No cursor ghosting across repaints; the
   cursor is only made visible for IME (`PI_HARDWARE_CURSOR=1`).
7. **Per-line style reset** — every non-image line gets
   `SGR 0` + OSC 8 hyperlink-close appended (`SEGMENT_RESET`,
   `:1093-1104`), so a partially repainted screen can never bleed styles
   across rows (partial-line tearing via leaked SGR is a whole flicker class
   K2's DOM renderer is immune to but any TTY re-emitter is not).
8. **Lines pre-padded to exactly terminal width** — components pad with real
   spaces (`pi:packages/tui/src/components/text.ts:83-86`), so an old longer
   line is fully overwritten even without `2K`, and the width-crash guard
   (§3) guarantees no line ever wraps and shears the row addressing.
9. **Relative cursor movement only** — the diff path never homes to `1;1`
   (which would flash on terminals that repaint on CUP); it walks
   `A`/`B`/`\r` deltas from the tracked `hardwareCursorRow` (`:1481-1488`).
10. **Full-clear ordering** — `2J` then `H` then `3J` (`:1289`), the
    #2155 lesson: wipe visible content before nuking scrollback.
11. **Shrink tolerance over redraw** — when content gets shorter,
    `clearOnShrink` (default **off**, `:313,1358-1365`) chooses leaving stale
    blank rows over a full redraw; empties are cleared surgically by strategy
    3 instead. On slow/remote terminals a full redraw *is* the flicker.
12. **Resize is the one accepted full flash** — width change invalidates all
    wrapping, so it redraws everything, atomically, under `?2026` (`:1342-1347`).
13. **Unicode-width defense** — a partial flag emoji (regional indicator)
    arriving mid-stream once caused wrap drift + stale-character artifacts in
    the differential renderer; fixed in width measurement
    (`CHANGELOG.md:469`). Width correctness *is* flicker correctness in a
    line-diff design.

---

## 5. What pi looks like TO a hosting terminal (i.e., to K2)

Derived from the emitting code (see caveat up top). Session shape, in order:

**Startup** (`pi:packages/tui/src/terminal.ts:134-167`,
`pi:packages/tui/src/tui.ts:635-647`):
- raw mode; `?2004h` (bracketed paste); self-`SIGWINCH` to refresh dims;
- kitty keyboard negotiation `CSI >7u` + `CSI ?u` + `CSI c` (DA sentinel);
  falls back to xterm `modifyOtherKeys` (`CSI >4;2m`) if DA answers first
  (`terminal.ts:17,220-250,320-330`);
- `?25l` (cursor hidden — stays hidden for the whole session unless IME);
- optional `?2031h` (color-scheme change notifications, `tui.ts:642-644`);
- `CSI 16 t` cell-size query **only when an image protocol is detected** via
  env (kitty/iTerm2/WezTerm/Ghostty; inside a K2 pane none match ⇒ never sent)
  (`tui.ts:677-685`, `terminal-image.ts:66-117`);
- the coding agent additionally queries **OSC 11 `?`** (background color, for
  theme detection, `tui.ts:1665-1686`) and **`CSI ?996n`** (color-scheme DSR,
  `tui.ts:1693-1713`), both promise-with-timeout.

**Per frame**: one write of `?2026h` + relative cursor moves + per-changed-line
`2K`+content (each line ending `SGR 0` + OSC-8 close) + `?2026l` (§1). Appends
scroll via `\r\n` at the bottom row ⇒ **history lands in the host's real
scrollback**. Full redraws (resize, viewport escape) emit `2J H 3J` — i.e.
**pi wipes the host pane's scrollback and rewrites the entire transcript** from
its component model (that's why it keeps every message component alive).

**Also emitted**: OSC 0 title (`terminal.ts:504-507`); OSC 8 hyperlinks;
**OSC 9;4 progress** (indeterminate while the agent works, re-asserted every
1 s, cleared on stop — `terminal.ts:11-13,509-530`); **OSC 133 A/B/C zone
marks around each assistant message** (`pi:packages/coding-agent/src/modes/interactive/components/assistant-message.ts:5-7,72-81`);
kitty graphics APC only if images were detected.

**Never emitted**: alt screen (`?1049`), any mouse mode
(`?1000/?1002/?1003/?1006` — zero hits repo-wide), DECSTBM scroll regions,
OSC 52. **pi is the opposite of claude-fullscreen** in the DECSET matrix of
`k2:.k2/notes/tui-mouse-interaction-study.md` §1.1 — it's an inline app like
claude's `tui:default` mode, permanently.

**Exit**: cursor moved below content + `\r\n`, `?25h`, `?2004l`, kitty pop
`CSI <u`, modifyOtherKeys off, progress cleared, then **stdin drained ~50 ms**
so late kitty key-release events can't leak to the parent shell over slow SSH
(`tui.ts:687-710`, `terminal.ts:368-452`).

**Consequences for K2 hosting pi** (all verified against K2 code):
- pi's `?2026` brackets are honored *inside* K2's Term: alacritty's event loop
  suppresses Wakeup while all processed bytes are within a sync block and
  applies the batch at ESU (or safety timeout)
  (`alac:src/event_loop.rs:165-166,228-246`). So pi's atomic frames arrive at
  K2's emitter as frame-aligned damage — **pi under K2 is flicker-free by
  construction**, then K2's own 16 ms coalescing stacks on top.
- pi never captures the mouse ⇒ K2 native selection/scrollback fully apply;
  the SGR-forwarding work from the mouse study is claude-fullscreen-scoped and
  irrelevant for pi.
- pi pre-wraps and space-pads every line ⇒ alacritty's `WRAPLINE` never sets
  (same verdict as claude, mouse study §1.3): grid-side logical-line rejoin
  can't work, and unlike claude there's no app-side OSC 52 copy path — native
  grid selection is the only copy for pi content. Themed lines (bg-styled
  padding) will carry trailing styled spaces into copies.
- **K2 answers no terminal queries at all**: `grep PtyWrite crates/` = zero.
  alacritty_terminal *generates* the responses (DA, DSR/CPR, kitty-keyboard
  reports, DECRPM, XTWINOPS) as `Event::PtyWrite(String)`
  (`alac:src/event.rs:40`; emit sites `alac:src/term/mod.rs:1262-1342,2090-2270`)
  but K2's `DaemonEventListener` just broadcasts events
  (`k2:crates/k2-core/src/terminal/daemon_pty.rs:110-123`) and every consumer
  ignores non-Wakeup (`k2:crates/k2-daemon/src/grid_emitter.rs:183-186`; v1
  swallowed too, `k2:crates/k2-core/src/terminal/alacritty_backend.rs:37-57`).
  Hosted pi therefore never gets its DA sentinel (kitty negotiation silently
  unresolved), claude's `CSI ?6n` goes unanswered, and pi's OSC-11/996 theme
  probes time out. See learning C1 below — the single best hosting fix found
  by this study.

---

## 6. The verdict — transferable learnings, ranked by feel-impact/effort

### First, the honest non-transfers

pi paints a **local TTY it fully controls, from the app's own model**; K2
renders a **remote, app-agnostic grid in a webview**. Different physics:
- `?2026` brackets, `2K`-erase discipline, relative-cursor walking, cursor
  parking — wire techniques for talking *to a terminal*. K2's DOM/WebGL
  renderers own every pixel and already apply frames atomically per rAF
  (`k2:src/renderer/terminal-v2/TerminalPane.tsx:126-131,557-591`); there is
  no terminal on the other side to flicker.
- Zero-lag-by-synchrony (§2): unreachable for a daemon-remote echo path.
- Full-redraw-on-width-change: K2's daemon already reaches the same
  conclusion (resize ⇒ snapshot; clear gated on real dim change,
  `k2:crates/k2-core/src/terminal/daemon_pty.rs:692-735`).
- pi has **no backpressure story** (local stdout can't fall behind the way a
  WS can) — nothing to steal for K2's ack/flow-control roadmap.
- One *convergence* worth recording, not porting: pi's pacing
  (immediate-when-idle, 16 ms-coalesce-when-bursting,
  `pi:packages/tui/src/tui.ts:741-759`) is **exactly** the policy K2's shared
  grid emitter landed on independently
  (`k2:crates/k2-daemon/src/grid_emitter.rs:122-132,137-204`) — and both
  projects retrofitted it after the naive per-event version melted under
  streaming (`pi:CHANGELOG.md:307` vs `k2:` commit `abeab5a`). Two codebases,
  same scar, same fix: strong evidence the current K2 cadence is right.

### (c) For K2 as a HOST of pi/claude-class TUIs — highest value

**C1. Answer terminal queries: plumb `Event::PtyWrite` back to the PTY.**
*Impact: high (correctness for every hosted TUI); effort: S (half-day).*
K2 currently drops all query responses alacritty generates (§5). Sketch: in
`k2:crates/k2-core/src/terminal/daemon_pty.rs` spawn a tiny per-session task at
creation (next to the event-loop spawn, ~`:502-580`) that subscribes via
`subscribe_events()` (`:806-808`) and on `AlacEvent::PtyWrite(text)` writes
`text` to the pty notifier (same path `session.write()` uses). While there,
answer `AlacEvent::ColorRequest` with the K2 theme's palette (unblocks pi's
OSC-11 theme detection and claude's future equivalents) — keep
`Osc52 = OnlyCopy` semantics per the mouse study. Add a scratch-daemon test:
spawn `printf '\x1b[c'`-style child, assert the reply bytes land on the pty.
This is the direct sibling of the mouse study's OSC-52 slice and could ship in
the same PR series.

**C2. Regression-pin the `?2026` atomicity property.**
*Impact: medium (protects the pi/claude rendering story); effort: S (hours).*
The study *proved* sync-deferral in the vendored crate
(`alac:src/event_loop.rs:165-246`) but K2 has no test asserting it survives an
alacritty upgrade. Add to `k2:crates/k2-daemon/tests/grid_emitter_integration.rs`:
feed `BSU + paint-half + ESU + paint-rest` through a session, assert exactly
one frame (no half-frame delta) reaches a subscriber.

**C3. Surface OSC 9;4 progress + OSC 133 zones (someday).**
*Impact: nice-to-have; effort: M.* pi emits both (§5). Progress → session-tab
activity/progress indicator (alacritty won't event unknown OSCs, so this needs
the same event-sink extension family as C1 if alacritty exposes it, else a
deliberate non-goal — v2 has no byte tap by design,
`k2:crates/k2-core/src/terminal/mod.rs:7-14`). OSC 133 aligns with the
existing kessel-note follow-up ("copy-a-command-block UX").

### (b) For K2's daemon emitter / wire pacing

**B1. Keep the current cadence — it's independently validated (see
convergence note above).** No change.

**B2. Emit-side frame counters + emulator-backed budget tests.**
*Impact: medium (keeps smoothness from regressing); effort: S/M (~1 day).*
pi's `fullRedraws` counter + xterm-headless test harness
(`pi:packages/tui/test/virtual-terminal.ts:1-30`,
`pi:packages/tui/test/tui-render.test.ts:336-398`) is the discipline K2 lacks:
K2 has the mechanism (16 ms pacing) but no test that a 1000-Wakeup burst
produces ~≤ burst_ms/16 frames, or that steady-state deltas never regress into
snapshots. Sketch: counters on `k2:crates/k2-daemon/src/grid_emitter.rs`
(`frames_emitted`, `snapshots_emitted` — snapshot = K2's "full redraw") +
assertions in `grid_emitter_integration.rs`; renderer twin: the dev FPS stats
overlay counts React commits per burst.

**B3. A pi-tui-style TTY diff painter for K2's own CLI surfaces.**
*Impact: medium-high (new capability); effort: M (multi-day).*
Where pi-tui transfers *wholesale* is any place K2 paints into a real TTY:
a future `k2 terminal view <session>` CLI attach, companion SSH views, or the
`k2 talk` conversational UI. pi-tui is the exact recipe: grid rows → padded
strings → line diff → relative-move + `2K` rewrites inside `?2026`, 16 ms
pacing, `2J H 3J` ordering on full redraws, per-line SGR reset (§4 items
1-2,7,9-10). Port as a small Rust module (`tty_diff_painter`) consuming the
existing `GridFrame`s. This also implements the kessel-note follow-up "DCS
2026 emission on K2's own wire" for the CLI consumer case.

### (a) For K2's own renderers (DOM + WebGL painter)

**A1. Adopt the "changed-range only + no-op frame = zero work" invariant in
the WebGL painter.** *Impact: medium; effort: folds into the painter build.*
The DOM path already has both (row `React.memo`,
`k2:src/renderer/terminal-v2/TerminalPane.tsx:224`; `build_emit` Skip,
`k2:crates/k2-daemon/src/sessions_grid_ws.rs:445-450`). The painter brief
(`k2:.k2/notes/webgl-painter-brief.md`) should inherit pi's two rules
explicitly: (i) damaged rows update bg+glyph instances in the same frame —
never a clear pass the compositor can observe alone (§4 item 2, the same bug
class as K2's resize clear-then-repaint); (ii) an empty-damage frame must not
touch the GPU at all (pi `:1396-1402`).

**A2. Crash-loud width/geometry invariants + reason-logged fallbacks.**
*Impact: low-medium; effort: S.* pi throws with a full dump when a line
exceeds width (`pi:tui.ts:1519-1547`) and logs every full-redraw *reason*
(`PI_DEBUG_REDRAW`, `:1327-1334`). K2 equivalent: debug-assert CellRun row
width ≤ cols at emit time (`k2:crates/k2-core/src/terminal/grid_snapshot.rs`)
and log the trigger whenever the renderer falls back from delta to snapshot
apply — turns "it flickered once" reports into one-session diagnoses, the same
observability lesson Kessel already taught (`KESSEL_RAW`).

**A3. Steal the shrink-tolerance default.** *Impact: low; effort: trivial.*
pi defaults `clearOnShrink` **off** — stale blank rows beat a full repaint
(`pi:tui.ts:313,1358-1365`). K2 analog: when a TUI shrinks its content, prefer
letting rows go blank via normal damage over any snapshot-level reset;
never "refresh to be safe" (this is also kessel §2.6/§2.8 restated from the
other side).

### Top-3 summary (impact × effort)

| # | Learning | Where | Effort |
|---|---|---|---|
| 1 | **C1** PtyWrite/ColorRequest query-response plumbing | `daemon_pty.rs` | S (half-day) |
| 2 | **B2** frame-count metrics + emulator-backed budget tests | `grid_emitter.rs` + tests | S/M (1 day) |
| 3 | **B3** pi-tui-style TTY diff painter for CLI attach views | new `k2-core` module | M (days) |

with **C2** (pin `?2026` atomicity in a test) as the cheap rider on #2, and
**A1** as a design requirement folded into the WebGL painter build.
