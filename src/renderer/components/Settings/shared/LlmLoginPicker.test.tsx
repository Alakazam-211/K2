// @vitest-environment jsdom
//
// LLM login pins: the workspace "LLM logins" group and the chat header
// login picker. Daemon calls are mocked; routes and bodies are asserted
// exactly. Fail loud: an unexpected route throws.

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

import { WorkspaceLlmLogins, SessionLoginPicker, POOL_LABEL, SESSION_PIN_NOTE, toolForProvider } from './LlmLoginPicker'
import { resetLlmAccountsForTests, CLAUDE_PIN_NOTE } from '@/stores/llm-accounts'
import { primaryScope } from '@/kessel/server-scope'
import { PROJECTS_MANIFEST } from '../sections/ProjectsSection'

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x', tool: 'claude', label: 'x', kind: 'subscription', active: false, state: 'signed_in',
    detail: null, plan: null, expiresAt: null, refreshedAt: null, lastUsedAt: null, createdAt: 1,
    createdBy: null, usage: null, usageCheckedAt: null, pinnedTo: [], inUse: false, ...over,
  }
}

const LIVE = account({ id: 'acc_live', label: 'main', active: true })
const SPARE = account({ id: 'acc_spare', label: 'spare' })
const KEY = account({ id: 'acc_key', label: 'metered', kind: 'api_key', billedPerToken: true })

let pins: Array<Record<string, unknown>> = []

function doc(): Record<string, unknown> {
  const t = (tool: string, display: string, accounts: unknown[], activeId: string | null, subscription = true) => ({
    tool, display, supported: true, subscription, apiKeys: true, activeId, liveAccountId: activeId,
    loginMethod: subscription ? 'temp_home' : null, accounts,
    pins: pins.filter((p) => p.tool === tool),
  })
  return {
    tools: [
      t('claude', 'Claude', [LIVE, SPARE, KEY], 'acc_live'),
      t('codex', 'Codex', [], null),
      t('grok', 'Grok', [], null),
      t('gemini', 'Gemini', [], null, false),
      { tool: 'cursor', display: 'Cursor Agent', supported: false, subscription: false, apiKeys: false, activeId: null, liveAccountId: null, loginMethod: null, accounts: [], pins: [] },
    ],
    logins: [],
    airgap: false,
    switchNote: '',
  }
}

beforeEach(() => {
  resetLlmAccountsForTests()
  pins = []
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/list') return doc()
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string, body: Record<string, unknown>) => {
    if (route === 'llm/accounts/pin') {
      const pin = { scopeKind: body.scope, scopeId: body.scopeId, tool: body.tool, accountId: body.id, label: 'research' }
      pins = [...pins.filter((p) => !(p.scopeKind === body.scope && p.scopeId === body.scopeId && p.tool === body.tool)), pin]
      return { pin, note: '' }
    }
    if (route === 'llm/accounts/unpin') {
      pins = pins.filter((p) => !(p.scopeKind === body.scope && p.scopeId === body.scopeId && p.tool === body.tool))
      return { unpinned: true }
    }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

function pick(trigger: string, option: string): void {
  fireEvent.click(screen.getByRole('button', { name: trigger }))
  const menu = screen.getByTestId('setting-dropdown-menu')
  fireEvent.click(within(menu).getByRole('button', { name: option }))
}

describe('Workspace LLM logins', () => {
  it('one row per supported tool; pinning posts the workspace id; Pool unpins', async () => {
    render(<WorkspaceLlmLogins scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    for (const t of ['claude', 'codex', 'grok', 'gemini']) {
      expect(screen.getByTestId(`llm-pin-workspace-${t}`)).toBeTruthy()
    }
    expect(screen.queryByTestId('llm-pin-workspace-cursor')).toBeNull()
    pick('Claude login', 'spare')
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/pin', {
        scope: 'workspace',
        scopeId: 'proj-1',
        tool: 'claude',
        id: 'acc_spare',
      }),
    )
    expect(await screen.findByText(CLAUDE_PIN_NOTE)).toBeTruthy()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Claude login' }).textContent).toContain('spare'))
    pick('Claude login', POOL_LABEL)
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/unpin', { scope: 'workspace', scopeId: 'proj-1', tool: 'claude' }),
    )
  })

  it("the pool's live login is disabled with the reason; API keys say billed per token", async () => {
    render(<WorkspaceLlmLogins scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    fireEvent.click(screen.getByRole('button', { name: 'Claude login' }))
    const menu = screen.getByTestId('setting-dropdown-menu')
    const live = within(menu).getByRole('button', { name: /^main — / }) as HTMLButtonElement
    expect(live.disabled).toBe(true)
    expect(live.textContent).toContain("pool's active login")
    expect(within(menu).getByRole('button', { name: 'metered · billed per token' })).toBeTruthy()
  })

  it('a daemon refusal shows its hint', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'pinned_active', hint: "main is the pool's active Claude login." } }))
    })
    render(<WorkspaceLlmLogins scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    pick('Claude login', 'spare')
    expect((await screen.findByRole('alert')).textContent).toBe("main is the pool's active Claude login.")
  })

  it('the settings search has the LLM logins entry', () => {
    const e = PROJECTS_MANIFEST.find((x) => x.id === 'projects.llm-logins')
    expect(e).toBeTruthy()
    expect(e!.keywords).toEqual(expect.arrayContaining(['login', 'account', 'pin', 'subscription', 'api key']))
  })
})

describe('Chat header login picker', () => {
  it('pins the pinned chat (session key = workspace id) and offers unpin', async () => {
    render(<SessionLoginPicker scope={primaryScope()} projectId="proj-1" provider="claude" />)
    const chip = await screen.findByRole('button', { name: 'Claude login for this chat' })
    expect(chip.textContent).toBe('Login: Pool')
    fireEvent.click(chip)
    expect(screen.getByText(SESSION_PIN_NOTE)).toBeTruthy()
    pick('Claude login', 'spare')
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/pin', {
        scope: 'session',
        scopeId: 'proj-1',
        tool: 'claude',
        id: 'acc_spare',
      }),
    )
    await waitFor(() => expect(screen.getByRole('button', { name: 'Claude login for this chat' }).textContent).toBe('Login: spare'))
    fireEvent.click(screen.getByRole('button', { name: 'Unpin and use the pool' }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/unpin', { scope: 'session', scopeId: 'proj-1', tool: 'claude' }),
    )
  })

  it('is hidden for a tool with no logins or a harness without a wallet', async () => {
    const { container } = render(
      <>
        <SessionLoginPicker scope={primaryScope()} projectId="proj-1" provider="codex" />
        <SessionLoginPicker scope={primaryScope()} projectId="proj-1" provider="pi" />
      </>,
    )
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('llm/accounts/list'))
    expect(container.querySelector('[data-testid="chat-login-picker"]')).toBeNull()
    expect(toolForProvider('pi')).toBeNull()
    expect(toolForProvider('Claude')).toBe('claude')
  })
})
