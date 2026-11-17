// @vitest-environment jsdom
//
// The shared nav row drag (Agents sidebar + Home roster): the pure order
// math, and the gesture — a press under the threshold stays a click, a
// drag paints the ghost/cursor and a drop line floating over the list (no
// row moves), reports its gap once, and swallows the click it would fire.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { useRef } from 'react'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import {
  DROP_LINE_CAP_PX,
  NAV_ROW_TEXT_INSET,
  ROW_DRAG_THRESHOLD,
  RowDropLine,
  dropLineY,
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

describe('dropLineY', () => {
  function list(tops: number[], height = 200): HTMLElement {
    document.body.innerHTML = `<div id="list">${tops.map((t) => `<div data-r="" data-top="${t}"></div>`).join('')}</div>`
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const row = this.dataset.top !== undefined
      const t = row ? Number(this.dataset.top) : 100
      const h = row ? 40 : height
      return { top: t, height: h, bottom: t + h, left: 0, right: 200, width: 200, x: 0, y: t, toJSON: () => ({}) } as DOMRect
    })
    return document.getElementById('list')!
  }
  afterEach(() => vi.restoreAllMocks())

  it('touching rows: on the seam; top and bottom kept half a cap inside', () => {
    // Rows at 100, 140, 180 in a list from 100, 200 tall.
    const box = list([100, 140, 180])
    expect(dropLineY(box, '[data-r]', 0, 2)).toBe(DROP_LINE_CAP_PX / 2)
    expect(dropLineY(box, '[data-r]', 1, 2)).toBe(40)
    expect(dropLineY(box, '[data-r]', 2, 0)).toBe(80)
    expect(dropLineY(box, '[data-r]', 3, 0)).toBe(120)
    // A list exactly as tall as its rows: the bottom keeps half a cap in.
    expect(dropLineY(list([100, 140], 80), '[data-r]', 2, 0)).toBe(80 - DROP_LINE_CAP_PX / 2)
  })

  it('a caption between two rows: the line keeps to the dragged row\'s side', () => {
    // a at 100, a 24px caption, b at 164.
    const box = list([100, 164])
    // a moving down to the end of its run: under a.
    expect(dropLineY(box, '[data-r]', 1, 0)).toBe(40)
    // b moving up to the start of its run: over b.
    expect(dropLineY(box, '[data-r]', 1, 1)).toBe(64)
  })

  it('no rows: no line', () => {
    expect(dropLineY(list([]), '[data-r]', 0, 0)).toBeNull()
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

function Harness({ rows, onDrop, onPick, landingGap, bounded = false }: {
  rows: string[]
  onDrop: (from: number, gap: number) => void
  onPick: (id: string) => void
  landingGap?: (from: number, gap: number) => number | null
  bounded?: boolean
}): React.JSX.Element {
  const drag = useRowDragReorder<'z'>()
  const listRef = useRef<HTMLDivElement>(null)
  return (
    <div data-bounds="">
      <div ref={listRef} data-list="" className="relative">
        <RowDropLine top={drag.dropLineTop('z')} inset={NAV_ROW_TEXT_INSET} />
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
                landingGap,
                bounds: bounded ? () => document.querySelector('[data-bounds]') : undefined,
                onDrop,
              })
            }
            onClickCapture={drag.onRowClickCapture}
          >
            <button data-agent-row="" onClick={() => onPick(id)}>{id}</button>
          </div>
        ))}
      </div>
    </div>
  )
}

/** Rows ROW_H tall from y=0; the list is as tall as its rows; the bounds
 *  box is x 0–200, y 0–400. */
function stubRowRects(): void {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const rows = Array.from(document.querySelectorAll('[data-row]'))
    const i = rows.indexOf(this)
    const top = i < 0 ? 0 : i * ROW_H
    const height = this.hasAttribute('data-list') ? rows.length * ROW_H : this.hasAttribute('data-bounds') ? 400 : ROW_H
    return { top, height, bottom: top + height, left: 0, right: 200, width: 200, x: 0, y: top, toJSON: () => ({}) } as DOMRect
  })
}

