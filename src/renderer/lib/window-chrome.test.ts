import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

const webFlag = vi.hoisted(() => ({ value: false }))
const windowLabel = vi.hoisted(() => ({ value: 'main' }))

vi.mock('@/lib/is-web', () => ({
  isWebClient: () => webFlag.value,
}))

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: windowLabel.value }),
}))

class MemoryStorage {
  private map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? this.map.get(k)! : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
  removeItem(k: string): void {
    this.map.delete(k)
  }
  clear(): void {
    this.map.clear()
  }
  key(i: number): string | null {
    return Array.from(this.map.keys())[i] ?? null
  }
  get length(): number {
    return this.map.size
  }
}

const localMem = new MemoryStorage()
const sessionMem = new MemoryStorage()

vi.stubGlobal('localStorage', localMem)
vi.stubGlobal('sessionStorage', sessionMem)

import {
  DEFAULT_WINDOW_CHROME,
  WINDOW_CHROME_KEY_PREFIX,
  getWindowLabel,
  hasWindowChromeKey,
  isFocusWindowLabel,
  readWindowChrome,
  seedWindowChromeIfMissing,
  windowChromeKey,
  writeWindowChrome,
} from './window-chrome'

describe('window chrome helper', () => {
  const addEventListener = vi.fn()

  beforeEach(() => {
    webFlag.value = false
    windowLabel.value = 'main'
    localMem.clear()
    sessionMem.clear()
    addEventListener.mockReset()
    vi.stubGlobal('window', {
      addEventListener,
      removeEventListener: vi.fn(),
    })
  })

  afterEach(() => {
    localMem.clear()
    sessionMem.clear()
  })

  it('keys as k2.windowChrome.<label>', () => {
    expect(WINDOW_CHROME_KEY_PREFIX).toBe('k2.windowChrome.')
    expect(windowChromeKey('main')).toBe('k2.windowChrome.main')
    expect(windowChromeKey('focus-abc')).toBe('k2.windowChrome.focus-abc')
    expect(windowChromeKey('window-uuid')).toBe('k2.windowChrome.window-uuid')
  })

  it('reads the live Tauri window label (shim falls back to main)', () => {
    windowLabel.value = 'window-abc'
    expect(getWindowLabel()).toBe('window-abc')
    windowLabel.value = ''
    expect(getWindowLabel()).toBe('main')
  })

  it('keeps per-label blobs independent', () => {
    writeWindowChrome({ leftOpen: false, rightOpen: true, sidebarCollapsed: true }, 'main')
    writeWindowChrome({ leftOpen: true, rightOpen: false, sidebarCollapsed: false }, 'focus-x')
    writeWindowChrome({ leftOpen: false, rightOpen: false, sidebarCollapsed: true }, 'window-y')

    expect(readWindowChrome('main')).toEqual({
      leftOpen: false,
      rightOpen: true,
      sidebarCollapsed: true,
    })
    expect(readWindowChrome('focus-x')).toEqual({
      leftOpen: true,
      rightOpen: false,
      sidebarCollapsed: false,
    })
    expect(readWindowChrome('window-y')).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: true,
    })
  })

  it('desktop writes localStorage, not sessionStorage', () => {
    webFlag.value = false
    writeWindowChrome({ leftOpen: false })
    expect(localMem.getItem('k2.windowChrome.main')).toContain('"leftOpen":false')
    expect(sessionMem.getItem('k2.windowChrome.main')).toBeNull()
  })

  it('web client (isWebClient) writes sessionStorage, not localStorage+main', () => {
    webFlag.value = true
    windowLabel.value = 'main'
    writeWindowChrome({ leftOpen: false, rightOpen: false, sidebarCollapsed: true })
    expect(sessionMem.getItem('k2.windowChrome.main')).toContain('"leftOpen":false')
    expect(localMem.getItem('k2.windowChrome.main')).toBeNull()
    expect(localMem.length).toBe(0)
  })

  it('seeds from daemon only when the key is missing', () => {
    const seeded = seedWindowChromeIfMissing({
      leftPanelOpen: false,
      rightPanelOpen: false,
      sidebarCollapsed: true,
    })
    expect(seeded).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: true,
    })
    expect(hasWindowChromeKey('main')).toBe(true)

    const again = seedWindowChromeIfMissing({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
    })
    expect(again).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: true,
    })
  })

  it('early toggle that writes the key wins over a later daemon seed', () => {
    writeWindowChrome({ leftOpen: false, rightOpen: false, sidebarCollapsed: true })
    const seeded = seedWindowChromeIfMissing({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
    })
    expect(seeded).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: true,
    })
  })

  it('focus-* ignores sidebarCollapsed read/write; drawers stay per-label', () => {
    expect(isFocusWindowLabel('focus-proj')).toBe(true)
    expect(isFocusWindowLabel('main')).toBe(false)

    const written = writeWindowChrome(
      { leftOpen: false, sidebarCollapsed: true },
      'focus-proj',
    )
    expect(written.leftOpen).toBe(false)
    expect(written.sidebarCollapsed).toBe(false)
    expect(readWindowChrome('focus-proj')).toEqual({
      leftOpen: false,
      rightOpen: true,
      sidebarCollapsed: false,
    })

    const seeded = seedWindowChromeIfMissing(
      { leftPanelOpen: false, rightPanelOpen: false, sidebarCollapsed: true },
      'focus-other',
    )
    expect(seeded).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: false,
    })
  })

  it('persists only { leftOpen, rightOpen, sidebarCollapsed } — no widths', () => {
    writeWindowChrome({
      leftOpen: false,
      rightOpen: true,
      sidebarCollapsed: true,
      // @ts-expect-error widths must never enter the chrome blob
      leftPanelWidth: 480,
      width: 320,
    })
    const raw = localMem.getItem('k2.windowChrome.main')
    expect(raw).toBeTruthy()
    const parsed = JSON.parse(raw!) as Record<string, unknown>
    expect(Object.keys(parsed).sort()).toEqual([
      'leftOpen',
      'rightOpen',
      'sidebarCollapsed',
    ])
    expect(parsed).not.toHaveProperty('leftPanelWidth')
    expect(parsed).not.toHaveProperty('width')
    expect(DEFAULT_WINDOW_CHROME).toEqual({
      leftOpen: true,
      rightOpen: true,
      sidebarCollapsed: false,
    })
  })

  it('does not register a storage-event peer listener', () => {
    writeWindowChrome({ leftOpen: false })
    readWindowChrome()
    seedWindowChromeIfMissing({ leftPanelOpen: true })
    expect(addEventListener).not.toHaveBeenCalled()
  })
})
