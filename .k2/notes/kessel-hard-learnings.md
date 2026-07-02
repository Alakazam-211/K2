# Kessel hard learnings — distilled for the terminal-smoothness build

**Distilled:** 2026-07-02, from the Kessel R&D repo
(`/Users/z3thon/DevProjects/Alakazam Labs/Kessel`, read-only) plus K2's own
Kessel-v1 postmortems (`.k2/prds/kessel-research-archive.md`,
`.k2/prds/kessel-resize-architecture-notes.md`, `.k2/prds/kessel-t1.md`).
Audience: sessions working the smoothness roadmap (binary wire + ack resync,
passive scale-to-fit viewing, WebGL2 painter) on the daemon-authoritative
alacritty grid. Kessel citations are `Kessel/<path>`; K2 citations are repo-relative.

---

## 1. What Kessel was

An R&D repo exploring a terminal session model that decouples screen width
from the byte stream: cooked/line-mode output kept as a **width-free
logical-line model** so each of N viewers reflows one live session to its own
width, with alt-screen/TUI falling back to a **single canonical grid + passive
clipped/letterboxed views** (`Kessel/README.md:24-41`). It was the second
attempt: K2 had already built and retired "Kessel v1 / Kessel-T0" (byte
broadcast + per-viewer `alacritty_terminal::Term`) in 0.34–0.39. Kessel-the-repo
got a working v0 (four goal criteria met, real vim + real claude verified,
`Kessel/docs/progress.md`) and then stopped at the honest frontiers it documented.

---

## 2. Hard learnings

### 2.1 The irreducible PTY constraint (the founding failure)

K2 v1 broadcast one PTY byte stream to N viewers, each with its own Term at its
own size. It failed for reasons that are kernel invariants, not bugs
(`.k2/prds/kessel-research-archive.md:106-148`, restated `Kessel/docs/design.md:3-30`):

1. **Layout is computed before the bytes reach you.** By the time the daemon
   reads the master FD, the line discipline + the app have already wrapped,
   padded, and positioned output for the PTY's one current width. You are
   reading *pre-rendered* output.
2. **A PTY has exactly one size.** `TIOCSWINSZ` sizes *the child*; there is no
   per-subscriber PTY view.
3. **Reflow works for flowing text, not positioned UI.** alacritty can re-wrap
   paragraphs; it cannot reflow a box drawn at col 80. A narrower receiver
   truncates or wraps into the next row → cascading artifacts.

Observed symptom, verbatim: two viewers at different widths = "a glitch reel:
prompt bars stacked two-high, claude's TUI banner duplicated and interleaved
with scrollback, padding chars consuming half the visible cells"
(`.k2/prds/kessel-research-archive.md:131-134`).

The compressed lesson (the north star of both projects):
> **Don't re-render someone else's pre-rendered output. Render from a
> source-of-truth that hasn't been laid out yet.**

### 2.2 "THE WALL": even cooked mode is not safely reflowable

