// @vitest-environment jsdom
//
// The shared nav row drag (Agents sidebar + Home roster): the pure order
// math, and the gesture — a press under the threshold stays a click, a
// drag paints the ghost/line/cursor, reports its gap once, and swallows
// the click it would fire.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { useRef } from 'react'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import {
  ROW_DRAG_THRESHOLD,
  RowDropLine,
  dropTargetIndex,
  isRowDragExempt,
  reorderedZoneIds,
  useRowDragReorder,
} from './row-drag-reorder'

describe('dropTargetIndex', () => {
  it('a row dropped in its own gap either side does not move', () => {
    expect(dropTargetIndex(2, 2)).toBeNull()
    expect(dropTargetIndex(2, 3)).toBeNull()
  })
  it('down counts the row it left; up does not', () => {
    expect(dropTargetIndex(0, 3)).toBe(2)
    expect(dropTargetIndex(3, 0)).toBe(0)
    expect(dropTargetIndex(1, 4)).toBe(3)
  })
  it('a row not in the list never moves', () => {
    expect(dropTargetIndex(-1, 0)).toBeNull()
  })
})

describe('reorderedZoneIds', () => {
  it('a whole-list zone is a plain move', () => {
    expect(reorderedZoneIds(['a', 'b', 'c', 'd'], ['a', 'b', 'c', 'd'], 'a', 3)).toEqual(['b', 'c', 'a', 'd'])
    expect(reorderedZoneIds(['a', 'b', 'c', 'd'], ['a', 'b', 'c', 'd'], 'd', 0)).toEqual(['d', 'a', 'b', 'c'])
  })

  it('rows outside the zone keep their slots; zone rows refill theirs in the new order', () => {
    // Pinned p1,p2 and unpinned u1,u2 interleaved in the canonical order.
    const all = ['u1', 'p1', 'u2', 'p2']
    expect(reorderedZoneIds(all, ['p1', 'p2'], 'p2', 0)).toEqual(['u1', 'p2', 'u2', 'p1'])
  })

  it('focus group view: hidden group rows never move, the visible list reorders around them', () => {
    // g1 = group shown; g2 = another group (not on screen); n = ungrouped (shown in every group view).
    const all = ['g1a', 'g2a', 'n1', 'g1b', 'g2b']
    const visible = ['g1a', 'n1', 'g1b']
    const next = reorderedZoneIds(all, visible, 'g1b', 0)
    expect(next).toEqual(['g1b', 'g2a', 'g1a', 'n1', 'g2b'])
    expect(next![1]).toBe('g2a')
    expect(next![4]).toBe('g2b')
  })

  it('returns a full permutation (every id once)', () => {
    const all = ['a', 'b', 'c', 'd', 'e']
    const next = reorderedZoneIds(all, ['b', 'd', 'e'], 'e', 0)!
    expect([...next].sort()).toEqual([...all].sort())
    expect(next).toHaveLength(all.length)
  })

  it('no move → null; a zone row the full list lacks → null (never drops a row)', () => {
    expect(reorderedZoneIds(['a', 'b'], ['a', 'b'], 'a', 1)).toBeNull()
    expect(reorderedZoneIds(['a', 'b'], ['a', 'b'], 'x', 0)).toBeNull()
    expect(reorderedZoneIds(['a', 'b'], ['a', 'ghost', 'b'], 'b', 0)).toBeNull()
  })

  it('a gap past the end clamps to the end', () => {
    expect(reorderedZoneIds(['a', 'b', 'c'], ['a', 'b', 'c'], 'a', 99)).toEqual(['b', 'c', 'a'])
  })
})

describe('isRowDragExempt', () => {
  it('the row shell drags; in-row controls keep their press', () => {
    document.body.innerHTML = `
      <div><button data-agent-row=""><span id="name">Nova</span><span role="button" id="chip">tag</span></button>
      <button id="worktree">wt</button></div>`
    expect(isRowDragExempt(document.getElementById('name'))).toBe(false)
    expect(isRowDragExempt(document.querySelector('[data-agent-row]'))).toBe(false)
    expect(isRowDragExempt(document.getElementById('chip'))).toBe(true)
    expect(isRowDragExempt(document.getElementById('worktree'))).toBe(true)
    expect(isRowDragExempt(null)).toBe(true)
  })
})

// ── The gesture ──────────────────────────────────────────────────────────

const ROW_H = 40

