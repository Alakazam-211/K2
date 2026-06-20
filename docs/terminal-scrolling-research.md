# Terminal Scrolling Research: Making K2SO's Terminal Buttery Smooth

> **Provenance**: Captured from a dedicated research session (transcript
> `15e5659a-5c01-45e3-973b-273c8a942e44`). The driving question:
> *"When I scroll in the terminal windows, how can we make it buttery
> smooth? Zed is buttery smooth and uses Alacritty, but we also have to
> push ours across a WS. Video games do this too and they're buttery
> smooth."* Three subagents ran in parallel: (1) analyze Zed's terminal,
> (2) analyze K2SO's current terminal pipeline, (3) research smooth-scroll
> techniques + reference repos across the industry. This doc is the
> consolidated output.
>
> **Companion doc**: [`terminal-rendering-research.md`](./terminal-rendering-research.md)
> covers rendering *correctness* (selection, URLs, TUI, IME). This doc is
> specifically about *scroll/throughput smoothness*.

---

## TL;DR — the headline finding

**Our backend is already doing the right thing, and our frontend is
throwing the advantage away.**

- **Daemon side** (`crates/k2so-daemon/src/sessions_grid_ws.rs`,
  `k2so-core/.../daemon_pty.rs`): we already run `alacritty_terminal` —
  the *same* core Zed uses. We get its ring-buffer grid, O(1) display
  offset, and damage tracking for free. We even send damage-based JSON
  **deltas** (only changed rows + appended scrollback), which is good.
- **Frontend side** (`src/renderer/terminal-v2/TerminalPane.tsx`,
  ~2200 lines): a **custom DOM renderer** — not xterm.js, not canvas,
  not WebGL. Each visible row is a `<div>`, each style-run a `<span>`
  with inline styles. Scroll is handled by a **50ms `setTimeout`
  debounce** that triggers a **full React re-render of every visible
  row** on every scroll tick.

So we mirror Alacritty's beautiful O(1) grid into JavaScript and then
re-render it with React DOM diffing on each wheel event. **That's the
jank.**

The throughline from every fast terminal (refterm, Kitty, Alacritty,
VS Code): **smoothness is a pipeline problem, not a draw-speed problem.**
Do less work per frame (scroll by offset, redraw only damage, cache
glyphs) and never couple unrelated clocks (PTY read ≠ network arrival ≠
render frame).

---

## Strategic options (filtered by the daemon-authoritative constraint)

**Hard constraint** (from the session): the authoritative terminal
session — `alacritty_terminal` grid + scrollback + PTY — **must stay
owned by the Rust daemon**. This is what enables one-session-many-windows
and K2 Connect (remote daemons). The test for any option: *does it keep
the terminal **emulator state** (not just the PTY process) in Rust?*

### ✅ Option A — Patch the custom DOM renderer (days)
Keep our architecture; frontend stays a thin mirror of the grid deltas we
already send.
- Swap the 50ms `setTimeout` for `requestAnimationFrame`.
- Scroll via `transform: translateY()` on a static row layer instead of
  re-creating divs.
- Fix the snapshot-churn memo dependency.
- Replace `scrollback.concat` with a ring buffer.
- Add ~8ms coalescing on the daemon.

~80% of the win for typical use. Ceiling is still DOM. **Sessions stay
in Rust. ✓**

### ❌ Option B — xterm.js + `@xterm/addon-webgl` (ELIMINATED)
xterm.js is itself a *full terminal emulator written in JavaScript* — you
feed it the raw PTY byte stream and **it** does the VT parsing and owns
the grid/scrollback client-side. Adopting it demotes `alacritty_terminal`
to a dumb byte pipe and moves authoritative state into the browser. That
breaks daemon-first, breaks one-session-many-windows, and undermines
K2 Connect (each client would re-emulate independently). **Fails the
constraint. Cut.**

