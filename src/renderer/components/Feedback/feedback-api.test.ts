// Pure-logic tests for the Feedback F2 page helpers. No daemon, no DOM —
// every function under test is side-effect-free. Fail-loud: exact equality
// assertions, no try/catch swallowing, no skip-if-missing fallbacks.

import { describe, it, expect } from 'vitest'
import {
  askingSessionWakeAction,
  collectAssignees,
  countByStatus,
  filterByAssignee,
  filterBySearch,
  formatFiledDate,
  groupByStatus,
  isUnlinked,
  optionsActionable,
  selectableStatusesFor,
  sortNewestFirst,
  unlinkedDetails,
  type FeedbackListRow,
  type FeedbackStatus,
} from './feedback-api'

describe('sortNewestFirst', () => {
  it('orders by createdAt descending without mutating the input', () => {
    const rows = [{ createdAt: 1 }, { createdAt: 3 }, { createdAt: 2 }]
    const sorted = sortNewestFirst(rows)
    expect(sorted.map((r) => r.createdAt)).toEqual([3, 2, 1])
    expect(rows.map((r) => r.createdAt)).toEqual([1, 3, 2])
  })
})

describe('groupByStatus', () => {
  it('splits waiting / needs_discussion / answered / planned / closed', () => {
    const rows = [
      { status: 'waiting' as FeedbackStatus, id: 'w' },
      { status: 'needs_discussion' as FeedbackStatus, id: 'n' },
      { status: 'answered' as FeedbackStatus, id: 'a' },
      { status: 'planned' as FeedbackStatus, id: 'p' },
      { status: 'resolved' as FeedbackStatus, id: 'r' },
      { status: 'dismissed' as FeedbackStatus, id: 'd' },
    ]
    const g = groupByStatus(rows)
    expect(g.waiting.map((r) => r.id)).toEqual(['w'])
    expect(g.needs_discussion.map((r) => r.id)).toEqual(['n'])
    expect(g.answered.map((r) => r.id)).toEqual(['a'])
    expect(g.planned.map((r) => r.id)).toEqual(['p'])
    expect(g.unlinked).toEqual([])
    expect(g.closed.map((r) => r.id)).toEqual(['r', 'd'])
  })

  it('open unlinked rows go in Unlinked workspace, never Waiting on you; closed ones go in Closed', () => {
    const rows = [
      { status: 'waiting' as FeedbackStatus, id: 'w-linked', linked: true },
      { status: 'waiting' as FeedbackStatus, id: 'w-gone', linked: false },
      { status: 'needs_discussion' as FeedbackStatus, id: 'n-gone', linked: false },
      { status: 'answered' as FeedbackStatus, id: 'a-gone', linked: false },
      { status: 'planned' as FeedbackStatus, id: 'p-gone', linked: false },
      { status: 'dismissed' as FeedbackStatus, id: 'd-gone', linked: false },
      { status: 'resolved' as FeedbackStatus, id: 'r-gone', linked: false },
    ]
    const g = groupByStatus(rows)
    expect(g.waiting.map((r) => r.id)).toEqual(['w-linked'])
    expect(g.needs_discussion).toEqual([])
    expect(g.answered).toEqual([])
    expect(g.planned).toEqual([])
    expect(g.unlinked.map((r) => r.id)).toEqual(['w-gone', 'n-gone', 'a-gone', 'p-gone'])
    expect(g.closed.map((r) => r.id)).toEqual(['d-gone', 'r-gone'])
  })
})

