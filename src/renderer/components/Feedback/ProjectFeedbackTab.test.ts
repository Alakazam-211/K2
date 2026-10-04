// Pure-logic tests for the project Feedback tab's fan-out scoping
// (P7, prd-projects-v1 §6.6): the member rows → feedback fan-out refs
// mapping. No DOM. Fail-loud: exact equality.

import { describe, it, expect } from 'vitest'
import { memberFeedbackRefs, rowsForMembers } from './ProjectFeedbackTab'
import type { ProjectGroupMemberInfo } from '@/components/Projects/projects-api'

function member(
  workspaceId: string,
  name: string | null,
  path: string | null,
): ProjectGroupMemberInfo {
  return { workspaceId, name, path, agentName: name && `${name} Agent`, createdAt: 0 }
}

describe('memberFeedbackRefs', () => {
  it('maps member rows to fan-out refs, preserving order', () => {
    const refs = memberFeedbackRefs([
      member('ws-b', 'Bravo', '/dev/bravo'),
      member('ws-a', 'Alpha', '/dev/alpha'),
    ])
    expect(refs).toEqual([
      { id: 'ws-b', name: 'Bravo', path: '/dev/bravo' },
      { id: 'ws-a', name: 'Alpha', path: '/dev/alpha' },
    ])
  })

  it('skips members whose workspace has been unregistered (null name/path)', () => {
    const refs = memberFeedbackRefs([
      member('ws-gone', null, null),
      member('ws-a', 'Alpha', '/dev/alpha'),
    ])
    expect(refs).toEqual([{ id: 'ws-a', name: 'Alpha', path: '/dev/alpha' }])
  })

  it('empty membership fans out to nothing', () => {
    expect(memberFeedbackRefs([])).toEqual([])
  })
})

describe('rowsForMembers', () => {
  it('keeps only tickets filed by member workspaces (list-all returns the whole host)', () => {
    const refs = memberFeedbackRefs([member('ws-a', 'Alpha', '/dev/alpha')])
    const rows = [
      { id: 't1', projectId: 'ws-a' },
      { id: 't2', projectId: 'ws-other' },
    ] as unknown as Parameters<typeof rowsForMembers>[0]
    expect(rowsForMembers(rows, refs).map((r) => r.id)).toEqual(['t1'])
    expect(rowsForMembers(rows, [])).toEqual([])
  })
})
