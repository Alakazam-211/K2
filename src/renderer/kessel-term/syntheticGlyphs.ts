// Synthetic CSS geometry for box-drawing + block-element glyphs.
//
// What xterm.js "custom glyphs", kitty and iTerm2 do: these ranges
// are GEOMETRY pretending to be text, and font ink is the wrong tool
// for them — a font glyph never reliably fills its line box (Claude
// Code's stacked-block logo shows horizontal seams between rows) and
// its stroke ends never reliably reach the cell edges (TUI box
// borders get hairline breaks/misjoins at some zooms). So the DOM
// painter REPLACES font rendering for these code points with painted
// rectangles/strokes that span exact cell fractions.
//
// ARCHITECTURE (three pure stages, all unit-tested without DOM):
//   1. syntheticGlyphSpec(cp)   → resolution-independent descriptor
//      (or null ⇒ render as text exactly like today).
//   2. glyphMetrics(cellW, cellH, dpr) → the shared per-cell device-
//      pixel geometry: stroke bands, double-line offsets. Computed
//      ONCE per metrics (memoized), so stroke thickness is identical
//      for every cell of the same weight — never per-glyph.
//   3. paintSyntheticGlyph(spec, …, color) → CSS: N rectangles as N
//      stacked `linear-gradient` background layers on the ONE
//      existing per-char cell span (zero extra DOM nodes); dashes as
//      repeating-linear-gradients; only the four rounded arcs ╭╮╰╯
//      need a single child div (a border-radius stroke corner —
//      gradients can't express an arc).
//
// DEVICE-PIXEL ROUNDING: every interior coordinate is computed in
// device pixels — round(fraction × cellDim × dpr) — then divided
// back to CSS px. Adjacent cells evaluate the same formula, so a
// rect's end and its neighbor's start land on the SAME device pixel:
// runs of ─ are one continuous crisp line and stacked blocks tile
// with zero gaps. Cell-edge coordinates are special-cased: a
// full-span axis uses `100%` and a far-edge-touching rect gets one
// device pixel of slack (backgrounds clip at the box edge, so the
// overshoot is free) — this keeps far edges flush even though the
// cell box itself may sit at a fractional device position.
//
// COVERAGE:
//   U+2500–257F box drawing — painted, except ╱╲╳ (U+2571–2573,
//     diagonals: gradients can't draw a crisp thin diagonal; they
//     stay font-rendered rather than be half-included).
//   U+2580–259F block elements — fully painted (halves, eighths,
//     quadrants; shades ░▒▓ as 25/50/75% alpha fills).
//   U+1FB00–1FB3B legacy-computing sextants — painted (2×3 grids;
//     the same rect machinery for free).
//   Powerline PUA (U+E0B0–E0B3) — EXCLUDED: the solid triangles
//     would clip-path cleanly but the chevron outlines wouldn't, and
//     patched Nerd Fonts already ship correct glyphs there.

// ── Stage 1: descriptors ────────────────────────────────────────────

/** Arm weight for straight box-drawing lines. */
export type Arm = 0 | 1 | 2 // none | light | heavy

/** Fractional rect [x0, y0, x1, y1] in cell-fraction units (0..1,
 *  y grows downward). */
export type FracRect = readonly [number, number, number, number]

/** Symbolic coordinates for the double-line glyphs (U+2550–256C),
 *  resolved against GlyphMetrics. x-axis tokens: 0 | 'W' (cell
 *  edges), 'va'/'vb' (single vertical stroke band), 'v1a'/'v1b' /
 *  'v2a'/'v2b' (left/right strokes of a double vertical). y-axis
 *  mirrors with h. Hand-authoring these 29 glyphs as segment lists
 *  is what makes their junctions exact (e.g. ╬'s open center). */
export type XTok = 0 | 'W' | 'va' | 'vb' | 'v1a' | 'v1b' | 'v2a' | 'v2b'
export type YTok = 0 | 'H' | 'ha' | 'hb' | 'h1a' | 'h1b' | 'h2a' | 'h2b'
export type SymRect = readonly [XTok, YTok, XTok, YTok]

