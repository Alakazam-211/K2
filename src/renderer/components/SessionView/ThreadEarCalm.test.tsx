// @vitest-environment jsdom
// Rosson 2026-11-23 (0.46.17): "if it jumps around in height, it makes it
// hard to read the messages above."
//
// The desktop Thread: the list, the message box under it, and the open
// activity ear on the box, against a fake layout. The box floats over the
// list's foot and the list pads its bottom by the box's height, so the ear
// changing height changed the list's padding; a list resting at the newest
// message re-pinned to the bottom on every change, and the messages moved
// up and down with the ear. The turn's frames come in the way the daemon
// sends them (each tool call as it starts and ends, thinking between, a
// reply), through the Thread's own turn (useOverlayThread → the column →
// the box's drawer).
//
// Fake layout: the list is 400px tall over 1000px of messages plus its
// bottom padding; the box is 60px plus the ear (its lines, as
// test-utils/activity-ear stacks them, or its own min-height). Frames run
// the way a browser does: the scroll event when the position moved,
// ResizeObserver, then requestAnimationFrame.
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { createElement } from 'react'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'
import { useThreadComposeDrawer } from './threadComposeDrawer'
import type { OverlayThreadItem, ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { FakeResizeObserver } from '@/test-utils/chat-layout'
import { burst, earRectPx, frame } from '@/test-utils/activity-ear'
import { ZEN_DRAWER_COALESCE_MS, ZEN_DRAWER_EXPANDED_KEY, ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'

const room = testRoom({ tabs: {}, scope: primaryScope() })

/** The Thread as useOverlayThread hands it over: messages and the live turn. */
const store = vi.hoisted(() => {
  const s = {
    items: [] as unknown[],
    turn: null as unknown,
    snap: null as unknown,
    listeners: new Set<() => void>(),
    emit(): void {
      s.snap = null
      for (const l of Array.from(s.listeners)) l()
    },
  }
  return s
})

vi.mock('./useOverlayThread', async () => {
  const React = await import('react')
  const none: unknown[] = []
  const snapshot = (): unknown => {
    if (!store.snap) {
      store.snap = {
        items: store.items,
        conversationId: 'c',
        error: null,
        posting: false,
        post: async () => {},
        answer: async () => {},
        voidCard: async () => {},
        hasMore: false,
        loadingOlder: false,
        loaded: true,
        loadOlder: async () => {},
        loadAll: async () => {},
        turn: store.turn,
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

// ── Fake layout ─────────────────────────────────────────────────────────

const VIEW = 400
const CONTENT = 1000
const BOX = 60
const layout = { top: 0, readerWrite: false, codeMoves: 0, lastEventTop: -1 }

function isList(el: Element): el is HTMLElement {
  return (el as HTMLElement).getAttribute?.('data-testid') === 'thread-overlay-list'
}
function scrollMax(el: HTMLElement): number {
  return Math.max(0, CONTENT + (parseFloat(el.style.paddingBottom) || 0) - VIEW)
}
function earPx(slot: Element): number {
  const detail = slot.querySelector<HTMLElement>('[data-zen-drawer-detail]')
  if (detail) return earRectPx(detail)
  return slot.querySelector('[data-zen-drawer-tile]') ? 32 : 0
}
function heightOf(el: HTMLElement): number {
  if (el.getAttribute?.('data-testid') === 'agent-session-thread-compose-slot') return BOX + earPx(el)
  if (el.hasAttribute?.('data-zen-drawer-detail')) return earRectPx(el)
  if (isList(el)) return VIEW
  return 0
}

const PROPS = ['clientHeight', 'scrollHeight', 'scrollTop'] as const
const saved: Record<string, PropertyDescriptor | undefined> = {}

beforeAll(() => {
  const proto = HTMLElement.prototype
  for (const k of PROPS) saved[k] = Object.getOwnPropertyDescriptor(proto, k)
  saved.rect = Object.getOwnPropertyDescriptor(Element.prototype, 'getBoundingClientRect')
  Object.defineProperty(proto, 'clientHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return isList(this) ? VIEW : 0
    },
  })
  Object.defineProperty(proto, 'scrollHeight', {
    configurable: true,
    get(this: HTMLElement) {
      return isList(this) ? CONTENT + (parseFloat(this.style.paddingBottom) || 0) : 0
    },
  })
  Object.defineProperty(proto, 'scrollTop', {
    configurable: true,
    get(this: HTMLElement) {
      if (!isList(this)) return 0
      // A shorter list clamps the position, as a browser does.
      layout.top = Math.min(layout.top, scrollMax(this))
      return layout.top
    },
    set(this: HTMLElement, v: number) {
      if (!isList(this)) return
      const next = Math.round(Math.max(0, Math.min(v, scrollMax(this))))
      if (!layout.readerWrite && next !== layout.top) layout.codeMoves += 1
      layout.top = next
    },
  })
  Object.defineProperty(Element.prototype, 'getBoundingClientRect', {
    configurable: true,
    value(this: HTMLElement) {
      const height = heightOf(this)
      return { top: 0, bottom: height, left: 0, right: 600, width: 600, height, x: 0, y: 0, toJSON: () => ({}) }
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

let rafQ: FrameRequestCallback[] = []

function list(): HTMLElement {
  return screen.getByTestId('thread-overlay-list')
}

/** One browser frame: the scroll event (if the position moved), resize
 *  observers, then requestAnimationFrame, then 16ms. */
function browserFrame(): void {
  act(() => {
    const top = list().scrollTop
    if (top !== layout.lastEventTop) {
      layout.lastEventTop = top
      fireEvent.scroll(list())
    }
  })
  act(() => {
    for (const ro of FakeResizeObserver.all) if (ro.observed.length) ro.fire()
  })
  act(() => {
    const q = rafQ
    rafQ = []
    for (const cb of q) cb(performance.now())
  })
  act(() => {
    vi.advanceTimersByTime(16)
  })
}

function settle(n = 4): void {
  for (let i = 0; i < n; i++) browserFrame()
}

/** The daemon's next frame of the turn, then the frames until the next. */
function send(turn: ThreadTurn | null, gapMs: number): void {
  act(() => {
    store.turn = turn
    store.emit()
  })
  const frames = Math.max(1, Math.round(gapMs / 16))
  for (let i = 0; i < frames; i++) browserFrame()
}

function open(): void {
  renderInRoom(
    room,
    createElement(ThreadOverlayColumn, {
      addr: 'sales',
      conversationId: 'c',
      active: true,
      composeBar: createElement(EarBox),
    }),
  )
  settle(6)
}

/** The message box's part that matters here: the drawer on its top edge,
 *  fed the column's live turn, as TerminalComposeBar draws it. */
function EarBox(): React.JSX.Element {
  const drawer = useThreadComposeDrawer()
  return createElement(
    'div',
    { 'data-compose-bar': '' },
    drawer ? createElement(ZenActivityDrawer, { turn: drawer.turn, margin: '0' }) : null,
    createElement('textarea', { 'aria-label': 'Message' }),
  )
}

function item(seq: number): OverlayThreadItem {
  const id = `m${seq}`
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'k2', body: `b${seq}`, created_at: 1 } }
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeResizeObserver.all = []
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  rafQ = []
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    rafQ.push(cb)
    return rafQ.length
  })
  vi.stubGlobal('cancelAnimationFrame', () => {})
  layout.top = 0
  layout.codeMoves = 0
  layout.lastEventTop = -1
  localStorage.setItem(ZEN_DRAWER_EXPANDED_KEY, '1')
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  store.items = Array.from({ length: 12 }, (_, i) => item(i + 1))
  store.turn = null
  store.emit()
})

afterEach(() => {
  cleanup()
  localStorage.removeItem(ZEN_DRAWER_EXPANDED_KEY)
  vi.unstubAllGlobals()
  vi.useRealTimers()
  resetThreadViewersForTests()
})

/** A burst the way tool calls run: frames 20–60ms apart, then a pause. */
function runBurst(onFrame: () => void): void {
  for (const [i, f] of burst().entries()) {
    send(f, i % 3 === 2 ? ZEN_DRAWER_COALESCE_MS + 40 : 32)
    onFrame()
  }
  settle(Math.ceil(ZEN_DRAWER_COALESCE_MS / 16) + 2)
  onFrame()
}

describe('the open activity ear does not move the messages above it', () => {
  it('a reader scrolled up: the list does not move at all through a burst of activity', () => {
    open()
    send(frame(1, 'thinking', null), 100)
    expect(document.querySelector('[data-zen-drawer-detail]')).not.toBeNull()
    // The reader scrolls up to read an earlier message.
    layout.readerWrite = true
    list().scrollTop = 200
    layout.readerWrite = false
    settle()
    expect(list().scrollTop).toBe(200)
    layout.codeMoves = 0
    const tops: number[] = []
    runBurst(() => tops.push(list().scrollTop))
    runBurst(() => tops.push(list().scrollTop))
    expect(new Set(tops), `scrollTop through the burst: ${tops.join(',')}`).toEqual(new Set([200]))
    expect(layout.codeMoves).toBe(0)
  })

  it('a reader at the newest message: the list moves only as the ear first grows, never back down, then holds', () => {
    open()
    send(frame(1, 'thinking', null), 100)
    settle()
    // Resting at the newest message (pinned to the bottom).
    expect(list().scrollTop).toBe(scrollMax(list()))
    const tops: number[] = []
    const pads: string[] = []
    const ears: number[] = []
    const note = (): void => {
      tops.push(list().scrollTop)
      pads.push(list().style.paddingBottom)
      const d = document.querySelector<HTMLElement>('[data-zen-drawer-detail]')
      ears.push(d ? earRectPx(d) : 0)
    }
    runBurst(note)
    // The first run may grow the ear (to the turn's tallest so far)...
    for (let i = 1; i < tops.length; i++) {
      expect(tops[i], `scrollTop ${tops.join(',')}`).toBeGreaterThanOrEqual(tops[i - 1])
      expect(ears[i], `ear ${ears.join(',')}`).toBeGreaterThanOrEqual(ears[i - 1])
    }
    // ...and the same kind of burst again moves nothing: the list's
    // padding, its position and the ear hold still.
    tops.length = 0
    pads.length = 0
    ears.length = 0
    layout.codeMoves = 0
    runBurst(note)
    runBurst(note)
    expect(new Set(tops).size, `scrollTop ${tops.join(',')}`).toBe(1)
    expect(new Set(pads).size, `padding ${pads.join(',')}`).toBe(1)
    expect(new Set(ears).size, `ear ${ears.join(',')}`).toBe(1)
    expect(layout.codeMoves).toBe(0)
  })
})
