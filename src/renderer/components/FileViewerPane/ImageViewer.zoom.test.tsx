// @vitest-environment jsdom

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

const loadImage = vi.hoisted(() => vi.fn())
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: (...args: unknown[]) => loadImage(...args),
  revokeObjectUrl: vi.fn(),
}))
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { ImageViewer } from './ImageViewer'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'

const VP_W = 800
const VP_H = 600
const VP_LEFT = 100
const VP_TOP = 50
const PAD = 16

let paneW = VP_W
let paneH = VP_H
let naturalW = 1600
let naturalH = 1200
let resizeCallbacks: Array<() => void> = []

class FakeResizeObserver {
  private readonly cb: () => void
  constructor(cb: () => void) {
    this.cb = cb
    resizeCallbacks.push(cb)
  }
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {
    resizeCallbacks = resizeCallbacks.filter((c) => c !== this.cb)
  }
}

function resizePane(w: number, h: number): void {
  paneW = w
  paneH = h
  act(() => resizeCallbacks.forEach((cb) => cb()))
}

async function open(
  w = 1600,
  h = 800,
): Promise<{ viewport: HTMLElement; rerender: (path: string) => Promise<void> }> {
  naturalW = w
  naturalH = h
  loadImage.mockResolvedValue({ url: 'blob:one', mime: 'image/png', byteLength: 10 })
  const view = renderInPrimaryRoom(<ImageViewer filePath="/w/a.png" />)
  await waitFor(() => expect(document.querySelector('img')).not.toBeNull())
  fireEvent.load(document.querySelector('img') as HTMLImageElement)
  return {
    viewport: screen.getByTestId('image-zoom-viewport'),
    rerender: async (path: string) => {
      loadImage.mockResolvedValue({ url: 'blob:two', mime: 'image/png', byteLength: 10 })
      view.rerender(<ImageViewer filePath={path} />)
      await waitFor(() =>
        expect(document.querySelector('img')?.getAttribute('src')).toBe('blob:two'),
      )
      fireEvent.load(document.querySelector('img') as HTMLImageElement)
    },
  }
}

function current(): { s: number; x: number; y: number } {
  const transform = (document.querySelector('img') as HTMLImageElement).style.transform
  const m = /translate\((-?[\d.]+)px, (-?[\d.]+)px\) scale\(([\d.e-]+)\)/.exec(transform)
  if (!m) throw new Error(`image has no zoom transform: "${transform}"`)
  return { x: Number(m[1]), y: Number(m[2]), s: Number(m[3]) }
}

function percent(): string {
  return screen.getByTestId('image-zoom-percent').textContent ?? ''
}

function fireGesture(
  el: HTMLElement,
  type: string,
  props: { scale?: number; clientX?: number; clientY?: number } = {},
): boolean {
  const e = new Event(type, { bubbles: true, cancelable: true })
  Object.assign(e, props)
  let notPrevented = true
  act(() => {
    notPrevented = el.dispatchEvent(e)
  })
  return notPrevented
}

/** Client coordinates of a point in the viewport. */
function client(x: number, y: number): { clientX: number; clientY: number } {
  return { clientX: VP_LEFT + x, clientY: VP_TOP + y }
}

function expectAnchored(
  before: { s: number; x: number; y: number },
  after: { s: number; x: number; y: number },
  px: number,
  py: number,
): void {
  // The transform is rounded to whole pixels, so allow one.
  const imageX = (px - before.x) / before.s
  const imageY = (py - before.y) / before.s
  expect(Math.abs(after.x + imageX * after.s - px)).toBeLessThanOrEqual(2)
  expect(Math.abs(after.y + imageY * after.s - py)).toBeLessThanOrEqual(2)
}

beforeEach(() => {
  paneW = VP_W
  paneH = VP_H
  resizeCallbacks = []
  loadImage.mockReset()
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(() => paneW)
  vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(() => paneH)
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(
    () =>
      ({
        left: VP_LEFT,
        top: VP_TOP,
        width: paneW,
        height: paneH,
        right: VP_LEFT + paneW,
        bottom: VP_TOP + paneH,
        x: VP_LEFT,
        y: VP_TOP,
        toJSON: () => ({}),
      }) as DOMRect,
  )
  vi.spyOn(HTMLImageElement.prototype, 'naturalWidth', 'get').mockImplementation(() => naturalW)
  vi.spyOn(HTMLImageElement.prototype, 'naturalHeight', 'get').mockImplementation(() => naturalH)
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  delete window.__k2soZoom
})

