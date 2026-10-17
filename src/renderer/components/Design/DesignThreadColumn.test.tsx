// @vitest-environment jsdom
// The Design column's Thread (Rosson 2026-10-16), fail loud: it is its own
// Thread instance for the workspace's address (overlay list + the Thread
// message box), it shows a post sent "via Design", and it never writes the
// pinned Chat's remembered view.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, screen } from '@testing-library/react'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'

const h = vi.hoisted(() => ({
  items: [] as unknown[],
  hookArgs: [] as Array<{ addr: string; enabled: boolean }>,
  composeProps: [] as Array<{ sendDestination?: string; sessionId: string }>,
}))

vi.mock('@/components/SessionView/useOverlayThread', () => ({
  useOverlayThread: (opts: { addr: string; enabled: boolean }) => {
    h.hookArgs.push({ addr: opts.addr, enabled: opts.enabled })
    return {
      items: h.items,
      conversationId: 'conv-1',
      error: null,
      posting: false,
      post: async () => {},
      answer: async () => {},
      voidCard: async () => {},
      hasMore: false,
      loadingOlder: false,
      loaded: true,
      loadOlder: async () => {},
      turn: null,
      turnsReported: true,
      currentAddr: opts.addr,
      pastAddresses: [],
    }
  },
}))
vi.mock('@/lib/chat-session-tab', async () => {
  const actual = await vi.importActual<typeof import('@/lib/chat-session-tab')>('@/lib/chat-session-tab')
  return {
    ...actual,
    resolvePinnedChatCopyableAddress: vi.fn(async () => ({ clipboard: 'docs', display: 'docs' })),
  }
})
vi.mock('@/components/Terminal/TerminalComposeBar', () => ({
  TerminalComposeBar: (props: { sendDestination?: string; sessionId: string }) => {
    h.composeProps.push(props)
    return <div data-testid="design-thread-compose" data-destination={props.sendDestination} />
  },
}))

import { primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { projectsStoreOf, renderInRoom, testRoom } from '@/test-utils/room'
import { DesignThreadColumn } from './DesignThreadColumn'

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const room = testRoom({ tabs: {}, scope: primaryScope(), projects: projectsStoreOf([]) })

const DESIGN_POST: OverlayThreadItem = {
  collection: 'thread',
  seq: 4,
  id: 't-design',
  doc: {
    id: 't-design',
    kind: 'text',
    from: 'owner',
    via: 'compose',
    created_at: Math.floor(Date.now() / 1000),
    body: 'Design notes (1) on http://localhost:5173/\n1. make it bigger\nDetails: `.k2/design/20261016-153012-make-it-bigger.md`',
    design: { itemId: 'd1', url: 'http://localhost:5173/', notes: 1 },
  },
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', ResizeObserverStub)
  setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
  h.items = []
  h.hookArgs = []
  h.composeProps = []
  localStorage.clear()
})

afterEach(() => {
  cleanup()
  resetThreadViewersForTests()
  vi.unstubAllGlobals()
})

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  })
}

describe('DesignThreadColumn', () => {
  it('renders a Thread view (list + Thread message box) for the workspace address', async () => {
    renderInRoom(room, <DesignThreadColumn projectPath="/w/docs" agentName="docs" visible />)
    await flush()
    expect(screen.getByTestId('agent-session-thread')).toBeTruthy()
    expect(screen.getByTestId('thread-overlay-pane')).toBeTruthy()
    expect(h.hookArgs.at(-1)).toEqual({ addr: 'docs', enabled: true })
    expect(screen.getByTestId('design-thread-compose').getAttribute('data-destination')).toBe('thread')
  })

  it('shows the post sent via Design', async () => {
    h.items = [DESIGN_POST]
    renderInRoom(room, <DesignThreadColumn projectPath="/w/docs" agentName="docs" visible />)
    await flush()
    expect(screen.getByText('You · via Design')).toBeTruthy()
    expect(screen.getByText(/make it bigger/)).toBeTruthy()
  })

  it('a hidden Design tab pauses the Thread; nothing writes a remembered view', async () => {
    renderInRoom(room, <DesignThreadColumn projectPath="/w/docs" agentName="docs" visible={false} />)
    await flush()
    expect(h.hookArgs.at(-1)).toEqual({ addr: 'docs', enabled: false })
    expect(Object.keys(localStorage).filter((k) => k.includes('session-view') || k.includes('viewTab'))).toEqual([])
  })
})
