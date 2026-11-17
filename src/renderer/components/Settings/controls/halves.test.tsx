// @vitest-environment jsdom
// The halves' scroll fades (ScrollEdge): each split half fades at its bottom
// edge while there is more below, and the fade ends exactly at the half's
// bottom edge. Browsers inset a sticky child by its scroller's padding, so a
// half with bottom padding held its fade that far above the edge (rows under
// the fade showed in full, cut by a hard line). The split halves keep their
// space under the last row inside the bottom fade instead, and have no
// bottom padding of their own.
import { afterEach, describe, expect, it } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ScrollEdge, halves } from './halves'

afterEach(cleanup)

const PADDING_BOTTOM = /(^|\s)(pb|py|p)-/

/** jsdom has no layout: give the half its scroll numbers. */
function setScroll(el: HTMLElement, { scrollTop, clientHeight, scrollHeight }: Record<string, number>): void {
  Object.defineProperty(el, 'clientHeight', { configurable: true, value: clientHeight })
  Object.defineProperty(el, 'scrollHeight', { configurable: true, value: scrollHeight })
  el.scrollTop = scrollTop
}

function Half({ rows }: { rows: number }): React.JSX.Element {
  return (
    <div data-testid="half" className={halves.left(false)}>
      <ScrollEdge at="top" inset="-mx-6" />
      {Array.from({ length: rows }, (_, i) => (
        <p key={i}>Row {i}</p>
      ))}
      <ScrollEdge at="bottom" inset="-mx-6" room />
    </div>
  )
}

const edge = (at: 'top' | 'bottom'): HTMLElement =>
  screen.getByTestId('half').querySelector(`[data-scroll-edge="${at}"]`) as HTMLElement

describe('halves: the bottom fade sits on the half’s bottom edge', () => {
  it('a split half has no bottom padding, so nothing lifts its sticky fade off the edge', () => {
    for (const cls of [halves.left(false), halves.right(false)]) {
      expect(cls).toContain('overflow-y-auto')
      expect(cls).not.toMatch(PADDING_BOTTOM)
    }
  })

  it('stacked, the halves keep their space under the last row (the body scrolls, not them)', () => {
    expect(halves.left(true)).toContain('pb-8')
    expect(halves.right(true)).toContain('pb-8')
  })

  it('the bottom fade is the half’s last child, sticky at bottom 0, holding the space under the last row', () => {
    render(<Half rows={3} />)
    const half = screen.getByTestId('half')
    const bottom = edge('bottom')
    expect(half.lastElementChild).toBe(bottom)
    expect(bottom.className).toMatch(/(^|\s)sticky(\s|$)/)
    expect(bottom.className).toMatch(/(^|\s)bottom-0(\s|$)/)
    // The room is the edge's own height (the space the half's padding was),
    // so the edge's bottom is the end of the scroll: stuck, it is the half's
    // bottom edge.
    expect(bottom.className).toMatch(/(^|\s)h-8(\s|$)/)
    expect(bottom.className).not.toMatch(/(^|\s)h-0(\s|$)/)
    // The gradient ends at the edge's bottom, not above it.
    const fade = bottom.firstElementChild as HTMLElement
    expect(fade.className).toMatch(/(^|\s)bottom-0(\s|$)/)
    expect(fade.className).toContain('bg-gradient-to-t')
  })

  it('without room (the stacked body’s edges), an edge takes no space', () => {
    render(<Half rows={3} />)
    expect(edge('top').className).toMatch(/(^|\s)h-0(\s|$)/)
    expect(edge('top').className).toMatch(/(^|\s)top-0(\s|$)/)
  })
})

describe('halves: a fade shows only while there is more that way', () => {
  it('nothing to scroll: neither fade shows', () => {
    render(<Half rows={3} />)
    setScroll(screen.getByTestId('half'), { scrollTop: 0, clientHeight: 400, scrollHeight: 400 })
    act(() => {
      fireEvent.scroll(screen.getByTestId('half'))
    })
    expect(edge('bottom').hasAttribute('data-more')).toBe(false)
    expect(edge('top').hasAttribute('data-more')).toBe(false)
    expect((edge('bottom').firstElementChild as HTMLElement).className).toContain('opacity-0')
  })

  it('more below: the bottom fade shows; at the end it goes', () => {
    render(<Half rows={40} />)
    const half = screen.getByTestId('half')
    setScroll(half, { scrollTop: 0, clientHeight: 400, scrollHeight: 1200 })
    act(() => {
      fireEvent.scroll(half)
    })
    expect(edge('bottom').getAttribute('data-more')).toBe('true')
    expect((edge('bottom').firstElementChild as HTMLElement).className).toContain('opacity-100')
    expect(edge('top').hasAttribute('data-more')).toBe(false)

    setScroll(half, { scrollTop: 800, clientHeight: 400, scrollHeight: 1200 })
    act(() => {
      fireEvent.scroll(half)
    })
    expect(edge('bottom').hasAttribute('data-more')).toBe(false)
    expect(edge('top').getAttribute('data-more')).toBe('true')
  })
})
