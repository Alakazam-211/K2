/**
 * Hosted web client — force a single same-origin ConnectHost.
 *
 * The browser SPA is served (and the data plane proxied) from the same
 * origin. There is no local Tauri daemon_ws_url path: activeHost must
 * never be `'local'`. Seed the connect-host store from `window.location`
 * (+ any session/local token) before ConnectionGate polls /boot-status.
 *
 * Token auth only for Stage B (cookie auth is Phase 2).
 */

import {
  useConnectHostStore,
  type ConnectHost,
  K2_CONNECT_KEYCHAIN_SERVICE,
} from '@/stores/connect-host'
import { isWebClient } from '@/lib/is-web'
import {
  readWebSessionToken,
  webSecretStorageKey,
} from '@/web/session-token'

/** Stable id for the forced same-origin host (not user-editable). */
export const WEB_ORIGIN_HOST_ID = 'web-same-origin'

function readStoredToken(): string {
  const session = readWebSessionToken()
  if (session.length > 0) return session
  try {
    if (typeof localStorage !== 'undefined') {
      const secret = localStorage.getItem(
        webSecretStorageKey(K2_CONNECT_KEYCHAIN_SERVICE, WEB_ORIGIN_HOST_ID),
      )
      if (secret && secret.length > 0) return secret
    }
  } catch {
    /* ignore */
  }
  return ''
}

/**
 * Build a ConnectHost for the page's own origin.
 * - hostname: location.hostname
 * - port: location.port, or 443/80 by protocol
 * - secure: true when https
 */
export function buildSameOriginHost(token = ''): ConnectHost {
  const loc = typeof window !== 'undefined' ? window.location : null
  const protocol = loc?.protocol ?? 'https:'
  const hostname =
    loc?.hostname && loc.hostname.length > 0 ? loc.hostname : 'localhost'
  const secure = protocol === 'https:'
  let port: number
  if (loc?.port && loc.port.length > 0) {
    port = Number(loc.port)
  } else {
    port = secure ? 443 : 80
  }
  if (!Number.isFinite(port) || port <= 0) {
    port = secure ? 443 : 80
  }

  // Preserve username / remember from a prior seed in the address book.
  const prev = useConnectHostStore
    .getState()
    .hosts.find((h) => h.id === WEB_ORIGIN_HOST_ID)

  return {
    id: WEB_ORIGIN_HOST_ID,
    label: hostname,
    hostname,
    port,
    secure,
    token,
    username: prev?.username,
    remember: prev?.remember ?? true,
    lastConnectedAt: prev?.lastConnectedAt ?? null,
  }
}

/**
 * Seed / force the connect-host store onto the single same-origin remote.
 * Safe to call multiple times (idempotent for a given location + token).
 */
export function forceSameOriginHost(): ConnectHost {
  const token = readStoredToken()
  const host = buildSameOriginHost(token)

  const { hosts } = useConnectHostStore.getState()
  const without = hosts.filter((h) => h.id !== WEB_ORIGIN_HOST_ID)
  const nextHosts: ConnectHost[] = [host, ...without]

  useConnectHostStore.setState({
    activeHost: host,
    hosts: nextHosts,
    connectionStatus: 'connecting',
    recovery: { kind: 'connected' },
    pendingSignIn: null,
  })

  return host
}

/**
 * Boot-time entry: only acts when VITE_WEB. Call from index.tsx before
 * ConnectionGate mounts so the first poll never hits the local path.
 */
export function bootWebHostIfNeeded(): void {
  if (!isWebClient()) return
  forceSameOriginHost()
}
