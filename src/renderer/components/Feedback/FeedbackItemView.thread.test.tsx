// @vitest-environment jsdom
// On a server that keeps ticket discussions on the Thread store
// (`ticket-thread-v1`) the open ticket doesn't refetch `show` on every
// ticket event (the discussion follows the Thread's socket):
//   - a post's answer moves the header at once (the pane's patch);
//   - a change made elsewhere arrives with the list's own refetch (a newer
//     list row);
//   - an older server keeps the event refetch.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const api = vi.hoisted(() => ({ fetchFeedbackShow: vi.fn() }))
vi.mock('./feedback-api', async () => {
  const actual = await vi.importActual<typeof import('./feedback-api')>('./feedback-api')
  return { ...actual, fetchFeedbackShow: api.fetchFeedbackShow }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ users: [] })),
  daemonCliGetText: vi.fn(async () => ''),
  daemonCliPost: vi.fn(async () => ({})),
  RecoveringError: class RecoveringError extends Error {},
}))
// The pane's own tests cover it; this stand-in hands the view a post's patch.
vi.mock('./TicketThreadPane', () => ({
  TicketThreadPane: (p: { onTicketPatch?: (x: { status: string; answer?: string }) => void }) => (
    <button type="button" data-testid="stub-pane-post" onClick={() => p.onTicketPatch?.({ status: 'answered', answer: 'Yes' })}>
      post
    </button>
  ),
}))

import { FeedbackItemView } from './FeedbackItemView'
import type { FeedbackListRow, FeedbackShow } from './feedback-api'
import { primaryScope } from '@/kessel/server-scope'
import { TICKET_THREAD_FEATURE } from './ticket-thread-contract'

const row: FeedbackListRow = {
  id: '3f2a9c1e-7b4d-4e0a-9d6f-0123456789ab',
  projectId: 'p1',
  sessionId: 'sess-1',
  sessionKind: 'canonical',
  agentName: 'ada',
  kind: 'question',
  title: 'Approve the vendor contract',
  body: 'Net-30.',
  options: ['Yes', 'No'],
  priority: 1,
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
const show = (r: FeedbackListRow): FeedbackShow => ({ ...r, workspace: r.projectName, projectPath: r.projectPath, canonicalSessionId: null, comments: [] })

let features = new Set<string>()
beforeEach(() => {
  features = new Set([TICKET_THREAD_FEATURE])
  vi.spyOn(primaryScope(), 'serverSupports').mockImplementation((f) => features.has(f))
  api.fetchFeedbackShow.mockResolvedValue(show(row))
})
afterEach(() => {
  cleanup()
  localStorage.clear()
  api.fetchFeedbackShow.mockReset()
  vi.restoreAllMocks()
})

const view = (r: FeedbackListRow, revision: number, onMutated = vi.fn()) => (
  <FeedbackItemView id={r.id} listRow={r} nowSec={1_759_241_200} revision={revision} onMutated={onMutated} />
)
const status = (): string => screen.getByTestId('ticket-status').textContent ?? ''
const wait = (ms: number) => act(() => new Promise((r) => setTimeout(r, ms)))

describe('the open ticket on the Thread store', () => {
  it('a ticket event doesn’t refetch the ticket', async () => {
    const r = render(view(row, 0))
    await screen.findByTestId('stub-pane-post')
    expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(1)
    r.rerender(view(row, 1))
    await wait(400)
    expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(1)
  })

  it('a post’s answer moves the header at once, and the list catches up', async () => {
    const onMutated = vi.fn()
    render(view(row, 0, onMutated))
    fireEvent.click(await screen.findByTestId('stub-pane-post'))
    await waitFor(() => expect(status()).toMatch(/Answered/))
    expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(1)
    expect(onMutated).toHaveBeenCalled()
  })

  it('a change made elsewhere comes with a newer list row', async () => {
    const r = render(view(row, 0))
    await screen.findByTestId('stub-pane-post')
    await waitFor(() => expect(status()).toMatch(/Waiting/))
    r.rerender(view({ ...row, status: 'resolved', updatedAt: row.updatedAt + 60 }, 1))
    await waitFor(() => expect(status()).toMatch(/Resolved/))
    expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(1)
  })
})

describe('an older server', () => {
  it('keeps the event refetch', async () => {
    features = new Set()
    const r = render(view(row, 0))
    await screen.findByTestId('ticket-agent-rail')
    r.rerender(view(row, 1))
    await waitFor(() => expect(api.fetchFeedbackShow).toHaveBeenCalledTimes(2), { timeout: 2000 })
  })
})
