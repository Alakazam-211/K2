// @vitest-environment jsdom
//
// Project chat: "Earlier messages" keeps the message being read where it
// is (usePrependHold), including when the reader scrolls while the longer
// read is on its way. The panel used to set scrollTop after the read from
// the height and scrollTop it kept when the read started, undoing that
// scrolling. The fake layout is the chats' (test-utils/chat-layout).
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
const cli = vi.hoisted(() => ({
  get: vi.fn(async (_path: string, _q?: unknown): Promise<unknown> => ({})),
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
  /** The longer read's answer, when the test says. */
  earlier: null as null | (() => void),
}))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((path: string, q?: unknown) => cli.get(path, q)),
    daemonCliPost: primaryOnly((path: string, body?: unknown) => cli.post(path, body)),
    withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
  }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))

import { act } from 'react'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import ProjectChatPanel from './ProjectChatPanel'
import type { ProjectGroupMessage, ProjectGroupShow } from './projects-api'
import { noteServerVersion } from '@/kessel/server-scope'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import {
  chatFrames,
  chatLayout,
  installChatLayout,
  readerScroll,
  rowOnScreen,
  rowTop,
  stubChatFrames,
  uninstallChatLayout,
} from '@/test-utils/chat-layout'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const SHOW = {
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
} as unknown as ProjectGroupShow

const ALL: ProjectGroupMessage[] = Array.from({ length: 300 }, (_, i) => ({
  id: `p${i + 1}`,
  groupId: 'g1',
  author: i % 2 ? 'owner' : 'Ada',
  body: `b${i + 1}`,
  createdAt: 1_700_000_000 + i,
  seq: i + 1,
  authorKind: i % 2 ? 'person' : 'agent',
}))
const newest = (limit: number): ProjectGroupMessage[] => ALL.slice(ALL.length - limit)

beforeAll(() => installChatLayout({ view: 400, rowPx: (key) => 30 + (Number(key.slice(1)) % 4) * 5 }))
afterAll(uninstallChatLayout)

beforeEach(() => {
  stubChatFrames()
  noteServerVersion('local', '0.46.1', ['thread-read-v1'])
  cli.earlier = null
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockImplementation(async (path: string, q?: unknown) => {
    if (path !== 'project-group/messages') return {}
    const limit = (q as { limit?: number } | undefined)?.limit ?? 20
    if (limit <= 20) return { ok: true, messages: newest(limit), truncated: true }
    return new Promise((resolve) => {
      cli.earlier = () => resolve({ ok: true, messages: newest(limit), truncated: true })
    })
  })
  cli.post.mockImplementation(async () => ({}))
  usePageViewStore.setState({ page: 'projects' })
  useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: false })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

describe('project chat: Earlier messages', () => {
  it('scrolling while the read loads is kept, and the row being read stays put when it lands', async () => {
    render(<ProjectChatPanel show={SHOW} />)
    await screen.findByText('b300')
    vi.useFakeTimers()
    chatFrames(5)
    for (let i = 0; i < 50 && chatLayout.scrollTop > 0; i++) readerScroll(-300)
    expect(chatLayout.scrollTop).toBe(0)
    await act(async () => {
      fireEvent.click(screen.getByTestId('project-chat-load-earlier'))
    })
    expect(cli.earlier).not.toBeNull()
    // The reader reads on down while the read is on its way.
    readerScroll(150)
    chatFrames(3)
    const before = rowOnScreen()
    expect(before).not.toBeNull()
    await act(async () => {
      cli.earlier?.()
    })
    chatFrames(3)
    expect(rowTop(before?.key ?? '')).toBe(before?.top)
    expect(document.querySelector('[data-thread-column] > [data-zen-message="p281"]')).not.toBeNull()
  })
})
