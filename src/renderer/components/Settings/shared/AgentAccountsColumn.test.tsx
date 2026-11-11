// @vitest-environment jsdom
//
// Settings → Language Models → Subs / Tokens + the sign-in sheet. Daemon
// calls are mocked; routes and bodies are asserted exactly. Fail loud: an
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
  PAGE_INTRO,
  SWITCH_NOTE,
  KEEP_OWN_NOTE,
  ACTIVE_REMOVE_REASON,
  ADMIN_SWITCH_REASON,
  noDefaultText,
  switchConfirmText,
} from './AgentAccountsColumn'
import { resetLlmAccountsForTests, parseAccountsDoc, errorText } from '@/stores/llm-accounts'
import { resetUsageLimitsForTests } from '@/stores/usage-limits'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { AGENTS_MANIFEST } from '../sections/AgentsSection'
import { LLM_TOKENS_MANIFEST, LlmTokensSection } from '../sections/LlmTokensSection'
import { useLlmSignInRequestStore } from '@/stores/llm-sign-in-request'

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

/** Open a token's "…" menu and return the item whose label starts with `name`. */
function menuItem(label: string, name: string): HTMLElement {
  fireEvent.click(screen.getByRole('button', { name: `More actions for ${label}` }))
  return screen.getByRole('menuitem', { name: new RegExp(`^${name}`) })
}

/** The open confirm dialog's button. */
function confirmIn(testId: string, name: string): void {
  fireEvent.click(within(screen.getByTestId(testId)).getByRole('button', { name }))
}

