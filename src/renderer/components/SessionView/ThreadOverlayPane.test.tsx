// @vitest-environment jsdom
import { describe, expect, it, afterEach, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, cleanup, act } from '@testing-library/react'
import { createElement, useState, type ReactNode } from 'react'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import {
  choiceLetter,
  ThreadItemRow,
  ThreadOverlayPane,
} from './ThreadOverlayPane'
import { overlayViewer } from './sessionViewTab'
import { coerceThreadTurn, type OverlayThreadItem, type ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope as overlayPrimaryScope } from '@/kessel/server-scope'

// Home M4: room components read their scope from their room.
const overlayRoom = testRoom({ tabs: {}, scope: overlayPrimaryScope() })

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
  turn: null as ThreadTurn | null,
  turnsReported: true,
}))

vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => threadHook,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { editor: { fontSize: number } }) => unknown) =>
    sel({ editor: { fontSize: 13 } }),
}))

describe('Thread overlay pane', () => {
  afterEach(() => {
    cleanup()
    threadHook.items = []
    threadHook.error = null
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
    threadHook.loaded = true
  })

  it('has no compose box — Message-the-agent stays on the terminal bar', () => {
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByTestId('thread-overlay-pane')).not.toBeNull()
    expect(screen.queryByTestId('thread-compose')).toBeNull()
  })

  it('overlay root is a flex-1 min-h-0 overflow-hidden column (not height 100%)', () => {
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    const pane = screen.getByTestId('thread-overlay-pane')
    expect(pane.className).toContain('flex-1')
    expect(pane.className).toContain('min-h-0')
    expect(pane.className).toContain('overflow-hidden')
    expect(pane.className.split(/\s+/)).not.toContain('h-full')
  })

  it('thread pane is a selectable region (copy-paste)', () => {
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    const pane = screen.getByTestId('thread-overlay-pane')
    expect(pane.className).toContain('selectable-copy')
    expect(pane.className).toContain('chat-thread-selectable')
  })

  it('shows Load older when hasMore and click calls loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    const list = overlayList()
    stubListBox(list, { scrollHeight: 800, clientHeight: 200, scrollTop: 40 })
    fireEvent.click(screen.getByTestId('overlay-load-older'))
    expect(loadOlder).toHaveBeenCalledTimes(1)
  })

  it('hides Load older when hasMore is false', () => {
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.queryByTestId('overlay-load-older')).toBeNull()
  })

  it('empty list shows the centered title and names the agent', () => {
    renderInRoom(
      overlayRoom,
      <ThreadOverlayPane addr="sales" conversationId="c" agentName="Sales Bot" />,
    )
    const empty = screen.getByTestId('thread-overlay-empty')
    expect(empty.className).toContain('items-center')
    expect(empty.className).toContain('justify-center')
    expect(screen.getByText('No Thread messages yet')).not.toBeNull()
    expect(empty.textContent).toContain('Send a message to Sales Bot in the box below.')
    expect(empty.textContent).not.toContain('the agent')
  })

  it('empty list with no name falls back to "the agent"', () => {
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    const empty = screen.getByTestId('thread-overlay-empty')
    expect(empty.textContent).toContain('Send a message to the agent in the box below.')
  })

  it('blank name also falls back to "the agent"', () => {
    renderInRoom(
      overlayRoom,
      <ThreadOverlayPane addr="sales" conversationId="c" agentName="   " />,
    )
    const empty = screen.getByTestId('thread-overlay-empty')
    expect(empty.textContent).toContain('Send a message to the agent in the box below.')
  })

  it('shows nothing before the first load finishes', () => {
    threadHook.loaded = false
    renderInRoom(
      overlayRoom,
      <ThreadOverlayPane addr="sales" conversationId="c" agentName="Sales Bot" />,
    )
    expect(screen.queryByTestId('thread-overlay-empty')).toBeNull()
    expect(screen.queryByText('No Thread messages yet')).toBeNull()
  })

  it('error replaces the empty state', () => {
    threadHook.error = 'boom'
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    expect(screen.getByText('boom')).not.toBeNull()
    expect(screen.queryByTestId('thread-overlay-empty')).toBeNull()
  })
})

