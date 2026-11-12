// The daemon's live deck previews, as this client reads them
// (`GET /cli/docs/preview`): the listing and one image, loud on a shape it
// doesn't know.

import { describe, expect, it } from 'vitest'
import { parseDeckListing, parsePreviewImage } from './docs-preview'

const listing = {
  ok: true,
  kind: 'deck',
  rev: 7,
  version: null,
  page: { w: 12192000, h: 6858000, unit: 'emu' },
  renderer: { build: 'deckrender-p1-0.1.0' },
  containers: [
    { key: '@sld:256', position: 1, title: 'Quarterly review', hidden: false, render_hash: 'aa', previews: { thumb: 'ready', full: 'none' } },
    { key: '@sld:257', position: 2, title: '', hidden: true, render_hash: 'bb', previews: { thumb: 'rendering', full: 'failed' } },
  ],
}

describe('parseDeckListing', () => {
  it('reads the slides, their hashes and their image states', () => {
    const l = parseDeckListing(listing)
    expect(l.rev).toBe(7)
    expect(l.page).toEqual({ w: 12192000, h: 6858000, unit: 'emu' })
    expect(l.slides).toEqual([
      { key: '@sld:256', position: 1, title: 'Quarterly review', hidden: false, renderHash: 'aa', thumb: 'ready', full: 'none' },
      { key: '@sld:257', position: 2, title: '', hidden: true, renderHash: 'bb', thumb: 'rendering', full: 'failed' },
    ])
  })

  it('refuses another kind, a missing hash or an unknown state', () => {
    expect(() => parseDeckListing({ ...listing, kind: 'sheet' })).toThrow(/not a deck/)
    expect(() => parseDeckListing({ ...listing, containers: [{ ...listing.containers[0], render_hash: 3 }] })).toThrow(/render_hash/)
    expect(() =>
      parseDeckListing({ ...listing, containers: [{ ...listing.containers[0], previews: { thumb: 'maybe', full: 'none' } }] }),
    ).toThrow(/preview state/)
  })
})

describe('parsePreviewImage', () => {
  it('decodes a ready or stale image and its size', () => {
    const r = parsePreviewImage({ state: 'ready', render_hash: 'aa', base64: btoa('RIFFxxxxWEBP'), mime: 'image/webp', size: { w: 320, h: 180 } })
    expect(r.state).toBe('ready')
    expect(new TextDecoder().decode(r.bytes ?? new Uint8Array())).toBe('RIFFxxxxWEBP')
    expect([r.mime, r.width, r.height, r.failedCode]).toEqual(['image/webp', 320, 180, null])
    expect(parsePreviewImage({ state: 'stale', render_hash: 'old', base64: btoa('x'), mime: 'image/webp', size: { w: 320, h: 180 } }).state).toBe('stale')
  })

  it('reads rendering and failed without bytes, and refuses an image without them', () => {
    expect(parsePreviewImage({ state: 'rendering', render_hash: 'aa' })).toMatchObject({ state: 'rendering', bytes: null })
    expect(parsePreviewImage({ state: 'failed', render_hash: 'aa', failed: { code: 'timeout' } })).toMatchObject({ failedCode: 'timeout' })
    expect(() => parsePreviewImage({ state: 'ready', render_hash: 'aa' })).toThrow(/without its bytes/)
    expect(() => parsePreviewImage({ state: 'drawn', render_hash: 'aa' })).toThrow(/not an image state/)
  })
})
