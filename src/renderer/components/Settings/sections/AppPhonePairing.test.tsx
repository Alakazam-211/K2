// @vitest-environment jsdom

// Settings → Apps → <App> → Phones (prd-app-phone-pairing-v1 §8.6): the card
// draws the daemon's pairing status in words; Turn off and Check post the
// owner routes; Setup sends the phone apps only after a fresh vault confirm
// bound to the App's service id; a refusal shows the daemon's words.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup } from '@testing-library/react'

const h = vi.hoisted(() => ({ daemonCliGet: vi.fn(), daemonCliPost: vi.fn() }))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.daemonCliGet(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.daemonCliPost(...a)),
    daemonCliPostQuery: vi.fn(),
  }
})

import { AppPhonePairing } from './AppPhonePairing'
import { lines, offWords, parseAndroidLines, parsePairing, setupBody } from './app-pairing-api'

const FPR = 'AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89'

const ON = {
  ok: true,
  app: 'svc-1',
  name: 'jobs',
  state: 'on',
  off: null,
  served: true,
  notServed: null,
  staySignedInDays: 30,
  hosts: [{ host: 'jobs.box.k2.dev', pairs: true }],
  ios: ['ABCDE12345.com.jobs.app'],
  android: [{ package: 'com.jobs.app', sha256: [FPR] }],
  hostsChosen: null,
  nativeOrigins: [],
  linkPaths: [],
  store: { ios: 'https://apps.apple.com/app/id1', android: null },
  updatedAt: 1,
  updatedBy: 'owner',
  pairedPhones: 2,
  pairingsLastDay: 1,
  outstanding: 0,
}

const OFF = { ...ON, state: 'off', off: 'not_set_up', ios: [], android: [], store: { ios: null, android: null }, pairedPhones: 0, pairingsLastDay: 0 }

beforeEach(() => {
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
})
afterEach(() => cleanup())

describe('app-pairing-api', () => {
  it('parses the status and the sheet fields', () => {
    const p = parsePairing(ON)
    expect(p.on).toBe(true)
    expect(p.hosts).toEqual([{ host: 'jobs.box.k2.dev', pairs: true }])
    expect(p.storeIos).toBe('https://apps.apple.com/app/id1')
    expect(offWords(parsePairing(OFF))).toBe('not set up yet')
    expect(offWords(parsePairing({ ...OFF, served: false, notServed: 'a raw splice' }))).toBe('a raw splice')
    expect(parsePairing({}).hosts).toEqual([])
    expect(lines(' a\n\nb, c ')).toEqual(['a', 'b', 'c'])
    expect(parseAndroidLines(`com.a ${FPR}, ${FPR}\n\ncom.b`)).toEqual([
      { package: 'com.a', sha256: [FPR, FPR] },
      { package: 'com.b', sha256: [] },
    ])
    expect(setupBody('svc-1', { ios: ['X'], android: [], nativeOrigins: [], linkPaths: [], storeIos: ' ', storeAndroid: '' }, 'tok')).toEqual({
      app: 'svc-1',
      confirmToken: 'tok',
      ios: ['X'],
    })
  })
})

describe('AppPhonePairing', () => {
  it('draws the daemon status, turns pairing off and checks the hosts', async () => {
    h.daemonCliGet.mockResolvedValue(ON)
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'app/pairing/off') return { ...ON, state: 'off', off: 'owner_off', wasOn: true }
      if (route === 'app/pairing/check') return { ok: true, hosts: [{ host: 'jobs.box.k2.dev', aasa: 'ok', assetlinks: 'stale', appleCdn: 'not_served' }] }
      throw new Error(`unexpected POST ${route}`)
    })
    render(<AppPhonePairing app="svc-1" />)
    expect((await screen.findByTestId('phones-state')).textContent).toBe('Phone pairing on')
    expect(h.daemonCliGet).toHaveBeenCalledWith('app/pairing', { app: 'svc-1' })
    expect(screen.getByText('ABCDE12345.com.jobs.app')).toBeTruthy()
    expect(screen.getByText('com.jobs.app')).toBeTruthy()
    expect(screen.getByTestId('phones-counts').textContent).toContain('Paired phones now: 2 · Pairings in the last day: 1')
    fireEvent.click(screen.getByRole('button', { name: 'Check' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('app/pairing/check', { app: 'svc-1' }))
    expect((await screen.findByText(/Android file another file is served there/)).textContent).toContain('iOS file ok')
    fireEvent.click(screen.getByRole('button', { name: 'Turn off' }))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalledWith('app/pairing/off', { app: 'svc-1' }))
    await waitFor(() => expect(screen.getByTestId('phones-state').textContent).toBe('Phone pairing off: turned off'))
  })

  it('sets the phone apps only after a fresh confirm bound to the App', async () => {
    h.daemonCliGet.mockResolvedValue(OFF)
    const posts: Array<[string, unknown]> = []
    h.daemonCliPost.mockImplementation(async (route: string, body: unknown) => {
      posts.push([route, body])
      if (route === 'vault/confirm/start') return { methods: ['vault-password'], expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'confirm-tok', expiresIn: 60 }
      if (route === 'app/pairing/set') return { ...ON, warnings: ['K2 answers the link files on jobs.box.k2.dev'] }
      throw new Error(`unexpected POST ${route}`)
    })
    render(<AppPhonePairing app="svc-1" />)
    expect((await screen.findByTestId('phones-state')).textContent).toBe('Phone pairing off: not set up yet')
    fireEvent.click(screen.getByRole('button', { name: 'Set up…' }))
    fireEvent.change(screen.getByLabelText('iOS app ids'), { target: { value: 'ABCDE12345.com.jobs.app' } })
    fireEvent.change(screen.getByLabelText('Android apps'), { target: { value: `com.jobs.app ${FPR}` } })
    fireEvent.change(screen.getByLabelText('Webview origins'), { target: { value: 'capacitor://localhost' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    const pw = await screen.findByLabelText('Vault password')
    expect(posts.map((p) => p[0])).toEqual(['vault/confirm/start'])
    expect(posts[0][1]).toEqual({ action: 'app-pairing-apps', secretId: 'svc-1' })
    fireEvent.change(pw, { target: { value: 'hunter2-vault' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(posts.map((p) => p[0])).toEqual(['vault/confirm/start', 'vault/confirm/finish', 'app/pairing/set']))
    expect(posts[1][1]).toEqual({ action: 'app-pairing-apps', secretId: 'svc-1', method: 'vault-password', password: 'hunter2-vault' })
    expect(posts[2][1]).toEqual({
      app: 'svc-1',
      confirmToken: 'confirm-tok',
      ios: ['ABCDE12345.com.jobs.app'],
      android: [{ package: 'com.jobs.app', sha256: [FPR] }],
      nativeOrigins: ['capacitor://localhost'],
    })
    expect((await screen.findByTestId('phones-notice')).textContent).toContain('K2 answers the link files')
    expect(screen.getByTestId('phones-state').textContent).toBe('Phone pairing on')
  })

  it('shows the daemon refusal in words', async () => {
    h.daemonCliGet.mockRejectedValue(new Error('pairing status is the owner’s'))
    render(<AppPhonePairing app="svc-1" />)
    expect((await screen.findByTestId('phones-error')).textContent).toContain('pairing status is the owner’s')
  })
})