### ✨ Option B′ — WebGL/canvas painter driven by our *own* grid deltas (weeks)
The option the research surfaced. Keep our daemon-authoritative grid model
exactly as-is, but replace the per-`<span>` DOM painting with a
canvas/WebGL renderer we drive from the `CellRun` deltas we *already*
stream. Build the glyph-atlas + instanced-quad painting ourselves
(reference: `@xterm/addon-webgl` internals, refterm, WezTerm's
`GlyphCache`) — but the VT emulation never leaves Rust. Gets the GPU
smoothness ceiling of B while satisfying the constraint. **Sessions stay
in Rust. ✓**

### ✅ Option C — Native GPU (wgpu) renderer in Tauri (months)
Go full Zed: render the grid with `wgpu` in the Tauri native layer.
Highest ceiling, sessions firmly in Rust — but conflicts with the
thin-client invariant (features live in core/daemon) and is a large bet.
**Sessions stay in Rust. ✓**

**Live decision list: A (DOM patch) · B′ (own-grid WebGL painter) ·
C (native wgpu).** B (xterm.js) is out.

---

## How K2SO holds up today — honest assessment

Fine for normal terminal work (bash, vim, watching Claude type). **Not**
"buttery smooth" by Zed/Figma/iPad standards. Specific failure modes
(file:line refs as of the research date — verify before acting):

| Problem | Where | Why it janks |
|---|---|---|
| Full re-render on every scroll | `TerminalPane.tsx:1502-1520` (visibleRows useMemo) | `viewportOffset` change → useMemo recompute → React re-renders 24-40 row divs |
| `setTimeout(50ms)` not `requestAnimationFrame` | `TerminalPane.tsx:1796-1841` | setState fires off the monitor's refresh cadence → visible stutter on trackpad flings |
| Snapshot object churn | `TerminalPane.tsx:1520` (`[viewportOffset, snapshot]`) | every daemon delta changes `snapshot` identity → re-runs the visible-rows memo even when you didn't scroll |
| `scrollback.concat()` per delta | `grid_snapshot.rs:243` | O(n) array copy on a 5000-row history, 10-100×/sec → GC pressure |
| No rAF write batching / no flow control | WS consume path | bursts of output can flood the render loop |
| No daemon-side coalescing | `sessions_grid_ws.rs` | one WS message per Alacritty wakeup; Zed batches at ~4ms, we don't |

**Architecture strengths to keep**: client-side scroll (zero WS latency),
delta wire format (only damaged rows + new scrollback), daemon-side damage
tracking, CellRun style-coalescing (already done — keep it).

**Top fixes for Option A, by impact:**
1. **Frame-lock the scroll handler** (HIGH) — replace 50ms `setTimeout`
   with `requestAnimationFrame`; accumulate wheel deltas, flush once per
   frame.
2. **Decouple viewport offset from row re-renders** (HIGH) — keep visible
   row elements static; shift them with CSS `transform: translateY()` /
   absolute positioning; only update row *content* when daemon sends new
   data, not when the viewport moves.
3. **8ms WS batching on the daemon** (MEDIUM) — accumulate Wakeup events
   into one Delta; reduces WS volume 50-80% on heavy output.
4. **Gate visible-row re-renders** (MEDIUM) — only recompute when viewport
   offset OR visible content actually changed, not on every unrelated
   daemon delta.

---

## How Zed does it (the five portable techniques)

Source files in `…/Zed/crates/terminal/` and `…/crates/terminal_view/`.
Zed uses a fork of Alacritty (`zed-industries/alacritty`).

1. **Ring-buffer grid storage (O(1) scrollback)** — *critical.* Alacritty
   stores the grid as a circular array; scrolling up into history just
   advances an index, no data copy, no GC. (We already get this on the
   daemon — and discard it in JS.)

2. **Display offset as the viewport window (O(1) scroll)** — *critical.*
   A single integer `display_offset` indexes into the ring buffer
   (`terminal.rs:792, 1651-1652`). `scrolled_to_top/bottom` are bounds
   checks against `display_offset`, no buffer scan. `term.scroll_display()`
   is one integer add. **We have this on the daemon but throw it away in
   JS.**

