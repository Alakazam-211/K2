// @vitest-environment jsdom
// The project chat in the Thread's look (Rosson 2026-10-18), with its own
// data and behaviour kept:
//   - the stream is the Thread's bubbles ("You" on the right for the owner,
//     others on the left), markdown inside, in light, dark and a named
//     Style (Square's chamfers, Glass round);
//   - the message box is the Thread's; Enter sends over the project chat
//     route (`project-group/msg`), the delivered line rides under the sent
//     message;
//   - unread: on screen and focused, the newest seq is marked read over
//     `project-group/read`; the header's bell still mutes the chat;
//   - the PoC's working strip (the daemon's activity row for the PoC
//     workspace's agent) rides on the message box; no PoC, no strip;
//     a Viewer (no message box) gets it as a free tile.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
const cli = vi.hoisted(() => ({
  get: vi.fn(async (_path: string, _q?: unknown): Promise<unknown> => ({})),
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string, q?: unknown) => cli.get(path, q)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
    withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  }
})
const viewerRole = vi.hoisted(() => ({ current: 'owner' as 'owner' | 'member' | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => viewerRole.current) }))

import { act } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import ProjectChatPanel, { projectChatTarget } from './ProjectChatPanel'
import type { ProjectGroupMessage, ProjectGroupShow } from './projects-api'
import { noteServerVersion, primaryScope } from '@/kessel/server-scope'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { __resetUnreadForTests, useUnreadStore } from '@/stores/unread'
import { __resetActivityForTests, __seedActivityForTests, type ActivityRow } from '@/stores/activity'
import { usePromptStashStore } from '@/stores/prompt-stash'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const SQUARE_CHAMFER = 'html[data-style="square"] [data-k2-thread-zen] [data-chamfer]'

function show(over: Partial<ProjectGroupShow> = {}): ProjectGroupShow {
  return {
    id: 'g1',
    name: 'Launch',
    pocWorkspaceId: 'poc-ws',
    pinned: false,
    color: null,
    sortOrder: 0,
    createdAt: 1,
    updatedAt: 1,
    memberCount: 1,
    memberWorkspaceIds: ['poc-ws'],
    members: [{ workspaceId: 'poc-ws', name: 'ada-ws', path: '/ws/ada', agentName: 'Ada', createdAt: 1 }],
    dashboards: [],
    ...over,
  } as ProjectGroupShow
}

const NOW = Math.floor(Date.now() / 1000)
const MESSAGES: ProjectGroupMessage[] = [
  { id: 'm1', groupId: 'g1', author: 'Ada', body: 'Plan: **ship** it\n\n- one\n- two', createdAt: NOW - 120, seq: 2, authorKind: 'agent' },
  { id: 'm2', groupId: 'g1', author: 'owner', body: 'sounds good', createdAt: NOW - 60, seq: 3, authorKind: 'person' },
]

function activityRow(over: Partial<ActivityRow> = {}): ActivityRow {
  return {
    sessionId: 'pty-poc',
    agentName: 'poc-ws',
    projectId: 'poc-ws',
    workspacePath: '/ws/ada',
    harness: 'claude',
    display: 'working',
    lead: { state: 'working', outcome: 'none', since: Date.now() - 2_000, promptId: null },
    children: { subagents: 0, shells: 0, monitors: 0, crons: 0, unknown: 0, owed: 0, waiting: 0 },
    counts: { subagents: 1, tools: 3, commands: 1 },
    turnStartedAt: Date.now() - 9_000,
    evidenceAt: Date.now(),
    evidenceSource: 'hook',
    reason: 'tool_running',
    staleSince: null,
    confirmed: true,
    rev: 1,
    ...over,
  }
}

let hasFocus: ReturnType<typeof vi.spyOn>

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)))
}

async function mount(s: ProjectGroupShow = show()): Promise<HTMLElement> {
  const view = render(<ProjectChatPanel show={s} />)
  await waitFor(() => expect(view.container.querySelectorAll('[data-zen-message]').length).toBe(MESSAGES.length))
  return view.container
}

