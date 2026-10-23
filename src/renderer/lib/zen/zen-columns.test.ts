// prd-garden-column-collapse-and-agent-list-v1 A1 (CA5, CA10, CA15) — the
// fold settings from the page answer, where each column is (this computer,
// per Garden), the narrow-window fold, the last-open rule and the width
// math. Pure: no DOM. Fixtures are made up (g-test0001).

import { describe, expect, it } from 'vitest'
import {
  attachZenColumnsStorageSync,
  createZenColumnsStore,
  parseZenColumnsDoc,
  zenAutoFolds,
  zenColumnFoldRefusal,
  zenColumnFolds,
  zenColumnKey,
  zenColumnLabel,
  zenLayoutColumns,
  ZEN_COLUMNS_KEY,
  type KeyValueStorage,
  type ZenColumnFoldInput,
} from './zen-columns'
import { parseZenGet, type ZenColumnFold } from './zen-page'
import { singleViewPage, zenPageForView } from './zen-rail-views'
import { zenClampedMinWidths, zenColumnMinWidthCss, ZEN_COLUMN_TAB_PX } from './zen-overflow'

const G = 'g-test0001'

function answer(columns: unknown[], split: number[] = [40, 60], mins: number[] = [240, 360]): unknown {
  return {
    page: {
      template: 'k2.texting@1',
      layout: { kind: 'columns', split, minWidths: mins, columns },
      widgets: [
        { id: 'agents', kind: 'agents', column: 0, props: {}, caps: [] },
        { id: 'chat', kind: 'conversation', column: 1, props: { agents: 'agents' }, caps: [] },
      ],
      controls: [],
    },
  }
}

class MemKv implements KeyValueStorage {
  data = new Map<string, string>()
  getItem(k: string): string | null {
    return this.data.get(k) ?? null
  }
  setItem(k: string, v: string): void {
    this.data.set(k, v)
  }
}

const FOLDS: ZenColumnFold[] = [
  { collapsible: false, collapsed: false, label: null },
  { collapsible: true, collapsed: false, label: 'Chat' },
]

function input(over: Partial<ZenColumnFoldInput> = {}): ZenColumnFoldInput {
  return {
    gardenId: G,
    folds: FOLDS,
    minWidths: [240, 360],
    saved: {},
    transient: new Map(),
    keepOpen: new Set(),
    width: null,
    gap: 12,
    ...over,
  }
}

describe('fold settings in the page answer (A1, CA5)', () => {
  it('no folds unless a column is collapsible; the keys ride per column', () => {
    const plain = parseZenGet(answer([{ size: 40, 'min-width': 240 }, { size: 60, 'min-width': 360 }]))
    expect(plain.layout.folds).toBeUndefined()
    expect(Object.keys(plain.layout).sort()).toEqual(['kind', 'minWidths', 'split'])
    // A daemon without `columns` at all (older): nothing folds.
    const old = parseZenGet({ page: { template: 'k2.texting@1', layout: { kind: 'columns', split: [40, 60], minWidths: [0, 0] }, widgets: [], controls: [] } })
    expect(old.layout.folds).toBeUndefined()

    const p = parseZenGet(
      answer([{ size: 40, 'min-width': 240 }, { size: 60, 'min-width': 360, collapsible: true, collapsed: true, label: 'Chat' }]),
    )
    expect(p.layout.folds).toEqual([
      { collapsible: false, collapsed: false, label: null },
      { collapsible: true, collapsed: true, label: 'Chat' },
    ])
    expect(p.layout.split).toEqual([40, 60])
  })

  it('a bad label falls back to "Column N"; collapsed without collapsible is ignored; one column never folds', () => {
    const p = parseZenGet(
      answer([
        { size: 40, collapsed: true, label: 'x'.repeat(25) },
        { size: 60, collapsible: true, label: 'Two\nlines' },
      ]),
    )
    expect(p.layout.folds).toEqual([
      { collapsible: false, collapsed: false, label: null },
      { collapsible: true, collapsed: false, label: null },
    ])
    expect(zenColumnLabel(p.layout.folds, 1)).toBe('Column 2')
    expect(zenColumnLabel(undefined, 0)).toBe('Column 1')
    const one = parseZenGet(answer([{ size: 100, collapsible: true }], [100], [0]))
    expect(one.layout.folds).toBeUndefined()
  })

  it('single-view pages (Projects, Tickets) carry no folds (CA15)', () => {
    const p = parseZenGet(answer([{ size: 40 }, { size: 60, collapsible: true }]))
    const withRail = {
      ...p,
      widgets: [...p.widgets, { id: 'nav', kind: 'nav-rail', column: 0, props: {}, caps: [], source: 'builtin' }],
    }
    expect(withRail.layout.folds).toBeDefined()
    expect(zenPageForView(withRail, 'tickets').layout.folds).toBeUndefined()
    expect(zenPageForView(withRail, 'projects').layout.folds).toBeUndefined()
    expect(singleViewPage(withRail, withRail.widgets[0]).layout).toEqual({ kind: 'columns', split: [100], minWidths: [0] })
    // My Home and Agents keep the Garden's folds.
    expect(zenPageForView(withRail, 'home').layout.folds).toEqual(withRail.layout.folds)
    expect(zenPageForView(withRail, 'agents').layout.folds).toEqual(withRail.layout.folds)
  })
})

