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
import { coerceThreadTurn, overlayItemFromThreadPost, type OverlayThreadItem, type ThreadTurn } from './overlayThread'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope as overlayPrimaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests, type ThreadViewer } from '@/lib/thread-viewer'
import { zenEmptyThreadText } from '@/components/Thread/ThreadZenKit'
import { SUPERSEDE_HOLD_MS } from '@/components/Zen/widgets/ZenActivityDrawer'

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

// The pane asks the room's server who is looking; these tests answer for it
// (the owner on this computer) so no request leaves the test.
beforeEach(() => {
  setThreadViewerForTests(overlayRoom.scope, { principal: 'owner', aliases: [] })
})
afterEach(() => {
  resetThreadViewersForTests()
})

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

  it('render window (0.46.2): a long Thread draws its newest 30; Earlier messages waits until memory runs out', () => {
    const loadOlder = vi.fn(async () => {})
    threadHook.items = Array.from({ length: 100 }, (_, i) => ({
      collection: 'thread',
      seq: i + 1,
      id: `w${i}`,
      doc: { id: `w${i}`, kind: 'text', from: 'sales', created_at: 1_700_000_000 + i, body: `message ${i}` },
    }))
    threadHook.hasMore = true
    threadHook.loadOlder = loadOlder
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    const rows = Array.from(document.querySelectorAll('[data-zen-message]')).map((el) => el.getAttribute('data-zen-message'))
    expect(rows).toHaveLength(30)
    expect(rows[0]).toBe('w70')
    expect(rows[29]).toBe('w99')
    // 70 older messages are still in memory: no fetch button yet.
    expect(screen.queryByTestId('overlay-load-older')).toBeNull()
    // Reaching the top draws from memory first, never fetches.
    const list = overlayList()
    const box = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    // The first scroll after the list gets a height is a resize (re-pin).
    fireEvent.scroll(list)
    expect(document.querySelectorAll('[data-zen-message]').length).toBe(30)
    box.scrollTop = 0
    fireEvent.scroll(list)
    expect(loadOlder).not.toHaveBeenCalled()
    expect(document.querySelectorAll('[data-zen-message]').length).toBe(50)
    expect(document.querySelector('[data-zen-message]')?.getAttribute('data-zen-message')).toBe('w50')
  })

  it('empty list shows the Zen Thread widget\'s centered line and names the agent', () => {
    renderInRoom(
      overlayRoom,
      <ThreadOverlayPane addr="sales" conversationId="c" agentName="Sales Bot" />,
    )
    const empty = screen.getByTestId('thread-overlay-empty')
    expect(empty.className).toContain('items-center')
    expect(empty.className).toContain('justify-center')
    expect(empty.textContent).toBe(zenEmptyThreadText('Sales Bot'))
    expect(empty.textContent).toBe('Message Sales Bot. It reads this like a text.')
    expect(empty.textContent).not.toContain('the agent')
  })

  it('empty list with no name falls back to "the agent"', () => {
    renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
    const empty = screen.getByTestId('thread-overlay-empty')
    expect(empty.textContent).toBe('Message the agent. It reads this like a text.')
  })

  it('blank name also falls back to "the agent"', () => {
    renderInRoom(
      overlayRoom,
      <ThreadOverlayPane addr="sales" conversationId="c" agentName="   " />,
    )
    const empty = screen.getByTestId('thread-overlay-empty')
    expect(empty.textContent).toBe('Message the agent. It reads this like a text.')
  })

  it('shows nothing before the first load finishes', () => {
    threadHook.loaded = false
    renderInRoom(
      overlayRoom,
      <ThreadOverlayPane addr="sales" conversationId="c" agentName="Sales Bot" />,
    )
    expect(screen.queryByTestId('thread-overlay-empty')).toBeNull()
    expect(screen.queryByText(/It reads this like a text/)).toBeNull()
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

  it('says a post came from a Design tab (D-A10)', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 4,
      id: 't-design',
      doc: {
        id: 't-design',
        kind: 'text',
        from: 'alice',
        via: 'compose',
        created_at: Math.floor(Date.now() / 1000),
        body: 'Design notes (1) on http://localhost:5173/',
        design: { itemId: 'd1', url: 'http://localhost:5173/', notes: 1 },
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} />)
    expect(screen.getByText('You · via Design')).not.toBeNull()
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

  describe('a shared Thread: "You" only on the viewer\'s own posts', () => {
    const post = (
      id: string,
      from: string,
      author: { kind: string; login: string } | undefined,
      extra: Partial<OverlayThreadItem['doc']> = {},
    ): OverlayThreadItem => ({
      collection: 'thread',
      seq: 10,
      id,
      doc: {
        id,
        kind: 'text',
        from,
        via: 'compose',
        created_at: Math.floor(Date.now() / 1000),
        body: `body of ${id}`,
        ...(author ? { author } : {}),
        ...extra,
      },
    })
    const ownerViewer: ThreadViewer = { principal: 'owner', aliases: [] }
    const aliceViewer: ThreadViewer = { principal: 'alice', aliases: [] }
    /** The author line's text; "You" (and only "You") is my bubble, on the right. */
    const authorText = (): string => {
      const row = screen.getByTestId('thread-item')
      const el = row.querySelector('[data-thread-author]')
      if (!el) throw new Error(`no author element in ${row.innerHTML}`)
      const text = el.textContent ?? ''
      expect(row.hasAttribute('data-mine')).toBe(text.startsWith('You'))
      expect(row.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe(text.startsWith('You') ? 'me' : 'agent')
      return text
    }

    it('owner viewer: own post is You, a Connect user\'s post shows their name', () => {
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-own', 'Rosson', { kind: 'owner', login: 'owner' })} viewer={ownerViewer} />)
      expect(authorText()).toBe('You')
      cleanup()
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-bob', 'bob', { kind: 'user', login: 'bob' })} viewer={ownerViewer} />)
      expect(authorText()).toBe('bob')
      expect(screen.queryByText('You')).toBeNull()
    })

    it('Connect viewer: their own post is You, the owner\'s post shows the owner\'s name', () => {
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-alice', 'alice', { kind: 'user', login: 'Alice' })} viewer={aliceViewer} />)
      expect(authorText()).toBe('You')
      cleanup()
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-owner', 'Rosson', { kind: 'owner', login: 'owner' })} viewer={aliceViewer} />)
      expect(authorText()).toBe('Rosson')
      expect(screen.queryByText('You')).toBeNull()
    })

    it('the owner\'s linked login (their phone) reads as You to the owner', () => {
      const viewer: ThreadViewer = { principal: 'owner', aliases: ['rosson'] }
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-phone', 'rosson', { kind: 'user', login: 'Rosson' })} viewer={viewer} />)
      expect(authorText()).toBe('You')
    })

    it('an app guest\'s post shows the guest, never You', () => {
      renderInRoom(
        overlayRoom,
        <ThreadItemRow item={post('p-guest', 'guest1', { kind: 'guest', login: 'guest1' }, { via: 'thread' })} viewer={ownerViewer} />,
      )
      expect(authorText()).toBe('guest1')
    })

    it('someone else\'s widget post names them and the widget', () => {
      renderInRoom(
        overlayRoom,
        <ThreadItemRow
          item={post('p-bob-widget', 'bob', { kind: 'user', login: 'bob' }, { widget: { widget: 'Agent Arcade', garden: 'g-test0001' } })}
          viewer={ownerViewer}
        />,
      )
      expect(authorText()).toBe('bob · via Agent Arcade')
    })

    it('an unknown viewer sees names, not You', () => {
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-unknown', 'Rosson', { kind: 'owner', login: 'owner' })} viewer={null} />)
      expect(authorText()).toBe('Rosson')
    })

    it('an old compose doc with no author keeps the old You for any viewer', () => {
      renderInRoom(overlayRoom, <ThreadItemRow item={post('p-old', 'bob', undefined)} viewer={aliceViewer} />)
      expect(authorText()).toBe('You')
    })

    it('the pane paints with the room server\'s viewer', () => {
      setThreadViewerForTests(overlayRoom.scope, aliceViewer)
      threadHook.items = [
        post('pane-alice', 'alice', { kind: 'user', login: 'alice' }),
        { ...post('pane-owner', 'Rosson', { kind: 'owner', login: 'owner' }), seq: 11 },
      ]
      renderInRoom(overlayRoom, <ThreadOverlayPane addr="sales" conversationId="c" />)
      const rows = screen.getAllByTestId('thread-item')
      expect(rows).toHaveLength(2)
      expect(rows[0].textContent).toContain('You')
      expect(rows[1].textContent).toContain('Rosson')
      expect(rows[1].textContent).not.toContain('You')
    })
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

  it('names who answered a card ("You" for the viewer); an older card names nobody', () => {
    const answered = (answeredBy?: { kind: string; login: string }): OverlayThreadItem => ({
      collection: 'thread',
      seq: 1,
      id: 'c-by',
      doc: {
        id: 'c-by',
        kind: 'choice',
        from: 'k2',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status: 'answered',
          answer: 'Go',
          ...(answeredBy ? { answeredBy, answeredAt: 1_700_000_000 } : {}),
        },
      },
    })
    const owner: ThreadViewer = { principal: 'owner', aliases: [] }
    const alice: ThreadViewer = { principal: 'alice', aliases: [] }
    renderInRoom(overlayRoom, <ThreadItemRow item={answered({ kind: 'guest', login: 'patty' })} viewer={owner} />)
    expect(screen.getByTestId('thread-choice-answered-by').textContent).toBe('answered by patty')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={answered({ kind: 'user', login: 'alice' })} viewer={alice} />)
    expect(screen.getByTestId('thread-choice-answered-by').textContent).toBe('answered by You')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={answered({ kind: 'owner', login: 'owner' })} viewer={alice} />)
    expect(screen.getByTestId('thread-choice-answered-by').textContent).toBe('answered by the owner')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={answered()} viewer={owner} />)
    expect(screen.queryByTestId('thread-choice-answered-by')).toBeNull()
  })

  it('shows a pending card\'s time frame, and an expired card as expired with no live chips', () => {
    const now = 1_700_000_000
    const card = (status: string, expiresAt?: number): OverlayThreadItem => ({
      collection: 'thread',
      seq: 1,
      id: 'c-exp',
      doc: {
        id: 'c-exp',
        kind: 'choice',
        from: 'k2',
        choice: {
          prompt: 'Ship it?',
          options: [{ label: 'Go' }, { label: 'Stop' }],
          allow_custom: false,
          status,
          ...(expiresAt !== undefined ? { expiresAt } : {}),
        },
      },
    })
    renderInRoom(overlayRoom, <ThreadItemRow item={card('pending', now + 3 * 86400)} nowSec={now} />)
    expect(screen.getByTestId('thread-choice-expiry').textContent).toBe('expires in 3 days')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={card('pending', now + 2 * 3600)} nowSec={now} />)
    expect(screen.getByTestId('thread-choice-expiry').textContent).toBe('expires in 2 hours')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={card('expired', now - 10)} nowSec={now} />)
    expect(screen.getByTestId('thread-choice-expiry').textContent).toBe('expired')
    for (const chip of screen.getAllByTestId('thread-choice-chip')) expect(chip.getAttribute('disabled')).not.toBeNull()
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={card('pending')} nowSec={now} />)
    expect(screen.queryByTestId('thread-choice-expiry')).toBeNull()
  })

  it('a saved secret card names who saved it', () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 's-by',
      doc: {
        id: 's-by',
        kind: 'secret',
        from: 'k2',
        secret: { name: 'API_TOKEN', status: 'set', ref: 'k2v://ws/k2/API_TOKEN', answeredBy: { kind: 'user', login: 'bob' } },
      },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={item} viewer={{ principal: 'owner', aliases: [] }} />)
    expect(screen.getByTestId('thread-secret-answered-by').textContent).toBe(' by bob')
  })

  it('renders a secret field and submit/dismiss; never shows the value as body text', async () => {
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
        secret: {
          name: 'API_TOKEN',
          status: 'pending',
          prompt: 'Paste the Grok token',
          hosts: ['https://api.x.ai:443'],
        },
      },
    }
    renderInRoom(overlayRoom,
      <ThreadItemRow
        item={item}
        onAnswer={(p) => {
          submitted.push(p.secret || '')
        }}
        onVoid={() => {
          dismissed += 1
        }}
        cardContext={{ workspace: 'grok', server: 'This computer' }}
      />,
    )
    const field = screen.getByTestId('thread-secret-field') as HTMLInputElement
    expect(field.type).toBe('password')
    // The prompt is in the card as plain text, not the message body.
    expect(screen.getByTestId('thread-secret-prompt').textContent).toBe('k2 says: Paste the Grok token')
    fireEvent.change(field, { target: { value: 's3cr3t-bytes' } })
    fireEvent.click(screen.getByTestId('thread-secret-submit'))
    expect(submitted).toEqual(['s3cr3t-bytes'])
    await act(async () => {})
    expect(field.value).toBe('')
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

