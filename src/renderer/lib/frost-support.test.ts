// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { TILE_ONLY_PLATFORMS, frostIsTileOnly, stampFrostSupport } from './frost-support'

function setup({ supports = true, reduced = false }: { supports?: boolean; reduced?: boolean }): void {
  vi.stubGlobal('CSS', { supports: vi.fn(() => supports) })
  vi.stubGlobal('matchMedia', vi.fn((q: string) => ({ matches: reduced && q.includes('reduced-transparency') })))
}

describe('frost support', () => {
  beforeEach(() => document.documentElement.removeAttribute('data-frost'))
  afterEach(() => vi.unstubAllGlobals())

  it('keeps Frost when the webview can draw it', () => {
    setup({})
    expect(frostIsTileOnly()).toBe(false)
  })

  it('falls back when backdrop-filter is not supported', () => {
    setup({ supports: false })
    expect(frostIsTileOnly()).toBe(true)
  })

  it('falls back for reduced transparency', () => {
    setup({ reduced: true })
    expect(frostIsTileOnly()).toBe(true)
  })

  it('forces tile-only by the one switch, from the platform the webview runs on', () => {
    setup({})
    vi.stubGlobal('navigator', { platform: 'Linux x86_64', userAgent: '' })
    expect(frostIsTileOnly(['linux'])).toBe(true)
    expect(frostIsTileOnly(['windows'])).toBe(false)
  })

  it('ships with no platform forced', () => {
    expect(TILE_ONLY_PLATFORMS).toEqual([])
  })

  it('stamps and clears the attribute as the answer changes', () => {
    setup({ reduced: true })
    stampFrostSupport()
    expect(document.documentElement.getAttribute('data-frost')).toBe('tile')
    setup({})
    stampFrostSupport()
    expect(document.documentElement.hasAttribute('data-frost')).toBe(false)
  })
})
