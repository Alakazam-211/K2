// @vitest-environment jsdom
// PageLayers (macOS underlay): the set the window sends whole on each change.
// Fail loud: every field the native side reads is asserted.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { claimBrowserOverlay, resetBrowserOverlaysForTest, type OverlayClaim } from './browser-overlay'
import {
  buildPageLayers,
  toWindowPx,
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
  vi.restoreAllMocks()
  styles.clear()
  document.documentElement.removeAttribute('data-style')
  document.body.replaceChildren()
})

// Computed styles by element: jsdom doesn't cascade the app's CSS, so each
// test says what the browser would compute for the boxes it builds.
const styles = new Map<Element, Record<string, string>>()

function mockStyles() {
  return vi.spyOn(window, 'getComputedStyle').mockImplementation(
    (el: Element) => ({ getPropertyValue: (p: string) => styles.get(el)?.[p] ?? '' }) as CSSStyleDeclaration,
  )
}

function corners(tl: string, tr = tl, br = tl, bl = tl): Record<string, string> {
  return {
    'border-top-left-radius': tl,
    'border-top-right-radius': tr,
    'border-bottom-right-radius': br,
    'border-bottom-left-radius': bl,
  }
}

/** A box at `r` with the given attributes and computed style. */
function box(
  r: { x: number; y: number; width: number; height: number },
  css: Record<string, string> = {},
  attrs: Record<string, string> = {},
  parent: Element = document.body,
): HTMLElement {
  const el = document.createElement('div')
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v)
  el.getBoundingClientRect = () =>
    ({ ...r, top: r.y, left: r.x, right: r.x + r.width, bottom: r.y + r.height, toJSON: () => r }) as DOMRect
  styles.set(el, css)
  parent.appendChild(el)
  return el
}

describe('buildPageLayers', () => {
  it('orders pages back to front and keeps rect, corners and mode', () => {
    const set = buildPageLayers({
      parentWindow: 'main',
      epoch: 'e1',
      generation: 7,
      dpr: 2,
      viewport: VIEW,
      pages: [page('b', 2, 600), page('a', 1, 0, 'still')],
      claims: [],
    })
    expect(set).toEqual({
      parentWindow: 'main',
      epoch: 'e1',
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
      epoch: 'e1',
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
      epoch: 'e1',
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
      epoch: 'e1',
      generation: 1,
      dpr: 1,
      viewport: VIEW,
      pages: [{ ...page('a', 1, 0), rect: null }, { ...page('b', 2, 0), rect: { x: 0, y: 0, width: 0, height: 10 } }],
      claims: [],
    })
    expect(set.layers).toEqual([])
  })
})

