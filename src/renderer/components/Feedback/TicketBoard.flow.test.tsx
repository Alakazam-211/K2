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

import { TicketBoard, ticketMiddleState } from './TicketBoard'
import { briefCache, DISCUSSION_TAB_GUTTER } from './FeedbackItemView'
import { writeTicketOpenOn } from '@/lib/ticket-open-pref'
import { primaryScope } from '@/kessel/server-scope'

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

// The board's discussion here is the column of a server without
// `ticket-thread-v1` (its Thread | Agent tabs and its own box);
// TicketThreadPane.test covers the Thread pane.
let ticketThreadOff: { mockRestore: () => void } | null = null

beforeEach(() => {
  localStorage.clear()
  measured.width = 0
  {
    const scope = primaryScope()
    const supports = scope.serverSupports.bind(scope)
    ticketThreadOff = vi.spyOn(scope, 'serverSupports').mockImplementation((f) => f !== 'ticket-thread-v1' && supports(f))
  }
  writeTicketOpenOn('all')
})

afterEach(() => {
  cleanup()
  ticketThreadOff?.mockRestore()
  ticketThreadOff = null
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
    expect(screen.getByTestId('ticket-scope-all').textContent?.trim()).toBe('All')
    fireEvent.click(within(err).getByRole('button', { name: /Try again/ }))
    expect(onRetry).toHaveBeenCalledTimes(1)
  })

  it('keeps the last good list on screen under a slim line', async () => {
    board([first, second], { error: 'timed out' })
    expect(await screen.findByTestId('ticket-list-stale')).toBeTruthy()
    expect(screen.getAllByTestId('ticket-card')).toHaveLength(2)
    expect(screen.queryByTestId('ticket-list-error')).toBeNull()
  })

  it('a list that loaded empty says so in plain words, and so does the middle', async () => {
    board([])
    const empty = await screen.findByTestId('ticket-list-empty')
    expect(empty.textContent).toContain('No tickets yet')
    const hint = screen.getByTestId('ticket-list-empty-hint')
    expect(hint.textContent).toBe('Agents ask you things here. Each question shows up in this list.')
    expect(hint.querySelector('code')).toBeNull()
    // The middle never asks for a pick the list can't give.
    expect(screen.getByTestId('ticket-middle-line').textContent).toBe('When an agent asks you something, its report opens here.')
    expect(screen.queryByTestId('ticket-start-oldest')).toBeNull()
  })

  it('a search with no match says so once, in the list, with Clear search under it', async () => {
    board()
    fireEvent.change(screen.getByLabelText('Search tickets'), { target: { value: 'zzz nothing' } })
    const empty = screen.getByTestId('ticket-list-empty')
    expect(empty.textContent).toContain('No tickets match your search')
    expect(within(empty).getByTestId('ticket-clear-search').textContent).toBe('Clear search')
    // The middle stays quiet: no second message, no oldest waiting ticket.
    expect(screen.queryByTestId('ticket-middle-line')).toBeNull()
    expect(screen.queryByTestId('ticket-middle-clear-search')).toBeNull()
    expect(screen.queryByTestId('ticket-start-oldest')).toBeNull()
    fireEvent.click(screen.getByTestId('ticket-clear-search'))
    expect((screen.getByLabelText('Search tickets') as HTMLInputElement).value).toBe('')
    await waitFor(() => expect(screen.getAllByTestId('ticket-card').length).toBeGreaterThan(0))
  })

  it('with the list folded away, the middle says a search found nothing', () => {
    const base = { loading: false, failed: false, noTickets: false, noMatch: true, emptyTab: true }
    expect(ticketMiddleState(base)).toEqual({ state: 'no-match', line: null })
    expect(ticketMiddleState({ ...base, listFolded: true })).toEqual({ state: 'no-match', line: 'No tickets match your search.' })
  })

  it('while the list loads, no tab claims 0 and the middle asks for nothing', () => {
    render(<TicketBoard rows={null} error={null} revision={0} onMutated={vi.fn()} />)
    expect(document.querySelectorAll('[data-scope-count]')).toHaveLength(0)
    expect(screen.queryByTestId('ticket-middle-line')).toBeNull()
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
    // The tab stands in a gutter the report keeps clear, never on its box.
    const reportScroll = (): HTMLElement =>
      screen.getByTestId('ticket-report-scroll').querySelector<HTMLElement>('.overflow-y-auto') as HTMLElement
    expect(reportScroll().className).toContain(DISCUSSION_TAB_GUTTER)
    fireEvent.click(await screen.findByTestId('ticket-discussion-tab'))
    expect(await screen.findByTestId('ticket-agent-rail')).toBeTruthy()
    expect(reportScroll().className).not.toContain(DISCUSSION_TAB_GUTTER)
    fireEvent.click(screen.getByTestId('ticket-discussion-hide'))
    await waitFor(() => expect(screen.queryByTestId('ticket-agent-rail')).toBeNull())
  })
})

