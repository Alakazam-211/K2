// @vitest-environment jsdom
//
// The compose bar's slash menu under the app zoom: placed from the button's
// rect (window px) as fixed left/bottom (CSS px). At 150 % it opened right of
// the button and far above it.
import { afterEach, describe, expect, it } from 'vitest'
import { COMPOSE_SLASH_MENU_WIDTH, composeSlashMenuSpot } from './compose-slash-placement'
import { applyAppZoomForTest, resetAppZoomForTest, setWindowSize } from '@/test-utils/app-zoom'

const WIN = { width: 1000, height: 800 }

function button(z: number, left: number): { top: number; bottom: number; left: number; right: number; width: number; height: number } {
  // A 24 CSS px button, 40 CSS px above the window's bottom, `left` window px in.
  const top = WIN.height - (40 + 24) * z
  return { top, bottom: top + 24 * z, left, right: left + 24 * z, width: 24 * z, height: 24 * z }
}

afterEach(() => {
  resetAppZoomForTest()
  setWindowSize(1024, 768)
})

describe('composeSlashMenuSpot', () => {
  for (const z of [1, 1.25, 1.5]) {
    it(`at ${z * 100}%: opens 4 CSS px above the button, left edges lined up`, () => {
      setWindowSize(WIN.width, WIN.height)
      applyAppZoomForTest(z)
      const b = button(z, 300)
      const spot = composeSlashMenuSpot(b)
      // On screen: the menu's bottom edge is the window's height less bottom × z.
      expect(WIN.height - spot.bottom * z).toBeCloseTo(b.top - 4 * z, 5)
      expect(spot.left * z).toBeCloseTo(300, 5)
    })

    it(`at ${z * 100}%: a button near the right edge keeps the menu inside the window`, () => {
      setWindowSize(WIN.width, WIN.height)
      applyAppZoomForTest(z)
      const spot = composeSlashMenuSpot(button(z, WIN.width - 30))
      expect((spot.left + COMPOSE_SLASH_MENU_WIDTH) * z).toBeCloseTo(WIN.width - 4 * z, 5)
    })
  }
})
