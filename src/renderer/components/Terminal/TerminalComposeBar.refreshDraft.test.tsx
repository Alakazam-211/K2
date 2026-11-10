// @vitest-environment jsdom
// Refresh keeps the draft (Rosson 2026-11-10: "When pressing the refresh
// button to refresh a thread, don't lose the message that the user has
// typed up"). A refresh respawns the PTY: the pane remounts (AgentSessionChrome's
// nonce key, the pinned Chat's attach/refresh nonce) and the bar comes back
// with no sessionId, then a NEW one. The draft is keyed by the pane's agent
// (`draftId`), so it survives; another agent's bar never shows it; a sent
// message is gone after the refresh too. Fail loud.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  }),
  daemonCliPost: vi.fn(),
  picked: [] as string[][],
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  getLocalDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  invalidateDaemonWs: () => {},
  daemonWsBase: () => 'ws://127.0.0.1:1',
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))
vi.mock('@/lib/pick-compose-files', () => ({
  composeAttachPlan: () => ({ kind: 'native' }),
  pickLocalComposeFiles: vi.fn(async () => h.picked.shift() ?? null),
  pickRemoteComposeFile: vi.fn(async () => null),
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))

import { TerminalComposeBar } from './TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useContextMenuStore } from '@/stores/context-menu'

const WS = '/Users/me/ai/sales'
const scope = {
  ...fakeScope('local', { remote: false }),
  serverSupports: (f: string) => f === 'thread-attachments-v1',
} as ServerScope

const room = testRoom({
  tabs: {},
  scope,
  isPrimary: true,
  projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
  activeProjectId: 'p',
})

function chromeFor(addr: string): SessionViewChromeValue {
  return {
    viewTab: 'thread',
    splitLeft: 'terminal',
    splitRight: 'thread',
    overlayAddr: addr,
    conversationId: null,
    agentName: addr,
  }
}

interface BarAt {
  /** Bumped by a refresh: the pane remounts, as under AgentSessionChrome. */
  nonce: number
  sessionId: string
  draftId: string
  addr?: string
  dest?: 'thread' | 'pty'
}

function ui(at: BarAt): React.JSX.Element {
  return (
    <SessionViewChromeContext.Provider value={chromeFor(at.addr ?? 'sales')}>
      <TerminalComposeBar
        key={at.nonce}
        sessionId={at.sessionId}
        draftId={at.draftId}
        workspacePath={WS}
        sendDestination={at.dest ?? 'thread'}
      />
    </SessionViewChromeContext.Provider>
  )
}

function box(): HTMLTextAreaElement {
  return screen.getByRole('textbox') as HTMLTextAreaElement
}

function chipNames(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
}

/** A refresh as the person sees it: remount with no PTY, then the respawned one. */
async function refresh(rerender: (el: React.JSX.Element) => void, at: BarAt, newSessionId: string): Promise<BarAt> {
  const gone = { ...at, nonce: at.nonce + 1, sessionId: '' }
  await act(async () => {
    rerender(ui(gone))
  })
  const back = { ...gone, sessionId: newSessionId }
  await act(async () => {
    rerender(ui(back))
  })
  return back
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockClear()
  h.picked.length = 0
})

afterEach(() => {
  cleanup()
})

describe('refresh keeps the typed draft', () => {
  it('Thread bar: the text survives a refresh that respawns the PTY', async () => {
    const at: BarAt = { nonce: 0, sessionId: 'pty-old', draftId: 'tab-pg1' }
    const view = renderInRoom(room, ui(at))
    fireEvent.change(box(), { target: { value: 'half a thought about the release' } })
    expect(box().value).toBe('half a thought about the release')

    await refresh(view.rerender, at, 'pty-new')
    await waitFor(() => expect(box().value).toBe('half a thought about the release'))
  })

  it('Thread bar: picked files survive the refresh too', async () => {
    const at: BarAt = { nonce: 0, sessionId: 'pty-old', draftId: 'tab-pg1' }
    const view = renderInRoom(room, ui(at))
    h.picked.push([`${WS}/docs/plan.md`])
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Attach or save' }))
    })
    await act(async () => {
      useContextMenuStore.getState().selectItem('attach')
    })
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))

    await refresh(view.rerender, at, 'pty-new')
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
  })

  it('terminal bar (pinned Chat, Terminal view): the text survives a refresh', async () => {
    const at: BarAt = { nonce: 0, sessionId: 'pty-old', draftId: 'project-1', dest: 'pty' }
    const view = renderInRoom(room, ui(at))
    fireEvent.change(box(), { target: { value: 'ls the logs' } })

    await refresh(view.rerender, at, 'pty-new')
    await waitFor(() => expect(box().value).toBe('ls the logs'))
  })

  it("another agent's Thread does not get the draft", async () => {
    const at: BarAt = { nonce: 0, sessionId: 'pty-a', draftId: 'tab-a', addr: 'sales' }
    const view = renderInRoom(room, ui(at))
    fireEvent.change(box(), { target: { value: 'for sales only' } })

    // Same bar, reused for another agent's pane (a tab switch, no remount).
    await act(async () => {
      view.rerender(ui({ nonce: 0, sessionId: 'pty-b', draftId: 'tab-b', addr: 'reviewer' }))
    })
    await waitFor(() => expect(box().value).toBe(''))

    // And back: the first agent's draft is still its own.
    await act(async () => {
      view.rerender(ui(at))
    })
    await waitFor(() => expect(box().value).toBe('for sales only'))
  })

  it('a sent message is cleared and does not come back on refresh', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9, kind: 'text', from: 'owner', body: '' })
    const at: BarAt = { nonce: 0, sessionId: 'pty-old', draftId: 'tab-pg1' }
    const view = renderInRoom(room, ui(at))
    fireEvent.change(box(), { target: { value: 'ship it' } })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    })
    const posts = h.daemonCliPost.mock.calls.filter((c) => c[1] === 'thread/post')
    expect(posts).toHaveLength(1)
    expect((posts[0][2] as { text: string }).text).toBe('ship it')
    await waitFor(() => expect(box().value).toBe(''))

    await refresh(view.rerender, at, 'pty-new')
    await waitFor(() => expect(box().value).toBe(''))
  })
})
