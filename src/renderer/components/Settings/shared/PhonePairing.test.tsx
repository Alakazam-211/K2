// @vitest-environment jsdom
// Phone pairing (phone-web-off): the shared panel for Settings → K2
// Companion (`mode="all"`) and the profile modal (`mode="mine"`). It renders
// only what the daemon returns: who a code pairs as, the owner's choices,
// the password need, the code with its QR (from the daemon's link) for its
// 10 minutes, the phones with Revoke, and the daemon's refusal hints.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { PairCodeChangedEvent } from '@/stores/session-events'

// 0.46.6 pair success: the panel's `pair_code_changed` subscription,
// captured so a test can deliver a frame.
const pairHandlers = vi.hoisted(() => new Set<(e: PairCodeChangedEvent) => void>())
vi.mock('@/stores/session-events', () => ({
  onPairCodeChanged: (_scope: unknown, fn: (e: PairCodeChangedEvent) => void) => {
    pairHandlers.add(fn)
    return () => void pairHandlers.delete(fn)
  },
}))

function frame(e: Omit<PairCodeChangedEvent, 'kind'>): void {
  expect(pairHandlers.size, 'the panel subscribes to pair_code_changed').toBeGreaterThan(0)
  act(() => {
    for (const fn of pairHandlers) fn({ kind: 'pair_code_changed', ...e })
  })
}

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1234, token: 'owner-tok' })),
  invalidateDaemonWs: vi.fn(),
  daemonHttpBase: () => 'http://127.0.0.1:1234',
}))

import { PhonePairing } from './PhonePairing'
import { parseDevices, parsePairCode, parsePairCodeStatus, secondsLeft } from './phone-pairing-api'
import type { ServerScope } from '@/kessel/server-scope'

const SCOPE = { label: 'This computer', isRemote: false } as unknown as ServerScope

type Handler = (url: string, init?: RequestInit) => { status: number; body: unknown }
let handler: Handler
const calls: Array<{ url: string; init?: RequestInit }> = []

beforeEach(() => {
  calls.length = 0
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, init })
      const { status, body } = handler(url, init)
      return new Response(JSON.stringify(body), { status })
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

const PHONE = {
  id: 'sess-1',
  username: 'rosson',
  deviceId: 'ios-1',
  name: '<img src=x onerror=alert(1)>',
  createdAt: '2026-10-24T10:00:00Z',
  expiresAt: '2026-11-23T10:00:00Z',
  maxExpiresAt: '2027-04-22T10:00:00Z',
  pairedBy: 'owner-token',
  current: false,
}

const CODE = {
  ok: true,
  code: 'K2P-AAAAA-BBBBB-CCCCC-DDDDD',
  user: 'rosson',
  expiresAt: '2026-10-24T10:10:00Z',
  expiresInSecs: 600,
  host: 'rosson.k2.dev',
  pairUrl: 'https://rosson.k2.dev/pair#K2P-AAAAA-BBBBB-CCCCC-DDDDD',
  qr: { size: 2, rows: ['10', '01'] },
}

function body(init?: RequestInit): unknown {
  return JSON.parse(String(init?.body ?? '{}'))
}

describe('phone-pairing-api parsers', () => {
  it('reads devices, pairAs and the owner choices, tolerating junk', () => {
    const v = parseDevices({
      ok: true,
      devices: [PHONE, null, 3],
      pairAs: { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 7, 'rosson'] },
    })
    expect(v.devices).toHaveLength(1)
    expect(v.pairAs).toEqual({ user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 'rosson'], error: null })
    const none = parseDevices({ devices: [], pairAs: { user: null, error: { code: 'owner_login_not_linked', hint: 'Link one' } } })
    expect(none.pairAs.user).toBeNull()
    expect(none.pairAs.choices).toBeNull()
    expect(none.pairAs.error).toEqual({ code: 'owner_login_not_linked', hint: 'Link one' })
  })

  it('reads a code and counts down to its expiry, never below zero', () => {
    expect(parsePairCode(CODE).pairUrl).toBe(CODE.pairUrl)
    const t = Date.parse('2026-10-24T10:00:00Z')
    expect(secondsLeft('2026-10-24T10:10:00Z', t)).toBe(600)
    expect(secondsLeft('2026-10-24T09:00:00Z', t)).toBe(0)
    expect(secondsLeft('garbage', t)).toBe(0)
  })
})

