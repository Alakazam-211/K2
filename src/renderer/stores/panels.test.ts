// Per-window drawer chrome + remaining hasLoadedFromDaemon persist-gate
// for tab-list settingsUpdate (Phase 2.5 fix #547).
//
// Chrome contract (prd-per-window-chrome-v1):
//   1. toggleLeft/Right write local `k2.windowChrome.<label>` and must
//      NOT settingsUpdate leftPanelOpen / rightPanelOpen.
//   2. After a local close, initFromSettings with daemon leftPanelOpen
//      true must NOT reopen.
//   3. Tab lists may still refresh from daemon; persist gate still wraps
//      tab-list settingsUpdate.
//
// The store's import-time side-effect (`initFromSettings()` called
// immediately at module load) means we MUST set up `vi.mock` BEFORE
// importing the store. vitest hoists `vi.mock` calls automatically so
// `import` lines below get their mocked dependencies.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const settingsUpdateCalls: Array<Record<string, unknown>> = []
const settingsResetCalls: Array<void> = []

let settingsGetResolver: ((value: Record<string, unknown>) => void) | null = null
let settingsGetRejecter: ((reason: unknown) => void) | null = null
let settingsGetPromise: Promise<Record<string, unknown>> | null = null

function freshSettingsGetPromise(): Promise<Record<string, unknown>> {
  settingsGetPromise = new Promise((resolve, reject) => {
    settingsGetResolver = resolve
    settingsGetRejecter = reject
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
  settingsGet: vi.fn(() => {
    return settingsGetPromise ?? freshSettingsGetPromise()
  }),
  settingsUpdate: vi.fn((updates: Record<string, unknown>) => {
    settingsUpdateCalls.push(updates)
    return Promise.resolve({})
  }),
  settingsReset: vi.fn(() => {
    settingsResetCalls.push()
    return Promise.resolve({})
  }),
}))

vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

vi.mock('./toast', () => ({
  useToastStore: { getState: () => ({ showToast: vi.fn() }) },
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(() => Promise.resolve({ port: 0, token: '', host: '127.0.0.1', secure: false })),
  invalidateDaemonWs: vi.fn(),
  daemonHttpBase: (c: { host: string; port: number }) => `http://${c.host}:${c.port}`,
  daemonWsBase: (c: { host: string; port: number }) => `ws://${c.host}:${c.port}`,
}))

freshSettingsGetPromise()

import {
  usePanelsStore,
  __resetPanelsLoadGateForTests,
} from './panels'
import {
  readWindowChrome,
  windowChromeKey,
  writeWindowChrome,
} from '@/lib/window-chrome'

type PanelTab = 'files' | 'changes' | 'history' | 'workspace'
const DEFAULT_TABS: {
  leftPanelActiveTab: PanelTab
  rightPanelActiveTab: PanelTab
  leftPanelTabs: PanelTab[]
  rightPanelTabs: PanelTab[]
} = {
  leftPanelActiveTab: 'files',
  rightPanelActiveTab: 'history',
  leftPanelTabs: ['files', 'workspace'],
  rightPanelTabs: ['history', 'changes'],
}

function chromeUpdates(
  calls: Array<Record<string, unknown>>,
): Array<Record<string, unknown>> {
  return calls.filter(
    (c) =>
      Object.prototype.hasOwnProperty.call(c, 'leftPanelOpen') ||
      Object.prototype.hasOwnProperty.call(c, 'rightPanelOpen') ||
      Object.prototype.hasOwnProperty.call(c, 'sidebarCollapsed'),
  )
}

function resetPanelState(): void {
  usePanelsStore.setState({
    leftPanelOpen: true,
    rightPanelOpen: true,
    ...DEFAULT_TABS,
  })
}

