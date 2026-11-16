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
import { noticeView } from '@/lib/live-view/shared/notices'
import type { HeroRefusal } from '../shell/shell-config'

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
function mount(id: string, label: string | number, extra: { refusal?: HeroRefusal | null } = {}) {
  current = stateAt(id, label)
  const source = cameraSource(() => current)
  const el = (S: SimState) => <SceneStage S={S} control={{ retryPicture: () => undefined }} source={source} narrow={false} size={SIZE} offscreen={[]} onJump={() => undefined} {...extra} />
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

describe('labels and the stage’s chrome', () => {
  it('a label keeps 6 px clear of the stage’s own plates (the view picker here)', async () => {
    mount('turn', 2)
    await settle()
    const t0 = tags('hold')[0]
    const at = { x: parseFloat(t0.style.left), y: parseFloat(t0.style.top) }
    cleanup()
    // The view picker laid right where that label went.
    const zero = new DOMRect(0, 0, 0, 0)
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      return this.matches('.lv-vbtn') ? new DOMRect(at.x, at.y, 120, 30) : zero
    })
    mount('turn', 2)
    await settle()
    const t1 = tags('hold')[0]
    const r = { x0: parseFloat(t1.style.left), y0: parseFloat(t1.style.top) }
    const w = 22 + (t1.textContent ?? '').length * 7
    const clear = r.x0 + w + 6 <= at.x || r.x0 >= at.x + 120 + 6 || r.y0 + 22 + 6 <= at.y || r.y0 >= at.y + 30 + 6
    expect(clear, JSON.stringify({ at, r })).toBe(true)
  })
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
    expect(tags('note').map((t) => t.textContent)).not.toContain('Wall.Left (hidden)')
    expect(tags('hold').map((t) => t.textContent).join('|')).toMatch(/\(hidden\)/)
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
    expect(screen.getByTestId('lv-scene-stage').getAttribute('data-view')).toBe('@obj:15')
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

describe('a refusal on the picture', () => {
  const refusal = (onAction = vi.fn()): HeroRefusal => {
    const notice = { kind: 'refused' as const, actor: 'Brook', verb: 'rename', target: 'Chair.002', holder: 'Atlas', ask: { id: 'atlas', name: 'Atlas' } }
    return { notice, view: noticeView(notice, { fileName: '' }), onAction }
  }

  it('a callout by the chair: the shell’s words, the holder’s colour, a pin and a line, the code only in its tooltip', async () => {
    mount('refuse', 'refused', { refusal: refusal() })
    await settle()
    const c = screen.getByTestId('lv-callout')
    expect(c.getAttribute('role')).toBe('status')
    expect(c.getAttribute('data-a')).toBe('@obj:12')
    expect(c.textContent).toContain('Brook couldn’t rename Chair.002: Atlas is holding it.')
    expect(c.querySelector('b')?.textContent).toBe('Brook')
    expect(c.getAttribute('title')).toBe('Code MPC-E301')
    expect(c.textContent).not.toContain('MPC-E301')
    expect(c.style.getPropertyValue('--lv-c')).toMatch(/^var\(--color-presence-[1-4]\)$/)
    expect(screen.getByTestId('lv-callout-pin')).toBeTruthy()
    expect(document.querySelector('[data-testid="lv-callout-line"] path')).toBeTruthy()
  })

  it('Ask Atlas is the shell’s action, a focusable chip', async () => {
    const onAction = vi.fn()
    mount('refuse', 'refused', { refusal: refusal(onAction) })
    await settle()
    const ask = screen.getByRole('button', { name: 'Ask Atlas' })
    expect(ask.hasAttribute('data-k2-focus')).toBe(true)
    fireEvent.click(ask)
    expect(onAction).toHaveBeenCalledWith('ask')
  })

  it('no one to ask: the words alone, in the warning colour', async () => {
    const notice = { kind: 'refused' as const, actor: 'Atlas', verb: 'move', target: 'Clock hand', holder: null, why: 'animation drives it', ask: null }
    mount('limits', 'driven', { refusal: { notice, view: noticeView(notice, { fileName: '' }), onAction: vi.fn() } })
    await settle()
    const c = screen.getByTestId('lv-callout')
    expect(c.textContent).toContain('Atlas couldn’t move Clock hand: animation drives it.')
    expect(c.querySelector('button')).toBeNull()
    expect(c.hasAttribute('data-warn')).toBe(true)
  })

  it('goes when the hold is released', async () => {
    const m = mount('refuse', 'refused', { refusal: refusal() })
    await settle()
    expect(screen.queryByTestId('lv-callout')).toBeTruthy()
    m.step('released')
    await settle()
    expect(screen.queryByTestId('lv-callout')).toBeNull()
  })
})