export type SyntheticGlyph =
  | { kind: 'rects'; rects: readonly FracRect[]; alpha?: number }
  | {
      kind: 'lines'
      /** [left, up, right, down] */
      arms: readonly [Arm, Arm, Arm, Arm]
      /** Dash count for the dashed line glyphs (2/3/4 dashes per
       *  cell); dashed glyphs are always pure horizontal/vertical. */
      dash?: 2 | 3 | 4
    }
  | { kind: 'double'; rects: readonly SymRect[] }
  /** Rounded arcs ╭╮╰╯ — named by which corner of the cell the
   *  curve turns through ('tl' = rounded ┌, i.e. ╭). */
  | { kind: 'arc'; corner: 'tl' | 'tr' | 'br' | 'bl' }

// U+2500–254B + 2574–257F: [left, up, right, down] arm weights.
const LINE_ARMS: Record<number, readonly [Arm, Arm, Arm, Arm]> = {
  0x2500: [1, 0, 1, 0], 0x2501: [2, 0, 2, 0],
  0x2502: [0, 1, 0, 1], 0x2503: [0, 2, 0, 2],
  0x250c: [0, 0, 1, 1], 0x250d: [0, 0, 2, 1],
  0x250e: [0, 0, 1, 2], 0x250f: [0, 0, 2, 2],
  0x2510: [1, 0, 0, 1], 0x2511: [2, 0, 0, 1],
  0x2512: [1, 0, 0, 2], 0x2513: [2, 0, 0, 2],
  0x2514: [0, 1, 1, 0], 0x2515: [0, 1, 2, 0],
  0x2516: [0, 2, 1, 0], 0x2517: [0, 2, 2, 0],
  0x2518: [1, 1, 0, 0], 0x2519: [2, 1, 0, 0],
  0x251a: [1, 2, 0, 0], 0x251b: [2, 2, 0, 0],
  0x251c: [0, 1, 1, 1], 0x251d: [0, 1, 2, 1],
  0x251e: [0, 2, 1, 1], 0x251f: [0, 1, 1, 2],
  0x2520: [0, 2, 1, 2], 0x2521: [0, 2, 2, 1],
  0x2522: [0, 1, 2, 2], 0x2523: [0, 2, 2, 2],
  0x2524: [1, 1, 0, 1], 0x2525: [2, 1, 0, 1],
  0x2526: [1, 2, 0, 1], 0x2527: [1, 1, 0, 2],
  0x2528: [1, 2, 0, 2], 0x2529: [2, 2, 0, 1],
  0x252a: [2, 1, 0, 2], 0x252b: [2, 2, 0, 2],
  0x252c: [1, 0, 1, 1], 0x252d: [2, 0, 1, 1],
  0x252e: [1, 0, 2, 1], 0x252f: [2, 0, 2, 1],
  0x2530: [1, 0, 1, 2], 0x2531: [2, 0, 1, 2],
  0x2532: [1, 0, 2, 2], 0x2533: [2, 0, 2, 2],
  0x2534: [1, 1, 1, 0], 0x2535: [2, 1, 1, 0],
  0x2536: [1, 1, 2, 0], 0x2537: [2, 1, 2, 0],
  0x2538: [1, 2, 1, 0], 0x2539: [2, 2, 1, 0],
  0x253a: [1, 2, 2, 0], 0x253b: [2, 2, 2, 0],
  0x253c: [1, 1, 1, 1], 0x253d: [2, 1, 1, 1],
  0x253e: [1, 1, 2, 1], 0x253f: [2, 1, 2, 1],
  0x2540: [1, 2, 1, 1], 0x2541: [1, 1, 1, 2],
  0x2542: [1, 2, 1, 2], 0x2543: [2, 2, 1, 1],
  0x2544: [1, 2, 2, 1], 0x2545: [2, 1, 1, 2],
  0x2546: [1, 1, 2, 2], 0x2547: [2, 2, 2, 1],
  0x2548: [2, 1, 2, 2], 0x2549: [2, 2, 1, 2],
  0x254a: [1, 2, 2, 2], 0x254b: [2, 2, 2, 2],
  // Half lines + mixed-weight full lines.
  0x2574: [1, 0, 0, 0], 0x2575: [0, 1, 0, 0],
  0x2576: [0, 0, 1, 0], 0x2577: [0, 0, 0, 1],
  0x2578: [2, 0, 0, 0], 0x2579: [0, 2, 0, 0],
  0x257a: [0, 0, 2, 0], 0x257b: [0, 0, 0, 2],
  0x257c: [1, 0, 2, 0], 0x257d: [0, 1, 0, 2],
  0x257e: [2, 0, 1, 0], 0x257f: [0, 2, 0, 1],
}

