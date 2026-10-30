// @vitest-environment jsdom
//
// Chat Hosting end to end against a scripted fake supervisor (S32 §16.3,
// headless): not installed → preflight → Enable → the install advances one
// step per status poll → Degraded with the two expected alarms → reveal the
// recovery key with a person confirm → set the off-box backup destination
// → Running with no alarms. The page only renders the fake's truth; every
// state change comes from the fake daemon, never from the page.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

type Json = Record<string, unknown>

const fake = vi.hoisted(() => {
  const STEPS = ['master-key', 'stage', 'install-bin', 'install', 'enable-units', 'db', 'migrate', 'start', 'token', 'health', 'link']
  const s = {
    state: 'not-installed' as string,
    done: [] as string[],
    alarms: [] as string[],
    recoverySaved: false,
    ship: { kind: 'none', path: null as string | null },
    tokens: new Set<string>(),
    calls: [] as string[],
  }
  const YOU = { canManage: true, isOwner: true, isAdmin: true, passport: false, canRecoveryKey: true, canPurge: true, canOpenAsOwner: true }
  function status(): Json {
    // The machine runs in the "daemon": one step per status read.
    if (s.state === 'installing') {
      s.done.push(STEPS[s.done.length])
      if (s.done.length === STEPS.length) {
        s.state = 'degraded'
        s.alarms = ['k2_recovery_key_not_saved', 'k2_backup_offbox_none']
      }
    }
    return {
      ok: true,
      supported: true,
      released: true,
      state: s.state,
      consistent: true,
      pinnedVersion: '0.3.0',
      installedVersion: s.state === 'not-installed' ? null : '0.3.0',
      installId: s.state === 'not-installed' ? null : 'ins_flow',
      alarms: [...s.alarms],
      warnings: [],
      recoveryKeySaved: s.recoverySaved,
      backup: { ship: { ...s.ship }, alarms: [] },
      drill: {},
      mirror: { kind: 'none', path: null },
      appUrl: null,
      owner: s.state === 'not-installed' ? 'none' : 'set',
      target: { enabled: s.state !== 'not-installed' },
      purgeConfirmHostname: 'flow.box',
      enableProgress: s.state === 'installing' || s.done.length ? { steps: Object.fromEntries(s.done.map((d, i) => [d, { at: i }])), current: STEPS[s.done.length] ?? null } : null,
      you: YOU,
    }
  }
  function settle(): void {
    if (s.state === 'degraded' && s.alarms.length === 0) s.state = 'running'
  }
  async function get(route: string, params?: Json): Promise<unknown> {
    s.calls.push(`GET ${route}`)
    if (route === 'sew/status') return status()
    if (route === 'sew/preflight') return { ready: true, checks: [{ id: 'os', status: 'pass', detail: 'Ubuntu 24.04', hint: '' }, { id: 'ram', status: 'pass', detail: '8 GiB', hint: '' }] }
    if (route === 'sew/backups') return { backups: [] }
    throw new Error(`unexpected GET ${route} ${JSON.stringify(params ?? {})}`)
  }
  async function post(route: string, body: Json): Promise<unknown> {
    s.calls.push(`POST ${route}`)
    switch (route) {
      case 'sew/server/enable':
        if (s.state !== 'not-installed') throw new Error('already enabled')
        if (body.identity !== 'k2') throw new Error('identity must be explicit')
        s.state = 'installing'
        return { ok: true, state: 'installing' }
      case 'vault/confirm/start':
        if (body.action !== 'sew-recovery-key' || body.secretId !== 'ins_flow') throw new Error(`bad confirm ${JSON.stringify(body)}`)
        return { methods: ['local-auth'], expiresIn: 60 }
      case 'vault/confirm/finish': {
        const t = `k2vc_${s.tokens.size + 1}`
        s.tokens.add(t)
        return { token: t, expiresIn: 60 }
      }
      case 'sew/server/recovery-key': {
        const t = String(body.confirmToken ?? '')
        if (!s.tokens.delete(t)) throw new Error('owner_confirm_required')
        s.recoverySaved = true
        s.alarms = s.alarms.filter((a) => a !== 'k2_recovery_key_not_saved')
        settle()
        return { ok: true, recoveryKey: 'FLOW-RECOVERY-KEY', warning: 'store it' }
      }
      case 'sew/server/ship':
        if (body.kind !== 'dir' || body.path !== '/mnt/offbox/sew') throw new Error(`bad ship ${JSON.stringify(body)}`)
        s.ship = { kind: 'dir', path: '/mnt/offbox/sew' }
        s.alarms = s.alarms.filter((a) => a !== 'k2_backup_offbox_none')
        settle()
        return { ok: true }
      default:
        throw new Error(`unexpected POST ${route}`)
    }
  }
  return { s, get, post, STEPS }
})

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((route: string, params?: Json) => fake.get(route, params)),
    daemonCliPost: primaryOnly((route: string, body: Json) => fake.post(route, body)),
  }
})
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onSewChanged: () => () => undefined,
}))

