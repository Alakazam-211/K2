// @vitest-environment jsdom
//
// prd-zen-mode-v1 Z23, Z49 (T4.4) — one owner for the window's native
// chrome. While a window shows Zen, the Zen `[chrome]` values hold through
// every Styles re-apply trigger (resize, fullscreen, zoom, title change, a
// Style stamp / hover preview); leaving Zen restores the Style's values.
// Windows (and Linux) never get a native call.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.hoisted(() => vi.fn(async () => null))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

type ZoomWindow = Window & { __k2soZoom?: number }
type InsetCall = { x: number; y: number; square: boolean; zoom: number }

function setPlatform(platform: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
}

function insetCalls(): InsetCall[] {
  return invoke.mock.calls
    .filter((c) => (c as unknown[])[0] === 'set_traffic_light_inset')
    .map((c) => (c as unknown[])[1] as InsetCall)
}

function radiusCalls(): number[] {
  return invoke.mock.calls
    .filter((c) => (c as unknown[])[0] === 'set_window_corner_radius')
    .map((c) => ((c as unknown[])[1] as { radius: number }).radius)
}

function lastInset(): InsetCall {
  const last = insetCalls().at(-1)
  if (!last) throw new Error('no set_traffic_light_inset call')
  return last
}

function lastRadius(): number {
  const last = radiusCalls().at(-1)
  if (last === undefined) throw new Error('no set_window_corner_radius call')
  return last
}

function spacer(): string {
  return document.documentElement.style.getPropertyValue('--k2-stoplight-spacer')
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()))
}

/** The Style as the Styles store stamps it: a floating Glass window, inset 8. */
function stampGlass(): void {
  document.documentElement.setAttribute('data-style', 'glass')
  document.documentElement.style.setProperty('--inset-window', '8px')
}

const ZEN_SQUARE = { zen: { corners: 'square' as const, stoplights: 'square' as const, offset: [4, 2] as [number, number] } }

// One module for the whole file, as in the app: its window listeners are
// registered once (a re-imported copy would leave the old copy's resize and
// fullscreen listeners behind).
import * as style from './style'

/** Stamp the Style and let its controllers apply it, as the Styles store does. */
function applyStyle(stamp: () => void): void {
  stamp()
  style.reapplyTrafficLights()
  style.reapplyWindowCorners()
}

beforeEach(async () => {
  setPlatform('MacIntel')
  style.setChromeSource('style')
  delete (window as ZoomWindow).__k2soZoom
  document.documentElement.removeAttribute('data-style')
  document.documentElement.style.removeProperty('--inset-window')
  document.documentElement.style.removeProperty('--k2-stoplight-spacer')
  // Let any frame queued by the previous test run first.
  await nextFrame()
  invoke.mockClear()
})

afterEach(() => {
  style.setChromeSource('style')
  setPlatform('')
})

