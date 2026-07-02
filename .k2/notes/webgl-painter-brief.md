# WebGL2 Terminal Painter — Design Brief (Path B′)

> **Status**: design brief for the build agent. No code in this doc — it is the blueprint
> for the feature-flagged WebGL2 instanced painter inside the Tauri WKWebView.
> **Architecture decision record**: `.k2/prds/terminal-smoothness-rnd.md` §5 (Path B′ is the
> chosen architecture; WebGPU is NOT available in WKWebView — WebGL2 only, §4).
> Companions: `docs/terminal-rendering-research.md`, `docs/terminal-scrolling-research.md`.
>
> **Invariants that do not move**: the Rust daemon owns the authoritative grid
> (`crates/k2-core/src/terminal/grid_snapshot.rs`); the CellRun snapshot/delta wire protocol
> is unchanged; the painter is a **pure consumer** that replaces only the DOM `<span>` strip in
> `src/renderer/kessel-term/TerminalPane.tsx`.

**Citation legend** (local read-only clones; verify line drift before relying on exact numbers —
xterm.js @ `43e8365` 2026-05-28):

| Prefix | Path |
|---|---|
| `xterm:` | `/Users/z3thon/DevProjects/Alakazam Labs/terminal-research-repos/xterm.js/` |
| `zed:` | `/Users/z3thon/DevProjects/Alakazam Labs/Zed/` |
| `glide:` | `/Users/z3thon/DevProjects/Alakazam Labs/terminal-research-repos/glide-data-grid/` |
| `k2:` | this repo |