// Dashed lines: cp → [arms, dash count].
const DASH_ARMS: Record<
  number,
  readonly [readonly [Arm, Arm, Arm, Arm], 2 | 3 | 4]
> = {
  0x2504: [[1, 0, 1, 0], 3], 0x2505: [[2, 0, 2, 0], 3],
  0x2506: [[0, 1, 0, 1], 3], 0x2507: [[0, 2, 0, 2], 3],
  0x2508: [[1, 0, 1, 0], 4], 0x2509: [[2, 0, 2, 0], 4],
  0x250a: [[0, 1, 0, 1], 4], 0x250b: [[0, 2, 0, 2], 4],
  0x254c: [[1, 0, 1, 0], 2], 0x254d: [[2, 0, 2, 0], 2],
  0x254e: [[0, 1, 0, 1], 2], 0x254f: [[0, 2, 0, 2], 2],
}

// Double-line glyphs U+2550–256C as hand-authored segment lists.
// Convention: a double arm meeting a corner keeps the OUTER stroke
// long and the INNER stroke short (╔'s inner ┌ is nested inside its
// outer ┌); crossing arms leave the junction center open (╬).
const DOUBLE_RECTS: Record<number, readonly SymRect[]> = {
  0x2550: [[0, 'h1a', 'W', 'h1b'], [0, 'h2a', 'W', 'h2b']], // ═
  0x2551: [['v1a', 0, 'v1b', 'H'], ['v2a', 0, 'v2b', 'H']], // ║
  0x2552: [['va', 'h1a', 'W', 'h1b'], ['va', 'h2a', 'W', 'h2b'], ['va', 'h1a', 'vb', 'H']], // ╒
  0x2553: [['v1a', 'ha', 'v1b', 'H'], ['v2a', 'ha', 'v2b', 'H'], ['v1a', 'ha', 'W', 'hb']], // ╓
  0x2554: [['v1a', 'h1a', 'W', 'h1b'], ['v1a', 'h1a', 'v1b', 'H'], ['v2a', 'h2a', 'W', 'h2b'], ['v2a', 'h2a', 'v2b', 'H']], // ╔
  0x2555: [[0, 'h1a', 'vb', 'h1b'], [0, 'h2a', 'vb', 'h2b'], ['va', 'h1a', 'vb', 'H']], // ╕
  0x2556: [['v1a', 'ha', 'v1b', 'H'], ['v2a', 'ha', 'v2b', 'H'], [0, 'ha', 'v2b', 'hb']], // ╖
  0x2557: [[0, 'h1a', 'v2b', 'h1b'], ['v2a', 'h1a', 'v2b', 'H'], [0, 'h2a', 'v1b', 'h2b'], ['v1a', 'h2a', 'v1b', 'H']], // ╗
  0x2558: [['va', 'h1a', 'W', 'h1b'], ['va', 'h2a', 'W', 'h2b'], ['va', 0, 'vb', 'h2b']], // ╘
  0x2559: [['v1a', 0, 'v1b', 'hb'], ['v2a', 0, 'v2b', 'hb'], ['v1a', 'ha', 'W', 'hb']], // ╙
  0x255a: [['v1a', 0, 'v1b', 'h2b'], ['v1a', 'h2a', 'W', 'h2b'], ['v2a', 0, 'v2b', 'h1b'], ['v2a', 'h1a', 'W', 'h1b']], // ╚
  0x255b: [[0, 'h1a', 'vb', 'h1b'], [0, 'h2a', 'vb', 'h2b'], ['va', 0, 'vb', 'h2b']], // ╛
  0x255c: [['v1a', 0, 'v1b', 'hb'], ['v2a', 0, 'v2b', 'hb'], [0, 'ha', 'v2b', 'hb']], // ╜
  0x255d: [[0, 'h2a', 'v2b', 'h2b'], ['v2a', 0, 'v2b', 'h2b'], [0, 'h1a', 'v1b', 'h1b'], ['v1a', 0, 'v1b', 'h1b']], // ╝
  0x255e: [['va', 0, 'vb', 'H'], ['va', 'h1a', 'W', 'h1b'], ['va', 'h2a', 'W', 'h2b']], // ╞
  0x255f: [['v1a', 0, 'v1b', 'H'], ['v2a', 0, 'v2b', 'H'], ['v2a', 'ha', 'W', 'hb']], // ╟
  0x2560: [['v1a', 0, 'v1b', 'H'], ['v2a', 0, 'v2b', 'h1b'], ['v2a', 'h2a', 'v2b', 'H'], ['v2a', 'h1a', 'W', 'h1b'], ['v2a', 'h2a', 'W', 'h2b']], // ╠
  0x2561: [['va', 0, 'vb', 'H'], [0, 'h1a', 'vb', 'h1b'], [0, 'h2a', 'vb', 'h2b']], // ╡
  0x2562: [['v1a', 0, 'v1b', 'H'], ['v2a', 0, 'v2b', 'H'], [0, 'ha', 'v1b', 'hb']], // ╢
  0x2563: [['v2a', 0, 'v2b', 'H'], ['v1a', 0, 'v1b', 'h1b'], ['v1a', 'h2a', 'v1b', 'H'], [0, 'h1a', 'v1b', 'h1b'], [0, 'h2a', 'v1b', 'h2b']], // ╣
  0x2564: [[0, 'h1a', 'W', 'h1b'], [0, 'h2a', 'W', 'h2b'], ['va', 'h2a', 'vb', 'H']], // ╤
  0x2565: [[0, 'ha', 'W', 'hb'], ['v1a', 'ha', 'v1b', 'H'], ['v2a', 'ha', 'v2b', 'H']], // ╥
  0x2566: [[0, 'h1a', 'W', 'h1b'], [0, 'h2a', 'v1b', 'h2b'], ['v2a', 'h2a', 'W', 'h2b'], ['v1a', 'h2a', 'v1b', 'H'], ['v2a', 'h2a', 'v2b', 'H']], // ╦
  0x2567: [[0, 'h1a', 'W', 'h1b'], [0, 'h2a', 'W', 'h2b'], ['va', 0, 'vb', 'h1b']], // ╧
  0x2568: [[0, 'ha', 'W', 'hb'], ['v1a', 0, 'v1b', 'hb'], ['v2a', 0, 'v2b', 'hb']], // ╨
  0x2569: [[0, 'h2a', 'W', 'h2b'], [0, 'h1a', 'v1b', 'h1b'], ['v2a', 'h1a', 'W', 'h1b'], ['v1a', 0, 'v1b', 'h1b'], ['v2a', 0, 'v2b', 'h1b']], // ╩
  0x256a: [['va', 0, 'vb', 'H'], [0, 'h1a', 'W', 'h1b'], [0, 'h2a', 'W', 'h2b']], // ╪
  0x256b: [[0, 'ha', 'W', 'hb'], ['v1a', 0, 'v1b', 'H'], ['v2a', 0, 'v2b', 'H']], // ╫
  0x256c: [[0, 'h1a', 'v1b', 'h1b'], ['v2a', 'h1a', 'W', 'h1b'], [0, 'h2a', 'v1b', 'h2b'], ['v2a', 'h2a', 'W', 'h2b'], ['v1a', 0, 'v1b', 'h1b'], ['v1a', 'h2a', 'v1b', 'H'], ['v2a', 0, 'v2b', 'h1b'], ['v2a', 'h2a', 'v2b', 'H']], // ╬
}

