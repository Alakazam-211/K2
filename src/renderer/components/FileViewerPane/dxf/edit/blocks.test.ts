// Inserting a block, on the CAD pack's sampler: which blocks can be placed,
// the labels a new reference asks for, the verb's argv, and the new
// reference's address in the result.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { ModelBlock, ModelHeader, ModelPage } from '../model/types'
import { currentLayerName, insertableBlocks, insertArgv, insertedAddress, insertLabels } from './blocks'

const FIX = resolve(__dirname, '../../__fixtures__/drawing-model')
const HEADER = JSON.parse(readFileSync(`${FIX}/sampler-header.json`, 'utf8')) as ModelHeader
const ROWS = (JSON.parse(readFileSync(`${FIX}/sampler-page-model.json`, 'utf8')) as ModelPage).entities
const block = (name: string): ModelBlock => HEADER.blocks.find((b) => b.name === name)!
/** The header as a pack before v0.4.0 wrote it: no block flags, no label definitions, no current layer. */
const { current_layer: _cl, ...WITHOUT_LAYER } = HEADER
const OLDER: ModelHeader = { ...WITHOUT_LAYER, blocks: HEADER.blocks.map(({ dynamic: _d, annotative: _a, attdefs: _t, ...b }) => b) }
const older = (name: string): ModelBlock => OLDER.blocks.find((b) => b.name === name)!

describe('the vendored pack’s header (v0.4.0)', () => {
  it('states each block’s dynamic and annotative flags, DOOR’s label definition, and the current layer', () => {
    expect(HEADER.current_layer).toBe('@h:19')
    expect(HEADER.blocks.map((b) => [b.name, b.dynamic, b.annotative, b.attdefs])).toEqual(
      HEADER.blocks.map((b) => [b.name, false, false, b.name === 'DOOR' ? [{ tag: 'NO', default: 'D-00', constant: false, prompt: 'Door number' }] : []]),
    )
  })
})

describe('insertableBlocks', () => {
  it('the sampler’s named blocks, by name; its anonymous dimension block is left out', () => {
    expect(insertableBlocks(HEADER).map((b) => b.name)).toEqual(['DOOR', 'NEST'])
  })
  it('leaves out linked drawings, layouts’ own blocks, a CAD app’s own and marked dynamic blocks', () => {
    const extra = (b: Partial<ModelBlock>): ModelBlock => ({ ...block('DOOR'), ...b })
    const header = {
      ...HEADER,
      blocks: [
        ...HEADER.blocks,
        extra({ handle: '@h:90', name: 'SITE', xref: true }),
        extra({ handle: '@h:2B', name: '*Model_Space' }),
        extra({ handle: '@h:2C', name: 'LAYOUT ONE' }),
        extra({ handle: '@h:91', name: '*U4' }),
        extra({ handle: '@h:92', name: 'VALVE', dynamic: true }),
        extra({ handle: '@h:93', name: 'CHAIR' }),
        extra({ handle: '@h:94', name: 'NORTH', annotative: true }),
        // Annotative unknown (a reader that can’t say): kept.
        extra({ handle: '@h:95', name: 'TREE', annotative: null }),
      ],
    }
    expect(insertableBlocks(header).map((b) => b.name)).toEqual(['CHAIR', 'DOOR', 'NEST', 'TREE'])
  })
  it('without the flags (a pack before v0.4.0), nothing is left out for them', () => {
    for (const b of OLDER.blocks) expect([b.name, b.dynamic, b.annotative]).toEqual([b.name, undefined, undefined])
    expect(insertableBlocks(OLDER).map((b) => b.name)).toEqual(insertableBlocks(HEADER).map((b) => b.name))
  })
})

describe('insertLabels', () => {
  it('DOOR’s one label, by prompt, with its default; NEST has none', () => {
    expect(insertLabels(block('DOOR'), ROWS)).toEqual([{ tag: 'NO', value: 'D-00', prompt: 'Door number', known: true }])
    expect(insertLabels(block('NEST'), ROWS)).toEqual([])
  })
  it('without definitions (a pack before v0.4.0): the tags DOOR’s existing reference carries, defaults unknown', () => {
    expect(older('DOOR').attdefs).toBeUndefined()
    expect(insertLabels(older('DOOR'), ROWS)).toEqual([{ tag: 'NO', value: '', prompt: null, known: false }])
  })
  it('constant definitions are left out; the rest keep file order', () => {
    const b = {
      ...block('DOOR'),
      attdefs: [
        { tag: 'MAKER', default: 'ACME', constant: true, prompt: 'Maker' },
        { tag: 'NO', default: 'D-00', constant: false, prompt: 'Door number' },
        { tag: 'FIRE', default: 'FD30', constant: false, prompt: null },
      ],
    }
    expect(insertLabels(b, ROWS).map((l) => [l.tag, l.value, l.prompt])).toEqual([['NO', 'D-00', 'Door number'], ['FIRE', 'FD30', null]])
  })
  it('only when the model can’t read them (null): the tags an existing reference carries, defaults unknown', () => {
    expect(insertLabels({ ...block('DOOR'), attdefs: null }, ROWS)).toEqual([{ tag: 'NO', value: '', prompt: null, known: false }])
  })
})

describe('currentLayerName', () => {
  it('the header’s current layer, by name through the layers’ handles', () => {
    expect(currentLayerName(HEADER)).toBe('0')
    expect(currentLayerName({ ...HEADER, current_layer: '@h:01a' })).toBe('A-WALL')
  })
  it('nothing when it isn’t set (a pack before v0.4.0 states none) or isn’t in the drawing', () => {
    expect(OLDER.current_layer).toBeUndefined()
    expect(currentLayerName(OLDER)).toBeNull()
    expect(currentLayerName({ ...HEADER, current_layer: null })).toBeNull()
    expect(currentLayerName({ ...HEADER, current_layer: '@h:FFF' })).toBeNull()
  })
})

describe('insertArgv', () => {
  it('the block by handle, the base point’s place, the unit, then each label once', () => {
    expect(insertArgv('@h:2D', [12.5, -3], 'mm', [['NO', 'D-02']])).toEqual(['block', 'insert', '--block', '@h:2D', '--at', '12.5,-3', '--unit', 'mm', '--attr', 'NO=D-02'])
    expect(insertArgv('@h:2E', [0, 0], 'drawing')).toEqual(['block', 'insert', '--block', '@h:2E', '--at', '0,0', '--unit', 'drawing'])
  })
  it('the new reference is the change’s added row', () => {
    expect(insertedAddress({ entities: [{ address: '@h:A02', op: 'added', kind: 'INSERT' }] })).toBe('@h:A02')
    expect(insertedAddress({ entities: [{ address: '@h:41', op: 'changed' }] })).toBeNull()
    expect(insertedAddress(null)).toBeNull()
  })
})
