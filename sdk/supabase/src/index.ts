// @k2/supabase: the small shim that lets supabase-js run against an App's
// K2 doors (prd-app-supabase-compat-v1 §8.2). Data calls (.from, .rpc,
// .storage) need nothing from it. It does two things, so the session's pass
// never sits where page script can read it:
//
// 1. Server: `k2Cookies(store, {supabaseUrl})` is the `cookies` option of
//    @supabase/ssr's createServerClient. It keeps the session in two
//    HttpOnly cookies K2's door reads (`k2_skin_session`, the pass;
//    `k2_skin_jwt`, the same pass in the JWT shape supabase-js decodes) and
//    hands supabase-js the session it expects under its own cookie name.
// 2. Browser: `k2BrowserAuth(supabase)` points getUser, getSession,
//    updateUser and signOut at the door with the cookie, since supabase-js
//    in the page holds no token to send.
//
// MIT. No dependencies: supabase-js and @supabase/ssr are the App's.

export const SESSION_COOKIE = 'k2_skin_session'
export const JWT_COOKIE = 'k2_skin_jwt'

export interface Cookie {
  name: string
  value: string
}
export interface CookieOptions {
  httpOnly?: boolean
  secure?: boolean
  sameSite?: 'lax' | 'strict' | 'none' | boolean
  path?: string
  maxAge?: number
}
/** What Next.js `cookies()` (and most frameworks) give a server action. */
export interface CookieStore {
  getAll(): Cookie[]
  set(name: string, value: string, options?: CookieOptions): void
}

const KEEP: CookieOptions = { httpOnly: true, secure: true, sameSite: 'lax', path: '/', maxAge: 400 * 24 * 3600 }
const DROP: CookieOptions = { ...KEEP, maxAge: 0 }
const AUTH_COOKIE = /^sb-.+-auth-token(?:\.(\d+))?$/

function b64urlDecode(s: string): string {
  const b = s.replace(/-/g, '+').replace(/_/g, '/')
  return new TextDecoder().decode(Uint8Array.from(atob(b + '='.repeat((4 - (b.length % 4)) % 4)), (c) => c.charCodeAt(0)))
}

/** The claims inside a JWT-shaped token, or null. */
export function claimsOf(token: string): Record<string, unknown> | null {
  const parts = token.split('.')
  if (parts.length !== 3 || !parts[1]) return null
  try {
    return JSON.parse(b64urlDecode(parts[1])) as Record<string, unknown>
  } catch {
    return null
  }
}

/** The pass inside K2's JWT-shaped token (its third part), else the token. */
export function passOf(token: string): string {
  const third = token.split('.')[2]
  return third && (third.startsWith('k2skn_') || third.startsWith('k2vrf_')) ? third : token
}

/** supabase-js's storage key for a project URL: `sb-<first label>-auth-token`. */
export function storageKey(supabaseUrl: string): string {
  return `sb-${new URL(supabaseUrl).hostname.split('.')[0]}-auth-token`
}

/** The session supabase-js reads, rebuilt from K2's two cookies. */
export function sessionFrom(pass: string, jwt: string | undefined): Record<string, unknown> {
  const token = jwt && passOf(jwt) === pass ? jwt : pass
  const c = claimsOf(token) ?? {}
  const exp = typeof c.exp === 'number' ? c.exp : Math.floor(Date.now() / 1000) + 3600
  return {
    access_token: token,
    refresh_token: pass,
    token_type: 'bearer',
    expires_at: exp,
    expires_in: Math.max(0, exp - Math.floor(Date.now() / 1000)),
    user: c.sub ? { id: c.sub, aud: 'authenticated', role: 'authenticated', email: c.email ?? '', app_metadata: {}, user_metadata: {} } : null,
  }
}

/** @supabase/ssr's `cookies` option for createServerClient. */
export function k2Cookies(store: CookieStore, opts: { supabaseUrl: string }) {
  const key = storageKey(opts.supabaseUrl)
  return {
    getAll(): Cookie[] {
      const all = store.getAll().filter((c) => !AUTH_COOKIE.test(c.name))
      const pass = all.find((c) => c.name === SESSION_COOKIE)?.value
      if (!pass) return all
      const jwt = all.find((c) => c.name === JWT_COOKIE)?.value
      return [...all, { name: key, value: JSON.stringify(sessionFrom(pass, jwt)) }]
    },
    setAll(list: { name: string; value: string; options?: CookieOptions }[]): void {
      const chunks: [number, string][] = []
      let cleared = false
      for (const c of list) {
        const m = AUTH_COOKIE.exec(c.name)
        if (!m) {
          store.set(c.name, c.value, c.options)
          continue
        }
        if (!c.value || c.options?.maxAge === 0) cleared = true
        else chunks.push([m[1] === undefined ? 0 : Number(m[1]), c.value])
      }
      if (chunks.length > 0) {
        let raw = chunks.sort((a, b) => a[0] - b[0]).map((x) => x[1]).join('')
        if (raw.startsWith('base64-')) raw = b64urlDecode(raw.slice('base64-'.length))
        const s = JSON.parse(raw) as { access_token?: string }
        if (s.access_token) {
          store.set(SESSION_COOKIE, passOf(s.access_token), KEEP)
          store.set(JWT_COOKIE, s.access_token, KEEP)
          return
        }
      }
      if (cleared) {
        store.set(SESSION_COOKIE, '', DROP)
        store.set(JWT_COOKIE, '', DROP)
      }
    },
  }
}

type Result<T> = Promise<{ data: T; error: { message: string; status: number; code: string } | null }>
type User = Record<string, unknown>
export interface AuthLike {
  getUser(jwt?: string): Result<{ user: User | null }>
  getSession(): Result<{ session: Record<string, unknown> | null }>
  updateUser(attrs: Record<string, unknown>): Result<{ user: User | null }>
  signOut(opts?: { scope?: 'global' | 'local' | 'others' }): Promise<{ error: unknown }>
}

/** Point the browser client's session calls at the door, with the cookie. */
export function k2BrowserAuth<C extends { auth: AuthLike }>(client: C, opts: { url: string; key: string; fetch?: typeof fetch }): C {
  const f = opts.fetch ?? fetch
  const base = opts.url.replace(/\/+$/, '') + '/auth/v1'
  const call = async (method: string, path: string, body?: unknown) => {
    const r = await f(base + path, {
      method,
      credentials: 'include',
      headers: { apikey: opts.key, 'content-type': 'application/json', 'x-supabase-api-version': '2024-01-01' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const text = await r.text()
    const v = text ? (JSON.parse(text) as Record<string, unknown>) : {}
    const error = r.ok ? null : { message: String(v.message ?? v.msg ?? r.statusText), status: r.status, code: String(v.code ?? v.error_code ?? '') }
    return { v, error }
  }
  const auth = client.auth
  auth.getUser = async () => {
    const { v, error } = await call('GET', '/user')
    return { data: { user: error ? null : v }, error }
  }
  auth.getSession = async () => {
    const { v, error } = await call('GET', '/user')
    // A session with no token: the browser never holds one (the cookie does).
    const session = error ? null : { access_token: '', refresh_token: '', token_type: 'bearer', expires_at: 0, expires_in: 0, user: v }
    return { data: { session }, error: error && error.status >= 500 ? error : null }
  }
  auth.updateUser = async (attrs) => {
    const { v, error } = await call('PUT', '/user', attrs)
    return { data: { user: error ? null : v }, error }
  }
  auth.signOut = async (o) => {
    const { error } = await call('POST', `/logout?scope=${o?.scope ?? 'global'}`)
    return { error }
  }
  return client
}
