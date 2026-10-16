// @vitest-environment jsdom
// The desktop Thread wears the Zen Thread widget's look (Rosson 2026-10-16:
// "match what we have in mobile/the zen garden ... keep the / commands ...
// make sure Esc and the hot keys still pass through to the terminal").
//
//   1. Parity: the desktop row and the Zen widget's row are the same shared
//      bubble (`ThreadZenKit`), so they render the same markup.
//   2. The Thread's message box (TerminalComposeBar, sendDestination thread):
//      the slash menu still lists and runs /compact and /goal.
//   3. Keys: Esc, Ctrl+C, Ctrl+B and an empty Enter still reach the agent's
//      PTY (`onInjectInput`); ⌘/Ctrl chords still reach the app; Esc with
//      the slash menu open only closes the menu.
//   4. The activity drawer rides on the box's top edge; its Stop is Esc.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ReactElement } from 'react'

const h = vi.hoisted(() => {
  const daemonCliGet = vi.fn(async (route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  })
  const daemonCliPost = vi.fn(async (route: string, _body?: unknown) => {
    if (route === 'thread/post') return { ok: true, id: 'p1', seq: 7 }
    return { ok: true }
  })
  const threadHook = {
    items: [] as unknown[],
    error: null as string | null,
    posting: false,
    post: async () => {},
    answer: async () => {},
    voidCard: async () => {},
    hasMore: false,
    loadingOlder: false,
    loaded: true,
    loadOlder: async () => {},
    turn: null as unknown,
    turnsReported: true,
    currentAddr: 'sales',
    pastAddresses: [] as string[],
  }
  return { daemonCliGet, daemonCliPost, threadHook }
})

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(h.daemonCliGet),
    daemonCliPost: primaryOnly(h.daemonCliPost),
  }
})

vi.mock('./useOverlayThread', () => ({
  useOverlayThread: () => h.threadHook,
}))

import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { ThreadBubble, zenEmptyThreadText } from '@/components/Thread/ThreadZenKit'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests, type ThreadViewer } from '@/lib/thread-viewer'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { zenEmptyThreadText as zenWidgetEmptyText } from '@/components/Zen/widgets/ZenConversationWidget'
import { coerceThreadTurn, type OverlayThreadItem, type ThreadTurn } from './overlayThread'
import { SessionViewChromeContext, type SessionViewChromeValue } from './sessionViewChrome'
import { ThreadComposeDrawerContext } from './threadComposeDrawer'
import { ThreadItemRow } from './ThreadOverlayPane'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'

const room = testRoom({ tabs: {}, scope: primaryScope(), projects: [] })
const OWNER: ThreadViewer = { principal: 'owner', aliases: [] }

const chrome: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: 'c',
  chatConversationId: null,
  chatProvider: null,
  agentName: 'sales',
  displayName: 'Sales',
}

function src(rel: string): string {
  return readFileSync(resolve(__dirname, '..', rel), 'utf8')
}

beforeEach(() => {
  setThreadViewerForTests(room.scope, OWNER)
  localStorage.clear()
})
afterEach(() => {
  cleanup()
  resetThreadViewersForTests()
  h.daemonCliPost.mockClear()
  h.threadHook.items = []
  h.threadHook.turn = null
})

// ── 1. Parity with the Zen Thread widget ────────────────────────────────