describe('Tokens column', () => {
  it('renders the page header, a card per provider, plain token names, states, the Server default tag and usage', async () => {
    await renderColumn()
    expect(screen.getByRole('heading', { name: 'Subs / Tokens' })).toBeTruthy()
    expect(screen.getByTestId('llm-switch-note').textContent).toBe(`${PAGE_INTRO} ${SWITCH_NOTE}`)
    const work = screen.getByTestId('llm-account-acc_work')
    // The provider is the card's title: a row names the token, then says what it is.
    expect(screen.getByTestId('llm-name-acc_work').textContent).toBe('work')
    expect(screen.getByTestId('llm-kind-acc_work').textContent).toBe('Max subscription · person@example.test')
    expect(screen.getByTestId('llm-name-acc_home').textContent).toBe('home')
    expect(screen.getByTestId('llm-kind-acc_home').textContent).toBe('Subscription')
    expect(within(screen.getByTestId('llm-tool-claude')).getByRole('heading', { name: 'Claude' })).toBeTruthy()
    expect(work.textContent).toContain('Server default')
    expect(screen.queryByTestId('llm-state-acc_work')).toBeNull()
    expect(screen.getByTestId('llm-usage-acc_work').textContent).toContain('Session 42%')
    expect(screen.getByTestId('llm-account-acc_home').textContent).not.toContain('Server default')
    expect(screen.getByTestId('llm-state-acc_stale').textContent).toBe('Needs sign-in')
    // Tools without tokens yet: one quiet line, no card each.
    for (const t of ['cursor', 'pi', 'hermes']) expect(screen.queryByTestId(`llm-tool-${t}`)).toBeNull()
    expect(screen.getByTestId('llm-not-yet').textContent).toBe('Cursor Agent, Hermes and Pi can’t use saved tokens yet.')
    expect(document.body.textContent).not.toContain('Coming soon')
    // One primary action; an empty provider offers its own Add.
    const add = screen.getByRole('button', { name: 'Add a token' })
    expect(add.className).toContain('bg-[var(--color-accent-fill)]')
    for (const p of ['Codex', 'Grok', 'Gemini']) expect(screen.getByRole('button', { name: `Add a ${p} token` })).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Add a Claude token' })).toBeNull()
    expect(h.daemonCliGet).toHaveBeenCalledWith('llm/accounts/list')
    const column = document.querySelector('[data-settings-id="agents.accounts"]')!.textContent ?? ''
    expect(column).not.toMatch(/\blogins?\b|wallet|\bpool\b|pinned|Make active/i)
  })

  it('marks a token K2 saved from a sign-in made outside K2, and only that one', async () => {
    const outside = account({ id: 'acc_outside', label: 'someone@example.test', email: 'someone@example.test', addedOutsideK2: true, createdBy: 'outside-k2' })
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route !== 'llm/accounts/list') return routeGet(route, params)
      const d = doc() as { tools: Array<{ tool: string; accounts: unknown[] }> }
      d.tools.find((t) => t.tool === 'claude')!.accounts.push(outside)
      return d
    })
    await renderColumn()
    const chip = screen.getByTestId('llm-outside-acc_outside')
    expect(chip.textContent).toBe('Added outside K2')
    expect(chip.getAttribute('title')).toContain('outside K2')
    expect(screen.queryByTestId('llm-outside-acc_work')).toBeNull()
    expect(screen.queryByTestId('llm-outside-acc_home')).toBeNull()
  })

  it('Make default asks in the shared dialog, then posts switch with the id and says so', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_home')
    fireEvent.click(within(row).getByRole('button', { name: 'Make default' }))
    const dialog = screen.getByRole('dialog', { name: 'Make “home” the Claude server default?' })
    expect(dialog.textContent).toContain('From: Claude · Max · work')
    expect(dialog.textContent).toContain(KEEP_OWN_NOTE)
    // Cancel has the focus: Enter never switches by accident.
    expect(document.activeElement?.textContent).toBe('Cancel')
    confirmIn('llm-switch-confirm', 'Make default')
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_home' }))
    expect((await screen.findByTestId('llm-notice')).textContent).toContain('“home” is now the Claude server default.')
  })

  it('Cancel in the switch dialog sends nothing', async () => {
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_home')).getByRole('button', { name: 'Make default' }))
    confirmIn('llm-switch-confirm', 'Cancel')
    expect(screen.queryByTestId('llm-switch-confirm')).toBeNull()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
  })

  it('Use next token confirms the exact next token, then switches to it', async () => {
    await renderColumn()
    fireEvent.click(screen.getByRole('button', { name: 'Use next token' }))
    const dialog = screen.getByTestId('llm-switch-confirm')
    expect(dialog.textContent).toContain('From: Claude · Max · work')
    expect(dialog.textContent).toContain('To: Claude · home')
    confirmIn('llm-switch-confirm', 'Make default')
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_home' }))
    expect(document.body.textContent).toContain('K2 never switches on its own')
  })

  it('Remove is in the row menu, asks first (quiet, destructive), then posts remove; it is off on the server default', async () => {
    await renderColumn()
    const activeRemove = menuItem('work', 'Remove')
    expect(activeRemove.getAttribute('aria-disabled')).toBe('true')
    expect(activeRemove.textContent).toContain(ACTIVE_REMOVE_REASON)
    fireEvent.click(activeRemove)
    expect(screen.queryByTestId('llm-remove-confirm')).toBeNull()
    fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' })
    fireEvent.click(menuItem('home', 'Remove'))
    const dialog = screen.getByRole('dialog', { name: 'Remove “home”?' })
    expect(dialog.textContent).toContain('K2 stops using this Claude subscription on this server.')
    expect(within(dialog).getByRole('button', { name: 'Remove' }).className).toContain('bg-[var(--color-danger-muted)]')
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/remove', expect.anything())
    confirmIn('llm-remove-confirm', 'Remove')
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/remove', { id: 'acc_home' }))
    expect((await screen.findByTestId('llm-notice')).textContent).toContain('“home” was removed from this server.')
  })

  it('the server default API token: No default and Remove (with clearDefault) after a confirm; Admin and above only', async () => {
    // Smoke r6: the only token, an API token, is the server default.
    const keyDoc = (canChangeDefault: boolean): Record<string, unknown> => {
      const d = doc()
      const tools = d.tools as Array<Record<string, unknown>>
      tools[0] = { ...tools[0], accounts: [{ ...KEY, active: true }], activeId: 'acc_key', liveAccountId: null }
      return { ...d, canChangeDefault, changeDefaultReason: canChangeDefault ? null : ADMIN_SWITCH_REASON }
    }
    h.daemonCliGet.mockImplementation(async (route: string) => (route === 'llm/accounts/list' ? keyDoc(true) : routeGet(route)))
    render(<AgentAccountsColumn />)
    await screen.findByTestId('llm-account-acc_key')
    const remove = menuItem('metered', 'Remove')
    expect(remove.getAttribute('aria-disabled')).toBeNull()
    fireEvent.click(remove)
    expect(screen.getByTestId('llm-remove-confirm').textContent).toContain('Claude then has no server default')
    confirmIn('llm-remove-confirm', 'Remove')
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/remove', { id: 'acc_key', clearDefault: true }),
    )
    fireEvent.click(menuItem('metered', 'Stop using as default'))
    expect(screen.getByTestId('llm-no-default-confirm').textContent).toContain('“metered” stays saved.')
    confirmIn('llm-no-default-confirm', 'Stop using as default')
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/clear-default', { tool: 'claude' }))
    expect(noDefaultText({ display: 'Claude', activeId: 'acc_key', liveAccountId: 'acc_work', accounts: [WORK as never] })).toContain(
      'still signed in to Claude itself',
    )
    cleanup()
    // A Member: both are off, with the reason.
    resetLlmAccountsForTests()
    h.daemonCliGet.mockImplementation(async (route: string) => (route === 'llm/accounts/list' ? keyDoc(false) : routeGet(route)))
    render(<AgentAccountsColumn />)
    await screen.findByTestId('llm-account-acc_key')
    fireEvent.click(screen.getByRole('button', { name: 'More actions for metered' }))
    for (const name of [/^Remove/, /^Stop using as default/]) {
      const item = screen.getByRole('menuitem', { name })
      expect(item.getAttribute('aria-disabled')).toBe('true')
      expect(item.textContent).toContain(ADMIN_SWITCH_REASON)
    }
  })

  it('Rename opens a dialog, posts the new label on Enter and says so', async () => {
    await renderColumn()
    fireEvent.click(menuItem('home', 'Rename'))
    expect(screen.getByRole('dialog', { name: 'Rename “home”' })).toBeTruthy()
    const input = screen.getByLabelText('New label')
    fireEvent.change(input, { target: { value: 'personal' } })
    fireEvent.submit(input.closest('form')!)
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/rename', { id: 'acc_home', label: 'personal' }),
    )
    expect((await screen.findByTestId('llm-notice')).textContent).toContain('“home” is now called “personal”.')
    expect(screen.queryByTestId('llm-rename-dialog')).toBeNull()
  })

  it('a failed rename keeps the dialog open with the server’s reason', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'llm/accounts/rename') throw new Error(JSON.stringify({ error: { code: 'invalid_label', hint: 'that name is taken' } }))
      return {}
    })
    await renderColumn()
    fireEvent.click(menuItem('home', 'Rename'))
    fireEvent.change(screen.getByLabelText('New label'), { target: { value: 'work' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save name' }))
    const alert = await within(screen.getByTestId('llm-rename-dialog')).findByRole('alert')
    expect(alert.textContent).toContain('Couldn’t rename it.')
    expect(alert.textContent).toContain('The server said: that name is taken.')
  })

  it('row actions are the shared buttons with keyboard focus; the rest sit in the row menu', async () => {
    await renderColumn()
    const row = screen.getByTestId('llm-account-acc_home')
    for (const name of ['Make default', 'More actions for home']) {
      const btn = within(row).getByRole('button', { name }) as HTMLButtonElement
      expect(btn.type).toBe('button')
      expect(btn.className).toContain('k2-focus')
      expect(btn.className).toContain('rounded-[var(--radius-field)]')
    }
    // The row's button is quiet: the page's one accent is Add a token.
    expect(within(row).getByRole('button', { name: 'Make default' }).className).not.toContain('bg-[var(--color-accent-fill)]')
    expect(within(screen.getByTestId('llm-account-acc_stale')).getByRole('button', { name: 'Sign in again' })).toBeTruthy()
    fireEvent.click(within(row).getByRole('button', { name: 'More actions for home' }))
    const items = screen.getAllByRole('menuitem').map((m) => m.getAttribute('data-menu-item'))
    expect(items).toEqual(['rename', 'relogin', 'remove'])
    expect(screen.getByRole('button', { name: 'Use next token' }).className).toContain('rounded-[var(--radius-field)]')
  })

  it('a daemon refusal says what failed and the server’s hint, in that provider’s card', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'busy', hint: 'another change is in progress' } }))
    })
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_home')).getByRole('button', { name: 'Make default' }))
    confirmIn('llm-switch-confirm', 'Make default')
    const alert = await within(screen.getByTestId('llm-tool-claude')).findByTestId('llm-row-error')
    expect(alert.getAttribute('role')).toBe('alert')
    expect(alert.textContent).toContain('Couldn’t change the server default. Nothing changed; try again.')
    expect(alert.textContent).toContain('The server said: another change is in progress.')
    fireEvent.click(within(alert).getByRole('button', { name: 'Dismiss' }))
    expect(screen.queryByTestId('llm-row-error')).toBeNull()
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
    expect(page.className).toBe('max-w-[52rem]')
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

  it('marks exactly the bar that drives the top-bar number', async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route !== 'llm/accounts/list') return routeGet(route, params)
      const d = docWithResets() as { tools: Array<{ tool: string; accounts: Array<Record<string, unknown>> }> }
      const claude = d.tools.find((t) => t.tool === 'claude')!
      const work = claude.accounts.find((a) => a.id === WORK.id) as { usage: Record<string, unknown> }
      work.usage = { ...work.usage, chipWindow: { id: '', label: 'Session', percent: 42, observedAt: null } }
      return d
    })
    await renderColumn()
    const marks = await screen.findAllByTestId('limit-bar-chip-mark')
    expect(marks).toHaveLength(1)
    expect(marks[0].closest('[data-drives-chip]')?.textContent).toContain('Session')
    expect(screen.getByTestId('llm-usage-window-acc_work-Weekly').querySelector('[data-testid="limit-bar-chip-mark"]')).toBeNull()
  })

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
  it('Gemini offers only an API token: its Add opens the sheet straight on the key field', async () => {
    await renderColumn()
    const gem = screen.getByTestId('llm-tool-gemini')
    expect(gem.textContent).toContain('API tokens only.')
    expect(screen.getByTestId('llm-empty-gemini').textContent).toContain('Paste a Gemini API token.')
    fireEvent.click(within(gem).getByRole('button', { name: 'Add a Gemini token' }))
    const sheet = screen.getByTestId('llm-login-sheet')
    expect((within(sheet).getByRole('radio', { name: /^Gemini/ }) as HTMLInputElement).checked).toBe(true)
    expect(within(sheet).queryByRole('radio', { name: 'Sign in with a subscription' })).toBeNull()
    expect((screen.getByLabelText('API token') as HTMLInputElement).type).toBe('password')
  })

  it('Add a token → provider → Paste an API token posts tool, label and key; the key is never shown again', async () => {
    await renderColumn()
    fireEvent.click(screen.getByRole('button', { name: 'Add a token' }))
    const sheet = screen.getByTestId('llm-login-sheet')
    fireEvent.click(within(sheet).getByRole('radio', { name: /^Claude/ }))
    fireEvent.click(within(sheet).getByRole('radio', { name: 'Paste an API token' }))
    const keyInput = screen.getByLabelText('API token') as HTMLInputElement
    expect(keyInput.type).toBe('password')
    fireEvent.click(within(sheet).getByRole('button', { name: 'Show' }))
    expect(keyInput.type).toBe('text')
    fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'metered' } })
    fireEvent.change(keyInput, { target: { value: 'sk-test-SECRET-123456' } })
    fireEvent.click(within(sheet).getByRole('button', { name: 'Add API token' }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/add-key', {
        tool: 'claude',
        label: 'metered',
        key: 'sk-test-SECRET-123456',
      }),
    )
    expect(await screen.findByRole('dialog', { name: 'API token added' })).toBeTruthy()
    expect(screen.getByTestId('llm-key-tail').textContent).toContain('•••• 3456')
    expect(document.body.innerHTML).not.toContain('sk-test-SECRET-123456')
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(screen.queryByTestId('llm-login-sheet')).toBeNull()
    expect(within(screen.getByTestId('llm-tool-claude')).getByTestId('llm-notice').textContent).toContain(
      'Claude API token “metered” was added.',
    )
  })

  it('a key refusal keeps the sheet open: what failed, the hint, the next step, and the key cleared', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'llm/accounts/add-key') {
        throw new Error(JSON.stringify({ error: { code: 'invalid_label', hint: 'invalid label: the API key must be 8–512 characters with no spaces' } }))
      }
      return {}
    })
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-tool-codex')).getByRole('button', { name: 'Add a Codex token' }))
    fireEvent.click(screen.getByRole('radio', { name: 'Paste an API token' }))
    fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'k' } })
    fireEvent.change(screen.getByLabelText('API token'), { target: { value: 'short key' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add API token' }))
    const alert = await screen.findByTestId('dialog-error')
    expect(alert.textContent).toContain('K2 couldn’t add the API token.')
    expect(alert.textContent).toContain('8–512 characters')
    expect((screen.getByLabelText('API token') as HTMLInputElement).value).toBe('')
  })

  it('API token rows say what they are, with no usage bars', async () => {
    await renderColumn()
    expect(screen.getByTestId('llm-name-acc_key').textContent).toBe('metered')
    expect(screen.getByTestId('llm-kind-acc_key').textContent).toBe('API token · billed per token')
    expect(screen.queryByTestId('llm-usage-acc_key')).toBeNull()
  })

  it('a token set for a workspace shows where, is in use, and refuses server-default actions with the reason', async () => {
    await renderColumn()
    expect(screen.getByTestId('llm-facts-acc_pinned').textContent).toBe('Used by research · In use by a chat right now')
    const row = screen.getByTestId('llm-account-acc_pinned')
    const make = within(row).getByRole('button', { name: 'Make default' }) as HTMLButtonElement
    expect(make.disabled).toBe(true)
    expect(make.title).toContain("can't also be the server default")
    fireEvent.click(make)
    expect(screen.queryByTestId('llm-switch-confirm')).toBeNull()
    const remove = menuItem('team', 'Remove')
    expect(remove.getAttribute('aria-disabled')).toBe('true')
    expect(remove.textContent).toContain('Set those back to Server default first')
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_pinned' })
  })

  it('the switch confirm says workspaces and chats with their own token keep it', () => {
    const d = parseAccountsDoc(doc())
    const out = switchConfirmText(d.tools[0], d.tools[0].accounts[0], d.tools[0].accounts[1])
    expect(SWITCH_NOTE).toContain('every chat set to Server default')
    expect(out.message).toContain('Workspaces and chats that picked their own token keep it')
  })
})

