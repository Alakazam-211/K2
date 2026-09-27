import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-updater', () => ({ check: vi.fn() }))

import { invoke } from '@tauri-apps/api/core'
import { check } from '@tauri-apps/plugin-updater'
import { checkForUpdate } from '@/hooks/useUpdateChecker'
import { setConnectedAirgap } from '@/lib/airgap'
import { useToastStore } from '@/stores/toast'
import { useUpdateStore } from '@/stores/update'

const ZST =
  'https://github.com/Alakazam-211/K2/releases/download/v9.9.9/k2-9.9.9-x86_64.pkg.tar.zst'
const MISSING_PLATFORM =
  'None of the fallback platforms \'["linux-x86_64"]\' were found in the response \'platforms\' object'

interface Probe {
  kind: string
  version: string | null
  has_update: boolean
  download_url: string | null
}

function resetStores(): void {
  setConnectedAirgap(false)
  useUpdateStore.setState({
    status: 'idle',
    version: null,
    notes: null,
    progress: 0,
    error: null,
    archDownloadUrl: null,
  })
  useToastStore.setState({ toasts: [] })
  vi.mocked(invoke).mockReset()
  vi.mocked(check).mockReset()
}

function mockProbe(result: Probe | string): void {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'probe_install_update') {
      if (typeof result === 'string') throw result
      return result
    }
    if (cmd === 'plugin:opener|open_url') return null
    throw new Error(`unexpected invoke ${cmd}`)
  })
}

describe('checkForUpdate install kind', () => {
  beforeEach(() => {
    resetStores()
    vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  it('arch newer skips check() and Download opens the zst', async () => {
    mockProbe({
      kind: 'arch',
      version: '9.9.9',
      has_update: true,
      download_url: ZST,
    })
    const hasUpdate = await useUpdateStore.getState().checkForUpdate()
    expect(hasUpdate).toBe(true)
    expect(check).not.toHaveBeenCalled()
    expect(invoke).toHaveBeenCalledWith('probe_install_update')
    const state = useUpdateStore.getState()
    expect(state.status).toBe('available')
    expect(state.version).toBe('9.9.9')
    expect(state.error).toBeNull()
    expect(state.archDownloadUrl).toBe(ZST)

    await useUpdateStore.getState().openAvailableDownload()
    expect(invoke).toHaveBeenCalledWith('plugin:opener|open_url', { url: ZST })
    expect(useUpdateStore.getState().status).toBe('available')

    await useUpdateStore.getState().startDownload()
    await useUpdateStore.getState().installAndRelaunch()
    expect(useUpdateStore.getState().status).toBe('available')
    const cmds = vi.mocked(invoke).mock.calls.map((call) => call[0])
    expect(cmds).not.toContain('relaunch_via_open')
    expect(cmds).not.toContain('stop_bundled_daemon_for_update')
    expect(check).not.toHaveBeenCalled()
  })

  it('arch equal version is idle with no error', async () => {
    mockProbe({
      kind: 'arch',
      version: '0.41.1',
      has_update: false,
      download_url: null,
    })
    const hasUpdate = await useUpdateStore.getState().checkForUpdate()
    expect(hasUpdate).toBe(false)
    expect(check).not.toHaveBeenCalled()
    expect(useUpdateStore.getState().status).toBe('idle')
    expect(useUpdateStore.getState().error).toBeNull()
    expect(useUpdateStore.getState().archDownloadUrl).toBeNull()
  })

  it.each(['macos', 'appimage', 'other'])(
    'kind %s still calls plugin check()',
    async (kind) => {
      mockProbe({
        kind,
        version: null,
        has_update: false,
        download_url: null,
      })
      vi.mocked(check).mockResolvedValue(null)
      const hasUpdate = await useUpdateStore.getState().checkForUpdate()
      expect(hasUpdate).toBe(false)
      expect(check).toHaveBeenCalledTimes(1)
      expect(useUpdateStore.getState().status).toBe('idle')
      expect(useUpdateStore.getState().archDownloadUrl).toBeNull()
    },
  )

  it('arch fetch failure keeps the fetch text and does not call check()', async () => {
    const failure = 'error sending request for url (https://github.com/Alakazam-211/K2/releases/latest/download/latest.json)'
    mockProbe(failure)
    const hasUpdate = await useUpdateStore.getState().checkForUpdate()
    expect(hasUpdate).toBe(false)
    expect(check).not.toHaveBeenCalled()
    const state = useUpdateStore.getState()
    expect(state.status).toBe('error')
    expect(state.error).toBe(failure)
    expect(state.error).not.toBe(MISSING_PLATFORM)
    expect(state.error).not.toContain('fallback platforms')
    expect(invoke).not.toHaveBeenCalledWith('check_for_update')
  })

  it('air-gap returns before the Arch probe', async () => {
    setConnectedAirgap(true)
    vi.mocked(invoke).mockImplementation(async () => {
      throw new Error('probe should not run')
    })
    const hasUpdate = await useUpdateStore.getState().checkForUpdate()
    expect(hasUpdate).toBe(false)
    expect(invoke).not.toHaveBeenCalled()
    expect(check).not.toHaveBeenCalled()
    expect(useUpdateStore.getState().status).toBe('idle')
    expect(useUpdateStore.getState().error).toBeNull()
  })

  it('does not toast up to date when the Arch fetch already failed', async () => {
    mockProbe('connection timed out')
    await checkForUpdate(true)
    expect(useUpdateStore.getState().status).toBe('error')
    expect(useUpdateStore.getState().error).toBe('connection timed out')
    expect(useToastStore.getState().toasts.map((t) => t.message)).not.toContain(
      'K2 is up to date',
    )
    expect(invoke).not.toHaveBeenCalledWith('check_for_update')
    expect(check).not.toHaveBeenCalled()
  })

  it('toasts up to date when Arch is current', async () => {
    mockProbe({
      kind: 'arch',
      version: null,
      has_update: false,
      download_url: null,
    })
    await checkForUpdate(true)
    expect(useUpdateStore.getState().status).toBe('idle')
    expect(useUpdateStore.getState().error).toBeNull()
    expect(useToastStore.getState().toasts.map((t) => t.message)).toContain(
      'K2 is up to date',
    )
  })
})
