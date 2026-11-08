// @vitest-environment jsdom
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { ChatMessageBody } from './ChatMessage'
import { BALANCE_RESIZE_DEBOUNCE_MS, resetBalancedTableMeasurerForTests } from './BalancedTable'
import { wrapLines } from '@/lib/tableBalance'

vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({ writeText: vi.fn(async () => {}) }))

// Balanced chat tables (Rosson 2026-11-07): the chat markdown renderer
// (Thread, project and ticket chat, the Garden Thread widget) picks column
// widths that make a table as short as possible. jsdom has no layout or
// canvas, so the wrapper's width and the canvas are faked: every character
// is 7px wide, a space too. The cells' padding comes from the real
// globals.css.

const here = dirname(fileURLToPath(import.meta.url))
const globalsCss = readFileSync(join(here, '..', '..', 'globals.css'), 'utf8')
const CH = 7

const env = vi.hoisted(() => ({
  width: 480,
  measured: 0,
  observers: [] as Array<() => void>,
}))

class FakeResizeObserver {
  private readonly cb: () => void
  constructor(cb: () => void) {
    this.cb = cb
  }
  observe(): void {
    env.observers.push(this.cb)
  }
  unobserve(): void {}
  disconnect(): void {
    env.observers = env.observers.filter((c) => c !== this.cb)
  }
}

const fakeCtx = {
  font: '',
  measureText(text: string) {
    env.measured++
    return { width: text.length * CH }
  },
}

// K2's 0.46.10 sitrep (Rosson's screenshot): a long middle column between
// two short status columns.
const SITREP =
  '|  | Item | Status |\n' +
  '| --- | --- | --- |\n' +
  '| ✅ | Add-workspace fix, passports, DB fixes, App doors, split view default, pg_restore, GitHub setup card, signer hardening | on main, pushed |\n' +
  '| ✅ | Client CI pieces, off by default (ciPreview) | on main, pushed |\n' +
  "| ✅ | Voice tunnel move clears direct mode's leftovers | on main, pushed |\n" +
  '| ⏳ | Sew exit 78 is config refused | building |'

const words = (s: string) => s.split(/\s+/).filter(Boolean).map((w) => w.length * CH)

let style: HTMLStyleElement
const realClientWidth = Object.getOwnPropertyDescriptor(Element.prototype, 'clientWidth')
const realGetContext = HTMLCanvasElement.prototype.getContext
const realResizeObserver = (globalThis as { ResizeObserver?: unknown }).ResizeObserver

beforeEach(() => {
  style = document.createElement('style')
  style.textContent = globalsCss
  document.head.appendChild(style)
  env.width = 480
  env.measured = 0
  env.observers = []
  Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains('chat-md-table-wrap') ? env.width : 0
    },
  })
  HTMLCanvasElement.prototype.getContext = (() => fakeCtx) as unknown as typeof realGetContext
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = FakeResizeObserver
  resetBalancedTableMeasurerForTests()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  style.remove()
  delete (HTMLElement.prototype as { clientWidth?: number }).clientWidth
  if (realClientWidth) Object.defineProperty(Element.prototype, 'clientWidth', realClientWidth)
  HTMLCanvasElement.prototype.getContext = realGetContext
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = realResizeObserver
  resetBalancedTableMeasurerForTests()
})

function tableOf(container: HTMLElement): HTMLTableElement {
  const table = container.querySelector<HTMLTableElement>('.chat-markdown .chat-md-table-wrap > table')
  if (!table) throw new Error('GFM table did not render inside .chat-md-table-wrap')
  return table
}

function colWidths(table: HTMLTableElement): number[] {
  const group = table.querySelector('colgroup[data-balanced]')
  if (!group) throw new Error('no balanced colgroup')
  return Array.from(group.children, (c) => parseFloat((c as HTMLElement).style.width))
}

/** Horizontal padding + border the balancer counts for a cell. */
function padOf(cell: Element): number {
  const cs = getComputedStyle(cell)
  const n = (v: string) => (Number.isFinite(parseFloat(v)) ? parseFloat(v) : 0)
  return n(cs.paddingLeft) + n(cs.paddingRight) + n(cs.borderLeftWidth) + n(cs.borderRightWidth) + 2
}

/** The table's height in lines at these column widths. */
function tableLines(table: HTMLTableElement, widths: number[]): number {
  return Array.from(table.rows).reduce(
    (h, row) =>
      h +
      Math.max(
        ...Array.from(row.cells, (cell, c) => wrapLines(words(cell.textContent ?? ''), CH, widths[c] - padOf(cell))),
      ),
    0,
  )
}

