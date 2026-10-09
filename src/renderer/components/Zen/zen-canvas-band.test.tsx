// @vitest-environment jsdom
//
// Zen 0.45.3: the floating top band of a full-canvas Garden with a widget's
// hit regions (`k2.canvas.setHitRegions`, `lib/zen/zen-canvas.ts`). jsdom
// has no layout, so rects are stubbed and a click is resolved the way the
// browser would from what the band rendered: K2's control groups on top,
// then the band's drag pieces (or the whole band when it has no holes),
// then the frame below.
import { act, cleanup, render } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const win = vi.hoisted(() => ({ startDragging: vi.fn(async () => undefined) }))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    label: 'main',
    startDragging: win.startDragging,
    isMaximized: async () => false,
    maximize: async () => undefined,
    unmaximize: async () => undefined,
  }),
}))

import { ZenTopBand, ZEN_CHROME_CSS, type ZenRowSource } from './ZenBands'
import { createControlRegistry, type ZenRect } from '@/lib/zen/zen-controls'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { __resetZenCanvasForTests, registerZenCanvasFrame, setZenCanvasRegions, ZEN_HIT_REGION_APPLY_MS } from '@/lib/zen/zen-canvas'

const BOXES: Array<[string, ZenRect]> = [
  ['[data-zen-band-row], [data-zen-title-strip]', { left: 0, top: 0, width: 800, height: 52 }],
  ['[data-zen-row-group="start"]', { left: 14, top: 4, width: 120, height: 44 }],
  ['[data-zen-row-group="end"]', { left: 666, top: 4, width: 120, height: 44 }],
  ['[data-zen-row-spacer]', { left: 146, top: 0, width: 508, height: 52 }],
  ['iframe', { left: 0, top: 0, width: 800, height: 600 }],
]

function boxOf(el: Element): ZenRect {
  const hit = BOXES.find(([sel]) => el.matches(sel))
  return hit ? hit[1] : { left: 0, top: 0, width: 0, height: 0 }
}

function inside(r: ZenRect, x: number, y: number): boolean {
  return x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height
}

function pieceBox(el: HTMLElement): ZenRect {
  const n = (v: string): number => Number.parseFloat(v)
  return { left: n(el.style.left), top: n(el.style.top), width: n(el.style.width), height: n(el.style.height) }
}

/** What a click at window (x, y) lands on. The band sits at the window's top-left. */
function landsOn(band: HTMLElement, x: number, y: number): { what: 'chrome' | 'drag' | 'frame'; el: HTMLElement | null } {
  for (const g of Array.from(band.querySelectorAll<HTMLElement>('[data-zen-row-group]'))) {
    if (inside(boxOf(g), x, y)) return { what: 'chrome', el: g.querySelector('button') }
  }
  if (!inside(boxOf(band), x, y)) return { what: 'frame', el: null }
  if (band.style.pointerEvents !== 'none') return { what: 'drag', el: band }
  for (const p of Array.from(band.querySelectorAll<HTMLElement>('[data-zen-drag-piece]'))) {
    if (inside(pieceBox(p), x, y)) return { what: 'drag', el: p }
  }
  return { what: 'frame', el: null }
}

function source(): ZenRowSource {
  const registry = createControlRegistry({ exit: () => undefined, selectGarden: () => undefined, gardenIds: () => [] })
  const decl = (id: string): ZenWidgetDecl => ({ id, kind: 'nav-rail', column: 0, slot: 'top', props: {} }) as ZenWidgetDecl
  return {
    widget: (id) => (id === 'menu' || id === 'toggle' ? decl(id) : null),
    drawWidget: (w) => (
      <button key={w.id} type="button" data-testid={w.id}>
        {w.id}
      </button>
    ),
    item: () => null,
    menuItems: () => [],
    bridge: { controls: registry } as unknown as ZenWidgetBridge,
  }
}

const GROUPS = { start: ['menu'], center: [], end: ['toggle'] }
const KEY = 'g-test0001/diary'

function frame(): HTMLIFrameElement {
  const el = document.createElement('iframe')
  document.body.appendChild(el)
  return el
}

async function settle(): Promise<void> {
  await act(async () => {
    vi.advanceTimersByTime(ZEN_HIT_REGION_APPLY_MS + 1)
  })
}

beforeEach(() => {
  vi.useFakeTimers()
  __resetZenCanvasForTests()
  win.startDragging.mockClear()
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const r = boxOf(this)
    return { ...r, x: r.left, y: r.top, right: r.left + r.width, bottom: r.top + r.height, toJSON: () => r } as DOMRect
  })
})

afterEach(() => {
  cleanup()
  __resetZenCanvasForTests()
  document.body.innerHTML = ''
  vi.restoreAllMocks()
  vi.useRealTimers()
})

function band(container: HTMLElement): HTMLElement {
  const el = container.querySelector<HTMLElement>('[data-zen-band-row="top"]')
  if (!el) throw new Error('no top band')
  return el
}

