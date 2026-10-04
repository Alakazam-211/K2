// @vitest-environment jsdom
// 0.43.2 tickets redesign (Rosson): the board opens on Mine, the list
// width (400 px default) + collapsed state are remembered per window, the
// brief's Options become quick-answer buttons that answer the ticket (free
// text starts a discussion). The chat rail — open by default, toggled from
// the header and remembered per window — is the pre-redesign Thread | Agent
// surface: the ticket thread with the options above its text area and
// Send, and the asking session's terminal. No bottom action bar: status and
// reassign live in the header.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { FeedbackBrief, FeedbackListRow, FeedbackShow } from './feedback-api'

const api = vi.hoisted(() => ({
  fetchFeedbackShow: vi.fn(),
  fetchFeedbackBrief: vi.fn(),
  resolveFeedback: vi.fn(async (..._args: unknown[]) => {}),
  commentFeedback: vi.fn(async (..._args: unknown[]) => ({ ok: true, id: 'x', delivered: true })),
  assignFeedback: vi.fn(async (..._args: unknown[]): Promise<{ assignees: string[]; warnings?: Array<{ code: string; hint: string }> }> => ({
    assignees: [],
    warnings: [],
  })),
}))

vi.mock('./feedback-api', async () => {
  const actual = await vi.importActual<typeof import('./feedback-api')>('./feedback-api')
  return {
    ...actual,
    fetchFeedbackShow: api.fetchFeedbackShow,
    fetchFeedbackBrief: api.fetchFeedbackBrief,
    resolveFeedback: api.resolveFeedback,
    commentFeedback: api.commentFeedback,
    assignFeedback: api.assignFeedback,
  }
})

const cli = vi.hoisted(() => ({
  get: vi.fn(async (_scope: unknown, route: string): Promise<unknown> => {
    if (route === 'auth/whoami') return { owner: true, username: null, role: 'owner' }
    if (route === 'sessions/list-for-workspace') return cli.liveSessions
    if (route === 'sessions/lookup-by-agent') return { sessionAlive: false, sessionId: null }
    if (route === 'users') return { users: [{ username: 'julie' }, { username: 'baden' }] }
    return {}
  }),
  liveSessions: [] as unknown[],
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: cli.get,
  daemonCliGetText: vi.fn(async () => ''),
  daemonCliPost: vi.fn(async () => ({})),
  RecoveringError: class RecoveringError extends Error {},
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => {}) }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'window-test' }),
}))

// The Agent tab mounts the kessel TerminalPane; stub it and record props.
const seen = vi.hoisted(() => ({ terminal: [] as Array<Record<string, unknown>> }))
vi.mock('@/kessel-term/TerminalPane', () => ({
  TerminalPane: (props: Record<string, unknown>) => {
    seen.terminal.push(props)
    return <div data-testid="stub-terminal-pane" />
  },
}))

import { TicketBoard } from './TicketBoard'
import { FeedbackItemView, briefCache } from './FeedbackItemView'
import { readTicketBoardChrome, writeTicketBoardChrome } from '@/lib/ticket-board-chrome'
import { useToastStore } from '@/stores/toast'

const NOW = 1_759_241_200

function makeRow(over: Partial<FeedbackListRow>): FeedbackListRow {
  return {
    id: 'aaaaaaaa-0000-4000-8000-000000000001',
    projectId: 'p1',
    sessionId: null,
    sessionKind: null,
    agentName: 'scout',
    kind: 'question',
    title: 'Which DB?',
    body: 'Pick one.',
    options: null,
    priority: 3,
    status: 'waiting',
    answer: null,
    createdAt: NOW - 120,
    updatedAt: NOW - 120,
    answeredAt: null,
    commentCount: 1,
    assignees: ['owner'],
    projectPath: '/ws',
    projectName: 'Alpha',
    linked: true,
    ...over,
  }
}

function show(r: FeedbackListRow): FeedbackShow {
  return {
    ...r,
    workspace: r.projectName,
    projectPath: r.projectPath,
    canonicalSessionId: null,
    comments: [{ author: 'scout', body: r.title, at: r.createdAt }],
  }
}

const mine = makeRow({ id: 'aaaaaaaa-0000-4000-8000-00000000000a', title: 'Mine ticket', assignees: ['owner'] })
const julies = makeRow({ id: 'bbbbbbbb-0000-4000-8000-00000000000b', title: 'Julie ticket', assignees: ['julie'] })

