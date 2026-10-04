// @vitest-environment jsdom
//
// prd-zen-mode-v1 Z2 (T4.1) — the hosted web client has no Zen: no toggle
// row, no Zen layer, no escape hatch, no app-menu item, and no request to
// `/cli/zen/*` (fetch spy), whatever is stored in `k2.zen.homes.v1`.

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/is-web', () => ({ isWebClient: () => true }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))

import { act } from 'react'
import { cleanup, render } from '@testing-library/react'
import { webFeatures } from '@/web/features'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { useZenHomesStore } from '@/lib/zen/zen-homes'
import { __resetZenAvailableForTests, zenAvailable } from '@/lib/zen/zen-platform'
import { enterZen, toggleZenFromEscape, zenShownNow } from '@/lib/zen/zen-view'
import { ZenHost } from './ZenHost'
import { ZenToggleRow, ZenToggleRailButton } from '@/components/Home/ZenToggleRow'
import AppMenuPanel from '@/components/TopBar/AppMenuPanel'

afterEach(() => cleanup())

describe('no Zen on the web client', () => {
  it('no toggle, no layer, no menu item, no escape, no /cli/zen request', async () => {
    Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
    __resetZenAvailableForTests()
    const requests: string[] = []
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      requests.push(String(input))
      return new Response('{}', { status: 200 })
    })
    expect(webFeatures.zen).toBe(false)
    expect(zenAvailable()).toBe(false)

    // Even with this Home stored as on in another (desktop) window.
    act(() => {
      usePageViewStore.getState().setPage('home')
      useZenHomesStore.getState().setOn(selectedHome(useHomesStore.getState()).id, true)
    })
    render(
      <>
        <ZenToggleRow />
        <ZenToggleRailButton />
        <ZenHost />
        <AppMenuPanel onClose={() => undefined} />
      </>,
    )
    expect(document.querySelector('[data-zen-toggle-row]')).toBeNull()
    expect(document.querySelector('[data-zen-enter-rail]')).toBeNull()
    expect(document.querySelector('[data-zen-root]')).toBeNull()
    expect(document.body.textContent).not.toContain('Zen Mode')
    expect(zenShownNow()).toBe(false)

    act(() => {
      enterZen()
      toggleZenFromEscape()
      window.dispatchEvent(new Event('menu:zen-toggle'))
      window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', key: 'z', ctrlKey: true, altKey: true }))
    })
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(document.querySelector('[data-zen-root]')).toBeNull()
    expect(requests.filter((u) => u.includes('/cli/zen'))).toEqual([])
    fetchSpy.mockRestore()
  })
})
