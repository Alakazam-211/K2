// Unit tests for the host-aware daemon-ws URL helpers + getDaemonWs
// remote branch (K2 Connect step #1).
//
// `getDaemonWs()` for the LOCAL host calls the Tauri `invoke` command;
// we mock @tauri-apps/api/core so the local path resolves deterministically
// and assert it carries host '127.0.0.1' (byte-identical to pre-host-aware).
//
// For a REMOTE active host, getDaemonWs derives creds directly from the
// connect-host store with NO invoke — we assert that and the URL shape.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// localStorage stub so the connect-host store (imported transitively) is
// happy in the node env.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// Mock the Tauri invoke used by the LOCAL branch.
const invokeMock = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

import {
  getDaemonWs,
  invalidateDaemonWs,
  daemonHttpBase,
  daemonWsBase,
} from './daemon-ws'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
} from '@/stores/connect-host'

describe('daemon-ws URL helpers', () => {
  it('daemonHttpBase builds http://host:port for non-secure', () => {
    expect(daemonHttpBase({ host: '127.0.0.1', port: 47800, token: 't', secure: false })).toBe(
      'http://127.0.0.1:47800',
    )
    expect(daemonHttpBase({ host: 'box.example', port: 9000, token: 't', secure: false })).toBe(
      'http://box.example:9000',
    )
  })

  it('daemonWsBase builds ws://host:port for non-secure', () => {
    expect(daemonWsBase({ host: '127.0.0.1', port: 47800, token: 't', secure: false })).toBe(
      'ws://127.0.0.1:47800',
    )
  })

  // K2 Connect step #4 — TLS scheme selection.
  it('secure remote on 443 → https/wss with the port OMITTED', () => {
    const creds = { host: 'rosson.k2.dev', port: 443, token: 't', secure: true }
    expect(daemonHttpBase(creds)).toBe('https://rosson.k2.dev')
    expect(daemonWsBase(creds)).toBe('wss://rosson.k2.dev')
    // The shape a live socket builds.
    expect(`${daemonWsBase(creds)}/cli/sessions/grid`).toBe(
      'wss://rosson.k2.dev/cli/sessions/grid',
    )
  })

  it('secure remote on a NON-443 port keeps the explicit port', () => {
    const creds = { host: 'box.example', port: 8443, token: 't', secure: true }
    expect(daemonHttpBase(creds)).toBe('https://box.example:8443')
    expect(daemonWsBase(creds)).toBe('wss://box.example:8443')
  })

  it('local is never secure → byte-identical ws://127.0.0.1:port', () => {
    const creds = { host: '127.0.0.1', port: 47800, token: 't', secure: false }
    expect(daemonWsBase(creds)).toBe('ws://127.0.0.1:47800')
    expect(daemonHttpBase(creds)).toBe('http://127.0.0.1:47800')
  })
})

describe('getDaemonWs host-awareness', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    invokeMock.mockReset()
    invalidateDaemonWs()
  })

  it('local: invokes daemon_ws_url and carries host 127.0.0.1 (secure:false)', async () => {
    invokeMock.mockResolvedValue({ state: 'available', port: 47800, token: 'local-tok' })
    const creds = await getDaemonWs()
    expect(invokeMock).toHaveBeenCalledWith('daemon_ws_url')
    expect(creds).toEqual({ port: 47800, token: 'local-tok', host: '127.0.0.1', secure: false })
    // The resulting URL is byte-identical to the old hardcoded literal.
    expect(`${daemonHttpBase(creds)}/boot-status`).toBe('http://127.0.0.1:47800/boot-status')
  })

  it('remote (non-secure): derives creds from the active ConnectHost, no invoke', async () => {
    const host = {
      id: 'r1',
      label: 'Remote',
      hostname: '10.0.0.9',
      port: 51234,
      token: 'remote-tok',
      secure: false,
      remember: false,
      lastConnectedAt: null,
    }
    useConnectHostStore.getState().selectHost(host)
    // The assertion below is about getDaemonWs NOT invoking to resolve
    // creds for a remote host — so clear the mock after the switch to
    // isolate the creds-resolution path.
    invokeMock.mockClear()
    // Invalidate any cached local creds so we prove the remote path
    // doesn't fall back to invoke.
    invalidateDaemonWs()
    const creds = await getDaemonWs()
    expect(invokeMock).not.toHaveBeenCalled()
    expect(creds).toEqual({ port: 51234, token: 'remote-tok', host: '10.0.0.9', secure: false })
    expect(`${daemonWsBase(creds)}/cli/sessions/grid`).toBe(
      'ws://10.0.0.9:51234/cli/sessions/grid',
    )
  })

  it('remote (secure): carries secure:true and its OWN token; builds wss://', async () => {
    const host = {
      id: 'r2',
      label: 'Hosted',
      hostname: 'rosson.k2.dev',
      port: 443,
      token: 'hosted-tok',
      secure: true,
      remember: true,
      lastConnectedAt: null,
    }
    useConnectHostStore.getState().selectHost(host)
    // Ignore the set_active_daemon push selectHost makes; assert only that
    // creds resolution itself doesn't invoke.
    invokeMock.mockClear()
    invalidateDaemonWs()
    const creds = await getDaemonWs()
    expect(invokeMock).not.toHaveBeenCalled()
    // step #3: the remote's OWN token rides, not the local daemon's.
    expect(creds.token).toBe('hosted-tok')
    expect(creds.secure).toBe(true)
    // step #4: 443 omitted; wss scheme; token as ?token= over TLS.
    expect(`${daemonWsBase(creds)}/cli/sessions/grid?token=${creds.token}`).toBe(
      'wss://rosson.k2.dev/cli/sessions/grid?token=hosted-tok',
    )
  })

  it('switching back to local resumes the invoke path (secure:false)', async () => {
    const host = {
      id: 'r1',
      label: 'Remote',
      hostname: '10.0.0.9',
      port: 51234,
      token: 'remote-tok',
      secure: false,
      remember: false,
      lastConnectedAt: null,
    }
    useConnectHostStore.getState().selectHost(host)
    // Clear the set_active_daemon push so the remote-creds-resolution
    // assertion below stays about getDaemonWs only.
    invokeMock.mockClear()
    await getDaemonWs()
    expect(invokeMock).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost('local')
    invokeMock.mockResolvedValue({ state: 'available', port: 47800, token: 'local-tok' })
    const creds = await getDaemonWs()
    expect(invokeMock).toHaveBeenCalledWith('daemon_ws_url')
    expect(creds.host).toBe('127.0.0.1')
    expect(creds.secure).toBe(false)
  })
})
