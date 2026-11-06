// The sign-in door's server half (K2 0.46.9): PKCE pairs, the verifier
// cookie, and k2.auth.exchange on the link (App pass) or with the App key,
// projected onto the catalog's AppAuthSession. Fails loudly; no skips.

import { createHash } from 'node:crypto'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { K2Error, beginSignIn, clearSignInCookie, fromEnv, pkce, verifierFromCookieHeader, type K2App } from '../src/index.ts'
import { startFake, type Answer, type FakeGateway, type Seen } from './fake-gateway.ts'

const APP1 = 'k2skn_appPass1111111111111111111111111111111111'
const PASS = 'k2skn_guestAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
const CODE = 'k2lgn_' + 'ab'.repeat(32)

let dir = ''
let gw: FakeGateway
let answer: (s: Seen) => Answer

function client(withPass: boolean): K2App {
  if (!withPass) rmSync(join(dir, 'pass'), { force: true })
  return fromEnv({ K2_GATEWAY_URL: gw.url, K2_PASS_FILE: join(dir, 'pass'), K2_APP_ID: 'svc-jobs-1', K2_SECRETS_DIR: join(dir, 'secrets') })
}

const session = {
  ok: true,
  token: PASS,
  username: 'dana',
  principalId: 'p-1',
  rooms: [],
  roomHandles: [],
  caps: [],
  role: null,
  roomAccess: [],
  app: 'svc-jobs-1',
  mustChangePassword: false,
  authAt: 1_700_000_000,
  via: 'sign-in',
  extra: 'dropped',
}

beforeEach(async () => {
  dir = mkdtempSync(join(tmpdir(), 'k2-sdk-auth-'))
  writeFileSync(join(dir, 'pass'), `${APP1}\n`)
  answer = () => ({ status: 200, body: session })
  gw = await startFake((s) => answer(s))
})

afterEach(async () => {
  await gw.stop()
  rmSync(dir, { recursive: true, force: true })
})

describe('PKCE', () => {
  it('makes an S256 pair: 43 characters each, the challenge the hash of the verifier', () => {
    const p = pkce()
    expect(p.verifier).toMatch(/^[A-Za-z0-9_-]{43}$/)
    expect(p.challenge).toMatch(/^[A-Za-z0-9_-]{43}$/)
    const want = createHash('sha256').update(p.verifier).digest('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
    expect(p.challenge).toBe(want)
    expect(pkce().verifier).not.toBe(p.verifier)
  })

  it('keeps the verifier in an HttpOnly, SameSite=Lax, Secure cookie and reads it back', () => {
    const s = beginSignIn()
    expect(s.setCookie).toBe(`k2_signin=${s.verifier}; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=300`)
    expect(verifierFromCookieHeader(`a=1; k2_signin=${s.verifier}; b=2`)).toBe(s.verifier)
    expect(verifierFromCookieHeader('k2_signin=short')).toBeNull()
    expect(verifierFromCookieHeader(null)).toBeNull()
    expect(clearSignInCookie()).toBe('k2_signin=; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=0')
    expect(() => beginSignIn({ maxAgeSecs: 0 })).toThrowError(/maxAgeSecs/)
    expect(() => beginSignIn({ cookie: 'bad name' })).toThrowError(/cookie names/)
  })

  it('k2.auth.begin and verifierFrom work on a request', () => {
    const k2 = client(true)
    const s = k2.auth.begin()
    expect(k2.auth.verifierFrom({ headers: { cookie: `k2_signin=${s.verifier}` } })).toBe(s.verifier)
    expect(k2.auth.verifierFrom({ headers: new Headers({ cookie: `k2_signin=${s.verifier}` }) })).toBe(s.verifier)
  })
})

describe('k2.auth.exchange', () => {
  it('in a wall: the link route with the App pass, the answer projected (pass, no extras)', async () => {
    const k2 = client(true)
    const { verifier } = pkce()
    const s = await k2.auth.exchange({ code: CODE, verifier })
    expect(gw.seen).toHaveLength(1)
    expect(gw.seen[0]?.method).toBe('POST')
    expect(gw.seen[0]?.url).toBe('/cli/skin/auth/exchange')
    expect(gw.seen[0]?.headers.authorization).toBe(`Bearer ${APP1}`)
    expect(JSON.parse(gw.seen[0]?.body ?? '')).toEqual({ code: CODE, verifier })
    expect(s.pass).toBe(PASS)
    expect(s.authAt).toBe(1_700_000_000)
    expect(s.via).toBe('sign-in')
    expect((s as unknown as Record<string, unknown>).extra).toBeUndefined()
  })

  it('no pass file: the App key on /v1/apps/<id>/auth/exchange', async () => {
    const k2 = client(false)
    const err = (await k2.auth.exchange({ code: CODE, verifier: pkce().verifier }).catch((e: unknown) => e)) as K2Error
    expect(err.code).toBe('no_app_key')
    expect(gw.seen).toHaveLength(0)
    mkdirSync(join(dir, 'secrets'))
    writeFileSync(join(dir, 'secrets', 'K2_APP_KEY'), 'k2sk_appkey\n')
    const s = await k2.auth.exchange({ code: CODE, verifier: pkce().verifier, device: { id: 'phone-1', name: 'Dana' } })
    expect(gw.seen[0]?.url).toBe('/v1/apps/svc-jobs-1/auth/exchange')
    expect(gw.seen[0]?.headers.authorization).toBe('Bearer k2sk_appkey')
    expect(JSON.parse(gw.seen[0]?.body ?? '').device).toEqual({ id: 'phone-1', name: 'Dana' })
    expect(s.pass).toBe(PASS)
  })

  it('a bad code is a K2Error bad_code', async () => {
    const k2 = client(true)
    answer = () => ({ status: 401, body: { ok: false, error: { code: 'bad_code', hint: 'sign in again' } } })
    const err = (await k2.auth.exchange({ code: CODE, verifier: pkce().verifier }).catch((e: unknown) => e)) as K2Error
    expect(err).toBeInstanceOf(K2Error)
    expect(err.code).toBe('bad_code')
    expect(err.status).toBe(401)
  })
})
