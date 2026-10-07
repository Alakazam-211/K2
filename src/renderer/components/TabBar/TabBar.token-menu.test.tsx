// @vitest-environment jsdom
//
// Right-click an agent chat tab → "Token ▸": the same entries as the chat
// header picker (default, Subscriptions, API tokens; the one in use checked
// and marked "In use"; the note). Picking one pins the tab's session key
// and its conversation. Shell tabs have no Token row. Daemon calls are
// mocked; an unexpected route throws.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn((_scope: unknown, ...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: vi.fn((_scope: unknown, ...a: unknown[]) => h.daemonCliPost(...a)),
  }
})

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Element.prototype.scrollIntoView = () => {}
}

import ContextMenu from '@/components/ContextMenu/ContextMenu'
import { TabBar } from '@/components/TabBar/TabBar'
import { useContextMenuStore } from '@/stores/context-menu'
import { resetLlmAccountsForTests } from '@/stores/llm-accounts'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useTabsStore, type TerminalItemData } from '@/stores/tabs'
import { useToastStore } from '@/stores/toast'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { SESSION_PIN_NOTE } from '@/components/Settings/shared/LlmLoginPicker'

const CWD = '/ws/sales'
const PROJECT = { id: 'proj-sales', name: 'sales', path: CWD, workspaces: [] } as unknown as ProjectWithWorkspaces

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

function resetTabs(): void {
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    activeWorkspaceKey: null,
  })
}

function terminalOf(tabId: string): TerminalItemData {
  const tab = useTabsStore.getState().tabs.find((t) => t.id === tabId)!
  const item = Array.from(tab.paneGroups.values())[0].items[0]
  return item.data as TerminalItemData
}

async function rightClick(tabId: string): Promise<void> {
  const el = document.querySelector(`[data-tab-id="${tabId}"]`)
  expect(el).toBeTruthy()
  await act(async () => {
    fireEvent.contextMenu(el!)
  })
  await waitFor(() => expect(useContextMenuStore.getState().isOpen).toBe(true))
}

function row(id: string): HTMLButtonElement {
  const el = document.querySelector(`[data-context-menu-item="${id}"]`)
  if (!el) throw new Error(`no menu row ${id}`)
  return el as HTMLButtonElement
}

beforeEach(() => {
  localStorage.clear()
  resetTabs()
  resetLlmAccountsForTests()
  useContextMenuStore.getState().close()
  useProjectsStore.setState({ projects: [PROJECT] })
  pins = [{ scopeKind: 'session', scopeId: 'conversation:conv-1', tool: 'claude', accountId: 'acc_key', label: 'conv-1', createdAt: 3 }]
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockImplementation(async (route: string) => {
    if (route === 'llm/accounts/list') return doc()
    if (route === 'sessions/list-for-workspace') return []
    throw new Error(`unexpected GET ${route}`)
  })
  h.daemonCliPost.mockImplementation(async (route: string, body: Record<string, unknown>) => {
    if (route === 'llm/accounts/pin') return { pin: { scopeKind: 'session', scopeId: body.scopeId, tool: body.tool, accountId: body.id, label: 'x' } }
    throw new Error(`unexpected POST ${route}`)
  })
})

afterEach(() => {
  cleanup()
  useContextMenuStore.getState().close()
  resetTabs()
  useProjectsStore.setState({ projects: [] })
  useToastStore.setState({ toasts: [] })
})

describe('tab right-click Token submenu', () => {
  it('an agent chat tab offers Token ▸ with the header entries and pins this tab and its conversation', async () => {
    useTabsStore.getState().addTabToGroup(0, CWD, { title: 'Claude', command: 'claude', args: ['--session-id', 'conv-1'] })
    const tab = useTabsStore.getState().tabs[0]
    const sessionKey = `tab-${terminalOf(tab.id).terminalId}`
    renderInPrimaryRoom(
      <>
        <TabBar cwd={CWD} />
        <ContextMenu />
      </>,
    )
    await rightClick(tab.id)
    const token = row('token')
    expect(token.textContent).toContain('Token')
    expect(token.textContent).toContain('▸')
    expect(document.querySelector('[data-context-submenu]')).toBeNull()
    await act(async () => {
      fireEvent.mouseEnter(token)
    })
    const sub = document.querySelector('[data-context-submenu]') as HTMLElement
    expect(sub).toBeTruthy()
    const text = sub.textContent ?? ''
    expect(text).toContain('Server default — Claude · Max · main')
    expect(text).toContain('Subscriptions')
    expect(text).toContain('API tokens')
    expect(text).toContain(SESSION_PIN_NOTE)
    // The conversation's pick (made in another tab) is the one in use.
    const inUse = row('token:acc_key')
    expect(inUse.getAttribute('aria-pressed')).toBe('true')
    expect(inUse.textContent).toContain('In use')
    expect(row('token:__pool__').getAttribute('aria-pressed')).toBe('false')
    expect(row('token:acc_live').disabled).toBe(true)
    await act(async () => {
      fireEvent.click(row('token:acc_spare'))
    })
    await waitFor(() =>
      expect(h.daemonCliPost).toHaveBeenCalledWith('llm/accounts/pin', {
        scope: 'session',
        scopeId: sessionKey,
        tool: 'claude',
        id: 'acc_spare',
        conversationId: 'conv-1',
      }),
    )
    expect(useContextMenuStore.getState().isOpen).toBe(false)
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.message)).toEqual([
        "This chat runs on this token from its next start. K2 copies this chat's Claude history to this subscription when it restarts.",
      ]),
    )
  })

  it('a shell tab has no Token row', async () => {
    useTabsStore.getState().addTabToGroup(0, CWD, { title: 'zsh' })
    const tab = useTabsStore.getState().tabs[0]
    renderInPrimaryRoom(
      <>
        <TabBar cwd={CWD} />
        <ContextMenu />
      </>,
    )
    await rightClick(tab.id)
    expect(document.querySelector('[data-context-menu-item="token"]')).toBeNull()
    expect(screen.getByRole('button', { name: /Close Tab/ })).toBeTruthy()
    expect(h.daemonCliGet).not.toHaveBeenCalledWith('llm/accounts/list')
  })
})
