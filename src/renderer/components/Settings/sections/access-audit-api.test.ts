import { describe, it, expect, beforeEach, vi } from 'vitest'

const h = vi.hoisted(() => ({
  getDaemonWs: vi.fn(),
  daemonHttpBase: vi.fn(() => 'http://127.0.0.1:9'),
  invalidateDaemonWs: vi.fn(),
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: h.getDaemonWs,
  daemonHttpBase: h.daemonHttpBase,
  invalidateDaemonWs: h.invalidateDaemonWs,
}))

vi.mock('@/web/session-token', () => ({
  cliSearchParams: (
    token: string,
    params?: Record<string, string | number | boolean | undefined | null>,
  ) => {
    const search = new URLSearchParams()
    if (params) {
      for (const [k, v] of Object.entries(params)) {
        if (v !== undefined && v !== null) search.set(k, String(v))
      }
    }
    if (token) search.set('token', token)
    return search
  },
  withDaemonFetch: (init: RequestInit) => init,
}))

import {
  AUDIT_TAIL_DEFAULT,
  fetchUsersAudit,
  fetchWhoamiRole,
  isSuccessLogin,
  newestFirst,
  parseAuditEvents,
  type AuthAuditEvent,
} from './access-audit-api'

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

const OLDEST: AuthAuditEvent = {
  ts: '2026-09-16T13:55:02.000Z',
  event: 'login',
  user: 'root',
  outcome: 'blocked_ingress',
  ingress: 'tunnel',
  ip: '-',
}
const MIDDLE: AuthAuditEvent = {
  ts: '2026-09-16T14:01:58.000Z',
  event: 'login',
  user: 'alice',
  outcome: 'bad_creds',
  ingress: 'edge:k2-edge-app-2026-09',
  ip: '198.51.100.7',
}
const NEWEST: AuthAuditEvent = {
  ts: '2026-09-16T14:02:11.000Z',
  event: 'login',
  user: 'alice',
  outcome: 'ok',
  ingress: 'edge:k2-edge-app-2026-09',
  ip: '203.0.113.9',
}

beforeEach(() => {
  h.getDaemonWs.mockReset()
  h.invalidateDaemonWs.mockReset()
  h.daemonHttpBase.mockReturnValue('http://127.0.0.1:9')
  h.getDaemonWs.mockResolvedValue({
    port: 9,
    token: 'owner-tok',
    host: '127.0.0.1',
    secure: false,
  })
  vi.unstubAllGlobals()
})

describe('parse / display helpers', () => {
  it('parseAuditEvents reads events[] and drops client/path', () => {
    const parsed = parseAuditEvents({
      events: [
        { ...NEWEST, client: 'web', extra: 1 },
        { notAnObject: true },
        null,
      ],
      path: '/home/rosson/.k2/auth-audit.jsonl',
      tail: 200,
    })
    expect(parsed).toEqual([expect.objectContaining({ user: 'alice', outcome: 'ok' })])
    expect(JSON.stringify(parsed)).not.toContain('web')
    expect(JSON.stringify(parsed)).not.toContain('auth-audit.jsonl')
  })

  it('newestFirst reverses daemon oldest-first order', () => {
    expect(newestFirst([OLDEST, MIDDLE, NEWEST]).map((e) => e.ts)).toEqual([
      NEWEST.ts,
      MIDDLE.ts,
      OLDEST.ts,
    ])
  })

  it('isSuccessLogin is login+ok only', () => {
    expect(isSuccessLogin(NEWEST)).toBe(true)
    expect(isSuccessLogin(MIDDLE)).toBe(false)
    expect(isSuccessLogin({ ...NEWEST, event: 'logout' })).toBe(false)
  })

  it('default tail is 200', () => {
    expect(AUDIT_TAIL_DEFAULT).toBe(200)
  })
})

describe('fetchUsersAudit', () => {
  it('GETs /cli/users/audit?tail= and returns events as the daemon sent them', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse(200, { events: [OLDEST, NEWEST], tail: 200, path: '/x' }),
    )
    vi.stubGlobal('fetch', fetchMock)

    const result = await fetchUsersAudit({ tail: 200 })
    expect(result).toEqual({
      kind: 'ok',
      events: [OLDEST, NEWEST],
      tail: 200,
    })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const url = String(fetchMock.mock.calls[0]?.[0])
    expect(url).toContain('/cli/users/audit')
    expect(url).toContain('tail=200')
    expect(url).toContain('token=owner-tok')
    expect(fetchMock.mock.calls[0]?.[1]).toMatchObject({ method: 'GET' })
    expect(h.invalidateDaemonWs).not.toHaveBeenCalled()
  })

  it('403 is terminal and does not invalidateDaemonWs', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('forbidden', { status: 403 }))
    vi.stubGlobal('fetch', fetchMock)

    const result = await fetchUsersAudit({ tail: 200 })
    expect(result).toEqual({ kind: 'forbidden' })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(h.invalidateDaemonWs).not.toHaveBeenCalled()
  })

  it('404 is terminal (old host) with no retry', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response('not found', { status: 404 }))
    vi.stubGlobal('fetch', fetchMock)

    const result = await fetchUsersAudit({ tail: 50 })
    expect(result).toEqual({ kind: 'not-found' })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(h.invalidateDaemonWs).not.toHaveBeenCalled()
  })

  it('401 retries once after invalidateDaemonWs; second 401 is unauthorized', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response('auth', { status: 401 }))
      .mockResolvedValueOnce(new Response('auth', { status: 401 }))
    vi.stubGlobal('fetch', fetchMock)

    const result = await fetchUsersAudit({ tail: 200 })
    expect(result).toEqual({ kind: 'unauthorized' })
    expect(h.invalidateDaemonWs).toHaveBeenCalledTimes(1)
    expect(fetchMock).toHaveBeenCalledTimes(2)
  })

  it('empty events[] is ok, not an error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse(200, { events: [], tail: 200 })))
    const result = await fetchUsersAudit({ tail: 200 })
    expect(result).toEqual({ kind: 'ok', events: [], tail: 200 })
  })
})

describe('fetchWhoamiRole', () => {
  it('reads owner from whoami', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(jsonResponse(200, { role: 'owner', owner: true })),
    )
    expect(await fetchWhoamiRole()).toBe('owner')
  })

  it('403 does not invalidateDaemonWs', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('no', { status: 403 })))
    expect(await fetchWhoamiRole()).toBeNull()
    expect(h.invalidateDaemonWs).not.toHaveBeenCalled()
  })
})
