// @vitest-environment jsdom
//
// The one-use owner link goes to the app's `open_owner_link` command, which
// opens it through a private redirect page; it never goes to plugin-opener's
// openUrl, which would put it on the `open` / `xdg-open` command line.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ invoke: vi.fn(), openUrl: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...a: unknown[]) => h.invoke(...a) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (...a: unknown[]) => h.openUrl(...a) }))

import { openOwnerLink } from './owner-link'
import { invoke as webInvoke } from '@/shims/tauri-core'

const LINK = 'https://chat.example.test/k2/owner-handoff#code=k2sho_TESTSECRET0001'

afterEach(() => {
  h.invoke.mockReset()
  h.openUrl.mockReset()
  vi.restoreAllMocks()
})

describe('openOwnerLink', () => {
  it('hands the link to open_owner_link and never to openUrl', async () => {
    h.invoke.mockResolvedValue(undefined)
    await openOwnerLink(LINK)
    expect(h.invoke.mock.calls).toEqual([['open_owner_link', { link: LINK }]])
    expect(h.openUrl).not.toHaveBeenCalled()
  })

  it('a failed open is the caller’s error', async () => {
    h.invoke.mockRejectedValue('Couldn’t make the page’s folder')
    await expect(openOwnerLink(LINK)).rejects.toBe('Couldn’t make the page’s folder')
    expect(h.openUrl).not.toHaveBeenCalled()
  })
})

describe('the two owner-link callers', () => {
  // Both callers reach the link only through openOwnerLink; neither imports
  // the opener plugin (their behaviour is tested in SewSection.test.tsx and
  // permissionCardWire.test.tsx).
  for (const file of ['components/Settings/sections/SewSection.tsx', 'components/SessionView/permissionCardView.ts']) {
    it(`${file} opens the link with openOwnerLink`, () => {
      const src = readFileSync(resolve(__dirname, '..', file), 'utf8')
      expect(src).toContain("import { openOwnerLink } from '@/lib/owner-link'")
      expect(src).not.toContain('@tauri-apps/plugin-opener')
      expect(src).not.toMatch(/\bopenUrl\(/)
    })
  }
})

describe('the web client', () => {
  it('opens an https link in a new tab without a referrer', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null)
    await webInvoke('open_owner_link', { link: LINK })
    expect(open.mock.calls).toEqual([[LINK, '_blank', 'noopener,noreferrer']])
  })

  it('refuses anything but https', async () => {
    const open = vi.spyOn(window, 'open').mockReturnValue(null)
    await expect(webInvoke('open_owner_link', { link: 'javascript:alert(1)' })).rejects.toThrow('https')
    expect(open).not.toHaveBeenCalled()
  })
})
