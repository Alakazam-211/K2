// @vitest-environment jsdom
//
// The chat render window as a hook (useRenderWindow, 0.46.2), against a
// fake layout: every row is 100px, the list's gap is 10px, the scroller
// shows 500px. jsdom lays nothing out, so the layout getters below compute
// offsets from the DOM the hook drew (spacers by their inline height and
// margins). Checks the window's bounds, the spacers, the held row, the
// follow-the-newest trim, memory first, and the reset on re-entry.
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, useMemo, useRef } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { useRenderWindow, type RenderWindow } from './useRenderWindow'
import { ThreadWindowSpacer } from './ThreadZenKit'
import { WINDOW_IDLE_MS, WINDOW_MAX_ROWS, WINDOW_ROWS, WINDOW_STEP } from './threadWindow'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const ROW = 100
const GAP = 10
const VIEW = 500

// ── Fake layout ────────────────────────────────────────────────────────────

const box = { scrollTop: 0 }

function isList(el: Element | null): el is HTMLElement {
  return !!el && (el as HTMLElement).hasAttribute?.('data-list')
}

function heightOf(el: HTMLElement): number {
  if (el.hasAttribute('data-win-spacer')) return parseFloat(el.style.height) || 0
  if (el.hasAttribute('data-zen-message')) return ROW
  return 0
}

function marginOf(el: HTMLElement, side: 'top' | 'bottom'): number {
  return parseFloat(side === 'top' ? el.style.marginTop : el.style.marginBottom) || 0
}

/** Each child's top in the list, and the list's height. */
function layout(list: HTMLElement): { tops: Map<Element, number>; height: number } {
  const tops = new Map<Element, number>()
  let pos = 0
  const kids = Array.from(list.children) as HTMLElement[]
  kids.forEach((el, i) => {
    pos += marginOf(el, 'top')
    tops.set(el, pos)
    pos += heightOf(el) + marginOf(el, 'bottom')
    if (i < kids.length - 1) pos += GAP
  })
  return { tops, height: pos }
}

function listOf(scroller: HTMLElement): HTMLElement {
  const list = scroller.querySelector('[data-list]')
  if (!isList(list)) throw new Error('no list')
  return list
}

const saved: Record<string, PropertyDescriptor | undefined> = {}
beforeAll(() => {
  const proto = HTMLElement.prototype
  for (const k of ['offsetTop', 'offsetHeight', 'clientHeight', 'scrollHeight', 'scrollTop']) {
    saved[k] = Object.getOwnPropertyDescriptor(proto, k)
  }
  saved.rect = Object.getOwnPropertyDescriptor(Element.prototype, 'getBoundingClientRect')
  Object.defineProperty(proto, 'offsetTop', {
    configurable: true,
    get(this: HTMLElement) {
      const parent = this.parentElement
      return isList(parent) ? layout(parent).tops.get(this) ?? 0 : 0
    },
  })
  Object.defineProperty(proto, 'offsetHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return heightOf(this)
    },
  })
  Object.defineProperty(proto, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.hasAttribute('data-scroller') ? VIEW : 0
    },
  })
  Object.defineProperty(proto, 'scrollHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return this.hasAttribute('data-scroller') ? layout(listOf(this)).height : 0
    },
  })
  Object.defineProperty(proto, 'scrollTop', {
    configurable: true,
    get(this: HTMLElement) {
      return this.hasAttribute('data-scroller') ? box.scrollTop : 0
    },
    set(this: HTMLElement, v: number) {
      if (!this.hasAttribute('data-scroller')) return
      box.scrollTop = Math.max(0, Math.min(v, this.scrollHeight - VIEW))
    },
  })
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      const parent = this.parentElement
      const top = isList(parent) ? (layout(parent).tops.get(this) ?? 0) - box.scrollTop : 0
      const height = this.hasAttribute?.('data-scroller') ? VIEW : heightOf(this)
      return { top, bottom: top + height, left: 0, right: 0, width: 0, height, x: 0, y: top, toJSON: () => ({}) }
    },
  })
})
afterAll(() => {
  const proto = HTMLElement.prototype
  for (const k of ['offsetTop', 'offsetHeight', 'clientHeight', 'scrollHeight', 'scrollTop']) {
    const d = saved[k]
    if (d) Object.defineProperty(proto, k, d)
    else delete (proto as unknown as Record<string, unknown>)[k]
  }
  if (saved.rect) Object.defineProperty(Element.prototype, 'getBoundingClientRect', saved.rect)
})

// ── Harness ────────────────────────────────────────────────────────────────

