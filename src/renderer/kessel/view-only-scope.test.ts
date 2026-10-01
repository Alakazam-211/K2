// Home M4 — a view-only (preview) room is strictly non-mutating on its
// server: the request layer refuses every POST on its scope except the
// keep-alive, before any request leaves; the twin is the same server
// (same id, host key and creds); the primary room on a remote window loses
// this computer's commands (MS57); browser webviews and file drops stay in
// their room's server.

import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  VIEW_ONLY_POST_ROUTES,
  ViewOnlyWriteError,
  primaryScope,
  scopeForHost,
  viewOnlyScope,
  __resetServerScopesForTests,
} from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { primaryRoom, __resetPrimaryRoomForTests } from '@/stores/room'
import { browserNativeItemId, hostHash, refuseLoopbackInRoom } from '@/components/BrowserPane/BrowserPane'
import { fileDropRefusal } from '@/lib/file-drag'

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: '127.0.0.1',
  port: 59_999,
  secure: false,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}

beforeEach(() => {
  __resetServerScopesForTests()
  __resetPrimaryRoomForTests()
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local' } as never)
})

describe('view-only scope', () => {
  it('is the same server: same id, host key and creds; one twin per scope', async () => {
    const base = scopeForHost(B)
    const twin = viewOnlyScope(base)
    expect(twin.viewOnly).toBe(true)
    expect(base.viewOnly).toBe(undefined)
    expect(twin.id).toBe(base.id)
    expect(twin.hostKey).toBe(base.hostKey)
    expect(twin.isRemote).toBe(true)
    expect(await twin.creds()).toEqual(await base.creds())
    expect(viewOnlyScope(base)).toBe(twin)
    expect(viewOnlyScope(twin)).toBe(twin)
  })

  it('refuses every POST but the keep-alive before any request leaves', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch')
    const twin = viewOnlyScope(scopeForHost(B))
    for (const route of ['workspace-layouts/save', 'sessions/v2/close', 'fs/write-file', 'terminal/send-message', 'thread/post']) {
      await expect(daemonCliPost(twin, route, {})).rejects.toBeInstanceOf(ViewOnlyWriteError)
    }
    expect(fetchSpy).not.toHaveBeenCalled()
    // The keep-alive, and file search (a read the daemon takes as POST).
    expect([...VIEW_ONLY_POST_ROUTES]).toEqual(['projects/activate', 'fs/search-tree'])
    fetchSpy.mockRestore()
  })
})

describe('the primary room on a remote window (MS57)', () => {
  it('runs this computer’s commands only while the window is on this computer', () => {
    const room = primaryRoom()
    expect(room.localCommands).toBe(true)
    useConnectHostStore.setState({ activeHost: { ...B } } as never)
    expect(primaryScope().isRemote).toBe(true)
    expect(room.localCommands).toBe(false)
    useConnectHostStore.setState({ activeHost: 'local' } as never)
    expect(room.localCommands).toBe(true)
  })
})

describe('browser webviews in a pinned room (MS20/MS69)', () => {
  it('the native id carries a host hash in a pinned room only; the layout id is the suffix', () => {
    expect(browserNativeItemId({ isPrimary: true, scope: { hostKey: 'b.k2.dev' } }, 'item-1')).toBe('item-1')
    const a = browserNativeItemId({ isPrimary: false, scope: { hostKey: 'a.k2.dev' } }, 'item-1')
    const b = browserNativeItemId({ isPrimary: false, scope: { hostKey: 'b.k2.dev' } }, 'item-1')
    expect(a).toBe(`${hostHash('a.k2.dev')}-item-1`)
    expect(a).not.toBe(b)
    expect(hostHash('b.k2.dev')).toMatch(/^[0-9a-f]{8}$/)
  })

  it('localhost is this computer: refused in a room on another server, allowed on this computer', () => {
    expect(refuseLoopbackInRoom({ isRemote: true }, 'http://localhost:3000/')).not.toBe(null)
    expect(refuseLoopbackInRoom({ isRemote: false }, 'http://localhost:3000/')).toBe(null)
    expect(refuseLoopbackInRoom({ isRemote: true }, 'https://example.com/')).toBe(null)
  })
})

describe('file drops across rooms (MS4/MS19)', () => {
  const a = { hostKey: 'local', label: 'This computer' }
  const b = { hostKey: 'b.k2.dev', label: 'B' }

  it('refuses a drop into a room on another server or into a view-only room', () => {
    expect(fileDropRefusal({ scope: b as never }, { scope: a, readOnly: false })).toMatch(/can't be dropped/)
    expect(fileDropRefusal({ scope: b as never }, { scope: b, readOnly: true })).toMatch(/View only/)
    expect(fileDropRefusal({ scope: a as never }, { scope: a, readOnly: false })).toBe(null)
    expect(fileDropRefusal({ scope: a as never }, null)).toBe(null)
  })
})
