// The order a tile's bar gives way in: hint, then centring, then the picker
// folds to its menu button, then the bell, and the name last. The ⌘N hint
// always keeps its place: the name truncates before it would. Every step
// keeps one row.
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
const F = (o: Partial<TileBarFit>): TileBarFit => ({ hint: false, centred: false, bell: true, compact: false, asleepWord: true, ...o })

describe('fitTileBar', () => {
  it('centres the picker while the name fits in a side column', () => {
    // side = (360 - 96 - 16) / 2 = 124 ≥ 114 + 1
    expect(fitTileBar(agent(360))).toEqual(F({ hint: true, centred: true }))
  })

  it('lets the picker follow the name before the name gives anything up', () => {
    // side = (330 - 112) / 2 = 109 < 115; 114 + 8 + 96 + 8 + 52 + 1 = 279 ≤ 330
    expect(fitTileBar(agent(330))).toEqual(F({}))
    expect(fitTileBar(agent(279))).toEqual(F({}))
  })

  it('never gives up ⌘N: the name truncates instead', () => {
    expect(fitTileBar(agent(278))).toEqual(F({}))
    expect(fitTileBar(agent(120))).toEqual(F({}))
    expect('keys' in fitTileBar(agent(120))).toBe(false)
  })

  it('hides the hint first, keeping the picker centred', () => {
    // with hint: 114 + 8 + 40 = 162 > side 124; without: 114 ≤ 124
    expect(fitTileBar(agent(360, { hint: 40 }))).toEqual(F({ centred: true }))
    expect(fitTileBar(agent(460, { hint: 40 }))).toEqual(F({ hint: true, centred: true }))
  })

  it('keeps room for the controls, ⌘N included, when the name is short', () => {
    // name 30, controls 52: side must hold 52 + 1
    expect(fitTileBar(agent(225, { name: 30 })).centred).toBe(true)
    expect(fitTileBar(agent(200, { name: 30 })).centred).toBe(false)
  })

  it('a plain row (a doc tile): the hint goes, ⌘N stays', () => {
    const doc = (width: number): TileBarSizes => ({ width, gap: 8, name: 80, hint: 40, picker: 0, keys: 28, tools: 0, bell: 0, close: 24 })
    // 80 + 8 + 40 + 8 + 52 + 1 = 189
    expect(fitTileBar(doc(189))).toEqual(F({ hint: true }))
    expect(fitTileBar(doc(188))).toEqual(F({}))
    expect(fitTileBar(doc(100))).toEqual(F({}))
  })

  it('a read-only bar has no close button to keep room for', () => {
    expect(fitTileBar(agent(330, { close: 0, keys: 20 })).centred).toBe(false)
    expect(fitTileBar(agent(345, { close: 0, keys: 20 })).centred).toBe(true)
  })

  it('Thread view: the picker folds, then the bell, and only then the name; ⌘N stays', () => {
    // centred needs side ≥ max(114, 24 + 28 + 59) + 1 = 115: (width - 112) / 2 ≥ 115 → 342
    expect(fitTileBar(thread(342))).toEqual(F({ hint: true, centred: true }))
    expect(fitTileBar(thread(341)).centred).toBe(false)
    // following in full: 114 + 8 + 96 + 8 + 111 + 1 = 338
    expect(fitTileBar(thread(338))).toEqual(F({}))
    // the picker folds: 338 - 96 + 40 = 282
    expect(fitTileBar(thread(337))).toEqual(F({ compact: true }))
    expect(fitTileBar(thread(282))).toEqual(F({ compact: true }))
    // then the bell folds (search stays) and the name gives way
    expect(fitTileBar(thread(281))).toEqual(F({ compact: true, bell: false }))
    expect(fitTileBar(thread(140))).toEqual(F({ compact: true, bell: false }))
  })

  it('Chatter view (search only) has no bell to fold', () => {
    expect(fitTileBar(thread(140, { tools: 35, bell: 0 }))).toEqual(F({ compact: true }))
  })

  it('a picker that cannot fold keeps its capsule, and the bell goes next', () => {
    expect(fitTileBar(thread(337, { pickerCompact: 0 }))).toEqual(F({ bell: false }))
  })

  describe('an asleep agent (its state beats symmetry)', () => {
    // "asleep" 38px, the moon 12px, each after an 8px gap.
    const asleep = (width: number, o: Partial<TileBarSizes> = {}): TileBarSizes => thread(width, { asleep: 38, asleepIcon: 12, ...o })

    it('shows the word, centred while it fits', () => {
      // side ≥ 114 + 8 + 38 + 1 = 161: (width - 112) / 2 ≥ 161 → 434
      expect(fitTileBar(asleep(434))).toEqual(F({ hint: true, centred: true }))
    })

    it('moves the picker off centre before the word shrinks', () => {
      expect(fitTileBar(asleep(433))).toEqual(F({}))
      // following with the word: 160 + 8 + 96 + 8 + 111 + 1 = 384
      expect(fitTileBar(asleep(384))).toEqual(F({}))
    })

    it('then the word becomes the moon, before the picker folds', () => {
      // the moon: 384 - 38 + 12 = 358
      expect(fitTileBar(asleep(383))).toEqual(F({ asleepWord: false }))
      expect(fitTileBar(asleep(358))).toEqual(F({ asleepWord: false }))
      // the picker folds: 358 - 96 + 40 = 302
      expect(fitTileBar(asleep(357))).toEqual(F({ compact: true, asleepWord: false }))
      expect(fitTileBar(asleep(302))).toEqual(F({ compact: true, asleepWord: false }))
    })

    it('never hides the moon: the bell folds and the name gives way instead', () => {
      for (const width of [301, 200, 140, 60]) {
        const f = fitTileBar(asleep(width))
        expect(f).toEqual(F({ compact: true, bell: false, asleepWord: false }))
      }
    })

    it('a plain row keeps the mark too: the word, then the moon', () => {
      const row = (width: number): TileBarSizes => ({ width, gap: 8, name: 80, hint: 0, picker: 0, keys: 28, tools: 0, bell: 0, close: 24, asleep: 38, asleepIcon: 12 })
      // 80 + 46 + 8 + 52 + 1 = 187
      expect(fitTileBar(row(187))).toEqual(F({ hint: true }))
      // the moon: 187 - 38 + 12 = 161
      expect(fitTileBar(row(186))).toEqual(F({ asleepWord: false }))
      expect(fitTileBar(row(161))).toEqual(F({ asleepWord: false }))
      expect(fitTileBar(row(100))).toEqual(F({ asleepWord: false }))
    })

    it('an awake agent always reports the word slot (nothing to shrink)', () => {
      expect(fitTileBar(thread(140)).asleepWord).toBe(true)
    })
  })
})
