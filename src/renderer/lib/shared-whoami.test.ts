import { describe, it, expect, vi, beforeEach } from 'vitest'

const { getMock } = vi.hoisted(() => ({ getMock: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: (...a: unknown[]) => getMock(...a) }))

import { loadWhoami, resetSharedWhoamiForTests, WHOAMI_TTL_MS } from './shared-whoami'
import type { ServerScope } from '@/kessel/server-scope'

function scope(key: string, token = 'tok'): ServerScope {
  return {
    connectionKey: `host:${key}`,
    hostKey: key,
    isRemote: true,
    connectHost: () => ({ username: 'rosson', token }),
  } as unknown as ServerScope
}

beforeEach(() => {
  resetSharedWhoamiForTests()
  getMock.mockReset()
})

describe('shared whoami', () => {
  it('one request per login: concurrent and later callers share it', async () => {
    getMock.mockResolvedValue({ owner: true, role: 'owner' })
    const s = scope('rosson.k2.dev')
    const [a, b] = await Promise.all([loadWhoami(s), loadWhoami(s)])
    expect(a).toEqual({ owner: true, role: 'owner' })
    expect(b).toBe(a)
    expect(await loadWhoami(scope('rosson.k2.dev'))).toBe(a)
    expect(getMock).toHaveBeenCalledTimes(1)
    expect(getMock).toHaveBeenCalledWith(s, 'auth/whoami')
  })

  it('another server, a new login token, or the window passing asks again', async () => {
    let now = 1_000
    getMock.mockResolvedValue({ owner: false, role: 'member' })
    await loadWhoami(scope('a.k2.dev'), () => now)
    await loadWhoami(scope('b.k2.dev'), () => now)
    await loadWhoami(scope('a.k2.dev', 'tok-2'), () => now)
    expect(getMock).toHaveBeenCalledTimes(3)
    now += WHOAMI_TTL_MS - 1
    await loadWhoami(scope('a.k2.dev'), () => now)
    expect(getMock).toHaveBeenCalledTimes(3)
    now += 2
    await loadWhoami(scope('a.k2.dev'), () => now)
    expect(getMock).toHaveBeenCalledTimes(4)
  })

  it('a failure is thrown and not remembered', async () => {
    getMock.mockRejectedValueOnce(new Error('down'))
    await expect(loadWhoami(scope('a.k2.dev'))).rejects.toThrow('down')
    getMock.mockResolvedValueOnce({ owner: true })
    await expect(loadWhoami(scope('a.k2.dev'))).resolves.toEqual({ owner: true })
    expect(getMock).toHaveBeenCalledTimes(2)
  })
})