describe('ImageViewer fit', () => {
  it('fills the width and centres a wide image; never upscales a small one', async () => {
    await open(1600, 800)
    expect(current()).toEqual({ s: 0.48, x: 16, y: 108 })
    expect(percent()).toBe('48%')
    cleanup()

    await open(100, 50)
    expect(current()).toEqual({ s: 1, x: 350, y: 275 })
    expect(percent()).toBe('100%')
  })

  it('keeps a tall image at full width from the top, as the viewer did', async () => {
    await open(400, 2000)
    expect(current()).toEqual({ s: 1, x: 200, y: PAD })
  })

  it('stays hidden until it has its size, then lays out at natural size', async () => {
    naturalW = 1600
    naturalH = 1200
    loadImage.mockResolvedValue({ url: 'blob:one', mime: 'image/png', byteLength: 10 })
    renderInPrimaryRoom(<ImageViewer filePath="/w/a.png" />)
    await waitFor(() => expect(document.querySelector('img')).not.toBeNull())
    const img = document.querySelector('img') as HTMLImageElement
    expect(img.style.visibility).toBe('hidden')

    fireEvent.load(img)
    expect(img.style.visibility).toBe('')
    // Natural size in layout, so a zoomed image is drawn from the full bitmap.
    expect(img.style.width).toBe('1600px')
    expect(img.style.height).toBe('1200px')
    expect(img.style.maxWidth).toBe('none')
  })
})

describe('ImageViewer pinch', () => {
  it('ctrl+wheel zooms about the pointer and stops the window zooming', async () => {
    const { viewport } = await open()
    const before = current()

    const notPrevented = fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })

    const after = current()
    expect(notPrevented).toBe(false)
    expect(after.s).toBeCloseTo(0.48 * Math.exp(0.5), 5)
    expectAnchored(before, after, 300, 300)
  })

  it('ctrl+wheel anchors correctly under app zoom', async () => {
    window.__k2soZoom = 1.5
    const { viewport } = await open()
    const before = current()

    // Post-zoom client coordinates: this is layout point (200, 300).
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, clientX: VP_LEFT + 300, clientY: VP_TOP + 450 })

    expectAnchored(before, current(), 200, 300)
  })

  it('gesture events zoom about their point, from the running scale', async () => {
    const { viewport } = await open()
    const before = current()

    expect(fireGesture(viewport, 'gesturestart', { scale: 1 })).toBe(false)
    expect(fireGesture(viewport, 'gesturechange', { scale: 2, ...client(300, 300) })).toBe(false)
    const doubled = current()
    expect(doubled.s).toBeCloseTo(0.96, 5)
    expectAnchored(before, doubled, 300, 300)

    fireGesture(viewport, 'gesturechange', { scale: 3, ...client(300, 300) })
    expect(current().s).toBeCloseTo(1.44, 5)
    expect(fireGesture(viewport, 'gestureend', { scale: 3 })).toBe(false)
  })

  it('a pinch sending both gesture and ctrl+wheel only counts once', async () => {
    const { viewport } = await open()
    fireGesture(viewport, 'gesturestart', { scale: 1 })
    fireGesture(viewport, 'gesturechange', { scale: 2, ...client(300, 300) })
    const during = current().s

    const notPrevented = fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })

    expect(notPrevented).toBe(false)
    expect(current().s).toBe(during)
    fireGesture(viewport, 'gestureend', { scale: 2 })
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })
    expect(current().s).toBeGreaterThan(during)
  })

  it('clamps at the maximum', async () => {
    const { viewport } = await open()
    for (let i = 0; i < 20; i++) {
      fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -500, ...client(300, 300) })
    }
    expect(current().s).toBe(16)
    expect(percent()).toBe('1600%')
  })

  it('clamps at the minimum: the whole image on screen', async () => {
    const { viewport } = await open(400, 2000)
    for (let i = 0; i < 20; i++) {
      fireEvent.wheel(viewport, { ctrlKey: true, deltaY: 500, ...client(300, 300) })
    }
    const { s, y } = current()
    expect(s).toBeCloseTo((VP_H - PAD * 2) / 2000, 5)
    expect(y).toBe(PAD)
  })
})

