// @vitest-environment jsdom
//
// Rosson (0.46.12): "sometimes when I scroll up really far and then I try to
// scroll back down really quickly, it gets stuck and then I can't scroll
// down or up" (until the window reloads).
//
// The Thread overlay pane against a fake layout, frame by frame the way a
// browser runs them: the scroll event (when the position moved), then
// requestAnimationFrame, then ResizeObserver (when the rows' column changed
// size). Rows are short (one-line messages) in a tall view; "Earlier
// messages" reads resolve when the test says. jsdom lays nothing out, so the
// getters below compute offsets from the DOM the pane drew (spacers by their
// inline height and margins, offsets rounded the way a browser rounds them).
//
// What got stuck: near the oldest loaded message, the render window moving
// off it hides "Earlier messages" (the row under the reader's eye moves up
// 40px, so the window writes scrollTop back by 40px), and that write's own
// scroll event read as the reader turning around. The window grew at the
// other end, "Earlier messages" came back, the opposite write read as
// another turn... one window change and one scrollTop write every frame,
// forever. In the desktop app's WebKit every programmatic scroll stops the
// reader's own, so the list would not move up or down.
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { createElement } from 'react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import type { OverlayThreadItem } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'

const room = testRoom({ tabs: {}, scope: primaryScope() })

/** The Thread's history: `all` on the server, `all.slice(from)` in memory. */
const store = vi.hoisted(() => {
  const s = {
    all: [] as unknown[],
    from: 0,
    loadingOlder: false,
    pending: [] as Array<() => void>,
    listeners: new Set<() => void>(),
    snap: null as unknown,
    emit(): void {
      s.snap = null
      for (const l of Array.from(s.listeners)) l()
    },
  }
  return s
})

/** One "Earlier messages" page. */
const PAGE = 25

vi.mock('./useOverlayThread', async () => {
  const React = await import('react')
  const loadOlder = async (): Promise<void> => {
    if (store.loadingOlder || store.from <= 0) return
    store.loadingOlder = true
    store.emit()
    await new Promise<void>((resolve) => {
      store.pending.push(() => {
        store.from = Math.max(0, store.from - PAGE)
        store.loadingOlder = false
        store.emit()
        resolve()
      })
    })
  }
  // The real hook hands over the same arrays until they change.
  const none: unknown[] = []
  let held: { from: number; all: unknown[]; items: unknown[] } | null = null
  const itemsFor = (from: number): unknown[] => {
    if (!held || held.from !== from || held.all !== store.all) held = { from, all: store.all, items: store.all.slice(from) }
    return held.items
  }
  const snapshot = (): unknown => {
    if (!store.snap) {
      store.snap = {
        items: itemsFor(store.from),
        conversationId: 'c',
        error: null,
        posting: false,
        post: async () => {},
        answer: async () => {},
        voidCard: async () => {},
        hasMore: store.from > 0,
        loadingOlder: store.loadingOlder,
        loaded: true,
        loadOlder,
        loadAll: async () => {},
        turn: null,
        turnsReported: true,
        outbox: none,
      }
    }
    return store.snap
  }
  return {
    useOverlayThread: () =>
      React.useSyncExternalStore((cb: () => void) => {
        store.listeners.add(cb)
        return () => store.listeners.delete(cb)
      }, snapshot),
  }
})
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))
vi.mock('@/hooks/useReducedMotion', () => ({ useReducedMotion: () => true }))

// ── Fake layout ────────────────────────────────────────────────────────────

const VIEW = 900
/** "Earlier messages" (the button). */
const OLDER_PX = 30
/** One-line messages: 30–45px. */
const rowPx = (key: string): number => 30 + (Number(key.slice(1)) % 4) * 5

const box = { scrollTop: 0 }
/** The test's own (the reader's) scroll writes, which aren't counted. */
let readerWrite = false
/** scrollTop writes by the code that moved the list. */
let codeMoves = 0

function isColumn(el: Element | null): el is HTMLElement {
  return !!el && (el as HTMLElement).hasAttribute?.('data-thread-column')
}
function isScroller(el: Element): boolean {
  return (el as HTMLElement).getAttribute?.('data-testid') === 'thread-overlay-list'
}
function heightOf(el: HTMLElement): number {
  if (el.hasAttribute('data-win-spacer')) return parseFloat(el.style.height) || 0
  if (el.hasAttribute('data-zen-message')) return rowPx(el.getAttribute('data-zen-message') ?? 'm0')
  if (el.hasAttribute('data-zen-load-older')) return OLDER_PX
  return 0
}
function marginOf(el: HTMLElement, side: 'top' | 'bottom'): number {
  return parseFloat(side === 'top' ? el.style.marginTop : el.style.marginBottom) || 0
}
/** Each child's top in the column, and the column's height. */
function layout(col: HTMLElement): { tops: Map<Element, number>; height: number } {
  const gap = parseFloat(col.style.gap) || 0
  const tops = new Map<Element, number>()
  let pos = 0
  const kids = Array.from(col.children) as HTMLElement[]
  kids.forEach((el, i) => {
    pos += marginOf(el, 'top')
    tops.set(el, pos)
    pos += heightOf(el) + marginOf(el, 'bottom')
    if (i < kids.length - 1) pos += gap
  })
  return { tops, height: pos }
}
function columnOf(scroller: HTMLElement): HTMLElement {
  const c = scroller.querySelector('[data-thread-column]')
  if (!isColumn(c)) throw new Error('no column')
  return c
}

