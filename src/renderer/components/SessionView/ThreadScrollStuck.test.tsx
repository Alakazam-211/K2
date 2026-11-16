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
// messages" reads resolve when the test says. The fake layout and frames are
// the chats' (test-utils/chat-layout).
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
import { act, cleanup, screen } from '@testing-library/react'
import { createElement } from 'react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import type { OverlayThreadItem } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import {
  chatFrame,
  chatFrames,
  chatLayout,
  drawnRows,
  installChatLayout,
  readerScroll,
  rowOnScreen,
  rowTop,
  stubChatFrames,
  uninstallChatLayout,
} from '@/test-utils/chat-layout'

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

// ── Fake layout and frames (test-utils/chat-layout) ───────────────────────

beforeAll(() => installChatLayout({ view: 900, rowPx: (key) => 30 + (Number(key.slice(1)) % 4) * 5 }))
afterAll(uninstallChatLayout)

const box = chatLayout
const frame = chatFrame
const frames = chatFrames
const wheel = readerScroll
const drawn = drawnRows
const onScreen = rowOnScreen

/** A fling: fast, slowing down. */
function fling(px: number): void {
  for (let d = px; Math.abs(d) > 5; d *= 0.85) wheel(Math.round(d))
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
  stubChatFrames()
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  // 200 on the server, the newest 50 in memory.
  store.all = Array.from({ length: 200 }, (_, i) => item(i + 1))
  store.from = 150
  store.loadingOlder = false
  store.pending = []
  store.emit()
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
    box.codeMoves = 0
    const seen = new Set<string>()
    for (let i = 0; i < 60; i++) {
      frame()
      seen.add(`${box.scrollTop} ${drawn()}`)
    }
    expect(box.codeMoves, `scrollTop writes by the code while the reader is still (window ${[...seen].join(' | ')})`).toBe(0)
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
      box.codeMoves = 0
      const rows = drawn()
      frames(3)
      expect(box.codeMoves, `step ${i}: the list still moving on its own`).toBe(0)
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
