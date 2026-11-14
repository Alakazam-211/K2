// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async () => {
    throw new Error('no reads here')
  },
  daemonCliPost: async () => {
    throw new Error('no posts here')
  },
}))

import { useHomesStore, type HomeRow } from '@/stores/homes'
import { useUnreadStore, unreadKey } from '@/stores/unread'
import { hostPool } from '@/lib/host-pool-instance'
import { homePrefetchTargets, PREFETCH_TOP_ROWS } from './home-thread-prefetch'

function row(handle: string, host: string, workspaceId: string | null = null): HomeRow {
  return { address: `${handle}::${host}`, workspaceId, label: handle }
}

beforeEach(() => {
  useUnreadStore.setState({ byKey: {}, total: 0, waitingTotal: 0 })
  hostPool.store.setState({ entries: {} } as never)
})

describe('homePrefetchTargets', () => {
  it('the shown Home’s top 8 rows, plus lower rows with unread (this computer and another server)', () => {
    const rows = Array.from({ length: 12 }, (_, i) => row(`a${i}`, 'local', `p${i}`))
    rows.push(row('far', 'b.example.test', 'wb'))
    useHomesStore.setState({ homes: [{ id: 'h1', name: 'Group 1', rows }], selectedId: 'h1' } as never)
    // a10 has unread on this computer; `far` has unread on its server.
    useUnreadStore.setState({
      byKey: {
        [unreadKey('thread', 'conv-a10')]: {
          kind: 'thread', id: 'conv-a10', projectId: 'p10', lastReadSeq: 1, lastSeq: 3, unread: 2, waiting: 0, muted: false, mutedUntil: null,
        },
      },
    })
    hostPool.store.setState({ entries: { 'b.example.test': { activity: [{ workspaceId: 'wb', unread: 4 }] } } } as never)
    const got = homePrefetchTargets()
    expect(PREFETCH_TOP_ROWS).toBe(8)
    expect(got).toEqual([
      ...Array.from({ length: 8 }, (_, i) => ({ hostKey: 'local', addr: `a${i}` })),
      { hostKey: 'local', addr: 'a10' },
      { hostKey: 'b.example.test', addr: 'far' },
    ])
  })

  it('no Homes: nothing', () => {
    useHomesStore.setState({ homes: [], selectedId: null } as never)
    expect(homePrefetchTargets()).toEqual([])
  })
})
