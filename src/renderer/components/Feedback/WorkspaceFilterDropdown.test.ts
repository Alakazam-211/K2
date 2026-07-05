// Pure-logic tests for the workspace-filter dropdown's section builder
// (grouping / ordering / search — the ergonomics mirrored from the
// Settings → Workspaces list) and the P7 (§6.6) project-group filter
// (value encoding, section builder, member-scoped row filter). No DOM.
// Fail-loud: exact equality.

import { describe, it, expect } from 'vitest'
import {
  filterProjectGroupsForFilter,
  groupWorkspacesForFilter,
  parseProjectFilter,
  projectFilterValue,
  rowsForWorkspaceFilter,
  workspaceMatchesSearch,
  type FilterableProjectGroup,
  type FilterableWorkspace,
} from './WorkspaceFilterDropdown'
import type { FocusGroup } from '@/stores/focus-groups'

function ws(
  id: string,
  name: string,
  focusGroupId: string | null = null,
  path = `/tmp/${id}`,
): FilterableWorkspace {
  return { id, name, path, color: '#123456', iconUrl: null, focusGroupId }
}

function group(id: string, name: string, color: string | null = null): FocusGroup {
  return { id, name, color, tabOrder: 0, createdAt: 0 }
}

const projects = [
  ws('p-zulu', 'Zulu', 'g1'),
  ws('p-alpha', 'Alpha', 'g1'),
  ws('p-mike', 'Mike', 'g2'),
  ws('p-echo', 'Echo', null),
  ws('p-bravo', 'Bravo', null),
]
const groups = [group('g1', 'Clients', '#ff0000'), group('g2', 'Internal')]

describe('groupWorkspacesForFilter', () => {
  it('groups by focus group in store order, alphabetical within, Ungrouped last', () => {
    const sections = groupWorkspacesForFilter(projects, groups, true, '')
    expect(sections.map((s) => s.label)).toEqual(['Clients', 'Internal', 'Ungrouped'])
    expect(sections[0].color).toBe('#ff0000')
    expect(sections[0].workspaces.map((w) => w.name)).toEqual(['Alpha', 'Zulu'])
    expect(sections[1].workspaces.map((w) => w.name)).toEqual(['Mike'])
    expect(sections[2].workspaces.map((w) => w.name)).toEqual(['Bravo', 'Echo'])
  })

  it('drops empty sections (a group with no matches disappears)', () => {
    const sections = groupWorkspacesForFilter(projects, groups, true, 'mike')
    expect(sections.map((s) => s.label)).toEqual(['Internal'])
    expect(sections[0].workspaces.map((w) => w.id)).toEqual(['p-mike'])
  })

  it('groups off = one flat alphabetical section without a header', () => {
    const sections = groupWorkspacesForFilter(projects, groups, false, '')
    expect(sections).toHaveLength(1)
    expect(sections[0].label).toBeNull()
    expect(sections[0].workspaces.map((w) => w.name)).toEqual([
      'Alpha',
      'Bravo',
      'Echo',
      'Mike',
      'Zulu',
    ])
  })

  it('no matches at all = no sections', () => {
    expect(groupWorkspacesForFilter(projects, groups, true, 'zzzz')).toEqual([])
    expect(groupWorkspacesForFilter([], groups, false, '')).toEqual([])
  })
})

describe('workspaceMatchesSearch', () => {
  it('matches name OR path, case-insensitively; empty query matches all', () => {
    const w = ws('p1', 'Fleet Console', null, '/Users/z/dev/fleet-console')
    expect(workspaceMatchesSearch(w, '')).toBe(true)
    expect(workspaceMatchesSearch(w, '   ')).toBe(true)
    expect(workspaceMatchesSearch(w, 'FLEET')).toBe(true)
    expect(workspaceMatchesSearch(w, 'dev/fleet')).toBe(true)
    expect(workspaceMatchesSearch(w, 'cortana')).toBe(false)
  })
})

// ── Projects V1 P7 (§6.6) — project-group filter ──────────────────────────

function pg(id: string, name: string, memberCount = 2): FilterableProjectGroup {
  return { id, name, memberCount }
}

describe('projectFilterValue / parseProjectFilter', () => {
  it('round-trips a group id through the value channel', () => {
    expect(parseProjectFilter(projectFilterValue('pg-1'))).toBe('pg-1')
  })

  it('non-project values parse to null', () => {
    expect(parseProjectFilter('all')).toBeNull()
    expect(parseProjectFilter('p-workspace-uuid')).toBeNull()
    expect(parseProjectFilter('')).toBeNull()
  })
})

describe('filterProjectGroupsForFilter', () => {
  const pgs = [pg('pg-z', 'Zephyr'), pg('pg-a', 'Apollo'), pg('pg-k', 'K2 Launch')]

  it('sorts alphabetically; empty query keeps every group', () => {
    expect(filterProjectGroupsForFilter(pgs, '').map((g) => g.name)).toEqual([
      'Apollo',
      'K2 Launch',
      'Zephyr',
    ])
  })

  it('matches on name substring, case-insensitively', () => {
    expect(filterProjectGroupsForFilter(pgs, 'LAUNCH').map((g) => g.id)).toEqual(['pg-k'])
    expect(filterProjectGroupsForFilter(pgs, 'nope')).toEqual([])
  })
})

describe('rowsForWorkspaceFilter', () => {
  const rows = [
    { id: 'f1', projectId: 'ws-a' },
    { id: 'f2', projectId: 'ws-b' },
    { id: 'f3', projectId: 'ws-c' },
    { id: 'f4', projectId: 'ws-a' },
  ]

  it("'all' passes every row through untouched", () => {
    expect(rowsForWorkspaceFilter(rows, 'all', null)).toEqual(rows)
  })

  it('a workspace id keeps only that host workspace', () => {
    expect(rowsForWorkspaceFilter(rows, 'ws-a', null).map((r) => r.id)).toEqual(['f1', 'f4'])
  })

  it('a project value keeps rows from MEMBER workspaces only', () => {
    const members = new Set(['ws-a', 'ws-c'])
    expect(
      rowsForWorkspaceFilter(rows, projectFilterValue('pg-1'), members).map((r) => r.id),
    ).toEqual(['f1', 'f3', 'f4'])
  })

  it('a project value with unresolved membership (null) shows nothing', () => {
    expect(rowsForWorkspaceFilter(rows, projectFilterValue('pg-1'), null)).toEqual([])
  })
})
