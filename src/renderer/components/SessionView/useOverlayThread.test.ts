// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { act, renderHook, waitFor } from '@testing-library/react'
import { OVERLAY_PAGE_SIZE } from './overlayThread'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())
const getDaemonWs = vi.hoisted(() =>
  vi.fn(async () => {
    throw new Error('ws unused when conversation_id is empty')
  }),
)

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet,
  daemonCliPost,
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs,
  daemonWsBase: () => 'ws://test',
}))

import { ingestOverlayThreadItem } from './overlayThread'
import { useOverlayThread } from './useOverlayThread'
import { useOverlayChatter } from './useOverlayChatter'

function pageItems(start: number, end: number, collection: 'thread' | 'chatter') {
  const items = []
  for (let seq = start; seq <= end; seq++) {
    items.push({
      collection,
      seq,
      id: `${collection}-${seq}`,
      doc: {
        id: `${collection}-${seq}`,
        kind: collection === 'chatter' ? 'chatter' : 'text',
        from: 'k2',
        body: `m${seq}`,
      },
    })
  }
  return items
}

describe('useOverlayThread paging', () => {
  afterEach(() => {
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
  })

  it('initial GET uses limit 50; loadOlder prepends unique items', async () => {
    daemonCliGet
      .mockResolvedValueOnce({
        conversation_id: '',
        has_more: true,
        items: pageItems(11, 60, 'thread'),
      })
      .mockResolvedValueOnce({
        conversation_id: '',
        has_more: false,
        items: pageItems(1, 10, 'thread'),
      })

    const { result } = renderHook(() =>
      useOverlayThread({ addr: 'sales', conversationId: null, enabled: true }),
    )
    await waitFor(() => expect(result.current.items).toHaveLength(50))
    expect(daemonCliGet).toHaveBeenCalledWith('thread', { addr: 'sales', limit: OVERLAY_PAGE_SIZE })
    expect(result.current.hasMore).toBe(true)
    expect(result.current.items[0].seq).toBe(11)
    expect(result.current.items[49].seq).toBe(60)

    await act(async () => {
      await result.current.loadOlder()
    })
    expect(daemonCliGet).toHaveBeenCalledWith('thread', {
      addr: 'sales',
      limit: OVERLAY_PAGE_SIZE,
      before_seq: 11,
    })
    expect(result.current.items).toHaveLength(60)
    expect(result.current.items[0].seq).toBe(1)
    expect(result.current.hasMore).toBe(false)
  })

  it('loadOlder is a no-op when hasMore is false', async () => {
    daemonCliGet.mockResolvedValueOnce({
      conversation_id: '',
      has_more: false,
      items: pageItems(1, 3, 'thread'),
    })
    const { result } = renderHook(() =>
      useOverlayThread({ addr: 'sales', conversationId: null, enabled: true }),
    )
    await waitFor(() => expect(result.current.items).toHaveLength(3))
    await act(async () => {
      await result.current.loadOlder()
    })
    expect(daemonCliGet).toHaveBeenCalledTimes(1)
  })

  it('compose ingest appends a new thread row without waiting on WS', async () => {
    daemonCliGet.mockResolvedValueOnce({
      conversation_id: 'conv-1',
      has_more: false,
      items: pageItems(1, 2, 'thread'),
    })
    const { result } = renderHook(() =>
      useOverlayThread({ addr: 'sales', conversationId: 'conv-1', enabled: true }),
    )
    await waitFor(() => expect(result.current.items).toHaveLength(2))
    act(() => {
      ingestOverlayThreadItem({
        collection: 'thread',
        seq: 3,
        id: 'thread-3',
        conversation_id: 'conv-1',
        doc: { id: 'thread-3', kind: 'text', from: 'rosson', body: 'from compose', via: 'compose' },
      })
    })
    expect(result.current.items).toHaveLength(3)
    expect(result.current.items[2].doc.body).toBe('from compose')
  })
})