beforeEach(() => {
  __resetUnreadForTests()
  __resetActivityForTests()
  noteServerVersion('local', '0.46.1', ['thread-read-v1', 'daemon-activity'])
  viewerRole.current = 'owner'
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockImplementation(async (path: string) => {
    if (path === 'project-group/messages') return { ok: true, messages: MESSAGES, truncated: false }
    return {}
  })
  // The daemon's read cursor and mute, as it would answer them.
  const daemon = { muted: false }
  cli.post.mockImplementation(async (path: string, body?: unknown) => {
    const b = body as { group?: string; body?: string; seq?: number; id?: string; muted?: boolean; kind?: string }
    if (path === 'project-group/msg') {
      return { id: 'm3', groupId: b.group, author: 'owner', body: b.body, createdAt: NOW, seq: 4, delivered: true, deliveryReason: null, deliveredSessionId: 'pty-poc' }
    }
    if (path === 'project-group/read') {
      return { ok: true, changed: true, kind: 'project', id: b.group, seq: b.seq, lastReadSeq: b.seq, lastSeq: b.seq, unread: 0, waiting: 0, muted: daemon.muted, total: 0, waitingTotal: 0 }
    }
    if (path === 'thread/mute') {
      daemon.muted = b.muted === true
      return { ok: true, changed: true, kind: b.kind, id: b.id, seq: 3, lastReadSeq: 3, lastSeq: 3, unread: 0, waiting: 0, muted: b.muted, total: 0, waitingTotal: 0 }
    }
    return {}
  })
  useUnreadStore.setState({
    byKey: {
      'project:g1': { kind: 'project', id: 'g1', projectId: null, lastReadSeq: 1, lastSeq: 3, unread: 2, waiting: 0, muted: false, mutedUntil: null },
    },
    total: 2,
    waitingTotal: 0,
    loaded: true,
  })
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: false })
  hasFocus = vi.spyOn(document, 'hasFocus').mockImplementation(() => true)
})

afterEach(() => {
  cleanup()
  hasFocus.mockRestore()
  document.documentElement.removeAttribute('data-style')
  document.documentElement.removeAttribute('data-scheme')
})

describe('project chat in the Thread look', () => {
  for (const [style, scheme] of [
    ['square', 'light'],
    ['square', 'dark'],
    ['glass', 'dark'],
    ['bezel', 'light'],
  ] as const) {
    it(`draws the Thread's bubbles and box (${style} / ${scheme})`, async () => {
      document.documentElement.setAttribute('data-style', style)
      document.documentElement.setAttribute('data-scheme', scheme)
      const root = await mount()
      const scope = screen.getByTestId('project-chat-thread')
      expect(scope.hasAttribute('data-k2-thread-zen')).toBe(true)
      expect(scope.style.getPropertyValue('--zen-bubble-me')).toBe('var(--color-accent)')
      const [agent, mine] = Array.from(root.querySelectorAll('[data-zen-message]')) as HTMLElement[]
      expect(agent.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('agent')
      expect(agent.querySelector('[data-thread-author]')?.textContent).toBe('Ada')
      expect(mine.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('me')
      expect(mine.querySelector('[data-thread-author]')?.textContent).toBe('You')
      // Markdown in the bubble.
      const body = agent.querySelector('[data-zen-bubble]') as HTMLElement
      expect(body.querySelector('strong')?.textContent).toBe('ship')
      expect(Array.from(body.querySelectorAll('li')).map((li) => li.textContent)).toEqual(['one', 'two'])
      // The Style's shape: Square chamfers the bubble and the field.
      const field = screen.getByTestId('project-chat-compose').querySelector('[data-zen-compose-box]') as HTMLElement
      for (const el of [body, field]) expect([style, el.matches(SQUARE_CHAMFER)]).toEqual([style, style === 'square'])
      // The old chat-row look is gone.
      expect(root.querySelector('.chat-message')).toBeNull()
    })
  }

  it('Enter sends over the project chat route; the delivered line rides under the sent message', async () => {
    await mount()
    const input = screen.getByTestId('project-chat-input') as HTMLTextAreaElement
    expect(input.placeholder).toBe('Message Ada')
    fireEvent.change(input, { target: { value: 'go **now**' } })
    fireEvent.keyDown(input, { key: 'Enter', shiftKey: true })
    expect(cli.post.mock.calls.filter(([p]) => p === 'project-group/msg')).toHaveLength(0)
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(cli.post).toHaveBeenCalledWith('project-group/msg', { group: 'g1', body: 'go **now**' }))
    await waitFor(() => expect(input.value).toBe(''))
    const sent = await waitFor(() => {
      const el = document.querySelector('[data-zen-message="m3"]') as HTMLElement | null
      if (!el) throw new Error('the sent message is not drawn')
      return el
    })
    expect(sent.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('me')
    expect(sent.querySelector('[data-zen-message-footer]')?.textContent).toBe('delivered to Ada')
  })

  it('unread: on screen and focused, the newest seq is marked read; the bell mutes the chat', async () => {
    await mount()
    await waitFor(() => expect(cli.post).toHaveBeenCalledWith('project-group/read', { group: 'g1', seq: 3 }))
    const bell = document.querySelector('[data-unread-mute]') as HTMLButtonElement | null
    if (!bell) throw new Error('the mute bell is gone')
    expect(bell.getAttribute('data-unread-mute')).toBe('on')
    fireEvent.click(bell)
    await waitFor(() => expect(cli.post).toHaveBeenCalledWith('thread/mute', { kind: 'project', id: 'g1', muted: true }))
    await waitFor(() => expect(useUnreadStore.getState().byKey['project:g1']?.muted).toBe(true))
  })

  it('collapsed (not on screen): nothing is marked read', async () => {
    useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: true })
    await mount()
    await flush()
    expect(cli.post.mock.calls.filter(([p]) => p === 'project-group/read')).toHaveLength(0)
  })

  it('the PoC’s working strip rides on the message box', async () => {
    expect(projectChatTarget('poc-ws')).toEqual({ agentName: 'poc-ws' })
    expect(projectChatTarget(null)).toBeNull()
    __seedActivityForTests(primaryScope(), { rows: [activityRow()], supported: true })
    await mount()
    const compose = screen.getByTestId('project-chat-compose')
    const drawer = compose.querySelector('[data-zen-drawer]') as HTMLElement | null
    if (!drawer) throw new Error('no working strip on the message box')
    expect(drawer.getAttribute('data-zen-drawer-attached')).toBe('true')
    expect(drawer.querySelector('[aria-label]')?.getAttribute('aria-label')).toMatch(/^Agent working/)
  })

  it('no PoC: no strip, even while some agent works', async () => {
    __seedActivityForTests(primaryScope(), { rows: [activityRow()], supported: true })
    await mount(show({ pocWorkspaceId: null }))
    expect(screen.getByTestId('project-chat-compose').querySelector('[data-zen-drawer]')).toBeNull()
  })

  it('the PoC idle: no strip', async () => {
    __seedActivityForTests(primaryScope(), {
      rows: [activityRow({ display: 'idle', lead: { state: 'idle', outcome: 'success', since: 1, promptId: null }, counts: { subagents: 0, tools: 0, commands: 0 } })],
      supported: true,
    })
    await mount()
    expect(document.querySelector('[data-zen-drawer]')).toBeNull()
  })

  it('a login that may not post: no message box; the strip is a free tile', async () => {
    viewerRole.current = null
    __seedActivityForTests(primaryScope(), { rows: [activityRow()], supported: true })
    await mount()
    await flush()
    expect(screen.queryByTestId('project-chat-compose')).toBeNull()
    expect(screen.getByText(/Viewers can read project chat/)).toBeTruthy()
    const drawer = document.querySelector('[data-zen-drawer]') as HTMLElement | null
    if (!drawer) throw new Error('no free strip')
    expect(drawer.getAttribute('data-zen-drawer-attached')).toBe('false')
  })
})

