// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup, act } from '@testing-library/react'

const PROJECT = '/work/continue-new-chat'

const h = vi.hoisted(() => {
  const posts: Array<{ route: string; body: unknown }> = []
  const sessions: Array<Record<string, unknown>> = []
  const sandbox: Array<Record<string, unknown>> = []
  const api: Array<Record<string, unknown>> = []
  let sendResult: unknown = {
    success: true,
    target_session_id: 'pty',
    attempts: 1,
    reason: null,
    hint: null,
  }
  let sendThrows: Error | null = null
  let seedThrows: Error | null = null
  const seedText = 'SEED TEXT from the daemon'
  return { posts, sessions, sandbox, api, sendResult, sendThrows, seedThrows, seedText }
})

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (route: string) => {
    if (route === 'chat/list') return h.sessions
    if (route === 'sandbox/list') return h.sandbox
    if (route === 'host-sessions/list') return h.api
    if (route === 'chat/custom-names') return {}
    if (route === 'chat/pinned') return []
    return []
  }),
  daemonCliPost: vi.fn(async (route: string, body: unknown) => {
    h.posts.push({ route, body })
    if (route === 'chat/continue-seed') {
      if (h.seedThrows) throw h.seedThrows
      return { text: h.seedText }
    }
    if (route === 'terminal/send-message') {
      if (h.sendThrows) throw h.sendThrows
      return h.sendResult
    }
    return {}
  }),
  RecoveringError: class RecoveringError extends Error {},
  isHostSwitchedError: () => false,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}))

import ChatHistory from './ChatHistory'
import { useTabsStore } from '@/stores/tabs'
import { usePresetsStore } from '@/stores/presets'
import { usePinnedSizeStore } from '@/stores/pinned-size'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'

function session(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    sessionId: 'sess-src-1',
    project: PROJECT,
    title: 'Finish the editor refactor',
    timestamp: Date.now(),
    provider: 'claude',
    messageCount: 4,
    originBranch: null,
    archived: false,
    customName: null,
    ...over,
  }
}

function menuRoot(): HTMLElement | null {
  return document.querySelector('body > div[style*="z-index: 9999"]')
}

function menuLabels(): string[] {
  const root = menuRoot()
  if (!root) return []
  return Array.from(root.querySelectorAll('button')).map((btn) => btn.textContent ?? '')
}

function closeMenus(): void {
  menuRoot()?.remove()
}

function row(title: string): HTMLElement {
  const el = screen.getByText(title).closest('button')
  if (!el) throw new Error(`no row button for ${title}`)
  return el
}

async function openMenu(title: string): Promise<void> {
  fireEvent.contextMenu(row(title))
  await waitFor(() => {
    expect(menuLabels().some((label) => label.length > 0)).toBe(true)
  })
}

beforeEach(() => {
  h.posts.length = 0
  h.sessions.length = 0
  h.sandbox.length = 0
  h.api.length = 0
  h.sendResult = {
    success: true,
    target_session_id: 'pty',
    attempts: 1,
    reason: null,
    hint: null,
  }
  h.sendThrows = null
  h.seedThrows = null
  h.seedText = 'SEED TEXT from the daemon'
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    extraGroups: [],
    splitCount: 1,
    activeGroupIndex: 0,
  })
  usePinnedSizeStore.setState({ pins: {}, sessions: {}, dims: {} })
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
  closeMenus()
})

