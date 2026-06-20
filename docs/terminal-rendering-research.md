# Terminal Rendering Research: Alacritty + Tauri WebView

> **Context**: K2SO is building the first open-source terminal that combines alacritty_terminal (Rust VT100 emulator) with a Tauri WebView frontend. No one has done this before — every prior alacritty integration uses native GPU rendering (Metal/OpenGL). This document compiles everything we've learned about the pieces we're working with.

---

## Table of Contents

1. [Desired Outcomes](#1-desired-outcomes)
2. [Alacritty Terminal Engine](#2-alacritty-terminal-engine)
3. [How Zed Does It](#3-how-zed-does-it)
4. [Tauri + WKWebView Rendering](#4-tauri--wkwebview-rendering)
5. [Web Terminal Rendering Approaches](#5-web-terminal-rendering-approaches)
6. [K2SO Current Implementation](#6-k2so-current-implementation)
7. [What We've Tried and Why It Failed](#7-what-weve-tried-and-why-it-failed)
8. [Key Constraints and Trade-offs](#8-key-constraints-and-trade-offs)
9. [Implementation Plans](#9-implementation-plans)

---

## 1. Desired Outcomes

The terminal must:

- **Render text that is selectable** — users must be able to highlight and copy text with mouse/keyboard
- **Support clickable URLs** — detected URLs should be clickable (Cmd+click or auto-detect)
- **Scroll smoothly** through large output (10,000+ line Claude chat sessions)
- **Handle high-throughput output** without hanging (`yes | head -100000`, large file `cat`)
- **Render correctly** — ANSI colors, bold/italic/underline, wide characters, cursor shapes
- **Support TUI apps** — vim, htop, top must render correctly
- **Type responsively** — keystrokes must appear within one frame (16ms)
- **Use minimal idle CPU** — 0% when nothing is happening
- **Work in split panes** — multiple terminals in the same window

---

## 2. Alacritty Terminal Engine

### Overview

We use `alacritty_terminal` v0.26.0-rc1 as a library (not the app). It handles VT100 parsing, grid management, scrollback, and selection. We are responsible for rendering.

### Core Architecture

```
PTY (child process)
  → EventLoop (background I/O thread, reads PTY fd)
  → vte::ansi::Processor (parses escape sequences)
  → Handler methods on Term (updates grid cells)
  → Event::Wakeup sent via EventListener
```

### Key Structs

**`Term<T: EventListener>`** — The terminal emulator
- `grid: Grid<Cell>` — active grid (primary or alternate screen)
- `mode: TermMode` — terminal mode flags (APP_CURSOR, ALT_SCREEN, etc.)
- `colors: Colors` — 269-entry color palette
- `selection: Option<Selection>` — active text selection

**`Grid<Cell>`** — 2D grid with scrollback ring buffer
- `display_offset: usize` — scroll position (0 = bottom, higher = scrolled up)
- `cursor: Cursor<Cell>` — current cursor position
- Ring buffer indexed by `Line(i32)` — negative = scrollback history

**`Cell`** — 24 bytes per cell
- `c: char` — the character
- `fg: Color` — foreground color (Named, Indexed, or Spec/RGB)
- `bg: Color` — background color
- `flags: Flags` — BOLD, ITALIC, INVERSE, WIDE_CHAR, UNDERLINE, etc.
- `extra: Option<Arc<CellExtra>>` — rare: zerowidth chars, underline color, hyperlink

### Data Extraction API

The primary API for reading terminal content:

```rust
let content = term.renderable_content();
// Returns RenderableContent:
//   .display_iter  — iterator over visible cells (respects scroll offset)
//   .cursor        — RenderableCursor { shape, point }
//   .selection     — Option<SelectionRange>
//   .colors        — &Colors (palette)
//   .mode          — TermMode
//   .display_offset — usize

for indexed in content.display_iter {
    let point = indexed.point;  // Point { line: Line(i32), column: Column }
    let cell = &indexed.cell;   // &Cell
    // cell.c, cell.fg, cell.bg, cell.flags
}
```

`display_iter` automatically respects the current scroll offset — it iterates only the visible viewport cells.

### Color Resolution

Colors in cells use the `Color` enum:
- `Color::Named(NamedColor)` — 28 named colors (ANSI 0-15, Foreground, Background, Cursor, Dim variants, Bright variants)
- `Color::Indexed(u8)` — 256-color palette index
- `Color::Spec(Rgb)` — true color RGB

To resolve to final RGB:
1. `Spec(rgb)` → use directly
2. `Indexed(i)` → lookup in palette (0-15: ANSI, 16-231: 6×6×6 RGB cube, 232-255: grayscale ramp)
3. `Named(name)` → apply DIM→dim variant, BOLD→bright variant, then lookup in palette
4. **INVERSE flag**: swap fg/bg after resolution (NOT pre-applied in cell data)

### Scroll

```rust
term.scroll_display(Scroll::Delta(n));  // positive = scroll up
// Also: Scroll::PageUp, PageDown, Top, Bottom
```

- `display_offset = 0` → showing live output (bottom)
- Scrolling **always marks full damage** (every row needs re-render)
- `grid.display_offset()` returns current offset
- `term.history_size()` returns max scrollback lines

### Selection

Alacritty has a built-in selection system:
```rust
let selection = Selection::new(SelectionType::Simple, point, side);
selection.update(end_point, end_side);
term.selection = Some(selection);

// Later:
let range = selection.to_range(&term);  // → SelectionRange { start, end, is_block }
let text = term.selection_to_string();  // → Option<String>
```

Selection types: Simple (character), Semantic (word), Lines, Block (column).

### Damage Tracking

```rust
match term.damage() {
    TermDamage::Full => { /* everything changed — redraw all */ }
    TermDamage::Partial(iter) => {
        for line_damage in iter {
            // line_damage.line, .left, .right — which columns changed
        }
    }
}
term.reset_damage();
```

**Important**: Selection and vi-mode cursor are NOT tracked by damage — you must diff those yourself.

### Terminal Modes (Key Flags)

| Flag | Meaning |
|------|---------|
| `APP_CURSOR` | Arrow keys send SS3 (\x1bO) instead of CSI (\x1b[) |
| `APP_KEYPAD` | Keypad sends application sequences |
| `ALT_SCREEN` | Alternate screen buffer active (no scrollback) |
| `SHOW_CURSOR` | Cursor should be visible |
| `MOUSE_MODE` | Mouse events intercepted by terminal app |
| `BRACKETED_PASTE` | Paste wrapped in escape sequences |

### FairMutex

Alacritty uses `FairMutex<Term>` to ensure the rendering thread gets fair access even when the PTY reader is flooding updates. Both Zed and K2SO use this pattern.

---

## 3. How Zed Does It

### Architecture

Zed renders terminals using GPUI (its custom GPU-accelerated UI framework built on Metal). The pipeline:

```
PTY → alacritty EventLoop → ZedListener (channel) → async event loop
  → 4ms batch window (max 100 events)
  → cx.emit(Event::Wakeup) → cx.notify() → schedules re-render
  → prepaint: terminal.sync() → make_content() → layout_grid()
  → paint: backgrounds → text runs → cursor
```

### Content Extraction

Zed does a **full snapshot per frame** — no incremental updates:

```rust
fn make_content(term: &Term<ZedListener>, last_content: &TerminalContent) -> TerminalContent {
    let content = term.renderable_content();
    let mut cells = Vec::with_capacity(content.display_iter.size_hint().0);
    cells.extend(content.display_iter.map(|ic| IndexedCell {
        point: ic.point,
        cell: ic.cell.clone(),
    }));
    // ... extract selection, cursor, mode, display_offset
}
```

### Rendering: Real Text, Not Bitmaps

Zed renders terminal text as **actual text through its GPU text shaping system**. Two key outputs from `layout_grid()`:

1. **`Vec<LayoutRect>`** — background color rectangles (merged adjacent cells with same bg)
2. **`Vec<BatchedTextRun>`** — batched runs of characters sharing the same style

**Batching**: Adjacent cells on the same line with identical font, color, underline, and strikethrough are merged into a single `BatchedTextRun`:

```rust
struct BatchedTextRun {
    start_point: AlacPoint<i32, i32>,
    text: String,          // concatenated characters
    cell_count: usize,
    style: TextRun,        // font, color, underline, etc.
    font_size: AbsoluteLength,
}
```

**Paint order**: background fill → cell background rects → selection highlights → text runs → cursor

Each `BatchedTextRun.paint()` calls `text_system.shape_line()` (using cosmic-text/swash) then `.paint()` for GPU glyph rendering.

### Selection

Zed uses **alacritty's built-in selection system**. Mouse events create/update `Selection` objects stored on the `Term`. Selection is rendered as highlighted rectangles. Text is extracted via `term.selection_to_string()`.

### URL Detection

Zed has its own URL detection layer:
- **OSC 8 hyperlinks**: Read from `cell.hyperlink()`
- **Regex pattern matching**: Custom URL/path regexes, throttled search (100ms + pixel distance)
- **Cmd+click**: Opens detected URLs
- **Hover**: Shows tooltip with URL, underline styling on hovered link

### Scroll

Full re-render on every scroll — no incremental update. `term.scroll_display()` adjusts offset, next `sync()` snapshots all visible cells fresh via `make_content()`.

### Performance

- **4ms batch window** for PTY events (max 100 events per batch)
- **FairMutex** for Term access
- **Vec pre-allocation** in make_content
- **Text run batching** reduces draw calls
- **Background rect merging** reduces paint calls
- **Viewport clipping** skips off-screen rows when terminal is in a scrollable container

---

## 4. Tauri + WKWebView Rendering

### Architecture

On macOS, Tauri v2 uses **WKWebView** (WebKit/Safari engine) in a multi-process architecture:
- **App process** — hosts the WKWebView NSView
- **WebContent process** — runs JS, DOM, layout, paint
- **GPU process** — compositing via CoreAnimation + Metal

### Compositor Behavior

WebKit uses a **display-link-driven compositing cycle**:
- Layout/style/paint happen in WebContent process
- Compositing layers serialized to GPU process
- GPU process composites on next display refresh
- `requestAnimationFrame` fires at beginning of frame, before style/layout/paint
- **Locked to 60fps** on macOS (even ProMotion displays)

### The Canvas Compositor Bug (Our Main Problem)

**`putImageData()` does NOT reliably trigger WKWebView repaints.**

`putImageData` writes pixels directly to the canvas backing store without going through the drawing pipeline that marks the canvas as needing compositing. In the GPU Process architecture, pixel data may reach the backing store but the compositor isn't notified. This is a known WebKit issue.

**`drawImage()` DOES trigger compositing** — it goes through the standard drawing pipeline.

### Tauri Event Delivery

Tauri events from Rust are delivered via `evaluateJavaScript` on WKWebView:
1. Rust: `app_handle.emit("event-name", payload)` → JSON serialization
2. Tauri generates JS string, dispatches via `evaluateJavaScript`
3. JS listener callbacks fire

**Critical**: Events delivered via `evaluateJavaScript` are **NOT in a user gesture context**. The compositor does not treat them as high-priority. User gesture-driven changes (clicks, key presses) get prioritized; programmatic changes may be batched more aggressively.

This explains why **typing works but scroll doesn't**: keyboard events are real DOM events that prime the compositor. Scroll bitmap responses arrive via Tauri IPC (evaluateJavaScript), outside any user gesture context.

### DOM Text Rendering

- Text goes through WebKit → CoreText (Apple's text engine) → glyph rasterization → GPU texture
- Text rasterization is CPU (CoreText), compositing is GPU
- Changing `textContent`/`innerHTML` **always triggers repaint** (style recalc → layout → paint → composite)
- Many styled spans (10,000+) are expensive but workable — xterm.js DOM renderer does this

### Image Rendering

- Setting `img.src` **always triggers repaint** — goes through image loading pipeline
- Both data URLs and blob URLs trigger repaints
- Data URLs: synchronous decode from memory, 33% size overhead from base64
- Blob URLs: async fetch from Blob store, more memory-efficient

### WebGL in WKWebView

**Known broken for terminal text rendering**. xterm.js issues document invisible text in Safari/WKWebView due to WebGL texture sampling issues. WebGPU not available until macOS 26.

### Alternative: Native Metal View Overlay

Tauri supports `macOSPrivateApi: true` (already enabled in K2SO). It's possible to:
- Create a native `MTKView` (Metal) as a child NSView alongside the WebView
- Render terminal content directly via Metal
- Coordinate with WebView via Tauri IPC for layout

This is what Alacritty, Kitty, and WezTerm do natively.

---

## 5. Web Terminal Rendering Approaches

### xterm.js Renderers

xterm.js supports three renderers:

| Renderer | How it works | Selection | Performance | WKWebView |
|----------|-------------|-----------|-------------|-----------|
| **DOM** | Rows of `<span>` elements with inline styles | Native browser selection | Slowest, but works | Works reliably |
| **Canvas** | Glyph atlas + `fillText` per cell | Hidden DOM layer underneath | Medium | `putImageData` bug issues |
| **WebGL** | Texture atlas + GPU quads per cell | Hidden DOM layer underneath | Fastest | **BROKEN** — invisible text |

**Key xterm.js pattern**: Canvas and WebGL renderers use an **invisible DOM layer** underneath for text selection. The visible rendering is canvas/WebGL, but a hidden `<div>` contains the same text content positioned identically. When the user selects text, they're actually selecting from the invisible DOM layer.

### DOM-Based Terminal Rendering

The approach: render each row as a `<div>` containing `<span>` elements for styled text runs.

```html
<div class="terminal">
  <div class="row"><span style="color:#e0e0e0">$ </span><span style="color:#80ff80;font-weight:bold">ls</span></div>
  <div class="row"><span style="color:#5555ff">file.txt</span>  <span>README.md</span></div>
  <!-- ... 24-80 rows ... -->
</div>
```

**Pros**:
- Native text selection works
- Native URL detection/clicking possible
- Always triggers WKWebView repaints (DOM mutations always composite)
- Accessibility built-in
- No WebGL/canvas issues

**Cons**:
- Slower than canvas/WebGL for high-throughput
- Many DOM nodes = style recalc overhead
- Must batch updates carefully to avoid layout thrashing

**Performance considerations**:
- A 134×64 terminal = ~8,576 cells. With text run batching, this might be ~500-2000 spans.
- Changing `textContent` of existing spans is cheap (no DOM tree rebuild)
- `innerHTML` replacement of entire rows is acceptable if batched per-frame
- Virtual scrolling (recycle row elements, only update visible rows) helps

### Hybrid Approach (Canvas + DOM Selection Layer)

Used by xterm.js canvas/WebGL renderers:
- **Visible layer**: Canvas or image rendering (fast, pixel-perfect)
- **Invisible layer**: DOM with identical text content for selection
- Selection events captured from DOM layer, visual feedback drawn on canvas

**Problem in our case**: The canvas layer has the WKWebView compositor bug. Using `<img>` as the visible layer with a DOM selection layer underneath is possible but complex.

---

## 6. K2SO Current Implementation

### Rust Side

**Terminal Manager** → creates PTY + alacritty EventLoop + bitmap emission thread per terminal.

**Bitmap Emission Loop** (background thread per terminal):
1. Block on wakeup channel (PTY events + manual scroll wakeups)
2. Rate limit: 16ms minimum frame interval, 4ms batch window
3. Lock term, check damage (or force_full_render for scroll)
4. Extract cells via `extract_row_cells()`, render via `render_row()` into RGBA buffer
5. QOI encode full RGBA buffer → base64 → emit `BitmapUpdate` via Tauri event

**Font Rendering**: fontdue-based glyph cache with MesloLG Nerd Font embedded. DPR forced to 1.0 (browser CSS upscales).

**Bitmap Renderer**: RGBA pixel buffer, renders backgrounds, glyphs (alpha-blended), underline/strikethrough, cursor.

### Frontend Side

**AlacrittyTerminalView.tsx** (~430 lines):
- Listens for `terminal:bitmap:{id}` events
- rAF-batched rendering: store latest frame, render on next animation frame
- Currently Method 7: QOI decode → temp canvas → `toDataURL('image/png')` → `img.src`
- Keyboard input via `keyEventToSequence()` + `terminal_write` IPC
- Scroll via `terminal_scroll` IPC
- Mouse selection tracked in JS, text extracted via `terminal_get_selection_text` IPC

### Data Flow

```
User types → keyEventToSequence → terminal_write → PTY → alacritty_terminal
  → Wakeup → bitmap_emission_loop → render damaged rows → QOI encode → base64
  → Tauri event → JS listener → rAF → QOI decode → temp canvas → PNG data URL
  → img.src → browser repaint
```

---

## 7. What We've Tried and Why It Failed

### Methods for Visual Scroll Rendering

| # | Method | Result | Why It Failed |
|---|--------|--------|---------------|
| 1 | Canvas `putImageData` + rAF | Typing works, scroll doesn't | `putImageData` doesn't trigger WKWebView compositor. Typing works because keyboard DOM events prime the compositor. |
| 2 | Canvas + opacity toggle | Same | Opacity toggle in JS doesn't force compositing when not in user gesture context |
| 3 | Canvas + `offsetHeight` reflow | Same | Layout reflow doesn't force a compositor repaint for canvas backing store |
| 4 | OffscreenCanvas + `transferFromImageBitmap` | Same | Same underlying issue — bitmap transfer doesn't go through the drawing pipeline |
| 5 | `<img>` + blob URL from `convertToBlob()` | Same | `convertToBlob().then()` resolves as microtask — `img.src` set outside compositor cycle |
| 6 | Visible canvas + `drawImage(offscreen)` | Nothing renders | `drawImage` from OffscreenCanvas apparently doesn't work reliably in WKWebView either |
| 7 | `<img>` + `toDataURL()` from temp canvas | Typing works, scroll doesn't | `toDataURL()` is synchronous but the Tauri event callback still isn't in a user gesture context |

### Root Cause Analysis

The fundamental issue: **WKWebView's compositor prioritizes user-gesture-driven changes over programmatic changes**.

- Keyboard events → JS handler → DOM change → compositor primed by user gesture → repaint happens
- Tauri IPC event (evaluateJavaScript) → JS handler → DOM change → no user gesture context → compositor may defer/skip repaint

This affects ALL rendering methods (canvas, img, even DOM) when the change is triggered from a Tauri event that originated from Rust (like scroll bitmap responses).

**But wait** — DOM mutations from `textContent`/`innerHTML` changes always trigger style recalc + layout + paint + composite. This is different from canvas/img changes which only need compositing. The full layout pipeline is harder for the compositor to skip.

**This is the key insight**: DOM text rendering may be the only approach that reliably triggers WKWebView repaints from non-user-gesture contexts, because it goes through the full layout pipeline rather than just the compositing pipeline.

---

## 8. Key Constraints and Trade-offs

### Hard Constraints

1. **Must use WKWebView** — Tauri on macOS, no alternative (CEF backend not ready)
2. **WKWebView locked to 60fps** — can't exceed this regardless of approach
3. **WebGL broken in WKWebView** for text rendering (xterm.js documented)
4. **WebGPU unavailable** until macOS 26
5. **Tauri events are not user gestures** — compositor doesn't prioritize them

### Trade-off Matrix

| Approach | Scroll Repaints | Text Selection | URL Clicking | Throughput | Complexity |
|----------|----------------|----------------|--------------|------------|------------|
| Bitmap (current) | NO (compositor bug) | NO (it's an image) | NO | High | Medium |
| DOM text rendering | YES (layout always repaints) | YES (native) | YES (native) | Medium | Medium |
| DOM + virtual scroll | YES | YES | YES | High | High |
| Native Metal overlay | YES (bypasses WebView) | Custom impl needed | Custom impl needed | Highest | Very High |
| Hybrid (img + DOM overlay) | Uncertain | YES (from DOM layer) | YES | High | High |

### Performance Budget

At 60fps, we have 16.67ms per frame. Budget breakdown:
- Rust: extract cells + serialize ≤ 2ms
- IPC: Tauri event delivery ≤ 1ms
- JS: process + render ≤ 5ms
- Browser: style + layout + paint + composite ≤ 5ms
- Headroom: ~3ms

For a 134×64 terminal (8,576 cells), with text run batching (~500-2000 spans), DOM rendering should fit within budget.

---

## 9. Implementation Plans

### Plan A: DOM Text Rendering (Recommended)

**Core idea**: Send structured cell data from Rust, render as styled `<span>` elements in the DOM. DOM mutations always trigger WKWebView repaints.

**Rust side changes**:
- Keep alacritty_terminal for VT100 parsing
- Replace bitmap rendering with **text-based serialization**
- Send rows as batched text runs (like Zed's layout_grid concept):
  ```json
  {
    "rows": [
      {
        "runs": [
          { "text": "$ ", "fg": "#e0e0e0" },
          { "text": "ls -la", "fg": "#80ff80", "bold": true }
        ]
      }
    ],
    "cursor": { "col": 7, "row": 0, "shape": "bar", "visible": true },
    "display_offset": 0,
    "mode": 3
  }
  ```
- Use damage tracking: only send changed rows (delta updates)
- Keep font rendering in Rust? No — let the browser handle fonts natively

**Frontend changes**:
- Replace `<img>` with a `<div>` grid of row elements
- Each row contains `<span>` elements for styled text runs
- Update only changed rows (keyed by row index)
- CSS handles colors, bold, italic, underline
- Cursor rendered as a positioned `<div>` overlay with CSS animation for blink
- Selection: use alacritty's selection system, render as highlight overlay or use browser native selection

**IPC optimization**:
- Only send changed rows (damage tracking)
- Batch text runs (same style → one span)
- Use compact encoding: `[row_idx, [[text, fg, bg, flags], ...]]`
- Consider binary encoding (MessagePack) instead of JSON for high-throughput

**Scroll**:
- `terminal_scroll` IPC → Rust scrolls → full viewport snapshot → send all rows
- DOM update always repaints → scroll visually works

**Pros**: Guaranteed repaints, native text selection, native URLs, accessible, simple mental model.
**Cons**: Slower than bitmap for extreme throughput; many DOM nodes during rapid output.
**Risk**: DOM update performance during `yes | head -100000` — mitigated by rate limiting (already have 16ms frame interval).

**Estimated effort**: Medium. Reuse existing alacritty integration, replace bitmap renderer with text serializer, rewrite frontend from ~430 lines of bitmap code to ~500 lines of DOM rendering.

---

### Plan B: DOM Text Rendering + Canvas Fast Path (Hybrid)

**Core idea**: Use DOM text rendering as the default (guaranteed repaints, selection, URLs), but switch to canvas bitmap rendering during high-throughput bursts for performance.

**How it works**:
- **Normal mode**: DOM text rendering (Plan A). Full interactivity.
- **Burst mode**: When output rate exceeds threshold (>50 rows/second for >500ms), switch to bitmap `<img>` rendering. Overlay a semi-transparent "scrollback loading" indicator.
- **Return to DOM**: When burst ends (output rate drops), re-render current viewport as DOM.

**The canvas/img scroll problem doesn't matter during bursts** because:
- During high-throughput output, the user isn't trying to scroll
- They're waiting for output to finish
- When output stops, we switch back to DOM (which repaints correctly)

**Scroll during burst**: If user tries to scroll during burst, temporarily switch to DOM rendering for that scroll operation.

**Pros**: Best of both worlds — DOM reliability + bitmap performance.
**Cons**: Complexity of mode switching; potential visual glitch on transitions.
**Risk**: Mode transitions may be jarring; dual rendering paths = double the bugs.

**Estimated effort**: High. Requires both Plan A DOM rendering AND the existing bitmap pipeline, plus mode switching logic.

---

### Plan C: Native Metal Terminal View

**Core idea**: Bypass WKWebView entirely for terminal rendering. Create a native Metal view (`MTKView`) as a child NSView alongside the Tauri WebView. Render terminal content directly via Metal.

**How it works**:
- Tauri's `macOSPrivateApi: true` gives access to the window's NSView
- Create an `MTKView` and add it as a subview positioned over the terminal area
- Rust renders directly to Metal (like Alacritty/WezTerm/Kitty)
- WebView handles everything else (tab bar, file tree, settings)
- IPC coordinates layout: WebView tells Rust where to position the Metal view

**Text selection**: Implement custom selection (like Alacritty does) — mouse tracking, highlight rendering, clipboard integration.

**URL clicking**: Implement custom URL detection and handling.

**Pros**: Maximum performance, zero WebView limitations, matches what real terminal apps do.
**Cons**: Very complex; must implement selection, URL detection, accessibility from scratch; fragile NSView coordination; can't use CSS/React for terminal styling.
**Risk**: High. NSView overlay coordination with Tauri is undocumented territory. Resize/reposition synchronization between WebView and Metal view.

**Estimated effort**: Very high. Essentially building a native terminal renderer from scratch, plus the bridge to Tauri.

---

### Recommendation

**Start with Plan A (DOM Text Rendering)**. It solves the scroll rendering problem definitively, gives us text selection and URLs for free, and has reasonable complexity. The performance risk (DOM updates during high throughput) is mitigated by existing rate limiting.

If Plan A's throughput proves insufficient for extreme cases, **upgrade to Plan B** (add bitmap fast path for bursts). The DOM rendering from Plan A remains the foundation.

Plan C (Native Metal) is the nuclear option — only pursue if Plans A and B both fail to deliver acceptable UX. It's architecturally the "right" answer but the engineering cost is very high.

---

## Appendix: Key File Locations

### K2SO
| File | Purpose |
|------|---------|
| `src-tauri/src/terminal/alacritty_backend.rs` | Main backend — PTY, event loop, emission loop |
| `src-tauri/src/terminal/grid_types.rs` | IPC structs (BitmapUpdate, GridUpdate, etc.) |
| `src-tauri/src/terminal/bitmap_renderer.rs` | RGBA bitmap rendering |
| `src-tauri/src/terminal/font_renderer.rs` | fontdue glyph cache |
| `src-tauri/src/commands/terminal.rs` | Tauri commands |
| `src/renderer/components/Terminal/AlacrittyTerminalView.tsx` | Frontend terminal view |
| `src/renderer/lib/qoi-decode.ts` | QOI decoder |
| `src/renderer/lib/key-mapping.ts` | Keyboard escape sequence mapping |

### Zed (Reference)
| File | Purpose |
|------|---------|
| `crates/terminal/src/terminal.rs` | Terminal model, make_content, selection |
| `crates/terminal_view/src/terminal_element.rs` | Rendering: layout_grid, cell_style, paint |
| `crates/terminal_view/src/terminal_view.rs` | View orchestration, scroll, events |
| `crates/terminal/src/terminal_hyperlinks.rs` | URL detection |

### Alacritty Engine (alacritty_terminal 0.26.0-rc1)
| API | Purpose |
|-----|---------|
| `term.renderable_content()` | Get visible cells, cursor, selection, mode |
| `content.display_iter` | Iterator over viewport cells |
| `term.scroll_display(Scroll)` | Scroll viewport |
| `term.damage()` / `term.reset_damage()` | Damage tracking |
| `term.selection_to_string()` | Extract selected text |
| `Selection::new()` / `.update()` | Create/update selection |
