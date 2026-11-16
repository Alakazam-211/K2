// Following on the scene (S5), the parts only the picture needs (the shell
// keeps whom you follow): the sample's scripted follows, when the one you
// follow stops, the finished words, what the say row points at (the view on
// screen, in K2 Render's view grammar), who looks through another view, how
// two views split the stage, and the turntable's frame under a drag.

import { describe, expect, it } from 'vitest'
import { SCENARIOS } from './scenarios-more'
import { TURN, simulate } from './turn'
import { parseView, SCENE_VIEW } from './view-address'
import { ATLAS, SAM } from './wording'
import { followEndWords, othersOnViews, paneRects, sayAtOf, scriptedFollow, stoppedAt, turntableFrameAt } from './follow-view'
import { coverBox } from './picture-source'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!
const at = (id: string, label: string) => {
  const s = sc(id)
  const i = s.steps.findIndex((x) => x.label === label)
  expect(i, `${id}: ${label}`).toBeGreaterThanOrEqual(0)
  return simulate(s, i)
}

describe('the sample’s scripted follows', () => {
  it('Follow Atlas, look away by picking a view, follow Sam', () => {
    expect(scriptedFollow(at('follow', 'follow Atlas'))).toEqual({ type: 'follow', who: ATLAS })
    expect(scriptedFollow(at('follow', 'say something'))).toBeNull()
    expect(scriptedFollow(at('follow', 'look away'))).toEqual({ type: 'away', who: ATLAS })
    expect(scriptedFollow(at('follow', 'follow Sam'))).toEqual({ type: 'follow', who: SAM })
    // Sam leaving isn't scripted: the shell sees Sam go.
    expect(scriptedFollow(at('follow', 'Sam leaves'))).toBeNull()
  })
})

describe('when the one you follow stops', () => {
  it('their turn ends or they let go of their hold, at this step only', () => {
    const j = TURN.steps.findIndex((s) => s.events.some((e) => e.kind === 'turn_end'))
    expect(stoppedAt(simulate(TURN, j), ATLAS)).toBe(true)
    expect(stoppedAt(simulate(TURN, j + 1), ATLAS)).toBe(false)
    expect(stoppedAt(simulate(TURN, j), SAM)).toBe(false)
  })

  it('reads as the editors and the timeline do', () => {
    expect(followEndWords({ who: ATLAS, name: 'Atlas', why: 'stopped' })).toEqual({ text: 'Atlas stopped. Stopped following.', back: 'Back to my place', again: 'Follow again' })
    expect(followEndWords({ who: SAM, name: 'Sam', why: 'left' })).toEqual({ text: 'Sam left the file. Stopped following.', back: 'Back to my place', again: null })
  })
})

describe('what the say row points at', () => {
  it('the view on screen, in the kit’s view grammar, named in words', () => {
    const D = simulate(TURN, 1).D
    expect(sayAtOf(D, SCENE_VIEW)).toEqual({ label: 'Scene', title: 'What you’re looking at: the whole scene', address: '@scene:main' })
    const t = sayAtOf(D, '@obj:Table')
    expect(t.address).toBe('@obj:Table')
    expect(t.label).toBe('Table, framed')
    for (const v of ['@scene:main', '@cam:Camera', '@obj:Table', '@turntable:3']) expect(parseView(sayAtOf(D, v).address), v).not.toBeNull()
    expect(sayAtOf(D, '@turntable:3').label).toBe('Turntable')
    // The shell's message shape: a view address and its words, no time.
    expect(Object.keys(sayAtOf(D, '@obj:Table')).sort()).toEqual(['address', 'label', 'title'])
  })
})

describe('people on other views', () => {
  it('everyone else whose view isn’t yours, with that view in words', () => {
    const S = at('who', 'on the render')
    const o = othersOnViews(S, SCENE_VIEW)
    expect(o.map((p) => `${p.name}: ${p.label}`)).toEqual(['Brook: Camera 2'])
    expect(othersOnViews(S, '@cam:Camera.001').map((p) => p.name)).toEqual(['Sam', 'Atlas'])
  })

  it('none while the connection is down', () => {
    expect(othersOnViews(at('lost', 'lost'), SCENE_VIEW)).toEqual([])
  })
})

describe('two views', () => {
  it('side by side only when each half is at least 4:3, else one above the other', () => {
    expect(paneRects(2520, 900, 2).split).toBe('side')
    expect(paneRects(1100, 700, 2).split).toBe('stacked')
    expect(paneRects(1100, 700, 1)).toEqual({ split: 'one', panes: [{ x: 0, y: 0, w: 1100, h: 700 }] })
    const s = paneRects(2520, 900, 2)
    expect(s.panes).toEqual([{ x: 0, y: 0, w: 1260, h: 900 }, { x: 1260, y: 0, w: 1260, h: 900 }])
    expect(paneRects(1100, 700, 2).panes).toEqual([{ x: 0, y: 0, w: 1100, h: 350 }, { x: 0, y: 350, w: 1100, h: 350 }])
  })
})

describe('a view in half the stage', () => {
  it('covers its half: the still fills it, cropped, centred, never short of an edge', () => {
    for (const [w, h] of [[1100, 350], [1260, 900], [560, 300], [700, 700]]) {
      const c = coverBox(w, h)
      expect(c.left).toBeLessThanOrEqual(0)
      expect(c.top).toBeLessThanOrEqual(0)
      expect(c.left + c.width).toBeGreaterThanOrEqual(w - 0.01)
      expect(c.top + c.height).toBeGreaterThanOrEqual(h - 0.01)
      expect(Math.abs(c.left + c.width / 2 - w / 2)).toBeLessThan(0.01)
      expect(Math.abs(c.top + c.height / 2 - h / 2)).toBeLessThan(0.01)
      expect(Math.abs(c.width / c.height - (c.shape === 'wide' ? 16 / 9 : 4 / 3))).toBeLessThan(0.01)
    }
    // A short wide half takes the wide still and crops its top and bottom.
    expect(coverBox(1100, 350)).toMatchObject({ shape: 'wide', left: 0, width: 1100 })
  })
})

describe('the turntable strip', () => {
  it('a drag across it picks one of the eight frames', () => {
    expect(turntableFrameAt(0, 400)).toBe(0)
    expect(turntableFrameAt(199, 400)).toBe(3)
    expect(turntableFrameAt(400, 400)).toBe(7)
    expect(turntableFrameAt(-50, 400)).toBe(0)
    expect(turntableFrameAt(999, 400)).toBe(7)
  })
})
