// The lanes while video is being prepared (T6, the design section's
// Preparing video): a clip that isn't ready is a plain block (no pictures);
// a clip that just got its pictures fades them in; a clip nobody can
// preview carries the no-preview glyph, highlighted when the monitor's
// "2 other clips can't be previewed" shows them; none of it outside
// preparing, except a source with no preview.

import { describe, expect, it } from 'vitest'
import { modelOf } from './fold'
import { DEFAULT_PV } from './picture'
import { prepMarks } from './prep-marks'
import { PREP } from './scenarios'
import { simulate } from './sim'
import { CTX, FACTS } from './world'

const marksAt = (i: number) => {
  const S = simulate(PREP, i, CTX)
  const M = modelOf(S.B)
  return { M, m: prepMarks(M, S.pv ?? DEFAULT_PV, FACTS), items: Object.values(M.items).filter((x) => x.kind === 'clip') }
}

describe('the lanes while preparing video', () => {
  it('only the ready clips show pictures; the ones that can’t be previewed are marked, not counted as waiting', () => {
    const { m, items } = marksAt(0)
    const ready = new Set(PREP.clips.first3)
    const unready = items.filter((x) => m.unready(x))
    expect(unready.length).toBeGreaterThan(5)
    for (const k of ready) expect(m.unready(items.find((x) => x.key === k)!)).toBe(false)
    const cant = items.filter((x) => m.noPreview(x)).map((x) => x.name)
    expect(cant.sort()).toEqual(['B-roll 05', 'B-roll 08'])
    expect(items.filter((x) => m.noPreview(x)).some((x) => m.unready(x))).toBe(false)
    expect(items.some((x) => m.highlight(x))).toBe(false)
  })

  it('“2 other clips can’t be previewed” highlights them; filling in fades the new ones in', () => {
    const hl = marksAt(1)
    expect(hl.items.filter((x) => hl.m.highlight(x)).map((x) => x.name).sort()).toEqual(['B-roll 05', 'B-roll 08'])
    const fill = marksAt(2)
    const fresh = fill.items.filter((x) => fill.m.fresh(x))
    expect(fresh.length).toBeGreaterThan(0)
    expect(fresh.some((x) => fill.m.unready(x))).toBe(false)
  })

  it('outside preparing nothing is marked but a source with no preview', () => {
    const S = simulate(PREP, 0, CTX)
    const M = modelOf(S.B)
    const it = Object.values(M.items).find((x) => x.kind === 'clip' && x.media)!
    const none = prepMarks(M, DEFAULT_PV, FACTS)
    expect([none.unready(it), none.fresh(it), none.noPreview(it), none.highlight(it)]).toEqual([false, false, false, false])
    const np = prepMarks(M, { ...DEFAULT_PV, nopreview: [it.media as string] }, FACTS)
    expect(np.noPreview(it)).toBe(true)
  })
})
