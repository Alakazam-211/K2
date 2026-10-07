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

import { AgentAccountsColumn, SWITCH_NOTE, ACTIVE_REMOVE_REASON } from './AgentAccountsColumn'
import { resetLlmAccountsForTests, parseAccountsDoc, errorText } from '@/stores/llm-accounts'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { AGENTS_MANIFEST } from '../sections/AgentsSection'

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

describe('Tokens column', () => {
  it('renders the Tokens heading, plain token names, states, the Server default badge, usage and the switch note', async () => {
    await renderColumn()
    expect(screen.getByRole('heading', { name: 'Tokens' })).toBeTruthy()
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

  it('Use next token posts next with the tool', async () => {
    await renderColumn()
    fireEvent.click(screen.getByRole('button', { name: 'Use next token' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/next', { tool: 'claude' }))
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

  it('a daemon refusal shows its hint', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'busy', hint: 'another change is in progress' } }))
    })
    await renderColumn()
    fireEvent.click(screen.getByTestId('llm-account-acc_home').querySelector('button')!)
    expect((await screen.findByRole('alert')).textContent).toBe('another change is in progress')
  })

  it('the search manifest has the Tokens entries, not the old credentials row', () => {
    const ids = AGENTS_MANIFEST.map((e) => e.id)
    expect(ids).toContain('agents.accounts')
    expect(ids).toContain('agents.add-login')
    expect(ids).not.toContain('agents.credentials')
    const acc = AGENTS_MANIFEST.find((e) => e.id === 'agents.accounts')!
    expect(acc.label).toBe('Tokens')
    expect(acc.keywords).toEqual(expect.arrayContaining(['token', 'subscription', 'api token', 'login', 'switch']))
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
    fireEvent.click(screen.getByRole('button', { name: 'Sign in again' }))
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

describe('store helpers', () => {
  it('parses a malformed list into an empty doc and reads a refusal hint', () => {
    expect(parseAccountsDoc(null)).toEqual({ tools: [], logins: [], airgap: false, switchNote: '' })
    expect(errorText(new Error('{"error":{"code":"owner_only","hint":"Change tokens on Settings → LLMs."}}'))).toBe(
      'Change tokens on Settings → LLMs.',
    )
    expect(errorText(new Error('plain'))).toBe('plain')
  })
})
