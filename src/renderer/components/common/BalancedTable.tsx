// A chat markdown table whose column widths make it as short as possible
// (Rosson 2026-11-07; the algorithm is lib/tableBalance, shared with the
// K2 Companion). The browser's auto layout gives the room to the column
// with the most text, so a short Status column wrapped into tall rows.
//
// Words are measured with a canvas in each cell's computed font, once per
// table and font. The widths go on a `<colgroup data-balanced>` with
// `table-layout: fixed`, and are worked out again only when the wrapper's
// width or the font changes (ResizeObserver, debounced). `natural`,
// `scroll` and `skip` remove the colgroup and leave the browser's layout:
// a table that can't fit scrolls inside `.chat-md-table-wrap`. A hidden
// table (width 0) is never measured; the observer catches it on show.
//
// Mirrors the phone's src/components/BalancedTable.tsx (K2SO-companion
// 71bc924); keep the two behaving the same.

import { useLayoutEffect, useRef, type ComponentPropsWithoutRef, type JSX } from 'react'
import { balanceColumns, type BalanceCell } from '@/lib/tableBalance'

export const BALANCE_RESIZE_DEBOUNCE_MS = 120

let ctx: CanvasRenderingContext2D | null | undefined
function measurer(): CanvasRenderingContext2D | null {
  if (ctx === undefined) {
    try {
      ctx = document.createElement('canvas').getContext('2d')
    } catch {
      ctx = null
    }
  }
  return ctx
}

/** Tests only: forget the cached canvas context. */
export function resetBalancedTableMeasurerForTests(): void {
  ctx = undefined
}

function fontOf(s: CSSStyleDeclaration): string {
  return `${s.fontStyle} ${s.fontWeight} ${s.fontSize} ${s.fontFamily}`
}

function px(v: string): number {
  const n = parseFloat(v)
  return Number.isFinite(n) ? n : 0
}

/** A cell's unbreakable runs: text split at whitespace, with pieces from
 *  neighbouring elements (bold, inline code, punctuation) glued together. */
function measureCell(el: HTMLTableCellElement, m: CanvasRenderingContext2D): BalanceCell {
  const cs = getComputedStyle(el)
  m.font = fontOf(cs)
  const space = m.measureText(' ').width
  const pad =
    px(cs.paddingLeft) + px(cs.paddingRight) + px(cs.borderLeftWidth) + px(cs.borderRightWidth) + 2
  const words: number[] = []
  let run = 0
  const seenCode = new Set<Element>()
  const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT)
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const parent = node.parentElement ?? el
    m.font = fontOf(getComputedStyle(parent))
    // Inline code's own padding and border, once per code span.
    const code = parent.closest('code')
    let codeBox = 0
    if (code && el.contains(code) && !seenCode.has(code)) {
      seenCode.add(code)
      const c = getComputedStyle(code)
      codeBox = px(c.paddingLeft) + px(c.paddingRight) + px(c.borderLeftWidth) + px(c.borderRightWidth)
    }
    for (const piece of (node.textContent ?? '').split(/(\s+)/)) {
      if (!piece) continue
      if (/^\s+$/.test(piece)) {
        if (run > 0) words.push(run)
        run = 0
        continue
      }
      run += m.measureText(piece).width + codeBox
      codeBox = 0
    }
  }
  if (run > 0) words.push(run)
  return { words, space, pad }
}

function unbalance(table: HTMLTableElement): void {
  table.querySelector('colgroup[data-balanced]')?.remove()
  table.style.tableLayout = ''
}

type MeasureCache = { key: string; rows: BalanceCell[][] | null }

function balance(table: HTMLTableElement, wrap: HTMLElement, cache: MeasureCache): void {
  // Width first: a hidden table (display: none, collapsed overlay) is 0
  // wide and is not measured at all.
  const available = wrap.clientWidth
  if (available <= 0 || !wrap.isConnected) return
  const m = measurer()
  if (!m) return
  const tableStyle = getComputedStyle(table)
  const fontKey = fontOf(tableStyle)
  if (cache.key !== fontKey || !cache.rows) {
    cache.key = fontKey
    cache.rows = Array.from(table.rows, (row) => Array.from(row.cells, (c) => measureCell(c, m)))
  }
  // `min-width: 4ch` on the cells, as px.
  const result = balanceColumns(cache.rows, available, { minColumn: px(tableStyle.fontSize) * 2.4 })
  if (result.kind !== 'fixed') {
    unbalance(table)
    return
  }
  let group = table.querySelector<HTMLTableColElement>('colgroup[data-balanced]')
  if (!group) {
    group = document.createElement('colgroup')
    group.setAttribute('data-balanced', '')
    table.insertBefore(group, table.firstChild)
  }
  while (group.children.length > result.widths.length) group.lastChild!.remove()
  while (group.children.length < result.widths.length) group.appendChild(document.createElement('col'))
  result.widths.forEach((w, i) => {
    ;(group!.children[i] as HTMLElement).style.width = `${w}px`
  })
  table.style.tableLayout = 'fixed'
}

/** react-markdown's `table`: the scroll wrapper plus balanced columns. */
export function BalancedTable({
  node: _node,
  ...props
}: ComponentPropsWithoutRef<'table'> & { node?: unknown }): JSX.Element {
  const wrapRef = useRef<HTMLDivElement>(null)
  const tableRef = useRef<HTMLTableElement>(null)
  useLayoutEffect(() => {
    const wrap = wrapRef.current
    const table = tableRef.current
    if (!wrap || !table) return
    const cache: MeasureCache = { key: '', rows: null }
    balance(table, wrap, cache)
    if (typeof ResizeObserver === 'undefined') return () => unbalance(table)
    let width = wrap.clientWidth
    let font = fontOf(getComputedStyle(table))
    let timer: ReturnType<typeof setTimeout> | null = null
    const ro = new ResizeObserver(() => {
      const w = wrap.clientWidth
      const f = fontOf(getComputedStyle(table))
      if (w === width && f === font) return
      width = w
      font = f
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        balance(table, wrap, cache)
      }, BALANCE_RESIZE_DEBOUNCE_MS)
    })
    ro.observe(wrap)
    ro.observe(table)
    return () => {
      ro.disconnect()
      if (timer) clearTimeout(timer)
      unbalance(table)
    }
  }, [props.children])
  return (
    <div ref={wrapRef} className="chat-md-table-wrap">
      <table ref={tableRef} {...props} />
    </div>
  )
}