beforeEach(() => {
  localStorage.clear()
  cli.liveSessions = []
  seen.terminal.length = 0
  useToastStore.setState({ toasts: [] })
})

afterEach(() => {
  cleanup()
  api.fetchFeedbackShow.mockReset()
  api.fetchFeedbackBrief.mockReset()
  api.commentFeedback.mockClear()
  api.resolveFeedback.mockClear()
  api.assignFeedback.mockClear()
  briefCache.clear()
})

function board(rows: FeedbackListRow[] = [mine, julies]): ReturnType<typeof render> {
  api.fetchFeedbackShow.mockImplementation(async (id: string) => show(rows.find((r) => r.id === id) ?? rows[0]))
  return render(<TicketBoard rows={rows} error={null} revision={0} onMutated={vi.fn()} />)
}

describe('TicketBoard filters', () => {
  it('opens on Mine (assigned to the current login); All shows everyone’s', async () => {
    board()
    expect(screen.getByTestId('ticket-scope-mine').getAttribute('aria-selected')).toBe('true')
    await waitFor(() => expect(screen.getAllByTestId('ticket-card')).toHaveLength(1))
    expect(screen.getByTestId('ticket-card').textContent).toContain('Mine ticket')
    expect(screen.queryByText('Julie ticket')).toBeNull()

    fireEvent.click(screen.getByTestId('ticket-scope-all'))
    expect(screen.getAllByTestId('ticket-card')).toHaveLength(2)

    fireEvent.click(screen.getByTestId('ticket-scope-waiting'))
    expect(screen.getAllByTestId('ticket-card').map((c) => c.textContent)).toEqual([
      expect.stringContaining('Mine ticket'),
    ])
  })

  it('search narrows the list', async () => {
    board()
    fireEvent.click(screen.getByTestId('ticket-scope-all'))
    fireEvent.change(screen.getByLabelText('Search tickets'), { target: { value: 'julie' } })
    await waitFor(() => expect(screen.getAllByTestId('ticket-card')).toHaveLength(1))
    expect(screen.getByTestId('ticket-card').textContent).toContain('Julie ticket')
  })
})

describe('TicketBoard per-window layout', () => {
  it('list is 400 px by default; drag-resize and collapse persist for this window', async () => {
    board()
    const list = screen.getByTestId('ticket-list')
    expect(list.style.width).toBe('400px')

    const handle = screen.getByTestId('ticket-list-resize')
    fireEvent.mouseDown(handle, { button: 0, clientX: 400 })
    fireEvent.mouseMove(document, { clientX: 480 })
    fireEvent.mouseUp(document)
    await waitFor(() => expect(screen.getByTestId('ticket-list').style.width).toBe('480px'))
    expect(readTicketBoardChrome('window-test')).toEqual({ listWidth: 480, collapsed: false, chatOpen: true })

    fireEvent.click(screen.getByTestId('ticket-list-collapse'))
    expect(screen.queryByTestId('ticket-list')).toBeNull()
    expect(screen.getByTestId('ticket-list-rail')).toBeTruthy()
    expect(readTicketBoardChrome('window-test')).toEqual({ listWidth: 480, collapsed: true, chatOpen: true })
    // Other windows keep their own layout.
    expect(readTicketBoardChrome('main')).toEqual({ listWidth: 400, collapsed: false, chatOpen: true })

    // A fresh board in this window comes back collapsed.
    cleanup()
    board()
    expect(screen.getByTestId('ticket-list-rail')).toBeTruthy()
    // Rail items: one per ticket with a status dot; clicking selects it.
    await waitFor(() => expect(screen.getAllByTestId('ticket-rail-item')).toHaveLength(1))
    const item = screen.getByTestId('ticket-rail-item')
    expect(within(item).getByTestId('ticket-rail-dot').getAttribute('data-status')).toBe('waiting')
    fireEvent.click(item)
    expect(await screen.findByTestId('ticket-detail')).toBeTruthy()

    fireEvent.click(screen.getByTestId('ticket-list-expand'))
    expect(screen.getByTestId('ticket-list').style.width).toBe('480px')
    expect(readTicketBoardChrome('window-test').collapsed).toBe(false)
  })

  it('a width already saved for this window wins over the 400 px default', () => {
    writeTicketBoardChrome({ listWidth: 260 }, 'window-test')
    board()
    expect(screen.getByTestId('ticket-list').style.width).toBe('260px')
  })
})