describe('Thread overlay choice chips + secret field', () => {
  afterEach(() => cleanup())

  it('letters options A, B, … then AA', () => {
    expect(choiceLetter(0)).toBe('A')
    expect(choiceLetter(1)).toBe('B')
    expect(choiceLetter(25)).toBe('Z')
    expect(choiceLetter(26)).toBe('AA')
  })

  it('renders markdown in a project-chat style message', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 't1',
      doc: {
        id: 't1',
        kind: 'text',
        from: 'owner',
        via: 'compose',
        created_at: Math.floor(Date.now() / 1000),
        body: '**Hello** and `code`',
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} />)
    expect(screen.getByText('You')).not.toBeNull()
    expect(screen.getByText('Hello').tagName).toBe('STRONG')
    expect(screen.getByText('code').tagName).toBe('CODE')
  })

  it('names the Garden widget that sent a post for you (UWB12a)', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 3,
      id: 't-widget',
      doc: {
        id: 't-widget',
        kind: 'text',
        from: 'alice',
        via: 'compose',
        created_at: Math.floor(Date.now() / 1000),
        body: 'hello from the arcade',
        widget: { widget: 'Agent Arcade', garden: 'g-test0001' },
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} />)
    expect(screen.getByText('You · via Agent Arcade')).not.toBeNull()
    expect(screen.queryByText('You')).toBeNull()
  })

  it('ignores a widget origin on a post that is not yours', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 't-agent-widget',
      doc: {
        id: 't-agent-widget',
        kind: 'text',
        from: 'cortana',
        via: 'thread',
        created_at: Math.floor(Date.now() / 1000),
        body: 'agent reply',
        widget: { widget: 'Agent Arcade', garden: 'g-test0001' },
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} />)
    expect(screen.getByText('cortana')).not.toBeNull()
    expect(screen.queryByText(/via Agent Arcade/)).toBeNull()
  })

  it('does not paint via=thread from=owner as You', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 2,
      id: 't-owner-thread',
      doc: {
        id: 't-owner-thread',
        kind: 'text',
        from: 'owner',
        via: 'thread',
        created_at: Math.floor(Date.now() / 1000),
        body: 'agent said hello',
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} />)
    expect(screen.queryByText('You')).toBeNull()
    expect(screen.getByText('owner')).not.toBeNull()
    expect(screen.getByText('agent said hello')).not.toBeNull()
  })

  it('renders a vertical lettered choice card; first option is primary; tap calls onAnswer', () => {
    const picks: string[] = []
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'c1',
      doc: {
        id: 'c1',
        kind: 'choice',
        from: 'k2',
        body: 'Ship it?',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'pending',
        },
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} onAnswer={(p) => picks.push(p.answer || '')} />)
    const card = screen.getByTestId('thread-choice-card')
    expect(card.className).toContain('flex-col')
    expect(card.className).toContain('w-full')
    expect(card.className).not.toContain('max-w-sm')
    expect(card.className).toMatch(/\bpx-2\b/)
    const chips = screen.getAllByTestId('thread-choice-chip')
    expect(chips).toHaveLength(2)
    expect(chips[0].getAttribute('data-letter')).toBe('A')
    expect(chips[1].getAttribute('data-letter')).toBe('B')
    expect(chips[0].textContent).toContain('A')
    expect(chips[0].textContent).toContain('Go')
    expect(chips[0].getAttribute('data-primary')).toBe('true')
    expect(chips[1].getAttribute('data-primary')).toBe('false')
    fireEvent.click(chips[0])
    expect(picks).toEqual(['Go'])
  })

  it('answered choice stays visible with the selected option highlighted', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'c1',
      doc: {
        id: 'c1',
        kind: 'choice',
        from: 'k2',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'answered',
          answer: 'Go',
        },
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} />)
    expect(screen.getByTestId('thread-choice-card')).not.toBeNull()
    const go = screen.getAllByTestId('thread-choice-chip')[0]
    expect(go.getAttribute('disabled')).not.toBeNull()
    expect(go.getAttribute('data-letter')).toBe('A')
    expect(go.className).toContain('border-[var(--thread-card-accent,var(--color-accent))]')
    expect(go.className).toContain('bg-[var(--thread-card-accent,var(--color-accent))]/15')
  })

  it('renders a secret field and submit/dismiss; never shows the value as body text', () => {
    const submitted: string[] = []
    let dismissed = 0
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 2,
      id: 's1',
      doc: {
        id: 's1',
        kind: 'secret',
        from: 'k2',
        secret: { name: 'API_TOKEN', status: 'pending', prompt: 'Paste the Grok token' },
      },
    }
    renderInRoom(overlayRoom, 
      <ThreadItemRow
        item={item}
        onAnswer={(p) => submitted.push(p.secret || '')}
        onVoid={() => {
          dismissed += 1
        }}
      />,
    )
    const field = screen.getByTestId('thread-secret-field') as HTMLInputElement
    expect(field.type).toBe('password')
    fireEvent.change(field, { target: { value: 's3cr3t-bytes' } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect(submitted).toEqual(['s3cr3t-bytes'])
    expect(screen.queryByText('s3cr3t-bytes')).toBeNull()
    fireEvent.click(screen.getByTestId('thread-secret-dismiss'))
    expect(dismissed).toBe(1)
  })
})