describe('the desktop Thread row is the Zen Thread widget\'s row', () => {
  const NOW = 2_000_000
  const agent: OverlayThreadItem = {
    collection: 'thread',
    seq: 3,
    id: 'a1',
    doc: { id: 'a1', kind: 'text', from: 'k2', body: 'Shipped **it**', created_at: NOW - 125 },
  }
  const mine: OverlayThreadItem = {
    collection: 'thread',
    seq: 4,
    id: 'm1',
    doc: { id: 'm1', kind: 'text', from: 'Rosson', via: 'compose', author: { kind: 'owner', login: 'owner' }, body: 'thanks', created_at: NOW },
  }
  const choice: OverlayThreadItem = {
    collection: 'thread',
    seq: 5,
    id: 'c1',
    doc: {
      id: 'c1',
      kind: 'choice',
      from: 'k2',
      body: 'Ship it?',
      created_at: NOW,
      choice: { prompt: 'Ship it?', options: [{ label: 'Go' }, { label: 'Wait' }], allow_custom: false, status: 'pending' },
    },
  }

  /** What the Zen Conversation widget draws for `item` (its `Bubble`). */
  function zenRow(item: OverlayThreadItem, isMine: boolean): ReactElement {
    return (
      <ThreadBubble
        item={item}
        mine={isMine}
        author={isMine ? 'You' : item.doc.from || 'agent'}
        timeLabel={formatRelativeTime(item.doc.created_at, NOW)}
        viewer={OWNER}
        nowSec={NOW}
        secret={{ compact: true }}
      />
    )
  }

  function markup(ui: ReactElement): { bubble: string; meta: string; side: string | null } {
    const view = renderInRoom(room, ui)
    const row = view.container.querySelector('[data-zen-message]')
    const bubble = row?.querySelector('[data-zen-bubble]')
    const meta = row?.querySelector('[data-zen-message-meta]')
    if (!row || !bubble || !meta) throw new Error(`no bubble row in ${view.container.innerHTML}`)
    const out = { bubble: bubble.outerHTML, meta: meta.textContent ?? '', side: bubble.getAttribute('data-zen-bubble') }
    cleanup()
    return out
  }

  it('an agent message, my message and a choice card render the same bubble on both', () => {
    for (const [item, isMine] of [[agent, false], [mine, true], [choice, false]] as const) {
      const desktop = markup(<ThreadItemRow item={item} viewer={OWNER} nowSec={NOW} />)
      const zen = markup(zenRow(item, isMine))
      expect(desktop.bubble, item.id).toBe(zen.bubble)
      expect(desktop.meta, item.id).toBe(zen.meta)
      expect(desktop.side, item.id).toBe(isMine ? 'me' : 'agent')
    }
  })

  it('own message reads "You · just now" on the right; the agent\'s reads its name and minutes on the left', () => {
    const desktopMine = markup(<ThreadItemRow item={mine} viewer={OWNER} nowSec={NOW} />)
    expect(desktopMine.meta).toBe('You · just now')
    expect(desktopMine.side).toBe('me')
    const desktopAgent = markup(<ThreadItemRow item={agent} viewer={OWNER} nowSec={NOW} />)
    expect(desktopAgent.meta).toBe('k2 · 2m ago')
    expect(desktopAgent.side).toBe('agent')
  })

  it('the empty line is the Zen widget\'s', () => {
    expect(zenWidgetEmptyText).toBe(zenEmptyThreadText)
  })

  it('both draw from one kit (shared, not copied)', () => {
    const pane = src('SessionView/ThreadOverlayPane.tsx')
    const zen = src('Zen/widgets/ZenConversationWidget.tsx')
    for (const file of [pane, zen]) {
      expect(file).toContain("from '@/components/Thread/ThreadZenKit'")
      expect(file).toContain('<ThreadBubble')
      expect(file).not.toContain('<ChatMessage ')
    }
    const bar = src('Terminal/TerminalComposeBar.tsx')
    const zenBox = src('Zen/widgets/ZenCompose.tsx')
    for (const file of [bar, zenBox]) {
      expect(file).toContain('<ThreadComposeBox')
      expect(file).toContain('<ThreadSendButton')
    }
    // The drawer is the Garden's own component on both.
    expect(src('Terminal/TerminalComposeBar.tsx')).toContain("import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'")
    expect(src('Zen/widgets/ZenConversationWidget.tsx')).toContain("from './ZenActivityDrawer'")
  })
})

// ── 2–4. The Thread's message box ───────────────────────────────────────

function renderBar(opts: { inject?: (d: string) => void; drawer?: { turn: ThreadTurn | null; onStop?: () => void } } = {}) {
  return renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <ThreadComposeDrawerContext.Provider value={opts.drawer ?? null}>
        <TerminalComposeBar sessionId="sess-1" workspacePath="/ws/sales" sendDestination="thread" onInjectInput={opts.inject} />
      </ThreadComposeDrawerContext.Provider>
    </SessionViewChromeContext.Provider>,
  )
}

function box(): HTMLTextAreaElement {
  return screen.getByRole('textbox') as HTMLTextAreaElement
}

function threadPosts(): Record<string, unknown>[] {
  return h.daemonCliPost.mock.calls
    .filter((c) => c[0] === 'thread/post')
    .map((c) => c[1] as Record<string, unknown>)
}