Kessel's cleanest negative result. `ls -C /` in *cooked* mode (no alt-screen)
queried winsize and columnated to the canonical width; reflowing to a narrower
viewer only soft-wrapped the already-committed columns → shredded layout
(`Kessel/docs/progress.md:16-18`). Conclusion: **the reflowable/unreflowable
frontier is "did the app query width," not "is it alt-screen."** Any app that
calls `ioctl(TIOCGWINSZ)` — column output, progress bars sized to width,
right-aligned status — is width-committed even in line mode. The cooked/TUI
split therefore leaks: real streams interleave prompt redraws, CR-overwrite
spinners, and cursor-addressed output *without* DECSET 1049, and the
classifier for that was never built (`Kessel/docs/progress.md:91-93`,
`Kessel/docs/design.md:96`). Width-free reflow was only ever proven lossless
for **owned/intent content** (Kessel's own transcript + input-box model) and
plain `printf`-style output.

### 2.3 Mode transitions must be handled in the parsed action stream, not by byte-scanning

First cut detected alt-screen with a whole-chunk byte scan, setting the final
mode before replaying actions. A transient enter→paint→exit inside one `read()`
leaked `TUI PAINT` into the cooked transcript. Fix: handle the DECSET/DECRST
transition **inline in the action stream**, which is also robust across read
splits (`Kessel/docs/progress.md:24-28`; regression test
`Kessel/crates/kessel-mux/src/capture.rs:243-248`). General rule: any
mode/state that changes rendering semantics must be sequenced with the events
it brackets — same family as K2's APC-on-the-byte-stream lesson (2.8).

### 2.4 Never hand-roll VT handling; toy grids fail on real apps

Kessel's hand-rolled alt grid (CUP + erase + print only) rendered real Claude
Code **completely blank** — claude positions with relative motion (`CUD`,
`CHA`, `CUF`) + truecolor SGR, none of which the toy grid placed. Diagnosed
only via a raw-byte dump (`KESSEL_RAW`); fixed by replacing the toy grid with
`alacritty_terminal` fed via `Processor::advance` — explicitly "reusing K2's
v0.35–0.36 pattern" (`Kessel/docs/progress.md:52-60`). This independently
re-validates K2's current architecture: the emulator must be a real one, and
consumers (renderers) must be pure consumers of its grid.

### 2.5 Settle-on-silence is a broken primitive; synchronized output (DCS 2026) is the real frame signal

Three separate bites:
- "Drain until quiet" **never terminates** against a continuously-repainting
  app (`Kessel/docs/progress.md:36-38`) — first as an observe hang, again as
  the resize-cycle limit (`:69-70`).
- A conservative 150 ms settle wait made the whole pipeline ~3 fps even though
  the app's actual repaint was **~2–5 ms** (`Kessel/docs/progress.md:66-68`).
- The fix that worked: **completion detection** — return the instant a frame is
  complete: synchronized-output END (`ESC[?2026l`, used by Ink/claude), else a
  short output-quiet (5 ms), bounded by a hard per-frame budget (16 ms) so fps
  stays predictable even for continuously-animating apps
  (`Kessel/crates/kessel-mux/src/lib.rs:311-347`,
  `Kessel/docs/progress.md:81-87`). Result: ~5–6 ms/frame, ~100 fps per viewer
  vs ~3 fps with the timer.

Learning: **timers guess; the stream tells you.** Frame boundaries exist in the
protocol (DCS 2026, and OSC 133 for block boundaries) — use them, and always
pair with a hard budget so a hostile/animating app can't starve the loop.

### 2.6 A no-op resize is not free — guard it

A `Claim` at the *current* size still cleared the grid and sent SIGWINCH; a TUI
that (correctly) doesn't repaint on an unchanged winsize then showed **blank**.
This masked vim's paint in the first observe run. Fix: skip resize when the
size is unchanged (`Kessel/docs/progress.md:45-48`). Corollary for K2: never
send `TIOCSWINSZ`/repaint-triggering work for size-unchanged claims or
attach-time re-asserts; you cannot count on the app to repaint.

### 2.7 Clip/letterbox for grids: 1:1 row mapping, never re-wrap

The only lossless treatment of a width-committed grid at a different viewport:
**letterbox** when the viewport is wider (natural width, pad right),
**clip** with an explicit truncation marker (`›`) when narrower; each grid row
maps to exactly one display row, never soft-wrapped
(`Kessel/crates/kessel-core/src/lib.rs:190-208`, verified with real vim at
80→40, "every row exactly 40 cells, non-corrupting",
`Kessel/docs/progress.md:40-44`). Re-wrapping a positioned row is what produces
the cascade artifacts of 2.1(3). Also: the client must **dedupe identical
frames** so re-sends don't flicker (`Kessel/docs/progress.md:78`).

### 2.8 The K2-v1 scar log: racing resize paths, wipe hacks, whack-a-mole

From `.k2/prds/kessel-resize-architecture-notes.md`:
- alacritty `term.resize()` **pulls scrollback rows back into the live grid**
  when growing rows; content painted at an earlier width survives in those
  rows, and the app's SIGWINCH repaint lands on top → "stacked paint" +
  cell-level interleaving (`:66-72`).
- The `CSI 2J` wipe hack (commit `cbb8a30f`) fixed stacked paint but blanked
  the terminal on workspace return (fresh Term + idle app = no repaint ever
  arrives) (`:73-84`). "Don't add more wipe hacks. The root cause is resize
  racing; wipes are whack-a-mole" (`:168`).
- **Three unserialized resize paths raced** (ResizeObserver → Term, APC
  grow_boundary, daemon). The correct shape: one path, serialized with the
  byte stream so app repaint bytes land in the already-resized Term (`:105-135`).
  APC injection was "a serialization tool, not a fix … correct engineering,
  just not solving the right problem" (`.k2/prds/kessel-research-archive.md:379-383`).
- **Measure-before-attach** kills the initial resize storm: create the Term at
  the real measured container size so the first ResizeObserver fire is a no-op
  (`:95-104`).
- Grow-then-shrink / `grow_boundary` APC / canonical-wide-then-reflow-down: all
  "heroic patches on the wrong premise"
  (`.k2/prds/kessel-research-archive.md:150-153`). Note kessel-pty still
  carried the same trade-off knob — a wide canonical width keeps lines
  reflowable-down but width-querying apps then lay out against it, not the
  viewer (`Kessel/crates/kessel-pty/src/lib.rs:28-33`). There is no free width.

### 2.9 Resize-cycling one shared child across widths: works, but it's a state-mutating trick

Kessel's boldest result: in alt-screen with 2+ distinct viewer widths, rotate
the ONE child's winsize across the distinct widths, capture each repaint into a
per-width alacritty grid, serve each viewer its width's grid — the app's *own*
layout engine runs per width; cost scales with distinct widths, not viewer
count; ~100 fps/viewer with completion detection
(`Kessel/docs/progress.md:62-87`, controller at
`Kessel/crates/kessel-mux/src/lib.rs:349-467`). Required machinery to make it
livable: focus-follows-typist (render only the focus width while it's
emitting, so typing never "hops"), lazy refresh of non-focus widths only when
focus goes quiet AND something changed, discard the stale byte tail before
each resize, restore focus width after a refresh pass.

Why K2 must NOT do this to real user sessions: it multiplies SIGWINCH into the
one real process (apps can react to resize with side effects — layout resets,
scroll-region churn, redraw cost), it assumes repaint-on-SIGWINCH (not
guaranteed, see 2.6), continuously-emitting apps break the cycle, and the
"frames" between rotations are transiently wrong-width on the wire. Kessel
itself reserved N-instance-per-width for read-only monitors because it "forks
the world" (`Kessel/docs/progress.md:70-71`).

### 2.10 The 95% lesson

"Workarounds that work for 95% of users are worth shipping … The lesson is to
recognize the 95% solution *earlier* — we spent a lot of cycles on wipe hacks
and grow-shrink protocols before accepting it"
(`.k2/prds/kessel-research-archive.md:385-390`). The active-viewer model
(last-active-wins + SIGWINCH repaint on claim swap) is that solution and is
still the production shape of `daemon_pty.rs`.

### 2.11 What survived into K2 (the mapping)

- **LineMux** (`crates/k2-core/src/term/line_mux.rs:1-27`) is the WezTerm-style
  width-free line model from the v1 era — retired from the grid/render path,
  kept only for awareness/ingress consumers (replay-ring `Frame::Text` reads in
  `crates/k2-daemon/src/terminal_routes.rs`, recognizers). Its own header
  documents the deferred hard parts: no SGR, no `\r` overwrite semantics — i.e.
  the exact places the width-free model leaks (2.2).
- **Kessel-T0 retirement** is recorded at `crates/k2-core/src/terminal/mod.rs:7-14`:
  the v2 `daemon_pty` path is deliberately "no LineMux, no byte broadcast, no
  ring, no APC" — each of those negations is a scar from this document.
- **grid_snapshot/CellRun** was lifted from the Kessel-era Tauri-side
  `kessel_term.rs` into the daemon (`.k2/prds/alacritty-v2.md` Phase A2;
  `crates/k2-core/src/terminal/grid_snapshot.rs:10-15`). The current
  architecture *is* the postmortem's conclusions, productized:
  daemon-authoritative Term, per-subscriber reflow declared "architecturally
  impossible at the byte layer" (`.k2/prds/alacritty-v2.md:109-110`).

---

## 3. Direct implications for the current smoothness build

### Wave 2 — binary wire + ack resync

- **Frame boundaries: adopt DCS 2026 (synchronized output) as the coalescing
  unit** where present. Kessel measured the difference between guessing
  (150 ms settle, ~3 fps) and knowing (`?2026l`, ~5 ms, ~100 fps) — see 2.5.
  Claude Code / Ink emit it today. Coalesce daemon deltas to sync-END
  boundaries when available, quiet-window otherwise, always under a hard
  per-frame budget so continuously-animating apps can't starve the loop or
  the ack window.
- **Resync must be epoch-aware around resize.** Kessel had to "discard any
  stale tail from the previous width before we resize"
  (`Kessel/crates/kessel-mux/src/bin/kessel-cycle.rs:128-132`). The K2
  equivalent: a resize bumps an epoch; deltas from the old geometry in flight
  must be droppable by the client, and the ack/resync handshake must not
  block on frames the daemon will never re-send. The wipe-hack saga (2.8) is
  what happens when old-geometry paint meets new-geometry state unserialized.
- **Idempotent dedupe on the client** — Kessel's `run` client deduped identical
  frames to stop re-send flicker (2.7). With ack-triggered re-sends and
  snapshot resyncs, an identical-frame check (cheap hash/seq compare) prevents
  visible churn.
- **Don't build flow control on silence.** "Quiet" is not a signal that the
  producer is done (2.5); acks must be tied to explicit frame ids
  (Guacamole-style, per `terminal-smoothness-rnd.md` §5), never to
  output-has-paused heuristics.
- Kessel's v0 wire (full JSON `SessionModel` snapshot broadcast on every
  keystroke, `Kessel/crates/kessel-mux/src/lib.rs:86-101`) is the
  anti-pattern the binary delta wire replaces — fine for a prototype,
  unbounded in production.

