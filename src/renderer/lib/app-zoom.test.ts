// @vitest-environment jsdom
//
// The app zoom (Cmd+= / Cmd+-) is CSS `zoom` on <html>. Measured in both
// WebKit and Chromium: getBoundingClientRect, clientX/Y and
// window.innerWidth/innerHeight are window px (after the zoom), while the
// `left`/`top` of a position:fixed element are CSS px and get multiplied by
// the zoom. A popover placed from a measured rect and clamped to innerWidth
// without converting lands zoom × too far right and down, off the window.
import { afterEach, describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { appZoom, clientRectToCss, cssViewport, clientToCssPx } from './app-zoom'

const W = 1400
const H = 900

function setWindow(w: number, h: number): void {
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: w })
  Object.defineProperty(window, 'innerHeight', { configurable: true, value: h })
}

afterEach(() => {
  delete window.__k2soZoom
})

describe('appZoom', () => {
  it('reads the zoom App.tsx applies, 1 when unset or not a positive number', () => {
    expect(appZoom()).toBe(1)
    window.__k2soZoom = 1.25
    expect(appZoom()).toBe(1.25)
    for (const bad of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
      window.__k2soZoom = bad
      expect(appZoom()).toBe(1)
    }
  })
})

describe('window px to CSS px', () => {
  it('divides a point and a rect by the zoom', () => {
    window.__k2soZoom = 1.5
    expect(clientToCssPx(300)).toBe(200)
    expect(clientRectToCss({ top: 15, bottom: 45, left: 900, right: 930, width: 30, height: 30 })).toEqual({
      top: 10,
      bottom: 30,
      left: 600,
      right: 620,
      width: 20,
      height: 20,
    })
  })

  it('the visible window in CSS px is innerWidth/innerHeight over the zoom', () => {
    setWindow(W, H)
    expect(cssViewport()).toEqual({ width: W, height: H })
    window.__k2soZoom = 1.25
    expect(cssViewport()).toEqual({ width: W / 1.25, height: H / 1.25 })
    window.__k2soZoom = 1.5
    expect(cssViewport()).toEqual({ width: W / 1.5, height: H / 1.5 })
  })
})

// What the top bar opens (Help, Send feedback, the profile, presence, the
// server list, the voice list) is sized to fit the window. `100vw`/`100vh`
// are the window in CSS px, which the zoom draws larger than the window, so
// at 150 % a box of `calc(100vh - 96px)` ran off the top and bottom.
describe('top-bar surfaces size from the zoom-aware window', () => {
  const renderer = join(dirname(fileURLToPath(import.meta.url)), '..')
  const read = (rel: string): string => readFileSync(join(renderer, rel), 'utf8')

  it('globals.css defines --k2-vw / --k2-vh as the window over the app zoom', () => {
    const css = read('globals.css')
    expect(css).toContain('--k2-vw: calc(100vw / var(--k2-app-zoom, 1));')
    expect(css).toContain('--k2-vh: calc(100vh / var(--k2-app-zoom, 1));')
  })

  const SURFACES = [
    'components/Help/HelpExplorer.tsx',
    'components/SendFeedback/SendFeedbackDialog.tsx',
    'components/TopBar/AdminProfileButton.tsx',
    'components/Presence/PresenceModal.tsx',
    'components/TopBar/ServerSwitcher.tsx',
    'components/Voice/VoicePopover.tsx',
  ]
  for (const rel of SURFACES) {
    it(`${rel} uses no raw vw/vh`, () => {
      const raw = read(rel).match(/\b\d+(\.\d+)?(vw|vh|dvh|svh)\b/g)
      expect(raw, `raw viewport units in ${rel}`).toBeNull()
    })
  }
})
