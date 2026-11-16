// The lanes' view rules (T3): lane heights S/M/L and what fits, zoom and
// fit, the drawn window (virtualising), the marks a clip carries (changed,
// held, refused, let go), whose playheads show and who is off screen, and
// the View choices saved per tab with their confirmation line.

import { beforeEach, describe, expect, it } from 'vitest'
import { lanesOf, modelOf, visibleItems } from './fold'
import {
  HEAD_W,
  HEAD_W_NARROW,
  clipMarks,
  confirmLine,
  defaultPpf,
  defaultPrefs,
  drawWindow,
  fitPpf,
  isBig,
  laneHeight,
  lanesHeight,
  loadPrefs,
  MAX_PPF,
  MIN_PPF,
  offscreenOf,
  openFitPpf,
  refitDue,
  playheadsOf,
  savePrefs,
  timeWidth,
  zoomStep,
} from './lanes-view'
import { nowOf } from './snapshot'
import { LARGE_SC, WATCH } from './scenarios'
import { at, sim } from './test-world'
import { ATLAS, BROOK, SAM, SELF } from './world'

describe('lane heights', () => {
  it('small, medium and large are 30/26, 58/48 and 92/72 px for video/audio', () => {
    expect([laneHeight('S', 'video'), laneHeight('S', 'audio')]).toEqual([30, 26])
    expect([laneHeight('M', 'video'), laneHeight('M', 'audio')]).toEqual([58, 48])
    expect([laneHeight('L', 'video'), laneHeight('L', 'audio')]).toEqual([92, 72])
  })

  it('the lanes’ height adds each lane, its 1 px rule and the 3 px split between video and audio', () => {
    const M = modelOf(sim(WATCH, 0).B)
    const kinds = lanesOf(M).map((t) => t.kind)
    expect(kinds).toEqual(['video', 'video', 'audio', 'audio'])
    expect(lanesHeight(M, 'M')).toBe(58 + 58 + 48 + 48 + 4 + 3)
  })

  it('a large timeline starts small; others start medium, pictures automatic, paging while playing', () => {
    expect(defaultPrefs(modelOf(sim(WATCH, 0).B))).toEqual({ lanes: 'M', pic: 'auto', scroll: 'page' })
    const L = modelOf(sim(LARGE_SC, 0).B)
    expect(isBig(L)).toBe(true)
    expect(defaultPrefs(L).lanes).toBe('S')
  })

  it('narrow lane heads keep only their short label (the name in a tooltip)', () => {
    expect(HEAD_W).toBe(244)
    expect(HEAD_W_NARROW).toBe(56)
  })
})

describe('zoom, fit and the drawn window', () => {
  const M = modelOf(sim(WATCH, 0).B)

  it('fit shows the whole edit in the lanes’ width', () => {
    const tw = 1000
    const p = fitPpf(M, tw)
    expect(M.endF * p).toBeLessThanOrEqual(tw - 30 + 1e-9)
    expect(M.endF * p).toBeGreaterThan(tw - 40)
  })

  it('zoom steps by half again, never past fit going out or 64 px a frame going in', () => {
    const tw = 1000
    const p = defaultPpf(M, tw)
    expect(zoomStep(p, 1, M, tw)).toBeCloseTo(p * 1.5)
    expect(zoomStep(p, -1, M, tw)).toBeCloseTo(p / 1.5)
    expect(zoomStep(fitPpf(M, tw), -1, M, tw)).toBeCloseTo(fitPpf(M, tw))
    expect(zoomStep(60, 1, M, tw)).toBe(64)
  })

  it('the lanes are as wide as the edit plus room after it, never narrower than the window', () => {
    expect(timeWidth(M, 1, 400)).toBeGreaterThanOrEqual(M.endF + 240)
    expect(timeWidth(M, 0.001, 900)).toBe(900)
  })

  it('a long edit draws only a window a screen either side of what shows', () => {
    const L = modelOf(sim(LARGE_SC, 0).B)
    const ppf = defaultPpf(L, 1000)
    const [f0, f1] = drawWindow(5000, 1000, ppf)
    expect(f0).toBeCloseTo((5000 - 1000) / ppf)
    expect(f1).toBeCloseTo((5000 + 2000) / ppf)
    const drawn = lanesOf(L).reduce((n, t) => n + visibleItems(t, f0, f1).length, 0)
    expect(drawn).toBeLessThan(L.count / 4)
    expect(drawn).toBeGreaterThan(0)
  })
})