type ListBox = {
  scrollHeight: number
  clientHeight: number
  scrollTop: number
}

function textItem(id: string, seq: number): OverlayThreadItem {
  return {
    collection: 'thread',
    seq,
    id,
    doc: {
      id,
      kind: 'text',
      from: 'k2',
      body: `m${seq}`,
      created_at: 1,
    },
  }
}

function overlayList(): HTMLElement {
  const pane = screen.getByTestId('thread-overlay-pane')
  const list =
    pane.querySelector('[data-testid="thread-overlay-list"]') ??
    pane.querySelector('.overflow-y-auto')
  if (!(list instanceof HTMLElement)) throw new Error('missing thread overlay list')
  return list
}

function stubListBox(el: HTMLElement, box: ListBox): void {
  Object.defineProperty(el, 'scrollHeight', {
    configurable: true,
    get: () => box.scrollHeight,
  })
  Object.defineProperty(el, 'clientHeight', {
    configurable: true,
    get: () => box.clientHeight,
  })
  Object.defineProperty(el, 'scrollTop', {
    configurable: true,
    get: () => box.scrollTop,
    set: (v: number) => {
      box.scrollTop = v
    },
  })
}

class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  private readonly cb: ResizeObserverCallback
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb
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
    this.cb(
      this.observed.map(
        (target) =>
          ({
            target,
            contentRect: { width: 0, height: 0 },
          }) as ResizeObserverEntry,
      ),
      this as unknown as ResizeObserver,
    )
  }
}

function observerOf(el: Element): FakeResizeObserver {
  const found = FakeResizeObserver.instances.find((o) => o.observed.includes(el))
  if (!found) throw new Error('no ResizeObserver is observing the list')
  return found
}

function renderOverlay(visible = true) {
  return renderInRoom(overlayRoom, 
    createElement(
      TabVisibilityContext.Provider,
      { value: visible },
      createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
    ),
  )
}

function VisibilityHarness({
  visible,
  children,
}: {
  visible: boolean
  children: ReactNode
}) {
  return createElement(TabVisibilityContext.Provider, { value: visible }, children)
}