import { SewSection } from './SewSection'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

const OFFBOX_NOTE = 'Every K2 box shows Degraded until its chat and voice backups have off-box copies. Set them under Off-box copies.'
const chip = (): string => screen.getByTestId('sew-card-state').textContent ?? ''

async function tick(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: false })
  useConfirmDialogStore.setState({ confirm: async () => true })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('Chat Hosting flow against a fake supervisor', () => {
  it('enable → degraded → reveal key → off-box → running', async () => {
    render(<SewSection />)
    await tick(10)
    expect(chip()).toBe('Not installed')
    expect(screen.getByTestId('sew-check-os').dataset.status).toBe('pass')

    fireEvent.click(screen.getByRole('button', { name: 'Enable Chat / Voice Hosting' }))
    await tick(10)
    expect(chip()).toBe('Installing…')
    expect(screen.getByTestId('sew-step-master-key').dataset.step).toBe('done')

    // One step per 2 s poll until the machine finishes.
    for (let i = 0; i < fake.STEPS.length && chip() === 'Installing…'; i++) await tick(2000)
    expect(chip()).toBe('Degraded')
    expect(fake.s.done).toEqual(fake.STEPS)
    const polls = fake.s.calls.filter((c) => c === 'GET sew/status').length
    await tick(10_000)
    expect(fake.s.calls.filter((c) => c === 'GET sew/status').length).toBe(polls)

    fireEvent.click(screen.getByRole('button', { name: /^Alarms/ }))
    expect(screen.getByTestId('sew-alarm-k2_recovery_key_not_saved')).toBeTruthy()
    expect(screen.getByTestId('sew-alarm-k2_backup_offbox_none')).toBeTruthy()
    // Not only the off-box alarms yet: no "every box" note.
    expect(screen.queryByText(OFFBOX_NOTE)).toBeNull()

    // The recovery-key alarm's action leads to the server panel's Reveal.
    fireEvent.click(screen.getByRole('button', { name: 'Reveal recovery key' }))
    expect(screen.getByText('Save your recovery key now. Without it, a dead box means lost messages.')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Reveal recovery key' }))
    await tick(10)
    expect(screen.getByTestId('sew-once-secret-value').textContent).toBe('FLOW-RECOVERY-KEY')
    fireEvent.click(screen.getByLabelText('I stored it off this machine'))
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    await tick(10)
    expect(screen.queryByTestId('sew-once-secret')).toBeNull()
    expect(document.body.innerHTML).not.toContain('FLOW-RECOVERY-KEY')

    fireEvent.click(screen.getByRole('button', { name: /^Alarms/ }))
    expect(screen.queryByTestId('sew-alarm-k2_recovery_key_not_saved')).toBeNull()
    expect(screen.getByText(OFFBOX_NOTE)).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Go to Off-box copies' }))
    fireEvent.click(screen.getByLabelText('Backup destination: a folder on another disk'))
    fireEvent.change(screen.getByLabelText('Backup destination path'), { target: { value: '/mnt/offbox/sew' } })
    fireEvent.click(screen.getAllByRole('button', { name: 'Save' })[0])
    await tick(10)
    expect(chip()).toBe('Running')
    fireEvent.click(screen.getByRole('button', { name: 'Alarms' }))
    expect(screen.getByText('No alarms.')).toBeTruthy()

    expect(fake.s.calls.filter((c) => c.startsWith('POST'))).toEqual([
      'POST sew/server/enable',
      'POST vault/confirm/start',
      'POST vault/confirm/finish',
      'POST sew/server/recovery-key',
      'POST sew/server/ship',
    ])
  })
})