describe('the saved folds: this computer, per Garden (A1)', () => {
  it('saves by "<garden>/<ncols>/<index>", and a second window follows through storage', () => {
    const kv = new MemKv()
    const one = createZenColumnsStore(kv)
    one.getState().save(zenColumnKey(G, 2, 1), true)
    expect(zenColumnKey(G, 2, 1)).toBe(`${G}/2/1`)
    expect(JSON.parse(kv.getItem(ZEN_COLUMNS_KEY) ?? 'null')).toEqual({ version: 1, folds: { [`${G}/2/1`]: true } })
    // A second window: its own store, the same storage, the storage event.
    const target = new EventTarget()
    const two = createZenColumnsStore(kv)
    const off = attachZenColumnsStorageSync(two, target, kv)
    one.getState().save(zenColumnKey(G, 2, 1), false)
    target.dispatchEvent(Object.assign(new Event('storage'), { key: ZEN_COLUMNS_KEY, newValue: kv.getItem(ZEN_COLUMNS_KEY) }))
    expect(two.getState().folds).toEqual({ [`${G}/2/1`]: false })
    off()
    // Another Garden's key never touches this one.
    one.getState().save(zenColumnKey('g-test0002', 2, 1), true)
    expect(one.getState().folds[`${G}/2/1`]).toBe(false)
  })

  it('a stored value that isn’t ours reads as nothing; storage that throws falls back to memory', () => {
    expect(parseZenColumnsDoc('{"version":2,"folds":{}}')).toBeNull()
    expect(parseZenColumnsDoc('not json')).toBeNull()
    expect(parseZenColumnsDoc('{"version":1,"folds":{"a/2/1":"yes","b/2/1":true}}')).toEqual({ version: 1, folds: { 'b/2/1': true } })
    const broken: KeyValueStorage = {
      getItem: () => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('blocked')
      },
    }
    const s = createZenColumnsStore(broken)
    s.getState().save(zenColumnKey(G, 2, 1), true)
    expect(s.getState().folds).toEqual({ [`${G}/2/1`]: true })
  })
})

describe('where each column is (A1, CA10)', () => {
  it('the file starts it; the person’s saved click wins; a widget’s fold wins over both and is never saved', () => {
    expect(zenColumnFolds(input()).collapsed).toEqual([false, false])
    const startsFolded = [FOLDS[0], { ...FOLDS[1], collapsed: true }]
    expect(zenColumnFolds(input({ folds: startsFolded })).collapsed).toEqual([false, true])
    expect(zenColumnFolds(input({ folds: startsFolded, saved: { [`${G}/2/1`]: false } })).collapsed).toEqual([false, false])
    expect(zenColumnFolds(input({ saved: { [`${G}/2/1`]: true } })).collapsed).toEqual([false, true])
    expect(
      zenColumnFolds(input({ saved: { [`${G}/2/1`]: true }, transient: new Map([[1, false]]) })).collapsed,
    ).toEqual([false, false])
  })

  it('a new column count ignores the old keys', () => {
    expect(zenColumnFolds(input({ saved: { [`${G}/3/1`]: true } })).collapsed).toEqual([false, false])
  })

  it('a fixed column never folds, whatever is saved', () => {
    expect(zenColumnFolds(input({ saved: { [`${G}/2/0`]: true } })).collapsed).toEqual([false, false])
  })

  it('never every column folded: the first opens', () => {
    const both: ZenColumnFold[] = [
      { collapsible: true, collapsed: false, label: null },
      { collapsible: true, collapsed: false, label: null },
    ]
    const s = zenColumnFolds(input({ folds: both, saved: { [`${G}/2/0`]: true, [`${G}/2/1`]: true } }))
    expect(s.collapsed).toEqual([false, true])
  })

  it('the last open column refuses to fold; a fixed column and a bad index are refused too', () => {
    const both: ZenColumnFold[] = [
      { collapsible: true, collapsed: false, label: null },
      { collapsible: true, collapsed: false, label: null },
    ]
    expect(zenColumnFoldRefusal(both, [false, true], 0, true)).toBe('last_open')
    expect(zenColumnFoldRefusal(both, [false, true], 1, false)).toBeNull()
    expect(zenColumnFoldRefusal(both, [false, false], 0, true)).toBeNull()
    expect(zenColumnFoldRefusal(FOLDS, [false, false], 0, true)).toBe('fixed')
    expect(zenColumnFoldRefusal(FOLDS, [false, false], 2, true)).toBe('bad_index')
    expect(zenColumnFoldRefusal(FOLDS, [false, false], -1, true)).toBe('bad_index')
    expect(zenColumnFoldRefusal(FOLDS, [false, false], 0.5, true)).toBe('bad_index')
    expect(zenColumnFoldRefusal(undefined, [false, false], 1, true)).toBe('fixed')
  })

  it('layout.get’s shape: index, label, collapsible, collapsed', () => {
    expect(zenLayoutColumns(FOLDS, [false, true])).toEqual([
      { index: 0, label: 'Column 1', collapsible: false, collapsed: false },
      { index: 1, label: 'Chat', collapsible: true, collapsed: true },
    ])
  })
})