describe('macOS: Zen holds the native chrome, the Style comes back on leave', () => {
  it('Style Glass + Zen square/square: enter, resize, fullscreen, zoom, title change, hover preview, leave', async () => {
    applyStyle(stampGlass)
    // Glass before Zen: round lights at the inset, system corners.
    expect(lastInset()).toMatchObject({ x: 8, y: 11, square: false })
    expect(lastRadius()).toBe(0)
    const glassSpacer = spacer()
    expect(glassSpacer).not.toBe('')
    invoke.mockClear()

    // Enter Zen.
    style.setChromeSource(ZEN_SQUARE)
    expect(lastInset()).toEqual({ x: 4, y: 5, square: true, zoom: 1 })
    expect(lastRadius()).toBe(0.5)
    // The Styles spacer keeps describing the Style.
    expect(spacer()).toBe(glassSpacer)

    // Same source again: nothing re-sent.
    const before = invoke.mock.calls.length
    style.setChromeSource({ zen: { ...ZEN_SQUARE.zen, offset: [4, 2] } })
    expect(invoke.mock.calls.length).toBe(before)

    // Resize while in Zen → the Styles triggers re-apply ZEN values.
    invoke.mockClear()
    window.dispatchEvent(new Event('resize'))
    await nextFrame()
    expect(insetCalls().length).toBeGreaterThan(0)
    expect(radiusCalls().length).toBeGreaterThan(0)
    for (const c of insetCalls()) expect(c).toMatchObject({ x: 4, y: 5, square: true })
    for (const r of radiusCalls()) expect(r).toBe(0.5)

    // Fullscreen enter/exit (both spellings).
    for (const ev of ['fullscreenchange', 'webkitfullscreenchange']) {
      invoke.mockClear()
      document.dispatchEvent(new Event(ev))
      await nextFrame()
      expect(insetCalls().length, ev).toBeGreaterThan(0)
      for (const c of insetCalls()) expect(c, ev).toMatchObject({ x: 4, y: 5, square: true })
      expect(radiusCalls(), ev).toEqual([0.5])
    }

    // App zoom (⌘= / ⌘-): Zen values with the zoom; no Style spacer write.
    invoke.mockClear()
    ;(window as ZoomWindow).__k2soZoom = 1.25
    style.onAppZoomChange()
    expect(lastInset()).toEqual({ x: 4, y: 5, square: true, zoom: 1.25 })
    expect(spacer()).toBe(glassSpacer)

    // setTitle parks the lights: App re-applies both.
    invoke.mockClear()
    style.reapplyTrafficLights()
    style.reapplyWindowCorners()
    expect(lastInset()).toMatchObject({ x: 4, y: 5, square: true })
    expect(lastRadius()).toBe(0.5)

    // A Style stamp (Settings hover preview of Square, or another window's
    // storage sync) while in Zen: Zen still wins.
    invoke.mockClear()
    style.stampStyleAttributes({ styleId: 'bezel', paletteId: 'default', schemeMode: 'dark', gapsPreset: '' })
    for (const c of insetCalls()) expect(c).toMatchObject({ x: 4, y: 5, square: true })
    for (const r of radiusCalls()) expect(r).toBe(0.5)
    stampGlass()

    // Leave Zen → Glass again: round lights at the inset, system corners,
    // spacer for the (zoomed) Style.
    invoke.mockClear()
    style.setChromeSource('style')
    expect(lastInset()).toEqual({ x: 8, y: 11, square: false, zoom: 1.25 })
    expect(lastRadius()).toBe(0)
    expect(spacer()).not.toBe(glassSpacer)

    // And the Styles triggers drive the Style again.
    invoke.mockClear()
    window.dispatchEvent(new Event('resize'))
    await nextFrame()
    for (const c of insetCalls()) expect(c).toMatchObject({ x: 8, y: 11, square: false })
    for (const r of radiusCalls()) expect(r).toBe(0)
    expect(style.getChromeSource()).toBe('style')
  })

  it('Style Square + Zen round/system: Zen turns Square’s square lights and corners off, leaving restores them', async () => {
    applyStyle(() => document.documentElement.setAttribute('data-style', 'square'))
    expect(lastInset()).toMatchObject({ x: 0, y: 3, square: true })
    expect(lastRadius()).toBe(0.5)
    invoke.mockClear()

    style.setChromeSource({ zen: { corners: 'system', stoplights: 'round', offset: [0, 0] } })
    expect(lastInset()).toEqual({ x: 0, y: 3, square: false, zoom: 1 })
    expect(lastRadius()).toBe(0)
    invoke.mockClear()
    window.dispatchEvent(new Event('resize'))
    await nextFrame()
    for (const c of insetCalls()) expect(c.square).toBe(false)
    for (const r of radiusCalls()) expect(r).toBe(0)

    style.setChromeSource('style')
    expect(lastInset()).toMatchObject({ x: 0, y: 3, square: true })
    expect(lastRadius()).toBe(0.5)
  })

  it('every stoplight apply notifies (Zen recomputes its safe area on zoom)', async () => {
    const seen = vi.fn()
    const off = style.onChromeApplied(seen)
    style.setChromeSource(ZEN_SQUARE)
    expect(seen).toHaveBeenCalled()
    seen.mockClear()
    style.onAppZoomChange()
    expect(seen).toHaveBeenCalled()
    off()
    seen.mockClear()
    style.onAppZoomChange()
    expect(seen).not.toHaveBeenCalled()
    style.setChromeSource('style')
  })
})

describe('Windows and Linux are untouched', () => {
  it('no native chrome call on enter, resize, fullscreen, zoom or leave', async () => {
    for (const platform of ['Win32', 'Linux x86_64']) {
      invoke.mockClear()
      setPlatform(platform)
      stampGlass()
      style.setChromeSource(ZEN_SQUARE)
      window.dispatchEvent(new Event('resize'))
      document.dispatchEvent(new Event('fullscreenchange'))
      await nextFrame()
      ;(window as ZoomWindow).__k2soZoom = 1.5
      style.onAppZoomChange()
      style.reapplyTrafficLights()
      style.reapplyWindowCorners()
      style.setChromeSource('style')
      await nextFrame()
      expect(invoke.mock.calls, platform).toEqual([])
      expect(spacer(), platform).toBe('')
      expect(style.getChromeSource(), platform).toBe('style')
    }
  })
})