describe('a full-canvas band with a widget’s hit regions', () => {
  it('a click in a region reaches the frame; outside it the strip still drags the window', async () => {
    const { container } = render(<ZenTopBand groups={GROUPS} src={source()} float />)
    registerZenCanvasFrame(KEY, frame())
    await settle()
    expect(band(container).hasAttribute('data-zen-band-holes')).toBe(false)
    expect(landsOn(band(container), 300, 26).what).toBe('drag')

    act(() => setZenCanvasRegions(KEY, [{ left: 260, top: 8, width: 120, height: 36 }]))
    await settle()
    const b = band(container)
    expect(b.hasAttribute('data-zen-band-holes')).toBe(true)
    expect(b.style.pointerEvents).toBe('none')
    expect(landsOn(b, 300, 26).what).toBe('frame')
    const outside = landsOn(b, 500, 26)
    expect(outside.what).toBe('drag')

    // That piece really drags: its mousedown reaches the band's drag-region binding.
    outside.el!.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, button: 0, detail: 1, clientX: 500, clientY: 26 }))
    await act(async () => {
      vi.advanceTimersByTime(600)
    })
    expect(win.startDragging).toHaveBeenCalledTimes(1)
  })

  it('K2’s controls win: a region over them changes nothing there, and the CSS keeps their groups clickable', async () => {
    const { container, getByTestId } = render(<ZenTopBand groups={GROUPS} src={source()} float />)
    registerZenCanvasFrame(KEY, frame())
    act(() => setZenCanvasRegions(KEY, [{ left: 0, top: 0, width: 400, height: 52 }]))
    await settle()
    const b = band(container)
    expect(landsOn(b, 50, 26)).toEqual({ what: 'chrome', el: getByTestId('menu') })
    expect(landsOn(b, 300, 26).what).toBe('frame')
    expect(landsOn(b, 500, 26).what).toBe('drag')
    expect(ZEN_CHROME_CSS).toMatch(/\[data-zen-band-holes\] \[data-zen-row-group\] \{\s*pointer-events: auto;/)
    expect(ZEN_CHROME_CSS).toMatch(/\[data-zen-band-holes\] \[data-zen-row-inner\] \{\s*position: relative;\s*z-index: 1;/)
  })

  it('regions over the whole strip leave the gaps between K2’s groups as drag area', async () => {
    const { container } = render(<ZenTopBand groups={GROUPS} src={source()} float />)
    registerZenCanvasFrame(KEY, frame())
    act(() => setZenCanvasRegions(KEY, [{ left: 0, top: 0, width: 800, height: 600 }]))
    await settle()
    const b = band(container)
    expect(b.hasAttribute('data-zen-drag-kept')).toBe(true)
    expect(landsOn(b, 400, 26).what).toBe('drag')
    expect(landsOn(b, 5, 26).what).toBe('frame')
  })

  it('clearing the regions ([]) gives the whole band back to the drag', async () => {
    const { container } = render(<ZenTopBand groups={GROUPS} src={source()} float />)
    registerZenCanvasFrame(KEY, frame())
    act(() => setZenCanvasRegions(KEY, [{ left: 260, top: 8, width: 120, height: 36 }]))
    await settle()
    expect(band(container).hasAttribute('data-zen-band-holes')).toBe(true)
    act(() => setZenCanvasRegions(KEY, []))
    await settle()
    const b = band(container)
    expect(b.hasAttribute('data-zen-band-holes')).toBe(false)
    expect(b.style.pointerEvents).toBe('')
    expect(b.querySelector('[data-zen-drag-piece]')).toBeNull()
    expect(landsOn(b, 300, 26).what).toBe('drag')
  })

  it('the title strip of a full canvas with no top band takes holes too', async () => {
    const { container } = render(<ZenTopBand groups={null} src={source()} float />)
    registerZenCanvasFrame(KEY, frame())
    act(() => setZenCanvasRegions(KEY, [{ left: 0, top: 0, width: 300, height: 52 }]))
    await settle()
    const strip = container.querySelector<HTMLElement>('[data-zen-title-strip]')!
    expect(strip.style.pointerEvents).toBe('none')
    expect(landsOn(strip, 100, 10).what).toBe('frame')
    expect(landsOn(strip, 500, 10).what).toBe('drag')
  })
})

describe('column Gardens are unchanged', () => {
  it('a band that doesn’t float never reads the regions', async () => {
    const { container } = render(<ZenTopBand groups={GROUPS} src={source()} />)
    registerZenCanvasFrame(KEY, frame())
    act(() => setZenCanvasRegions(KEY, [{ left: 0, top: 0, width: 800, height: 600 }]))
    await settle()
    const b = band(container)
    expect(b.hasAttribute('data-zen-band-holes')).toBe(false)
    expect(b.style.pointerEvents).toBe('')
    expect(b.querySelector('[data-zen-drag-pieces]')).toBeNull()
    expect(landsOn(b, 300, 26).what).toBe('drag')
  })
})
