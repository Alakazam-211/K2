// @vitest-environment jsdom
//
// Settings → Subs / Tokens: switching the server default (field bug
// 2026-10-14). The confirm (the shared dialog) names from → to and warns
// (never refuses) when the target is used up; the default row says since
// when; each row says how old its usage is and when the last check failed;
// "Sign in again" on every token that isn't the default; a token that is
// signing in can't be switched to; opening the page re-reads usage. Daemon
// calls are mocked; an unexpected route throws.

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
  ADMIN_SWITCH_REASON,
  SIGNING_IN_SWITCH_REASON,
  switchConfirmText,
} from './AgentAccountsColumn'
import {
  checkedAgoText,
  exhaustedWarnings,
  parseAccountsDoc,
  resetLlmAccountsForTests,
  useLlmAccountsStore,
  type LlmAccount,
} from '@/stores/llm-accounts'
import { primaryScope } from '@/kessel/server-scope'

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
const LUMON = account({
  id: 'acc_lumon', label: 'lumon', plan: 'max',
  usage: usage([{ label: 'Session', used: 0, resetsAt: IN_TWO_DAYS }, { label: 'Weekly', used: 1, resetsAt: IN_TWO_DAYS }]),
  usageCheckedAt: NOW_S - 30,
})
const SWITCHED_AT = NOW_S - 2 * 3600

let logins: unknown[] = []
let docExtra: Record<string, unknown> = {}
/** The daemon's server default for Claude (a switch moves it). */
let defaultId = 'acc_rakzm'

function doc(): Record<string, unknown> {
  const notYet = (tool: string, display: string) => ({
    tool, display, supported: false, subscription: false, apiKeys: false, activeId: null, liveAccountId: null, loginMethod: null, accounts: [], pins: [],
  })
  return {
    tools: [
      {
        tool: 'claude', display: 'Claude', supported: true, subscription: true, apiKeys: true, activeId: defaultId,
        liveAccountId: defaultId, switchedAt: SWITCHED_AT, switchedBy: 'owner-token', loginMethod: 'temp_home',
        accounts: [RAKZM, AFSROW, LUMON].map((a) => ({ ...a, active: a.id === defaultId })), pins: [],
      },
      notYet('codex', 'Codex'), notYet('grok', 'Grok'), notYet('gemini', 'Gemini'),
      notYet('cursor', 'Cursor Agent'), notYet('pi', 'Pi'), notYet('hermes', 'Hermes'),
    ],
    logins,
    airgap: false,
    switchNote: 'x',
    ...docExtra,
  }
}

beforeEach(() => {
  resetLlmAccountsForTests()
  logins = []
  docExtra = {}
  defaultId = 'acc_rakzm'
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
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

const makeDefault = (id: string): HTMLButtonElement =>
  within(screen.getByTestId(`llm-account-${id}`)).getByRole('button', { name: 'Make default' }) as HTMLButtonElement

const confirmButton = (name: string): HTMLElement => within(screen.getByTestId('llm-switch-confirm')).getByRole('button', { name })

describe('switching the server default', () => {
  it('asks first, naming from → to; Cancel sends nothing', async () => {
    await renderColumn()
    fireEvent.click(makeDefault('acc_afsrow'))
    const dialog = screen.getByRole('dialog', { name: 'Make “afsrow” the Claude server default?' })
    expect(dialog.textContent).toContain('From: Claude · Max · rAKZM')
    expect(dialog.textContent).toContain('To: Claude · Max · afsrow')
    expect(dialog.textContent).not.toContain('Warning')
    expect(confirmButton('Make default')).toBeTruthy()
    fireEvent.click(confirmButton('Cancel'))
    expect(screen.queryByTestId('llm-switch-confirm')).toBeNull()
    await Promise.resolve()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
  })

  it('warns, but still switches, when the target is at 100%', async () => {
    await renderColumn()
    fireEvent.click(makeDefault('acc_lumon'))
    expect(screen.getByTestId('llm-switch-confirm').textContent).toMatch(/Warning: lumon is at 100% of its Weekly limit until /)
    fireEvent.click(confirmButton('Make default'))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_lumon' }))
  })

  it('a token that is signing in cannot be switched to', async () => {
    logins = [{ loginId: 'l9', accountId: 'acc_afsrow', tool: 'claude', label: 'afsrow', state: 'waiting_for_browser', done: false }]
    await renderColumn()
    const btn = makeDefault('acc_afsrow')
    expect(btn.disabled).toBe(true)
    expect(btn.title).toBe(SIGNING_IN_SWITCH_REASON)
    fireEvent.click(btn)
    expect(screen.queryByTestId('llm-switch-confirm')).toBeNull()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
  })

  // 0.46.10 freeze: the daemon now answers `busy` at once (409) when
  // another Claude task holds the lock; the page says so and stays usable.
  it('a busy answer shows the daemon hint and the page stays usable', async () => {
    const hint = 'Another Claude account task is running; try again in a moment.'
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'llm/accounts/switch') {
        throw new Error(JSON.stringify({ error: { code: 'busy', hint, retryAfterSecs: 3 } }))
      }
      if (route.startsWith('llm/accounts/')) return {}
      throw new Error(`unexpected POST ${route}`)
    })
    await renderColumn()
    fireEvent.click(makeDefault('acc_afsrow'))
    fireEvent.click(confirmButton('Make default'))
    expect((await screen.findByTestId('llm-row-error')).textContent).toContain(hint.replace(/\.$/, ''))
    // Still usable: another switch asks again.
    fireEvent.click(makeDefault('acc_lumon'))
    fireEvent.click(confirmButton('Make default'))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_lumon' }))
  })
})

