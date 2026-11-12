// The grid: a frozen header row and row-number column, cells drawn only in
// the visible window (it handles thousands of rows), and the live marks on
// top: claims, peers' ranges and carets with name tags, row bands, your own
// caret, changed cells flashing then fading, an inserted row's band and
// slide, a deleted row's line, renumbered rows, and peers' visible ranges in
// the headers. Pure drawing from the state; the person's gestures go up.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { extentOf, geometryOf, GUTTER_PX, HEADER_PX, rectPx, type GridGeometry } from '@/lib/live-view/geometry'
import { caretOn, claimRect, others, type LiveWorkbookState, type Peer } from '@/lib/live-view/reducer'
import { cellShown, colAddress, formatNumber, gridOf, letters, rangeA1, rangeRect, styleOf, viewportA1, viewportRect, type Book, type Grid, type GridCell } from '@/lib/live-view/workbook'
import { actorFrom } from '@/lib/live-view/wording'
import { IconLock } from '@/components/icons/IconLock'
import { ArrowGlyph, Avatar } from './parts'

export interface WorkbookGridProps {
  state: LiveWorkbookState
  book: Book
  sheet: string
  /** Draw presence, claims and changes (live and loaded). */
  live: boolean
  formulas: boolean
  reduced: boolean
  /** The cell the formula bar describes (lit in the headers). */
  current: { r: number; c: number } | null
  /** Your own caret, from your client's state. */
  selfCaret: { r: number; c: number } | null
  /** Bring this cell into view when it changes (following the agent). */
  scrollTarget: { r: number; c: number; key: string } | null
  onPickCell: (r: number, c: number) => void
  /** The person scrolled or reached for the grid: following pauses. */
  onUserMove: () => void
  /** Your visible range, in cells, for the minimap. */
  onSeen?: (seen: { r0: number; r1: number; c0: number; c1: number }) => void
}

const DEFAULT_SIZE = { w: 900, h: 480 }

function useSize(ref: React.RefObject<HTMLElement | null>): { w: number; h: number } {
  const [size, setSize] = useState(DEFAULT_SIZE)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      if (el.clientWidth > 0 && el.clientHeight > 0) setSize({ w: el.clientWidth, h: el.clientHeight })
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref])
  return size
}

/** What the person's own client needs to see past: peers' visible ranges and ranges on this sheet. */
function needOf(state: LiveWorkbookState, g: Grid | null, sheet: string): { r: number; c: number } {
  let r = 0
  let c = 0
  if (!g) return { r, c }
  for (const p of others(state)) {
    if (p.focus?.key !== sheet) continue
    if (p.viewport) {
      const R = viewportRect(p.viewport)
      r = Math.max(r, R.r1)
      c = Math.max(c, R.c1)
    }
    for (const x of p.cells?.container === sheet ? p.cells.ranges : []) {
      const R = rangeRect(g, x)
      if (R) {
        r = Math.max(r, Math.min(R.r1, g.lastRow + 200))
        c = Math.max(c, Math.min(R.c1, g.lastCol + 12))
      }
    }
  }
  return { r, c }
}

interface Shown {
  text: string
  num: boolean
  kind: 'value' | 'cached' | 'nc' | 'fml' | 'err'
  title?: string
}

function show(g: Grid, cell: GridCell, formulas: boolean): Shown {
  const s = cellShown(cell)
  const st = styleOf(g, cell)
  if (s.kind === 'formula') {
    const title = `${letters(cell.c)}${cell.r}: ${s.formula}${s.resultState === 'not_calculated' ? ', not calculated' : ', as last calculated'}`
    if (formulas) return { text: s.formula, num: false, kind: 'fml', title }
    if (s.result == null) return { text: 'not calc.', num: true, kind: 'nc', title }
    const f = formatNumber(s.result, st.numfmt)
    return { text: f.text, num: Number.isFinite(Number(s.result)), kind: 'cached', title: f.applied ? title : `${title} (format ${st.numfmt})` }
  }
  if (cell.t === 'e') return { text: s.text, num: false, kind: 'err' }
  if (cell.t === 'b') return { text: s.text === '1' ? 'TRUE' : 'FALSE', num: true, kind: 'value' }
  if (cell.t === 'n') {
    const f = formatNumber(s.text, st.numfmt)
    return { text: f.text, num: true, kind: 'value', title: f.applied ? undefined : `Number format ${st.numfmt}` }
  }
  return { text: s.text, num: false, kind: 'value' }
}

