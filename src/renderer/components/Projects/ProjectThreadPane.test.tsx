// @vitest-environment jsdom
// A project's chat on the Thread store (`project-chat-thread-v1`):
//   - the Projects page's chat draws the agent chat's Thread pane on
//     `project:<id>` (its address and its conversation);
//   - the floating header has search, the point of contact's name and the
//     mute bell, and no terminal drawer, view menu, Chatter or Heartbeats;
//   - the box has no "/" commands, its draft is the project's, and a post
//     goes to `project:<id>`; its answer's delivery line shows over the box;
//   - a login with no Connect role yet reads only (no box);
//   - an older server keeps the old panel.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
const cli = vi.hoisted(() => ({
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
  get: vi.fn(async (_path: string, _q?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string, q?: unknown) => cli.get(path, q)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
    withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  }
})
const role = vi.hoisted(() => ({ value: 'owner' as string | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: async () => role.value,
}))
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))
vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({ AgentAvatar: () => null }))
// The Thread's list is the agent chat's own (its tests cover it); this
// stand-in records where it reads from and draws the real message box.
const column = vi.hoisted(() => ({ props: [] as Array<{ addr: string; conversationId: string | null }> }))
vi.mock('@/components/SessionView/ThreadOverlayColumn', () => ({
  ThreadOverlayColumn: (p: { addr: string; conversationId: string | null; composeBar: React.ReactNode }) => {
    column.props.push({ addr: p.addr, conversationId: p.conversationId })
    return (
      <div data-testid="stub-thread-column" data-addr={p.addr}>
        {p.composeBar}
      </div>
    )
  },
}))

import type React from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { ProjectChatSurface } from './ProjectThreadPane'
import { projectChatThreadOn } from './project-chat-thread-contract'
import type { ProjectGroupShow } from './projects-api'
import { primaryScope } from '@/kessel/server-scope'
import {
  PROJECT_CHAT_THREAD_FEATURE,
  projectConversationId,
  projectDraftId,
  projectPostOutcome,
  projectThreadAddr,
} from './project-chat-thread-contract'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const NOW = 1_759_241_200
const GROUP = 'g-0000-4000-8000-000000000001'

const SHOW = {
  id: GROUP,
  name: 'Launch',
  pocWorkspaceId: 'ws-ada',
  pinned: false,
  sortOrder: 0,
  createdAt: NOW,
  updatedAt: NOW,
  color: null,
  memberCount: 1,
  iconKey: null,
  defaultView: 'thread',
  members: [{ workspaceId: 'ws-ada', name: 'Ada', agentName: 'ada', joinedAt: NOW }],
  dashboards: [],
} as unknown as ProjectGroupShow

let features = new Set<string>()
let projectAnswer: Record<string, unknown> = { groupId: GROUP, groupName: 'Launch', delivered: false, deliveryReason: 'queued' }

function postAnswer(body: { text: string }): Record<string, unknown> {
  return {
    ok: true, id: 'm-new', seq: 4, from: 'owner', to: `project:${GROUP}`, kind: 'text', body: body.text, via: 'project',
    author: { kind: 'owner', login: 'owner' }, created_at: NOW, conversation_id: `project:${GROUP}`, addr: `project:${GROUP}`, mine: true,
    project: projectAnswer,
  }
}

function surface(): HTMLElement {
  return render(<ProjectChatSurface show={SHOW} width={360} threadOn={projectChatThreadOn(primaryScope())} />).container
}

