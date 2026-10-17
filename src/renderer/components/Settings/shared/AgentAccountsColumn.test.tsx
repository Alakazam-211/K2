// @vitest-environment jsdom
//
// Settings → LLMs "Tokens" column + the sign-in sheet. Daemon calls are
// mocked; routes and bodies are asserted exactly. Fail loud: an
// unexpected route throws.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, act, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
  openUrl: vi.fn(async (_url: string) => {}),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: (url: string) => h.openUrl(url) }))

import {
  AgentAccountsColumn,
  SWITCH_NOTE,
  ACTIVE_REMOVE_REASON,
  ACTIVE_RELOGIN_HINT,
  SIGNING_IN_SWITCH_REASON,
} from './AgentAccountsColumn'
import { resetLlmAccountsForTests, parseAccountsDoc, errorText } from '@/stores/llm-accounts'
import { resetUsageLimitsForTests } from '@/stores/usage-limits'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { AGENTS_MANIFEST } from '../sections/AgentsSection'
import { LLM_TOKENS_MANIFEST, LlmTokensSection } from '../sections/LlmTokensSection'

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x',
    tool: 'claude',
    label: 'x',
    active: false,
    state: 'signed_in',
    detail: null,
    email: null,
    org: null,
    plan: null,
    expiresAt: null,
    refreshedAt: null,
    lastUsedAt: null,
    createdAt: 1,
    createdBy: null,
    usage: null,
    usageCheckedAt: null,
    ...over,
  }
}

const WORK = account({
  id: 'acc_work',
  label: 'work',
  active: true,
  email: 'person@example.test',
  plan: 'max',
  usage: {
    harness: 'claude',
    plan: 'Max',
    windows: [
      { label: 'Session', used: 0.42, resetsAt: '' },
      { label: 'Weekly', used: 0.1, resetsAt: '' },
    ],
    checkedAt: '',
    status: '',
  },
})
const HOME = account({ id: 'acc_home', label: 'home' })
const STALE = account({ id: 'acc_stale', label: 'old', state: 'needs_login' })
const PINNED = account({
  id: 'acc_pinned',
  label: 'team',
  kind: 'subscription',
  inUse: true,
  pinnedTo: [{ scopeKind: 'workspace', scopeId: 'proj-1', tool: 'claude', accountId: 'acc_pinned', label: 'research' }],
})
const KEY = account({ id: 'acc_key', label: 'metered', kind: 'api_key', billedPerToken: true, plan: 'API key' })

function doc(): Record<string, unknown> {
  const notYet = (tool: string, display: string) => ({
    tool, display, supported: false, subscription: false, apiKeys: false, activeId: null, liveAccountId: null, loginMethod: null, accounts: [], pins: [],
  })
  const sub = (tool: string, display: string, accounts: unknown[], activeId: string | null) => ({
    tool, display, supported: true, subscription: true, apiKeys: true, activeId, liveAccountId: activeId, loginMethod: 'temp_home', accounts, pins: [],
  })
  return {
    tools: [
      sub('claude', 'Claude', [WORK, HOME, STALE, PINNED, KEY], 'acc_work'),
      sub('codex', 'Codex', [], null),
      sub('grok', 'Grok', [], null),
      { tool: 'gemini', display: 'Gemini', supported: true, subscription: false, apiKeys: true, activeId: null, liveAccountId: null, loginMethod: null, accounts: [], pins: [] },
      notYet('cursor', 'Cursor Agent'),
      notYet('pi', 'Pi'),
      notYet('hermes', 'Hermes'),
    ],
    logins: [],
    airgap: false,
    switchNote: 'Changing the server default token affects every chat that uses Server default on this server.',
  }
}

const LOGIN = {
  loginId: 'login_1',
  accountId: 'acc_new',
  tool: 'claude',
  label: 'team',
  mode: 'other_device',
  method: 'temp_home',
  state: 'waiting_for_code',
  url: 'https://example.test/oauth?x=1',
  code: 'ABCD-EFGH',
  error: null,
  screen: ['Paste code here if prompted >'],
  startedAt: 1,
  done: false,
  banner: null,
  offerMakeActive: false,
}

