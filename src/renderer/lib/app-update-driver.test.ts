import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/plugin-updater', () => ({ check: vi.fn() }))

import { invoke } from '@tauri-apps/api/core'
import { check } from '@tauri-apps/plugin-updater'
import { runTriggeredAppUpdate } from '@/lib/app-update-driver'
import { setConnectedAirgap } from '@/lib/airgap'
import { useUpdateStore } from '@/stores/update'

const ZST =
  'https://github.com/Alakazam-211/K2/releases/download/v9.9.9/k2-9.9.9-x86_64.pkg.tar.zst'

describe('runTriggeredAppUpdate Arch offer', () => {
  beforeEach(() => {
    setConnectedAirgap(false)
    useUpdateStore.setState({
      status: 'idle',
      version: null,
      notes: null,
      progress: 0,
      error: null,
      archDownloadUrl: null,
    })
    vi.mocked(invoke).mockReset()
    vi.mocked(check).mockReset()
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({ ok: true })),
    )
  })

  it('does not download, install, or relaunch the pacman package', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'daemon_ws_url') {
        return { state: 'available', port: 9, token: 't' }
      }
      if (cmd === 'probe_install_update') {
        return {
          kind: 'arch',
          version: '9.9.9',
          has_update: true,
          download_url: ZST,
        }
      }
      throw new Error(`unexpected invoke ${cmd}`)
    })

    await runTriggeredAppUpdate('job-1')

    expect(check).not.toHaveBeenCalled()
    const cmds = vi.mocked(invoke).mock.calls.map((call) => call[0])
    expect(cmds).not.toContain('relaunch_via_open')
    expect(cmds).not.toContain('stop_bundled_daemon_for_update')
    expect(useUpdateStore.getState().status).toBe('available')
    expect(useUpdateStore.getState().archDownloadUrl).toBe(ZST)

    const bodies = vi.mocked(fetch).mock.calls.map((call) => {
      const init = call[1] as { body?: string } | undefined
      return init?.body ?? ''
    })
    expect(bodies.some((body) => body.includes('restarting'))).toBe(false)
    expect(bodies.some((body) => body.includes('applying'))).toBe(false)
    expect(bodies.some((body) => body.includes('"failed"'))).toBe(true)
  })
})