let win!: RenderWindow
let followingNow = true

function Chat({ n, room = 'r', extra = 0 }: { n: number; room?: string; extra?: number }): React.JSX.Element {
  const scrollRef = useRef<HTMLDivElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const keys = useMemo(() => {
    const k = Array.from({ length: n }, (_, i) => `k${i}`)
    for (let i = 0; i < extra; i++) k.push(`new${i}`)
    return k
  }, [n, extra])
  win = useRenderWindow({
    room,
    keys,
    scroller: () => scrollRef.current,
    list: () => listRef.current,
    following: () => followingNow,
    stick: (el) => {
      el.scrollTop = el.scrollHeight
    },
  })
  return (
    <div data-scroller="" data-testid="scroller" ref={scrollRef} onScroll={() => win.onScroll()}>
      <div data-list="" ref={listRef} style={{ rowGap: `${GAP}px` }}>
        <ThreadWindowSpacer px={win.above} gap={GAP} edge="top" />
        {keys.slice(win.lo, win.hi).map((k) => (
          <div key={k} data-zen-message={k} data-still={win.wasDrawn(k) ? '' : undefined} />
        ))}
        <ThreadWindowSpacer px={win.below} gap={GAP} edge="bottom" />
      </div>
    </div>
  )
}

function drawnKeys(c: HTMLElement): string[] {
  return Array.from(c.querySelectorAll('[data-zen-message]')).map((el) => el.getAttribute('data-zen-message') ?? '')
}

function scroller(c: HTMLElement): HTMLElement {
  const el = c.querySelector('[data-scroller]') as HTMLElement | null
  if (!el) throw new Error('no scroller')
  return el
}

/** The screen top of the row with `key`. */
function screenTop(c: HTMLElement, key: string): number {
  const el = c.querySelector(`[data-zen-message="${key}"]`) as HTMLElement | null
  if (!el) throw new Error(`row ${key} is not drawn`)
  return el.getBoundingClientRect().top
}

/** Scroll to `top` (the reader's gesture) and let the frame run. */
async function scrollTo(c: HTMLElement, top: number): Promise<void> {
  const el = scroller(c)
  el.scrollTop = top
  fireEvent.scroll(el)
  await act(async () => {
    vi.advanceTimersByTime(20)
  })
}

/** The reader at the newest (a scroll there, so the hook knows). */
async function atNewest(c: HTMLElement): Promise<void> {
  const el = scroller(c)
  await scrollTo(c, el.scrollHeight)
}

