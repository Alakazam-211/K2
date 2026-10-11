// Terminal-column ↔ text-offset math for CellRun rows.
//
// The daemon skips alacritty's WIDE_CHAR_SPACER cells when building
// runs, so a run's text has NO phantom columns — but a double-width
// char (CJK/emoji) still occupies TWO terminal columns while being
// ONE char, and zero-width chars (combining accents, ZWJ) occupy
// ZERO columns. Runs whose column span differs from their char count
// carry it explicitly as `cols` (see grid_snapshot.rs::CellRun);
// runs without the field are one-column-per-char by contract.
//
// Everything here is pure so the mapping is unit-testable
// (runCols.test.ts) without DOM or wire plumbing. Text offsets are
// UTF-16 indices into the row's joined text — the unit JS string
// slicing and `detectLinks` ranges use.

/** The subset of a wire CellRun this module needs. */
export interface ColRun {
  text: string
  /** Terminal-column span; absent ⇒ one column per char. */
  cols?: number
}

/** One code point's placement in a row: its UTF-16 range in the
 *  joined text and its column range. Zero-width chars are folded
 *  into the preceding cluster (they render with it and can never be
 *  hit independently). */
export interface Cluster {
  utf16Start: number
  utf16End: number
  colStart: number
  width: number
}

// ── Per-code-point width classifier ─────────────────────────────────
//
// Only consulted for runs that CARRY a `cols` annotation (i.e. the
// daemon told us the run contains non-single-width content); plain
// runs take the exact one-per-char fast path. This mirrors the
// dominant ranges of Rust's unicode-width (what alacritty used to
// set WIDE_CHAR): a divergence on an exotic code point costs one
// column of hit-test bias inside that run, never a render break —
// the run's total width comes from the wire, not from this table.

export function isZeroWidthCp(cp: number): boolean {
  return (
    (cp >= 0x0300 && cp <= 0x036f) || // combining diacriticals
    (cp >= 0x0483 && cp <= 0x0489) || // Cyrillic combining
    (cp >= 0x0591 && cp <= 0x05bd) || // Hebrew points
    (cp >= 0x0610 && cp <= 0x061a) || // Arabic marks
    (cp >= 0x064b && cp <= 0x065f) ||
    (cp >= 0x1ab0 && cp <= 0x1aff) || // combining extended
    (cp >= 0x1dc0 && cp <= 0x1dff) || // combining supplement
    (cp >= 0x20d0 && cp <= 0x20ff) || // combining for symbols
    (cp >= 0xfe00 && cp <= 0xfe0f) || // variation selectors
    (cp >= 0xfe20 && cp <= 0xfe2f) || // combining half marks
    cp === 0x200b || // ZWSP
    cp === 0x200c || // ZWNJ
    cp === 0x200d || // ZWJ
    cp === 0xfeff // BOM/ZWNBSP
  )
}

/** BMP emoji-presentation code points (East Asian Width = Wide since
 *  Unicode 9) — the set the daemon's width table (Rust unicode-width)
 *  counts as TWO columns but our CJK + plane-1 ranges missed: ✅ ⚡
 *  ⭐ ❌ ⌚ etc. Missing them under-allotted one column and the cell
 *  frame clipped half the glyph (the "squished emoji" bug). Mirrors
 *  unicode-width's BMP wide-emoji entries. */
