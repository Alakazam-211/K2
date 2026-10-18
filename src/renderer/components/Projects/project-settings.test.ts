// Projects V1 P8 — unit tests for the per-project Settings surface's
// pure logic (project-settings.ts). Layout fixtures reuse the P5 blob
// machinery so the "Add to dashboard" rules are tested against the
// REAL serialize/adopt path, not hand-rolled JSON.

import { describe, it, expect } from 'vitest'
import {
  addableWorkspaces,
  dashboardShowsDoc,
  filterGroupsByQuery,
  hasHtmlDocPane,
  projectSettingsCaps,
  readOnlyBannerText,
  removeMemberBlockedReason,
} from './project-settings'
import { serializeDashboardLayout, type LayoutNode, type PaneSpec } from './dashboard-layout'

const EMPTY_LAYOUT_V1 = '{"version":1,"panes":[]}' // the daemon's untouched seed

const pane = (p: PaneSpec): LayoutNode => ({ type: 'pane', pane: p })

describe('filterGroupsByQuery', () => {
  const groups = [{ name: 'Release 41' }, { name: 'infra' }, { name: 'Docs sprint' }]

  it('blank query keeps everything; matching is case-insensitive substring', () => {
    expect(filterGroupsByQuery(groups, '')).toEqual(groups)
    expect(filterGroupsByQuery(groups, '   ')).toEqual(groups)
    expect(filterGroupsByQuery(groups, 'RELEASE')).toEqual([{ name: 'Release 41' }])
    expect(filterGroupsByQuery(groups, 'sprint')).toEqual([{ name: 'Docs sprint' }])
    expect(filterGroupsByQuery(groups, 'zzz')).toEqual([])
  })
})

describe('addableWorkspaces', () => {
  const registered = [
    { id: 'w1', name: 'api', path: '/repos/api' },
    { id: 'w2', name: 'web', path: '/repos/web' },
    { id: 'w3', name: 'infra-tools', path: '/repos/tools' },
  ]

  it('excludes current members', () => {
    expect(addableWorkspaces(registered, ['w2']).map((w) => w.id)).toEqual(['w1', 'w3'])
    expect(addableWorkspaces(registered, ['w1', 'w2', 'w3'])).toEqual([])
    expect(addableWorkspaces(registered, []).map((w) => w.id)).toEqual(['w1', 'w2', 'w3'])
  })

  it('filters by name OR path, case-insensitive', () => {
    expect(addableWorkspaces(registered, [], 'API').map((w) => w.id)).toEqual(['w1'])
    expect(addableWorkspaces(registered, [], '/repos/tools').map((w) => w.id)).toEqual(['w3'])
    expect(addableWorkspaces(registered, ['w1'], 'api')).toEqual([])
  })
})

describe('removeMemberBlockedReason', () => {
  it('blocks ONLY the current PoC, with the successor explanation', () => {
    expect(removeMemberBlockedReason('w1', 'w1')).toMatch(/point of contact/)
    expect(removeMemberBlockedReason('w2', 'w1')).toBeNull()
    expect(removeMemberBlockedReason('w1', null)).toBeNull()
  })
})

describe('dashboardShowsDoc', () => {
  const doc = { workspaceId: 'w1', filePath: '/tmp/status.html' }

  it('an untouched dashboard shows only the point of contact, never a file', () => {
    expect(dashboardShowsDoc(EMPTY_LAYOUT_V1, 'poc-1', doc)).toBe(false)
  })

  it('finds the file in a saved layout on BOTH workspace and path', () => {
    const saved = serializeDashboardLayout({
      type: 'split',
      dir: 'row',
      children: [
        { size: 50, node: pane({ kind: 'terminal', workspaceId: 'poc-1' }) },
        { size: 50, node: pane({ kind: 'htmlDoc', ...doc }) },
      ],
    })
    expect(dashboardShowsDoc(saved, 'poc-1', doc)).toBe(true)
    expect(dashboardShowsDoc(saved, 'poc-1', { ...doc, workspaceId: 'w2' })).toBe(false)
    expect(dashboardShowsDoc(saved, 'poc-1', { ...doc, filePath: '/tmp/other.html' })).toBe(false)
  })
})

describe('projectSettingsCaps / readOnlyBannerText (role, not window mode)', () => {
  it('owner and admin may change everything; no banner', () => {
    for (const role of ['owner', 'admin'] as const) {
      expect(projectSettingsCaps(role)).toEqual({
        canEdit: true,
        canEditLooks: true,
        canEditDashboards: true,
        canDelete: true,
      })
      expect(readOnlyBannerText(role)).toBeNull()
    }
  })

  it('a member may rename, pin and change agents, but not looks, dashboards or delete', () => {
    expect(projectSettingsCaps('member')).toEqual({
      canEdit: true,
      canEditLooks: false,
      canEditDashboards: false,
      canDelete: false,
    })
    const text = readOnlyBannerText('member')
    expect(text).toMatch(/owner or an admin/)
    expect(text).toMatch(/delete/)
  })

  it('an unknown role leaves it to the daemon (like roleMeetsFloor)', () => {
    expect(projectSettingsCaps(null).canDelete).toBe(true)
    expect(readOnlyBannerText(null)).toBeNull()
  })
})

describe('hasHtmlDocPane', () => {
  it('matches on BOTH workspaceId and filePath, anywhere in the tree; terminals never match', () => {
    const root: LayoutNode = {
      type: 'split',
      dir: 'row',
      children: [
        { size: 50, node: pane({ kind: 'terminal', workspaceId: 'w1' }) },
        {
          size: 50,
          node: {
            type: 'split',
            dir: 'col',
            children: [
              { size: 50, node: pane({ kind: 'terminal', workspaceId: 'w2' }) },
              { size: 50, node: pane({ kind: 'htmlDoc', workspaceId: 'w1', filePath: '/a.html' }) },
            ],
          },
        },
      ],
    }
    expect(hasHtmlDocPane(root, 'w1', '/a.html')).toBe(true)
    expect(hasHtmlDocPane(root, 'w2', '/a.html')).toBe(false)
    expect(hasHtmlDocPane(root, 'w1', '/b.html')).toBe(false)
    expect(hasHtmlDocPane(null, 'w1', '/a.html')).toBe(false)
  })
})