function CellBox({ g, cell, x, w, h, formulas, merged }: { g: Grid; cell: GridCell; x: number; w: number; h?: number; formulas: boolean; merged?: boolean }): React.JSX.Element {
  const st = styleOf(g, cell)
  const sh = show(g, cell, formulas)
  return (
    <div
      className="lv-cell"
      data-num={sh.num ? '' : undefined}
      data-cached={sh.kind === 'cached' ? '' : undefined}
      data-nc={sh.kind === 'nc' ? '' : undefined}
      data-fml={sh.kind === 'fml' ? '' : undefined}
      data-err={sh.kind === 'err' ? '' : undefined}
      data-bold={st.bold ? '' : undefined}
      data-merge={merged ? '' : undefined}
      data-testid={`lv-cell-${letters(cell.c)}${cell.r}`}
      title={sh.title}
      style={{ left: x, width: w, ...(h != null ? { height: h, bottom: 'auto' } : {}), ...(st.fill ? { background: st.fill, ['--lv-fill' as string]: st.fill } : {}) }}
    >
      {sh.text}
    </div>
  )
}

export function WorkbookGrid(props: WorkbookGridProps): React.JSX.Element {
  const { state, book, sheet, live, formulas, reduced, current, selfCaret, scrollTarget, onPickCell, onUserMove, onSeen } = props
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const bodyRef = useRef<HTMLDivElement | null>(null)
  const size = useSize(scrollRef)
  const [scroll, setScroll] = useState({ top: 0, left: 0 })
  const programmatic = useRef(false)
  const g = gridOf(book, sheet)
  const max = book.schema.grid.max
  const need = needOf(state, g, sheet)
  const extent = extentOf(g, need, max)
  const G: GridGeometry = useMemo(() => geometryOf(g, extent), [g, extent.rows, extent.cols]) // eslint-disable-line react-hooks/exhaustive-deps
  const vw = Math.max(0, size.w - GUTTER_PX)
  const vh = Math.max(0, size.h - HEADER_PX)
  const rw = G.rows.window(scroll.top, vh, 4)
  const cw = G.cols.window(scroll.left, vw, 2)
  const seen = useMemo(() => ({ r0: G.rows.at(scroll.top), r1: G.rows.at(scroll.top + vh - 1), c0: G.cols.at(scroll.left), c1: G.cols.at(scroll.left + vw - 1) }), [G, scroll, vh, vw])

  useEffect(() => onSeen?.(seen), [seen, onSeen])

  // A new sheet starts at the top-left.
  useEffect(() => {
    const el = scrollRef.current
    if (!el) return
    programmatic.current = true
    el.scrollTop = 0
    el.scrollLeft = 0
    setScroll({ top: 0, left: 0 })
  }, [sheet])

  const scrollToCell = useCallback(
    (r: number, c: number, smooth: boolean) => {
      const el = scrollRef.current
      const b = rectPx(G, r, r, c, c)
      if (!el || !b) return
      let top = el.scrollTop
      let left = el.scrollLeft
      if (b.y < top + 8 || b.y + b.h > top + vh - 40) top = Math.max(0, b.y - vh / 3)
      if (b.x < left || b.x + b.w > left + vw - 40) left = Math.max(0, b.x - 80)
      if (top === el.scrollTop && left === el.scrollLeft) return
      programmatic.current = true
      if (typeof el.scrollTo === 'function') el.scrollTo({ top, left, behavior: smooth ? 'smooth' : 'auto' })
      else {
        el.scrollTop = top
        el.scrollLeft = left
      }
      setScroll({ top, left })
    },
    [G, vh, vw],
  )

  useEffect(() => {
    if (scrollTarget) scrollToCell(scrollTarget.r, scrollTarget.c, !reduced)
  }, [scrollTarget?.key]) // eslint-disable-line react-hooks/exhaustive-deps

  const onScroll = (): void => {
    const el = scrollRef.current
    if (!el) return
    setScroll({ top: el.scrollTop, left: el.scrollLeft })
    programmatic.current = false
  }

  const latest = live ? state.recent.find((r) => r.age === 0) : undefined
  const prevGrid = latest ? gridOf(latest.before, sheet) : null
  const opKeys = new Set(latest?.frame.ops.flatMap((o) => (o.kind === 'text' ? [] : [o.key])) ?? [])
  const insertBands = (latest?.frame.derived?.bands ?? []).filter((b) => b.container === sheet && b.op === 'insert' && b.axis === 'rows')
  const insertedRows = new Set<number>()
  for (const b of insertBands) for (let n = b.from; n <= b.to; n++) insertedRows.add(n)
  const animKey = latest ? `:${latest.frame.rev}` : ''

  // ---------- rows, row numbers, cells ----------
  const rowEls: React.ReactNode[] = []
  const numEls: React.ReactNode[] = []
  const renum: number[] = []
  const merges = g ? g.merges.map((m) => rangeRect(g, m)).filter((m): m is NonNullable<typeof m> => !!m) : []
  const inMerge = (r: number, c: number): boolean => merges.some((M) => r >= M.r0 && r <= M.r1 && c >= M.c0 && c <= M.c1)
  for (let n = rw.first; n <= rw.last; n++) {
    const m = g?.rowByNumber.get(n)
    const y = G.rows.start(n)
    const h = G.rows.size(n)
    const was = m && prevGrid ? prevGrid.rowByKey.get(m.key)?.n : undefined
    const moved = m != null && was != null && was !== n && !opKeys.has(m.key)
    if (moved) renum.push(n)
    numEls.push(
      <div
        key={`n${n}${moved ? animKey : ''}`}
        className="lv-rownum lv-num"
        data-cur={current?.r === n ? '' : undefined}
        data-hidden={m?.hidden ? '' : undefined}
        data-renum={moved ? '' : undefined}
        title={m?.hidden ? `Row ${n} is hidden by the file` : undefined}
        style={{ top: y, height: Math.max(h, m?.hidden ? 2 : 0), ['--rc' as string]: latest ? actorFrom(latest.frame.actor).colour : undefined }}
      >
        {h > 0 ? n : null}
      </div>,
    )
    if (h === 0) continue
    const cells: React.ReactNode[] = []
    if (g && m) {
      const row: GridCell[] = []
      for (let c = cw.first; c <= cw.last; c++) {
        const cell = g.cellAt(n, c)
        if (cell && G.cols.size(c) > 0 && !inMerge(n, c)) row.push(cell)
      }
      row.forEach((cell, j) => {
        let w = G.cols.size(cell.c)
        const sh = cellShown(cell)
        // Text spills right over empty neighbours, as a sheet does.
        if (sh.kind === 'value' && cell.t !== 'n' && cell.t !== 'b') {
          const stop = row[j + 1]?.c ?? Math.min(G.cols.count + 1, cell.c + 12)
          let end = cell.c + 1
          while (end < stop && !inMerge(n, end)) end++
          w = G.cols.start(end) - G.cols.start(cell.c)
        }
        cells.push(<CellBox key={cell.field} g={g} cell={cell} x={G.cols.start(cell.c)} w={w} formulas={formulas} />)
      })
    }
    const slideDy = moved && was != null && !reduced ? (was - n) * h : 0
    rowEls.push(
      <div
        key={`${m?.key ?? `b${n}`}${moved || insertedRows.has(n) ? animKey : ''}`}
        className="lv-row"
        data-row={n}
        data-slide={slideDy ? '' : undefined}
        data-enter={insertedRows.has(n) && !reduced ? '' : undefined}
        style={{ top: y, height: h, width: G.cols.total, ...(slideDy ? { ['--lv-dy' as string]: `${slideDy}px` } : {}) }}
      >
        {cells}
      </div>,
    )
  }
  const mergeEls = g
    ? merges
        .filter((M) => M.r1 >= rw.first && M.r0 <= rw.last && M.c1 >= cw.first && M.c0 <= cw.last)
        .map((M) => {
          const anchor = g.cellAt(M.r0, M.c0)
          const b = rectPx(G, M.r0, M.r1, M.c0, M.c1)
          if (!b) return null
          return anchor ? (
            <div key={`m${M.r0}:${M.c0}`} className="lv-ovl" style={{ left: 0, top: b.y, width: G.cols.total, height: b.h, pointerEvents: 'auto' }}>
              <CellBox g={g} cell={anchor} x={b.x} w={b.w} h={b.h} formulas={formulas} merged />
            </div>
          ) : null
        })
    : []

  const letterEls: React.ReactNode[] = []
  const vlines: React.ReactNode[] = []
  for (let c = cw.first; c <= cw.last; c++) {
    const w = G.cols.size(c)
    if (w === 0) continue
    letterEls.push(
      <div key={c} className="lv-letter" data-cur={current?.c === c ? '' : undefined} style={{ left: G.cols.start(c), width: w }}>
        {letters(c)}
      </div>,
    )
    vlines.push(<div key={c} className="lv-vline" style={{ left: G.cols.end(c) - 1 }} />)
  }

  // ---------- the live marks ----------
  const ovl: React.ReactNode[] = []
  const tags: React.ReactNode[] = []
  const vpRows: React.ReactNode[] = []
  const vpCols: React.ReactNode[] = []
  const edges: { who: Peer; r: number; c: number; dir: 'up' | 'down' | 'left' | 'right' }[] = []
  if (live && g) {
    const mergedTag = new Set<string>()
    for (const cl of state.claims) {
      if (cl.container !== sheet) continue
      const CR = claimRect(book, cl)
      const b = CR && rectPx(G, CR.r0, CR.r1, CR.c0, Math.min(CR.c1, G.cols.count))
      if (!CR || !b) continue
      const a = actorFrom(cl.actor)
      ovl.push(<div key={`cl${cl.id}`} className="lv-ovl lv-claim" data-testid={`lv-claim-${cl.id}`} style={{ ['--c' as string]: a.colour, left: b.x, top: b.y, width: b.w, height: b.h }} />)
      const owner = state.peers[cl.actor.id]
      const oc = cl.scope === 'cells' ? caretOn(book, owner, sheet) : null
      if (oc && oc.r === CR.r0 && oc.c >= CR.c0 && oc.c <= CR.c1) {
        mergedTag.add(cl.actor.id)
        continue
      }
      tags.push(
        <div key={`clt${cl.id}`} className="lv-tag" style={{ ['--c' as string]: a.colour, left: cl.scope === 'rows' ? G.cols.end(Math.min(g.lastCol + 1, G.cols.count)) + 8 : b.x + b.w - 2, top: b.y }}>
          <IconLock className="h-3 w-3" />
          {a.name}: {cl.label}
          {cl.hold ? ' · held' : ''}
        </div>,
      )
    }
    others(state).forEach((p, j) => {
      if (p.focus?.key !== sheet) return
      const a = { name: p.name, kind: p.kind, colour: p.colour }
      if (p.cells && p.cells.container === sheet) {
        for (const [i, x] of p.cells.ranges.entries()) {
          const R = rangeRect(g, x)
          const b = R && rectPx(G, R.r0, R.r1, R.c0, R.c1)
          if (!R || !b) continue
          ovl.push(<div key={`rg${p.handle}${i}`} className="lv-ovl lv-range" style={{ ['--c' as string]: p.colour, left: b.x, top: b.y, width: b.w, height: b.h }} />)
          if (b.clipB || b.clipR) {
            const n = (R.r1 - R.r0 + 1) * (R.c1 - R.c0 + 1)
            tags.push(
              <div key={`rgt${p.handle}${i}`} className="lv-tag" data-plain="" style={{ ['--c' as string]: p.colour, left: b.x + 4, top: b.y + 4 }}>
                {rangeA1(g, x)} · {n.toLocaleString('en-US')} cells
              </div>,
            )
          }
        }
        const rc = caretOn(book, p, sheet)
        const b = rc && rectPx(G, rc.r, rc.r, rc.c, rc.c)
        if (rc && b) {
          const held = state.claims.find((cl) => cl.actor.id === p.handle && cl.container === sheet && (() => { const CR = claimRect(book, cl); return !!CR && rc.r >= CR.r0 && rc.r <= CR.r1 && rc.c >= CR.c0 && rc.c <= CR.c1 })())
          ovl.push(<div key={`ca${p.handle}`} className="lv-ovl lv-caret" data-agent={p.kind === 'agent' ? '' : undefined} data-testid={`lv-caret-${p.handle}`} style={{ ['--c' as string]: p.colour, left: b.x, top: b.y, width: b.w, height: b.h }} />)
          const mc = mergedTag.has(p.handle) ? held : null
          tags.push(
            <div key={`cat${p.handle}`} className="lv-tag" style={{ ['--c' as string]: p.colour, left: b.x - 2, top: b.y }}>
              <Avatar actor={a} size={12} />
              {mc ? (
                <>
                  <IconLock className="h-3 w-3" />
                  {p.name}: {mc.label}
                  {mc.hold ? ' · held' : ''} · {letters(rc.c)}
                  {rc.r}
                </>
              ) : (
                <>
                  {p.name} {held ? 'editing ' : ''}
                  {letters(rc.c)}
                  {rc.r}
                </>
              )}
            </div>,
          )
          const dir = b.y + b.h <= scroll.top ? 'up' : b.y >= scroll.top + vh ? 'down' : b.x >= scroll.left + vw ? 'right' : b.x + b.w <= scroll.left ? 'left' : null
          if (dir) edges.push({ who: p, r: rc.r, c: rc.c, dir })
        }
      } else if (p.selection && p.selection.root === book.schema.grid.rows) {
        for (const k of p.selection.keys) {
          const r = g.rowByKey.get(k)
          const b = r && !r.hidden ? rectPx(G, r.n, r.n, 1, G.cols.count) : null
          if (b) ovl.push(<div key={`sb${p.handle}${k}`} className="lv-ovl lv-rowband" style={{ ['--c' as string]: p.colour, left: b.x, top: b.y, width: b.w, height: b.h }} />)
        }
      }
      if (p.viewport) {
        const R = viewportRect(p.viewport)
        const b = rectPx(G, Math.max(1, R.r0), R.r1, Math.max(1, R.c0), R.c1)
        if (b) {
          const label = `${p.name} sees ${viewportA1(p.viewport)}`
          vpRows.push(<div key={p.handle} className="lv-vpb" title={label} style={{ ['--c' as string]: p.colour, left: 3 + j * 4, width: 3, top: b.y, height: b.h }} />)
          vpCols.push(<div key={p.handle} className="lv-vpb" title={label} style={{ ['--c' as string]: p.colour, top: HEADER_PX - 4 - j * 4, height: 3, left: b.x, width: b.w }} />)
        }
      }
    })
    if (selfCaret) {
      const b = rectPx(G, selfCaret.r, selfCaret.r, selfCaret.c, selfCaret.c)
      if (b) ovl.push(<div key="self" className="lv-ovl lv-self" title="Your cell" data-testid="lv-self-caret" style={{ left: b.x, top: b.y, width: b.w, height: b.h }} />)
    }
    // Changes: the latest flashes and fades; the one before stays faint. Outlines follow the keys.
    for (const r of [...state.recent].reverse()) {
      const col = actorFrom(r.frame.actor).colour
      const after = gridOf(book, sheet)
      if (!after) continue
      for (const o of r.frame.ops) {
        if (o.kind !== 'map' || o.root !== book.schema.grid.cells || o.field == null || o.field.startsWith(book.schema.grid.row_fields_start)) continue
        const row = after.rowByKey.get(o.key)
        const cc = after.colByKey.get(colAddress(book.schema.grid.col_of_cell_field, sheet, o.field))
        const b = row && cc && !row.hidden ? rectPx(G, row.n, row.n, cc.n, cc.n) : null
        if (!row || !cc || !b) continue
        const reg = (s: string | null | undefined): string | null => {
          if (s == null) return null
          try {
            const j = JSON.parse(s) as { v?: unknown }
            return j.v == null ? null : String(j.v)
          } catch {
            return s
          }
        }
        const fb = r.before.a1[o.key]?.[o.field]
        const fa = r.after.a1[o.key]?.[o.field]
        const was = fb ?? reg(o.before)
        const now = fa ?? reg(o.after)
        const who = actorFrom(r.frame.actor)
        const title = `${who.name} · ${cc.letter}${row.n} · rev ${r.frame.rev}\nwas: ${o.before == null ? 'empty (a new cell)' : (was ?? '')}\nnow: ${now ?? ''}${fa != null && reg(o.after) == null ? ' (not calculated)' : ''}`
        ovl.push(<div key={`ch${r.frame.rev}:${o.key}:${o.field}`} className="lv-ovl lv-chg" data-old={r.age > 0 || reduced ? '' : undefined} data-testid="lv-change" title={title} style={{ ['--c' as string]: col, left: b.x, top: b.y, width: b.w, height: b.h }} />)
      }
      if (r.age !== 0) continue
      for (const band of r.frame.derived?.bands ?? []) {
        if (band.container !== sheet || band.axis !== 'rows') continue
        if (band.op === 'insert') {
          const b = rectPx(G, band.from, band.to, 1, G.cols.count)
          if (b) ovl.push(<div key={`bi${r.frame.rev}`} className="lv-ovl lv-band" data-testid="lv-insert-band" style={{ ['--c' as string]: col, left: 0, top: b.y, width: b.w, height: b.h }} />)
        } else {
          // A delete's numbers are before the change: the line goes where row `from` now starts.
          ovl.push(<div key={`bd${r.frame.rev}`} className="lv-ovl lv-del" data-testid="lv-delete-line" title={`Deleted row${band.from === band.to ? '' : 's'} ${band.from}${band.from === band.to ? '' : `–${band.to}`}`} style={{ left: 0, top: G.rows.start(band.from) - 1, width: G.cols.total }} />)
        }
      }
    }
    if (renum.length) {
      const y = G.rows.start(renum[0])
      tags.push(
        <div key="renum" className="lv-tag" data-plain="" data-testid="lv-renumbered" style={{ ['--c' as string]: 'var(--color-text-secondary)', left: G.cols.end(Math.min(g.lastCol, G.cols.count)) + 8, top: y + 2 }}>
          Rows {renum[0]}–{renum[renum.length - 1]} renumbered · no op
        </div>,
      )
    }
  }

  const pick = (e: React.MouseEvent): void => {
    const el = bodyRef.current
    if (!el) return
    const box = el.getBoundingClientRect()
    onPickCell(G.rows.at(e.clientY - box.top), G.cols.at(e.clientX - box.left))
  }

  return (
    <div className="relative min-h-0 flex-1">
      <div
        ref={scrollRef}
        className="lv-scroll absolute inset-0"
        role="grid"
        aria-label={`${g?.title ?? 'Sheet'}, ${g?.rows.length ?? 0} rows with content`}
        aria-rowcount={G.rows.count}
        aria-colcount={G.cols.count}
        tabIndex={0}
        onScroll={onScroll}
        onWheel={onUserMove}
        onTouchStart={onUserMove}
        onKeyDown={(e) => {
          if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End'].includes(e.key)) onUserMove()
        }}
        onPointerDown={(e) => {
          if (e.target === e.currentTarget) onUserMove()
        }}
        data-testid="lv-grid"
      >
        <div style={{ position: 'relative', width: GUTTER_PX + G.cols.total, height: HEADER_PX + G.rows.total }}>
          <div className="lv-head-row" style={{ width: GUTTER_PX + G.cols.total, height: HEADER_PX }}>
            <div className="lv-corner" style={{ width: GUTTER_PX, height: HEADER_PX }} />
            <div style={{ position: 'relative', width: G.cols.total, height: HEADER_PX }}>
              {letterEls}
              {vpCols}
            </div>
          </div>
          <div style={{ display: 'flex' }}>
            <div className="lv-gutter" style={{ width: GUTTER_PX, height: G.rows.total, position: 'sticky' }}>
              {numEls}
              {vpRows}
            </div>
            <div ref={bodyRef} style={{ position: 'relative', width: G.cols.total, height: G.rows.total }} onClick={pick}>
              {vlines}
              {rowEls}
              {mergeEls}
              {ovl}
              {tags}
            </div>
          </div>
        </div>
      </div>
      {edges.map((e) => (
        <button
          key={e.who.handle}
          type="button"
          className="lv-edge k2-tag inline-flex items-center gap-1"
          data-k2-chamfer="chip"
          style={{
            ['--c' as string]: e.who.colour,
            ...(e.dir === 'up' ? { top: HEADER_PX + 6, left: GUTTER_PX + 8 } : e.dir === 'down' ? { bottom: 10, left: GUTTER_PX + 8 } : e.dir === 'right' ? { right: 12, top: HEADER_PX + 8 } : { left: GUTTER_PX + 8, top: HEADER_PX + 8 }),
          }}
          title={`Scroll to ${e.who.name} at ${letters(e.c)}${e.r}`}
          onClick={() => {
            onUserMove()
            scrollToCell(e.r, e.c, !reduced)
          }}
        >
          <Avatar actor={{ name: e.who.name, kind: e.who.kind, colour: e.who.colour }} size={14} />
          {e.who.name} · {letters(e.c)}
          {e.r}
          <ArrowGlyph dir={e.dir} size={12} />
        </button>
      ))}
    </div>
  )
}
