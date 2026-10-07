// @vitest-environment jsdom
//
// Token pickers: the workspace "LLM tokens" group and the chat header
// Token control. The closed control says only "Token"; the open menu shows
// "Server default — <name>", then Subscriptions, then API tokens, with the
// one in use marked. Daemon calls are mocked; routes and bodies are
// asserted exactly. Fail loud: an unexpected route throws.

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

import {
  WorkspaceLlmTokens,
  SessionTokenPicker,
  SESSION_PIN_NOTE,
  TOKEN_LABEL,
  IN_USE,
  toolForProvider,
} from './LlmLoginPicker'
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

const LIVE = account({ id: 'acc_live', label: 'main', active: true, plan: 'max' })
const SPARE = account({ id: 'acc_spare', label: 'spare', plan: 'pro' })
const KEY = account({ id: 'acc_key', label: 'metered', kind: 'api_key', billedPerToken: true })

const DEFAULT_NAME = 'Server default — Claude · Max · main'
const SPARE_NAME = 'Claude · Pro · spare'
const KEY_NAME = 'API token · metered · billed per token'

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

/** The workspace Claude trigger: its accessible name carries the token in use. */
function claudeTrigger(): HTMLElement {
  return screen.getByRole('button', { name: /^Claude token: / })
}

function openMenu(): HTMLElement {
  fireEvent.click(claudeTrigger())
  return screen.getByTestId('setting-dropdown-menu')
}

function pick(option: string): void {
  const menu = openMenu()
  fireEvent.click(within(menu).getByRole('button', { name: new RegExp(`^${option.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}`) }))
}

