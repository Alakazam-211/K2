// @vitest-environment jsdom
// macOS browser underlay (0.46.17): the pane sends its layer in the window's
// PageLayers set instead of bounds, a menu or toast over the page leaves it
// live (a claim, no freeze), a dialog still freezes it (still), and a native
// failure drops the window back to the legacy path.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useWindowFocusStore } from '@/stores/window-focus'
import { useBrowserLayeringStore } from '@/stores/browser-layering'
import { claimBrowserOverlay, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'
import type { PageLayers } from '@/lib/page-layers'
import { BrowserPane } from './BrowserPane'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))

vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => true,
}))

// The pane's drawn corners, when a test sets them (else the real reading).
let look: import('./dockGeometry').DockAppearance | null = null
vi.mock('./dockGeometry', async (orig) => {
  const real = await orig<typeof import('./dockGeometry')>()
  return { ...real, dockAppearance: (el: Element) => look ?? real.dockAppearance(el) }
})

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const DOCK = { x: 400, y: 72, width: 640, height: 480 }

function calls(): [string, unknown][] {
  return vi.mocked(invoke).mock.calls.map((c) => [String(c[0]), c[1]])
}

function since(start: number): [string, unknown][] {
  return calls().slice(start)
}

function sets(from = 0): PageLayers[] {
  return since(from)
    .filter((c) => c[0] === 'browser_set_page_layers')
    .map((c) => (c[1] as { layers: PageLayers }).layers)
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 6; i++) await Promise.resolve()
  })
}

async function frames(n: number): Promise<void> {
  for (let i = 0; i < n; i++) {
    await act(async () => {
      await new Promise<void>((r) => requestAnimationFrame(() => r()))
    })
    await flush()
  }
}

let layersImpl: () => Promise<unknown> = async () => null

beforeEach(() => {
  resetBrowserOverlaysForTest()
  useBrowserLayeringStore.setState({ mode: 'underlay', failed: null })
  useWindowFocusStore.setState({ isFocused: true })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.setState({ page: 'agents' })
  layersImpl = async () => null
  look = null
  vi.mocked(invoke).mockReset()
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'browser_snapshot') return new Uint8Array([0xff, 0xd8, 0xff, 0xd9]).buffer
    if (cmd === 'browser_set_page_layers') return layersImpl()
    return null
  })
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    ...DOCK,
    top: DOCK.y,
    left: DOCK.x,
    right: DOCK.x + DOCK.width,
    bottom: DOCK.y + DOCK.height,
    toJSON() {
      return {}
    },
  } as DOMRect)
  let n = 0
  URL.createObjectURL = vi.fn(() => `blob:underlay-${++n}`)
  URL.revokeObjectURL = vi.fn()
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  resetBrowserOverlaysForTest()
})

async function mount(): Promise<void> {
  renderInPrimaryRoom(<BrowserPane itemId="ul" tabId="tab-ul" paneGroupId="pg-ul" url="https://example.com" />)
  await flush()
  await frames(3)
}

