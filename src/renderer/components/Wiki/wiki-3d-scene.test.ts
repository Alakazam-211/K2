// prd-wiki-3d-view-v1 §8.1: the 3D scene's pure helpers and the shared
// material / halo / label bookkeeping (node env; three objects are CPU-only
// until rendered).

import { describe, expect, it, vi } from 'vitest'
import { AdditiveBlending, NormalBlending, type Sprite } from 'three'
import {
  flyToPosition,
  isLightBackground,
  nodeVisual,
  parseCssColor,
  pickLabelIds,
  rgbaCss,
  R_NOTE,
  R_SELECTED,
  Wiki3dScene,
  type NodeKind,
  type Wiki3dTheme,
} from './wiki-3d-scene'

describe('parseCssColor keeps alpha', () => {
  it('hex forms', () => {
    expect(parseCssColor('#fff')).toEqual({ r: 1, g: 1, b: 1, a: 1 })
    expect(parseCssColor('#0000ff')).toEqual({ r: 0, g: 0, b: 1, a: 1 })
    const c = parseCssColor('#ff000080')
    if (!c) throw new Error('no colour')
    expect([c.r, c.g, c.b]).toEqual([1, 0, 0])
    expect(c.a).toBeCloseTo(128 / 255, 5)
    expect(parseCssColor('#f008')?.a).toBeCloseTo(0x88 / 255, 5)
  })

  it('rgb()/rgba() in comma and space syntax', () => {
    expect(parseCssColor('rgba(255, 255, 255, 0.07)')).toEqual({ r: 1, g: 1, b: 1, a: 0.07 })
    expect(parseCssColor('rgb(0, 0, 0)')).toEqual({ r: 0, g: 0, b: 0, a: 1 })
    expect(parseCssColor('rgb(255 0 0 / 50%)')).toEqual({ r: 1, g: 0, b: 0, a: 0.5 })
    expect(parseCssColor('  RGBA(0,255,0,.25) ')).toEqual({ r: 0, g: 1, b: 0, a: 0.25 })
  })

  it('unknown forms are null', () => {
    expect(parseCssColor('')).toBeNull()
    expect(parseCssColor(null)).toBeNull()
    expect(parseCssColor('rebeccapurple')).toBeNull()
    expect(parseCssColor('color-mix(in srgb, red 10%, transparent)')).toBeNull()
    expect(parseCssColor('#12')).toBeNull()
  })

  it('rgbaCss multiplies alpha', () => {
    expect(rgbaCss({ r: 1, g: 0, b: 0, a: 0.5 }, 0.5)).toBe('rgba(255, 0, 0, 0.25)')
  })
})

describe('light vs dark from --color-bg', () => {
  it('white and porcelain-like backgrounds are light; near-black is dark; unknown is dark', () => {
    expect(isLightBackground(parseCssColor('#ffffff'))).toBe(true)
    expect(isLightBackground(parseCssColor('#f4f1ea'))).toBe(true)
    expect(isLightBackground(parseCssColor('#0d0d0d'))).toBe(false)
    expect(isLightBackground(parseCssColor('#1e1e2e'))).toBe(false)
    expect(isLightBackground(null)).toBe(false)
  })
})

describe('pickLabelIds', () => {
  const nodes = [
    { id: 'home', x: 500, y: 0, z: 0 },
    { id: 'far', x: 400, y: 0, z: 0 },
    { id: 'b', x: 2, y: 0, z: 0 },
    { id: 'a', x: 2, y: 0, z: 0 },
    { id: 'c', x: 0, y: 5, z: 0 },
    { id: 'nopos' },
  ]
  const cam = { x: 0, y: 0, z: 0 }

  it('always-on ids first (any distance), then nearest within maxDist, ties by id', () => {
    expect(pickLabelIds(nodes, cam, new Set(['home']), { max: 40, maxDist: 100 })).toEqual(['home', 'a', 'b', 'c'])
  })

  it('caps the nearest set but never the always-on set', () => {
    expect(pickLabelIds(nodes, cam, new Set(['home', 'far']), { max: 1, maxDist: 100 })).toEqual(['home', 'far', 'a'])
  })

  it('is stable for the same input', () => {
    const a = pickLabelIds(nodes, cam, new Set(), { max: 40, maxDist: 1000 })
    const b = pickLabelIds([...nodes].reverse(), cam, new Set(), { max: 40, maxDist: 1000 })
    expect(a).toEqual(b)
    expect(a).toEqual(['a', 'b', 'c', 'far', 'home'])
  })

  it('at most 40 nearest from a large set', () => {
    const many = Array.from({ length: 300 }, (_, i) => ({ id: `n${String(i).padStart(3, '0')}`, x: i, y: 0, z: 0 }))
    const ids = pickLabelIds(many, cam, new Set(), { max: 40, maxDist: 1e9 })
    expect(ids).toHaveLength(40)
    expect(ids[0]).toBe('n000')
    expect(ids[39]).toBe('n039')
  })
})

describe('flyToPosition', () => {
  it('keeps the view direction at a fixed distance from the node', () => {
    const p = flyToPosition({ x: 10, y: 10, z: 10 }, { x: 0, y: 0, z: 100 }, { x: 0, y: 0, z: 0 }, 50)
    expect(p).toEqual({ x: 10, y: 10, z: 60 })
    const q = flyToPosition({ x: 0, y: 0, z: 0 }, { x: 3, y: 4, z: 0 }, { x: 0, y: 0, z: 0 }, 10)
    expect(q.x).toBeCloseTo(6, 6)
    expect(q.y).toBeCloseTo(8, 6)
    expect(q.z).toBeCloseTo(0, 6)
  })

  it('a degenerate direction falls back to +z', () => {
    expect(flyToPosition({ x: 1, y: 2, z: 3 }, { x: 0, y: 0, z: 0 }, { x: 0, y: 0, z: 0 }, 5)).toEqual({ x: 1, y: 2, z: 8 })
  })
})

