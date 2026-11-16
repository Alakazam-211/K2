// K2 Render's scene view addresses (proposal, pending K2 Cores, 2026-11-15;
// .k2/prds/wire-shapes-k2-2026-11-15.md): `@scene:main`, `@cam:<enc(id)>`,
// `@obj:<enc(id)>`, `@turntable:<n>`, and `@cam-n:<i>` / `@obj-n:<i>` only
// for duplicate ids. enc() percent-encodes %, /, whitespace and control
// characters. They are the listing's own addresses: a person's viewAddress
// maps straight onto a view.

import { describe, expect, it } from 'vitest'
import { SCENE_VIEW, camView, decId, dupView, encId, formatView, objView, parseView, turntableView } from './view-address'

describe('enc()', () => {
  it('percent-encodes %, /, whitespace and control characters, and nothing else', () => {
    expect(encId('Coffee Table')).toBe('Coffee%20Table')
    expect(encId('Camera.001')).toBe('Camera.001')
    expect(encId('a/b')).toBe('a%2Fb')
    expect(encId('50%')).toBe('50%25')
    expect(encId('tab\there')).toBe('tab%09here')
    expect(encId('line\nbreak')).toBe('line%0Abreak')
    expect(encId('nul\u0000del\u007f')).toBe('nul%00del%7F')
    expect(encId('no break')).toBe('no%C2%A0break')
    expect(encId('Ünïcode:#?&')).toBe('Ünïcode:#?&')
  })

  it('decodes back, and leaves a malformed escape as it is', () => {
    for (const s of ['Coffee Table', 'a/b', '50%', 'tab\there', 'nul\u0000', 'no break', 'Ünï']) expect(decId(encId(s))).toBe(s)
    expect(decId('100%zz')).toBe('100%zz')
  })
})

describe('scene view addresses', () => {
  it('names each view kind', () => {
    expect(SCENE_VIEW).toBe('@scene:main')
    expect(camView('Camera.001')).toBe('@cam:Camera.001')
    expect(objView('Coffee Table')).toBe('@obj:Coffee%20Table')
    expect(turntableView(3)).toBe('@turntable:3')
    expect(dupView('object', 4)).toBe('@obj-n:4')
    expect(dupView('camera', 0)).toBe('@cam-n:0')
  })

  it('parses them back', () => {
    expect(parseView('@scene:main')).toEqual({ kind: 'scene' })
    expect(parseView('@cam:Camera.001')).toEqual({ kind: 'camera', id: 'Camera.001' })
    expect(parseView('@obj:Coffee%20Table')).toEqual({ kind: 'object', id: 'Coffee Table' })
    expect(parseView('@turntable:7')).toEqual({ kind: 'turntable', frame: 7 })
    expect(parseView('@obj-n:4')).toEqual({ kind: 'object', index: 4 })
    expect(parseView('@cam-n:0')).toEqual({ kind: 'camera', index: 0 })
  })

  it('round-trips', () => {
    for (const a of ['@scene:main', '@cam:Camera', '@obj:Coffee%20Table', '@obj:a%2Fb', '@turntable:0', '@obj-n:12']) expect(formatView(parseView(a)!)).toBe(a)
  })

  it('refuses what isn’t a scene view: the old @scene, empty ids, unencoded ids, frames past 7', () => {
    for (const a of ['@scene', '@scene:other', '@cam:', '@obj:Coffee Table', '@obj:a/b', '@turntable:8', '@turntable:x', '@obj-n:', '@obj-n:-1', 'scene', '', '@obj:100%zz']) expect(parseView(a), a).toBeNull()
  })
})
