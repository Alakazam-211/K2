// @vitest-environment jsdom
// PageLayers (macOS underlay): the set the window sends whole on each change.
// Fail loud: every field the native side reads is asserted.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { claimBrowserOverlay, resetBrowserOverlaysForTest, type OverlayClaim } from './browser-overlay'
import {
  buildPageLayers,
  cornerRadius,
  freezesInUnderlay,
  PageLayersSync,
  type PageEntry,
  type PageLayers,
} from './page-layers'

const VIEW: [number, number] = [1200, 800]

function page(windowId: string, order: number, x: number, mode: 'live' | 'still' = 'live', element: Element | null = null): PageEntry {
  return { windowId, order, rect: { x, y: 40, width: 400, height: 760, radius: 16 }, mode, element }
}

function claim(id: number, kind: OverlayClaim['kind'], area: OverlayClaim['area'], element: Element | null = null): OverlayClaim {
  return { id, kind, area, element }
}

afterEach(() => {
  resetBrowserOverlaysForTest()
})

describe('buildPageLayers', () => {
  it('orders pages back to front and keeps rect, corners and mode', () => {
    const set = buildPageLayers({
      parentWindow: 'main',
      generation: 7,
      dpr: 2,
      viewport: VIEW,
      pages: [page('b', 2, 600), page('a', 1, 0, 'still')],
      claims: [],
    })
    expect(set).toEqual({
      parentWindow: 'main',
      generation: 7,
      dpr: 2,
      viewport: VIEW,
      layers: [
        { windowId: 'a', z: 1, rect: { x: 0, y: 40, width: 400, height: 760, radius: 16 }, minus: [], mode: 'still' },
        { windowId: 'b', z: 2, rect: { x: 600, y: 40, width: 400, height: 760, radius: 16 }, minus: [], mode: 'live' },
      ],
      claims: [],
    } satisfies PageLayers)
  })

  it('menus, popovers, toasts and dialogs become claims; drags and motion do not', () => {
    const set = buildPageLayers({
      parentWindow: 'main',
      generation: 1,
      dpr: 1,
      viewport: VIEW,
      pages: [page('a', 1, 0)],
      claims: [
        claim(1, 'menu', { x: 10, y: 50, width: 100, height: 200 }),
        claim(2, 'toast', { x: 900, y: 700, width: 280, height: 60 }),
        claim(3, 'popover', { x: 5, y: 5, width: 0, height: 10 }),
        claim(4, 'modal', 'fullscreen'),
        claim(5, 'drag', 'fullscreen'),
        claim(6, 'motion', 'fullscreen'),
      ],
    })
    expect(set.claims).toEqual([
      { id: 1, kind: 'menu', x: 10, y: 50, width: 100, height: 200 },
      { id: 2, kind: 'toast', x: 900, y: 700, width: 280, height: 60 },
      { id: 4, kind: 'modal', x: 0, y: 0, width: 1200, height: 800 },
    ])
  })

  it('a claim that holds a page is minus for the other pages, not a claim', () => {
    const dialog = document.createElement('div')
    const embedded = document.createElement('div')
    dialog.appendChild(embedded)
    const set = buildPageLayers({
      parentWindow: 'main',
      generation: 1,
      dpr: 1,
      viewport: VIEW,
      pages: [page('tab', 1, 0), page('oauth', 2, 300, 'live', embedded)],
      claims: [claim(9, 'modal', { x: 200, y: 100, width: 600, height: 500 }, dialog)],
    })
    expect(set.claims).toEqual([])
    expect(set.layers.find((l) => l.windowId === 'oauth')?.minus).toEqual([])
    expect(set.layers.find((l) => l.windowId === 'tab')?.minus).toEqual([{ x: 200, y: 100, width: 600, height: 500 }])
  })

  it('a page with no size is left out', () => {
    const set = buildPageLayers({
      parentWindow: 'main',
      generation: 1,
      dpr: 1,
      viewport: VIEW,
      pages: [{ ...page('a', 1, 0), rect: null }, { ...page('b', 2, 0), rect: { x: 0, y: 0, width: 0, height: 10 } }],
      claims: [],
    })
    expect(set.layers).toEqual([])
  })
})

