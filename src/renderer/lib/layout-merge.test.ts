// Three-way layout merge (prd-split-view-multi-window-v1, V18 + D1).
// Pure: every case builds base / local / remote by hand and checks the
// merged columns exactly.
import { describe, it, expect } from 'vitest'
import type { SerializedLayout, SerializedTab } from '@/stores/tabs'
import {
  canonicalLayoutJson,
  collapseEmptyLeadingColumns,
  mergeSerializedLayouts,
} from './layout-merge'

function tab(id: string, pg = `pg-${id}`, extra: Partial<SerializedTab> = {}): SerializedTab {
  return {
    id,
    title: id,
    mosaicTree: pg,
    paneGroups: { [pg]: { id: pg, items: [{ id: `item-${pg}`, type: 'terminal', paneGroupId: pg }], activeItemIndex: 0 } },
    ...extra,
  }
}

function systemTab(id: string, section: 'chat' | 'inbox'): SerializedTab {
  const pg = `pg-${id}`
  return {
    id,
    title: section,
    mosaicTree: pg,
    isSystemAgent: true,
    paneGroups: { [pg]: { id: pg, items: [{ id: `item-${pg}`, type: 'agent', agentName: 'a', projectPath: '/p', section }], activeItemIndex: 0 } },
  }
}

function layout(...cols: SerializedTab[][]): SerializedLayout {
  const [first, ...rest] = cols
  return {
    version: 2,
    tabs: first,
    extraGroups: rest.length > 0 ? rest.map((tabs) => ({ tabs })) : undefined,
    splitCount: cols.length > 1 ? cols.length : undefined,
  }
}

function ids(l: SerializedLayout): string[][] {
  return [l.tabs.map((t) => t.id), ...(l.extraGroups ?? []).map((g) => g.tabs.map((t) => t.id))]
}

