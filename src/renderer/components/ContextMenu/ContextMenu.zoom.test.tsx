// @vitest-environment jsdom
//
// The right-click menu under the app zoom: it is stored in CSS px, and its
// clamp compared it with innerWidth/innerHeight, which are window px. At
// 150 % a menu opened near the right or bottom edge was let through and
// drawn past the window.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import ContextMenu from './ContextMenu'
import { useContextMenuStore } from '@/stores/context-menu'
import { applyAppZoomForTest, onScreen, resetAppZoomForTest, setWindowSize } from '@/test-utils/app-zoom'

const WIN = { width: 1000, height: 800 }
/** The menu's CSS size. */
const MENU = { width: 200, height: 160 }

afterEach(() => {
  act(() => useContextMenuStore.getState().close())
  cleanup()
  vi.restoreAllMocks()
  resetAppZoomForTest()
  setWindowSize(1024, 768)
})

function openAt(z: number, client: { x: number; y: number }): HTMLElement {
  setWindowSize(WIN.width, WIN.height)
  applyAppZoomForTest(z)
  // jsdom has no layout: the menu measures MENU drawn at zoom z.
  const real = HTMLElement.prototype.getBoundingClientRect
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    if (!this.hasAttribute('data-context-menu')) return real.call(this)
    const r = { left: 0, top: 0, x: 0, y: 0, width: MENU.width * z, height: MENU.height * z, right: MENU.width * z, bottom: MENU.height * z }
    return { ...r, toJSON: () => r } as DOMRect
  })
  render(<ContextMenu />)
  act(() => {
    void useContextMenuStore.getState().show(client.x, client.y, [{ id: 'a', label: 'Copy' }])
  })
  const menu = document.querySelector('[data-context-menu]')
  if (!(menu instanceof HTMLElement)) throw new Error('no open context menu')
  return menu
}

describe('ContextMenu under the app zoom', () => {
  for (const z of [1, 1.25, 1.5]) {
    it(`at ${z * 100}%: opens at the pointer`, () => {
      const menu = openAt(z, { x: 300, y: 200 })
      const box = onScreen(menu, MENU, z)
      expect(box.left).toBeCloseTo(300, 5)
      expect(box.top).toBeCloseTo(200, 5)
    })

    it(`at ${z * 100}%: near the bottom-right corner it stays inside the window`, () => {
      const menu = openAt(z, { x: WIN.width - 50, y: WIN.height - 40 })
      const box = onScreen(menu, MENU, z)
      expect(box.right).toBeLessThanOrEqual(WIN.width)
      expect(box.bottom).toBeLessThanOrEqual(WIN.height)
      expect(box.left).toBeGreaterThanOrEqual(0)
      expect(box.top).toBeGreaterThanOrEqual(0)
    })
  }
})