describe('ImageViewer pan', () => {
  it('dragging pans, and stops at the image edges', async () => {
    const { viewport } = await open()
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(400, 300) })
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(400, 300) })
    const before = current()
    const scaledW = 1600 * before.s
    const scaledH = 800 * before.s

    fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 300, clientY: 300 })
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 320, clientY: 310 })
    expect(current().x).toBe(before.x + 20)
    expect(current().y).toBe(before.y + 10)

    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 20000, clientY: 20000 })
    expect(current().x).toBe(PAD)
    expect(current().y).toBe(PAD)

    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: -60000, clientY: -60000 })
    expect(current().x).toBe(Math.round(VP_W - scaledW - PAD))
    expect(current().y).toBe(Math.round(VP_H - scaledH - PAD))

    fireEvent.pointerUp(viewport, { pointerId: 1 })
    const dropped = current()
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 300, clientY: 300 })
    expect(current()).toEqual(dropped)
  })

  it('two-finger scroll pans within the same limits and is consumed', async () => {
    const { viewport } = await open()
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(400, 300) })
    const before = current()

    const notPrevented = fireEvent.wheel(viewport, { deltaX: 10, deltaY: 30 })
    expect(notPrevented).toBe(false)
    expect(current().x).toBe(before.x - 10)
    expect(current().y).toBe(before.y - 30)
    expect(current().s).toBe(before.s)

    fireEvent.wheel(viewport, { deltaX: -9000, deltaY: -9000 })
    expect(current().x).toBe(PAD)
    expect(current().y).toBe(PAD)
  })

  it('at fit with nothing off-screen, a drag or scroll does nothing and is not consumed', async () => {
    const { viewport } = await open()
    const before = current()

    fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 300, clientY: 300 })
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 400, clientY: 400 })
    fireEvent.pointerUp(viewport, { pointerId: 1 })
    const notPrevented = fireEvent.wheel(viewport, { deltaY: 30 })

    expect(current()).toEqual(before)
    expect(notPrevented).toBe(true)
  })

  it('leaves the image draggable out at fit, and captures the pointer only to pan', async () => {
    const capture = vi.fn()
    Object.defineProperty(HTMLElement.prototype, 'setPointerCapture', {
      value: capture,
      configurable: true,
      writable: true,
    })
    try {
      const { viewport } = await open()
      const img = document.querySelector('img') as HTMLImageElement

      fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 300, clientY: 300 })
      fireEvent.pointerUp(viewport, { pointerId: 1 })
      expect(capture).not.toHaveBeenCalled()
      expect(img.getAttribute('draggable')).toBeNull()

      fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -500, ...client(300, 300) })
      fireEvent.pointerDown(viewport, { pointerId: 2, button: 0, clientX: 300, clientY: 300 })
      fireEvent.pointerUp(viewport, { pointerId: 2 })
      expect(capture).toHaveBeenCalledTimes(1)
      expect(img.getAttribute('draggable')).toBe('false')
    } finally {
      delete (HTMLElement.prototype as Partial<HTMLElement>).setPointerCapture
    }
  })

  it('a tall image scrolls at fit, like it did before', async () => {
    const { viewport } = await open(400, 2000)
    fireEvent.wheel(viewport, { deltaY: 100 })
    expect(current()).toEqual({ s: 1, x: 200, y: PAD - 100 })
    fireEvent.wheel(viewport, { deltaY: 90000 })
    expect(current().y).toBe(VP_H - 2000 - PAD)
  })
})

describe('ImageViewer reset and double-click', () => {
  it('Reset returns to fit and is off while already there', async () => {
    const { viewport } = await open()
    const reset = screen.getByRole('button', { name: 'Reset' }) as HTMLButtonElement
    expect(reset.disabled).toBe(true)

    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })
    expect(reset.disabled).toBe(false)
    fireEvent.click(reset)

    expect(current()).toEqual({ s: 0.48, x: 16, y: 108 })
    expect(percent()).toBe('48%')
    expect(reset.disabled).toBe(true)
  })

  it('double-click goes to 100% about the pointer, then back to fit', async () => {
    const { viewport } = await open()
    const before = current()

    fireEvent.dblClick(viewport, client(300, 300))
    const full = current()
    expect(full.s).toBe(1)
    expect(percent()).toBe('100%')
    expectAnchored(before, full, 300, 300)

    fireEvent.dblClick(viewport, client(300, 300))
    expect(current()).toEqual({ s: 0.48, x: 16, y: 108 })
  })

  it('double-clicking the readout does not zoom', async () => {
    await open()
    fireEvent.dblClick(screen.getByTestId('image-zoom-percent'))
    expect(current().s).toBe(0.48)
  })

  it('a new image starts at fit', async () => {
    const { viewport, rerender } = await open()
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })
    expect(current().s).not.toBe(0.48)

    await rerender('/w/b.png')

    expect(current()).toEqual({ s: 0.48, x: 16, y: 108 })
  })
})