export function isBmpEmojiWideCp(cp: number): boolean {
  return (
    cp === 0x231a || cp === 0x231b || // watch, hourglass
    (cp >= 0x23e9 && cp <= 0x23ec) || // av-control block
    cp === 0x23f0 ||
    cp === 0x23f3 ||
    cp === 0x25fd ||
    cp === 0x25fe || // small squares
    cp === 0x2614 ||
    cp === 0x2615 || // umbrella, hot beverage
    (cp >= 0x2648 && cp <= 0x2653) || // zodiac
    cp === 0x267f ||
    cp === 0x2693 ||
    cp === 0x26a1 ||
    cp === 0x26aa ||
    cp === 0x26ab ||
    cp === 0x26bd ||
    cp === 0x26be ||
    cp === 0x26c4 ||
    cp === 0x26c5 ||
    cp === 0x26ce ||
    cp === 0x26d4 ||
    cp === 0x26ea ||
    cp === 0x26f2 ||
    cp === 0x26f3 ||
    cp === 0x26f5 ||
    cp === 0x26fa ||
    cp === 0x26fd ||
    cp === 0x2705 || // ✅ — the reported glyph
    cp === 0x270a ||
    cp === 0x270b ||
    cp === 0x2728 ||
    cp === 0x274c ||
    cp === 0x274e ||
    (cp >= 0x2753 && cp <= 0x2755) ||
    cp === 0x2757 ||
    (cp >= 0x2795 && cp <= 0x2797) ||
    cp === 0x27b0 ||
    cp === 0x27bf ||
    cp === 0x2b1b ||
    cp === 0x2b1c ||
    cp === 0x2b50 ||
    cp === 0x2b55
  )
}

/** Emoji cluster test for the CLIP EXEMPTION (rowRender per-char
 *  cells + the WebGL atlas): color-emoji glyphs rasterize from the
 *  emoji font at whatever advance IT chooses, so framing them to the
 *  cell box can truncate ink even at the correct column width.
 *  Anchoring stays (no column drift); only the clip is lifted — DOM
 *  renders the cell overflow:visible, the atlas widens the slot and
 *  hangs the overflow symmetrically. Deliberately a superset (the
 *  exemption is harmless on a glyph that fits). */
/** Emoji glyphs render noticeably SMALLER than text at the same
 *  point size (color-emoji faces are square with built-in padding),
 *  so both painters draw emoji clusters at this scale factor — the
 *  DOM cell via font-size em, the WebGL atlas via a scaled
 *  fontDevicePx. The clip exemption absorbs the grown advance, and
 *  the cell box/columns are untouched. Tuned by eye; 1.0 = off. */
export const EMOJI_FONT_SCALE = 1.15

export function isEmojiCp(cp: number): boolean {
  return (
    isBmpEmojiWideCp(cp) ||
    (cp >= 0x1f000 && cp <= 0x1faff) || // plane-1 emoji blocks
    cp === 0x2764 // ❤ heavy black heart
  )
}

// ── Text-presentation emoji (Emoji=Yes, Emoji_Presentation=No) ──────
//
// Code points like ⏺ ⏸ ✔ ✳ ☀ ↔ ™ are ordinary one-column TEXT
// symbols by default, but also have an emoji form. When the terminal
// font lacks one, browser font fallback may pick the colour-emoji
// face (always on iOS; Linux/Windows with Noto/Segoe emoji) — a wide
// colour glyph the one-column cell then clips. Both painters force
// TEXT presentation for these (U+FE0E VS15 on the drawn cluster —
// glyphRoute.ts); the TUI status marks among them are painted
// outright (syntheticGlyphs.ts, kind 'symbol').
//
// Inclusive [lo, hi] pairs, generated from the engine's
// \p{Emoji} && !\p{Emoji_Presentation} (Unicode 16) over U+0080–
// U+1FFFF, minus the plane-1 emoji blocks isEmojiCp already owns
// (ASCII digits/#/* are Emoji=Yes too but never need it).
const TEXT_DEFAULT_EMOJI: readonly number[] = [
  0xa9, 0xa9, 0xae, 0xae, 0x203c, 0x203c, 0x2049, 0x2049,
  0x2122, 0x2122, 0x2139, 0x2139, 0x2194, 0x2199, 0x21a9, 0x21aa,
  0x2328, 0x2328, 0x23cf, 0x23cf, 0x23ed, 0x23ef, 0x23f1, 0x23f2,
  0x23f8, 0x23fa, 0x24c2, 0x24c2, 0x25aa, 0x25ab, 0x25b6, 0x25b6,
  0x25c0, 0x25c0, 0x25fb, 0x25fc, 0x2600, 0x2604, 0x260e, 0x260e,
  0x2611, 0x2611, 0x2618, 0x2618, 0x261d, 0x261d, 0x2620, 0x2620,
  0x2622, 0x2623, 0x2626, 0x2626, 0x262a, 0x262a, 0x262e, 0x262f,
  0x2638, 0x263a, 0x2640, 0x2640, 0x2642, 0x2642, 0x265f, 0x2660,
  0x2663, 0x2663, 0x2665, 0x2666, 0x2668, 0x2668, 0x267b, 0x267b,
  0x267e, 0x267e, 0x2692, 0x2692, 0x2694, 0x2697, 0x2699, 0x2699,
  0x269b, 0x269c, 0x26a0, 0x26a0, 0x26a7, 0x26a7, 0x26b0, 0x26b1,
  0x26c8, 0x26c8, 0x26cf, 0x26cf, 0x26d1, 0x26d1, 0x26d3, 0x26d3,
  0x26e9, 0x26e9, 0x26f0, 0x26f1, 0x26f4, 0x26f4, 0x26f7, 0x26f9,
  0x2702, 0x2702, 0x2708, 0x2709, 0x270c, 0x270d, 0x270f, 0x270f,
  0x2712, 0x2712, 0x2714, 0x2714, 0x2716, 0x2716, 0x271d, 0x271d,
  0x2721, 0x2721, 0x2733, 0x2734, 0x2744, 0x2744, 0x2747, 0x2747,
  0x2763, 0x2764, 0x27a1, 0x27a1, 0x2934, 0x2935, 0x2b05, 0x2b07,
  0x3030, 0x3030, 0x303d, 0x303d, 0x3297, 0x3297, 0x3299, 0x3299,
]

