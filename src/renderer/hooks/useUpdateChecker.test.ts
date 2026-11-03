// @vitest-environment jsdom
//
// The automatic update check fetches GitHub at most once per interval per
// install, however many windows or reloads there are. `openPage()` gives a
// fresh module graph per call, as a reload or a second window does: its own
// store and `pendingUpdate`, the same localStorage.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  AUTO_UPDATE_CHECKS_EVENT,
  LS_AUTO_UPDATE_CHECKS,
  LS_LAST_UPDATE_CHECK,
} from '@/lib/auto-update-checks'
import { UPDATE_CHECK_INTERVAL } from '@shared/constants'

const START = Date.UTC(2026, 10, 2, 12, 0, 0)
const STARTUP = 5000
/** Longer than any window's random wake spread. */
const SPREAD = 60_000
const RUNNING = '1.0.0'
const FOUND = { version: '9.9.9', notes: 'shiny' }
const PACKAGE_URL = 'https://github.com/Alakazam-211/K2/releases/tag/v9.9.9'

interface Probe {
  kind: string
  version: string | null
  has_update: boolean
  download_url: string | null
  can_install?: boolean
}

/** What the next GitHub fetch answers; throw to fail it. */
let answer: () => typeof FOUND | null
let probe: Probe
let running: string
let unmounts: Array<() => void>

function fetches(): number {
  return invokeMock.mock.calls.filter(([cmd]) => cmd === 'app_update_check').length
}

function calls(cmd: string): unknown[][] {
  return invokeMock.mock.calls.filter(([c]) => c === cmd)
}

async function loadModules() {
  vi.resetModules()
  const hook = await import('./useUpdateChecker')
  const { useUpdateStore } = await import('@/stores/update')
  const { useToastStore } = await import('@/stores/toast')
  const { setConnectedAirgap } = await import('@/lib/airgap')
  return { ...hook, useUpdateStore, useToastStore, setConnectedAirgap }
}

async function openPage() {
  const modules = await loadModules()
  const { unmount } = renderHook(() => modules.useUpdateChecker())
  unmounts.push(unmount)
  return { ...modules, close: unmount }
}

async function pass(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

function stored(): { at: number; offer: { version: string } | null } {
  const raw = localStorage.getItem(LS_LAST_UPDATE_CHECK)
  if (raw === null) throw new Error('no update check recorded')
  return JSON.parse(raw)
}

function remember(ageMs: number, offer: unknown = null): void {
  localStorage.setItem(LS_LAST_UPDATE_CHECK, JSON.stringify({ at: Date.now() - ageMs, offer }))
}

const UPDATE_KEYS = [LS_AUTO_UPDATE_CHECKS, LS_LAST_UPDATE_CHECK]
const OFFER = { version: FOUND.version, notes: FOUND.notes, downloadUrl: null, canInstall: false }

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(START)
  localStorage.clear()
  answer = () => null
  probe = { kind: 'other', version: null, has_update: false, download_url: null }
  running = RUNNING
  unmounts = []
  invokeMock.mockReset()
  invokeMock.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'get_current_version':
        return running
      case 'probe_install_update':
        return probe
      case 'app_update_check':
        return answer()
      case 'app_update_download':
        return 'U2'
      default:
        throw new Error(`unexpected invoke ${cmd}`)
    }
  })
})

