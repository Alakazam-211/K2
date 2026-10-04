// @vitest-environment jsdom
// App zoom → macOS stoplights. onAppZoomChange (App.tsx calls it on every
// Cmd+= / Cmd+- / Cmd+0 and at mount) re-sends the zoom to the native
// reposition and resizes the top-bar spacer. Off macOS it does nothing.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.hoisted(() => vi.fn(async () => null))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

type ZoomWindow = Window & { __k2soZoom?: number }

function setPlatform(platform: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
}

function spacerVar(): string {
  return document.documentElement.style.getPropertyValue('--k2-stoplight-spacer')
}

function insetCalls(): Array<Record<string, unknown>> {
  return invoke.mock.calls
    .filter((c) => (c as unknown[])[0] === 'set_traffic_light_inset')
    .map((c) => (c as unknown[])[1] as Record<string, unknown>)
}

beforeEach(() => {
  vi.resetModules()
  invoke.mockClear()
  document.documentElement.style.removeProperty('--k2-stoplight-spacer')
  delete (window as ZoomWindow).__k2soZoom
})

afterEach(() => {
  setPlatform('')
})

describe('onAppZoomChange', () => {
  it('on macOS sends the zoom to the native reposition and sizes the spacer', async () => {
    setPlatform('MacIntel')
    const { onAppZoomChange } = await import('./style')
    // Let the module's startup sync run first.
    await Promise.resolve()
    expect(spacerVar()).toBe('57px')
    invoke.mockClear()

    const cases: Array<[number, number]> = [
      [0.8, 69 / 0.8 - 12],
      [1, 57],
      [1.25, 69 / 1.25 - 12],
      [1.5, 69 / 1.5 - 12],
    ]
    for (const [zoom, spacer] of cases) {
      ;(window as ZoomWindow).__k2soZoom = zoom
      onAppZoomChange()
      const last = insetCalls().at(-1)
      expect(last, `zoom ${zoom}`).toBeDefined()
      expect(last!.zoom).toBe(zoom)
      expect(last!.x).toBe(0)
      expect(last!.y).toBe(3)
      expect(Number.parseFloat(spacerVar())).toBeCloseTo(spacer, 9)
      expect(spacerVar().endsWith('px')).toBe(true)
    }
    expect(insetCalls()).toHaveLength(cases.length)
  })

  it('on Linux and Windows never calls the native command or sets the spacer', async () => {
    for (const platform of ['Linux x86_64', 'Win32']) {
      vi.resetModules()
      invoke.mockClear()
      setPlatform(platform)
      const { onAppZoomChange } = await import('./style')
      await Promise.resolve()
      ;(window as ZoomWindow).__k2soZoom = 1.5
      onAppZoomChange()
      expect(insetCalls(), platform).toEqual([])
      expect(spacerVar(), platform).toBe('')
    }
  })
})
