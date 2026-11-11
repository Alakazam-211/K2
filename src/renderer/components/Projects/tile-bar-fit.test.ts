// The order a tile's bar gives way in: hint, then centring, then the ⌘N
// slot, then the picker folds to its menu button, then the bell, and the
// name last. Every step keeps one row.
import { describe, expect, it } from 'vitest'
import { fitTileBar, type TileBarFit, type TileBarSizes } from './tile-bar-fit'

// An agent tile: picture + "Design System" ≈ 24 + 90, a 96px picker, ⌘N 20 + 8, × 24.
const agent = (width: number, o: Partial<TileBarSizes> = {}): TileBarSizes => ({
  width, gap: 8, name: 114, hint: 0, picker: 96, keys: 28, tools: 0, bell: 0, close: 24, ...o,
})
// The same tile in Thread view: bell 24, search 2 + 24, the rule 4 + 1 + 4 = 59;
// the picker folds to a 40px button.
const thread = (width: number, o: Partial<TileBarSizes> = {}): TileBarSizes =>
  agent(width, { tools: 59, bell: 24, pickerCompact: 40, ...o })
const F = (o: Partial<TileBarFit>): TileBarFit => ({ hint: false, centred: false, keys: false, bell: true, compact: false, ...o })

describe('fitTileBar', () => {
  it('centres the picker while the name fits in a side column', () => {
    // side = (360 - 96 - 16) / 2 = 124 ≥ 114 + 1
    expect(fitTileBar(agent(360))).toEqual(F({ hint: true, centred: true, keys: true }))
  })

  it('lets the picker follow the name before the name gives anything up', () => {
    // side = (330 - 112) / 2 = 109 < 115; 114 + 8 + 96 + 8 + 52 + 1 = 279 ≤ 330
    expect(fitTileBar(agent(330))).toEqual(F({ keys: true }))
    expect(fitTileBar(agent(279))).toEqual(F({ keys: true }))
  })

  it('drops the ⌘N slot before the name truncates', () => {
    expect(fitTileBar(agent(278))).toEqual(F({}))
  })

  it('hides the hint first, keeping the picker centred', () => {
    // with hint: 114 + 8 + 40 = 162 > side 124; without: 114 ≤ 124
    expect(fitTileBar(agent(360, { hint: 40 }))).toEqual(F({ centred: true, keys: true }))
    expect(fitTileBar(agent(460, { hint: 40 }))).toEqual(F({ hint: true, centred: true, keys: true }))
  })

  it('keeps room for the controls when the name is short', () => {
    // name 30, controls 52: side must hold 52 + 1
    expect(fitTileBar(agent(225, { name: 30 })).centred).toBe(true)
    expect(fitTileBar(agent(200, { name: 30 })).centred).toBe(false)
  })

  it('a plain row (a doc tile): hint first, then ⌘N', () => {
    const doc = (width: number): TileBarSizes => ({ width, gap: 8, name: 80, hint: 40, picker: 0, keys: 28, tools: 0, bell: 0, close: 24 })
    // 80 + 8 + 40 + 8 + 52 + 1 = 189
    expect(fitTileBar(doc(189))).toEqual(F({ hint: true, keys: true }))
    expect(fitTileBar(doc(188))).toEqual(F({ keys: true }))
    // 80 + 8 + 52 + 1 = 141
    expect(fitTileBar(doc(140))).toEqual(F({}))
  })

  it('a read-only bar has no close button to keep room for', () => {
    expect(fitTileBar(agent(330, { close: 0, keys: 20 })).centred).toBe(false)
    expect(fitTileBar(agent(345, { close: 0, keys: 20 })).centred).toBe(true)
  })

  it('Thread view: ⌘N goes, then the picker folds, then the bell, and only then the name', () => {
    // centred needs side ≥ max(114, 24 + 28 + 59) + 1 = 115: (width - 112) / 2 ≥ 115 → 342
    expect(fitTileBar(thread(342))).toEqual(F({ hint: true, centred: true, keys: true }))
    expect(fitTileBar(thread(341)).centred).toBe(false)
    // following in full: 114 + 8 + 96 + 8 + 111 + 1 = 338
    expect(fitTileBar(thread(338))).toEqual(F({ keys: true }))
    // without ⌘N: 338 - 28 = 310
    expect(fitTileBar(thread(310))).toEqual(F({}))
    // the picker folds: 310 - 96 + 40 = 254
    expect(fitTileBar(thread(309))).toEqual(F({ compact: true }))
    expect(fitTileBar(thread(254))).toEqual(F({ compact: true }))
    // then the bell folds (search stays) and the name gives way
    expect(fitTileBar(thread(253))).toEqual(F({ compact: true, bell: false }))
    expect(fitTileBar(thread(140))).toEqual(F({ compact: true, bell: false }))
  })

  it('Chatter view (search only) has no bell to fold', () => {
    expect(fitTileBar(thread(140, { tools: 35, bell: 0 }))).toEqual(F({ compact: true }))
  })

  it('a picker that cannot fold keeps its capsule, and the bell goes after ⌘N', () => {
    expect(fitTileBar(thread(309, { pickerCompact: 0 }))).toEqual(F({ bell: false }))
  })
})