afterEach(() => {
  for (const unmount of unmounts) unmount()
  cleanup()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('the interval guard', () => {
  it('a first load with no memory checks after 5 seconds', async () => {
    await openPage()
    await pass(STARTUP - 1)
    expect(fetches()).toBe(0)
    await pass(1)
    expect(fetches()).toBe(1)
    expect(stored().at).toBe(START + STARTUP)
  })

  it('a reload inside the interval does not fetch, and the check comes when the interval is up', async () => {
    const first = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    const checkedAt = stored().at
    first.close()

    await pass(10 * 60_000)
    await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)

    await pass(checkedAt + UPDATE_CHECK_INTERVAL - Date.now() - 1)
    expect(fetches()).toBe(1)
    await pass(SPREAD + 1)
    expect(fetches()).toBe(2)
    expect(stored().at).toBeGreaterThanOrEqual(checkedAt + UPDATE_CHECK_INTERVAL)
  })

  it('a reload loop fetches once', async () => {
    for (let i = 0; i < 20; i++) {
      const page = await openPage()
      await pass(STARTUP + 1000)
      page.close()
    }
    expect(fetches()).toBe(1)
  })

  it('a load past the interval checks after 5 seconds', async () => {
    remember(UPDATE_CHECK_INTERVAL)
    await openPage()
    await pass(STARTUP - 1)
    expect(fetches()).toBe(0)
    await pass(1)
    expect(fetches()).toBe(1)
  })

  it('keeps checking once per interval on one open page', async () => {
    await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(2)
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(3)
  })

  it('a failed check records the time and waits for the next interval', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    answer = () => {
      throw new Error('connection timed out')
    }
    const first = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    expect(first.useUpdateStore.getState().status).toBe('error')
    first.close()

    await openPage()
    await pass(STARTUP + 1000)
    expect(fetches()).toBe(1)
  })

  it('the page dying during the fetch still holds the next load back', async () => {
    answer = () => new Promise<null>(() => {}) as unknown as null
    const first = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    expect(stored().at).toBe(START + STARTUP)
    first.close()

    await openPage()
    await pass(STARTUP + 1000)
    expect(fetches()).toBe(1)
  })
})

describe('two windows', () => {
  it('fetch once when both load together', async () => {
    await openPage()
    await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
  })

  it('fetch once when both wake at the same moment for the same stored time', async () => {
    vi.spyOn(Math, 'random').mockReturnValue(0)
    remember(60_000)
    await openPage()
    await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(0)
    await pass(UPDATE_CHECK_INTERVAL - 60_000 - STARTUP)
    expect(fetches()).toBe(1)
  })

  it('the second window shows what the first found when its timer wakes', async () => {
    answer = () => FOUND
    const a = await openPage()
    const b = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    expect(a.useUpdateStore.getState().status).toBe('available')
    expect(b.useUpdateStore.getState().status).toBe('available')
    expect(b.useUpdateStore.getState().version).toBe(FOUND.version)
  })

  it("the storage event brings another window's answer straight away", async () => {
    remember(60_000)
    const b = await openPage()
    await pass(STARTUP)
    expect(b.useUpdateStore.getState().status).toBe('idle')

    remember(0, OFFER)
    await act(async () => {
      window.dispatchEvent(new StorageEvent('storage', { key: LS_LAST_UPDATE_CHECK }))
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(b.useUpdateStore.getState().status).toBe('available')
    expect(fetches()).toBe(0)

    remember(0, null)
    await act(async () => {
      window.dispatchEvent(new StorageEvent('storage', { key: LS_LAST_UPDATE_CHECK }))
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(b.useUpdateStore.getState().status).toBe('idle')
    expect(b.useUpdateStore.getState().version).toBeNull()
  })
})

describe('what ignores the guard', () => {
  it('a manual check fetches inside the interval and records the time', async () => {
    remember(60_000)
    const before = stored().at
    const page = await openPage()
    await page.checkForUpdate(true)
    expect(fetches()).toBe(1)
    expect(stored().at).toBeGreaterThan(before)
  })

  it('a remote update trigger fetches inside the interval and records the time', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true })))
    const base = invokeMock.getMockImplementation()
    invokeMock.mockImplementation(async (cmd: string, args: unknown) => {
      if (cmd === 'daemon_ws_url') return { state: 'available', port: 9, token: 't' }
      if (cmd === 'stop_bundled_daemon_for_update') return null
      if (cmd === 'app_update_install') return null
      if (cmd === 'relaunch_via_open') return null
      return base?.(cmd, args)
    })
    answer = () => FOUND
    remember(60_000)
    const before = stored().at
    vi.resetModules()
    const { runTriggeredAppUpdate } = await import('@/lib/app-update-driver')
    await runTriggeredAppUpdate('job-1')
    expect(fetches()).toBe(1)
    expect(calls('app_update_download')).toHaveLength(1)
    expect(stored().at).toBeGreaterThan(before)
  })
})