function line(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[data-row-drop-line]')
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
    // It floats in the list (absolute, its first child, so the last row
    // stays last), at the list's bottom: 3 rows, kept half a cap inside,
    // its 2px centred there.
    expect(lines[0].parentElement).toBe(container.querySelector('[data-list]'))
    expect(lines[0].nextElementSibling).toBe(screen.getByTestId('wrap-a'))
    expect(lines[0].className).toContain('absolute')
    expect((lines[0] as HTMLElement).style.top).toBe(`${3 * ROW_H - DROP_LINE_CAP_PX / 2 - 1}px`)

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

  it('no row moves while dragging: the line floats, no gap opens, rows keep their place and transform', () => {
    stubRowRects()
    const { container } = render(<Harness rows={['a', 'b', 'c', 'd']} onDrop={vi.fn()} onPick={vi.fn()} />)
    const list = container.querySelector('[data-list]')!
    const rowsBefore = Array.from(list.querySelectorAll('[data-row]'))
    fireEvent.mouseDown(screen.getByText('a'), { button: 0, clientX: 10, clientY: 20 })
    for (const y of [70, 110, 150, 5]) {
      fireEvent.mouseMove(document, { clientX: 10, clientY: y })
      const rows = Array.from(list.querySelectorAll('[data-row]'))
      expect(rows).toEqual(rowsBefore)
      // The rows stay the list's children, in order, the last one last; the
      // only other child is the floating line.
      const others = Array.from(list.children).filter((c) => !c.hasAttribute('data-row-drop-line'))
      expect(others).toEqual(rowsBefore)
      expect(list.lastElementChild).toBe(rowsBefore[rowsBefore.length - 1])
      expect(list.children.length).toBeLessThanOrEqual(rows.length + 1)
      for (const r of rows) expect((r as HTMLElement).style.transform).toBe('')
      expect(container.querySelector('[data-row-drop-gap]')).toBeNull()
    }
    fireEvent.mouseUp(document)
  })

  it('the line sits on the seam of the gap it will land in: top, middle and bottom', () => {
    stubRowRects()
    render(<Harness rows={['a', 'b', 'c', 'd']} onDrop={vi.fn()} onPick={vi.fn()} />)
    // Drag c (index 2): its own gaps (2, 3) draw nothing.
    fireEvent.mouseDown(screen.getByText('c'), { button: 0, clientX: 10, clientY: 100 })
    const at = (y: number): string | null => {
      fireEvent.mouseMove(document, { clientX: 10, clientY: y })
      return line()?.style.top ?? null
    }
    // Top: gap 0, kept half a cap inside the list.
    expect(at(5)).toBe(`${DROP_LINE_CAP_PX / 2 - 1}px`)
    // Middle: gap 1, on the seam between a and b (40), the 2px centred.
    expect(at(30)).toBe(`${ROW_H - 1}px`)
    // Its own place: no line.
    expect(at(95)).toBeNull()
    expect(at(125)).toBeNull()
    // Bottom: gap 4, half a cap inside the list's end.
    expect(at(155)).toBe(`${4 * ROW_H - DROP_LINE_CAP_PX / 2 - 1}px`)
    // The line spans the row text's inset, with its 6px cap at the left.
    const l = line()!
    expect(l.style.left).toBe(`${NAV_ROW_TEXT_INSET.left}px`)
    expect(l.style.right).toBe(`${NAV_ROW_TEXT_INSET.right}px`)
    expect(l.style.height).toBe('2px')
    const cap = l.querySelector<HTMLElement>('[data-row-drop-line-cap]')!
    expect(cap.style.width).toBe(`${DROP_LINE_CAP_PX}px`)
    expect(cap.style.height).toBe(`${DROP_LINE_CAP_PX}px`)
    fireEvent.mouseUp(document)
  })

  it('the line is square in Terminal and round in Glass (the pill radius token), and jumps under reduced motion', () => {
    stubRowRects()
    render(<Harness rows={['a', 'b']} onDrop={vi.fn()} onPick={vi.fn()} />)
    fireEvent.mouseDown(screen.getByText('a'), { button: 0, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 75 })
    const l = line()!
    expect(l.className).toContain('rounded-[var(--radius-pill)]')
    expect(l.querySelector('[data-row-drop-line-cap]')!.className).toContain('rounded-[var(--radius-pill)]')
    expect(l.className).toContain('bg-[var(--color-accent)]')
    expect(l.className).toContain('transition-[top]')
    expect(l.className).toContain('motion-reduce:transition-none')
    fireEvent.mouseUp(document)
  })

  it('outside its bounds there is no line, and a release there does nothing', () => {
    stubRowRects()
    const onDrop = vi.fn()
    render(<Harness rows={['a', 'b', 'c']} onDrop={onDrop} onPick={vi.fn()} bounded />)
    fireEvent.mouseDown(screen.getByText('a'), { button: 0, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 110 })
    expect(line()).not.toBeNull()
    // Off to the side (the main pane), then below the panel.
    fireEvent.mouseMove(document, { clientX: 260, clientY: 110 })
    expect(line()).toBeNull()
    fireEvent.mouseMove(document, { clientX: 10, clientY: 450 })
    expect(line()).toBeNull()
    fireEvent.mouseUp(document)
    expect(onDrop).not.toHaveBeenCalled()
  })

  it('landingGap: the line marks the clamped landing and a release drops there; null means no line and no drop', () => {
    stubRowRects()
    const onDrop = vi.fn()
    // Rows a,b are one section and c,d another; a row lands only in its own.
    const landing = (from: number, gap: number): number | null => (from < 2 ? Math.min(gap, 2) : gap < 2 ? null : gap)
    render(<Harness rows={['a', 'b', 'c', 'd']} onDrop={onDrop} onPick={vi.fn()} landingGap={landing} />)
    fireEvent.mouseDown(screen.getByText('a'), { button: 0, clientX: 10, clientY: 20 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 155 })
    expect(line()!.style.top).toBe(`${2 * ROW_H - 1}px`)
    fireEvent.mouseUp(document)
    expect(onDrop).toHaveBeenCalledWith(0, 2)
    onDrop.mockClear()
    fireEvent.mouseDown(screen.getByText('d'), { button: 0, clientX: 10, clientY: 140 })
    fireEvent.mouseMove(document, { clientX: 10, clientY: 5 })
    expect(line()).toBeNull()
    fireEvent.mouseUp(document)
    expect(onDrop).not.toHaveBeenCalled()
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
