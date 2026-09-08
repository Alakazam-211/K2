import { describe, it, expect, beforeEach, vi } from 'vitest'

const settingsUpdateCalls: Array<Record<string, unknown>> = []

let settingsGetResolver: ((value: Record<string, unknown>) => void) | null = null
let settingsGetPromise: Promise<Record<string, unknown>> | null = null

function freshSettingsGetPromise(): Promise<Record<string, unknown>> {
  settingsGetPromise = new Promise((resolve) => {
    settingsGetResolver = resolve
  })
  return settingsGetPromise
}

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
vi.stubGlobal('localStorage', localMem)
vi.stubGlobal('sessionStorage', new MemoryStorage())

const windowLabel = vi.hoisted(() => ({ value: 'main' }))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: windowLabel.value }),
}))

vi.mock('@/lib/is-web', () => ({
  isWebClient: () => false,
}))

vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => settingsGetPromise ?? freshSettingsGetPromise()),
  settingsUpdate: vi.fn((updates: Record<string, unknown>) => {
    settingsUpdateCalls.push(updates)
    return Promise.resolve({})
  }),
  settingsReset: vi.fn(async () => ({})),
}))

freshSettingsGetPromise()

import { useSidebarStore, __resetSidebarChromeForTests } from './sidebar'
import { readWindowChrome, writeWindowChrome } from '@/lib/window-chrome'

describe('sidebar store — per-window rail chrome', () => {
  beforeEach(() => {
    __resetSidebarChromeForTests()
    settingsUpdateCalls.length = 0
    localMem.clear()
    windowLabel.value = 'main'
    useSidebarStore.setState({ isCollapsed: false, width: 240 })
    freshSettingsGetPromise()
  })

  it('toggle / collapse / expand do not settingsUpdate sidebarCollapsed', () => {
    useSidebarStore.getState().toggle()
    useSidebarStore.getState().collapse()
    useSidebarStore.getState().expand()
    expect(settingsUpdateCalls).toEqual([])
  })

  it('writes local chrome and is not gated on daemon load', () => {
    useSidebarStore.getState().toggle()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
    expect(readWindowChrome('main')).toEqual({
      leftOpen: true,
      rightOpen: true,
      sidebarCollapsed: true,
    })
  })

  it('initFromSettings with daemon sidebarCollapsed false does not expand after local collapse', async () => {
    useSidebarStore.getState().collapse()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)

    const initPromise = useSidebarStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
    })
    await initPromise

    expect(useSidebarStore.getState().isCollapsed).toBe(true)
    expect(readWindowChrome('main')?.sidebarCollapsed).toBe(true)
    expect(settingsUpdateCalls).toEqual([])
  })

  it('host switch does not pull collapsed from a new daemon (no host-change restamp)', async () => {
    writeWindowChrome({ leftOpen: true, rightOpen: true, sidebarCollapsed: true }, 'main')
    useSidebarStore.setState({ isCollapsed: true })

    const initPromise = useSidebarStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
    })
    await initPromise

    expect(useSidebarStore.getState().isCollapsed).toBe(true)

    freshSettingsGetPromise()
    const again = useSidebarStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: false,
      rightPanelOpen: false,
      sidebarCollapsed: false,
    })
    await again
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
  })

  it('focus-* ignores sidebarCollapsed writes; width stays memory-only', () => {
    windowLabel.value = 'focus-proj'
    useSidebarStore.getState().collapse()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
    expect(readWindowChrome('focus-proj')?.sidebarCollapsed ?? false).toBe(false)

    useSidebarStore.getState().setWidth(400)
    expect(useSidebarStore.getState().width).toBe(400)
    const raw = localMem.getItem('k2.windowChrome.focus-proj')
    if (raw) {
      expect(JSON.parse(raw)).not.toHaveProperty('width')
    }
    expect(settingsUpdateCalls).toEqual([])
  })
})
