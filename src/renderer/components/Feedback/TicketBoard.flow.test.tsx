// @vitest-environment jsdom
// The Tickets board's flow: pick a ticket in the list, read its report in
// the middle, discuss it on the right. These pin the behaviour that makes
// that flow work everywhere:
//   - the list is a keyboard listbox: one tab stop, ↑/↓ move AND open the
//     next ticket in the report, Enter opens, focus stays in the list;
//   - a failed load is never also "No tickets yet";
//   - the width rules: the report gets the most room, the list folds to its
//     rail by itself when narrow, and narrower still the discussion is a
//     drawer with a tab;
//   - closed and planned cards are not dimmed;
//   - the no-tickets hint shows the command as code, not backticks.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import type { FeedbackListRow, FeedbackShow } from './feedback-api'

const api = vi.hoisted(() => ({
  fetchFeedbackShow: vi.fn(),
  fetchFeedbackBrief: vi.fn(),
  resolveFeedback: vi.fn(async (..._args: unknown[]) => {}),
  commentFeedback: vi.fn(async (..._args: unknown[]) => ({ ok: true, id: 'x', delivered: true })),
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

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string): Promise<unknown> => {
    if (route === 'auth/whoami') return { owner: true, username: null, role: 'owner' }
    if (route === 'sessions/list-for-workspace') return []
    if (route === 'sessions/lookup-by-agent') return { sessionAlive: false, sessionId: null }
    return {}
  }),
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
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))

// jsdom has no layout: the board's measured width is set by each test.
const measured = vi.hoisted(() => ({ width: 0 }))
vi.mock('@/hooks/useElementWidth', () => ({
  useElementWidth: (el: HTMLElement | null) => (el ? measured.width : 0),
}))

import { TicketBoard } from './TicketBoard'
import { briefCache } from './FeedbackItemView'
import { writeTicketOpenOn } from '@/lib/ticket-open-pref'

const NOW = 1_759_241_200

function makeRow(over: Partial<FeedbackListRow>): FeedbackListRow {
  return {
    id: 'aaaaaaaa-0000-4000-8000-000000000001',
    projectId: 'p1',
    sessionId: null,
    sessionKind: null,
    agentName: 'ada',
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
    comments: [{ author: 'ada', body: r.title, at: r.createdAt }],
  }
}

const first = makeRow({ id: 'aaaaaaaa-0000-4000-8000-00000000000a', title: 'First ticket' })
const second = makeRow({ id: 'bbbbbbbb-0000-4000-8000-00000000000b', title: 'Second ticket' })
const third = makeRow({ id: 'cccccccc-0000-4000-8000-00000000000c', title: 'Third ticket', status: 'answered' })
const closed = makeRow({ id: 'dddddddd-0000-4000-8000-00000000000d', title: 'Closed ticket', status: 'resolved' })
const planned = makeRow({ id: 'eeeeeeee-0000-4000-8000-00000000000e', title: 'Planned ticket', status: 'planned' })

beforeEach(() => {
  localStorage.clear()
  measured.width = 0
  writeTicketOpenOn('all')
})

afterEach(() => {
  cleanup()
  api.fetchFeedbackShow.mockReset()
  api.fetchFeedbackBrief.mockReset()
  briefCache.clear()
})

function board(
  rows: FeedbackListRow[] | null = [first, second, third],
  props: Partial<React.ComponentProps<typeof TicketBoard>> = {},
): ReturnType<typeof render> {
  api.fetchFeedbackShow.mockImplementation(async (id: string) =>
    show((rows ?? []).find((r) => r.id === id) ?? first),
  )
  return render(<TicketBoard rows={rows} error={null} revision={0} onMutated={vi.fn()} {...props} />)
}

const card = (id: string): HTMLElement =>
  screen.getAllByTestId('ticket-card').find((c) => c.getAttribute('data-ticket-id') === id)!

describe('the list from the keyboard', () => {
  it('is a listbox of options with one tab stop', async () => {
    board()
    const list = await screen.findByRole('listbox', { name: 'Tickets' })
    const options = within(list).getAllByRole('option')
    expect(options).toHaveLength(3)
    expect(options.filter((o) => o.tabIndex === 0)).toHaveLength(1)
    expect(options[0].tabIndex).toBe(0)
    // The status chip is a pointer shortcut, out of the tab order.
    for (const chip of screen.getAllByTestId('card-status')) expect(chip.tabIndex).toBe(-1)
  })

  it('Enter opens the card in focus in the report', async () => {
    board()
    const c = card(first.id)
    c.focus()
    fireEvent.keyDown(c, { key: 'Enter' })
    await screen.findByTestId('ticket-detail')
    expect(c.getAttribute('aria-selected')).toBe('true')
    expect(screen.getByTestId('ticket-detail-title').textContent).toBe('First ticket')
  })

  it('↓ opens the next ticket straight into the report, and the keys stay in the list', async () => {
    board()
    const c = card(first.id)
    c.focus()
    fireEvent.keyDown(c, { key: 'Enter' })
    await screen.findByTestId('ticket-rail-thread')
    fireEvent.keyDown(card(first.id), { key: 'ArrowDown' })
    await waitFor(() => expect(screen.getByTestId('ticket-detail-title').textContent).toBe('Second ticket'))
    expect(card(second.id).getAttribute('aria-selected')).toBe('true')
    expect(document.activeElement).toBe(card(second.id))
    // The roving stop follows.
    expect(card(second.id).tabIndex).toBe(0)
    expect(card(first.id).tabIndex).toBe(-1)
    // The reply box doesn't take the focus from the list.
    await screen.findByTestId('ticket-rail-thread')
    await new Promise((r) => setTimeout(r, 80))
    expect(document.activeElement).toBe(card(second.id))
    fireEvent.keyDown(card(second.id), { key: 'ArrowDown' })
    await waitFor(() => expect(screen.getByTestId('ticket-detail-title').textContent).toBe('Third ticket'))
    fireEvent.keyDown(card(third.id), { key: 'Home' })
    await waitFor(() => expect(screen.getByTestId('ticket-detail-title').textContent).toBe('First ticket'))
  })
})

