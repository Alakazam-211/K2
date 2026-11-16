// What the picture draws over the still (S4): boxes from each object's
// bounds through the still's camera, holds in the holder's presence colour
// with one "Atlas · dining set" label per hold, your pick and others'
// selections, the live change (ghost, slide, tag, arrows, settle), dashed
// approximate boxes, the hidden-object rule and chips for what is off the
// picture. Run against the sample's own turn, through the stand-in's
// cameras and bounds, so the numbers are the ones the tab draws.

import { describe, expect, it } from 'vitest'
import { basis, stillCamera } from './project'
import { geomOf } from './overlay-geom'
import { standInCamera } from './stand-in/camera'
import { sampleBounds } from './stand-in/shapes'
import { stillDoc, viewOnScreen } from './picture-model'
import { SCENARIOS } from './scenarios-more'
import { TURN, simulate, simulateRaw } from './turn'
import { applyGroup, finish, type SimEvent, type SimState } from './sim'
import { ACTORS, ATLAS, BROOK, SAM } from './wording'
import { liveChange, overlayDraw, presenceSlots, slotVar, type OverlayDraw } from './overlay'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!

/** The still on screen's camera, as the stand-in draws it. */
function still(S: SimState, view?: string) {
  const v = view ?? viewOnScreen(S, { view: null, pick: undefined })
  return { camera: stillCamera(standInCamera(stillDoc(S), v, 16 / 9)), width: 1920, height: 1080 }
}
const draw = (S: SimState, o: { pick?: string | null; view?: string; t?: number } = {}): OverlayDraw => overlayDraw(S, { pick: o.pick ?? null, still: still(S, o.view), bounds: sampleBounds, t: o.t ?? 1 })
const boxOf = (d: OverlayDraw, a: string) => d.boxes.find((b) => b.a === a)
/** One more step on top of a state. */
function plus(S: SimState, events: SimEvent[]): SimState {
  applyGroup(S, { at: null, label: 'x', events }, S.i + 1)
  return finish(S)
}
const hold = (who: string, keys: string[], label: string): SimEvent => ({ kind: 'claim_started', claim: { id: `c-${who}-${keys[0]}`, actor: ACTORS[who], root: 'objs', keys, label, hold: true, started_ms: null, expires_ms: null } })

describe('presence colours on the picture', () => {
  it('one token per person: you are presence-4 (your pick), others take the nearest free hue', () => {
    const S = simulate(TURN, 11)
    const slots = presenceSlots(S)
    expect(slots[S.self]).toBe(4)
    expect(slots[ATLAS]).toBe(1) // violet
    expect(slots[SAM]).toBe(2) // blue
    expect(slots[BROOK]).toBe(3) // the one left
    expect(slotVar(1)).toBe('var(--color-presence-1)')
    expect(slotVar('file')).toBe('var(--color-text-secondary)')
  })
})

describe('holds', () => {
  it('a thin box per held object in the holder’s colour, one label per hold', () => {
    const d = draw(simulate(TURN, 2))
    for (const a of ['@obj:10', '@obj:11', '@obj:12']) expect(boxOf(d, a)).toMatchObject({ style: 'hold', slot: 1 })
    expect(boxOf(d, '@obj:20')).toMatchObject({ style: 'hold', slot: 3 })
    const holds = d.tags.filter((t) => t.kind === 'hold')
    expect(holds.map((t) => `${t.who} · ${t.text}`)).toEqual(['Atlas · dining set', 'Brook · vase'])
    expect(holds[0].title).toBe('Atlas is holding Table, Chair.001 and Chair.002: arranging the dining set')
    // The label sits over the whole group.
    const group = ['@obj:10', '@obj:11', '@obj:12'].map((a) => boxOf(d, a)!.rect)
    expect(holds[0].anchor).toEqual({ x0: Math.min(...group.map((r) => r.x0)), y0: Math.min(...group.map((r) => r.y0)), x1: Math.max(...group.map((r) => r.x1)), y1: Math.max(...group.map((r) => r.y1)) })
  })

  it('a hold that ends leaves the picture', () => {
    const d = draw(simulate(TURN, 11))
    expect(d.boxes.filter((b) => b.style === 'hold')).toEqual([])
    expect(d.tags.filter((t) => t.kind === 'hold')).toEqual([])
  })
})