const ARCS: Record<number, 'tl' | 'tr' | 'br' | 'bl'> = {
  0x256d: 'tl', // ╭ arc down and right
  0x256e: 'tr', // ╮ arc down and left
  0x256f: 'br', // ╯ arc up and left
  0x2570: 'bl', // ╰ arc up and right
}

// U+2580–259F block elements as fractional rects.
const E = 1 / 8
const BLOCKS: Record<number, readonly FracRect[]> = {
  0x2580: [[0, 0, 1, 0.5]], // ▀ upper half
  0x2581: [[0, 7 * E, 1, 1]], // ▁
  0x2582: [[0, 6 * E, 1, 1]], // ▂
  0x2583: [[0, 5 * E, 1, 1]], // ▃
  0x2584: [[0, 0.5, 1, 1]], // ▄ lower half
  0x2585: [[0, 3 * E, 1, 1]], // ▅
  0x2586: [[0, 2 * E, 1, 1]], // ▆
  0x2587: [[0, 1 * E, 1, 1]], // ▇
  0x2588: [[0, 0, 1, 1]], // █ full block
  0x2589: [[0, 0, 7 * E, 1]], // ▉
  0x258a: [[0, 0, 6 * E, 1]], // ▊
  0x258b: [[0, 0, 5 * E, 1]], // ▋
  0x258c: [[0, 0, 0.5, 1]], // ▌ left half
  0x258d: [[0, 0, 3 * E, 1]], // ▍
  0x258e: [[0, 0, 2 * E, 1]], // ▎
  0x258f: [[0, 0, 1 * E, 1]], // ▏
  0x2590: [[0.5, 0, 1, 1]], // ▐ right half
  0x2594: [[0, 0, 1, 1 * E]], // ▔ upper eighth
  0x2595: [[7 * E, 0, 1, 1]], // ▕ right eighth
  0x2596: [[0, 0.5, 0.5, 1]], // ▖
  0x2597: [[0.5, 0.5, 1, 1]], // ▗
  0x2598: [[0, 0, 0.5, 0.5]], // ▘
  0x2599: [[0, 0, 0.5, 1], [0.5, 0.5, 1, 1]], // ▙
  0x259a: [[0, 0, 0.5, 0.5], [0.5, 0.5, 1, 1]], // ▚
  0x259b: [[0, 0, 1, 0.5], [0, 0.5, 0.5, 1]], // ▛
  0x259c: [[0, 0, 1, 0.5], [0.5, 0.5, 1, 1]], // ▜
  0x259d: [[0.5, 0, 1, 0.5]], // ▝
  0x259e: [[0.5, 0, 1, 0.5], [0, 0.5, 0.5, 1]], // ▞
  0x259f: [[0.5, 0, 1, 1], [0, 0.5, 0.5, 1]], // ▟
}

