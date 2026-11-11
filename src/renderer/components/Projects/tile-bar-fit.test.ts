// The order a tile's bar gives way in: hint, then centring, then the ⌘N
// slot, then the bell, and the name last.
import { describe, expect, it } from 'vitest'
import { fitTileBar, type TileBarSizes } from './tile-bar-fit'

// An agent tile: picture + "Design System" ≈ 24 + 90, a 96px picker, ⌘N 20 + 8, × 24.
const agent = (width: number, o: Partial<TileBarSizes> = {}): TileBarSizes => ({
  width, gap: 8, name: 114, hint: 0, picker: 96, keys: 28, tools: 0, bell: 0, close: 24, ...o,
})
// The same tile in Thread view: bell 24, search 24 + 2, then 4 to the ×.
const thread = (width: number, o: Partial<TileBarSizes> = {}): TileBarSizes => agent(width, { tools: 54, bell: 24, ...o })

describe('fitTileBar', () => {
  it('centres the picker while the name fits in a side column', () => {
    // side = (360 - 96 - 16) / 2 = 124 ≥ 114 + 1
    expect(fitTileBar(agent(360))).toEqual({ hint: true, centred: true, keys: true, bell: true })
  })

  it('lets the picker follow the name before the name gives anything up', () => {
    // side = (330 - 112) / 2 = 109 < 115; 114 + 8 + 96 + 8 + 52 + 1 = 279 ≤ 330
    expect(fitTileBar(agent(330))).toEqual({ hint: false, centred: false, keys: true, bell: true })
    expect(fitTileBar(agent(279))).toEqual({ hint: false, centred: false, keys: true, bell: true })
  })

  it('drops the ⌘N slot before the name truncates', () => {
    expect(fitTileBar(agent(278))).toEqual({ hint: false, centred: false, keys: false, bell: true })
  })

  it('hides the hint first, keeping the picker centred', () => {
    // with hint: 114 + 8 + 40 = 162 > side 124; without: 114 ≤ 124
    expect(fitTileBar(agent(360, { hint: 40 }))).toEqual({ hint: false, centred: true, keys: true, bell: true })
    expect(fitTileBar(agent(460, { hint: 40 }))).toEqual({ hint: true, centred: true, keys: true, bell: true })
  })

  it('keeps room for the controls when the name is short', () => {
    // name 30, controls 52: side must hold 52 + 1
    expect(fitTileBar(agent(225, { name: 30 })).centred).toBe(true)
    expect(fitTileBar(agent(200, { name: 30 })).centred).toBe(false)
  })

  it('a plain row (a doc tile): hint first, then ⌘N', () => {
    const doc = (width: number): TileBarSizes => ({ width, gap: 8, name: 80, hint: 40, picker: 0, keys: 28, tools: 0, bell: 0, close: 24 })
    // 80 + 8 + 40 + 8 + 52 + 1 = 189
    expect(fitTileBar(doc(189))).toEqual({ hint: true, centred: false, keys: true, bell: true })
    expect(fitTileBar(doc(188))).toEqual({ hint: false, centred: false, keys: true, bell: true })
    // 80 + 8 + 52 + 1 = 141
    expect(fitTileBar(doc(140))).toEqual({ hint: false, centred: false, keys: false, bell: true })
  })

  it('a read-only bar has no close button to keep room for', () => {
    expect(fitTileBar(agent(330, { close: 0, keys: 20 })).centred).toBe(false)
    expect(fitTileBar(agent(345, { close: 0, keys: 20 })).centred).toBe(true)
  })

  it('Thread view: ⌘N goes, then the bell folds, and only then does the name truncate', () => {
    // centred needs side ≥ max(114, 24 + 28 + 54) + 1 = 115: (width - 112) / 2 ≥ 115 → 342
    expect(fitTileBar(thread(342))).toEqual({ hint: false, centred: true, keys: true, bell: true })
    // follow: 114 + 8 + 96 + 8 + 106 + 1 = 333
    expect(fitTileBar(thread(333))).toEqual({ hint: false, centred: false, keys: true, bell: true })
    // without ⌘N: 333 - 28 = 305
    expect(fitTileBar(thread(305))).toEqual({ hint: false, centred: false, keys: false, bell: true })
    // under that, the bell folds (search stays) and the name gives way
    expect(fitTileBar(thread(304))).toEqual({ hint: false, centred: false, keys: false, bell: false })
    expect(fitTileBar(thread(200))).toEqual({ hint: false, centred: false, keys: false, bell: false })
  })

  it('Chatter view (search only) has no bell to fold', () => {
    expect(fitTileBar(thread(200, { tools: 30, bell: 0 })).bell).toBe(true)
  })

  it('a stacked bar (the picker on its own row) keeps the bell while name, tools and × fit', () => {
    // 114 + 8 + 24 + 54 + 1 = 201
    expect(fitTileBar(thread(201, { stacked: true }))).toEqual({ hint: false, centred: false, keys: false, bell: true })
    expect(fitTileBar(thread(200, { stacked: true })).bell).toBe(false)
  })
})
