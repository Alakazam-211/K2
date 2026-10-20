// @vitest-environment jsdom
// PRD boot-status-ghost (0.46.3) — the window's own server (R1).
//
// A 0.46.3 daemon answers `/boot-status` in three views. The desktop sends
// its token for a remote host, so it normally gets `full`. When it gets a
// stranger's view anyway (a stale session after a restart, or a host in
// ghost mode with a dead token), that body must NOT wipe the version and
// features the window already knows (B3) — every `serverSupports` gate would
// drop mid-session — and it must start the session-revive check (B4).

import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { noteAcceptedBootStatus, remoteHostPolicy } from './ConnectionGate'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'
import { serverSupports } from '@/lib/server-capabilities'
import {
  bootViewOf,
  isPartialBootView,
  nextAirgap,
  nextKnownVersion,
  protocolInRange,
  shouldRetryWithoutToken,
  tokenNotAccepted,
  bootStatusUrl,
  webBundleReloadUrl,
} from '@/lib/boot-status-view'

const HOST: ConnectHost = {
  id: 'id-rosson',
  label: 'Rosson',
  hostname: 'rosson.k2.dev',
  port: 443,
  secure: true,
  username: 'rosson',
  token: 'tok',
  remember: true,
  lastConnectedAt: null,
}

const FEATURES = ['spawn-attach-only', 'tickets-list-all', 'daemon-activity', 'boot-status-views-v1']

const FULL = {
  view: 'full',
  version: '0.46.3',
  protocol: 1,
  protocolMin: 1,
  phase: 'ready',
  detail: '',
  instanceId: 'aaaa000000000001',
  serverId: '0123456789abcdef',
  features: FEATURES,
  airgap: { enabled: false, baked: false },
}
const PUBLIC = { view: 'public', phase: 'ready', protocol: 1, protocolMin: 1, instanceId: 'bbbb000000000002' }
const COMPAT = {
  view: 'compat',
  phase: 'ready',
  protocol: 1,
  protocolMin: 1,
  instanceId: 'cccc000000000003',
  version: '0.46.4',
  features: [...FEATURES, 'thread-delta-v1'],
  serverId: '0123456789abcdef',
  publicHost: 'rosson.k2.dev',
  airgap: { enabled: false },
}

const REMOTE = { isRemote: true, appVersion: '0.46.3', tokenSent: true }

function gates(): Record<string, boolean> {
  const s = primaryScope()
  return {
    remoteUpdate: serverSupports('remote-update'),
    spawnAttachOnly: s.serverSupports('spawn-attach-only'),
    ticketsListAll: s.serverSupports('tickets-list-all'),
    daemonActivity: s.serverSupports('daemon-activity'),
  }
}

beforeEach(() => {
  localStorage.clear()
  useConnectHostStore.setState({ activeHost: HOST, hosts: [HOST], serverVersion: null, serverProtocol: null })
})

describe('R1 no clobber (B3) + stale-token signal (B4)', () => {
  it('full body then public body → serverVersion and every serverSupports gate unchanged', () => {
    expect(noteAcceptedBootStatus(FULL as never, REMOTE)).toBe(false)
    expect(useConnectHostStore.getState().serverVersion).toBe('0.46.3')
    const before = gates()
    expect(before).toEqual({ remoteUpdate: true, spawnAttachOnly: true, ticketsListAll: true, daemonActivity: true })

    // A token-sent poll answered with the public view: stale session.
    expect(noteAcceptedBootStatus(PUBLIC as never, REMOTE)).toBe(true)
    expect(useConnectHostStore.getState().serverVersion).toBe('0.46.3')
    expect(useConnectHostStore.getState().serverProtocol).toBe(1)
    expect(gates()).toEqual(before)
  })

  it('a compat body refreshes what it carries (version, features) and never clears', () => {
    noteAcceptedBootStatus(FULL as never, REMOTE)
    expect(noteAcceptedBootStatus(COMPAT as never, REMOTE)).toBe(true)
    expect(useConnectHostStore.getState().serverVersion).toBe('0.46.4')
    expect(primaryScope().serverSupports('daemon-activity')).toBe(true)
  })

  it('no token sent (signed out) → a public body is not a stale-token signal', () => {
    noteAcceptedBootStatus(FULL as never, REMOTE)
    expect(noteAcceptedBootStatus(PUBLIC as never, { ...REMOTE, tokenSent: false })).toBe(false)
    expect(useConnectHostStore.getState().serverVersion).toBe('0.46.3')
  })

  it('an OLD daemon (no `view`) keeps the pre-0.46.3 behaviour: its body is the truth', () => {
    noteAcceptedBootStatus(FULL as never, REMOTE)
    const old = { version: '0.46.2', protocol: 1, phase: 'ready', detail: '', features: ['spawn-attach-only'] }
    expect(noteAcceptedBootStatus(old as never, REMOTE)).toBe(false)
    expect(useConnectHostStore.getState().serverVersion).toBe('0.46.2')
    expect(primaryScope().serverSupports('daemon-activity')).toBe(false)
    // …and one without a version clears it, as before (an old daemon that omits it).
    noteAcceptedBootStatus({ protocol: 1, phase: 'ready', detail: '' } as never, REMOTE)
    expect(useConnectHostStore.getState().serverVersion).toBeNull()
  })

  it('local: the bundled app version stands in when the body has none', () => {
    useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
    expect(noteAcceptedBootStatus(PUBLIC as never, { isRemote: false, appVersion: '0.46.3', tokenSent: false })).toBe(false)
    expect(useConnectHostStore.getState().serverVersion).toBe('0.46.3')
  })
})

