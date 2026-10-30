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
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
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

import { composeRowChromeWidth, TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { composeTextareaHeight } from '@/components/Terminal/terminalCompose'
import { ThreadAttachmentChip, ThreadFooterFade, threadComposeLines, threadComposeStackNext } from '@/components/Thread/ThreadZenKit'
import { CHAMFER_SCOPE, FOOTER_BLUR_LAYERS, FOOTER_FADE_PX, FOOTER_FADE_TARGETS, FOOTER_GLASS_WASH, FOOTER_TINT_STOPS, FOOTER_TINT_STOPS_OPAQUE, threadFadeTop, THREAD_CHAMFER_CSS, THREAD_COMPOSE_BOX_BORDER, THREAD_COMPOSE_BOX_PAD, THREAD_FOOTER_FADE_CSS, ThreadBubble, zenEmptyThreadText } from '@/components/Thread/ThreadZenKit'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests, type ThreadViewer } from '@/lib/thread-viewer'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { zenEmptyThreadText as zenWidgetEmptyText } from '@/components/Zen/widgets/ZenConversationWidget'
import { coerceThreadTurn, type OverlayThreadItem, type ThreadTurn } from './overlayThread'
import { SessionViewChromeContext, type SessionViewChromeValue } from './sessionViewChrome'
import { ThreadComposeDrawerContext } from './threadComposeDrawer'
import { ThreadItemRow, ThreadOverlayPane } from './ThreadOverlayPane'
import { ThreadOverlayColumn } from './ThreadOverlayColumn'

const room = testRoom({ tabs: {}, scope: primaryScope(), projects: [] })
const OWNER: ThreadViewer = { principal: 'owner', aliases: [] }

const chrome: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: 'c',
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
    expect(field.contains(screen.getByRole('button', { name: 'Attach or save' }))).toBe(true)
    expect(field.contains(screen.getByTestId('compose-slash-button'))).toBe(true)
    // Rosson: the "+" to "/" gap equals the field's edge to "+" (visible ink).
    const tools = field.querySelector('[data-compose-tools]') as HTMLElement
    const plus = screen.getByRole('button', { name: 'Attach or save' })
    const slash = screen.getByTestId('compose-slash-button')
    expect(tools.contains(plus) && tools.contains(slash)).toBe(true)
    const px = (v: string): number => {
      const n = Number.parseFloat(v)
      if (!Number.isFinite(n)) throw new Error(`not a px value: "${v}"`)
      return n
    }
    // The glyphs are drawn shapes whose ink fills their box (no side bearings).
    const plusInk = plus.querySelector('svg[data-compose-glyph="plus"]') as SVGElement
    const slashInk = slash.querySelector('svg[data-compose-glyph="slash"]') as SVGElement
    expect(px(plusInk.getAttribute('width') ?? '')).toBe(12)
    expect(px(slashInk.getAttribute('width') ?? '')).toBe(8)
    expect(plus.textContent).toBe('')
    // The field's own edge: its padding (inline) and its 1px border (the
    // shorthand carries a var(), which jsdom drops, so the kit's constant).
    expect(px(field.style.paddingLeft)).toBe(THREAD_COMPOSE_BOX_PAD)
    expect(THREAD_COMPOSE_BOX_BORDER).toBe(1)
    const edgeToPlus = THREAD_COMPOSE_BOX_BORDER + px(field.style.paddingLeft) + px(tools.style.marginLeft) + px(plus.style.paddingLeft)
    const plusToSlash = px(plus.style.paddingRight) + px(tools.style.gap) + px(slash.style.paddingLeft)
    expect(edgeToPlus).toBe(10)
    expect(plusToSlash).toBe(edgeToPlus)
    // Hit areas: the buttons keep their pads (22px and 18px wide, 26px tall).
    expect(plus.style.height).toBe('26px')
    expect(slash.style.height).toBe('26px')
    // Rosson: the field is 6px shorter (46 → 40): 26px controls + 2×6 padding + 2×1 border.
    const sendBtn = screen.getByRole('button', { name: 'Send message' })
    expect([sendBtn.style.width, sendBtn.style.height]).toEqual(['26px', '26px'])
    expect(26 + 2 * THREAD_COMPOSE_BOX_PAD + 2 * THREAD_COMPOSE_BOX_BORDER).toBe(40)
    // The one-line textarea is 26px at the default 13px font (13 × 1.4 + 8).
    expect(composeTextareaHeight({ value: '', scrollHeight: 0, fontSize: 13 })).toBe(26)
    expect(px(plus.style.paddingLeft) + 12 + px(plus.style.paddingRight)).toBe(22)
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

