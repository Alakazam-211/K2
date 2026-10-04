// @vitest-environment jsdom
// prd-tickets-badge-orphans TB18: a ticket whose workspace was removed has
// no agent left to receive a reply. Its view is the thread, read-only, with
// Resolve and Dismiss — no comment box, no option taps, no assignee edit,
// no Agent tab (session preview / wake). A linked ticket keeps all of them.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { FeedbackItemView } from './FeedbackItemView'
import type { FeedbackListRow, FeedbackShow } from './feedback-api'

const api = vi.hoisted(() => ({
  fetchFeedbackShow: vi.fn(),
  resolveFeedback: vi.fn(async () => {}),
  commentFeedback: vi.fn(async () => ({ ok: true, id: 'x' })),
}))

vi.mock('./feedback-api', async () => {
  const actual = await vi.importActual<typeof import('./feedback-api')>('./feedback-api')
  return {
    ...actual,
    fetchFeedbackShow: api.fetchFeedbackShow,
    resolveFeedback: api.resolveFeedback,
    commentFeedback: api.commentFeedback,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

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

const base: FeedbackListRow = {
  id: '3f2a9c1e-7b4d-4e0a-9d6f-0123456789ab',
  projectId: 'gone-project-id',
  sessionId: 'sess-1',
  sessionKind: 'canonical',
  agentName: 'scout',
  kind: 'question',
  title: 'Approve the vendor contract',
  body: 'Net-30, $12k.',
  options: ['Yes', 'No'],
  priority: 1,
  status: 'waiting',
  answer: null,
  createdAt: 1_759_241_100, // 2025-09-30 14:05 UTC
  updatedAt: 1_759_241_100,
  answeredAt: null,
  commentCount: 1,
  assignees: [],
  projectPath: null,
  projectName: null,
  linked: false,
}

function show(row: FeedbackListRow): FeedbackShow {
  return {
    ...row,
    workspace: row.projectName,
    projectPath: row.projectPath,
    canonicalSessionId: null,
    comments: [{ author: 'scout', body: 'Approve the vendor contract', at: row.createdAt }],
  }
}

afterEach(() => {
  cleanup()
  localStorage.clear()
  api.fetchFeedbackShow.mockReset()
  api.resolveFeedback.mockClear()
  api.commentFeedback.mockClear()
})

describe('FeedbackItemView — unlinked ticket', () => {
  it('is read-only with only Resolve and Dismiss, and no chat with the agent', async () => {
    api.fetchFeedbackShow.mockResolvedValue(show(base))
    const onMutated = vi.fn()
    render(
      <FeedbackItemView id={base.id} listRow={base} nowSec={1_759_241_200} revision={0} onMutated={onMutated} />,
    )
    const footer = await screen.findByTestId('unlinked-thread-footer')

    // Identifying header: workspace label + exact filing date.
    expect(screen.getByText(/Unlinked workspace · asked 2025-09-30 14:05 UTC/)).toBeTruthy()
    // No action bar: no Answer box, no quick answers, no Reassign, no
    // Chat with agent (no session preview / wake for a removed workspace).
    expect(screen.queryByTestId('ticket-action-bar')).toBeNull()
    expect(screen.queryByTestId('ticket-action-chat')).toBeNull()
    expect(screen.queryByTestId('ticket-action-reassign')).toBeNull()
    expect(screen.queryByTestId('ticket-quick-answers')).toBeNull()
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Yes' })).toBeNull()

    const actions = Array.from(footer.querySelectorAll('button')).map((b) => b.textContent?.trim())
    expect(actions).toEqual(['Resolve', 'Dismiss'])

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    await waitFor(() => {
      expect(api.resolveFeedback).toHaveBeenCalledWith(base.id, 'dismissed')
      expect(onMutated).toHaveBeenCalled()
    })
    expect(api.commentFeedback).not.toHaveBeenCalled()
  })

  it('a linked ticket has the pinned action bar and quick answers (control)', async () => {
    const linked: FeedbackListRow = {
      ...base,
      projectId: 'p1',
      projectPath: '/ws',
      projectName: 'Alpha',
      linked: true,
    }
    api.fetchFeedbackShow.mockResolvedValue(show(linked))
    render(
      <FeedbackItemView id={linked.id} listRow={linked} nowSec={1_759_241_200} revision={0} onMutated={vi.fn()} />,
    )
    const bar = await screen.findByTestId('ticket-action-bar')
    expect(screen.queryByTestId('unlinked-thread-footer')).toBeNull()
    const actions = ['ticket-action-answer', 'ticket-action-resolve', 'ticket-action-reassign', 'ticket-action-chat']
      .map((t) => screen.getByTestId(t))
    // The chat rail is open by default, so the toggle reads Hide chat.
    expect(actions.map((b) => b.textContent)).toEqual(['Answer', 'Resolve', 'Reassign', 'Hide chat'])
    for (const b of actions) expect(bar.contains(b)).toBe(true)
    // Structured --options become quick answers (no brief here), in the rail.
    const rail = screen.getByTestId('ticket-agent-rail')
    expect(screen.getAllByTestId('ticket-quick-answer').map((b) => b.textContent)).toEqual(['Yes', 'No'])
    for (const b of screen.getAllByTestId('ticket-quick-answer')) expect(rail.contains(b)).toBe(true)
    // Answer opens the inline box.
    expect(screen.queryByRole('textbox')).toBeNull()
    fireEvent.click(screen.getByTestId('ticket-action-answer'))
    expect(screen.getByRole('textbox')).toBeTruthy()
  })
})