/** One-column text symbol that ALSO has an emoji form — must be
 *  drawn with text presentation (see table above). Excludes the
 *  code points this renderer deliberately treats as emoji
 *  (isEmojiCp: ❤, plane-1). */
export function isTextDefaultEmojiCp(cp: number): boolean {
  if (cp < 0xa9 || cp > 0x3299 || isEmojiCp(cp)) return false
  const t = TEXT_DEFAULT_EMOJI
  for (let i = 0; i < t.length; i += 2) {
    if (cp < t[i]) return false
    if (cp <= t[i + 1]) return true
  }
  return false
}

export function isWideCp(cp: number): boolean {
  return (
    isBmpEmojiWideCp(cp) || // ✅⚡⭐… — 2 columns per the daemon
    (cp >= 0x1100 && cp <= 0x115f) || // Hangul Jamo
    (cp >= 0x2e80 && cp <= 0x303e) || // CJK radicals, punctuation
    (cp >= 0x3041 && cp <= 0x33ff) || // Hiragana..CJK compat
    (cp >= 0x3400 && cp <= 0x4dbf) || // CJK ext A
    (cp >= 0x4e00 && cp <= 0x9fff) || // CJK unified
    (cp >= 0xa000 && cp <= 0xa4cf) || // Yi
    (cp >= 0xa960 && cp <= 0xa97f) || // Hangul Jamo ext A
    (cp >= 0xac00 && cp <= 0xd7a3) || // Hangul syllables
    (cp >= 0xf900 && cp <= 0xfaff) || // CJK compat ideographs
    (cp >= 0xfe30 && cp <= 0xfe4f) || // CJK compat forms
    (cp >= 0xff00 && cp <= 0xff60) || // fullwidth forms
    (cp >= 0xffe0 && cp <= 0xffe6) ||
    (cp >= 0x1f300 && cp <= 0x1f64f) || // emoji: misc + emoticons
    (cp >= 0x1f680 && cp <= 0x1f6ff) || // emoji: transport
    (cp >= 0x1f900 && cp <= 0x1f9ff) || // emoji: supplemental
    (cp >= 0x1fa00 && cp <= 0x1faff) || // emoji: extended A
    (cp >= 0x20000 && cp <= 0x3fffd) // CJK ext B+
  )
}