describe('Browser pane in the macOS underlay', () => {
  it('creates below K2 and sends its layer instead of bounds', async () => {
    await mount()
    const create = calls().find((c) => c[0] === 'browser_create')
    expect(create?.[1]).toMatchObject({ itemId: 'ul', parentWindow: 'main', layering: 'underlay' })
    expect(calls().map((c) => c[0])).not.toContain('browser_set_bounds')
    const last = sets().at(-1)
    expect(last).toMatchObject({
      parentWindow: 'main',
      layers: [{ windowId: 'ul', z: 1, rect: { x: DOCK.x, y: DOCK.y, width: DOCK.width, height: DOCK.height }, minus: [], mode: 'live' }],
      claims: [],
    })
    expect(last?.viewport).toEqual([window.innerWidth, window.innerHeight])
    const opts = calls().find((c) => c[0] === 'browser_set_page_layers')?.[1] as { options: unknown }
    expect(opts.options).toEqual({ hoverPrivateWebkitFallback: false, pageFirstClick: false })
    expect(typeof last?.epoch).toBe('string')
  })

  it("the page layer's hole takes the pane's drawn radius at each corner", async () => {
    look = { background: null, cornerRadius: 16, corners: [false, false, true, true], radii: [0, 0, 16, 16] }
    await mount()
    expect(sets().at(-1)?.layers[0].rect.radius).toEqual([0, 0, 16, 16])
    // The hole is the one source of the corners: the page layer gets no
    // radius of its own in underlay (it rounded all four corners).
    const create = calls().find((c) => c[0] === 'browser_create')?.[1] as { appearance: { cornerRadius: number; corners: boolean[] } }
    expect(create.appearance.cornerRadius).toBe(0)
    expect(create.appearance.corners).toEqual([false, false, false, false])
    expect(calls().filter((c) => c[0] === 'browser_set_appearance').map((c) => (c[1] as { appearance: { cornerRadius: number } }).appearance.cornerRadius)).not.toContain(16)
  })

  it('the legacy path keeps the page layer radius (the bottom corners only)', async () => {
    useBrowserLayeringStore.setState({ mode: 'legacy', failed: null })
    look = { background: null, cornerRadius: 16, corners: [false, false, true, true], radii: [0, 0, 16, 16] }
    await mount()
    const create = calls().find((c) => c[0] === 'browser_create')?.[1] as { appearance: { cornerRadius: number; corners: boolean[] }; layering?: string }
    expect(create.layering).toBeUndefined()
    expect(create.appearance.cornerRadius).toBe(16)
    expect(create.appearance.corners).toEqual([false, false, true, true])
  })

  it('a menu over the page leaves it live: a claim, no snapshot and no hide', async () => {
    await mount()
    const start = calls().length
    let menu = { id: 0, release: (): void => {} }
    act(() => {
      menu = claimBrowserOverlay({ kind: 'menu', area: { x: 900, y: 80, width: 200, height: 200 } })
    })
    await flush()
    const names = since(start).map((c) => c[0])
    expect(names).not.toContain('browser_snapshot')
    expect(names).not.toContain('browser_set_visible')
    expect(sets(start).at(-1)?.claims).toEqual([{ id: menu.id, kind: 'menu', x: 900, y: 80, width: 200, height: 200 }])
    expect(sets(start).at(-1)?.layers[0].mode).toBe('live')
    const before = calls().length
    act(() => menu.release())
    await flush()
    expect(sets(before).at(-1)?.claims).toEqual([])
  })

  it('a dialog still freezes the page and makes its layer still', async () => {
    await mount()
    const start = calls().length
    act(() => {
      claimBrowserOverlay({ kind: 'modal', area: 'fullscreen' })
    })
    await flush()
    expect(since(start).map((c) => c[0])).toContain('browser_snapshot')
    expect(sets(start).at(-1)?.layers[0].mode).toBe('still')
  })

  it('a native failure falls back to the legacy path and clears the underlay', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    layersImpl = async () => {
      throw 'underlay: the UI view has no layer to mask'
    }
    await mount()
    expect(useBrowserLayeringStore.getState().failed).toContain('no layer to mask')
    expect(calls()).toContainEqual(['browser_underlay_clear', { parentWindow: 'main', epoch: expect.any(String) }])
    expect(warn).toHaveBeenCalled()
    // The legacy path places the view again.
    await frames(2)
    expect(calls().map((c) => c[0])).toContain('browser_set_bounds')
  })

  it('turning the setting off goes back to the legacy path', async () => {
    await mount()
    act(() => useBrowserLayeringStore.getState().setMode('legacy'))
    await flush()
    await frames(2)
    expect(calls()).toContainEqual(['browser_underlay_clear', { parentWindow: 'main', epoch: expect.any(String) }])
    expect(calls().map((c) => c[0])).toContain('browser_set_bounds')
  })

  it('after a freeze the page comes back through a live set, never a deferred set_bounds', async () => {
    const view = await (async () => {
      renderInPrimaryRoom(<BrowserPane itemId="ul" tabId="tab-ul" paneGroupId="pg-ul" url="https://example.com" />)
      await flush()
      await frames(3)
      return document
    })()
    let dialog = { release: (): void => {} }
    act(() => {
      dialog = claimBrowserOverlay({ kind: 'modal', area: 'fullscreen' })
    })
    await flush()
    const img = view.querySelector('img[data-browser-freeze]') as HTMLImageElement
    expect(img).not.toBeNull()
    act(() => {
      img.dispatchEvent(new Event('load'))
    })
    await frames(2)
    const start = calls().length
    act(() => dialog.release())
    await flush()
    await frames(2)
    const after = since(start).map((c) => c[0])
    expect(after).toContain('browser_set_visible')
    expect(after).not.toContain('browser_set_bounds')
    expect(sets(start).at(-1)?.layers[0].mode).toBe('live')
  })
})
