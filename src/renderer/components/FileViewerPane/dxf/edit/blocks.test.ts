// Inserting a block, on the CAD pack's sampler: which blocks can be placed,
// the labels a new reference asks for, the verb's argv, and the new
// reference's address in the result.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import type { ModelBlock, ModelHeader, ModelPage } from '../model/types'
import { insertableBlocks, insertArgv, insertedAddress, insertLabels } from './blocks'

const FIX = resolve(__dirname, '../../__fixtures__/drawing-model')
const HEADER = JSON.parse(readFileSync(`${FIX}/sampler-header.json`, 'utf8')) as ModelHeader
const ROWS = (JSON.parse(readFileSync(`${FIX}/sampler-page-model.json`, 'utf8')) as ModelPage).entities
const block = (name: string): ModelBlock => HEADER.blocks.find((b) => b.name === name)!

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
      ],
    }
    expect(insertableBlocks(header).map((b) => b.name)).toEqual(['CHAIR', 'DOOR', 'NEST'])
  })
})

describe('insertLabels', () => {
  it('the model’s definitions that aren’t constant, with their defaults', () => {
    const b = { ...block('DOOR'), attdefs: [{ tag: 'NO', default: 'D-00', prompt: 'Door number' }, { tag: 'MAKER', default: 'ACME', constant: true }] }
    expect(insertLabels(b, ROWS)).toEqual([{ tag: 'NO', value: 'D-00', prompt: 'Door number', known: true }])
  })
  it('without them, the tags an existing reference carries, defaults unknown', () => {
    expect(insertLabels(block('DOOR'), ROWS)).toEqual([{ tag: 'NO', value: '', prompt: null, known: false }])
    expect(insertLabels(block('NEST'), ROWS)).toEqual([])
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
