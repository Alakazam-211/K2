// @vitest-environment jsdom
//
// A token whose access token expired on a good sign-in: the row says so, with the daemon's note on who renews it,
// and is not drawn as signed out. A fine token and a signed-out one look as
// before. Daemon calls are mocked; an unexpected route throws.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, cleanup, within } from '@testing-library/react'

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

const NOTE =
  'Its access token expired on 2026-11-16 11:02 UTC. Its sign-in is still good: Claude renews it by itself the next time it runs on this token. Nothing to do.'

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x', tool: 'claude', label: 'x', kind: 'subscription', active: false, state: 'signed_in', detail: null,
    email: null, org: null, plan: null, expiresAt: null, refreshedAt: null, lastUsedAt: null, createdAt: 1,
    createdBy: null, usage: null, usageCheckedAt: null, ...over,
  }
}

function docWith(accounts: Record<string, unknown>[]): Record<string, unknown> {
  const active = accounts.find((a) => a.active)?.id ?? null
  return {
    tools: [{ tool: 'claude', display: 'Claude', supported: true, subscription: true, apiKeys: true, activeId: active, liveAccountId: active, loginMethod: 'temp_home', accounts, pins: [] }],
    logins: [],
    airgap: false,
    switchNote: 'Changing the server default token affects every chat that uses Server default on this server.',
    canChangeDefault: true,
    changeDefaultReason: null,
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
    if (route.startsWith('llm/accounts/')) return {}
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

describe('an expired access token on a good sign-in', () => {
  it('the server default says “Access token expired” with who renews it, and is not signed out', async () => {
    listDoc = docWith([
      account({ id: 'acc_def', label: 'jeff', active: true, access: 'expired', accessNote: NOTE }),
      account({ id: 'acc_ok', label: 'fine', access: 'fine' }),
      account({ id: 'acc_out', label: 'out', state: 'needs_login', detail: 'Claude’s sign-in reached its time limit; sign in again.', access: 'signed_out' }),
    ])
    render(<AgentAccountsColumn />)
    const row = await screen.findByTestId('llm-account-acc_def')
    const tag = within(row).getByTestId('llm-access-acc_def')
    expect(tag.textContent).toBe('Access token expired')
    expect(tag.getAttribute('title')).toBe(NOTE)
    expect(within(row).getByTestId('llm-access-note-acc_def').textContent).toBe(NOTE)
    expect(within(row).queryByTestId('llm-state-acc_def')).toBeNull()
    expect(within(row).queryByRole('button', { name: 'Sign in again' })).toBeNull()

    const ok = screen.getByTestId('llm-account-acc_ok')
    expect(within(ok).queryByTestId('llm-access-acc_ok')).toBeNull()
    expect(within(ok).queryByTestId('llm-access-note-acc_ok')).toBeNull()

    const out = screen.getByTestId('llm-account-acc_out')
    expect(within(out).queryByTestId('llm-access-acc_out')).toBeNull()
    expect(within(out).getByTestId('llm-state-acc_out').textContent).toBe('Needs sign-in')
  })
})