// Shades: full-cell fg fill at fractional alpha.
const SHADES: Record<number, number> = {
  0x2591: 0.25, // ░
  0x2592: 0.5, // ▒
  0x2593: 0.75, // ▓
}

const FULL: readonly FracRect[] = [[0, 0, 1, 1]]

/** Sextant rects for U+1FB00–1FB3B. The block encodes the 60 2×3
 *  patterns not already representable elsewhere: pattern indices
 *  1..62 minus 21 (= ▌) and 42 (= ▐). Bit k of the pattern: cell
 *  (row k>>1, col k&1), rows top→bottom. */
function sextantRects(cp: number): readonly FracRect[] {
  const i = cp - 0x1fb00
  const pattern = i < 0x14 ? i + 1 : i < 0x28 ? i + 2 : i + 3
  const rects: FracRect[] = []
  for (let bit = 0; bit < 6; bit++) {
    if (!(pattern & (1 << bit))) continue
    const row = bit >> 1
    const col = bit & 1
    rects.push([col * 0.5, row / 3, (col + 1) * 0.5, (row + 1) / 3])
  }
  return rects
}

/** The lookup: paint descriptor for a code point, or null ⇒ the
 *  glyph renders as font text exactly like today. Total per-cp cost
 *  is a couple of range checks + one object-literal lookup. */
export function syntheticGlyphSpec(cp: number): SyntheticGlyph | null {
  if (cp >= 0x2500 && cp <= 0x257f) {
    const arms = LINE_ARMS[cp]
    if (arms) return { kind: 'lines', arms }
    const dash = DASH_ARMS[cp]
    if (dash) return { kind: 'lines', arms: dash[0], dash: dash[1] }
    const dbl = DOUBLE_RECTS[cp]
    if (dbl) return { kind: 'double', rects: dbl }
    const arc = ARCS[cp]
    if (arc) return { kind: 'arc', corner: arc }
    return null // ╱ ╲ ╳ — excluded (see header)
  }
  if (cp >= 0x2580 && cp <= 0x259f) {
    const rects = BLOCKS[cp]
    if (rects) return { kind: 'rects', rects }
    const alpha = SHADES[cp]
    if (alpha) return { kind: 'rects', rects: FULL, alpha }
    return null // unreachable: 2580–259F is fully covered
  }
  if (cp >= 0x1fb00 && cp <= 0x1fb3b) {
    return { kind: 'rects', rects: sextantRects(cp) }
  }
  return null
}

