// @vitest-environment jsdom
// Settings → General → Update (remote host) across the boot-status views
// (PRD boot-status-ghost, 0.46.3). Rosson's condition for shipping the
// views: "boot status ghost is fine as long as it doesn't ruin how the
// server update flow works from the general settings page". These tests
// drive the real `UpdateHostRow` through check → start → staged → install
// & restart → comeback against a scripted remote daemon that HIDES its
// version from strangers (ghost) and answers its full body only to the
// host's own token — the restart is simulated, never a real daemon.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

const { cliGet, cliPost } = vi.hoisted(() => ({ cliGet: vi.fn(), cliPost: vi.fn() }))
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return { ...actual, daemonCliGet: cliGet, daemonCliPost: cliPost }
})
// The comeback watcher sleeps `jittered(2500)` between polls: run it fast.
vi.mock('@/lib/backoff', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/backoff')>()
  return { ...actual, jittered: () => 5 }
})

import { UpdateHostRow } from './GeneralSection'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

const HOST: ConnectHost = {
  id: 'id-rosson',
  label: 'Rosson',
  hostname: 'rosson.k2.dev',
  port: 443,
  secure: true,
  username: 'rosson',
  token: 'host-session-tok',
  remember: true,
  lastConnectedAt: null,
}

/** A scripted remote daemon. `ghost`: strangers get the public view. */
interface FakeRemote {
  ghost: boolean
  up: boolean
  version: string
  instanceId: string
  bootUrls: string[]
}
let remote: FakeRemote

function bootBody(token: string | null): Record<string, unknown> {
  const authed = token === HOST.token
  if (remote.ghost && !authed) {
    return { view: 'public', phase: 'ready', protocol: 1, protocolMin: 1, instanceId: remote.instanceId }
  }
  return {
    view: authed ? 'full' : 'compat',
    phase: 'ready',
    protocol: 1,
    protocolMin: 1,
    instanceId: remote.instanceId,
    version: remote.version,
    features: ['boot-status-views-v1'],
  }
}

function installFetch(): void {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string) => {
      const u = new URL(input)
      // Module-level stores (settings, themes, LLM status) poke the active
      // host on import; they are not part of this flow.
      if (u.pathname !== '/boot-status') return new Response('{}', { status: 404 })
      remote.bootUrls.push(input)
      if (!remote.up) {
        // The relay answers while the host's tunnel is down.
        return new Response(JSON.stringify({ error: 'no tunnel' }), { status: 502 })
      }
      return new Response(JSON.stringify(bootBody(u.searchParams.get('token'))), { status: 200 })
    }),
  )
}

/** daemon/update/* answers; `jobAfterRestart` is the job verdict once back. */
function scriptUpdate(opts: {
  check: Record<string, unknown>
  installKind?: string
  jobAfterRestart?: Record<string, unknown> | null
  backOn?: string
}): void {
  let restarted = false
  cliGet.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'auth/whoami') return { username: 'rosson', role: 'owner' }
    if (route === 'daemon/update/status') {
      if (!restarted) return { phase: 'staged', progress: 1 }
      if (opts.jobAfterRestart === null) throw Object.assign(new Error('unknown job'), { status: 404 })
      return opts.jobAfterRestart ?? { phase: 'done', running_version: opts.backOn ?? '0.46.3' }
    }
    throw new Error(`unexpected GET ${route}`)
  })
  cliPost.mockImplementation(async (_scope: unknown, route: string) => {
    if (route === 'daemon/update/check') return opts.check
    if (route === 'daemon/update/start') return { job_id: 'job-1' }
    if (route === 'daemon/update/apply') {
      // The host restarts onto the update: down for a moment, then a new
      // process (new instanceId) on the new version.
      restarted = true
      remote.up = false
      setTimeout(() => {
        remote.version = opts.backOn ?? '0.46.3'
        remote.instanceId = 'inst-new'
        remote.up = true
      }, 30)
      return { ok: true }
    }
    throw new Error(`unexpected POST ${route}`)
  })
}

function row(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-settings-id="general.update-host"]')
  if (!el) throw new Error('UpdateHostRow not rendered')
  return el
}

async function clickButton(name: string | RegExp): Promise<void> {
  const btn = await screen.findByRole('button', { name })
  await waitFor(() => expect((btn as HTMLButtonElement).disabled).toBe(false))
  await act(async () => {
    fireEvent.click(btn)
  })
}

const CHECK_AVAILABLE = {
  current: '0.46.2',
  latest: '0.46.3',
  available: true,
  installKind: 'standalone',
  platform: 'linux-x86_64',
}

