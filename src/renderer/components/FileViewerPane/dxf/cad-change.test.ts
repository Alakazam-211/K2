// What a new version changed, read from its `cad.change/1` answer: the
// outlines (boxes in drawing coordinates, in each author's presence colour),
// the note's words and what Undo reverts. Fixtures: __fixtures__/cad-change/
// against the plan in __fixtures__/dxf-change-plan.dxf.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { buildDrawing } from '../dxfModel'
import { parseCadChange, type CadChange } from './cad-change'
import { changeAuthors, changeMarks, changeSlots } from './cad-change-marks'
import { changeNote } from './cad-change-words'
import { viewBounds } from './view-address'

const FIX = resolve(__dirname, '../__fixtures__')
const plan = buildDrawing(readFileSync(`${FIX}/dxf-change-plan.dxf`, 'utf8'))
const change = (name: string): CadChange => parseCadChange(JSON.parse(readFileSync(`${FIX}/cad-change/${name}.json`, 'utf8')))!
const marksOf = (name: string) => {
  const c = change(name)
  return changeMarks(c, changeSlots(new Map(), changeAuthors(c)), plan)
}

describe('parseCadChange', () => {
  it('reads every fixture, and nothing that is not cad.change/1', () => {
    for (const n of ['move', 'add', 'remove', 'block', 'layer-freeze', 'two-authors', 'three-in-a-row', 'doubt', 'refused']) expect(change(n)).not.toBeNull()
    expect(parseCadChange({ schema: 'cad.change/2' })).toBeNull()
    expect(parseCadChange('nope')).toBeNull()
    expect(change('refused')).toMatchObject({ pairing: 'refused', refusedWhy: 'an app that renumbers handles saved this drawing', entities: [] })
  })
})

describe('changeMarks: outlines in drawing coordinates, one presence colour per author', () => {
  it('a move outlines where it went, quietly where it was, with a line from old to new', () => {
    const m = marksOf('move')
    expect(m.marks).toEqual([
      { address: '@h:2B1', box: [0, 0, 100, 0], slot: 1, dashed: false, quiet: false, was: true },
      { address: '@h:2B1', box: [10, 0, 110, 0], slot: 1, dashed: false, quiet: false, was: false },
    ])
    expect(m.links).toEqual([{ from: [50, 0], to: [60, 0], slot: 1 }])
    expect(m.counted).toBe(1)
  })

  it('an add outlines its new box; a removal its old one, dashed when the box is approximate', () => {
    expect(marksOf('add').marks).toEqual([{ address: '@h:2C0', box: [40, 40, 60, 60], slot: 1, dashed: false, quiet: false, was: false }])
    expect(marksOf('remove').marks).toEqual([{ address: '@h:2A7', box: [60, 85, 85, 89], slot: 1, dashed: true, quiet: false, was: false }])
  })

  it('a block change outlines each placement from the drawing, never the block’s own rows', () => {
    const m = marksOf('block')
    expect(m.marks.map((k) => k.address)).toEqual(['@h:3F1', '@h:3F2'])
    // The door at (110, 20) turned 90°: its 9-unit leaf runs up to (110, 29), its swing to x = 101.
    const [x0, y0, x1, y1] = m.marks[0].box
    expect([x0, y0, x1, y1].map((v) => Math.round(v * 1000) / 1000)).toEqual([101, 20, 110, 29])
    expect(m.counted).toBe(1)
  })

  it('tables draw nothing; a box of quality none is counted, not drawn', () => {
    expect(marksOf('layer-freeze').marks).toEqual([])
    const m = marksOf('three-in-a-row')
    expect(m.marks.filter((k) => !k.was).map((k) => k.address)).toEqual(['@h:2B1', '@h:2B4'])
    expect(m.counted).toBe(3)
  })

  it('two authors take two presence slots, never the blue; each row in its author’s', () => {
    const c = change('two-authors')
    const slots = changeSlots(new Map(), changeAuthors(c))
    expect([...slots]).toEqual([['p_atlas', 1], ['p_sam', 3]])
    const after = changeMarks(c, slots, plan).marks.filter((k) => !k.was)
    expect(after.map((k) => [k.address, k.slot])).toEqual([['@h:2B2', 1], ['@h:2C0', 1], ['@h:2A8', 3], ['@h:2B3', 1]])
    // A third, fourth and fifth author still never get the blue (2).
    const more = changeSlots(slots, ['p_atlas', 'p_sam', 'p_kai', 'p_ana', 'p_lee'])
    expect([...more.values()]).not.toContain(2)
  })

  it('pairing doubt: rows paired across an outside save are quiet; refused: nothing is outlined', () => {
    expect(marksOf('doubt').marks.filter((k) => !k.was).map((k) => [k.address, k.quiet])).toEqual([['@h:2B4', true], ['@h:2C0', false]])
    expect(marksOf('refused')).toEqual({ marks: [], links: [], counted: 0 })
  })
})

describe('changeNote: who changed what, and what Undo reverts', () => {
  const note = (name: string) => changeNote(change(name), plan)

  it('one version: name, past verb, a kind, on its layer; Undo reverts that version', () => {
    expect(note('move')).toEqual({ text: 'Atlas moved a line on A-WALL', undo: [42] })
    expect(note('add')).toEqual({ text: 'Atlas added a circle on A-WALL', undo: [43] })
    expect(note('remove').text).toBe('Sam deleted text on ANNO')
    expect(note('block').text).toBe('Atlas changed block DOOR-900')
  })

  it('a layer change reads by name, from the drawing’s state now', () => {
    expect(note('layer-freeze')).toEqual({ text: 'Sam froze A-FURN', undo: [46] })
  })

  it('several versions by one author collapse; Undo reverts them newest first', () => {
    expect(note('three-in-a-row')).toEqual({ text: 'Atlas made 3 changes', undo: [53, 52, 51] })
  })

  it('several authors are named together, with no Undo', () => {
    expect(note('two-authors')).toEqual({ text: 'Atlas and Sam made 4 changes', undo: null })
  })

  it('an outside save in the range, doubted or refused, says so, with no Undo', () => {
    expect(note('doubt')).toEqual({ text: 'Changed by an outside save', undo: null })
    expect(note('refused')).toEqual({ text: 'Changed by an outside save', undo: null })
  })
})

describe('viewBounds: opening at a view address', () => {
  it('the whole drawing, one entity, a layer, or a set', () => {
    expect(viewBounds('@model:fit', plan)).toEqual(plan.bounds)
    expect(viewBounds('@h:2c0', plan)).toEqual({ minX: 40, minY: 40, maxX: 60, maxY: 60 })
    expect(viewBounds('@layer:ANNO', plan)).toMatchObject({ minX: 15, minY: 85 })
    expect(viewBounds(['@h:2C0', '@h:2B1'], plan)).toEqual({ minX: 10, minY: 0, maxX: 110, maxY: 60 })
    expect(viewBounds('@h:2C0 @h:2B1', plan)).toEqual({ minX: 10, minY: 0, maxX: 110, maxY: 60 })
  })

  it('an address the drawing has no box for frames nothing (the caller fits the drawing)', () => {
    expect(viewBounds('@h:FFFF', plan)).toBeNull()
    expect(viewBounds(undefined, plan)).toBeNull()
  })
})