function routeGet(route: string, params?: Record<string, string>): unknown {
  if (route === 'llm/accounts/list') return doc()
  if (route === 'llm/accounts/login/status') return { login: { ...LOGIN, loginId: params?.loginId } }
  throw new Error(`unexpected GET ${route}`)
}

beforeEach(() => {
  resetLlmAccountsForTests()
  resetUsageLimitsForTests()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openUrl.mockClear()
  h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => routeGet(route, params))
  // Switches ask first; tests that look at the dialog replace this.
  useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) })
  h.daemonCliPost.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/add' || route === 'llm/accounts/login') return { account: account({ id: 'acc_new' }), login: LOGIN }
    if (route.startsWith('llm/accounts/')) return {}
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

async function renderColumn(): Promise<void> {
  render(<AgentAccountsColumn />)
  await screen.findByTestId('llm-account-acc_work')
}

describe('Tokens column', () => {
  it('renders the Subs / Tokens heading, plain token names, states, the Server default badge, usage and the switch note', async () => {
    await renderColumn()
    expect(screen.getByRole('heading', { name: 'Subs / Tokens' })).toBeTruthy()
    expect(screen.getByTestId('llm-switch-note').textContent).toBe(SWITCH_NOTE)
    const work = screen.getByTestId('llm-account-acc_work')
    expect(screen.getByTestId('llm-name-acc_work').textContent).toBe('Claude · Max · work')
    expect(screen.getByTestId('llm-name-acc_home').textContent).toBe('Claude · home')
    expect(work.textContent).toContain('Server default')
    expect(work.textContent).toContain('Signed in')
    expect(work.textContent).toContain('person@example.test')
    expect(screen.getByTestId('llm-usage-acc_work').textContent).toContain('Session 42%')
    expect(screen.getByTestId('llm-account-acc_home').textContent).not.toContain('Server default')
    expect(screen.getByTestId('llm-account-acc_stale').textContent).toContain('Needs sign-in')
    for (const t of ['cursor', 'pi', 'hermes']) {
      expect(screen.getByTestId(`llm-tool-${t}`).textContent).toContain('Not available yet')
    }
    expect(document.body.textContent).not.toContain('Coming soon')
    expect(screen.getAllByRole('button', { name: '+ Add subscription' })).toHaveLength(3)
    expect(screen.getAllByRole('button', { name: '+ Add API token' })).toHaveLength(4)
    expect(h.daemonCliGet).toHaveBeenCalledWith('llm/accounts/list')
    const column = document.querySelector('[data-settings-id="agents.accounts"]')!.textContent ?? ''
    expect(column).not.toMatch(/\blogins?\b|wallet|\bpool\b|pinned|Make active/i)
  })

  it('Use this token posts switch with the id', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_home')
    fireEvent.click(within(row).getByRole('button', { name: 'Use this token' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_home' }))
  })

  it('Use next token confirms the exact next token, then switches to it', async () => {
    await renderColumn()
    fireEvent.click(screen.getByRole('button', { name: 'Use next token' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_home' }))
    const opts = (useConfirmDialogStore.getState().confirm as ReturnType<typeof vi.fn>).mock.calls[0][0]
    expect(opts.message).toContain('From: Claude · Max · work')
    expect(opts.message).toContain('To: Claude · home')
    expect(document.body.textContent).toContain('K2 never switches on its own')
  })

  it('Remove asks, then posts remove; it is disabled on the server default', async () => {
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) })
    await renderColumn()
    const activeRemove = Array.from(screen.getByTestId('llm-account-acc_work').querySelectorAll('button')).find(
      (b) => b.textContent === 'Remove',
    )!
    expect((activeRemove as HTMLButtonElement).disabled).toBe(true)
    expect(activeRemove.getAttribute('title')).toBe(ACTIVE_REMOVE_REASON)
    const idleRemove = Array.from(screen.getByTestId('llm-account-acc_home').querySelectorAll('button')).find(
      (b) => b.textContent === 'Remove',
    )!
    fireEvent.click(idleRemove)
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/remove', { id: 'acc_home' }))
  })

  it('Rename posts the new label on Enter', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_home')
    fireEvent.click(Array.from(row.querySelectorAll('button')).find((b) => b.textContent === 'Rename')!)
    const input = screen.getByLabelText('New label')
    fireEvent.change(input, { target: { value: 'personal' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/rename', { id: 'acc_home', label: 'personal' }),
    )
  })

  it('tile actions are real buttons (bordered chips, not clickable text); Remove is the destructive style', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_home')
    const names = ['Use this token', 'Rename', 'Remove']
    for (const name of names) {
      const btn = within(row).getByRole('button', { name }) as HTMLButtonElement
      expect(btn.type).toBe('button')
      expect(btn.className).toContain('border')
      expect(btn.className).toContain('rounded-[var(--radius-field)]')
      expect(btn.className).toContain('focus-visible:outline')
    }
    expect(within(row).getByRole('button', { name: 'Remove' }).className).toContain('bg-[var(--color-danger-muted)]')
    expect(within(row).getByRole('button', { name: 'Use this token' }).className).toContain('bg-[var(--color-accent)]')
    const claude = screen.getByTestId('llm-tool-claude')
    for (const name of ['+ Add subscription', '+ Add API token']) {
      const btn = within(claude).getByRole('button', { name })
      expect(btn.className).toContain('rounded-[var(--radius-field)]')
    }
    expect(screen.getByRole('button', { name: 'Use next token' }).className).toContain('rounded-[var(--radius-field)]')
  })

  it('a daemon refusal shows its hint', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'busy', hint: 'another change is in progress' } }))
    })
    await renderColumn()
    fireEvent.click(screen.getByTestId('llm-account-acc_home').querySelector('button')!)
    expect((await screen.findByRole('alert')).textContent).toBe('another change is in progress')
  })

  it('the search manifest has the Tokens entries on Settings → Language Models → Subs / Tokens, not the old credentials row', () => {
    // 0.45.7: Tokens is its own page (id llm-tokens); the entry ids are unchanged.
    expect(LLM_TOKENS_MANIFEST.map((e) => e.id)).toEqual(['agents.accounts', 'agents.add-login'])
    expect(LLM_TOKENS_MANIFEST.every((e) => e.section === 'llm-tokens')).toBe(true)
    const llms = AGENTS_MANIFEST.map((e) => e.id)
    expect(llms).not.toContain('agents.accounts')
    expect(llms).not.toContain('agents.add-login')
    expect(llms).not.toContain('agents.credentials')
    expect(AGENTS_MANIFEST.every((e) => e.section === 'agents')).toBe(true)
    const acc = LLM_TOKENS_MANIFEST.find((e) => e.id === 'agents.accounts')!
    expect(acc.label).toBe('Subs / Tokens')
    // Old words still find it (was Settings → LLMs → Tokens).
    expect(acc.keywords).toEqual(expect.arrayContaining(['token', 'tokens', 'subscription', 'api token', 'login', 'switch', 'llm', 'llms', 'language models']))
  })

  it('Settings → Language Models → Subs / Tokens is the same column on its own page', async () => {
    render(<LlmTokensSection />)
    await screen.findByTestId('llm-account-acc_work')
    const page = document.querySelector('[data-llm-tokens-section]') as HTMLElement
    expect(page.className).toBe('max-w-3xl')
    expect(page.children.length).toBe(1)
    expect(page.firstElementChild?.getAttribute('data-settings-id')).toBe('agents.accounts')
    expect(within(page).getByRole('heading', { name: 'Subs / Tokens' })).toBeTruthy()
    expect(page.querySelector('[data-settings-id="agents.add-login"]')).not.toBeNull()
  })
})

