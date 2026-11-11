// The order a tile's bar gives way in: hint, then centring, then the ⌘N
// slot, and the name last.
import { describe, expect, it } from 'vitest'
import { fitTileBar, type TileBarSizes } from './tile-bar-fit'

// An agent tile: picture + "Design System" ≈ 24 + 90, a 96px picker, ⌘N 20 + 8, × 24.
const agent = (width: number, o: Partial<TileBarSizes> = {}): TileBarSizes => ({
  width, gap: 8, name: 114, hint: 0, picker: 96, keys: 28, close: 24, ...o,
})

describe('fitTileBar', () => {
  it('centres the picker while the name fits in a side column', () => {
    // side = (360 - 96 - 16) / 2 = 124 ≥ 114 + 1
    expect(fitTileBar(agent(360))).toEqual({ hint: true, centred: true, keys: true })
  })

  it('lets the picker follow the name before the name gives anything up', () => {
    // side = (330 - 112) / 2 = 109 < 115; 114 + 8 + 96 + 8 + 52 + 1 = 279 ≤ 330
    expect(fitTileBar(agent(330))).toEqual({ hint: false, centred: false, keys: true })
    expect(fitTileBar(agent(279))).toEqual({ hint: false, centred: false, keys: true })
  })

  it('drops the ⌘N slot before the name truncates', () => {
    expect(fitTileBar(agent(278))).toEqual({ hint: false, centred: false, keys: false })
  })

  it('hides the hint first, keeping the picker centred', () => {
    // with hint: 114 + 8 + 40 = 162 > side 124; without: 114 ≤ 124
    expect(fitTileBar(agent(360, { hint: 40 }))).toEqual({ hint: false, centred: true, keys: true })
    expect(fitTileBar(agent(460, { hint: 40 }))).toEqual({ hint: true, centred: true, keys: true })
  })

  it('keeps room for the controls when the name is short', () => {
    // name 30, controls 52: side must hold 52 + 1
    expect(fitTileBar(agent(225, { name: 30 })).centred).toBe(true)
    expect(fitTileBar(agent(200, { name: 30 })).centred).toBe(false)
  })

  it('a plain row (a doc tile): hint first, then ⌘N', () => {
    const doc = (width: number): TileBarSizes => ({ width, gap: 8, name: 80, hint: 40, picker: 0, keys: 28, close: 24 })
    // 80 + 8 + 40 + 8 + 52 + 1 = 189
    expect(fitTileBar(doc(189))).toEqual({ hint: true, centred: false, keys: true })
    expect(fitTileBar(doc(188))).toEqual({ hint: false, centred: false, keys: true })
    // 80 + 8 + 52 + 1 = 141
    expect(fitTileBar(doc(140))).toEqual({ hint: false, centred: false, keys: false })
  })

  it('a read-only bar has no close button to keep room for', () => {
    expect(fitTileBar(agent(330, { close: 0, keys: 20 })).centred).toBe(false)
    expect(fitTileBar(agent(345, { close: 0, keys: 20 })).centred).toBe(true)
  })
})