// Rosson 11-12: "Make default" showed its note, but the list kept the old
// default and the top bar kept the old numbers; a second try worked.
describe('Make default shows the change on the screen', () => {
  const isDefault = (id: string): boolean =>
    within(screen.getByTestId(`llm-account-${id}`)).queryByText('Server default') !== null
  const switches = (): number => h.daemonCliPost.mock.calls.filter((c) => c[0] === 'llm/accounts/switch').length

  /** The switch route: held until the test answers it. */
  function holdSwitch(): { answer: () => void; fail: (msg: string) => void } {
    let answer = (): void => {}
    let fail = (_msg: string): void => {}
    h.daemonCliPost.mockImplementation((route: string, body: { id?: string }) => {
      if (route === 'llm/accounts/switch') {
        return new Promise((resolve, reject) => {
          answer = () => {
            defaultId = body.id ?? ''
            resolve({ switch: { to: body.id, alreadyActive: false }, account: { ...AFSROW, active: true } })
          }
          fail = (msg) => reject(new Error(msg))
        })
      }
      if (route.startsWith('llm/accounts/')) return Promise.resolve({})
      return Promise.reject(new Error(`unexpected POST ${route}`))
    })
    return { answer: () => answer(), fail: (m) => fail(m) }
  }

  it('the first confirm sends the switch exactly once', async () => {
    const sw = holdSwitch()
    await renderColumn()
    fireEvent.click(makeDefault('acc_afsrow'))
    fireEvent.click(confirmButton('Make default'))
    await waitFor(() => expect(switches()).toBe(1))
    sw.answer()
    expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_afsrow' })
    await waitFor(() => expect(isDefault('acc_afsrow')).toBe(true))
    expect(switches()).toBe(1)
  })

  it('the badge moves at once, before the daemon answers, and stays after', async () => {
    const sw = holdSwitch()
    await renderColumn()
    expect(isDefault('acc_rakzm')).toBe(true)
    fireEvent.click(makeDefault('acc_afsrow'))
    fireEvent.click(confirmButton('Make default'))
    await waitFor(() => expect(isDefault('acc_afsrow')).toBe(true))
    expect(isDefault('acc_rakzm')).toBe(false)
    // The old default now offers Make default; the new one doesn't.
    expect(makeDefault('acc_rakzm').disabled).toBe(false)
    expect(within(screen.getByTestId('llm-account-acc_afsrow')).queryByRole('button', { name: 'Make default' })).toBeNull()
    sw.answer()
    expect((await screen.findByText('“afsrow” is now the Claude server default.'))).toBeTruthy()
    await waitFor(() => expect(h.daemonCliGet.mock.calls.filter((c) => c[0] === 'llm/accounts/list').length).toBeGreaterThan(1))
    expect(isDefault('acc_afsrow')).toBe(true)
    expect(isDefault('acc_rakzm')).toBe(false)
  })

  it('a list read that began before the switch cannot put the old default back', async () => {
    await renderColumn()
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/refresh', { usage: true }))
    await waitFor(() => expect(useLlmAccountsStore.getState().entries[primaryScope().id]?.doc).toBeTruthy())
    // A read already on its way (the page's open refresh, a usage event)
    // that will answer with the list from before the switch.
    let answerOld: (v: unknown) => void = () => {}
    const old = doc()
    h.daemonCliGet.mockImplementationOnce(() => new Promise((r) => (answerOld = r)))
    void useLlmAccountsStore.getState().load(primaryScope())
    const sw = holdSwitch()
    fireEvent.click(makeDefault('acc_afsrow'))
    fireEvent.click(confirmButton('Make default'))
    sw.answer()
    await screen.findByText('“afsrow” is now the Claude server default.')
    answerOld(old)
    const reads = h.daemonCliGet.mock.calls.length
    await waitFor(() => expect(h.daemonCliGet.mock.calls.length).toBeGreaterThan(reads))
    await new Promise((r) => setTimeout(r, 0))
    expect(isDefault('acc_afsrow')).toBe(true)
    expect(isDefault('acc_rakzm')).toBe(false)
    expect(switches()).toBe(1)
  })

  it('a refused switch puts the badge back and says why', async () => {
    const sw = holdSwitch()
    await renderColumn()
    fireEvent.click(makeDefault('acc_afsrow'))
    fireEvent.click(confirmButton('Make default'))
    await waitFor(() => expect(isDefault('acc_afsrow')).toBe(true))
    sw.fail('the server is busy')
    expect((await screen.findByTestId('llm-row-error')).textContent).toContain('the server is busy')
    expect(isDefault('acc_rakzm')).toBe(true)
    expect(isDefault('acc_afsrow')).toBe(false)
  })
})