async function idle(): Promise<void> {
  await act(async () => {
    vi.advanceTimersByTime(WINDOW_IDLE_MS * 3)
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  box.scrollTop = 0
  followingNow = true
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('the render window', () => {
  it('opens at the newest 30 of 500 with no spacer above (never measured)', () => {
    const { container } = render(<Chat n={500} />)
    const keys = drawnKeys(container)
    expect(keys).toHaveLength(WINDOW_ROWS)
    expect(keys[0]).toBe('k470')
    expect(keys[keys.length - 1]).toBe('k499')
    expect([win.above, win.below, win.drawsNewest, win.drawsOldest]).toEqual([0, 0, true, false])
  })

  it('reading up draws 20 more from memory before the top, holding the row on screen', async () => {
    const { container } = render(<Chat n={500} />)
    followingNow = false
    await atNewest(container)
    const el = scroller(container)
    // Up, the window's top within 1.5 screens of the view.
    el.scrollTop = 400
    const before = screenTop(container, 'k474')
    expect(before).toBe(4 * (ROW + GAP) - 400)
    await scrollTo(container, 400)
    const keys = drawnKeys(container)
    expect(keys).toHaveLength(WINDOW_ROWS + WINDOW_STEP)
    expect(keys[0]).toBe('k450')
    // The new rows went in above; scrollTop moved by their height, so what
    // the reader looks at did not move.
    expect(screenTop(container, 'k474')).toBe(before)
    expect(el.scrollTop).toBe(400 + WINDOW_STEP * (ROW + GAP))
  })

  it('at the cap, growing the top drops as many wholly off-screen rows at the bottom, behind a measured spacer', async () => {
    const { container } = render(<Chat n={500} />)
    followingNow = false
    await atNewest(container)
    const el = scroller(container)
    await scrollTo(container, 400)
    expect(drawnKeys(container)).toHaveLength(50)
    const heightBefore = el.scrollHeight
    // Up near the (new) top: 20 more above, 20 dropped below.
    await scrollTo(container, 300)
    const keys = drawnKeys(container)
    expect(keys.length).toBeLessThanOrEqual(WINDOW_MAX_ROWS)
    expect(keys[0]).toBe('k430')
    expect(keys[keys.length - 1]).toBe('k479')
    // The rows that left are a spacer of their measured height (row + gap),
    // so the list is as tall as before, plus the 20 new rows above.
    expect(win.below).toBe(20 * (ROW + GAP))
    expect(el.scrollHeight).toBe(heightBefore + 20 * (ROW + GAP))
  })

  it('reading back down draws the dropped rows again (no message-in animation) and drops the top behind a spacer', async () => {
    const { container } = render(<Chat n={500} />)
    followingNow = false
    await atNewest(container)
    const el = scroller(container)
    await scrollTo(container, 400)
    await scrollTo(container, 300)
    expect(drawnKeys(container)).not.toContain('k499')
    const heightBefore = el.scrollHeight
    // Down toward the bottom spacer: within 1.5 screens of the last row.
    await scrollTo(container, el.scrollHeight - VIEW - 20 * (ROW + GAP))
    const keys = drawnKeys(container)
    expect(keys[0]).toBe('k450')
    expect(keys.at(-1)).toBe('k499')
    expect(keys).toHaveLength(WINDOW_MAX_ROWS)
    expect([win.above, win.below]).toEqual([20 * (ROW + GAP), 0])
    // The spacers stood in exactly: the list did not change height.
    expect(el.scrollHeight).toBe(heightBefore)
    const back = container.querySelector('[data-zen-message="k499"]') as HTMLElement
    expect(back.hasAttribute('data-still')).toBe(true)
    const never = container.querySelector('[data-zen-message="k450"]') as HTMLElement
    expect(never.hasAttribute('data-still')).toBe(false)
  })

  it('following the newest: arrivals draw at once, and once quiet the oldest drop to 30 with the view still at the newest', async () => {
    const view = render(<Chat n={500} />)
    const el = scroller(view.container)
    el.scrollTop = el.scrollHeight
    const heightBefore = el.scrollHeight
    view.rerender(<Chat n={500} extra={25} />)
    expect(drawnKeys(view.container)).toHaveLength(WINDOW_ROWS + 25)
    expect(drawnKeys(view.container).at(-1)).toBe('new24')
    el.scrollTop = el.scrollHeight
    await idle()
    const keys = drawnKeys(view.container)
    expect(keys).toHaveLength(WINDOW_ROWS)
    expect(keys[0]).toBe('k495')
    // 25 rows left; their measured height stands in for them.
    expect(win.above).toBe(25 * (ROW + GAP))
    expect(el.scrollHeight).toBe(heightBefore + 25 * (ROW + GAP))
    expect(el.scrollTop).toBe(el.scrollHeight - VIEW)
  })

  it('memory first: growTop draws rows above the window, and says false at the first row in memory', async () => {
    const { container } = render(<Chat n={60} />)
    followingNow = false
    expect(drawnKeys(container)[0]).toBe('k30')
    let grew = false
    await act(async () => {
      grew = win.growTop()
    })
    expect(grew).toBe(true)
    expect(drawnKeys(container)[0]).toBe('k10')
    await act(async () => {
      grew = win.growTop()
    })
    expect(grew).toBe(true)
    expect(win.drawsOldest).toBe(true)
    expect(drawnKeys(container)[0]).toBe('k0')
    await act(async () => {
      grew = win.growTop()
    })
    expect(grew).toBe(false)
  })

  it('toNewest brings the newest back after reading far up', async () => {
    const { container } = render(<Chat n={500} />)
    followingNow = false
    await atNewest(container)
    const el = scroller(container)
    await scrollTo(container, 400)
    await scrollTo(container, 300)
    expect(win.drawsNewest).toBe(false)
    followingNow = true
    await act(async () => {
      win.toNewest(true)
    })
    expect(win.drawsNewest).toBe(true)
    expect(drawnKeys(container).at(-1)).toBe('k499')
    expect(el.scrollTop).toBe(el.scrollHeight - VIEW)
  })

  it('a new chat (re-entry) starts at 30 again', async () => {
    const view = render(<Chat n={500} />)
    followingNow = false
    await act(async () => {
      win.growTop()
    })
    expect(drawnKeys(view.container)).toHaveLength(50)
    view.rerender(<Chat n={500} room="other" />)
    expect(drawnKeys(view.container)).toHaveLength(WINDOW_ROWS)
    expect([win.above, win.below]).toEqual([0, 0])
  })
})
