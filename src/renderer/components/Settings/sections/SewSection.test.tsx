// @vitest-environment jsdom
//
// Settings → Cores → Chat Hosting (Sew S32 K27). Daemon mocked at
// `@/lib/daemon-cli`; every route string and body is asserted exactly.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'

type Json = Record<string, unknown>

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  openUrl: vi.fn(),
  sewHandlers: [] as Array<(reason: string) => void>,
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (...a: unknown[]) => h.openUrl(...a) }))
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onSewChanged: (_scope: unknown, fn: (reason: string) => void) => {
    h.sewHandlers.push(fn)
    return () => {
      h.sewHandlers = h.sewHandlers.filter((x) => x !== fn)
    }
  },
}))

import { SewSection, SEW_MANIFEST } from './SewSection'
import { SEW_MAC_BANNER } from './sew-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'

const YOU_OWNER = {
  canManage: true,
  isOwner: true,
  isAdmin: true,
  passport: false,
  canRecoveryKey: true,
  canPurge: true,
  canOpenAsOwner: true,
}

function running(over: Json = {}): Json {
  return {
    ok: true,
    supported: true,
    released: true,
    state: 'running',
    consistent: true,
    pinnedVersion: '0.3.0',
    installedVersion: '0.3.0',
    upgradeAvailable: false,
    installId: 'ins_1',
    identityProvider: 'k2',
    units: { sewDb: { active: 'active' }, sew: { active: 'active' } },
    alarms: [],
    warnings: [],
    recoveryKeySaved: true,
    backup: { ship: { kind: 'none', path: null }, lastOkAt: null, alarms: [] },
    drill: {},
    mirror: { kind: 'none', path: null },
    appUrl: 'https://hearth.box.k2.dev',
    publicUrl: 'https://hearth.box.k2.dev',
    owner: 'set',
    target: { enabled: true },
    purgeConfirmHostname: 'hearth.box.k2.dev',
    you: YOU_OWNER,
    ...over,
  }
}

/** Route table for GETs; unknown routes fail loudly. */
function gets(table: Record<string, unknown | ((params?: Json) => unknown)>): void {
  h.daemonCliGet.mockImplementation(async (route: string, params?: Json) => {
    if (!(route in table)) throw new Error(`unexpected GET ${route}`)
    const v = table[route]
    const out = typeof v === 'function' ? (v as (p?: Json) => unknown)(params) : v
    if (out instanceof Error) throw out
    return out
  })
}

function posts(table: Record<string, unknown | ((body: Json) => unknown)>): void {
  h.daemonCliPost.mockImplementation(async (route: string, body: Json) => {
    if (!(route in table)) throw new Error(`unexpected POST ${route}`)
    const v = table[route]
    const out = typeof v === 'function' ? (v as (b: Json) => unknown)(body) : v
    if (out instanceof Error) throw out
    return out
  })
}

function postBodies(route: string): Json[] {
  return h.daemonCliPost.mock.calls.filter((c) => c[0] === route).map((c) => c[1] as Json)
}

const confirmSpy = vi.fn(async () => true)

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openUrl.mockReset()
  h.sewHandlers = []
  confirmSpy.mockReset()
  confirmSpy.mockResolvedValue(true)
  useConfirmDialogStore.setState({ confirm: confirmSpy })
  useToastStore.setState({ toasts: [] } as never)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

const CHIP: Record<string, string> = {
  Running: 'Running',
  Degraded: 'Degraded',
  'Enable Chat / Voice Hosting': 'Not installed',
  Resume: 'Error',
}

/** Wait until the preflight answered and Enable is live. */
async function enableReady(): Promise<HTMLButtonElement> {
  const b = screen.getByRole('button', { name: 'Enable Chat / Voice Hosting' }) as HTMLButtonElement
  await waitFor(() => expect(b.disabled).toBe(false))
  return b
}

/** Render and wait for the status chip (`label` = the chip, or the wizard
 *  button whose state it implies). */
async function renderReady(label: string): Promise<void> {
  render(<SewSection />)
  const want = CHIP[label] ?? label
  await waitFor(() => expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe(want))
}

