// @vitest-environment jsdom
//
// Chatter: an older page landing keeps the message being read where it is
// (usePrependHold). The pane used to keep the list's height from when the
// page was asked for; a re-pin meanwhile (here an arrival at the bottom)
// spent it, and the page landed with the reader thrown to its first row at
// the very top, where no scroll event asks for the next page. The fake
// layout is the chats' (test-utils/chat-layout).
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'
import type { OverlayThreadItem } from './overlayThread'
import {
  chatFrames,
  chatLayout,
  installChatLayout,
  readerScroll,
  rowOnScreen,
  rowTop,
  stubChatFrames,
  uninstallChatLayout,
} from '@/test-utils/chat-layout'

/** The mailbox: `all` on the server, `all.slice(from)` in memory. */
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
const PAGE = 25

vi.mock('./useOverlayChatter', async () => {
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
  let held: { from: number; all: unknown[]; items: unknown[] } | null = null
  const snapshot = (): unknown => {
    if (!store.snap) {
      if (!held || held.from !== store.from || held.all !== store.all) {
        held = { from: store.from, all: store.all, items: store.all.slice(store.from) }
      }
      store.snap = {
        items: held.items,
        conversationId: 'c',
        error: null,
        hasMore: store.from > 0,
        loadingOlder: store.loadingOlder,
        loading: false,
        loadOlder,
        loadAll: async () => {},
      }
    }
    return store.snap
  }
  return {
    useOverlayChatter: () =>
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

import { ChatterOverlayPane } from './ChatterOverlayPane'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'

const room = testRoom({ tabs: {}, scope: primaryScope() })

function msg(seq: number): OverlayThreadItem {
  const id = `c${seq}`
  const from = seq % 2 ? 'sales' : 'ops'
  return { collection: 'chatter', seq, id, doc: { id, kind: 'chatter', from, to: from === 'sales' ? 'ops' : 'sales', body: `b${seq}`, via: 'msg', created_at: 1_700_000_000 + seq } }
}

function land(): void {
  const done = store.pending.shift()
  if (!done) throw new Error('no older read in flight')
  act(() => done())
}

function upToTheTop(): void {
  for (let i = 0; i < 200 && chatLayout.scrollTop > 0; i++) readerScroll(-300)
  expect(chatLayout.scrollTop).toBe(0)
}

function open(): void {
  renderInRoom(room, <ChatterOverlayPane addr="sales" conversationId="c" />)
  chatFrames(5)
}

beforeAll(() => installChatLayout({ view: 900, rowPx: (key) => 30 + (Number(key.slice(1)) % 4) * 5 }))
afterAll(uninstallChatLayout)

beforeEach(() => {
  vi.useFakeTimers()
  stubChatFrames()
  store.all = Array.from({ length: 200 }, (_, i) => msg(i + 1))
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
})

describe('Chatter: older pages', () => {
  it('a page landing keeps the row being read where it was (an arrival meanwhile)', () => {
    open()
    upToTheTop()
    expect(store.pending).toHaveLength(1)
    chatFrames(10)
    // A new message at the bottom while the page is on its way.
    store.all = [...store.all, msg(201)]
    act(() => store.emit())
    chatFrames(15)
    const before = rowOnScreen()
    expect(before).not.toBeNull()
    land()
    chatFrames(3)
    expect(rowTop(before?.key ?? '')).toBe(before?.top)
    expect(chatLayout.scrollTop).toBeGreaterThan(PAGE * 30)
  })

  it('pages keep coming as the reader keeps going up', () => {
    open()
    for (let page = 1; page <= 4; page++) {
      upToTheTop()
      expect(store.pending, `page ${page} asked for`).toHaveLength(1)
      chatFrames(25)
      land()
      chatFrames(3)
      expect(chatLayout.scrollTop, `page ${page} landed above the reader`).toBeGreaterThan(0)
    }
    expect(store.from).toBe(50)
  })
})
