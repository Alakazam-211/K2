// @vitest-environment jsdom
// A ticket's discussion on the Thread store (`ticket-thread-v1`):
//   - the column draws the agent chat's Thread pane on `ticket:<id>` (its
//     address and its conversation), with no Thread | Agent tabs;
//   - the floating header has the terminal drawer, which opens the ticket
//     agent's terminal, and no view menu, Chatter, Heartbeats, bell or
//     session history;
//   - the message box keeps the ticket's quick answers above it (a pick is
//     the answer) and its closed line, has no "/" commands, and its draft is
//     the ticket's, never the agent's own chat's;
//   - Resolve still works;
//   - Zen's Tickets view has no terminal;
//   - an older server keeps the old column.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
const cli = vi.hoisted(() => ({
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
  get: vi.fn(async (_path: string, _q?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string, q?: unknown) => cli.get(path, q)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
    withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  }
})
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))
vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({ AgentAvatar: () => null }))
// The Thread's list is the agent chat's own (its tests cover it); this
// stand-in records where it reads from and draws the real message box.
const column = vi.hoisted(() => ({ props: [] as Array<{ addr: string; conversationId: string | null }> }))
vi.mock('@/components/SessionView/ThreadOverlayColumn', () => ({
  ThreadOverlayColumn: (p: { addr: string; conversationId: string | null; composeBar: React.ReactNode }) => {
    column.props.push({ addr: p.addr, conversationId: p.conversationId })
    return (
      <div data-testid="stub-thread-column" data-addr={p.addr}>
        {p.composeBar}
      </div>
    )
  },
}))

import type React from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { TicketAgentRail, TicketRailTerminalContext } from './TicketAgentRail'
import type { FeedbackShow, QuickAnswerOption } from './feedback-api'
import { primaryScope } from '@/kessel/server-scope'
import {
  TICKET_THREAD_FEATURE,
  ticketConversationId,
  ticketDraftId,
  ticketImportedAuthor,
  ticketPostOutcome,
  ticketThreadAddr,
  ticketThreadItem,
} from './ticket-thread-contract'
import { threadSender } from '@/components/Thread/threadSenders'
import { threadAuthorLabel } from '@/components/SessionView/ThreadOverlayPane'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const NOW = 1_759_241_200
const TICKET = 'aaaaaaaa-0000-4000-8000-000000000001'

const ITEM: FeedbackShow = {
  id: TICKET,
  projectId: 'p1',
  sessionId: null,
  sessionKind: null,
  agentName: 'ada',
  kind: 'question',
  title: 'Which DB?',
  body: 'Pick one.',
  options: ['SQLite', 'Postgres'],
  priority: 3,
  status: 'waiting',
  answer: null,
  createdAt: NOW - 120,
  updatedAt: NOW - 120,
  answeredAt: null,
  commentCount: 1,
  assignees: ['owner'],
  workspace: 'Alpha',
  projectPath: '/ws',
  canonicalSessionId: null,
  comments: [{ author: 'ada', body: 'Which DB?', at: NOW - 120 }],
}

const OPTIONS: QuickAnswerOption[] = [
  { answer: 'SQLite', label: 'SQLite', detail: 'SQLite' },
  { answer: 'Postgres', label: 'Postgres', detail: 'Postgres — scale' },
]

let features = new Set<string>()

/** The daemon's answer to a ticket post (the Thread post's shape plus
 *  `ticket`). */
let ticketAnswer: Record<string, unknown> = { status: 'answered', answered: true, delivered: true, deliveryReason: null }
function threadPostAnswer(body: { text: string; optionPick?: boolean }): Record<string, unknown> {
  return {
    ok: true, id: 'm-new', seq: 9, from: 'owner', to: `ticket:${TICKET}`, kind: 'text', body: body.text, via: 'ticket',
    author: { kind: 'owner', login: 'owner' }, created_at: NOW, conversation_id: `ticket:${TICKET}`, addr: `ticket:${TICKET}`, mine: true,
    ticket: { id: TICKET, ...ticketAnswer },
  }
}