describe('a failed load', () => {
  it('says it couldn’t load, with Try again, and never also "No tickets yet"', async () => {
    const onRetry = vi.fn(async () => {})
    board([], { error: 'the tickets database is locked', onRetry })
    const err = await screen.findByTestId('ticket-list-error')
    expect(err.textContent).toContain('Couldn’t load tickets')
    expect(err.textContent).toContain('the tickets database is locked')
    expect(screen.queryByText('No tickets yet')).toBeNull()
    expect(screen.queryByTestId('ticket-list-empty')).toBeNull()
    // The tabs don't claim 0 either.
    expect(screen.getByTestId('ticket-scope-all').textContent).toBe('All')
    fireEvent.click(within(err).getByRole('button', { name: /Try again/ }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it('keeps the last good list on screen under a slim line', async () => {
    board([first, second], { error: 'timed out' })
    expect(await screen.findByTestId('ticket-list-stale')).toBeTruthy()
    expect(screen.getAllByTestId('ticket-card')).toHaveLength(2)
    expect(screen.queryByTestId('ticket-list-error')).toBeNull()
  })

  it('a list that loaded empty says so, with the command as code', async () => {
    board([])
    const empty = await screen.findByTestId('ticket-list-empty')
    expect(empty.textContent).toContain('No tickets yet')
    const hint = screen.getByTestId('ticket-list-empty-hint')
    expect(hint.textContent).not.toContain('`')
    expect(hint.querySelector('code')?.textContent).toBe('k2 tickets ask')
  })
})

describe('closed and planned cards', () => {
  it('are not dimmed', async () => {
    board([first, closed, planned])
    await screen.findAllByTestId('ticket-card')
    for (const c of screen.getAllByTestId('ticket-card')) {
      expect(c.className).not.toMatch(/\bopacity-/)
    }
  })
})

describe('the width rules', () => {
  async function openAt(width: number): Promise<void> {
    measured.width = width
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-detail')
  }

  it('at 1280 the full list, the report and the discussion are all on screen', async () => {
    await openAt(1264)
    const b = screen.getByTestId('ticket-board')
    expect(b.getAttribute('data-list-mode')).toBe('open')
    expect(b.getAttribute('data-discussion-mode')).toBe('side')
    expect(screen.getByTestId('ticket-list').style.width).toBe('400px')
    const rail = await screen.findByTestId('ticket-agent-rail')
    expect(parseInt(rail.style.width, 10)).toBeLessThan(1264 - 400 - parseInt(rail.style.width, 10))
  })

  it('at 900 the list narrows but stays a list', async () => {
    await openAt(900)
    expect(screen.getByTestId('ticket-board').getAttribute('data-list-mode')).toBe('open')
    expect(parseInt(screen.getByTestId('ticket-list').style.width, 10)).toBeLessThan(400)
    expect(screen.getByTestId('ticket-agent-rail')).toBeTruthy()
  })

  it('at 800 the list folds to its rail by itself; » shows it over the report, and a pick closes it', async () => {
    await openAt(800)
    expect(screen.getByTestId('ticket-board').getAttribute('data-list-mode')).toBe('rail')
    expect(screen.getByTestId('ticket-list-rail').hasAttribute('data-auto-folded')).toBe(true)
    expect(screen.getByTestId('ticket-agent-rail')).toBeTruthy()
    fireEvent.click(screen.getByTestId('ticket-list-expand'))
    const overlay = await screen.findByTestId('ticket-list-overlay')
    fireEvent.click(within(overlay).getAllByTestId('ticket-card')[1])
    await waitFor(() => expect(screen.queryByTestId('ticket-list-overlay')).toBeNull())
    await waitFor(() => expect(screen.getByTestId('ticket-detail-title').textContent).toBe('Second ticket'))
  })

  it('narrower still, the discussion is a drawer behind a tab', async () => {
    await openAt(620)
    expect(screen.getByTestId('ticket-board').getAttribute('data-discussion-mode')).toBe('drawer')
    expect(screen.queryByTestId('ticket-agent-rail')).toBeNull()
    fireEvent.click(await screen.findByTestId('ticket-discussion-tab'))
    expect(await screen.findByTestId('ticket-agent-rail')).toBeTruthy()
    fireEvent.click(screen.getByTestId('ticket-discussion-hide'))
    await waitFor(() => expect(screen.queryByTestId('ticket-agent-rail')).toBeNull())
  })
})