// Rosson 2026-10-16: in K2's square Style the bubbles, the message field,
// Send and the activity drawer (tab, Stop, tags, pulse dot), then the file
// chips, cards and "Earlier messages", get chamfered corners (the presence
// chips' 45° cuts); Bezel and Glass keep them round. "+" and "/" have no
// fill and are left as they are.
describe('chamfered corners in K2\'s square Style', () => {
  const NOW = Date.now()
  const turn = (): ThreadTurn => {
    const t = coerceThreadTurn(
      { turnId: 't1', state: 'working', phase: 'tool', phaseSince: NOW, line: 'Reading `a.rs`', startedAt: NOW - 5_000, subagents: 0, background: 0, tally: { read: 1 }, end: null, serverNow: NOW, rev: 1 },
      NOW,
    )
    if (!t) throw new Error('fixture is not a turn')
    return t
  }
  const item = (id: string, mine: boolean): OverlayThreadItem => ({
    collection: 'thread',
    seq: mine ? 2 : 1,
    id,
    doc: mine
      ? { id, kind: 'text', from: 'Rosson', via: 'compose', author: { kind: 'owner', login: 'owner' }, body: 'hi', created_at: 1 }
      : { id, kind: 'text', from: 'k2', body: 'hello', created_at: 1 },
  })

  afterEach(() => {
    document.documentElement.removeAttribute('data-style')
    h.threadHook.hasMore = false
  })

  const choice: OverlayThreadItem = {
    collection: 'thread',
    seq: 3,
    id: 'c',
    doc: { id: 'c', kind: 'choice', from: 'k2', body: 'Ship?', created_at: 1, choice: { prompt: 'Ship?', options: [{ label: 'Go' }, { label: 'Wait' }], allow_custom: false, status: 'pending' } },
  }
  const secret: OverlayThreadItem = {
    collection: 'thread',
    seq: 4,
    id: 's',
    doc: { id: 's', kind: 'secret', from: 'k2', body: '', created_at: 1, secret: { name: 'GROK_TOKEN', prompt: 'Paste it', status: 'pending' } },
  }

  function renderThread(extra: OverlayThreadItem[] = []): void {
    h.threadHook.items = [item('a', false), item('m', true), ...extra]
    h.threadHook.hasMore = true
    renderInRoom(
      room,
      <SessionViewChromeContext.Provider value={chrome}>
        <ThreadOverlayColumn
          addr="sales"
          conversationId="c"
          active
          composeBar={
            <ThreadComposeDrawerContext.Provider value={{ turn: turn(), onStop: () => {} }}>
              <TerminalComposeBar sessionId="sess-1" workspacePath="/ws/sales" sendDestination="thread" onInjectInput={() => {}} />
            </ThreadComposeDrawerContext.Provider>
          }
        />
      </SessionViewChromeContext.Provider>,
    )
  }

  const el = (sel: string): HTMLElement => {
    const found = document.querySelector(sel)
    if (!(found instanceof HTMLElement)) throw new Error(`no ${sel}`)
    return found
  }
  /** Does the chamfer rule (the cut-shape layers) apply to `e`? */
  const cut = (e: HTMLElement): boolean => e.matches(`${CHAMFER_SCOPE} [data-chamfer]`)

  it('bubbles, the field, Send, the drawer\'s tab and Stop, and "Earlier messages" carry the chamfer; "+" and "/" do not', () => {
    renderThread()
    const named = Array.from(document.querySelectorAll('[data-chamfer]')).map((e) =>
      e.hasAttribute('data-zen-bubble')
        ? `bubble:${e.getAttribute('data-zen-bubble')}`
        : e.hasAttribute('data-zen-compose-box')
          ? 'field'
          : e.hasAttribute('data-zen-send')
            ? 'send'
            : e.hasAttribute('data-zen-drawer-tile')
              ? 'drawer'
              : e.hasAttribute('data-zen-drawer-stop')
                ? 'stop'
                : e.hasAttribute('data-zen-load-older')
                  ? 'earlier'
                  : e.outerHTML.slice(0, 60),
    )
    expect(named.sort()).toEqual(['bubble:agent', 'bubble:me', 'drawer', 'earlier', 'field', 'send', 'stop'])
    for (const small of [screen.getByRole('button', { name: 'Attach or save' }), screen.getByTestId('compose-slash-button')]) {
      expect(small.hasAttribute('data-chamfer')).toBe(false)
    }
    // Scaled like Send (5px at 32px): Stop 20px → 3px, "Earlier messages" 22px → 3.5px.
    expect(screen.getByTestId('thread-strip-stop').style.getPropertyValue('--ch-tl')).toBe('3px')
    expect(screen.getByTestId('overlay-load-older').style.getPropertyValue('--ch-tl')).toBe('3.5px')
    expect(screen.getByTestId('overlay-load-older').style.getPropertyValue('--ch-fill')).toBe('transparent')
    // The cuts: 4px on surfaces (a bubble tail corner stays sharp), Send 4px (5px at 32px, scaled to its 26px), the drawer 5px (its 32px tab, like Send at 32px).
    const mine = el('[data-zen-bubble="me"]')
    expect(['tl', 'tr', 'br', 'bl'].map((k) => mine.style.getPropertyValue(`--ch-${k}`))).toEqual(['4px', '4px', '0px', '4px'])
    expect(el('[data-zen-bubble="agent"]').style.getPropertyValue('--ch-bl')).toBe('0px')
    expect(el('[data-zen-compose-box]').style.getPropertyValue('--ch-tl')).toBe('4px')
    expect(el('[data-zen-compose-box]').style.getPropertyValue('--ch-bw')).toBe('1px')
    expect(el('[data-zen-send]').style.getPropertyValue('--ch-tl')).toBe('4px')
    // On the box's top edge: top corners cut, the bottom runs into the field.
    const tile = el('[data-zen-drawer-tile]')
    expect(['tl', 'tr', 'br', 'bl'].map((k) => tile.style.getPropertyValue(`--ch-${k}`))).toEqual(['5px', '5px', '0px', '0px'])
  })

  // Rosson 2026-10-30: the stash is a bookmark left of Send (only while the
  // box is empty) that opens the drawer (one ✕); native in each K2 Style.
  it.each(['square', 'glass', 'bezel'])('%s Style: the stash button shows on an empty box, hides on typing, opens the drawer', async (style) => {
    document.documentElement.setAttribute('data-style', style)
    renderThread()
    const send = el('[data-zen-send]')
    const btn = screen.getByTestId('stash-button')
    expect(btn.nextElementSibling).toBe(send)
    // A soft round button like "+" and "/": no fill, so no chamfer in any Style.
    expect(btn.hasAttribute('data-zen-soft-button')).toBe(true)
    expect(btn.hasAttribute('data-chamfer')).toBe(false)
    expect(document.querySelector('[data-stash-tab]')).toBeNull()
    fireEvent.change(box(), { target: { value: 'x' } })
    expect(screen.queryByTestId('stash-button')).toBeNull()
    fireEvent.change(box(), { target: { value: '' } })
    fireEvent.click(screen.getByTestId('stash-button'))
    const drawer = await screen.findByTestId('stash-drawer')
    expect(drawer.querySelectorAll('button[aria-label="Close stash"]')).toHaveLength(1)
    // Square: chamfered; Glass and Bezel: the Thread's round top corners.
    expect(cut(drawer)).toBe(style === 'square')
    expect(drawer.style.borderRadius).toBe('calc(var(--zen-radius) + 2px) calc(var(--zen-radius) + 2px) 0 0')
    // It takes the activity bar's place on the box's top edge.
    expect(el('[data-zen-stash-row]').contains(drawer)).toBe(true)
  })

  it('square Style: the four lose their round corners (pseudo layers draw the cut shape)', () => {
    document.documentElement.setAttribute('data-style', 'square')
    renderThread()
    expect(THREAD_CHAMFER_CSS).toContain('html[data-style="square"] [data-k2-thread-zen] [data-chamfer]')
    expect(THREAD_CHAMFER_CSS).toContain('clip-path: polygon(')
    for (const sel of ['[data-zen-bubble="me"]', '[data-zen-bubble="agent"]', '[data-zen-compose-box]', '[data-zen-send]', '[data-zen-drawer-tile]', '[data-zen-drawer-stop]', '[data-zen-load-older]']) {
      expect(cut(el(sel)), sel).toBe(true)
    }
    // "+" and "/" have no fill: left as they are.
    expect(cut(screen.getByRole('button', { name: 'Attach or save' }))).toBe(false)
    expect(cut(screen.getByTestId('compose-slash-button'))).toBe(false)
    // The rule flattens the radius and paints the cut shape behind the content.
    expect(THREAD_CHAMFER_CSS).toMatch(/\[data-chamfer\] \{ border-radius: 0 !important;/)
    expect(THREAD_CHAMFER_CSS).toContain('[data-chamfer]::after { inset: 0; background: var(--ch-fill); clip-path: polygon(')
    // The drawer's tab clips its own overflow (for the round corners); in the
    // square Style it must not, or its edge layer (on the border) is cut off.
    expect(THREAD_CHAMFER_CSS).toContain(`${CHAMFER_SCOPE} [data-zen-drawer-tile][data-chamfer] { overflow: visible !important; }`)
    expect(el('[data-zen-drawer-tile]').style.overflow).toBe('hidden')
  })

  it('square Style: the cards, their options, the file chips, the drawer\'s tags and its pulse dot', () => {
    document.documentElement.setAttribute('data-style', 'square')
    renderThread([choice, secret])
    const choiceCard = el('[data-testid="thread-choice-card"]')
    expect(cut(choiceCard)).toBe(true)
    expect(choiceCard.style.getPropertyValue('--ch-tl')).toBe('4px')
    for (const opt of screen.getAllByTestId('thread-choice-chip')) {
      expect(cut(opt)).toBe(true)
      expect(opt.style.getPropertyValue('--ch-tl')).toBe('4px')
    }
    const secretCard = el('[data-testid="thread-secret-card"]')
    expect(cut(secretCard)).toBe(true)
    expect(secretCard.style.getPropertyValue('--ch-bw')).toBe('1px')
    // The pulse dot squares off (same pulse); the counter and code tags are cut 3px / 2.5px.
    const dot = el('[data-zen-drawer-dot]')
    expect(dot.matches(`${CHAMFER_SCOPE} [data-zen-drawer-dot]`)).toBe(true)
    expect(dot.hasAttribute('data-zen-pulse')).toBe(true)
    expect(THREAD_CHAMFER_CSS).toContain(`${CHAMFER_SCOPE} [data-zen-drawer-dot] { border-radius: 0 !important; }`)
    expect(THREAD_CHAMFER_CSS).toContain(`${CHAMFER_SCOPE} .zen-drawer-tag { border-radius: 0 !important; --ch-tl: 3px;`)
    expect(THREAD_CHAMFER_CSS).toContain(`${CHAMFER_SCOPE} .zen-drawer-tag.zen-drawer-code { --ch-tl: 2.5px;`)
    const code = el('[data-zen-drawer-code]')
    expect(code.matches(`${CHAMFER_SCOPE} .zen-drawer-tag`)).toBe(true)
  })

  it('square Style: a file chip is chamfered', () => {
    document.documentElement.setAttribute('data-style', 'square')
    render(
      <div data-k2-thread-zen="">
        <ThreadAttachmentChip name="shot.png" onRemove={() => {}} />
      </div>,
    )
    const chip = el('[data-zen-attachment="shot.png"]')
    expect(cut(chip)).toBe(true)
    expect(chip.style.getPropertyValue('--ch-tl')).toBe('4px')
    expect(chip.style.getPropertyValue('--ch-edge')).toBe('var(--zen-border)')
  })

  it('Bezel and Glass keep the round look', () => {
    for (const style of ['bezel', 'glass']) {
      document.documentElement.setAttribute('data-style', style)
      renderThread()
      for (const sel of ['[data-zen-bubble="me"]', '[data-zen-bubble="agent"]', '[data-zen-compose-box]', '[data-zen-send]', '[data-zen-drawer-tile]', '[data-zen-drawer-stop]', '[data-zen-load-older]']) {
        expect(cut(el(sel)), `${style} ${sel}`).toBe(false)
      }
      expect(el('[data-zen-drawer-dot]').matches(`${CHAMFER_SCOPE} [data-zen-drawer-dot]`), style).toBe(false)
      // Their own round corners stay (inline, untouched by the rule).
      expect(el('[data-zen-send]').style.borderRadius, style).toBe('999px')
      cleanup()
    }
  })
})

// Rosson 2026-10-16, from K2 Companion (7cbf584): frosted glass behind the
// whole bottom area; messages scroll under the box and the drawer's tab,
// blurred, and show through them.
describe('frosted glass behind the message box', () => {
  /** The rule for one selector (a line of the stylesheet), or a loud error. */
  const ruleFor = (css: string, needle: string): string => {
    const line = css.split('\n').find((l) => l.includes(needle))
    if (!line) throw new Error(`no rule with ${needle}`)
    return line
  }
  /** The body of an @-block, by its head. */
  const block = (head: string): string => {
    const start = THREAD_FOOTER_FADE_CSS.indexOf(head)
    if (start < 0) throw new Error(`no block ${head}`)
    // The body only (after the head's own "{"), up to the block's "}".
    const open = THREAD_FOOTER_FADE_CSS.indexOf(' {\n', start)
    const end = THREAD_FOOTER_FADE_CSS.indexOf('\n}', open)
    return THREAD_FOOTER_FADE_CSS.slice(open + 3, end)
  }

  it('the strip starts 6px above whatever is on top of the box: the tab when out, else the field', () => {
    expect(FOOTER_FADE_PX).toBe(6)
    // Slot top at 500; field at 512.
    expect(threadFadeTop(500, [512])).toBe(6)
    // The tab out at 480 (above the slot top's padding row): 6px above it.
    expect(threadFadeTop(500, [480, 512])).toBe(-26)
    // Nothing to follow: the stylesheet's default (6px above the slot).
    expect(threadFadeTop(500, [])).toBeNull()
    expect(THREAD_FOOTER_FADE_CSS).toContain('top: -6px; pointer-events: none; z-index: 0; transition: top 200ms')
    expect(FOOTER_FADE_TARGETS).toEqual(['[data-zen-attachments]', '[data-zen-drawer-clip]', '[data-stash-tab]', '[data-stash-drawer]', '[data-zen-compose-box]'])
  })

  it('the column measures it: the tab while it is out, the field when it is closed', () => {
    const rects: Record<string, number> = {}
    const orig = Element.prototype.getBoundingClientRect
    Element.prototype.getBoundingClientRect = function getBoundingClientRect() {
      const node = this as HTMLElement
      for (const [sel, top] of Object.entries(rects)) {
        if (node.matches(sel)) return new DOMRect(0, top, 300, sel.includes('compose-slot') ? 900 - top : 30)
      }
      return orig.call(this)
    }
    const observers: Array<() => void> = []
    vi.stubGlobal(
      'ResizeObserver',
      class {
        constructor(cb: ResizeObserverCallback) {
          observers.push(() => cb([], this as unknown as ResizeObserver))
        }
        observe() {}
        disconnect() {}
        unobserve() {}
      },
    )
    try {
      rects['[data-testid="agent-session-thread-compose-slot"]'] = 820
      rects['[data-zen-compose-box]'] = 828
      const composeBar = (drawerOut: boolean): ReactElement => (
        <div data-zen-compose="">
          {drawerOut ? <div data-zen-drawer-clip="" /> : null}
          <div data-zen-compose-box="" />
        </div>
      )
      const view = renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={composeBar(false)} />)
      const fade = (): HTMLElement => document.querySelector('[data-thread-footer-fade]') as HTMLElement
      // Closed: 6px above the field (828 - 820 - 6).
      expect(fade().style.top).toBe('2px')
      // The tab comes out above the field; the slot grows upward.
      rects['[data-testid="agent-session-thread-compose-slot"]'] = 788
      rects['[data-zen-drawer-clip]'] = 796
      view.rerender(<ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={composeBar(true)} />)
      act(() => {
        for (const fire of observers) fire()
      })
      expect(fade().style.top).toBe('2px')
      // Out, the top is the tab's (following the field it would be 34px).
      rects['[data-zen-drawer-clip]'] = 790
      act(() => {
        for (const fire of observers) fire()
      })
      expect(fade().style.top).toBe('-4px')
      // The list pads by the bar's height so the newest message rests above it.
      expect(screen.getByTestId('thread-overlay-list').style.paddingBottom).toBe(`${8 + (900 - 788)}px`)
    } finally {
      Element.prototype.getBoundingClientRect = orig
      vi.unstubAllGlobals()
    }
  })

  it('a progressive blur: four layers light to heavy, each masked from its start down, overlapping', () => {
    const radii = FOOTER_BLUR_LAYERS.map(([px]) => px)
    expect(radii).toEqual([1, 4, 10, 20])
    FOOTER_BLUR_LAYERS.forEach(([px, from, to], i) => {
      const rule = ruleFor(THREAD_FOOTER_FADE_CSS, `.thread-footer-blur-${i + 1} {`)
      expect(rule).toContain(`-webkit-backdrop-filter: blur(${px}px);`)
      expect(rule).toContain(` backdrop-filter: blur(${px}px);`)
      const mask = `linear-gradient(to bottom, transparent ${from}%, #000 ${to}%)`
      expect(rule).toContain(`-webkit-mask-image: ${mask};`)
      expect(rule).toContain(` mask-image: ${mask};`)
      if (i > 0) {
        const [prevPx, prevFrom, prevTo] = FOOTER_BLUR_LAYERS[i - 1]
        // Heavier, starting lower, and before the one above is fully on (no bands).
        expect(px).toBeGreaterThan(prevPx)
        expect(from).toBeGreaterThan(prevFrom)
        expect(from).toBeLessThan(prevTo)
      }
    })
    // Top ~20%: only the light layers (4px or less) have begun. Bottom ~40%:
    // the two heavy layers are on from 60% and the heaviest is full by 80%.
    expect(FOOTER_BLUR_LAYERS.filter(([, from]) => from < 20).every(([px]) => px <= 4)).toBe(true)
    expect(FOOTER_BLUR_LAYERS[2][2]).toBeLessThanOrEqual(60)
    expect(FOOTER_BLUR_LAYERS[3][2]).toBeLessThanOrEqual(80)
    // Clicks pass through every layer; no will-change; nothing animates a blur.
    expect(THREAD_FOOTER_FADE_CSS).toContain('.thread-footer-fade > span { position: absolute; inset: 0; pointer-events: none; }')
    expect(THREAD_FOOTER_FADE_CSS).not.toMatch(/will-change|transition:[^;]*filter/)
    render(<ThreadFooterFade top={12} />)
    const strip = document.querySelector('[data-thread-footer-fade]') as HTMLElement
    expect(Array.from(strip.children).map((c) => c.className)).toEqual([
      'thread-footer-blur-1',
      'thread-footer-blur-2',
      'thread-footer-blur-3',
      'thread-footer-blur-4',
      'thread-footer-tint',
    ])
    expect(strip.style.top).toBe('12px')
  })

  it('the tint ramps with the blur: nearly clear at the top, stronger toward the bottom', () => {
    expect(FOOTER_TINT_STOPS[0]).toEqual([0, 0])
    expect(FOOTER_TINT_STOPS.at(-1)).toEqual([100, 46])
    for (let i = 1; i < FOOTER_TINT_STOPS.length; i++) {
      expect(FOOTER_TINT_STOPS[i][0]).toBeGreaterThan(FOOTER_TINT_STOPS[i - 1][0])
      expect(FOOTER_TINT_STOPS[i][1]).toBeGreaterThan(FOOTER_TINT_STOPS[i - 1][1])
    }
    // The top 20% stays nearly clear.
    expect(FOOTER_TINT_STOPS.find(([at]) => at === 20)?.[1]).toBeLessThanOrEqual(8)
    expect(ruleFor(THREAD_FOOTER_FADE_CSS, '.thread-footer-tint {')).toContain(
      'linear-gradient(to bottom, color-mix(in srgb, var(--zen-canvas) 0%, transparent) 0%, color-mix(in srgb, var(--zen-canvas) 6%, transparent) 20%,',
    )
    // Reduced transparency keeps its own px ramp to nearly opaque.
    expect(FOOTER_TINT_STOPS_OPAQUE[0]).toEqual([0, 0])
    expect(FOOTER_TINT_STOPS_OPAQUE.at(-1)).toEqual([6, 95])
  })

  it('the field on the bar is see-through (stronger in light), opaque with reduced transparency; the tab keeps its own fill', () => {
    const field = '[data-k2-thread-zen] [data-zen-compose] [data-zen-compose-box]'
    const tab = '[data-k2-thread-zen] [data-zen-compose] [data-zen-drawer-tile]'
    const wash = (pct: number): string => `background: color-mix(in srgb, var(--zen-surface-raised) ${pct}%, transparent) !important; --ch-fill: color-mix(in srgb, var(--zen-surface-raised) ${pct}%, transparent) !important;`
    expect(FOOTER_GLASS_WASH).toEqual({ dark: 72, light: 86 })
    const dark = ruleFor(THREAD_FOOTER_FADE_CSS, `${field} {`)
    expect(dark).toContain(wash(72))
    const light = ruleFor(THREAD_FOOTER_FADE_CSS, `html[data-scheme="light"] ${field} {`)
    expect(light).toContain(wash(86))
    for (const head of ['@media (prefers-reduced-transparency: reduce)', '@supports not ((-webkit-backdrop-filter']) {
      const body = block(head)
      expect(body, head).toContain('[class^="thread-footer-blur-"] { display: none; }')
      expect(body, head).not.toMatch(/blur\(\d|backdrop-filter:/)
      expect(body, head).toContain('background: var(--zen-surface-raised) !important; --ch-fill: var(--zen-surface-raised) !important;')
      expect(body, head).toContain(`html[data-scheme="light"] ${field}`)
      expect(body, head).toContain('var(--zen-canvas) 95%, transparent) 6px')
    }
    // The activity tab keeps its own darker fill in every state of the box
    // (Rosson 2026-10-16), so no glass rule names it.
    expect(THREAD_FOOTER_FADE_CSS).not.toContain(tab)
    // Only the desktop bar's own field (not Zen's box).
    expect(THREAD_FOOTER_FADE_CSS.split('\n').filter((l) => l.includes('surface-raised')).every((l) => l.includes('[data-k2-thread-zen] [data-zen-compose] '))).toBe(true)
  })

  it('a column without a message box draws no fade', () => {
    renderInRoom(room, <ThreadOverlayColumn addr="sales" conversationId="c" active composeBar={null} />)
    expect(document.querySelector('[data-thread-footer-fade]')).toBeNull()
    expect(screen.getByTestId('thread-overlay-list').style.paddingBottom).toBe('8px')
  })

  it('a list pinned at the bottom stays pinned when the box (its inset) grows', () => {
    h.threadHook.items = [
      { collection: 'thread', seq: 1, id: 'x', doc: { id: 'x', kind: 'text', from: 'k2', body: 'hi', created_at: 1 } },
    ]
    const view = renderInRoom(room, <ThreadOverlayPane addr="sales" conversationId="c" bottomInset={40} />)
    const list = screen.getByTestId('thread-overlay-list')
    const box = { scrollHeight: 600, clientHeight: 300, scrollTop: 0 }
    Object.defineProperty(list, 'scrollHeight', { configurable: true, get: () => box.scrollHeight })
    Object.defineProperty(list, 'clientHeight', { configurable: true, get: () => box.clientHeight })
    Object.defineProperty(list, 'scrollTop', { configurable: true, get: () => box.scrollTop, set: (v: number) => { box.scrollTop = v } })
    box.scrollHeight = 660
    view.rerender(<ThreadOverlayPane addr="sales" conversationId="c" bottomInset={100} />)
    expect(list.style.paddingBottom).toBe('108px')
    expect(box.scrollTop).toBe(660)
  })
})

// Rosson 2026-10-16 ("really clean"): past 3 lines the Thread's box stacks
// (text on its own full-width line, "+", "/" and Send under it); back at 2.
// The rule is the shared kit's, so a Garden box can use it too.
describe('stacked message box', () => {
  const composeStackNext = (o: { stacked: boolean; lines: number; width: number }): boolean =>
    threadComposeStackNext({ ...o, rowChrome: composeRowChromeWidth() })

  it('lives in the shared kit and the desktop box uses it', () => {
    const bar = src('Terminal/TerminalComposeBar.tsx')
    for (const name of ['threadComposeStackNext(', 'threadComposeLines(', 'threadComposeTextSlotStyle(', '<ThreadComposeStackSpacer']) {
      expect(bar).toContain(name)
    }
    expect(bar).not.toContain('PREVIEW')
    expect(threadComposeLines({ value: '', scrollHeight: 999 }, 18.2, 8)).toBe(1)
    expect(threadComposeLines({ value: 'x', scrollHeight: 8 + 4 * 18.2 }, 18.2, 8)).toBe(4)
  })

  it('stacks past 3 lines, keeps one row at 3, and unstacks only when the text would fit in 2 one-row lines', () => {
    expect(composeRowChromeWidth()).toBe(80)
    expect(composeStackNext({ stacked: false, lines: 3, width: 300 })).toBe(false)
    expect(composeStackNext({ stacked: false, lines: 4, width: 300 })).toBe(true)
    // Stacked at 300px wide: one row would be 220px wide.
    expect(composeStackNext({ stacked: true, lines: 3, width: 300 })).toBe(true)
    expect(composeStackNext({ stacked: true, lines: 2, width: 300 })).toBe(true)
    expect(composeStackNext({ stacked: true, lines: 1, width: 300 })).toBe(false)
    // Narrower than the row's buttons: stay stacked.
    expect(composeStackNext({ stacked: true, lines: 1, width: 60 })).toBe(true)
  })

  it('never flickers: for any text length the layout settles in one step', () => {
    for (const width of [240, 320, 480, 760]) {
      for (let text = 0; text <= 4000; text += 7) {
        // Lines the text wraps to in each layout (stacked = full width).
        const lines = (stacked: boolean): number => Math.max(1, Math.ceil(text / (stacked ? width : width - 80)))
        let stacked = false
        const seen: boolean[] = []
        for (let i = 0; i < 6; i++) {
          stacked = composeStackNext({ stacked, lines: lines(stacked), width: stacked ? width : width - 80 })
          seen.push(stacked)
        }
        expect(seen.slice(1).every((s) => s === seen[1]), `width ${width}, text ${text}: ${seen}`).toBe(true)
      }
    }
  })

  it('the box stacks and unstacks with the same textarea: focus and the caret stay', () => {
    renderBar({ inject: () => {} })
    const ta = box()
    let height = 26
    Object.defineProperty(ta, 'scrollHeight', { configurable: true, get: () => height })
    Object.defineProperty(ta, 'clientWidth', { configurable: true, get: () => 300 })
    const field = document.querySelector('[data-zen-compose-box]') as HTMLElement
    ta.focus()
    fireEvent.change(ta, { target: { value: 'a'.repeat(120) } })
    expect(field.hasAttribute('data-stacked')).toBe(false)

    // Five lines (13px × 1.4 each + 8): stacked.
    height = Math.round(8 + 5 * 13 * 1.4)
    fireEvent.change(ta, { target: { value: 'a'.repeat(300) } })
    expect(field.getAttribute('data-stacked')).toBe('true')
    expect(box()).toBe(ta)
    expect(document.activeElement).toBe(ta)
    expect(ta.selectionStart).toBe(300)
    // The text goes first across the full width; the buttons wrap under it.
    const wrap = ta.parentElement as HTMLElement
    expect(wrap.style.order).toBe('-1')
    expect(wrap.style.flexBasis).toBe('100%')
    expect(field.style.flexWrap).toBe('wrap')

    // Three lines while stacked: stays (no flicker at the boundary).
    height = Math.round(8 + 3 * 13 * 1.4)
    fireEvent.change(ta, { target: { value: 'a'.repeat(200) } })
    expect(field.getAttribute('data-stacked')).toBe('true')

    // One line: back to one row, same textarea, still focused.
    height = 26
    fireEvent.change(ta, { target: { value: 'short' } })
    expect(field.hasAttribute('data-stacked')).toBe(false)
    expect(wrap.style.order).toBe('')
    expect(box()).toBe(ta)
    expect(document.activeElement).toBe(ta)
    expect(ta.selectionStart).toBe(5)
  })

  it('keys still reach the agent\'s PTY while stacked', () => {
    const inject = vi.fn()
    renderBar({ inject })
    const ta = box()
    Object.defineProperty(ta, 'scrollHeight', { configurable: true, get: () => Math.round(8 + 6 * 13 * 1.4) })
    Object.defineProperty(ta, 'clientWidth', { configurable: true, get: () => 300 })
    fireEvent.change(ta, { target: { value: 'b'.repeat(400) } })
    expect(document.querySelector('[data-zen-compose-box]')?.getAttribute('data-stacked')).toBe('true')
    fireEvent.keyDown(ta, { key: 'Escape' })
    fireEvent.keyDown(ta, { key: 'c', ctrlKey: true })
    expect(inject.mock.calls.map((c) => c[0])).toEqual(['\x1b', '\x03'])
  })
})
