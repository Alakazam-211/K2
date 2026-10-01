// Home M1 — MS6 / MS52 f / MS60 / MS64: workspace-only keys carry the
// server's host key, the three older formats migrate exactly once, and two
// servers with the same workspace key never share a storage key or a map
// entry.

import { describe, it, expect, beforeEach, vi } from 'vitest'

class MemStorage {
  private m = new Map<string, string>()
  get length(): number {
    return this.m.size
  }
  key(i: number): string | null {
    return Array.from(this.m.keys())[i] ?? null
  }
  getItem(k: string): string | null {
    return this.m.has(k) ? this.m.get(k)! : null
  }
  setItem(k: string, v: string): void {
    this.m.set(k, v)
  }
  removeItem(k: string): void {
    this.m.delete(k)
  }
  clear(): void {
    this.m.clear()
  }
  dump(): Record<string, string> {
    return Object.fromEntries(Array.from(this.m.entries()).sort(([a], [b]) => a.localeCompare(b)))
  }
}

const mem = new MemStorage()
vi.stubGlobal('localStorage', mem)
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import {
  HOST_SCOPED_KEYS_MARKER,
  migrateHostScopedKeys,
  __resetHostScopedKeysMigrationForTests,
} from './host-scoped-storage'
import { scopeForHost, scopedKey, __resetServerScopesForTests } from '@/kessel/server-scope'
import { useSelectedTabsStore } from '@/stores/selected-tabs'
import { pinOf, sessionsOf, usePinnedSizeStore } from '@/stores/pinned-size'
import { getSessionLabel, useSessionLabelsStore } from '@/stores/session-labels'
import {
  lookupNamedChatTitle,
  rememberChatCustomName,
  __resetNamedChatTitleCachesForTests,
} from '@/lib/chat-session-tab'
import { migrateHomeRowHostKeys, type Home } from '@/stores/homes'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

function host(over: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'a',
    label: 'A',
    hostname: 'rosson.k2.dev',
    port: 443,
    secure: true,
    token: 'tok-a',
    remember: false,
    lastConnectedAt: null,
    ...over,
  }
}
const A = host({})
const B = host({ id: 'b', label: 'B', hostname: 'z3thon.k2.dev', token: 'tok-b' })
const LAN = host({ id: 'lan-id', label: 'LAN', hostname: '192.168.1.20', port: 38471, secure: false })

beforeEach(() => {
  mem.clear()
  __resetHostScopedKeysMigrationForTests()
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  __resetNamedChatTitleCachesForTests()
  useConnectHostStore.getState().addHost(A)
  useConnectHostStore.getState().addHost(B)
  usePinnedSizeStore.setState({ pins: {}, sessions: {}, dims: {} })
  useSessionLabelsStore.setState({ labels: {} })
  useSelectedTabsStore.setState({ selected: {} })
})

describe('MS64 one-time migration of the three key formats', () => {
  it('moves unprefixed keys to local|, maps activeHostKey keys to host keys, drops unknown ids', () => {
    const storage = new MemStorage()
    // Format 1: unprefixed.
    storage.setItem('k2:composer:draft:sess-1', 'hello')
    storage.setItem('k2:composer:draft:sess-1:thread', 'thread draft')
    storage.setItem('k2:composer:caret:sess-1', '{"start":3,"end":3}')
    storage.setItem('urls-ports.section-collapsed.p1', 'closed')
    storage.setItem('worktrees.section-collapsed.p1', 'closed')
    storage.setItem('workspace-api.section-collapsed.p1', 'open')
    storage.setItem('connected-agents.section-collapsed.p1', 'closed')
    storage.setItem('heartbeats.archive-collapsed.p1', 'open')
    storage.setItem('heartbeats.section-collapsed.p1', 'closed')
    storage.setItem('k2so:selected-tabs', JSON.stringify({ 'p1:w1': 'pg-a' }))
    // Format 2: keyed by the store's activeHostKey.
    storage.setItem('k2.showLaunchBar.local', '0')
    storage.setItem('k2.showLaunchBar.lan-id:192.168.1.20:38471', '0')
    storage.setItem('k2.showLaunchBar.gone:old.k2.dev:443', '0')
    storage.setItem('k2:session-view-tab:local:conv-1', 'split')
    storage.setItem('k2:session-view-split:lan-id:192.168.1.20:38471:conv-2', '{"left":"chat"}')
    storage.setItem('k2:project-groups:last-seen:a:rosson.k2.dev:443:g1', '1700000000')
    storage.setItem('k2:project-groups:last-seen:gone:old.k2.dev:443:g2', '1700000001')
    // Not ours: untouched.
    storage.setItem('k2.homes.v1', '{"version":1,"homes":[]}')
    storage.setItem('k2.connect-hosts.v1', '[]')
    storage.setItem('k2so.showHiddenFiles', 'true')

    expect(migrateHostScopedKeys(storage, [A, B, LAN])).toBe(true)
    expect(storage.dump()).toEqual({
      [HOST_SCOPED_KEYS_MARKER]: '1',
      '192.168.1.20:38471|k2.showLaunchBar': '0',
      '192.168.1.20:38471|k2:session-view-split:conv-2': '{"left":"chat"}',
      'k2.connect-hosts.v1': '[]',
      'k2.homes.v1': '{"version":1,"homes":[]}',
      'k2so.showHiddenFiles': 'true',
      'k2so:selected-tabs': JSON.stringify({ 'local|p1:w1': 'pg-a' }),
      'local|connected-agents.section-collapsed.p1': 'closed',
      'local|heartbeats.archive-collapsed.p1': 'open',
      'local|heartbeats.section-collapsed.p1': 'closed',
      'local|k2.showLaunchBar': '0',
      'local|k2:composer:caret:sess-1': '{"start":3,"end":3}',
      'local|k2:composer:draft:sess-1': 'hello',
      'local|k2:composer:draft:sess-1:thread': 'thread draft',
      'local|k2:session-view-tab:conv-1': 'split',
      'local|urls-ports.section-collapsed.p1': 'closed',
      'local|workspace-api.section-collapsed.p1': 'open',
      'local|worktrees.section-collapsed.p1': 'closed',
      'rosson.k2.dev|k2:project-groups:last-seen:g1': '1700000000',
    })
  })

  it('runs once: a second call (marker set) changes nothing', () => {
    const storage = new MemStorage()
    storage.setItem('k2:composer:draft:s', 'x')
    expect(migrateHostScopedKeys(storage, [])).toBe(true)
    storage.setItem('k2:composer:draft:later', 'y')
    expect(migrateHostScopedKeys(storage, [])).toBe(false)
    expect(storage.getItem('k2:composer:draft:later')).toBe('y')
    expect(storage.getItem('local|k2:composer:draft:s')).toBe('x')
  })

  it('the first scoped key in a webview migrates the real localStorage', () => {
    mem.setItem('heartbeats.section-collapsed.p9', 'closed')
    expect(scopedKey(scopeForHost('local'), 'heartbeats.section-collapsed.p9')).toBe(
      'local|heartbeats.section-collapsed.p9',
    )
    expect(mem.getItem('local|heartbeats.section-collapsed.p9')).toBe('closed')
    expect(mem.getItem('heartbeats.section-collapsed.p9')).toBe(null)
    expect(mem.getItem(HOST_SCOPED_KEYS_MARKER)).toBe('1')
  })
})

