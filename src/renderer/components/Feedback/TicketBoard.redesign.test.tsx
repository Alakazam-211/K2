// @vitest-environment jsdom
// 0.43.2 tickets redesign (Rosson): the board opens on Mine, the list
// width (400 px default) + collapsed state are remembered per window, the
// brief's Options become quick-answer buttons in the chat rail that answer
// the ticket (free text starts a discussion), and the chat rail — open by
// default, remembered per window — is the Agents page's Thread | Terminal
// view (AgentSessionChrome + its View menu).
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { FeedbackBrief, FeedbackListRow, FeedbackShow } from './feedback-api'

const api = vi.hoisted(() => ({
  fetchFeedbackShow: vi.fn(),
  fetchFeedbackBrief: vi.fn(),
  resolveFeedback: vi.fn(async () => {}),
  commentFeedback: vi.fn(async () => ({ ok: true, id: 'x', delivered: true })),
}))

vi.mock('./feedback-api', async () => {
  const actual = await vi.importActual<typeof import('./feedback-api')>('./feedback-api')
  return {
    ...actual,
    fetchFeedbackShow: api.fetchFeedbackShow,
    fetchFeedbackBrief: api.fetchFeedbackBrief,
    resolveFeedback: api.resolveFeedback,
    commentFeedback: api.commentFeedback,
  }
})

