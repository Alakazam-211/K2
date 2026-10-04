// @vitest-environment jsdom
// prd-ticket-html-brief-v1 T11 + H39: the HTML badge shows only for tickets
// with a brief; a ticket without one renders the body box exactly as before
// and never asks for a brief; a ticket with one fetches it ONCE, and thread
// refreshes (revision bumps) neither refetch it nor rebuild the frame.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, waitFor } from '@testing-library/react'
import { FeedbackCard, cardAssigneeNames } from './FeedbackPage'
import { FeedbackItemView, briefCache } from './FeedbackItemView'
import type { FeedbackBrief, FeedbackListRow, FeedbackShow } from './feedback-api'

const api = vi.hoisted(() => ({
  fetchFeedbackShow: vi.fn(),
  fetchFeedbackBrief: vi.fn(),
  resolveFeedback: vi.fn(async () => {}),
  commentFeedback: vi.fn(async () => ({ ok: true, id: 'x' })),
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

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => {}) }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ users: [] })),
  daemonCliGetText: vi.fn(async () => ''),
  daemonCliPost: vi.fn(async () => ({})),
  RecoveringError: class RecoveringError extends Error {},
}))

const row: FeedbackListRow = {
  id: 'b7c1d2e3-0000-4000-8000-000000000001',
  projectId: 'p1',
  sessionId: null,
  sessionKind: null,
  agentName: 'scout',
  kind: 'approval',
  title: 'Deploy blocked: DNS?',
  body: 'Short summary.',
  options: null,
  priority: 2,
  status: 'waiting',
  answer: null,
  createdAt: 1_759_241_100,
  updatedAt: 1_759_241_100,
  answeredAt: null,
  commentCount: 1,
  assignees: [],
  projectPath: '/ws',
  projectName: 'Alpha',
  linked: true,
}

const brief: FeedbackBrief = {
  html: '<h2>Problem</h2><p>See <a href="https://example.com/x">x</a>.</p>',
  text: 'Problem See x.',
  bytes: 64,
  sha256: 'deadbeef',
  sanitizer: 'k2-brief-v1',
  createdAt: 1_759_241_100,
}

function show(r: FeedbackListRow): FeedbackShow {
  return {
    ...r,
    workspace: r.projectName,
    projectPath: r.projectPath,
    canonicalSessionId: null,
    comments: [{ author: 'scout', body: `${r.title}\n${r.body}`, at: r.createdAt }],
  }
}

afterEach(() => {
  cleanup()
  api.fetchFeedbackShow.mockReset()
  api.fetchFeedbackBrief.mockReset()
  briefCache.clear()
})

describe('FeedbackCard — HTML badge (T11)', () => {
  const card = (r: FeedbackListRow): void => {
    render(
      <FeedbackCard
        row={r}
        workspace={undefined}
        nowSec={1_759_241_200}
        selected={false}
        onSelect={vi.fn()}
        onMutated={vi.fn()}
      />,
    )
  }

  it('shows the badge only when hasBrief', () => {
    card({ ...row, hasBrief: true, briefBytes: 64 })
    expect(screen.getByTestId('html-badge').textContent).toBe('HTML')
    cleanup()
    card({ ...row, hasBrief: false, briefBytes: null })
    expect(screen.queryByTestId('html-badge')).toBeNull()
    cleanup()
    card(row) // an older daemon: no field at all
    expect(screen.queryByTestId('html-badge')).toBeNull()
  })

  it('a card without a brief keeps its exact title markup', () => {
    card(row)
    const title = screen.getByText('Deploy blocked: DNS?')
    expect(title.tagName).toBe('P')
    expect(title.innerHTML).toBe('Deploy blocked: DNS?')
  })
})