describe('the stash in project chat (Rosson 2026-10-30)', () => {
  it('a bookmark left of Send on an empty box, hidden on typing; it opens the stash drawer (one ✕) and a pick fills the box', async () => {
    usePromptStashStore.setState({
      loaded: true,
      items: [
        {
          v: 1,
          id: 's1',
          text: 'saved for the project',
          quotes: [],
          attachments: [],
          created_at: 1_000,
          updated_at: 1_000,
          deleted_at: null,
          source: { server_id: null, server_label: null, user: null, agent: 'Ada', agent_name: 'Ada' },
        },
      ],
      markUsed: vi.fn(async () => undefined),
    })
    await mount()
    const input = screen.getByTestId('project-chat-input') as HTMLTextAreaElement
    const btn = screen.getByTestId('stash-button')
    expect(btn.nextElementSibling).toBe(screen.getByTestId('project-chat-send'))
    expect(btn.getAttribute('aria-label')).toBe('Stash (1 item)')
    fireEvent.change(input, { target: { value: 'x' } })
    expect(screen.queryByTestId('stash-button')).toBeNull()
    fireEvent.change(input, { target: { value: '' } })
    fireEvent.click(screen.getByTestId('stash-button'))
    const drawer = await screen.findByTestId('stash-drawer')
    expect(drawer.querySelectorAll('button[aria-label="Close stash"]')).toHaveLength(1)
    fireEvent.click(drawer.querySelector('[data-stash-row]') as HTMLElement)
    await waitFor(() => expect(input.value).toBe('saved for the project'))
    await waitFor(() => expect(screen.queryByTestId('stash-drawer')).toBeNull())
  })
})
