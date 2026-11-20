// @vitest-environment jsdom
//
// Settings → Cores → Chat Hosting (Sew S32 K27). Daemon mocked at
// `@/lib/daemon-cli`; every route string and body is asserted exactly.

import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { DaemonHttpError } from '@/lib/daemon-http-error'

type Json = Record<string, unknown>

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  openUrl: vi.fn(),
  openOwnerLink: vi.fn(),
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
// The owner link goes to the app's open_owner_link (lib/owner-link.ts);
// every other command keeps the real invoke.
vi.mock('@tauri-apps/api/core', async (orig) => {
  const real = await orig<typeof import('@tauri-apps/api/core')>()
  return {
    ...real,
    invoke: (cmd: string, args?: Record<string, unknown>) =>
      cmd === 'open_owner_link' ? h.openOwnerLink(args) : real.invoke(cmd, args),
  }
})
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
import { SEW_NO_PERMISSION } from './sew-api'
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

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openUrl.mockReset()
  h.openOwnerLink.mockReset()
  h.sewHandlers = []
  useToastStore.setState({ toasts: [] } as never)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

const CHIPS = new Set(['Running', 'Degraded', 'Setting up…', 'Error', 'Turned off', 'Uninstalled, data kept'])

/** Wait until the preflight answered and setup is live. */
async function enableReady(): Promise<HTMLButtonElement> {
  const b = screen.getByRole('button', { name: 'Set up chat and voice' }) as HTMLButtonElement
  await waitFor(() => expect(b.disabled).toBe(false))
  return b
}

/** Render and wait for the status word, or (not installed / error) the
 *  setup button that state shows. */
async function renderReady(label: string): Promise<void> {
  render(<SewSection />)
  if (CHIPS.has(label)) await waitFor(() => expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe(label))
  else await screen.findByRole('button', { name: label })
}

const tab = (name: string): void => {
  fireEvent.click(screen.getByRole('tab', { name: new RegExp(`^${name}`) }))
}

/** Answer the page's confirm-first question with its confirm button. */
async function answer(title: string, button: string): Promise<void> {
  const sheet = await screen.findByTestId('sew-ask')
  expect(sheet.textContent).toContain(title)
  fireEvent.click(within(sheet).getByRole('button', { name: button }))
}

describe('SEW_MANIFEST', () => {
  it('every entry is the sew section and chat finds it', () => {
    expect(SEW_MANIFEST.every((e) => e.section === 'sew')).toBe(true)
    expect(SEW_MANIFEST.some((e) => e.keywords?.includes('chat'))).toBe(true)
  })
})