function rail(
  opts: { zen?: boolean; item?: FeedbackShow; live?: boolean; onChanged?: () => void; onTicketPatch?: (p: unknown) => void } = {},
): HTMLElement {
  const el = (
    <TicketAgentRail
      feedbackId={TICKET}
      item={opts.item ?? ITEM}
      error={null}
      nowSec={NOW}
      options={OPTIONS}
      optionsLive={opts.live ?? true}
      onChanged={opts.onChanged ?? (() => {})}
      onTicketPatch={opts.onTicketPatch}
      projectId="p1"
      projectPath="/ws"
      sessionId={null}
      sessionKind={null}
      canonicalSessionId={null}
    />
  )
  const { container } = render(
    opts.zen ? <TicketRailTerminalContext.Provider value={false}>{el}</TicketRailTerminalContext.Provider> : el,
  )
  return container
}

beforeEach(() => {
  localStorage.clear()
  ticketAnswer = { status: 'answered', answered: true, delivered: true, deliveryReason: null }
  column.props = []
  features = new Set([TICKET_THREAD_FEATURE])
  vi.spyOn(primaryScope(), 'serverSupports').mockImplementation((f) => features.has(f))
  cli.post.mockReset()
  cli.post.mockImplementation(async (path: string, body?: unknown) => {
    if (path === 'feedback/comment') return { ok: true, delivered: true, deliveryReason: null }
    if (path === 'thread/post') return threadPostAnswer(body as { text: string; optionPick?: boolean })
    return { ok: true }
  })
  cli.get.mockReset()
  cli.get.mockImplementation(async (path: string) => (path === 'sessions/list-for-workspace' ? [] : {}))
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the contract seam', () => {
  it('a ticket is `ticket:<id>` as its address, conversation and draft', () => {
    expect(ticketThreadAddr(TICKET)).toBe(`ticket:${TICKET}`)
    expect(ticketConversationId(TICKET)).toBe(`ticket:${TICKET}`)
    expect(ticketDraftId(TICKET)).toBe(`ticket:${TICKET}`)
    expect(TICKET_THREAD_FEATURE).toBe('ticket-thread-v1')
  })

  it('a post’s answer says the ticket’s status and whether the agent got it', () => {
    expect(ticketPostOutcome({ ok: true, ticket: { id: TICKET, status: 'needs_discussion', answered: false, delivered: false, deliveryReason: 'the agent is asleep' } })).toEqual({
      status: 'needs_discussion', answered: false, delivered: false, deliveryReason: 'the agent is asleep',
    })
    expect(ticketPostOutcome({ ok: true, ticket: { id: TICKET, status: 'waiting', answered: false, delivered: null, deliveryReason: null } })?.delivered).toBeNull()
    expect(ticketPostOutcome({ ok: true })).toBeNull()
  })

  it('a message moved in from the old store names its author as text only', () => {
    expect(ticketImportedAuthor({ imported: 'jordan' })).toBe('jordan')
    expect(ticketImportedAuthor({ imported: { author: 'jordan' } })).toBe('jordan')
    expect(ticketImportedAuthor({})).toBeNull()
    const moved: OverlayThreadItem = {
      collection: 'thread', seq: 1, id: 'c-1',
      doc: { id: 'c-1', kind: 'text', from: 'owner', body: 'Looks close.', created_at: NOW, via: 'ticket', author: { kind: 'owner', login: 'owner' }, mine: true, imported: { author: 'jordan' } },
    }
    const shown = ticketThreadItem(moved)
    // A name, not a person (no roster lookup) and not an agent (no picture).
    expect(threadSender(shown.doc)).toEqual({ key: 'name:jordan', kind: 'name', label: 'jordan', id: '' })
    expect(threadAuthorLabel(shown.doc, { kind: 'owner', login: 'owner' } as never)).toEqual({ label: 'jordan', mine: false })
    // A message written on the Thread comes back as it is.
    const plain: OverlayThreadItem = { ...moved, doc: { ...moved.doc, imported: undefined } }
    expect(ticketThreadItem(plain)).toBe(plain)
  })
})

