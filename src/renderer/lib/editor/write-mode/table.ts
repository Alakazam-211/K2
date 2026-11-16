// A markdown table, rendered (build-editors.md E2; v3.1): a header rule and a
// hairline under every row, the same as the CSV table. Cells keep their
// inline formatting (bold, italic, code, links). A click in a cell puts the
// caret there, which reveals the table's text for editing.

import type { EditorState } from '@codemirror/state'
import { EditorView, WidgetType } from '@codemirror/view'
import type { SyntaxNode } from '@lezer/common'

interface Part {
  text: string
  cls: string
  href?: string
}
interface Cell {
  /** The cell's offset from the table's start (where a click puts the caret). */
  off: number
  parts: Part[]
}
export interface TableModel {
  /** The table's text: two tables with the same text draw the same. */
  source: string
  align: Array<'left' | 'center' | 'right' | null>
  head: Cell[]
  rows: Cell[][]
}

const SKIP = new Set(['EmphasisMark', 'CodeMark', 'LinkMark', 'StrikethroughMark', 'LinkTitle', 'LinkLabel'])
const WRAP: Record<string, string> = { Emphasis: 'cm-md-em', StrongEmphasis: 'cm-md-strong', Strikethrough: 'cm-md-strike', InlineCode: 'cm-md-code' }

/** A cell's inline text as styled runs: marks dropped, formatting kept. */
function inlineParts(state: EditorState, node: SyntaxNode, from: number, to: number, cls: string, href: string | undefined, out: Part[]): void {
  let pos = from
  const push = (a: number, b: number): void => {
    if (b > a) out.push({ text: state.sliceDoc(a, b), cls, href })
  }
  for (let c = node.firstChild; c; c = c.nextSibling) {
    if (c.to <= from || c.from >= to) continue
    push(pos, c.from)
    pos = c.to
    if (SKIP.has(c.name) || (c.name === 'URL' && node.name === 'Link')) continue
    if (c.name === 'Escape') push(c.from + 1, c.to)
    else if (c.name === 'Link') inlineParts(state, c, c.from, c.to, `${cls} cm-md-link`.trim(), linkHref(state, c), out)
    else if (c.name === 'URL' || c.name === 'Autolink') out.push({ text: state.sliceDoc(c.from, c.to).replace(/^<|>$/g, ''), cls: `${cls} cm-md-link`.trim(), href: state.sliceDoc(c.from, c.to).replace(/^<|>$/g, '') })
    else if (WRAP[c.name]) inlineParts(state, c, c.from, c.to, `${cls} ${WRAP[c.name]}`.trim(), href, out)
    else push(c.from, c.to)
  }
  push(pos, to)
}

/** A link's destination as typed. */
export function linkHref(state: EditorState, link: SyntaxNode): string | undefined {
  const url = link.getChild('URL')
  return url ? state.sliceDoc(url.from, url.to) : undefined
}

function cellsOf(state: EditorState, row: SyntaxNode, base: number): Cell[] {
  const cells: Cell[] = []
  let sawDelimiter = false
  let pending = false
  for (let c = row.firstChild; c; c = c.nextSibling) {
    if (c.name === 'TableDelimiter') {
      // Two delimiters in a row: an empty cell between them.
      if (sawDelimiter && pending) cells.push({ off: c.from - base, parts: [] })
      sawDelimiter = true
      pending = true
    } else if (c.name === 'TableCell') {
      const parts: Part[] = []
      inlineParts(state, c, c.from, c.to, '', undefined, parts)
      cells.push({ off: c.from - base, parts })
      pending = false
    }
  }
  return cells
}

/** The model of a Table node (from the syntax tree). */
export function tableModel(state: EditorState, table: SyntaxNode): TableModel {
  const base = table.from
  let head: Cell[] = []
  const rows: Cell[][] = []
  let align: TableModel['align'] = []
  for (let c = table.firstChild; c; c = c.nextSibling) {
    if (c.name === 'TableHeader') head = cellsOf(state, c, base)
    else if (c.name === 'TableRow') rows.push(cellsOf(state, c, base))
    else if (c.name === 'TableDelimiter' && /-/.test(state.sliceDoc(c.from, c.to))) {
      align = state
        .sliceDoc(c.from, c.to)
        .replace(/^\s*\|/, '')
        .replace(/\|\s*$/, '')
        .split('|')
        .map((s) => {
          const t = s.trim()
          return t.startsWith(':') && t.endsWith(':') ? 'center' : t.endsWith(':') ? 'right' : t.startsWith(':') ? 'left' : null
        })
    }
  }
  return { source: state.sliceDoc(table.from, table.to), align, head, rows }
}

function fill(cell: HTMLElement, c: Cell | undefined): void {
  for (const p of c?.parts ?? []) {
    if (!p.cls) {
      cell.append(p.text)
      continue
    }
    const s = document.createElement('span')
    s.className = p.cls
    if (p.href) s.dataset.href = p.href
    s.textContent = p.text
    cell.appendChild(s)
  }
  if (c) cell.dataset.off = String(c.off)
}

export class TableWidget extends WidgetType {
  constructor(readonly model: TableModel) {
    super()
  }
  eq(o: TableWidget): boolean {
    return o.model.source === this.model.source
  }
  get estimatedHeight(): number {
    return (this.model.rows.length + 1) * 34 + 22
  }
  toDOM(view: EditorView): HTMLElement {
    const wrap = document.createElement('div')
    wrap.className = 'cm-md-table-wrap'
    const table = document.createElement('table')
    table.className = 'cm-md-table'
    const cols = Math.max(this.model.head.length, ...this.model.rows.map((r) => r.length))
    const row = (cells: Cell[], tag: 'th' | 'td'): HTMLTableRowElement => {
      const tr = document.createElement('tr')
      for (let i = 0; i < cols; i++) {
        const cell = document.createElement(tag)
        const a = this.model.align[i]
        if (a) cell.style.textAlign = a
        fill(cell, cells[i])
        tr.appendChild(cell)
      }
      return tr
    }
    const thead = document.createElement('thead')
    thead.appendChild(row(this.model.head, 'th'))
    const tbody = document.createElement('tbody')
    for (const r of this.model.rows) tbody.appendChild(row(r, 'td'))
    table.append(thead, tbody)
    wrap.appendChild(table)
    // A click in a cell: the caret goes to that cell's text, which reveals the table.
    wrap.addEventListener('mousedown', (e) => {
      if (view.state.readOnly || e.button !== 0) return
      const target = e.target as Element
      if (target.closest('[data-href]') && (e.metaKey || e.ctrlKey)) return
      const cell = target.closest<HTMLElement>('[data-off]')
      e.preventDefault()
      const at = view.posAtDOM(wrap) + Number(cell?.dataset.off ?? 0)
      view.dispatch({ selection: { anchor: Math.min(at, view.state.doc.length) }, userEvent: 'select.pointer' })
      view.focus()
    })
    return wrap
  }
  ignoreEvent(): boolean {
    return true
  }
}
