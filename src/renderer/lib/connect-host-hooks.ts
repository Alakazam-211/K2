// Home M2 hooks on the saved-server list (MS28, MS61, MS62).
//
// `stores/connect-host.ts` fires these; the connection pool, the Home store
// and the host-prefixed storage subscribe. They live in their own module,
// free of runtime imports, so subscribers never need connect-host itself at
// module load (and tests that mock connect-host keep working).

import type { ConnectHost } from '@/stores/connect-host'

type SavedHostRekeyListener = (oldKey: string, newKey: string, host: ConnectHost) => void
const savedHostRekeyListeners = new Set<SavedHostRekeyListener>()

/** MS61: a saved server's host key changed (an edit of its hostname, port
 *  or scheme, in this window or another one). Listeners move Home rows,
 *  host-prefixed keys and pool entries from `oldKey` to `newKey`. */
export function onSavedHostRekey(fn: SavedHostRekeyListener): () => void {
  savedHostRekeyListeners.add(fn)
  return () => {
    savedHostRekeyListeners.delete(fn)
  }
}

export function fireSavedHostRekey(oldKey: string, newKey: string, host: ConnectHost): void {
  for (const fn of [...savedHostRekeyListeners]) fn(oldKey, newKey, host)
}

/** What `loginToHost` reports once a login lands. */
export interface LoginLanded {
  host: ConnectHost
  token: string
  mustChangePassword: boolean
}

type LoginLandedListener = (event: LoginLanded) => void
const loginLandedListeners = new Set<LoginLandedListener>()

/** MS28: every successful `POST /cli/auth/login` (automatic or by click)
 *  lands here. The cross-window sync broadcasts it and clears that server's
 *  automatic-login blocks. */
export function onLoginLanded(fn: LoginLandedListener): () => void {
  loginLandedListeners.add(fn)
  return () => {
    loginLandedListeners.delete(fn)
  }
}

export function fireLoginLanded(event: LoginLanded): void {
  for (const fn of [...loginLandedListeners]) fn(event)
}