const brief: FeedbackBrief = {
  html: '<h2>Options</h2><ol class="k2-options"><li><strong>SQLite</strong> — simple</li><li><strong>Postgres</strong> — later</li></ol>',
  text: 'Options SQLite — simple Postgres — later',
  bytes: 120,
  sha256: 'cafe',
  sanitizer: 'k2-brief-v1',
  createdAt: NOW,
}

function detail(row: FeedbackListRow, canonical: string | null = null): void {
  api.fetchFeedbackShow.mockResolvedValue({ ...show(row), canonicalSessionId: canonical })
  api.fetchFeedbackBrief.mockResolvedValue(brief)
  render(<FeedbackItemView id={row.id} listRow={row} nowSec={NOW} revision={0} onMutated={vi.fn()} />)
}


function railOf(): HTMLElement {
  return screen.getByTestId('ticket-agent-rail')
}

describe('ticket detail', () => {
  it('header: title, from agent → assignee, Expand, Open in window and the chat toggle; history collapsed under a brief', async () => {
    detail(makeRow({ hasBrief: true, briefBytes: 120 }))
    await screen.findByTestId('brief-frame')
    const header = screen.getByTestId('ticket-detail-header')
    expect(screen.getByTestId('ticket-detail-title').textContent).toBe('Which DB?')
    expect(screen.getByTestId('ticket-detail-byline').textContent).toMatch(/^from scout → assigned to Owner/)
    for (const t of ['ticket-expand', 'ticket-open-window', 'ticket-chat-toggle', 'ticket-status', 'ticket-reassign']) {
      expect(header.contains(screen.getByTestId(t)), t).toBe(true)
    }
    expect(screen.getByTestId('ticket-chat-toggle').textContent).toBe('Hide chat')
    const toggle = screen.getByTestId('ticket-history-toggle')
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByTestId('ticket-history')).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getByTestId('ticket-history')).toBeTruthy()
    // Expand opens the brief overlay.
    fireEvent.click(screen.getByTestId('ticket-expand'))
    expect(screen.getByTestId('brief-overlay')).toBeTruthy()
  })

  it('has no bottom action bar: no Answer / Resolve / Reassign / Chat with agent buttons', async () => {
    detail(makeRow({ hasBrief: true, briefBytes: 120 }))
    await screen.findByTestId('brief-frame')
    await within(railOf()).findByTestId('ticket-compose-input')
    expect(screen.queryByTestId('ticket-action-bar')).toBeNull()
    for (const t of ['ticket-action-answer', 'ticket-action-resolve', 'ticket-action-reassign', 'ticket-action-chat']) {
      expect(screen.queryByTestId(t), t).toBeNull()
    }
    const detailPane = screen.getByTestId('ticket-detail')
    const labels = Array.from(detailPane.querySelectorAll('button')).map((b) => b.textContent?.trim())
    for (const gone of ['Answer', 'Resolve', 'Reassign', 'Chat with agent']) {
      expect(labels, gone).not.toContain(gone)
    }
  })

  it('the rail has Thread | Agent tabs; Thread is the ticket thread with a text area and Send', async () => {
    const row = makeRow({})
    detail(row)
    const rail = await screen.findByTestId('ticket-agent-rail')
    const thread = within(rail).getByTestId('ticket-rail-tab-thread')
    const agent = within(rail).getByTestId('ticket-rail-tab-agent')
    expect([thread.textContent, agent.textContent]).toEqual(['Thread', 'Agent'])
    expect(thread.getAttribute('aria-selected')).toBe('true')
    expect(agent.getAttribute('aria-selected')).toBe('false')
    // The ticket's comments are in the thread.
    const pane = await within(rail).findByTestId('ticket-rail-thread')
    expect(pane.textContent).toContain('Which DB?')

    const input = within(rail).getByTestId('ticket-compose-input') as HTMLTextAreaElement
    const send = within(rail).getByTestId('ticket-compose-send') as HTMLButtonElement
    expect(input.tagName).toBe('TEXTAREA')
    expect(send.textContent).toContain('Send')
    expect(send.disabled).toBe(true)
    fireEvent.change(input, { target: { value: 'what about both?' } })
    expect(send.disabled).toBe(false)
    fireEvent.click(send)
    // Free text: the free-text path (no option pick) → needs_discussion.
    await waitFor(() =>
      expect(api.commentFeedback).toHaveBeenCalledWith(row.id, 'what about both?', { optionPick: false }),
    )
    expect(api.commentFeedback).toHaveBeenCalledTimes(1)
    await waitFor(() => expect(input.value).toBe(''))

    // ⌘⏎ sends too.
    fireEvent.change(input, { target: { value: 'and one more' } })
    fireEvent.keyDown(input, { key: 'Enter', metaKey: true })
    await waitFor(() =>
      expect(api.commentFeedback).toHaveBeenLastCalledWith(row.id, 'and one more', { optionPick: false }),
    )
  })

  it('the brief Options render above the text area and a pick sends optionPick', async () => {
    const row = makeRow({ hasBrief: true, briefBytes: 120, options: ['Yes', 'No'] })
    detail(row)
    const rail = await screen.findByTestId('ticket-agent-rail')
    await waitFor(() =>
      expect(within(rail).getAllByTestId('ticket-quick-answer').map((b) => b.textContent)).toEqual(['SQLite', 'Postgres']),
    )
    // Only in the rail, and above the text area.
    expect(screen.getAllByTestId('ticket-quick-answer')).toHaveLength(2)
    const picks = within(rail).getByTestId('ticket-quick-answers')
    const input = within(rail).getByTestId('ticket-compose-input')
    expect(picks.compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(within(rail).getAllByTestId('ticket-quick-answer')[0].getAttribute('title')).toBe('SQLite — simple')

    fireEvent.click(within(rail).getAllByTestId('ticket-quick-answer')[1])
    await waitFor(() =>
      expect(api.commentFeedback).toHaveBeenCalledWith(row.id, 'Postgres', { optionPick: true }),
    )
    expect(api.commentFeedback).toHaveBeenCalledTimes(1)
  })

  it('the Agent tab attaches the asking session’s terminal in place', async () => {
    const row = makeRow({ sessionId: 'sess-1', sessionKind: 'sandbox' })
    cli.liveSessions = [{ sessionId: 'sess-1', agentName: 'tab-x', command: null, args: [], cwd: '/ws', isV2: true }]
    detail(row)
    const rail = await screen.findByTestId('ticket-agent-rail')
    expect(within(rail).queryByTestId('stub-terminal-pane')).toBeNull()
    fireEvent.click(within(rail).getByTestId('ticket-rail-tab-agent'))
    expect(within(rail).getByTestId('ticket-rail-tab-agent').getAttribute('aria-selected')).toBe('true')
    await within(rail).findByTestId('stub-terminal-pane')
    const last = seen.terminal.at(-1)
    if (!last) throw new Error('the terminal never mounted')
    expect(last.attachAgentName).toBe('tab-x')
    expect(last.sessionId).toBe('sess-1')
    expect(last.terminalId).toBe(`feedback-term:${row.id}`)
    // Back to the Thread: the compose box is there again.
    fireEvent.click(within(rail).getByTestId('ticket-rail-tab-thread'))
    expect(within(rail).getByTestId('ticket-compose-input')).toBeTruthy()
  })

  it('the Agent tab without a session says so instead of a terminal', async () => {
    detail(makeRow({ sessionId: null, sessionKind: null }))
    const rail = await screen.findByTestId('ticket-agent-rail')
    fireEvent.click(within(rail).getByTestId('ticket-rail-tab-agent'))
    expect(within(rail).getByTestId('ticket-agent-empty').textContent).toMatch(/No session attached/)
    expect(within(rail).queryByTestId('stub-terminal-pane')).toBeNull()
  })

  it('the header status dropdown sets waiting / needs discussion / resolved over the resolve route', async () => {
    const row = makeRow({ status: 'waiting' })
    detail(row)
    await within(railOf()).findByTestId('ticket-compose-input')
    fireEvent.click(screen.getByTestId('ticket-status'))
    const menu = screen.getByTestId('ticket-header-status-menu')
    expect(Array.from(menu.querySelectorAll('[role="menuitemradio"]')).map((b) => b.textContent)).toEqual([
      'Waiting✓',
      'Needs discussion',
      'Answered',
      'Resolved',
    ])
    fireEvent.click(screen.getByTestId('ticket-status-option-needs_discussion'))
    await waitFor(() => expect(api.resolveFeedback).toHaveBeenCalledWith(row.id, 'needs_discussion', undefined))
    await waitFor(() => expect(screen.queryByTestId('ticket-header-status-menu')).toBeNull())

    fireEvent.click(screen.getByTestId('ticket-status'))
    fireEvent.click(screen.getByTestId('ticket-status-option-resolved'))
    await waitFor(() => expect(api.resolveFeedback).toHaveBeenLastCalledWith(row.id, 'resolved', undefined))
    await waitFor(() => expect(screen.queryByTestId('ticket-header-status-menu')).toBeNull())
    // Picking the current status sends nothing.
    fireEvent.click(screen.getByTestId('ticket-status'))
    fireEvent.click(screen.getByTestId('ticket-status-option-waiting'))
    expect(api.resolveFeedback).toHaveBeenCalledTimes(2)
  })

  it('Answered in the header asks for the agreed outcome, then calls resolve with it', async () => {
    const row = makeRow({ status: 'needs_discussion' })
    detail(row)
    await within(railOf()).findByTestId('ticket-compose-input')
    fireEvent.click(screen.getByTestId('ticket-status'))
    fireEvent.click(screen.getByTestId('ticket-status-option-answered'))
    expect(api.resolveFeedback).not.toHaveBeenCalled()
    const save = screen.getByTestId('ticket-answered-save') as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('ticket-answered-input'), { target: { value: '  SQLite for now  ' } })
    fireEvent.click(save)
    await waitFor(() => expect(api.resolveFeedback).toHaveBeenCalledWith(row.id, 'answered', 'SQLite for now'))
  })

  it('reassign lives in the header: “assigned to” opens the box’s users and saves over the assign route, warning as the daemon does', async () => {
    const row = makeRow({ assignees: ['owner'] })
    detail(row)
    await within(railOf()).findByTestId('ticket-compose-input')
    fireEvent.click(screen.getByTestId('ticket-reassign'))
    const menu = screen.getByTestId('ticket-reassign-menu')
    expect(screen.getByTestId('ticket-detail-header').contains(menu)).toBe(true)
    await waitFor(() =>
      expect(within(menu).getAllByTestId('ticket-reassign-option').map((b) => b.textContent)).toEqual([
        '✓owner',
        'julie',
        'baden',
      ]),
    )
    api.assignFeedback.mockResolvedValueOnce({
      assignees: ['owner', 'julie'],
      warnings: [{ code: 'assignee_unknown', hint: 'julie is not a user on this server' }],
    })
    fireEvent.click(within(menu).getAllByTestId('ticket-reassign-option')[1])
    fireEvent.click(within(menu).getByTestId('ticket-reassign-save'))
    await waitFor(() => expect(api.assignFeedback).toHaveBeenCalledWith(row.id, ['owner', 'julie']))
    await waitFor(() => expect(screen.queryByTestId('ticket-reassign-menu')).toBeNull())
    expect(useToastStore.getState().toasts.map((t) => [t.type, t.message])).toEqual([
      ['warning', 'julie is not a user on this server'],
    ])
  })

  it('the header chat toggle hides and shows the rail, remembered per window (open by default)', async () => {
    const row = makeRow({})
    detail(row)
    await screen.findByTestId('ticket-agent-rail')
    const toggle = screen.getByTestId('ticket-chat-toggle')
    expect(toggle.getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(toggle)
    expect(screen.queryByTestId('ticket-agent-rail')).toBeNull()
    expect(screen.getByTestId('ticket-chat-toggle').textContent).toBe('Show chat')
    expect(readTicketBoardChrome('window-test').chatOpen).toBe(false)
    // Another window keeps its own (default open).
    expect(readTicketBoardChrome('main').chatOpen).toBe(true)

    cleanup()
    detail(row)
    await screen.findByTestId('ticket-detail-header')
    await waitFor(() => expect(api.fetchFeedbackShow).toHaveBeenCalled())
    expect(screen.queryByTestId('ticket-agent-rail')).toBeNull()
    fireEvent.click(screen.getByTestId('ticket-chat-toggle'))
    expect(await screen.findByTestId('ticket-agent-rail')).toBeTruthy()
    expect(readTicketBoardChrome('window-test').chatOpen).toBe(true)
  })
})
