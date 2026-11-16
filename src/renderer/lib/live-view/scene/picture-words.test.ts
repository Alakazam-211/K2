// Every picture state's words and next step, from the scenarios that play
// them (the mock's v2.4/v2.5 words): ready, updating, first open, failed and
// retry, no still at all, no picture yet, empty, and the details rows.

import { describe, expect, it } from 'vitest'
import { pictureWords, type PictureContext } from './picture-words'
import { pictureContext, pictureDetails, pictureStatus } from './picture-model'
import { sampleBounds } from './stand-in/shapes'
import { simulate } from './turn'
import { SCENARIOS } from './scenarios-more'
import type { PictureStatus } from './picture-source'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!
const at = (id: string, label: string) => {
  const s = sc(id)
  const i = s.steps.findIndex((x) => x.label === label)
  expect(i, `${id}: ${label}`).toBeGreaterThanOrEqual(0)
  return simulate(s, i)
}
const wordsAt = (id: string, label: string) => {
  const S = at(id, label)
  return pictureWords(pictureStatus(S), pictureContext(S, undefined))
}

const PV0: PictureStatus = { shown: 60, want: 60, state: 'ready', renderMs: 200, reusedFrom: null, withoutModifiers: false, reason: null, landed: false, recovered: false, retrying: false, again: false }
const C0: PictureContext = { loading: false, tier: 'live', conn: 'ok', objects: true, markers: false, approx: null, camera: 'Camera 1', askAgent: 'Atlas' }

describe('the picture’s words', () => {
  it('ready: the still, no card, no chips', () => {
    const w = pictureWords(PV0, C0)
    expect(w).toMatchObject({ card: null, chips: [], older: null, still: true, ground: false, dim: false })
  })

  it('updating: the old still stays up with "Updating picture…", the lag on hover', () => {
    const w = pictureWords({ ...PV0, state: 'stale', want: 61 }, C0)
    expect(w.still).toBe(true)
    expect(w.chips).toEqual([{ id: 'updating', tone: 'busy', text: 'Updating picture…', title: 'The picture is one change behind while the new one is drawn; the boxes are already live.' }])
    expect(pictureWords({ ...PV0, state: 'stale', want: 63 }, C0).chips[0].title).toContain('3 changes behind')
  })

  it('first open: "Opening…" with its steps, connecting then drawing', () => {
    const a = wordsAt('first', 'connecting')
    expect(a.card?.kind).toBe('opening')
    expect(a.card?.title).toBe('Opening…')
    expect(a.card?.steps).toEqual([
      { label: 'Connecting', state: 'now' },
      { label: 'The list of objects', state: 'next' },
      { label: 'The first picture', state: 'next' },
    ])
    expect(a.still).toBe(false)
    const b = wordsAt('first', 'list in')
    expect(b.card?.steps?.map((s) => s.state)).toEqual(['done', 'done', 'now'])
    const c = wordsAt('first', 'first picture')
    expect(c.card).toBeNull()
    expect(c.still).toBe(true)
  })

  it('failed with an older still: keep it, mark it older, say so with Retry; then the retry in words', () => {
    const f = wordsAt('failed', 'couldn\'t update')
    expect(f.still).toBe(true)
    expect(f.older).toBe('Older picture: one change behind')
    expect(f.chips).toEqual([{ id: 'failed', tone: 'bad', text: 'Couldn’t update the picture', action: { id: 'retry', label: 'Retry' } }])
    const s = sc('failed')
    const retrying = pictureWords(pictureStatus(simulate(s, 2)), pictureContext(simulate(s, 2), undefined))
    expect(retrying.chips[0].text).toBe('Trying again to update the picture…')
    expect(wordsAt('failed', 'fails again').chips[0].text).toBe('Tried again, still couldn’t update it')
    const drawn = wordsAt('failed', 'drawn')
    expect(drawn.chips).toEqual([{ id: 'updated', tone: 'ok', text: 'Picture updated' }])
    expect(drawn.older).toBeNull()
  })

  it('no still at all: "Couldn’t draw this view" with Retry and Try Camera 1', () => {
    const w = wordsAt('failfirst', 'failed')
    expect(w.still).toBe(false)
    expect(w.card).toMatchObject({ kind: 'nostill', title: 'Couldn’t draw this view', body: 'The picture didn’t finish. The outliner and numbers are still live.' })
    expect(w.card?.actions).toEqual([
      { id: 'retry', label: 'Retry', primary: true },
      { id: 'try-camera', label: 'Try Camera 1', primary: false },
    ])
  })

  it('no picture yet: the origin dots, "No picture for this file yet", Retry and Show the list', () => {
    const w = wordsAt('nopic', 'no picture')
    expect(w.ground).toBe(true)
    expect(w.still).toBe(false)
    expect(w.card).toMatchObject({ kind: 'nopicture', title: 'No picture for this file yet', body: 'K2 can’t read this file’s shapes yet. The list still updates live; the dots show where objects sit.', atTop: true })
    expect(w.card?.actions.map((a) => a.label)).toEqual(['Retry', 'Show the list'])
  })

  it('an empty scene says where objects come from, and offers to ask the agent', () => {
    const w = wordsAt('empty', 'no objects')
    expect(w.card).toMatchObject({ kind: 'empty', title: 'No objects in this scene' })
    expect(w.card?.actions).toEqual([{ id: 'ask', label: 'Ask Atlas', primary: true }])
  })

  it('without modifiers, approximate and the saved version are quiet chips with the why on hover', () => {
    expect(wordsAt('nomod', 'without modifiers').chips.map((c) => c.text)).toEqual(['Shown without modifiers'])
    expect(wordsAt('nomod', 'driven').chips.map((c) => c.text)).toContain('Clock hand is approximate')
    expect(wordsAt('nomod', 'approximate box').chips.map((c) => c.text)).toContain('Shelf’s box is approximate')
    expect(wordsAt('blob', 'saved version').chips.map((c) => c.text)).toEqual(['Picture of the saved version'])
  })

  it('a lost connection dims the picture', () => {
    expect(wordsAt('lost', 'lost').dim).toBe(true)
  })

  it('never shows a rev, a size, a time or an address on the picture', () => {
    for (const s of SCENARIOS)
      for (let i = 0; i < s.steps.length; i++) {
        const S = simulate(s, i)
        const w = pictureWords(pictureStatus(S), pictureContext(S, undefined))
        const text = [w.card?.title, w.card?.body, ...(w.card?.steps ?? []).map((x) => x.label), ...(w.card?.actions ?? []).map((a) => a.label), ...w.chips.map((c) => c.text), w.older].join(' ')
        expect(text, `${s.id} ${i}`).not.toMatch(/\brev \d|\d+ ?ms\b|\d+ ?px\b|@obj|@cam|@coll|undefined|null/)
      }
  })
})