describe('mergeSerializedLayouts', () => {
  it('keeps a local close and a remote open', () => {
    const base = layout([tab('a'), tab('x')])
    const local = layout([tab('a')])
    const remote = layout([tab('a'), tab('x'), tab('b')])
    expect(ids(mergeSerializedLayouts(base, local, remote))).toEqual([['a', 'b']])
  })

  it('keeps a remote close even when this window still has the tab', () => {
    const base = layout([tab('a'), tab('x')])
    const local = layout([tab('a'), tab('x'), tab('n')])
    const remote = layout([tab('a')])
    expect(ids(mergeSerializedLayouts(base, local, remote))).toEqual([['a', 'n']])
  })

  it('uses this window\'s copy of a tab it edited, the remote copy otherwise', () => {
    const base = layout([tab('a'), tab('b')])
    const local = layout([tab('a', 'pg-a', { title: 'mine' }), tab('b')])
    const remote = layout([tab('a'), tab('b', 'pg-b', { title: 'theirs' })])
    const merged = mergeSerializedLayouts(base, local, remote)
    expect(merged.tabs.map((t) => t.title)).toEqual(['mine', 'theirs'])
  })

  it('a local split wins the column count; the new column keeps its tab', () => {
    const base = layout([tab('a'), tab('b')])
    const local = layout([tab('a'), tab('b')], [tab('s')])
    const remote = layout([tab('a'), tab('b'), tab('c')])
    expect(ids(mergeSerializedLayouts(base, local, remote))).toEqual([['a', 'b', 'c'], ['s']])
  })

  it('a remote split is kept when this window did not change the column count', () => {
    const base = layout([tab('a')])
    const local = layout([tab('a'), tab('n')])
    const remote = layout([tab('a')], [tab('s')])
    const merged = mergeSerializedLayouts(base, local, remote)
    expect(ids(merged)).toEqual([['a', 'n'], ['s']])
    expect(merged.splitCount).toBe(2)
  })

  it('a local unsplit folds the remote\'s extra column into the last kept column', () => {
    const base = layout([tab('a')], [tab('s')])
    const local = layout([tab('a'), tab('s')])
    const remote = layout([tab('a')], [tab('s'), tab('t')])
    const merged = mergeSerializedLayouts(base, local, remote)
    expect(ids(merged)).toEqual([['a', 's', 't']])
    expect(merged.splitCount).toBeUndefined()
  })

  it('a tab this window moved to another column moves there', () => {
    const base = layout([tab('a'), tab('m')], [tab('s')])
    const local = layout([tab('a')], [tab('s'), tab('m')])
    const remote = layout([tab('a'), tab('m'), tab('b')], [tab('s')])
    expect(ids(mergeSerializedLayouts(base, local, remote))).toEqual([['a', 'b'], ['s', 'm']])
  })

  it('keeps the remote order unless this window reordered the column', () => {
    const base = layout([tab('a'), tab('b'), tab('c')])
    const untouched = layout([tab('a'), tab('b'), tab('c')])
    const reordered = layout([tab('c'), tab('a'), tab('b')])
    const remote = layout([tab('b'), tab('a'), tab('c'), tab('d')])
    expect(ids(mergeSerializedLayouts(base, untouched, remote))).toEqual([['b', 'a', 'c', 'd']])
    expect(ids(mergeSerializedLayouts(base, reordered, remote))).toEqual([['c', 'a', 'b', 'd']])
  })

  it('does not duplicate a tab the remote already has by pane group (two windows adopted one session)', () => {
    const base = layout([tab('a')])
    const local = layout([tab('a'), tab('adopted-pg-z', 'pg-z')])
    const remote = layout([tab('a')], [tab('their-z', 'pg-z')])
    expect(ids(mergeSerializedLayouts(base, local, remote))).toEqual([['a'], ['their-z']])
  })

  it('does not duplicate the pinned Chat tab when each window minted its own id', () => {
    const base = layout([])
    const local = layout([systemTab('chat-mine', 'chat'), tab('n')])
    const remote = layout([systemTab('chat-theirs', 'chat')])
    expect(ids(mergeSerializedLayouts(base, local, remote))).toEqual([['chat-theirs', 'n']])
  })

  it('never leaves column 0 empty while a later column has tabs', () => {
    const base = layout([tab('a')], [tab('s')])
    const local = layout([], [tab('s')])
    const remote = layout([tab('a')], [tab('s')])
    const merged = mergeSerializedLayouts(base, local, remote)
    expect(ids(merged)).toEqual([['s']])
    expect(merged.splitCount).toBeUndefined()
  })

  it('never writes the focused column (D2)', () => {
    const base = layout([tab('a')], [tab('s')])
    const local = { ...layout([tab('a')], [tab('s'), tab('n')]), activeGroupIndex: 1 }
    const remote = { ...layout([tab('a')], [tab('s')]), activeGroupIndex: 1 }
    expect('activeGroupIndex' in mergeSerializedLayouts(base, local, remote)).toBe(false)
  })

  it('with no base, keeps every remote tab and adds the local ones it lacks (next to their local neighbour)', () => {
    const local = layout([tab('a'), tab('n')])
    const remote = layout([tab('a'), tab('b')])
    expect(ids(mergeSerializedLayouts(null, local, remote))).toEqual([['a', 'n', 'b']])
  })
})

describe('canonicalLayoutJson', () => {
  it('ignores key order, per-window fields, and locked:false vs absent', () => {
    const a = { version: 2, tabs: [{ ...tab('a'), locked: false }], activeGroupIndex: 1, activeTabId: 'a' } as SerializedLayout
    const b = { tabs: [JSON.parse(JSON.stringify(tab('a')))], version: 2 } as SerializedLayout
    expect(canonicalLayoutJson(a)).toBe(canonicalLayoutJson(b))
  })

  it('sees a real change', () => {
    expect(canonicalLayoutJson(layout([tab('a')]))).not.toBe(canonicalLayoutJson(layout([tab('a', 'pg-a', { title: 'x' })])))
    expect(canonicalLayoutJson(layout([tab('a')]))).not.toBe(canonicalLayoutJson(layout([tab('a')], [])))
  })
})

describe('collapseEmptyLeadingColumns', () => {
  it('shifts left only while a later column has tabs', () => {
    expect(collapseEmptyLeadingColumns([[], [], [1]])).toEqual([[1]])
    expect(collapseEmptyLeadingColumns([[], []])).toEqual([[], []])
    expect(collapseEmptyLeadingColumns([[1], []])).toEqual([[1], []])
  })
})
