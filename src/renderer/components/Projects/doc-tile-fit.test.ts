import { describe, expect, it } from 'vitest'
import { DOC_TILE_FIT_CSS, fitToTile } from './doc-tile-fit'

describe('a doc in a narrow tile', () => {
  it('goes inside <head>, so the doc keeps its doctype (no quirks mode)', () => {
    const out = fitToTile('<!doctype html><html><head><title>x</title></head><body>hi</body></html>')
    expect(out.startsWith('<!doctype html><html><head>' + DOC_TILE_FIT_CSS)).toBe(true)
  })
  it('after a doctype when there is no head, else first', () => {
    expect(fitToTile('<!DOCTYPE html><p>hi</p>')).toBe('<!DOCTYPE html>' + DOC_TILE_FIT_CSS + '<p>hi</p>')
    expect(fitToTile('<p>hi</p>')).toBe(DOC_TILE_FIT_CSS + '<p>hi</p>')
  })
  it('breaks long words and scrolls what is still wider', () => {
    expect(DOC_TILE_FIT_CSS).toContain('overflow-wrap:anywhere')
    expect(DOC_TILE_FIT_CSS).toContain('overflow:auto')
  })
})
