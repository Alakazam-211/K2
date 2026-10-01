// Home M1 — ServerScope: the primary scope follows the window's active host
// at call time (exactly what getDaemonWs resolved before M1); a pinned scope
// is fixed to one saved server and reads its saved login at call time.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import {
  primaryScope,
  scopeForHost,
  scopedKey,
  hostKeyOf,
  noteServerVersion,
  ServerRemovedError,
  LOCAL_SCOPE_LABEL,
  __resetServerScopesForTests,
} from './server-scope'
import { canonicalHostKey, homeHostKey } from '@/lib/host-key'
import { invalidateDaemonWs } from './daemon-ws'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

function host(over: Partial<ConnectHost>): ConnectHost {
  return {
    id: 'h1',
    label: 'Box',
    hostname: 'rosson.k2.dev',
    port: 443,
    secure: true,
    token: 'tok-rosson',
    remember: false,
    lastConnectedAt: null,
    ...over,
  }
}

const ROSSON = host({})
const LAN = host({ id: 'h2', label: 'LAN', hostname: '192.168.1.20', port: 38471, secure: false, token: 'tok-lan' })

beforeEach(() => {
  mem.clear()
  __resetConnectHostStoreForTests()
  __resetServerScopesForTests()
  invalidateDaemonWs()
  invokeMock.mockReset()
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'daemon_ws_url') return { state: 'available', port: 50123, token: 'local-tok' }
    return null
  })
  useConnectHostStore.getState().addHost(ROSSON)
  useConnectHostStore.getState().addHost(LAN)
})

describe('primaryScope — the window active host at call time', () => {
  it('is one shared object, local while local is active', async () => {
    const s = primaryScope()
    expect(primaryScope()).toBe(s)
    expect(s.id).toBe('primary')
    expect(s.isPrimary).toBe(true)
    expect(s.hostKey).toBe('local')
    expect(s.connectionKey).toBe('local')
    expect(s.isRemote).toBe(false)
    expect(s.connectHost()).toBe(null)
    expect(s.isWindowHost()).toBe(true)
    const c = await s.creds()
    expect(c).toEqual({ port: 50123, token: 'local-tok', host: '127.0.0.1', secure: false })
    expect(await s.httpBase()).toBe('http://127.0.0.1:50123')
    expect(await s.wsBase()).toBe('ws://127.0.0.1:50123')
    expect(s.serverSupports('canonical-active')).toBe(true)
  })

  it('follows a server switch without being re-fetched', async () => {
    const s = primaryScope()
    useConnectHostStore.getState().selectHost(ROSSON)
    expect(s.hostKey).toBe('rosson.k2.dev')
    expect(s.connectionKey).toBe('h1:rosson.k2.dev:443')
    expect(s.isRemote).toBe(true)
    expect(s.connectHost()?.id).toBe('h1')
    expect(await s.httpBase()).toBe('https://rosson.k2.dev')
    expect(await s.wsBase()).toBe('wss://rosson.k2.dev')
    expect((await s.creds()).token).toBe('tok-rosson')
    // Unknown version on an active remote → gated features are false.
    expect(s.serverSupports('canonical-active')).toBe(false)
    useConnectHostStore.getState().setServerInfo({ version: '0.41.6', protocol: 1 })
    expect(s.serverSupports('canonical-active')).toBe(true)
  })
})