describe('selections', () => {
  it('your pick in your colour with its name; Sam’s selection labelled on its front edge', () => {
    const S = simulate(sc('who'), 0)
    const d = draw(S, { pick: '@obj:15' })
    expect(boxOf(d, '@obj:15')).toMatchObject({ style: 'mine', slot: 4 })
    expect(d.tags.find((t) => t.kind === 'mine')).toMatchObject({ a: '@obj:15', text: 'Sofa', place: 'tag' })
    expect(boxOf(d, '@obj:4')).toMatchObject({ style: 'sel', slot: 2 })
    const sam = d.tags.find((t) => t.kind === 'sel' && t.who === 'Sam')!
    expect(sam.place).toBe('point')
    // On the floor's lowest visible edge, not at its middle.
    const floor = boxOf(d, '@obj:4')!.rect
    expect(sam.anchor.y0).toBeGreaterThan((floor.y0 + floor.y1) / 2)
  })

  it('an away person’s selection is drawn quieter and says so', () => {
    const S = simulate(sc('who'), 6)
    const d = draw(S, { view: '@scene:main' })
    expect(boxOf(d, '@obj:4')).toMatchObject({ style: 'sel', away: true })
    expect(d.tags.find((t) => t.kind === 'sel' && t.who === 'Sam')).toMatchObject({ away: true, text: '(away)' })
  })

  it('nothing on the picture while the connection is down', () => {
    expect(draw(simulate(sc('lost'), 1)).boxes).toEqual([])
  })
})

describe('the live change', () => {
  it('slides from the old place: at the start the box is where it was, with a ghost there, and no tag yet', () => {
    const S = simulate(TURN, 3)
    expect(liveChange(S)).toMatchObject({ phase: 'live', pending: true, direct: ['@obj:10'] })
    const d0 = draw(S, { t: 0 })
    const table0 = boxOf(d0, '@obj:10')!
    expect(table0.style).toBe('live')
    expect(d0.ghosts.find((g) => g.a === '@obj:10')!.d).toBe(table0.d)
    expect(d0.tags).toEqual(d0.tags.filter((t) => t.kind !== 'live'))
    expect(d0.arrows).toEqual([])
  })

  it('ends at the new place, tagged, with an arrow for the move; what rides along moves too', () => {
    const S = simulate(TURN, 3)
    const d = draw(S, { t: 1 })
    const table = boxOf(d, '@obj:10')!
    expect(table.d).not.toBe(d.ghosts.find((g) => g.a === '@obj:10')!.d)
    expect(boxOf(d, '@obj:20')).toMatchObject({ style: 'live', rides: true })
    const tag = d.tags.find((t) => t.kind === 'live')!
    expect(tag).toMatchObject({ a: '@obj:10', who: 'Atlas', text: 'moved Table', sub: 'Vase moves with it', slot: 1, fading: false })
    expect(d.arrows.map((a) => `${a.a} ${a.kind}`)).toEqual(['@obj:10 move'])
    // Halfway, it is between the two.
    const half = boxOf(draw(S, { t: 0.5 }), '@obj:10')!.rect
    const ghost = boxOf(draw(S, { t: 0 }), '@obj:10')!.rect
    expect((half.x0 - ghost.x0) * (table.rect.x0 - half.x0)).toBeGreaterThan(0)
  })

  it('settles when the new picture lands: a plain outline, the tag and arrows fade, no ghost', () => {
    const S = simulate(TURN, 4)
    expect(liveChange(S)).toMatchObject({ phase: 'settled', landed: true })
    const d = draw(S)
    expect(boxOf(d, '@obj:10')!.style).toBe('settled')
    expect(d.ghosts).toEqual([])
    expect(d.tags.find((t) => t.kind === 'live')).toMatchObject({ fading: true })
    expect(d.arrows.every((a) => a.fading)).toBe(true)
  })

  it('then it is history: Activity has it, the picture doesn’t', () => {
    const S = simulate(TURN, 9)
    expect(liveChange(S)).toBeNull()
    expect(draw(S).tags.filter((t) => t.kind === 'live')).toEqual([])
  })

  it('a turn draws an arc, a resize outward arrows', () => {
    expect(draw(simulate(TURN, 5)).arrows.map((a) => a.kind)).toContain('turn')
    expect(draw(simulate(TURN, 7)).arrows.map((a) => a.kind)).toContain('scale')
  })

  it('a rename isn’t redrawn: tagged, settled at once', () => {
    const S = simulate(sc('refuse'), 3)
    expect(draw(S).tags.find((t) => t.kind === 'live')).toMatchObject({ who: 'Brook', text: 'renamed Chair.002 to Dining chair' })
  })

  it('a failed picture keeps the box but loses the tag', () => {
    const d = draw(simulate(sc('failed'), 1))
    expect(boxOf(d, '@obj:10')!.style).toBe('live')
    expect(d.tags.filter((t) => t.kind === 'live')).toEqual([])
  })
})

