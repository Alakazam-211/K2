// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { screen, fireEvent, cleanup, act } from '@testing-library/react'
import type { OverlayThreadItem } from './overlayThread'

const chatterHook = vi.hoisted(() => ({
  items: [] as OverlayThreadItem[],
  conversationId: 'c',
  error: null as string | null,
  hasMore: false,
  loadingOlder: false,
  loading: false,
  loadOlder: async () => {},
  loadAll: async () => {},
  /** The mounted panes: `push()` re-reads, as a live frame does. */
  listeners: new Set<() => void>(),
}))

vi.mock('./useOverlayChatter', async () => {
  const React = await import('react')
  return {
    useOverlayChatter: () => {
      const [, force] = React.useReducer((n: number) => n + 1, 0)
      React.useEffect(() => {
        chatterHook.listeners.add(force)
        return () => {
          chatterHook.listeners.delete(force)
        }
      }, [])
      return { ...chatterHook }
    },
  }
})

/** The hook's state changed (a page, a live frame): the pane re-reads it. */
function push(): void {
  act(() => {
    for (const l of chatterHook.listeners) l()
  })
}

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) =>
    sel({ editor: { fontSize: 13 } }),
}))

import { ChatterOverlayPane, chatterPaneRuns, chatterRow } from './ChatterOverlayPane'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope as overlayPrimaryScope } from '@/kessel/server-scope'

// Home M4: room components read their scope from their room.
const overlayRoom = testRoom({ tabs: {}, scope: overlayPrimaryScope() })

const T0 = 1_700_000_000
function msg(seq: number, from: string, to: string, body: string, at = T0 + seq * 10): OverlayThreadItem {
  const id = `c${seq}`
  return { collection: 'chatter', seq, id, doc: { id, kind: 'chatter', from, to, body, via: 'msg', created_at: at } }
}

