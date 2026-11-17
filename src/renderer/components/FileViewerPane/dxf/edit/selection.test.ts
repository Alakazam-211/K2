// The selection's maths and rules, on the CAD pack's sampler drawing:
// what can be picked, a click's pick, a window (left to right: wholly
// inside) against a crossing (right to left: touching), Shift's lock to one
// axis, which entities the verbs can change and why not, an attribute
// value's checks as you type, and whose holds cover a selection.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { ModelHeader, ModelPage, ModelRow } from '../model/types'
import { attribProblem } from './attrib-value'
import { heldHandles, holdPrompt, holdsOn } from './holds'
import { boxMode, boxSelect, editReason, pickAt, pickable, shiftLock, snapDelta, toggle } from './selection'

const FIX = resolve(__dirname, '../../__fixtures__/drawing-model')
const HEADER = JSON.parse(readFileSync(`${FIX}/sampler-header.json`, 'utf8')) as ModelHeader
const ROWS = (JSON.parse(readFileSync(`${FIX}/sampler-page-model.json`, 'utf8')) as ModelPage).entities
const row = (h: string): ModelRow => ROWS.find((r) => r.h === h)!

describe('pickable', () => {
  it('only what is drawn and shown in model space: hidden, off-layer, frozen and not-drawn rows are left out', () => {
    const hs = pickable(ROWS).map((r) => r.h)
    expect(hs).toContain('@h:41')
    for (const h of ['@h:44', '@h:45', '@h:46', '@h:5B', '@h:7A']) expect(hs, h).not.toContain(h)
    expect(pickable([{ ...row('@h:41'), owner: '@h:2D' }])).toEqual([])
  })
})

describe('box selection', () => {
  const rows = pickable(ROWS)
  it('left to right is a window, right to left a crossing', () => {
    expect(boxMode([0, 0], [10, 5])).toBe('window')
    expect(boxMode([10, 0], [0, 5])).toBe('crossing')
  })
  it('a window takes only what is wholly inside', () => {
    // Around the three A-WALL lines at y 0..2 (x 0..10) and the door beside them; the text and polylines cross out of it.
    expect(boxSelect(rows, [-1, -0.5], [11, 2.5]).sort()).toEqual(['@h:41', '@h:61', '@h:A00', '@h:A01'])
  })
  it('a crossing also takes what it touches', () => {
    const got = boxSelect(rows, [5.5, -0.5], [4.5, 2.5])
    for (const h of ['@h:41', '@h:A00', '@h:A01', '@h:51', '@h:52', '@h:5C']) expect(got, h).toContain(h)
    expect(got).not.toContain('@h:4B')
    expect(got).not.toContain('@h:4A')
  })
  it('a window that cuts a line takes nothing of it', () => {
    expect(boxSelect(rows, [4.5, 1.5], [5.5, 2.5])).not.toContain('@h:41')
  })
  it('rows without a box (an XLINE, a RAY) are never boxed', () => {
    expect(boxSelect(rows, [-1e9, -1e9], [1e9, 1e9])).not.toContain('@h:48')
  })
})

describe('pickAt', () => {
  const rows = pickable(ROWS)
  it('picks the line under the pointer, within the tolerance', () => {
    expect(pickAt(rows, [5, 2.02], 0.05)).toBe('@h:41')
    expect(pickAt(rows, [5, 1.01], 0.05)).toBe('@h:A01')
  })
  it('on its curve, not anywhere inside it', () => {
    // The circle about (20, 20), radius 2: its curve picks it, its middle doesn't.
    expect(pickAt(rows, [18.02, 20], 0.05)).toBe('@h:4B')
    expect(pickAt(rows, [20, 20.5], 0.05)).toBeNull()
    // The arc from 0° to 90° about (30, 0): on it, and not on the rest of its circle.
    expect(pickAt(rows, [30 + 5 * Math.SQRT1_2, 5 * Math.SQRT1_2], 0.05)).toBe('@h:4C')
    expect(pickAt(rows, [25, 0], 0.05)).toBeNull()
  })
  it('a text or a block anywhere on it', () => {
    expect(pickAt(rows, [10.4, 0.6], 0.05)).toBe('@h:61')
  })
  it('nothing near: null', () => {
    expect(pickAt(rows, [500, 500], 0.05)).toBeNull()
  })
})