function Harness({ rows, onDrop, onPick }: {
  rows: string[]
  onDrop: (from: number, gap: number) => void
  onPick: (id: string) => void
}): React.JSX.Element {
  const drag = useRowDragReorder<'z'>()
  const listRef = useRef<HTMLDivElement>(null)
  return (
    <div ref={listRef}>
      {rows.map((id, idx) => (
        <div
          key={id}
          data-row={id}
          data-testid={`wrap-${id}`}
          style={{ opacity: drag.isDragging('z', idx) ? 0.4 : 1 }}
          onMouseDown={(e) =>
            drag.onRowMouseDown(e, {
              zone: 'z',
              index: idx,
              container: () => listRef.current,
              rowSelector: '[data-row]',
              onDrop,
            })
          }
          onClickCapture={drag.onRowClickCapture}
        >
          {drag.showsDropLineAt('z', idx) && <RowDropLine />}
          <button data-agent-row="" onClick={() => onPick(id)}>{id}</button>
        </div>
      ))}
      {drag.showsDropLineAt('z', rows.length) && <RowDropLine />}
    </div>
  )
}

function stubRowRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const rows = Array.from(document.querySelectorAll('[data-row]'))
    const i = rows.indexOf(this)
    const top = i < 0 ? 0 : i * ROW_H
    return { top, height: ROW_H, bottom: top + ROW_H, left: 0, right: 200, width: 200, x: 0, y: top, toJSON: () => ({}) } as DOMRect
  })
}

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  document.body.style.cursor = ''
  document.body.style.userSelect = ''
})

describe('useRowDragReorder', () => {
  it('a press that moves less than the threshold is a click, not a drag', () => {
    stubRowRects()
    const onDrop = vi.fn()
    const onPick = vi.fn()
    render(<Harness rows={['a', 'b', 'c']} onDrop={onDrop} onPick={onPick} />)
    const a = screen.getByText('a')
    fireEvent.mouseDown(a, { button: 0, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10 + ROW_DRAG_THRESHOLD.x, clientY: 20 + ROW_DRAG_THRESHOLD.y })
    expect(screen.getByTestId('wrap-a').style.opacity).toBe('1')
    expect(document.body.style.cursor).toBe('')
    fireEvent.mouseUp(document)
    fireEvent.click(a)
    expect(onDrop).not.toHaveBeenCalled()
    expect(onPick).toHaveBeenCalledWith('a')
  })

  it('a drag paints ghost + line + grabbing cursor, drops once, and swallows its click', async () => {
    stubRowRects()
    const onDrop = vi.fn()
    const onPick = vi.fn()
    const { container } = render(<Harness rows={['a', 'b', 'c']} onDrop={onDrop} onPick={onPick} />)
    const a = screen.getByText('a')
    fireEvent.mouseDown(a, { button: 0, clientX: 10, clientY: 20 })
    // Below c's middle (2*40+20 = 100) → gap 3 (the end).
    fireEvent.mouseMove(document, { clientX: 10, clientY: 110 })
    expect(screen.getByTestId('wrap-a').style.opacity).toBe('0.4')
    expect(document.body.style.cursor).toBe('grabbing')
    expect(document.body.style.userSelect).toBe('none')
    const lines = container.querySelectorAll('[data-row-drop-line]')
    expect(lines).toHaveLength(1)
    expect(lines[0].parentElement).toBe(container.firstElementChild)
    expect(lines[0].previousElementSibling).toBe(screen.getByTestId('wrap-c'))

    fireEvent.mouseUp(document)
    fireEvent.click(screen.getByText('c'))
    expect(onDrop).toHaveBeenCalledTimes(1)
    expect(onDrop).toHaveBeenCalledWith(0, 3)
    expect(onPick).not.toHaveBeenCalled()
    expect(document.body.style.cursor).toBe('')
    expect(screen.getByTestId('wrap-a').style.opacity).toBe('1')
    expect(container.querySelectorAll('[data-row-drop-line]')).toHaveLength(0)
  })

  it('a drag released off the rows does not eat the next real click', async () => {
    stubRowRects()
    const onPick = vi.fn()
    render(<Harness rows={['a', 'b']} onDrop={vi.fn()} onPick={onPick} />)
    fireEvent.mouseDown(screen.getByText('a'), { button: 0, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 70 })
    fireEvent.mouseUp(document)
    await act(async () => { await new Promise((r) => setTimeout(r, 0)) })
    fireEvent.click(screen.getByText('b'))
    expect(onPick).toHaveBeenCalledWith('b')
  })

  it('the right button never starts a drag', () => {
    stubRowRects()
    const onDrop = vi.fn()
    render(<Harness rows={['a', 'b']} onDrop={onDrop} onPick={vi.fn()} />)
    fireEvent.mouseDown(screen.getByText('a'), { button: 2, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 90 })
    fireEvent.mouseUp(document)
    expect(onDrop).not.toHaveBeenCalled()
    expect(screen.getByTestId('wrap-a').style.opacity).toBe('1')
  })
})