describe('three panes', () => {
  async function openIt(): Promise<void> {
    measured.width = 1264
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-rail-thread')
  }

  it('the list, the report and the discussion are three tiles with the pane gap', async () => {
    await openIt()
    expect(screen.getByTestId('ticket-board').className).toContain('gap-[var(--gap-pane)]')
    expect(screen.getByTestId('ticket-detail-wrap').className).toContain('gap-[var(--gap-pane)]')
    const list = screen.getByTestId('ticket-list')
    expect(list.hasAttribute('data-nav-tile')).toBe(true)
    expect(list.getAttribute('data-material')).toBe('tile')
    expect(screen.getByTestId('ticket-detail').getAttribute('data-material')).toBe('tile')
    expect(screen.getByTestId('ticket-agent-rail').getAttribute('data-material')).toBe('tile')
  })

  it('the three top rows share one height', async () => {
    await openIt()
    const heights = [
      screen.getByTestId('ticket-list').querySelector<HTMLElement>('[data-ticket-list-header]')!.style.height,
      screen.getByTestId('ticket-detail-title').parentElement!.style.height,
      screen.getByTestId('ticket-agent-rail').querySelector<HTMLElement>('[data-ticket-discussion-header]')!.style.height,
    ]
    expect(heights[0]).toMatch(/48px/)
    expect(new Set(heights).size).toBe(1)
  })

  it('the folded list is a tile of its own, its top row the same height', async () => {
    localStorage.setItem('k2.windowChrome.tickets.window-test', JSON.stringify({ collapsed: true }))
    measured.width = 1264
    board()
    fireEvent.click(screen.getAllByTestId('ticket-rail-item')[0])
    await screen.findByTestId('ticket-rail-thread')
    const rail = screen.getByTestId('ticket-list-rail')
    expect(rail.getAttribute('data-material')).toBe('tile')
    expect(rail.querySelector<HTMLElement>('[data-rail-top-row]')!.style.height).toMatch(/48px/)
  })

  it('both edges are the shared resize edge, moved by the keyboard', async () => {
    await openIt()
    const listEdge = screen.getByTestId('ticket-list-resize')
    expect(listEdge.getAttribute('role')).toBe('separator')
    expect(listEdge.tabIndex).toBe(0)
    expect(listEdge.getAttribute('aria-valuenow')).toBe('400')
    fireEvent.keyDown(listEdge, { key: 'ArrowLeft' })
    await waitFor(() => expect(screen.getByTestId('ticket-list').style.width).toBe('384px'))

    const talkEdge = screen.getByTestId('ticket-discussion-resize')
    const before = parseInt(screen.getByTestId('ticket-agent-rail').style.width, 10)
    expect(talkEdge.getAttribute('aria-valuenow')).toBe(String(before))
    // The discussion is on the edge's right: ← widens it.
    fireEvent.keyDown(talkEdge, { key: 'ArrowLeft' })
    await waitFor(() =>
      expect(parseInt(screen.getByTestId('ticket-agent-rail').style.width, 10)).toBe(before + 16),
    )
    expect(JSON.parse(localStorage.getItem('k2.windowChrome.tickets.window-test') ?? '{}').discussionWidth).toBe(
      before + 16,
    )
  })
})

