// @vitest-environment jsdom
// The overlay on the stage (S4): boxes over the still from the still's own
// camera and each object's bounds, holds in the holder's presence token
// with "Atlas · dining set" and a lock, the live change step by step
// (ghost, slide, tag, arrows, settle) and the same under reduced motion
// (no slide: the end state and the tag), the hidden-object rule, and the
// chip on the edge for what is off the picture, which frames it on click.
// The stills come from a source that hands back the stand-in's cameras
// and bounds without drawing.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SCENARIOS } from '@/lib/live-view/scene/scenarios-more'
import { simulate } from '@/lib/live-view/scene/turn'
import { stillCamera } from '@/lib/live-view/scene/project'
import { standInCamera } from '@/lib/live-view/scene/stand-in/camera'
import { sampleBounds } from '@/lib/live-view/scene/stand-in/shapes'
import { SHAPE_ASPECT, stillHeight, type PictureSource } from '@/lib/live-view/scene/picture-source'
import type { SimState } from '@/lib/live-view/scene/sim'
import { SceneStage } from './SceneStage'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!
const stateAt = (id: string, label: string | number): SimState => {
  const s = sc(id)
  const i = typeof label === 'number' ? label : s.steps.findIndex((x) => x.label === label)
  expect(i, `${id}: ${label}`).toBeGreaterThanOrEqual(0)
  return simulate(s, i)
}

/** The stand-in's cameras and bounds, no pixels. */
function cameraSource(S: () => SimState): PictureSource {
  return {
    kind: 'sample',
    still: (req) => {
      const D = S().revDocs[req.rev] ?? S().D
      return Promise.resolve({ rev: req.rev, view: req.view, shape: req.shape, width: 1920, height: stillHeight(1920, req.shape), image: { kind: 'rgba', data: new Uint8ClampedArray(4) }, camera: stillCamera(standInCamera(D, req.view, SHAPE_ASPECT[req.shape])), ids: null, renderMs: 1 })
    },
    bounds: sampleBounds,
    close: () => undefined,
  }
}

const SIZE = { width: 1100, height: 700 }
let current: SimState
function mount(id: string, label: string | number) {
  current = stateAt(id, label)
  const source = cameraSource(() => current)
  const el = (S: SimState) => <SceneStage S={S} control={{ retryPicture: () => undefined }} source={source} narrow={false} size={SIZE} offscreen={[]} onJump={() => undefined} />
  const r = render(el(current))
  return {
    ...r,
    step: (label: string | number) => {
      current = stateAt(id, label)
      r.rerender(el(current))
    },
  }
}
const settle = () => act(async () => undefined)
const tags = (kind?: string) => screen.queryAllByTestId('lv-tag').filter((t) => !kind || t.getAttribute('data-kind') === kind)
const boxes = (style?: string) => [...document.querySelectorAll('[data-testid="lv-scene-overlay"] path.lv-bx')].filter((p) => !style || p.getAttribute('data-style') === style)
const overlay = () => screen.getByTestId('lv-scene-overlay')

function reducedMotion(on: boolean): void {
  vi.stubGlobal('matchMedia', (q: string) => ({ matches: on && q.includes('prefers-reduced-motion'), media: q, addEventListener: () => undefined, removeEventListener: () => undefined, addListener: () => undefined, removeListener: () => undefined, onchange: null, dispatchEvent: () => false }))
}