describe('MS52 f — same workspace key on two servers never shares a key', () => {
  const PROJECT = 'p-k2'
  const WORKSPACE = 'w-k2'

  it('storage keys differ', () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const base = `urls-ports.section-collapsed.${PROJECT}`
    expect(scopedKey(a, base)).toBe(`rosson.k2.dev|${base}`)
    expect(scopedKey(b, base)).toBe(`z3thon.k2.dev|${base}`)
  })

  it('selected tabs', () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const st = useSelectedTabsStore.getState()
    st.setSelected(a, PROJECT, WORKSPACE, 'tab-on-a')
    st.setSelected(b, PROJECT, WORKSPACE, 'tab-on-b')
    expect(st.getSelected(a, PROJECT, WORKSPACE)).toBe('tab-on-a')
    expect(useSelectedTabsStore.getState().getSelected(b, PROJECT, WORKSPACE)).toBe('tab-on-b')
    expect(JSON.parse(mem.getItem('k2so:selected-tabs')!)).toEqual({
      [`rosson.k2.dev|${PROJECT}:${WORKSPACE}`]: 'tab-on-a',
      [`z3thon.k2.dev|${PROJECT}:${WORKSPACE}`]: 'tab-on-b',
    })
  })

  it('pinned size, session labels and remembered chat names', () => {
    const a = scopeForHost(A)
    const b = scopeForHost(B)
    const ps = usePinnedSizeStore.getState()
    ps.registerSession(a, `agent-chat:${PROJECT}`, 'sess-a')
    ps.registerSession(b, `agent-chat:${PROJECT}`, 'sess-b')
    ps.setPin(a, 'sess-same', { cols: 100, rows: 30, setBy: 'owner' })
    const pinned = usePinnedSizeStore.getState()
    expect(sessionsOf(pinned.sessions, a)).toEqual({ [`agent-chat:${PROJECT}`]: 'sess-a' })
    expect(sessionsOf(pinned.sessions, b)).toEqual({ [`agent-chat:${PROJECT}`]: 'sess-b' })
    expect(pinOf(pinned, a, 'sess-same')).toEqual({ cols: 100, rows: 30, setBy: 'owner' })
    expect(pinOf(pinned, b, 'sess-same')).toBe(undefined)

    useSessionLabelsStore.getState().setSessionLabel(a, 'sess-same', 'label on A')
    expect(getSessionLabel(a, 'sess-same')).toBe('label on A')
    expect(getSessionLabel(b, 'sess-same')).toBe(undefined)

    rememberChatCustomName(a, 'conv-same', 'Name on A')
    expect(lookupNamedChatTitle(a, 'conv-same')).toBe('Name on A')
    expect(lookupNamedChatTitle(b, 'conv-same')).toBe(undefined)
  })
})

describe('MS60 — Home rows on plain http:80 keep their port', () => {
  const homes = (addresses: string[]): Home[] => [
    { id: 'h', name: 'Home', rows: addresses.map((address) => ({ address, workspaceId: null, label: address })) },
  ]
  const HTTP80 = { hostname: 'box.lan', port: 80, secure: false }

  it('rewrites a bare host whose only saved match is http:80', () => {
    const out = migrateHomeRowHostKeys(homes(['anna::box.lan', 'cortana::local']), [HTTP80])
    expect(out).not.toBe(null)
    expect(out![0]!.rows.map((r) => r.address)).toEqual(['anna::box.lan:80', 'cortana::local'])
  })

  it('leaves rows alone when an https entry still matches, or nothing is saved', () => {
    expect(
      migrateHomeRowHostKeys(homes(['anna::box.lan']), [HTTP80, { hostname: 'box.lan', port: 443, secure: true }]),
    ).toBe(null)
    expect(migrateHomeRowHostKeys(homes(['anna::box.lan']), [])).toBe(null)
    expect(migrateHomeRowHostKeys(homes(['anna::rosson.k2.dev']), [A])).toBe(null)
  })
})