describe('shared controls', () => {
  it('the tabs are the shared segmented control, Waiting on me first; the arrows choose', async () => {
    board()
    const group = screen.getByRole('radiogroup', { name: 'Which tickets' })
    const radios = within(group).getAllByRole('radio')
    expect(radios.map((r) => r.getAttribute('data-testid'))).toEqual([
      'ticket-scope-waiting',
      'ticket-scope-mine',
      'ticket-scope-all',
    ])
    // Opened on All (this file's pref); ← goes to Mine.
    expect(screen.getByTestId('ticket-scope-all').getAttribute('aria-checked')).toBe('true')
    fireEvent.keyDown(screen.getByTestId('ticket-scope-all'), { key: 'ArrowLeft' })
    expect(screen.getByTestId('ticket-scope-mine').getAttribute('aria-checked')).toBe('true')
  })

  it('search is the shared search field; Esc clears it and keeps the open ticket', async () => {
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-detail')
    const search = screen.getByLabelText('Search tickets')
    expect(search.hasAttribute('data-k2-search')).toBe(true)
    fireEvent.change(search, { target: { value: 'second' } })
    fireEvent.keyDown(search, { key: 'Escape' })
    expect((search as HTMLInputElement).value).toBe('')
    expect(screen.getByTestId('ticket-detail')).toBeTruthy()
  })

  it('a card is a raised, cut surface; the selected one has one fill and no ring', async () => {
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-detail')
    const c = card(first.id)
    expect(c.getAttribute('data-material')).toBe('raised')
    expect(c.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(c.className).not.toMatch(/(^|\s)ring-/)
  })

  it('the header status menu takes the focus, and the arrows move through it', async () => {
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-rail-thread')
    // Let the reply box take its opening focus first.
    await new Promise((r) => setTimeout(r, 100))
    fireEvent.click(screen.getByTestId('ticket-status'))
    const menu = screen.getByTestId('ticket-header-status-menu')
    expect(menu.getAttribute('role')).toBe('menu')
    await waitFor(() => expect(document.activeElement).toBe(within(menu).getByTestId('ticket-status-option-waiting')))
    fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(within(menu).getByTestId('ticket-status-option-needs_discussion'))
    fireEvent.keyDown(document.activeElement!, { key: 'End' })
    expect(document.activeElement).toBe(within(menu).getByTestId('ticket-status-option-dismissed'))
    fireEvent.keyDown(document.activeElement!, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('ticket-header-status-menu')).toBeNull())
    // The ticket stays open.
    expect(screen.getByTestId('ticket-detail')).toBeTruthy()
  })

  it('the header controls are glyph buttons; a hidden discussion comes back from the report’s header', async () => {
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-rail-thread')
    expect(screen.queryByText('Hide chat')).toBeNull()
    fireEvent.click(screen.getByTestId('ticket-chat-toggle'))
    await waitFor(() => expect(screen.queryByTestId('ticket-agent-rail')).toBeNull())
    const show = screen.getByTestId('ticket-chat-toggle')
    expect(screen.getByTestId('ticket-detail-header').contains(show)).toBe(true)
    fireEvent.click(show)
    expect(await screen.findByTestId('ticket-agent-rail')).toBeTruthy()
  })
})

