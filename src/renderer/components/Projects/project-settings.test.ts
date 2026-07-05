// Projects V1 P8 — unit tests for the per-project Settings surface's
// pure logic (project-settings.ts). Layout fixtures reuse the P5 blob
// machinery so the "Add to dashboard" rules are tested against the
// REAL serialize/adopt path, not hand-rolled JSON.

import { describe, it, expect } from 'vitest'
import {
  addableWorkspaces,
  appendHtmlDocColumn,
  filterGroupsByQuery,
  findHtmlDocColumn,
  removeMemberBlockedReason,
} from './project-settings'
import {
  parseDashboardLayout,
  serializeDashboardLayout,
  isHtmlDocPane,
  isTerminalPane,
  type DashboardColumn,
} from './dashboard-layout'

const EMPTY_LAYOUT_V1 = '{"version":1,"panes":[]}' // the daemon's untouched seed

function columnsOf(layoutJson: string): DashboardColumn[] {
  const parsed = parseDashboardLayout(layoutJson)
  if (parsed.kind !== 'layout') throw new Error(`expected a layout blob, got ${parsed.kind}`)
  return parsed.columns
}

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
    expect(removeMemberBlockedReason('w1', 'w1')).toMatch(/Point of Contact/)
    expect(removeMemberBlockedReason('w2', 'w1')).toBeNull()
    expect(removeMemberBlockedReason('w1', null)).toBeNull()
  })
})

describe('appendHtmlDocColumn', () => {
  const doc = { workspaceId: 'w1', filePath: '/tmp/status.html' }

  it('materializes the PoC seed from an untouched blob, then appends', () => {
    const { layoutJson, added } = appendHtmlDocColumn(EMPTY_LAYOUT_V1, 'poc-1', doc)
    expect(added).toBe(true)
    const cols = columnsOf(layoutJson)
    expect(cols).toHaveLength(2)
    expect(isTerminalPane(cols[0].pane) && cols[0].pane.workspaceId === 'poc-1').toBe(true)
    expect(isHtmlDocPane(cols[1].pane)).toBe(true)
    expect(cols[0].widthPct + cols[1].widthPct).toBeCloseTo(100)
  })

  it('appends at the end of a saved layout, rebalancing widths', () => {
    const saved = serializeDashboardLayout([
      { widthPct: 60, pane: { kind: 'terminal', workspaceId: 'poc-1' } },
      { widthPct: 40, pane: { kind: 'terminal', workspaceId: 'w9' } },
    ])
    const { layoutJson, added } = appendHtmlDocColumn(saved, 'poc-1', doc)
    expect(added).toBe(true)
    const cols = columnsOf(layoutJson)
    expect(cols).toHaveLength(3)
    expect(cols[2].pane).toEqual({ kind: 'htmlDoc', workspaceId: 'w1', filePath: '/tmp/status.html' })
    expect(cols.reduce((acc, c) => acc + c.widthPct, 0)).toBeCloseTo(100)
  })

  it('memberless (no PoC) untouched blob → the doc becomes the only column', () => {
    const { layoutJson, added } = appendHtmlDocColumn(EMPTY_LAYOUT_V1, null, doc)
    expect(added).toBe(true)
    const cols = columnsOf(layoutJson)
    expect(cols).toHaveLength(1)
    expect(cols[0].widthPct).toBeCloseTo(100)
  })

  it('is idempotent per (workspaceId, filePath): already present → added:false, blob untouched', () => {
    const first = appendHtmlDocColumn(EMPTY_LAYOUT_V1, 'poc-1', doc)
    const again = appendHtmlDocColumn(first.layoutJson, 'poc-1', doc)
    expect(again.added).toBe(false)
    expect(again.layoutJson).toBe(first.layoutJson)
    // The SAME path from a DIFFERENT workspace is a distinct doc.
    const other = appendHtmlDocColumn(first.layoutJson, 'poc-1', {
      workspaceId: 'w2',
      filePath: '/tmp/status.html',
    })
    expect(other.added).toBe(true)
    expect(columnsOf(other.layoutJson)).toHaveLength(3)
  })
})

describe('findHtmlDocColumn', () => {
  it('matches on BOTH workspaceId and filePath; terminals never match', () => {
    const cols = columnsOf(
      serializeDashboardLayout([
        { widthPct: 50, pane: { kind: 'terminal', workspaceId: 'w1' } },
        { widthPct: 50, pane: { kind: 'htmlDoc', workspaceId: 'w1', filePath: '/a.html' } },
      ]),
    )
    expect(findHtmlDocColumn(cols, 'w1', '/a.html')).toBe(1)
    expect(findHtmlDocColumn(cols, 'w2', '/a.html')).toBe(-1)
    expect(findHtmlDocColumn(cols, 'w1', '/b.html')).toBe(-1)
  })
})