describe('approximate boxes', () => {
  it('dashed when a modifier adds what the bounds miss (Shelf), or animation drives it (Clock hand)', () => {
    const S = simulate(sc('nomod'), 0)
    // The listing's approx is a bool; the why is approx_reason.
    expect(sampleBounds(S.D, '@obj:16')).toMatchObject({ approx: true, approx_reason: 'modifiers' })
    expect(sampleBounds(S.D, '@obj:15')).toMatchObject({ approx: false, approx_reason: null })
    expect(boxOf(draw(S, { pick: '@obj:16' }), '@obj:16')).toMatchObject({ approx: 'modifiers' })
    expect(draw(S, { pick: '@obj:16' }).tags.find((t) => t.kind === 'mine')!.text).toBe('Shelf · approximate')
    expect(boxOf(draw(S, { pick: '@obj:23' }), '@obj:23')).toMatchObject({ approx: 'driven' })
  })

  it('a driven child is dashed from the first frame of its parent’s move', () => {
    const d = draw(simulate(sc('nomod'), 3))
    expect(boxOf(d, '@obj:23')).toMatchObject({ rides: true, approx: 'driven' })
  })
})

describe('hidden and linked objects', () => {
  const base = (): SimState => simulateRaw(TURN, 8)

  it('a hidden object has no box when someone just selects or picks it', () => {
    const S = plus(base(), [{ kind: 'presence', who: SAM, type: 'selection', value: { root: 'objs', keys: ['@obj:6'] } }])
    expect(boxOf(draw(S, { pick: '@obj:6' }), '@obj:6')).toBeUndefined()
  })

  it('a dashed “(hidden)” ghost box when an agent holds it, with ONE label: the hold says it’s hidden', () => {
    const S = plus(base(), [hold(ATLAS, ['@obj:6'], 'rebuilding the left wall')])
    const d = draw(S)
    expect(boxOf(d, '@obj:6')).toMatchObject({ style: 'hold', hidden: true, slot: 1 })
    const on = d.tags.filter((t) => t.a === '@obj:6')
    expect(on.map((t) => [t.kind, t.who, t.text])).toEqual([['hold', 'Atlas', 'left wall (hidden)']])
  })

  it('a hold over several objects names the hidden ones in its one label', () => {
    const S = plus(base(), [hold(ATLAS, ['@obj:6', '@obj:1'], 'rebuilding the room')])
    const d = draw(S)
    expect(d.tags.filter((t) => t.kind === 'note' && (t.a === '@obj:6' || t.a === '@obj:1'))).toEqual([])
    expect(d.tags.find((t) => t.kind === 'hold' && t.title?.endsWith('rebuilding the room'))!.text).toBe('room (Wall.Left hidden)')
  })

  it('and when an agent acts on it', () => {
    const S = plus(base(), [{ kind: 'frame', frame: null, made: null, multi: [[['@obj:6', [-2.1, 0, 0.6], null]], ATLAS] }, { kind: 'preview', p: { rev: 64, state: 'rendering' } }])
    expect(boxOf(draw(S), '@obj:6')).toMatchObject({ style: 'live', hidden: true })
  })

  it('but not when a person holds it', () => {
    const S = plus(base(), [hold(SAM, ['@obj:6'], 'checking the wall')])
    expect(boxOf(draw(S), '@obj:6')).toBeUndefined()
  })

  it('the listing decides what is hidden: hidden: true with hidden_by', () => {
    expect(sampleBounds(simulate(TURN, 0).D, '@obj:6')).toMatchObject({ hidden: true, hidden_by: 'self' })
    expect(sampleBounds(simulate(TURN, 0).D, '@obj:15')).toMatchObject({ hidden: false, hidden_by: null })
    // Sofa isn't hidden in the doc; the listing says it is, so a person's pick of it draws nothing.
    const S = simulate(sc('who'), 0)
    const flagged = (D: Parameters<typeof sampleBounds>[0], a: string) => {
      const b = sampleBounds(D, a)
      return b && a === '@obj:15' ? { ...b, hidden: true, hidden_by: 'collection' } : b
    }
    expect(overlayDraw(S, { pick: '@obj:15', still: still(S), bounds: flagged, t: 1 }).boxes.find((b) => b.a === '@obj:15')).toBeUndefined()
  })

  it('an object linked from another file gets no box', () => {
    const S = simulate(sc('limits'), 2)
    expect(boxOf(draw(S, { pick: '@obj:24' }), '@obj:24')).toBeUndefined()
  })
})