// ── Per-character cell classifier (DOM painter) ─────────────────────
//
// Run-level column anchoring (rowRender.tsx) contains fallback-font
// drift BETWEEN runs, but WITHIN a run glyphs still flow at the
// font's natural advance. When a TUI re-colors animated art every
// frame (grok's shimmering braille logo), the run boundaries
// re-slice each frame, so a different amount of internal overflow
// gets clipped at different places — glyphs pop in/out of view and
// the art appears to breathe. The fix is to anchor each CHARACTER of
// such a run to its own cell rect; this classifier says which runs
// need it.
//
// A run needs per-char cells when its text contains any code point
// whose rendered advance can't be trusted to equal its column count
// in the user's monospace font — i.e. glyphs that routinely come
// from a FALLBACK font (braille, powerline PUA) or that the primary
// font may draw at a non-cell advance (wide CJK/emoji — kept
// consistent with isWideCp, the same table rowClusters uses, so
// render geometry and hit-testing agree).

// Printable-ASCII fast path: the overwhelmingly common run. One
// regex test, no allocation, and `test` on a non-global regex keeps
// no lastIndex state.
const ASCII_PRINTABLE_RE = /^[\x20-\x7E]*$/

/** True when this code point's advance in a monospace font can't be
 *  trusted to equal its terminal-column span. */
export function isUntrustedAdvanceCp(cp: number): boolean {
  return (
    // Box drawing (U+2500–257F), block elements (U+2580–259F) and
    // geometric shapes (U+25A0–25FF) — contiguous, so one range.
    // Fonts without native coverage fall back at arbitrary advances.
    (cp >= 0x2500 && cp <= 0x25ff) ||
    // Braille patterns — grok's logo art; essentially always a
    // fallback glyph on macOS (Apple Braille), advance ≠ cell.
    (cp >= 0x2800 && cp <= 0x28ff) ||
    // Private Use Area — powerline / Nerd Font symbols, patched-font
    // territory with arbitrary advances.
    (cp >= 0xe000 && cp <= 0xf8ff) ||
    // Legacy-computing sextants — synthetic-geometry glyphs
    // (syntheticGlyphs.ts); fonts rarely cover them at all.
    (cp >= 0x1fb00 && cp <= 0x1fb3b) ||
    // Anything the column math already treats as wide (CJK, emoji):
    // those cells span 2 columns and the glyph advance is whatever
    // the CJK/emoji font says, so pin each to its own 2-col rect.
    isWideCp(cp) ||
    // Text symbols with an emoji form (⏺ ✔ ☀ …): a fallback glyph at
    // best, and the cell must carry the VS15 text-presentation
    // request — only a per-char cell can (glyphRoute.ts).
    isTextDefaultEmojiCp(cp) ||
    // Painted TUI marks ⏴⏵⏶⏷⏸⏹⏺ (syntheticGlyphs 'symbol'; ⏴–⏷
    // aren't Emoji but are fallback glyphs all the same): only
    // per-char cells reach the synthetic canvas.
    (cp >= 0x23f4 && cp <= 0x23fa)
  )
}

/** Should this run be rendered as one span PER CHARACTER instead of
 *  a single flowing span? Pure function of the text — called per run
 *  per frame by the DOM painter, so it must stay allocation-free. */
export function runNeedsPerCharCells(text: string): boolean {
  if (ASCII_PRINTABLE_RE.test(text)) return false
  for (let i = 0; i < text.length; i++) {
    const cp = text.codePointAt(i) as number
    if (cp > 0xffff) i++ // skip the low surrogate
    if (isUntrustedAdvanceCp(cp)) return true
  }
  return false
}

/** One per-character cell of a run: its text (a single code point
 *  plus any zero-width followers), its column offset WITHIN the run,
 *  and its column width. */
export interface RunCell {
  text: string
  col: number
  width: number
}

/** Split one run into per-character cells for anchored rendering.
 *  Same width table and zero-width folding as rowClusters (so cell
 *  columns agree with hit-testing); unannotated runs are one column
 *  per code point by the wire contract. If the daemon's `cols` ever
 *  disagreed with our width table the divergence stays INSIDE this
 *  run — the next run's offset comes from the wire span
 *  (runColOffsets), so it can never become cross-run drift. */
