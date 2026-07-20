/**
 * Browser-side session helpers for the hosted web client.
 * No store imports — safe for connect-host + boot-host + daemon-cli.
 *
 * Phase 2b (PRD hosted-web-client §2.3): when `VITE_WEB` is true the SPA
 * authenticates via HttpOnly `k2_session` cookie + `X-K2-Client: web`
 * (CSRF) instead of `?token=` on HTTP. The login JSON body still returns
 * a session token which we keep in memory / sessionStorage for:
 *   - pragmatic WebSocket query auth (browsers send cookies on same-origin
 *     WS upgrades too, but in-memory token covers edge proxies / reload
 *     races while the cookie is settling)
 *   - boot restore before the first cookie-authenticated request
 *
 * Desktop (`!VITE_WEB`) never uses these cookies or headers.
 */

import { isWebClient } from '@/lib/is-web'

/** sessionStorage key for the live session bearer after login. */
export const WEB_SESSION_TOKEN_KEY = 'k2_web.session_token'

/** CSRF / client-identity header required by the daemon on cookie-only
 *  mutating `/cli/*` requests (PRD §2.3). */
export const K2_WEB_CLIENT_HEADER = 'X-K2-Client'
export const K2_WEB_CLIENT_VALUE = 'web'

/**
 * localStorage key used by the web `k2_secret_*` shim (and boot restore)
 * for the same-origin host token when "remember" is on.
 */
export function webSecretStorageKey(service: string, account: string): string {
  return `k2_secret:${service}:${account}`
}

/** Persist (or clear) the tab-session token used on reload / WS. */
export function persistWebSessionToken(token: string): void {
  if (!isWebClient()) return
  try {
    if (typeof sessionStorage === 'undefined') return
    if (token.length > 0) sessionStorage.setItem(WEB_SESSION_TOKEN_KEY, token)
    else sessionStorage.removeItem(WEB_SESSION_TOKEN_KEY)
  } catch {
    /* private mode / disabled */
  }
}

export function readWebSessionToken(): string {
  try {
    if (typeof sessionStorage !== 'undefined') {
      const s = sessionStorage.getItem(WEB_SESSION_TOKEN_KEY)
      if (s && s.length > 0) return s
    }
  } catch {
    /* ignore */
  }
  try {
    if (typeof localStorage !== 'undefined') {
      const legacy = localStorage.getItem(WEB_SESSION_TOKEN_KEY)
      if (legacy && legacy.length > 0) return legacy
    }
  } catch {
    /* ignore */
  }
  return ''
}

/**
 * True when the SPA also sends cookie credentials (`credentials: include`
 * + `X-K2-Client: web`). Desktop is false.
 *
 * Note: we still attach `?token=` from memory on web when a token is known.
 * That dual-auth path keeps the portal working against daemons that predate
 * cookie Set-Cookie (e.g. installed 0.40.52) while cookie-capable daemons
 * accept either source. Production edge still prefers cookies once the
 * daemon is current; query token is the transition fallback.
 */
export function useWebCookieAuth(): boolean {
  return isWebClient()
}

/**
 * Merge `X-K2-Client: web` + `credentials: 'include'` onto a RequestInit
 * for same-origin daemon fetches. Desktop returns `init` unchanged so
 * existing call sites stay byte-identical.
 */
export function withDaemonFetch(init: RequestInit = {}): RequestInit {
  if (!isWebClient()) return init
  const headers = new Headers(init.headers as HeadersInit | undefined)
  headers.set(K2_WEB_CLIENT_HEADER, K2_WEB_CLIENT_VALUE)
  return {
    ...init,
    headers,
    credentials: 'include',
  }
}

/**
 * Append `token=<token>` when a bearer is available (desktop always;
 * hosted web as dual-auth fallback for pre-cookie daemons). Cookie is
 * still sent separately via {@link withDaemonFetch}.
 */
export function withCliTokenQuery(url: string, token: string): string {
  if (!token) return url
  // Avoid double-append if a caller already put token in the URL.
  if (/[?&]token=/.test(url)) return url
  const sep = url.includes('?') ? '&' : '?'
  return `${url}${sep}token=${encodeURIComponent(token)}`
}

/**
 * Build `/cli/<route>?…` query params. Includes `token` whenever a
 * non-empty bearer is provided (desktop + web dual-auth).
 */
export function cliSearchParams(
  token: string,
  params?: Record<string, string | number | boolean | undefined | null>,
): URLSearchParams {
  const search = new URLSearchParams()
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) search.set(k, String(v))
    }
  }
  if (token) {
    search.set('token', token)
  }
  return search
}

/**
 * Best-effort `POST /cli/auth/logout` for the hosted web client:
 * clears the HttpOnly `k2_session` cookie server-side and drops the
 * tab-session bearer. Desktop is a no-op (uses keychain forget paths).
 *
 * Always sends CSRF header + credentials include (cookie-only logout is
 * a mutating POST that the daemon CSRF-gates).
 */
export async function logoutWebSession(httpBase?: string): Promise<void> {
  if (!isWebClient()) return
  const base =
    httpBase && httpBase.length > 0
      ? httpBase.replace(/\/$/, '')
      : typeof window !== 'undefined'
        ? window.location.origin
        : ''
  if (!base) {
    persistWebSessionToken('')
    return
  }
  try {
    await fetch(
      `${base}/cli/auth/logout`,
      withDaemonFetch({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      }),
    )
  } catch {
    /* network blip — still clear the tab token below */
  }
  persistWebSessionToken('')
}
