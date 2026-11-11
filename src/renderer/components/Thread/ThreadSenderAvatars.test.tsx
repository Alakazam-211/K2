// @vitest-environment jsdom
// Sender pictures in a Thread (Rosson 2026-11-04, 2026-11-09): only once
// two or more other senders are among the loaded messages, then on while
// that Thread is open; one picture per run; never on your own messages; no
// motion class under reduced motion.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, screen, waitFor } from '@testing-library/react'
import { ThreadOverlayPane } from '@/components/SessionView/ThreadOverlayPane'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import {
  SENDER_GUTTER_PX,
  SENDER_MOTION_CLASS,
  threadSender,
  threadSenderSlots,
  type ThreadSenderRow,
} from './threadSenders'

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
  turn: null,
  turnsReported: true,
  outbox: [] as unknown[],
  currentAddr: '',
  pastAddresses: [] as string[],
}))

vi.mock('@/components/SessionView/useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) => sel({ editor: { fontSize: 13 } }),
}))

/** Mine: the owner's compose post (the daemon's stamp). */
function me(i: number): OverlayThreadItem {
  const id = `m${i}`
  return {
    collection: 'thread',
    seq: i + 1,
    id,
    doc: { id, kind: 'text', from: 'Rosson', via: 'compose', author: { kind: 'owner', login: 'owner' }, created_at: 1_700_000_000 + i, body: `me ${i}` },
  }
}

/** An agent's post. */
function agent(i: number, from = 'sales'): OverlayThreadItem {
  const id = `${from}${i}`
  return { collection: 'thread', seq: i + 1, id, doc: { id, kind: 'text', from, created_at: 1_700_000_000 + i, body: `${from} ${i}` } }
}

/** Another person's post (a Connect login). */
function person(i: number, login: string): OverlayThreadItem {
  const id = `${login}${i}`
  return {
    collection: 'thread',
    seq: i + 1,
    id,
    doc: { id, kind: 'text', from: login, via: 'compose', author: { kind: 'user', login }, created_at: 1_700_000_000 + i, body: `${login} ${i}` },
  }
}