describe('with storage that cannot be trusted, as today', () => {
  it('storage throwing: every load checks after 5 seconds, then every interval', async () => {
    // Only the update keys: other stores read storage at import, unguarded.
    const realGet = Storage.prototype.getItem
    const realSet = Storage.prototype.setItem
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, key: string) {
      if (UPDATE_KEYS.includes(key)) throw new Error('denied')
      return realGet.call(this, key)
    })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key: string, value: string) {
      if (UPDATE_KEYS.includes(key)) throw new Error('denied')
      realSet.call(this, key, value)
    })
    const first = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(2)
    first.close()

    await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(3)
  })

  it('only writes throwing: an old time is not rechecked in a tight loop', async () => {
    remember(UPDATE_CHECK_INTERVAL)
    const realSet = Storage.prototype.setItem
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, key: string, value: string) {
      if (key === LS_LAST_UPDATE_CHECK) throw new Error('quota')
      realSet.call(this, key, value)
    })
    await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    await pass(UPDATE_CHECK_INTERVAL - 1)
    expect(fetches()).toBe(1)
  })

  for (const [name, raw] of [
    ['not JSON', 'yesterday'],
    ['a bare number', '12345'],
    ['a text time', '{"at":"soon","offer":null}'],
    ['no time', '{"offer":null}'],
    ['a future time', JSON.stringify({ at: START + 60_000, offer: null })],
    ['a broken offer', JSON.stringify({ at: START - 1000, offer: { version: 7 } })],
    [
      'a download URL off K2 releases',
      JSON.stringify({
        at: START - 1000,
        offer: { version: '9.9.9', notes: null, downloadUrl: 'https://evil.example/k2.deb', canInstall: false },
      }),
    ],
  ] as const) {
    it(`${name}: checks after 5 seconds and repairs the record`, async () => {
      localStorage.setItem(LS_LAST_UPDATE_CHECK, raw)
      const page = await openPage()
      await pass(STARTUP)
      expect(fetches()).toBe(1)
      expect(page.useUpdateStore.getState().status).toBe('idle')
      expect(stored().at).toBe(START + STARTUP)
    })
  }
})

describe('the update offer across a reload', () => {
  it('a reload keeps the offer, with no fetch, and offers it again', async () => {
    answer = () => FOUND
    const first = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    first.close()

    const second = await openPage()
    expect(second.useUpdateStore.getState().status).toBe('idle')
    await pass(STARTUP)
    expect(fetches()).toBe(1)
    const state = second.useUpdateStore.getState()
    expect(state.status).toBe('available')
    expect(state.version).toBe(FOUND.version)
    expect(state.notes).toBe(FOUND.notes)
    const toasts = second.useToastStore.getState().toasts
    expect(toasts.map((t) => t.message)).toEqual([`K2 v${FOUND.version} is available`])
  })

  it('Download after a reload fetches once, then downloads, and never fails on an empty Rust slot', async () => {
    answer = () => FOUND
    const first = await openPage()
    await pass(STARTUP)
    first.close()
    invokeMock.mockClear()

    const second = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(0)

    const before = stored().at
    await pass(1000)
    await second.useUpdateStore.getState().openAvailableDownload()
    expect(fetches()).toBe(1)
    expect(calls('app_update_download')).toHaveLength(1)
    const order = invokeMock.mock.calls.map(([cmd]) => cmd)
    expect(order.indexOf('app_update_check')).toBeLessThan(order.indexOf('app_update_download'))
    expect(second.useUpdateStore.getState().status).toBe('ready')
    expect(stored().at).toBeGreaterThan(before)

    await second.useUpdateStore.getState().startDownload()
    expect(fetches()).toBe(1)
  })

  it('Download after a reload shows the error when the fresh check fails', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    answer = () => FOUND
    const first = await openPage()
    await pass(STARTUP)
    first.close()

    const second = await openPage()
    await pass(STARTUP)
    answer = () => {
      throw new Error('connection timed out')
    }
    await second.useUpdateStore.getState().openAvailableDownload()
    expect(second.useUpdateStore.getState().status).toBe('error')
    expect(second.useUpdateStore.getState().error).toContain('connection timed out')
    expect(calls('app_update_download')).toHaveLength(0)
  })

  it('a reload after a failed check keeps the update found before it', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    answer = () => FOUND
    const first = await openPage()
    await pass(STARTUP)
    answer = () => {
      throw new Error('connection timed out')
    }
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(2)
    expect(first.useUpdateStore.getState().status).toBe('error')
    first.close()

    const second = await openPage()
    await pass(STARTUP)
    expect(fetches()).toBe(2)
    expect(second.useUpdateStore.getState().status).toBe('available')
  })

  it('a check that finds nothing ends the offer', async () => {
    answer = () => FOUND
    const first = await openPage()
    await pass(STARTUP)
    answer = () => null
    await first.checkForUpdate(true)
    expect(stored().offer).toBeNull()
    first.close()

    const second = await openPage()
    await pass(STARTUP)
    expect(second.useUpdateStore.getState().status).toBe('idle')
  })

  it('after installing the update, the remembered offer is not shown', async () => {
    answer = () => FOUND
    const first = await openPage()
    await pass(STARTUP)
    first.close()

    running = FOUND.version
    const second = await openPage()
    await pass(STARTUP)
    expect(second.useUpdateStore.getState().status).toBe('idle')
    expect(second.useToastStore.getState().toasts).toEqual([])
    expect(fetches()).toBe(1)
  })

  it('a Linux package offer comes back whole and installs with no fetch', async () => {
    probe = { kind: 'linux-package', version: '9.9.9', has_update: true, download_url: PACKAGE_URL, can_install: true }
    const first = await openPage()
    await pass(STARTUP)
    expect(calls('probe_install_update')).toHaveLength(1)
    first.close()

    const second = await openPage()
    await pass(STARTUP)
    expect(calls('probe_install_update')).toHaveLength(1)
    const state = second.useUpdateStore.getState()
    expect(state.status).toBe('available')
    expect(state.archDownloadUrl).toBe(PACKAGE_URL)
    expect(state.linuxCanInstall).toBe(true)
  })

  it('leaves a running download alone', async () => {
    remember(60_000, OFFER)
    const page = await openPage()
    page.useUpdateStore.setState({ status: 'downloading' })
    await pass(STARTUP)
    expect(page.useUpdateStore.getState().status).toBe('downloading')
    expect(page.useToastStore.getState().toasts).toEqual([])
  })
})