describe('Workspace LLM tokens', () => {
  it('one Token picker per supported tool; the closed control says only "Token"', async () => {
    render(<WorkspaceLlmTokens scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    for (const t of ['claude', 'codex', 'grok', 'gemini']) {
      expect(screen.getByTestId(`llm-pin-workspace-${t}`)).toBeTruthy()
    }
    expect(screen.queryByTestId('llm-pin-workspace-cursor')).toBeNull()
    const trigger = claudeTrigger()
    expect(trigger.textContent).toBe(TOKEN_LABEL)
    expect(trigger.getAttribute('aria-label')).toBe(`Claude token: ${DEFAULT_NAME}`)
  })

  it('the open menu: Server default (in use) first, then Subscriptions, then API tokens', async () => {
    render(<WorkspaceLlmTokens scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    const menu = openMenu()
    const text = menu.textContent ?? ''
    expect(Array.from(menu.children).map((c) => c.textContent)).toEqual([
      `${DEFAULT_NAME}${IN_USE}`,
      'Subscriptions',
      'Claude · Max · main — This is the server default; pick Server default',
      SPARE_NAME,
      'API tokens',
      KEY_NAME,
    ])
    const def = within(menu).getByRole('button', { name: new RegExp(`^${DEFAULT_NAME}`) })
    expect(def.textContent).toBe(`${DEFAULT_NAME}${IN_USE}`)
    expect(within(menu).getByRole('button', { name: new RegExp(`^${SPARE_NAME}`) }).textContent).not.toContain(IN_USE)
    expect(text).not.toMatch(/Pool|login|pin/i)
  })

  it('picking a subscription posts the workspace id; it becomes the one in use; Server default unpins', async () => {
    render(<WorkspaceLlmTokens scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    pick(SPARE_NAME)
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/pin', {
        scope: 'workspace',
        scopeId: 'proj-1',
        tool: 'claude',
        id: 'acc_spare',
      }),
    )
    expect(await screen.findByText(CLAUDE_PIN_NOTE)).toBeTruthy()
    expect(CLAUDE_PIN_NOTE).toBe("This chat's Claude history will stay with this subscription.")
    await waitFor(() => expect(claudeTrigger().getAttribute('aria-label')).toBe(`Claude token: ${SPARE_NAME}`))
    expect(claudeTrigger().textContent).toBe(TOKEN_LABEL)
    const menu = openMenu()
    expect(within(menu).getByRole('button', { name: new RegExp(`^${SPARE_NAME}`) }).textContent).toContain(IN_USE)
    expect(within(menu).getByRole('button', { name: new RegExp(`^${DEFAULT_NAME}`) }).textContent).not.toContain(IN_USE)
    fireEvent.click(within(menu).getByRole('button', { name: new RegExp(`^${DEFAULT_NAME}`) }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/unpin', { scope: 'workspace', scopeId: 'proj-1', tool: 'claude' }),
    )
  })

  it('the server default subscription is disabled with the reason; API tokens say billed per token', async () => {
    render(<WorkspaceLlmTokens scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    const menu = openMenu()
    const live = within(menu).getByRole('button', { name: /^Claude · Max · main — / }) as HTMLButtonElement
    expect(live.disabled).toBe(true)
    expect(live.textContent).toContain('This is the server default; pick Server default')
    expect(within(menu).getByRole('button', { name: KEY_NAME })).toBeTruthy()
  })

  it('a daemon refusal shows its hint', async () => {
    h.daemonCliPost.mockImplementation(async () => {
      throw new Error(JSON.stringify({ error: { code: 'pinned_active', hint: 'main is the server default Claude token.' } }))
    })
    render(<WorkspaceLlmTokens scope={primaryScope()} projectId="proj-1" />)
    await screen.findByTestId('llm-pin-workspace-claude')
    pick(SPARE_NAME)
    expect((await screen.findByRole('alert')).textContent).toBe('main is the server default Claude token.')
  })

  it('the settings search has the LLM tokens entry', () => {
    const e = PROJECTS_MANIFEST.find((x) => x.id === 'projects.llm-logins')
    expect(e).toBeTruthy()
    expect(e!.label).toBe('LLM tokens')
    expect(e!.keywords).toEqual(expect.arrayContaining(['token', 'subscription', 'api token', 'server default']))
  })
})

describe('Chat header Token picker', () => {
  it('closed it says only "Token"; open it marks the one in use and pins the pinned chat', async () => {
    render(<SessionTokenPicker scope={primaryScope()} projectId="proj-1" provider="claude" />)
    const chip = await screen.findByRole('button', { name: `Claude token for this chat: ${DEFAULT_NAME}` })
    expect(chip.textContent).toBe(TOKEN_LABEL)
    fireEvent.click(chip)
    expect(screen.getByText('Use this token for this chat')).toBeTruthy()
    expect(screen.getByText(SESSION_PIN_NOTE)).toBeTruthy()
    const list = screen.getByTestId('chat-token-list')
    expect(within(list).getByText('Subscriptions')).toBeTruthy()
    expect(within(list).getByText('API tokens')).toBeTruthy()
    expect(within(list).getByRole('button', { name: new RegExp(`^${DEFAULT_NAME}`) }).textContent).toContain(IN_USE)
    fireEvent.click(within(list).getByRole('button', { name: new RegExp(`^${SPARE_NAME}`) }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/pin', {
        scope: 'session',
        scopeId: 'proj-1',
        tool: 'claude',
        id: 'acc_spare',
      }),
    )
    const after = await screen.findByRole('button', { name: `Claude token for this chat: ${SPARE_NAME}` })
    expect(after.textContent).toBe(TOKEN_LABEL)
    expect(
      within(screen.getByTestId('chat-token-list')).getByRole('button', { name: new RegExp(`^${SPARE_NAME}`) }).textContent,
    ).toContain(IN_USE)
    fireEvent.click(within(screen.getByTestId('chat-token-list')).getByRole('button', { name: new RegExp(`^${DEFAULT_NAME}`) }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/unpin', { scope: 'session', scopeId: 'proj-1', tool: 'claude' }),
    )
  })

  it("a chat whose workspace has its own token defaults to that token", async () => {
    pins = [{ scopeKind: 'workspace', scopeId: 'proj-1', tool: 'claude', accountId: 'acc_key', label: 'research' }]
    render(<SessionTokenPicker scope={primaryScope()} projectId="proj-1" provider="claude" />)
    const chip = await screen.findByRole('button', { name: `Claude token for this chat: Workspace default — ${KEY_NAME}` })
    expect(chip.textContent).toBe(TOKEN_LABEL)
  })

  it('is hidden for a tool with no tokens or a harness without tokens', async () => {
    const { container } = render(
      <>
        <SessionTokenPicker scope={primaryScope()} projectId="proj-1" provider="codex" />
        <SessionTokenPicker scope={primaryScope()} projectId="proj-1" provider="pi" />
      </>,
    )
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('llm/accounts/list'))
    expect(container.querySelector('[data-testid="chat-login-picker"]')).toBeNull()
    expect(toolForProvider('pi')).toBeNull()
    expect(toolForProvider('Claude')).toBe('claude')
  })
})
