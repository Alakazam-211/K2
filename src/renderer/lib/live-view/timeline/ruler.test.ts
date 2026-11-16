// The ruler (v2.1, v2.7b): ticks by the density rule; the top row's
// playhead tags and marker flags share one 12px tag style, a tag wins over
// a flag, flags closer than 14 px merge, a marker label is the whole name
// with a capital or no label at all, and nothing on the row overlaps
// (the off-screen stacks' room included).

import { describe, expect, it } from 'vitest'
import { modelOf } from './fold'
import { FLAG_MERGE_PX, layoutRulerTop, markerText, rulerTicks, type RulerHead, type RulerMarker } from './ruler'
import { LARGE } from './scenarios-large'
import { B30 } from './world'

const M = modelOf(B30)
/** About the painted width of a 12px label (the tests' stand-in for measureText). */
const measure = (s: string): number => s.length * 7

describe('ruler ticks', () => {
  for (const ppf of [0.02, 0.1, 0.42, 0.85, 1.2, 3, 8, 24]) {
    it(`labels at least 110 px apart and minor ticks at least 12 px at ${ppf} px a frame`, () => {
      const ticks = rulerTicks(M, ppf, 0, 2000 / ppf)
      const labels = ticks.filter((t) => t.label)
      expect(labels.length).toBeGreaterThan(0)
      for (let i = 1; i < labels.length; i++) expect(labels[i].x - labels[i - 1].x).toBeGreaterThanOrEqual(110 - 1e-9)
      for (let i = 1; i < ticks.length; i++) expect(ticks[i].x - ticks[i - 1].x).toBeGreaterThanOrEqual(12 - 1e-9)
      // every label is a whole timecode on the frame grid
      for (const t of labels) expect(t.label).toMatch(/^\d\d:\d\d:\d\d:\d\d$/)
    })
  }

  it('draws only the window it is asked for (virtualised), from the window’s first step', () => {
    const ppf = 0.85
    const all = rulerTicks(M, ppf, 0, 20000)
    const part = rulerTicks(M, ppf, 10000, 10500)
    expect(part.length).toBeLessThan(all.length / 10)
    expect(part[0].f).toBeLessThanOrEqual(10000)
    expect(part[part.length - 1].f).toBeLessThanOrEqual(10500)
  })

  it('starts at the timeline’s own timecode, and a long edit stays sparse', () => {
    expect(rulerTicks(M, 0.85, 0, 400).find((t) => t.label)!.label).toBe('01:00:00:00')
    const L = modelOf(LARGE.B0)
    const t = rulerTicks(L, 0.004, 0, L.endF)
    expect(t.filter((x) => x.label).length).toBeLessThan(L.endF * 0.004 / 110 + 2)
  })
})

const head = (id: string, name: string, f: number): RulerHead => ({ id, name, f, colour: '#336699' })
const mk = (key: string, name: string, f: number): RulerMarker => ({ key, name, f, color: 'GREEN', note: '' })

function boxes(r: ReturnType<typeof layoutRulerTop>): { x0: number; x1: number; what: string }[] {
  const out = r.tags.map((t) => ({ x0: t.x0, x1: t.x0 + t.w, what: 'tag ' + t.head.name }))
  for (const f of r.flags) {
    out.push({ x0: f.x - 4, x1: f.x + 4, what: 'flag ' + f.markers[0].name })
    if (f.label) out.push({ x0: f.label.x0, x1: f.label.x0 + f.label.w, what: 'label ' + f.label.text })
  }
  return out
}
const overlap = (a: { x0: number; x1: number }, b: { x0: number; x1: number }): boolean => a.x0 < b.x1 && a.x1 > b.x0

describe('the ruler’s top row', () => {
  it('gives a marker its whole name with a capital, beside its flag', () => {
    const r = layoutRulerTop([], [mk('@mk:1', 'act two', 100)], 1, measure)
    expect(r.flags).toHaveLength(1)
    expect(r.flags[0].label?.text).toBe('Act two')
    expect(markerText('act two')).toBe('Act two')
    expect(r.flags[0].label!.x0).toBeGreaterThan(100)
  })

  it('merges flags closer than 14 px into one, every name kept for its tooltip, and no label', () => {
    const r = layoutRulerTop([], [mk('a', 'Note 1', 100), mk('b', 'Note 2', 100 + FLAG_MERGE_PX - 1), mk('c', 'Note 3', 200)], 1, measure)
    expect(r.flags.map((f) => f.markers.map((m) => m.name))).toEqual([['Note 1', 'Note 2'], ['Note 3']])
    expect(r.flags[0].label).toBeNull()
  })

  it('a playhead tag wins over a flag', () => {
    const r = layoutRulerTop([head('sam', 'Sam', 100)], [mk('a', 'Act two', 101)], 1, measure)
    expect(r.tags).toHaveLength(1)
    expect(r.flags).toHaveLength(0)
  })

  it('a label that would not fit whole is left off; the flag stays', () => {
    const r = layoutRulerTop([head('sam', 'Sam', 140)], [mk('a', 'A very long marker name here', 100)], 1, measure)
    expect(r.flags).toHaveLength(1)
    expect(r.flags[0].label).toBeNull()
  })

  it('tags step aside from each other and from the off-screen stacks, else hide', () => {
    const r = layoutRulerTop([head('a', 'Atlas', 100), head('b', 'Brook', 104), head('c', 'Kai', 990)], [], 1, measure, [{ x0: 960, x1: 1000 }], 1000)
    expect(r.tags.map((t) => t.head.id)).toEqual(['a', 'b'])
    expect(r.hidden.map((h) => h.id)).toEqual(['c'])
    expect(overlap({ x0: r.tags[0].x0, x1: r.tags[0].x0 + r.tags[0].w }, { x0: r.tags[1].x0, x1: r.tags[1].x0 + r.tags[1].w })).toBe(false)
  })

  it('nothing on the row overlaps, for any spread of heads and markers', () => {
    let seed = 7
    const rnd = (): number => ((seed = (seed * 16807) % 2147483647) / 2147483647)
    for (let run = 0; run < 60; run++) {
      const heads = Array.from({ length: 1 + Math.floor(rnd() * 5) }, (_, i) => head('h' + i, ['You', 'Sam', 'Atlas', 'Brook', 'Kai'][i], rnd() * 1200))
      const marks = Array.from({ length: Math.floor(rnd() * 12) }, (_, i) => mk('m' + i, `note ${i}`, rnd() * 1200))
      const reserved = [{ x0: 0, x1: 60 }, { x0: 1140, x1: 1200 }]
      const r = layoutRulerTop(heads, marks, 1, measure, reserved, 1200)
      const b = boxes(r)
      for (let i = 0; i < b.length; i++) {
        for (const z of reserved) expect(overlap(b[i], z), `${b[i].what} under a stack`).toBe(false)
        for (let j = i + 1; j < b.length; j++) {
          // a flag's own label sits beside it, never on it
          expect(overlap(b[i], b[j]), `${b[i].what} / ${b[j].what}`).toBe(false)
        }
      }
      for (const f of r.flags) if (f.label) expect(f.label.text).toBe(markerText(f.markers[0].name))
      expect(r.tags.length + r.hidden.length).toBe(heads.length)
    }
  })
})
