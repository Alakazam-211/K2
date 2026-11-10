// @vitest-environment jsdom
//
// Owner 2026-11-09: no K2 logo on macOS desktop, where the system menu bar
// has the app menu. Windows and Linux keep it (it is their Menu button);
// hosted web keeps it (a browser tab has no app menu).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render } from '@testing-library/react'

const h = vi.hoisted(() => ({ web: false }))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => {}) }))
vi.mock('@/lib/is-web', () => ({ isWebClient: () => h.web }))
vi.mock('./AppMenuPanel', () => ({ default: () => null }))

import K2MarkButton from './K2MarkButton'

const NAVS = {
  mac: { platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' },
  windows: { platform: 'Win32', userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' },
  linux: { platform: 'Linux x86_64', userAgent: 'Mozilla/5.0 (X11; Linux x86_64)' },
} as const

function asOs(os: keyof typeof NAVS): void {
  Object.defineProperty(navigator, 'platform', { value: NAVS[os].platform, configurable: true })
  Object.defineProperty(navigator, 'userAgent', { value: NAVS[os].userAgent, configurable: true })
}

afterEach(() => {
  cleanup()
  h.web = false
})

describe('K2MarkButton by platform', () => {
  it('draws nothing on macOS desktop', () => {
    asOs('mac')
    const { container } = render(<K2MarkButton />)
    expect(container.innerHTML).toBe('')
  })

  it('keeps the logo as the Menu button on Linux and Windows', () => {
    for (const os of ['linux', 'windows'] as const) {
      asOs(os)
      const { getByRole, unmount } = render(<K2MarkButton />)
      const btn = getByRole('button', { name: 'Menu' })
      expect(btn.getAttribute('aria-haspopup'), os).toBe('menu')
      expect(btn.querySelector('img'), os).not.toBeNull()
      unmount()
    }
  })

  it('keeps the dashboard logo on hosted web, Mac browsers included', () => {
    h.web = true
    asOs('mac')
    const { getByRole } = render(<K2MarkButton />)
    expect(getByRole('button', { name: 'Open the K2 dashboard' })).toBeTruthy()
  })
})
