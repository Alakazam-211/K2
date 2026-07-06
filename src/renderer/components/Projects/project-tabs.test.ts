// §6.7 pure logic: dashboards-as-tabs ordering + active-tab healing
// (§6.7.6), the reorder move math the Settings buttons ride, and the
// Esc-to-pane focus target (§6.7.4).

import { describe, it, expect } from 'vitest'
import type { DashboardColumn } from './dashboard-layout'
import {
  FEEDBACK_TAB,
  moveDashboardId,
  orderedDashboards,
  resolveActiveTab,
  resolveEscFocusPane,
} from './project-tabs'

function dash(id: string, position: number, createdAt = 0): {
  id: string
  position: number
  createdAt: number
} {
  return { id, position, createdAt }
}

describe('orderedDashboards (§6.7.6 — tabs in position order)', () => {
  it('sorts by position ascending', () => {
    const ds = [dash('c', 2), dash('a', 0), dash('b', 1)]
    expect(orderedDashboards(ds).map((d) => d.id)).toEqual(['a', 'b', 'c'])
  })

  it('tie-breaks duplicate positions by createdAt, then id — a stable row', () => {
    const ds = [dash('z', 1, 50), dash('m', 1, 10), dash('m2', 1, 10)]
    expect(orderedDashboards(ds).map((d) => d.id)).toEqual(['m', 'm2', 'z'])
  })

  it('never mutates the input', () => {
    const ds = [dash('b', 1), dash('a', 0)]
    orderedDashboards(ds)
    expect(ds.map((d) => d.id)).toEqual(['b', 'a'])
  })
})

describe('resolveActiveTab (§6.7.6 — selection healing)', () => {
  const ids = ['d1', 'd2']

  it('keeps a live dashboard id and the Feedback sentinel', () => {
    expect(resolveActiveTab('d2', ids)).toBe('d2')
    expect(resolveActiveTab(FEEDBACK_TAB, ids)).toBe(FEEDBACK_TAB)
  })

  it('heals a stale id (deleted dashboard) and a fresh mount (null) to the first dashboard', () => {
    expect(resolveActiveTab('gone', ids)).toBe('d1')
    expect(resolveActiveTab(null, ids)).toBe('d1')
  })

  it('falls back to Feedback when the project has no dashboards', () => {
    expect(resolveActiveTab(null, [])).toBe(FEEDBACK_TAB)
    expect(resolveActiveTab('gone', [])).toBe(FEEDBACK_TAB)
  })
})

describe('moveDashboardId (§6.7.6 — reorder moves)', () => {
  const order = ['a', 'b', 'c']

  it('moves left/right by swapping with the neighbor', () => {
    expect(moveDashboardId(order, 'b', -1)).toEqual(['b', 'a', 'c'])
    expect(moveDashboardId(order, 'b', 1)).toEqual(['a', 'c', 'b'])
  })

  it('is a no-op (null) at the edges and for unknown ids', () => {
    expect(moveDashboardId(order, 'a', -1)).toBeNull()
    expect(moveDashboardId(order, 'c', 1)).toBeNull()
    expect(moveDashboardId(order, 'nope', 1)).toBeNull()
  })

  it('never mutates the input order', () => {
    moveDashboardId(order, 'b', 1)
    expect(order).toEqual(['a', 'b', 'c'])
  })
})

describe('resolveEscFocusPane (§6.7.4 — Esc focus target)', () => {
  const term = (workspaceId: string): DashboardColumn => ({
    pane: { kind: 'terminal', workspaceId },
    widthPct: 50,
  })
  const doc: DashboardColumn = {
    pane: { kind: 'htmlDoc', workspaceId: 'w9', filePath: '/x.html' },
    widthPct: 50,
  }

  it('prefers the last-used pane when it is still a terminal column', () => {
    expect(resolveEscFocusPane([term('w1'), term('w2')], 'w2')).toBe('w2')
  })

  it('falls back to the FIRST terminal pane when the last-used one left the dashboard', () => {
    expect(resolveEscFocusPane([doc, term('w1'), term('w2')], 'gone')).toBe('w1')
    expect(resolveEscFocusPane([term('w1')], null)).toBe('w1')
  })

  it('no terminal panes (docs only / empty) → null, the no-op', () => {
    expect(resolveEscFocusPane([doc], 'w9')).toBeNull() // htmlDoc never matches
    expect(resolveEscFocusPane([], null)).toBeNull()
  })
})