// ── Stage 2: device-pixel metrics ───────────────────────────────────

/** Per-cell paint geometry, all in DEVICE pixels. Derived once per
 *  (cellWidth, cellHeight, dpr) — every glyph of a frame shares the
 *  exact same stroke bands, which is what makes a run of ─ one
 *  continuous line instead of per-cell approximations. */
export interface GlyphMetrics {
  dpr: number
  /** Cell size in device px. */
  Wd: number
  Hd: number
  /** Light / heavy stroke thickness in device px. Light ≈ cellH/12
   *  (a 20px cell at 2× ⇒ 3 device px ≈ 1.5 css px, matching what
   *  kitty/xterm draw); heavy = 2× light. Always ≥ 1 device px. */
  lt: number
  ht: number
  /** Single vertical/horizontal stroke bands (light + heavy),
   *  centered on the cell midlines: [va,vb) etc. */
  va: number
  vb: number
  vha: number
  vhb: number
  ha: number
  hb: number
  hha: number
  hhb: number
  /** Double-line strokes: two light-weight bands per axis separated
   *  by a light-thickness gap, centered as a group. */
  v1a: number
  v1b: number
  v2a: number
  v2b: number
  h1a: number
  h1b: number
  h2a: number
  h2b: number
}

let metricsCache: { key: string; m: GlyphMetrics } | null = null

export function glyphMetrics(
  cellWidth: number,
  cellHeight: number,
  dpr: number,
): GlyphMetrics {
  const key = `${cellWidth}|${cellHeight}|${dpr}`
  if (metricsCache && metricsCache.key === key) return metricsCache.m
  const Wd = Math.max(1, Math.round(cellWidth * dpr))
  const Hd = Math.max(1, Math.round(cellHeight * dpr))
  const lt = Math.max(1, Math.round(Hd / 12))
  const ht = Math.max(lt + 1, lt * 2)
  const cx = Math.round(Wd / 2)
  const cy = Math.round(Hd / 2)
  const va = cx - (lt >> 1)
  const vha = cx - (ht >> 1)
  const ha = cy - (lt >> 1)
  const hha = cy - (ht >> 1)
  const gh = (lt + 1) >> 1 // half the double-line gap, rounded up
  const v1b = cx - gh
  const h1b = cy - gh
  const m: GlyphMetrics = {
    dpr,
    Wd,
    Hd,
    lt,
    ht,
    va,
    vb: va + lt,
    vha,
    vhb: vha + ht,
    ha,
    hb: ha + lt,
    hha,
    hhb: hha + ht,
    v1a: v1b - lt,
    v1b,
    v2a: v1b + lt,
    v2b: v1b + lt * 2,
    h1a: h1b - lt,
    h1b,
    h2a: h1b + lt,
    h2b: h1b + lt * 2,
  }
  metricsCache = { key, m }
  return m
}

/** One painted rectangle in device px, [x0,y0)–(x1,y1). x1 === Wd /
 *  y1 === Hd means "flush with the far cell edge" (the CSS builder
 *  adds clip-safe slack there). */
export interface DeviceRect {
  x0: number
  y0: number
  x1: number
  y1: number
}

function fracX(f: number, m: GlyphMetrics): number {
  return f >= 1 ? m.Wd : Math.round(f * m.Wd)
}
function fracY(f: number, m: GlyphMetrics): number {
  return f >= 1 ? m.Hd : Math.round(f * m.Hd)
}

function resolveXTok(t: XTok, m: GlyphMetrics): number {
  return t === 0 ? 0 : t === 'W' ? m.Wd : m[t]
}
function resolveYTok(t: YTok, m: GlyphMetrics): number {
  return t === 0 ? 0 : t === 'H' ? m.Hd : m[t]
}

/** Resolve a descriptor to its device-px rect list. Arcs resolve to
 *  [] (they paint via resolveArcChild); a dashed line resolves to
 *  its full stroke band (the dash pattern is a paint-stage detail).
 *  Degenerate rounding (a rect collapsing to zero thickness on a
 *  tiny cell) is clamped to one device px. */
