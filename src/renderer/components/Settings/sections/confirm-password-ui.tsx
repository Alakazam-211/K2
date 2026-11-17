// The words and the hidden username field of every person confirm that asks
// for a password (a field report, 0.46.13: an Owner's correct sign-in
// password kept failing at a card's confirm, because "Your K2 Connect
// password" reads as the k2.dev account's password and the browser's
// autofill offered that saved entry instead of this server's).
//
// The daemon says who is confirming where: `vault/confirm/start` (and App
// Approvals' `ownerConfirm`) carry `server` (this server's public name) and
// `login` (the Connect login confirming). The label names both, and a hidden
// `autocomplete="username"` field holding the login sits beside the
// `current-password` one, so a password manager picks this site's entry.

import React from 'react'
import type { ServerScope } from '@/kessel/server-scope'

/** Who is confirming, and on which server, as the daemon said it. */
export interface ConfirmIdentity {
  server?: string | null
  login?: string | null
}

function text(v: unknown): string | null {
  return typeof v === 'string' && v.trim() !== '' ? v.trim() : null
}

/** `server` and `login` from a daemon answer (missing on an older daemon). */
export function readConfirmIdentity(res: unknown): ConfirmIdentity {
  const r = (res && typeof res === 'object' ? res : {}) as Record<string, unknown>
  return { server: text(r.server), login: text(r.login) }
}

/** The password field's label. A login's own password: "Your sign-in
 *  password for <server> (login <name>)"; the vault password: as before.
 *  `fallbackServer` (the window's name for that server) stands in when the
 *  daemon didn't name it. */
export function confirmPasswordLabel(method: string, who: ConfirmIdentity, fallbackServer?: string): string {
  if (method !== 'connect-password' && method !== 'owner-login-password') return 'Your vault password'
  const server = text(who.server) ?? text(fallbackServer) ?? 'this server'
  const login = text(who.login)
  const kind = method === 'owner-login-password' ? 'owner login' : 'login'
  return login ? `Your sign-in password for ${server} (${kind} ${login})` : `Your sign-in password for ${server}`
}

/** The window's own name for a server, when the daemon didn't name itself:
 *  a pinned server's label; the window's server by its host when it is
 *  another machine; nothing for this computer ("This computer" says nothing
 *  about which sign-in it is). */
export function scopeServerName(
  scope: Pick<ServerScope, 'isPrimary' | 'isRemote' | 'hostKey' | 'label'> | null | undefined,
): string | undefined {
  if (!scope) return undefined
  if (!scope.isPrimary) return scope.label
  return scope.isRemote ? scope.hostKey : undefined
}

/** The hidden username beside the password field, for password managers.
 *  Nothing when the login isn't known (the vault password has none). */
export function LoginUsernameField({ method, who, testId }: { method: string; who: ConfirmIdentity; testId?: string }): React.JSX.Element | null {
  const login = text(who.login)
  if ((method !== 'connect-password' && method !== 'owner-login-password') || !login) return null
  return (
    <input
      type="text"
      name="username"
      autoComplete="username"
      value={login}
      readOnly
      hidden
      aria-hidden="true"
      tabIndex={-1}
      data-testid={testId ?? 'confirm-login-username'}
    />
  )
}
