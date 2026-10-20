// @vitest-environment jsdom
// A ticket's discussion in the Thread's look (Rosson 2026-10-18), with the
// ticket's own data and routes kept:
//   - the comments are the Thread's bubbles ("You" on the owner's, the
//     agent's on the left), markdown inside, in light, dark and named Styles;
//   - the message box is the Thread's: Enter sends a free-text comment
//     (`feedback/comment`, no optionPick), Shift+Enter does not; the quick
//     answers are pills above the box and a pick sends `optionPick: true`;
//   - the working strip of the session a reply lands in (the workspace's
//     agent: feedback_routes `deliver_to_asker`) rides on the box, from the
//     daemon's activity row; an unlinked ticket (no workspace) has none;
//   - in Zen (its Tickets view) the chat takes Zen's tokens, not the app's.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
const cli = vi.hoisted(() => ({
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (path: string) => (path === 'sessions/list-for-workspace' ? [] : {})),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
    withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  }
})
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { TicketAgentRail, TicketRailTerminalContext, ticketChatTarget } from './TicketAgentRail'
import type { FeedbackShow, QuickAnswerOption } from './feedback-api'
import { ThreadChatZenContext } from '@/components/Thread/ThreadChat'
import { primaryScope } from '@/kessel/server-scope'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow } from '@/stores/activity'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const SQUARE_CHAMFER = 'html[data-style="square"] [data-k2-thread-zen] [data-chamfer]'
const NOW = 1_759_241_200
const TICKET = 'aaaaaaaa-0000-4000-8000-000000000001'

const ITEM: FeedbackShow = {
  id: TICKET,
  projectId: 'p1',
  sessionId: null,
  sessionKind: null,
  agentName: 'scout',
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
  commentCount: 2,
  assignees: ['owner'],
  workspace: 'Alpha',
  projectPath: '/ws',
  canonicalSessionId: null,
  comments: [
    { author: 'scout', body: 'Which DB? **SQLite** or `postgres`?\n\n- fast\n- simple', at: NOW - 120 },
    { author: 'owner', body: 'thinking', at: NOW - 60 },
  ],
}

const OPTIONS: QuickAnswerOption[] = [
  { answer: 'SQLite', label: 'SQLite', detail: 'SQLite' },
  { answer: 'Postgres', label: 'Postgres', detail: 'Postgres — scale' },
]

function activityRow(over: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId: 'pty-1',
    agentName: 'p1',
    projectId: 'p1',
    workspacePath: '/ws',
    harness: 'claude',
    display: 'waiting',
    lead: { state: 'waiting', outcome: 'none', since: Date.now() - 3_000, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    counts: { subagents: 0, tools: 2, commands: 1 },
    turnStartedAt: Date.now() - 20_000,
    evidenceAt: Date.now(),
    evidenceSource: 'hook',
    reason: 'waiting_permission',
    staleSince: null,
    confirmed: true,
    rev: 1,
    ...over,
  }
}

function rail(opts: { projectPath?: string | null; zen?: boolean } = {}): HTMLElement {
  const projectPath = opts.projectPath === undefined ? '/ws' : opts.projectPath
  const el = (
    <TicketAgentRail
      feedbackId={TICKET}
      item={ITEM}
      error={null}
      nowSec={NOW}
      options={OPTIONS}
      optionsLive
      onChanged={() => {}}
      projectId="p1"
      projectPath={projectPath}
      sessionId={null}
      sessionKind={null}
      canonicalSessionId={null}
    />
  )
  const { container } = render(
    opts.zen ? (
      <TicketRailTerminalContext.Provider value={false}>
        <ThreadChatZenContext.Provider value={true}>{el}</ThreadChatZenContext.Provider>
      </TicketRailTerminalContext.Provider>
    ) : (
      el
    ),
  )
  return container
}

beforeEach(() => {
  localStorage.clear()
  __resetActivityForTests()
  cli.post.mockReset()
  cli.post.mockImplementation(async (path: string) =>
    path === 'feedback/comment' ? { ok: true, delivered: true, deliveryReason: null } : {},
  )
})

afterEach(() => {
  cleanup()
  document.documentElement.removeAttribute('data-style')
  document.documentElement.removeAttribute('data-scheme')
})

