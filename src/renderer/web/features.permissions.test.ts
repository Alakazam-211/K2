// @vitest-environment jsdom
// Settings → Accessibility (`webFeatures.permissions`) exists only in the
// macOS desktop app: on Windows and Linux its checks and System Settings
// links do nothing.

import { afterEach, describe, expect, it, vi } from 'vitest'

const realPlatform = navigator.platform

async function permissionsOn(platform: string): Promise<boolean> {
  Object.defineProperty(window.navigator, 'platform', { value: platform, configurable: true })
  vi.resetModules()
  const { webFeatures } = await import('./features')
  return webFeatures.permissions
}

afterEach(() => {
  Object.defineProperty(window.navigator, 'platform', { value: realPlatform, configurable: true })
  vi.resetModules()
})

describe('Settings → Accessibility by desktop', () => {
  it('shows on macOS only', async () => {
    expect(await permissionsOn('MacIntel')).toBe(true)
    expect(await permissionsOn('Win32')).toBe(false)
    expect(await permissionsOn('Linux x86_64')).toBe(false)
  })
})