// prd-daemon-activity-and-thread-working-v1 S7 (T-S7a, T-S7b), drawn the Zen
// Thread widget's way (Rosson 2026-10-16, "match mobile/the zen garden"): the
// activity drawer (`ZenActivityDrawer`) from the hook's `turn`, in place of
// the strip under the last message. Alone (no message box) the pane draws it
// under the list; the strip's Stop (Esc to the agent's PTY) is kept.
describe('Thread activity drawer (S7, the Zen look)', () => {
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
  function pane(props: { onStop?: () => void; onTurn?: (t: ThreadTurn | null) => void } = {}) {
    renders += 1
    return createElement(ThreadOverlayPane, { addr: 'sales', conversationId: 'c', agentName: `k2-${renders}`, ...props })
  }

  const drawer = (): Element | null => document.querySelector('[data-zen-drawer]')
  const text = (sel: string): string => {
    const el = document.querySelector(sel)
    if (!el) throw new Error(`no ${sel}`)
    return (el.textContent ?? '').replace(/\s+/g, ' ').trim()
  }
  const stop = (): HTMLElement | null => screen.queryByTestId('thread-strip-stop')

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

  it('T-S7a: a tool frame draws the drawer under the list: pulse, the line, the clock on the server clock', () => {
    threadHook.turn = turnOf()
    renderInRoom(overlayRoom, pane())
    expect(drawer()?.getAttribute('data-tone')).toBe('working')
    // No message box here: a free tile, outside the scrolling list.
    expect(drawer()?.getAttribute('data-zen-drawer-attached')).toBe('false')
    expect(overlayList().contains(drawer())).toBe(false)
    expect(text('[data-zen-drawer-step]')).toBe('Running cargo test')
    expect(document.querySelector('[data-zen-drawer-step] code')?.textContent).toBe('cargo test')
    // 72 s on the server's clock; this client's own clock would say 70 s.
    expect(text('[data-zen-drawer-clock]')).toBe('1:12')
    expect(document.querySelector('[data-zen-drawer-tile]')?.getAttribute('aria-label')).toBe(
      'Agent working, 1m 12s since your message: Running cargo test',
    )
    // The drawer stands in for the three dots.
    expect(document.querySelector('[data-zen-typing]')).toBeNull()
  })

  it('T-S7a: the clock counts on locally at 1 Hz without a new frame', () => {
    threadHook.turn = turnOf()
    renderInRoom(overlayRoom, pane())
    act(() => {
      vi.advanceTimersByTime(8_000)
    })
    expect(text('[data-zen-drawer-clock]')).toBe('1:20')
  })

  it('a click opens the detail in words (the strip\'s words) and keeps it on this computer', () => {
    localStorage.removeItem('k2.zen.activityDrawerExpanded')
    threadHook.turn = turnOf()
    renderInRoom(overlayRoom, pane())
    const tile = document.querySelector('[data-zen-drawer-tile]') as HTMLElement
    fireEvent.click(tile)
    expect(text('[data-zen-drawer-head]')).toBe('Working · 1m 12s since your message')
    expect(text('[data-zen-drawer-tally]')).toBe('Read 3 files · ran 2 commands')
    expect(localStorage.getItem('k2.zen.activityDrawerExpanded')).toBe('1')
    fireEvent.click(tile)
    expect(document.querySelector('[data-zen-drawer-head]')).toBeNull()
    expect(localStorage.getItem('k2.zen.activityDrawerExpanded')).toBe('0')
  })

  it('T-S7a: an end takes the drawer away (no stored row, no trace; Q8)', () => {
    for (const reason of ['reply', 'done', 'session_gone', 'superseded']) {
      threadHook.turn = turnOf()
      const view = renderInRoom(overlayRoom, pane())
      expect(drawer(), reason).not.toBeNull()
      threadHook.turn = turnOf({ state: reason === 'reply' || reason === 'done' ? 'idle' : 'stopped', rev: 9, end: { reason, detail: null, at: SERVER_NOW } })
      view.rerender(pane())
      // A supersede (a message sent mid-turn) holds the drawer for the new
      // turn's first frame; with none, it goes once the hold runs out.
      if (reason === 'superseded') {
        act(() => {
          vi.advanceTimersByTime(SUPERSEDE_HOLD_MS + 100)
        })
      }
      act(() => {
        vi.advanceTimersByTime(400)
      })
      expect(drawer(), reason).toBeNull()
      // The message keeps its own row; nothing was added.
      expect(screen.getAllByTestId('thread-item'), reason).toHaveLength(1)
      cleanup()
    }
  })

  it('an interrupt reads "Stopped" for a moment, with no Stop, then goes', () => {
    threadHook.turn = turnOf({ state: 'stopped', rev: 9, end: { reason: 'interrupted', detail: null, at: SERVER_NOW } })
    renderInRoom(overlayRoom, pane({ onStop: () => {} }))
    expect(drawer()?.getAttribute('data-tone')).toBe('stopped')
    expect(text('[data-zen-drawer-step]')).toBe('Stopped')
    expect(stop()).toBeNull()
    act(() => {
      vi.advanceTimersByTime(3_100)
    })
    // The linger is over: it slides away.
    expect(drawer()?.getAttribute('data-phase')).toBe('out')
    act(() => {
      vi.advanceTimersByTime(400)
    })
    expect(drawer()).toBeNull()
  })

  it('Stop sends the pane\'s Esc; without a way to type there is no Stop', () => {
    threadHook.turn = turnOf()
    const onStop = vi.fn()
    const view = renderInRoom(overlayRoom, pane({ onStop }))
    const button = stop()
    if (!button) throw new Error('no Stop on a working turn')
    expect(button.getAttribute('title')).toBe('Sends Esc to the agent')
    // A press keeps the caret in the message box (no focus steal).
    expect(fireEvent.mouseDown(button)).toBe(false)
    fireEvent.click(button)
    expect(onStop).toHaveBeenCalledTimes(1)
    view.rerender(pane())
    expect(drawer()).not.toBeNull()
    expect(stop()).toBeNull()
  })

  it('Stop follows the strip\'s rule: monitoring yes; needs-you and a reply\'s subagents no', () => {
    threadHook.turn = turnOf({ state: 'needs-you', phase: 'waiting', line: null })
    const view = renderInRoom(overlayRoom, pane({ onStop: () => {} }))
    expect(drawer()?.getAttribute('data-tone')).toBe('needs-you')
    // A fact, with no call to action and no pointer to the terminal.
    expect(text('[data-zen-drawer-step]')).toBe('Stuck on a permission prompt')
    expect(drawer()?.textContent).not.toMatch(/terminal|approve/i)
    expect(stop()).toBeNull()

    threadHook.turn = turnOf({ state: 'monitoring', rev: 5 })
    view.rerender(pane({ onStop: () => {} }))
    expect(drawer()?.getAttribute('data-tone')).toBe('monitoring')
    expect(stop()).not.toBeNull()

    threadHook.turn = turnOf({ phase: 'children', line: null, subagents: 2, subagentsDone: 1, background: 0, rev: 6 })
    view.rerender(pane({ onStop: () => {} }))
    expect(text('[data-zen-drawer-step]')).toBe('Replied')
    expect(stop()).toBeNull()
  })

  it('T-S7b: no turn (an older server reports none) draws no drawer', () => {
    threadHook.turn = null
    threadHook.turnsReported = false
    renderInRoom(overlayRoom, pane())
    expect(screen.getAllByTestId('thread-item')).toHaveLength(1)
    expect(drawer()).toBeNull()
  })

  it('with a message box (onTurn) the pane hands the turn up and draws no drawer itself', () => {
    const t = turnOf()
    threadHook.turn = t
    const seen: (ThreadTurn | null)[] = []
    const onTurn = (x: ThreadTurn | null): void => {
      seen.push(x)
    }
    const view = renderInRoom(overlayRoom, pane({ onTurn }))
    expect(drawer()).toBeNull()
    expect(seen.at(-1)).toBe(t)
    view.unmount()
    // Gone: the box drops its drawer too.
    expect(seen.at(-1)).toBeNull()
  })
})

