// @vitest-environment jsdom
//
// Zen under the app zoom (CSS zoom on <html>): the Add agent picker and the
// window-control cluster's safe area are CSS px, measured from window-px
// rects. At 150 % the picker opened right of and below its button, and the
// safe area was 1.5× too wide.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
// The native min/max/close buttons need a Tauri window; only the box counts.
vi.mock('@/components/TopBar/WindowControls', () => ({ default: () => null }))

import { ZEN_ADD_AGENT_WIDTH_PX, zenAddAgentSpot } from './ZenAddAgent'
import { ZenChromeCluster } from './ZenChromeCluster'
import type { ZenRect } from '@/lib/zen/zen-controls'
import { applyAppZoomForTest, resetAppZoomForTest, setScreenRect, setWindowSize } from '@/test-utils/app-zoom'

const Z = 1.5

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  resetAppZoomForTest()
  setWindowSize(1024, 768)
})

describe('Zen Add agent picker spot', () => {
  it('at 150%: sits 6 CSS px above its button, its panel lined up with the button', () => {
    setWindowSize(1200, 800)
    applyAppZoomForTest(Z)
    const anchor = document.createElement('button')
    document.body.appendChild(anchor)
    setScreenRect(anchor, { left: 300, top: 600, width: 30 * Z, height: 30 * Z })
    const spot = zenAddAgentSpot(anchor)
    // The picker's panel is inset 12 CSS px; on screen it starts at the button.
    expect((spot.left + 12) * Z).toBeCloseTo(300, 5)
    expect(spot.top * Z).toBeCloseTo(600 - 6 * Z, 5)
    anchor.remove()
  })

  it('at 150%: a button near the right edge keeps the picker inside the window', () => {
    setWindowSize(1200, 800)
    applyAppZoomForTest(Z)
    const anchor = document.createElement('button')
    document.body.appendChild(anchor)
    setScreenRect(anchor, { left: 1150, top: 600, width: 30 * Z, height: 30 * Z })
    const spot = zenAddAgentSpot(anchor)
    expect((spot.left + ZEN_ADD_AGENT_WIDTH_PX) * Z).toBeLessThanOrEqual(1200 + 1e-6)
    anchor.remove()
  })
})

describe('Zen window-control cluster', () => {
  it('at 150%: reports its rect in CSS px (the safe-area vars are CSS px)', () => {
    applyAppZoomForTest(Z)
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
      const r = { left: 900, top: 6, width: 90, height: 30, right: 990, bottom: 36, x: 900, y: 6 }
      return { ...r, toJSON: () => r } as DOMRect
    })
    const seen: Array<ZenRect | null> = []
    render(<ZenChromeCluster os="windows" onRect={(r) => seen.push(r)} />)
    const last = seen.filter((r): r is ZenRect => r !== null).at(-1)
    if (!last) throw new Error('the cluster reported no rect')
    expect(last).toEqual({ left: 600, top: 4, width: 60, height: 20 })
  })
})