beforeEach(() => {
  localStorage.clear()
  remote = { ghost: true, up: true, version: '0.46.2', instanceId: 'inst-old', bootUrls: [] }
  installFetch()
  cliGet.mockReset()
  cliPost.mockReset()
  useConnectHostStore.setState({ activeHost: HOST, hosts: [HOST], serverVersion: '0.46.2', serverProtocol: 1 })
  useConfirmDialogStore.setState({ confirm: async () => true })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('Settings → General → Update on a remote host (boot-status views)', () => {
  it('up to date: check shows the host version, no update button', async () => {
    scriptUpdate({ check: { current: '0.46.3', latest: '0.46.3', available: false, installKind: 'standalone' } })
    render(<UpdateHostRow />)
    await clickButton('Check for updates')
    await screen.findByText('Rosson is on v0.46.3')
    expect(screen.queryByRole('button', { name: 'Download' })).toBeNull()
    expect(screen.getByRole('button', { name: 'Check for updates' })).toBeTruthy()
  })

  it('ghost host: check → download → staged → install & restart → back on the new version (read with the host token)', async () => {
    scriptUpdate({ check: CHECK_AVAILABLE, jobAfterRestart: null })
    render(<UpdateHostRow />)

    await clickButton('Check for updates')
    await screen.findByText('Update available for Rosson — 0.46.2 → 0.46.3')

    await clickButton('Download')
    await screen.findByText('Update staged for Rosson — ready to install & restart.')

    await clickButton('Install & restart Rosson')
    // While it is away: the reconnecting line, never a failure.
    await screen.findByText('Updating Rosson… reconnecting…')
    // Back: the job is gone (an older daemon would 404), so the version can
    // only come from /boot-status — which a ghost host tells its own token.
    await screen.findByText('Updated Rosson to v0.46.3.', undefined, { timeout: 5000 })
    expect(screen.getByText('Rosson is on v0.46.3')).toBeTruthy()
    expect(row().textContent).not.toMatch(/failed|Couldn't/i)

    // Every handshake read carried the host's own token (never a stranger's view).
    expect(remote.bootUrls.length).toBeGreaterThanOrEqual(3)
    for (const url of remote.bootUrls) {
      expect(url).toBe('https://rosson.k2.dev/boot-status?token=host-session-tok')
    }
  })

  it('compat host (the 0.46.3 default) and a job that says done: success with the version', async () => {
    remote.ghost = false
    scriptUpdate({ check: CHECK_AVAILABLE })
    render(<UpdateHostRow />)
    await clickButton('Check for updates')
    await clickButton('Download')
    await clickButton('Install & restart Rosson')
    await screen.findByText('Updated Rosson to v0.46.3.', undefined, { timeout: 5000 })
  })

  it('bundled-app host: one-shot "Update Host", comeback on the new instance', async () => {
    scriptUpdate({ check: { ...CHECK_AVAILABLE, installKind: 'bundled-app' }, jobAfterRestart: null })
    // Bundled-app hosts restart themselves after `start`; simulate that here.
    const post = cliPost.getMockImplementation()!
    cliPost.mockImplementation(async (scope: unknown, route: string, body: unknown) => {
      if (route === 'daemon/update/start') {
        await post(scope, 'daemon/update/apply', {})
        return { job_id: 'job-1' }
      }
      return post(scope, route, body)
    })
    cliGet.mockImplementation(async (_scope: unknown, route: string) => {
      if (route === 'auth/whoami') return { username: 'rosson', role: 'owner' }
      if (route === 'daemon/update/status') throw new TypeError('Load failed') // the host went away
      throw new Error(`unexpected GET ${route}`)
    })
    render(<UpdateHostRow />)
    await clickButton('Check for updates')
    await clickButton('Update Host')
    await screen.findByText('Updated Rosson to v0.46.3.', undefined, { timeout: 5000 })
    expect(remote.bootUrls.every((u) => u.endsWith('?token=host-session-tok'))).toBe(true)
  })

  it('back on the OLD version after the restart: a host-named failure, not success', async () => {
    remote.ghost = false
    scriptUpdate({ check: CHECK_AVAILABLE, jobAfterRestart: null, backOn: '0.46.2' })
    render(<UpdateHostRow />)
    await clickButton('Check for updates')
    await clickButton('Download')
    await clickButton('Install & restart Rosson')
    await screen.findByText(
      'Update failed on Rosson: it came back on v0.46.2; the update to v0.46.3 did not install.',
      undefined,
      { timeout: 5000 },
    )
  })

  it('a job that failed on the host surfaces its reason', async () => {
    remote.ghost = false
    scriptUpdate({ check: CHECK_AVAILABLE, jobAfterRestart: { phase: 'failed', error: 'boot verifier failed' } })
    render(<UpdateHostRow />)
    await clickButton('Check for updates')
    await clickButton('Download')
    await clickButton('Install & restart Rosson')
    await screen.findByText('Update failed on Rosson: boot verifier failed.', undefined, { timeout: 5000 })
  })

  it('a Member sees the row but cannot check or update', async () => {
    scriptUpdate({ check: CHECK_AVAILABLE })
    cliGet.mockImplementation(async (_s: unknown, route: string) => {
      if (route === 'auth/whoami') return { username: 'm', role: 'member' }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<UpdateHostRow />)
    const btn = await screen.findByRole('button', { name: 'Check for updates' })
    await waitFor(() => expect(cliGet).toHaveBeenCalledWith(expect.anything(), 'auth/whoami'))
    expect((btn as HTMLButtonElement).disabled).toBe(true)
  })
})
