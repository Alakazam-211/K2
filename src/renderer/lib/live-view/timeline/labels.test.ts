// Clip labels (v2.7b): the full name, else a whole short name, else the
// kind glyph. Never a fragment, never "…", never an identifier dropped.

import { describe, expect, it } from 'vitest'
import { modelOf } from './fold'
import { bestName, clipLabel, kindGlyphOf, shortNames, twoLines } from './labels'
import { LARGE } from './scenarios-large'
import { B30IMG } from './scenarios-monitor'

describe('short names', () => {
  it('drops a phrase that starts with a small word, never an identifier', () => {
    expect(shortNames('Harbour at dawn')).toEqual(['Harbour at dawn', 'Harbour'])
    expect(shortNames('Interview B')).toEqual(['Interview B'])
    expect(shortNames('Shot A again')).toEqual(['Shot A again'])
  })

  it('takes the part before " · " or ":", and drops leading zeros', () => {
    expect(shortNames('Lower third · Interview C')).toEqual(['Lower third · Interview C', 'Lower third'])
    expect(shortNames('Title: Rough cut')).toEqual(['Title: Rough cut', 'Title'])
    expect(shortNames('B-roll 03')).toEqual(['B-roll 03', 'B-roll 3'])
    expect(shortNames('Interview B · funding')).toEqual(['Interview B · funding', 'Interview B'])
  })

  it('gives the longest whole form that fits, else nothing', () => {
    expect(bestName('Harbour at dawn', 10)).toBe('Harbour')
    expect(bestName('Interview B', 10)).toBe('')
    expect(bestName('B-roll 03', 8)).toBe('B-roll 3')
    expect(bestName('B-roll 03', 2)).toBe('')
  })

  it('every name in the samples: each form is whole words of the name (or its zeros dropped), and none ends on a small word', () => {
    const names = new Set([...Object.values(modelOf(B30IMG).items), ...Object.values(modelOf(LARGE.B0).items)].map((x) => x.name))
    for (const full of names) {
      const forms = shortNames(full)
      expect(forms[0]).toBe(full)
      for (const x of forms) {
        expect(x).not.toMatch(/…|\.\.\./)
        expect(x).not.toMatch(/\b(at|of|the|on|in|to|by|for|with|from|and)$/i)
        const words = full.replace(/\b0+(\d)/g, '$1').split(/ · |:| /).filter(Boolean)
        for (const w of x.split(/ · |:| /).filter(Boolean)) expect(words.includes(w) || full.split(/ · |:| /).includes(w)).toBe(true)
      }
    }
  })
})

describe('clip labels, measured as painted', () => {
  const chars = (n: number) => (t: string) => t.length <= n

  it('shows the full name when it fits, else a short one, else the glyph', () => {
    expect(clipLabel('Harbour at dawn', 'video', chars(40))).toEqual({ text: 'Harbour at dawn', glyph: null, full: 'Harbour at dawn' })
    expect(clipLabel('Harbour at dawn', 'video', chars(8))).toEqual({ text: 'Harbour', glyph: null, full: 'Harbour at dawn' })
    expect(clipLabel('Interview B', 'video', chars(8))).toEqual({ text: null, glyph: 'video', full: 'Interview B' })
    expect(clipLabel('Interview B', 'video', chars(8), false).glyph).toBeNull()
  })

  it('titles and nests lead with their glyph', () => {
    expect(kindGlyphOf('generator', 'video')).toBe('title')
    expect(kindGlyphOf('nest', 'video')).toBe('nest')
    expect(kindGlyphOf('clip', 'audio')).toBe('audio')
    expect(clipLabel('Title: Rough cut', 'title', chars(6))).toEqual({ text: 'Title', glyph: 'title', full: 'Title: Rough cut' })
  })

  it('a status in two lines drops trailing phrases whole, never “…”', () => {
    expect(twoLines('holding 2 clips · since 14:02 · tightening the open', 20)).toBe('holding 2 clips')
    expect(twoLines('its hold on Music ended: the track was renamed outside K2', 30)).toBe('its hold on Music ended')
  })
})