const saved: Record<string, PropertyDescriptor | undefined> = {}
const PROPS = ['offsetTop', 'offsetHeight', 'clientHeight', 'scrollHeight', 'scrollTop'] as const
beforeAll(() => {
  const proto = HTMLElement.prototype
  for (const k of PROPS) saved[k] = Object.getOwnPropertyDescriptor(proto, k)
  saved.rect = Object.getOwnPropertyDescriptor(Element.prototype, 'getBoundingClientRect')
  Object.defineProperty(proto, 'offsetTop', {
    configurable: true,
    get(this: HTMLElement) {
      const p = this.parentElement
      return isColumn(p) ? Math.round(layout(p).tops.get(this) ?? 0) : 0
    },
  })
  Object.defineProperty(proto, 'offsetHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return Math.round(heightOf(this))
    },
  })
  Object.defineProperty(proto, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return isScroller(this) ? VIEW : 0
    },
  })
  Object.defineProperty(proto, 'scrollHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return isScroller(this) ? Math.round(layout(columnOf(this)).height) : 0
    },
  })
  Object.defineProperty(proto, 'scrollTop', {
    configurable: true,
    get(this: HTMLElement) {
      return isScroller(this) ? box.scrollTop : 0
    },
    set(this: HTMLElement, v: number) {
      if (!isScroller(this)) return
      const next = Math.round(Math.max(0, Math.min(v, this.scrollHeight - VIEW)))
      if (!readerWrite && next !== box.scrollTop) codeMoves += 1
      box.scrollTop = next
    },
  })
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      const p = this.parentElement
      const top = isColumn(p) ? (layout(p).tops.get(this) ?? 0) - box.scrollTop : 0
      const height = isScroller(this) ? VIEW : heightOf(this)
      return { top, bottom: top + height, left: 0, right: 600, width: 600, height, x: 0, y: top, toJSON: () => ({}) }
    },
  })
})
afterAll(() => {
  const proto = HTMLElement.prototype
  for (const k of PROPS) {
    const d = saved[k]
    if (d) Object.defineProperty(proto, k, d)
    else delete (proto as unknown as Record<string, unknown>)[k]
  }
  if (saved.rect) Object.defineProperty(Element.prototype, 'getBoundingClientRect', saved.rect)
})

class FakeResizeObserver {
  static all: FakeResizeObserver[] = []
  observed: Element[] = []
  constructor(private readonly cb: ResizeObserverCallback) {
    FakeResizeObserver.all.push(this)
  }
  observe(el: Element): void {
    this.observed.push(el)
  }
  unobserve(el: Element): void {
    this.observed = this.observed.filter((o) => o !== el)
  }
  disconnect(): void {
    this.observed = []
  }
  fire(): void {
    this.cb([], this as unknown as ResizeObserver)
  }
}

// ── Frames ─────────────────────────────────────────────────────────────────

let rafQ: Array<{ id: number; cb: FrameRequestCallback }> = []
let rafId = 0
let lastEventTop = -1
let lastColumnPx = -1

const list = (): HTMLElement => screen.getByTestId('thread-overlay-list')

/** One frame: the scroll event (if the position moved), rAF, then resize. */
function frame(): void {
  act(() => {
    if (box.scrollTop !== lastEventTop) {
      lastEventTop = box.scrollTop
      fireEvent.scroll(list())
    }
  })
  act(() => {
    const q = rafQ
    rafQ = []
    for (const r of q) r.cb(performance.now())
  })
  act(() => {
    const px = layout(columnOf(list())).height
    if (px !== lastColumnPx) {
      lastColumnPx = px
      for (const ro of FakeResizeObserver.all) if (ro.observed.length) ro.fire()
    }
  })
  act(() => {
    vi.advanceTimersByTime(16)
  })
}
function frames(n: number): void {
  for (let i = 0; i < n; i++) frame()
}

/** The reader scrolls by `dy` px (a wheel, a fling's frame). */
function wheel(dy: number): void {
  readerWrite = true
  list().scrollTop = box.scrollTop + dy
  readerWrite = false
  frame()
}

/** A fling: fast, slowing down. */
function fling(px: number): void {
  for (let d = px; Math.abs(d) > 5; d *= 0.85) wheel(Math.round(d))
}

function drawn(): string {
  const rows = Array.from(list().querySelectorAll('[data-zen-message]'))
  return `${rows[0]?.getAttribute('data-zen-message')}..${rows[rows.length - 1]?.getAttribute('data-zen-message')}`
}

/** The first row on screen and where it is on screen. */
function onScreen(): { key: string; top: number } | null {
  for (const r of Array.from(list().querySelectorAll<HTMLElement>('[data-zen-message]'))) {
    const b = r.getBoundingClientRect()
    if (b.bottom > 1 && b.top < VIEW) return { key: r.getAttribute('data-zen-message') ?? '', top: Math.round(b.top) }
  }
  return null
}