describe('unlinked helpers (TB18 + Appa A1)', () => {
  it('an unlinked row offers only Resolve and Dismiss; a linked row the full set', () => {
    expect(selectableStatusesFor({ linked: false })).toEqual(['resolved', 'dismissed'])
    expect(selectableStatusesFor({ linked: true })).toEqual([
      'waiting',
      'needs_discussion',
      'planned',
      'resolved',
      'dismissed',
    ])
    expect(isUnlinked({ linked: false })).toBe(true)
    expect(isUnlinked({ linked: true })).toBe(false)
    expect(isUnlinked({})).toBe(false)
  })

  it('identifying details: title, exact UTC filing date, agent, short id', () => {
    expect(
      unlinkedDetails({
        id: '3f2a9c1e-7b4d-4e0a-9d6f-0123456789ab',
        title: 'Approve the vendor contract',
        agentName: 'scout',
        createdAt: 1_759_241_100,
      }),
    ).toEqual({
      title: 'Approve the vendor contract',
      filed: '2025-09-30 14:05 UTC',
      agent: 'scout',
      shortId: '3f2a9c1e',
    })
    expect(formatFiledDate(0)).toBe('1970-01-01 00:00 UTC')
  })

  it('search tolerates a null workspace name', () => {
    const rows: Pick<FeedbackListRow, 'id' | 'title' | 'agentName' | 'projectName' | 'kind' | 'status'>[] = [
      { id: 'fb-1', title: 'Approve PR', agentName: 'scout', projectName: null, kind: 'approval', status: 'waiting' },
    ]
    expect(filterBySearch(rows, 'scout approve').map((r) => r.id)).toEqual(['fb-1'])
    expect(filterBySearch(rows, 'null')).toEqual([])
  })
})

describe('countByStatus', () => {
  it('counts every status plus the total, zeroes included', () => {
    const rows = [
      { status: 'waiting' as FeedbackStatus },
      { status: 'waiting' as FeedbackStatus },
      { status: 'needs_discussion' as FeedbackStatus },
      { status: 'answered' as FeedbackStatus },
      { status: 'dismissed' as FeedbackStatus },
      { status: 'planned' as FeedbackStatus },
    ]
    expect(countByStatus(rows)).toEqual({
      all: 6,
      waiting: 2,
      needs_discussion: 1,
      answered: 1,
      resolved: 0,
      dismissed: 1,
      planned: 1,
    })
  })
  it('an empty list is all zeroes', () => {
    expect(countByStatus([])).toEqual({
      all: 0,
      waiting: 0,
      needs_discussion: 0,
      answered: 0,
      resolved: 0,
      dismissed: 0,
      planned: 0,
    })
  })
})

describe('filterBySearch', () => {
  const rows: Pick<FeedbackListRow, 'id' | 'title' | 'agentName' | 'projectName' | 'kind' | 'status'>[] = [
    { id: 'fb-abc123', title: 'Ship the release?', agentName: 'Cortana', projectName: 'K2', kind: 'approval', status: 'waiting' },
    { id: 'fb-def456', title: 'Which DB schema wins', agentName: 'Appa', projectName: 'Fleet', kind: 'question', status: 'answered' },
  ]

  it('empty / whitespace query returns all rows unfiltered', () => {
    expect(filterBySearch(rows, '')).toEqual(rows)
    expect(filterBySearch(rows, '   ')).toEqual(rows)
  })

  it('every term must match, terms can hit different fields, any order', () => {
    expect(filterBySearch(rows, 'cortana release').map((r) => r.id)).toEqual(['fb-abc123'])
    expect(filterBySearch(rows, 'release cortana').map((r) => r.id)).toEqual(['fb-abc123'])
    expect(filterBySearch(rows, 'cortana schema')).toEqual([])
  })

  it('matches case-insensitively across title, agent, workspace, kind, status, and id', () => {
    expect(filterBySearch(rows, 'APPA').map((r) => r.id)).toEqual(['fb-def456'])
    expect(filterBySearch(rows, 'fleet').map((r) => r.id)).toEqual(['fb-def456'])
    expect(filterBySearch(rows, 'approval').map((r) => r.id)).toEqual(['fb-abc123'])
    expect(filterBySearch(rows, 'answered').map((r) => r.id)).toEqual(['fb-def456'])
    expect(filterBySearch(rows, 'def456').map((r) => r.id)).toEqual(['fb-def456'])
  })
})