const cli = vi.hoisted(() => ({
  get: vi.fn(async (_scope: unknown, route: string): Promise<unknown> => {
    if (route === 'auth/whoami') return { owner: true, username: null, role: 'owner' }
    if (route === 'sessions/list-for-workspace') return cli.liveSessions
    if (route === 'sessions/lookup-by-agent') return { sessionAlive: false, sessionId: null }
    if (route === 'users') return { users: [] }
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

// The rail reuses the Agents page surfaces; stub the heavy leaves and
// record what the reused chrome hands them.
const seen = vi.hoisted(() => ({ terminal: [] as Array<Record<string, unknown>>, thread: [] as string[] }))
vi.mock('@/kessel-term/TerminalPane', async () => {
  const { useSessionViewChrome } = await import('@/components/SessionView/sessionViewChrome')
  return {
    TerminalPane: (props: Record<string, unknown>) => {
      const chrome = useSessionViewChrome()
      seen.terminal.push({ ...props, viewTab: chrome?.viewTab, splitLeft: chrome?.splitLeft, splitRight: chrome?.splitRight, overlayAddr: chrome?.overlayAddr })
      return <div data-testid="stub-terminal-pane" />
    },
  }
})
vi.mock('@/components/SessionView/ThreadOverlayPane', () => ({
  ThreadOverlayPane: ({ addr }: { addr: string }) => {
    seen.thread.push(addr)
    return <div data-testid="stub-thread-pane">{addr}</div>
  },
}))
vi.mock('@/lib/chat-session-tab', () => ({
  resolvePinnedChatCopyableAddress: vi.fn(async () => ({ clipboard: 'alpha', display: 'alpha' })),
}))

import { TicketBoard } from './TicketBoard'
import { FeedbackItemView, briefCache } from './FeedbackItemView'
import { readTicketBoardChrome, writeTicketBoardChrome } from '@/lib/ticket-board-chrome'
import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { useContextMenuStore } from '@/stores/context-menu'

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
  seen.thread.length = 0
})

afterEach(() => {
  cleanup()
  api.fetchFeedbackShow.mockReset()
  api.fetchFeedbackBrief.mockReset()
  api.commentFeedback.mockClear()
  briefCache.clear()
  useContextMenuStore.getState().close()
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

describe('ticket detail', () => {
  it('header: title, from agent → assignee, Expand and Open in window; history collapsed under a brief', async () => {
    detail(makeRow({ hasBrief: true, briefBytes: 120 }))
    await screen.findByTestId('brief-frame')
    expect(screen.getByTestId('ticket-detail-title').textContent).toBe('Which DB?')
    expect(screen.getByTestId('ticket-detail-byline').textContent).toMatch(/^from scout → assigned to Owner/)
    expect(screen.getByTestId('ticket-expand')).toBeTruthy()
    expect(screen.getByTestId('ticket-open-window')).toBeTruthy()
    const toggle = screen.getByTestId('ticket-history-toggle')
    expect(toggle.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByTestId('ticket-history')).toBeNull()
    fireEvent.click(toggle)
    expect(screen.getByTestId('ticket-history')).toBeTruthy()
    // Expand opens the brief overlay.
    fireEvent.click(screen.getByTestId('ticket-expand'))
    expect(screen.getByTestId('brief-overlay')).toBeTruthy()
  })

  it('the brief Options are quick answers in the chat rail; a pick answers, typed text starts a discussion', async () => {
    const row = makeRow({ hasBrief: true, briefBytes: 120, options: ['Yes', 'No'] })
    detail(row)
    const rail = await screen.findByTestId('ticket-agent-rail')
    await waitFor(() =>
      expect(within(rail).getAllByTestId('ticket-quick-answer').map((b) => b.textContent)).toEqual(['SQLite', 'Postgres']),
    )
    expect(within(rail).getByTestId('ticket-quick-answers').getAttribute('data-placement')).toBe('rail')
    // Not in the action bar while the rail is open.
    expect(within(screen.getByTestId('ticket-action-bar')).queryByTestId('ticket-quick-answer')).toBeNull()
    expect(screen.getAllByTestId('ticket-quick-answer')).toHaveLength(2)
    expect(within(rail).getAllByTestId('ticket-quick-answer')[0].getAttribute('title')).toBe('SQLite — simple')

    fireEvent.click(within(rail).getAllByTestId('ticket-quick-answer')[1])
    await waitFor(() =>
      expect(api.commentFeedback).toHaveBeenCalledWith(row.id, 'Postgres', { optionPick: true }),
    )

    // Answer / Resolve / Reassign stay in the action bar.
    const bar = screen.getByTestId('ticket-action-bar')
    expect(within(bar).getByTestId('ticket-action-resolve')).toBeTruthy()
    expect(within(bar).getByTestId('ticket-action-reassign')).toBeTruthy()
    fireEvent.click(within(bar).getByTestId('ticket-action-answer'))
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'what about both?' } })
    fireEvent.click(screen.getByTestId('ticket-answer-send'))
    await waitFor(() =>
      expect(api.commentFeedback).toHaveBeenCalledWith(row.id, 'what about both?', { optionPick: false }),
    )

    // Hiding the chat moves the picks back to the action bar; still a pick.
    fireEvent.click(screen.getByTestId('ticket-action-chat'))
    expect(screen.queryByTestId('ticket-agent-rail')).toBeNull()
    const barPicks = within(screen.getByTestId('ticket-action-bar')).getAllByTestId('ticket-quick-answer')
    expect(barPicks.map((b) => b.textContent)).toEqual(['SQLite', 'Postgres'])
    fireEvent.click(barPicks[0])
    await waitFor(() =>
      expect(api.commentFeedback).toHaveBeenCalledWith(row.id, 'SQLite', { optionPick: true }),
    )
  })

  it('the rail is open by default and is the Agents page Thread | Terminal view with its View menu', async () => {
    const row = makeRow({ sessionId: 'sess-1', sessionKind: 'sandbox' })
    cli.liveSessions = [{ sessionId: 'sess-1', agentName: 'tab-x', command: null, args: [], cwd: '/ws', isV2: true }]
    detail(row)
    render(<ContextMenu />)
    // Open without a click.
    const rail = await screen.findByTestId('ticket-agent-rail')
    expect(screen.getByTestId('ticket-action-chat').textContent).toBe('Hide chat')
    // The reused chrome: AgentSessionChrome's header + the shared View menu.
    const chrome = await within(rail).findByTestId('sidecar-session-chrome')
    expect(within(chrome).getByTestId('sidecar-session-title').textContent).toBe('scout')
    const menu = within(chrome).getByTestId('session-view-menu')
    await within(rail).findByTestId('stub-terminal-pane')
    await waitFor(() => expect(seen.terminal.at(-1)?.overlayAddr).toBe('alpha'))
    const last = seen.terminal.at(-1)!
    expect(last.attachAgentName).toBe('tab-x')
    expect(last.sessionId).toBe('sess-1')
    // One pane: the shared default view (Terminal), not a custom split.
    expect(menu.getAttribute('data-view')).toBe('terminal')
    expect(last.viewTab).toBe('terminal')

    // The switcher flips the same pane to the Thread.
    fireEvent.click(within(menu).getByTestId('session-view-button'))
    const contextMenu = document.querySelector('[data-context-menu]')
    if (!contextMenu) throw new Error('the View menu did not open')
    const threadRow = Array.from(contextMenu.querySelectorAll('button')).find((b) =>
      Array.from(b.querySelectorAll('span')).some((s) => s.textContent === 'Thread'),
    )
    if (!threadRow) throw new Error('no Thread row in the View menu')
    fireEvent.click(threadRow)
    await waitFor(() => expect(seen.terminal.at(-1)?.viewTab).toBe('thread'))
    expect(within(rail).getByTestId('session-view-menu').getAttribute('data-view')).toBe('thread')

    // Hide chat closes it and the window remembers.
    fireEvent.click(screen.getByTestId('ticket-action-chat'))
    expect(screen.queryByTestId('ticket-agent-rail')).toBeNull()
    expect(readTicketBoardChrome('window-test').chatOpen).toBe(false)
    expect(screen.getByTestId('ticket-action-chat').textContent).toBe('Chat with agent')
    cleanup()
    detail(row)
    await screen.findByTestId('ticket-action-bar')
    expect(screen.queryByTestId('ticket-agent-rail')).toBeNull()
    fireEvent.click(screen.getByTestId('ticket-action-chat'))
    expect(await screen.findByTestId('ticket-agent-rail')).toBeTruthy()
    expect(readTicketBoardChrome('window-test').chatOpen).toBe(true)
  })

  it('no terminal to show: the rail shows the Thread only, with a note', async () => {
    detail(makeRow({ sessionId: null, sessionKind: null }))
    await screen.findByTestId('ticket-action-bar')
    const rail = await screen.findByTestId('ticket-agent-rail')
    expect(within(rail).getByTestId('ticket-agent-rail-thread-only')).toBeTruthy()
    expect(within(rail).getByTestId('ticket-agent-rail-note').textContent).toMatch(/no terminal to show/)
    await within(rail).findByTestId('stub-thread-pane')
    expect(within(rail).queryByTestId('stub-terminal-pane')).toBeNull()
  })
})