beforeEach(() => {
  const h = document.documentElement
  h.setAttribute('data-style', 'glass')
  h.setAttribute('data-palette', 'obsidian')
  h.setAttribute('data-scheme', 'dark')
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
  reducedMotion(false)
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('holds on the picture', () => {
  it('a thin box per held object and one label per hold: lock, “Atlas · dining set”, the full note on hover', async () => {
    mount('turn', 2)
    await settle()
    expect(boxes('hold').length).toBe(4)
    const held = tags('hold')
    expect(held.map((t) => t.textContent)).toEqual(['Atlas · dining set', 'Brook · vase'])
    expect(held[0].getAttribute('title')).toBe('Atlas is holding Table, Chair.001 and Chair.002: arranging the dining set')
    expect(held[0].querySelector('svg')).toBeTruthy() // the lock
    expect(held[0].style.getPropertyValue('--lv-c')).toBe('var(--color-presence-1)')
    expect(held[1].style.getPropertyValue('--lv-c')).toBe('var(--color-presence-3)')
    for (const b of boxes('hold')) expect((b as SVGElement).style.getPropertyValue('--lv-c')).toMatch(/^var\(--color-presence-[13]\)$/)
  })
})

describe('the live change', () => {
  it('slides from a ghost of the old place, then tags it with arrows, then settles and fades', async () => {
    vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'setTimeout', 'clearTimeout', 'Date'] })
    const m = mount('turn', 3)
    await settle()
    expect(overlay().getAttribute('data-t')).toBe('0')
    expect(boxes('ghost').length).toBe(2) // Table, and Vase riding on it
    expect(tags('live')).toEqual([])
    expect(document.querySelectorAll('path.lv-dir').length).toBe(0)
    await act(async () => {
      vi.advanceTimersByTime(500)
    })
    expect(overlay().getAttribute('data-t')).toBe('1')
    expect(tags('live').map((t) => t.textContent)).toEqual(['Atlas moved TableVase moves with it'])
    expect(tags('live')[0].hasAttribute('data-fading')).toBe(false)
    expect([...document.querySelectorAll('path.lv-dir')].map((p) => p.getAttribute('data-kind'))).toEqual(['move'])
    m.step(4)
    await settle()
    expect(boxes('ghost')).toEqual([])
    expect(boxes('settled').length).toBeGreaterThan(0)
    expect(tags('live')[0].hasAttribute('data-fading')).toBe(true)
    expect([...document.querySelectorAll('path.lv-dir')].every((p) => p.hasAttribute('data-fading'))).toBe(true)
    m.step(9)
    await settle()
    expect(tags('live')).toEqual([])
  })

  it('under reduced motion: no slide, the end state and the tag at once, and the tag stays until it is history', async () => {
    reducedMotion(true)
    const m = mount('turn', 3)
    await settle()
    expect(overlay().getAttribute('data-t')).toBe('1')
    expect(overlay().hasAttribute('data-motion')).toBe(false)
    expect(tags('live').map((t) => t.textContent)).toEqual(['Atlas moved TableVase moves with it'])
    m.step(4)
    await settle()
    expect(tags('live').length).toBe(1)
    expect(tags('live')[0].hasAttribute('data-fading')).toBe(false)
  })
})

describe('hidden objects', () => {
  it('a dashed “(hidden)” ghost box when an agent holds one', async () => {
    mount('onpic', 'hidden held')
    await settle()
    const wall = boxes().find((b) => b.getAttribute('data-a') === '@obj:6')!
    expect(wall.hasAttribute('data-hidden')).toBe(true)
    expect(tags('note').map((t) => t.textContent)).toContain('Wall.Left (hidden)')
  })
})

describe('off the picture', () => {
  it('a chip on the edge points to your pick; clicking it frames the object', async () => {
    mount('onpic', 'off the picture')
    await settle()
    const chip = screen.getByRole('button', { name: 'Sofa is off the picture. Frame it' })
    expect(chip.hasAttribute('data-k2-focus')).toBe(true)
    expect(['left', 'right', 'top', 'bottom']).toContain(chip.getAttribute('data-side'))
    fireEvent.click(chip)
    await settle()
    expect(screen.getByTestId('lv-scene-stage').getAttribute('data-view')).toBe('@obj:Sofa')
    expect(screen.queryByRole('button', { name: 'Sofa is off the picture. Frame it' })).toBeNull()
  })
})

describe('labels never touch', () => {
  it('every label placed, none touching, in the busiest step', async () => {
    mount('who', 0)
    await settle()
    const all = [...tags(), ...screen.queryAllByTestId('lv-offchip')]
    expect(all.length).toBeGreaterThan(3)
    for (const t of all) {
      expect(t.style.visibility, t.textContent ?? '').not.toBe('hidden')
      expect(t.hasAttribute('data-touching'), t.textContent ?? '').toBe(false)
    }
  })
})
