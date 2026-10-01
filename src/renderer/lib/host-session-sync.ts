// Home M2 — one login, every window (MS28, MS62).
//
// Each window holds its saved servers' session tokens in memory. When a
// login lands in one window (by click or automatic), that window has
// already written the token to the keychain (`rememberToken`, when
// "remember" is on). It then emits the Tauri event `k2:host-session`
// `{hostKey, hostId, kind:'signed-in', from}` to every window. The event
// carries no secret. A receiving window maps the host KEY to its own saved
// entry (MS62: the saved list is synced across windows through `storage`
// events, so a server added in window 1 exists in window 2), re-reads the
// keychain by that entry's id and uses the token. It never posts a login of
// its own for that. A server without "remember" has no keychain token, so
// the other windows keep showing Sign in and make no login POST.
//
// Signing out of a server here emits `kind:'signed-out'`: the others drop
// their in-memory token for it (never the window's own server, which finds
// out on its next refused request).

import { homeHostKey, savedHostForKey } from '@/lib/host-key'
import type { ConnectHost } from '@/stores/connect-host'
import type { LoginLanded } from '@/lib/connect-host-hooks'
import type { LoginCoordinator } from '@/lib/host-login-coord'
import type { HostPool } from '@/lib/host-pool'

export const HOST_SESSION_EVENT = 'k2:host-session'

export interface HostSessionEvent {
  hostKey: string
  hostId: string
  kind: 'signed-in' | 'signed-out'
  /** The emitting window, so it can ignore its own echo. */
  from: string
}

export interface HostSessionSyncDeps {
  windowId: string
  emit: (event: string, payload: HostSessionEvent) => Promise<void>
  listen: (event: string, handler: (payload: HostSessionEvent) => void) => Promise<() => void>
  hosts: () => ConnectHost[]
  /** The window's own server key. */
  windowHostKey: () => string
  resolveToken: (hostId: string) => Promise<string | null>
  setHostToken: (hostId: string, token: string) => void
  dropSessionInMemory: (hostId: string) => void
  coord: Pick<LoginCoordinator, 'clearBlock'>
  pool: Pick<HostPool, 'noteSessionRefreshed' | 'noteSignedOut'>
}

export interface HostSessionSync {
  /** Wire to `onLoginLanded`: clear blocks, mark ok, tell the others. */
  loginLanded(event: LoginLanded): void
  /** Tell the others this window signed out of `host`. */
  signedOut(host: ConnectHost): void
  /** Apply another window's event. Resolves when the token is in place. */
  apply(event: HostSessionEvent): Promise<void>
  /** Listen for other windows' events. */
  install(): Promise<() => void>
}

function isEvent(v: unknown): v is HostSessionEvent {
  if (typeof v !== 'object' || v === null) return false
  const e = v as HostSessionEvent
  return (
    typeof e.hostKey === 'string' &&
    typeof e.hostId === 'string' &&
    (e.kind === 'signed-in' || e.kind === 'signed-out') &&
    typeof e.from === 'string'
  )
}

export function createHostSessionSync(deps: HostSessionSyncDeps): HostSessionSync {
  const send = (payload: HostSessionEvent): void => {
    deps.emit(HOST_SESSION_EVENT, payload).catch((err: unknown) => {
      console.warn('[host-session] broadcast failed:', err)
    })
  }

  const apply = async (event: HostSessionEvent): Promise<void> => {
    if (event.from === deps.windowId) return
    // MS62: map by host key, then this window's own id for that server.
    const saved = savedHostForKey(deps.hosts(), event.hostKey)
    if (!saved) return
    if (event.kind === 'signed-out') {
      if (event.hostKey !== deps.windowHostKey()) deps.dropSessionInMemory(saved.id)
      deps.pool.noteSignedOut(event.hostKey)
      return
    }
    // Not remembered: this window does not pick the login up (MS28). The
    // row keeps saying Sign in and this window makes no login POST.
    if (!saved.remember) return
    const token = await deps.resolveToken(saved.id)
    if (!token) return
    const now = savedHostForKey(deps.hosts(), event.hostKey)
    if (!now) return
    if (now.token !== token) deps.setHostToken(now.id, token)
    deps.pool.noteSessionRefreshed(event.hostKey)
  }

  return {
    loginLanded(event) {
      // A restricted (temporary-password) session is not a usable login.
      if (event.mustChangePassword) return
      const hostKey = homeHostKey(event.host)
      deps.coord.clearBlock(hostKey)
      deps.pool.noteSessionRefreshed(hostKey)
      send({ hostKey, hostId: event.host.id, kind: 'signed-in', from: deps.windowId })
    },
    signedOut(host) {
      send({ hostKey: homeHostKey(host), hostId: host.id, kind: 'signed-out', from: deps.windowId })
    },
    apply,
    install() {
      return deps.listen(HOST_SESSION_EVENT, (payload) => {
        if (!isEvent(payload)) return
        void apply(payload)
      })
    },
  }
}
