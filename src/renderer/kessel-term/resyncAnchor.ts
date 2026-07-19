// Resync scroll re-anchoring by CONTENT seam-match.
//
// Audit finding (2026-07-13, both auditors + verified): a k1 resync
// FULL snapshot arrives precisely when acks lag — i.e. during fast
// scrolling. The total-row-growth heuristic in applyFrameBatch
// anchors correctly while the daemon's scrollback ring is below its
// cap, but AT the cap totals stop growing while content keeps
// SHIFTING (rows scroll in as old rows fall off the ring), so the
// heuristic computes growth 0 and the scrolled-up view yanks by the
// hidden shift. The daemon cannot cheaply report an exact count
// either: alacritty exposes only the CURRENT history size, so
// build_emit's `new_scrollback` subtraction is blind past the cap.
//
// The robust client-side answer: find the rows the user is LOOKING
// AT in the new snapshot by content, and re-derive scrollPx so they
// stay stationary. Pure and wire-format-free — works against any
// daemon version.

/** Minimal structural view of a run (CellRun compatible). */
interface RunText {
  readonly text: string
}
type AnchorRow = readonly RunText[]

/** Minimal structural view of a snapshot (TermGridSnapshot
 *  compatible): scrollback + live grid + viewport row count. */
export interface AnchorGrid {
  readonly scrollback: readonly AnchorRow[]
  readonly grid: readonly AnchorRow[]
  readonly rows: number
}

/** Rows in the match window. 4 consecutive rows containing ink are
 *  effectively unique in real terminal content; blank-heavy windows
 *  are rejected instead of widened (see MIN_INK_ROWS). */
const MATCH_ROWS = 4
/** At least this many window rows must contain non-whitespace —
 *  matching a run of blank lines would anchor to noise. */
const MIN_INK_ROWS = 1

function rowSig(row: AnchorRow): string {
  let s = ''
  // NUL separator so adjacent runs "ab"+"c" vs "a"+"bc" don't collide.
  for (const r of row) s += r.text + '\0'
  return s
}

function rowAt(g: AnchorGrid, i: number): AnchorRow | null {
  if (i < 0) return null
  if (i < g.scrollback.length) return g.scrollback[i]
  return g.grid[i - g.scrollback.length] ?? null
}

/** Re-derive scrollPx across a full-snapshot replace so the content
 *  at the top of the viewport stays stationary. Returns the new
 *  scrollPx (unclamped — caller clamps against the new scrollback),
 *  or null when no confident match exists (caller falls back to the
 *  growth heuristic). Sub-row fraction is preserved. */
export function computeResyncScrollPx(
  prev: AnchorGrid,
  next: AnchorGrid,
  scrollPx: number,
  cellH: number,
): number | null {
  if (scrollPx <= 0 || cellH <= 0) return null
  const prevTotal = prev.scrollback.length + prev.grid.length
  const nextTotal = next.scrollback.length + next.grid.length
  if (prevTotal === 0 || nextTotal === 0) return null

  // The row currently at the top of the viewport (same math as
  // computeStripLayout's firstVisibleRow, minus overscan concerns).
  const topPx = Math.max(0, (prevTotal - prev.rows) * cellH - scrollPx)
  const topAbs = Math.floor(topPx / cellH)
  const frac = topPx - topAbs * cellH

  // Signature window from the OLD grid at the viewed position.
  const sigs: string[] = []
  let ink = 0
  for (let k = 0; k < MATCH_ROWS; k++) {
    const row = rowAt(prev, topAbs + k)
    if (row === null) break
    const s = rowSig(row)
    if (/\S/.test(s)) ink++
    sigs.push(s)
  }
  if (sigs.length < 2 || ink < MIN_INK_ROWS) return null

  // Scan the NEW grid for the window; among multiple matches (rare,
  // repetitive content) pick the one closest to the old position —
  // resync shifts are small relative to the buffer.
  let best = -1
  let bestDist = Infinity
  const limit = nextTotal - sigs.length
  for (let i = 0; i <= limit; i++) {
    const r0 = rowAt(next, i)
    if (r0 === null || rowSig(r0) !== sigs[0]) continue
    let ok = true
    for (let k = 1; k < sigs.length; k++) {
      const rk = rowAt(next, i + k)
      if (rk === null || rowSig(rk) !== sigs[k]) {
        ok = false
        break
      }
    }
    if (!ok) continue
    const d = Math.abs(i - topAbs)
    if (d < bestDist) {
      bestDist = d
      best = i
    }
  }
  if (best < 0) return null

  const newTopPx = best * cellH + frac
  return (nextTotal - next.rows) * cellH - newTopPx
}