describe('unsupported daemon (Mac sample)', () => {
  it('the Linux note, sample data, every control off, exactly one daemon call', async () => {
    gets({ 'sew/status': { supported: false, state: 'not-installed', you: YOU_OWNER } })
    render(<SewSection />)
    expect((await screen.findByTestId('linux-only-note')).textContent).toContain('Chat and voice runs on Linux servers.')
    expect(document.body.textContent).not.toMatch(/ONLY RUNS ON LINUX|EXAMPLE PURPOSES/)
    // Sample is Degraded with alarms; no setup.
    expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe('Degraded')
    expect((screen.getByRole('button', { name: 'Turn off…' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Uninstall…' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Open as owner' }) as HTMLButtonElement).disabled).toBe(true)
    for (const t of ['Backups and keys', 'App Spaces', 'Recording', 'Overview']) tab(t)
    tab('Backups and keys')
    expect((screen.getByRole('button', { name: 'Back up now' }) as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByRole('button', { name: 'Show recovery key' }) as HTMLButtonElement).disabled).toBe(true)
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
  it('a 403 shows the no-permission card and no retry', async () => {
    gets({ 'sew/status': new DaemonHttpError('forbidden', 403, 'forbidden') })
    render(<SewSection />)
    expect((await screen.findByTestId('sew-forbidden')).textContent).toContain(SEW_NO_PERMISSION)
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull()
    // It names who to ask and a next step.
    expect(screen.getByTestId('sew-forbidden').textContent).toContain('Ask the server’s owner or an admin')
    expect(screen.getByRole('button', { name: 'See who’s an admin' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Check again' })).toBeTruthy()
  })

  it('another failure shows the error card with Try again, which reads again', async () => {
    let n = 0
    gets({ 'sew/status': () => (++n === 1 ? new Error('daemon down') : running()) })
    render(<SewSection />)
    const card = await screen.findByTestId('sew-load-error')
    expect(card.textContent).toContain('Couldn’t read chat and voice status')
    expect(card.textContent).toContain('The server said: daemon down.')
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(screen.getAllByTestId('sew-state-chip')[0].textContent).toBe('Running'))
  })

  it('a reply without supported / state is a failed read, never a Mac', async () => {
    gets({ 'sew/status': {} })
    render(<SewSection />)
    expect((await screen.findByTestId('sew-load-error')).textContent).toContain('the reply had no status in it')
    expect(screen.queryByTestId('linux-only-note')).toBeNull()
    expect(document.body.textContent).not.toContain('undefined')
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

describe('setup', () => {
  const notInstalled = (): Json => running({ state: 'not-installed', installedVersion: null, installId: null, units: undefined, owner: 'none' })

  it('says it is not set up, runs the checks; a failing hard check blocks setup; source/existing_data do not', async () => {
    let checks: Json[] = [
      { id: 'os', status: 'pass', detail: 'Ubuntu 24.04', hint: '' },
      { id: 'ram', status: 'fail', detail: '1 GiB', hint: 'needs 2 GiB' },
    ]
    gets({ 'sew/status': notInstalled(), 'sew/preflight': () => ({ ready: false, checks }) })
    await renderReady('Set up chat and voice')
    expect(screen.getByTestId('sew-not-installed').textContent).toContain('Chat and voice isn’t set up yet')
    await screen.findByTestId('sew-check-ram')
    expect(screen.getByTestId('sew-check-ram').dataset.status).toBe('fail')
    expect(screen.getByTestId('sew-check-ram').textContent).toContain('Needs fixing')
    expect((screen.getByRole('button', { name: 'Set up chat and voice' }) as HTMLButtonElement).disabled).toBe(true)
    checks = [{ id: 'source', status: 'fail', detail: 'no release', hint: 'pass --tarball' }]
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    await screen.findByTestId('sew-check-source')
    expect((screen.getByRole('button', { name: 'Set up chat and voice' }) as HTMLButtonElement).disabled).toBe(false)
    // The source check failing opens the release file field.
    expect(screen.getByLabelText('Chat release file')).toBeTruthy()
  })

  it('sends identity always, voice and paths only when set', async () => {
    let state = notInstalled()
    gets({ 'sew/status': () => state, 'sew/preflight': { ready: true, checks: [{ id: 'os', status: 'pass', detail: '', hint: '' }] } })
    posts({ 'sew/server/enable': () => ((state = running({ state: 'installing', enableProgress: { steps: {}, current: 'master-key' } })), { ok: true, state: 'installing' }) })
    await renderReady('Set up chat and voice')
    await screen.findByTestId('sew-check-os')
    fireEvent.click(await enableReady())
    await waitFor(() => expect(postBodies('sew/server/enable')).toEqual([{ identity: 'k2' }]))
    expect(await screen.findByTestId('sew-progress')).toBeTruthy()
  })

  it('standalone identity and voice go in the body', async () => {
    gets({ 'sew/status': notInstalled(), 'sew/preflight': { ready: true, checks: [] } })
    posts({ 'sew/server/enable': { ok: true, state: 'installing' } })
    await renderReady('Set up chat and voice')
    await enableReady()
    fireEvent.click(screen.getByLabelText('Chat’s own accounts'))
    fireEvent.click(screen.getByRole('switch', { name: 'Also set up voice calls' }))
    fireEvent.change(screen.getByLabelText('Voice address'), { target: { value: 'wss://hearth.box.k2.dev' } })
    fireEvent.click(screen.getByRole('button', { name: 'Show' }))
    fireEvent.change(screen.getByLabelText('Chat release file'), { target: { value: '/tmp/sew.tar.gz' } })
    fireEvent.click(screen.getByRole('button', { name: 'Set up chat and voice' }))
    await waitFor(() =>
      expect(postBodies('sew/server/enable')).toEqual([
        { identity: 'standalone', tarballPath: '/tmp/sew.tar.gz', voice: { publicUrl: 'wss://hearth.box.k2.dev' } },
      ]),
    )
  })

  it('a typed release file is still sent after the check it fixed passes', async () => {
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
    await renderReady('Set up chat and voice')
    await waitFor(() => expect(screen.getByTestId('sew-check-source').dataset.status).toBe('fail'))
    fireEvent.change(screen.getByLabelText('Chat release file'), { target: { value: '/home/k2/sew-0.5.0.tar.gz' } })
    fireEvent.click(screen.getByRole('button', { name: 'Check again' }))
    await waitFor(() => expect(screen.getByTestId('sew-check-source').dataset.status).toBe('ok'))
    expect(preflightParams).toEqual([undefined, { tarballPath: '/home/k2/sew-0.5.0.tar.gz' }])
    // "Install from files" was never opened; the typed path stays in view.
    expect(screen.getByRole('button', { name: 'Show' }).getAttribute('aria-expanded')).toBe('false')
    expect((screen.getByLabelText('Chat release file') as HTMLInputElement).value).toBe('/home/k2/sew-0.5.0.tar.gz')
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

  it('a refusal says setup did not start, with the daemon reason', async () => {
    gets({ 'sew/status': notInstalled(), 'sew/preflight': { ready: true, checks: [] } })
    posts({ 'sew/server/enable': new DaemonHttpError('Another Sew action', 409, 'lifecycle_in_progress') })
    await renderReady('Set up chat and voice')
    fireEvent.click(await enableReady())
    const note = await screen.findByTestId('dialog-error')
    // The daemon's code reads as a plain line; its own words follow quietly.
    expect(note.textContent).toContain('Another chat change is still running.')
    expect(note.textContent).toContain('Wait for it to finish, then try again.')
    expect(note.textContent).toContain('The server said: Another Sew action.')
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
    await renderReady('Resume setup')
    expect(screen.getByTestId('sew-step-db').dataset.step).toBe('failed')
    expect(screen.getByText(/The server said: db: sew-db.service failed\./)).toBeTruthy()
  })
})

describe('recovery key (person-only)', () => {
  it('ask → vault password → reveal → Done (nothing to tick: the key is in the vault); the key never reaches a log or toast', async () => {
    const SECRET = 'AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-abcde'
    const logs: unknown[] = []
    const spies = (['log', 'info', 'warn', 'error', 'debug'] as const).map((k) =>
      vi.spyOn(console, k).mockImplementation((...a: unknown[]) => void logs.push(...a)),
    )
    const status = running({ recoveryKeySaved: true, recoveryKeyInVault: true, recoveryKeyVaultName: 'sew-recovery-key' })
    gets({ 'sew/status': () => status, 'sew/backups': { backups: [] } })
    posts({
      'vault/confirm/start': { methods: ['vault-password'], expiresIn: 60 },
      'vault/confirm/finish': { token: 'k2vc_tok', expiresIn: 60 },
      'sew/server/recovery-key': { ok: true, recoveryKey: SECRET, warning: 'w' },
    })
    await renderReady('Running')
    tab('Backups and keys')
    // Rosson 2026-10-30: one line says where it is; nothing asks to save it.
    expect(screen.getByTestId('sew-recovery-in-vault').textContent).toContain('in the K2 vault')
    expect(document.body.textContent).not.toContain('Save your recovery key')
    fireEvent.click(screen.getByRole('button', { name: 'Show recovery key' }))
    await answer('Show the recovery key?', 'Continue')
    const pw = (await screen.findByLabelText('Confirm password')) as HTMLInputElement
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    expect((await screen.findByTestId('sew-once-secret-value')).textContent).toBe(SECRET)
    expect(postBodies('vault/confirm/start')).toEqual([{ action: 'sew-recovery-key', secretId: 'ins_1' }])
    expect(postBodies('vault/confirm/finish')).toEqual([
      { action: 'sew-recovery-key', secretId: 'ins_1', method: 'vault-password', label: 'show the chat recovery key', password: 'hunter2' },
    ])
    expect(postBodies('sew/server/recovery-key')).toEqual([{ confirmToken: 'k2vc_tok' }])
    const done = screen.getByRole('button', { name: 'Done' }) as HTMLButtonElement
    expect(screen.queryByLabelText('I stored it off this machine')).toBeNull()
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
    gets({ 'sew/status': running(), 'sew/backups': { backups: [] } })
    posts({
      'vault/confirm/start': { methods: ['local-auth', 'vault-password'] },
      'vault/confirm/finish': { token: 'k2vc_mac' },
      'sew/server/recovery-key': { ok: true, recoveryKey: 'KEY-ON-MAC', warning: 'w' },
    })
    await renderReady('Running')
    tab('Backups and keys')
    fireEvent.click(screen.getByRole('button', { name: 'Show recovery key' }))
    await answer('Show the recovery key?', 'Continue')
    expect(await screen.findByText('KEY-ON-MAC')).toBeTruthy()
    expect(postBodies('vault/confirm/finish')[0].method).toBe('local-auth')
    cleanup()
    expect(document.body.innerHTML).not.toContain('KEY-ON-MAC')
  })

  it('a caller the daemon says may not see the key gets a disabled button', async () => {
    gets({ 'sew/status': running({ you: { ...YOU_OWNER, canRecoveryKey: false } }), 'sew/backups': { backups: [] } })
    await renderReady('Running')
    tab('Backups and keys')
    expect((screen.getByRole('button', { name: 'Show recovery key' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('cancelling the question makes no call', async () => {
    gets({ 'sew/status': running(), 'sew/backups': { backups: [] } })
    await renderReady('Running')
    tab('Backups and keys')
    fireEvent.click(screen.getByRole('button', { name: 'Show recovery key' }))
    await answer('Show the recovery key?', 'Cancel')
    await waitFor(() => expect(screen.queryByTestId('sew-ask')).toBeNull())
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })
})

describe('turn off or uninstall', () => {
  it('purge lives in the uninstall dialog: the exact hostname typed back and a person confirm', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 'k2vc_purge' },
      'sew/server/uninstall': { ok: true, warning: 'Off-box backups need the recovery key.' },
    })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall…' }))
    const dialog = await screen.findByTestId('sew-uninstall-dialog')
    fireEvent.click(within(dialog).getByLabelText('Also delete all chat and voice data'))
    const go = within(dialog).getByRole('button', { name: 'Uninstall and delete data' }) as HTMLButtonElement
    expect(go.disabled).toBe(true)
    const typed = within(dialog).getByLabelText('Type hearth.box.k2.dev to confirm')
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
    expect((await screen.findByTestId('sew-notice')).textContent).toContain('Off-box backups need the recovery key.')
    expect(screen.queryByTestId('sew-uninstall-dialog')).toBeNull()
  })

  it('purge is off for a caller who is not the owner', async () => {
    gets({ 'sew/status': running({ you: { ...YOU_OWNER, canPurge: false, isOwner: false } }) })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall…' }))
    const dialog = await screen.findByTestId('sew-uninstall-dialog')
    expect((within(dialog).getByLabelText('Also delete all chat and voice data') as HTMLInputElement).disabled).toBe(true)
    expect(dialog.textContent).toContain('Only the server owner can delete chat data.')
  })

  it('turn off and uninstall confirm first; uninstall can drop grants; each says what happened', async () => {
    gets({ 'sew/status': running() })
    posts({ 'sew/server/disable': { ok: true }, 'sew/server/uninstall': { ok: true } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Turn off…' }))
    expect(postBodies('sew/server/disable')).toEqual([])
    await answer('Turn off chat and voice?', 'Turn off')
    await waitFor(() => expect(postBodies('sew/server/disable')).toEqual([{}]))
    expect((await screen.findByTestId('sew-notice')).textContent).toContain('Chat and voice are off.')
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall…' }))
    const dialog = await screen.findByTestId('sew-uninstall-dialog')
    fireEvent.click(within(dialog).getByLabelText("Also remove everyone's chat and voice access"))
    fireEvent.click(within(dialog).getByRole('button', { name: 'Uninstall' }))
    await waitFor(() => expect(postBodies('sew/server/uninstall')).toEqual([{ dropGrants: true }]))
  })

  it('a failed turn off says so and keeps the page', async () => {
    gets({ 'sew/status': running() })
    posts({ 'sew/server/disable': new DaemonHttpError('systemctl stop failed', 500, 'unit_failed') })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Turn off…' }))
    await answer('Turn off chat and voice?', 'Turn off')
    const note = await screen.findByTestId('dialog-error')
    expect(note.textContent).toContain('Couldn’t turn off chat and voice.')
    expect(note.textContent).toContain('The server said: systemctl stop failed.')
  })

  it('canManage false disables every change', async () => {
    gets({ 'sew/status': running({ upgradeAvailable: true, pinnedVersion: '0.4.0', you: { ...YOU_OWNER, canManage: false } }), 'sew/backups': { backups: [] } })
    await renderReady('Running')
    expect(screen.getByTestId('sew-read-only')).toBeTruthy()
    for (const name of ['Turn off…', 'Uninstall…', 'Update to 0.4.0']) {
      expect((screen.getByRole('button', { name }) as HTMLButtonElement).disabled, name).toBe(true)
    }
    for (const b of screen.getAllByRole('button', { name: 'Edit' })) expect((b as HTMLButtonElement).disabled).toBe(true)
    tab('Backups and keys')
    expect((await screen.findByRole('button', { name: 'Back up now' }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('status rows follow the state', () => {
  it('a turned-off server never reads as running', async () => {
    gets({ 'sew/status': running({ state: 'disabled' }), 'sew/preflight': { ready: true, checks: [] } })
    await renderReady('Turned off')
    expect(screen.getByTestId('sew-services').textContent).toBe('Turned off')
    expect(screen.getByTestId('sew-version').textContent).toBe('Version 0.3.0')
    expect(document.body.textContent).not.toContain('Database and chat running')
  })

  it('after an uninstall the page is the fresh setup, and its words agree', async () => {
    gets({ 'sew/status': running({ state: 'removed' }), 'sew/preflight': { ready: true, checks: [] } })
    await renderReady('Set up chat and voice')
    expect(screen.getByTestId('sew-removed').textContent).toContain('Set it up again below to bring them back.')
    expect(screen.queryByText('Turn chat and voice back on')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Turn back on' })).toBeNull()
    expect(screen.queryByTestId('sew-services')).toBeNull()
    expect(screen.getByRole('button', { name: 'Delete data…' })).toBeTruthy()
  })

  it('while setup runs, each row waits for its step', async () => {
    gets({ 'sew/status': running({ state: 'installing', installedVersion: null, enableProgress: { steps: { postgres: { at: 1 } }, current: 'stage' } }) })
    await renderReady('Setting up…')
    expect(screen.getByTestId('sew-services').textContent).toBe('Database waiting, chat waiting')
    expect(screen.getByTestId('sew-owner').textContent).toBe('Waiting')
    expect(screen.getByTestId('sew-version').textContent).toBe('Waiting')
  })

  it('no install ID or other raw id is shown', async () => {
    gets({ 'sew/status': running() })
    await renderReady('Running')
    expect(document.body.textContent).not.toContain('ins_1')
  })
})

describe('updates', () => {
  it('only when available, after a confirm', async () => {
    gets({ 'sew/status': running({ upgradeAvailable: true, pinnedVersion: '0.4.0', onDiskVersion: '0.3.0' }) })
    posts({ 'sew/server/upgrade': { ok: true, state: 'upgrading' } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Update to 0.4.0' }))
    await answer('Update chat to 0.4.0?', 'Update')
    await waitFor(() => expect(postBodies('sew/server/upgrade')).toEqual([{}]))
  })

  it('a rolled-back update shows its callout', async () => {
    gets({ 'sew/status': running({ upgradeProgress: { result: 'rolled_back' }, lastError: 'migrate failed' }) })
    await renderReady('Running')
    expect(screen.getByText(/The last update didn’t finish, so chat went back/)).toBeTruthy()
    expect(screen.getByText('The server said: migrate failed.')).toBeTruthy()
  })
})

describe('open chat as owner', () => {
  it('confirms, mints, opens the fragment URL; the code is never rendered', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 'k2vc_ho' },
      'sew/owner-handoff': { ok: true, code: 'k2sho_SECRETCODE', expiresInMs: 60000 },
    })
    h.openOwnerLink.mockResolvedValue(undefined)
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Open as owner' }))
    await waitFor(() => expect(h.openOwnerLink).toHaveBeenCalledTimes(1))
    // The app's open_owner_link gets it (a redirect page, never a command line).
    expect(h.openOwnerLink.mock.calls).toEqual([[{ link: 'https://hearth.box.k2.dev/k2/owner-handoff#code=k2sho_SECRETCODE' }]])
    expect(h.openUrl).not.toHaveBeenCalled()
    expect(postBodies('vault/confirm/start')).toEqual([{ action: 'chat-owner-handoff' }])
    expect(postBodies('sew/owner-handoff')).toEqual([{ confirmToken: 'k2vc_ho' }])
    expect(document.body.innerHTML).not.toContain('k2sho_SECRETCODE')
    expect(JSON.stringify(useToastStore.getState())).not.toContain('k2sho_SECRETCODE')
  })

  it('a browser that will not open: a plain note without the code', async () => {
    gets({ 'sew/status': running() })
    posts({
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 't' },
      'sew/owner-handoff': { ok: true, code: 'k2sho_X', expiresInMs: 60000 },
    })
    h.openOwnerLink.mockRejectedValue('no browser')
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Open as owner' }))
    expect((await screen.findByTestId('dialog-error')).textContent).toContain('Your browser didn’t open')
    expect(document.body.innerHTML).not.toContain('k2sho_X')
    expect(JSON.stringify(useToastStore.getState())).not.toContain('k2sho_X')
  })

  it('disabled without an owner sign-in address or while the owner is not set', async () => {
    gets({ 'sew/status': running({ appUrl: null }) })
    await renderReady('Running')
    expect((screen.getByRole('button', { name: 'Open as owner' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText('Set the owner sign-in address first.')).toBeTruthy()
    cleanup()
    gets({ 'sew/status': running({ owner: 'pending' }) })
    await renderReady('Running')
    expect((screen.getByRole('button', { name: 'Open as owner' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('owner sign-in address: clear, then edit and save', async () => {
    gets({ 'sew/status': running() })
    posts({ 'sew/server/app-url': { ok: true, appUrl: null } })
    await renderReady('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Clear' }))
    await waitFor(() => expect(postBodies('sew/server/app-url')).toEqual([{ url: '' }]))
    fireEvent.click(screen.getAllByRole('button', { name: 'Edit' })[1])
    fireEvent.change(screen.getByLabelText('Owner sign-in address'), { target: { value: ' https://chat.box.k2.dev ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(postBodies('sew/server/app-url')[1]).toEqual({ url: 'https://chat.box.k2.dev' }))
    expect(await screen.findByText('Saved')).toBeTruthy()
  })
})

describe('panels', () => {
  it('alarms: every alarm with its plain fix and a way to it', async () => {
    gets({ 'sew/status': running({ state: 'degraded', alarms: ['backup_failed', 'k2_backup_offbox_none', 'mystery_alarm'] }), 'sew/backups': { backups: [] } })
    await renderReady('Degraded')
    expect(screen.getByTestId('sew-alarm-backup_failed').textContent).toContain('The last backup failed')
    expect(screen.getByTestId('sew-alarm-mystery_alarm').textContent).toContain('k2 hostchat status')
    fireEvent.click(screen.getAllByRole('button', { name: 'Choose where copies go' })[0])
    expect(screen.getByRole('tab', { name: 'Backups and keys' }).getAttribute('aria-selected')).toBe('true')
    expect(screen.getByText('Where copies of backups go')).toBeTruthy()
  })

  it('backups: back up now, then the list refreshes and the page says so', async () => {
    let list: Json[] = []
    gets({ 'sew/status': running(), 'sew/backups': () => ({ backups: list }) })
    posts({
      'sew/server/backup': () => {
        list = [{ id: '20261020T000000Z', kind: 'manual', created_at: '2026-10-20T00:00:00Z', bytes: 1048576 }]
        return { ok: true, jobId: 'job1', backup: { id: '20261020T000000Z' } }
      },
    })
    await renderReady('Running')
    tab('Backups and keys')
    expect(await screen.findByText('No backups yet')).toBeTruthy()
    fireEvent.click(await screen.findByRole('button', { name: 'Back up now' }))
    expect(await screen.findByText(/Made by hand/)).toBeTruthy()
    expect(screen.getByTestId('sew-backup-row').dataset.backup).toBe('20261020T000000Z')
    // Raw backup stamps never show; the row reads as a local date.
    expect(screen.queryByText('20261020T000000Z')).toBeNull()
    expect(postBodies('sew/server/backup')).toEqual([{}])
    expect(screen.getByTestId('sew-notice').textContent).toContain('Backup done.')
  })

  it('backups: a failed backup says so with the server reason', async () => {
    gets({ 'sew/status': running(), 'sew/backups': { backups: [] } })
    posts({ 'sew/server/backup': new DaemonHttpError('no space left on device', 500, 'backup_failed') })
    await renderReady('Running')
    tab('Backups and keys')
    fireEvent.click(await screen.findByRole('button', { name: 'Back up now' }))
    const note = await screen.findByTestId('dialog-error')
    expect(note.textContent).toContain('The backup ran out of disk space.')
    expect(note.textContent).toContain('Free up space on the server, then back up again.')
    expect(note.textContent).toContain('The server said: no space left on device.')
    expect(within(note).getByRole('button', { name: 'Back up again' })).toBeTruthy()
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
    tab('Backups and keys')
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

  it('off-box: backups and keys send kind and path; the keys confirm the restart', async () => {
    gets({ 'sew/status': running(), 'sew/backups': { backups: [] } })
    posts({ 'sew/server/ship': { ok: true }, 'sew/server/mirror': { ok: true, restarted: true } })
    await renderReady('Running')
    tab('Backups and keys')
    fireEvent.click(screen.getByLabelText('Backups: a folder on another disk'))
    fireEvent.change(screen.getByLabelText('Backups folder'), { target: { value: 'mnt/rel' } })
    expect(screen.getByText('Use a full path that starts with /.')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Backups folder'), { target: { value: ' /mnt/offbox/sew ' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[0])
    await waitFor(() => expect(postBodies('sew/server/ship')).toEqual([{ kind: 'dir', path: '/mnt/offbox/sew' }]))
    fireEvent.click(screen.getByLabelText('Keys: a folder on another disk'))
    fireEvent.change(screen.getByLabelText('Keys folder'), { target: { value: '/mnt/keys' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[1])
    await answer('Change where copies of keys go?', 'Save and restart')
    await waitFor(() => expect(postBodies('sew/server/mirror')).toEqual([{ kind: 'dir', path: '/mnt/keys' }]))
    expect(document.body.textContent).not.toMatch(/escrow/i)
  })

  it("off-box: the server's own backup is for backups only, with what K2 sees", async () => {
    const reason = 'k2-backup.timer is not enabled (systemctl is-enabled says \'disabled\')'
    const ship = { kind: 'box-backup', path: null, boxBackup: { ok: false, reason, timer: 'disabled' } }
    gets({ 'sew/status': running({ state: 'degraded', alarms: ['k2_backup_offbox_none'], backup: { ship, lastOkAt: null, alarms: [] } }), 'sew/backups': { backups: [] } })
    posts({ 'sew/server/ship': { ok: true } })
    await renderReady('Degraded')
    tab('Backups and keys')
    expect(screen.getByTestId('sew-ship-box-backup').textContent).toBe('With this server’s own backup, not confirmed yet')
    expect(document.body.textContent).toContain(`The server said: ${reason}.`)
    const radio = screen.getByLabelText("Backups: with this server's own backup") as HTMLInputElement
    expect(radio.checked).toBe(true)
    expect(screen.getAllByText(/K2 can’t see the server’s backup keep them on another machine yet\. Check that scheduled backups are turned on\./).length).toBeGreaterThan(0)
    expect(screen.queryByLabelText("Keys: with this server's own backup")).toBeNull()
    const save = screen.getAllByRole('button', { name: 'Save' })[0] as HTMLButtonElement
    expect(save.disabled).toBe(true)
    fireEvent.click(screen.getByLabelText('Backups: only on this server'))
    fireEvent.click(save)
    await waitFor(() => expect(postBodies('sew/server/ship')).toEqual([{ kind: 'none' }]))
  })

  it('web address posts config publicUrl', async () => {
    gets({ 'sew/status': running({ publicUrl: null, warnings: [{ code: 'public_url_unset', message: 'set a public URL' }] }) })
    posts({ 'sew/server/config': { ok: true } })
    await renderReady('Running')
    expect(screen.getByTestId('sew-warning-public_url_unset').textContent).toContain('Chat’s web address isn’t set')
    fireEvent.click(screen.getByRole('button', { name: 'Set' }))
    fireEvent.change(screen.getByLabelText('Chat web address'), { target: { value: 'http://hearth.box.k2.dev' } })
    expect(screen.getByText('Use an address that starts with https://.')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('Chat web address'), { target: { value: 'https://hearth.box.k2.dev' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(postBodies('sew/server/config')).toEqual([{ publicUrl: 'https://hearth.box.k2.dev' }]))
  })

  it('App Spaces: list, bind, unbind with a choice; the copy says Admin Access adds nobody', async () => {
    let spaces: Json[] = [{ app: 'hearth', spaceId: 'spc_1', boundAt: 1, members: { added: 3, held: 1, pending: 0 } }]
    gets({ 'sew/status': running(), 'sew/app-spaces': () => ({ ok: true, spaces }) })
    posts({
      'sew/app-spaces/bind': (b: Json) => ((spaces = [...spaces, { app: b.app, spaceId: b.spaceId, members: { added: 0, held: 0, pending: 0 } }]), { ok: true }),
      'sew/app-spaces/unbind': { ok: true },
    })
    await renderReady('Running')
    tab('App Spaces')
    expect(screen.getByTestId('sew-app-spaces').textContent).toContain('Admin Access never adds anyone.')
    expect(await screen.findByText('3 members · 1 on hold until their App access is fixed')).toBeTruthy()
    expect(screen.getByText('Hearth App')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Connect an App Space' }))
    fireEvent.change(screen.getByLabelText('App name'), { target: { value: 'meet' } })
    fireEvent.change(screen.getByLabelText('Space ID'), { target: { value: 'spc_2' } })
    fireEvent.click(within(screen.getByTestId('sew-bind-dialog')).getByRole('button', { name: 'Connect' }))
    await waitFor(() => expect(postBodies('sew/app-spaces/bind')).toEqual([{ app: 'meet', spaceId: 'spc_2' }]))
    expect(await screen.findByText('spc_2')).toBeTruthy()
    // S32 Q3: unbinding is a choice; the default keeps everyone.
    fireEvent.click(screen.getByRole('button', { name: /^Disconnect the Hearth App/ }))
    expect(postBodies('sew/app-spaces/unbind')).toEqual([])
    fireEvent.click(within(screen.getByTestId('sew-unbind-choice')).getByRole('button', { name: 'Disconnect' }))
    await waitFor(() => expect(postBodies('sew/app-spaces/unbind')).toEqual([{ app: 'hearth', spaceId: 'spc_1' }]))
    await waitFor(() => expect(screen.queryByTestId('sew-unbind-choice')).toBeNull())
    expect(screen.getByTestId('sew-notice').textContent).toContain('Everyone stays in it.')
    fireEvent.click(screen.getByRole('button', { name: /^Disconnect the Hearth App/ }))
    fireEvent.click(screen.getByLabelText('Remove the people K2 added'))
    fireEvent.click(within(screen.getByTestId('sew-unbind-choice')).getByRole('button', { name: 'Disconnect' }))
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
    tab('App Spaces')
    expect(await screen.findByText('This server doesn’t report App Spaces yet')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Connect an App Space' })).toBeNull()
  })

  // Sew 0.9.0 recording in Apps (K-c; vs-live B22).
  const REC_FLAGS = { appRecording: true, voiceRecordCap: true, recordingRequired: true, mgmtRecordingReads: true }
  const REC_POLICIES = {
    ok: true,
    flags: REC_FLAGS,
    policies: [{ key: 'recording.enabled', want: true, seen: true, setBy: 'owner', delivered: true }],
    locks: [],
    k5SpaceId: null,
  }
  function recView(app: string, over: Json = {}): Json {
    return {
      ok: true,
      app: { serviceId: app, name: app, appId: app },
      flags: REC_FLAGS,
      homeClient: false,
      recording: {},
      sent: null,
      delivered: true,
      orgAudioMaxDays: 30,
      spaces: [{ spaceId: '5101', governance: 'app', startLevel: null, required: null, locked: false }],
      rollout: null,
      ...over,
    }
  }
  function recGets(over: Record<string, unknown> = {}): void {
    gets({
      'sew/status': running(),
      'sew/recording/policies': REC_POLICIES,
      'sew/home': { ok: true, homeClient: 'hearth', apps: [{ app: 'jobs', delivered: true }] },
      'sew/app-spaces': { ok: true, spaces: [{ app: 'jobs', spaceId: '5101', members: { added: 1, held: 0, pending: 0 } }] },
      'app/recording': (p?: Json) => (p?.app === 'hearth' ? recView('hearth', { homeClient: true, spaces: [] }) : recView('jobs', { delivered: false })),
      ...over,
    })
  }

  it('Recording tab hidden without flags; row shows delivered', async () => {
    gets({ 'sew/status': running(), 'sew/recording/policies': { ...REC_POLICIES, flags: { ...REC_FLAGS, appRecording: false } } })
    await renderReady('Running')
    tab('Recording')
    expect((await screen.findByTestId('sew-recording-unavailable')).textContent).toContain("This server's chat can't record in Apps yet")
    expect(screen.queryByTestId('sew-recording-policies')).toBeNull()
    expect(screen.queryAllByTestId('sew-recording-app')).toHaveLength(0)
    expect(h.daemonCliGet.mock.calls.map((c) => c[0])).toEqual(['sew/status', 'sew/recording/policies'])
    cleanup()

    recGets()
    await renderReady('Running')
    tab('Recording')
    await waitFor(() => expect(screen.getAllByTestId('sew-recording-app')).toHaveLength(2))
    expect(screen.queryByTestId('sew-recording-unavailable')).toBeNull()
    // prd-home-call-recording-v1 HR-11: recording.enabled is the "Recording on this server" switch, not a read-only row.
    const sw = document.querySelector('[data-testid="sew-recording-switch"][data-key="recording.enabled"]')
    expect(sw?.textContent).toContain('Recording on this server')
    expect(screen.queryAllByTestId('sew-recording-policy')).toHaveLength(0)
    expect(screen.getByTestId('sew-recording-switches').textContent).not.toContain('recording.enabled')
    const [jobs, hearth] = screen.getAllByTestId('sew-recording-app')
    expect(jobs.textContent).toContain('jobs')
    expect(screen.getByTestId('sew-recording-delivered').textContent).toBe('Sending to chat…')
    expect(hearth.textContent).toContain('No recording settings here')
    expect(hearth.textContent).not.toContain('hearth')
    cleanup()

    recGets({ 'app/recording': (p?: Json) => recView(String(p?.app), p?.app === 'hearth' ? { homeClient: true } : {}) })
    await renderReady('Running')
    tab('Recording')
    await waitFor(() => expect(screen.getByTestId('sew-recording-delivered').textContent).toBe('Up to date in chat'))
  })

  it('Recording: an Admin sees required and server reads read-only; the owner turns server reads on with a confirm', async () => {
    recGets({ 'sew/status': running({ you: { ...YOU_OWNER, isOwner: false } }) })
    await renderReady('Running')
    tab('Recording')
    await waitFor(() => expect(screen.getAllByTestId('sew-recording-app')).toHaveLength(2))
    expect(screen.getAllByText('Only the owner turns this on')).toHaveLength(2)
    expect((screen.getByRole('switch', { name: "Let the App's server read transcripts" }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.queryByRole('button', { name: 'Lock against recording' })).toBeNull()
    cleanup()

    let sets = 0
    recGets()
    posts({
      'app/recording/set': () => {
        sets += 1
        if (sets === 1) {
          return new DaemonHttpError('needs the owner', 403, 'vault_confirm_required', {
            code: 'vault_confirm_required',
            serviceId: 'jobs',
            action: 'app-recording-owner',
          })
        }
        return recView('jobs', { recording: { server_read: true } })
      },
      'vault/confirm/start': { methods: ['local-auth'] },
      'vault/confirm/finish': { token: 'k2vc_rec' },
    })
    await renderReady('Running')
    tab('Recording')
    await waitFor(() => expect(screen.getAllByTestId('sew-recording-app')).toHaveLength(2))
    expect(screen.queryByText('Only the owner turns this on')).toBeNull()
    fireEvent.click(screen.getByRole('switch', { name: "Let the App's server read transcripts" }))
    await waitFor(() =>
      expect(postBodies('app/recording/set')).toEqual([
        { app: 'jobs', serverRead: true },
        { app: 'jobs', serverRead: true, confirmToken: 'k2vc_rec' },
      ]),
    )
    expect(postBodies('vault/confirm/start')).toEqual([{ action: 'app-recording-owner', secretId: 'jobs' }])
  })

  it('agents, approvals and deliveries (S30/S31) have no tab until their routes exist', async () => {
    gets({ 'sew/status': running() })
    await renderReady('Running')
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Overview', 'Backups and keys', 'App Spaces', 'Recording'])
    expect(document.body.textContent).not.toContain('S30')
  })

  it('tabs other than the overview wait for a live install', async () => {
    gets({ 'sew/status': running({ state: 'not-installed', owner: 'none' }), 'sew/preflight': { ready: true, checks: [] } })
    await renderReady('Set up chat and voice')
    expect(screen.getAllByRole('tab').map((t) => t.textContent)).toEqual(['Overview'])
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
