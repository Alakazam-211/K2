// @vitest-environment jsdom
//
// Zoom on a video tile, as a viewer uses it: the overlay's buttons, the keys,
// ⌘/Ctrl + scroll and pinch (applied once per animation frame), drag,
// double-click, and the resets (the share ending, another track). Animation
// frames are queued and run by the test, never by a clock.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { VideoTile as Tile } from '@/lib/voice/video-labels'
import type { VideoController } from '@/stores/video-core'
import VideoTile from './VideoTile'

const FRAME = { width: 1600, height: 900 }

function share(over: Partial<Tile> = {}): Tile {
  return {
    key: 'TR_S7',
    memberId: '7',
    self: false,
    source: 'screen',
    name: "Ada's screen",
    speaking: false,
    audioMuted: false,
    state: 'live',
    paused: null,
    mirrored: false,
    large: true,
    ...over,
  }
}

/** Animation frames, run when the test says. */
let frames: FrameRequestCallback[] = []
function runFrames(): void {
  const now = frames
  frames = []
  act(() => {
    for (const f of now) f(0)
  })
}

function fakeCtl() {
  const setZoom = vi.fn<(track: string, zoom: number) => void>()
  const ctl = {
    attach: () => () => undefined,
    setHeld: () => undefined,
    setZoom,
  } as unknown as VideoController
  return { ctl, setZoom }
}

/** The tile, with a frame of a known size (jsdom lays nothing out). */
function setup(tile: Tile = share(), zoomable = true) {
  const f = fakeCtl()
  const onParentKey = vi.fn()
  const r = render(
    <div onKeyDown={onParentKey}>
      <VideoTile tile={tile} ctl={f.ctl} fill zoomable={zoomable} />
    </div>,
  )
  const el = screen.getByTestId('video-tile')
  Object.defineProperty(el, 'clientWidth', { configurable: true, value: FRAME.width })
  Object.defineProperty(el, 'clientHeight', { configurable: true, value: FRAME.height })
  const rerender = (next: Tile): void =>
    r.rerender(
      <div onKeyDown={onParentKey}>
        <VideoTile tile={next} ctl={f.ctl} fill zoomable={zoomable} />
      </div>,
    )
  return { ...f, el, onParentKey, rerender }
}

/** The zoom layer's transform, read back as numbers. Fit: null. */
function view(): { x: number; y: number; scale: number } | null {
  const t = screen.getByTestId('video-zoom-layer').style.transform
  if (!t) return null
  const m = /translate\((-?[\d.e-]+)px, (-?[\d.e-]+)px\) scale\(([\d.e-]+)\)/.exec(t)
  if (!m) throw new Error(`unexpected transform: ${t}`)
  return { x: Number(m[1]), y: Number(m[2]), scale: Number(m[3]) }
}

function percent(): string | null {
  return screen.queryByTestId('video-zoom-percent')?.textContent ?? null
}

beforeEach(() => {
  frames = []
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    frames.push(cb)
    return frames.length
  })
  vi.stubGlobal('cancelAnimationFrame', () => undefined)
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('the overlay on a share', () => {
  it('shows − and + (− off at fit), no % and no Fit until zoomed; the tile is a labelled keyboard stop', () => {
    const s = setup()
    const controls = screen.getByTestId('video-zoom-controls')
    expect(controls.getAttribute('aria-label')).toBe("Zoom on Ada's screen")
    expect(within(controls).getByRole('button', { name: 'Zoom out (−)' })).toHaveProperty('disabled', true)
    expect(within(controls).getByRole('button', { name: 'Zoom in (+)' })).toHaveProperty('disabled', false)
    expect(percent()).toBeNull()
    expect(screen.queryByTestId('video-zoom-fit')).toBeNull()
    expect(view()).toBeNull()
    expect(s.el.tabIndex).toBe(0)
    expect(s.el.getAttribute('role')).toBe('group')
    expect(s.el.getAttribute('aria-label')).toContain("Ada's screen")
  })

  it("the zoom's buttons show keyboard focus in the look: round in Glass, the chip's cut in Terminal", () => {
    setup()
    for (const b of Array.from(screen.getByTestId('video-zoom-controls').querySelectorAll('button'))) {
      expect(b.className).toMatch(/(^|\s)k2-focus(\s|$)/)
      expect(b.className).toContain('rounded-[var(--radius-pill)]')
      expect(b.getAttribute('data-k2-chamfer')).toBe('chip')
      // The cut draws no edge of its own (the chip has none).
      expect(b.style.getPropertyValue('--k2-ch-w')).toBe('0px')
    }
  })

  it('+ zooms about the middle and shows how far; Fit goes back; the sink asks for a sharper picture meanwhile', () => {
    const s = setup()
    fireEvent.click(screen.getByTestId('video-zoom-in'))
    expect(percent()).toBe('150%')
    // About the middle: the middle stays put.
    expect(view()).toEqual({ x: -400, y: -225, scale: 1.5 })
    expect(s.el.dataset.zoomed).toBe('true')
    expect(s.setZoom).toHaveBeenLastCalledWith('TR_S7', 2)
    fireEvent.click(screen.getByTestId('video-zoom-in'))
    expect(percent()).toBe('225%')
    expect(s.setZoom).toHaveBeenLastCalledWith('TR_S7', 4)
    fireEvent.click(screen.getByTestId('video-zoom-out'))
    expect(percent()).toBe('150%')
    fireEvent.click(screen.getByTestId('video-zoom-fit'))
    expect(percent()).toBeNull()
    expect(view()).toBeNull()
    expect(screen.queryByTestId('video-zoom-fit')).toBeNull()
    expect(s.setZoom).toHaveBeenLastCalledWith('TR_S7', 1)
  })

  it('+ is off at 6×', () => {
    setup()
    for (let i = 0; i < 6; i++) fireEvent.click(screen.getByTestId('video-zoom-in'))
    expect(percent()).toBe('600%')
    expect(screen.getByTestId('video-zoom-in')).toHaveProperty('disabled', true)
  })

  it('a tile that is not zoomable (or has no picture) has no overlay and is no keyboard stop', () => {
    setup(share(), false)
    expect(screen.queryByTestId('video-zoom-controls')).toBeNull()
    expect(screen.getByTestId('video-tile').hasAttribute('tabindex')).toBe(false)
    cleanup()
    setup(share({ state: 'paused', paused: 'congestion' }))
    expect(screen.queryByTestId('video-zoom-controls')).toBeNull()
  })
})