describe('Thread overlay list scroll restore', () => {
  beforeEach(() => {
    FakeResizeObserver.instances = []
    vi.stubGlobal('ResizeObserver', FakeResizeObserver)
    threadHook.items = [textItem('t1', 1), textItem('t2', 2)]
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
  })

  afterEach(() => {
    cleanup()
    threadHook.items = []
    threadHook.error = null
    threadHook.hasMore = false
    threadHook.loadingOlder = false
    threadHook.loadOlder = async () => {}
    vi.unstubAllGlobals()
  })

  it('(a) items + real height pin to bottom', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(b) 0-height then ResizeObserver to real height pins if pinned', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 0, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(0)

    box.clientHeight = 200
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(c) user scrolled up stays put on resize and new items', () => {
    const view = renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    box.scrollTop = 120
    fireEvent.scroll(list)
    expect(box.scrollTop).toBe(120)

    box.clientHeight = 140
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(120)

    threadHook.items = [...threadHook.items, textItem('t3', 3)]
    view.rerender(
      createElement(
        TabVisibilityContext.Provider,
        { value: true },
        createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
      ),
    )
    expect(box.scrollTop).toBe(120)
  })

  it('(d) scrollTop === 0 at clientHeight === 0 does not call loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 0, scrollTop: 0 }
    stubListBox(list, box)
    fireEvent.scroll(list)
    expect(loadOlder).not.toHaveBeenCalled()
  })

  it('(e) split and thread-only both overlayViewer.thread', () => {
    expect(overlayViewer('thread').thread).toBe(true)
    expect(overlayViewer('split').thread).toBe(true)
    expect(overlayViewer('terminal').thread).toBe(false)
    expect(overlayViewer('chatter').thread).toBe(false)
  })

  it('(f) list clientHeight shrinks while pinned stays at bottom', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    box.clientHeight = 80
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(f2) compose-grow scroll event before ResizeObserver does not unpin', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    // Browser keeps old scrollTop; new clientHeight is no longer at bottom.
    box.clientHeight = 80
    box.scrollTop = 600
    fireEvent.scroll(list)
    expect(box.scrollTop).toBe(box.scrollHeight)

    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('(g) 0-height onScroll does not clear pinBottomRef', () => {
    renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    box.clientHeight = 0
    box.scrollTop = 0
    fireEvent.scroll(list)

    box.clientHeight = 200
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('visibility false→true re-pins when still pinned', () => {
    function Flip() {
      const [visible, setVisible] = useState(false)
      return createElement(
        'div',
        null,
        createElement(VisibilityHarness, {
          visible,
          children: createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
        }),
        createElement('button', {
          type: 'button',
          'data-testid': 'flip-visible',
          onClick: () => setVisible(true),
        }),
      )
    }
    renderInRoom(overlayRoom, createElement(Flip))
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    expect(box.scrollTop).toBe(0)

    fireEvent.click(screen.getByTestId('flip-visible'))
    expect(box.scrollTop).toBe(box.scrollHeight)
  })

  it('visibility false→true does not steal a scrolled-up list', () => {
    function Flip() {
      const [visible, setVisible] = useState(true)
      return createElement(
        'div',
        null,
        createElement(VisibilityHarness, {
          visible,
          children: createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c' }),
        }),
        createElement('button', {
          type: 'button',
          'data-testid': 'hide-tab',
          onClick: () => setVisible(false),
        }),
        createElement('button', {
          type: 'button',
          'data-testid': 'show-tab',
          onClick: () => setVisible(true),
        }),
      )
    }
    renderInRoom(overlayRoom, createElement(Flip))
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    box.scrollTop = 90
    fireEvent.scroll(list)

    fireEvent.click(screen.getByTestId('hide-tab'))
    box.scrollTop = 0
    box.clientHeight = 0
    fireEvent.scroll(list)
    box.clientHeight = 200
    fireEvent.click(screen.getByTestId('show-tab'))
    expect(box.scrollTop).toBe(90)
  })

  it('hidden tab onScroll does not call loadOlder', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    renderOverlay(false)
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    fireEvent.scroll(list)
    expect(loadOlder).not.toHaveBeenCalled()
  })
})