describe('time until each token refreshes', () => {
  const T0 = Date.parse('2026-10-15T12:00:00Z')
  const iso = (ms: number) => new Date(ms).toISOString()

  function reading(windows: Array<{ label: string; used: number; resetsAt: string }>): Record<string, unknown> {
    return { harness: 'claude', plan: 'Max', windows, checkedAt: iso(T0), status: '' }
  }

  function docWithResets(): Record<string, unknown> {
    const d = doc() as { tools: Array<{ tool: string; accounts: unknown[] }> }
    const claude = d.tools.find((t) => t.tool === 'claude')!
    claude.accounts = [
      { ...WORK, usage: reading([
        { label: 'Session', used: 0.42, resetsAt: iso(T0 + 3 * 3_600_000) },
        { label: 'Weekly', used: 0.1, resetsAt: '' },
      ]) },
      { ...HOME, usage: reading([
        { label: 'Session', used: 1, resetsAt: iso(T0 - 5 * 60_000) },
        { label: 'Weekly', used: 0.6, resetsAt: iso(T0 + 4 * 86_400_000) },
      ]) },
      STALE,
    ]
    return d
  }

  it('shows each token its own reset time in the top bar format, nothing when unknown, "now" when past, and ticks', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    vi.setSystemTime(T0)
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) =>
      route === 'llm/accounts/list' ? docWithResets() : routeGet(route, params),
    )
    await renderColumn()

    const win = (id: string, label: string) => screen.getByTestId(`llm-usage-window-${id}-${label}`)
    // Known reset: the top bar's "Session 42% · 3h".
    expect(win('acc_work', 'Session').textContent).toBe('Session 42% · 3h')
    expect(win('acc_work', 'Session').getAttribute('title')).toBe('Session 42% used · refreshes in 3h')
    // Unknown reset: no time at all, not a guess.
    expect(win('acc_work', 'Weekly').textContent).toBe('Weekly 10%')
    expect(win('acc_work', 'Weekly').getAttribute('title')).toBeNull()
    // A different token counts down to its own reset; a past reset is being
    // re-checked (Usage v2 P8), never its old 100%.
    expect(win('acc_home', 'Session').textContent).toBe('Session · reset — re-checking')
    expect(win('acc_home', 'Weekly').textContent).toBe('Weekly 60% · 4d')
    // No reading: no windows.
    expect(screen.queryByTestId('llm-usage-window-acc_stale-Session')).toBeNull()

    // Two hours later (the page ticks once a minute): 1h left = "60m".
    await act(async () => {
      vi.advanceTimersByTime(2 * 3_600_000)
    })
    expect(win('acc_work', 'Session').textContent).toBe('Session 42% · 60m')
  })
})