describe('the keyboard', () => {
  it('+ and − zoom, the arrows move a zoomed picture (held inside the frame), 0 fits', () => {
    const s = setup()
    s.el.focus()
    fireEvent.keyDown(s.el, { key: '+' })
    fireEvent.keyDown(s.el, { key: '=' })
    expect(percent()).toBe('225%')
    const before = view()
    fireEvent.keyDown(s.el, { key: 'ArrowRight' })
    expect(view()?.x).toBe((before?.x ?? 0) - 160)
    fireEvent.keyDown(s.el, { key: 'ArrowDown', shiftKey: true })
    expect(view()?.y).toBe((before?.y ?? 0) - 360)
    // Far past the bottom right: held at the picture's edge.
    for (let i = 0; i < 20; i++) fireEvent.keyDown(s.el, { key: 'ArrowRight', shiftKey: true })
    expect(view()?.x).toBe(FRAME.width - FRAME.width * 2.25)
    fireEvent.keyDown(s.el, { key: '-' })
    expect(percent()).toBe('150%')
    fireEvent.keyDown(s.el, { key: '0' })
    expect(view()).toBeNull()
  })

  it('arrows at fit do nothing and are left to the page', () => {
    const s = setup()
    fireEvent.keyDown(s.el, { key: 'ArrowRight' })
    expect(view()).toBeNull()
    expect(s.onParentKey).toHaveBeenCalledTimes(1)
  })

  it('Esc while zoomed goes back to fit and stops there; at fit it goes on to the view (Shrink)', () => {
    const s = setup()
    fireEvent.click(screen.getByTestId('video-zoom-in'))
    fireEvent.keyDown(screen.getByTestId('video-zoom-in'), { key: 'Escape' })
    expect(view()).toBeNull()
    expect(s.onParentKey).not.toHaveBeenCalled()
    fireEvent.keyDown(s.el, { key: 'Escape' })
    expect(s.onParentKey).toHaveBeenCalledTimes(1)
  })
})

