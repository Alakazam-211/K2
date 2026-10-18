// 0.46.1 (gap 6): `[[widget]] weight` from the daemon's page answer, and the
// column flex the page draws from it.

import { describe, expect, it } from 'vitest'
import { parseZenGet, zenColumnFlex, type ZenWidgetDecl } from './zen-page'

function page(widgets: unknown[]): unknown {
  return {
    page: {
      template: 'k2.texting@1',
      layout: { kind: 'columns', split: [40, 60], minWidths: [240, 360] },
      widgets,
      controls: [],
    },
  }
}

describe('widget weight', () => {
  it('keeps a whole-number weight 1-12 on a column body widget and drops anything else', () => {
    const p = parseZenGet(
      page([
        { id: 'a', kind: 'agents', column: 0, weight: 3, props: {}, caps: [] },
        { id: 'b', kind: 'agents', column: 0, weight: 0, props: {}, caps: [] },
        { id: 'c', kind: 'agents', column: 1, weight: 13, props: {}, caps: [] },
        { id: 'd', kind: 'agents', column: 1, weight: 2.5, props: {}, caps: [] },
        { id: 'e', kind: 'agents', column: 1, edge: 'top', weight: 2, props: {}, caps: [] },
        { id: 'f', kind: 'agents', slot: 'top', weight: 2, props: {}, caps: [] },
      ]),
    )
    const w = Object.fromEntries(p.widgets.map((x) => [x.id, x.weight]))
    expect(w).toEqual({ a: 3, b: undefined, c: undefined, d: undefined, e: undefined, f: undefined })
  })

  it('zenColumnFlex: null without weights; otherwise each widget its share (absent = 1)', () => {
    const decl = (id: string, weight?: number): ZenWidgetDecl => ({
      id,
      kind: 'agents',
      column: 0,
      props: {},
      caps: [],
      source: 'builtin',
      ...(weight !== undefined ? { weight } : {}),
    })
    expect(zenColumnFlex([decl('a'), decl('b')])).toBeNull()
    expect(zenColumnFlex([])).toBeNull()
    expect(zenColumnFlex([decl('a', 2), decl('b')])).toEqual(
      new Map([
        ['a', '2 1 0%'],
        ['b', '1 1 0%'],
      ]),
    )
  })
})
