// @vitest-environment jsdom
// A side chat on ANOTHER server, made by an agent there: the tab opens while
// the side chat is still starting, before it has a Thread address. The
// address must reach the open tab, and a Thread send never waits forever.
// Fail loud — no skip.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => {
  const listQueue: Array<unknown | 'hang'> = []
  const sticky = { current: [] as unknown }
  const scopes: string[] = []
  const daemonCliGet = vi.fn(async (scope: ServerScope, route: string) => {
    scopes.push(scope.id)
    if (route === 'sessions/list-for-workspace') {
      const next = listQueue.length > 0 ? listQueue.shift() : sticky.current
      // A request that never answers (a Connect socket dropped mid-request).
      if (next === 'hang') return new Promise<never>(() => {})
      if (next instanceof Error) throw next
      return next
    }
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  })
  const daemonCliPost = vi.fn(async () => ({ ok: true }))
  return { listQueue, sticky, scopes, daemonCliGet, daemonCliPost }
})

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

import { AgentSessionChrome, SIDECAR_OVERLAY_ADDR_ATTEMPT_MS, useSidecarOverlayAddr } from './AgentSessionChrome'
import { TerminalComposeBar, THREAD_ADDR_WAIT_MS } from '@/components/Terminal/TerminalComposeBar'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'

const HOST = 'your-server.k2.dev'
const WS = '/home/k2/ai/builder'
const ROW = { agentName: 'tab-pg-remote', kind: 'sidecar', handle: 'builder/reviewer' }

function Harness() {
  const overlay = useSidecarOverlayAddr(WS, 'pg-remote')
  return (
    <AgentSessionChrome
      title={overlay.title || 'reviewer'}
      addr={overlay.addr}
      conversationId={null}
      agentName="tab-pg-remote"
      cwd={WS}
      addrProblem={overlay.problem}
      onRetryAddr={overlay.retry}
    >
      <TerminalComposeBar sessionId="sess-remote" workspacePath={WS} sendDestination="thread" />
    </AgentSessionChrome>
  )
}

function renderRemote(): void {
  const room = testRoom({
    tabs: {},
    scope: fakeScope(HOST, { remote: true }),
    isPrimary: false,
    localCommands: false,
    projects: [{ id: 'p-builder', name: 'builder', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p-builder',
  })
  renderInRoom(room, <Harness />)
}

function listCalls(): unknown[][] {
  return (h.daemonCliGet.mock.calls as unknown[][]).filter((c) => c[1] === 'sessions/list-for-workspace')
}

function threadPosts(): unknown[][] {
  return (h.daemonCliPost.mock.calls as unknown[][]).filter((c) => c[1] === 'thread/post')
}

function typeAndSend(text: string): HTMLTextAreaElement {
  const box = screen.getByRole('textbox') as HTMLTextAreaElement
  fireEvent.change(box, { target: { value: text } })
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
  return box
}

describe('a remote side chat made by an agent, opened before it has an address', () => {
  beforeEach(() => {
    cleanup()
    vi.useFakeTimers({ shouldAdvanceTime: true })
    h.listQueue.length = 0
    h.sticky.current = []
    h.scopes.length = 0
    h.daemonCliGet.mockClear()
    h.daemonCliPost.mockClear()
    localStorage.clear()
  })

  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('a lookup that never answers does not freeze the address: the waiting send goes once it comes', async () => {
    // First ask: the server is still making the side chat and the request
    // is lost. Every later ask: the side chat with its address.
    h.listQueue.push('hang')
    h.sticky.current = [ROW]
    renderRemote()
    await waitFor(() => expect(listCalls().length).toBe(1))
    expect(listCalls().every((c) => (c[0] as ServerScope).id === `host:${HOST}`)).toBe(true)

    const box = typeAndSend('are you there')
    expect(screen.getByTestId('compose-thread-waiting').textContent).toBe('Sends when reviewer is ready.')
    expect(threadPosts()).toEqual([])

    await act(async () => {
      await vi.advanceTimersByTimeAsync(SIDECAR_OVERLAY_ADDR_ATTEMPT_MS + 50)
    })
    await waitFor(() =>
      expect(threadPosts()).toEqual([
        [expect.objectContaining({ id: `host:${HOST}` }), 'thread/post', { addr: 'builder/reviewer', text: 'are you there', via: 'compose', clientMsgId: expect.stringMatching(/^[0-9a-f]{32}$/) }],
      ]),
    )
    await waitFor(() => expect(box.value).toBe(''))
    expect(screen.queryByTestId('compose-thread-waiting')).toBeNull()
    expect(screen.queryByTestId('compose-thread-addr-error')).toBeNull()
    // The ask that never answered was given up, so it holds no slot.
    const first = listCalls()[0][3] as { signal: AbortSignal }
    expect(first.signal.aborted).toBe(true)
  })

  it('no address in 10 s: stops waiting, says why, keeps the draft, and Try again sends once it comes', async () => {
    // The server answers, but never lists the side chat.
    h.sticky.current = []
    renderRemote()
    await waitFor(() => expect(listCalls().length).toBeGreaterThanOrEqual(1))

    const box = typeAndSend('ping')
    expect(screen.getByTestId('compose-thread-waiting')).toBeTruthy()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(THREAD_ADDR_WAIT_MS + 50)
    })
    expect(screen.queryByTestId('compose-thread-waiting')).toBeNull()
    const err = screen.getByTestId('compose-thread-addr-error')
    expect(err.textContent).toBe(`Can't reach reviewer's Thread: ${HOST} doesn't list this side chat.`)
    expect(box.value).toBe('ping')
    expect(threadPosts()).toEqual([])
    // Send is usable again (not stuck waiting).
    expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false)

    // The side chat comes up; Try again asks at once and sends.
    h.sticky.current = [ROW]
    const before = listCalls().length
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(listCalls().length).toBeGreaterThan(before))
    await waitFor(() =>
      expect(threadPosts()).toEqual([
        [expect.objectContaining({ id: `host:${HOST}` }), 'thread/post', { addr: 'builder/reviewer', text: 'ping', via: 'compose', clientMsgId: expect.stringMatching(/^[0-9a-f]{32}$/) }],
      ]),
    )
    await waitFor(() => expect(box.value).toBe(''))
    expect(screen.queryByTestId('compose-thread-addr-error')).toBeNull()
  })

  it('a failing ask is named as the reason', async () => {
    h.sticky.current = new Error('Failed to fetch')
    renderRemote()
    await waitFor(() => expect(listCalls().length).toBeGreaterThanOrEqual(1))
    typeAndSend('hello')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THREAD_ADDR_WAIT_MS + 50)
    })
    expect(screen.getByTestId('compose-thread-addr-error').textContent).toBe(
      `Can't reach reviewer's Thread: couldn't ask ${HOST} (Failed to fetch).`,
    )
  })

  it('a server that never answers is named as the reason', async () => {
    // Every ask hangs.
    for (let i = 0; i < 20; i++) h.listQueue.push('hang')
    renderRemote()
    await waitFor(() => expect(listCalls().length).toBe(1))
    typeAndSend('hello')
    await act(async () => {
      await vi.advanceTimersByTimeAsync(THREAD_ADDR_WAIT_MS + 50)
    })
    expect(screen.getByTestId('compose-thread-addr-error').textContent).toBe(
      `Can't reach reviewer's Thread: ${HOST} didn't answer.`,
    )
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('hello')
    // It kept asking while it waited (a hung ask is given up, not awaited).
    expect(listCalls().length).toBeGreaterThan(1)
  })
})