describe('the narrow-window fold (CA15)', () => {
  const three: ZenColumnFold[] = [
    { collapsible: true, collapsed: false, label: null },
    { collapsible: true, collapsed: false, label: null },
    { collapsible: true, collapsed: false, label: null },
  ]
  const mins = [300, 300, 300]

  it('folds right to left while the min widths don’t fit, never the last open column, and isn’t saved', () => {
    // 3 × 300 + 2 × 12 = 924 fits.
    expect(zenAutoFolds(mins, three, [false, false, false], new Set(), 924, 12)).toEqual([false, false, false])
    // 923: the rightmost folds (300 + 300 + 28 + 24 = 652).
    expect(zenAutoFolds(mins, three, [false, false, false], new Set(), 923, 12)).toEqual([false, false, true])
    // 600: two fold (300 + 28 + 28 + 24 = 380).
    expect(zenAutoFolds(mins, three, [false, false, false], new Set(), 600, 12)).toEqual([false, true, true])
    // 100: never the last open one.
    expect(zenAutoFolds(mins, three, [false, false, false], new Set(), 100, 12)).toEqual([false, true, true])
    // Not measured yet: nothing.
    expect(zenAutoFolds(mins, three, [false, false, false], new Set(), null, 12)).toEqual([false, false, false])
    const s = zenColumnFolds(input({ folds: three, minWidths: mins, width: 600 }))
    expect(s.collapsed).toEqual([false, true, true])
    expect(s.auto).toEqual([false, true, true])
  })

  it('skips fixed columns and columns the person opened; counts what is already folded', () => {
    const mixed: ZenColumnFold[] = [three[0], { collapsible: false, collapsed: false, label: null }, three[2]]
    expect(zenAutoFolds(mins, mixed, [false, false, false], new Set(), 700, 12)).toEqual([false, false, true])
    expect(zenAutoFolds(mins, mixed, [false, false, false], new Set([2]), 700, 12)).toEqual([true, false, false])
    // Column 2 already folded by the person: 300 + 300 + 28 + 24 = 652 fits 700.
    expect(zenAutoFolds(mins, three, [false, false, true], new Set(), 700, 12)).toEqual([false, false, false])
  })
})

describe('the width share with folded columns (CA15)', () => {
  it('a folded column is its tab; the open ones share what is left', () => {
    expect(zenColumnMinWidthCss([240, 360], 1, [false, true])).toBe(ZEN_COLUMN_TAB_PX)
    expect(zenColumnMinWidthCss([240, 360], 0, [false, true])).toBe(
      `min(240px, calc((100% - 1 * var(--zen-gap) - ${ZEN_COLUMN_TAB_PX}px) * 1))`,
    )
    // Unchanged without folds.
    expect(zenColumnMinWidthCss([240, 360], 0, [false, false])).toBe('min(240px, calc((100% - 1 * var(--zen-gap)) * 0.4))')
    expect(zenColumnMinWidthCss([240, 360], 0)).toBe('min(240px, calc((100% - 1 * var(--zen-gap)) * 0.4))')
    expect(zenClampedMinWidths([400, 400, 400], 600, 12, [false, true, false])).toEqual([
      (600 - 24 - ZEN_COLUMN_TAB_PX) / 2,
      ZEN_COLUMN_TAB_PX,
      (600 - 24 - ZEN_COLUMN_TAB_PX) / 2,
    ])
    expect(zenClampedMinWidths([240, 360], 1200, 12, [false, true])).toEqual([240, ZEN_COLUMN_TAB_PX])
  })
})
