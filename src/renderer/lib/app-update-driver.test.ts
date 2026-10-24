import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }))

import { invoke } from '@tauri-apps/api/core'
import { runTriggeredAppUpdate } from '@/lib/app-update-driver'
import { setConnectedAirgap } from '@/lib/airgap'
import { useUpdateStore } from '@/stores/update'

/** Stands in for the Rust `app_update_check` (SS4). */
const check = vi.fn()

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
      if (cmd === 'app_update_check') return check()
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

describe('runTriggeredAppUpdate target version relay (0.46.3)', () => {
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

  it('names the version it installs on applying and restarting, so the daemon can settle the job after the restart', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'daemon_ws_url') return { state: 'available', port: 9, token: 't' }
      if (cmd === 'probe_install_update') {
        return { kind: 'macos', version: null, has_update: false, download_url: null }
      }
      if (cmd === 'app_update_check') return check()
      if (cmd === 'app_update_download') return 'ABCDEF0123456789'
      if (cmd === 'app_update_install') return null
      if (cmd === 'stop_bundled_daemon_for_update' || cmd === 'relaunch_via_open') return undefined
      throw new Error(`unexpected invoke ${cmd}`)
    })
    check.mockResolvedValue({ version: '0.46.3', notes: null })

    await runTriggeredAppUpdate('job-7')

    const bodies = vi.mocked(fetch).mock.calls.map((call) => {
      const init = call[1] as { body?: string } | undefined
      return JSON.parse(init?.body ?? '{}') as Record<string, unknown>
    })
    expect(bodies.map((b) => b.phase)).toEqual(['downloading', 'applying', 'restarting'])
    expect(bodies[0]).toEqual({ job_id: 'job-7', phase: 'downloading' })
    expect(bodies[1]).toEqual({ job_id: 'job-7', phase: 'applying', target_version: '0.46.3' })
    expect(bodies[2]).toEqual({ job_id: 'job-7', phase: 'restarting', target_version: '0.46.3' })
  })
})
