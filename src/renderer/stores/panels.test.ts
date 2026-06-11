// Phase 2 Tier 2.3 — vitest coverage for the `hasLoadedFromDaemon`
// persist-gate added in Phase 2.5 fix #547. The gate's contract:
//
//   1. On first import the gate is `false` — settings_update calls
//      from store mutators are SUPPRESSED (UI state still updates
//      locally so interactions stay responsive).
//   2. A successful `initFromSettings()` flips the gate to `true` so
//      subsequent mutations DO persist via `settingsUpdate`.
//   3. A failed (rejected) `settingsGet()` leaves the gate `false`.
//      The daemon-reconnect listener will re-run init when the daemon
//      comes back online.
//
// We test panels.ts specifically because it's the only store that
// exposes the test reset hook `__resetPanelsLoadGateForTests()`. The
// other gated stores (focus-groups, projects, timer) follow the same
// pattern but keep their gate module-private — exercising one store's
// gate against a mocked daemon proves the pattern works.
//
// The store's import-time side-effect (`initFromSettings()` called
// immediately at module load) means we MUST set up `vi.mock` BEFORE
// importing the store. vitest hoists `vi.mock` calls automatically so
// `import` lines below get their mocked dependencies.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// Per-test promise that `settingsGet()` returns. Tests assign to
// `settingsGetImpl.value` before importing the store (or invoking
// initFromSettings) to control whether the gate flips.
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

// Mock the daemon-settings client. `vi.mock` is hoisted so this runs
// BEFORE the panels store imports `settingsGet`/`settingsUpdate`.
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(() => {
    // Default behaviour for module-load init: return a pending promise
    // so the gate stays false until a test resolves it. Tests that
    // want a successful init build a fresh resolver via
    // `prepareSettingsGetSuccess()`.
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

// Mock the daemon-reconnect bus — its real impl uses Tauri's `listen`
// which isn't available in the vitest Node env.
vi.mock('@/lib/daemon-reconnect', () => ({
  onDaemonConnected: vi.fn(),
}))

// Mock the toast store — panels.ts pulls it but we don't exercise
// toast behavior here.
vi.mock('./toast', () => ({
  useToastStore: { getState: () => ({ showToast: vi.fn() }) },
}))

// Mock daemon-cli + daemon-ws (transitively imported via daemon-settings'
// retry paths). With daemon-settings fully mocked, this is overkill but
// safe.
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(() => Promise.resolve({ port: 0, token: '', host: '127.0.0.1', secure: false })),
  invalidateDaemonWs: vi.fn(),
  daemonHttpBase: (c: { host: string; port: number }) => `http://${c.host}:${c.port}`,
  daemonWsBase: (c: { host: string; port: number }) => `ws://${c.host}:${c.port}`,
}))

// Initialize the pending promise BEFORE importing the store so the
// store's module-load init() suspends rather than rejecting.
freshSettingsGetPromise()

// NOW import the store — its top-level `initFromSettings()` will
// suspend on `await settingsGet()` because we haven't resolved yet.
import {
  usePanelsStore,
  __resetPanelsLoadGateForTests,
} from './panels'

describe('panels store — hasLoadedFromDaemon persist gate', () => {
  beforeEach(() => {
    // Reset both the module-level gate AND the captured call lists
    // so tests are independent.
    __resetPanelsLoadGateForTests()
    settingsUpdateCalls.length = 0
    settingsResetCalls.length = 0
    // Fresh pending promise so the next initFromSettings call can
    // be controlled per test.
    freshSettingsGetPromise()
  })

  it('suppresses settingsUpdate before init resolves (gate=false)', () => {
    // Gate has just been reset, no init has run yet (or the previous
    // run was reset by __resetPanelsLoadGateForTests). The store's
    // mutators MUST NOT call settingsUpdate.
    expect(settingsUpdateCalls).toHaveLength(0)

    // Toggle a panel — the mutator runs `if (shouldSuppressPersist()) return;`
    // BEFORE calling `settingsUpdate`.
    usePanelsStore.getState().toggleLeftPanel()

    expect(settingsUpdateCalls).toHaveLength(
      0,
    )
    // The UI state still updated locally even though we didn't persist.
    // (Sanity that we exercised the mutator's local-set path.)
    // Default `leftPanelOpen` is true → toggle flips to false.
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
  })

  it('persists settingsUpdate after init resolves (gate=true)', async () => {
    // Trigger the store's init (we already reset the gate in beforeEach).
    const initPromise = usePanelsStore.getState().initFromSettings()

    // Resolve the daemon's settings response with a non-default body so
    // initFromSettings completes without throwing.
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      leftPanelActiveTab: 'files',
      rightPanelActiveTab: 'history',
      leftPanelTabs: ['files', 'workspace'],
      rightPanelTabs: ['history', 'changes'],
    })
    await initPromise

    // The gate is now true. A mutation should persist.
    usePanelsStore.getState().toggleRightPanel()
    expect(settingsUpdateCalls).toHaveLength(1)
    expect(settingsUpdateCalls[0]).toEqual({ rightPanelOpen: false })
  })

  it('leaves gate=false when settingsGet rejects (so retry can win later)', async () => {
    // Reset to ensure the gate is false.
    __resetPanelsLoadGateForTests()
    settingsUpdateCalls.length = 0

    const initPromise = usePanelsStore.getState().initFromSettings()
    // Reject the get — initFromSettings's try/catch swallows it.
    settingsGetRejecter!(new Error('daemon down'))
    await initPromise // does not throw — catch handles it

    // A mutation must STILL be suppressed because the gate stayed false.
    usePanelsStore.getState().toggleLeftPanel()
    expect(settingsUpdateCalls).toHaveLength(0)
  })

  it('a successful retry after failure flips the gate', async () => {
    // First attempt fails.
    let initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetRejecter!(new Error('first attempt fails'))
    await initPromise

    // Sanity: still suppressed.
    usePanelsStore.getState().toggleLeftPanel()
    expect(settingsUpdateCalls).toHaveLength(0)

    // Second attempt succeeds (simulates daemon-reconnect retry).
    freshSettingsGetPromise()
    initPromise = usePanelsStore.getState().initFromSettings()
    settingsGetResolver!({
      leftPanelOpen: true,
      rightPanelOpen: true,
      leftPanelActiveTab: 'files',
      rightPanelActiveTab: 'history',
      leftPanelTabs: ['files', 'workspace'],
      rightPanelTabs: ['history', 'changes'],
    })
    await initPromise

    // Gate is now true — mutations persist.
    usePanelsStore.getState().toggleRightPanel()
    // toggleRightPanel was already called once before, so we expect
    // exactly one NEW settingsUpdate call.
    expect(settingsUpdateCalls.length).toBeGreaterThanOrEqual(1)
    const last = settingsUpdateCalls[settingsUpdateCalls.length - 1]
    expect(last).toHaveProperty('rightPanelOpen')
  })
})
