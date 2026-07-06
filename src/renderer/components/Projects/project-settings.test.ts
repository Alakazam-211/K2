// Projects V1 P8 — unit tests for the per-project Settings surface's
// pure logic (project-settings.ts). Layout fixtures reuse the P5 blob
// machinery so the "Add to dashboard" rules are tested against the
// REAL serialize/adopt path, not hand-rolled JSON.

import { describe, it, expect } from 'vitest'
import {
  addableWorkspaces,
  appendHtmlDocPane,
  filterGroupsByQuery,
  hasHtmlDocPane,
  removeMemberBlockedReason,
} from './project-settings'
import {
  parseDashboardLayout,
  readingOrder,
  serializeDashboardLayout,
  isHtmlDocPane,
  isTerminalPane,
  type LayoutNode,
  type PaneSpec,
} from './dashboard-layout'

const EMPTY_LAYOUT_V1 = '{"version":1,"panes":[]}' // the daemon's untouched seed

const pane = (p: PaneSpec): LayoutNode => ({ type: 'pane', pane: p })

function panesOf(layoutJson: string): PaneSpec[] {
  const parsed = parseDashboardLayout(layoutJson)
  if (parsed.kind !== 'layout') throw new Error(`expected a layout blob, got ${parsed.kind}`)
  return readingOrder(parsed.root)
}

function rootOf(layoutJson: string): LayoutNode | null {
  const parsed = parseDashboardLayout(layoutJson)
  if (parsed.kind !== 'layout') throw new Error(`expected a layout blob, got ${parsed.kind}`)
  return parsed.root
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

describe('appendHtmlDocPane', () => {
  const doc = { workspaceId: 'w1', filePath: '/tmp/status.html' }

  it('materializes the PoC seed from an untouched blob, then appends right-most', () => {
    const { layoutJson, added } = appendHtmlDocPane(EMPTY_LAYOUT_V1, 'poc-1', doc)
    expect(added).toBe(true)
    const panes = panesOf(layoutJson)
    expect(panes).toHaveLength(2)
    expect(isTerminalPane(panes[0]) && panes[0].workspaceId === 'poc-1').toBe(true)
    expect(isHtmlDocPane(panes[1])).toBe(true)
    // 50/50 row (insertEdge right on a single-pane root).
    const root = rootOf(layoutJson)
    expect(root?.type).toBe('split')
    if (root?.type !== 'split') return
    expect(root.dir).toBe('row')
    expect(root.children.map((c) => c.size)).toEqual([50, 50])
  })

  it('appends as a right-most region of a saved layout (equal share)', () => {
    const saved = serializeDashboardLayout({
      type: 'split',
      dir: 'row',
      children: [
        { size: 60, node: pane({ kind: 'terminal', workspaceId: 'poc-1' }) },
        { size: 40, node: pane({ kind: 'terminal', workspaceId: 'w9' }) },
      ],
    })
    const { layoutJson, added } = appendHtmlDocPane(saved, 'poc-1', doc)
    expect(added).toBe(true)
    const panes = panesOf(layoutJson)
    expect(panes).toHaveLength(3)
    expect(panes[2]).toEqual({ kind: 'htmlDoc', workspaceId: 'w1', filePath: '/tmp/status.html' })
    const root = rootOf(layoutJson)
    if (root?.type !== 'split') throw new Error('expected a split root')
    expect(root.children.reduce((acc, c) => acc + c.size, 0)).toBeCloseTo(100)
  })

  it('accepts a v1 blob (converts on adopt, saves v2)', () => {
    const v1 = JSON.stringify({
      version: 1,
      columns: [{ widthPct: 100, pane: { kind: 'terminal', workspaceId: 'poc-1' } }],
    })
    const { layoutJson, added } = appendHtmlDocPane(v1, 'poc-1', doc)
    expect(added).toBe(true)
    expect((JSON.parse(layoutJson) as { version: number }).version).toBe(2)
    expect(panesOf(layoutJson)).toHaveLength(2)
  })

  it('memberless (no PoC) untouched blob → the doc becomes the only pane', () => {
    const { layoutJson, added } = appendHtmlDocPane(EMPTY_LAYOUT_V1, null, doc)
    expect(added).toBe(true)
    const panes = panesOf(layoutJson)
    expect(panes).toHaveLength(1)
    expect(rootOf(layoutJson)?.type).toBe('pane')
  })

  it('is idempotent per (workspaceId, filePath): already present → added:false, blob untouched', () => {
    const first = appendHtmlDocPane(EMPTY_LAYOUT_V1, 'poc-1', doc)
    const again = appendHtmlDocPane(first.layoutJson, 'poc-1', doc)
    expect(again.added).toBe(false)
    expect(again.layoutJson).toBe(first.layoutJson)
    // The SAME path from a DIFFERENT workspace is a distinct doc.
    const other = appendHtmlDocPane(first.layoutJson, 'poc-1', {
      workspaceId: 'w2',
      filePath: '/tmp/status.html',
    })
    expect(other.added).toBe(true)
    expect(panesOf(other.layoutJson)).toHaveLength(3)
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
