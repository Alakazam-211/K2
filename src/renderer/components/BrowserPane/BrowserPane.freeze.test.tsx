// @vitest-environment jsdom
// Fail loud: only an overlay over THIS dock covers the page, and a covered
// page shows its picture before the native view hides.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useWindowFocusStore } from '@/stores/window-focus'
import { claimBrowserOverlay, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'
import { BrowserPane } from './BrowserPane'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))

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
  }
}

let snapshotImpl: () => Promise<unknown> = async () => new Uint8Array([0xff, 0xd8, 0xff, 0xd9]).buffer

beforeEach(() => {
  resetBrowserOverlaysForTest()
  useWindowFocusStore.setState({ isFocused: true })
  useSettingsStore.setState({ settingsOpen: false })
  usePageViewStore.setState({ page: 'agents' })
  snapshotImpl = async () => new Uint8Array([0xff, 0xd8, 0xff, 0xd9]).buffer
  vi.mocked(invoke).mockReset()
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'browser_snapshot') return snapshotImpl()
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
  URL.createObjectURL = vi.fn(() => `blob:freeze-${++n}`)
  URL.revokeObjectURL = vi.fn()
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  resetBrowserOverlaysForTest()
})

async function mountLive(): Promise<ReturnType<typeof renderInPrimaryRoom>> {
  const view = renderInPrimaryRoom(
    <BrowserPane itemId="fz" tabId="tab-fz" paneGroupId="pg-fz" url="https://example.com" />,
  )
  await flush()
  expect(calls().map((c) => c[0])).toContain('browser_create')
  // Mount's own bounds push (rAF → rAF) lands before a test counts calls;
  // on a slow machine it otherwise arrived inside the measured window.
  await frames(2)
  expect(calls().map((c) => c[0])).toContain('browser_set_bounds')
  return view
}

describe('Browser pane under overlays', () => {
  it('a menu outside the dock leaves the page live: no snapshot, no hide', async () => {
    await mountLive()
    const start = calls().length
    let menu = { release: (): void => {} }
    act(() => {
      menu = claimBrowserOverlay({ kind: 'menu', area: { x: 10, y: 40, width: 300, height: 400 } })
    })
    await flush()
    expect(since(start).map((c) => c[0])).toEqual([])
    act(() => menu.release())
  })

  it('a menu over the dock: snapshot, picture in the dock, then hide; close shows and drops it', async () => {
    const view = await mountLive()
    const start = calls().length
    let menu = { release: (): void => {} }
    act(() => {
      menu = claimBrowserOverlay({ kind: 'menu', area: { x: 900, y: 80, width: 200, height: 200 } })
    })
    await flush()
    expect(since(start)).toEqual([['browser_snapshot', { itemId: 'fz', parentWindow: 'main' }]])

    const img = view.container.querySelector('img[data-browser-freeze]') as HTMLImageElement | null
    expect(img).not.toBeNull()
    expect(img?.getAttribute('src')).toBe('blob:freeze-1')
    expect(img?.style.width).toBe(`${DOCK.width}px`)
    expect(img?.style.height).toBe(`${DOCK.height}px`)
    // Not hidden until the picture has painted.
    expect(since(start).map((c) => c[0])).not.toContain('browser_set_visible')
    fireEvent.load(img as HTMLImageElement)
    await frames(1)
    await flush()
    expect(since(start)).toContainEqual(['browser_set_visible', { itemId: 'fz', visible: false, parentWindow: 'main' }])

    const beforeClose = calls().length
    act(() => menu.release())
    await flush()
    const closed = since(beforeClose)
    expect(closed.map((c) => c[0])).toEqual(['browser_set_bounds', 'browser_set_visible'])
    expect(closed[1]).toEqual(['browser_set_visible', { itemId: 'fz', visible: true, parentWindow: 'main' }])
    // The picture stays a couple of frames under the native view, then goes.
    expect(view.container.querySelector('img[data-browser-freeze]')).not.toBeNull()
    await frames(3)
    expect(view.container.querySelector('img[data-browser-freeze]')).toBeNull()
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:freeze-1')
  })

  it('a snapshot slower than the budget falls back to the blank hide', async () => {
    snapshotImpl = () => new Promise(() => {})
    await mountLive()
    const start = calls().length
    act(() => {
      claimBrowserOverlay({ kind: 'modal', area: 'fullscreen' })
    })
    await flush()
    expect(since(start).map((c) => c[0])).toEqual(['browser_snapshot'])
    await act(async () => {
      await new Promise((r) => setTimeout(r, 200))
    })
    expect(since(start)).toContainEqual(['browser_set_visible', { itemId: 'fz', visible: false, parentWindow: 'main' }])
  })

  it('a failed snapshot hides at once (old behaviour, never blocks)', async () => {
    snapshotImpl = async () => {
      throw new Error('no such browser view')
    }
    await mountLive()
    const start = calls().length
    act(() => {
      claimBrowserOverlay({ kind: 'drag', area: 'fullscreen' })
    })
    await flush()
    expect(since(start).map((c) => c[0])).toEqual(['browser_snapshot', 'browser_set_visible'])
  })

  it('Settings over a covered page hides it without a picture', async () => {
    const view = await mountLive()
    act(() => {
      claimBrowserOverlay({ kind: 'modal', area: 'fullscreen' })
    })
    await flush()
    const img = view.container.querySelector('img[data-browser-freeze]') as HTMLImageElement
    fireEvent.load(img)
    await frames(1)
    await flush()
    act(() => useSettingsStore.setState({ settingsOpen: true }))
    await flush()
    expect(view.container.querySelector('img[data-browser-freeze]')).toBeNull()
  })
})