### Wave 3 — passive scale-to-fit viewing

Kessel's letterbox/clip fallback is the same idea, and it validates the wave's
core bet: **for a non-active viewer, the only lossless options for a
width-committed grid are scale, letterbox, or clip — never reflow** (2.7,
2.1(3), and the prior-art note that sshx's "different sizes" is CSS zoom, not
reflow, `Kessel/docs/research/prior-art-survey.md:20`). Specifics to carry:
- Keep the mapping strictly 1:1 (grid row → display row). Any re-wrapping of
  the active viewer's grid for the passive viewer re-opens the glitch reel.
- If clipping instead of scaling at any point, mark truncation explicitly
  (Kessel's `›`) so users don't mistake a clipped row for the full row.
- **Claim-swap hygiene:** on takeover, expect one frame of wrong-size content
  before the SIGWINCH repaint lands (documented as inherent,
  `.k2/prds/kessel-research-archive.md:248-249`); and **skip the resize
  entirely when the claimed size equals the current size** — the app will not
  repaint and forcing state churn can blank it (2.6).
- Dedupe identical frames on the passive side; scaled viewers otherwise
  flicker on re-broadcasts (2.7).
- Kessel also had per-terminal font auto-shrink to the owner's grid sketched in
  K2 (`.k2/prds/multi-user-presence-and-claiming.md:84-86`) — same family;
  CSS-scale is the simpler, glyph-metric-free version.

### Wave 4 — WebGL2 painter

Kessel's renderer experience is thinner (its client was a TUI viewer), but
three learnings transfer:
- **The painter must be a pure consumer.** Every time Kessel let non-emulator
  code interpret VT (the toy grid, the byte-scan mode detector), a real app
  broke it (2.3, 2.4). The WebGL2 painter should consume daemon
  CellRun/grid deltas exclusively — zero ANSI knowledge client-side.
- **Don't defer SGR/style fidelity.** Both LineMux ("SGR → Phase 2",
  `line_mux.rs:25`) and Kessel ("SGR styling dropped … TODO",
  `Kessel/docs/progress.md:90-91`) deferred styles and never got them; the
  style-across-rewrap problem was a listed risk that stayed unpaid
  (`Kessel/docs/design.md:97`). The painter's atlas/instance format should
  carry full fg/bg/attrs from day one.