describe('Sign-in sheet', () => {
  it('a sign-in asked for from outside Settings opens the sheet for that tool once the list loads', async () => {
    act(() => useLlmSignInRequestStore.getState().request({ tool: 'codex', toolLabel: 'Codex' }))
    await renderColumn()
    const sheet = await screen.findByRole('dialog', { name: 'Add a token' })
    expect(within(sheet).getByText('Codex runs its own sign-in on the server. K2 never sees your password.')).toBeTruthy()
    expect(useLlmSignInRequestStore.getState().pending).toBeNull()
  })

  it('a request for a tool that cannot sign in falls back to the first of Claude, Codex and Grok that can', async () => {
    act(() => useLlmSignInRequestStore.getState().request({ tool: 'gemini', toolLabel: 'Gemini' }))
    await renderColumn()
    const sheet = await screen.findByRole('dialog', { name: 'Add a token' })
    expect(within(sheet).getByText('Claude runs its own sign-in on the server. K2 never sees your password.')).toBeTruthy()
  })

  it('in air-gap mode a request opens no sheet', async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) =>
      route === 'llm/accounts/list' ? { ...(doc() as object), airgap: true } : routeGet(route, params),
    )
    act(() => useLlmSignInRequestStore.getState().request())
    await renderColumn()
    await waitFor(() => expect(useLlmSignInRequestStore.getState().pending).toBeNull())
    expect(screen.queryByTestId('llm-login-sheet')).toBeNull()
  })

  it('Add a token → Claude → sign in posts tool, label and mode, then offers Open sign-in page, Copy code and Paste code', async () => {
    const writeText = vi.fn(async () => {})
    Object.assign(navigator, { clipboard: { writeText } })
    await renderColumn()
    fireEvent.click(screen.getByRole('button', { name: 'Add a token' }))
    const sheet = screen.getByTestId('llm-login-sheet')
    expect(screen.getByRole('dialog', { name: 'Add a token' })).toBeTruthy()
    // Nothing chosen yet: starting says so and sends nothing.
    fireEvent.click(screen.getByTestId('llm-login-start'))
    expect(within(sheet).getByText('Choose a provider.')).toBeTruthy()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/add', expect.anything())
    fireEvent.click(within(sheet).getByRole('radio', { name: /^Claude/ }))
    fireEvent.change(screen.getByTestId('llm-login-label'), { target: { value: 'team' } })
    fireEvent.click(screen.getByRole('radio', { name: 'Another device' }))
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
    fireEvent.click(screen.getByRole('button', { name: 'Cancel sign-in' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login/cancel', { loginId: 'login_1' }))
    await waitFor(() => expect(screen.queryByTestId('llm-login-sheet')).toBeNull())
  })

  it('polls login/status, shows the live-swap banner, then says what is signed in and offers to make it the server default', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    let polls = 0
    const banner = 'Signing in temporarily switches this tool for every session on this server.'
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route !== 'llm/accounts/login/status') return routeGet(route, params)
      polls++
      const finished = polls >= 2
      return {
        login: {
          ...LOGIN,
          label: 'old',
          method: 'live_swap',
          banner,
          state: finished ? 'signed_in' : 'waiting_for_browser',
          done: finished,
          offerMakeActive: finished,
        },
      }
    })
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_stale')).getByRole('button', { name: 'Sign in again' }))
    expect(screen.getByRole('dialog', { name: 'Sign “old” in again' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login', { id: 'acc_stale', mode: 'this_computer' }),
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600)
    })
    expect(polls).toBe(1)
    expect(screen.getByTestId('llm-login-banner').textContent).toContain('every session on this server')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600)
    })
    expect(screen.getByRole('dialog', { name: 'Signed in to Claude' })).toBeTruthy()
    expect(screen.getByTestId('llm-login-done').textContent).toContain('Claude “old” is signed in and saved on')
    expect(screen.getByText('Make it the Claude server default?')).toBeTruthy()
    fireEvent.click(within(screen.getByTestId('llm-login-sheet')).getByRole('button', { name: 'Make it the default' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/switch', { id: 'acc_new' }))
    expect((await screen.findByTestId('llm-login-made-default')).textContent).toBe('It’s now the Claude server default.')
    // The page says so too, in Claude's card.
    expect(within(screen.getByTestId('llm-tool-claude')).getByTestId('llm-notice').textContent).toContain('“old” is signed in again.')
    // Done: polling stops.
    const after = polls
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000)
    })
    expect(polls).toBe(after)
  })

  it('a Member is never offered "make it the server default" after a sign-in', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true })
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route === 'llm/accounts/list') {
        return { ...doc(), canChangeDefault: false, changeDefaultReason: 'admins only' }
      }
      if (route !== 'llm/accounts/login/status') return routeGet(route, params)
      return { login: { ...LOGIN, state: 'signed_in', done: true, offerMakeActive: true } }
    })
    await renderColumn()
    fireEvent.click(within(screen.getByTestId('llm-account-acc_stale')).getByRole('button', { name: 'Sign in again' }))
    fireEvent.click(screen.getByTestId('llm-login-start'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/login', { id: 'acc_stale', mode: 'this_computer' }),
    )
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1600)
    })
    expect(screen.queryByText('Make it the Claude server default?')).toBeNull()
    expect(screen.getByRole('button', { name: 'Done' })).toBeTruthy()
    expect(h.daemonCliPost).not.toHaveBeenCalledWith('llm/accounts/switch', expect.anything())
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
    expect(parseAccountsDoc(null)).toEqual({
      tools: [],
      logins: [],
      airgap: false,
      switchNote: '',
      keychainRepairs: [],
      canChangeDefault: true,
      changeDefaultReason: null,
    })
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
    fireEvent.click(menuItem('home', 'Check usage now'))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('usage/limits/refresh', { accountId: 'acc_home' }),
    )
  })

  it("a Member's 403 leaves each row on its wallet reading and offers no per-login check", async () => {
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, string>) => {
      if (route === 'usage/limits') throw new DaemonHttpError('Usage is visible to the server Owner and Admins', 403, 'admin_only')
      return routeGet(route, params)
    })
    await renderColumn()
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('usage/limits'))
    expect(screen.getByTestId('llm-usage-acc_work').textContent).toContain('Session 42%')
    fireEvent.click(screen.getByRole('button', { name: 'More actions for work' }))
    expect(screen.queryByRole('menuitem', { name: /^Check usage now/ })).toBeNull()
  })
})