describe('SEW_MANIFEST', () => {
  it('every entry is the sew section and chat finds it', () => {
    expect(SEW_MANIFEST.every((e) => e.section === 'sew')).toBe(true)
    expect(SEW_MANIFEST.some((e) => e.keywords?.includes('chat'))).toBe(true)
  })
})

describe('unsupported daemon (Mac sample)', () => {
  it('banner, sample data, every control off, exactly one daemon call', async () => {
    gets({ 'sew/status': { supported: false, state: 'not-installed', you: YOU_OWNER } })
    render(<SewSection />)
    expect(await screen.findByText(SEW_MAC_BANNER)).toBeTruthy()
    // Sample is Degraded with alarms; no wizard.
    expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe('Degraded')
    expect((screen.getByRole('button', { name: 'Disable Chat / Voice Hosting' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Reveal recovery key' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Open Sew as owner' }) as HTMLButtonElement).disabled).toBe(true)
    for (const panel of ['Backups', 'Off-box copies', 'Public URL', 'App Spaces', 'Alarms']) {
      fireEvent.click(screen.getByRole('button', { name: new RegExp(`^${panel}`) }))
    }
    fireEvent.click(screen.getByRole('button', { name: 'Backups' }))
    expect((screen.getByRole('button', { name: 'Back up now' }) as HTMLButtonElement).disabled).toBe(true)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30))
    })
    expect(h.daemonCliGet).toHaveBeenCalledTimes(1)
    expect(h.daemonCliGet).toHaveBeenCalledWith('sew/status', { health: 1 })
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    expect(h.sewHandlers.length).toBe(0)
  })
})

describe('status read', () => {
  it('a 403 shows the no-permission line and no retry', async () => {
    gets({ 'sew/status': new DaemonHttpError('forbidden', 403, 'forbidden') })
    render(<SewSection />)
    expect((await screen.findByTestId('sew-forbidden')).textContent).toBe('Only the box owner or an admin can manage Chat / Voice Hosting.')
    expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull()
  })

  it('another failure shows Retry, which reads again', async () => {
    let n = 0
    gets({ 'sew/status': () => (++n === 1 ? new Error('daemon down') : running()) })
    render(<SewSection />)
    fireEvent.click(await screen.findByRole('button', { name: 'Retry' }))
    await waitFor(() => expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe('Running'))
  })

  it('sew_changed refetches, debounced', async () => {
    let n = 0
    gets({ 'sew/status': () => (++n, running()) })
    await renderReady('Running')
    // The subscription is a passive effect after the first status commit.
    await waitFor(() => expect(h.sewHandlers.length).toBe(1))
    act(() => {
      h.sewHandlers[0]('backup')
      h.sewHandlers[0]('grant-hook')
    })
    await waitFor(() => expect(n).toBe(2))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 300))
    })
    expect(n).toBe(2)
  })
})