describe('remote acceptance with views', () => {
  it('accepts a public body (phase + protocol range are enough to mount)', () => {
    expect(remoteHostPolicy().decide(PUBLIC as never)).toEqual({ kind: 'accept' })
    expect(remoteHostPolicy().decide(COMPAT as never)).toEqual({ kind: 'accept' })
  })

  it('waits on a daemon that no longer serves this client’s protocol', () => {
    const d = remoteHostPolicy().decide({ ...PUBLIC, protocol: 3, protocolMin: 2 } as never)
    expect(d.kind).toBe('wait')
  })

  it('a migrating public body (no detail) reads migrating with empty detail', () => {
    expect(remoteHostPolicy().decide({ ...PUBLIC, phase: 'migrating' } as never)).toEqual({ kind: 'migrating', detail: '' })
  })
})

describe('lib/boot-status-view', () => {
  it('reads the view; old daemons have none', () => {
    expect(bootViewOf(FULL)).toBe('full')
    expect(bootViewOf(PUBLIC)).toBe('public')
    expect(bootViewOf({})).toBeUndefined()
    expect(bootViewOf({ view: 'weird' })).toBeUndefined()
    expect(isPartialBootView(PUBLIC)).toBe(true)
    expect(isPartialBootView(COMPAT)).toBe(true)
    expect(isPartialBootView(FULL)).toBe(false)
    expect(isPartialBootView({ version: '0.46.2' })).toBe(false)
  })

  it('tokenNotAccepted only for a token-sent stranger view', () => {
    expect(tokenNotAccepted(true, PUBLIC)).toBe(true)
    expect(tokenNotAccepted(true, COMPAT)).toBe(true)
    expect(tokenNotAccepted(true, FULL)).toBe(false)
    expect(tokenNotAccepted(true, { version: '0.46.2' })).toBe(false)
    expect(tokenNotAccepted(false, PUBLIC)).toBe(false)
  })

  it('no-clobber helpers', () => {
    expect(nextKnownVersion(PUBLIC, '0.46.3')).toBe('0.46.3')
    expect(nextKnownVersion(COMPAT, '0.46.3')).toBe('0.46.4')
    expect(nextKnownVersion({ phase: 'ready' } as never, '0.46.3')).toBeNull()
    expect(nextAirgap(PUBLIC)).toBeUndefined()
    expect(nextAirgap(COMPAT)).toBe(false)
    expect(nextAirgap({ airgap: { enabled: true } })).toBe(true)
    expect(nextAirgap({})).toBe(false)
  })

  it('hosted web reloads once onto the server bundle after a full body (ghost sign-in shell)', () => {
    const base = { isWeb: true, body: FULL, bundleVersion: '0.46.4', pathname: '/app/0.46.4/', alreadyTried: false }
    expect(webBundleReloadUrl(base)).toBe('/?v=0.46.3')
    expect(webBundleReloadUrl({ ...base, bundleVersion: '0.46.3' })).toBeNull()
    expect(webBundleReloadUrl({ ...base, alreadyTried: true })).toBeNull()
    expect(webBundleReloadUrl({ ...base, isWeb: false })).toBeNull()
    expect(webBundleReloadUrl({ ...base, pathname: '/' })).toBeNull()
    expect(webBundleReloadUrl({ ...base, body: PUBLIC })).toBeNull()
    expect(webBundleReloadUrl({ ...base, body: { ...FULL, version: '../x' } })).toBeNull()
    expect(webBundleReloadUrl({ ...base, bundleVersion: undefined })).toBeNull()
  })

  it('urls, retry rule and protocol range', () => {
    expect(bootStatusUrl('https://h', 'a b')).toBe('https://h/boot-status?token=a%20b')
    expect(bootStatusUrl('https://h', '')).toBe('https://h/boot-status')
    expect(bootStatusUrl('https://h', null)).toBe('https://h/boot-status')
    expect(shouldRetryWithoutToken(true, 403)).toBe(true)
    expect(shouldRetryWithoutToken(true, 401)).toBe(true)
    expect(shouldRetryWithoutToken(true, 404)).toBe(false)
    expect(shouldRetryWithoutToken(false, 403)).toBe(false)
    expect(protocolInRange({ protocol: 1, protocolMin: 1 })).toBe(true)
    expect(protocolInRange({ protocol: 1 })).toBe(true)
    expect(protocolInRange({ protocol: 2, protocolMin: 1 })).toBe(true)
    expect(protocolInRange({ protocol: 3, protocolMin: 2 })).toBe(false)
  })
})