The xterm.js WebGL addon (`xterm:addons/addon-webgl/src/`) is THE template: it is the canonical,
shipped WebGL2 terminal painter (VS Code's default renderer). Zed contributes the alpha-only-atlas
+ tint-per-instance idea; glide-data-grid contributes canvas damage discipline.

---

## 0. What the painter consumes (K2's model, restated precisely)

- **Wire types**: `TermGridSnapshot { grid: Vec<Vec<CellRun>>, scrollback, cursor, cols, rows,
  version, displayOffset, mouseReport, sgrMouse, altScreen }` and
  `TermGridDelta { damagedRows: Vec<DamagedRow>, scrollbackAppended, cursor, version, … }`
  (`k2:crates/k2-core/src/terminal/grid_snapshot.rs:33-68,151-162`).
- **CellRun** = style-coalesced run: `{ text, fg: Option<u32 /*0xRRGGBB*/>, bg: Option<u32>,
  bold, italic, underline, inverse, dim, strikeout, wrapped }` (`grid_snapshot.rs:84-111`).
  `None` fg/bg = theme default. Rows are **whole-row replaced** on damage and trailing
  undecorated blanks are trimmed (`grid_snapshot.rs:264-305`).
- **Damage granularity is the row**: delta ships full damaged rows + rows newly scrolled into
  scrollback (`grid_snapshot.rs:435-537`); the client merge (`mergeDelta`,
  `k2:src/renderer/kessel-term/TerminalPane.tsx:319-352`) replaces damaged rows **by reference**
  and concatenates `scrollbackAppended` — untouched row arrays keep object identity. The DOM
  renderer's `React.memo` row skip exploits exactly this (`TerminalPane.tsx:218-242`); the
  painter's dirty-row detection will exploit it the same way (§2.4).
- **rAF frame coalescing already exists**: WS messages queue and apply once per animation frame,
  with a queued full snapshot superseding earlier frames and a size-cap synchronous flush at 60
  queued frames (`TerminalPane.tsx:557-595`). The painter plugs in downstream of this (§6).
- **Client-side scroll**: `viewportOffset` (lines-from-bottom) windows over
  `scrollback ++ grid`; visible rows = `[totalLen - offset - rows, totalLen - offset)`
  (`TerminalPane.tsx:2047-2065`). Wheel → rAF-flushed offset change (`TerminalPane.tsx:2521-2555`).
- **Cell metrics** are measured in CSS px from a `'W'` span; height =
  `ceil(fontSize * lineHeightMultiplier)` (`TerminalPane.tsx:1617-1630`). Pane content is inset
  by a 4px padding used in all pixel→cell math (`TerminalPane.tsx:2468-2478,2641-2646`).

---

## 1. Glyph atlas

### 1.1 What xterm.js actually does (baked-color RGBA atlas)

- Glyphs are rasterized on a temp **Canvas2D** with `fillText` (`xterm:addons/addon-webgl/src/TextureAtlas.ts:517-521,718-721`),
  the background color is then **erased to alpha** with a pixel scan (`clearColor`,
  `TextureAtlas.ts:1111-1150`), the non-transparent bounding box is found and the glyph is
  **trimmed** (`_findGlyphBoundingBox`, `TextureAtlas.ts:955-1035`), and blitted into an atlas
  page canvas with `putImageData` (`TextureAtlas.ts:933-941`). Pages are plain `<canvas>`
  elements; the GPU texture is uploaded whole-page via `texImage2D(gl.RGBA, …, page.canvas)`
  keyed off a monotonic page `version` (`xterm:addons/addon-webgl/src/GlyphRenderer.ts:363-370,387-395`).
- **Cache key is (code, bg, fg, ext)** — a `FourKeyMap` (`TextureAtlas.ts:60-61,272-287`). i.e.
  xterm bakes the **resolved colors into the pixels**. Every fg/bg/style combination is a distinct
  atlas entry. They need this because minimum-contrast adjustment, selection blending and
  decorations all mutate the final color per cell (`TextureAtlas.ts:332-371,408-439`).
- **Page size / growth**: pages start at **512×512** (`TextureAtlas.ts:78`), packing is
  shelf-style rows left→right/top→bottom with a "fixed row" trick for short glyphs
  (`TextureAtlas.ts:1061-1076`), plus a dedicated overflow page for oversized glyphs
  (`TextureAtlas.ts:815-834`). When the page count hits the texture-unit budget, the **4 most-used
  same-size pages are merged into one 2× page** (`TextureAtlas.ts:154-259`), the fragment shader
  samples `u_texture[maxPages]` with an if/else-if chain on a per-instance `texpage`
  (`GlyphRenderer.ts:59-79`), and `maxAtlasPages = min(32, MAX_TEXTURE_IMAGE_UNITS)`
  (`GlyphRenderer.ts:124-129`). A mid-frame merge forces a model clear + full re-update, retried
  up to 32 times (`xterm:addons/addon-webgl/src/WebglRenderer.ts:31-33,386-396`).
- **Style handling** (the exact split the brief was asked for):
  - **bold** → atlas variant via font weight in the Canvas2D font string; **italic** → atlas
    variant via font style (`TextureAtlas.ts:515-518`).
  - **dim** → **baked** into the glyph color at 0.5 opacity (`TextureAtlas.ts:366-368`;
    `DIM_OPACITY` `xterm:addons/addon-webgl/src/Constants.ts:8`).
  - **inverse** → resolved **before** rasterization by swapping fg/bg color modes
    (`TextureAtlas.ts:496-503`); the bg-rect pass also resolves inverse (`RectangleRenderer.ts:313-340`).
  - **underline (all 5 styles, colored), overline, strikethrough** → **drawn into the glyph
    bitmap** with Canvas2D stroking (`TextureAtlas.ts:549-716,742-752`), including a
    background-colored outline between text and underline (`TextureAtlas.ts:678-703`) and a
    per-column `variantOffset` so dotted/dashed patterns phase-continue across cells
    (`xterm:addons/addon-webgl/src/CellColorResolver.ts:63-69`).
  - **blink/invisible** → shader-side nothing; handled by flagging fg INVISIBLE in the model on
    blink ticks (`WebglRenderer.ts:551-553`).
  - **cursor** → rect pass + per-cell color override, see §2.2.
- **Zoom / font / theme changes rebuild the atlas**: the atlas config includes dpr, font family,
  size, weights, letterSpacing, lineHeight, cell dims, min-contrast and fg/bg/ansi colors
  (`xterm:addons/addon-webgl/src/CharAtlasUtils.ts:55-79`); any change acquires a different atlas
  from a global cache shared across terminals (`xterm:addons/addon-webgl/src/CharAtlasCache.ts:27-79`).
  Theme change → `_refreshCharAtlas()` + full model clear (`WebglRenderer.ts:176-181`).
- **DPR**: device char width is `floor(cssCharWidth * dpr)`, height is `ceil(...)`; cell height =
  `floor(charHeight * lineHeight)`; CSS canvas size is `round(device / dpr)` — floor/ceil/round
  discipline is deliberate to keep glyphs on an integer device grid and avoid blur at fractional
  DPRs (`WebglRenderer.ts:631-681`). Actual canvas backing size is corrected from
  `devicePixelContentBoxSize` via a ResizeObserver so CSS↔device rounding can't drift
  (`xterm:addons/addon-webgl/src/DevicePixelObserver.ts:8-40`, consumed at `WebglRenderer.ts:148-151,683-693`).
  DPR change → full `handleResize` → atlas rebuild (`WebglRenderer.ts:183-190`).

### 1.2 What Zed does (alpha-only atlas + tint per instance)

- Two atlas texture lists by kind: **monochrome = `A8Unorm`** (1 byte/px), polychrome (emoji) =
  `BGRA8Unorm` (`zed:crates/gpui_macos/src/metal_atlas.rs:34-35,142-149`). Kind chosen by
  `is_emoji` (`zed:crates/gpui/src/platform.rs:821-835`). Monochrome glyphs are rasterized as an
  **alpha-only coverage mask** (`kCGImageAlphaOnly`, `zed:crates/gpui_macos/src/text_system.rs:402-425`).
- **Color is applied at draw time**: fragment shader does `color.a *= sample.a; return color`
  where `color` is a flat per-instance tint (`zed:crates/gpui_macos/src/shaders.metal:637,645-660`).
  One atlas entry serves every color and theme.
- Packing = etagere shelf allocator per texture, textures 1024² → new texture on overflow (no
  merge), 16384² cap (`metal_atlas.rs:96-135`). Atlas key includes scale factor + a 4×4 subpixel
  variant (`zed:crates/gpui/src/window.rs:3309-3326`, `zed:crates/gpui/src/text_system.rs:45-51`) —
  Zed positions glyphs at arbitrary subpixel x; a terminal grid doesn't need this (§7.3).
- Underline/strikethrough are **separate quad primitives**, never baked into glyphs
  (`zed:crates/terminal_view/src/terminal_element.rs:566-579`,
  `zed:crates/gpui/src/window.rs:3229-3289`).

### 1.3 K2 recommendation — "white-glyph atlas": Zed's economics on xterm's plumbing

**Rasterize glyphs white-on-transparent into Canvas2D atlas pages (xterm's pipeline), upload as
RGBA, and treat the texture as coverage: `outColor = v_fg * texture(...).a` for monochrome
instances.** Key the cache by **(codepoint, bold, italic)** only.

Why this beats copying either template verbatim:

1. **K2's wire already resolves color** to concrete `0xRRGGBB` (or theme-default) on the daemon
   (`grid_snapshot.rs:201-222`). We have none of the reasons xterm bakes color (min-contrast,
   decoration overrides, selection blending happen in *their* client model —
   `CellColorResolver.ts:82-175`). Baking would make atlas entries explode under 24-bit-color TUI
   output; keying by glyph+weight+slant keeps entries = unique glyphs (~hundreds).
2. Drawing **white text on a transparent canvas** means the alpha channel *is* the coverage mask —
   no `clearColor` background-erase scan, no `getImageData` per glyph beyond the bounding-box trim.
   Fringing is neutral-colored (see §7.6); xterm's threshold hack (`TextureAtlas.ts:1120-1127`)
   becomes unnecessary.
3. Uploading the page **canvas** via `texImage2D(gl.RGBA, …, canvas)` keeps xterm's dead-simple
   versioned-page upload (`GlyphRenderer.ts:387-395`) — no manual byte buffers. (An `R8`/`LUMINANCE`
   texture would save 3 bytes/px but forfeits canvas-source upload; not worth it at our sizes.)
4. **Emoji / multi-color glyphs**: rasterize normally (not forced white), one-time pixel scan of
   the trimmed glyph — if any pixel's RGB isn't monochrome-white, mark the atlas entry
   `colorGlyph=true` and have the shader output the sample directly (per-instance flag, §2.3).
   Same page, two shader paths — mirrors Zed's monochrome/polychrome split
   (`platform.rs:821-835`) without a second texture list.

Style-by-style plan for K2's 7 CellRun booleans:

| CellRun flag | Mechanism | Rationale / citation |
|---|---|---|
| `bold` | atlas variant (font weight in rasterizer font string) | xterm `TextureAtlas.ts:515-518` |
| `italic` | atlas variant (font style) | same |
| `dim` | **per-instance**: multiply instance fg alpha ×0.5 | xterm bakes 0.5 (`Constants.ts:8`); with tint-at-draw we get it free |
| `inverse` | resolved at run→cell expansion (swap fg/bg, defaults resolved) | matches DOM `runStyle` (`TerminalPane.tsx:155-196`) and xterm pre-swap (`TextureAtlas.ts:496-503`) |
| `underline` | **rect pass** (1 device-px-scaled bar per run) | K2 has only boolean single-style underline; xterm bakes only because of 5 styles/colors (`TextureAtlas.ts:549-716`); Zed uses quads (`terminal_element.rs:566-579`) |
| `strikeout` | rect pass, same as underline | xterm `TextureAtlas.ts:742-752` shows the y-math (`charHeight/2`) |
| `bg` | rect pass (§2.2) | xterm `RectangleRenderer.ts:199-246` |

**Page size/growth**: start one 512² page, grow by **doubling the single page** (512→1024→2048→4096,
re-blitting old page into the new canvas — trivial with canvas pages) instead of xterm's multi-page
merge choreography. Only if 4096² fills (essentially impossible for monochrome trimmed glyphs at
one font size: xterm's forced max is 4096, `TextureAtlas.ts:34-46`) do we add pages, capped at 4,
with an LRU **full clear + warm-up** as the overflow strategy — xterm's own `clearTexture()` path
(`TextureAtlas.ts:142-152`). This removes the entire §7.2 merge/texture-unit pitfall class: fragment
shader samples at most 4 pages. Adopt xterm's shelf packing incl. the fixed-row trick for short
glyphs (`TextureAtlas.ts:1061-1076,839-899`) and the idle-queue ASCII 33-126 warm-up
(`TextureAtlas.ts:115-133`).

**Rebuild triggers** (dispose atlas + repopulate lazily): font family/size change, DPR change,
`lineHeightMultiplier` change. Theme change does **not** rebuild the atlas in our design (color is
per-instance) — it only re-tints instances and the default-bg uniform; this is a concrete win over
xterm (`CharAtlasUtils.ts:55-79` forces their rebuild on any fg/bg/ansi change). K2 zoom
(Cmd+plus → `fontSize` store change, `k2:src/renderer/stores/terminal-settings.ts:74-88`) is a
font-size change → rebuild.

**DPR handling**: adopt xterm's rules wholesale — `deviceCharWidth = floor(cssW × dpr)`,
`deviceCharHeight = ceil(cssH × dpr)`, canvas CSS size = `round(device/dpr)`
(`WebglRenderer.ts:637-673`), plus `devicePixelContentBoxSize` observation
(`DevicePixelObserver.ts:8-40`). Note today's DOM renderer uses **fractional CSS cell width**
straight from `getBoundingClientRect` (`TerminalPane.tsx:1617-1630`); the painter must floor to the
device grid, so painter cell width ≢ DOM cell width by sub-pixel amounts — resize col math
(`TerminalPane.tsx:2400-2426`) must use the painter's metrics when the flag is on.

---

## 2. Buffers, instance layout, draw passes

### 2.1 xterm's exact layouts (the reference)

- **Glyph pass**: one interleaved `Float32Array`, **11 floats (44 B) per cell**
  (`GlyphRenderer.ts:81-85`): `a_offset`(2f, px offset of trimmed glyph vs cell origin),
  `a_size`(2f, glyph size / canvas size), `a_texpage`(1f), `a_texcoord`(2f), `a_texsize`(2f),
  `a_cellpos`(2f, col/cols, row/rows — written once per resize, `GlyphRenderer.ts:313-319`).
  All attributes `vertexAttribDivisor 1` over a 4-vertex unit-quad TRIANGLE_STRIP
  (`GlyphRenderer.ts:144-183`). Vertex shader:
  `pos = (a_offset / u_resolution) + a_cellpos + (a_unitquad * a_size)` (`GlyphRenderer.ts:37-57`).
- **Model**: parallel `Uint32Array`, 4 ints per cell (code, bg, fg, ext)
  (`xterm:addons/addon-webgl/src/RenderModel.ts:10-15`) used only for the per-cell "did anything
  change" skip (`WebglRenderer.ts:559-565`).
- **Upload strategy**: every frame, rows are packed **up to their line length** (last non-blank
  cell, `WebglRenderer.ts:555-557`) into one of two alternating CPU buffers (double-buffered so the
  GPU isn't stalled on a locked buffer, `GlyphRenderer.ts:15-25,340-342`), then a single
  `gl.bufferData(STREAM_DRAW)` + one `drawElementsInstanced(TRIANGLE_STRIP, 4, …, cellCount)`
  (`GlyphRenderer.ts:351-373`).
- **Rect pass** (backgrounds + cursor): separate program, **8 floats per rectangle**:
  x,y,w,h (all normalized 0-1) + rgba (`xterm:addons/addon-webgl/src/RectangleRenderer.ts:53-56,356-376`).
  Backgrounds are built by a per-row run-length merge over the model (bg change or inverse-fg
  change breaks a run; default-bg cells emit nothing) (`RectangleRenderer.ts:199-246`), with
  rect[0] = a full-viewport clear in theme background (`RectangleRenderer.ts:186-197`).
- **Draw order per frame** (`WebglRenderer.ts:398-404`): (1) `renderBackgrounds()` — clear rect +
  merged bg rects, (2) glyph instanced draw, (3) `renderCursor()` — bar/underline/outline cursor
  styles as 1-4 rects (`RectangleRenderer.ts:248-311`); **block cursor** is not a rect: it
  overrides the cell's fg/bg in the model before glyph draw (`WebglRenderer.ts:526-548`).
  **Selection is not a pass either**: it's blended into per-cell bg/fg during model update
  (`CellColorResolver.ts:82-175`) and thus lands in the bg-rect merge. Blending:
  `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` enabled once (`GlyphRenderer.ts:208-210`); context created with
  `{antialias:false, depth:false}` (`WebglRenderer.ts:91-97`).

### 2.2 K2 recommended passes

All passes share one canvas/context; back-to-front:

1. **Background pass** (rect program): full-viewport theme-bg rect, then per-row merged bg rects.
   Input: expanded rows; a run with `bg=None && !inverse` emits nothing (matches wire trimming
   semantics, `grid_snapshot.rs:251-253`). Runs arrive pre-coalesced by style — the merge is
   simply "extend rect while adjacent runs share resolved bg", cheaper than xterm's per-cell scan.
2. **Selection pass** (same rect program, ≤3 rects): translucent theme selection color over
   head/body/tail rows (§4). Drawn after bg, before glyphs, so glyphs stay full-contrast — this is
   K2's deliberate divergence from xterm's bake-into-cell-colors approach; our daemon-resolved
   colors leave no client hook for per-cell blending, and a translucent overlay matches the current
   native-selection look.
3. **Glyph pass** (instanced, one draw): §2.3 layout.
4. **Decoration pass** (rect program): underline + strikethrough bars, one rect per decorated run
   (thickness `max(1, floor(fontSize*dpr/15))`, y from xterm's math `TextureAtlas.ts:549-576,742-752`).
5. **Cursor**: stays a DOM overlay (§5) — do NOT port xterm's cursor rects/color-override.

Rationale for pass order and for keeping rects+glyphs in two programs: identical to xterm
(`WebglRenderer.ts:398-404`); program switches are 2/frame, negligible.

### 2.3 K2 instance layout (glyph pass)

Fixed slot per visible cell: `index = visualRow * cols + col`, **12 floats (48 B) per cell**:

| offset | attr | contents |
|---|---|---|
| 0-1 | `a_offset` (2f) | trimmed-glyph px offset within cell (xterm semantics) |
| 2-3 | `a_size` (2f) | glyph quad size in clip-normalized units |
| 4 | `a_texpage` (1f) | atlas page index; **negative ⇒ color glyph** (emoji path, §1.3) |
| 5-6 | `a_texcoord` (2f) | atlas UV origin |
| 7-8 | `a_texsize` (2f) | atlas UV size |
| 9-11 | `a_fg` (3f) | fg RGB 0-1; **dim premultiplies a u_dimAlpha into the shader's alpha** — pack dim as fg alpha by widening to 4f if preferred (13 floats) — build agent's choice, document it |

Drop xterm's `a_cellpos` 2 floats: derive `cellpos` in the vertex shader from `gl_InstanceID`
with `u_cols`/`u_rows` uniforms (`float(gl_InstanceID % u_cols) / u_cols`, integer ops are fine in
GLSL ES 3.00). Saves 8 B/cell and removes the resize re-init loop (`GlyphRenderer.ts:313-319`).
Fragment shader: monochrome → `v_fg * texture(page, uv).a`; color-glyph → `texture(page, uv)`;
≤4 page conditionals (§1.3) vs xterm's up-to-32 chain (`GlyphRenderer.ts:59-79`).

Keep xterm's **double-buffered upload** (`GlyphRenderer.ts:15-25,340-361`): pack visible rows into
alternating CPU `Float32Array`s, one `bufferData(STREAM_DRAW)`, one `drawElementsInstanced`. At a
maxed 300×80 pane that's 24k cells ≈ 1.15 MB/frame — xterm ships the same order of magnitude and
VS Code measured frames <1 ms average (docs/terminal-scrolling-research.md:221).

### 2.4 Run→cell expansion and row-damage application

The painter keeps a **per-row expanded cache**: `Map<CellRun[] /*by reference*/, ExpandedRow>`
where `ExpandedRow` is the row's pre-built 12-float-per-cell slab (cols × 48 B) plus its bg/deco
rect list. Because `mergeDelta` preserves row references for undamaged rows and never rebuilds
scrollback rows (`TerminalPane.tsx:319-352`, comment at 218-223), cache hits ARE the damage test —
the painter re-expands exactly the rows the daemon damaged, mirroring glide-data-grid's
damage-set discipline (`glide:packages/core/src/internal/data-grid/render/data-grid-render.cells.ts:202-204`
— skip every cell not in the damage set) without needing an explicit damage set. Use a WeakMap +
generation sweep, or an LRU sized ~4× viewport rows; scrollback rows off-window can be evicted
freely.

Expansion rules (per run, walking a `col` cursor):

- Iterate `run.text` **by Unicode code point** (`for..of`), one column per code point.
- **Wide chars (CJK/emoji)**: alacritty stores a spacer cell after the wide char; the wire renders
  it as a space (`cell_to_run` maps `'\0'`→`' '`, `grid_snapshot.rs:228-245`), so the run text
  already contains the spacer column. Rule: if `wcwidth(cp) == 2`, emit the glyph instance sized
  2 cells (xterm's wide-glyph handling: the trailing cell writes NULL and is skipped,
  `GlyphRenderer.ts:228-235`; Zed: spacer contributes bg only,
  `zed:crates/terminal_view/src/terminal_element.rs:390-393`), then **consume the next code point
  as the spacer** (skip glyph, keep bg). Ship a small wcwidth table (or `Intl.Segmenter` +
  East-Asian-Width ranges) in the painter.
- `inverse` → swap fg/bg **after** resolving `None` against theme defaults (exactly `runStyle`,
  `TerminalPane.tsx:155-196`).
- Space/blank cells with no decoration → zero the glyph slot (xterm nulls but keeps
  underline-able spaces, `GlyphRenderer.ts:230-235`) — in K2 underline-on-space is handled by the
  decoration rect pass, so blank glyph slots are always zeroed.
- Rows shorter than `cols` (wire trims trailing blanks): zero-fill remaining slots; row slabs are
  allocated at full `cols` width so slot indexing stays fixed.
- **Known wire limitation, not a painter regression**: combining marks / zerowidth chars live in
  alacritty's `cell.extra` and are dropped by `cell_to_run` today (`grid_snapshot.rs:228-245`);
  astral-plane code points also break the DOM copy path's UTF-16 column math
  (`TerminalPane.tsx:268-283`). The by-code-point expansion above is forward-compatible; fixing
  zerowidth requires a wire change (out of scope).

Frame assembly: for each of the `rows` visible slots, look up `visibleRows[i]` (same windowing as
`TerminalPane.tsx:2047-2065`), memcpy its cached slab into the packed upload buffer (`.set()`),
concatenate its rect lists. Damaged/new rows expand first, everything else is copy-only. This is
xterm's per-frame pack loop (`GlyphRenderer.ts:344-357`) with the expansion amortized behind the
row cache.

---

## 3. Scroll

**Recommendation: rebuild the packed instance buffer from the row cache on offset change; add a
sub-cell pixel offset uniform for future fractional smoothness. Do not blit, do not keep a
GPU-resident scrollback ring.**

- A scroll changes only *which* cached rows are packed (window arithmetic identical to
  `TerminalPane.tsx:2047-2065`). Packing = `rows × cols × 48 B` of `Float32Array.set` — tens of
  microseconds. This is the GPU analog of the "keep row content static, move the viewport"
  discipline (docs/terminal-scrolling-research.md:119-131), and it's precisely what xterm does per
  frame anyway (`GlyphRenderer.ts:344-361`). glide-data-grid's canvas self-blit + repaint-exposed-
  strips (`glide:…/data-grid-render.blit.ts:21-191`) is the Canvas2D equivalent; with instanced
  GL the full repack is cheaper than blit bookkeeping and has no diagonal/frozen-region edge cases
  (they bail to full redraw for those, `blit.ts:75-79`).
- **Wheel pipeline stays exactly as-is**: accumulate → rAF flush → `setViewportOffset`
  (`TerminalPane.tsx:2521-2555`), including the mouse-report SGR forwarding branch
  (`TerminalPane.tsx:2450-2519`) — the painter only ever sees the resolved
  `{snapshot, viewportOffset}` pair per frame (§6).
- **Fractional smooth scroll (phase 2, cheap once plumbing exists)**: keep `scrollAccumRef`'s
  sub-line remainder (already preserved, `TerminalPane.tsx:2545-2546`) and pass
  `u_scrollOffsetPx = remainder × dpr` to both programs; render `rows + 1` slots and translate all
  y by the fraction; snap to 0 when scrolling settles. This is the "fractional lines in the
  renderer" approach (docs/terminal-scrolling-research.md:239-243, Ghostty #2355 framing) and needs
  the DOM overlays (cursor) to apply the same CSS translate — gate it behind offset ≠ 0 so the
  cursor row (only visible at offset 0, `TerminalPane.tsx:2686`) never needs the translate.
- **Scrollback append at bottom**: no special case. When pinned (`offset == 0`) the window formula
  slides automatically; appended rows are new references → expanded on first pack. When scrolled up
  (`offset > 0`) current K2 semantics keep the offset (distance-from-bottom) constant, so content
  visually flows past — the painter must **mirror, not "fix"**, this (an anchor-to-content mode =
  increment offset per appended row is a product decision for later; the clamp effect at
  `TerminalPane.tsx:2576-2584` stays authoritative either way).
- Big scrollbacks: the row cache holds expanded slabs only for rows that have been visible;
  eviction keeps memory O(viewport), not O(history) — the JS mirror of history remains the
  `snapshot.scrollback` array that already exists.

---

## 4. Selection over canvas

### 4.1 How xterm models + renders it

- `SelectionModel` stores `selectionStart/End` as `[col, bufferRow]` (absolute row incl.
  scrollback) plus `selectionStartLength` for word/line selects; `finalSelectionStart/End`
  normalize reversed drags (`xterm:src/browser/selection/SelectionModel.ts:28-116`).
- Pixel→cell: `getCoords` — subtract element rect + padding, divide by CSS cell size, **selection
  adds half a cell to x** so the left half of a cell selects it and the right half selects the next
  (`xterm:src/browser/input/Mouse.ts:33-54`).
- `SelectionService` owns mousedown/move/up, double-click word / triple-click line modes, wide-char
  end-inclusion, and a **drag-scroll interval** that scrolls the viewport while the pointer is
  outside and pins `selectionEnd` to viewport edges
  (`xterm:src/browser/services/SelectionService.ts:619-716`); refresh is rAF-throttled
  (`SelectionService.ts:279-306`).
- Rendering: `SelectionRenderModel` translates buffer coords → viewport-clamped rows
  (`xterm:src/browser/renderer/shared/SelectionRenderModel.ts:39-88`); the WebGL renderer then
  **recolors cells** via `CellColorResolver` (bg ← selection color, blend if cell had bg;
  `CellColorResolver.ts:82-130`) so selection rides the bg-rect pass. Copy uses the buffer model
  (`selectionText`, `SelectionService.ts:203`), never the pixels.

### 4.2 K2 plan

**Model** (new, tiny, outside React render path — a ref + version bump):
`{ startAbs, startCol, endAbs, endCol, mode: 'char'|'word'|'line' }` in **absolute row coords**
(same space as `data-abs-row` today), normalized on read like `finalSelectionStart/End`
(`SelectionModel.ts:53-116`). Absolute coords make the selection survive scrolling AND
scrollback-append shifts for free (K2 absolute row indices never shift — scrollback only appends;
xterm needs `handleTrim` compensation, `SelectionModel.ts:123-143`, only because their buffer
trims; K2's daemon-capped scrollback does trim on the daemon but the client mirror today only
grows within a snapshot generation — on full-snapshot replace, clear the selection).

**Mouse math**: reuse the pane's existing pixel→cell idiom —
`col = floor((clientX - rect.left - 4) / cellW)`, `visualRow = floor((clientY - rect.top - 4) / cellH)`
(`TerminalPane.tsx:2468-2478`, hover-link uses the same, `TerminalPane.tsx:2107-2140`), then
`abs = (totalLen - viewportOffset - rows) + visualRow` (inverse of `TerminalPane.tsx:2053-2058`).
Add xterm's half-cell x rounding for the drag end (`Mouse.ts:44`). Wide-char end-inclusion: if the
end column lands on a spacer column (expansion knows, §2.4), extend by one
(`SelectionService.ts:670-678`).

**Interactions**: mousedown (button 0, not on a link) sets anchor + installs window-level
move/up listeners (pattern already in the pane for drag guards,
`TerminalPane.tsx:2886-2909`); dblclick → word mode (expand via `rowToText` +
`/[^ ]/` boundaries, wrapped-row aware using `isRowWrapped`, `TerminalPane.tsx:244-253`);
triple-click → line mode; **drag auto-scroll**: interval timer nudging `setViewportOffset` while
pointer is above/below the pane, selectionEnd pinned to window edge (port
`SelectionService.ts:692-716`). Mouse-report mode (`snap.mouseReport`) suppresses selection unless
a modifier is held — same conditional the wheel handler already applies
(`TerminalPane.tsx:2463`).

**Rendering**: selection pass rects (§2.2): ≤3 rects computed from the normalized model clipped to
the visible window (port the viewport-clamp math of `SelectionRenderModel.ts:39-69`). Repaint =
bump painter frame; no instance-buffer touch.

**Copy**: `handleCopy` already rebuilds text purely from the CellRun model given
`(startAbs, startCol, endAbs, endCol)` — including trailing-space trim and wrapped-line join
(`TerminalPane.tsx:2224-2267`). **Refactor step**: extract the loop at
`TerminalPane.tsx:2243-2264` into `buildCopyText(snap, startAbs, startCol, endAbs, endCol)`;
DOM path keeps deriving the 4 coords from `window.getSelection()` + `data-abs-row`
(`TerminalPane.tsx:2234-2241`), canvas path feeds the selection model directly. **Copy trigger**:
with no native selection the `copy` event may not fire in WKWebView — intercept Cmd+C in the
pane's key handling (keys already funnel through the shadow textarea) when the selection model is
non-empty and write via `navigator.clipboard.writeText` (keyboard gesture ⇒ permitted); keep the
`onCopy` handler as the DOM-path fallback. Also honor "copy on select for middle-click paste"
never — not a K2 behavior today.

Set `user-select: none` on the container when the flag is on so stray native selection of overlay
text (debug HUD, cursor char) can't fight the model.

---

## 5. What stays DOM (confirmations + z-order)

The canvas is inserted as the **first child** of the pane container (below all overlays), sized to
the row area, `pointer-events: none` (all mouse handlers already live on the container div,
`TerminalPane.tsx:2886-2913`). Everything below already positions absolutely against the container
and needs zero structural change:

| Element | Today | Over canvas | Notes |
|---|---|---|---|
| **Cursor overlay** | absolutely-positioned div, `pointerEvents:'none'`, solid/hollow + TUI-inverse-cell variant (`TerminalPane.tsx:2675-2784,2947-2951`) | ✅ unchanged | Scenario B (TUI hollow cursor) scans grid runs for `inverse` — model-based, canvas-agnostic. Keep it DOM: xterm renders cursor on canvas only because it has no DOM; our overlay avoids per-blink GL redraws. |
| **IME shadow textarea** | 1-cell textarea following the cursor, frozen during composition, `opacity:0; zIndex:-5` (`TerminalPane.tsx:2625-2673,2923-2934`) | ✅ unchanged | xterm does the identical trick (`xterm:src/browser/CoreBrowserTerminal.ts:338-360`, textarea `zIndex:-5`). `zIndex:-5` puts it *behind the container background* today; verify it stays behind the canvas too (canvas is opaque — fine, the textarea is invisible by opacity anyway). |
| **Link hover/click** | model-based hit test (cellMetrics + visibleRows) + container `cursor:pointer`; no DOM text involved (`TerminalPane.tsx:2093-2140,2812-2814`) | ✅ unchanged | If an underline-on-hover affordance is ever wanted, add it to the decoration rect pass (xterm uses a dedicated 2D canvas layer for this — `xterm:addons/addon-webgl/src/renderLayer/LinkRenderLayer.ts:55-77`, `BaseRenderLayer.ts:42-46` — we don't need a whole layer for it). |
| **Scrollbar** | none rendered today (viewportOffset only, debug HUD shows `off:`) | ✅ any future scrollbar = DOM overlay div | Reads `viewportOffset`/`scrollback.length`; `pointer-events:auto` on the thumb only. |
| **Debug HUD / compose bar / drop targets** | DOM (`TerminalPane.tsx:2956-2981,2991-2993`, drag-drop 2269-2330) | ✅ unchanged | Drop hit-testing targets the container, not rows. |

Z-order contract (bottom→top): canvas → shadow textarea (invisible) → cursor overlay → debug HUD.
All overlays keep `pointerEvents:'none'` except the container itself.

**One real loss to record**: native-selection accessibility. xterm pairs its canvas with an
`AccessibilityManager` DOM mirror (`xterm:src/browser/CoreBrowserTerminal.ts:294-298`). Out of
scope for v1; the DOM renderer remains one flag-flip away (§6).

---

## 6. Integration seam

### 6.1 The `TerminalPainter` interface

```
interface PainterFrame {
  snapshot: TermGridSnapshot        // the merged, post-rAF-coalesce object
  viewportOffset: number
  selection: SelectionRange | null  // absolute grid coords (§4)
  theme: { fg: number; bg: number; selection: number }
}
interface TerminalPainter {
  mount(container: HTMLElement): void
  setMetrics(m: { cssCellW: number; cssCellH: number; dpr: number;
                  fontFamily: string; fontSize: number; padding: number }): void
  render(frame: PainterFrame): void            // idempotent; cheap when nothing changed
  resize(cols: number, rows: number): void
  onFatal(cb: (reason: string) => void): void  // context lost & unrestored, shader compile fail…
  dispose(): void
}
```

`TerminalPane` keeps sole ownership of: WS lifecycle + reconnect, `mergeDelta`, rAF coalescing
(`TerminalPane.tsx:557-595`), wheel/keyboard/IME/link/drag handlers, resize protocol
(`TerminalPane.tsx:1658-1665,2400-2426`), and all DOM overlays. When the flag is on it renders the
canvas host div instead of the row divs and calls `painter.render(...)` from a `useLayoutEffect`
keyed on `[snapshot, viewportOffset, selectionVersion, theme]`.

### 6.2 Snapshot vs raw deltas — consume the **merged snapshot**

Recommendation with justification (this was an explicit open question,
`.k2/prds/terminal-smoothness-rnd.md:270-273`):

1. **The coalescer already exists and is correct** — per-rAF apply, snapshot-supersedes,
   starvation cap (`TerminalPane.tsx:557-595`). Feeding the painter raw deltas would duplicate
   that logic and create two sources of truth for "current grid".
2. **Row-reference stability gives the painter row-level damage for free** (§2.4). A raw-delta
   feed gives the same information (damagedRows) but forces the painter to *also* maintain the
   merged mirror for scrollback windowing, copy, and resync — i.e. re-implement `mergeDelta`.
3. **One code path** serves full snapshot, delta, reconnect-resync, and multi-frame coalesced
   bursts identically; painter stays a pure `(frame) → pixels` function — unit-testable headless
   (feedback_daemon_first / thin-client invariant).
4. Cost of the indirection is one Map lookup per visible row per frame; the expansion work is
   identical either way. If profiling ever shows the lookup mattering, `mergeDelta` can hand the
   painter the damaged-row index list as an optional hint without changing the seam.

### 6.3 Feature flag

`k2:src/renderer/stores/terminal-settings.ts` — add `painter: 'dom' | 'webgl'` (default `'dom'`)
alongside `renderer` (`terminal-settings.ts:38-52`), persisted via the existing partialize +
bump `version: 3 → 4` with a migrate default (`terminal-settings.ts:115-152`). Keep it out of the
Settings UI until the final PR (dev toggle via DevTools/localStorage, mirroring how `renderer`
options were staged, `terminal-settings.ts:102-113`). Flag reads at pane mount; changing it
affects new panes only (same contract as `renderer`, `terminal-settings.ts:29-31`).

### 6.4 Lifecycle

- **Mount/unmount**: painter created in a `useEffect` gated on `phase.kind === 'ready' && flag`;
  `dispose()` deletes program/buffers/textures + removes canvas (xterm's disposal discipline:
  everything registered, `GlyphRenderer.ts:131-206`; canvas removed on renderer dispose,
  `WebglRenderer.ts:161-169`).
- **Context loss/restore** (port exactly): `webglcontextlost` → `e.preventDefault()` + 3 s timer;
  `webglcontextrestored` → clear timer, drop atlas cache, reinit all GL state, full redraw; timer
  expiry → fatal (`WebglRenderer.ts:125-146`). K2's `onFatal` handler flips the pane to the DOM
  strip **for that pane instance** and toasts once — the DOM renderer is the permanent fallback
  (VS Code does the same demotion via `onContextLoss`; addon self-removal restores the previous
  renderer, `xterm:addons/addon-webgl/src/WebglAddon.ts:84-97`).
- **Resize**: existing ResizeObserver → cols/rows → `sendResize` stays (`TerminalPane.tsx:2400-2426`);
  painter additionally observes `devicePixelContentBoxSize` for exact backing-store pixels
  (`DevicePixelObserver.ts:8-40`) and re-derives metrics per §1.3. Grid-size change from the
  daemon (`snapshot.cols/rows`) → `painter.resize` → reallocate slabs/buffers (xterm:
  `WebglRenderer.ts:192-227`, `GlyphRenderer.ts:293-320`).
- **Visibility**: rAF starvation while occluded is already handled upstream by the coalescer cap
  (`TerminalPane.tsx:583-589`); painter renders only inside `render()` — no self-scheduling loop.

---

## 7. Pitfalls (what bit xterm.js / will bite us)

1. **Context loss** — WKWebView reclaims GL contexts under GPU memory pressure; many panes = many
   contexts. Mitigations: the §6.4 restore protocol; keep one canvas per pane (not per layer —
   skip xterm's extra 2D layer canvases, `BaseRenderLayer.ts:42-46`); `onFatal` → DOM fallback.
   Consider `preserveDrawingBuffer:false` default (xterm exposes it as an option only,
   `WebglRenderer.ts:92-96`).
2. **Texture-unit / atlas-page complexity** — xterm's page-merge machinery
   (`TextureAtlas.ts:154-259`), mid-frame merge retry loop (`WebglRenderer.ts:386-396`) and
   32-branch fragment shader (`GlyphRenderer.ts:59-79`) all exist because color-baked atlases
   overflow. The §1.3 white-glyph atlas + single-growing-page design avoids the entire class; keep
   the hard cap (4 pages) + clear-and-warm-up escape hatch. Never exceed
   `MAX_TEXTURE_SIZE`/4096 (`TextureAtlas.ts:40-46`, `GlyphRenderer.ts:124-129`).
3. **Subpixel positioning / blur** — device-int glyph grid: floor char width, ceil char height,
   round CSS canvas size (fractional-DPR blur note, `WebglRenderer.ts:667-673`);
   `devicePixelContentBoxSize` observation (`DevicePixelObserver.ts:8-40`); no mid-cell subpixel
   variants needed on a grid (Zed's 4×4 variants, `zed:…/window.rs:3309-3326`, are for arbitrary
   text layout — explicitly skip). Do **not** `generateMipmap` (xterm does per upload,
   `GlyphRenderer.ts:393` — wasted work at 1:1 sampling; use LINEAR/NEAREST min filter).
4. **Ligatures — explicitly deferred.** xterm needs a separate addon that registers a character
   joiner + `font-feature-settings: "calt"` (`xterm:addons/addon-ligatures/src/LigaturesAddon.ts:36-43`),
   rasterizes joined ranges as combined-char glyphs, and *breaks joins* when selection state or the
   cursor differ across the range (`WebglRenderer.ts:485-511`). K2's wire is strictly per-column
   (daemon-side alacritty has no shaping), so the painter draws per-cell; note the current DOM
   renderer *may* incidentally shape ligatures inside a coalesced `<span>` — flag-on behavior
   change to document, not fix. Revisit only if users notice.
5. **Box-drawing / powerline seams** — font-rendered `─│┌`/powerline glyphs don't reliably fill the
   cell → hairline gaps, and AA edges shimmer at fractional sizes. xterm ships a full procedural
   rasterizer for these (`xterm:addons/addon-webgl/src/customGlyphs/CustomGlyphRasterizer.ts:15-41`,
   1019-line definitions table) and strips padding for powerline so edges butt exactly
   (`TextureAtlas.ts:521-527`, vscode#120129). Zed instead just widens/keeps font glyphs and skips
   contrast-tweaks for U+2500-25FF + powerline PUA (`zed:…/terminal_element.rs:524-558`). K2 v1:
   font path + accept seams; port `customGlyphs` (self-contained, MIT) as the dedicated stretch PR.
   Related: glyphs overdrawing the neighbor cell — xterm clips the glyph's left overhang when the
   bg changes at the cell boundary (`GlyphRenderer.ts:249-265`) and offers
   `rescaleOverlappingGlyphs` (`GlyphRenderer.ts:284-290`); adopt the bg-boundary clip, skip the
   rescale option initially.
6. **Alpha blending order / premultiplication** — xterm relies on an opaque full-viewport bg rect
   first (`RectangleRenderer.ts:186-197`) + `SRC_ALPHA, ONE_MINUS_SRC_ALPHA`
   (`GlyphRenderer.ts:208-210`); their `clearColor` erase exists partly to stop dark fringes where
   AA edges blended against the baked bg (`TextureAtlas.ts:1120-1127`). K2: request the context
   with `{alpha:false, antialias:false, depth:false}` (opaque canvas dodges page-compositing
   premultiply surprises in WebKit), always draw the full bg rect, and rasterize glyphs white — AA
   fringes then tint with the glyph and never halo. Leave `UNPACK_PREMULTIPLY_ALPHA_WEBGL` at
   default false for canvas uploads (matches xterm) and verify against a colored-bg TUI screen.
7. **WKWebView specifics** — WebGL2 exists in Safari/WebKit ≥16 only; xterm hard-gates
   (`WebglAddon.ts:34-45`). K2's own earlier note "WebGL broken in WKWebView / invisible text"
   (`docs/terminal-rendering-research.md:300-302,443`) dates from the pre-Safari-16 era — treat it
   as stale but **verify on-device first** (PR1 acceptance includes a rendered-pixels readback
   sanity check). rAF throttling in occluded webviews is handled upstream (§6.4). Canvas element
   max dimensions in WebKit (4096+ safe; huge panes on 5K displays: canvas = rows×cellH device px,
   fine). Tauri specifics: no user-gesture context for WS-driven paints was the *bitmap* era
   problem (`docs/terminal-rendering-research.md:267-284`) — WebGL draws present via the normal
   compositor path (same as xterm-in-Safari), but keep the PR1 smoke test on a real streaming
   session to confirm no stale-presentation issue resurfaces.
8. **GC discipline in the hot loop** — per-cell code must not allocate: xterm hoists work
   variables to module scope (`GlyphRenderer.ts:87-91`, `WebglRenderer.ts:421-439`,
   `CellColorResolver.ts:12-19`) and pre-allocates model arrays. K2's expansion cache (§2.4) keeps
   per-frame allocation to zero on the steady path (glide's same rule:
   `glide:…/data-grid-render.cells.ts:196-204`).
9. **First-paint blanks** — atlas misses on the first frame draw nothing until rasterized; xterm
   pre-warms ASCII 33-126 in idle tasks (`TextureAtlas.ts:115-133`) and paints red placeholder
   textures if a page is unexpectedly unbound (`GlyphRenderer.ts:194-206`). Port the warm-up;
   rasterize-on-miss is synchronous anyway (Canvas2D), so blanks only occur if rasterization is
   deferred — don't defer.
10. **Multi-pane atlas sharing** — xterm shares atlases across terminals keyed by config
    (`CharAtlasCache.ts:27-79`) and notes the cache can leak terminals
    (`CharAtlasCache.ts:15-18`). K2 has split panes + many tabs with identical font config: share
    one atlas module-wide keyed by (fontFamily, fontSize, dpr, bold/italic weights), refcounted on
    painter dispose — but only in the perf PR; start per-pane for simplicity.

---

## 8. Effort map — 6 PR-sized slices (flag stays `'dom'`-default until slice 6)

Each slice lands green (vitest for pure logic per feedback_test_discipline; manual harness via the
dev-only flag) and is independently revertible.

1. **Seam + flag + rect skeleton** — `painter` setting (v4 migration) in
   `terminal-settings.ts`; extract `buildCopyText` from `handleCopy` (pure refactor, DOM path
   re-verified); `TerminalPainter` interface; canvas host + WebGL2 context + context-loss protocol
   + `onFatal`→DOM fallback; **background pass only** (theme bg + per-run bg rects from the
   snapshot). *Test*: flag on → correct bg rectangles under `htop`/vim; context-kill via
   `WEBGL_lose_context` extension → clean DOM fallback; on-device WKWebView pixel-readback sanity.
2. **Glyph atlas + glyph pass** — white-glyph atlas (packing, trim, warm-up, page growth),
   run→cell expansion with row cache + wcwidth, instanced glyph draw, dim/inverse/bold/italic,
   emoji color-path flag; DOM overlays (cursor/IME/HUD) verified above canvas. *Test*: expansion
   unit tests (wide chars, spacers, inverse-default swap, trimmed rows); visual A/B vs DOM
   renderer on a golden screen; `cat` of a UTF-8 torture file.
3. **Scroll + resize + lifecycle hardening** — viewport windowing from the row cache, scrollback
   append, offset clamp parity (`TerminalPane.tsx:2576-2584`), cols/rows resize, font-size zoom
   rebuild, DPR-change rebuild (drag window across monitors), `devicePixelContentBoxSize`.
   *Test*: `yes | head -100000` flood + flick-scroll stays coherent; zoom in/out; 1.25-DPR blur
   check.
4. **Selection + copy** — selection model, mouse handlers incl. word/line modes + drag
   auto-scroll, selection rect pass, Cmd+C → `buildCopyText` → clipboard, mouse-report suppression.
   *Test*: unit tests for coord normalization + word expansion + wrapped-join copy parity with the
   DOM path; multi-screen drag-scroll select.
5. **Decorations + polish** — underline/strikethrough rect pass, bg-boundary glyph clip,
   shared-atlas refcounting across panes, perf pass (frame-time HUD in the dev overlay, zero-alloc
   audit), optional: `customGlyphs` box-drawing port. *Test*: TUI gallery (btop, claude, vim,
   tmux) A/B; Performance-panel frame times vs DOM under streaming agent output.
6. **Expose + default-flip staging** — Settings UI toggle (labeled experimental), telemetry-free
   soak on dev machines, then flip the default for fresh installs only (mirroring the
   Kessel rollout convention, `terminal-settings.ts:64-72`). DOM renderer remains the
   permanent fallback path and the mouse-report/alt-screen behaviors stay byte-identical on the
   wire.

Explicit non-goals for this arc: ligatures (§7.4), accessibility DOM mirror (§5), binary wire
frames / flow control / scroll-copy-refs (separate Phase-1/-3 tracks in
`.k2/prds/terminal-smoothness-rnd.md:286-304`), fractional smooth scroll (staged behind §3's
uniform once slice 3 lands).
