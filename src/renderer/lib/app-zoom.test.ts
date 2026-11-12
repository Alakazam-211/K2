// @vitest-environment jsdom
//
// The app zoom (Cmd+= / Cmd+-) is CSS `zoom` on <html>. Measured in both
// WebKit and Chromium: getBoundingClientRect, clientX/Y and
// window.innerWidth/innerHeight are window px (after the zoom), while the
// `left`/`top` of a position:fixed element are CSS px and get multiplied by
// the zoom. A popover placed from a measured rect and clamped to innerWidth
// without converting lands zoom × too far right and down, off the window.
import { afterEach, describe, expect, it } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
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

// Anything sized to fit the window or placed from a measured rect has to
// use the zoom-aware window. `100vw`/`100vh` are the window in CSS px, which
// the zoom draws larger than the window, so at 150 % a box of
// `calc(100vh - 96px)` ran off the top and bottom; and innerWidth/innerHeight
// are window px, so a fixed box clamped to them could still leave the window.
describe('the renderer sizes and places from the zoom-aware window', () => {
  const renderer = join(dirname(fileURLToPath(import.meta.url)), '..')
  const read = (rel: string): string => readFileSync(join(renderer, rel), 'utf8')

  /** Source files under src/renderer (not tests, vendored or dev pages). */
  function sources(dir = ''): string[] {
    const out: string[] = []
    for (const name of readdirSync(join(renderer, dir))) {
      const rel = dir ? `${dir}/${name}` : name
      if (statSync(join(renderer, rel)).isDirectory()) {
        if (['zen-lib', 'public', 'dev', 'test-utils', 'node_modules'].includes(name)) continue
        out.push(...sources(rel))
      } else if (/\.(tsx?|css)$/.test(name) && !/\.test\./.test(name)) {
        out.push(rel)
      }
    }
    return out
  }

  it('globals.css defines --k2-vw / --k2-vh as the window over the app zoom', () => {
    const css = read('globals.css')
    expect(css).toContain('--k2-vw: calc(100vw / var(--k2-app-zoom, 1));')
    expect(css).toContain('--k2-vh: calc(100vh / var(--k2-app-zoom, 1));')
  })

  it('no raw vw/vh outside the --k2-vw / --k2-vh definitions', () => {
    const found: string[] = []
    for (const rel of sources()) {
      // The definitions, and the note that explains them.
      if (rel === 'globals.css' || rel === 'lib/app-zoom.ts') continue
      for (const m of read(rel).matchAll(/(?<![\w.-])\d+(\.\d+)?(vw|vh|dvh|svh|lvh)\b/g)) found.push(`${rel}: ${m[0]}`)
    }
    expect(found).toEqual([])
  })

  // Each of these reads the window size in window px on purpose.
  const WINDOW_PX = new Map([
    ['lib/app-zoom.ts', 'converts it to CSS px for everyone else'],
    ['components/ContextMenu/ContextMenu.tsx', 'the submenu flip compares it with a measured rect, both window px'],
    ['components/Settings/controls/SettingControls.tsx', 'the clip rect is converted with the trigger rect'],
    ['lib/zen/zen-controls.ts', 'on-screen checks next to elementFromPoint, all window px'],
    ['shims/tauri-window.ts', 'the web stand-in for the native window size'],
  ])

  it('innerWidth/innerHeight only where window px are meant', () => {
    const found: string[] = []
    for (const rel of sources()) {
      if (WINDOW_PX.has(rel)) continue
      if (/\.inner(Width|Height)\b/.test(read(rel))) found.push(rel)
    }
    expect(found).toEqual([])
  })
})
