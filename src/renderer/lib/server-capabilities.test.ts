// Unit tests for the remote capability layer (#638).
//
// Coverage:
//   - gte() semver edge cases (equal, off-by-one in each component,
//     missing patch, pre-release/build suffix stripping, leading 'v')
//   - serverSupports():
//       * always true for the LOCAL host (regardless of version)
//       * false for a remote with UNKNOWN (null) version
//       * exact-min / below-min / above-min gating for 'fs-info' & 'roles'
//   - featureMinVersion() returns the mapped string
//
// We drive the REAL connect-host store via setState (it needs a
// localStorage stub in node), so the store wiring is exercised, not mocked.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// localStorage stub so the connect-host store loads in node.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import {
  gte,
  serverSupports,
  featureMinVersion,
  FEATURES,
} from './server-capabilities'

function remoteHost(): ConnectHost {
  return {
    id: 'h1',
    label: 'Box',
    hostname: 'box.k2.dev',
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

/** Put the store into "active remote with version V" (or null). */
function setActiveRemote(version: string | null): void {
  useConnectHostStore.setState({
    activeHost: remoteHost(),
    serverVersion: version,
  })
}

beforeEach(() => {
  // Reset to the local default before each test.
  useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
})

describe('gte (semver)', () => {
  it('is true for equal versions', () => {
    expect(gte('0.39.24', '0.39.24')).toBe(true)
    expect(gte('1.0.0', '1.0.0')).toBe(true)
  })

  it('compares major, then minor, then patch', () => {
    expect(gte('1.0.0', '0.99.99')).toBe(true)
    expect(gte('0.99.99', '1.0.0')).toBe(false)
    expect(gte('0.40.0', '0.39.99')).toBe(true)
    expect(gte('0.39.99', '0.40.0')).toBe(false)
    expect(gte('0.39.25', '0.39.24')).toBe(true)
    expect(gte('0.39.23', '0.39.24')).toBe(false)
  })

  it('treats a missing patch as 0', () => {
    expect(gte('0.39', '0.39.0')).toBe(true)
    expect(gte('0.39', '0.39.1')).toBe(false)
  })

  it('ignores pre-release / build suffixes and a leading v', () => {
    expect(gte('0.39.24-rc.1', '0.39.24')).toBe(true)
    expect(gte('0.39.24+build7', '0.39.24')).toBe(true)
    expect(gte('v0.39.24', '0.39.24')).toBe(true)
    expect(gte('0.39.23-rc.9', '0.39.24')).toBe(false)
  })
})

describe('featureMinVersion', () => {
  it('returns the mapped minimum version', () => {
    expect(featureMinVersion('fs-info')).toBe('0.39.24')
    expect(featureMinVersion('roles')).toBe('0.39.23')
    expect(featureMinVersion('fs-info')).toBe(FEATURES['fs-info'])
  })
})

describe('serverSupports — local host', () => {
  it('is always true for local, even with a null version', () => {
    useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
    expect(serverSupports('fs-info')).toBe(true)
    expect(serverSupports('roles')).toBe(true)
  })

  it('is true for local even if a stale older version lingers', () => {
    useConnectHostStore.setState({ activeHost: 'local', serverVersion: '0.1.0' })
    expect(serverSupports('fs-info')).toBe(true)
  })
})

describe('serverSupports — remote, unknown version', () => {
  it('returns false for gated features when version is null', () => {
    setActiveRemote(null)
    expect(serverSupports('fs-info')).toBe(false)
    expect(serverSupports('roles')).toBe(false)
  })
})

describe('serverSupports — remote, fs-info gating (min 0.39.24)', () => {
  it('false below min', () => {
    setActiveRemote('0.39.23')
    expect(serverSupports('fs-info')).toBe(false)
  })
  it('true at exactly min', () => {
    setActiveRemote('0.39.24')
    expect(serverSupports('fs-info')).toBe(true)
  })
  it('true above min', () => {
    setActiveRemote('0.40.0')
    expect(serverSupports('fs-info')).toBe(true)
  })
})

describe('serverSupports — remote, daemon-restart gating (min 0.39.32)', () => {
  it('false below min (older remote hides the Restart-host button)', () => {
    setActiveRemote('0.39.31')
    expect(serverSupports('daemon-restart')).toBe(false)
  })
  it('true at exactly min', () => {
    setActiveRemote('0.39.32')
    expect(serverSupports('daemon-restart')).toBe(true)
  })
  it('true above min', () => {
    setActiveRemote('0.40.0')
    expect(serverSupports('daemon-restart')).toBe(true)
  })
  it('always true for the local Mac (paired with this app)', () => {
    useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
    expect(serverSupports('daemon-restart')).toBe(true)
  })
})

describe('serverSupports — remote, remote-update gating (min 0.39.33)', () => {
  it('false below min (older remote hides the Update-host control)', () => {
    setActiveRemote('0.39.32')
    expect(serverSupports('remote-update')).toBe(false)
  })
  it('true at exactly min', () => {
    setActiveRemote('0.39.33')
    expect(serverSupports('remote-update')).toBe(true)
  })
  it('true above min', () => {
    setActiveRemote('0.40.0')
    expect(serverSupports('remote-update')).toBe(true)
  })
  it('always true for the local Mac (the Tauri auto-updater owns local)', () => {
    useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
    expect(serverSupports('remote-update')).toBe(true)
  })
})

describe('serverSupports — remote, daemon-broadcasts gating (min 0.39.39)', () => {
  it('maps to 0.39.39', () => {
    expect(featureMinVersion('daemon-broadcasts')).toBe('0.39.39')
    expect(FEATURES['daemon-broadcasts']).toBe('0.39.39')
  })
  it('false below min (older remote keeps its renderer polling fallback)', () => {
    setActiveRemote('0.39.38')
    expect(serverSupports('daemon-broadcasts')).toBe(false)
  })
  it('true at exactly min', () => {
    setActiveRemote('0.39.39')
    expect(serverSupports('daemon-broadcasts')).toBe(true)
  })
  it('true above min', () => {
    setActiveRemote('0.40.0')
    expect(serverSupports('daemon-broadcasts')).toBe(true)
  })
  it('always true for the local Mac (byte-paired with this app)', () => {
    useConnectHostStore.setState({ activeHost: 'local', serverVersion: null })
    expect(serverSupports('daemon-broadcasts')).toBe(true)
  })
})

describe('serverSupports — remote, roles gating (min 0.39.23)', () => {
  it('false below min', () => {
    setActiveRemote('0.39.22')
    expect(serverSupports('roles')).toBe(false)
  })
  it('true at exactly min', () => {
    setActiveRemote('0.39.23')
    expect(serverSupports('roles')).toBe(true)
  })
  it('true above min', () => {
    setActiveRemote('0.39.24')
    expect(serverSupports('roles')).toBe(true)
  })
})