3. **Pixel-accumulation decoupled from line updates** — *smooth 120fps.*
   `terminal.rs:2059-2083`: scroll events accumulate in a `scroll_px`
   pixel buffer (sub-line fractional); only crossing a line boundary fires
   a discrete `Scroll(Delta)`. Decouples continuous input from discrete
   grid updates.

4. **Viewport clipping** — *important.* `terminal_element.rs:1050-1116`:
   intersect visible bounds with element bounds; `chunk_by` line →
   `skip(rows_above_viewport).take(visible_row_count)`. Only lay out rows
   that intersect the visible mask, even for a 10K-line buffer.

5. **Batched text runs by style + GPU glyph atlas** — *important.*
   `terminal_element.rs:82-165, 421-446`: greedily merge horizontally
   adjacent same-style cells into one `BatchedTextRun` → one shaping call
   for ~50-200 cells instead of per-glyph. GPUI caches shaped glyphs in a
   texture atlas; each batch paints as one GPU op. Layout (`prepaint`) is
   separated from `paint` so it can be cached when content is unchanged.
   *(We already coalesce into CellRuns — keep it.)*

**Zed's invalidation** is implicit via GPUI reactivity: `Terminal::sync()`
rebuilds `TerminalContent`, GPUI marks the element dirty, repaints
coalesce at display refresh (60/120Hz) so idle terminals don't burn CPU.

---

## Industry research — ranked techniques for a WS-streamed web terminal

> The dominant insight: in a WebSocket-streamed terminal, smoothness is
> bottlenecked by **the pipeline between data arrival and pixels**, not
> raw render speed. VS Code (xterm.js + WebGL) is the single best
> reference for our exact stack.

### Tier 1 — highest leverage, do first
1. **Decouple network arrival from render cadence — drain on `rAF`.**
   Buffer incoming WS bytes in a client-side queue; flush to the
   renderer on a `requestAnimationFrame`-paced loop (≤ once per frame).
   This is xterm.js's `WriteBuffer` + `RenderService` debounce — the
   game-engine "decouple simulation from render" pattern. Data arrival =
   sim tick, rAF = render tick.
2. **Server-side output batching/coalescing (~8-16ms frames).** Coalesce
   PTY reads into frames before sending over the WS. Keep `TCP_NODELAY`
   on and batch at the application layer (don't rely on Nagle — it adds
   first-byte latency).
3. **Flow control / backpressure across the WS boundary.** The #1 source
   of jank and lockups. xterm.js has a **hardcoded 50MB input buffer
   limit — data beyond it is silently discarded.** Fix: watermark scheme
   using the `write(chunk, callback)` completion to pause/resume the
   producer; propagate across the network with ACK-based signaling so the
   daemon pauses PTY reads (`pty.pause()`/`resume()`) when the client
   falls behind. Equivalent to an SSH window / XON-XOFF.
   ```js
   const HIGH = 100000, LOW = 10000; let watermark = 0;
   onData(chunk => {
     watermark += chunk.length;
     term.write(chunk, () => { watermark = Math.max(watermark - chunk.length, 0);
       if (watermark < LOW) resumeProducer(); });
     if (watermark > HIGH) pauseProducer();
   });
   ```
   *(Directly related to this repo's v0.39.7 fd-exhaustion fix — same
   class of "never let transport stalls back up" problem.)*
4. **Scroll as a cheap viewport offset, never a re-render.** Ring-buffer
   scrollback (Alacritty `Grid` model) + display offset; render only the
   visible window (virtualized scrollback).

### Tier 2 — big wins, standard for GPU/web terminals
5. **GPU rendering: glyph atlas + instanced quads.** Rasterize each unique
   glyph once into a GPU texture atlas; draw all visible cells as
   instanced quads in one call → cost scales with *unique glyphs*, not
   characters. `@xterm/addon-webgl` does this; VS Code measured it **up to
   900% faster than canvas, frames averaging < 1ms** (worst 3.96ms).
