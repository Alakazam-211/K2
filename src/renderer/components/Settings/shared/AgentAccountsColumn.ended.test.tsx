// @vitest-environment jsdom
//
// A Claude token whose sign-in has ended (Claude's time limit): the
// server default included, the row offers a way back (apply its saved
// sign-in, or sign in again). Daemon calls are mocked; routes and bodies
// are asserted exactly. Fail loud: an unexpected route throws.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: async () => {} }))

import { AgentAccountsColumn } from './AgentAccountsColumn'
import { resetLlmAccountsForTests } from '@/stores/llm-accounts'
import { resetUsageLimitsForTests } from '@/stores/usage-limits'

const ENDED_DETAIL = 'Claude’s sign-in reached its time limit; sign in again.'

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x', tool: 'claude', label: 'x', kind: 'subscription', active: false, state: 'signed_in', detail: null,
    email: null, org: null, plan: null, expiresAt: null, refreshedAt: null, lastUsedAt: null, createdAt: 1,
    createdBy: null, usage: null, usageCheckedAt: null, ...over,
  }
}

function docWith(accounts: Record<string, unknown>[], extra: Record<string, unknown> = {}): Record<string, unknown> {
  const active = accounts.find((a) => a.active)?.id ?? null
  return {
    tools: [{ tool: 'claude', display: 'Claude', supported: true, subscription: true, apiKeys: true, activeId: active, liveAccountId: active, loginMethod: 'temp_home', accounts, pins: [] }],
    logins: [],
    airgap: false,
    switchNote: 'Changing the server default token affects every chat that uses Server default on this server.',
    canChangeDefault: true,
    changeDefaultReason: null,
    ...extra,
  }
}

let listDoc: Record<string, unknown> = docWith([])

beforeEach(() => {
  resetLlmAccountsForTests()
  resetUsageLimitsForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/list') return listDoc
    if (route === 'claude-auth/status') return { state: 'missing', expiresAt: null, secondsRemaining: null, schedulerInstalled: false, retired: true }
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/login') return { account: account({ id: 'acc_def' }), login: { loginId: 'login_1', accountId: 'acc_def', tool: 'claude', label: 'jeff', mode: 'other_device', method: 'temp_home', state: 'signing_in', url: null, code: null, error: null, screen: [], startedAt: 1, done: false, banner: null, offerMakeActive: false } }
    if (route.startsWith('llm/accounts/')) return {}
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

async function renderRow(id: string): Promise<HTMLElement> {
  render(<AgentAccountsColumn />)
  return screen.findByTestId(`llm-account-${id}`)
}

describe('an ended Claude sign-in', () => {
  it('the server default with a good saved sign-in offers Apply saved sign-in, which posts a switch to it', async () => {
    listDoc = docWith([account({ id: 'acc_def', label: 'jeff', active: true, state: 'needs_login', detail: ENDED_DETAIL, canApply: true })])
    const row = await renderRow('acc_def')
    expect(within(row).getByTestId('llm-state-acc_def').textContent).toBe('Needs sign-in')
    expect(row.textContent).toContain(ENDED_DETAIL)
    fireEvent.click(within(row).getByRole('button', { name: 'Apply saved sign-in' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_def' }))
    expect((await screen.findByTestId('llm-notice')).textContent).toContain('“jeff”’s saved sign-in is back in Claude')
  })

  it('Apply saved sign-in is off for someone who may not change the default, with the reason', async () => {
    listDoc = docWith([account({ id: 'acc_def', label: 'jeff', active: true, state: 'needs_login', canApply: true })], {
      canChangeDefault: false,
      changeDefaultReason: 'Only an Admin or the Owner can change the server default.',
    })
    const row = await renderRow('acc_def')
    const b = within(row).getByRole('button', { name: 'Apply saved sign-in' }) as HTMLButtonElement
    expect(b.disabled).toBe(true)
    expect(b.getAttribute('title')).toBe('Only an Admin or the Owner can change the server default.')
  })

  it('the server default with nothing to apply offers Sign in again, which starts the sign-in for it', async () => {
    listDoc = docWith([account({ id: 'acc_def', label: 'jeff', active: true, state: 'needs_login', detail: ENDED_DETAIL })])
    const row = await renderRow('acc_def')
    expect(within(row).queryByRole('button', { name: 'Apply saved sign-in' })).toBeNull()
    fireEvent.click(within(row).getByRole('button', { name: 'Sign in again' }))
    expect(await screen.findByRole('dialog')).toBeTruthy()
  })
})