describe('toggle and Shift', () => {
  it('shift-click adds, and removes what is already in', () => {
    expect(toggle(['@h:41'], '@h:42')).toEqual(['@h:41', '@h:42'])
    expect(toggle(['@h:41', '@h:42'], '@h:41')).toEqual(['@h:42'])
  })
  it('Shift locks a move to the longer axis', () => {
    expect(shiftLock(10, 3)).toEqual([10, 0])
    expect(shiftLock(-2, -9)).toEqual([0, -9])
    expect(shiftLock(4, -4)).toEqual([4, 0])
  })
  it('a dragged offset is rounded to what a screen pixel can say', () => {
    expect(snapDelta(12.3456, 0.013)).toBe(12.35)
    expect(snapDelta(12.3456, 0.4)).toBe(12.3)
    expect(snapDelta(1234.5, 3)).toBe(1235)
    expect(snapDelta(-0.004, 0.013)).toBe(0)
  })
})

describe('editReason', () => {
  const r = (h: string) => editReason(row(h), HEADER)
  it('the kinds the verbs change: none', () => {
    for (const h of ['@h:41', '@h:4B', '@h:4C', '@h:51', '@h:5C', '@h:60', '@h:61', '@h:66', '@h:67']) expect(r(h), h).toBeNull()
  })
  it('the row’s own edit word, as the pack gives it', () => {
    expect(r('@h:68')).toBe('hatch')
    expect(r('@h:6B')).toBe('dimension')
    expect(r('@h:6C')).toBe('leader')
    expect(r('@h:79')).toBe('leader')
    expect(r('@h:6F')).toBe('image')
    expect(r('@h:6D')).toBe('image')
    expect(r('@h:7B')).toBe('unknown')
    expect(r('@h:48')).toBe('unknown')
    expect(r('@h:4D')).toBe('own-frame')
    expect(r('@h:7A')).toBe('solid3d')
  })
  it('without a word from the model, the pack’s rule by kind', () => {
    const bare = (kind: string) => editReason({ ...row('@h:41'), kind, edit: undefined }, HEADER)
    expect(bare('3DSOLID')).toBe('solid3d')
    expect(bare('ACAD_PROXY_ENTITY')).toBe('proxy')
    expect(bare('HATCH')).toBe('hatch')
    expect(bare('XLINE')).toBe('unknown')
    expect(bare('LINE')).toBeNull()
  })
  it('the model’s word wins; null is editable', () => {
    for (const w of ['reactors', 'embedded', 'dynamic', 'no-handle', 'xref', 'proxy', 'version', 'multiline-attribute'])
      expect(editReason({ ...row('@h:41'), edit: w }, HEADER), w).toBe(w)
    expect(editReason({ ...row('@h:41'), edit: 'locked-layer' }, HEADER)).toBe('locked-layer')
    expect(editReason({ ...row('@h:41'), edit: null }, HEADER)).toBeNull()
  })
  it('an external reference, a locked layer, inside a block', () => {
    expect(editReason({ ...row('@h:61'), geom: { ...row('@h:61').geom, xref: true } }, HEADER)).toBe('xref')
    const locked = { ...HEADER, layers: HEADER.layers.map((l) => (l.name === 'A-WALL' ? { ...l, locked: true } : l)) }
    expect(editReason(row('@h:41'), locked)).toBe('locked-layer')
    expect(editReason({ ...row('@h:41'), owner: '@h:2D', edit: undefined }, HEADER)).toBe('in-block')
  })
})