describe('face rules in underlay', () => {
  it('only dialogs, drags and motion freeze a page', () => {
    expect(freezesInUnderlay(claim(1, 'menu', 'fullscreen'))).toBe(false)
    expect(freezesInUnderlay(claim(1, 'popover', 'fullscreen'))).toBe(false)
    expect(freezesInUnderlay(claim(1, 'toast', 'fullscreen'))).toBe(false)
    expect(freezesInUnderlay(claim(1, 'modal', 'fullscreen'))).toBe(true)
    expect(freezesInUnderlay(claim(1, 'drag', 'fullscreen'))).toBe(true)
    expect(freezesInUnderlay(claim(1, 'motion', 'fullscreen'))).toBe(true)
  })

  it('dock corners become a radius per corner', () => {
    expect(cornerRadius(16, [true, true, true, true])).toBe(16)
    expect(cornerRadius(16, [false, false, true, true])).toEqual([0, 0, 16, 16])
    expect(cornerRadius(16, [false, false, false, false])).toBe(0)
  })
})

describe('PageLayersSync', () => {
  function harness(opts: { active?: boolean; failWith?: string } = {}) {
    const sent: PageLayers[] = []
    const frames: Array<() => void> = []
    const tasks: Array<() => void> = []
    const fail = vi.fn()
    const sync = new PageLayersSync({
      parentWindow: () => 'window-2',
      active: () => opts.active ?? true,
      send: async (set) => {
        if (opts.failWith) throw new Error(opts.failWith)
        sent.push(set)
      },
      fail,
      hoverPrivateFallback: () => false,
      viewport: () => VIEW,
      dpr: () => 2,
      raf: (fn) => frames.push(fn),
      microtask: (fn) => tasks.push(fn),
    })
    const run = async (q: Array<() => void>): Promise<void> => {
      for (const fn of q.splice(0)) fn()
      for (let i = 0; i < 4; i++) await Promise.resolve()
    }
    return { sent, frames, tasks, fail, sync, run }
  }

  const src = (windowId: string, x: number, mode: () => 'live' | 'still' = () => 'live') => ({
    windowId,
    element: () => null,
    measure: () => ({ x, y: 40, width: 400, height: 760 }),
    mode,
  })

  it('sends one whole set per frame, generations rising', async () => {
    const h = harness()
    h.sync.register(src('a', 0))
    h.sync.register(src('b', 600))
    h.sync.frame()
    expect(h.frames).toHaveLength(1)
    await h.run(h.frames)
    expect(h.sent).toHaveLength(1)
    expect(h.sent[0].parentWindow).toBe('window-2')
    expect(h.sent[0].layers.map((l) => l.windowId)).toEqual(['a', 'b'])
    h.sync.frame()
    await h.run(h.frames)
    expect(h.sent[1].generation).toBeGreaterThan(h.sent[0].generation)
  })

  it('a claim change sends at once, and its release gives the area back', async () => {
    const h = harness()
    h.sync.register(src('a', 0))
    await h.run(h.frames)
    const menu = claimBrowserOverlay({ kind: 'menu', area: { x: 10, y: 50, width: 100, height: 200 } })
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.claims).toEqual([{ id: menu.id, kind: 'menu', x: 10, y: 50, width: 100, height: 200 }])
    menu.release()
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.claims).toEqual([])
  })

  it('a mode change reads the pane at send time', async () => {
    const h = harness()
    let mode: 'live' | 'still' = 'live'
    h.sync.register(src('a', 0, () => mode))
    await h.run(h.frames)
    mode = 'still'
    h.sync.now()
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.layers[0].mode).toBe('still')
  })

  it('the last page leaving sends one empty set, then nothing', async () => {
    const h = harness()
    const off = h.sync.register(src('a', 0))
    await h.run(h.frames)
    off()
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.layers).toEqual([])
    const n = h.sent.length
    h.sync.now()
    await h.run(h.tasks)
    expect(h.sent).toHaveLength(n)
  })

  it('nothing is sent while the underlay is off', async () => {
    const h = harness({ active: false })
    h.sync.register(src('a', 0))
    await h.run(h.frames)
    expect(h.sent).toEqual([])
  })

  it('a native failure falls back once and stops sending', async () => {
    const h = harness({ failWith: 'underlay: the UI view has no layer to mask' })
    h.sync.register(src('a', 0))
    await h.run(h.frames)
    expect(h.fail).toHaveBeenCalledTimes(1)
    expect(String(h.fail.mock.calls[0][0])).toContain('no layer to mask')
    h.sync.frame()
    await h.run(h.frames)
    expect(h.fail).toHaveBeenCalledTimes(1)
  })
})
