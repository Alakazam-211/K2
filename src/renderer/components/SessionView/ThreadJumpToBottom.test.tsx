// @vitest-environment jsdom
// The Thread's "Jump to bottom" pill (ThreadJumpPill in ThreadOverlayPane):
// shown while scrolled up, its words turning into the new-message count (or
// "Sent"), one element whose width animates, a click to the bottom, the
// count reset there, and a polite announcement per count change. Scroll
// metrics are stubbed and timers are fake: nothing waits on a real clock.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'
import { createElement } from 'react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import { ThreadOverlayPane } from './ThreadOverlayPane'
import type { OverlayThreadItem, ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { THREAD_JUMP_PILL_ANIMATING, THREAD_JUMP_PILL_WIDTH_MS, threadJumpPillWords } from '@/components/Thread/ThreadJumpPill'

const room = testRoom({ tabs: {}, scope: primaryScope() })

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
  loadAll: async () => {},
  turn: null as ThreadTurn | null,
  turnsReported: true,
  outbox: [] as OverlayThreadItem[],
}))
const motion = vi.hoisted(() => ({ reduced: false }))

vi.mock('./useOverlayThread', () => ({ useOverlayThread: () => threadHook }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))
vi.mock('@/hooks/useReducedMotion', () => ({ useReducedMotion: () => motion.reduced }))

function textItem(id: string, seq: number, o: { mine?: boolean } = {}): OverlayThreadItem {
  return {
    collection: 'thread',
    seq,
    id,
    doc: { id, kind: 'text', from: 'k2', body: `m${seq}`, created_at: 1, ...(o.mine ? { mine: true } : {}) },
  }
}

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  constructor(private readonly cb: ResizeObserverCallback) {
    FakeResizeObserver.instances.push(this)
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

type Box = { scrollHeight: number; clientHeight: number; scrollTop: number }

let renders = 0
function pane() {
  renders += 1
  // A changing prop: the pane is memoized, so a rerender must differ.
  return createElement(
    TabVisibilityContext.Provider,
    { value: true },
    createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c', agentName: `k2 ${renders}` }),
  )
}

function list(): HTMLElement {
  return screen.getByTestId('thread-overlay-list')
}
function pill(): HTMLButtonElement {
  return screen.getByTestId('thread-jump-pill') as HTMLButtonElement
}
function words(): string {
  return pill().querySelector('[data-jump-text]')?.textContent ?? ''
}

/** Render with stubbed metrics, pinned at the bottom. */
function setup(): { box: Box; rerender: () => void; scrollTo: ReturnType<typeof vi.fn> } {
  const view = renderInRoom(room, pane())
  const el = list()
  const box: Box = { scrollHeight: 1000, clientHeight: 200, scrollTop: 0 }
  Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => box.scrollHeight })
  Object.defineProperty(el, 'clientHeight', { configurable: true, get: () => box.clientHeight })
  Object.defineProperty(el, 'scrollTop', {
    configurable: true,
    get: () => box.scrollTop,
    set: (v: number) => {
      box.scrollTop = Math.min(v, box.scrollHeight - box.clientHeight)
    },
  })
  const scrollTo = vi.fn()
  ;(el as unknown as { scrollTo: unknown }).scrollTo = scrollTo
  // The box gets its height: the pane's first pin (its ResizeObserver).
  act(() => {
    for (const ro of FakeResizeObserver.instances) if (ro.observed.includes(el)) ro.fire()
  })
  expect(box.scrollTop).toBe(800)
  return { box, scrollTo, rerender: () => view.rerender(pane()) }
}

function scrollTo(box: Box, top: number): void {
  box.scrollTop = top
  act(() => {
    fireEvent.scroll(list())
  })
}

function arrive(rerender: () => void, ...more: OverlayThreadItem[]): void {
  threadHook.items = [...threadHook.items, ...more]
  act(() => rerender())
}

