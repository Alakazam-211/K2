// @vitest-environment jsdom
// The browserLayering switch: underlay by default on macOS only, the
// previous path kept as legacy, and a run-time failure falls back for the session.
import { afterEach, describe, expect, it, vi } from 'vitest'
import {
  LS_BROWSER_HOVER_PRIVATE_FALLBACK,
  LS_BROWSER_LAYERING,
  LS_BROWSER_PAGE_FIRST_CLICK,
  readBrowserLayering,
  readHoverPrivateFallback,
  readPageFirstClick,
  readHoverNoGating,
  LS_BROWSER_HOVER_ROUTE,
  layeringStatus,
  underlayActive,
  useBrowserLayeringStore,
} from './browser-layering'

afterEach(() => {
  localStorage.clear()
  useBrowserLayeringStore.setState({ mode: 'underlay', failed: null })
  vi.restoreAllMocks()
})

describe('browserLayering', () => {
  it('defaults to underlay on macOS and legacy elsewhere', () => {
    expect(readBrowserLayering(true)).toBe('underlay')
    expect(readBrowserLayering(false)).toBe('legacy')
  })

  it('keeps the stored choice and ignores garbage', () => {
    localStorage.setItem(LS_BROWSER_LAYERING, 'legacy')
    expect(readBrowserLayering(true)).toBe('legacy')
    localStorage.setItem(LS_BROWSER_LAYERING, 'sideways')
    expect(readBrowserLayering(true)).toBe('underlay')
  })

  it('setMode writes through', () => {
    useBrowserLayeringStore.getState().setMode('legacy')
    expect(localStorage.getItem(LS_BROWSER_LAYERING)).toBe('legacy')
    expect(useBrowserLayeringStore.getState().mode).toBe('legacy')
  })

  it('is active only on the macOS desktop app, chosen and not failed', () => {
    expect(underlayActive({ mode: 'underlay', failed: null }, true, true)).toBe(true)
    expect(underlayActive({ mode: 'underlay', failed: null }, false, true)).toBe(false)
    expect(underlayActive({ mode: 'underlay', failed: null }, true, false)).toBe(false)
    expect(underlayActive({ mode: 'legacy', failed: null }, true, true)).toBe(false)
    expect(underlayActive({ mode: 'underlay', failed: 'x' }, true, true)).toBe(false)
  })

  it('a failure falls back for the session, logs once, and leaves the setting alone', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    useBrowserLayeringStore.getState().markFailed('the UI view has no layer to mask')
    useBrowserLayeringStore.getState().markFailed('again')
    expect(useBrowserLayeringStore.getState().failed).toBe('the UI view has no layer to mask')
    expect(useBrowserLayeringStore.getState().mode).toBe('underlay')
    expect(warn).toHaveBeenCalledTimes(1)
    expect(String(warn.mock.calls[0][0])).toContain('legacy browser path')
  })

  it('the private hover fallback is off unless stored as 1', () => {
    expect(readHoverPrivateFallback()).toBe(false)
    localStorage.setItem(LS_BROWSER_HOVER_PRIVATE_FALLBACK, '1')
    expect(readHoverPrivateFallback()).toBe(true)
  })

  it('says which path a window takes and why', () => {
    expect(layeringStatus({ mode: 'underlay', failed: null }, true, true)).toBe('underlay ON')
    expect(layeringStatus({ mode: 'underlay', failed: null }, false, true)).toBe('legacy (not macOS)')
    expect(layeringStatus({ mode: 'legacy', failed: null }, true, true)).toContain('setting is legacy')
    expect(layeringStatus({ mode: 'underlay', failed: 'stale' }, true, true)).toBe('legacy (fell back this session: stale)')
    expect(layeringStatus({ mode: 'underlay', failed: null }, true, false)).toContain('web client')
  })

  it('macOS with no stored key reads underlay; the Settings row reads the same store', () => {
    localStorage.removeItem(LS_BROWSER_LAYERING)
    expect(readBrowserLayering(true)).toBe('underlay')
  })

  it('hover gating stays on unless the route is stored as none', () => {
    expect(readHoverNoGating()).toBe(false)
    localStorage.setItem(LS_BROWSER_HOVER_ROUTE, 'gate')
    expect(readHoverNoGating()).toBe(false)
    localStorage.setItem(LS_BROWSER_HOVER_ROUTE, 'none')
    expect(readHoverNoGating()).toBe(true)
  })

  it('the first click on a page is off unless stored as 1', () => {
    expect(readPageFirstClick()).toBe(false)
    localStorage.setItem(LS_BROWSER_PAGE_FIRST_CLICK, '1')
    expect(readPageFirstClick()).toBe(true)
  })
})