describe('ticket-thread-v1 on', () => {
  it('mounts the Thread pane on the ticket’s conversation, with no Thread | Agent tabs', () => {
    rail()
    expect(screen.getByTestId('ticket-thread-pane')).toBeTruthy()
    expect(screen.queryByTestId('ticket-rail-tab-thread')).toBeNull()
    expect(screen.queryByTestId('ticket-rail-tab-agent')).toBeNull()
    expect(screen.queryByTestId('ticket-rail-thread')).toBeNull()
    expect(column.props.at(-1)).toEqual({ addr: `ticket:${TICKET}`, conversationId: `ticket:${TICKET}` })
  })

  it('the header: the terminal drawer and search, never the view menu, Chatter, Heartbeats, bell or history', () => {
    rail()
    const header = screen.getByTestId('ticket-thread-header')
    expect(within(header).getByTestId('thread-terminal-toggle')).toBeTruthy()
    expect(within(header).queryByTestId('thread-panel-toggle-chatter')).toBeNull()
    expect(within(header).queryByTestId('thread-panel-toggle-heartbeats')).toBeNull()
    expect(within(header).queryByTestId('session-view-menu')).toBeNull()
    expect(header.querySelector('[aria-label="Switch pinned chat session"]')).toBeNull()
    expect(header.querySelector('[data-testid*="bell"]')).toBeNull()
    expect(within(header).getByTestId('thread-view-name').textContent).toBe('ada')
  })

  it('the terminal drawer opens and closes the ticket agent’s terminal', async () => {
    rail()
    const toggle = screen.getByTestId('thread-terminal-toggle')
    expect(toggle.getAttribute('aria-pressed')).toBe('false')
    // A closed drawer doesn't look for the agent's session.
    expect(screen.queryByTestId('ticket-agent-empty')).toBeNull()
    expect(cli.get.mock.calls.map((c) => c[0])).not.toContain('sessions/list-for-workspace')
    await act(async () => {
      fireEvent.click(toggle)
    })
    expect(screen.getByTestId('thread-terminal-toggle').getAttribute('aria-pressed')).toBe('true')
    // The drawer holds the ticket's own wake-and-attach terminal: this
    // ticket was filed outside a session, so it says there is none.
    const drawer = screen.getByTestId('session-reveal-terminal')
    expect(within(drawer).getByTestId('ticket-agent-empty')).toBeTruthy()
    await act(async () => {
      fireEvent.click(screen.getByTestId('thread-terminal-toggle'))
    })
    expect(screen.getByTestId('thread-terminal-toggle').getAttribute('aria-pressed')).toBe('false')
  })

  it('the quick answers sit over the message box and a pick is the answer', async () => {
    const onChanged = vi.fn()
    rail({ onChanged })
    const box = screen.getByTestId('stub-thread-column')
    const chips = within(box).getAllByTestId('ticket-quick-answer')
    expect(chips.map((c) => c.textContent)).toEqual(['SQLite', 'Postgres'])
    expect((within(box).getByRole('textbox') as HTMLTextAreaElement).placeholder).toBe('Or type your own')
    await act(async () => {
      fireEvent.click(chips[1])
    })
    await waitFor(() => expect(onChanged).toHaveBeenCalled())
    // One Thread post on the ticket, sent once: it is the answer.
    const [path, body] = cli.post.mock.calls.find((c) => c[0] === 'thread/post')!
    expect(path).toBe('thread/post')
    expect(body).toMatchObject({ addr: `ticket:${TICKET}`, text: 'Postgres', via: 'compose', optionPick: true })
    expect(typeof (body as { clientMsgId?: unknown }).clientMsgId).toBe('string')
    expect(cli.post.mock.calls.map((c) => c[0])).not.toContain('feedback/comment')
  })

  it('a pick’s answer moves the header at once, and says when the agent didn’t get it', async () => {
    const onTicketPatch = vi.fn()
    const onChanged = vi.fn()
    ticketAnswer = { status: 'answered', answered: true, delivered: false, deliveryReason: 'the agent is asleep' }
    rail({ onTicketPatch, onChanged })
    await act(async () => {
      fireEvent.click(screen.getAllByTestId('ticket-quick-answer')[0])
    })
    await waitFor(() => expect(onTicketPatch).toHaveBeenCalledWith({ status: 'answered', answer: 'SQLite' }))
    expect(onChanged).not.toHaveBeenCalled()
    expect(screen.getByText('Saved. The agent did not receive it (the agent is asleep).')).toBeTruthy()
  })

  it('a closed ticket says a reply reopens it, and takes no pick', () => {
    rail({ item: { ...ITEM, status: 'resolved' }, live: false })
    expect(screen.getByTestId('ticket-quick-answers-closed').textContent).toBe('Resolved. Replying reopens it.')
    expect(screen.queryByTestId('ticket-quick-answer')).toBeNull()
  })

  it('Resolve still resolves the ticket', async () => {
    const onChanged = vi.fn()
    rail({ onChanged })
    await act(async () => {
      fireEvent.click(screen.getByTestId('ticket-discussion-resolve'))
    })
    await waitFor(() => expect(onChanged).toHaveBeenCalled())
    expect(cli.post.mock.calls.map((c) => c[0])).toContain('feedback/resolve')
  })

  it('the box has no "/" commands', () => {
    rail()
    const box = screen.getByTestId('stub-thread-column')
    expect(within(box).queryByTestId('compose-slash-button')).toBeNull()
    fireEvent.change(within(box).getByRole('textbox'), { target: { value: '/mo' } })
    expect(screen.queryByTestId('compose-slash-menu')).toBeNull()
  })

  it('the draft is the ticket’s, never the agent’s own chat’s', () => {
    rail()
    const box = screen.getByTestId('stub-thread-column')
    fireEvent.change(within(box).getByRole('textbox'), { target: { value: 'about the schema' } })
    const keys = Object.keys(localStorage).filter((k) => k.includes('k2:composer:draft:'))
    expect(keys.some((k) => k.endsWith(`k2:composer:draft:agent:ticket:${TICKET}:thread`))).toBe(true)
    // The workspace chat's own draft (its agent name is the project id).
    expect(keys.some((k) => k.endsWith('k2:composer:draft:agent:p1:thread'))).toBe(false)
  })
})

describe('Zen’s Tickets view', () => {
  it('the same pane with no terminal', () => {
    rail({ zen: true })
    expect(screen.getByTestId('ticket-thread-pane')).toBeTruthy()
    expect(screen.queryByTestId('thread-terminal-toggle')).toBeNull()
    expect(screen.queryByTestId('ticket-agent-empty')).toBeNull()
    expect(screen.queryByTestId('stub-terminal-pane')).toBeNull()
    expect(screen.queryByTestId('ticket-rail-tab-agent')).toBeNull()
  })
})

describe('ticket-thread-v1 off (an older server)', () => {
  it('keeps the old column: its tabs, its own list and box', () => {
    features = new Set()
    rail()
    expect(screen.queryByTestId('ticket-thread-pane')).toBeNull()
    expect(screen.getByTestId('ticket-rail-tab-thread')).toBeTruthy()
    expect(screen.getByTestId('ticket-rail-tab-agent')).toBeTruthy()
    expect(screen.getByTestId('ticket-rail-thread')).toBeTruthy()
    expect(screen.getByTestId('ticket-compose')).toBeTruthy()
    expect(column.props).toEqual([])
  })
})