describe('Continue in a new chat', () => {
  it('shows the menu item on a live provider row and not on archived, API, or sandbox rows', async () => {
    h.sessions.push(
      session(),
      session({
        sessionId: 'sess-arch',
        title: 'Old archived chat',
        archived: true,
        provider: 'claude',
      }),
    )
    h.api.push({
      sessionId: 'api-1',
      agentName: 'Api Agent',
      live: true,
      lastSeenAt: Date.now(),
    })
    h.sandbox.push({
      sessionId: 'sand-1',
      title: 'Sandbox chat',
      timestamp: Date.now(),
      messageCount: 1,
    })
    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    await screen.findByText('Old archived chat')
    expect(screen.queryByText('Api Agent')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /API/ }))
    await screen.findByText('Api Agent')
    await screen.findByText('Sandbox chat')

    await openMenu('Finish the editor refactor')
    const labels = menuLabels()
    const resumeAt = labels.indexOf('Copy resume command')
    const continueAt = labels.indexOf('Continue in a new chat…')
    const archiveAt = labels.indexOf('Archive')
    expect(continueAt).toBeGreaterThan(-1)
    expect(resumeAt).toBeGreaterThan(-1)
    expect(continueAt).toBe(resumeAt + 1)
    expect(archiveAt).toBe(continueAt + 1)
    closeMenus()

    await openMenu('Old archived chat')
    const archived = menuLabels()
    expect(archived).toContain('Copy Path')
    expect(archived).toContain('Copy resume command')
    expect(archived).toContain('Restore')
    expect(archived).not.toContain('Continue in a new chat…')
    closeMenus()

    const before = document.body.querySelectorAll('button').length
    fireEvent.contextMenu(row('Api Agent'))
    fireEvent.contextMenu(row('Sandbox chat'))
    expect(menuLabels()).not.toContain('Continue in a new chat…')
    expect(document.body.querySelectorAll('button').length).toBe(before)
  })

  it('opening the dialog does not spawn, and Esc, Cancel, and scrim do not either', async () => {
    h.sessions.push(session())
    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    const tabsBefore = useTabsStore.getState().tabs.length

    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    const dialog = await screen.findByTestId('continue-new-chat')
    expect(dialog.textContent).toContain('Finish the editor refactor')
    expect(dialog.textContent).toContain('From Claude')
    const harness = screen.getByLabelText('Harness')
    expect(harness.tagName).not.toBe('SELECT')
    expect(harness.textContent).toContain('Claude')
    const recent = screen.getByRole('radio', { name: /Recent turns/ }) as HTMLInputElement
    expect(recent.checked).toBe(true)
    expect(h.posts).toEqual([])
    expect(useTabsStore.getState().tabs.length).toBe(tabsBefore)

    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => {
      expect(screen.queryByTestId('continue-new-chat')).toBeNull()
    })
    expect(h.posts).toEqual([])
    expect(useTabsStore.getState().tabs.length).toBe(tabsBefore)

    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => {
      expect(screen.queryByTestId('continue-new-chat')).toBeNull()
    })
    expect(h.posts).toEqual([])

    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    const frame = await screen.findByTestId('continue-new-chat')
    const scrim = frame.previousElementSibling
    expect(scrim).toBeTruthy()
    fireEvent.mouseDown(scrim as Element)
    await waitFor(() => {
      expect(screen.queryByTestId('continue-new-chat')).toBeNull()
    })
    expect(h.posts).toEqual([])
    expect(useTabsStore.getState().tabs.length).toBe(tabsBefore)
  })

  it('Start spawns preset args with no resume, then send-message only after a PTY id', async () => {
    h.sessions.push(session())
    const sourcePane = useTabsStore.getState().addTab(PROJECT, {
      title: 'already open',
      command: 'claude',
      args: ['--resume', 'sess-src-1'],
      conversationId: 'sess-src-1',
    })
    const sourceTab = useTabsStore.getState().tabs.find((tab) =>
      Array.from(tab.paneGroups.values()).some((pg) =>
        pg.items.some((item) => item.type === 'terminal' && (item.data as { conversationId?: string }).conversationId === 'sess-src-1'),
      ),
    )
    expect(sourceTab).toBeTruthy()
    const sourceTabId = sourceTab!.id
    expect(sourcePane).toBeTruthy()

    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))

    await waitFor(() => {
      expect(h.posts.some((post) => post.route === 'chat/continue-seed')).toBe(true)
    })
    const seedPost = h.posts.find((post) => post.route === 'chat/continue-seed')
    expect(seedPost?.body).toEqual({
      provider: 'claude',
      sessionId: 'sess-src-1',
      projectPath: PROJECT,
      mode: 'recent',
      targetProvider: 'claude',
    })
    expect(h.posts.some((post) => post.route === 'terminal/send-message')).toBe(false)

    await waitFor(() => {
      const created = useTabsStore.getState().tabs.find((tab) => tab.title === 'Finish the editor refactor (from Claude)')
      expect(created).toBeTruthy()
    })
    expect(h.posts.some((post) => post.route === 'terminal/send-message')).toBe(false)
    const created = useTabsStore.getState().tabs.find((tab) => tab.title === 'Finish the editor refactor (from Claude)')!
    const pane = Array.from(created.paneGroups.values())[0]
    const item = pane.items[0]
    expect(item.type).toBe('terminal')
    const data = item.data as { command?: string; args?: string[]; conversationId?: string; sessionId?: string }
    expect(data.command).toBe('claude')
    expect(data.args).toEqual(['--dangerously-skip-permissions'])
    expect(data.args ?? []).not.toContain('--resume')
    expect(data.args ?? []).not.toContain('resume')
    expect(data.args ?? []).not.toContain('--fork-session')
    expect(data.args ?? []).not.toContain('sess-src-1')
    expect(data.conversationId).toBeUndefined()
    expect(useTabsStore.getState().activeTabId).not.toBe(sourceTabId)

    const paneGroupId = pane.id
    act(() => {
      usePinnedSizeStore.getState().registerSession(paneGroupId, 'pty-live-1')
    })
    await waitFor(() => {
      expect(h.posts.some((post) => post.route === 'terminal/send-message')).toBe(true)
    })
    const send = h.posts.find((post) => post.route === 'terminal/send-message')
    expect(send?.body).toEqual({ session_id: 'pty-live-1', text: h.seedText })
    expect(String((send?.body as { text: string }).text).startsWith('[from')).toBe(false)
    await waitFor(() => {
      expect(screen.queryByTestId('continue-new-chat')).toBeNull()
    })
    expect(useTabsStore.getState().activeTabId).not.toBe(sourceTabId)
    expect(screen.getByText('Finish the editor refactor')).toBeTruthy()
  })

  it('keeps the dialog open when send-message reports pty_died or throws', async () => {
    h.sessions.push(session())
    h.sendResult = {
      success: false,
      reason: 'pty_died',
      hint: 'the pty died',
      target_session_id: 'pty-live-1',
      attempts: 1,
    }
    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    await screen.findByTestId('continue-new-chat')
    expect(screen.queryByRole('button', { name: 'Copy text' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    await waitFor(() => {
      const created = useTabsStore.getState().tabs.find((tab) => tab.title.includes('(from Claude)'))
      expect(created).toBeTruthy()
    })
    const created = useTabsStore.getState().tabs.find((tab) => tab.title.includes('(from Claude)'))!
    act(() => {
      usePinnedSizeStore.getState().registerSession(created.paneGroups.values().next().value!.id, 'pty-live-1')
    })
    const dialog = await screen.findByRole('alert')
    expect(dialog.textContent).toContain('the pty died')
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Copy text' })).toBeTruthy()
    expect((screen.getByRole('radio', { name: /Recent turns/ }) as HTMLInputElement).checked).toBe(true)

    cleanup()
    h.posts.length = 0
    h.sendThrows = new Error('invalid or missing token')
    h.sendResult = {
      success: true,
      target_session_id: 'pty',
      attempts: 1,
      reason: null,
      hint: null,
    }
    useTabsStore.setState({ tabs: [], activeTabId: null, extraGroups: [], splitCount: 1, activeGroupIndex: 0 })
    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    await waitFor(() => {
      const created = useTabsStore.getState().tabs.find((tab) => tab.title.includes('(from Claude)'))
      expect(created).toBeTruthy()
    })
    const again = useTabsStore.getState().tabs.find((tab) => tab.title.includes('(from Claude)'))!
    act(() => {
      usePinnedSizeStore.getState().registerSession(again.paneGroups.values().next().value!.id, 'pty-live-2')
    })
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('invalid or missing token')
    expect(screen.getByTestId('continue-new-chat')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Copy text' })).toBeTruthy()
  })

  it('does not offer Copy text when continue-seed fails before returning text', async () => {
    h.sessions.push(session())
    h.seedThrows = new Error('no turns')
    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    await screen.findByTestId('continue-new-chat')
    fireEvent.click(screen.getByRole('button', { name: 'Start' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('no turns')
    expect(screen.queryByRole('button', { name: 'Copy text' })).toBeNull()
    expect(h.posts.some((post) => post.route === 'terminal/send-message')).toBe(false)
    expect(useTabsStore.getState().tabs.find((tab) => tab.title.includes('(from Claude)'))).toBeUndefined()
  })

  it('opens a themed harness menu and updates the trigger when another harness is chosen', async () => {
    h.sessions.push(session())
    render(<ChatHistory projectPath={PROJECT} />)
    await screen.findByText('Finish the editor refactor')
    await openMenu('Finish the editor refactor')
    fireEvent.click(screen.getByText('Continue in a new chat…'))
    const dialog = await screen.findByTestId('continue-new-chat')
    expect(dialog.querySelector('select')).toBeNull()

    const trigger = screen.getByLabelText('Harness')
    expect(trigger.tagName).toBe('BUTTON')
    expect(trigger.textContent).toContain('Claude')
    fireEvent.click(trigger)

    const menu = trigger.parentElement?.querySelector(':scope > div') ?? null
    expect(menu).toBeTruthy()
    expect(menu!.tagName).toBe('DIV')
    expect(menu!.className).toContain('bg-[var(--color-bg-surface)]')
    expect(menu!.className).toContain('border-[var(--color-border)]')
    expect(menu!.querySelector('select')).toBeNull()
    expect(menu!.querySelector('option')).toBeNull()
    const choices = Array.from(menu!.querySelectorAll('button'))
    expect(choices.length).toBeGreaterThan(1)
    for (const label of ['Claude', 'Cursor', 'Grok', 'Gemini', 'Pi', 'Codex', 'Hermes']) {
      const choice = choices.find((btn) => btn.textContent?.includes(label))
      expect(choice, label).toBeTruthy()
      expect(choice!.tagName).toBe('BUTTON')
    }
    fireEvent.click(choices.find((btn) => btn.textContent?.includes('Grok'))!)
    const updated = screen.getByLabelText('Harness')
    expect(updated.textContent).toContain('Grok')
    expect(updated.textContent).not.toContain('Claude')
    expect(h.posts).toEqual([])
  })
})