// prd-daemon-activity-and-thread-working-v1 S7 (T-S7a, T-S7b): the working
// strip under the last message, drawn from the hook's `turn`.
describe('Thread working strip (S7)', () => {
  /** This client's clock; the server's runs 2 s ahead of it. */
  const CLIENT_NOW = 1_000_070_000
  const SERVER_NOW = CLIENT_NOW + 2_000
  const STARTED = SERVER_NOW - 72_000

  /** A turn as one §7.6 frame received at CLIENT_NOW makes it. */
  function turnOf(over: Record<string, unknown> = {}): ThreadTurn {
    const t = coerceThreadTurn(
      {
        turnId: 'turn-1',
        state: 'working',
        phase: 'tool',
        phaseSince: SERVER_NOW - 12_000,
        line: 'Running `cargo test`',
        startedAt: STARTED,
        since: STARTED,
        subagents: 2,
        subagentsDone: 1,
        background: 1,
        tally: { read: 3, search: 0, cmd: 2, edit: 0 },
        end: null,
        serverNow: SERVER_NOW,
        rev: 3,
        ...over,
      },
      CLIENT_NOW,
    )
    if (!t) throw new Error('fixture is not a turn')
    return t
  }

  // The pane is memoized and the mocked hook is one mutable object: a fresh
  // prop per render stands in for the hook's state change.
  let renders = 0
  function pane(props: { onStop?: () => void } = {}) {
    renders += 1
    return createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c', agentName: `k2-${renders}`, ...props })
  }

  const strip = () => screen.queryByTestId('thread-working-strip')
  const text = (id: string) => {
    const el = screen.getByTestId(id)
    return (el.textContent ?? '').replace(/\s+/g, ' ').trim()
  }

  beforeEach(() => {
    vi.useFakeTimers({ now: CLIENT_NOW })
    threadHook.items = [textItem('t1', 1)]
  })

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
    threadHook.items = []
    threadHook.turn = null
    threadHook.turnsReported = true
  })

  it('T-S7a: a tool frame reads the pulse, time since your message on the server clock, the line, the tally', () => {
    threadHook.turn = turnOf()
    renderInRoom(overlayRoom, pane())
    expect(strip()?.getAttribute('data-tone')).toBe('working')
    // The step line says what it's doing: no state word in the header.
    expect(screen.queryByTestId('thread-strip-state')).toBeNull()
    // 72 s on the server's clock; this client's own clock would say 70 s.
    expect(text('thread-strip-since')).toBe('1m 12s')
    expect(screen.getByTestId('thread-strip-since').getAttribute('title')).toBe('since your message')
    expect(strip()?.getAttribute('aria-label')).toBe('Agent working, 1m 12s since your message: Running cargo test')
    expect(text('thread-strip-step-text')).toBe('Running cargo test')
    expect(screen.getByTestId('thread-strip-step-text').querySelector('code')?.textContent).toBe('cargo test')
    expect(text('thread-strip-step')).toContain('12s')
    expect(text('thread-strip-tally')).toBe('Read 3 files · ran 2 commands · 2 subagents running, 1 done · 1 background task')
    // "working" is said at most once, and never "subagents working".
    expect((strip()?.textContent ?? '').match(/working/gi)).toBeNull()
    // The strip sits under the last message.
    const content = screen.getByTestId('thread-item').parentElement
    expect(content?.lastElementChild).toBe(strip())
  })

  it('T-S7a: the clock counts on locally at 1 Hz without a new frame', () => {
    threadHook.turn = turnOf()
    renderInRoom(overlayRoom, pane())
    act(() => {
      vi.advanceTimersByTime(8_000)
    })
    expect(text('thread-strip-since')).toBe('1m 20s')
    expect(text('thread-strip-step')).toContain('20s')
  })

  it('T-S7a: thinking reads "Thinking… 8s" and shimmers', () => {
    threadHook.turn = turnOf({ phase: 'thinking', line: null, phaseSince: SERVER_NOW - 8_000 })
    renderInRoom(overlayRoom, pane())
    expect(text('thread-strip-step-text')).toBe('Thinking… 8s')
    expect(screen.getByTestId('thread-strip-step-text').className).toContain('thread-strip-shimmer')
    expect(screen.queryByTestId('thread-strip-state')).toBeNull()
  })

  it('T-S7a: delivering reads its own line; an unknown step says "Working" once, in the header', () => {
    threadHook.turn = turnOf({ phase: 'delivering', line: null, subagents: 0, subagentsDone: 0, background: 0, tally: {} })
    const view = renderInRoom(overlayRoom, pane())
    expect(text('thread-strip-step-text')).toBe('Delivering…')
    expect(screen.queryByTestId('thread-strip-state')).toBeNull()
    expect(strip()?.textContent).not.toMatch(/working/i)
    expect(screen.queryByTestId('thread-strip-tally')).toBeNull()
    for (const [phase, rev] of [['working', 4], ['tool', 5]] as const) {
      threadHook.turn = turnOf({ phase, line: null, rev })
      view.rerender(pane())
      expect(text('thread-strip-state'), phase).toBe('Working')
      expect(text('thread-strip-since'), phase).toBe('· 1m 12s')
      expect(screen.queryByTestId('thread-strip-step'), phase).toBeNull()
      expect((strip()?.textContent ?? '').match(/working/gi), phase).toHaveLength(1)
      expect(strip()?.getAttribute('aria-label'), phase).toBe('Agent working, 1m 12s since your message')
    }
  })

  it('T-S7a: an end removes the strip (no stored row, no trace; Q8)', () => {
    threadHook.turn = turnOf()
    const view = renderInRoom(overlayRoom, pane())
    expect(strip()).not.toBeNull()
    for (const reason of ['reply', 'done', 'session_gone', 'superseded']) {
      threadHook.turn = turnOf({ state: reason === 'reply' || reason === 'done' ? 'idle' : 'stopped', rev: 9, end: { reason, detail: null, at: SERVER_NOW } })
      view.rerender(pane())
      expect(strip(), reason).toBeNull()
    }
    // The message keeps its own row; nothing was added.
    expect(screen.getAllByTestId('thread-item')).toHaveLength(1)
  })

  it('an interrupt reads "Stopped" (muted) for a moment, then goes', () => {
    threadHook.turn = turnOf({ state: 'stopped', rev: 9, end: { reason: 'interrupted', detail: null, at: SERVER_NOW } })
    renderInRoom(overlayRoom, pane({ onStop: () => {} }))
    expect(strip()?.getAttribute('data-tone')).toBe('stopped')
    expect(text('thread-strip-state')).toBe('Stopped')
    expect(text('thread-strip-since')).toBe('· you stopped it after 1m 12s')
    expect(screen.queryByTestId('thread-strip-stop')).toBeNull()
    expect(screen.queryByTestId('thread-strip-tally')).toBeNull()
    act(() => {
      vi.advanceTimersByTime(3_100)
    })
    expect(strip()).toBeNull()
  })

  it('needs-you is amber with no Stop; monitoring is blue', () => {
    threadHook.turn = turnOf({ state: 'needs-you', phase: 'waiting', line: null })
    const view = renderInRoom(overlayRoom, pane({ onStop: () => {} }))
    expect(strip()?.getAttribute('data-tone')).toBe('needs-you')
    expect(strip()?.className).toContain('--color-status-warn-amber')
    expect(text('thread-strip-state')).toBe('Needs you')
    expect(text('thread-strip-since')).toBe('· 1m 12s')
    // A fact, with no call to action and no pointer to the terminal.
    expect(text('thread-strip-step-text')).toBe('Stuck on a permission prompt')
    expect(strip()?.textContent).not.toMatch(/terminal|approve/i)
    expect(screen.queryByTestId('thread-strip-stop')).toBeNull()
    threadHook.turn = turnOf({ state: 'needs-you', phase: 'waiting', line: null, waitingOn: 'question', rev: 4 })
    view.rerender(pane({ onStop: () => {} }))
    expect(text('thread-strip-step-text')).toBe('Stuck on a question')

    threadHook.turn = turnOf({ state: 'monitoring', rev: 5 })
    view.rerender(pane({ onStop: () => {} }))
    expect(strip()?.getAttribute('data-tone')).toBe('monitoring')
    expect(strip()?.className).toContain('--color-accent')
    // Monitoring keeps its word: it says something the pulse doesn't.
    expect(text('thread-strip-state')).toBe('Monitoring')
    expect(screen.getByTestId('thread-strip-stop')).not.toBeNull()
  })

  it('after a reply, live subagents keep the strip: counts and clock in the header, the tally, no step, no Stop', () => {
    threadHook.items = [textItem('t1', 1), textItem('t2', 2)]
    threadHook.turn = turnOf({ phase: 'children', line: null, subagents: 2, subagentsDone: 1, background: 0, phaseSince: SERVER_NOW - 5_000 })
    const view = renderInRoom(overlayRoom, pane({ onStop: () => {} }))
    expect(strip()?.getAttribute('data-tone')).toBe('working')
    expect(strip()?.getAttribute('data-phase')).toBe('children')
    expect(text('thread-strip-state')).toBe('2 subagents running, 1 done')
    expect(text('thread-strip-since')).toBe('· 1m 12s')
    expect(screen.getByTestId('thread-strip-since').getAttribute('title')).toBe('since your message')
    expect(screen.queryByTestId('thread-strip-step')).toBeNull()
    expect(text('thread-strip-tally')).toBe('Read 3 files · ran 2 commands')
    expect(screen.queryByTestId('thread-strip-stop')).toBeNull()
    expect(strip()?.textContent).not.toMatch(/working/i)
    expect(strip()?.getAttribute('aria-label')).toBe('Agent replied; 2 subagents running, 1 done, 1m 12s since your message')
    // It sits under the reply.
    const content = screen.getAllByTestId('thread-item')[1].parentElement
    expect(content?.lastElementChild).toBe(strip())

    // Only a background task left: monitoring.
    threadHook.turn = turnOf({ state: 'monitoring', phase: 'children', line: null, subagents: 0, subagentsDone: 2, background: 1, rev: 5 })
    view.rerender(pane({ onStop: () => {} }))
    expect(strip()?.getAttribute('data-tone')).toBe('monitoring')
    expect(text('thread-strip-state')).toBe('2 subagents done · 1 background task')
    expect(screen.queryByTestId('thread-strip-stop')).toBeNull()

    // The children finish: the reply's end takes the strip away.
    threadHook.turn = turnOf({ state: 'idle', phase: 'children', line: null, subagents: 0, background: 0, rev: 6, end: { reason: 'reply', detail: null, at: SERVER_NOW } })
    view.rerender(pane({ onStop: () => {} }))
    expect(strip()).toBeNull()
  })

  it('"No update in Nm" uses a dashed rule and no step line', () => {
    // A24: phase `stale` starts at the row's staleSince (30 min after the
    // last evidence); 4 more minutes have passed since.
    threadHook.turn = turnOf({ state: 'unverifiable', phase: 'stale', line: null, phaseSince: SERVER_NOW - 4 * 60_000 })
    renderInRoom(overlayRoom, pane())
    expect(strip()?.getAttribute('data-tone')).toBe('unverifiable')
    expect(strip()?.className).toContain('border-dashed')
    expect(text('thread-strip-state')).toBe('No update in 34m')
    expect(text('thread-strip-since')).toBe('· the session is still open')
    expect(screen.queryByTestId('thread-strip-step')).toBeNull()
  })

  it('Stop sends the pane\'s Esc; without a way to type there is no Stop', () => {
    threadHook.turn = turnOf()
    const onStop = vi.fn()
    const view = renderInRoom(overlayRoom, pane({ onStop }))
    fireEvent.click(screen.getByTestId('thread-strip-stop'))
    expect(onStop).toHaveBeenCalledTimes(1)
    view.rerender(pane())
    expect(strip()).not.toBeNull()
    expect(screen.queryByTestId('thread-strip-stop')).toBeNull()
  })

  it('T-S7b: no turn (an older server reports none) draws no strip', () => {
    threadHook.turn = null
    threadHook.turnsReported = false
    renderInRoom(overlayRoom, pane())
    expect(screen.getAllByTestId('thread-item')).toHaveLength(1)
    expect(strip()).toBeNull()
  })
})

