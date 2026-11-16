// The scene panel's words and rows, from the model: the section order, who's
// here and what they hold, Activity's lines, the outliner's rows (nesting,
// the eye, badges, holds, people's dots, pages still arriving, a search with
// no match) and Properties' numbers, as the mock draws them.

import { describe, expect, it } from 'vitest'
import { PANEL, activityItems, angle, fmt, objRow, openByDefault, outlinerModel, peopleRows, propsModel, recentMap } from './panel'
import { SCENARIOS } from './scenarios-more'
import { simulate } from './turn'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!
const at = (id: string, step: number) => simulate(sc(id), step)

describe('the panel', () => {
  it('runs Activity, People and holds, Outliner, Properties; narrow, People and Properties start closed', () => {
    expect(PANEL.map((s) => s.title)).toEqual(['Activity', 'People and holds', 'Outliner', 'Properties'])
    expect(PANEL.filter((s) => openByDefault(s.id, true)).map((s) => s.id)).toEqual(['activity', 'outliner'])
    expect(PANEL.every((s) => openByDefault(s.id, false))).toBe(true)
  })
})

describe('People and holds', () => {
  it('names each person with their view on one line and what they do below, you last', () => {
    const S = at('who', 0)
    const rows = peopleRows(S)
    expect(rows[rows.length - 1].self).toBe(true)
    expect(rows.map((r) => r.name)).toEqual(['Sam', 'Atlas', 'Brook', 'You'])
    const atlas = rows.find((r) => r.name === 'Atlas')!
    expect(atlas.what).toBe('arranging the dining set')
    expect(atlas.holding).toBe(true)
    expect(rows.find((r) => r.name === 'Sam')!.what).toBe('on Floor')
    expect(rows.find((r) => r.name === 'Brook')!.view).toBe('Camera 2')
    expect(rows.find((r) => r.name === 'Brook')!.camera).toBe(true)
  })

  it('says whom you follow, and an agent whose turn ended is done', () => {
    const S = at('who', 0)
    expect(peopleRows(S, { followingId: 'agent:brook' }).find((r) => r.name === 'Brook')!.following).toBe(true)
    const end = at('turn', sc('turn').steps.length - 1)
    expect(peopleRows(end).find((r) => r.name === 'Atlas')!.what).toBe('done')
  })
})

describe('Activity', () => {
  it('leads with a Now line for an agent’s hold, then the changes, newest first', () => {
    const items = activityItems(at('who', 0), 'living-room')
    expect(items[0]).toMatchObject({ kind: 'now', text: 'arranging the dining set' })
    expect(items.length).toBeLessThanOrEqual(4)
    expect(items.some((i) => i.kind === 'change')).toBe(true)
  })

  it('a hold let go reads “let go of the dining set”, and the refusal then says “was holding it”', () => {
    const items = activityItems(at('refuse', 2), 'living-room')
    expect(items.find((i) => i.kind === 'let-go')).toMatchObject({ text: ' let go of the dining set' })
    const r = items.find((i) => i.kind === 'refused')
    expect(r).toMatchObject({ who: 'Brook', text: ' couldn’t rename Chair.002: Atlas was holding it' })
  })

  it('an outside save is one line, with what it changed', () => {
    const items = activityItems(at('O2', 1), 'living-room')
    const line = items.find((i) => i.kind === 'change' && i.outside)
    expect(line && 'text' in line && line.text).toMatch(/^ · now the live file; earlier edits kept as a copy: resized Rug, moved Floor lamp$/)
  })

  it('the conversation sits above the Now lines when it is newest', () => {
    const items = activityItems(at('follow', 4), 'living-room')
    expect(items.slice(0, 2).map((i) => i.kind)).toEqual(['reply', 'said'])
    expect(items[0]).toMatchObject({ text: 'On it.', refName: 'Table' })
  })
})