describe('Workspace/chat tokens and API tokens', () => {
  it('Gemini offers only an API token', async () => {
    await renderColumn()
    const gem = screen.getByTestId('llm-tool-gemini')
    expect(within(gem).queryByRole('button', { name: '+ Add subscription' })).toBeNull()
    expect(within(gem).getByRole('button', { name: '+ Add API token' })).toBeTruthy()
    expect(gem.textContent).toContain('No API token on this server yet.')
  })

  it('Add API token posts tool, label and key; the key is never rendered after submit', async () => {
    await renderColumn()
    const claude = screen.getByTestId('llm-tool-claude')
    fireEvent.click(within(claude).getByRole('button', { name: '+ Add API token' }))
    const dialog = screen.getByTestId('llm-apikey-dialog')
    expect(dialog.textContent).toContain('Billed per token by the provider.')
    const keyInput = screen.getByLabelText('API token') as HTMLInputElement
    expect(keyInput.type).toBe('password')
    fireEvent.change(screen.getByLabelText('API token label'), { target: { value: 'metered' } })
    fireEvent.change(keyInput, { target: { value: 'sk-test-SECRET-123456' } })
    fireEvent.click(within(dialog).getByRole('button', { name: 'Add API token' }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/add-key', {
        tool: 'claude',
        label: 'metered',
        key: 'sk-test-SECRET-123456',
      }),
    )
    await waitFor(() => expect(screen.queryByTestId('llm-apikey-dialog')).toBeNull())
    expect(document.body.innerHTML).not.toContain('sk-test-SECRET-123456')
  })

  it('a key refusal keeps the dialog open with the hint and the key cleared', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'invalid_label', hint: 'invalid label: the API key must be 8–512 characters with no spaces' } }))
    })
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-tool-codex')).getByRole('button', { name: '+ Add API token' }))
    fireEvent.change(screen.getByLabelText('API token label'), { target: { value: 'k' } })
    fireEvent.change(screen.getByLabelText('API token'), { target: { value: 'short key' } })
    fireEvent.click(within(screen.getByTestId('llm-apikey-dialog')).getByRole('button', { name: 'Add API token' }))
    expect((await screen.findByRole('alert')).textContent).toContain('8–512 characters')
    expect((screen.getByLabelText('API token') as HTMLInputElement).value).toBe('')
  })

  it('API token rows read "API token · <label> · billed per token"', async () => {
    await renderColumn()
    expect(screen.getByTestId('llm-name-acc_key').textContent).toBe('API token · metered · billed per token')
    expect(screen.queryByTestId('llm-usage-acc_key')).toBeNull()
  })

  it('a token set for a workspace shows where, is in use, and refuses server-default actions with the reason', async () => {
    await renderColumn()
    expect(screen.getByTestId('llm-pinned-acc_pinned').textContent).toBe('Used by: research')
    expect(screen.getByTestId('llm-inuse-acc_pinned').textContent).toBe('In use by a chat right now')
    const row = screen.getByTestId('llm-account-acc_pinned')
    const make = within(row).getByRole('button', { name: 'Use this token' }) as HTMLButtonElement
    expect(make.disabled).toBe(true)
    expect(make.title).toContain("can't also be the server default")
    const remove = within(row).getByRole('button', { name: 'Remove' }) as HTMLButtonElement
    expect(remove.disabled).toBe(true)
    expect(remove.title).toContain('Set those back to Server default first')
    fireEvent.click(make)
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_pinned' })
  })

  it('the switch note says workspaces and chats with their own token keep it', async () => {
    await renderColumn()
    expect(SWITCH_NOTE).toContain('every chat that uses Server default')
    expect(SWITCH_NOTE).toContain('Workspaces and chats set to their own token keep it')
  })
})