describe('enable wizard', () => {
  const notInstalled = (): Json => running({ state: 'not-installed', installedVersion: null, installId: null, units: undefined, owner: 'none' })

  it('runs preflight; a failing hard check blocks Enable; source/existing_data do not', async () => {
    let checks: Json[] = [
      { id: 'os', status: 'pass', detail: 'Ubuntu 24.04', hint: '' },
      { id: 'ram', status: 'fail', detail: '1 GiB', hint: 'needs 2 GiB' },
    ]
    gets({ 'sew/status': notInstalled(), 'sew/preflight': () => ({ ready: false, checks }) })
    await renderReady('Enable Chat / Voice Hosting')
    await screen.findByTestId('sew-check-ram')
    expect(screen.getByTestId('sew-check-ram').dataset.status).toBe('fail')
    expect((screen.getByRole('button', { name: 'Enable Chat / Voice Hosting' }) as HTMLButtonElement).disabled).toBe(true)
    checks = [{ id: 'source', status: 'fail', detail: 'no release', hint: 'pass --tarball' }]
    fireEvent.click(screen.getByRole('button', { name: 'Re-run preflight' }))
    await screen.findByTestId('sew-check-source')
    expect((screen.getByRole('button', { name: 'Enable Chat / Voice Hosting' }) as HTMLButtonElement).disabled).toBe(false)
    // The source check failing opens the tarball input.
    expect(screen.getByLabelText('Release tarball path')).toBeTruthy()
  })

  it('sends identity always, voice and paths only when set', async () => {
    let state = notInstalled()
    gets({ 'sew/status': () => state, 'sew/preflight': { ready: true, checks: [{ id: 'os', status: 'pass', detail: '', hint: '' }] } })
    posts({ 'sew/server/enable': () => ((state = running({ state: 'installing', enableProgress: { steps: {}, current: 'master-key' } })), { ok: true, state: 'installing' }) })
    await renderReady('Enable Chat / Voice Hosting')
    await screen.findByTestId('sew-check-os')
    fireEvent.click(await enableReady())
    await waitFor(() => expect(postBodies('sew/server/enable')).toEqual([{ identity: 'k2' }]))
    expect(await screen.findByTestId('sew-progress')).toBeTruthy()
  })

  it('standalone identity and voice go in the body', async () => {
    gets({ 'sew/status': notInstalled(), 'sew/preflight': { ready: true, checks: [] } })
    posts({ 'sew/server/enable': { ok: true, state: 'installing' } })
    await renderReady('Enable Chat / Voice Hosting')
    await enableReady()
    fireEvent.click(screen.getByLabelText("Sew's own logins"))
    fireEvent.click(screen.getByLabelText('Also set up voice'))
    fireEvent.change(screen.getByLabelText('Voice URL'), { target: { value: 'wss://hearth.box.k2.dev' } })
    fireEvent.click(screen.getByRole('button', { name: 'Advanced' }))
    fireEvent.change(screen.getByLabelText('Release tarball path'), { target: { value: '/tmp/sew.tar.gz' } })
    fireEvent.click(screen.getByRole('button', { name: 'Enable Chat / Voice Hosting' }))
    await waitFor(() =>
      expect(postBodies('sew/server/enable')).toEqual([
        { identity: 'standalone', tarballPath: '/tmp/sew.tar.gz', voice: { publicUrl: 'wss://hearth.box.k2.dev' } },
      ]),
    )
  })

  it('a typed tarball path is still sent after the preflight it fixed passes', async () => {
    // Live bug (10-29): source failed → the path field opened → the person
    // typed a path and re-ran preflight → source passed → the field hid and
    // Enable dropped the path, so the daemon refused "no Sew release source".
    const preflightParams: Array<Json | undefined> = []
    gets({
      'sew/status': notInstalled(),
      'sew/preflight': (params?: Json) => {
        preflightParams.push(params)
        const given = typeof params?.tarballPath === 'string'
        return {
          ready: given,
          checks: [
            given
              ? { id: 'source', status: 'ok', detail: `--tarball ${params?.tarballPath as string}`, hint: '' }
              : { id: 'source', status: 'fail', detail: 'no release', hint: 'k2 hostchat enable --tarball …' },
          ],
        }
      },
    })
    posts({ 'sew/server/enable': { ok: true, state: 'installing' } })
    await renderReady('Enable Chat / Voice Hosting')
    await waitFor(() => expect(screen.getByTestId('sew-check-source').dataset.status).toBe('fail'))
    fireEvent.change(screen.getByLabelText('Release tarball path'), { target: { value: '/home/k2/sew-0.5.0.tar.gz' } })
    fireEvent.click(screen.getByRole('button', { name: 'Re-run preflight' }))
    await waitFor(() => expect(screen.getByTestId('sew-check-source').dataset.status).toBe('ok'))
    expect(preflightParams).toEqual([undefined, { tarballPath: '/home/k2/sew-0.5.0.tar.gz' }])
    // Advanced was never opened; the typed path stays in view.
    expect(screen.getByRole('button', { name: 'Advanced' })).toBeTruthy()
    expect((screen.getByLabelText('Release tarball path') as HTMLInputElement).value).toBe('/home/k2/sew-0.5.0.tar.gz')
    fireEvent.click(await enableReady())
    await waitFor(() =>
      expect(postBodies('sew/server/enable')).toEqual([{ identity: 'k2', tarballPath: '/home/k2/sew-0.5.0.tar.gz' }]),
    )
  })

  it('shows the release download beside the download step while it runs', async () => {
    const download = {
      file: 'sew-0.5.0-x86_64-unknown-linux-gnu.tar.gz',
      label: 'Sew 0.5.0',
      source: 'download',
      bytes: 12_300_000,
      total: 24_667_386,
    }
    gets({
      'sew/status': running({ state: 'installing', enableProgress: { steps: { 'master-key': { at: 1 } }, current: 'stage', download } }),
      'sew/preflight': { ready: true, checks: [] },
    })
    render(<SewSection />)
    expect((await screen.findByTestId('sew-download')).textContent).toBe('Sew 0.5.0: 12.3 of 24.7 MB')
    expect(screen.getByTestId('sew-step-stage').dataset.step).toBe('current')
  })

  it('a refusal shows the daemon hint', async () => {
    gets({ 'sew/status': notInstalled(), 'sew/preflight': { ready: true, checks: [] } })
    posts({ 'sew/server/enable': new DaemonHttpError('Another Sew action', 409, 'lifecycle_in_progress') })
    await renderReady('Enable Chat / Voice Hosting')
    fireEvent.click(await enableReady())
    expect(await screen.findByText('Another Chat / Voice Hosting action is running. Wait for it to finish.')).toBeTruthy()
  })

  it('while installing, status polls every 2 s and stops when it leaves', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: false })
    let n = 0
    gets({
      'sew/status': () => {
        n++
        return n < 3
          ? running({ state: 'installing', enableProgress: { steps: { 'master-key': { at: 1 }, stage: { at: 2 } }, current: 'install-bin' } })
          : running()
      },
    })
    render(<SewSection />)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    expect(screen.getByTestId('sew-step-master-key').dataset.step).toBe('done')
    expect(screen.getByTestId('sew-step-install-bin').dataset.step).toBe('current')
    expect(screen.getByTestId('sew-step-link').dataset.step).toBe('todo')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(n).toBe(2)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(n).toBe(3)
    expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe('Running')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10_000)
    })
    expect(n).toBe(3)
  })

  it('an error marks the current step failed and shows lastError', async () => {
    gets({
      'sew/status': running({ state: 'error', lastError: 'db: sew-db.service failed', enableProgress: { steps: { 'master-key': { at: 1 } }, current: 'db' } }),
      'sew/preflight': { ready: true, checks: [] },
    })
    await renderReady('Resume')
    expect(screen.getByTestId('sew-step-db').dataset.step).toBe('failed')
    expect(screen.getByText('db: sew-db.service failed')).toBeTruthy()
  })
})

