// The cell-derived values kept as KeyedWatch keeps them (names the grid lists
// only; a page's text sets, a change's null removes), and the sample's view
// schema in the kit's grid form.

import { describe, expect, it } from 'vitest'
import type { GridSlot, KeyedPageFrame } from '@k2/mpc-client'
import { A1, cellDerived, NO_CELL_DERIVED, withFrameDerived, withPageDerived } from './cell-derived'
import { sampleSnapshotFrames } from './sample-source'

const grid = { cell_derived: [A1] } as GridSlot
const page = (derived: KeyedPageFrame['derived']): KeyedPageFrame => ({ type: 'keyed_page', seq: 2, upto: 20, page: 1, pages: 1, root: 'cells', entries: [], derived })

describe('cell-derived values', () => {
  it("keeps a page's values for the grid's names only, and reads them as cellDerived(name, unit, field)", () => {
    const v = withPageDerived(NO_CELL_DERIVED, page({ a1: { r3: { c5: '=C4+D4' } }, other: { r3: { c5: 'x' } } }), grid)
    expect(cellDerived({ cellDerived: v }, A1, 'r3', 'c5')).toBe('=C4+D4')
    expect(cellDerived({ cellDerived: v }, 'other', 'r3', 'c5')).toBeNull()
    expect(withPageDerived(NO_CELL_DERIVED, page({ a1: { r3: { c5: '=1' } } }), { ...grid, cell_derived: undefined })).toBe(NO_CELL_DERIVED)
  })
  it("a change's text sets one, its null removes one, and the values before stay whole", () => {
    const v0 = withPageDerived(NO_CELL_DERIVED, page({ a1: { r3: { c5: '=C4+D4', c6: '=E3' } } }), grid)
    const v1 = withFrameDerived(v0, { units: { r3: { a1: { c5: null, c7: '=A1' } } } }, grid)
    const at = (v: typeof v0, f: string) => cellDerived({ cellDerived: v }, A1, 'r3', f)
    expect([at(v1, 'c5'), at(v1, 'c6'), at(v1, 'c7')]).toEqual([null, '=E3', '=A1'])
    expect([at(v0, 'c5'), at(v0, 'c7')]).toEqual(['=C4+D4', null])
  })
  it("the sample sends the kit's grid slot: cell_derived as names, style_hint as the kit names it", () => {
    const head = sampleSnapshotFrames()[0]
    const g = head.type === 'keyed_snapshot' ? head.view?.schema.grid : null
    expect(g?.cell_derived).toEqual(['a1'])
    expect(g?.style_hint).toEqual({ field: 's', default: '0', table: 'styles', parts: ['bold', 'fill', 'numfmt'] })
  })
})
