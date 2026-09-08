import { describe, it, expect } from 'vitest'
import type { ConnectHost } from '@/stores/connect-host'
import { loopbackForbiddenOnRemote } from './loopback-remote'

function remoteHost(): ConnectHost {
  return {
    id: 'h1',
    label: 'box',
    hostname: 'anna.k2.dev',
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

describe('loopbackForbiddenOnRemote', () => {
  it('allows loopback when activeHost is local', () => {
    expect(loopbackForbiddenOnRemote('local', 'http://127.0.0.1:1')).toBe(false)
    expect(loopbackForbiddenOnRemote('local', 'http://localhost:8788')).toBe(false)
    expect(loopbackForbiddenOnRemote('local', 'http://[::1]/')).toBe(false)
  })

  it('forbids loopback when activeHost is a Connect host', () => {
    const host = remoteHost()
    expect(loopbackForbiddenOnRemote(host, 'http://127.0.0.1:1')).toBe(true)
    expect(loopbackForbiddenOnRemote(host, 'http://localhost:8788/')).toBe(true)
    expect(loopbackForbiddenOnRemote(host, 'https://127.0.0.1')).toBe(true)
    expect(loopbackForbiddenOnRemote(host, 'http://[::1]:9/')).toBe(true)
  })

  it('allows non-loopback URLs on a remote host', () => {
    expect(loopbackForbiddenOnRemote(remoteHost(), 'https://example.com/')).toBe(false)
    expect(loopbackForbiddenOnRemote(remoteHost(), 'https://anna.k2.dev/')).toBe(false)
  })
})