describe('off the picture', () => {
  it('your pick out of this camera’s frame becomes a chip on the edge, not a tag', () => {
    const S = simulate(sc('who'), 0)
    const d = draw(S, { pick: '@obj:15', view: '@cam:Camera.001' })
    const chip = d.chips.find((c) => c.a === '@obj:15')!
    expect(chip).toMatchObject({ name: 'Sofa', slot: 4 })
    expect(['left', 'right', 'top', 'bottom']).toContain(chip.side)
    expect(d.tags.find((t) => t.kind === 'mine')).toBeUndefined()
  })

  it('on the picture, no chip', () => {
    expect(draw(simulate(sc('who'), 0), { pick: '@obj:15' }).chips).toEqual([])
  })
})

describe('geometry the browser can draw', () => {
  // A corner just in front of the camera projects tens of thousands of pixels away; a dashed
  // stroke that long stalls the renderer. Every edge is clipped to a band round the still.
  const cam = stillCamera(basis([0, -5, 1], [0, 1, 0], [0, 0, 1], 0.5, 16 / 9))
  const sv = { camera: cam, width: 1920, height: 1080 }
  const coords = (d: string): number[] => (d.match(/-?\d+(\.\d+)?/g) ?? []).map(Number)
  const BAND = 2 * 1920 + 1

  it('clips a box with a corner at the near plane to within one still of the picture', () => {
    const g = geomOf({ corners: [[-1, -4.97, 0], [1, -4.97, 0], [-1, 0, 0], [1, 0, 0], [-1, -4.97, 2], [1, -4.97, 2], [-1, 0, 2], [1, 0, 2]] }, sv)!
    expect(g).not.toBeNull()
    expect(Math.max(...coords(g.d).map(Math.abs))).toBeLessThanOrEqual(BAND)
    expect(g.rect.x0).toBeGreaterThanOrEqual(-1921)
    expect(g.rect.x1).toBeLessThanOrEqual(BAND)
  })

  it('in the sample’s close views, no box, ghost or arrow strays past that band', () => {
    const cases: [string, number, string, string | null][] = [['who', 2, '@cam:Camera.001', '@obj:15'], ['onpic', 1, '@cam:Camera.001', '@obj:15'], ['who', 3, '@obj:Table', null], ['turn', 3, '@obj:Table', null], ['onpic', 1, '@obj:Sofa', '@obj:15']]
    for (const [id, step, view, pick] of cases) {
      const d = draw(simulate(sc(id), step), { view, pick })
      for (const p of [...d.boxes, ...d.ghosts, ...d.arrows]) expect(Math.max(...coords(p.d).map(Math.abs)), `${id}@${step} ${view} ${p.a}`).toBeLessThanOrEqual(BAND)
    }
  })
})