describe('air-gap and the off switch, as today', () => {
  it('air-gap: no fetch, no offer, no time recorded', async () => {
    remember(60_000, OFFER)
    const before = localStorage.getItem(LS_LAST_UPDATE_CHECK)
    const modules = await loadModules()
    modules.setConnectedAirgap(true)
    const { unmount } = renderHook(() => modules.useUpdateChecker())
    unmounts.push(unmount)
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(0)
    expect(modules.useUpdateStore.getState().status).toBe('idle')
    await modules.checkForUpdate(true)
    expect(fetches()).toBe(0)
    expect(localStorage.getItem(LS_LAST_UPDATE_CHECK)).toBe(before)
  })

  it('air-gap turning on after load: the wake neither fetches nor loops', async () => {
    const page = await openPage()
    page.setConnectedAirgap(true)
    await pass(STARTUP)
    expect(fetches()).toBe(0)
    expect(vi.getTimerCount()).toBeLessThanOrEqual(1)
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(0)
  })

  it('off switch: no fetch and no offer, a manual check still runs', async () => {
    remember(60_000, OFFER)
    localStorage.setItem(LS_AUTO_UPDATE_CHECKS, '0')
    const page = await openPage()
    await pass(UPDATE_CHECK_INTERVAL + SPREAD)
    expect(fetches()).toBe(0)
    expect(page.useUpdateStore.getState().status).toBe('idle')

    await act(async () => {
      window.dispatchEvent(new StorageEvent('storage', { key: LS_LAST_UPDATE_CHECK }))
      await vi.advanceTimersByTimeAsync(0)
    })
    expect(page.useUpdateStore.getState().status).toBe('idle')

    await page.checkForUpdate(true)
    expect(fetches()).toBe(1)
  })

  it('turning the switch on inside the interval does not fetch', async () => {
    remember(60_000)
    localStorage.setItem(LS_AUTO_UPDATE_CHECKS, '0')
    await openPage()
    localStorage.setItem(LS_AUTO_UPDATE_CHECKS, '1')
    window.dispatchEvent(new Event(AUTO_UPDATE_CHECKS_EVENT))
    await pass(STARTUP)
    expect(fetches()).toBe(0)
  })
})