describe('the pointer', () => {
  it('⌘/Ctrl + scroll zooms about the pointer, once per animation frame', () => {
    const s = setup()
    const at = { clientX: 400, clientY: 300 }
    const ev = new WheelEvent('wheel', { deltaY: -50, ctrlKey: true, cancelable: true, bubbles: true, ...at })
    act(() => {
      s.el.dispatchEvent(ev)
    })
    expect(ev.defaultPrevented).toBe(true)
    fireEvent.wheel(s.el, { deltaY: -50, metaKey: true, ...at })
    // Nothing until the frame.
    expect(view()).toBeNull()
    expect(frames).toHaveLength(1)
    runFrames()
    const v = view()
    expect(v?.scale).toBeCloseTo(Math.E, 6) // e^(100 × 0.01)
    // The point under the pointer is still under it.
    expect((v?.x ?? 0) + 400 * (v?.scale ?? 0)).toBeCloseTo(400, 6)
    expect((v?.y ?? 0) + 300 * (v?.scale ?? 0)).toBeCloseTo(300, 6)
  })

  it('a plain scroll is the page’s at fit, and moves the picture while zoomed', () => {
    const s = setup()
    const atFit = new WheelEvent('wheel', { deltaY: 40, cancelable: true, bubbles: true })
    act(() => {
      s.el.dispatchEvent(atFit)
    })
    expect(atFit.defaultPrevented).toBe(false)
    expect(frames).toHaveLength(0)
    fireEvent.click(screen.getByTestId('video-zoom-in'))
    const before = view()
    const zoomed = new WheelEvent('wheel', { deltaX: 30, deltaY: 40, cancelable: true, bubbles: true })
    act(() => {
      s.el.dispatchEvent(zoomed)
    })
    expect(zoomed.defaultPrevented).toBe(true)
    runFrames()
    expect(view()).toEqual({ x: (before?.x ?? 0) - 30, y: (before?.y ?? 0) - 40, scale: 1.5 })
  })

  it('a pinch (WebKit gesture events) zooms about the pinch from where it started', () => {
    const s = setup()
    const gesture = (type: string, scale: number): Event => {
      const e = new Event(type, { cancelable: true, bubbles: true })
      Object.assign(e, { scale, clientX: 800, clientY: 450 })
      act(() => {
        s.el.dispatchEvent(e)
      })
      return e
    }
    expect(gesture('gesturestart', 1).defaultPrevented).toBe(true)
    gesture('gesturechange', 2)
    // A Ctrl wheel during the pinch is the same pinch: ignored.
    fireEvent.wheel(s.el, { deltaY: -500, ctrlKey: true, clientX: 800, clientY: 450 })
    gesture('gesturechange', 3)
    runFrames()
    expect(view()?.scale).toBe(3)
    gesture('gestureend', 3)
    expect(view()).toEqual({ x: -1600, y: -900, scale: 3 })
  })

  it('double-click: fit → 2× at the click, then back to fit', () => {
    const s = setup()
    fireEvent.doubleClick(s.el, { clientX: 1200, clientY: 300 })
    expect(view()).toEqual({ x: -1200, y: -300, scale: 2 })
    fireEvent.doubleClick(s.el, { clientX: 10, clientY: 10 })
    expect(view()).toBeNull()
  })

  it('a double-click on the overlay is its buttons’, not a zoom', () => {
    setup()
    fireEvent.doubleClick(screen.getByTestId('video-zoom-in'))
    expect(view()).toBeNull()
  })

  it('drag moves a zoomed picture, held inside the frame; at fit it does nothing', () => {
    const s = setup()
    fireEvent.pointerDown(s.el, { pointerId: 1, button: 0, clientX: 500, clientY: 500 })
    fireEvent.pointerMove(s.el, { pointerId: 1, clientX: 400, clientY: 400 })
    fireEvent.pointerUp(s.el, { pointerId: 1 })
    runFrames()
    expect(view()).toBeNull()
    fireEvent.doubleClick(s.el, { clientX: 800, clientY: 450 })
    expect(view()).toEqual({ x: -800, y: -450, scale: 2 })
    fireEvent.pointerDown(s.el, { pointerId: 2, button: 0, clientX: 500, clientY: 500 })
    fireEvent.pointerMove(s.el, { pointerId: 2, clientX: 450, clientY: 480 })
    fireEvent.pointerMove(s.el, { pointerId: 2, clientX: 400, clientY: 460 })
    runFrames()
    expect(view()).toEqual({ x: -900, y: -490, scale: 2 })
    fireEvent.pointerMove(s.el, { pointerId: 2, clientX: -5000, clientY: -5000 })
    runFrames()
    expect(view()).toEqual({ x: -1600, y: -900, scale: 2 })
    fireEvent.pointerUp(s.el, { pointerId: 2 })
    fireEvent.pointerMove(s.el, { pointerId: 2, clientX: 9000, clientY: 9000 })
    runFrames()
    expect(view()).toEqual({ x: -1600, y: -900, scale: 2 })
  })
})

describe('resets', () => {
  it('the share ending goes back to fit, and it stays fit when the same share comes back', () => {
    const s = setup()
    fireEvent.click(screen.getByTestId('video-zoom-in'))
    expect(percent()).toBe('150%')
    s.rerender(share({ state: 'off' }))
    expect(screen.queryByTestId('video-zoom-controls')).toBeNull()
    expect(view()).toBeNull()
    expect(s.setZoom).toHaveBeenLastCalledWith('TR_S7', 1)
    s.rerender(share())
    expect(screen.getByTestId('video-zoom-controls')).toBeTruthy()
    expect(percent()).toBeNull()
    expect(view()).toBeNull()
  })

  it('another track in the tile starts at fit', () => {
    const s = setup()
    fireEvent.doubleClick(s.el, { clientX: 800, clientY: 450 })
    expect(percent()).toBe('200%')
    s.rerender(share({ key: 'TR_S9', memberId: '9', name: "Linus's screen" }))
    expect(percent()).toBeNull()
    expect(view()).toBeNull()
  })

  it('a pause keeps the zoom for when the picture comes back', () => {
    const s = setup()
    fireEvent.doubleClick(s.el, { clientX: 800, clientY: 450 })
    s.rerender(share({ state: 'paused', paused: 'congestion' }))
    expect(view()).toBeNull()
    s.rerender(share())
    expect(percent()).toBe('200%')
  })

  it('a wheel left over from before the share ended never lands on the next one', () => {
    const s = setup()
    fireEvent.wheel(s.el, { deltaY: -100, ctrlKey: true, clientX: 800, clientY: 450 })
    s.rerender(share({ key: 'TR_S9' }))
    runFrames()
    expect(view()).toBeNull()
  })
})
