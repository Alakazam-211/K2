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
  api.fetchFeedbackShow.mockReset()
  api.resolveFeedback.mockClear()
  api.commentFeedback.mockClear()
})

describe('FeedbackItemView — unlinked ticket', () => {
  it('is read-only with only Resolve and Dismiss, and no Agent tab', async () => {
    api.fetchFeedbackShow.mockResolvedValue(show(base))
    const onMutated = vi.fn()
    render(
      <FeedbackItemView id={base.id} listRow={base} nowSec={1_759_241_200} revision={0} onMutated={onMutated} />,
    )
    const footer = await screen.findByTestId('unlinked-thread-footer')

    // Identifying header: workspace label + exact filing date.
    expect(screen.getByText(/Unlinked workspace · asked 2025-09-30 14:05 UTC/)).toBeTruthy()
    // Thread only — the Agent (session preview / wake) tab is gone.
    expect(screen.getByRole('button', { name: 'Thread' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Agent' })).toBeNull()
    // No comment box, no Comment button, no option taps, no assignee edit.
    expect(screen.queryByRole('textbox')).toBeNull()
    expect(screen.queryByRole('button', { name: /Comment/ })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Yes' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Edit' })).toBeNull()

    const actions = Array.from(footer.querySelectorAll('button')).map((b) => b.textContent?.trim())
    expect(actions).toEqual(['Resolve', 'Dismiss'])

    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }))
    await waitFor(() => {
      expect(api.resolveFeedback).toHaveBeenCalledWith(base.id, 'dismissed')
      expect(onMutated).toHaveBeenCalled()
    })
    expect(api.commentFeedback).not.toHaveBeenCalled()
  })

  it('a linked ticket keeps the comment box, the extra actions and the Agent tab (control)', async () => {
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
    await screen.findByRole('textbox')
    expect(screen.queryByTestId('unlinked-thread-footer')).toBeNull()
    expect(screen.getByRole('button', { name: 'Agent' })).toBeTruthy()
    expect(screen.getByRole('button', { name: /Comment/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Needs discussion' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Planned' })).toBeTruthy()
  })
})
