// @vitest-environment jsdom
//
// The one click on an owner-notice ticket (plan-getting-started-settings-v1):
// its options show only for the roles the daemon's `optionRoles` lists. The
// Owner sees them; an Admin where the option allows an Admin and not where it
// doesn't; a Member never; an agent never; a ticket without `optionRoles`
// keeps them hidden. A pick is `feedback/comment` with `optionPick`.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { noticeOptionsFor, ticketViewerRole, type FeedbackShow, type TicketViewerRole } from './feedback-api'
import { NoticeQuickAnswers, NoticeQuickAnswersFor } from './NoticeQuickAnswers'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const START = 'Start setup again'

function ticket(over: Partial<FeedbackShow> = {}): FeedbackShow {
  return {
    id: 't-1',
    projectId: '_orphan',
    sessionId: null,
    sessionKind: null,
    agentName: 'anna',
    kind: 'question',
    title: 'anna asks to turn on the setup list',
    body: 'b',
    options: [START],
    priority: 3,
    status: 'waiting',
    answer: null,
    createdAt: 1,
    updatedAt: 1,
    answeredAt: null,
    commentCount: 1,
    assignees: ['owner'],
    workspace: null,
    projectPath: null,
    comments: [],
    optionRoles: { [START]: ['owner', 'admin'] },
    ...over,
  } as FeedbackShow
}

function shown(item: FeedbackShow, role: TicketViewerRole | null): string[] {
  render(<NoticeQuickAnswersFor item={item} role={role} onChanged={() => {}} />)
  const labels = screen.queryAllByTestId('ticket-quick-answer').map((b) => b.textContent ?? '')
  cleanup()
  return labels
}

beforeEach(() => {
  h.daemonCliPost.mockResolvedValue({ ok: true, id: 't-1', answered: true, status: 'answered' })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('who sees the options on an owner notice', () => {
  it('the Owner sees them', () => {
    expect(shown(ticket(), 'owner')).toEqual([START])
  })

  it('an Admin sees them where the option allows an Admin, and not where it does not', () => {
    expect(shown(ticket(), 'admin')).toEqual([START])
    expect(shown(ticket({ optionRoles: { [START]: ['owner'] } }), 'admin')).toEqual([])
    expect(shown(ticket({ optionRoles: { [START]: ['owner'] } }), 'owner')).toEqual([START])
  })

  it('a Member never does', () => {
    expect(shown(ticket(), 'member')).toEqual([])
  })

  it('an agent never does', () => {
    expect(shown(ticket(), 'agent')).toEqual([])
    expect(noticeOptionsFor(ticket({ optionRoles: { [START]: ['owner', 'admin', 'agent'] } }), 'agent')).toEqual([])
  })

  it('a ticket without optionRoles keeps its options hidden, as before; so does an unknown role or a closed ticket', () => {
    expect(shown(ticket({ optionRoles: undefined }), 'owner')).toEqual([])
    expect(shown(ticket(), null)).toEqual([])
    expect(shown(ticket({ status: 'resolved' }), 'owner')).toEqual([])
  })

  it('reads the role from whoami', () => {
    expect(ticketViewerRole({ owner: true })).toBe('owner')
    expect(ticketViewerRole({ owner: false, role: 'owner' })).toBe('owner')
    expect(ticketViewerRole({ owner: false, role: 'admin' })).toBe('admin')
    expect(ticketViewerRole({ owner: false, role: 'member' })).toBe('member')
    expect(ticketViewerRole(null)).toBe(null)
  })
})

describe('a pick', () => {
  it('is the ticket comment with optionPick, then the ticket is read again', async () => {
    const onChanged = vi.fn()
    render(<NoticeQuickAnswersFor item={ticket()} role="admin" onChanged={onChanged} />)
    fireEvent.click(screen.getByTestId('ticket-quick-answer'))
    await waitFor(() => expect(onChanged).toHaveBeenCalledTimes(1))
    expect(h.daemonCliPost).toHaveBeenCalledWith('feedback/comment', { id: 't-1', body: START, optionPick: true })
  })

  it('the mounted one asks whoami for the role', async () => {
    h.daemonCliGet.mockImplementation(async (route: string) => {
      if (route === 'auth/whoami') return { owner: false, role: 'member' }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<NoticeQuickAnswers item={ticket()} onChanged={() => {}} />)
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('auth/whoami'))
    expect(screen.queryByTestId('ticket-quick-answer')).toBeNull()
    cleanup()
    h.daemonCliGet.mockImplementation(async () => ({ owner: true }))
    render(<NoticeQuickAnswers item={ticket()} onChanged={() => {}} />)
    await screen.findByTestId('ticket-quick-answer')
  })
})
