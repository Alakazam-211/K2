// @vitest-environment jsdom
// The project chat in the Thread's look (Rosson 2026-10-18), with its own
// data and behaviour kept:
//   - the stream is the Thread's bubbles ("You" on the right for the owner,
//     others on the left), markdown inside, in light, dark and a named
//     Style (Terminal's chamfers, Glass round);
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
import { TILE_FOOT_CLIP } from '@/components/ui/Surface'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const TERMINAL_CHAMFER = 'html[data-style="terminal"] [data-k2-thread-zen] [data-chamfer]'

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
    ['terminal', 'light'],
    ['terminal', 'dark'],
    ['glass', 'dark'],
    ['glass', 'light'],
  ] as const) {
    it(`draws the Thread's bubbles and box (${style} / ${scheme})`, async () => {
      document.documentElement.setAttribute('data-style', style)
      document.documentElement.setAttribute('data-scheme', scheme)
      const root = await mount()
      const scope = screen.getByTestId('project-chat-thread')
      expect(scope.hasAttribute('data-k2-thread-zen')).toBe(true)
      expect(scope.style.getPropertyValue('--zen-bubble-me')).toBe('var(--color-accent-fill)')
      const [agent, mine] = Array.from(root.querySelectorAll('[data-zen-message]')) as HTMLElement[]
      expect(agent.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('agent')
      expect(agent.querySelector('[data-thread-author]')?.textContent).toBe('Ada')
      expect(mine.querySelector('[data-zen-bubble]')?.getAttribute('data-zen-bubble')).toBe('me')
      // As the Thread: the name over another's run; yours has none, and its
      // status line under it.
      expect(agent.querySelector('[data-thread-meta="name"]')?.compareDocumentPosition(agent.querySelector('[data-zen-bubble]')!)).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
      expect(mine.hasAttribute('data-mine')).toBe(true)
      expect(mine.querySelector('[data-thread-author]')).toBeNull()
      expect(mine.querySelector('[data-thread-meta="status"]')).not.toBeNull()
      // Markdown in the bubble.
      const body = agent.querySelector('[data-zen-bubble]') as HTMLElement
      expect(body.querySelector('strong')?.textContent).toBe('ship')
      expect(Array.from(body.querySelectorAll('li')).map((li) => li.textContent)).toEqual(['one', 'two'])
      // The Style's shape: Terminal chamfers the bubble and the field.
      const field = screen.getByTestId('project-chat-compose').querySelector('[data-zen-compose-box]') as HTMLElement
      for (const el of [body, field]) expect([style, el.matches(TERMINAL_CHAMFER)]).toEqual([style, style === 'terminal'])
      // The old chat-row look is gone.
      expect(root.querySelector('.chat-message')).toBeNull()
    })
  }

  it('the Thread at the chat’s foot clips to the drawer tile’s inner curve (the drawer itself can’t: its resize edge hangs past it)', async () => {
    await mount()
    const scope = screen.getByTestId('project-chat-thread')
    for (const c of TILE_FOOT_CLIP.split(/\s+/)) expect(scope.classList).toContain(c)
  })

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

  it('post-idempotency-v1: a send that failed on the way re-sends the same words with the same id; a new text gets a new id', async () => {
    noteServerVersion('local', '0.46.8', ['thread-read-v1', 'daemon-activity', 'post-idempotency-v1'])
    await mount()
    const posted = (): Array<Record<string, unknown>> =>
      cli.post.mock.calls.filter(([p]) => p === 'project-group/msg').map(([, b]) => b as Record<string, unknown>)
    const base = cli.post.getMockImplementation()!
    cli.post.mockImplementationOnce(async () => {
      throw new TypeError('Load failed')
    })
    const input = screen.getByTestId('project-chat-input') as HTMLTextAreaElement
    fireEvent.change(input, { target: { value: 'ship it' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(screen.getByTestId('project-chat-send-error')).toBeTruthy())
    expect(screen.getByTestId('project-chat-send-error').textContent).toBe(
      'Couldn’t send your message (Load failed). It’s still in the box: press Enter to try again.',
    )
    expect(input.value).toBe('ship it')
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(posted()).toHaveLength(2))
    await waitFor(() => expect(input.value).toBe(''))
    const [first, retry] = posted()
    expect(first).toEqual({ group: 'g1', body: 'ship it', clientMsgId: expect.stringMatching(/^[0-9a-f]{32}$/) })
    expect(retry).toEqual(first)
    cli.post.mockImplementation(base)
    fireEvent.change(input, { target: { value: 'next one' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    await waitFor(() => expect(posted()).toHaveLength(3))
    expect(posted()[2].clientMsgId).toMatch(/^[0-9a-f]{32}$/)
    expect(posted()[2].clientMsgId).not.toBe(first.clientMsgId)
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
    expect(screen.getByText(/View only: you can read but not post/)).toBeTruthy()
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

describe('project chat states and header', () => {
  it('a failed read shows the state card with the reason; Try again is busy while it reads, then the chat loads', async () => {
    let fail = true
    let answer: () => void = () => {}
    cli.get.mockImplementation(async (path: string) => {
      if (path !== 'project-group/messages') return {}
      if (fail) throw new Error('database is locked')
      await new Promise<void>((r) => (answer = r))
      return { ok: true, messages: MESSAGES, truncated: false }
    })
    const view = render(<ProjectChatPanel show={show()} />)
    const card = await screen.findByTestId('project-chat-error')
    expect(card.getAttribute('role')).toBe('alert')
    expect(card.textContent).toContain('Couldn’t load the chat')
    expect(card.querySelector('[data-load-detail]')?.textContent).toBe('database is locked')
    expect(card.textContent).not.toContain('Failed to load chat')
    // The page's card, outside the Thread box (whose rules would keep the
    // card and its button square and drop the theme's focus ring).
    expect(card.closest('[data-k2-thread-zen]')).toBeNull()
    expect(card.getAttribute('data-k2-chamfer')).toBe('surface')
    const again = screen.getByRole('button', { name: 'Try again' })
    expect(again.closest('[data-k2-thread-zen]')).toBeNull()
    expect(again.className).toContain('k2-focus')
    // The message box is off while the chat isn't there.
    const input = screen.getByTestId('project-chat-input') as HTMLTextAreaElement
    expect(input.disabled).toBe(true)
    expect(input.placeholder).toBe('The chat didn’t load')
    expect(screen.queryByTestId('stash-button')).toBeNull()
    fail = false
    fireEvent.click(again)
    const busy = (await screen.findByRole('button', { name: 'Trying again…' })) as HTMLButtonElement
    expect(busy.disabled).toBe(true)
    expect(screen.getByTestId('project-chat-error')).toBeTruthy()
    await act(async () => answer())
    await waitFor(() => expect(view.container.querySelectorAll('[data-zen-message]').length).toBe(MESSAGES.length))
    expect(screen.queryByTestId('project-chat-error')).toBeNull()
    expect((screen.getByTestId('project-chat-input') as HTMLTextAreaElement).disabled).toBe(false)
  })

  it('while the chat loads: the loading card, and the message box is off', async () => {
    // No draft left from the tests above (the hint shows on an empty box).
    const { clearProjectChatDraft } = await import('@/lib/composer-drafts')
    clearProjectChatDraft('g1')
    cli.get.mockImplementation(async (path: string) =>
      path === 'project-group/messages' ? new Promise(() => {}) : {},
    )
    render(<ProjectChatPanel show={show()} />)
    const card = await screen.findByTestId('project-chat-loading')
    expect(card.closest('[data-k2-thread-zen]')).toBeNull()
    const input = screen.getByTestId('project-chat-input') as HTMLTextAreaElement
    expect(input.disabled).toBe(true)
    expect(input.placeholder).toBe('Loading the chat…')
    // The hint is drawn on one line (cut with an ellipsis), over the field.
    const hint = screen.getByTestId('project-chat-compose').querySelector('[data-compose-hint]') as HTMLElement
    expect(hint.textContent).toBe('Loading the chat…')
    expect(hint.className).toContain('truncate')
    expect(input.hasAttribute('data-own-hint')).toBe(true)
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(cli.post.mock.calls.filter(([p]) => p === 'project-group/msg')).toHaveLength(0)
  })

  it('an empty chat says who gets each message, in plain words', async () => {
    cli.get.mockImplementation(async (path: string) =>
      path === 'project-group/messages' ? { ok: true, messages: [], truncated: false } : {},
    )
    render(<ProjectChatPanel show={show()} />)
    const empty = await screen.findByTestId('project-chat-empty')
    expect(empty.textContent).toContain('No messages yet')
    expect(empty.textContent).toContain('Ada, the point of contact, gets each one.')
    expect(empty.textContent).not.toContain('PoC')
  })

  it('the header reads "Project chat" and names the point of contact in full', async () => {
    await mount()
    const title = screen.getByText('Project chat')
    expect(title.className).not.toContain('uppercase')
    // Two lines: the title over who gets each message.
    expect(title.nextElementSibling?.hasAttribute('data-project-chat-poc')).toBe(true)
    const poc = document.querySelector('[data-project-chat-poc]') as HTMLElement
    expect(poc.textContent).toBe('Point of contact: Ada')
    expect(poc.getAttribute('title')).toContain('Point of contact: Ada')
  })
})