6. **Damage tracking / dirty-rect rendering.** Redraw only changed
   cells/regions. Alacritty added terminal-level damage + partial render
   via the buffer-age extension (PR #5863). Windows Terminal AtlasEngine
   uses generational change-tracking (`til::generational<T>`).
7. **Decouple the PTY/IO thread from the render thread.** Kitty (official
   docs): child I/O runs in a separate thread from rendering. **Our Rust
   daemon owning the PTY is already this** — keep PTY draining continuous,
   never let render/transport stalls block it.
8. **vsync / refresh-rate-aware frame pacing.** In the browser, `rAF` *is*
   the refresh-rate pacer. Pair with **DEC mode 2026 (synchronized
   output)** for flicker-free atomic frame updates.

### Tier 3 — polish / perceived latency
9. **Predictive/speculative local echo (mosh SSP).** Client-side
   predictive model echoes keystrokes immediately, underlines unconfirmed
   predictions, reconciles against authoritative frames. **Directly
   applicable to K2 Connect / remote daemons** to hide RTT on typed input.
10. **Pixel-level / fractional-line smooth scrolling.** Render the viewport
    at a sub-line `translateY` offset, snap to a line only when scrolling
    settles. Textual reports mouse coords in pixels + dims in cells *and*
    pixels → fractional cell positions. Ghostty's planned approach: map
    scrollbar position → lines, support "fractional lines in the renderer."
11. **Momentum / easing on scroll** — cheap, only changes the viewport
    offset per frame.
12. **Binary framing over the WS** — send terminal bytes as binary frames,
    not base64/JSON (avoids ~33% inflation + encode/decode). *(We
    currently send JSON deltas — a candidate optimization.)*
13. **Triple buffering — situational.** Smooths cadence at the cost of one
    frame of latency; the browser compositor already manages this. Prefer
    low latency for interactive use.

---

## Reference repositories

| Project | What to learn / key files | Relevance |
|---|---|---|
| **VS Code** ([repo](https://github.com/microsoft/vscode)) | PR #84440 (experimental WebGL terminal), #106202 (WebGL default + benchmarks). The canonical "web terminal done smoothly" — same PTY→stream→web stack. | ★★★ highest |
| **xterm.js** ([repo](https://github.com/xtermjs/xterm.js)) | `WriteBuffer` (async write queue), `RenderService` (debounce → one frame, DEC 2026 sync output), [flow-control guide](https://xtermjs.org/docs/guides/flowcontrol/). | ★★★ |
| **@xterm/addon-webgl** ([npm](https://www.npmjs.com/package/@xterm/addon-webgl)) | `Float32Array` of all cells → GPU → shader draw; glyph texture atlas. The GPU path for B′ without writing wgpu. | ★★★ |
| **Alacritty** ([repo](https://github.com/alacritty/alacritty)) | `Grid` ring-buffer + display offset (O(1) scrollback); damage via buffer-age (PR #5863, #5773); separate PTY event loop. We already depend on it. | ★★ |
| **Kitty** ([perf docs](https://github.com/kovidgoyal/kitty/blob/master/docs/performance.rst)) | (Verified) VRAM glyph cache, child I/O in separate thread, SIMD byte-stream parse, few bytes to GPU per update. | ★★ |
| **WezTerm** ([repo](https://github.com/wezterm/wezterm)) | `GlyphCache` (multi-level GPU atlas via `guillotiere`); `wgpu`/WebGPU backend — the Rust path for Option C. | ★★ |
| **mosh** ([repo](https://github.com/mobile-shell/mosh), [paper](https://mosh.org/mosh-paper.pdf)) | SSP + predictive local echo with epochs + reconciliation. | ★★ for remote / K2 Connect |
| **Windows Terminal (AtlasEngine)** ([repo](https://github.com/microsoft/terminal)) | `QuadInstance` per glyph, shelf-packing atlas, D3D11 instanced single draw, `til::generational<T>` change tracking. | ★ |
| **refterm** ([repo](https://github.com/cmuratori/refterm)) | Tile renderer + glyph cache (generate only on unseen glyph); 16-byte block scan for control codes. Proves a *simple* tile renderer + glyph cache suffices. | ★ |
| **foot** ([repo](https://codeberg.org/dnkl/foot)) | Tight damage-tracking CPU renderer — proof GPU isn't strictly required if damage tracking is good. | ★ |
| **ttyd** ([repo](https://github.com/tsl0922/ttyd)) | C + libwebsockets + xterm.js, message-type-prefixed WS frames. Closest architectural twin (PTY→WS→client). | ★ |
| **gotty** ([repo](https://github.com/yudai/gotty)) / **wetty** ([repo](https://github.com/butlerx/wetty)) | Minimal PTY→WS→xterm.js wiring baselines. | ☆ |
| **Textual blog** ([post](https://textual.textualize.io/blog/2025/02/16/smoother-scrolling-in-the-terminal-mdash-a-feature-decades-in-the-making/)) | Concrete fractional-scroll math (pixel mouse reporting + cells-and-pixels dims). | ★ |
| **Ghostty discussion #2355** ([link](https://github.com/ghostty-org/ghostty/discussions/2355)) | Maintainer framing: smooth scroll = "fractional lines in the renderer." | ★ |

---

## The four decisive techniques for *our* stack (research conclusion)

In priority order for a Tauri (Rust daemon + web frontend) WS-streamed
terminal:

1. **Buffer WS bytes client-side, drain on `requestAnimationFrame`** —
   never render per-message. (Our Option-A frame-lock fix.)
2. **End-to-end flow control / backpressure** — watermark the client
   buffer, ACK to the daemon, daemon pauses PTY reads when the client
   falls behind. The difference between smooth and frozen under
   `yes`/build-log floods.
3. **GPU renderer (glyph atlas + instanced quads)** — `@xterm/addon-webgl`
   internals as the reference for our own painter (Option B′), since we
   can't adopt xterm.js wholesale without breaking daemon-authority.
4. **Scroll = fractional-line viewport offset over ring-buffer scrollback**
   — never re-render history per scroll event.

**Bottom line: smoothness comes from doing less work per frame (cache
glyphs, redraw only damage, scroll by offset) and never coupling unrelated
clocks (PTY read ≠ network arrival ≠ render frame) — not from a faster
inner draw loop.**

---

## Sources

- xterm.js: [flow control guide](https://xtermjs.org/docs/guides/flowcontrol/) · [repo](https://github.com/xtermjs/xterm.js) · [@xterm/addon-webgl](https://www.npmjs.com/package/@xterm/addon-webgl) · [architecture (DeepWiki)](https://deepwiki.com/xtermjs/xterm.js/1-overview)
- VS Code: [WebGL terminal PR #84440](https://github.com/microsoft/vscode/pull/84440) · [WebGL default #106202](https://github.com/microsoft/vscode/issues/106202)
- Alacritty: [repo](https://github.com/alacritty/alacritty) · [buffer-age partial render PR #5863](https://github.com/alacritty/alacritty/pull/5863) · [damage tracking PR #5773](https://github.com/alacritty/alacritty/pull/5773)
- [Kitty performance docs](https://github.com/kovidgoyal/kitty/blob/master/docs/performance.rst) · [WezTerm](https://github.com/wezterm/wezterm) · [Windows Terminal AtlasEngine (DeepWiki)](https://deepwiki.com/microsoft/terminal/3.2-atlas-engine) · [refterm](https://github.com/cmuratori/refterm)
- [mosh](https://mosh.org/) · [mosh paper](https://mosh.org/mosh-paper.pdf) · [ttyd](https://github.com/tsl0922/ttyd) · [gotty](https://github.com/yudai/gotty)
- [Textual: smoother scrolling](https://textual.textualize.io/blog/2025/02/16/smoother-scrolling-in-the-terminal-mdash-a-feature-decades-in-the-making/) · [Ghostty #2355](https://github.com/ghostty-org/ghostty/discussions/2355)
- [Raph Levien: swapchains & frame pacing](https://raphlinus.github.io/ui/graphics/gpu/2021/10/22/swapchain-frame-pacing.html)