describe('FeedbackCard — assignee on the bottom row', () => {
  const card = (r: FeedbackListRow): HTMLElement => {
    const { container } = render(
      <FeedbackCard
        row={r}
        workspace={undefined}
        nowSec={1_759_241_200}
        selected={false}
        onSelect={vi.fn()}
        onMutated={vi.fn()}
      />,
    )
    return container
  }

  it('shows the assigned person as initials on the bottom row, no agent name', () => {
    card({ ...row, assignees: ['julie'], hasBrief: true, briefBytes: 64 })
    const a = screen.getByTestId('card-assignee')
    expect(a.getAttribute('data-unassigned')).toBeNull()
    expect(screen.getAllByTestId('card-assignee-initials').map((e) => e.textContent)).toEqual(['J'])
    expect(a.getAttribute('title')).toBe('Assigned to julie')
    // Bottom row: status chip, initials, age, HTML mark — and no agent.
    const bottom = screen.getByTestId('card-bottom-row')
    expect(bottom.contains(a)).toBe(true)
    expect(bottom.contains(screen.getByTestId('card-status'))).toBe(true)
    expect(screen.getByTestId('card-status').textContent).toBe('Waiting')
    expect(bottom.contains(screen.getByTestId('html-badge'))).toBe(true)
    expect(screen.getByTestId('card-age').textContent).toBe('2m ago')
    const card0 = screen.getByTestId('ticket-card')
    expect(card0.textContent).not.toContain('scout')
    expect(card0.textContent).not.toContain('Alpha')
    // Title, then the one-line summary (the --body).
    expect(screen.getByTestId('card-title').textContent).toBe('Deploy blocked: DNS?')
    expect(screen.getByTestId('card-title').className).toContain('truncate')
    expect(screen.getByTestId('card-summary').textContent).toBe('Short summary.')
    expect(screen.getByTestId('card-summary').className).toContain('truncate')
  })

  it('the wire owner reads Owner; several people are initials', () => {
    card({ ...row, assignees: ['owner', 'julie', 'owner', ' '] })
    expect(screen.getAllByTestId('card-assignee-initials').map((e) => e.textContent)).toEqual(['O', 'J'])
    expect(screen.getByTestId('card-assignee').getAttribute('title')).toBe('Assigned to Owner, julie')
  })

  it('shows a subtle Unassigned when nobody is assigned', () => {
    card({ ...row, assignees: [] })
    const a = screen.getByTestId('card-assignee')
    expect(a.textContent).toBe('Unassigned')
    expect(a.getAttribute('data-unassigned')).toBe('true')
    expect(a.className).toContain('opacity-60')
    expect(screen.queryByTestId('card-assignee-initial')).toBeNull()
  })

  it('names helper: dedup, blanks dropped, owner display', () => {
    expect(cardAssigneeNames(undefined)).toEqual([])
    expect(cardAssigneeNames(['', '  '])).toEqual([])
    expect(cardAssigneeNames(['owner', 'bob', 'bob'])).toEqual(['Owner', 'bob'])
  })
})

describe('FeedbackItemView — brief', () => {
  it('without a brief: the body box is unchanged and no brief is fetched', async () => {
    api.fetchFeedbackShow.mockResolvedValue(show(row))
    const { rerender } = render(
      <FeedbackItemView id={row.id} listRow={row} nowSec={1_759_241_200} revision={0} onMutated={vi.fn()} />,
    )
    const body = await screen.findByText('Short summary.')
    const box = body.closest('div.mb-3')
    expect(box).not.toBeNull()
    expect(box!.className).toBe('mb-3 px-3 py-2 bg-white/[0.03] border border-[var(--color-border)]')
    expect(screen.queryByTestId('brief')).toBeNull()
    expect(screen.queryByTestId('brief-loading')).toBeNull()
    expect(screen.queryByTestId('html-badge')).toBeNull()
    rerender(
      <FeedbackItemView id={row.id} listRow={row} nowSec={1_759_241_200} revision={1} onMutated={vi.fn()} />,
    )
    await waitFor(() => expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(2))
    expect(api.fetchFeedbackBrief).not.toHaveBeenCalled()
  })

  it('with a brief: summary, then the frame; fetched once across revision bumps (H39)', async () => {
    const withBrief: FeedbackListRow = { ...row, hasBrief: true, briefBytes: 64 }
    api.fetchFeedbackShow.mockImplementation(async () => show(withBrief))
    api.fetchFeedbackBrief.mockResolvedValue(brief)
    const props = { id: withBrief.id, listRow: withBrief, nowSec: 1_759_241_200, onMutated: vi.fn() }
    const { rerender } = render(<FeedbackItemView {...props} revision={0} />)

    const frame = await screen.findByTestId('brief-frame')
    expect(frame.getAttribute('sandbox')).toBe('')
    const firstDoc = frame.getAttribute('srcdoc')
    expect(firstDoc).toContain('<body class="k2-brief">')
    expect(screen.getByText('Short summary.')).toBeTruthy()
    // The summary block comes before the brief in document order.
    const summary = screen.getByText('Short summary.')
    expect(summary.compareDocumentPosition(frame) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(screen.getAllByTestId('html-badge').length).toBe(1)

    rerender(<FeedbackItemView {...props} revision={1} />)
    await waitFor(() => expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(2))
    rerender(<FeedbackItemView {...props} revision={2} />)
    await waitFor(() => expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(3))

    expect(api.fetchFeedbackBrief).toHaveBeenCalledTimes(1)
    expect(api.fetchFeedbackBrief).toHaveBeenCalledWith(withBrief.id)
    const after = screen.getByTestId('brief-frame')
    expect(after).toBe(frame) // same element: the frame was never remounted
    expect(after.getAttribute('srcdoc')).toBe(firstDoc)
  })

  it('a failed brief fetch shows an error, not a blank frame', async () => {
    const withBrief: FeedbackListRow = { ...row, hasBrief: true, briefBytes: 64 }
    api.fetchFeedbackShow.mockResolvedValue(show(withBrief))
    api.fetchFeedbackBrief.mockRejectedValue(new Error('boom'))
    render(
      <FeedbackItemView id={withBrief.id} listRow={withBrief} nowSec={1_759_241_200} revision={0} onMutated={vi.fn()} />,
    )
    const err = await screen.findByTestId('brief-error')
    expect(err.textContent).toContain('boom')
    expect(screen.queryByTestId('brief-frame')).toBeNull()
  })
})
