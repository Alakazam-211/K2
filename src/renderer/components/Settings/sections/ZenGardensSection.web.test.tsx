// @vitest-environment jsdom
//
// Settings → Gardens on the hosted web client (prd-zen-gardens-v1 item 17):
// no sidebar item, no section even on a deep link, and no `/cli/zen/*`
// request, exactly like the Zen toggle (zen-web.test.tsx).

import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ routes: [] as string[] }))

vi.mock('@/lib/is-web', () => ({ isWebClient: () => true }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', listen: async () => () => undefined }),
}))
vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(async () => undefined),
  openPath: vi.fn(async () => undefined),
  revealItemInDir: vi.fn(async () => undefined),
}))
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...mod,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
      h.routes.push(route)
      if (route === 'feedback/waiting-count') return { count: 0 }
      return {}
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
      h.routes.push(route)
      return {}
    }),
  }
})

import { act } from 'react'
import { cleanup, render, screen } from '@testing-library/react'
import { webFeatures } from '@/web/features'
import { useSettingsStore } from '@/stores/settings'
import { __resetZenAvailableForTests, zenAvailable } from '@/lib/zen/zen-platform'
import { zenGardensSettingsShown } from '@/lib/zen/zen-settings'
import Settings from '../Settings'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

afterEach(() => cleanup())

describe('no Settings → Gardens on the web client', () => {
  it('no nav item, no section on a deep link, no /cli/zen request', async () => {
    Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
    __resetZenAvailableForTests()
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
      h.routes.push(String(input))
      return new Response('{}', { status: 200 })
    })
    expect(webFeatures.zen).toBe(false)
    expect(zenAvailable()).toBe(false)
    expect(zenGardensSettingsShown()).toBe(false)

    useSettingsStore.setState({ settingsOpen: true, activeSection: 'zen-gardens' })
    render(<Settings />)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })

    const nav = [...document.querySelectorAll('[data-settings-nav]')].map((el) => el.getAttribute('data-settings-nav'))
    expect(nav).toContain('project-groups')
    expect(nav).toContain('context-catalog')
    expect(nav).not.toContain('zen-gardens')
    expect(screen.queryByRole('button', { name: 'Gardens' })).toBeNull()
    expect(document.querySelector('[data-zen-gardens-section]')).toBeNull()
    expect(useSettingsStore.getState().activeSection).toBe('general')
    expect(h.routes.filter((r) => r.includes('zen'))).toEqual([])
    fetchSpy.mockRestore()
  })
})