describe('words and the flow’s loop', () => {
  it('nothing open: says what the middle is for, and starts with the oldest waiting ticket', async () => {
    const older = makeRow({ id: 'ffffffff-0000-4000-8000-00000000000f', title: 'Older ticket', createdAt: NOW - 9000 })
    board([first, older, second])
    expect(screen.getByText('Pick a ticket to read its report and answer the agent.')).toBeTruthy()
    // Once the board knows who "me" is.
    fireEvent.click(await screen.findByTestId('ticket-start-oldest'))
    await waitFor(() => expect(screen.getByTestId('ticket-detail-title').textContent).toBe('Older ticket'))
  })

  it('a settled ticket offers the next waiting one', async () => {
    board([first, second, third])
    fireEvent.click(card(third.id))
    const next = await screen.findByTestId('ticket-next-waiting')
    expect(next.textContent).toContain('First ticket')
    fireEvent.click(screen.getByTestId('ticket-next-waiting-open'))
    await waitFor(() => expect(screen.getByTestId('ticket-detail-title').textContent).toBe('First ticket'))
    // An open ticket offers none.
    expect(screen.queryByTestId('ticket-next-waiting')).toBeNull()
  })

  it('the discussion shows the status and ends it with Resolve', async () => {
    board()
    fireEvent.click(card(first.id))
    const status = await screen.findByTestId('ticket-discussion-status')
    expect(status.textContent).toContain('Waiting')
    fireEvent.click(within(status).getByTestId('ticket-discussion-resolve'))
    await waitFor(() => expect(api.resolveFeedback).toHaveBeenCalledWith(first.id, 'resolved'))
  })

  it('plain words: Report, Urgent, who asked whom, and no P3', async () => {
    const urgent = makeRow({ id: 'abababab-0000-4000-8000-0000000000ab', title: 'Urgent one', priority: 1, hasBrief: true })
    api.fetchFeedbackBrief.mockResolvedValue(null)
    board([urgent, first])
    expect(within(card(urgent.id)).getByTestId('html-badge').textContent).toBe('Report')
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-rail-thread')
    // Priority 3 is the everyday level: nothing shown.
    expect(screen.queryByTestId('ticket-priority')).toBeNull()
    // The person looking is "you".
    expect(screen.getByTestId('ticket-detail-byline').textContent).toMatch(/^ada asked you/)
    fireEvent.click(card(urgent.id))
    await waitFor(() => expect(screen.getByTestId('ticket-priority').textContent).toBe('Urgent'))
  })

  it('a report that fails to load says so, with Try again', async () => {
    const withBrief = makeRow({ id: 'cdcdcdcd-0000-4000-8000-0000000000cd', title: 'Has a report', hasBrief: true })
    api.fetchFeedbackBrief.mockRejectedValueOnce(new Error('timed out'))
    board([withBrief])
    fireEvent.click(card(withBrief.id))
    const err = await screen.findByTestId('brief-error')
    expect(err.textContent).toContain('This ticket’s report couldn’t be loaded.')
    api.fetchFeedbackBrief.mockResolvedValueOnce(null)
    fireEvent.click(within(err).getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(api.fetchFeedbackBrief).toHaveBeenCalledTimes(2))
  })

  it('the ticket files use the app’s type roles, not raw pixel sizes', async () => {
    const { readFileSync } = await import('node:fs')
    for (const f of ['TicketBoard.tsx', 'TicketCard.tsx', 'TicketListBody.tsx', 'FeedbackItemView.tsx', 'TicketAgentRail.tsx', 'badges.tsx', 'TicketWindow.tsx', 'BriefFrame.tsx']) {
      const src = readFileSync(`${process.cwd()}/src/renderer/components/Feedback/${f}`, 'utf8')
      expect(src.match(/text-\[\d+px\]|\btext-(xs|sm|base)\b/g), f).toBeNull()
    }
  })
})

describe('after-shot fixes', () => {
  it('a card’s title keeps its ellipsis when clicked (the caret never lands in it)', async () => {
    board()
    fireEvent.click(card(second.id))
    await screen.findByTestId('ticket-detail')
    for (const t of screen.getAllByTestId('card-title')) expect(t.className).toContain('select-none')
  })

  it('the list and the report fade with the heavier tint, into their tile’s fill', async () => {
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-detail')
    expect(screen.getByTestId('ticket-list-scroll').hasAttribute('data-edge-heavy')).toBe(true)
    expect(screen.getByTestId('ticket-report-scroll').hasAttribute('data-edge-heavy')).toBe(true)
  })

  it('the list’s fade only tints: blurring the raised cards left a lighter band with a hard top edge', async () => {
    board()
    expect((await screen.findByTestId('ticket-list-scroll')).hasAttribute('data-edge-noblur')).toBe(true)
  })

  it('a narrow card’s bottom row wraps its right-hand group instead of squeezing the status under the people', async () => {
    board()
    const row = (await screen.findAllByTestId('card-bottom-row'))[0]
    expect(row.className).toContain('flex-wrap')
    const status = within(row).getByTestId('card-status')
    const left = row.firstElementChild as HTMLElement
    expect(left.contains(status)).toBe(true)
    expect(left.contains(within(row).getByTestId('card-assignee'))).toBe(true)
    expect(left.className).toContain('flex-shrink-0')
    const right = row.lastElementChild as HTMLElement
    expect(right.contains(within(row).getByTestId('card-age'))).toBe(true)
    expect(right.className).toContain('ml-auto')
  })

  it('the byline wraps in a narrow report', async () => {
    board()
    fireEvent.click(card(first.id))
    await screen.findByTestId('ticket-detail')
    const byline = screen.getByTestId('ticket-detail-byline')
    expect(byline.className).toContain('flex-wrap')
    expect(screen.getByTestId('ticket-detail-status-line').className).toContain('flex-wrap')
  })
})