export function runCells(run: ColRun): RunCell[] {
  const annotated = run.cols !== undefined
  const out: RunCell[] = []
  let col = 0
  for (const ch of run.text) {
    const cp = ch.codePointAt(0) ?? 0
    const w = annotated ? (isZeroWidthCp(cp) ? 0 : isWideCp(cp) ? 2 : 1) : 1
    if (w === 0 && out.length > 0) {
      // Zero-width follower: render with its base char's cell.
      out[out.length - 1].text += ch
      continue
    }
    const width = Math.max(w, 1)
    out.push({ text: ch, col, width })
    col += width
  }
  return out
}

/** Char count of a run in code points (what the wire's `cols`
 *  contract compares against). */
function codePointCount(text: string): number {
  let n = 0
  for (const _ of text) n++
  return n
}

/** Column span of one run: explicit wire `cols` when present, else
 *  its code-point count (the daemon omits the field iff equal). */
export function runColSpan(run: ColRun): number {
  return run.cols ?? codePointCount(run.text)
}

/** Total terminal-column span of a row. */
export function rowColSpan(row: ColRun[]): number {
  let total = 0
  for (const run of row) total += runColSpan(run)
  return total
}

/** Start column of each run in a row — prefix sums of the runs'
 *  column spans. `runColOffsets(row)[i]` is the terminal column at
 *  which `row[i]` begins; paired with `runColSpan(row[i])` it pins
 *  the run to its true grid rect (rowRender.tsx), so a glyph the
 *  font falls back for (braille art, exotic symbols — whose fallback
 *  advance ≠ one cell) can never push the runs to its right off
 *  their columns. Empty row ⇒ empty array. */
export function runColOffsets(row: ColRun[]): number[] {
  const out: number[] = new Array(row.length)
  let col = 0
  for (let i = 0; i < row.length; i++) {
    out[i] = col
    col += runColSpan(row[i])
  }
  return out
}

/** Build the per-cluster placement list for a row. Exported for the
 *  WebGL painter's selection/word math (webgl/selection.ts), which
 *  needs cluster granularity, not just index mapping. */
export function rowClusters(row: ColRun[]): Cluster[] {
  const out: Cluster[] = []
  let col = 0
  let u16 = 0
  for (const run of row) {
    // Fast path: unannotated run ⇒ every code point is one column.
    const annotated = run.cols !== undefined
    for (const ch of run.text) {
      const cp = ch.codePointAt(0) ?? 0
      const w = annotated ? (isZeroWidthCp(cp) ? 0 : isWideCp(cp) ? 2 : 1) : 1
      if (w === 0 && out.length > 0) {
        // Zero-width: extend the previous cluster's UTF-16 range.
        out[out.length - 1].utf16End += ch.length
        u16 += ch.length
        continue
      }
      out.push({
        utf16Start: u16,
        utf16End: u16 + ch.length,
        colStart: col,
        width: Math.max(w, 1),
      })
      u16 += ch.length
      col += Math.max(w, 1)
    }
  }
  return out
}

/** Map a terminal column to the UTF-16 index (into the row's joined
 *  text) of the char occupying it. Either half of a double-width
 *  char maps to that char. Columns past the row's content map to the
 *  text length (so range comparisons find nothing). */
export function colToTextIndex(row: ColRun[], col: number): number {
  const clusters = rowClusters(row)
  for (const c of clusters) {
    if (col >= c.colStart && col < c.colStart + c.width) return c.utf16Start
  }
  const last = clusters[clusters.length - 1]
  return last ? last.utf16End : 0
}

/** Slice a row's joined text by terminal-column range [startCol,
 *  endCol). A double-width char is included when ANY of its columns
 *  fall in the range; zero-width followers ride their base char. */
export function sliceRowByCols(
  row: ColRun[],
  startCol: number,
  endCol: number,
): string {
  if (endCol <= startCol) return ''
  const clusters = rowClusters(row)
  let text = ''
  for (const run of row) text += run.text
  let out = ''
  for (const c of clusters) {
    if (c.colStart + c.width > startCol && c.colStart < endCol) {
      out += text.slice(c.utf16Start, c.utf16End)
    }
  }
  return out
}