export function resolveGlyphRects(
  spec: SyntheticGlyph,
  m: GlyphMetrics,
): DeviceRect[] {
  const out: DeviceRect[] = []
  const push = (x0: number, y0: number, x1: number, y1: number) => {
    if (x1 <= x0) x1 = x0 + 1
    if (y1 <= y0) y1 = y0 + 1
    out.push({ x0, y0, x1, y1 })
  }
  switch (spec.kind) {
    case 'rects':
      for (const [x0, y0, x1, y1] of spec.rects) {
        push(fracX(x0, m), fracY(y0, m), fracX(x1, m), fracY(y1, m))
      }
      return out
    case 'lines': {
      const [left, up, right, down] = spec.arms
      // Each arm runs from its cell edge THROUGH the center band of
      // its own thickness, so any two arms overlap at the junction
      // (same-color overlap is invisible; a heavier crossing arm
      // simply covers a lighter one's tail).
      if (left) {
        const [a, b] = left === 2 ? [m.hha, m.hhb] : [m.ha, m.hb]
        push(0, a, left === 2 ? m.vhb : m.vb, b)
      }
      if (right) {
        const [a, b] = right === 2 ? [m.hha, m.hhb] : [m.ha, m.hb]
        push(right === 2 ? m.vha : m.va, a, m.Wd, b)
      }
      if (up) {
        const [a, b] = up === 2 ? [m.vha, m.vhb] : [m.va, m.vb]
        push(a, 0, b, up === 2 ? m.hhb : m.hb)
      }
      if (down) {
        const [a, b] = down === 2 ? [m.vha, m.vhb] : [m.va, m.vb]
        push(a, down === 2 ? m.hha : m.ha, b, m.Hd)
      }
      return out
    }
    case 'double':
      for (const [x0, y0, x1, y1] of spec.rects) {
        push(
          resolveXTok(x0, m),
          resolveYTok(y0, m),
          resolveXTok(x1, m),
          resolveYTok(y1, m),
        )
      }
      return out
    case 'arc':
      return out
  }
}

/** The one-child-div geometry for ╭╮╰╯ (CSS px). The child carries a
 *  light-weight border on the two stroked sides and a border-radius
 *  on the turning corner; its straight ends reach the cell edges so
 *  it joins neighboring ─/│ seamlessly. */
export interface ArcChild {
  left: number
  top: number
  width: number
  height: number
  borderWidth: number
  radius: number
  corner: 'tl' | 'tr' | 'br' | 'bl'
}

export function resolveArcChild(
  corner: 'tl' | 'tr' | 'br' | 'bl',
  m: GlyphMetrics,
): ArcChild {
  const d = m.dpr
  // One device px of far-edge slack, clipped by the cell span's
  // overflow:hidden — same flush-edge trick as the backgrounds.
  const farW = (m.Wd - m.va + 1) / d
  const farH = (m.Hd - m.ha + 1) / d
  let box: { left: number; top: number; width: number; height: number }
  switch (corner) {
    case 'tl': // ╭ : stroke bottom-center → curve → right-center
      box = { left: m.va / d, top: m.ha / d, width: farW, height: farH }
      break
    case 'tr': // ╮ : left-center → curve → bottom-center
      box = { left: 0, top: m.ha / d, width: m.vb / d, height: farH }
      break
    case 'br': // ╯ : left-center → curve → top-center
      box = { left: 0, top: 0, width: m.vb / d, height: m.hb / d }
      break
    case 'bl': // ╰ : top-center → curve → right-center
      box = { left: m.va / d, top: 0, width: farW, height: m.hb / d }
      break
  }
  return {
    ...box,
    borderWidth: m.lt / d,
    radius: Math.min(box.width, box.height),
    corner,
  }
}

// ── Stage 3: CSS paint ──────────────────────────────────────────────

/** `color` at fractional alpha, for the shades ░▒▓. The two color
 *  syntaxes this renderer actually produces (hexToCss's `rgb(r,g,b)`
 *  and hex configs) are converted to a precomputed rgba(); anything
 *  else falls back to color-mix(), which WebKit supports but jsdom
 *  and older engines may not — hence "precomputed preferred". */