beforeEach(() => {
  vi.useFakeTimers()
  FakeResizeObserver.instances = []
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  threadHook.items = [textItem('t1', 1), textItem('t2', 2)]
  threadHook.outbox = []
  motion.reduced = false
  // Words measure by length (jsdom lays nothing out). Only the words: the
  // arrow (icon or glyph) is in the fixed part.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const words = this.querySelector('[data-jump-text]')?.textContent ?? ''
    const w = this.hasAttribute('data-jump-inner') ? words.length * 7 + 36 : 0
    return { width: w, height: 24, top: 0, left: 0, right: w, bottom: 24, x: 0, y: 0, toJSON: () => ({}) } as DOMRect
  })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  vi.useRealTimers()
  resetThreadViewersForTests()
  threadHook.items = []
  threadHook.outbox = []
})

describe('Thread: Jump to bottom pill', () => {
  it('words and accessible names follow the phone', () => {
    expect(threadJumpPillWords(0, false)).toMatchObject({ text: 'Jump to bottom', label: 'Jump to bottom', accent: false })
    expect(threadJumpPillWords(1, false)).toMatchObject({ text: 'New message', label: 'New message, jump to bottom', accent: true })
    expect(threadJumpPillWords(3, true)).toMatchObject({ text: '3 new messages', label: '3 new messages, jump to bottom', accent: true })
    expect(threadJumpPillWords(0, true)).toMatchObject({ text: 'Sent', label: 'Sent, jump to bottom', accent: false })
  })

  it('shows when scrolled up with nothing new: "Jump to bottom", quiet', () => {
    const { box } = setup()
    expect(pill().hasAttribute('data-shown')).toBe(false)
    scrollTo(box, 300)
    const p = pill()
    expect(p.tagName).toBe('BUTTON')
    expect(p.hasAttribute('data-shown')).toBe(true)
    expect(p.getAttribute('aria-hidden')).toBeNull()
    expect(p.tabIndex).toBe(0)
    expect(words()).toBe('Jump to bottom')
    expect(p.getAttribute('aria-label')).toBe('Jump to bottom')
    expect(p.hasAttribute('data-accent')).toBe(false)
    expect(p.querySelector('[data-k2-icon="arrow-down"]')).not.toBeNull()
  })

  it('is hidden within the threshold of the bottom', () => {
    const { box } = setup()
    scrollTo(box, 300)
    expect(pill().hasAttribute('data-shown')).toBe(true)
    // 32px from the bottom still counts as the bottom.
    scrollTo(box, 768)
    const p = pill()
    expect(p.hasAttribute('data-shown')).toBe(false)
    expect(p.getAttribute('aria-hidden')).toBe('true')
    expect(p.tabIndex).toBe(-1)
  })

  it('turns into "New message", then "2 new messages", on arrivals while scrolled up (one element)', () => {
    const { box, rerender } = setup()
    scrollTo(box, 300)
    const first = pill()
    arrive(rerender, textItem('t3', 3))
    expect(words()).toBe('New message')
    expect(pill().getAttribute('aria-label')).toBe('New message, jump to bottom')
    expect(pill().hasAttribute('data-accent')).toBe(true)
    arrive(rerender, textItem('t4', 4))
    expect(words()).toBe('2 new messages')
    expect(pill().getAttribute('aria-label')).toBe('2 new messages, jump to bottom')
    // The same button throughout, and only one.
    expect(pill()).toBe(first)
    expect(screen.getAllByTestId('thread-jump-pill')).toHaveLength(1)
    // A send doesn't move the list.
    expect(box.scrollTop).toBe(300)
  })

  it("doesn't count this viewer's own posts, nor arrivals at the bottom", () => {
    const { box, rerender } = setup()
    arrive(rerender, textItem('t3', 3))
    scrollTo(box, 300)
    expect(words()).toBe('Jump to bottom')
    arrive(rerender, textItem('t4', 4, { mine: true }))
    expect(words()).toBe('Jump to bottom')
  })

  it('says "Sent" when the person sends while scrolled up, and the list stays put', () => {
    const { box, rerender } = setup()
    scrollTo(box, 300)
    threadHook.outbox = [{ ...textItem('p1', 99, { mine: true }), pending: { clientMsgId: 'p1', state: 'sending' } } as OverlayThreadItem]
    act(() => rerender())
    expect(words()).toBe('Sent')
    expect(pill().getAttribute('aria-label')).toBe('Sent, jump to bottom')
    expect(pill().hasAttribute('data-accent')).toBe(false)
    expect(box.scrollTop).toBe(300)
    // New messages from others outrank it.
    arrive(rerender, textItem('t3', 3))
    expect(words()).toBe('New message')
  })

  it('animates the width between the measured widths when the words change', () => {
    const { box, rerender } = setup()
    scrollTo(box, 300)
    const clip = pill().querySelector('[data-jump-clip]') as HTMLElement
    expect(clip.style.width).toBe(`${'Jump to bottom'.length * 7 + 36}px`)
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(false)
    arrive(rerender, textItem('t3', 3))
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(true)
    expect(clip.style.width).toBe(`${'New message'.length * 7 + 36}px`)
    act(() => {
      vi.advanceTimersByTime(THREAD_JUMP_PILL_WIDTH_MS + 100)
    })
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(false)
  })

  it('reduced motion: the words swap and the width jumps (no animation class)', () => {
    motion.reduced = true
    const { box, rerender } = setup()
    scrollTo(box, 300)
    arrive(rerender, textItem('t3', 3))
    expect(words()).toBe('New message')
    expect(pill().classList.contains(THREAD_JUMP_PILL_ANIMATING)).toBe(false)
    const clip = pill().querySelector('[data-jump-clip]') as HTMLElement
    expect(clip.style.width).toBe(`${'New message'.length * 7 + 36}px`)
  })

  it('a click scrolls smoothly to the bottom', () => {
    const { box, scrollTo: spy } = setup()
    scrollTo(box, 300)
    fireEvent.click(pill())
    expect(spy).toHaveBeenCalledWith({ top: 1000, behavior: 'smooth' })
  })

  it('reduced motion: a click jumps (no smooth scroll)', () => {
    motion.reduced = true
    const { box, scrollTo: spy } = setup()
    scrollTo(box, 300)
    fireEvent.click(pill())
    expect(spy).toHaveBeenCalledWith({ top: 1000, behavior: 'instant' })
  })

  it('the count resets at the bottom', () => {
    const { box, rerender } = setup()
    scrollTo(box, 300)
    arrive(rerender, textItem('t3', 3), textItem('t4', 4))
    expect(words()).toBe('2 new messages')
    scrollTo(box, 800)
    expect(pill().hasAttribute('data-shown')).toBe(false)
    scrollTo(box, 300)
    expect(words()).toBe('Jump to bottom')
    expect(pill().hasAttribute('data-accent')).toBe(false)
  })

  it('announces "N new messages" politely, once per count change', () => {
    const { box, rerender } = setup()
    const live = screen.getByTestId('thread-jump-live')
    expect(live.getAttribute('aria-live')).toBe('polite')
    // Outside the pill, so a hidden pill never silences it.
    expect(pill().contains(live)).toBe(false)
    const changes = new MutationObserver(() => {})
    changes.observe(live, { childList: true, characterData: true, subtree: true })
    scrollTo(box, 300)
    expect(live.textContent).toBe('')
    expect(changes.takeRecords()).toHaveLength(0)
    arrive(rerender, textItem('t3', 3))
    expect(live.textContent).toBe('1 new message')
    expect(changes.takeRecords().length).toBeGreaterThan(0)
    // Scrolling (still up) says nothing new.
    scrollTo(box, 250)
    scrollTo(box, 320)
    expect(changes.takeRecords()).toHaveLength(0)
    arrive(rerender, textItem('t4', 4))
    expect(live.textContent).toBe('2 new messages')
    expect(changes.takeRecords().length).toBeGreaterThan(0)
    changes.disconnect()
  })
})
