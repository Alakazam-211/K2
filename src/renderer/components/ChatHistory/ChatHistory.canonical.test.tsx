// @vitest-environment jsdom
// One conversation, one writer: clicking the agent's own conversation in
// Chat history used to open an extra `claude --resume <canonical id>` tab
// (a second harness on the pinned Chat's transcript). It must focus the
// pinned Chat tab and add no tab. A different conversation still opens.
// Fail loud — no skip.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, fireEvent, waitFor, cleanup } from '@testing-library/react'

const PROJECT = '/work/canonical-history'
const CANONICAL_ID = '0b9c7d2e-1111-4222-8333-444455556666'
const OTHER_ID = '7a1f3c90-aaaa-4bbb-8ccc-ddddeeeeffff'

const h = vi.hoisted(() => ({
  sessions: [] as Array<Record<string, unknown>>,
  calls: [] as string[],
  resumeChatArgs: null as unknown,
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(vi.fn(async (route: string) => {
      h.calls.push(route)
      if (route === 'chat/list') return h.sessions
      if (route === 'chat/custom-names') return {}
      if (route === 'chat/pinned') return []
      if (route === 'workspace/resume-chat-args') return h.resumeChatArgs
      return []
    })),
    daemonCliPost: primaryOnly(vi.fn(async () => ({}))),
    RecoveringError: class RecoveringError extends Error {},
    isHostSwitchedError: () => false,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}))

import ChatHistory from './ChatHistory'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { usePresetsStore } from '@/stores/presets'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'

function session(sessionId: string, title: string): Record<string, unknown> {
  return {
    sessionId,
    project: PROJECT,
    title,
    timestamp: Date.now(),
    provider: 'claude',
    messageCount: 4,
    originBranch: null,
    archived: false,
    customName: null,
  }
}

function chatTab(stampedSessionId?: string): Tab {
  const pgId = 'pg-chat'
  return {
    id: 'tab-chat',
    title: 'Chat',
    mosaicTree: pgId,
    isSystemAgent: true,
    paneGroups: new Map([[pgId, {
      id: pgId,
      activeItemIndex: 0,
      items: [{
        id: 'item-chat',
        type: 'agent',
        data: {
          agentName: 'canonical-history',
          projectPath: PROJECT,
          section: 'chat',
          ...(stampedSessionId ? { sessionId: stampedSessionId } : {}),
        },
      }],
    }]]),
  } as Tab
}

function shellTab(): Tab {
  const pgId = 'pg-shell'
  return {
    id: 'tab-shell',
    title: 'zsh',
    mosaicTree: pgId,
    paneGroups: new Map([[pgId, {
      id: pgId,
      activeItemIndex: 0,
      items: [{ id: 'item-shell', type: 'terminal', data: { cwd: PROJECT } }],
    }]]),
  } as Tab
}

function row(title: string): HTMLElement {
  const el = screen.getByText(title).closest('button')
  if (!el) throw new Error(`no row button for ${title}`)
  return el
}

function terminalArgs(): string[][] {
  const out: string[][] = []
  for (const tab of useTabsStore.getState().tabs) {
    for (const [, pg] of tab.paneGroups) {
      for (const item of pg.items) {
        if (item.type !== 'terminal') continue
        out.push(((item.data as { args?: string[] }).args) ?? [])
      }
    }
  }
  return out
}

beforeEach(() => {
  h.sessions.length = 0
  h.calls.length = 0
  h.resumeChatArgs = null
  useTabsStore.setState({
    tabs: [chatTab(), shellTab()],
    activeTabId: 'tab-shell',
    extraGroups: [],
    splitCount: 1,
    activeGroupIndex: 0,
  })
  usePresetsStore.setState({
    presets: [{
      id: 'preset-claude',
      label: 'Claude',
      command: 'claude --dangerously-skip-permissions --resume',
      icon: null,
      enabled: 1,
      sortOrder: 0,
      isBuiltIn: 1,
      createdAt: 0,
    }],
  })
  useHeartbeatSessionsStore.setState({
    loadedFor: PROJECT,
    active: [],
    archived: [],
    loading: false,
    lastError: null,
  })
})

afterEach(() => {
  cleanup()
})

describe("Chat history: the agent's own conversation", () => {
  it('focuses the pinned Chat tab (daemon id) and adds no tab', async () => {
    h.resumeChatArgs = { resumeSession: CANONICAL_ID, resumedExisting: true, command: 'claude', args: [] }
    h.sessions.push(session(CANONICAL_ID, 'The agent chat'), session(OTHER_ID, 'Side quest'))
    renderInPrimaryRoom(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('The agent chat')
    await waitFor(() => expect(h.calls).toContain('workspace/resume-chat-args'))
    // Let the resume-chat-args answer land in state before clicking.
    await new Promise((r) => setTimeout(r, 0))
    const tabsBefore = useTabsStore.getState().tabs.length

    fireEvent.click(row('The agent chat'))

    await waitFor(() => expect(useTabsStore.getState().activeTabId).toBe('tab-chat'))
    expect(useTabsStore.getState().tabs.length).toBe(tabsBefore)
    expect(terminalArgs().some((args) => args.includes(CANONICAL_ID))).toBe(false)
  })

  it("focuses the pinned Chat tab from the Chat pane's stamped id", async () => {
    // The daemon read has not resolved an id; the Chat pane's stamp covers it.
    h.resumeChatArgs = { resumeSession: '', resumedExisting: false, command: 'claude', args: [] }
    useTabsStore.setState({ tabs: [chatTab(CANONICAL_ID), shellTab()], activeTabId: 'tab-shell' })
    h.sessions.push(session(CANONICAL_ID, 'The agent chat'))
    renderInPrimaryRoom(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('The agent chat')
    const tabsBefore = useTabsStore.getState().tabs.length

    fireEvent.click(row('The agent chat'))

    expect(useTabsStore.getState().activeTabId).toBe('tab-chat')
    expect(useTabsStore.getState().tabs.length).toBe(tabsBefore)
    expect(terminalArgs().some((args) => args.includes(CANONICAL_ID))).toBe(false)
  })

  it('a different conversation still opens its own resume tab', async () => {
    h.resumeChatArgs = { resumeSession: CANONICAL_ID, resumedExisting: true, command: 'claude', args: [] }
    h.sessions.push(session(CANONICAL_ID, 'The agent chat'), session(OTHER_ID, 'Side quest'))
    renderInPrimaryRoom(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Side quest')
    await waitFor(() => expect(h.calls).toContain('workspace/resume-chat-args'))
    await new Promise((r) => setTimeout(r, 0))
    const tabsBefore = useTabsStore.getState().tabs.length

    fireEvent.click(row('Side quest'))

    await waitFor(() => expect(useTabsStore.getState().tabs.length).toBe(tabsBefore + 1))
    const resumed = terminalArgs().find((args) => args.includes(OTHER_ID))
    if (!resumed) throw new Error(`no resume tab for ${OTHER_ID}: ${JSON.stringify(terminalArgs())}`)
    expect(resumed).toContain('--resume')
    expect(useTabsStore.getState().activeTabId).not.toBe('tab-chat')
  })
})
