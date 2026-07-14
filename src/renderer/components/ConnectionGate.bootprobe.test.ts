// 0.40.48 — unit tests for the DISCRIMINATED boot probe.
//
// The old fetchBootStatus returned null for BOTH a non-2xx response and a
// network error, which made the two failure modes indistinguishable. The
// wedge detector keys on exactly that distinction:
//   - {kind:'http'}    → transport worked, HTTP failed — the poisoned-pool
//                        signature when sustained (tunnel-edge 404s riding
//                        a healthy pooled connection),
//   - {kind:'network'} → fetch threw — the ordinary down/mid-restart signal.
// Policy call sites fold non-'ok' back to null, so this file pins ONLY the
// discrimination itself (plus the creds invalidation both paths must keep).

import { describe, it, expect, beforeEach, vi } from 'vitest'

const { getDaemonWsMock, invalidateDaemonWsMock } = vi.hoisted(() => ({
  getDaemonWsMock: vi.fn(),
  invalidateDaemonWsMock: vi.fn(),
}))

// Keep the REAL daemonHttpBase (URL shape must not drift) — only the creds
// resolver + invalidation are scripted.
vi.mock('@/kessel/daemon-ws', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/kessel/daemon-ws')>()
  return {
    ...actual,
    getDaemonWs: (...a: unknown[]) => getDaemonWsMock(...a),
    invalidateDaemonWs: (...a: unknown[]) => invalidateDaemonWsMock(...a),
  }
})

import { fetchBootStatus } from './ConnectionGate'

const CREDS = { port: 443, token: 'tok', host: 'rosson.k2.dev', secure: true }

const READY = { version: '0.40.48', protocol: 1, phase: 'ready', detail: '' }

describe('fetchBootStatus — discriminated boot probe', () => {
  beforeEach(() => {
    getDaemonWsMock.mockReset()
    invalidateDaemonWsMock.mockReset()
    getDaemonWsMock.mockResolvedValue(CREDS)
  })

  it("2xx with a parseable body → {kind:'ok'} carrying the status (instanceId included when present)", async () => {
    const body = { ...READY, instanceId: 'abcd1234abcd1234' }
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      expect(url).toBe('https://rosson.k2.dev/boot-status')
      return { ok: true, status: 200, json: async () => body } as unknown as Response
    }))

    await expect(fetchBootStatus()).resolves.toEqual({ kind: 'ok', status: body })
    expect(invalidateDaemonWsMock).not.toHaveBeenCalled()
  })

  it("a non-2xx response → {kind:'http'} with the status code, creds invalidated", async () => {
    vi.stubGlobal('fetch', vi.fn(async () =>
      ({ ok: false, status: 404, json: async () => ({}) }) as unknown as Response,
    ))

    await expect(fetchBootStatus()).resolves.toEqual({ kind: 'http', httpStatus: 404 })
    expect(invalidateDaemonWsMock).toHaveBeenCalledTimes(1)
  })

  it("a thrown fetch (refused / timeout / dead socket) → {kind:'network'}, creds invalidated", async () => {
    vi.stubGlobal('fetch', vi.fn(async () => {
      throw new TypeError('Load failed')
    }))

    await expect(fetchBootStatus()).resolves.toEqual({ kind: 'network' })
    expect(invalidateDaemonWsMock).toHaveBeenCalledTimes(1)
  })

  it("creds unresolvable (getDaemonWs rejects) → {kind:'network'} (daemon not reachable yet)", async () => {
    getDaemonWsMock.mockRejectedValue(new Error('daemon not reachable: no port file'))
    vi.stubGlobal('fetch', vi.fn())

    await expect(fetchBootStatus()).resolves.toEqual({ kind: 'network' })
    expect(invalidateDaemonWsMock).toHaveBeenCalledTimes(1)
  })

  it("an unparseable 2xx body folds to {kind:'network'} (matches the old null behavior — never a wedge signal)", async () => {
    vi.stubGlobal('fetch', vi.fn(async () =>
      ({ ok: true, status: 200, json: async () => { throw new SyntaxError('bad json') } }) as unknown as Response,
    ))

    await expect(fetchBootStatus()).resolves.toEqual({ kind: 'network' })
  })
})