describe('the server default is Admin and above (Rosson 2026-10-20)', () => {
  const REASON = 'Only an Admin or the Owner can change the server default token. Ask an Admin, or set a token for your own chat instead.'

  it('a Member sees the tokens, but both switch controls are off with the reason shown', async () => {
    docExtra = { canChangeDefault: false, changeDefaultReason: REASON }
    await renderColumn()
    expect(screen.getByTestId('llm-default-admin-only').textContent).toBe(REASON)
    const use = makeDefault('acc_afsrow')
    expect(use.disabled).toBe(true)
    expect(use.title).toBe(REASON)
    fireEvent.click(use)
    const next = screen.getByRole('button', { name: 'Use next token' }) as HTMLButtonElement
    expect(next.disabled).toBe(true)
    expect(next.title).toBe(REASON)
    fireEvent.click(next)
    await Promise.resolve()
    expect(screen.queryByTestId('llm-switch-confirm')).toBeNull()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
  })

  it('an Admin or the Owner keeps both controls', async () => {
    docExtra = { canChangeDefault: true, changeDefaultReason: null }
    await renderColumn()
    expect(screen.queryByTestId('llm-default-admin-only')).toBeNull()
    expect(makeDefault('acc_afsrow').disabled).toBe(false)
    expect((screen.getByRole('button', { name: 'Use next token' }) as HTMLButtonElement).disabled).toBe(false)
  })

  it('a doc without the field (older daemon) keeps the controls, and a missing reason falls back', () => {
    expect(parseAccountsDoc({ tools: [] }).canChangeDefault).toBe(true)
    const d = parseAccountsDoc({ tools: [], canChangeDefault: false })
    expect(d.canChangeDefault).toBe(false)
    expect(d.changeDefaultReason).toBeNull()
    expect(ADMIN_SWITCH_REASON).toContain('Admin')
  })
})

describe('what each row says', () => {
  it('the server default says since when and, in its menu, how to sign it in again', async () => {
    await renderColumn()
    const since = screen.getByTestId('llm-facts-acc_rakzm').textContent ?? ''
    expect(since.startsWith('Server default since ')).toBe(true)
    expect(since.length).toBeGreaterThan('Server default since '.length)
    const row = screen.getByTestId('llm-account-acc_rakzm')
    expect(within(row).queryByRole('button', { name: 'Sign in again' })).toBeNull()
    expect(within(row).queryByRole('button', { name: 'Make default' })).toBeNull()
    expect(screen.queryByTestId('llm-facts-acc_afsrow')).toBeNull()
    fireEvent.click(within(row).getByRole('button', { name: 'More actions for rAKZM' }))
    const relogin = screen.getByRole('menuitem', { name: /^Sign in again/ })
    expect(relogin.getAttribute('aria-disabled')).toBe('true')
    expect(relogin.textContent).toContain(ACTIVE_RELOGIN_HINT)
  })

  it('each row says how old its usage is, and a failed check says so', async () => {
    await renderColumn()
    expect(screen.getByTestId('llm-usage-age-acc_rakzm').textContent).toContain('Usage checked 5 min ago')
    expect(screen.getByTestId('llm-usage-error-acc_rakzm').textContent).toContain('last check failed (usage endpoint status 429)')
    expect(screen.getByTestId('llm-usage-age-acc_afsrow').textContent).toContain('Usage checked 3 h ago')
    expect(screen.queryByTestId('llm-usage-error-acc_afsrow')).toBeNull()
    expect(screen.getByTestId('llm-usage-age-acc_lumon').textContent).toContain('Usage checked just now')
  })

  it('Sign in again is on every subscription that is not the default and opens the sign-in', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_afsrow')
    fireEvent.click(within(row).getByRole('button', { name: 'More actions for afsrow' }))
    fireEvent.click(screen.getByRole('menuitem', { name: /^Sign in again/ }))
    expect(screen.getByRole('dialog', { name: 'Sign “afsrow” in again' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login', { id: 'acc_afsrow', mode: 'this_computer' }),
    )
    fireEvent.click(within(screen.getByTestId('llm-account-acc_lumon')).getByRole('button', { name: 'More actions for lumon' }))
    expect(screen.getByRole('menuitem', { name: /^Sign in again/ }).getAttribute('aria-disabled')).toBeNull()
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
    const a = { ...LUMON, usage: usage([{ label: 'Weekly', used: 1, resetsAt: '2020-01-01T00:00:00Z' }]) } as unknown as LlmAccount
    expect(exhaustedWarnings(a)).toEqual([])
    expect(exhaustedWarnings(LUMON as unknown as LlmAccount)).toHaveLength(1)
  })

  it('switchConfirmText names "no token" when nothing is the default yet', () => {
    const d = parseAccountsDoc(doc())
    const out = switchConfirmText(d.tools[0], null, d.tools[0].accounts[1])
    expect(out.message).toContain('From: no token')
    expect(out.warnings).toEqual([])
  })
})