describe('Sign-in sheet', () => {
  it('Add posts tool, label and mode, then offers Open sign-in page, Copy code and Paste code', async () => {
    const writeText = vi.fn(async () => {})
    Object.assign(navigator, { clipboard: { writeText } })
    await renderColumn()
    fireEvent.click(screen.getAllByRole('button', { name: '+ Add subscription' })[0])
    const sheet = screen.getByTestId('llm-login-sheet')
    expect(screen.getByRole('dialog', { name: 'Add a Claude subscription' })).toBeTruthy()
    fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
    fireEvent.click(screen.getByRole('button', { name: 'Another device' }))
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/add', { tool: 'claude', label: 'team', mode: 'other_device' }),
    )
    expect(sheet.textContent).toContain('Paste the code from the sign-in page')
    fireEvent.click(await screen.findByRole('button', { name: 'Open sign-in page' }))
    expect(h.openUrl).toHaveBeenCalledWith('https://example.test/oauth?x=1')
    expect(screen.getByTestId('llm-login-code').textContent).toBe('ABCD-EFGH')
    fireEvent.click(screen.getByRole('button', { name: 'Copy code' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('ABCD-EFGH'))
    fireEvent.change(screen.getByLabelText('Paste code'), { target: { value: ' pasted-code#state ' } })
    fireEvent.click(screen.getByRole('button', { name: 'Send' }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login/input', { loginId: 'login_1', text: 'pasted-code#state' }),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Show terminal' }))
    expect(screen.getByTestId('llm-login-terminal').textContent).toContain('Paste code here if prompted')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login/cancel', { loginId: 'login_1' }))
  })

  it('polls login/status, shows the live-swap banner, and offers to make the new subscription the server default', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let polls = 0
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route !== 'llm/accounts/login/status') return routeGet(route, params)
      polls++
      return {
        login: {
          ...LOGIN,
          method: 'live_swap',
          banner: 'Signing in temporarily switches this tool for every session on this server.',
          state: 'signed_in',
          done: true,
          offerMakeActive: true,
        },
      }
    })
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_stale')).getByRole('button', { name: 'Sign in again' }))
    expect(screen.getByRole('dialog', { name: 'Sign old in again' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login', { id: 'acc_stale', mode: 'this_computer' }),
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600)
    })
    expect(polls).toBeGreaterThanOrEqual(1)
    expect(screen.getByTestId('llm-login-banner').textContent).toContain('every session on this server')
    expect(screen.getByText('Make the new subscription the server default?')).toBeTruthy()
    fireEvent.click(within(screen.getByTestId('llm-login-sheet')).getByRole('button', { name: 'Use this token' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_new' }))
    expect(await screen.findByText('The new subscription is now the server default.')).toBeTruthy()
    // Done: polling stops.
    const after = polls
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(polls).toBe(after)
  })
})

describe('keychain repair warning', () => {
  const FIX = "security set-generic-password-partition-list -s 'Claude Code-credentials' -a 'appa' -S 'apple-tool:,apple:,teamid:36B8R93HXV'"

  it('shows no warning when the daemon reports none', async () => {
    await renderColumn()
    expect(screen.queryByTestId('llm-keychain-repair')).toBeNull()
  })

  it('shows each item that makes macOS prompt, with the exact Terminal fix', async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) =>
      route === 'llm/accounts/list'
        ? {
            ...doc(),
            keychainRepairs: [
              { service: 'Claude Code-credentials', account: 'appa', holds: 'server default', partitions: ['teamid:36B8R93HXV'], command: FIX },
              { service: 'Claude Code-credentials-1a2b3c4d', account: 'appa', holds: 'token "home"', partitions: ['teamid:36B8R93HXV'], command: 'x' },
              { service: 'no-command', account: 'appa', holds: 'server default', partitions: [] },
            ],
          }
        : routeGet(route, params),
    )
    await renderColumn()
    const warnings = screen.getAllByTestId('llm-keychain-repair')
    expect(warnings).toHaveLength(2)
    expect(warnings[0].getAttribute('role')).toBe('alert')
    expect(warnings[0].textContent).toContain('Claude Code-credentials')
    expect(warnings[0].textContent).toContain('server default')
    expect(warnings[0].textContent).toContain('Run this once in Terminal')
    expect(warnings[0].querySelector('code')!.textContent).toBe(FIX)
    expect(warnings[1].textContent).toContain('token "home"')
  })
})