describe('host keys are canonical (MS60)', () => {
  it('one https server has one key however it is spelled', () => {
    for (const spelling of [
      'dtl.k2.dev',
      'DTL.k2.dev',
      ' dtl.k2.dev ',
      'dtl.k2.dev:443',
      'https://dtl.k2.dev',
      'https://dtl.k2.dev/',
      'https://DTL.k2.dev:443',
      'wss://dtl.k2.dev',
    ]) {
      expect(canonicalHostKey(spelling)).toBe('dtl.k2.dev')
    }
    expect(scopeForHost('https://dtl.k2.dev')).toBe(scopeForHost('dtl.k2.dev:443'))
  })

  it('plain http keeps its port, :80 included, so http and https never share a key', () => {
    expect(canonicalHostKey('http://box.lan')).toBe('box.lan:80')
    expect(canonicalHostKey('http://box.lan:80')).toBe('box.lan:80')
    expect(canonicalHostKey('box.lan:80')).toBe('box.lan:80')
    expect(canonicalHostKey('192.168.1.20:38471')).toBe('192.168.1.20:38471')
    expect(canonicalHostKey('http://192.168.1.20:38471/')).toBe('192.168.1.20:38471')
    expect(homeHostKey({ hostname: 'box.lan', port: 80, secure: false })).toBe('box.lan:80')
    expect(homeHostKey({ hostname: 'box.lan', port: 443, secure: true })).toBe('box.lan')
    expect(homeHostKey({ hostname: 'box.lan', port: 8443, secure: true })).toBe('box.lan:8443')
  })

  it('IPv6 literals are bracketed with a port', () => {
    expect(homeHostKey({ hostname: '::1', port: 38471, secure: false })).toBe('[::1]:38471')
    expect(homeHostKey({ hostname: '[FE80::1]', port: 443, secure: true })).toBe('[fe80::1]:443')
    expect(canonicalHostKey('http://[::1]:38471')).toBe('[::1]:38471')
    expect(canonicalHostKey('[::1]:38471')).toBe('[::1]:38471')
  })

  it('anything that is not a server address throws', () => {
    for (const bad of ['', 'https://', 'dtl.k2.dev/cli/x', 'user@dtl.k2.dev', 'dtl.k2.dev:0', 'dtl.k2.dev:99999', 'a b', 'dtl.k2.dev:abc', '[::1']) {
      expect(() => canonicalHostKey(bad)).toThrow('not a host key')
    }
  })
})

