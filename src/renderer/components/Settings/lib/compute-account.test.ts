import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setConnectedAirgap, AIRGAP_TEACHING } from '@/lib/airgap'
import {
  COMPUTE_ENROLL_TICKET_URL,
  ONE_CLICK_UNAVAILABLE,
  listAccountServers,
  requestEnrollTicket,
} from './compute-account'

const REQ = { controllerSub: 'rosson', nodeFp: 'ab'.repeat(32), nodeName: 'mini-1' }

function respond(status: number, body: unknown): Response {
  return new Response(typeof body === 'string' ? body : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })
}

let fetchMock: ReturnType<typeof vi.fn>

beforeEach(() => {
  setConnectedAirgap(false)
  fetchMock = vi.fn()
  vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
  setConnectedAirgap(false)
  vi.unstubAllGlobals()
})

describe('requestEnrollTicket', () => {
  it('posts the pairing to Connect with the bearer token and returns the ticket', async () => {
    fetchMock.mockResolvedValue(respond(200, { ticket: 'tkt_123' }))
    const out = await requestEnrollTicket('acc-token', REQ)
    expect(out).toEqual({ ok: true, ticket: 'tkt_123' })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe(COMPUTE_ENROLL_TICKET_URL)
    expect(url).toBe('https://connect.k2.dev/compute/enroll-ticket')
    expect(init.method).toBe('POST')
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer acc-token')
    expect(JSON.parse(String(init.body))).toEqual({
      controller_sub: 'rosson',
      node_fp: 'ab'.repeat(32),
      node_name: 'mini-1',
    })
  })

  it('404 means the route is not on this control plane yet', async () => {
    fetchMock.mockResolvedValue(respond(404, 'Not Found'))
    expect(await requestEnrollTicket('t', REQ)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE })
    expect(ONE_CLICK_UNAVAILABLE).toBe("One-click isn't available on this account yet; use an enroll code")
  })

  it('405 and a network failure fall back the same way', async () => {
    fetchMock.mockResolvedValueOnce(respond(405, ''))
    expect(await requestEnrollTicket('t', REQ)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE })
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'))
    expect(await requestEnrollTicket('t', REQ)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE })
  })

  it('a real refusal shows its hint and does not fall back', async () => {
    fetchMock.mockResolvedValue(respond(403, { error: { code: 'not_yours', hint: 'that server is on another account' } }))
    expect(await requestEnrollTicket('t', REQ)).toEqual({
      ok: false,
      unavailable: false,
      message: 'that server is on another account',
    })
  })

  it('a 200 without a ticket is treated as unavailable', async () => {
    fetchMock.mockResolvedValue(respond(200, {}))
    expect(await requestEnrollTicket('t', REQ)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE })
  })

  it('never calls Connect under air-gap', async () => {
    setConnectedAirgap(true)
    expect(await requestEnrollTicket('t', REQ)).toEqual({ ok: false, unavailable: true, message: AIRGAP_TEACHING })
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe('listAccountServers', () => {
  it('keeps active apex servers only', async () => {
    fetchMock.mockResolvedValue(
      respond(200, [
        { label: 'rosson', status: 'active', tunnel_token: 'x' },
        { label: 'later', status: 'pending', tunnel_token: 'y' },
        { label: 'api.rosson', status: 'active', tunnel_token: 'z' },
      ]),
    )
    const rows = await listAccountServers('acc')
    expect(rows.map((r) => r.label)).toEqual(['rosson'])
  })
})