function row(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-zen-message="${id}"]`)
  if (!el) throw new Error(`row ${id} is not drawn`)
  return el
}

function drawnIds(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-message]')).map((el) => el.getAttribute('data-zen-message') ?? '')
}

/** Rows whose sender column is shown. */
function columnsOn(): string[] {
  return Array.from(document.querySelectorAll('[data-sender-column="on"]')).map((el) => el.getAttribute('data-zen-message') ?? '')
}

function stubListBox(el: HTMLElement, box: { scrollHeight: number; clientHeight: number; scrollTop: number }): void {
  Object.defineProperty(el, 'scrollHeight', { configurable: true, get: () => box.scrollHeight })
  Object.defineProperty(el, 'clientHeight', { configurable: true, get: () => box.clientHeight })
  Object.defineProperty(el, 'scrollTop', {
    configurable: true,
    get: () => box.scrollTop,
    set: (v: number) => {
      box.scrollTop = v
    },
  })
}

function list(): HTMLElement {
  return screen.getByTestId('thread-overlay-list')
}

function setReducedMotion(on: boolean): void {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    writable: true,
    value: (query: string) => ({
      matches: on && query.includes('prefers-reduced-motion'),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }),
  })
}

beforeEach(() => {
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  setReducedMotion(false)
})

afterEach(() => {
  cleanup()
  resetThreadViewersForTests()
  threadHook.items = []
  vi.useRealTimers()
})

describe('threadSenderSlots', () => {
  const r = (it: OverlayThreadItem, mine = false): ThreadSenderRow => ({ id: it.id, sender: threadSender(it.doc), mine })
  const opts = { motion: true, host: null }

  it('you and one agent: no column', () => {
    const out = threadSenderSlots([r(me(0), true), r(agent(1)), r(me(2), true), r(agent(3))], opts)
    expect(out.on).toBe(false)
    expect([...out.slots.values()].every((s) => s.on === false)).toBe(true)
  })

  it('a second agent or another person turns it on, for every other row', () => {
    const agents = threadSenderSlots([r(me(0), true), r(agent(1)), r(agent(2, 'ops'))], opts)
    expect(agents.on).toBe(true)
    expect([...agents.slots.keys()]).toEqual(['sales1', 'ops2'])
    expect([...agents.slots.values()].every((s) => s.on)).toBe(true)
    const people = threadSenderSlots([r(me(0), true), r(agent(1)), r(person(2, 'anna'))], opts)
    expect(people.on).toBe(true)
    expect(people.slots.get('anna2')?.kind).toBe('person')
  })

  it('the owner on two clients is one sender, not two', () => {
    const a = threadSender(me(0).doc)
    const b = threadSender({ ...me(1).doc, from: 'rosson-laptop' })
    expect(a.key).toBe(b.key)
  })

  it('a run of one sender draws one picture, on its first row', () => {
    const out = threadSenderSlots(
      [r(agent(0)), r(agent(1)), r(agent(2, 'ops')), r(agent(3, 'ops')), r(me(4), true), r(agent(5, 'ops'))],
      opts,
    )
    const first = [...out.slots.entries()].filter(([, s]) => s.first).map(([id]) => id)
    // `me` ends the ops run: the next ops row starts a new one.
    expect(first).toEqual(['sales0', 'ops2', 'ops5'])
  })

  it('your own rows never get a slot', () => {
    const out = threadSenderSlots([r(me(0), true), r(agent(1)), r(agent(2, 'ops')), r(me(3), true)], opts)
    expect(out.slots.has('m0')).toBe(false)
    expect(out.slots.has('m3')).toBe(false)
  })
})

describe('Thread pane sender pictures', () => {
  it('you plus one agent: no pictures and no column', () => {
    threadHook.items = [me(0), agent(1), me(2), agent(3)]
    renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(drawnIds()).toEqual(['m0', 'sales1', 'm2', 'sales3'])
    expect(columnsOn()).toEqual([])
    expect(row('sales1').getAttribute('data-sender-column')).toBe('off')
    expect(row('sales1').style.marginLeft).toBe('0px')
    expect(document.querySelectorAll('[data-sender-column="on"] [data-sender-avatar]').length).toBe(0)
  })

  it('a third sender: every other row gets the column, one picture per run, yours none', async () => {
    threadHook.items = [me(0), agent(1), agent(2), agent(3, 'ops'), me(4), person(5, 'anna')]
    renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(columnsOn()).toEqual(['sales1', 'sales2', 'ops3', 'anna5'])
    for (const id of columnsOn()) expect(row(id).style.marginLeft).toBe(`${SENDER_GUTTER_PX}px`)
    // One picture per run: sales1 draws it, sales2 leaves the space empty.
    expect(row('sales1').querySelector('[data-sender-avatar="agent"]')).not.toBeNull()
    expect(row('sales2').querySelector('[data-sender-avatar]')).toBeNull()
    expect(row('ops3').querySelector('[data-sender-avatar="agent"]')).not.toBeNull()
    expect(row('anna5').querySelector('[data-sender-avatar="person"]')).not.toBeNull()
    // The pictures K2 paints elsewhere (loaded on first use): a person's
    // chip, an agent's letter tile (no image for it in these stores).
    await waitFor(() => expect(row('anna5').querySelector('[data-sender-avatar] [data-presence-chip]')).not.toBeNull())
    expect(row('sales1').querySelector('[data-sender-avatar]')?.textContent).toBe('S')
    expect(row('ops3').querySelector('[data-sender-avatar]')?.textContent).toBe('O')
    // Yours: no column, no picture, no nudge.
    for (const id of ['m0', 'm4']) {
      expect(row(id).hasAttribute('data-sender-column')).toBe(false)
      expect(row(id).querySelector('[data-sender-avatar]')).toBeNull()
      expect(row(id).style.marginLeft).toBe('')
    }
  })

  it('once on, the pictures stay on while that Thread is open (the third sender rolling off the window)', () => {
    vi.useFakeTimers({ now: 1_700_000_000_000 })
    // ops posts once, at the start of the newest 30.
    const first = Array.from({ length: 30 }, (_, i) => (i === 0 ? agent(i, 'ops') : i % 2 ? me(i) : agent(i)))
    threadHook.items = first
    const view = renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    stubListBox(list(), { scrollHeight: 2000, clientHeight: 400, scrollTop: 1600 })
    expect(drawnIds()).toHaveLength(30)
    expect(columnsOn()).toContain('ops0')
    // Ten more arrive while following the newest: once scrolling is quiet
    // the oldest roll off, ops with them. ops is still loaded: still on.
    threadHook.items = [...first, ...Array.from({ length: 10 }, (_, j) => (j % 2 ? me(30 + j) : agent(30 + j)))]
    // (A new prop re-renders the memoized pane, as the hook's state would.)
    view.rerender(<ThreadOverlayPane addr="sales" conversationId="c" agentName="Sales" />)
    act(() => {
      vi.advanceTimersByTime(1_000)
    })
    expect(drawnIds()).toHaveLength(30)
    expect(drawnIds()).not.toContain('ops0')
    expect(row('sales30').getAttribute('data-sender-column')).toBe('on')
    // Even a re-read that no longer holds ops keeps them on: nothing jumps.
    threadHook.items = Array.from({ length: 10 }, (_, j) => (j % 2 ? me(40 + j) : agent(40 + j)))
    view.rerender(<ThreadOverlayPane addr="sales" conversationId="c" agentName="Sales." />)
    expect(row('sales40').getAttribute('data-sender-column')).toBe('on')
    // Another Thread starts over.
    view.rerender(<ThreadOverlayPane addr="ops" conversationId="c2" agentName="Ops" />)
    expect(columnsOn()).toEqual([])
    expect(row('sales40').getAttribute('data-sender-column')).toBe('off')
  })

  it('counts every loaded message, not just the drawn rows: a third sender above the window turns them on', () => {
    // 100 in memory; the newest 30 are only you and sales; ops is at 55.
    threadHook.items = Array.from({ length: 100 }, (_, i) => (i === 55 ? agent(i, 'ops') : i % 2 ? me(i) : agent(i)))
    renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(drawnIds()[0]).toBe('sales70')
    expect(drawnIds()).not.toContain('ops55')
    expect(columnsOn()).toContain('sales70')
    expect(row('sales70').querySelector('[data-sender-avatar="agent"]')).not.toBeNull()
    // Your sends in progress are yours: they never count as a sender.
  })

  it('your own posts from another device are you, not a second sender', () => {
    const other: OverlayThreadItem = { ...me(2), id: 'm2b', doc: { ...me(2).doc, id: 'm2b', from: 'rosson-phone' } }
    threadHook.items = [me(0), agent(1), other, agent(3)]
    renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(columnsOn()).toEqual([])
  })

  it('moves by CSS transition; reduced motion: no transition class', () => {
    threadHook.items = [me(0), agent(1), agent(2, 'ops')]
    const view = renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(row('sales1').classList.contains(SENDER_MOTION_CLASS)).toBe(true)
    view.unmount()
    setReducedMotion(true)
    renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(columnsOn()).toEqual(['sales1', 'ops2'])
    expect(row('sales1').classList.contains(SENDER_MOTION_CLASS)).toBe(false)
    expect(row('ops2').classList.contains(SENDER_MOTION_CLASS)).toBe(false)
  })
})
