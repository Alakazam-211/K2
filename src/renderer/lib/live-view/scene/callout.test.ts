// The refusal on the picture (S6; live-scene-v2 mock, "Refused on the
// render"): a callout by the object that was refused, with a pin on its box
// and a line to it, in the holder's presence colour (no holder: the warning
// colour). The words are the shell's; the code only in the tooltip.

import { describe, expect, it } from 'vitest'
import { stillCamera } from './project'
import { standInCamera } from './stand-in/camera'
import { sampleBounds } from './stand-in/shapes'
import { stillDoc, viewOnScreen } from './picture-model'
import { SCENARIOS } from './scenarios-more'
import { simulate } from './turn'
import type { SimState } from './sim'
import { calloutOf } from './callout'
import { presenceSlots } from './overlay'
import { ATLAS } from './wording'

const at = (id: string, label: string): SimState => {
  const sc = SCENARIOS.find((s) => s.id === id)!
  return simulate(sc, sc.steps.findIndex((s) => s.label === label))
}
const still = (S: SimState) => ({ camera: stillCamera(standInCamera(stillDoc(S), viewOnScreen(S, { view: null, pick: undefined }), 16 / 9)), width: 1920, height: 1080 })

describe('the refusal callout', () => {
  it('sits by the refused object, in the holder’s colour, the code in its tooltip', () => {
    const S = at('refuse', 'refused')
    const c = calloutOf(S, { still: still(S), bounds: sampleBounds })!
    expect(c).toMatchObject({ a: '@obj:12', slot: presenceSlots(S)[ATLAS], held: true })
    expect(c.anchor.x1).toBeGreaterThan(c.anchor.x0)
    expect(c.anchor.y1).toBeGreaterThan(c.anchor.y0)
    expect(c.tip).toBe('Code MPC-E301')
  })

  it('goes once the hold is released (the next step)', () => {
    const S = at('refuse', 'released')
    expect(calloutOf(S, { still: still(S), bounds: sampleBounds })).toBeNull()
  })

  it('no holder (animation drives it): the warning colour', () => {
    const S = at('limits', 'driven')
    expect(calloutOf(S, { still: still(S), bounds: sampleBounds })).toMatchObject({ a: '@obj:23', slot: null, held: false, tip: 'Code blend_driven' })
  })
})
