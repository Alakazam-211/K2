// @vitest-environment jsdom
// Phone pairing (phone-web-off): the shared panel for Settings → K2
// Companion (`mode="all"`) and the profile modal (`mode="mine"`). It renders
// only what the daemon returns: who a code pairs as, the owner's choices,
// the password need, the code with its QR (from the daemon's link) for its
// 10 minutes, the phones with Revoke, and the daemon's refusal hints.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ host: '127.0.0.1', port: 1234, token: 'owner-tok' })),
  invalidateDaemonWs: vi.fn(),
  daemonHttpBase: () => 'http://127.0.0.1:1234',
}))

import { PhonePairing } from './PhonePairing'
import { parseDevices, parsePairCode, secondsLeft } from './phone-pairing-api'

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
    render(<PhonePairing mode="all" nowMs={() => now} />)
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
  })

  it('lets the owner token pick another login (sent as user)', async () => {
    handler = (url) => {
      if (url.includes('/cli/auth/devices')) {
        return { status: 200, body: { ok: true, devices: [], pairAs: { user: 'rosson', source: 'owner-linked', needsPassword: false, choices: ['ada', 'rosson'] } } }
      }
      return { status: 200, body: { ...CODE, user: 'ada' } }
    }
    render(<PhonePairing mode="all" />)
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
    render(<PhonePairing mode="all" />)
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
    render(<PhonePairing mode="all" />)
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
    render(<PhonePairing mode="mine" />)
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
    render(<PhonePairing mode="mine" />)
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
    render(<PhonePairing mode="mine" />)
    await waitFor(() => expect(document.querySelector('[data-pair-error]')?.textContent).toContain('connection refused'))
  })
})