- **Raw observability pays for itself.** `KESSEL_RAW` (dump raw child bytes)
  turned "claude renders blank" from a mystery into a one-session diagnosis
  (`Kessel/docs/progress.md:48-56`). Keep an equivalent tap (raw PTY bytes
  and/or wire-frame dump) available while building the painter.

### The daemon-authoritative single-grid model generally

Proven dead ends — do **not** re-attempt, however tempting:
1. **Byte broadcast + per-viewer emulators** (per-viewer reflow of a live PTY
   stream). Structurally impossible; twice-confirmed (2.1). This includes any
   "just run N Terms at N sizes off one PTY" shortcut.
2. **Width-free logical-line rendering as the live grid path.** Even cooked
   mode is width-committed the moment an app queries winsize (2.2); the
   cooked/TUI classifier needed to make it safe was never built and is fragile
   by nature (CR spinners, prompt redraws, cursor addressing sans alt-screen).
   LineMux stays awareness/ingress-only.
3. **Resize-cycling the shared child to serve multiple widths** of real user
   sessions (2.9). Legitimate only as an offline/read-only capture trick.
4. **Wipe/clear hacks to mask resize races** (2.8). Fix ordering, not pixels.
5. **Grow-then-shrink / wide-canonical-width schemes** — the width trade-off
   just moves (2.8, `kessel-pty` note).
