// @vitest-environment jsdom
//
// Settings → LLMs: switching the server default (field bug 2026-10-14).
// The confirm names from → to and warns (never refuses) when the target is
// used up; the default row says since when; each row says how old its
// usage is and when the last check failed; "Sign in again" on every token
// that isn't the default; a token that is signing in can't be switched to;
// opening the page re-reads usage. Daemon calls are mocked; an unexpected
// route throws.

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

import {
  AgentAccountsColumn,
  ACTIVE_RELOGIN_HINT,
  SIGNING_IN_SWITCH_REASON,
  switchConfirmText,
} from './AgentAccountsColumn'
import {
  checkedAgoText,
  exhaustedWarnings,
  parseAccountsDoc,
  resetLlmAccountsForTests,
  type LlmAccount,
} from '@/stores/llm-accounts'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

const NOW_S = Math.floor(Date.now() / 1000)
const IN_TWO_DAYS = new Date((NOW_S + 2 * 86400) * 1000).toISOString()

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x', tool: 'claude', label: 'x', kind: 'subscription', active: false, state: 'signed_in', detail: null,
    email: null, org: null, plan: null, expiresAt: null, refreshedAt: null, lastUsedAt: null, createdAt: 1,
    createdBy: null, usage: null, usageCheckedAt: null, ...over,
  }
}

const usage = (windows: Array<{ label: string; used: number; resetsAt: string }>, extra: Record<string, unknown> = {}) => ({
  harness: 'claude', plan: 'Max', windows, checkedAt: '', status: '', ...extra,
})

const RAKZM = account({
  id: 'acc_rakzm', label: 'rAKZM', active: true, plan: 'max',
  usage: usage([{ label: 'Session', used: 0.95, resetsAt: IN_TWO_DAYS }, { label: 'Weekly', used: 0.27, resetsAt: IN_TWO_DAYS }], {
    error: 'usage endpoint status 429',
  }),
  usageCheckedAt: NOW_S - 5 * 60,
})
const AFSROW = account({
  id: 'acc_afsrow', label: 'afsrow', plan: 'max',
  usage: usage([{ label: 'Session', used: 0.79, resetsAt: IN_TWO_DAYS }, { label: 'Weekly', used: 0.23, resetsAt: IN_TWO_DAYS }]),
  usageCheckedAt: NOW_S - 3 * 3600,
})
const LZTEK = account({
  id: 'acc_lztek', label: 'lztek', plan: 'max',
  usage: usage([{ label: 'Session', used: 0, resetsAt: IN_TWO_DAYS }, { label: 'Weekly', used: 1, resetsAt: IN_TWO_DAYS }]),
  usageCheckedAt: NOW_S - 30,
})
const SWITCHED_AT = NOW_S - 2 * 3600

let logins: unknown[] = []

function doc(): Record<string, unknown> {
  const notYet = (tool: string, display: string) => ({
    tool, display, supported: false, subscription: false, apiKeys: false, activeId: null, liveAccountId: null, loginMethod: null, accounts: [], pins: [],
  })
  return {
    tools: [
      {
        tool: 'claude', display: 'Claude', supported: true, subscription: true, apiKeys: true, activeId: 'acc_rakzm',
        liveAccountId: 'acc_rakzm', switchedAt: SWITCHED_AT, switchedBy: 'owner-token', loginMethod: 'temp_home',
        accounts: [RAKZM, AFSROW, LZTEK], pins: [],
      },
      notYet('codex', 'Codex'), notYet('grok', 'Grok'), notYet('gemini', 'Gemini'),
      notYet('cursor', 'Cursor Agent'), notYet('pi', 'Pi'), notYet('hermes', 'Hermes'),
    ],
    logins,
    airgap: false,
    switchNote: 'x',
  }
}

const confirmSpy = vi.fn(async (_o: { title: string; message: string; confirmLabel?: string }) => true)

