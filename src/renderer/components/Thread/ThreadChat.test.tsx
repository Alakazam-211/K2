// @vitest-environment jsdom
// The Thread's look for chats that are not Threads (Rosson 2026-10-18):
// a ticket's discussion and the project chat draw the Thread's own pieces.
//
//   1. Parity: a chat message is the Thread's bubble, markup for markup
//      (only the overlay item's own ids differ).
//   2. Every K2 Style: the box carries the Thread's desktop tokens, so the
//      Style's palette paints it; under Square the bubbles and the field
//      take the Thread's chamfers, under Glass and Bezel they stay round;
//      the light scheme gets the field's light wash.
//   3. Markdown renders inside the bubble.
//   4. The message box: Enter sends, Shift+Enter does not, ⌘/Ctrl+Enter
//      sends, an IME composition does not, Send is off while empty or busy.
//   5. Zen: no desktop tokens over Zen's.
//   6. The activity drawer: the daemon's activity row for the chat's one
//      target session, read as the drawer's turn; none without a target.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { useState } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow } from '@/stores/activity'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { CHAMFER_SCOPE, THREAD_CHAMFER_CSS, THREAD_FOOTER_FADE_CSS, THREAD_ZEN_DESKTOP_TOKENS, ThreadBubble } from './ThreadZenKit'
import {
  THREAD_CHAT_SEND_SIZE,
  ThreadChatBubble,
  ThreadChatComposer,
  ThreadChatScope,
  ThreadChatZenContext,
  threadChatScopeProps,
  threadChatSendKey,
} from './ThreadChat'
import { rowForChatTarget, sessionTurnFromRow, useChatSessionTurn, type ChatSessionTarget } from './sessionTurn'
import { COMPOSE_TOOL } from '@/components/Terminal/TerminalComposeBar'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const STYLES = ['square', 'glass', 'bezel'] as const
const SCHEMES = ['light', 'dark'] as const

/** The Square chamfer rule's selector (from the kit's own CSS). */
const SQUARE_CHAMFER = 'html[data-style="square"] [data-k2-thread-zen] [data-chamfer]'
/** The light-scheme wash on the message field (from the kit's own CSS). */
const LIGHT_FIELD = 'html[data-scheme="light"] [data-k2-thread-zen] [data-zen-compose] [data-zen-compose-box]'

function must<T>(v: T | null | undefined, what: string): T {
  if (v === null || v === undefined) throw new Error(`missing ${what}`)
  return v
}

function row(over: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId: 's-1',
    agentName: 'proj-1',
    projectId: 'proj-1',
    workspacePath: '/ws',
    harness: 'claude',
    display: 'working',
    lead: { state: 'working', outcome: 'none', since: Date.now() - 4_000, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    counts: { subagents: 0, tools: 5, commands: 2 },
    turnStartedAt: Date.now() - 12_000,
    evidenceAt: Date.now(),
    evidenceSource: 'hook',
    reason: 'tool_running',
    staleSince: null,
    confirmed: true,
    rev: 3,
    ...over,
  }
}

function Composer({ onSend, busy = false, initial = '' }: { onSend: (t: string) => void; busy?: boolean; initial?: string }): React.JSX.Element {
  const [value, setValue] = useState(initial)
  return (
    <ThreadChatScope>
      <ThreadChatComposer
        value={value}
        onChange={setValue}
        onSend={() => onSend(value)}
        busy={busy}
        placeholder="Message sales"
        inputTestId="input"
        sendTestId="send"
      />
    </ThreadChatScope>
  )
}

function DrawerFor({ target }: { target: ChatSessionTarget | null }): React.JSX.Element {
  const turn = useChatSessionTurn(primaryScope(), target)
  return (
    <ThreadChatScope>
      <ThreadChatComposer value="" onChange={() => {}} onSend={() => {}} placeholder="p" drawer={<ZenActivityDrawer turn={turn} />} />
    </ThreadChatScope>
  )
}

beforeEach(() => {
  __resetActivityForTests()
})

afterEach(() => {
  cleanup()
  document.documentElement.removeAttribute('data-style')
  document.documentElement.removeAttribute('data-scheme')
})