describe('balanced chat tables', () => {
  it('sitrep: applies colgroup widths and keeps the short Status column on one line', () => {
    env.width = 560
    const { container } = render(<ChatMessageBody text={SITREP} />)
    const table = tableOf(container)
    expect(table.style.tableLayout).toBe('fixed')
    const group = table.firstElementChild
    if (!group) throw new Error('table is empty')
    expect(group.tagName).toBe('COLGROUP')
    const widths = colWidths(table)
    expect(widths).toHaveLength(3)
    expect(widths.reduce((a, b) => a + b, 0)).toBeCloseTo(560, 5)

    // Padding really comes from globals.css (th 12px a side), not 0.
    const statusCell = table.rows[1].cells[2]
    expect(padOf(statusCell)).toBeGreaterThanOrEqual(26)

    // "on main, pushed" is never crushed: every Status cell sits on one line.
    for (const row of Array.from(table.rows)) {
      const cell = row.cells[2]
      expect(wrapLines(words(cell.textContent ?? ''), CH, widths[2] - padOf(cell))).toBe(1)
    }
    // No column is narrower than its longest word.
    for (const row of Array.from(table.rows)) {
      Array.from(row.cells).forEach((cell, c) => {
        expect(widths[c]).toBeGreaterThanOrEqual(Math.max(0, ...words(cell.textContent ?? '')) + padOf(cell) - 0.5)
      })
    }
    // Shorter than auto layout's habit (Status and the tick at their
    // narrowest, Item takes the rest): 7 lines against 9.
    const tick = Math.max(12 * 2.4, CH + padOf(statusCell))
    const status = 'building'.length * CH + padOf(statusCell)
    const greedy = [tick, 560 - tick - status, status]
    expect(tableLines(table, widths)).toBeLessThan(tableLines(table, greedy))
  })

  it('a table that fits one line per cell gets no colgroup', () => {
    const { container } = render(<ChatMessageBody text={'| a | b |\n| --- | --- |\n| 1 | 2 |'} />)
    const table = tableOf(container)
    expect(table.querySelector('colgroup')).toBeNull()
    expect(table.style.tableLayout).toBe('')
    expect(env.measured).toBeGreaterThan(0)
  })

  it('a table too wide for even its narrowest columns scrolls with no colgroup', () => {
    env.width = 120
    const { container } = render(<ChatMessageBody text={SITREP} />)
    const table = tableOf(container)
    expect(table.querySelector('colgroup')).toBeNull()
    expect(table.style.tableLayout).toBe('')
  })

  it('a hidden table (0 wide) is not measured; showing it balances it after the debounce', () => {
    vi.useFakeTimers()
    env.width = 0
    const { container } = render(<ChatMessageBody text={SITREP} />)
    const table = tableOf(container)
    expect(env.measured).toBe(0)
    expect(table.querySelector('colgroup')).toBeNull()

    env.width = 600
    act(() => env.observers.forEach((cb) => cb()))
    expect(table.querySelector('colgroup')).toBeNull()
    act(() => {
      vi.advanceTimersByTime(BALANCE_RESIZE_DEBOUNCE_MS)
    })
    expect(colWidths(table).reduce((a, b) => a + b, 0)).toBeCloseTo(600, 5)
  })

  it('recomputes on a width change only, debounced, without re-measuring', () => {
    vi.useFakeTimers()
    const { container } = render(<ChatMessageBody text={SITREP} />)
    const table = tableOf(container)
    expect(colWidths(table).reduce((a, b) => a + b, 0)).toBeCloseTo(480, 5)
    const measuredOnce = env.measured

    // Same width: nothing to do.
    act(() => env.observers.forEach((cb) => cb()))
    act(() => {
      vi.advanceTimersByTime(BALANCE_RESIZE_DEBOUNCE_MS * 2)
    })
    expect(colWidths(table).reduce((a, b) => a + b, 0)).toBeCloseTo(480, 5)

    env.width = 400
    act(() => env.observers.forEach((cb) => cb()))
    act(() => {
      vi.advanceTimersByTime(BALANCE_RESIZE_DEBOUNCE_MS - 1)
    })
    expect(colWidths(table).reduce((a, b) => a + b, 0)).toBeCloseTo(480, 5)
    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(colWidths(table).reduce((a, b) => a + b, 0)).toBeCloseTo(400, 5)
    // Words are measured once per table and font, not per resize.
    expect(env.measured).toBe(measuredOnce)
  })

  it('unmounting removes the observers', () => {
    const { unmount } = render(<ChatMessageBody text={SITREP} />)
    expect(env.observers.length).toBeGreaterThan(0)
    unmount()
    expect(env.observers).toHaveLength(0)
  })
})