describe('scopeForHost — pinned to one saved server', () => {
  it('host keys match Home row keys (local, <sub>.k2.dev, ip:port)', () => {
    expect(hostKeyOf('local')).toBe('local')
    expect(hostKeyOf(ROSSON)).toBe('rosson.k2.dev')
    expect(hostKeyOf(LAN)).toBe('192.168.1.20:38471')
    expect(hostKeyOf('  Rosson.K2.dev ')).toBe('rosson.k2.dev')
  })

  it('returns the same object per host key, and never follows a switch', async () => {
    const a = scopeForHost(ROSSON)
    expect(scopeForHost('rosson.k2.dev')).toBe(a)
    expect(a.id).toBe('host:rosson.k2.dev')
    expect(a.isPrimary).toBe(false)
    expect(a.isWindowHost()).toBe(false)
    useConnectHostStore.getState().selectHost(LAN)
    expect(a.hostKey).toBe('rosson.k2.dev')
    expect(a.connectionKey).toBe('host:rosson.k2.dev')
    expect(await a.httpBase()).toBe('https://rosson.k2.dev')
    expect(a.isWindowHost()).toBe(false)
    useConnectHostStore.getState().selectHost(ROSSON)
    expect(a.isWindowHost()).toBe(true)
  })

  it('reads the saved token at call time, so a re-login is picked up', async () => {
    const a = scopeForHost(ROSSON)
    expect((await a.creds()).token).toBe('tok-rosson')
    useConnectHostStore.getState().setHostToken('h1', 'tok-rosson-2')
    expect((await a.creds()).token).toBe('tok-rosson-2')
  })

  it('LAN scope keeps its port; local scope uses this computer daemon even while remote is active', async () => {
    expect(await scopeForHost(LAN).httpBase()).toBe('http://192.168.1.20:38471')
    useConnectHostStore.getState().selectHost(ROSSON)
    const local = scopeForHost('local')
    expect(local.isRemote).toBe(false)
    expect(await local.httpBase()).toBe('http://127.0.0.1:50123')
    expect(local.serverSupports('canonical-active')).toBe(true)
  })

  it('a removed server rejects with ServerRemovedError (no fallback to the window host)', async () => {
    const ghost = scopeForHost('ghost.k2.dev')
    expect(ghost.connectHost()).toBe(null)
    await expect(ghost.creds()).rejects.toBeInstanceOf(ServerRemovedError)
    await expect(ghost.httpBase()).rejects.toThrow('no saved server for ghost.k2.dev')
    // A saved one that is then removed goes the same way.
    const a = scopeForHost(ROSSON)
    expect((await a.creds()).token).toBe('tok-rosson')
    useConnectHostStore.getState().removeHost('h1')
    const err = await a.creds().then(
      () => null,
      (e: unknown) => e,
    )
    expect(err).toBeInstanceOf(ServerRemovedError)
    expect((err as ServerRemovedError).hostKey).toBe('rosson.k2.dev')
  })

  it('label is the saved label, or This computer for local', () => {
    expect(LOCAL_SCOPE_LABEL).toBe('This computer')
    expect(primaryScope().label).toBe('This computer')
    expect(scopeForHost('local').label).toBe('This computer')
    expect(scopeForHost(ROSSON).label).toBe('Box')
    useConnectHostStore.getState().selectHost(LAN)
    expect(primaryScope().label).toBe('LAN')
  })

  it('serverSupports uses that server version, not the window one', () => {
    useConnectHostStore.getState().selectHost(LAN)
    useConnectHostStore.getState().setServerInfo({ version: '0.41.6', protocol: 1 })
    const b = scopeForHost(ROSSON)
    expect(b.serverSupports('canonical-active')).toBe(false)
    noteServerVersion('rosson.k2.dev', '0.39.10')
    expect(b.serverSupports('canonical-active')).toBe(false)
    noteServerVersion('rosson.k2.dev', '0.41.0')
    expect(b.serverSupports('canonical-active')).toBe(true)
  })

  it('a reported feature (spawn-attach-only) is only what that server listed, never its version', () => {
    const b = scopeForHost(ROSSON)
    // Unknown server: unsupported.
    expect(b.serverSupports('spawn-attach-only')).toBe(false)
    // A released daemon (no `features` in /boot-status) with the same
    // version string as main: still unsupported.
    noteServerVersion('rosson.k2.dev', '0.41.6', [])
    expect(b.serverSupports('spawn-attach-only')).toBe(false)
    expect(b.serverSupports('canonical-active')).toBe(true)
    noteServerVersion('rosson.k2.dev', '0.41.6', ['spawn-attach-only'])
    expect(b.serverSupports('spawn-attach-only')).toBe(true)
    // A later boot without the key (downgraded) takes it back.
    noteServerVersion('rosson.k2.dev', '0.41.6', [])
    expect(b.serverSupports('spawn-attach-only')).toBe(false)
    // The window's remote server: the same rule, from its own report.
    useConnectHostStore.getState().selectHost(LAN)
    expect(primaryScope().serverSupports('spawn-attach-only')).toBe(false)
    noteServerVersion('192.168.1.20:38471', '0.41.6', ['spawn-attach-only'])
    expect(primaryScope().serverSupports('spawn-attach-only')).toBe(true)
    // This computer's daemon supports everything.
    expect(scopeForHost('local').serverSupports('spawn-attach-only')).toBe(true)
  })

  it('scopedKey never collides across two servers for the same key', () => {
    const a = scopeForHost(ROSSON)
    const b = scopeForHost(LAN)
    expect(scopedKey(a, 'p1:w1')).toBe('rosson.k2.dev|p1:w1')
    expect(scopedKey(b, 'p1:w1')).toBe('192.168.1.20:38471|p1:w1')
    expect(scopedKey(primaryScope(), 'p1:w1')).toBe('local|p1:w1')
  })
})