describe('what a clip carries: an outline and a dot, never words', () => {
  it('a hold outlines its clips in the holder’s colour', () => {
    const S = sim(WATCH, at(WATCH, 'Atlas holds'))
    const m = clipMarks(S)
    expect(m['@item:2'].held?.who).toBe(ATLAS)
    expect(m['@item:13'].held?.colour).toBeTruthy()
  })

  it('a fresh change gets the changed dot; after the next change it stays outlined, without the dot', () => {
    const S = sim(WATCH, at(WATCH, 'trim'))
    expect(clipMarks(S)['@item:2'].changed).toMatchObject({ fresh: true })
    const S2 = sim(WATCH, at(WATCH, 'move'))
    expect(clipMarks(S2)['@item:2'].changed).toMatchObject({ fresh: false })
    expect(clipMarks(S2)['@item:13'].changed).toMatchObject({ fresh: true })
  })

  it('a refused verb marks its clip with the refusal; letting go marks the clips let go', () => {
    expect(clipMarks(sim(WATCH, at(WATCH, 'refused')))['@item:2'].refused).toBeTruthy()
    const go = clipMarks(sim(WATCH, at(WATCH, 'Atlas lets go')))
    expect(go['@item:2'].released).toBeTruthy()
    expect(go['@item:13'].released).toBeTruthy()
  })
})

describe('playheads on the ruler', () => {
  it('everyone with a place has a playhead; yours is the monitor’s', () => {
    const S = sim(WATCH, at(WATCH, 'sharp'))
    const M = modelOf(S.B)
    const hs = playheadsOf(S, M, nowOf(S))
    const ids = hs.map((h) => h.id)
    expect(hs.find((h) => h.self)!.id).toBe(SELF)
    expect(hs.find((h) => h.self)!.name).toBe('You')
    expect(ids).toEqual(expect.arrayContaining([SAM, ATLAS]))
    expect(ids).not.toContain(BROOK)
  })

  it('people past either end of the window are off screen, nearest first', () => {
    const hs = [
      { id: 'a', name: 'A', f: 10, colour: '#111111', self: false, away: false },
      { id: 'b', name: 'B', f: 500, colour: '#111111', self: false, away: false },
      { id: 'c', name: 'C', f: 2000, colour: '#111111', self: false, away: false },
      { id: 'd', name: 'D', f: 900, colour: '#111111', self: false, away: false },
      { id: 'me', name: 'You', f: 5000, colour: '#111111', self: true, away: false },
      { id: 'gone', name: 'Gone', f: 5000, colour: '#111111', self: false, away: true },
    ]
    const o = offscreenOf(hs, 100, 800)
    expect(o.left.map((h) => h.id)).toEqual(['a'])
    expect(o.right.map((h) => h.id)).toEqual(['d', 'c'])
  })
})

describe('the View menu’s choices, saved per tab', () => {
  beforeEach(() => {
    try {
      localStorage.clear()
    } catch {
      /* no storage in this run */
    }
  })

  it('says what was chosen and that it is saved for this tab', () => {
    expect(confirmLine('lanes', 'L')).toBe('Lane height: Large. Saved for this tab.')
    expect(confirmLine('pic', 'headtail')).toBe('Pictures on clips: First and last. Saved for this tab.')
    expect(confirmLine('scroll', 'centre')).toBe('While playing: Keep the playhead centred. Saved for this tab.')
  })

  it('keeps one tab’s choice apart from another’s', () => {
    const M = modelOf(sim(WATCH, 0).B)
    savePrefs('tab-a', { lanes: 'L', pic: 'off', scroll: 'centre' })
    expect(loadPrefs('tab-a', M)).toEqual({ lanes: 'L', pic: 'off', scroll: 'centre' })
    expect(loadPrefs('tab-b', M)).toEqual(defaultPrefs(M))
  })
})

describe('a real file opens fitted', () => {
  it('fits the whole edit across the lanes with a small margin, clamped to the zoom range', () => {
    // 8 s at 24 fps across 1000 px: 970 px for 192 frames.
    expect(openFitPpf(192, 1000)).toBeCloseTo(970 / 192, 6)
    expect(192 * openFitPpf(192, 1000)).toBeLessThan(1000)
    expect(openFitPpf(1, 1000)).toBeLessThanOrEqual(MAX_PPF)
    expect(openFitPpf(10_000_000, 1000)).toBe(MIN_PPF)
  })

  it('re-fits only when the length changes a lot (a quarter, at least 2 s)', () => {
    expect(refitDue(192, 200)).toBe(false)
    expect(refitDue(192, 260)).toBe(true)
    expect(refitDue(2400, 2500)).toBe(false)
    expect(refitDue(2400, 3200)).toBe(true)
  })
})
