import { describe, expect, it } from 'vitest'

import { type AuthLike, JWT_COOKIE, SESSION_COOKIE, claimsOf, k2BrowserAuth, k2Cookies, passOf, sessionFrom, storageKey } from '../src/index.ts'

const b64 = (s: string) => Buffer.from(s).toString('base64url')
const PASS = 'k2skn_abc123'
const JWT = `${b64('{"alg":"HS256","typ":"JWT","kid":"k2pass"}')}.${b64(JSON.stringify({ sub: 'u-1', email: 'a@x.test', exp: 4102444800 }))}.${PASS}`
const URL_ = 'https://shop.box.k2.dev'

function store(initial: Record<string, string> = {}) {
  const jar = new Map(Object.entries(initial))
  const sets: { name: string; value: string; options?: Record<string, unknown> }[] = []
  return {
    jar,
    sets,
    getAll: () => [...jar].map(([name, value]) => ({ name, value })),
    set(name: string, value: string, options?: Record<string, unknown>) {
      sets.push({ name, value, options })
      if (value) jar.set(name, value)
      else jar.delete(name)
    },
  }
}

describe('the server cookie adapter', () => {
  it('reads K2 cookies back as the session supabase-js expects', () => {
    const s = store({ [SESSION_COOKIE]: PASS, [JWT_COOKIE]: JWT, other: '1' })
    const all = k2Cookies(s, { supabaseUrl: URL_ }).getAll()
    expect(storageKey(URL_)).toBe('sb-shop-auth-token')
    const sess = JSON.parse(all.find((c) => c.name === 'sb-shop-auth-token')!.value)
    expect(sess.access_token).toBe(JWT)
    expect(sess.refresh_token).toBe(PASS)
    expect(sess.expires_at).toBe(4102444800)
    expect(sess.user.id).toBe('u-1')
    expect(all.some((c) => c.name === 'other')).toBe(true)
  })

  it('never takes a JWT cookie that carries another pass', () => {
    const other = JWT.replace(PASS, 'k2skn_someoneelse')
    expect(sessionFrom(PASS, other).access_token).toBe(PASS)
  })

  it('writes a saved session as two HttpOnly cookies, chunked or base64', () => {
    const s = store()
    const value = 'base64-' + b64(JSON.stringify({ access_token: JWT, refresh_token: PASS, expires_at: 1 }))
    k2Cookies(s, { supabaseUrl: URL_ }).setAll([
      { name: 'sb-shop-auth-token.1', value: value.slice(20) },
      { name: 'sb-shop-auth-token.0', value: value.slice(0, 20) },
      { name: 'theme', value: 'dark' },
    ])
    expect(s.jar.get(SESSION_COOKIE)).toBe(PASS)
    expect(s.jar.get(JWT_COOKIE)).toBe(JWT)
    expect(s.jar.get('theme')).toBe('dark')
    expect([...s.jar.keys()].some((k) => k.startsWith('sb-'))).toBe(false)
    for (const set of s.sets.filter((x) => x.name.startsWith('k2_'))) {
      expect(set.options).toMatchObject({ httpOnly: true, secure: true, sameSite: 'lax', path: '/' })
    }
  })

  it('clears both cookies on sign-out', () => {
    const s = store({ [SESSION_COOKIE]: PASS, [JWT_COOKIE]: JWT })
    k2Cookies(s, { supabaseUrl: URL_ }).setAll([{ name: 'sb-shop-auth-token', value: '', options: { maxAge: 0 } }])
    expect(s.jar.size).toBe(0)
  })

  it('reads the pass and claims out of the JWT shape', () => {
    expect(passOf(JWT)).toBe(PASS)
    expect(passOf(PASS)).toBe(PASS)
    expect(claimsOf(JWT)?.sub).toBe('u-1')
    expect(claimsOf('not.a')).toBeNull()
  })
})

describe('the browser auth calls', () => {
  it('ask the door with the cookie and hold no token', async () => {
    const calls: { url: string; init: RequestInit }[] = []
    const fake = (async (url: string, init: RequestInit) => {
      calls.push({ url, init })
      if (url.endsWith('/user') && init.method === 'GET') return new Response(JSON.stringify({ id: 'u-1', email: 'a@x.test' }), { status: 200 })
      if (url.includes('/logout')) return new Response(null, { status: 204 })
      return new Response(JSON.stringify({ code: 'weak_password', message: 'too short' }), { status: 422 })
    }) as unknown as typeof fetch
    const client = k2BrowserAuth({ auth: {} as AuthLike }, { url: URL_ + '/', key: 'k2pk_svc', fetch: fake })
    const u = await client.auth.getUser()
    expect(u.data.user?.id).toBe('u-1')
    const s = await client.auth.getSession()
    expect(s.data.session?.access_token).toBe('')
    const up = await client.auth.updateUser({ password: 'x' })
    expect(up.error?.code).toBe('weak_password')
    expect((await client.auth.signOut({ scope: 'local' })).error).toBeNull()
    expect(calls.every((c) => c.init.credentials === 'include' && (c.init.headers as Record<string, string>).apikey === 'k2pk_svc')).toBe(true)
    expect(calls.map((c) => c.url)).toContain('https://shop.box.k2.dev/auth/v1/logout?scope=local')
  })
})