describe('PhonePairing in Settings → K2 Companion (mode="all")', () => {
  it('pairs as the linked login by default, shows the code + QR, and drops it at expiry', async () => {
    let now = Date.parse('2026-10-24T10:00:00Z')
    handler = (url) => {
      if (url.includes('/cli/auth/devices')) {
        expect(url).not.toContain('mine=')
        return { status: 200, body: { ok: true, devices: [], pairAs: { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 'rosson'] } } }
      }
      if (url.includes('/cli/auth/device/pair-code')) return { status: 200, body: CODE }
      throw new Error(`unexpected ${url}`)
    }
    render(<PhonePairing scope={SCOPE} mode="all" nowMs={() => now} />)
    const select = (await screen.findByLabelText('Pairs as')) as HTMLSelectElement
    expect(select.value).toBe('rosson')
    expect(screen.getByText('rosson (your linked login)')).toBeTruthy()
    expect(screen.queryByLabelText('Your password')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Pair a phone' }))
    await waitFor(() => expect(document.querySelector('[data-pair-code-text]')?.textContent).toBe(CODE.code))
    expect(body(calls.find((c) => c.url.includes('pair-code'))?.init)).toEqual({})
    const qr = document.querySelector('[data-pair-qr] svg')
    expect(qr, 'QR drawn from the daemon link').toBeTruthy()
    expect(screen.getByText(CODE.pairUrl)).toBeTruthy()
    expect(document.querySelector('[data-pair-countdown]')?.textContent).toBe('Works once, for 10:00')
    now += 10 * 60 * 1000
    await waitFor(() => expect(document.querySelector('[data-pair-code]')).toBeNull(), { timeout: 3000 })
    // 0.46.6: the countdown running out says so instead of just vanishing.
    expect(document.querySelector('[data-pair-expired="expired"]')?.textContent).toContain('expired before a phone used it')
    // An older server's code has no codeId: nothing polls a status route.
    expect(calls.some((c) => c.url.includes('pair-code/status'))).toBe(false)
  })

  it('lets the owner token pick another login (sent as user)', async () => {
    handler = (url) => {
      if (url.includes('/cli/auth/devices')) {
        return { status: 200, body: { ok: true, devices: [], pairAs: { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 'rosson'] } } }
      }
      return { status: 200, body: { ...CODE, user: 'ada' } }
    }
    render(<PhonePairing scope={SCOPE} mode="all" />)
    fireEvent.change(await screen.findByLabelText('Pairs as'), { target: { value: 'ada' } })
    fireEvent.click(screen.getByRole('button', { name: 'Pair a phone' }))
    await waitFor(() => expect(document.querySelector('[data-pair-code]')?.textContent).toContain('Pairs as ada'))
    expect(body(calls.find((c) => c.url.includes('pair-code'))?.init)).toEqual({ user: 'ada' })
  })

  it('with no linked login shows the daemon hint and waits for a pick', async () => {
    handler = (url) => {
      if (url.includes('/cli/auth/devices')) {
        return {
          status: 200,
          body: { ok: true, devices: [], pairAs: { user: null, needsPassword: false, choices: ['ada'], error: { code: 'owner_login_not_linked', hint: 'Link the Connect login you use (Settings → General → Your account)' } } },
        }
      }
      throw new Error(`unexpected ${url}`)
    }
    render(<PhonePairing scope={SCOPE} mode="all" />)
    await waitFor(() => expect(document.querySelector('[data-pair-as-missing]')?.textContent).toContain('Your account'))
    expect((screen.getByRole('button', { name: 'Pair a phone' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('lists every phone as text and revokes one', async () => {
    let devices = [PHONE]
    handler = (url, init) => {
      if (url.includes('/cli/auth/devices')) {
        return { status: 200, body: { ok: true, devices, pairAs: { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['rosson'] } } }
      }
      if (url.includes('/cli/auth/device/revoke')) {
        expect(body(init)).toEqual({ id: 'sess-1' })
        devices = []
        return { status: 200, body: { ok: true, revoked: 'sess-1', username: 'rosson' } }
      }
      throw new Error(`unexpected ${url}`)
    }
    render(<PhonePairing scope={SCOPE} mode="all" />)
    const row = await waitFor(() => {
      const el = document.querySelector('[data-pair-device="sess-1"]')
      if (!el) throw new Error('no row yet')
      return el as HTMLElement
    })
    expect(row.textContent).toContain('<img src=x onerror=alert(1)>')
    expect(row.textContent).toContain('rosson · paired')
    expect(row.querySelector('img')).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Revoke' }))
    await waitFor(() => expect(screen.getByText('No paired phones.')).toBeTruthy())
  })
})

describe('PhonePairing in the profile modal (mode="mine")', () => {
  it('a Connect login types its password first; a wrong one shows the daemon hint', async () => {
    handler = (url, init) => {
      if (url.includes('/cli/auth/devices')) {
        expect(url).toContain('mine=1')
        return { status: 200, body: { ok: true, devices: [PHONE], pairAs: { user: 'ada', source: 'self', needsPassword: true } } }
      }
      if (url.includes('pair-code')) {
        expect(body(init)).toEqual({ password: 'hunter22' })
        return { status: 401, body: { ok: false, error: { code: 'password_invalid', hint: 'that password is wrong' } } }
      }
      throw new Error(`unexpected ${url}`)
    }
    render(<PhonePairing scope={SCOPE} mode="mine" />)
    const pw = await screen.findByLabelText('Your password')
    expect(screen.queryByLabelText('Pairs as')).toBeNull()
    expect(document.querySelector('[data-pair-as]')?.textContent).toContain('A new code pairs as ada.')
    const btn = screen.getByRole('button', { name: 'Pair a phone' }) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
    fireEvent.change(pw, { target: { value: 'hunter22' } })
    fireEvent.click(btn)
    await waitFor(() => expect(document.querySelector('[data-pair-code-error]')?.textContent).toBe('that password is wrong'))
    expect(document.querySelector('[data-pair-code]')).toBeNull()
    // A 401 password_invalid is not retried (it would count as a second
    // failed password).
    expect(calls.filter((c) => c.url.includes('pair-code'))).toHaveLength(1)
    // Own phones only: no login column.
    expect(document.querySelector('[data-pair-device="sess-1"]')?.textContent).not.toContain('rosson ·')
  })

  it('the owner key on this computer pairs without a password', async () => {
    handler = (url) => {
      if (url.includes('/cli/auth/devices')) {
        return { status: 200, body: { ok: true, devices: [], pairAs: { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 'rosson'] } } }
      }
      return { status: 200, body: CODE }
    }
    render(<PhonePairing scope={SCOPE} mode="mine" />)
    await waitFor(() => expect(document.querySelector('[data-pair-as]')?.textContent).toContain('A new code pairs as rosson.'))
    expect(screen.queryByLabelText('Pairs as'), 'no picker in your own profile').toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Pair a phone' }))
    await waitFor(() => expect(document.querySelector('[data-pair-code-text]')?.textContent).toBe(CODE.code))
    expect(body(calls.find((c) => c.url.includes('pair-code'))?.init)).toEqual({})
  })

  it('an unreachable server is an answer, not a crash', async () => {
    handler = () => {
      throw new Error('connection refused')
    }
    render(<PhonePairing scope={SCOPE} mode="mine" />)
    await waitFor(() => expect(document.querySelector('[data-pair-error]')?.textContent).toContain('connection refused'))
  })
})

// ── 0.46.6 pair success ────────────────────────────────────────────────

const CODE_ID = 'pc_0123456789abcdef01234567'
const LIVE_CODE = { ...CODE, codeId: CODE_ID }
const PAIRS_AS = { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 'rosson'] }
const NEW_PHONE = { ...PHONE, id: 'sess-new', name: 'iPhone', deviceId: 'ios-new' }
/** A clock inside CODE's ten minutes. */
const NOW = (): number => Date.parse('2026-10-24T10:00:00Z')

/** A server whose phones list gains `NEW_PHONE` once `paired()` is called. */
function server(opts: { status?: () => unknown } = {}): { paired: () => void; mints: () => number } {
  let isPaired = false
  let mints = 0
  handler = (url) => {
    if (url.includes('/cli/auth/device/pair-code/status')) {
      if (!opts.status) throw new Error('status polled without a stub')
      return { status: 200, body: opts.status() }
    }
    if (url.includes('/cli/auth/devices')) {
      return { status: 200, body: { ok: true, devices: isPaired ? [NEW_PHONE] : [], pairAs: PAIRS_AS } }
    }
    if (url.includes('/cli/auth/device/pair-code')) {
      mints += 1
      return { status: 200, body: { ...LIVE_CODE, codeId: `${CODE_ID.slice(0, -1)}${mints}` } }
    }
    throw new Error(`unexpected ${url}`)
  }
  return { paired: () => void (isPaired = true), mints: () => mints }
}

async function showCode(): Promise<string> {
  fireEvent.click(await screen.findByRole('button', { name: 'Pair a phone' }))
  await waitFor(() => expect(document.querySelector('[data-pair-code-text]')?.textContent).toBe(CODE.code))
  return `${CODE_ID.slice(0, -1)}1`
}

describe('PhonePairing success state (0.46.6)', () => {
  it('reads a code id and a status, tolerating junk', () => {
    expect(parsePairCode(LIVE_CODE).codeId).toBe(CODE_ID)
    expect(parsePairCode(CODE).codeId, 'an older server sends none').toBeNull()
    expect(parsePairCodeStatus({ codeId: 'pc_1', state: 'paired', user: 'a', deviceName: 'iPad', sessionId: 's' })).toEqual({
      codeId: 'pc_1',
      state: 'paired',
      user: 'a',
      deviceName: 'iPad',
      sessionId: 's',
    })
    expect(parsePairCodeStatus({ codeId: 'pc_1', state: 'exploded' })).toBeNull()
    expect(parsePairCodeStatus(null)).toBeNull()
  })

  it('a redeem frame swaps the QR for "Paired <device> as <user>" and the phone appears highlighted', async () => {
    const s = server()
    render(<PhonePairing scope={SCOPE} mode="all" nowMs={NOW} />)
    const id = await showCode()
    s.paired()
    frame({ codeId: id, state: 'paired', user: 'rosson', deviceName: 'iPhone', sessionId: 'sess-new' })
    expect(document.querySelector('[data-pair-code]'), 'the QR is gone').toBeNull()
    expect(document.querySelector('[data-pair-success-text]')?.textContent).toBe('Paired iPhone as rosson')
    // At once, before any refetch answers: the row is there and highlighted.
    const row = document.querySelector('[data-pair-device="sess-new"]')
    expect(row?.hasAttribute('data-pair-new')).toBe(true)
    expect(row?.textContent).toContain('iPhone')
    expect(row?.textContent).toContain('just paired')
    // The refetch keeps it (one row, not two).
    await waitFor(() => expect(calls.filter((c) => c.url.includes('/cli/auth/devices')).length).toBe(2))
    await waitFor(() => expect(document.querySelectorAll('[data-pair-device="sess-new"]').length).toBe(1))
    fireEvent.click(screen.getByRole('button', { name: 'Done' }))
    expect(document.querySelector('[data-pair-success]')).toBeNull()
    expect(screen.getByRole('button', { name: 'Pair a phone' })).toBeTruthy()
  })

  it('a frame for another code leaves this code up; a phone it paired still joins the list', async () => {
    server()
    render(<PhonePairing scope={SCOPE} mode="all" nowMs={NOW} />)
    await showCode()
    frame({ codeId: 'pc_other', state: 'expired', user: 'rosson', deviceName: null, sessionId: null })
    expect(document.querySelector('[data-pair-code]'), 'still showing').toBeTruthy()
    frame({ codeId: 'pc_other2', state: 'paired', user: 'ada', deviceName: 'iPad', sessionId: 'sess-ada' })
    expect(document.querySelector('[data-pair-code]'), 'still showing').toBeTruthy()
    expect(document.querySelector('[data-pair-success]')).toBeNull()
    expect(document.querySelector('[data-pair-device="sess-ada"]')?.textContent).toContain('iPad')
  })

  it('an expiry frame says so and "Make a new code" mints another', async () => {
    const s = server()
    render(<PhonePairing scope={SCOPE} mode="all" nowMs={NOW} />)
    const id = await showCode()
    frame({ codeId: id, state: 'expired', user: 'rosson', deviceName: null, sessionId: null })
    expect(document.querySelector('[data-pair-code]')).toBeNull()
    expect(document.querySelector('[data-pair-expired="expired"]')?.textContent).toContain('expired before a phone used it')
    fireEvent.click(screen.getByRole('button', { name: 'Make a new code' }))
    await waitFor(() => expect(document.querySelector('[data-pair-code-text]')?.textContent).toBe(CODE.code))
    expect(s.mints()).toBe(2)
    expect(document.querySelector('[data-pair-expired]')).toBeNull()
  })

  it('a void frame (a newer code replaced it) offers a new code too', async () => {
    server()
    render(<PhonePairing scope={SCOPE} mode="all" nowMs={NOW} />)
    const id = await showCode()
    frame({ codeId: id, state: 'void', user: 'rosson', deviceName: null, sessionId: null })
    expect(document.querySelector('[data-pair-expired="void"]')?.textContent).toContain('can no longer be used')
  })

  it('a login that must type its password goes back to the form for a new code', async () => {
    handler = (url) => {
      if (url.includes('/cli/auth/devices')) {
        return { status: 200, body: { ok: true, devices: [], pairAs: { user: 'ada', source: 'self', needsPassword: true } } }
      }
      if (url.includes('/cli/auth/device/pair-code')) return { status: 200, body: { ...LIVE_CODE, user: 'ada' } }
      throw new Error(`unexpected ${url}`)
    }
    render(<PhonePairing scope={SCOPE} mode="mine" nowMs={NOW} />)
    fireEvent.change(await screen.findByLabelText('Your password'), { target: { value: 'hunter22' } })
    fireEvent.click(screen.getByRole('button', { name: 'Pair a phone' }))
    await waitFor(() => expect(document.querySelector('[data-pair-code]')).toBeTruthy())
    frame({ codeId: CODE_ID, state: 'expired', user: 'ada', deviceName: null, sessionId: null })
    fireEvent.click(screen.getByRole('button', { name: 'Make a new code' }))
    expect(screen.getByLabelText('Your password')).toBeTruthy()
    expect(calls.filter((c) => c.url.includes('pair-code')).length, 'no mint without the password').toBe(1)
  })

  it('with no frame, the status fallback finds the redeem', async () => {
    let state = 'pending'
    const s = server({
      status: () => ({
        ok: true,
        codeId: `${CODE_ID.slice(0, -1)}1`,
        state,
        user: 'rosson',
        expiresAt: CODE.expiresAt,
        deviceName: state === 'paired' ? 'Android phone' : null,
        sessionId: state === 'paired' ? 'sess-new' : null,
      }),
    })
    render(<PhonePairing scope={SCOPE} mode="all" nowMs={NOW} />)
    const id = await showCode()
    state = 'paired'
    s.paired()
    await waitFor(() => expect(document.querySelector('[data-pair-success-text]')?.textContent).toBe('Paired Android phone as rosson'), {
      timeout: 6000,
    })
    const polled = calls.find((c) => c.url.includes('pair-code/status'))
    expect(polled?.url).toContain(`id=${id}`)
    expect(polled?.init?.method).toBe('GET')
    expect(document.querySelector('[data-pair-device="sess-new"]')).toBeTruthy()
  }, 10_000)
})