describe('the Thread\'s message box (Zen look)', () => {
  it('is the Zen box: round field, "+", "/", round Send', () => {
    renderBar()
    const bar = document.querySelector('[data-compose-bar]')
    expect(bar?.getAttribute('data-compose-look')).toBe('zen')
    expect(bar?.getAttribute('data-compose-destination')).toBe('thread')
    const field = bar?.querySelector('[data-zen-compose-box]') as HTMLElement
    expect(field).not.toBeNull()
    expect(field.contains(box())).toBe(true)
    expect(field.contains(screen.getByRole('button', { name: 'Attach file' }))).toBe(true)
    expect(field.contains(screen.getByTestId('compose-slash-button'))).toBe(true)
    const send = screen.getByRole('button', { name: 'Send message' })
    expect(send.hasAttribute('data-zen-send')).toBe(true)
    expect(send.style.borderRadius).toBe('999px')
    expect(box().getAttribute('placeholder')).toContain('Message')
  })

  it('the slash menu lists /compact and /goal, and /compact sends as the command', async () => {
    renderBar()
    fireEvent.click(screen.getByTestId('compose-slash-button'))
    const menu = screen.getByTestId('compose-slash-menu')
    const rows = Array.from(menu.querySelectorAll('[role="menuitem"]')).map((r) => (r.textContent ?? '').replace(/\s+/g, ' '))
    expect(rows).toEqual(['/compactcompact context', '/goalset a goal'])
    fireEvent.click(menu.querySelector('[role="menuitem"]') as HTMLElement)
    expect(screen.queryByTestId('compose-slash-menu')).toBeNull()
    expect(screen.getByTestId('compose-slash-button').getAttribute('aria-pressed')).toBe('true')
    // An empty draft with a command still sends (the command alone).
    const send = screen.getByRole('button', { name: 'Send message' })
    expect(send.hasAttribute('disabled')).toBe(false)
    fireEvent.click(send)
    await waitFor(() => expect(threadPosts()).toHaveLength(1))
    expect(threadPosts()[0]).toEqual({ addr: 'sales', text: '', via: 'compose', command: '/compact' })
  })

  it('typing "/go" opens the menu on /goal; Enter picks it (no send, no PTY), the next Enter sends with it', async () => {
    const inject = vi.fn()
    renderBar({ inject })
    fireEvent.change(box(), { target: { value: '/go' } })
    const menu = screen.getByTestId('compose-slash-menu')
    expect(Array.from(menu.querySelectorAll('[role="menuitem"]')).map((r) => r.textContent)).toEqual(['/goalset a goal'])
    fireEvent.keyDown(box(), { key: 'Enter' })
    expect(screen.queryByTestId('compose-slash-menu')).toBeNull()
    expect(box().value).toBe('')
    expect(threadPosts()).toEqual([])
    expect(inject).not.toHaveBeenCalled()
    fireEvent.change(box(), { target: { value: 'ship the release' } })
    fireEvent.keyDown(box(), { key: 'Enter' })
    await waitFor(() => expect(threadPosts()).toHaveLength(1))
    expect(threadPosts()[0]).toEqual({ addr: 'sales', text: 'ship the release', via: 'compose', command: '/goal' })
    expect(inject).not.toHaveBeenCalled()
  })

  it('Esc, Ctrl+C, Ctrl+B and an empty Enter reach the agent\'s PTY; the draft stays', () => {
    const inject = vi.fn()
    renderBar({ inject })
    fireEvent.change(box(), { target: { value: 'half typed' } })
    fireEvent.keyDown(box(), { key: 'Escape' })
    fireEvent.keyDown(box(), { key: 'c', ctrlKey: true })
    fireEvent.keyDown(box(), { key: 'b', ctrlKey: true })
    expect(inject.mock.calls.map((c) => c[0])).toEqual(['\x1b', '\x03', '\x02'])
    expect(box().value).toBe('half typed')
    fireEvent.change(box(), { target: { value: '' } })
    fireEvent.keyDown(box(), { key: 'Enter' })
    expect(inject.mock.calls.map((c) => c[0])).toEqual(['\x1b', '\x03', '\x02', '\r'])
    // Cmd+C is copy, not an interrupt; Shift+Enter is a newline.
    fireEvent.keyDown(box(), { key: 'c', metaKey: true })
    fireEvent.keyDown(box(), { key: 'Enter', shiftKey: true })
    expect(inject).toHaveBeenCalledTimes(4)
    expect(threadPosts()).toEqual([])
  })

  it('Esc with the slash menu open closes the menu only', () => {
    const inject = vi.fn()
    renderBar({ inject })
    fireEvent.click(screen.getByTestId('compose-slash-button'))
    expect(screen.getByTestId('compose-slash-menu')).not.toBeNull()
    fireEvent.keyDown(box(), { key: 'Escape' })
    expect(screen.queryByTestId('compose-slash-menu')).toBeNull()
    expect(inject).not.toHaveBeenCalled()
    // Closed: the next Esc is the agent's again.
    fireEvent.keyDown(box(), { key: 'Escape' })
    expect(inject.mock.calls.map((c) => c[0])).toEqual(['\x1b'])
  })

  it('⌘/Ctrl chords reach the app\'s window shortcuts; plain keys stay in the box', () => {
    const seen: string[] = []
    const onKey = (e: KeyboardEvent): void => {
      seen.push(`${e.metaKey ? 'Meta+' : ''}${e.ctrlKey ? 'Ctrl+' : ''}${e.key}`)
    }
    window.addEventListener('keydown', onKey)
    try {
      renderBar({ inject: () => {} })
      fireEvent.keyDown(box(), { key: 't', metaKey: true })
      fireEvent.keyDown(box(), { key: 'T', metaKey: true, shiftKey: true })
      fireEvent.keyDown(box(), { key: 'w', metaKey: true })
      fireEvent.keyDown(box(), { key: 'a' })
      fireEvent.keyDown(box(), { key: 'ArrowLeft' })
      expect(seen).toEqual(['Meta+t', 'Meta+T', 'Meta+w'])
    } finally {
      window.removeEventListener('keydown', onKey)
    }
  })
})