describe('useOverlayChatter paging', () => {
  afterEach(() => {
    daemonCliGet.mockReset()
  })

  it('initial GET uses limit 50; loadOlder prepends', async () => {
    daemonCliGet
      .mockResolvedValueOnce({
        conversation_id: '',
        has_more: true,
        items: pageItems(11, 60, 'chatter'),
      })
      .mockResolvedValueOnce({
        conversation_id: '',
        has_more: false,
        items: pageItems(1, 10, 'chatter'),
      })

    const { result } = renderHook(() =>
      useOverlayChatter({ addr: 'sales', conversationId: null, enabled: true }),
    )
    await waitFor(() => expect(result.current.items).toHaveLength(50))
    expect(daemonCliGet).toHaveBeenCalledWith('chatter', { addr: 'sales', limit: OVERLAY_PAGE_SIZE })
    expect(result.current.hasMore).toBe(true)

    await act(async () => {
      await result.current.loadOlder()
    })
    expect(daemonCliGet).toHaveBeenCalledWith('chatter', {
      addr: 'sales',
      limit: OVERLAY_PAGE_SIZE,
      before_seq: 11,
    })
    expect(result.current.items).toHaveLength(60)
    expect(result.current.items[0].id).toBe('chatter-1')
    expect(result.current.hasMore).toBe(false)
  })
})

class FakeOverlayWS {
  static instances: FakeOverlayWS[] = []
  url: string
  readyState = 1
  onmessage: ((ev: { data: string }) => void) | null = null
  onerror: ((ev: Event) => void) | null = null
  onopen: ((ev: Event) => void) | null = null
  closed = false
  constructor(url: string) {
    this.url = url
    FakeOverlayWS.instances.push(this)
  }
  close() {
    this.closed = true
    this.readyState = 3
  }
}

describe('overlay hooks disable', () => {
  afterEach(() => {
    daemonCliGet.mockReset()
    daemonCliPost.mockReset()
    getDaemonWs.mockReset()
    getDaemonWs.mockImplementation(async () => {
      throw new Error('ws unused when conversation_id is empty')
    })
    FakeOverlayWS.instances = []
    vi.unstubAllGlobals()
  })

  it('skips GET while enabled is false', async () => {
    daemonCliGet.mockResolvedValue({
      conversation_id: '',
      has_more: false,
      items: pageItems(1, 2, 'thread'),
    })
    const { result } = renderHook(() =>
      useOverlayThread({ addr: 'sales', conversationId: null, enabled: false }),
    )
    await act(async () => {
      await Promise.resolve()
    })
    expect(daemonCliGet).not.toHaveBeenCalled()
    expect(result.current.items).toEqual([])
  })

  it('closes overlay WS and clears items when enabled flips false', async () => {
    vi.stubGlobal('WebSocket', FakeOverlayWS)
    getDaemonWs.mockResolvedValue({
      host: '127.0.0.1',
      port: 1,
      token: 't',
      secure: false,
    })
    daemonCliGet.mockResolvedValue({
      conversation_id: 'conv-1',
      has_more: false,
      items: pageItems(1, 1, 'thread'),
    })
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) =>
        useOverlayThread({ addr: 'sales', conversationId: 'conv-1', enabled }),
      { initialProps: { enabled: true } },
    )
    await waitFor(() => expect(FakeOverlayWS.instances.length).toBeGreaterThan(0))
    expect(result.current.items).toHaveLength(1)
    rerender({ enabled: false })
    await waitFor(() => expect(result.current.items).toHaveLength(0))
    expect(FakeOverlayWS.instances.every((ws) => ws.closed)).toBe(true)
  })

  it('closes chatter overlay WS when enabled flips false', async () => {
    vi.stubGlobal('WebSocket', FakeOverlayWS)
    getDaemonWs.mockResolvedValue({
      host: '127.0.0.1',
      port: 1,
      token: 't',
      secure: false,
    })
    daemonCliGet.mockResolvedValue({
      conversation_id: 'conv-1',
      has_more: false,
      items: pageItems(1, 1, 'chatter'),
    })
    const { result, rerender } = renderHook(
      ({ enabled }: { enabled: boolean }) =>
        useOverlayChatter({ addr: 'sales', conversationId: 'conv-1', enabled }),
      { initialProps: { enabled: true } },
    )
    await waitFor(() => expect(FakeOverlayWS.instances.length).toBeGreaterThan(0))
    expect(result.current.items).toHaveLength(1)
    rerender({ enabled: false })
    await waitFor(() => expect(result.current.items).toHaveLength(0))
    expect(FakeOverlayWS.instances.every((ws) => ws.closed)).toBe(true)
  })
})