describe('optionsActionable', () => {
  it('one-tap options are live only while waiting AND options exist', () => {
    expect(optionsActionable({ status: 'waiting', options: ['Yes', 'No'] })).toBe(true)
    expect(optionsActionable({ status: 'waiting', options: [] })).toBe(false)
    expect(optionsActionable({ status: 'waiting', options: null })).toBe(false)
    expect(optionsActionable({ status: 'answered', options: ['Yes'] })).toBe(false)
    expect(optionsActionable({ status: 'resolved', options: ['Yes'] })).toBe(false)
  })
})

describe('collectAssignees / filterByAssignee', () => {
  const rows = [
    { id: '1', assignees: ['owner', 'julie'] },
    { id: '2', assignees: ['julie'] },
    { id: '3', assignees: [] as string[] },
    { id: '4', assignees: null as unknown as string[] },
  ]

  it('collects unique usernames sorted A–Z', () => {
    expect(collectAssignees(rows)).toEqual(['julie', 'owner'])
  })

  it('filters all / unassigned / named assignee', () => {
    expect(filterByAssignee(rows, 'all').map((r) => r.id)).toEqual(['1', '2', '3', '4'])
    expect(filterByAssignee(rows, 'unassigned').map((r) => r.id)).toEqual(['3', '4'])
    expect(filterByAssignee(rows, 'julie').map((r) => r.id)).toEqual(['1', '2'])
    expect(filterByAssignee(rows, 'owner').map((r) => r.id)).toEqual(['1'])
    expect(filterByAssignee(rows, 'nobody')).toEqual([])
  })
})

describe('askingSessionWakeAction (D6)', () => {
  const pinned = 'conv-pinned'

  it('canonical id + sessionKind=sandbox does not sandbox/reopen', () => {
    expect(
      askingSessionWakeAction({
        sessionId: pinned,
        sessionKind: 'sandbox',
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('ensure-pinned-chat')
  })

  it('true sandbox UUID still sandbox/reopens once the pinned id is known', () => {
    expect(
      askingSessionWakeAction({
        sessionId: 'sess-sandbox-pty',
        sessionKind: 'sandbox',
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('sandbox/reopen')
  })

  it('does not sandbox/reopen a poison stamp before the pinned id is loaded', () => {
    expect(
      askingSessionWakeAction({
        sessionId: pinned,
        sessionKind: 'sandbox',
        canonicalSessionId: undefined,
        liveById: false,
      }),
    ).toBe('checking')
  })

  it('unknown kinds attach if live-by-id and are not treated as canonical', () => {
    expect(
      askingSessionWakeAction({
        sessionId: 'sess-x',
        sessionKind: 'host',
        canonicalSessionId: pinned,
        liveById: true,
      }),
    ).toBe('attach-live')
    expect(
      askingSessionWakeAction({
        sessionId: 'sess-x',
        sessionKind: 'sidecar',
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('dormant-unwakeable')
    expect(
      askingSessionWakeAction({
        sessionId: 'sess-x',
        sessionKind: 'api',
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('dormant-unwakeable')
  })

  it('unknown kind that IS the pinned conversation id still ensure-pinned-chat (D6)', () => {
    expect(
      askingSessionWakeAction({
        sessionId: pinned,
        sessionKind: 'sidecar',
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('ensure-pinned-chat')
  })

  it('canonical kind wakes via ensure-pinned-chat even if ids differ', () => {
    expect(
      askingSessionWakeAction({
        sessionId: 'old-conv',
        sessionKind: 'canonical',
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('ensure-pinned-chat')
  })

  it('sessionless is none', () => {
    expect(
      askingSessionWakeAction({
        sessionId: null,
        sessionKind: null,
        canonicalSessionId: pinned,
        liveById: false,
      }),
    ).toBe('none')
  })
})