describe('attribProblem: the pack’s rule, as you type', () => {
  const utf8 = { utf8: true }
  it('a short single line is fine', () => {
    expect(attribProblem('D-02', utf8)).toBeNull()
    expect(attribProblem('Tür Ø 900', utf8)).toBeNull()
    expect(attribProblem('', utf8)).toBeNull()
  })
  it('one line, no control characters, no \\U+, nothing the kit would read as its own flag', () => {
    expect(attribProblem('a\nb', utf8)).toMatch(/one line/)
    expect(attribProblem('a\u0007b', utf8)).toMatch(/hidden character/)
    expect(attribProblem('a\u0085b', utf8)).toMatch(/hidden character/)
    expect(attribProblem('x\\u+00e9', utf8)).toMatch(/\\U\+/)
    expect(attribProblem('--base', utf8)).toMatch(/--/)
  })
  it('at most 255 bytes as the file writes them', () => {
    expect(attribProblem('a'.repeat(255), utf8)).toBeNull()
    expect(attribProblem('a'.repeat(256), utf8)).toBe('Too long: 256 bytes, and a label holds 255.')
    // UTF-8 from 2007 on: Ø is two bytes.
    expect(attribProblem('Ø'.repeat(128), utf8)).toBe('Too long: 256 bytes, and a label holds 255. Some characters take more than one byte.')
    // Before 2007, Ø is one byte; a character the code page lacks is written \U+XXXX (7).
    expect(attribProblem('Ø'.repeat(200), { utf8: false })).toBeNull()
    expect(attribProblem('α'.repeat(37), { utf8: false })).toMatch(/259 bytes/)
    expect(attribProblem('😀', { utf8: false })).toMatch(/can’t be saved/)
  })
})

describe('holds', () => {
  const atlas = { pid: 'p_atlas', name: 'Atlas', note: 'moving the doors', keys: ['@h:41'] }
  const layer = HEADER.layers.find((l) => l.name === 'E-LITE')!
  const kai = { pid: 'p_kai', name: 'Kai', note: null, keys: [layer.handle] }
  it('a hold covers its entities, a layer’s hold every entity on it, model space’s everything', () => {
    expect(holdsOn(['@h:41', '@h:A00'], [atlas], ROWS, HEADER)).toEqual([atlas])
    expect(holdsOn(['@h:A00'], [atlas], ROWS, HEADER)).toEqual([])
    expect(holdsOn(['@h:42'], [atlas, kai], ROWS, HEADER)).toEqual([kai])
    const model = HEADER.layouts.find((l) => l.kind === 'model')!
    expect(holdsOn(['@h:A00'], [{ ...atlas, keys: [model.handle] }], ROWS, HEADER)).toHaveLength(1)
    expect(holdsOn(['@h:A00'], [{ ...atlas, keys: ['@doc:dxf'] }], ROWS, HEADER)).toHaveLength(1)
    // Keys are handles only: a layer's name is no key.
    expect(holdsOn(['@h:A00'], [{ ...atlas, keys: ['@layer:A-WALL'] }], ROWS, HEADER)).toEqual([])
    // A handle spelt with leading zeros or lower case is the same handle.
    expect(holdsOn(['@h:A00'], [{ ...atlas, keys: ['@h:0a00'] }], ROWS, HEADER)).toHaveLength(1)
  })
  it('a hold on model space’s block record covers everything in model space, never a block’s insides', () => {
    const header = { ...HEADER, layouts: [], model_space_block: '@h:2B' }
    const ms = { ...atlas, keys: ['@h:2b'] }
    expect(holdsOn(['@h:A00'], [ms], ROWS, header)).toEqual([ms])
    expect(heldHandles([ms], ROWS, header).get('@h:41')).toBe('p_atlas')
    expect(holdsOn(['@h:35'], [ms], [{ ...row('@h:41'), h: '@h:35', owner: '@h:2D' }], header)).toEqual([])
    expect(holdsOn(['@h:A00'], [ms], ROWS, { ...header, model_space_block: null })).toEqual([])
  })
  it('asks once, naming who and what for', () => {
    expect(holdPrompt([atlas], 2, 'move')).toBe('Atlas is working on these: moving the doors. Move anyway?')
    expect(holdPrompt([kai], 1, 'move')).toBe('Kai is working on this. Move anyway?')
    expect(holdPrompt([atlas, kai], 3, 'relabel')).toBe('Atlas is working on these: moving the doors. Kai is working on these. Change it anyway?')
  })
})
