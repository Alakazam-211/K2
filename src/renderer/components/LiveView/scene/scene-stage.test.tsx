// @vitest-environment jsdom
// The scene's picture on its stage: the dark stage in the light looks, each
// picture state's words and next step, the view picker (menu and keys), the
// keys on the picture (Home, 0, ., Esc), the details popover, and the still
// it asks for. A fake PictureSource stands in for the stand-in, so nothing
// is drawn.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SCENARIOS } from '@/lib/live-view/scene/scenarios-more'
import { simulate } from '@/lib/live-view/scene/turn'
import type { PictureSource, Still, StillRequest } from '@/lib/live-view/scene/picture-source'
import { sampleBounds } from '@/lib/live-view/scene/stand-in/shapes'
import { SceneStage } from './SceneStage'

const sc = (id: string) => SCENARIOS.find((s) => s.id === id)!
/** A scenario's step by its label, or by its index. */
const stateAt = (id: string, label: string | number) => {
  const s = sc(id)
  const i = typeof label === 'number' ? label : s.steps.findIndex((x) => x.label === label)
  expect(i, `${id}: ${label}`).toBeGreaterThanOrEqual(0)
  return simulate(s, i)
}

function fakeSource(): PictureSource & { asked: StillRequest[] } {
  const asked: StillRequest[] = []
  return {
    kind: 'sample',
    asked,
    still: (req) => {
      asked.push(req)
      const st: Still = { rev: req.rev, view: req.view, shape: req.shape, width: 1920, height: 1080, image: { kind: 'rgba', data: new Uint8ClampedArray(4) }, camera: { view: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, -5, 1], proj: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, -1, -1, 0, 0, -0.1, 0] }, ids: null, renderMs: 200 }
      return Promise.resolve(st)
    },
    bounds: sampleBounds,
    close: () => undefined,
  }
}

const SIZE = { width: 1100, height: 700 }
let retry = vi.fn()
function mount(id: string, label: string | number, o: { narrow?: boolean; source?: PictureSource } = {}) {
  retry = vi.fn()
  const S = stateAt(id, label)
  const source = o.source ?? fakeSource()
  const r = render(<SceneStage S={S} control={{ retryPicture: retry }} source={source} narrow={!!o.narrow} size={SIZE} offscreen={[]} onJump={() => undefined} />)
  return { ...r, S, source }
}
const stage = () => screen.getByTestId('lv-scene-stage')
const picker = () => screen.getByRole('button', { name: /^View:/ })

beforeEach(() => {
  const h = document.documentElement
  h.setAttribute('data-style', 'glass')
  h.setAttribute('data-palette', 'veil')
  h.setAttribute('data-scheme', 'light')
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null)
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the stage', () => {
  it('is dark in a light look: it carries the look’s dark palette', () => {
    mount('turn', 1)
    expect(stage().getAttribute('data-style')).toBe('glass')
    expect(stage().getAttribute('data-palette')).toBe('obsidian')
    expect(stage().getAttribute('data-scheme')).toBe('dark')
  })

  it('is dark in Terminal paper too, and keeps Charcoal as it is', () => {
    document.documentElement.setAttribute('data-style', 'terminal')
    document.documentElement.setAttribute('data-palette', 'paper')
    mount('turn', 1)
    expect(stage().getAttribute('data-palette')).toBe('charcoal')
    expect(stage().getAttribute('data-k2-chamfer')).toBe('surface')
  })

  it('asks for the shown rev’s still at 1920 with its ids, in the pane’s shape', async () => {
    const { source, S } = mount('turn', 1)
    await act(async () => undefined)
    expect((source as ReturnType<typeof fakeSource>).asked[0]).toEqual({ rev: S.pv.shown, view: '@scene:main', shape: 'wide', width: 1920, ids: true })
    expect(screen.getByRole('img', { name: 'Clay picture of the scene, Scene' })).toBeTruthy()
  })

  it('shows the key hint, the compass and ⓘ', async () => {
    mount('turn', 1)
    await act(async () => undefined)
    expect(screen.getByTestId('lv-scene-keys').textContent).toBe('HomeScene0Camera.FrameEscBack')
    expect(screen.getByTitle('Which way this picture faces: X red, Y green, Z blue (up)')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Picture details' })).toBeTruthy()
  })

  it('drops the key hint when narrow', () => {
    mount('turn', 1, { narrow: true })
    expect(screen.queryByTestId('lv-scene-keys')).toBeNull()
  })
})

