/**
 * Browser-side session token storage for the hosted web client.
 * No store imports — safe for connect-host + boot-host.
 */

import { isWebClient } from '@/lib/is-web'

/** sessionStorage key for the live session bearer after login. */
export const WEB_SESSION_TOKEN_KEY = 'k2_web.session_token'

/**
 * localStorage key used by the web `k2_secret_*` shim (and boot restore)
 * for the same-origin host token when "remember" is on.
 */
export function webSecretStorageKey(service: string, account: string): string {
  return `k2_secret:${service}:${account}`
}

/** Persist (or clear) the tab-session token used on reload. */
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