describe('Outliner', () => {
  it('nests by parent, in position order, with the object count in the head', () => {
    const S = at('who', 0)
    const m = outlinerModel(S, { collapsed: new Set(), q: '' })
    expect(m.count).toBe('26')
    const top = m.rows.slice(0, 9).map((r) => `${'  '.repeat(r.depth)}${r.t === 'obj' ? m.T!.objs[r.addr].name : r.addr}`)
    expect(top).toEqual(['@coll:1', '  @coll:2', '    Floor', '    Wall.Back', '    Wall.Left', '    Window', '    Rug', '    Ceiling fan', '  @coll:3'])
  })

  it('a row has its eye, what can’t change, who holds it and who is on it', () => {
    const S = at('who', 0)
    const T = outlinerModel(S, { collapsed: new Set(), q: '' }).T!
    const rec = recentMap(S)
    expect(objRow(S, T, '@obj:6', rec).obj.hidden).toBe(true) // Wall.Left
    expect(objRow(S, T, '@obj:5', rec).obj.hidden).toBe(false) // Wall.Back
    expect(objRow(S, T, '@obj:9', rec).badge?.kind).toBe('quat') // Ceiling fan
    expect(objRow(S, T, '@obj:23', rec).badge?.kind).toBe('driven') // Clock hand
    expect(objRow(S, T, '@obj:24', rec).badge?.kind).toBe('linked') // Plant
    expect(objRow(S, T, '@obj:10', rec).hold).toMatchObject({ name: 'Atlas', label: 'arranging the dining set' })
    expect(objRow(S, T, '@obj:4', rec).on.map((p) => p.name)).toEqual(['Sam'])
  })

  it('collections start collapsed in a large scene; a search keeps matches flat; no match says so', () => {
    const S0 = at('large', 0)
    const m0 = outlinerModel(S0, { collapsed: new Set(S0.ui.collapsed), q: '' })
    expect(m0.count).toBe('2,000')
    expect(m0.rows.length).toBe(101)
    const S2 = at('large', 2)
    const m2 = outlinerModel(S2, { collapsed: new Set(S2.ui.collapsed), q: S2.ui.q })
    expect(m2.rows.filter((r) => r.t === 'obj').every((r) => m2.T!.objs[r.addr].name.toLowerCase().includes('obj_7_1'))).toBe(true)
    const S3 = at('large', 3)
    const m3 = outlinerModel(S3, { collapsed: new Set(S3.ui.collapsed), q: S3.ui.q })
    expect(m3.none).toBe(true)
    expect(m3.query).toBe('Lamp')
  })

  it('pages still arriving say how many more are coming', () => {
    const S = at('pin-rig', 1)
    const m = outlinerModel(S, { collapsed: new Set(), q: '' })
    expect(m.rows.find((r) => r.t === 'more')).toMatchObject({ n: 31 })
    const S2 = at('pin-rig', 2)
    expect(outlinerModel(S2, { collapsed: new Set(), q: '' }).rows.some((r) => r.t === 'more')).toBe(false)
  })

  it('while loading, skeleton rows', () => {
    const m = outlinerModel(at('first', 0), { collapsed: new Set(), q: '' })
    expect(m.rows.every((r) => r.t === 'skel')).toBe(true)
  })
})

describe('Properties', () => {
  it('numbers with 3 decimals, degrees, the units in the label, and who holds it', () => {
    const S = at('who', 0)
    const p = propsModel(S, '@obj:10', recentMap(S))
    expect(p.state).toBe('obj')
    if (p.state !== 'obj') return
    expect(p.obj.name).toBe('Table')
    expect(p.lenUnit).toBe('m')
    expect(p.nums!.loc.map(fmt)).toEqual(['0.200', '0.900', '0.000'])
    expect(p.hold?.label).toBe('arranging the dining set')
    expect(fmt(-0.0001)).toBe('0.000')
    expect(angle(Math.PI)).toBe('180.000')
    expect(angle(Math.PI, true)).toBe('3.142')
  })

  it('values that just changed keep what they were (Before)', () => {
    const S = at('turn', 4)
    const p = propsModel(S, '@obj:10', recentMap(S))
    expect(p.state === 'obj' && p.just?.xb?.loc).toEqual([0.6, 0.4, 0])
  })

  it('nothing picked, no objects, and the limits in plain sentences', () => {
    expect(propsModel(at('O3', 0), null, {})).toMatchObject({ state: 'nopick', text: 'Pick an object to see its numbers.' })
    expect(propsModel(at('empty', 0), null, {})).toMatchObject({ state: 'empty', text: 'No objects yet.' })
    const q = propsModel(at('limits', 1), null, {})
    expect(q.state === 'obj' && q.limit).toBe('It turns by quaternion, so its rotation can’t change live.')
    const arm = propsModel(at('pin-rig', 3), null, {})
    expect(arm.state === 'obj' && arm.attached).toBe('Attached to Base, which also turns and moves it. Where it sits in the scene: -1.500, -2.000, 1.000.')
    const walker = propsModel(at('pin-outliner', 1), null, {})
    expect(walker.state === 'obj' && walker.lenUnit).toBe('units') // no metres per unit in the pinned outliner scene
  })
})