/** Where row `key` is on screen (null: not drawn). */
function rowTop(key: string): number | null {
  const r = list().querySelector<HTMLElement>(`[data-zen-message="${key}"]`)
  return r ? Math.round(r.getBoundingClientRect().top) : null
}

function land(): void {
  const done = store.pending.shift()
  if (!done) throw new Error('no Earlier messages read in flight')
  act(() => done())
}

function item(seq: number): OverlayThreadItem {
  const id = `m${seq}`
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'k2', body: `b${seq}`, created_at: 1 } }
}

function open(): void {
  renderInRoom(
    room,
    createElement(
      TabVisibilityContext.Provider,
      { value: true },
      createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c', agentName: 'k2' }),
    ),
  )
  frames(5)
}

/** The reader scrolls up until the list can't go further. */
function upToTheTop(): void {
  for (let i = 0; i < 200 && box.scrollTop > 0; i++) wheel(-300)
  expect(box.scrollTop).toBe(0)
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeResizeObserver.all = []
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  rafQ = []
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    rafId += 1
    rafQ.push({ id: rafId, cb })
    return rafId
  })
  vi.stubGlobal('cancelAnimationFrame', (id: number) => {
    rafQ = rafQ.filter((r) => r.id !== id)
  })
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  // 200 on the server, the newest 50 in memory.
  store.all = Array.from({ length: 200 }, (_, i) => item(i + 1))
  store.from = 150
  store.loadingOlder = false
  store.pending = []
  store.emit()
  box.scrollTop = 0
  readerWrite = false
  codeMoves = 0
  lastEventTop = -1
  lastColumnPx = -1
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  resetThreadViewersForTests()
})

describe('Thread: up far, then back down fast', () => {
  it('the list comes to rest once the reader stops: no window change and no scroll write every frame', () => {
    // Pages already read: 100 in memory, more than the window draws.
    store.from = 100
    store.emit()
    open()
    upToTheTop()
    // Reading back to the first message in memory; the next page is asked
    // for and takes a while.
    expect(store.pending).toHaveLength(1)
    frames(25)
    expect(screen.getByTestId('overlay-load-older')).toBeTruthy()
    // Back down, quickly, and a small move back up.
    fling(150)
    wheel(-50)
    // The reader has stopped. Give everything a moment to settle...
    frames(30)
    // ...then nothing may move on its own.
    const at = { top: box.scrollTop, rows: drawn(), row: onScreen() }
    codeMoves = 0
    const seen = new Set<string>()
    for (let i = 0; i < 60; i++) {
      frame()
      seen.add(`${box.scrollTop} ${drawn()}`)
    }
    expect(codeMoves, `scrollTop writes by the code while the reader is still (window ${[...seen].join(' | ')})`).toBe(0)
    expect(seen.size).toBe(1)
    expect({ top: box.scrollTop, rows: drawn(), row: onScreen() }).toEqual(at)
    // The reader can still scroll both ways.
    const before = onScreen()
    for (let i = 0; i < 4; i++) wheel(-100)
    const up = onScreen()
    expect(up?.key).not.toBe(before?.key)
    for (let i = 0; i < 8; i++) wheel(100)
    expect(onScreen()?.key).not.toBe(up?.key)
  })

  it('reading down slowly from the oldest loaded row: every step settles', () => {
    store.from = 100
    store.emit()
    open()
    upToTheTop()
    expect(store.pending).toHaveLength(1)
    frames(25)
    for (let i = 0; i < 60; i++) {
      wheel(i % 7 === 6 ? -15 : 25)
      frames(2)
      codeMoves = 0
      const rows = drawn()
      frames(3)
      expect(codeMoves, `step ${i}: the list still moving on its own`).toBe(0)
      expect(drawn(), `step ${i}`).toBe(rows)
    }
  })

  it('an older page landing while the reader waits at the top keeps the row they were reading where it was (an arrival meanwhile)', () => {
    open()
    upToTheTop()
    expect(store.pending).toHaveLength(1)
    // The read takes a while: the window trims the rows far below
    // meanwhile, and the agent posts a new message at the bottom.
    frames(10)
    store.all = [...store.all, item(201)]
    act(() => store.emit())
    frames(15)
    const before = onScreen()
    expect(before).not.toBeNull()
    land()
    frames(3)
    // The page drew above: the same row, at the same place on screen.
    expect(rowTop(before?.key ?? '')).toBe(before?.top)
    expect(box.scrollTop).toBeGreaterThan(PAGE * 30)
    // And reading on up reaches the page that landed (and asks for the next).
    upToTheTop()
    expect(store.pending).toHaveLength(1)
  })

  it('pages keep coming as the reader keeps going up', () => {
    open()
    for (let page = 1; page <= 4; page++) {
      upToTheTop()
      expect(store.pending, `page ${page} asked for`).toHaveLength(1)
      frames(25)
      land()
      frames(3)
      expect(box.scrollTop, `page ${page} landed above the reader`).toBeGreaterThan(0)
    }
    expect(store.from).toBe(50)
  })
})
