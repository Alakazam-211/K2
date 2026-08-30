// @vitest-environment jsdom
import { describe, expect, it, afterEach, vi } from 'vitest'
import { act, renderHook, waitFor } from '@testing-library/react'
import { OVERLAY_PAGE_SIZE } from './overlayThread'

const daemonCliGet = vi.hoisted(() => vi.fn())
const daemonCliPost = vi.hoisted(() => vi.fn())

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet,
  daemonCliPost,
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: async () => {
    throw new Error('ws unused when conversation_id is empty')
  },
  daemonWsBase: () => 'ws://test',
}))

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
