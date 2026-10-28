// @vitest-environment jsdom
//
// Accept cards from Home S2 (prd-accept-cards-from-home-v1 §3.2, AH-A1): a
// Garden's built-in conversation widget decides a secret handoff card on the
// AGENT's server (its feed's scope, with the person's login there), never on
// the window's server, and never with a widget `origin`.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { zenThreadFeeds, type ZenAgentRow, type ZenThreadView } from '@/lib/zen/zen-data'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { scopeForHost, __resetServerScopesForTests } from '@/kessel/server-scope'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { ZenConversation } from './ZenConversationWidget'
import { handoffApi } from '@/components/Settings/sections/handoff-api'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: '127.0.0.1',
  port: 59_997,
  secure: false,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}

class NoopResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

function bridge(): { bridge: ZenWidgetBridge; push(v: ZenThreadView): void } {
  let listener: ((v: ZenThreadView) => void) | null = null
  const b = {
    widgetId: 'conversation',
    caps: new Set<string>(['thread:read', 'thread:post']),
    call(verb: string, ...args: unknown[]): unknown {
      if (verb === 'thread.subscribe') {
        listener = args[1] as (v: ZenThreadView) => void
        return () => {
          listener = null
        }
      }
      if (verb === 'thread.read') return Promise.resolve()
      throw new Error(`unexpected bridge verb ${verb}`)
    },
    gardens: { list: () => [], current: () => null, switch: () => {}, create: () => Promise.reject(new Error('no')) },
    homes: { list: () => [] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return {
    bridge: b,
    push(v) {
      const fn = listener
      if (!fn) throw new Error('nobody subscribed to the thread')
      act(() => fn(v))
    },
  }
}

const ADDRESS = 'ops::127.0.0.1:59997'

function row(): ZenAgentRow {
  return {
    address: ADDRESS,
    label: 'ops',
    index: 0,
    hostKey: '127.0.0.1:59997',
    server: 'B',
    role: null,
    reach: 'live',
    auth: null,
    activity: 'idle',
    counts: null,
    working: false,
    needsYou: true,
    state: 'ok',
    stateLabel: null,
    detail: null,
    preview: null,
    people: [],
    selected: true,
    openable: true,
    avatarUrl: null,
    color: null,
    image: null,
  } as ZenAgentRow
}

const HANDOFF: OverlayThreadItem = {
  collection: 'thread',
  seq: 1,
  id: 'h1',
  doc: {
    id: 'h1',
    kind: 'handoff',
    from: 'k2',
    created_at: 1,
    body: 'sales sent STRIPE_KEY.',
    handoff: { deliveryId: 'hd_b1', name: 'STRIPE_KEY', from: 'sales', kind: 'api_key', hosts: [], note: '', clash: false, status: 'pending' },
  },
} as OverlayThreadItem

function view(items: OverlayThreadItem[]): ZenThreadView {
  return { address: ADDRESS, phase: 'ready', note: null, items, loaded: true, hasMore: false, loadingOlder: false, error: null }
}

beforeEach(() => {
  __resetServerScopesForTests()
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local' } as never)
  ;(globalThis as { ResizeObserver?: unknown }).ResizeObserver = NoopResizeObserver
})

afterEach(() => {
  cleanup()
  zenThreadFeeds.setState({ feeds: [] })
})

describe('the Garden conversation widget: a handoff card decides on the agent’s server', () => {
  it('Accept posts vault/incoming/decide to B with B’s login, never the window’s server, and no origin', async () => {
    zenThreadFeeds.setState({ feeds: [{ address: ADDRESS, scope: scopeForHost(B), threadAddr: 'ops', generation: 1 }] })
    const calls: { url: string; body: string }[] = []
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (input, init) => {
      calls.push({ url: String(input), body: String(init?.body ?? '') })
      return new Response(JSON.stringify({ ok: true, delivery: { status: 'landed' }, ref: 'k2v://ws/ops/STRIPE_KEY' }), { status: 200 })
    })
    try {
      const fb = bridge()
      const r = render(<ZenConversation bridge={fb.bridge} row={row()} />)
      fb.push(view([HANDOFF]))
      fireEvent.click(r.getByTestId('thread-handoff-accept'))
      await waitFor(() => expect(calls.length).toBe(1))
      const url = new URL(calls[0].url)
      expect(url.port).toBe(String(B.port))
      expect(url.pathname).toBe('/cli/vault/incoming/decide')
      expect(url.searchParams.get('token')).toBe('tok-b')
      expect(url.searchParams.has('origin')).toBe(false)
      const body = JSON.parse(calls[0].body) as Record<string, unknown>
      expect(body).toEqual({ id: 'hd_b1', action: 'accept' })
      expect('origin' in body).toBe(false)
    } finally {
      fetchSpy.mockRestore()
    }
  })

  it('before the feed is mounted the card has no Accept (no fallback to the window’s server)', () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch')
    try {
      const fb = bridge()
      const r = render(<ZenConversation bridge={fb.bridge} row={row()} />)
      fb.push(view([HANDOFF]))
      expect(r.queryByTestId('thread-handoff-accept')).toBeNull()
      expect(r.getByTestId('thread-handoff-remote').textContent).toBe('Open the agent’s server directly to accept this.')
      expect(fetchSpy).not.toHaveBeenCalled()
    } finally {
      fetchSpy.mockRestore()
    }
  })

  it('handoffApi.decide has no default server: the caller names it (checked by the typechecker)', () => {
    // @ts-expect-error the scope is required, so no card decides on the window's server by default
    const unscoped = (): unknown => handoffApi.decide({ id: 'hd_x', action: 'decline' })
    expect(typeof unscoped).toBe('function')
  })
})