beforeEach(() => {
  resetLlmAccountsForTests()
  logins = []
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  confirmSpy.mockReset()
  confirmSpy.mockImplementation(async () => true)
  useConfirmDialogStore.setState({ confirm: confirmSpy })
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/list') return doc()
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/login') return { account: account({ id: 'acc_afsrow' }), login: { loginId: 'l1', accountId: 'acc_afsrow', tool: 'claude', label: 'afsrow', mode: 'this_computer', method: 'temp_home', state: 'signing_in', url: null, code: null, error: null, screen: [], startedAt: 1, done: false, banner: null, offerMakeActive: false } }
    if (route.startsWith('llm/accounts/')) return {}
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

async function renderColumn(): Promise<void> {
  render(<AgentAccountsColumn />)
  await screen.findByTestId('llm-account-acc_rakzm')
}

describe('switching the server default', () => {
  it('asks first, naming from → to; Cancel sends nothing', async () => {
    confirmSpy.mockImplementation(async () => false)
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_afsrow')).getByRole('button', { name: 'Use this token' }))
    await waitFor(() => expect(confirmSpy).toHaveBeenCalledTimes(1))
    const opts = confirmSpy.mock.calls[0][0]
    expect(opts.title).toBe('Switch the Claude server default?')
    expect(opts.message).toContain('From: Claude · Max · rAKZM')
    expect(opts.message).toContain('To: Claude · Max · afsrow')
    expect(opts.message).not.toContain('Warning')
    expect(opts.confirmLabel).toBe('Switch')
    await Promise.resolve()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
  })

  it('warns, but still switches, when the target is at 100%', async () => {
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_lztek')).getByRole('button', { name: 'Use this token' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_lztek' }))
    const msg = confirmSpy.mock.calls[0][0].message
    expect(msg).toMatch(/Warning: lztek is at 100% of its Weekly limit until /)
  })

  it('a token that is signing in cannot be switched to', async () => {
    logins = [{ loginId: 'l9', accountId: 'acc_afsrow', tool: 'claude', label: 'afsrow', state: 'waiting_for_browser', done: false }]
    await renderColumn()
    const btn = within(screen.getByTestId('llm-account-acc_afsrow')).getByRole('button', { name: 'Use this token' }) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    expect(btn.title).toBe(SIGNING_IN_SWITCH_REASON)
    fireEvent.click(btn)
    expect(confirmSpy).not.toHaveBeenCalled()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
  })
})

describe('what each row says', () => {
  it('the server default says since when and how to sign it in again', async () => {
    await renderColumn()
    const since = screen.getByTestId('llm-default-since-acc_rakzm').textContent ?? ''
    expect(since.startsWith('Server default since ')).toBe(true)
    expect(since.length).toBeGreaterThan('Server default since '.length)
    expect(screen.getByTestId('llm-relogin-hint-acc_rakzm').textContent).toBe(ACTIVE_RELOGIN_HINT)
    const row = screen.getByTestId('llm-account-acc_rakzm')
    expect(within(row).queryByRole('button', { name: 'Sign in again' })).toBeNull()
    expect(within(row).queryByRole('button', { name: 'Use this token' })).toBeNull()
    expect(screen.queryByTestId('llm-default-since-acc_afsrow')).toBeNull()
  })

  it('each row says how old its usage is, and a failed check says so', async () => {
    await renderColumn()
    expect(screen.getByTestId('llm-usage-age-acc_rakzm').textContent).toContain('Usage checked 5 min ago')
    expect(screen.getByTestId('llm-usage-error-acc_rakzm').textContent).toContain('last check failed (usage endpoint status 429)')
    expect(screen.getByTestId('llm-usage-age-acc_afsrow').textContent).toContain('Usage checked 3 h ago')
    expect(screen.queryByTestId('llm-usage-error-acc_afsrow')).toBeNull()
    expect(screen.getByTestId('llm-usage-age-acc_lztek').textContent).toContain('Usage checked just now')
  })

  it('Sign in again is on every subscription that is not the default and opens the sign-in', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_afsrow')
    fireEvent.click(within(row).getByRole('button', { name: 'Sign in again' }))
    expect(screen.getByRole('dialog', { name: 'Sign afsrow in again' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login', { id: 'acc_afsrow', mode: 'this_computer' }),
    )
    expect(within(screen.getByTestId('llm-account-acc_lztek')).getByRole('button', { name: 'Sign in again' })).toBeTruthy()
  })

  it('opening the page re-reads usage on that server', async () => {
    await renderColumn()
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/refresh', { usage: true }))
  })
})

describe('helpers', () => {
  it('checkedAgoText', () => {
    const now = 1_000_000 * 1000
    expect(checkedAgoText(null, now)).toBe('never checked')
    expect(checkedAgoText(1_000_000 - 10, now)).toBe('checked just now')
    expect(checkedAgoText(1_000_000 - 600, now)).toBe('checked 10 min ago')
    expect(checkedAgoText(1_000_000 - 7200, now)).toBe('checked 2 h ago')
    expect(checkedAgoText(1_000_000 - 3 * 86400, now)).toBe('checked 3 d ago')
  })

  it('exhaustedWarnings skips windows that already reset', () => {
    const a = { ...LZTEK, usage: usage([{ label: 'Weekly', used: 1, resetsAt: '2020-01-01T00:00:00Z' }]) } as unknown as LlmAccount
    expect(exhaustedWarnings(a)).toEqual([])
    expect(exhaustedWarnings(LZTEK as unknown as LlmAccount)).toHaveLength(1)
  })

  it('switchConfirmText names "no token" when nothing is the default yet', () => {
    const d = parseAccountsDoc(doc())
    const out = switchConfirmText(d.tools[0], null, d.tools[0].accounts[1])
    expect(out.message).toContain('From: no token')
    expect(out.warnings).toEqual([])
  })
})
