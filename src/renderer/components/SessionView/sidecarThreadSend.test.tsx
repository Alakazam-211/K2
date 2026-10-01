// @vitest-environment jsdom
// Sidecar Thread send keeps asking for its own address. An early Send shows
// an error and leaves the draft. Fail loud — no skip.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { useSessionViewChrome } from './sessionViewChrome'

const h = vi.hoisted(() => {
  const listQueue: unknown[] = []
  const sticky = { current: [] as unknown }
  const daemonCliGet = vi.fn(async (route: string) => {
    if (route === 'sessions/list-for-workspace') {
      const next = listQueue.length > 0 ? listQueue.shift() : sticky.current
      if (next instanceof Error) throw next
      return next
    }
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  })
  const daemonCliPost = vi.fn(async () => ({ ok: true }))
  return { listQueue, sticky, daemonCliGet, daemonCliPost }
})

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(h.daemonCliGet),
    daemonCliPost: primaryOnly(h.daemonCliPost),
  }
})

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

import { AgentSessionChrome, SIDECAR_OVERLAY_ADDR_RETRY_MS, useSidecarOverlayAddr } from './AgentSessionChrome'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'

const NOT_READY = "This session isn't ready yet. Your draft is still here."
const seenAddrs: string[] = []

function AddrProbe() {
  const chrome = useSessionViewChrome()
  seenAddrs.push(chrome?.overlayAddr ?? '')
  return null
}

function Harness() {
  const overlay = useSidecarOverlayAddr('/ws/sales', 'pg-1')
  return (
    <AgentSessionChrome
      title={overlay.title || 'agent'}
      addr={overlay.addr}
      conversationId={null}
      agentName="tab-pg-1"
      command="claude"
    >
      <AddrProbe />
      <TerminalComposeBar
        sessionId="sess-sidecar"
        workspacePath="/ws/sales"
        sendDestination="thread"
      />
    </AgentSessionChrome>
  )
}

function listCalls(): unknown[][] {
  const calls = h.daemonCliGet.mock.calls as unknown[][]
  return calls.filter((call) => call[0] === 'sessions/list-for-workspace')
}

function threadPosts(): unknown[][] {
  const calls = h.daemonCliPost.mock.calls as unknown[][]
  return calls.filter((call) => call[0] === 'thread/post')
}

function typeAndSend(text: string): HTMLTextAreaElement {
  const box = screen.getByRole('textbox') as HTMLTextAreaElement
  fireEvent.change(box, { target: { value: text } })
  const send = screen.getByRole('button', { name: 'Send message' })
  if (send.hasAttribute('disabled')) {
    throw new Error('Send is disabled; the draft never became sendable')
  }
  fireEvent.click(send)
  return box
}

describe('sidecar thread send address', () => {
  beforeEach(() => {
    cleanup()
    h.listQueue.length = 0
    h.sticky.current = []
    h.daemonCliGet.mockClear()
    h.daemonCliPost.mockClear()
    seenAddrs.length = 0
    localStorage.clear()
  })

  afterEach(() => {
    cleanup()
  })

  it('does not freeze on an empty lookup; a later row is the send address', async () => {
    const reviewer = [
      { agentName: 'proj', kind: 'canonical', handle: 'sales' },
      { agentName: 'tab-other', kind: 'sidecar', handle: 'sales/other' },
      { agentName: 'tab-pg-1', kind: 'sidecar', handle: 'sales/reviewer' },
    ]
    h.listQueue.push(
      [],
      [{ agentName: 'tab-pg-1', kind: 'sidecar', handle: 'sales' }],
      reviewer,
    )
    h.sticky.current = reviewer

    render(<Harness />)

    await waitFor(
      () => {
        expect(screen.getByTestId('sidecar-session-title').textContent).toBe('sales/reviewer')
      },
      { timeout: 2500 },
    )

    expect(seenAddrs).not.toContain('sales')
    expect(seenAddrs).not.toContain('sales/other')
    expect(seenAddrs).toContain('sales/reviewer')
    expect(listCalls().length).toBeGreaterThanOrEqual(3)
    expect(listCalls().every((call) => (call[1] as { path?: string } | undefined)?.path === '/ws/sales')).toBe(true)

    const callsAtReady = listCalls().length
    await new Promise((resolve) => setTimeout(resolve, SIDECAR_OVERLAY_ADDR_RETRY_MS + 150))
    expect(listCalls().length).toBe(callsAtReady)

    typeAndSend('ship the notes')
    await waitFor(() => {
      expect(threadPosts()).toEqual([
        ['thread/post', { addr: 'sales/reviewer', text: 'ship the notes', via: 'compose' }],
      ])
    })
    expect(screen.queryByTestId('compose-thread-addr-error')).toBeNull()
  })

  it('a thrown lookup does not freeze the address', async () => {
    h.listQueue.push(new Error('list failed'), [
      { agentName: 'tab-pg-1', kind: 'sidecar', handle: 'sales/1' },
    ])
    render(<Harness />)
    await waitFor(
      () => {
        expect(screen.getByTestId('sidecar-session-title').textContent).toBe('sales/1')
      },
      { timeout: 2500 },
    )
    expect(seenAddrs).toContain('sales/1')
    expect(seenAddrs).not.toContain('sales')
  })

  it('send before any address does not post and keeps the draft', async () => {
    h.sticky.current = []
    render(<Harness />)
    await waitFor(() => expect(listCalls().length).toBeGreaterThanOrEqual(1))

    const box = typeAndSend('  not yet  ')
    expect(box.value).toBe('  not yet  ')
    expect(screen.getByTestId('compose-thread-addr-error').textContent).toBe(NOT_READY)
    expect(threadPosts()).toEqual([])
    expect(screen.getByRole('button', { name: 'Send message' }).hasAttribute('disabled')).toBe(false)

    await waitFor(
      () => expect(listCalls().length).toBeGreaterThanOrEqual(2),
      { timeout: 2500 },
    )
    expect(threadPosts()).toEqual([])
    expect(box.value).toBe('  not yet  ')
    expect(screen.getByTestId('compose-thread-addr-error').textContent).toBe(NOT_READY)
    expect(screen.getByTestId('sidecar-session-title').textContent).toBe('agent')
    expect(seenAddrs).not.toContain('sales')
    expect(seenAddrs).not.toContain('sales/reviewer')
  })
})
