// After your own edit lands, its result's change (`cad.change/1`, which
// leaves who and which version to the result) goes into the change
// highlight as yours: the version it made, "You" as its author, outlined in
// your own accent, with Undo. Someone else's change reads its author from
// `by`, never from `owner` (the entity's space).

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { parseCadChange } from '../cad-change'
import { changeAuthors, changeMarks, changeSlots, SELF_PID } from '../cad-change-marks'
import { changeNote } from '../cad-change-words'
import { ownChange } from './own-change'
import { parseToolResult } from './tool-result'

const FIX = resolve(__dirname, '../../__fixtures__/cad-edit')
const result = (n: string) => parseToolResult(JSON.parse(readFileSync(`${FIX}/${n}.json`, 'utf8')))!
const drawing = { index: { handles: {}, layers: {}, layerState: {} } }

describe('ownChange', () => {
  it('fills the version and the author from the result', () => {
    const c = ownChange(result('move-done'))!
    expect(c).toMatchObject({ from: 7, to: 8, pairing: 'kept', versions: [{ version: 8, by: { pid: SELF_PID, name: 'You' }, verb: 'entity move' }] })
    expect(c.entities.map((r) => [r.by, r.version, r.owner])).toEqual([
      [SELF_PID, 8, 'model'],
      [SELF_PID, 8, 'model'],
      [SELF_PID, 8, 'model'],
    ])
  })

  it('a dry run, a refusal or no change: nothing to show', () => {
    expect(ownChange(result('move-dry'))).toBeNull()
    expect(ownChange(result('refused-locked'))).toBeNull()
  })

  it('reads "You moved 3 lines on A-WALL", with Undo of that version', () => {
    expect(changeNote(ownChange(result('move-done'))!, drawing)).toEqual({ text: 'You moved 3 lines on A-WALL', undo: [8] })
    expect(changeNote(ownChange(result('attrib-done'))!, drawing).text).toBe('You changed a label in a block on A-WALL')
  })

  it('several kinds are counted as the panel counts them', () => {
    const r = result('move-done')
    const data = r.data as { entities: { kind: string }[] }
    const mixed = { ...r, data: { ...data, entities: data.entities.map((e, i) => (i === 2 ? { ...e, kind: 'INSERT' } : e)) } }
    expect(changeNote(ownChange(mixed)!, drawing).text).toBe('You moved 2 lines and 1 block on A-WALL')
  })

  it('outlines in your own accent, and takes no one’s presence colour', () => {
    const c = ownChange(result('move-done'))!
    const slots = changeSlots(new Map(), changeAuthors(c))
    expect(slots.size).toBe(0)
    const m = changeMarks(c, slots, drawing)
    expect(m.marks.length).toBe(6)
    expect(m.marks.every((k) => k.self === true)).toBe(true)
  })
})

describe('someone else’s change names them from `by`, not `owner`', () => {
  it('a model-space row with `by` set', () => {
    const c = parseCadChange({
      schema: 'cad.change/1', format: 'dxf', from: 8, to: 9, pairing: 'kept',
      versions: [{ version: 9, by: { pid: 'p_atlas', name: 'Atlas', kind: 'agent' }, verb: 'entity move' }],
      entities: [
        { address: '@h:41', op: 'changed', kind: 'LINE', layer: 'A-WALL', owner: 'model', fields: ['geom'], bbox_before: [0, 2, 10, 2], bbox_after: [0, 4, 10, 4], bbox_q: 'exact', version: 9, by: 'p_atlas', identity: null },
        { address: '@h:A00', op: 'changed', kind: 'LINE', layer: 'A-WALL', owner: 'model', fields: ['geom'], bbox_before: [0, 0, 10, 0], bbox_after: [0, 2, 10, 2], bbox_q: 'exact', version: 9, by: 'p_atlas', identity: null },
      ],
      tables: [], blocks: [],
    })!
    expect(changeNote(c, drawing).text).toBe('Atlas moved 2 lines on A-WALL')
    const slots = changeSlots(new Map(), changeAuthors(c))
    expect(changeAuthors(c)).toEqual(['p_atlas'])
    expect(changeMarks(c, slots, drawing).marks.every((k) => k.slot === slots.get('p_atlas') && !k.self)).toBe(true)
  })
})