describe('store helpers', () => {
  it('parses a malformed list into an empty doc and reads a refusal hint', () => {
    expect(parseAccountsDoc(null)).toEqual({ tools: [], logins: [], airgap: false, switchNote: '', keychainRepairs: [] })
    expect(errorText(new Error('{"error":{"code":"owner_only","hint":"Change tokens on Settings → LLMs → Tokens."}}'))).toBe(
      'Change tokens on Settings → LLMs → Tokens.',
    )
    expect(errorText(new Error('plain'))).toBe('plain')
  })
})

describe('Usage v2 limit bars from /cli/usage/limits (Owner / Admin)', () => {
  const T0 = Date.parse('2026-10-16T12:00:00Z')
  const iso = (ms: number) => new Date(ms).toISOString()
  const week = 7 * 24 * 3600

  function limitsDoc(): Record<string, unknown> {
    return {
      generatedAt: iso(T0),
      airgap: false,
      tools: [
        {
          tool: 'claude',
          display: 'Claude',
          supported: true,
          logins: [
            {
              accountId: 'acc_work',
              label: 'work',
              kind: 'subscription',
              isDefault: true,
              isLive: true,
              pinned: [],
              state: 'ok',
              status: '',
              plan: 'Max',
              checkedAt: iso(T0 - 2 * 60_000),
              source: 'live_statusline',
              windows: [
                { id: 'five_hour', label: 'Session', used: 0.2, usedPercent: 20, resetsAt: iso(T0 + 3 * 3_600_000) },
                {
                  id: 'seven_day',
                  label: 'Weekly',
                  used: 0.92,
                  usedPercent: 92,
                  resetsAt: iso(T0 + 3 * 86_400_000),
                  windowSecs: week,
                  windowStart: iso(T0 + 3 * 86_400_000 - week * 1000),
                },
                { id: 'model:fable:weekly', label: 'Fable Weekly', used: 0.5, usedPercent: 50, resetsAt: iso(T0 + 3 * 86_400_000) },
              ],
            },
            {
              accountId: 'acc_home',
              label: 'home',
              kind: 'subscription',
              isDefault: false,
              isLive: false,
              pinned: [],
              state: 'expired',
              status: 'Sign-in expired',
              plan: '',
              checkedAt: iso(T0 - 3 * 3_600_000),
              nextProbeAt: iso(T0 + 30 * 60_000),
              windows: [],
            },
          ],
        },
      ],
    }
  }

  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    vi.setSystemTime(T0)
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) =>
      route === 'usage/limits' ? limitsDoc() : routeGet(route, params),
    )
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'usage/limits/refresh') return limitsDoc()
      if (route.startsWith('llm/accounts/')) return {}
      throw new Error(`unexpected POST ${route}`)
    })
  })

  afterEach(() => {
    resetUsageLimitsForTests()
  })

  it('draws every window (model-scoped weeks too, B30) with tones, a pace mark and freshness', async () => {
    await renderColumn()
    await waitFor(() => expect(screen.getByTestId('llm-usage-window-acc_work-Fable Weekly')).toBeTruthy())
    const meter = screen.getByTestId('llm-usage-acc_work')
    expect([...meter.querySelectorAll('[data-limit-tone]')].map((el) => el.getAttribute('data-limit-tone'))).toEqual([
      'ok',
      'over',
      'ok',
    ])
    expect(meter.querySelectorAll('[data-testid="limit-pace-mark"]')).toHaveLength(1)
    // Rosson 2026-10-16: the Subs / Tokens bars were hair-thin (4px on the
    // border colour). Every bar is now 6px on the visible control track.
    const bars = [...meter.querySelectorAll('[role="progressbar"]')]
    expect(bars).toHaveLength(3)
    for (const bar of bars) {
      expect(bar.className.split(' ')).toContain('h-1.5')
      expect(bar.className.split(' ')).not.toContain('h-1')
      const track = bar.querySelector('[data-testid="limit-bar-track"]')
      if (!track) throw new Error('limit bar has no track')
      expect(track.className.split(' ')).toContain('bg-[var(--color-control-track-off)]')
      expect(track.className.split(' ')).toContain('rounded-[var(--radius-selector)]')
    }
    expect(screen.getByTestId('llm-usage-window-acc_work-Weekly').textContent).toBe('Weekly 92% · 3d · ahead of pace')
    expect(screen.getByTestId('llm-usage-age-acc_work').textContent).toContain('Usage checked 2 min ago · live from a running session')
  })

  it('says why an expired login has no numbers, when it is next checked, and offers a Refresh', async () => {
    await renderColumn()
    await waitFor(() => expect(screen.getByTestId('llm-usage-state-acc_home')).toBeTruthy())
    expect(screen.getByTestId('llm-usage-state-acc_home').textContent).toBe('Sign-in expired. Sign in again to see its limits.')
    expect(screen.getByTestId('llm-usage-age-acc_home').textContent).toContain('Usage checked 3 h ago · next check 30m')
    fireEvent.click(screen.getByTestId('llm-usage-refresh-acc_home'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('usage/limits/refresh', { accountId: 'acc_home' }),
    )
  })

  it("a Member's 403 leaves each row on its wallet reading and offers no per-login Refresh", async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route === 'usage/limits') throw new DaemonHttpError('Usage is visible to the server Owner and Admins', 403, 'admin_only')
      return routeGet(route, params)
    })
    await renderColumn()
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('usage/limits'))
    expect(screen.getByTestId('llm-usage-acc_work').textContent).toContain('Session 42%')
    expect(screen.queryByTestId('llm-usage-refresh-acc_work')).toBeNull()
  })
})