describe('picture states', () => {
  it('first open: "Opening…" with its steps, then the picture', () => {
    mount('first', 'connecting')
    const card = screen.getByTestId('lv-scene-card')
    expect(within(card).getByText('Opening…')).toBeTruthy()
    expect(within(card).getAllByRole('listitem').map((li) => `${li.textContent}:${li.getAttribute('data-state')}`)).toEqual(['Connecting:now', 'The list of objects:next', 'The first picture:next'])
    cleanup()
    mount('first', 'list in')
    expect(screen.getAllByRole('listitem').map((li) => li.getAttribute('data-state'))).toEqual(['done', 'done', 'now'])
    cleanup()
    mount('first', 'first picture')
    expect(screen.queryByTestId('lv-scene-card')).toBeNull()
  })

  it('updating: the older still stays up, "Updating picture…" says so', () => {
    mount('failed', 'updating')
    expect(screen.getByTestId('lv-scene-chips').textContent).toBe('Updating picture…')
  })

  it('failed with an older still: "Couldn’t update the picture", the older mark, and Retry asks again', () => {
    mount('failed', 'couldn\'t update')
    expect(screen.getByText('Older picture: one change behind')).toBeTruthy()
    const chips = screen.getByTestId('lv-scene-chips')
    expect(within(chips).getByText('Couldn’t update the picture')).toBeTruthy()
    fireEvent.click(within(chips).getByRole('button', { name: 'Retry' }))
    expect(retry).toHaveBeenCalledTimes(1)
  })

  it('no still at all: "Couldn’t draw this view", Retry, and Try Camera 1 switches the view', () => {
    mount('failfirst', 'failed')
    const card = screen.getByTestId('lv-scene-card')
    expect(within(card).getByRole('heading', { name: 'Couldn’t draw this view' })).toBeTruthy()
    expect(within(card).getByText('The picture didn’t finish. The outliner and numbers are still live.')).toBeTruthy()
    fireEvent.click(within(card).getByRole('button', { name: 'Retry' }))
    expect(retry).toHaveBeenCalledTimes(1)
    fireEvent.click(within(card).getByRole('button', { name: 'Try Camera 1' }))
    expect(picker().textContent).toContain('Camera 1')
    expect(retry).toHaveBeenCalledTimes(2)
  })

  it('no picture yet: the card and a dot per object’s origin on a ground grid', () => {
    const { S } = mount('nopic', 'no picture')
    const card = screen.getByTestId('lv-scene-card')
    expect(within(card).getByRole('heading', { name: 'No picture for this file yet' })).toBeTruthy()
    expect(within(card).getAllByRole('button').map((b) => b.textContent)).toEqual(['Retry', 'Show the list'])
    const dots = screen.getByTestId('lv-scene-ground').querySelectorAll('circle')
    const placed = S.D.lists['objs'].filter((a) => !['camera', 'light'].includes(S.D.kinds[a]))
    expect(dots.length).toBeGreaterThan(10)
    expect(dots.length).toBeLessThanOrEqual(placed.length)
  })

  it('an empty scene says where objects come from', () => {
    mount('empty', 'no objects')
    expect(within(screen.getByTestId('lv-scene-card')).getByRole('heading', { name: 'No objects in this scene' })).toBeTruthy()
  })
})