beforeEach(() => {
  localStorage.clear()
  role.value = 'owner'
  column.props = []
  projectAnswer = { groupId: GROUP, groupName: 'Launch', delivered: false, deliveryReason: 'queued' }
  features = new Set([PROJECT_CHAT_THREAD_FEATURE])
  vi.spyOn(primaryScope(), 'serverSupports').mockImplementation((f) => features.has(f))
  cli.post.mockReset()
  cli.post.mockImplementation(async (path: string, body?: unknown) => {
    if (path === 'thread/post') return postAnswer(body as { text: string })
    return { ok: true }
  })
  cli.get.mockReset()
  cli.get.mockImplementation(async () => ({}))
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

describe('the contract seam', () => {
  it('a project chat is `project:<id>` as its address, conversation and draft', () => {
    expect(projectThreadAddr(GROUP)).toBe(`project:${GROUP}`)
    expect(projectConversationId(GROUP)).toBe(`project:${GROUP}`)
    expect(projectDraftId(GROUP)).toBe(`project:${GROUP}`)
    expect(PROJECT_CHAT_THREAD_FEATURE).toBe('project-chat-thread-v1')
  })

  it('a post’s answer says whether the point of contact got the line', () => {
    expect(projectPostOutcome({ ok: true, project: { delivered: false, deliveryReason: 'queued' } })).toEqual({ delivered: false, deliveryReason: 'queued' })
    expect(projectPostOutcome({ ok: true, project: { delivered: true, deliveryReason: null } })).toEqual({ delivered: true, deliveryReason: null })
    expect(projectPostOutcome({ ok: true, project: { delivered: null } })?.delivered).toBeNull()
    expect(projectPostOutcome({ ok: true })).toBeNull()
  })
})

describe('project-chat-thread-v1 on', () => {
  it('mounts the Thread pane on the project’s conversation', () => {
    surface()
    expect(screen.getByTestId('project-thread-pane')).toBeTruthy()
    expect(screen.queryByTestId('project-chat-thread')).toBeNull()
    expect(column.props.at(-1)).toEqual({ addr: `project:${GROUP}`, conversationId: `project:${GROUP}` })
  })

  it('the header: search, the point of contact, the bell; no terminal drawer, view menu, Chatter or Heartbeats', () => {
    surface()
    const header = screen.getByTestId('project-thread-header')
    expect(within(header).queryByTestId('thread-terminal-toggle')).toBeNull()
    expect(within(header).queryByTestId('thread-panel-toggle-chatter')).toBeNull()
    expect(within(header).queryByTestId('thread-panel-toggle-heartbeats')).toBeNull()
    expect(within(header).queryByTestId('session-view-menu')).toBeNull()
    expect(within(header).getByTestId('thread-view-name').textContent).toBe('ada')
    expect(header.querySelector('[data-project-chat-close]')).toBeTruthy()
  })

  it('a post goes to project:<id> once, and its delivery line shows over the box', async () => {
    surface()
    await waitFor(() => expect(within(screen.getByTestId('stub-thread-column')).queryByRole('textbox')).toBeTruthy())
    const box = within(screen.getByTestId('stub-thread-column')).getByRole('textbox') as HTMLTextAreaElement
    expect(box.placeholder).toBe('Message ada')
    fireEvent.change(box, { target: { value: 'Kickoff at noon' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    await waitFor(() => expect(cli.post.mock.calls.map((c) => c[0])).toContain('thread/post'))
    const post = cli.post.mock.calls.find((c) => c[0] === 'thread/post')![1] as { addr: string; text: string; clientMsgId?: unknown }
    expect(post).toMatchObject({ addr: `project:${GROUP}`, text: 'Kickoff at noon' })
    expect(typeof post.clientMsgId).toBe('string')
    // Never the old route.
    expect(cli.post.mock.calls.map((c) => c[0])).not.toContain('project-group/msg')
    await waitFor(() => expect(screen.getByTestId('project-thread-delivery').textContent).toBe('sending to ada'))
  })

  it('a line the point of contact didn’t get says so', async () => {
    projectAnswer = { groupId: GROUP, delivered: false, deliveryReason: 'workspace_not_found' }
    surface()
    await waitFor(() => expect(within(screen.getByTestId('stub-thread-column')).queryByRole('textbox')).toBeTruthy())
    const box = within(screen.getByTestId('stub-thread-column')).getByRole('textbox')
    fireEvent.change(box, { target: { value: 'hello' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    await waitFor(() => expect(screen.getByTestId('project-thread-delivery').textContent).toBe('ada didn’t get it: its session isn’t answering'))
  })

  it('the box has no "/" commands and no card UI', async () => {
    surface()
    await waitFor(() => expect(within(screen.getByTestId('stub-thread-column')).queryByRole('textbox')).toBeTruthy())
    const box = screen.getByTestId('stub-thread-column')
    expect(within(box).queryByTestId('compose-slash-button')).toBeNull()
    fireEvent.change(within(box).getByRole('textbox'), { target: { value: '/as' } })
    expect(screen.queryByTestId('compose-slash-menu')).toBeNull()
  })

  it('the draft is the project’s, never the point of contact’s own chat’s', async () => {
    surface()
    await waitFor(() => expect(within(screen.getByTestId('stub-thread-column')).queryByRole('textbox')).toBeTruthy())
    fireEvent.change(within(screen.getByTestId('stub-thread-column')).getByRole('textbox'), { target: { value: 'about the deck' } })
    const keys = Object.keys(localStorage).filter((k) => k.includes('k2:composer:draft:'))
    expect(keys.some((k) => k.endsWith(`k2:composer:draft:agent:project:${GROUP}:thread`))).toBe(true)
    expect(keys.some((k) => k.endsWith('k2:composer:draft:agent:ws-ada:thread'))).toBe(false)
  })

  it('a login with no Connect role yet reads only: no box', async () => {
    role.value = null
    surface()
    await act(async () => {})
    expect(screen.getByTestId('project-thread-pane').hasAttribute('data-project-thread-readonly')).toBe(true)
    expect(within(screen.getByTestId('stub-thread-column')).queryByRole('textbox')).toBeNull()
  })
})

describe('project-chat-thread-v1 off (an older server)', () => {
  it('keeps the old panel', () => {
    features = new Set()
    surface()
    expect(screen.queryByTestId('project-thread-pane')).toBeNull()
    expect(document.querySelector('[data-project-chat-header]')).toBeTruthy()
    expect(column.props).toEqual([])
  })
})
