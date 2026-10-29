// @vitest-environment jsdom
// The phone frame's geometry and drawing (research-mobile-pane-polish-v1
// §4 item 1), fail loud: the shape per device, fit both ways without
// distorting, proportional corners and buttons, the island / punch-hole,
// landscape, and screen-only.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import {
  ANDROID_PHONE_SHAPE,
  ANDROID_TABLET_SHAPE,
  cutoutBox,
  fitDeviceFrame,
  frameLayout,
  IOS_PHONE_SHAPE,
  IOS_TABLET_SHAPE,
  resolveDeviceShape,
} from './device-frame'
import { DeviceFrame } from './DeviceFrame'

const IPHONE = { widthPt: 393, heightPt: 852 }
const PIXEL = { widthPt: 412, heightPt: 915 }
const IPAD = { widthPt: 820, heightPt: 1180 }

afterEach(() => cleanup())

describe('resolveDeviceShape', () => {
  it('picks by lane, then tablet by name or squareness', () => {
    expect(resolveDeviceShape('ios', 'iPhone 16', IPHONE)).toBe(IOS_PHONE_SHAPE)
    expect(resolveDeviceShape('ios-node', 'iPad Air', IPAD)).toBe(IOS_TABLET_SHAPE)
    expect(resolveDeviceShape('ios', 'Some device', IPAD)).toBe(IOS_TABLET_SHAPE)
    expect(resolveDeviceShape('android', 'Pixel 9', PIXEL)).toBe(ANDROID_PHONE_SHAPE)
    expect(resolveDeviceShape('android', 'Pixel Tablet', { widthPt: 800, heightPt: 1280 })).toBe(ANDROID_TABLET_SHAPE)
  })
})

describe('fitDeviceFrame', () => {
  it('fits by height, then clamps to the width left after the inset', () => {
    expect(fitDeviceFrame(0.5, 1000, 400, 0)).toEqual({ width: 200, height: 400 })
    expect(fitDeviceFrame(0.5, 156, 400, 56)).toEqual({ width: 100, height: 200 })
    expect(fitDeviceFrame(0.5, 0, 400, 0)).toEqual({ width: 0, height: 0 })
  })
})

describe('frameLayout', () => {
  it('keeps the body aspect at any stage and scales corners with it', () => {
    const tall = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 1000, height: 600 })
    const wide = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 200, height: 2000 })
    expect(tall.height).toBe(600)
    expect(wide.width).toBe(200)
    expect(tall.width / tall.height).toBeCloseTo(wide.width / wide.height, 6)
    // Corners are a fraction of the size, not a fixed px.
    expect(tall.bodyRadius / tall.width).toBeCloseTo(wide.bodyRadius / wide.width, 6)
    expect(tall.bezel).toBeCloseTo(IOS_PHONE_SHAPE.bezel * tall.unit, 6)
    // The screen inside the bezel keeps the device's aspect.
    expect((tall.width - 2 * tall.bezel) / (tall.height - 2 * tall.bezel)).toBeCloseTo(393 / 852, 6)
  })

  it('an Android phone has a thinner bezel than an iPhone at the same size', () => {
    const i = frameLayout(IOS_PHONE_SHAPE, PIXEL, { width: 1000, height: 600 })
    const a = frameLayout(ANDROID_PHONE_SHAPE, PIXEL, { width: 1000, height: 600 })
    expect(a.bezel).toBeLessThan(i.bezel)
  })

  it('landscape turns the body and moves the side buttons to the top and bottom', () => {
    const l = frameLayout(IOS_PHONE_SHAPE, { widthPt: 852, heightPt: 393 }, { width: 1000, height: 600 })
    expect(l.landscape).toBe(true)
    expect(l.width).toBeGreaterThan(l.height)
    expect(l.buttons.map((b) => b.edge).sort()).toEqual(['bottom', 'bottom', 'bottom', 'top'])
  })

  it('screen-only has no bezel and no buttons', () => {
    const s = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 1000, height: 600 }, { screenOnly: true })
    expect(s.bezel).toBe(0)
    expect(s.buttons).toEqual([])
    expect(s.width / s.height).toBeCloseTo(393 / 852, 6)
  })

  it('the right inset leaves room for the control rail', () => {
    const f = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 300, height: 2000 }, { rightInset: 56 })
    expect(f.width).toBeCloseTo(244, 6)
  })
})