export function colorWithAlpha(color: string, alpha: number): string {
  const rgb = /^rgb\((\d+),\s*(\d+),\s*(\d+)\)$/.exec(color)
  if (rgb) return `rgba(${rgb[1]},${rgb[2]},${rgb[3]},${alpha})`
  const hex6 = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color)
  if (hex6) {
    return `rgba(${parseInt(hex6[1], 16)},${parseInt(hex6[2], 16)},${parseInt(hex6[3], 16)},${alpha})`
  }
  const hex3 = /^#([0-9a-f])([0-9a-f])([0-9a-f])$/i.exec(color)
  if (hex3) {
    const c = (s: string) => parseInt(s + s, 16)
    return `rgba(${c(hex3[1])},${c(hex3[2])},${c(hex3[3])},${alpha})`
  }
  return `color-mix(in srgb, ${color} ${alpha * 100}%, transparent)`
}

/** The background longhands for one cell span: N rects as N stacked
 *  no-repeat gradient layers. */
export interface GlyphBackground {
  backgroundImage: string
  backgroundPosition: string
  backgroundSize: string
  backgroundRepeat: 'no-repeat'
}

export type GlyphPaint =
  | { background: GlyphBackground }
  | { arc: ArcChild; color: string }

interface Layer {
  image: string
  pos: string
  size: string
}

/** One solid rect → one background layer. Device px → CSS px happens
 *  here (divide by dpr); edge handling per the header notes. */
function rectLayer(r: DeviceRect, m: GlyphMetrics, color: string): Layer {
  const d = m.dpr
  const w =
    r.x1 >= m.Wd
      ? r.x0 === 0
        ? '100%'
        : `${(m.Wd - r.x0 + 1) / d}px`
      : `${(r.x1 - r.x0) / d}px`
  const h =
    r.y1 >= m.Hd
      ? r.y0 === 0
        ? '100%'
        : `${(m.Hd - r.y0 + 1) / d}px`
      : `${(r.y1 - r.y0) / d}px`
  return {
    image: `linear-gradient(${color},${color})`,
    pos: `${r.x0 / d}px ${r.y0 / d}px`,
    size: `${w} ${h}`,
  }
}

/** Dashed stroke: the full band painted with a repeating hard-stop
 *  gradient — `dash` segments per cell, ~60% ink each, phase-locked
 *  to the cell edge so adjacent dashed cells continue the pattern. */
function dashLayer(
  spec: Extract<SyntheticGlyph, { kind: 'lines' }>,
  m: GlyphMetrics,
  color: string,
): Layer {
  const d = m.dpr
  const horizontal = spec.arms[0] !== 0
  const heavy = (horizontal ? spec.arms[0] : spec.arms[1]) === 2
  const t = heavy ? m.ht : m.lt
  const span = horizontal ? m.Wd : m.Hd
  const seg = span / (spec.dash as number)
  const on = Math.max(1, Math.round(seg * 0.6))
  const image = `repeating-linear-gradient(${horizontal ? 'to right' : 'to bottom'}, ${color} 0 ${on / d}px, transparent ${on / d}px ${seg / d}px)`
  if (horizontal) {
    const a = heavy ? m.hha : m.ha
    return { image, pos: `0px ${a / d}px`, size: `100% ${t / d}px` }
  }
  const a = heavy ? m.vha : m.va
  return { image, pos: `${a / d}px 0px`, size: `${t / d}px 100%` }
}

/** Build the CSS paint for one synthetic glyph. `color` is the
 *  run's RESOLVED foreground (inverse already applied) — the same
 *  ink the font glyph would have used. */
export function paintSyntheticGlyph(
  spec: SyntheticGlyph,
  cellWidth: number,
  cellHeight: number,
  dpr: number,
  color: string,
): GlyphPaint {
  const m = glyphMetrics(cellWidth, cellHeight, dpr)
  if (spec.kind === 'arc') {
    return { arc: resolveArcChild(spec.corner, m), color }
  }
  const ink =
    spec.kind === 'rects' && spec.alpha !== undefined
      ? colorWithAlpha(color, spec.alpha)
      : color
  const layers =
    spec.kind === 'lines' && spec.dash
      ? [dashLayer(spec, m, ink)]
      : resolveGlyphRects(spec, m).map((r) => rectLayer(r, m, ink))
  return {
    background: {
      backgroundImage: layers.map((l) => l.image).join(','),
      backgroundPosition: layers.map((l) => l.pos).join(','),
      backgroundSize: layers.map((l) => l.size).join(','),
      backgroundRepeat: 'no-repeat',
    },
  }
}
