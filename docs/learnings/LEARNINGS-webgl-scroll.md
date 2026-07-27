# WebGL scroll smoothness — learnings from xterm.js + field survey

Five-agent research pass (2026-07-13): three agents on the xterm.js
source (viewport/scroll pipeline, WebGL addon internals, render
scheduling), one diagnosing K2's own scroll path, one surveying the
field (kitty, Alacritty, Ghostty, WezTerm, iTerm2 Metal, Warp, foot,
refterm/termbench). xterm.js clone: `$CLAUDE_JOB_DIR/tmp/xterm.js`.

## The symptom

WebGL painter: scrolling "hops around" instead of flowing; static
content renders at full rate. DOM painter scrolls smoothly.

## Diagnosis of K2's WebGL path (verified, file:line in agent report)

Ruled OUT:
- Daemon round-trips: scrolling is 100% client-side for both painters
  (scrollback rides inside the snapshot; wheel never touches the WS).
- Row quantization: our painter already applies sub-row pixel offsets
  (fraction baked into rect y, `u_scrollY` uniform for glyphs) —
  AHEAD of xterm.js, which renders whole-row-aligned always.

Root causes (ranked):
1. **Paint cadence follows React's scheduler, not vsync.** The wheel
   rAF only did `setScrollPx(...)`; the actual paint happened in a
   `useLayoutEffect` after React's concurrent scheduler got around to
   committing. Commits are not vsync-locked.
2. **The hop mechanism:** when a commit lands late/dropped, the wheel
   accumulator keeps collecting deltas, so the next painted frame
   jumps several rows at once. The DOM strip hides this because its
   `translateY` layer glides on the compositor at vsync regardless of
   React; the canvas has no such cushion.
3. **Fast scroll is the pack path's worst case:** newly revealed
   scrollback rows miss the identity-keyed RowCache and pay
   `expandRow` + slab allocation synchronously inside the paint.

## What makes xterm.js seamless (the transferable lessons)

1. **Content is whole-row quantized — smoothness is architectural,
   not sub-pixel.** A virtual float scrollTop (VS Code's ported
   scrollbar) accumulates deltas losslessly; `Math.round` to a row
   happens once per frame at render time. The scrollbar slider glides
   fractionally; content steps rows at 60 Hz and reads as smooth.
2. **One rAF-debounced render, ≤1 and ≥1 per display frame while
   dirty.** Every invalidation folds a min/max row range into a
   single pending rAF (`RenderDebouncer`). Scroll bursts collapse
   into one full-viewport pass per frame.
3. **Parsing is budgeted so it can't starve rendering.**
   `WRITE_TIMEOUT_MS = 12`: PTY parsing yields to the renderer every
   12 ms via macrotask re-scheduling; each parse slice emits its own
   dirty range, so streaming output renders every frame instead of in
   catch-up lurches. (K2 analogue: WS deltas already coalesce one
   `setSnapshot` per rAF — we inherit this for free.)
4. **No scroll cleverness in the WebGL renderer at all.** Scroll
   re-packs the entire viewport and re-uploads the instance buffer
   every step. It's smooth because the repack is engineered cheap:
   flat typed arrays, zero allocation in hot paths, one instanced
   draw per pass, double-buffered STREAM_DRAW VBOs.
5. **Trackpad vs physical-wheel classifier:** only physical wheels
   get the ease-out animation (`smoothScrollDuration`); trackpads
   apply immediately (macOS momentum already supplies the easing) —
   no double-smoothing. Wheel ticks retarget one in-flight animation,
   accumulating against its *target*.
6. **Integer device grid everywhere** (floor cell width, round CSS
   canvas size, never ceil) — kills seams/shimmer at fractional DPR.
   K2 already does this (brief §1.3).

## Field survey highlights (non-xterm)

- Alacritty/kitty: full-screen redraw every frame "because it's so
  cheap" (2 draw calls); smoothness = vsync pacing, not partial
  updates. foot (CPU) memmoves the still-valid pixmap region on
  scroll — the CPU analogue of our slab cache.
- refterm's proof: terminal rendering is ~free if you never do
  redundant work (glyph cache = rasterize once). termbench's SGR
  cliff: per-cell color must be a per-instance attribute, never
  state changes (K2: already per-instance tint).
- kitty's knobs (`repaint_delay`/`input_delay`/`sync_to_monitor`):
  coalescing PTY bytes before repaint is a deliberate latency/
  smoothness trade. Under-coalescing draws torn partial updates.
- MDN/WebGL pitfalls: no per-frame readbacks (`getError`,
  `readPixels`, `getImageData`), `texStorage2D`+`texSubImage2D` over
  `texImage2D` re-alloc, double-buffer VBOs (K2: already done),
  debounced device-pixel-content-box resize, context-loss handling
  (K2: already done), `preserveDrawingBuffer:false` (K2: already),
  `desynchronized:true` where supported, OffscreenCanvas worker as
  the endgame for main-thread isolation.

## Changes implemented (this branch)

1. **Vsync scroll pump** (TerminalPane): the wheel rAF now computes
   the new scrollPx and paints the WebGL frame DIRECTLY inside the
   rAF callback (vsync-aligned), then commits state for the rest of
   the UI. A last-painted dedupe makes the follow-up React
   layout-effect paint a no-op, so it's exactly one paint per frame.
   The layout effect remains the paint path for snapshot/theme/
   selection/metrics changes. (setMetrics/mount reset the dedupe so
   font/DPR changes always repaint.)
2. **RowCache prewarm** (packFrame/webglPainter): after each painted
   frame, pre-expand + pack up to PREWARM_BAND rows just outside the
   window (nearest-first, both directions) under a PREWARM_BUDGET_MS
   time budget — fast scroll hits warm slabs instead of paying
   expandRow+alloc mid-frame.
3. **Context attrs** (glBackend): `desynchronized: true` (shorter
   present path on Chromium/WebView2; ignored by WebKit) +
   `powerPreference: 'high-performance'`.

## Deferred (documented, not built)

- Partial atlas upload (`texStorage2D` + dirty-rect `texSubImage2D`)
  — today a new glyph re-uploads the whole page canvas; measurable
  only during first-scroll through unseen glyph-heavy content.
- OffscreenCanvas + worker (paint off the main thread entirely);
  pairs with moving WS decode into the worker. Big win, big change.
- Momentum ease for physical wheels (xterm's classifier +
  ease-out-cubic). macOS trackpads already feel right without it;
  revisit if externally-reported.
- kitty-style flicker coalescing knob (input_delay) if partial-frame
  tearing is ever observed on full-screen TUI redraws.

## POSTSCRIPT (2026-07-13) — the ACTUAL fast-scroll jump, found last

After the vsync pump + ref-race fixes, jumping persisted on BOTH
painters. Real root cause: **no scroll anchoring.** scrollPx is
bottom-anchored and nothing compensated for appended rows; under fast
scroll the pane's k1 acks lag, the daemon resyncs with a full
snapshot carrying the whole backlog, and the view yanked by that many
rows at once — "jumps like it's catching up" was the resync itself.
Fix `1e96f54`: applyFrameBatch counts appended rows (deltas:
scrollbackAppended; snapshots: total growth) and pins the view via
anchorScrollPx while scrolled up (xterm isUserScrolling / kitty
scrolled_by equivalent). User-confirmed fixed.

Lesson for the file: when a scroll defect is painter-independent and
load-correlated, suspect the DATA path (flow control, anchoring)
before the paint path. The two paint-side fixes were real bugs, but
the mechanism matching the symptom lived in the wire protocol's
interaction with a bottom-anchored coordinate.