/** The drawn rows, oldest first. */
function rows(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-testid="chatter-overlay-column"] > [data-zen-message]'))
}
const row = (id: string): HTMLElement => {
  const el = document.querySelector<HTMLElement>(`[data-zen-message="${id}"]`)
  if (!el) throw new Error(`no row ${id}`)
  return el
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

describe('Chatter overlay pane', () => {
  afterEach(() => {
    cleanup()
    chatterHook.items = []
    chatterHook.error = null
    chatterHook.hasMore = false
    chatterHook.loadingOlder = false
    chatterHook.loading = false
    chatterHook.loadOlder = async () => {}
  })

  it('has no compose / send / textarea', () => {
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByTestId('chatter-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
    expect(screen.queryByTestId('chatter-compose')).toBeNull()
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.queryByRole('button', { name: /send/i })).toBeNull()
    expect(document.querySelector('textarea')).toBeNull()
  })

  it('wears the Thread look: a desktop Thread box with the kit rules', () => {
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    const pane = screen.getByTestId('chatter-overlay-pane')
    expect(pane.hasAttribute('data-k2-thread-zen')).toBe(true)
    expect(pane.querySelector('style[data-thread-zen-styles]')).not.toBeNull()
  })

  it('empty, loading and error each say one quiet line', () => {
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByTestId('chatter-overlay-empty').textContent).toBe('No agent-to-agent messages yet.')
    expect(screen.queryByText(/Message the agent below/i)).toBeNull()
    chatterHook.loading = true
    push()
    expect(screen.getByTestId('chatter-overlay-loading').textContent).toBe('Loading chatter…')
    expect(screen.queryByTestId('chatter-overlay-empty')).toBeNull()
    chatterHook.loading = false
    chatterHook.error = 'daemon not reachable'
    push()
    expect(screen.getByTestId('chatter-overlay-error').textContent).toBe('daemon not reachable')
  })

  it('sides: this agent on the right, other agents on the left with their name over each run', () => {
    chatterHook.items = [
      msg(1, 'ops', 'sales', 'export failed'),
      msg(2, 'ops', 'sales', 'did the retry run?'),
      msg(3, 'sales', 'ops', '**Looking** now'),
      msg(4, 'sales/reviewer', 'sales', 'review notes'),
    ]
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(rows().map((r) => r.getAttribute('data-zen-message'))).toEqual(['c1', 'c2', 'c3', 'c4'])
    expect(row('c3').hasAttribute('data-mine')).toBe(true)
    expect(row('c3').querySelector('[data-zen-bubble="me"]')).not.toBeNull()
    for (const id of ['c1', 'c2', 'c4']) {
      expect(row(id).hasAttribute('data-mine')).toBe(false)
      expect(row(id).querySelector('[data-zen-bubble="agent"]')).not.toBeNull()
    }
    // ops' two messages are one run: the name once, over the first.
    expect(row('c1').getAttribute('data-run')).toBe('first')
    expect(row('c2').getAttribute('data-run')).toBe('last')
    expect(row('c1').querySelector('[data-thread-author]')?.textContent).toBe('ops')
    expect(row('c2').querySelector('[data-thread-author]')).toBeNull()
    expect(row('c4').querySelector('[data-thread-author]')?.textContent).toBe('sales/reviewer')
    // The body is the Thread's markdown.
    expect(screen.getByText('Looking').tagName).toBe('STRONG')
    // Two other agents: their pictures' column is on.
    expect(row('c1').getAttribute('data-sender-column')).toBe('on')
    expect(row('c3').hasAttribute('data-sender-column')).toBe(false)
  })

  it('one other agent: no picture column', () => {
    chatterHook.items = [msg(1, 'ops', 'sales', 'hi'), msg(2, 'sales', 'ops', 'hey')]
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(row('c1').getAttribute('data-sender-column')).not.toBe('on')
  })

  it('recipients: each run of yours says who it went to, under its last bubble', () => {
    chatterHook.items = [
      msg(1, 'sales', 'ops', 'first to ops'),
      msg(2, 'sales', 'ops', 'second to ops'),
      msg(3, 'sales', 'sales/reviewer', 'to the reviewer'),
    ]
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    // A new recipient starts a new run.
    expect(row('c1').getAttribute('data-run')).toBe('first')
    expect(row('c2').getAttribute('data-run')).toBe('last')
    expect(row('c3').getAttribute('data-run')).toBe('solo')
    expect(row('c1').querySelector('[data-thread-meta="status"]')).toBeNull()
    const ops = row('c2').querySelector('[data-thread-meta="status"]')
    expect(ops?.querySelector('[data-thread-recipient]')?.textContent).toBe('to ops')
    expect(ops?.textContent).toMatch(/^to ops · /)
    expect(row('c3').querySelector('[data-thread-recipient]')?.textContent).toBe('to sales/reviewer')
    // No Thread receipt on chatter.
    expect(document.querySelector('[data-thread-receipt]')).toBeNull()
  })

  it('Load older fetches the next page', () => {
    const loadOlder = vi.fn(async () => {})
    chatterHook.items = [msg(1, 'ops', 'sales', 'hi')]
    chatterHook.hasMore = true
    chatterHook.loadOlder = loadOlder
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    stubListBox(screen.getByTestId('chatter-overlay-list'), { scrollHeight: 800, clientHeight: 200, scrollTop: 40 })
    fireEvent.click(screen.getByTestId('overlay-load-older'))
    expect(loadOlder).toHaveBeenCalledTimes(1)
    expect(screen.getByTestId('overlay-load-older').textContent).toBe('Earlier messages')
  })

  it('no Load older when the mailbox is all loaded', () => {
    chatterHook.items = [msg(1, 'ops', 'sales', 'hi')]
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(screen.queryByTestId('overlay-load-older')).toBeNull()
  })

  it('live: a new message draws at once and joins its run', () => {
    chatterHook.items = [msg(1, 'ops', 'sales', 'deploy in 10')]
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" />)
    expect(row('c1').getAttribute('data-run')).toBe('solo')
    chatterHook.items = [...chatterHook.items, msg(2, 'ops', 'sales', 'freeze on main')]
    push()
    expect(rows()).toHaveLength(2)
    expect(screen.getByText('freeze on main')).not.toBeNull()
    expect(row('c1').getAttribute('data-run')).toBe('first')
    expect(row('c2').getAttribute('data-run')).toBe('last')
  })

  it('caps and centres the column at the Thread text width', () => {
    chatterHook.items = [msg(1, 'ops', 'sales', 'hi')]
    renderInRoom(overlayRoom, <ChatterOverlayPane addr="sales" conversationId="c" textCapPx={820} />)
    const col = screen.getByTestId('chatter-overlay-column')
    expect(col.style.maxWidth).toBe('820px')
    expect(col.style.marginInline).toBe('auto')
  })
})

describe('chatterRow / chatterPaneRuns', () => {
  it('mine is this agent, by address (case and spaces folded)', () => {
    expect(chatterRow(msg(1, ' Sales ', 'ops', 'x'), 'sales').mine).toBe(true)
    expect(chatterRow(msg(1, 'sales/reviewer', 'sales', 'x'), 'sales').mine).toBe(false)
  })

  it('names the recipient only on yours, and an incoming doc not to us says where it went', () => {
    const out = chatterRow(msg(1, 'sales', 'ops', 'x'), 'sales')
    expect(out.to).toBe('ops')
    const inbound = chatterRow(msg(2, 'ops', 'sales', 'x'), 'sales')
    expect(inbound.to).toBeNull()
    expect(inbound.author).toBe('ops')
    expect(chatterRow(msg(3, 'ops', 'billing', 'x'), 'sales').author).toBe('ops → billing')
  })

  it('runs count the rows outside the window, and break on a long gap', () => {
    const all = [
      msg(1, 'ops', 'sales', 'a'),
      msg(2, 'ops', 'sales', 'b'),
      msg(3, 'ops', 'sales', 'c', T0 + 10_000),
    ].map((it) => chatterRow(it, 'sales'))
    const r = chatterPaneRuns(all, { lo: 1, hi: 3, motion: false, host: null })
    // c2 continues c1 (outside the window), and ends before the gap.
    expect(r.starts.has('c2')).toBe(false)
    expect(r.tails.has('c2')).toBe(true)
    expect(r.starts.has('c3')).toBe(true)
  })
})