describe('recovery key (person-only)', () => {
  it('confirm → vault password → reveal → Done needs the checkbox; the key never reaches a log or toast', async () => {
    const SECRET = 'AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde'
    const logs: unknown[] = []
    const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((k) =>
      vi.spyOn(console, k).mockImplementation((...a: unknown[]) => void logs.push(...a)),
    )
    let status = running({ recoveryKeySaved: false, alarms: ['k2_recovery_key_not_saved'] })
    gets({ 'sew/status': () => status })
    posts({
      'vault/confirm/start': { methods: ['vault-password'], expiresIn: 60 },
      'vault/confirm/finish': { token: 'k2vc_tok', expiresIn: 60 },
      'sew/server/recovery-key': () => ((status = running()), { ok: true, recoveryKey: SECRET, warning: 'w' }),
    })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Reveal recovery key' }))
    expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ title: 'Reveal the recovery key?', confirmLabel: 'Reveal' }))
    const pw = (await screen.findByLabelText('Confirm password')) as HTMLInputElement
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    expect((await screen.findByTestId('sew-once-secret-value')).textContent).toBe(SECRET)
    expect(postBodies('vault/confirm/start')).toEqual([{ action: 'sew-recovery-key', secretId: 'ins_1' }])
    expect(postBodies('vault/confirm/finish')).toEqual([
      { action: 'sew-recovery-key', secretId: 'ins_1', method: 'vault-password', label: 'reveal the Sew recovery key', password: 'hunter2' },
    ])
    expect(postBodies('sew/server/recovery-key')).toEqual([{ confirmToken: 'k2vc_tok' }])
    const done = screen.getByRole('button', { name: 'Done' }) as HTMLButtonElement
    expect(done.disabled).toBe(true)
    fireEvent.click(screen.getByLabelText('I stored it off this machine'))
    expect(done.disabled).toBe(false)
    fireEvent.click(done)
    await waitFor(() => expect(screen.queryByTestId('sew-once-secret')).toBeNull())
    expect(document.body.innerHTML).not.toContain(SECRET)
    const toasts = JSON.stringify(useToastStore.getState())
    expect(toasts).not.toContain(SECRET)
    expect(JSON.stringify(logs)).not.toContain(SECRET)
    spies.forEach((s) => s.mockRestore())
  })

  it('Touch ID (local-auth) needs no password; unmount drops the key', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth', 'vault-password'] },
      'vault/confirm/finish': { token: 'k2vc_mac' },
      'sew/server/recovery-key': { ok: true, recoveryKey: 'KEY-ON-MAC', warning: 'w' },
    })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Reveal recovery key' }))
    expect(await screen.findByText('KEY-ON-MAC')).toBeTruthy()
    expect(postBodies('vault/confirm/finish')[0].method).toBe('local-auth')
    cleanup()
    expect(document.body.innerHTML).not.toContain('KEY-ON-MAC')
  })

  it('a caller the daemon says may not see the key gets a disabled button', async () => {
    gets({ 'sew/status': running({ you: { ...YOU_OWNER, canRecoveryKey: false } }) })
    await renderReady('Running')
    expect((screen.getByRole('button', { name: 'Reveal recovery key' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('cancelling the dialog makes no call', async () => {
    confirmSpy.mockResolvedValue(false)
    gets({ 'sew/status': running() })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Reveal recovery key' }))
    await waitFor(() => expect(confirmSpy).toHaveBeenCalled())
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })
})

describe('danger zone', () => {
  it('purge needs the exact hostname typed back, a confirm, and a person confirm', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 'k2vc_purge' },
      'sew/server/uninstall': { ok: true, warning: 'Off-box backups need the recovery key.' },
    })
    await renderReady('Running')
    fireEvent.click(screen.getByLabelText('Also delete all chat and voice data'))
    const go = screen.getByRole('button', { name: 'Uninstall and delete data' }) as HTMLButtonElement
    expect(go.disabled).toBe(true)
    const typed = screen.getByLabelText('Type hearth.box.k2.dev to confirm')
    fireEvent.change(typed, { target: { value: 'HEARTH.box.k2.dev' } })
    expect(go.disabled).toBe(true)
    fireEvent.change(typed, { target: { value: ' hearth.box.k2.dev ' } })
    expect(go.disabled).toBe(false)
    fireEvent.click(go)
    await waitFor(() =>
      expect(postBodies('sew/server/uninstall')).toEqual([
        { purge: true, confirmHostname: 'hearth.box.k2.dev', confirmToken: 'k2vc_purge', keepBackups: true },
      ]),
    )
    expect(postBodies('vault/confirm/start')).toEqual([{ action: 'sew-purge', secretId: 'ins_1' }])
    expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ destructive: true, title: 'Delete all chat and voice data?' }))
  })

  it('purge is off for a caller who is not the owner', async () => {
    gets({ 'sew/status': running({ you: { ...YOU_OWNER, canPurge: false, isOwner: false } }) })
    await renderReady('Running')
    expect((screen.getByLabelText('Also delete all chat and voice data') as HTMLInputElement).disabled).toBe(true)
  })

  it('disable and uninstall confirm first; uninstall can drop grants', async () => {
    gets({ 'sew/status': running() })
    posts({ 'sew/server/disable': { ok: true }, 'sew/server/uninstall': { ok: true } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Disable Chat / Voice Hosting' }))
    await waitFor(() => expect(postBodies('sew/server/disable')).toEqual([{}]))
    expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ title: 'Disable Chat / Voice Hosting?', destructive: true }))
    fireEvent.click(screen.getByLabelText("Also remove everyone's chat and voice access"))
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall Chat / Voice Hosting' }))
    await waitFor(() => expect(postBodies('sew/server/uninstall')).toEqual([{ dropGrants: true }]))
  })

  it('canManage false disables every mutation', async () => {
    gets({ 'sew/status': running({ upgradeAvailable: true, pinnedVersion: '0.4.0', you: { ...YOU_OWNER, canManage: false } }) })
    await renderReady('Running')
    for (const name of ['Disable Chat / Voice Hosting', 'Uninstall Chat / Voice Hosting', 'Upgrade to 0.4.0', 'Edit']) {
      expect((screen.getByRole('button', { name }) as HTMLButtonElement).disabled, name).toBe(true)
    }
    fireEvent.click(screen.getByRole('button', { name: 'Backups' }))
    gets({ 'sew/status': running(), 'sew/backups': { backups: [] } })
    expect((await screen.findByRole('button', { name: 'Back up now' }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('upgrade', () => {
  it('only when available, after a confirm', async () => {
    gets({ 'sew/status': running({ upgradeAvailable: true, pinnedVersion: '0.4.0', onDiskVersion: '0.3.0' }) })
    posts({ 'sew/server/upgrade': { ok: true, state: 'upgrading' } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Upgrade to 0.4.0' }))
    await waitFor(() => expect(postBodies('sew/server/upgrade')).toEqual([{}]))
  })

  it('a rolled-back upgrade shows its callout', async () => {
    gets({ 'sew/status': running({ upgradeProgress: { result: 'rolled_back' }, lastError: 'migrate failed' }) })
    await renderReady('Running')
    expect(screen.getByText(/The upgrade rolled back: migrate failed/)).toBeTruthy()
  })
})

describe('open Sew as owner', () => {
  it('confirms, mints, opens the fragment URL; the code is never rendered', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 'k2vc_ho' },
      'sew/owner-handoff': { ok: true, code: 'k2sho_SECRETCODE', expiresInMs: 60000 },
    })
    h.openUrl.mockResolvedValue(undefined)
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Open Sew as owner' }))
    await waitFor(() => expect(h.openUrl).toHaveBeenCalledTimes(1))
    expect(h.openUrl).toHaveBeenCalledWith('https://hearth.box.k2.dev/k2/owner-handoff#code=k2sho_SECRETCODE')
    expect(String(h.openUrl.mock.calls[0][0])).not.toContain('?code')
    expect(postBodies('vault/confirm/start')).toEqual([{ action: 'chat-owner-handoff' }])
    expect(postBodies('sew/owner-handoff')).toEqual([{ confirmToken: 'k2vc_ho' }])
    expect(document.body.innerHTML).not.toContain('k2sho_SECRETCODE')
    expect(JSON.stringify(useToastStore.getState())).not.toContain('k2sho_SECRETCODE')
  })

  it('a browser that will not open: a toast without the code', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 't' },
      'sew/owner-handoff': { ok: true, code: 'k2sho_X', expiresInMs: 60000 },
    })
    h.openUrl.mockRejectedValue(new Error('no browser'))
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Open Sew as owner' }))
    await waitFor(() => expect(JSON.stringify(useToastStore.getState())).toContain('Could not open your browser.'))
    expect(JSON.stringify(useToastStore.getState())).not.toContain('k2sho_X')
  })

  it('disabled without an app URL or while the owner is not set', async () => {
    gets({ 'sew/status': running({ appUrl: null }) })
    await renderReady('Running')
    expect((screen.getByRole('button', { name: 'Open Sew as owner' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText("Set Sew's app URL first.")).toBeTruthy()
    cleanup()
    gets({ 'sew/status': running({ owner: 'pending' }) })
    await renderReady('Running')
    expect((screen.getByRole('button', { name: 'Open Sew as owner' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('app URL: save and clear', async () => {
    gets({ 'sew/status': running() })
    posts({ 'sew/server/app-url': { ok: true, appUrl: null } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }))
    await waitFor(() => expect(postBodies('sew/server/app-url')).toEqual([{ url: '' }]))
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    fireEvent.change(screen.getByLabelText('Sew app URL'), { target: { value: ' https://chat.box.k2.dev ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(postBodies('sew/server/app-url')[1]).toEqual({ url: 'https://chat.box.k2.dev' }))
  })
})

describe('panels', () => {
  it('alarms: every alarm with its fix and action', async () => {
    gets({ 'sew/status': running({ state: 'degraded', alarms: ['backup_failed', 'k2_backup_offbox_none', 'mystery_alarm'] }) })
    await renderReady('Degraded')
    fireEvent.click(screen.getByRole('button', { name: /^Alarms/ }))
    expect(screen.getByTestId('sew-alarm-backup_failed').textContent).toContain('The last backup failed')
    expect(screen.getByTestId('sew-alarm-mystery_alarm').textContent).toContain('Run k2 hostchat status for details.')
    fireEvent.click(screen.getAllByRole('button', { name: 'Go to Off-box copies' })[0])
    expect(screen.getByText('Backup destination')).toBeTruthy()
  })

  it('backups: back up now, then the list refreshes', async () => {
    let list: Json[] = []
    gets({ 'sew/status': running(), 'sew/backups': () => ({ backups: list }) })
    posts({
      'sew/server/backup': () => {
        list = [{ id: '20261020T000000Z', kind: 'manual', created_at: '2026-10-20T00:00:00Z', bytes: 1048576 }]
        return { ok: true, jobId: 'job1', backup: { id: '20261020T000000Z' } }
      },
    })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Backups' }))
    fireEvent.click(await screen.findByRole('button', { name: 'Back up now' }))
    expect(await screen.findByText('20261020T000000Z')).toBeTruthy()
    expect(postBodies('sew/server/backup')).toEqual([{}])
  })

  it('backups: a job without a result is polled until it ends', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: false })
    let jobState = 'running'
    gets({ 'sew/status': running(), 'sew/backups': { backups: [] }, 'sew/job': () => ({ id: 'j2', kind: 'backup', state: jobState }) })
    posts({ 'sew/server/backup': { ok: true, jobId: 'j2' } })
    render(<SewSection />)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Backups' }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    fireEvent.click(screen.getByRole('button', { name: 'Back up now' }))
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10)
    })
    expect(postBodies('sew/server/backup')).toEqual([{}])
    expect(screen.getByRole('button', { name: 'Backing up…' })).toBeTruthy()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    const jobCalls = (): number => h.daemonCliGet.mock.calls.filter((c) => c[0] === 'sew/job').length
    expect(jobCalls()).toBe(1)
    expect(h.daemonCliGet).toHaveBeenCalledWith('sew/job', { id: 'j2' })
    jobState = 'succeeded'
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000)
    })
    expect(jobCalls()).toBe(2)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(6000)
    })
    expect(jobCalls()).toBe(2)
  })

  it('off-box: ship and mirror send kind and path; the mirror confirms the restart', async () => {
    gets({ 'sew/status': running() })
    posts({ 'sew/server/ship': { ok: true }, 'sew/server/mirror': { ok: true, restarted: true } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Off-box copies' }))
    fireEvent.click(screen.getByLabelText('Backup destination: a folder on another disk'))
    fireEvent.change(screen.getByLabelText('Backup destination path'), { target: { value: 'mnt/rel' } })
    expect(screen.getByText('Use an absolute path.')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Backup destination path'), { target: { value: ' /mnt/offbox/sew ' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[0])
    await waitFor(() => expect(postBodies('sew/server/ship')).toEqual([{ kind: 'dir', path: '/mnt/offbox/sew' }]))
    fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[1])
    await waitFor(() => expect(postBodies('sew/server/mirror')).toEqual([{ kind: 'none' }]))
    expect(confirmSpy).toHaveBeenCalledWith(expect.objectContaining({ message: 'Sew restarts to apply this. Sessions reconnect on their own.' }))
    expect((screen.getByLabelText('Keyring mirror: Alakazam escrow') as HTMLInputElement).disabled).toBe(true)
  })

  it('public URL posts config publicUrl', async () => {
    gets({ 'sew/status': running({ publicUrl: null, warnings: [{ code: 'public_url_unset', message: 'Set a public URL' }] }) })
    posts({ 'sew/server/config': { ok: true } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Public URL' }))
    expect(screen.getAllByText('Set a public URL').length).toBeGreaterThan(0)
    fireEvent.change(screen.getByLabelText('Sew public URL'), { target: { value: 'https://hearth.box.k2.dev' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(postBodies('sew/server/config')).toEqual([{ publicUrl: 'https://hearth.box.k2.dev' }]))
  })

  it('App Spaces: list, bind, unbind; the copy says Admin Access adds nobody', async () => {
    let spaces: Json[] = [{ app: 'hearth', spaceId: 'spc_1', boundAt: 1, members: { added: 3, held: 1, pending: 0 } }]
    gets({ 'sew/status': running(), 'sew/app-spaces': () => ({ ok: true, spaces }) })
    posts({
      'sew/app-spaces/bind': (b: Json) => ((spaces = [...spaces, { app: b.app, spaceId: b.spaceId, members: { added: 0, held: 0, pending: 0 } }]), { ok: true }),
      'sew/app-spaces/unbind': { ok: true },
    })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'App Spaces' }))
    expect(screen.getByText("Members come only from each App's user grants; Admin Access never adds anyone.")).toBeTruthy()
    expect(await screen.findByText('3 in · 0 pending · 1 held')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('App id'), { target: { value: 'meet' } })
    fireEvent.change(screen.getByLabelText('Space id'), { target: { value: 'spc_2' } })
    fireEvent.click(screen.getByRole('button', { name: 'Bind' }))
    await waitFor(() => expect(postBodies('sew/app-spaces/bind')).toEqual([{ app: 'meet', spaceId: 'spc_2' }]))
    expect(await screen.findByText('spc_2')).toBeTruthy()
    // S32 Q3: unbinding is a choice; the default keeps everyone.
    expect(screen.getByText('Unbinding keeps everyone by default. You can instead remove the members K2 added; people added by hand always stay.')).toBeTruthy()
    fireEvent.click(screen.getAllByRole('button', { name: 'Unbind' })[0])
    expect(postBodies('sew/app-spaces/unbind')).toEqual([])
    fireEvent.click(screen.getByRole('button', { name: 'Unbind, keep members' }))
    await waitFor(() => expect(postBodies('sew/app-spaces/unbind')).toEqual([{ app: 'hearth', spaceId: 'spc_1' }]))
    await waitFor(() => expect(screen.queryByTestId('sew-unbind-choice')).toBeNull())
    fireEvent.click(screen.getAllByRole('button', { name: 'Unbind' })[0])
    fireEvent.click(screen.getByRole('button', { name: 'Unbind and remove members K2 added' }))
    await waitFor(() =>
      expect(postBodies('sew/app-spaces/unbind')).toEqual([
        { app: 'hearth', spaceId: 'spc_1' },
        { app: 'hearth', spaceId: 'spc_1', removeAppMembers: true },
      ]),
    )
  })

  it('App Spaces on an older daemon (404) say so and offer no bind', async () => {
    gets({ 'sew/status': running(), 'sew/app-spaces': new DaemonHttpError('no such Sew route', 404, 'not_found') })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'App Spaces' }))
    expect(await screen.findByText('This server does not report App Spaces yet.')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Bind' })).toBeNull()
  })

  it('S30/S31 stubs render their note and make no calls', async () => {
    gets({ 'sew/status': running() })
    await renderReady('Running')
    const before = h.daemonCliGet.mock.calls.length
    for (const [nav, id] of [['Agents', 'sew-stub-agents'], ['Approvals', 'sew-stub-approvals'], ['Deliveries', 'sew-stub-deliveries']]) {
      fireEvent.click(screen.getByRole('button', { name: nav }))
      expect(screen.getByTestId(id).textContent).toContain('Arrives with k2 chat (S30/S31).')
    }
    expect(h.daemonCliGet.mock.calls.length).toBe(before)
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('panels other than the server wait for a live install', async () => {
    gets({ 'sew/status': running({ state: 'not-installed', owner: 'none' }), 'sew/preflight': { ready: true, checks: [] } })
    await renderReady('Enable Chat / Voice Hosting')
    for (const name of [/^Alarms/, 'Backups', 'Off-box copies', 'Public URL', 'App Spaces']) {
      expect((screen.getByRole('button', { name }) as HTMLButtonElement).disabled).toBe(true)
    }
  })
})

describe('source rules', () => {
  it('no Sew file fetches projects', () => {
    const dir = dirname(fileURLToPath(import.meta.url))
    const files = readdirSync(dir).filter((f) => /^(sew-|Sew)/.test(f) && !f.includes('.test.'))
    expect(files.length).toBeGreaterThanOrEqual(5)
    for (const f of files) {
      // Code only: comments may say "never fetchProjects()".
      const src = readFileSync(join(dir, f), 'utf8')
        .replace(/\/\*[\s\S]*?\*\//g, '')
        .replace(/^\s*\/\/.*$/gm, '')
      expect(src, f).not.toMatch(/fetchProjects\s*\(/)
      expect(src, f).not.toContain("'projects/list'")
    }
  })
})