describe('app zoom: everything is sent in window px (the UI view points)', () => {
  it('at 125 %, a claim at CSS (100,100,200,50) with radius 16 is sent as (125,125,250,62.5) with radius 20', () => {
    const set: PageLayers = {
      parentWindow: 'main',
      epoch: 'e1',
      generation: 1,
      dpr: 2,
      viewport: [1250, 1000],
      layers: [{ windowId: 'a', z: 1, rect: { x: 0, y: 40, width: 400, height: 600, radius: [0, 0, 16, 16] }, minus: [{ x: 10, y: 10, width: 20, height: 20, chamfer: 4 }], mode: 'live' }],
      claims: [{ id: 1, kind: 'menu', x: 100, y: 100, width: 200, height: 50, radius: 16 }],
    }
    const out = toWindowPx(set, 1.25)
    expect(out.claims).toEqual([{ id: 1, kind: 'menu', x: 125, y: 125, width: 250, height: 62.5, radius: 20 }])
    expect(out.layers[0].rect).toEqual({ x: 0, y: 50, width: 500, height: 750, radius: [0, 0, 20, 20] })
    expect(out.layers[0].minus).toEqual([{ x: 12.5, y: 12.5, width: 25, height: 25, chamfer: 5 }])
    expect(toWindowPx(set, 1)).toBe(set)
  })

  it('an engine that measures in CSS px: rects are scaled up to window px', () => {
    const set = buildPageLayers({
      parentWindow: 'main',
      epoch: 'e1',
      generation: 1,
      dpr: 2,
      viewport: [1250, 1000],
      zoom: 1.25,
      measuredScale: 1,
      pages: [{ windowId: 'a', order: 1, rect: { x: 0, y: 40, width: 400, height: 600, radius: 16 }, mode: 'live', element: null }],
      claims: [claim(1, 'menu', { x: 100, y: 100, width: 200, height: 50 }), claim(2, 'modal', 'fullscreen')],
    })
    expect(set.viewport).toEqual([1250, 1000])
    expect(set.layers[0].rect).toEqual({ x: 0, y: 50, width: 500, height: 750, radius: 20 })
    expect(set.claims).toEqual([
      { id: 1, kind: 'menu', x: 125, y: 125, width: 250, height: 62.5 },
      { id: 2, kind: 'modal', x: 0, y: 0, width: 1250, height: 1000 },
    ])
  })

  it("K2's engines (getBoundingClientRect already in window px): rects pass through, corners scale", () => {
    const set = buildPageLayers({
      parentWindow: 'main',
      epoch: 'e1',
      generation: 1,
      dpr: 2,
      viewport: [1250, 1000],
      zoom: 1.25,
      pages: [{ windowId: 'a', order: 1, rect: { x: 0, y: 50, width: 500, height: 750, radius: 16 }, mode: 'live', element: null }],
      claims: [claim(1, 'menu', { x: 125, y: 125, width: 250, height: 62.5 }), claim(2, 'modal', 'fullscreen')],
    })
    expect(set.layers[0].rect).toEqual({ x: 0, y: 50, width: 500, height: 750, radius: 20 })
    expect(set.claims).toEqual([
      { id: 1, kind: 'menu', x: 125, y: 125, width: 250, height: 62.5 },
      { id: 2, kind: 'modal', x: 0, y: 0, width: 1250, height: 1000 },
    ])
  })
})

describe('device-pixel snap after the zoom', () => {
  it('at 110 % page edges land on the 2x device grid in window px', () => {
    const onGrid = (v: number): boolean => Math.abs(v * 2 - Math.round(v * 2)) < 1e-9
    for (const measuredScale of [1, 1.1]) {
      const set = buildPageLayers({
        parentWindow: 'main',
        epoch: 'e1',
        generation: 1,
        dpr: 2,
        viewport: [1320, 990],
        zoom: 1.1,
        measuredScale,
        pages: [{ windowId: 'a', order: 1, rect: { x: 100.3, y: 40.7, width: 333.3, height: 500.45, radius: 16 }, mode: 'live', element: null }],
        claims: [],
      })
      const r = set.layers[0].rect
      for (const edge of [r.x, r.y, r.x + r.width, r.y + r.height]) expect(onGrid(edge), `edge ${edge} (measured ${measuredScale})`).toBe(true)
      expect(r.radius).toBeCloseTo(17.6, 9)
    }
  })
})