describe('the details popover', () => {
  it('speaks plainly, and keeps the raw facts for "Copy for an agent"', () => {
    const S = at('nomod', 'approximate box')
    const d = pictureDetails(S, { view: '@scene:main', pick: undefined, size: { width: 1920, height: 1080 }, bounds: sampleBounds })
    expect(d.rows).toEqual([
      { label: 'Picture', value: 'Up to date' },
      { label: 'Size', value: '1920 × 1080, drawn in 164 ms' },
      { label: 'Looking at', value: 'the whole scene' },
      { label: 'Modifiers', value: 'Not shown in this picture' },
      { label: 'Selected', value: 'Shelf' },
      { label: 'Its box', value: 'Approximate: a modifier adds parts the box leaves out' },
    ])
    expect(d.forAgents).toBe('change 60 · picture of change 60 · @scene:main · @obj:16')
  })

  it('counts what the picture leaves out, quietly, only when there is any', () => {
    const S = at('nomod', 'approximate box')
    const rows = (meta: Parameters<typeof pictureDetails>[1]['meta']) => pictureDetails(S, { view: '@scene:main', pick: undefined, size: null, meta }).rows.filter((r) => r.label === 'Left out')
    expect(rows({ base_values: false, hidden_left_out: true, objects_hidden: 3, objects_not_evaluated: 1 }).map((r) => r.value)).toEqual(['3 hidden objects', '1 object that couldn’t be evaluated'])
    expect(rows({ base_values: false, hidden_left_out: true, objects_hidden: 1, objects_not_evaluated: 0 }).map((r) => r.value)).toEqual(['1 hidden object'])
    expect(rows({ base_values: false, hidden_left_out: false, objects_hidden: 0, objects_not_evaluated: 0 })).toEqual([])
    expect(rows(null)).toEqual([])
    expect(pictureDetails(S, { view: '@scene:main', pick: undefined, size: null, meta: { base_values: false, hidden_left_out: true, objects_hidden: 3, objects_not_evaluated: 1 } }).forAgents).toContain('objects_hidden 3 · objects_not_evaluated 1')
  })

  it('says how current the picture is while it redraws or failed', () => {
    const S = at('failed', 'couldn\'t update')
    expect(pictureDetails(S, { view: '@cam:1', pick: undefined, size: null }).rows.slice(0, 2)).toEqual([
      { label: 'Picture', value: 'One change behind; the redraw failed' },
      { label: 'Looking at', value: 'Camera 1, a camera in the file' },
    ])
  })
})
