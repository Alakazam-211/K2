import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { setConnectedAirgap, AIRGAP_TEACHING } from '@/lib/airgap'
import {
  COMPUTE_ENROLL_TICKET_URL,
  connectErrorMessage,
  ONE_CLICK_UNAVAILABLE,
  listAccountServers,
  listComputeMachines,
  machineOnline,
  ONE_CLICK_NEEDS_UPDATE,
  requestEnrollOffer,
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

  it('503 (route present, no signing key yet) falls back like 404', async () => {
    fetchMock.mockResolvedValue(respond(503, { error: 'supabase_disabled', detail: 'compute store is not configured on this relay' }))
    expect(await requestEnrollTicket('t', REQ)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_UNAVAILABLE })
  })

  it('a k2-connect {error, detail} body becomes a plain sentence', async () => {
    fetchMock.mockResolvedValue(respond(429, { error: 'rate_limited', detail: 'too many tickets; try again in a minute' }))
    expect(await requestEnrollTicket('t', REQ)).toEqual({
      ok: false,
      unavailable: false,
      message: 'Too many one-click requests. Wait a minute and try again',
    })
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

describe('listComputeMachines', () => {
  const ROW = { sub: 'z13', node_fp: 'AB'.repeat(32), node_name: 'z13', node_state: 'unenrolled', cpu_cores: 8, updated_at: '2026-10-11T00:00:00Z' }

  it('reads the table through PostgREST as the signed-in user', async () => {
    fetchMock.mockResolvedValue(respond(200, [ROW, { sub: 'bad' }]))
    const out = await listComputeMachines('acc')
    expect(out.ok).toBe(true)
    if (!out.ok) throw new Error('expected rows')
    expect(out.machines).toHaveLength(1)
    expect(out.machines[0].node_fp).toBe('ab'.repeat(32))
    expect(out.machines[0].cpu_cores).toBe(8)
    expect(out.machines[0].sas).toBeNull()
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toContain('/rest/v1/compute_machines?select=sub,node_fp,')
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer acc')
  })

  it('a missing table (404 / PGRST205) is the capability check', async () => {
    fetchMock.mockResolvedValueOnce(respond(404, { code: 'PGRST205', message: 'Could not find the table' }))
    expect(await listComputeMachines('acc')).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
    fetchMock.mockResolvedValueOnce(respond(400, { code: '42P01', message: 'relation does not exist' }))
    expect(await listComputeMachines('acc')).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
    expect(ONE_CLICK_NEEDS_UPDATE).toBe('One-click needs a K2 Connect update; meanwhile add a machine with an enroll code')
  })

  it('other errors are errors, not the capability notice', async () => {
    fetchMock.mockResolvedValueOnce(respond(401, { message: 'JWT expired' }))
    expect(await listComputeMachines('acc')).toEqual({ ok: false, unavailable: false, message: "Couldn't read your machines: JWT expired" })
  })

  it('never under air-gap', async () => {
    setConnectedAirgap(true)
    expect(await listComputeMachines('acc')).toEqual({ ok: false, unavailable: true, message: AIRGAP_TEACHING })
    expect(fetchMock).not.toHaveBeenCalled()
  })

  it('online = advertised in the last five minutes', () => {
    const now = Date.parse('2026-10-11T00:10:00Z')
    expect(machineOnline({ updated_at: '2026-10-11T00:06:00Z' }, now)).toBe(true)
    expect(machineOnline({ updated_at: '2026-10-11T00:04:00Z' }, now)).toBe(false)
    expect(machineOnline({ updated_at: null }, now)).toBe(false)
  })
})

describe('requestEnrollOffer', () => {
  const OFFER = { controllerSub: 'rosson', nodeSub: 'z13', nodeFp: 'ab'.repeat(32), nodeName: 'z13' }

  it('asks Connect to mint and deliver to the machine', async () => {
    fetchMock.mockResolvedValue(respond(200, { ticket: 't', exp: 1, delivered: true, offer_id: 'o1' }))
    expect(await requestEnrollOffer('acc', OFFER)).toEqual({ ok: true, ticket: 't' })
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(url).toBe(COMPUTE_ENROLL_TICKET_URL)
    expect(JSON.parse(String(init.body))).toEqual({ controller_sub: 'rosson', node_fp: 'ab'.repeat(32), node_name: 'z13', deliver_to_sub: 'z13' })
  })

  it('a route that minted without delivering is the needs-update case', async () => {
    fetchMock.mockResolvedValue(respond(200, { ticket: 't', exp: 1 }))
    expect(await requestEnrollOffer('acc', OFFER)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
  })

  it('404 and network failures are the needs-update case; 403 is an error', async () => {
    fetchMock.mockResolvedValueOnce(respond(404, ''))
    expect((await requestEnrollOffer('acc', OFFER)).ok).toBe(false)
    fetchMock.mockRejectedValueOnce(new TypeError('Failed to fetch'))
    expect(await requestEnrollOffer('acc', OFFER)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
    fetchMock.mockResolvedValueOnce(respond(403, { error: 'not_owner', detail: "you don't own that server" }))
    expect(await requestEnrollOffer('acc', OFFER)).toEqual({
      ok: false,
      unavailable: false,
      message: "That server isn't on your k2.dev account",
    })
  })

  it('503 (route dormant on this relay) is the needs-update case', async () => {
    fetchMock.mockResolvedValue(
      respond(503, { error: 'ticket_unavailable', detail: 'enroll tickets are not configured on this relay' }),
    )
    expect(await requestEnrollOffer('acc', OFFER)).toEqual({ ok: false, unavailable: true, message: ONE_CLICK_NEEDS_UPDATE })
  })

  it('409 machine_not_advertised reads as a sentence, not a code', async () => {
    fetchMock.mockResolvedValue(respond(409, { error: 'machine_not_advertised', detail: 'that machine hasn\'t checked in' }))
    const out = await requestEnrollOffer('acc', OFFER)
    expect(out.ok).toBe(false)
    if (out.ok) throw new Error('expected a refusal')
    expect(out.unavailable).toBe(false)
    expect(out.message).toMatch(/^That machine hasn't checked in with K2 Connect in the last 10 minutes/)
  })
})

describe('connectErrorMessage', () => {
  const cases: Array<[number, Record<string, unknown>, string]> = [
    [403, { error: 'not_owner', detail: "you don't own that server" }, "That server isn't on your k2.dev account"],
    [403, { error: 'unknown' }, "Connect doesn't know that server"],
    [429, { error: 'rate_limited', detail: 'too many tickets; try again in a minute' }, 'Too many one-click requests. Wait a minute and try again'],
    [401, { error: 'invalid_token', detail: 'sign in again' }, 'Your k2.dev sign-in has expired. Sign in again and retry'],
    [401, { error: 'unauthorized' }, 'Your k2.dev sign-in has expired. Sign in again and retry'],
    [503, { error: 'service_unavailable' }, 'K2 Connect is busy or being updated. Try again in a few minutes'],
    [502, { error: 'supabase_error', detail: 'store unavailable' }, "K2 Connect couldn't reach its account store. Try again in a few minutes"],
    [500, { error: 'internal', detail: 'could not mint' }, 'K2 Connect hit an internal error. Try again in a few minutes'],
    [400, { error: 'bad_request', detail: 'node_name is invalid' }, "Connect couldn't read the request: node_name is invalid"],
    [400, { error: 'invalid_node_fp' }, "Connect didn't accept the node fp"],
    [400, { error: 'invalid_controller_sub', detail: 'controller_sub must be a label' }, "Connect didn't accept the controller sub: controller_sub must be a label"],
  ]
  for (const [status, body, want] of cases) {
    it(`${status} ${String(body.error)} → plain sentence`, () => {
      expect(connectErrorMessage(status, body)).toBe(want)
    })
  }

  it('an unknown code shows Connect\'s detail (or message), never the bare code', () => {
    expect(connectErrorMessage(418, { error: 'teapot', detail: 'Connect is a teapot today' })).toBe('Connect is a teapot today')
    expect(connectErrorMessage(418, { error: 'teapot', message: 'short and stout' })).toBe('short and stout')
    expect(connectErrorMessage(418, { error: 'teapot' })).toBe('Connect refused the request (418, teapot)')
    expect(connectErrorMessage(418, {})).toBe('Connect refused the request (418)')
    expect(connectErrorMessage(418, {}, 'the enroll ticket')).toBe('Connect refused the enroll ticket (418)')
  })
})