describe('claim shapes (read from the claim element at send time)', () => {
  const MENU = { x: 10, y: 50, width: 200, height: 300 }
  const build = (claims: OverlayClaim[], pages: PageEntry[] = [page('a', 1, 0)]) =>
    buildPageLayers({ parentWindow: 'main', epoch: 'e1', generation: 1, dpr: 2, viewport: VIEW, pages, claims })

  it('a Glass menu at 16px gives radius 16', () => {
    document.documentElement.setAttribute('data-style', 'glass')
    mockStyles()
    const el = box(MENU, { ...corners('16px'), 'backdrop-filter': 'none' }, { 'data-material': 'raised', 'data-float': 'menu' })
    expect(build([claim(1, 'menu', MENU, el)]).claims).toEqual([{ id: 1, kind: 'menu', ...MENU, radius: 16 }])
  })

  it('mixed corners give [tl, tr, br, bl]', () => {
    mockStyles()
    const el = box(MENU, corners('12px', '12px', '0px', '4px'))
    expect(build([claim(1, 'popover', MENU, el)]).claims[0].radius).toEqual([12, 12, 0, 4])
  })

  it('percentages and elliptical radii use the horizontal radius against the box; a pill fits its side', () => {
    mockStyles()
    const pct = box(MENU, corners('10%', '8px 20px', '0px', '0px'))
    expect(build([claim(1, 'popover', MENU, pct)]).claims[0].radius).toEqual([20, 8, 0, 0])
    const pill = { x: 10, y: 50, width: 200, height: 40 }
    const p = box(pill, corners('9999px'))
    expect(build([claim(2, 'toast', pill, p)]).claims[0].radius).toBe(20)
  })

  it('a Terminal chamfered menu gives a chamfer, read through its shadow wrapper', () => {
    document.documentElement.setAttribute('data-style', 'terminal')
    mockStyles()
    const wrapper = box(MENU, { 'clip-path': 'none' }, { 'data-k2-chamfer-float': 'menu' })
    box(
      MENU,
      { ...corners('0px'), '--k2-ch-cut': '7px', 'clip-path': 'polygon(0px 0px, 1px 1px)' },
      { 'data-k2-chamfer': 'surface', 'data-material': 'raised', 'data-k2-menu': '' },
      wrapper,
    )
    expect(build([claim(1, 'menu', MENU, wrapper)]).claims).toEqual([{ id: 1, kind: 'menu', ...MENU, chamfer: 7 }])
  })

  it('a chamfered first row cuts only its top corners', () => {
    mockStyles()
    const el = box(MENU, { ...corners('0px'), '--k2-ch-cut': '5px', 'clip-path': 'polygon(0px 0px)' }, { 'data-k2-chamfer-rows': 'first' })
    expect(build([claim(1, 'popover', MENU, el)]).claims[0].chamfer).toEqual([5, 5, 0, 0])
  })

  it('a chamfer attribute the style does not draw (Glass) is not a chamfer', () => {
    document.documentElement.setAttribute('data-style', 'glass')
    mockStyles()
    const el = box(MENU, { ...corners('14px'), '--k2-ch-cut': '7px', 'clip-path': 'none' }, { 'data-k2-chamfer': 'surface' })
    const [c] = build([claim(1, 'menu', MENU, el)]).claims
    expect(c.chamfer).toBeUndefined()
    expect(c.radius).toBe(14)
  })

  it('a frost popover gives frost true', () => {
    document.documentElement.setAttribute('data-style', 'glass')
    mockStyles()
    const el = box(MENU, { ...corners('16px'), 'backdrop-filter': 'none', '-webkit-backdrop-filter': 'blur(24px) saturate(1.6)' }, { 'data-material': 'frost' })
    expect(build([claim(1, 'popover', MENU, el)]).claims).toEqual([{ id: 1, kind: 'popover', ...MENU, radius: 16, frost: true }])
  })

  it('frost follows what is drawn: the frost marker in Terminal, or with no blur, is not frost', () => {
    document.documentElement.setAttribute('data-style', 'terminal')
    mockStyles()
    const el = box(MENU, { ...corners('0px'), 'backdrop-filter': 'none' }, { 'data-material': 'frost' })
    expect(build([claim(1, 'popover', MENU, el)]).claims[0].frost).toBeUndefined()
  })

  it('a fullscreen modal gets no shape', () => {
    mockStyles()
    const el = box({ x: 0, y: 0, width: 1200, height: 800 }, { ...corners('16px'), 'backdrop-filter': 'blur(8px)' })
    expect(build([claim(1, 'modal', 'fullscreen', el)]).claims).toEqual([{ id: 1, kind: 'modal', x: 0, y: 0, width: 1200, height: 800 }])
  })

  it('a claim whose area is not its element\'s box gets no shape', () => {
    mockStyles()
    const el = box({ x: 300, y: 300, width: 50, height: 50 }, corners('16px'))
    expect(build([claim(1, 'popover', MENU, el)]).claims).toEqual([{ id: 1, kind: 'popover', ...MENU }])
  })

  it('a claim clipped at the right edge gets zero right corners', () => {
    mockStyles()
    const r = { x: 1100, y: 50, width: 200, height: 300 }
    const el = box(r, corners('16px'))
    expect(build([claim(1, 'menu', r, el)]).claims[0].radius).toEqual([16, 0, 0, 16])
  })

  it('a nested claim\'s minus carries its shape', () => {
    mockStyles()
    const area = { x: 200, y: 100, width: 600, height: 500 }
    const dialog = box(area, corners('16px'))
    const embedded = document.createElement('div')
    dialog.appendChild(embedded)
    const set = build([claim(9, 'modal', area, dialog)], [page('tab', 1, 0), page('oauth', 2, 300, 'live', embedded)])
    expect(set.claims).toEqual([])
    expect(set.layers.find((l) => l.windowId === 'tab')?.minus).toEqual([{ ...area, radius: 16 }])
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
  function harness(opts: { active?: boolean; failWith?: string; zoom?: number } = {}) {
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
      options: () => ({ hoverPrivateWebkitFallback: false, pageFirstClick: false }),
      viewport: () => VIEW,
      zoom: () => opts.zoom ?? 1,
      measuredScale: () => opts.zoom ?? 1,
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

  it('unchanged claims do not re-read styles; a moved claim reuses its reading', async () => {
    const spy = mockStyles()
    const h = harness()
    h.sync.register(src('a', 0))
    await h.run(h.frames)
    const el = box({ x: 10, y: 50, width: 100, height: 200 }, corners('16px'))
    const menu = claimBrowserOverlay({ kind: 'menu', area: { x: 10, y: 50, width: 100, height: 200 }, element: el })
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.claims[0].radius).toBe(16)
    const reads = spy.mock.calls.length
    expect(reads).toBeGreaterThan(0)
    for (let i = 0; i < 3; i++) {
      h.sync.frame()
      await h.run(h.frames)
    }
    expect(spy.mock.calls.length).toBe(reads)
    el.getBoundingClientRect = () => ({ x: 10, y: 60, width: 100, height: 200, top: 60, left: 10, right: 110, bottom: 260 }) as DOMRect
    menu.update({ x: 10, y: 60, width: 100, height: 200 })
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.claims[0]).toMatchObject({ y: 60, radius: 16 })
    expect(spy.mock.calls.length).toBe(reads)
    // A new claim on a new element is read.
    const el2 = box({ x: 500, y: 50, width: 100, height: 100 }, corners('8px'))
    claimBrowserOverlay({ kind: 'popover', area: { x: 500, y: 50, width: 100, height: 100 }, element: el2 })
    await h.run(h.tasks)
    expect(h.sent.at(-1)?.claims[1].radius).toBe(8)
    expect(spy.mock.calls.length).toBeGreaterThan(reads)
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

  it('at 125 % the pane\'s measured rect goes out as measured and its CSS radius scaled', async () => {
    const h = harness({ zoom: 1.25 })
    h.sync.register({
      windowId: 'a',
      element: () => null,
      measure: () => ({ x: 500, y: 50, width: 500, height: 950, radius: 16 }),
      mode: () => 'live',
    })
    await h.run(h.frames)
    expect(h.sent[0].layers[0].rect).toEqual({ x: 500, y: 50, width: 500, height: 950, radius: 20 })
    expect(h.sent[0].viewport).toEqual(VIEW)
  })

  it('nothing is sent while the underlay is off', async () => {
    const h = harness({ active: false })
    h.sync.register(src('a', 0))
    await h.run(h.frames)
    expect(h.sent).toEqual([])
  })

  it('a reload starts a new epoch whose generations start over (K2 applies them)', async () => {
    const before = harness()
    before.sync.register(src('a', 0))
    for (let i = 0; i < 5; i++) {
      before.sync.frame()
      await before.run(before.frames)
    }
    const old = before.sent.at(-1)!
    expect(old.generation).toBeGreaterThan(1)
    // A new page load: a new module instance, a new sync.
    const after = harness()
    after.sync.register(src('a', 0))
    await after.run(after.frames)
    const fresh = after.sent[0]
    expect(fresh.generation).toBe(1)
    expect(fresh.epoch).not.toBe(old.epoch)
  })

  it('leaving underlay returns the old epoch and starts a new one', async () => {
    const h = harness()
    h.sync.register(src('a', 0))
    await h.run(h.frames)
    const first = h.sent[0].epoch
    const left = h.sync.reset()
    expect(left).toBe(first)
    expect(h.sync.epoch).not.toBe(first)
    h.sync.frame()
    await h.run(h.frames)
    expect(h.sent.at(-1)?.epoch).toBe(h.sync.epoch)
    expect(h.sent.at(-1)?.generation).toBe(1)
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