describe('a chat message is the Thread bubble', () => {
  it('renders the same markup as the Thread’s own row (mine and others)', () => {
    for (const mine of [true, false]) {
      const threadItem: OverlayThreadItem = {
        collection: 'thread',
        seq: 4,
        id: 'm1',
        doc: { id: 'm1', kind: 'message', from: 'sales', body: 'Hello **there**', created_at: 100 },
      }
      const a = render(
        <ThreadBubble item={threadItem} mine={mine} author={mine ? 'You' : 'sales'} timeLabel="2m" viewer={null} />,
      )
      const thread = a.container.innerHTML
      a.unmount()
      const b = render(
        <ThreadChatBubble id="m1" seq={4} author="sales" body="Hello **there**" createdAt={100} mine={mine} timeLabel="2m" />,
      )
      expect(b.container.innerHTML).toBe(thread)
      b.unmount()
    }
  })

  it('author · time under it; "You" on the viewer’s own post; a footer under that', () => {
    const { container } = render(
      <>
        <ThreadChatBubble id="a" author="owner" body="mine" createdAt={1} mine timeLabel="now" footer="delivered to Ada" />
        <ThreadChatBubble id="b" author="ada" body="theirs" createdAt={1} mine={false} timeLabel="1m" />
      </>,
    )
    const [me, them] = Array.from(container.querySelectorAll('[data-zen-message]')) as HTMLElement[]
    expect(me.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('me')
    expect(me.querySelector('[data-thread-author]')?.textContent).toBe('You')
    expect(me.querySelector('[data-thread-time]')?.textContent).toBe('now')
    expect(me.querySelector('[data-zen-message-footer]')?.textContent).toBe('delivered to Ada')
    expect(me.style.alignSelf).toBe('flex-end')
    expect(them.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('agent')
    expect(them.querySelector('[data-thread-author]')?.textContent).toBe('ada')
    expect(them.querySelector('[data-zen-message-footer]')).toBeNull()
    expect(them.style.alignSelf).toBe('flex-start')
  })

  it('renders markdown in the bubble', () => {
    const { container } = render(
      <ThreadChatBubble id="md" author="ada" body={'**bold** and `code`\n\n- one\n- two'} createdAt={1} mine={false} timeLabel="1m" />,
    )
    const bubble = must(container.querySelector('[data-zen-bubble]'), 'bubble')
    expect(bubble.querySelector('strong')?.textContent).toBe('bold')
    expect(bubble.querySelector('code')?.textContent).toBe('code')
    expect(Array.from(bubble.querySelectorAll('li')).map((li) => li.textContent)).toEqual(['one', 'two'])
  })
})

describe('every K2 Style paints the chat as it paints the Thread', () => {
  it('the kit CSS still carries the selectors this checks', () => {
    expect(THREAD_CHAMFER_CSS).toContain(CHAMFER_SCOPE)
    expect(SQUARE_CHAMFER).toBe(`${CHAMFER_SCOPE} [data-chamfer]`)
    expect(THREAD_CHAMFER_CSS).toContain(SQUARE_CHAMFER)
    expect(THREAD_FOOTER_FADE_CSS).toContain(LIGHT_FIELD)
  })

  for (const style of STYLES) {
    for (const scheme of SCHEMES) {
      it(`${style} / ${scheme}`, () => {
        document.documentElement.setAttribute('data-style', style)
        document.documentElement.setAttribute('data-scheme', scheme)
        const { container } = render(
          <ThreadChatScope testId="scope">
            <ThreadChatBubble id="a" author="ada" body="hi" createdAt={1} mine={false} timeLabel="1m" />
            <ThreadChatBubble id="b" author="owner" body="yo" createdAt={1} mine timeLabel="now" />
            <ThreadChatComposer value="x" onChange={() => {}} onSend={() => {}} placeholder="p" sendTestId="send" />
          </ThreadChatScope>,
        )
        const scope = must(container.querySelector('[data-testid="scope"]') as HTMLElement | null, 'scope')
        // The Thread's desktop box: its tokens, from the Style's palette.
        expect(scope.hasAttribute('data-k2-thread-zen')).toBe(true)
        for (const [k, v] of Object.entries(THREAD_ZEN_DESKTOP_TOKENS as Record<string, string>)) {
          expect([k, scope.style.getPropertyValue(k)]).toEqual([k, v])
        }
        expect(scope.querySelector('style[data-thread-zen-styles]')).not.toBeNull()
        const bubbles = Array.from(scope.querySelectorAll('[data-zen-bubble]')) as HTMLElement[]
        expect(bubbles).toHaveLength(2)
        const box = must(scope.querySelector('[data-zen-compose-box]') as HTMLElement | null, 'field')
        const send = must(scope.querySelector('[data-testid="send"]') as HTMLElement | null, 'send')
        // Square: chamfered; Glass / Bezel: round (the rule does not reach).
        for (const el of [...bubbles, box, send]) {
          expect([style, el.matches(SQUARE_CHAMFER)]).toEqual([style, style === 'square'])
        }
        expect(bubbles[0].style.borderRadius).toBe('var(--zen-bubble-radius)')
        // The light scheme's wash on the field, as on the Thread's field.
        expect([scheme, box.matches(LIGHT_FIELD)]).toEqual([scheme, scheme === 'light'])
      })
    }
  }

  it('inside Zen the box sets no tokens over Zen’s (and no desktop scope)', () => {
    expect(threadChatScopeProps(true, 13)).toEqual({ 'data-thread-chat': 'zen' })
    const { container } = render(
      <ThreadChatZenContext.Provider value={true}>
        <ThreadChatScope testId="scope">
          <ThreadChatBubble id="a" author="ada" body="hi" createdAt={1} mine={false} timeLabel="1m" />
        </ThreadChatScope>
      </ThreadChatZenContext.Provider>,
    )
    const scope = must(container.querySelector('[data-testid="scope"]') as HTMLElement | null, 'scope')
    expect(scope.hasAttribute('data-k2-thread-zen')).toBe(false)
    expect(scope.style.getPropertyValue('--zen-bubble-me')).toBe('')
    expect(scope.querySelector('[data-zen-bubble]')).not.toBeNull()
  })
})

describe('the message box', () => {
  it('is the Thread’s field and round Send at the Thread box’s size', () => {
    expect(THREAD_CHAT_SEND_SIZE).toBe(COMPOSE_TOOL.height)
    const { getByTestId, container } = render(<Composer onSend={() => {}} />)
    const input = getByTestId('input') as HTMLTextAreaElement
    expect(input.hasAttribute('data-zen-compose-input')).toBe(true)
    expect(input.placeholder).toBe('Message sales')
    expect(must(container.querySelector('[data-zen-compose]'), 'bar').contains(input)).toBe(true)
    const send = getByTestId('send') as HTMLButtonElement
    expect(send.hasAttribute('data-zen-send')).toBe(true)
    expect(send.style.width).toBe(`${COMPOSE_TOOL.height}px`)
  })

  it('Enter sends, Shift+Enter does not, ⌘/Ctrl+Enter sends; Send is off while empty', () => {
    const sent: string[] = []
    const { getByTestId } = render(<Composer onSend={(t) => sent.push(t)} />)
    const input = getByTestId('input') as HTMLTextAreaElement
    const send = getByTestId('send') as HTMLButtonElement
    expect(send.disabled).toBe(true)
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(sent).toEqual([])
    fireEvent.change(input, { target: { value: 'hello' } })
    expect(send.disabled).toBe(false)
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    expect(sent).toEqual([])
    fireEvent.keyDown(input, { key: 'Enter' })
    fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true })
    fireEvent.click(send)
    expect(sent).toEqual(['hello', 'hello', 'hello', 'hello'])
  })

  it('busy: nothing sends', () => {
    const sent: string[] = []
    const { getByTestId } = render(<Composer onSend={(t) => sent.push(t)} busy initial="hi" />)
    expect((getByTestId('send') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.keyDown(getByTestId('input'), { key: 'Enter' })
    fireEvent.click(getByTestId('send'))
    expect(sent).toEqual([])
  })

  it('an IME composition’s Enter is not a send', () => {
    expect(threadChatSendKey({ key: 'Enter', shiftKey: false, altKey: false, nativeEvent: { isComposing: true } })).toBe(false)
    expect(threadChatSendKey({ key: 'Enter', shiftKey: false, altKey: false, nativeEvent: { isComposing: false } })).toBe(true)
    expect(threadChatSendKey({ key: 'a', shiftKey: false, altKey: false })).toBe(false)
  })
})

describe('the activity drawer of the chat’s target session', () => {
  it('reads the daemon row as the drawer’s turn (working, needs you, children, stale, idle)', () => {
    // One row: its clock-based times read once (two rows can differ by 1 ms).
    const r = row()
    const working = must(sessionTurnFromRow(r, 250), 'working turn')
    expect(working.state).toBe('working')
    expect(working.phase).toBe('working')
    expect(working.end).toBeNull()
    expect(working.tally).toEqual({ read: 0, search: 0, cmd: 2, edit: 0 })
    expect(working.skewMs).toBe(250)
    expect(working.turnId).toBe(`s-1:${r.turnStartedAt}`)
    expect(must(sessionTurnFromRow(row({ reason: 'thinking' }), 0), 'thinking').phase).toBe('thinking')

    const waiting = must(
      sessionTurnFromRow(row({ display: 'waiting', lead: { state: 'waiting', outcome: 'none', since: 1, promptId: null }, reason: 'waiting_question' }), 0),
      'waiting turn',
    )
    expect([waiting.state, waiting.phase, waiting.waitingOn]).toEqual(['needs-you', 'waiting', 'question'])

    const kids = must(
      sessionTurnFromRow(
        row({
          display: 'working',
          lead: { state: 'idle', outcome: 'success', since: 1, promptId: null },
          counts: { subagents: 2, tools: 0, commands: 0 },
        }),
        0,
      ),
      'children turn',
    )
    expect([kids.state, kids.phase, kids.subagents]).toEqual(['working', 'children', 2])

    const stale = must(sessionTurnFromRow(row({ display: 'unverifiable', staleSince: 77 }), 0), 'stale turn')
    expect([stale.state, stale.phase, stale.phaseSince]).toEqual(['unverifiable', 'stale', 77])

    expect(sessionTurnFromRow(row({ display: 'idle', lead: { state: 'idle', outcome: 'success', since: 1, promptId: null } }), 0)).toBeNull()
    expect(sessionTurnFromRow(null, 0)).toBeNull()
  })

  it('one turn while the lead works: a newer row of the same turn keeps its id', () => {
    // row() stamps its times from Date.now(): freeze the clock so both rows
    // carry the same turn start (under load the two calls straddle a ms).
    vi.useFakeTimers({ toFake: ['Date'], now: 1_793_145_600_000 })
    try {
      const a = must(sessionTurnFromRow(row({ rev: 3 }), 0), 'a')
      const b = must(sessionTurnFromRow(row({ rev: 9, counts: { subagents: 1, tools: 9, commands: 4 } }), 0), 'b')
      expect(b.turnId).toBe(a.turnId)
      const next = must(sessionTurnFromRow(row({ turnStartedAt: 999 }), 0), 'next')
      expect(next.turnId).not.toBe(a.turnId)
    } finally {
      vi.useRealTimers()
    }
  })

  it('finds the row by session id, else by the agent name (a pinned Chat: the project id)', () => {
    const rows = new Map([['s-1', row()], ['s-2', row({ sessionId: 's-2', agentName: 'other' })]])
    expect(rowForChatTarget({ rows }, { sessionId: 's-2' })?.sessionId).toBe('s-2')
    expect(rowForChatTarget({ rows }, { sessionId: 'gone', agentName: 'proj-1' })?.sessionId).toBe('s-1')
    expect(rowForChatTarget({ rows }, { agentName: 'nobody' })).toBeNull()
    expect(rowForChatTarget({ rows }, null)).toBeNull()
  })

  it('draws on the message box while the target works; nothing without a target or while idle', () => {
    __seedActivityForTests(primaryScope(), { rows: [row()], supported: true })
    const on = render(<DrawerFor target={{ agentName: 'proj-1' }} />)
    const drawer = must(on.container.querySelector('[data-zen-drawer]') as HTMLElement | null, 'drawer')
    expect(drawer.getAttribute('data-zen-drawer-attached')).toBe('true')
    // On the field's top edge, inside the message box.
    const box = must(on.container.querySelector('[data-zen-compose-box]'), 'field')
    expect(drawer.compareDocumentPosition(box) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(must(on.container.querySelector('[aria-label]'), 'tile').getAttribute('aria-label')).toMatch(/^Agent working/)
    on.unmount()

    const none = render(<DrawerFor target={null} />)
    expect(none.container.querySelector('[data-zen-drawer]')).toBeNull()
    none.unmount()

    __seedActivityForTests(primaryScope(), {
      rows: [row({ display: 'idle', lead: { state: 'idle', outcome: 'success', since: 1, promptId: null } })],
      supported: true,
    })
    const idle = render(<DrawerFor target={{ agentName: 'proj-1' }} />)
    expect(idle.container.querySelector('[data-zen-drawer]')).toBeNull()
  })
})