6. **Timer-based settle/backpressure heuristics** (2.5).

---

## 4. Open ideas worth revisiting someday (explicit non-goals now)

- **Kessel v2 / T1 — the JSONL semantic viewer.** Render claude-class sessions
  from the structured stream (`--output-format stream-json`) instead of the
  PTY: true per-viewer layout because nothing was laid out upstream. The
  postmortem's "right answer," fully sketched
  (`.k2/prds/kessel-research-archive.md` Layer 3, `.k2/prds/kessel-t1.md`).
  Additive to the PTY path; claude/agent-specific; schema-drift risk.
- **Resize-cycle as a read-only snapshot service** — per-width TUI captures for
  mobile glance/preview surfaces, using `capture_frame`-style completion
  detection (2.9). Never wired to live interactive sessions.
- **OSC 133 block boundaries** for prompt/command/output segmentation —
  metadata for awareness, copy-a-command-block UX, and smarter coalescing
  (`Kessel/docs/design.md:60`, prior-art survey).
- **DCS 2026 emission on K2's own wire** — daemon brackets its delta batches in
  synchronized-output semantics for downstream consumers (the inverse of 2.5).
- **Owned-content reflow** — Kessel's `Block::Input`/intent model works when we
  own the widget (`Kessel/crates/kessel-core/src/lib.rs:109-135`); relevant if
  K2 ever draws its own input box or renders agent prose outside the grid
  (Companion).
- **N-instance-per-width read-only monitors** — "forks the world," reserved in
  Kessel for non-interactive mirrors (`Kessel/docs/progress.md:70-71`).