describe('panels store — per-window chrome + tab persist gate', () => {
  beforeEach(() => {
    __resetPanelsLoadGateForTests()
    settingsUpdateCalls.length = 0
    settingsResetCalls.length = 0
    localMem.clear()
    windowLabel.value = 'main'
    resetPanelState()
    freshSettingsGetPromise()
  })

  it('suppresses tab settingsUpdate before init resolves (gate=false)', () => {
    expect(settingsUpdateCalls).toHaveLength(0)
    usePanelsStore.getState().setLeftPanelActiveTab('workspace')
    expect(settingsUpdateCalls).toHaveLength(0)
    expect(usePanelsStore.getState().leftPanelActiveTab).toBe('workspace')
  })

  it('toggles do not settingsUpdate chrome keys, even after init', async () => {
    const initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
      ...DEFAULT_TABS,
    })
    await initPromise

    usePanelsStore.getState().toggleRightPanel()
    usePanelsStore.getState().toggleLeftPanel()
    expect(chromeUpdates(settingsUpdateCalls)).toEqual([])
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(readWindowChrome('main')).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: false,
    })
  })

  it('local chrome writes are not gated on hasLoadedFromDaemon', () => {
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
    usePanelsStore.getState().toggleLeftPanel()
    expect(settingsUpdateCalls).toHaveLength(0)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(readWindowChrome('main')).toEqual({
      leftOpen: false,
      rightOpen: true,
      sidebarCollapsed: false,
    })
  })

  it('initFromSettings with daemon leftPanelOpen true does not reopen after local close', async () => {
    usePanelsStore.getState().toggleLeftPanel()
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)

    const initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
      ...DEFAULT_TABS,
    })
    await initPromise

    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(readWindowChrome('main')?.leftOpen).toBe(false)

    freshSettingsGetPromise()
    const again = usePanelsStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
      leftPanelActiveTab: 'workspace',
      rightPanelActiveTab: 'changes',
      leftPanelTabs: ['files', 'workspace'],
      rightPanelTabs: ['history', 'changes'],
    })
    await again

    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(usePanelsStore.getState().leftPanelActiveTab).toBe('workspace')
  })

  it('host switch / later init does not restamp chrome from a new daemon', async () => {
    writeWindowChrome({ leftOpen: false, rightOpen: false, sidebarCollapsed: true }, 'main')
    usePanelsStore.setState({ leftPanelOpen: false, rightPanelOpen: false })

    const initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
      leftPanelActiveTab: 'workspace',
      rightPanelActiveTab: 'changes',
      leftPanelTabs: ['workspace'],
      rightPanelTabs: ['changes'],
    })
    await initPromise

    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
    expect(readWindowChrome('main')).toEqual({
      leftOpen: false,
      rightOpen: false,
      sidebarCollapsed: true,
    })
    expect(usePanelsStore.getState().leftPanelTabs).toEqual(['workspace'])
    expect(localMem.getItem(windowChromeKey('main'))).toBeTruthy()
  })

  it('activateTab is memory-only (does not write chrome or settingsUpdate)', async () => {
    usePanelsStore.setState({ leftPanelOpen: false, rightPanelOpen: false })
    writeWindowChrome({ leftOpen: false, rightOpen: false, sidebarCollapsed: false }, 'main')

    usePanelsStore.getState().activateTab('files')
    expect(usePanelsStore.getState().leftPanelOpen).toBe(true)
    expect(readWindowChrome('main')?.leftOpen).toBe(false)
    expect(settingsUpdateCalls).toHaveLength(0)
  })

  it('leaves gate=false when settingsGet rejects (so retry can win later)', async () => {
    __resetPanelsLoadGateForTests()
    settingsUpdateCalls.length = 0

    const initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetRejecter!(new Error('daemon down'))
    await initPromise

    usePanelsStore.getState().setLeftPanelActiveTab('workspace')
    expect(settingsUpdateCalls).toHaveLength(0)
  })

  it('a successful retry after failure flips the tab persist gate', async () => {
    let initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetRejecter!(new Error('first attempt fails'))
    await initPromise

    usePanelsStore.getState().setRightPanelActiveTab('changes')
    expect(settingsUpdateCalls).toHaveLength(0)

    freshSettingsGetPromise()
    initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      sidebarCollapsed: false,
      ...DEFAULT_TABS,
    })
    await initPromise

    usePanelsStore.getState().setRightPanelActiveTab('history')
    expect(settingsUpdateCalls.length).toBeGreaterThanOrEqual(1)
    const last = settingsUpdateCalls[settingsUpdateCalls.length - 1]
    expect(last).toEqual({ rightPanelActiveTab: 'history' })
    expect(last).not.toHaveProperty('rightPanelOpen')
    expect(last).not.toHaveProperty('leftPanelOpen')
  })

  it('per-label chrome keys stay independent from this window', () => {
    windowLabel.value = 'main'
    usePanelsStore.getState().toggleLeftPanel()
    writeWindowChrome({ leftOpen: true, rightOpen: false, sidebarCollapsed: true }, 'focus-x')
    writeWindowChrome({ leftOpen: false, rightOpen: false, sidebarCollapsed: true }, 'window-y')

    expect(readWindowChrome('main')?.leftOpen).toBe(false)
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
})