describe('Thread working strip keeps the scroll pin (S7)', () => {
  beforeEach(() => {
    FakeResizeObserver.instances = []
    vi.stubGlobal('ResizeObserver', FakeResizeObserver)
    threadHook.items = [textItem('t1', 1), textItem('t2', 2)]
  })

  afterEach(() => {
    cleanup()
    threadHook.items = []
    threadHook.turn = null
    vi.unstubAllGlobals()
  })

  const turnAt = (rev: number, over: Record<string, unknown> = {}): ThreadTurn => {
    const now = Date.now()
    const t = coerceThreadTurn(
      { turnId: 'turn-1', state: 'working', phase: 'delivering', phaseSince: now, line: null, startedAt: now, subagents: 0, background: 0, tally: {}, end: null, serverNow: now, rev, ...over },
      now,
    )
    if (!t) throw new Error('fixture is not a turn')
    return t
  }

  function rerenderPane(view: ReturnType<typeof renderOverlay>): void {
    view.rerender(
      createElement(
        TabVisibilityContext.Provider,
        { value: true },
        // A fresh prop: the pane is memoized over a mutable hook mock.
        createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c', agentName: `k2-${Date.now()}-${Math.random()}` }),
      ),
    )
  }

  it('a pinned list stays at the bottom as the strip appears and grows', () => {
    const view = renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    // The strip shows up under the last message (a render, no new item).
    threadHook.turn = turnAt(1)
    box.scrollHeight = 850
    rerenderPane(view)
    expect(screen.getByTestId('thread-working-strip')).not.toBeNull()
    expect(box.scrollTop).toBe(850)

    // It grows a tally line on a later frame.
    threadHook.turn = turnAt(2, { phase: 'tool', line: 'Reading `a.rs`', tally: { read: 1 } })
    box.scrollHeight = 870
    rerenderPane(view)
    expect(box.scrollTop).toBe(870)

    // A height change the renders don't see (the strip's own 1 Hz text, a
    // wrap) re-pins through the content's ResizeObserver.
    const content = screen.getByTestId('thread-working-strip').parentElement
    if (!content) throw new Error('no content wrapper')
    box.scrollHeight = 900
    act(() => observerOf(content).fire())
    expect(box.scrollTop).toBe(900)
  })

  it('a list the user scrolled up stays put when the strip changes', () => {
    const view = renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    box.scrollTop = 120
    fireEvent.scroll(list)

    threadHook.turn = turnAt(1)
    box.scrollHeight = 850
    rerenderPane(view)
    expect(box.scrollTop).toBe(120)
  })
})
