// @vitest-environment jsdom
// thread-reply-to-v1 (0.46.18) in the desktop Thread: a reply's `↩ #N`
// line and a `#N` in a message jump to message N and highlight it the way
// K2 Companion does (data-highlight="true", FOCUS_HIGHLIGHT_MS); a `#N`
// links only when N is a message or card of this Thread; a target older
// than what is loaded loads older history first.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import type { OverlayThreadItem, ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { FOCUS_HIGHLIGHT_MS } from '@/components/Thread/threadFocus'

const reads = vi.hoisted(() => ({
  thread: new Map<number, boolean>(),
  calls: [] as Record<string, unknown>[],
}))
vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params: Record<string, unknown> = {}) => {
    if (route !== 'thread') return {}
    reads.calls.push(params)
    const seq = Number(params.before_seq) - 1
    const items = reads.thread.get(seq)
      ? [{ collection: 'thread', seq, id: `old${seq}`, doc: { id: `old${seq}`, kind: 'text', from: 'sales', body: 'old' } }]
      : []
    return { ok: true, items, has_more: true }
  }),
}))

const threadHook = vi.hoisted(() => ({
  items: [] as OverlayThreadItem[],
  conversationId: 'c',
  error: null as string | null,
  posting: false,
  post: async () => {},
  answer: async () => {},
  voidCard: async () => {},
  hasMore: false,
  loadingOlder: false,
  loaded: true,
  loadOlder: async () => {},
  loadAll: undefined as undefined | (() => Promise<void>),
  loadBack: vi.fn(async (_seq: number) => {}),
  turn: null as ThreadTurn | null,
  turnsReported: true,
  outbox: [] as OverlayThreadItem[],
}))
// The hook's state lives here; `rerenderHook` re-renders the pane with it
// (the pane is memoized, so a plain rerender with equal props would not).
const hookState = vi.hoisted(() => ({ force: null as null | ((n: (x: number) => number) => void) }))
vi.mock('./useOverlayThread', async () => {
  const { useState } = await import('react')
  return {
    useOverlayThread: () => {
      const [, force] = useState(0)
      hookState.force = force
      return threadHook
    },
  }
})
function rerenderHook(): void {
  act(() => hookState.force?.((n) => n + 1))
}
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

const room = testRoom({ tabs: {}, scope: primaryScope() })

function textItem(id: string, seq: number, body: string, doc: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem {
  return { collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'sales', body, created_at: 1_700_000_000 + seq, ...doc } }
}

function renderPane(): ReturnType<typeof renderInRoom> {
  return renderInRoom(
    room,
    <TabVisibilityContext.Provider value={true}>
      <ThreadOverlayPane addr="sales" conversationId="c" />
    </TabVisibilityContext.Provider>,
  )
}

const row = (id: string): HTMLElement => document.querySelector<HTMLElement>(`[data-zen-message="${id}"]`)!
const highlighted = (): string[] =>
  Array.from(document.querySelectorAll<HTMLElement>("[data-highlight='true']")).map((el) => el.getAttribute('data-zen-message') ?? '')
const refLinks = (id: string): string[] =>
  Array.from(row(id).querySelectorAll<HTMLElement>('[data-k2-thread-ref-link]')).map((el) => el.textContent ?? '')

let scrolled: { el: Element; arg: unknown }[] = []

beforeEach(() => {
  scrolled = []
  reads.thread.clear()
  reads.calls = []
  threadHook.hasMore = false
  threadHook.loadBack.mockReset()
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  vi.stubGlobal(
    'ResizeObserver',
    class {
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    },
  )
  Element.prototype.scrollIntoView = function (this: Element, arg?: unknown) {
    scrolled.push({ el: this, arg })
  } as Element['scrollIntoView']
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  resetThreadViewersForTests()
  threadHook.items = []
  threadHook.outbox = []
})

const CARD: OverlayThreadItem = {
  collection: 'thread',
  seq: 1,
  id: 'card1',
  doc: {
    id: 'card1',
    kind: 'choice',
    from: 'sales',
    body: 'Review the contrast pass?',
    created_at: 1_700_000_001,
    choice: { prompt: 'Review the contrast pass?', options: [{ label: 'Yes' }, { label: 'Later' }], allow_custom: false, status: 'pending' },
  } as OverlayThreadItem['doc'],
}