describe('Thread activity drawer keeps the scroll pin (S7)', () => {
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

  it('a pinned list stays at the bottom as the drawer appears and the list shrinks', () => {
    const view = renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)

    threadHook.turn = turnAt(1)
    rerenderPane(view)
    expect(document.querySelector('[data-zen-drawer]')).not.toBeNull()
    // The drawer takes room under the list: the list gets shorter.
    box.clientHeight = 168
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(800)
    box.scrollHeight = 860
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(860)
  })

  it('a list the user scrolled up stays put when the drawer changes', () => {
    const view = renderOverlay()
    const list = overlayList()
    const box: ListBox = { scrollHeight: 800, clientHeight: 200, scrollTop: 0 }
    stubListBox(list, box)
    act(() => observerOf(list).fire())
    box.scrollTop = 120
    fireEvent.scroll(list)

    threadHook.turn = turnAt(1)
    rerenderPane(view)
    box.clientHeight = 168
    act(() => observerOf(list).fire())
    expect(box.scrollTop).toBe(120)
  })
})

// Rosson 2026-10-16: his own post read "You -" until the echo. It reads
// "You · just now" on send and then minutes, like an agent's message.
describe('own post time on the author line', () => {
  afterEach(() => cleanup())
  const ownerViewer: ThreadViewer = { principal: 'owner', aliases: [] }
  const timeText = (): string => {
    const row = screen.getByTestId('thread-item')
    const el = row.querySelector('[data-thread-time]')
    if (!el) throw new Error(`no time element in ${row.innerHTML}`)
    return el.textContent ?? ''
  }
  const sent = 2_000_000
  const own = (): OverlayThreadItem => {
    const item = overlayItemFromThreadPost(
      { ok: true, id: 'own', seq: 9, from: 'Rosson', body: 'hey', via: 'compose', author: { kind: 'owner', login: 'owner' } },
      'hey',
      sent,
    )
    if (!item) throw new Error('no row from the post answer')
    return item
  }

  it('reads just now on send, then minutes', () => {
    renderInRoom(overlayRoom, <ThreadItemRow item={own()} viewer={ownerViewer} nowSec={sent} />)
    expect(screen.getByText('You')).toBeTruthy()
    expect(timeText()).toBe('just now')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={own()} viewer={ownerViewer} nowSec={sent + 125} />)
    expect(timeText()).toBe('2m ago')
  })

  it('an agent message reads the same way', () => {
    const agent: OverlayThreadItem = {
      collection: 'thread',
      seq: 10,
      id: 'agent',
      doc: { id: 'agent', kind: 'text', from: 'k2', body: 'reply', created_at: sent },
    }
    renderInRoom(overlayRoom, <ThreadItemRow item={agent} viewer={ownerViewer} nowSec={sent} />)
    expect(timeText()).toBe('just now')
    cleanup()
    renderInRoom(overlayRoom, <ThreadItemRow item={agent} viewer={ownerViewer} nowSec={sent + 125} />)
    expect(timeText()).toBe('2m ago')
  })
})
