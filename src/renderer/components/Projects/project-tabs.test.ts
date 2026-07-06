// §6.7 pure logic: dashboards-as-tabs ordering + active-tab healing
// (§6.7.6), the reorder move math the Settings buttons ride, and the
// Esc-to-pane focus target (§6.7.4).

import { describe, it, expect } from 'vitest'
import type { LayoutNode, PaneSpec } from './dashboard-layout'
import {
  FEEDBACK_TAB,
  MAX_PANE_SHORTCUTS,
  moveDashboardId,
  orderedDashboards,
  paneByNumber,
  paneNumbersById,
  paneSwitchDigit,
  resolveActiveTab,
  resolveEscFocusPane,
  terminalPaneNumbers,
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
  const pane = (p: PaneSpec): LayoutNode => ({ type: 'pane', pane: p })
  const term = (workspaceId: string): PaneSpec => ({ kind: 'terminal', workspaceId })
  const doc: PaneSpec = { kind: 'htmlDoc', workspaceId: 'w9', filePath: '/x.html' }
  const row = (...nodes: LayoutNode[]): LayoutNode => ({
    type: 'split',
    dir: 'row',
    children: nodes.map((node) => ({ size: 100 / nodes.length, node })),
  })

  it('prefers the last-used pane when it is still a terminal pane in the tree', () => {
    expect(resolveEscFocusPane(row(pane(term('w1')), pane(term('w2'))), 'w2')).toBe('w2')
  })

  it('falls back to the FIRST terminal pane (reading order) when the last-used one left', () => {
    expect(resolveEscFocusPane(row(pane(doc), pane(term('w1')), pane(term('w2'))), 'gone')).toBe(
      'w1',
    )
    expect(resolveEscFocusPane(pane(term('w1')), null)).toBe('w1')
    // Reading order descends NESTED splits too.
    const nested = row(pane(doc), {
      type: 'split',
      dir: 'col',
      children: [
        { size: 50, node: pane(term('w3')) },
        { size: 50, node: pane(term('w4')) },
      ],
    })
    expect(resolveEscFocusPane(nested, null)).toBe('w3')
  })

  it('no terminal panes (docs only / empty) → null, the no-op', () => {
    expect(resolveEscFocusPane(pane(doc), 'w9')).toBeNull() // htmlDoc never matches
    expect(resolveEscFocusPane(null, null)).toBeNull()
  })
})

describe('pane numbering (⌘1…⌘9 — reading order, first 9 only)', () => {
  const pane = (p: PaneSpec): LayoutNode => ({ type: 'pane', pane: p })
  const term = (workspaceId: string): PaneSpec => ({ kind: 'terminal', workspaceId })
  const doc = (workspaceId: string, filePath: string): PaneSpec => ({
    kind: 'htmlDoc',
    workspaceId,
    filePath,
  })
  const row = (...nodes: LayoutNode[]): LayoutNode => ({
    type: 'split',
    dir: 'row',
    children: nodes.map((node) => ({ size: 100 / nodes.length, node })),
  })

  it('numbers panes 1..N in reading order — ALL kinds occupy a number', () => {
    const root = row(pane(doc('w9', '/a.html')), pane(term('w1')), pane(term('w2')))
    const nums = paneNumbersById(root)
    expect(nums.get('h:w9:/a.html')).toBe(1)
    expect(nums.get('t:w1')).toBe(2)
    expect(nums.get('t:w2')).toBe(3)
  })

  it('descends nested splits in reading order', () => {
    const nested = row(pane(term('w1')), {
      type: 'split',
      dir: 'col',
      children: [
        { size: 50, node: pane(term('w2')) },
        { size: 50, node: pane(term('w3')) },
      ],
    })
    expect(paneNumbersById(nested).get('t:w3')).toBe(3)
  })

  it('only the first 9 panes get a number', () => {
    const root = row(...Array.from({ length: 11 }, (_, i) => pane(term(`w${i + 1}`))))
    const nums = paneNumbersById(root)
    expect(nums.size).toBe(MAX_PANE_SHORTCUTS)
    expect(nums.get('t:w9')).toBe(9)
    expect(nums.get('t:w10')).toBeUndefined()
  })

  it('terminalPaneNumbers keys by workspaceId and skips non-terminal panes WITHOUT renumbering', () => {
    const root = row(pane(doc('w9', '/a.html')), pane(term('w1')), pane(term('w2')))
    expect(terminalPaneNumbers(root)).toEqual({ w1: 2, w2: 3 })
    expect(terminalPaneNumbers(null)).toEqual({})
  })

  it('paneByNumber addresses the 1-based reading order; out of range → null', () => {
    const root = row(pane(term('w1')), pane(term('w2')))
    expect(paneByNumber(root, 1)?.paneId).toBe('t:w1')
    expect(paneByNumber(root, 2)?.paneId).toBe('t:w2')
    expect(paneByNumber(root, 3)).toBeNull()
    expect(paneByNumber(root, 0)).toBeNull()
    expect(paneByNumber(root, 10)).toBeNull()
    expect(paneByNumber(null, 1)).toBeNull()
  })
})

describe('paneSwitchDigit (the keyboard-scope guard)', () => {
  const key = (
    k: string,
    mods: Partial<{ metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean }> = {},
  ): { key: string; metaKey: boolean; ctrlKey: boolean; altKey: boolean; shiftKey: boolean } => ({
    key: k,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    ...mods,
  })

  it('plain Cmd+1…Cmd+9 → the digit', () => {
    expect(paneSwitchDigit(key('1', { metaKey: true }))).toBe(1)
    expect(paneSwitchDigit(key('9', { metaKey: true }))).toBe(9)
  })

  it('leaves the neighbouring shortcuts alone (Ctrl+digit presets, ⌘⌥digit workspace switch, ⌘⇧ screenshots)', () => {
    expect(paneSwitchDigit(key('1', { ctrlKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('1', { metaKey: true, altKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('1', { metaKey: true, shiftKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('1', { metaKey: true, ctrlKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('1'))).toBeNull() // bare digit = typing
  })

  it('non-digits, 0, and multi-char keys never match', () => {
    expect(paneSwitchDigit(key('0', { metaKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('t', { metaKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('F1', { metaKey: true }))).toBeNull()
    expect(paneSwitchDigit(key('Escape', { metaKey: true }))).toBeNull()
  })
})