describe('ImageViewer keyboard', () => {
  function press(
    target: Element,
    init: KeyboardEventInit,
  ): { notPrevented: boolean; reachedWindow: boolean } {
    let reachedWindow = false
    const onWindow = (): void => {
      reachedWindow = true
    }
    window.addEventListener('keydown', onWindow)
    const notPrevented = fireEvent.keyDown(target, init)
    window.removeEventListener('keydown', onWindow)
    return { notPrevented, reachedWindow }
  }

  it('Cmd/Ctrl + = - 0 zoom when focused, and never reach the app-wide zoom', async () => {
    const { viewport } = await open()
    viewport.focus()
    expect(document.activeElement).toBe(viewport)

    const zoomIn = press(viewport, { key: '=', metaKey: true })
    expect(zoomIn).toEqual({ notPrevented: false, reachedWindow: false })
    expect(current().s).toBeCloseTo(0.6, 5)

    const ctrlIn = press(viewport, { key: '=', ctrlKey: true })
    expect(ctrlIn.reachedWindow).toBe(false)
    expect(current().s).toBeCloseTo(0.75, 5)

    const zoomOut = press(viewport, { key: '-', metaKey: true })
    expect(zoomOut).toEqual({ notPrevented: false, reachedWindow: false })
    expect(current().s).toBeCloseTo(0.6, 5)

    const fit = press(viewport, { key: '0', metaKey: true })
    expect(fit).toEqual({ notPrevented: false, reachedWindow: false })
    expect(current()).toEqual({ s: 0.48, x: 16, y: 108 })
  })

  it('leaves the keys to the app when focus is elsewhere', async () => {
    await open()
    const elsewhere = document.createElement('input')
    document.body.appendChild(elsewhere)
    elsewhere.focus()

    const result = press(document.activeElement as Element, { key: '=', metaKey: true })

    expect(result).toEqual({ notPrevented: true, reachedWindow: true })
    expect(current().s).toBe(0.48)
    elsewhere.remove()
  })

  it('leaves Cmd+Shift+= (terminal font size) alone', async () => {
    const { viewport } = await open()
    viewport.focus()

    const result = press(viewport, { key: '+', metaKey: true, shiftKey: true })

    expect(result).toEqual({ notPrevented: true, reachedWindow: true })
    expect(current().s).toBe(0.48)
  })

  it('stays reachable from the Reset button inside the viewer', async () => {
    const { viewport } = await open()
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })
    const reset = screen.getByRole('button', { name: 'Reset' })
    reset.focus()

    expect(press(reset, { key: '-', metaKey: true }).reachedWindow).toBe(false)
  })

  it('App leaves a click on the image focused on the viewer', async () => {
    const { viewport } = await open()
    expect(viewport.tabIndex).toBe(0)
    expect((document.querySelector('img') as HTMLImageElement).closest('[data-k2-keeps-focus]')).toBe(viewport)
    const app = readFileSync(resolve(__dirname, '../../App.tsx'), 'utf8')
    expect(app).toMatch(/if \(target\.closest\('\[data-k2-keeps-focus\]'\)\) return/)
  })
})

describe('ImageViewer pane size', () => {
  it('keeps the zoom while the tab is parked at display:none', async () => {
    const { viewport } = await open()
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -50, ...client(300, 300) })
    const zoomed = current()

    resizePane(0, 0)
    expect(current()).toEqual(zoomed)
    resizePane(VP_W, VP_H)
    expect(current()).toEqual(zoomed)
  })

  it('fits when an image loaded in a parked tab is first shown', async () => {
    paneW = 0
    paneH = 0
    await open()
    const img = document.querySelector('img') as HTMLImageElement
    expect(img.style.visibility).toBe('hidden')

    resizePane(VP_W, VP_H)

    expect(img.style.visibility).toBe('')
    expect(current()).toEqual({ s: 0.48, x: 16, y: 108 })
  })

  it('refits on resize at fit, and re-clamps the pan when zoomed in', async () => {
    const { viewport } = await open()
    resizePane(400, 300)
    expect(current().s).toBeCloseTo((400 - PAD * 2) / 1600, 5)

    resizePane(VP_W, VP_H)
    fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -500, ...client(400, 300) })
    fireEvent.wheel(viewport, { deltaX: -9000, deltaY: -9000 })
    expect(current().x).toBe(PAD)

    resizePane(VP_W + 100, VP_H)
    const { s, x } = current()
    expect(s).toBeGreaterThan(0.48)
    expect(x).toBe(PAD)
  })
})