const theme = (light = false): Wiki3dTheme => ({
  accent: { r: 0.4, g: 0.5, b: 1, a: 1 },
  text: { r: 0.9, g: 0.9, b: 0.9, a: 1 },
  muted: { r: 0.4, g: 0.4, b: 0.45, a: 1 },
  bg: light ? { r: 1, g: 1, b: 1, a: 1 } : { r: 0.05, g: 0.05, b: 0.05, a: 1 },
  homeSoft: { r: 0.56, g: 0.66, b: 1, a: 1 },
  light,
  font: 'sans-serif',
})

const kind = (k: Partial<NodeKind> = {}): NodeKind => ({ missing: false, home: false, hub: false, org: false, orgColor: null, ...k })
const idle = { selected: false, hovered: false, dimmed: false }

describe('nodeVisual', () => {
  it('maps the 2D meanings onto 3D shapes', () => {
    const t = theme()
    expect(nodeVisual(kind(), idle, t)).toMatchObject({ shape: 'sphere', opacity: 0.85, radius: R_NOTE, glow: false })
    expect(nodeVisual(kind({ missing: true }), idle, t)).toMatchObject({ shape: 'wire', opacity: 0.45, glow: false })
    expect(nodeVisual(kind({ home: true }), idle, t)).toMatchObject({ shape: 'sphere', color: t.homeSoft, glow: true })
    expect(nodeVisual(kind({ hub: true, orgColor: '#ff0000' }), idle, t)).toMatchObject({ shape: 'octa', color: { r: 1, g: 0, b: 0, a: 1 }, glow: true })
    expect(nodeVisual(kind({ hub: true }), idle, t)).toMatchObject({ shape: 'octa', color: t.muted })
    expect(nodeVisual(kind({ org: true, orgColor: '#00ff00' }), idle, t)).toMatchObject({ shape: 'box', glow: true })
    expect(nodeVisual(kind(), { ...idle, selected: true }, t)).toMatchObject({ color: t.accent, radius: R_SELECTED, glow: true, opacity: 1 })
    expect(nodeVisual(kind(), { ...idle, hovered: true }, t)).toMatchObject({ color: t.accent, opacity: 0.75 })
    expect(nodeVisual(kind(), { ...idle, dimmed: true }, t).opacity).toBeCloseTo(0.85 * 0.2, 6)
  })
})

describe('Wiki3dScene', () => {
  it('shares geometries and materials per visual state; hover swaps references only', () => {
    const s = new Wiki3dScene(theme())
    const t = s.theme
    const a = s.createNode('a', nodeVisual(kind(), idle, t))
    const b = s.createNode('b', nodeVisual(kind(), idle, t))
    const meshA = a.children[0] as unknown as { geometry: unknown; material: unknown }
    const meshB = b.children[0] as unknown as { geometry: unknown; material: unknown }
    expect(meshA.geometry).toBe(meshB.geometry)
    expect(meshA.material).toBe(meshB.material)
    const before = meshA.material
    s.applyVisual('a', nodeVisual(kind(), { ...idle, dimmed: true }, t))
    const dimMat = meshA.material
    expect(dimMat).not.toBe(before)
    s.applyVisual('b', nodeVisual(kind(), { ...idle, dimmed: true }, t))
    expect(meshB.material).toBe(dimMat)
    s.dispose()
  })

  it('glow: a halo on key nodes only, additive on dark, normal on light, off on degrade', () => {
    // A fake canvas so the shared halo texture can be drawn in node.
    vi.stubGlobal('document', {
      createElement: () => ({
        width: 0,
        height: 0,
        getContext: () => ({
          createRadialGradient: () => ({ addColorStop: () => undefined }),
          fillRect: () => undefined,
          fillStyle: '',
        }),
      }),
    })
    try {
      for (const light of [false, true]) {
        const s = new Wiki3dScene(theme(light))
        const home = s.createNode('home', nodeVisual(kind({ home: true }), idle, s.theme))
        const note = s.createNode('note', nodeVisual(kind(), idle, s.theme))
        const sel = s.createNode('sel', nodeVisual(kind(), { ...idle, selected: true }, s.theme))
        const sprites = (g: typeof home) => g.children.filter((c) => c.type === 'Sprite') as unknown as Sprite[]
        expect(sprites(note)).toHaveLength(0)
        expect(sprites(home)).toHaveLength(1)
        expect(sprites(sel)).toHaveLength(1)
        const mat = sprites(home)[0].material
        expect(mat.blending).toBe(light ? NormalBlending : AdditiveBlending)
        expect(mat.opacity).toBe(light ? 0.25 : 0.35)
        // One shared texture for every halo.
        expect(sprites(sel)[0].material.map).toBe(mat.map)
        s.glowEnabled = false
        s.applyVisual('home', nodeVisual(kind({ home: true }), idle, s.theme))
        expect(sprites(home)[0].visible).toBe(false)
        s.dispose()
        expect(s.isDisposed).toBe(true)
      }
    } finally {
      vi.unstubAllGlobals()
    }
  })

  it('prune forgets nodes the graph removed', () => {
    const s = new Wiki3dScene(theme())
    s.createNode('a', nodeVisual(kind(), idle, s.theme))
    s.createNode('b', nodeVisual(kind(), idle, s.theme))
    s.prune(new Set(['b']))
    expect(s.nodeIds()).toEqual(['b'])
    s.dispose()
  })
})
