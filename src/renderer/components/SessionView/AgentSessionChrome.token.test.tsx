// @vitest-environment jsdom
//
// The Token control in an agent chat tab's header (not the pinned chat):
// it pins the tab's session key and its conversation, shows the workspace
// default from the tab's own workspace, and is absent for shell tabs and
// view-only rooms. Daemon calls are mocked; an unexpected route throws.

import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'

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

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1, token: 't', secure: false })),
  daemonWsBase: () => 'ws://127.0.0.1:1',
}))

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: Object.assign(
    (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
    { getState: () => ({ activeHost: 'local' as const, hosts: [] }) },
  ),
  activeHostKey: () => 'local',
  onActiveHostChange: () => () => {},
}))

vi.stubGlobal(
  'WebSocket',
  class {
    onmessage: unknown = null
    close(): void {}
  },
)

import { AgentSessionChrome } from './AgentSessionChrome'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { resetLlmAccountsForTests } from '@/stores/llm-accounts'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const PROJECT = { id: 'proj-sales', name: 'sales', path: '/ws/sales', workspaces: [] } as unknown as ProjectWithWorkspaces

function account(over: Record<string, unknown>): Record<string, unknown> {
  return {
    id: 'acc_x', tool: 'claude', label: 'x', kind: 'subscription', active: false, state: 'signed_in', detail: null,
    plan: null, expiresAt: null, refreshedAt: null, lastUsedAt: null, createdAt: 1, createdBy: null, usage: null,
    usageCheckedAt: null, pinnedTo: [], inUse: false, ...over,
  }
}

let pins: Array<Record<string, unknown>> = []

function doc(): Record<string, unknown> {
  return {
    tools: [
      {
        tool: 'claude', display: 'Claude', supported: true, subscription: true, apiKeys: true, activeId: 'acc_live',
        liveAccountId: 'acc_live', loginMethod: 'temp_home', pins: pins.filter((p) => p.tool === 'claude'),
        accounts: [
          account({ id: 'acc_live', label: 'main', active: true, plan: 'max' }),
          account({ id: 'acc_spare', label: 'spare', plan: 'pro' }),
          account({ id: 'acc_key', label: 'metered', kind: 'api_key', billedPerToken: true }),
        ],
      },
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
      const pin = { scopeKind: 'session', scopeId: body.scopeId, tool: body.tool, accountId: body.id, label: 'x', createdAt: 7 }
      pins = [pin]
      return { pin }
    }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => cleanup())

function renderTab(opts: { command?: string; readOnly?: boolean } = {}) {
  const room = testRoom({ tabs: {}, projects: [PROJECT], readOnly: opts.readOnly })
  return renderInRoom(
    room,
    <AgentSessionChrome
      title="sales/reviewer"
      addr="sales/reviewer"
      conversationId="conv-r"
      agentName="tab-xyz"
      cwd="/ws/sales"
      command={opts.command ?? 'claude'}
    >
      <div data-testid="terminal-pane" />
    </AgentSessionChrome>,
  )
}

describe('agent chat tab header Token picker', () => {
  it('sits in the tab header and pins this tab and its conversation', async () => {
    pins = [{ scopeKind: 'workspace', scopeId: 'proj-sales', tool: 'claude', accountId: 'acc_key', label: 'sales', createdAt: 1 }]
    renderTab()
    const header = screen.getByTestId('sidecar-session-header')
    const chip = await within(header).findByRole('button', {
      name: 'Claude token for this chat: Workspace default — API token · metered · billed per token',
    })
    expect(chip.textContent).toBe('Token')
    fireEvent.click(chip)
    fireEvent.click(within(screen.getByTestId('chat-token-list')).getByRole('button', { name: /^Claude · Pro · spare/ }))
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/pin', {
        scope: 'session',
        scopeId: 'tab-xyz',
        tool: 'claude',
        id: 'acc_spare',
        conversationId: 'conv-r',
      }),
    )
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
  })

  it('is not shown for a shell tab or in a view-only room', async () => {
    renderTab({ command: 'zsh' })
    renderTab({ readOnly: true })
    // Neither header mounts a picker, so neither loads the token list.
    await new Promise((r) => setTimeout(r, 20))
    expect(screen.getAllByTestId('sidecar-session-header')).toHaveLength(2)
    expect(screen.queryByTestId('chat-login-picker')).toBeNull()
    expect(h.daemonCliGet).not.toHaveBeenCalled()
  })
})
