// Settings → Connections "Add / Edit server": the credential-verification
// decision, pulled out of the (large) ConnectionsSection component so the
// W4 entry point is unit-testable without rendering Settings.
//
// PRD connect-login-edge-only D4/W4: this path goes through the SAME
// `loginToHost` as every other sign-in (so the edge login URL rule applies
// by construction), and a `mustChangePassword: true` answer is NOT a
// successful add — the caller must run the shared PasswordRotationStep and
// only then treat the host as signed in.

import {
  duplicateSavedHost,
  duplicateSavedHostMessage,
  loginToHost as storeLoginToHost,
  useConnectHostStore,
  type ConnectHost,
  type LoginResult,
} from '@/stores/connect-host'

export type AddServerLoginOutcome =
  /** Credentials accepted; the fresh session token is committed to the store. */
  | { kind: 'signed-in'; token: string }
  /** Credentials accepted but the daemon requires a new password first.
   *  `host` carries the RESTRICTED session token for the rotation step. A
   *  NEW host's provisional tile is left in the address book so the step can
   *  address it; the caller removes it if the user cancels. */
  | { kind: 'rotate'; host: ConnectHost }
  /** Login failed. A NEW host's provisional tile has been removed. */
  | { kind: 'error'; reason: string }

export interface AddServerLoginDeps {
  /** False when another saved entry already uses this host key (MS61). */
  addHost: (host: ConnectHost) => boolean
  removeHost: (id: string) => void
  loginToHost: (host: ConnectHost, password: string) => Promise<LoginResult>
}

function storeDeps(): AddServerLoginDeps {
  const s = useConnectHostStore.getState()
  return { addHost: s.addHost, removeHost: s.removeHost, loginToHost: storeLoginToHost }
}

/**
 * Add `host` to the address book (loginToHost keys by id, so it must exist
 * first), verify `password` against the daemon, and classify the outcome.
 * On a failed login for a NEW host the provisional tile is removed so a
 * wrong password never sticks in the list.
 */
export async function verifyHostCredentials(
  host: ConnectHost,
  password: string,
  isNew: boolean,
  deps: AddServerLoginDeps = storeDeps(),
): Promise<AddServerLoginOutcome> {
  if (!deps.addHost(host)) {
    const dup = duplicateSavedHost(useConnectHostStore.getState().hosts, host, host.id)
    return {
      kind: 'error',
      reason: dup ? duplicateSavedHostMessage(dup) : 'This server is already saved.',
    }
  }
  const result = await deps.loginToHost(host, password)
  if (!result.ok) {
    if (isNew) deps.removeHost(host.id)
    return { kind: 'error', reason: result.reason }
  }
  if (result.mustChangePassword) {
    return { kind: 'rotate', host: { ...host, token: result.token } }
  }
  return { kind: 'signed-in', token: result.token }
}