describe('cutoutBox', () => {
  it('an island is centred at the top; landscape puts it on the left; a tablet has none', () => {
    const p = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 1000, height: 600 })
    const box = { width: p.width - 2 * p.bezel, height: p.height - 2 * p.bezel }
    const c = cutoutBox(IOS_PHONE_SHAPE, p, box)
    if (!c) throw new Error('no island')
    expect(c.left + c.width / 2).toBeCloseTo(box.width / 2, 6)
    expect(c.width).toBeGreaterThan(c.height)
    const l = frameLayout(IOS_PHONE_SHAPE, { widthPt: 852, heightPt: 393 }, { width: 1000, height: 600 })
    const lc = cutoutBox(IOS_PHONE_SHAPE, l, { width: 500, height: 230 })
    if (!lc) throw new Error('no landscape island')
    expect(lc.height).toBeGreaterThan(lc.width)
    const pc = cutoutBox(ANDROID_PHONE_SHAPE, p, box)
    if (!pc) throw new Error('no punch-hole')
    expect(pc.width).toBe(pc.height)
    expect(cutoutBox(IOS_TABLET_SHAPE, p, box)).toBeNull()
  })
})

describe('DeviceFrame', () => {
  it('draws the body, the cut-out, the side buttons and the screen content', () => {
    const layout = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 800, height: 600 })
    render(
      <DeviceFrame shape={IOS_PHONE_SHAPE} layout={layout}>
        <canvas data-testid="screen" />
      </DeviceFrame>,
    )
    const root = document.querySelector('[data-mobile-frame]') as HTMLElement
    expect(root.getAttribute('data-mobile-frame')).toBe('ios-phone')
    expect(root.getAttribute('data-mobile-bezel')).toBe('portrait')
    expect(root.style.height).toBe('600px')
    expect(document.querySelector('[data-mobile-cutout]')?.getAttribute('data-mobile-cutout')).toBe('island')
    expect(document.querySelectorAll('[data-mobile-frame-button]').length).toBe(4)
    expect(document.querySelector('[data-mobile-floor]')).not.toBeNull()
    expect(document.querySelector('[data-mobile-frame-screen] [data-testid="screen"]')).not.toBeNull()
  })

  it('turning landscape animates the frame from the old pose (not a snap)', () => {
    const animate = vi.fn()
    const had = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'animate')
    Object.defineProperty(HTMLElement.prototype, 'animate', { configurable: true, value: animate })
    try {
      const portrait = frameLayout(IOS_PHONE_SHAPE, IPHONE, { width: 800, height: 600 })
      const landscape = frameLayout(IOS_PHONE_SHAPE, { widthPt: 852, heightPt: 393 }, { width: 800, height: 600 })
      const view = render(
        <DeviceFrame shape={IOS_PHONE_SHAPE} layout={portrait}>
          <canvas />
        </DeviceFrame>,
      )
      expect(animate).not.toHaveBeenCalled()
      view.rerender(
        <DeviceFrame shape={IOS_PHONE_SHAPE} layout={landscape}>
          <canvas />
        </DeviceFrame>,
      )
      expect(animate).toHaveBeenCalledTimes(1)
      expect(animate.mock.calls[0][0]).toEqual([{ transform: 'rotate(90deg)' }, { transform: 'rotate(0deg)' }])
      view.rerender(
        <DeviceFrame shape={IOS_PHONE_SHAPE} layout={portrait}>
          <canvas />
        </DeviceFrame>,
      )
      expect(animate.mock.calls[1][0]).toEqual([{ transform: 'rotate(-90deg)' }, { transform: 'rotate(0deg)' }])
    } finally {
      if (had) Object.defineProperty(HTMLElement.prototype, 'animate', had)
      else delete (HTMLElement.prototype as { animate?: unknown }).animate
    }
  })

  it('screen only drops the cut-out, buttons and floor', () => {
    const layout = frameLayout(ANDROID_PHONE_SHAPE, PIXEL, { width: 800, height: 600 }, { screenOnly: true })
    render(
      <DeviceFrame shape={ANDROID_PHONE_SHAPE} layout={layout} screenOnly>
        <canvas />
      </DeviceFrame>,
    )
    expect(document.querySelector('[data-mobile-frame]')?.getAttribute('data-mobile-frame')).toBe('screen-only')
    expect(document.querySelector('[data-mobile-cutout]')).toBeNull()
    expect(document.querySelector('[data-mobile-frame-button]')).toBeNull()
    expect(document.querySelector('[data-mobile-floor]')).toBeNull()
  })
})