describe('a ticket’s discussion in the Thread look', () => {
  for (const [style, scheme] of [
    ['square', 'light'],
    ['square', 'dark'],
    ['glass', 'light'],
    ['bezel', 'dark'],
  ] as const) {
    it(`draws the Thread's bubbles, pills and box (${style} / ${scheme})`, () => {
      document.documentElement.setAttribute('data-style', style)
      document.documentElement.setAttribute('data-scheme', scheme)
      const root = rail()
      const thread = screen.getByTestId('ticket-rail-thread')
      expect(thread.hasAttribute('data-k2-thread-zen')).toBe(true)
      expect(thread.style.getPropertyValue('--zen-bubble-agent')).toBe('var(--color-bg-elevated)')
      const [agent, mine] = Array.from(root.querySelectorAll('[data-zen-message]')) as HTMLElement[]
      expect(agent.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('agent')
      expect(agent.querySelector('[data-thread-author]')?.textContent).toBe('scout')
      expect(mine.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('me')
      expect(mine.querySelector('[data-thread-author]')?.textContent).toBe('You')
      // Markdown in the bubble.
      const body = agent.querySelector('[data-zen-bubble]') as HTMLElement
      expect(body.querySelector('strong')?.textContent).toBe('SQLite')
      expect(body.querySelector('code')?.textContent).toBe('postgres')
      expect(Array.from(body.querySelectorAll('li')).map((li) => li.textContent)).toEqual(['fast', 'simple'])
      // The quick answers: pills above the Thread's field.
      const pills = screen.getAllByTestId('ticket-quick-answer')
      expect(pills.map((p) => p.textContent)).toEqual(['SQLite', 'Postgres'])
      const field = screen.getByTestId('ticket-compose').querySelector('[data-zen-compose-box]') as HTMLElement
      expect(pills[0].compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
      // The Style's shape.
      for (const el of [body, field, pills[0]]) expect([style, el.matches(SQUARE_CHAMFER)]).toEqual([style, style === 'square'])
    })
  }

  it('0.46.3: a path that is a file in the ticket workspace is a link; an unlinked ticket asks nothing', async () => {
    cli.post.mockImplementation(async (path: string, body?: unknown) => {
      if (path !== 'fs/resolve-links') return {}
      const { workspace, paths } = body as { workspace: string; paths: string[] }
      expect(workspace).toBe('/ws')
      return {
        root: '/ws',
        prefixes: ['/ws'],
        links: paths.map((p) =>
          p === 'src/db.rs' ? { input: p, linkable: true, kind: 'file', rel: p, path: `/ws/${p}` } : { input: p, linkable: false },
        ),
      }
    })
    const withPath: FeedbackShow = {
      ...ITEM,
      comments: [{ author: 'scout', body: 'The schema lives in src/db.rs today.', at: NOW - 30 }],
    }
    const { container } = render(
      <TicketAgentRail
        feedbackId={TICKET}
        item={withPath}
        error={null}
        nowSec={NOW}
        options={[]}
        optionsLive={false}
        onChanged={() => {}}
        projectId="p1"
        projectPath="/ws"
        sessionId={null}
        sessionKind={null}
        canonicalSessionId={null}
      />,
    )
    await waitFor(() => expect(container.querySelector('[role="link"]')?.textContent).toBe('src/db.rs'))
    cleanup()
    cli.post.mockClear()
    const unlinked = rail({ projectPath: null })
    await new Promise((r) => setTimeout(r, 20))
    expect(unlinked.querySelector('[role="link"]')).toBeNull()
    expect(cli.post.mock.calls.filter((c: unknown[]) => c[0] === 'fs/resolve-links')).toEqual([])
  })

  it('Enter sends a free-text comment; Shift+Enter does not; a pill sends the answer', async () => {
    rail()
    const input = screen.getByTestId('ticket-compose-input') as HTMLTextAreaElement
    expect(input.placeholder).toBe('Pick an option above, or message scout')
    fireEvent.change(input, { target: { value: 'what about **both**?' } })
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    // 0.46.3: the ticket's file links ask `fs/resolve-links`; only the
    // comment posts count here.
    const comments = (): unknown[][] => cli.post.mock.calls.filter((c: unknown[]) => c[0] === 'feedback/comment')
    expect(comments()).toEqual([])
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(cli.post).toHaveBeenCalledWith('feedback/comment', { id: TICKET, body: 'what about **both**?' }))
    await waitFor(() => expect(input.value).toBe(''))
    fireEvent.click(screen.getAllByTestId('ticket-quick-answer')[1])
    await waitFor(() =>
      expect(cli.post).toHaveBeenLastCalledWith('feedback/comment', { id: TICKET, body: 'Postgres', optionPick: true }),
    )
    expect(comments()).toHaveLength(2)
  })

  it('the working strip of the workspace agent a reply lands in rides on the box', () => {
    expect(ticketChatTarget('p1', '/ws')).toEqual({ agentName: 'p1' })
    __seedActivityForTests(primaryScope(), { rows: [activityRow()], supported: true })
    rail()
    const drawer = screen.getByTestId('ticket-compose').querySelector('[data-zen-drawer]') as HTMLElement | null
    if (!drawer) throw new Error('no working strip on the ticket’s message box')
    expect(drawer.getAttribute('data-zen-drawer-attached')).toBe('true')
    expect(drawer.querySelector('[aria-label]')?.getAttribute('aria-label')).toMatch(/^Agent needs you/)
  })

  it('another workspace’s agent working: no strip here', () => {
    __seedActivityForTests(primaryScope(), { rows: [activityRow({ agentName: 'p2', projectId: 'p2' })], supported: true })
    rail()
    expect(document.querySelector('[data-zen-drawer]')).toBeNull()
  })

  it('an unlinked ticket (no workspace) has no target session: no strip', () => {
    expect(ticketChatTarget('p1', null)).toBeNull()
    __seedActivityForTests(primaryScope(), { rows: [activityRow()], supported: true })
    rail({ projectPath: null })
    expect(document.querySelector('[data-zen-drawer]')).toBeNull()
  })

  it('in Zen the chat takes Zen’s tokens (no app tokens over them), chat only', () => {
    rail({ zen: true })
    const thread = screen.getByTestId('ticket-rail-thread')
    expect(thread.hasAttribute('data-k2-thread-zen')).toBe(false)
    expect(thread.getAttribute('data-thread-chat')).toBe('zen')
    expect(thread.querySelectorAll('[data-zen-bubble]')).toHaveLength(2)
    expect(document.querySelector('[role="tablist"]')).toBeNull()
  })
})