describe('the activity drawer on the Thread\'s message box', () => {
  const NOW = Date.now()
  const turn = (): ThreadTurn => {
    const t = coerceThreadTurn(
      { turnId: 't1', state: 'working', phase: 'tool', phaseSince: NOW, line: 'Reading `a.rs`', startedAt: NOW - 5_000, subagents: 0, background: 0, tally: { read: 1 }, end: null, serverNow: NOW, rev: 1 },
      NOW,
    )
    if (!t) throw new Error('fixture is not a turn')
    return t
  }

  it('sits on the box\'s top edge; Stop sends Esc and keeps the caret in the box', () => {
    const onStop = vi.fn()
    renderBar({ drawer: { turn: turn(), onStop } })
    const bar = document.querySelector('[data-compose-bar]') as HTMLElement
    const drawer = bar.querySelector('[data-zen-drawer]')
    expect(drawer?.getAttribute('data-zen-drawer-attached')).toBe('true')
    // Above the field, inside the bar.
    const field = bar.querySelector('[data-zen-compose-box]') as HTMLElement
    if (!drawer) throw new Error('no drawer on the box')
    expect(drawer.compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
    box().focus()
    const stop = screen.getByTestId('thread-strip-stop')
    expect(fireEvent.mouseDown(stop)).toBe(false)
    fireEvent.click(stop)
    expect(onStop).toHaveBeenCalledTimes(1)
    expect(document.activeElement).toBe(box())
  })

  it('a column with a message box moves the list\'s drawer onto the box; Stop is the column\'s Esc', () => {
    h.threadHook.turn = turn()
    const orig = Element.prototype.getBoundingClientRect
    Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
      const node = this as HTMLElement
      if (node.getAttribute?.('data-testid') === 'agent-session-thread-compose-slot') return new DOMRect(0, 0, 300, 90)
      return orig.call(this)
    }
    try {
      const onStop = vi.fn()
      renderInRoom(
        room,
        <SessionViewChromeContext.Provider value={chrome}>
          <ThreadOverlayColumn
            addr="sales"
            conversationId="c"
            active
            agentName="Sales"
            onStop={onStop}
            composeBar={<TerminalComposeBar sessionId="sess-1" workspacePath="/ws/sales" sendDestination="thread" onInjectInput={() => {}} />}
          />
        </SessionViewChromeContext.Provider>,
      )
      const drawers = document.querySelectorAll('[data-zen-drawer]')
      expect(drawers).toHaveLength(1)
      const slot = screen.getByTestId('agent-session-thread-compose-slot')
      expect(slot.contains(drawers[0])).toBe(true)
      expect(drawers[0].getAttribute('data-zen-drawer-attached')).toBe('true')
      fireEvent.click(screen.getByTestId('thread-strip-stop'))
      expect(onStop).toHaveBeenCalledTimes(1)
      // The whole column is one Zen Thread box with the app's colours.
      const column = screen.getByTestId('agent-session-thread')
      expect(column.hasAttribute('data-k2-thread-zen')).toBe(true)
      expect(column.style.getPropertyValue('--zen-bubble-me')).toBe('var(--color-accent)')
    } finally {
      Element.prototype.getBoundingClientRect = orig
    }
  })

  it('without a message box the list keeps the drawer, free-standing', () => {
    h.threadHook.turn = turn()
    renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={null} />)
    const drawer = document.querySelector('[data-zen-drawer]')
    expect(drawer?.getAttribute('data-zen-drawer-attached')).toBe('false')
    expect(screen.queryByTestId('thread-strip-stop')).toBeNull()
  })
})
