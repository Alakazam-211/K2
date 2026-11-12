// The minimap: the sheet's filled cells, your screen, and each peer's visible
// range and caret on this sheet. A click scrolls there (and pauses following).

import { Surface } from '@/components/ui'
import { caretOn, others, type LiveWorkbookState } from '@/lib/live-view/reducer'
import { colLetters } from '@k2/mpc-client'
import { viewportA1, viewportRect, type Book, type Grid } from '@/lib/live-view/workbook'

const W = 150
const H = 92
const DOTS_MAX = 2500

export function Minimap({ state, book, g, seen, onGo }: { state: LiveWorkbookState; book: Book; g: Grid; seen: { r0: number; r1: number; c0: number; c1: number } | null; onGo: (r: number, c: number) => void }): React.JSX.Element {
  const peers = others(state).filter((p) => p.focus?.key === g.key)
  let Rm = Math.max(g.lastRow, seen?.r1 ?? 0, 24)
  let Cm = Math.max(g.lastCol, seen?.c1 ?? 0, 8)
  for (const p of peers) {
    if (!p.viewport) continue
    const R = viewportRect(p.viewport)
    Rm = Math.max(Rm, R.r1)
    Cm = Math.max(Cm, R.c1)
  }
  Rm = Math.min(Rm, Math.max(g.lastRow + 40, 60))
  Cm = Math.min(Cm, Math.max(g.lastCol + 4, 16))
  const sx = W / Cm
  const sy = H / Rm
  const box = (r0: number, r1: number, c0: number, c1: number): React.CSSProperties => ({
    left: (c0 - 1) * sx,
    top: (r0 - 1) * sy,
    width: Math.max(2, (Math.min(c1, Cm) - c0 + 1) * sx),
    height: Math.max(2, (Math.min(r1, Rm) - r0 + 1) * sy),
  })
  const legend = peers.filter((p) => p.viewport)
  return (
    <Surface role2="elevated" elevation={2} chamfer material="raised" className="absolute bottom-3 right-4 z-[8] w-[170px] p-2" data-testid="lv-minimap">
      <div className="mb-1 flex items-center gap-1 type-hint text-[var(--color-text-secondary)]">
        <b className="truncate text-[var(--color-text-primary)]">{g.title}</b>
        <span className="flex-1" />
        <span className="lv-num">
          {Rm} × {colLetters(Cm)}
        </span>
      </div>
      <button
        type="button"
        aria-label="Scroll the grid to this place"
        className="relative block border border-[var(--color-border)] bg-[var(--color-bg)]"
        style={{ width: W, height: H }}
        onClick={(e) => {
          const r = e.currentTarget.getBoundingClientRect()
          onGo(Math.max(1, Math.ceil(((e.clientY - r.top) / r.height) * Rm)), Math.max(1, Math.ceil(((e.clientX - r.left) / r.width) * Cm)))
        }}
      >
        {g.cells.slice(0, DOTS_MAX).map((c) => (c.r <= Rm && c.c <= Cm ? <i key={`${c.row}:${c.field}`} className="lv-mini-dot" style={box(c.r, c.r, c.c, c.c)} /> : null))}
        {seen ? <span className="lv-mini-you" title="Your screen" style={box(seen.r0, seen.r1, seen.c0, seen.c1)} /> : null}
        {legend.map((p) => {
          const R = viewportRect(p.viewport!)
          return <span key={p.handle} className="lv-mini-vp" title={`${p.name}: ${viewportA1(p.viewport!)}`} style={{ ['--c' as string]: p.colour, ...box(Math.max(1, R.r0), R.r1, Math.max(1, R.c0), R.c1) }} />
        })}
        {peers.map((p) => {
          const rc = caretOn(book, p, g.key)
          return rc && rc.r <= Rm && rc.c <= Cm ? <span key={`c${p.handle}`} className="lv-av absolute" style={{ ['--c' as string]: p.colour, width: 6, height: 6, left: (rc.c - 0.5) * sx - 3, top: (rc.r - 0.5) * sy - 3 }} /> : null
        })}
      </button>
      <div className="mt-1 flex flex-col gap-0.5 type-hint text-[var(--color-text-muted)]">
        {legend.length ? (
          legend.map((p) => (
            <span key={p.handle} className="inline-flex items-center gap-1 truncate">
              <i className="lv-av" style={{ ['--c' as string]: p.colour, width: 8, height: 8 }} />
              {p.name} {viewportA1(p.viewport!)}
            </span>
          ))
        ) : (
          <span>No other screens on this sheet</span>
        )}
      </div>
    </Surface>
  )
}