describe('a reply line jumps to what it answers', () => {
  it('clicking the ↩ line scrolls the card to the middle and highlights it for FOCUS_HIGHLIGHT_MS', () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    threadHook.items = [
      CARD,
      textItem('m2', 2, 'In between.'),
      textItem('m3', 3, '↩ #1 sales:\n> Review the contrast pass?\n\n  This is the card I meant.', { replyTo: { seq: 1, cardId: 'card1' } }),
    ]
    renderPane()
    expect(highlighted()).toEqual([])
    const line = row('m3').querySelector<HTMLElement>('[data-reply-jump]')
    expect(line).not.toBeNull()
    act(() => {
      fireEvent.click(line!)
    })
    expect(highlighted()).toEqual(['card1'])
    expect(scrolled.map((s) => (s.el as HTMLElement).getAttribute('data-zen-message'))).toEqual(['card1'])
    expect(scrolled[0].arg).toEqual({ block: 'center' })
    act(() => {
      vi.advanceTimersByTime(FOCUS_HIGHLIGHT_MS - 1)
    })
    expect(highlighted()).toEqual(['card1'])
    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(highlighted()).toEqual([])
  })

  it('an older server (no replyTo) jumps by the header’s #N', () => {
    threadHook.items = [textItem('m1', 1, 'Ship it?'), textItem('m2', 2, '↩ #1 sales:\n> Ship it?\n\nyes')]
    renderPane()
    act(() => {
      fireEvent.click(row('m2').querySelector<HTMLElement>('[data-reply-jump]')!)
    })
    expect(highlighted()).toEqual(['m1'])
  })

  it('a target older than what is loaded loads history back through it first', async () => {
    threadHook.hasMore = true
    threadHook.items = [textItem('m10', 10, 'ten'), textItem('m11', 11, '↩ #4 sales:\n> four\n\n  that one', { replyTo: { seq: 4 } })]
    threadHook.loadBack.mockImplementation(async () => {
      threadHook.items = [textItem('m4', 4, 'four'), ...threadHook.items]
    })
    renderPane()
    expect(row('m4')).toBeNull()
    await act(async () => {
      fireEvent.click(row('m11').querySelector<HTMLElement>('[data-reply-jump]')!)
    })
    expect(threadHook.loadBack).toHaveBeenCalledWith(4)
    expect(row('m4')).not.toBeNull()
    expect(highlighted()).toEqual(['m4'])
  })

  it('a target that is not there gives up quietly: nothing highlights', async () => {
    threadHook.items = [textItem('m5', 5, 'five'), textItem('m6', 6, '↩ #9 sales:\n> gone\n\nhm', { replyTo: { seq: 9 } })]
    renderPane()
    await act(async () => {
      fireEvent.click(row('m6').querySelector<HTMLElement>('[data-reply-jump]')!)
    })
    expect(highlighted()).toEqual([])
    expect(threadHook.loadBack).not.toHaveBeenCalled()
  })
})

describe('#N in a message', () => {
  it('links only a message or card of this Thread, never code, a URL fragment or GH#N', () => {
    threadHook.items = [
      CARD,
      textItem('m2', 2, 'two'),
      textItem('m3', 3, 'See #1 and #2, not #9, `#1`, [x](https://e.dev/#2) or GH#2.'),
    ]
    renderPane()
    expect(refLinks('m3')).toEqual(['#1', '#2'])
    expect(row('m3').textContent).toContain('not #9')
    act(() => {
      fireEvent.click(row('m3').querySelector<HTMLElement>('[data-k2-thread-ref-link="1"]')!)
    })
    expect(highlighted()).toEqual(['card1'])
  })

  it('an older #N links once the server says it is there; one that is not stays text', async () => {
    threadHook.hasMore = true
    reads.thread.set(4, true)
    threadHook.items = [textItem('m10', 10, 'ten'), textItem('m11', 11, 'Look at #4, not #5.')]
    renderPane()
    expect(refLinks('m11')).toEqual([])
    expect(await screen.findByText('#4', { selector: '[data-k2-thread-ref-link]' })).toBeTruthy()
    expect(refLinks('m11')).toEqual(['#4'])
    expect(reads.calls.map((c) => [c.before_seq, c.limit])).toEqual(
      expect.arrayContaining([
        [5, 1],
        [6, 1],
      ]),
    )
  })

  it('a newer message arriving makes its #N a link', () => {
    threadHook.items = [textItem('m1', 1, 'Watch #2.')]
    renderPane()
    expect(refLinks('m1')).toEqual([])
    threadHook.items = [...threadHook.items, textItem('m2', 2, 'two')]
    rerenderHook()
    expect(refLinks('m1')).toEqual(['#2'])
  })
})