describe('the view picker', () => {
  it('opens a menu of views with the one on screen checked; arrows move, Enter picks and says so', () => {
    mount('turn', 1)
    fireEvent.click(picker())
    const menu = screen.getByRole('menu', { name: 'Views' })
    const rows = within(menu).getAllByRole('menuitemradio')
    expect(rows.map((r) => r.getAttribute('aria-label'))).toEqual(['Scene, The whole room', 'Camera 1, The file’s main camera', 'Camera 2, A camera in the file', 'Framed object, Table', 'Turntable, 8 stills around the scene', 'Atlas’s view, Follows Atlas: now Table, framed'])
    expect(rows[0].getAttribute('aria-checked')).toBe('true')
    expect(document.activeElement).toBe(rows[0])
    fireEvent.keyDown(menu, { key: 'ArrowDown' })
    fireEvent.keyDown(menu, { key: 'ArrowDown' })
    expect(document.activeElement).toBe(rows[2])
    fireEvent.click(rows[2])
    expect(screen.queryByRole('menu')).toBeNull()
    expect(picker().textContent).toContain('Camera 2')
    expect(screen.getByRole('status', { name: 'View changed' }).textContent).toBe('Now looking through Camera 2')
  })

  it('Esc closes the menu and gives focus back to the picker', () => {
    mount('turn', 1)
    fireEvent.click(picker())
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' })
    expect(screen.queryByRole('menu')).toBeNull()
    expect(document.activeElement).toBe(picker())
  })

  it('shows the keyboard highlight the scenario puts on a row', () => {
    mount('keys', 'down twice')
    const row = within(screen.getByRole('menu')).getByRole('menuitemradio', { name: 'Camera 2, A camera in the file' })
    expect(row.hasAttribute('data-kbd')).toBe(true)
  })
})

describe('keys on the picture', () => {
  it('0 the main camera, . frames the selection, Home Scene, Esc back', () => {
    mount('turn', 1)
    const st = stage()
    // The view on screen is K2 Render's own address, so a viewAddress maps straight onto it.
    expect(st.getAttribute('data-view')).toBe('@scene:main')
    fireEvent.keyDown(st, { key: '0' })
    expect(picker().textContent).toContain('Camera 1')
    expect(st.getAttribute('data-view')).toBe('@cam:1')
    fireEvent.keyDown(st, { key: '.' })
    expect(picker().textContent).toContain('Table, framed')
    expect(st.getAttribute('data-view')).toBe('@obj:10')
    fireEvent.keyDown(st, { key: 'Home' })
    expect(picker().textContent).toContain('Scene')
    fireEvent.keyDown(st, { key: 'Escape' })
    expect(picker().textContent).toContain('Table, framed')
    fireEvent.keyDown(st, { key: 'Escape' })
    expect(picker().textContent).toContain('Camera 1')
  })

  it('leaves keys it can’t use alone (they reach the rest of the app)', () => {
    mount('turn', 1)
    const e = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })
    stage().dispatchEvent(e)
    expect(e.defaultPrevented).toBe(false)
    const z = new KeyboardEvent('keydown', { key: '0', bubbles: true, cancelable: true })
    stage().dispatchEvent(z)
    expect(z.defaultPrevented).toBe(true)
  })

  it('Esc closes the details first, then the menu, then goes back', () => {
    mount('turn', 1)
    fireEvent.keyDown(stage(), { key: '0' })
    fireEvent.click(screen.getByRole('button', { name: 'Picture details' }))
    expect(screen.getByRole('dialog', { name: 'Picture details' })).toBeTruthy()
    fireEvent.keyDown(stage(), { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(picker().textContent).toContain('Camera 1')
    fireEvent.keyDown(stage(), { key: 'Escape' })
    expect(picker().textContent).toContain('Scene')
  })
})

describe('the details popover', () => {
  it('speaks plainly, copies the raw facts for an agent, and closes', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    mount('nomod', 'approximate box')
    await act(async () => undefined)
    const ib = screen.getByRole('button', { name: 'Picture details' })
    fireEvent.click(ib)
    expect(ib.getAttribute('aria-expanded')).toBe('true')
    const d = screen.getByRole('dialog', { name: 'Picture details' })
    const terms = within(d).getAllByRole('term').map((t) => t.textContent)
    expect(terms).toEqual(['Picture', 'Size', 'Looking at', 'Modifiers', 'Selected', 'Its box'])
    expect(d.textContent).not.toMatch(/@obj|@scene/)
    await act(async () => fireEvent.click(within(d).getByRole('button', { name: 'Copy for an agent' })))
    expect(writeText).toHaveBeenCalledWith('change 60 · picture of change 60 · @scene:main · @obj:16')
    expect(within(d).getByRole('button', { name: 'Copied' })).toBeTruthy()
    fireEvent.click(ib)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('is a sheet across the stage when narrow', () => {
    mount('turn', 1, { narrow: true })
    fireEvent.click(screen.getByRole('button', { name: 'Picture details' }))
    expect(screen.getByRole('dialog').hasAttribute('data-sheet')).toBe(true)
  })
})
