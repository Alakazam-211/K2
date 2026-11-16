// The scene's view addresses are native addresses in the kit's grammar,
// keyed by the live store's keys (K2 Render "scene previews: every address
// is a native address", f2bb30c00; k2-mpc-blend WIRE.md, Addresses):
// `@scene:main`, `@obj:N` (an object's live key `oN`), `@cam:N` (its camera
// object's key: the same N as that `@obj:N`), `@turntable:<0-7>`. A key
// never changes while the doc lives, so a renamed object keeps its view and
// a person following it keeps following. They are the route's and the
// listing's own addresses (tests/fixtures/scene-preview), and a person's
// viewAddress.

import { describe, expect, it } from 'vitest'
import { SCENE_VIEW, camView, formatView, keyOf, objView, parseView, turntableView } from './view-address'

describe('scene view addresses', () => {
  it('names each view kind by key', () => {
    expect(SCENE_VIEW).toBe('@scene:main')
    expect(camView(26)).toBe('@cam:26')
    expect(objView(15)).toBe('@obj:15')
    expect(turntableView(3)).toBe('@turntable:3')
  })

  it('parses them back', () => {
    expect(parseView('@scene:main')).toEqual({ kind: 'scene' })
    expect(parseView('@cam:1')).toEqual({ kind: 'camera', key: 1 })
    expect(parseView('@obj:15')).toEqual({ kind: 'object', key: 15 })
    expect(parseView('@turntable:7')).toEqual({ kind: 'turntable', frame: 7 })
  })

  it('round-trips', () => {
    for (const a of ['@scene:main', '@cam:1', '@obj:0', '@obj:15', '@turntable:0']) expect(formatView(parseView(a)!)).toBe(a)
  })

  it('refuses what isn’t one: the old @scene, names (the older proposal), a sign or a leading zero, frames past 7', () => {
    for (const a of ['@scene', '@scene:other', '@cam:', '@cam:Camera', '@obj:Sofa', '@obj:Coffee%20Table', '@obj-n:4', '@obj:015', '@obj:-1', '@obj:1.5', '@turntable:8', '@turntable:x', 'scene', ''])
      expect(parseView(a), a).toBeNull()
  })

  it('an object’s key from its kit address', () => {
    expect(keyOf('@obj:15')).toBe(15)
    expect(keyOf('@coll:3')).toBeNull()
    expect(keyOf('@obj:x')).toBeNull()
  })
})
